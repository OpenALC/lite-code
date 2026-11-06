//! Task 存储、并发队列与 git worktree 管理（docs/TASKS.md §2–§5，P4）。
//!
//! 本模块是 headless 核心：状态按工作区隔离，落在
//! `lc_core::config::data_dir()/tasks/<project-key>.json`；worktree 落在数据
//! 目录（绝不进入项目目录），Tauri 传输层只负责 `Mutex` 包装与 `task:*` 事件
//! 发射。旧版全局 `tasks.json` 不自动迁移，仅记录 warning 提示手动迁移。
//!
//! 语义要点：
//! - 非 git 项目不阻塞任务创建：没有 worktree，任务在串行模式下运行，
//!   预览端口照常分配（预览与 git 无关）；
//! - 并发上限默认 2、硬上限 4，超出时任务进入 `Queued`，由调用方显式
//!   [`TaskStore::promote_queued`] 按创建顺序启动；
//! - 所有 git 调用统一 `git -C <dir>`（不经 shell 字符串拼接）并带超时，
//!   失败信息携带 stderr/stdout 尾部；
//! - 重启时 `Running` 视为中断并转为 `Failed`（worktree 保留供 Review/Discard），
//!   `Queued` 与 `Review` 原样保留。

pub mod diff;
pub mod store;
pub mod worktree;

#[cfg(test)]
mod tests;

pub use diff::TaskDiff;
pub use store::{DEFAULT_MAX_RUNNING, HARD_MAX_RUNNING, TaskStore};
pub use worktree::ApplyOutcome;

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lc_proto::{ErrorBody, ErrorCode, TaskStatus};

/// 普通 git 操作超时（docs/TASKS.md §5）。
pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// merge 单独放宽（大仓库的 ort 策略可能更慢）。
pub(crate) const MERGE_TIMEOUT: Duration = Duration::from_secs(30);

/// 预览端口范围（docs/TASKS.md §4）。
pub const PORT_RANGE_START: u16 = 4300;
pub const PORT_RANGE_END: u16 = 4399;

/// 错误信息里保留的输出尾部长度（字符数）。
const OUTPUT_TAIL_CHARS: usize = 1_500;

/// `IgnoredFiles` 错误 detail 中最多列出的路径数。
const OUTPUT_PATH_LIMIT: usize = 20;

/// task lane 的错误集合。[`TaskError::code`] 映射到契约错误码，
/// 传输层包装成 `ErrorBody` 返回 UI。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    #[error("task not found: {task_id}")]
    NotFound { task_id: String },
    #[error("{task_id} is {status:?}; {action} requires {expected}")]
    InvalidState {
        task_id: String,
        status: TaskStatus,
        action: &'static str,
        expected: &'static str,
    },
    #[error("invalid task input: {message}")]
    InvalidInput { message: String },
    #[error("tasks.json parse failed: {message}")]
    StoreCorrupt { message: String },
    #[error("git executable unavailable: {message}")]
    GitUnavailable { message: String },
    #[error("not a git repository: {root}")]
    NotGit { root: String },
    #[error("git {args} failed (exit {code:?}): {stderr}")]
    GitFailed {
        args: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("git {args} timed out after {timeout_secs}s")]
    GitTimeout { args: String, timeout_secs: u64 },
    #[error("main worktree is dirty: {detail}")]
    DirtyMainWorktree { detail: String },
    #[error("task worktree is dirty: {task_id} at {path}")]
    DirtyWorktree { task_id: String, path: String },
    #[error("task worktree contains ignored files that would be deleted: {task_id}")]
    IgnoredFiles { task_id: String, paths: Vec<String> },
    #[error("task worktree is missing: {task_id} at {path}")]
    WorktreeMissing { task_id: String, path: String },
    #[error("merge conflict while applying {task_id}: {detail}")]
    MergeConflict { task_id: String, detail: String },
    #[error("no preview port available in {}-{}", PORT_RANGE_START, PORT_RANGE_END)]
    PortExhausted,
    #[error("io error at {path}: {message}")]
    Io { path: String, message: String },
    #[error("configuration error: {message}")]
    Config { message: String },
}

impl TaskError {
    /// 对应的契约错误码。lane 私有的细节不进契约，调用方需要时放入
    /// `ErrorBody::detail`（与 fs/process lane 同约定）。
    pub fn code(&self) -> ErrorCode {
        match self {
            TaskError::NotFound { .. } | TaskError::WorktreeMissing { .. } => ErrorCode::NotFound,
            TaskError::GitTimeout { .. } => ErrorCode::Timeout,
            TaskError::InvalidState { .. }
            | TaskError::InvalidInput { .. }
            | TaskError::NotGit { .. }
            | TaskError::DirtyMainWorktree { .. }
            | TaskError::DirtyWorktree { .. }
            | TaskError::IgnoredFiles { .. }
            | TaskError::MergeConflict { .. }
            | TaskError::PortExhausted => ErrorCode::InvalidArgument,
            TaskError::StoreCorrupt { .. }
            | TaskError::GitUnavailable { .. }
            | TaskError::GitFailed { .. }
            | TaskError::Io { .. }
            | TaskError::Config { .. } => ErrorCode::Io,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        let body = ErrorBody::new(self.code(), self.to_string());
        match self {
            TaskError::IgnoredFiles { paths, .. } => {
                let mut detail =
                    String::from("remove or back up these ignored files before apply/discard:\n");
                for path in paths.iter().take(OUTPUT_PATH_LIMIT) {
                    detail.push_str(path);
                    detail.push('\n');
                }
                if paths.len() > OUTPUT_PATH_LIMIT {
                    detail.push_str(&format!(
                        "... and {} more\n",
                        paths.len() - OUTPUT_PATH_LIMIT
                    ));
                }
                body.with_detail(detail.trim_end().to_string())
            }
            _ => body,
        }
    }
}

/// git 命令的原始输出（不做 exit code 检查）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

impl GitOutput {
    pub(crate) fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// 统一的 git 执行入口：`git -C <dir> <args...>`，stdin 关闭、输出走管道、
/// 带硬超时。超时后 kill 直接子进程；不经过 shell，参数原样传递（路径中的
/// 空格/引号无需转义）。
pub(crate) fn run_git(
    dir: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<GitOutput, TaskError> {
    let mut command = Command::new("git");
    crate::process::scrub_secret_env(&mut command);
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C");

    // 探测/任务操作不应在 Windows 上闪控制台窗口（与 runtime lane 同约定）。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            TaskError::GitUnavailable {
                message: e.to_string(),
            }
        } else {
            TaskError::Io {
                path: dir.display().to_string(),
                message: e.to_string(),
            }
        }
    })?;

    let stdout_reader = child.stdout.take().map(|mut stream| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_reader = child.stderr.take().map(|mut stream| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            buf
        })
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // 超时路径不 join：若 git 的钩子进程继承了管道，join 会把
                    // 10s 超时拖成无限等待；读取线程在管道关闭后自行结束。
                    drop(stdout_reader);
                    drop(stderr_reader);
                    return Err(TaskError::GitTimeout {
                        args: args.join(" "),
                        timeout_secs: timeout.as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                drop(stdout_reader);
                drop(stderr_reader);
                return Err(TaskError::Io {
                    path: dir.display().to_string(),
                    message: e.to_string(),
                });
            }
        }
    };

    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);
    Ok(GitOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        code: status.code(),
    })
}

fn join_reader(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// 失败即 `TaskError::GitFailed` 的便捷封装。
pub(crate) fn git_ok(dir: &Path, args: &[&str], timeout: Duration) -> Result<GitOutput, TaskError> {
    let out = run_git(dir, args, timeout)?;
    if out.success() {
        Ok(out)
    } else {
        Err(git_failure(args, &out))
    }
}

pub(crate) fn git_failure(args: &[&str], out: &GitOutput) -> TaskError {
    let detail = match (out.stdout.trim(), out.stderr.trim()) {
        ("", stderr) => stderr.to_string(),
        (stdout, "") => stdout.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    };
    TaskError::GitFailed {
        args: args.join(" "),
        code: out.code,
        stderr: tail(&detail, OUTPUT_TAIL_CHARS),
    }
}

/// 文本尾部（按字符，避免切坏 UTF-8）。
pub(crate) fn tail(text: &str, max_chars: usize) -> String {
    let mut chars: Vec<char> = text.chars().rev().take(max_chars).collect();
    chars.reverse();
    chars.into_iter().collect::<String>().trim().to_string()
}

/// 当前 unix 毫秒。任务 ID 与 `created_at` 共用该时钟。
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 任务 ID：`task-<millis>-<counter>`。
///
/// 与 ULID 同语义——前缀是毫秒时间戳，因此字典序即创建顺序；同毫秒内用
/// 进程级原子计数消歧。不引入 uuid/ulid 依赖（docs/TASKS.md 允许自定义）。
/// 重启后计数器归零，但同一毫秒内跨进程碰撞的概率可忽略，且 [`TaskStore`]
/// 创建时会用现有 ID 集合去重。
pub(crate) fn next_task_id(existing: &HashSet<String>) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let millis = now_millis();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("task-{millis}-{seq}");
        if !existing.contains(&id) {
            return id;
        }
    }
}

/// 标题 → 分支名 slug：仅保留 ASCII 字母数字，其余折叠为单个 `-`。
/// 全中文/空标题回退为 `task`（分支仍合法）。
pub(crate) fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 24 {
            break;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "task".to_string()
    } else {
        trimmed.to_string()
    }
}

/// project-key：根目录名 + 根路径的稳定短 hash（`DefaultHasher`）。
/// 用来隔离不同项目的 worktree 目录：`<data>/worktrees/<project-key>/<task-id>`。
pub(crate) fn project_key(root: &Path) -> String {
    let raw_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let mut name = String::new();
    for c in raw_name.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
        if name.len() >= 32 {
            break;
        }
    }
    let name = name.trim_matches('-');
    let base = if name.is_empty() { "project" } else { name };

    let mut hasher = DefaultHasher::new();
    root.to_string_lossy().to_lowercase().hash(&mut hasher);
    format!("{base}-{:08x}", hasher.finish() as u32)
}

/// 词法绝对化（不解析符号链接）：用于“新 worktree 路径是否落在项目内”的检查。
pub(crate) fn path_inside(child: &Path, parent: &Path) -> bool {
    let child = std::path::absolute(child).unwrap_or_else(|_| child.to_path_buf());
    let parent = std::path::absolute(parent).unwrap_or_else(|_| parent.to_path_buf());

    #[cfg(windows)]
    {
        let child = child.to_string_lossy().replace('/', "\\").to_lowercase();
        let parent = parent
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase();
        child == parent || child.starts_with(&format!("{parent}\\"))
    }
    #[cfg(not(windows))]
    {
        child.starts_with(&parent)
    }
}

/// `git rev-parse --show-toplevel`；非 git / git 缺失一律返回 `None`
/// （创建任务时降级为串行，不阻塞）。
pub(crate) fn git_toplevel(root: &Path) -> Option<PathBuf> {
    let out = run_git(root, &["rev-parse", "--show-toplevel"], GIT_TIMEOUT).ok()?;
    if !out.success() {
        return None;
    }
    let line = out.stdout.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(PathBuf::from(line))
    }
}
