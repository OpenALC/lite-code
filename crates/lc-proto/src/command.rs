//! 命令契约。UI → 核心的全部入口；Tauri 命令只是这里的转发层
//! （docs/README.md：lc-core 不依赖 Tauri）。

use crate::agent::{ApprovalDecision, SessionInfo, SessionMode};
use crate::runtime::{NodeStatus, ShellInfo};

/// 当前契约版本。规则见 docs/VERSIONING.md §8。
pub const CONTRACT_VERSION: &str = "0.3.0";

/// 全部命令名，与 [`CoreCommand`] 的 serde tag 一一对应。
/// 单元测试保证与枚举同步（数量 + 命名）。
pub const CONTRACT_COMMANDS: &[&str] = &[
    "node_detect",
    "node_set_manual",
    "node_download_portable",
    "shell_detect",
    "workspace_root",
    "workspace_open",
    "fs_read_dir",
    "fs_read_file",
    "fs_write_file",
    "session_create",
    "agent_send",
    "agent_cancel",
    "approval_resolve",
    "contract_info",
];

/// 全部事件名，与 [`crate::CoreEvent`] 的 serde `type` 值对应。
pub const CONTRACT_EVENTS: &[&str] = &[
    "node:status",
    "shell:status",
    "fs:tree",
    "fs:entry",
    "task:created",
    "task:status",
    "task:needs_input",
    "task:merged",
    "agent:text_delta",
    "agent:thinking_delta",
    "agent:tool_start",
    "agent:tool_result",
    "agent:approval_required",
    "agent:session_state",
    "agent:usage",
    "agent:done",
    "error",
];

/// 全部错误码名，与 [`crate::ErrorCode`] 的 serde tag 对应。
pub const CONTRACT_ERROR_CODES: &[&str] = &[
    "internal",
    "not_implemented",
    "invalid_argument",
    "timeout",
    "cancelled",
    "path_outside_root",
    "not_found",
    "io",
    "node_unavailable",
    "node_broken",
    "shell_unavailable",
    "contract_mismatch",
];

/// 请求信封。`id` 用于关联响应（会话内单调递增）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct CommandEnvelope {
    pub id: u64,
    pub command: CoreCommand,
}

/// 核心命令集。v0.1 覆盖 runtime 探测、workspace 根与 fs 读写；
/// agent 命令在 P3 以 minor 版本加入。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum CoreCommand {
    /// 探测 Node（惰性触发，串行枚举候选，见 docs/RUNTIME.md）
    NodeDetect,
    /// 手动指定 node.exe 路径；选中后立即校验，通过才写配置
    NodeSetManual { path: String },
    /// 下载便携版（校验 SHASUMS256.txt 后解压，见 docs/RUNTIME.md §5）
    NodeDownloadPortable { version: Option<String> },
    /// 探测 shell（启动时执行，见 docs/RUNTIME.md §8）
    ShellDetect,
    /// 当前工作区根（文件树与 fs 命令的安全边界，docs/PLAN.md R3）
    WorkspaceRoot,
    /// 打开一个目录作为工作区根；路径必须存在且是目录
    WorkspaceOpen { path: String },
    /// 目录按层返回，不整树序列化（docs/ARCHITECTURE.md §5.2）
    FsReadDir { path: String },
    /// 读文件。v0 返回文本；P1 传输层改走原始字节（避免 base64 往返）
    FsReadFile { path: String },
    /// 保存文本文件；目标必须落在 workspace 根内，允许新建
    FsWriteFile { path: String, content: String },
    /// 创建 P3 会话（模式在会话级生效）
    SessionCreate { mode: SessionMode },
    /// 向会话发送用户消息；流式结果走 agent:* 事件
    AgentSend { session_id: String, text: String },
    /// 取消运行中的会话
    AgentCancel { session_id: String },
    /// 处理审批卡
    ApprovalResolve {
        session_id: String,
        approval_id: u64,
        decision: ApprovalDecision,
    },
    /// 契约元信息（lc-cli 与 UI 关于页共用）
    ContractInfo,
}

/// 命令结果。每个命令对应一个 `kind`；错误走统一的 `ErrorBody`（transport 层包装）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum CommandResult {
    NodeStatus {
        status: NodeStatus,
    },
    Shell {
        shell: Option<ShellInfo>,
    },
    Session {
        session: SessionInfo,
    },
    WorkspaceRoot {
        root: Option<String>,
    },
    DirListing {
        path: String,
        entries: Vec<DirEntryDto>,
        generation: u64,
    },
    FileContent {
        path: String,
        text: String,
    },
    Contract {
        meta: ContractMeta,
    },
    Ack,
}

/// 目录项。按层返回，不含子树。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// 契约元信息。lc-cli `contract info` 与 UI“关于”页共用同一来源。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ContractMeta {
    pub name: String,
    pub contract_version: String,
    pub commands: Vec<String>,
    pub events: Vec<String>,
    pub error_codes: Vec<String>,
}

impl ContractMeta {
    pub fn current() -> Self {
        Self {
            name: "lc-proto".into(),
            contract_version: CONTRACT_VERSION.into(),
            commands: CONTRACT_COMMANDS.iter().map(|s| (*s).into()).collect(),
            events: CONTRACT_EVENTS.iter().map(|s| (*s).into()).collect(),
            error_codes: CONTRACT_ERROR_CODES.iter().map(|s| (*s).into()).collect(),
        }
    }
}
