//! [`ProcessSupervisor`] 的真实实现：Windows Job Object / unix 进程组。
//!
//! 平台差异集中在 spawn 与回收两条路径（docs/ARCHITECTURE.md §3）：
//! - Windows：supervisor 持有一个设 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的
//!   Job Object，spawn 后立即 `AssignProcessToJobObject`；supervisor drop =
//!   关句柄 = 全树终止（win32job 的 `Job` drop 关闭句柄），kill_all = 一次
//!   `TerminateJobObject`（win32job 2.0.3 未封装，kernel32 FFI 直连）；
//! - unix：spawn 前 `setsid`（子进程自成会话与进程组，pgid == pid）并设
//!   `PR_SET_PDEATHSIG = SIGKILL`（父进程意外退出时内核直接杀，ARCHITECTURE
//!   §3 的 PDEATHSIG），回收时对 `-pid` 进程组发 SIGKILL。libc 符号 FFI
//!   直连——依赖白名单没有 libc crate，std 自身已链接 libc。

use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use super::{ProcessError, ProcessSupervisor};

/// 被跟踪子进程的共享状态：supervisor 注册表与调用方句柄指向同一份。
#[derive(Debug)]
struct ChildInner {
    pid: u32,
    child: Mutex<Child>,
}

/// [`spawn_tracked`](ProcessSupervisor::spawn_tracked) 返回的观察/控制句柄。
///
/// 克隆即共享同一子进程（Arc）。句柄不承担回收责任：drop 句柄不影响跟踪，
/// 整组回收统一在 supervisor 的 kill_all / drop（docs/PLAN.md R5）。
#[derive(Debug, Clone)]
pub struct TrackedChild {
    inner: Arc<ChildInner>,
}

impl TrackedChild {
    /// 子进程 pid。
    pub fn id(&self) -> u32 {
        self.inner.pid
    }

    /// 非阻塞收集 exit 状态：已退出返回 `Some(status)`。
    pub fn try_wait(&self) -> Result<Option<ExitStatus>, ProcessError> {
        let mut child = self.inner.child.lock().unwrap();
        child.try_wait().map_err(|e| ProcessError::Wait {
            pid: self.inner.pid,
            message: e.to_string(),
        })
    }

    /// 阻塞等待退出并收集 exit 状态。持锁等待——同一子进程的并发 wait 串行，
    /// 收割只发生一次。
    pub fn wait(&self) -> Result<ExitStatus, ProcessError> {
        let mut child = self.inner.child.lock().unwrap();
        child.wait().map_err(|e| ProcessError::Wait {
            pid: self.inner.pid,
            message: e.to_string(),
        })
    }

    /// 终止单个子进程。对已退出的进程幂等（返回 Ok）。
    ///
    /// - unix：对子进程所在**进程组**发 SIGKILL，孙进程一并终止；
    /// - Windows：单 Job 设计下只能终止直接子进程（std `Child::kill`），
    ///   孙进程的整组回收走 kill_all / supervisor drop（见模块文档已知限制）。
    pub fn kill(&self) -> Result<(), ProcessError> {
        let mut child = self.inner.child.lock().unwrap();
        let exited = child.try_wait().map_err(|e| ProcessError::Wait {
            pid: self.inner.pid,
            message: e.to_string(),
        })?;
        if exited.is_some() {
            return Ok(());
        }
        #[cfg(windows)]
        let result = child.kill().map_err(|e| e.to_string());
        #[cfg(unix)]
        let result = kill_group(self.inner.pid).map_err(|e| e.to_string());
        result.map_err(|message| ProcessError::Kill {
            pid: self.inner.pid,
            message,
        })
    }
}

/// 真实实现（lc-cli / Tauri 壳共用）。
pub struct RealProcessSupervisor {
    children: Vec<TrackedChild>,
    #[cfg(windows)]
    job: win32job::Job,
}

impl RealProcessSupervisor {
    /// 创建 supervisor。Windows 上建 Job Object（`KILL_ON_JOB_CLOSE`：句柄
    /// 关闭即终止全组——supervisor drop 的回收机制）。
    pub fn new() -> Result<Self, ProcessError> {
        new_impl()
    }
}

/// 平台构造分支：整函数级 cfg，避免语句级 cfg 组合出 clippy 边角。
#[cfg(windows)]
fn new_impl() -> Result<RealProcessSupervisor, ProcessError> {
    let mut info = win32job::ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&info).map_err(|e| ProcessError::Group {
        pid: None,
        message: format!("create job object: {e}"),
    })?;
    Ok(RealProcessSupervisor {
        children: Vec::new(),
        job,
    })
}

#[cfg(unix)]
fn new_impl() -> Result<RealProcessSupervisor, ProcessError> {
    Ok(RealProcessSupervisor {
        children: Vec::new(),
    })
}

impl ProcessSupervisor for RealProcessSupervisor {
    fn spawn_tracked(
        &mut self,
        program: &Path,
        args: &[&str],
    ) -> Result<TrackedChild, ProcessError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            // 全 null：管道不读会撑爆阻塞长驻子进程；流式 IO 属后续 PTY lane
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(win::CREATE_NO_WINDOW);
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // setsid：子进程自成会话与进程组（pgid == pid），回收按组发信号；
            // PR_SET_PDEATHSIG：父进程退出时内核直接 SIGKILL。
            // 闭包失败经 std 由 spawn 返回，不会留下"已 spawn 未设防"的进程。
            unsafe {
                cmd.pre_exec(|| {
                    if unix_ffi::prctl(unix_ffi::PR_SET_PDEATHSIG, unix_ffi::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if unix_ffi::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let child = spawn_command(program, cmd)?;
        let pid = child.id();

        // spawn 后第一件事入组；失败就地回收，绝不留孤儿（R5）
        #[cfg(windows)]
        let child = assign_to_job(&self.job, child)?;

        let tracked = TrackedChild {
            inner: Arc::new(ChildInner {
                pid,
                child: Mutex::new(child),
            }),
        };
        self.children.push(tracked.clone());
        Ok(tracked)
    }

    fn kill_all(&mut self) -> Result<(), ProcessError> {
        #[cfg(windows)]
        {
            if !self.children.is_empty() {
                // 一次 TerminateJobObject 终止全组（含孙进程）；Job 对象本身
                // 仍可用，之后 spawn 的子进程照常入组
                terminate_job_object(&self.job)
                    .map_err(|message| ProcessError::Group { pid: None, message })?;
                // 终止已发起，等全组退出并收割（此后必然退出，毫秒级）
                for t in std::mem::take(&mut self.children) {
                    let _ = t.inner.child.lock().unwrap().wait();
                }
            }
        }
        #[cfg(unix)]
        {
            let mut first_err = None;
            for t in std::mem::take(&mut self.children) {
                let killed = kill_group(t.inner.pid);
                // unix 必须收割防僵尸；SIGKILL 后 wait 毫秒级返回
                let _ = t.inner.child.lock().unwrap().wait();
                if let Err(e) = killed {
                    first_err.get_or_insert(ProcessError::Kill {
                        pid: t.inner.pid,
                        message: e.to_string(),
                    });
                }
            }
            if let Some(e) = first_err {
                return Err(e);
            }
        }
        Ok(())
    }

    fn collect_exited(&mut self) -> Result<Vec<(u32, ExitStatus)>, ProcessError> {
        let mut exited = Vec::new();
        for t in &self.children {
            if let Some(status) = t.try_wait()? {
                exited.push((t.inner.pid, status));
            }
        }
        if !exited.is_empty() {
            let done: Vec<u32> = exited.iter().map(|(pid, _)| *pid).collect();
            self.children.retain(|t| !done.contains(&t.inner.pid));
        }
        Ok(exited)
    }

    fn tracked_pids(&self) -> Vec<u32> {
        self.children.iter().map(|t| t.inner.pid).collect()
    }
}

impl Drop for RealProcessSupervisor {
    fn drop(&mut self) {
        // R5：drop 即整组回收，不留孤儿。
        // - Windows：`self.job` 在此 drop → 句柄关闭 → KILL_ON_JOB_CLOSE
        //   终止全组，无需显式 terminate（tests.rs 的 drop 用例验证该路径；
        //   实测该路径退出码为 0，不同于显式 TerminateJobObject 的指定码）；
        // - unix：没有 Job Object，逐组 SIGKILL 并收割。
        #[cfg(unix)]
        for t in &self.children {
            let _ = kill_group(t.inner.pid);
            let _ = t.inner.child.lock().unwrap().wait();
        }
    }
}

/// spawn 并把 io 错误按 kind 分类：程序不存在单列（契约有 `NotFound` 码）。
fn spawn_command(program: &Path, mut cmd: Command) -> Result<Child, ProcessError> {
    cmd.spawn().map_err(|e| {
        let program = program.to_string_lossy().into_owned();
        if e.kind() == std::io::ErrorKind::NotFound {
            ProcessError::NotFound { program }
        } else {
            ProcessError::Spawn {
                program,
                message: e.to_string(),
            }
        }
    })
}

/// 把刚 spawn 的子进程纳入 Job Object；失败就地回收（R5 不留孤儿）。
#[cfg(windows)]
fn assign_to_job(job: &win32job::Job, mut child: Child) -> Result<Child, ProcessError> {
    use std::os::windows::io::AsRawHandle;

    let pid = child.id();
    if let Err(e) = job.assign_process(child.as_raw_handle() as isize) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ProcessError::Group {
            pid: Some(pid),
            message: format!("assign pid {pid} to job object: {e}"),
        });
    }
    Ok(child)
}

/// 一次 `TerminateJobObject` 终止全组；失败以消息返回（调用方包成
/// [`ProcessError::Group`]）。
#[cfg(windows)]
fn terminate_job_object(job: &win32job::Job) -> Result<(), String> {
    use std::ffi::c_void;

    // exit code 1：被整组回收的进程不该表现为"正常退出"
    let rc = unsafe { win::TerminateJobObject(job.handle() as *mut c_void, 1) };
    if rc == 0 {
        return Err(format!(
            "TerminateJobObject: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// 对子进程所在进程组发 SIGKILL；组不存在（已退出）视为成功。
#[cfg(unix)]
fn kill_group(pid: u32) -> std::io::Result<()> {
    // 子进程 setsid 后 pgid == pid，对 -pid 发信号即整组
    let rc = unsafe { unix_ffi::kill(-(pid as i32), unix_ffi::SIGKILL) };
    if rc == 0 {
        return Ok(());
    }
    let e = std::io::Error::last_os_error();
    if e.raw_os_error() == Some(unix_ffi::ESRCH) {
        return Ok(());
    }
    Err(e)
}

/// Windows 平台常量与 FFI。
#[cfg(windows)]
mod win {
    // 与 runtime/runner.rs 同规则：探测/后台子进程不允许闪黑框
    // （docs/RUNTIME.md §4/§6；例外是用户可见的安装流程，走可见 PTY，不归本模块）
    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // win32job 2.0.3 未封装 TerminateJobObject（kill_all 需要），kernel32 直连
    unsafe extern "system" {
        pub fn TerminateJobObject(hjob: *mut std::ffi::c_void, uexitcode: u32) -> i32;
    }
}

/// unix 平台常量与 FFI（libc 符号直连，std 已链接 libc）。
/// `PR_SET_PDEATHSIG` / `SIGKILL` / `ESRCH` 在 Linux 与 macOS 取值相同。
#[cfg(unix)]
mod unix_ffi {
    use std::ffi::c_int;

    unsafe extern "C" {
        pub fn prctl(option: c_int, ...) -> c_int;
        pub fn setsid() -> c_int;
        pub fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    pub const PR_SET_PDEATHSIG: c_int = 1; // linux/prctl.h
    pub const SIGKILL: c_int = 9; // signal.h
    pub const ESRCH: c_int = 3; // errno.h
}
