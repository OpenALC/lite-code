//! 进程生命周期（P1 Lifecycle lane：docs/ARCHITECTURE.md §3 `ProcessSupervisor`）。
//!
//! 职责：被跟踪地 spawn 子进程——终端 / Agent 会话 / git 调用统一入口——
//! drop 与 [`ProcessSupervisor::kill_all`] 时**整组回收**（docs/PLAN.md R5：不留孤儿）：
//! - Windows：所有子进程挂进**一个 Job Object**（`win32job`），设
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`——supervisor drop 即关句柄，全树终止；
//! - unix：子进程 `setsid` 成独立会话与进程组（pgid == pid）并设
//!   `PR_SET_PDEATHSIG`（父死即杀），回收时对进程组发 SIGKILL。
//!
//! 设计约束：
//! - 回收责任统一在 supervisor：[`TrackedChild`] 只是观察/控制句柄（Arc 共享
//!   内部状态），drop 句柄不影响跟踪，整组回收走 kill_all / supervisor drop；
//! - **入组先于一切**：spawn 后第一件事纳入整组机制，任何一步失败就地 kill
//!   已启动的子进程，绝不留孤儿；
//! - 子进程 stdin/stdout/stderr 全部 `Stdio::null()`：管道不读会撑爆阻塞
//!   长驻子进程，而流式 IO 是后续 PTY lane 的事（docs/AGENT.md §9：本 trait
//!   只需"支持长驻会话"）；
//! - 错误映射 [`lc_proto::ErrorCode`]，风格对齐 fs lane 的 `FsError`。
//!
//! 测试策略：生命周期语义只能靠真实进程验证（docs/PLAN.md §5：R4/R5 这类
//! 问题只能靠真实运行发现），全部用例 spawn 真实子进程，不依赖网络。
//!
//! 已知限制（v1 接受，见 supervisor.rs 内注释）：
//! - spawn → 入 Job 之间子进程可能先产出孙进程（std 无 CREATE_SUSPENDED
//!   钩子），孙进程在该窗口内暂不受 Job 约束；
//! - Windows 单 Job 设计下，单句柄 [`TrackedChild::kill`] 只能终止直接子进程，
//!   整组回收统一走 kill_all / supervisor drop。

pub mod supervisor;

#[cfg(test)]
mod tests;

pub use supervisor::{RealProcessSupervisor, TrackedChild};

use std::path::Path;
use std::process::ExitStatus;

use lc_proto::{ErrorBody, ErrorCode};

/// process lane 的错误集合。[`ProcessError::code`] 映射到契约错误码
/// （crates/lc-proto/src/error.rs），传输层包装成 `ErrorBody` 返回 UI。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProcessError {
    /// spawn 失败且程序不存在（`io::ErrorKind::NotFound`，契约有对应码）。
    #[error("program not found: {program}")]
    NotFound { program: String },
    /// spawn 失败（权限 / 可执行格式等；unix 上 setsid / PDEATHSIG 失败由
    /// std 并入 spawn 错误返回，也归此类）。
    #[error("spawn failed for {program}: {message}")]
    Spawn { program: String, message: String },
    /// 平台整组载体失败（Windows：创建 Job / 入 Job / TerminateJobObject）。
    /// `pid` 为 `None` 表示载体本身（Job Object）没建起来；入组失败时
    /// 子进程已被就地回收，不会泄漏。
    #[error("process group failure: {message}")]
    Group { pid: Option<u32>, message: String },
    #[error("kill failed for pid {pid}: {message}")]
    Kill { pid: u32, message: String },
    #[error("wait failed for pid {pid}: {message}")]
    Wait { pid: u32, message: String },
}

impl ProcessError {
    /// 对应的契约错误码。lane 私有的细节不进契约，调用方需要时放入
    /// `ErrorBody::detail`（与 fs lane 同约定）。
    pub fn code(&self) -> ErrorCode {
        match self {
            ProcessError::NotFound { .. } => ErrorCode::NotFound,
            ProcessError::Spawn { .. } => ErrorCode::Io,
            ProcessError::Group { .. } => ErrorCode::Internal,
            ProcessError::Kill { .. } | ProcessError::Wait { .. } => ErrorCode::Io,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        ErrorBody::new(self.code(), self.to_string())
    }
}

/// 子进程生命周期抽象（docs/ARCHITECTURE.md §3 关键抽象表）。
///
/// Windows 靠 Job Object、unix 靠进程组 + PDEATHSIG 落实"整组回收"；
/// HarmonyOS（NAPI 持有）留待 P6 可行性验证。
pub trait ProcessSupervisor: Send + Sync {
    /// 被跟踪地 spawn `<program> args...`：子进程立即纳入整组回收范围，
    /// 返回观察/控制句柄（supervisor 注册表内同时持有一份）。
    fn spawn_tracked(
        &mut self,
        program: &Path,
        args: &[&str],
    ) -> Result<TrackedChild, ProcessError>;

    /// 立即终止并回收全部被跟踪子进程（整组，含孙进程）；成功后注册表清空，
    /// 本 supervisor 可继续使用。尽力而为：即使部分失败也终止其余进程，
    /// 再返回首个错误。
    fn kill_all(&mut self) -> Result<(), ProcessError>;

    /// 收集已退出的子进程的 exit 状态：返回 `(pid, status)` 并从跟踪列表
    /// 移除（收割）。未退出的保持跟踪。
    fn collect_exited(&mut self) -> Result<Vec<(u32, ExitStatus)>, ProcessError>;

    /// 当前跟踪的 pid 列表（docs/PLAN.md R5："诊断"面板列出所有子进程 PID）。
    fn tracked_pids(&self) -> Vec<u32>;
}
