//! Git CLI 执行层：`git -C <root> ...`、硬超时与错误分类。
//!
//! 与 task lane 的 worktree 调用保持独立：本 lane 只服务主工作区的
//! status/diff/stage/commit/log，不共享 task 的 `run_git`，避免 P5 Git 面板
//! 的语义被 worktree 生命周期绑死（docs/ARCHITECTURE.md §5.3）。

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::GitError;

/// 普通 Git 操作超时（status/diff/stage/log）。
pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// commit 可能触发钩子，单独放宽。
pub(crate) const COMMIT_TIMEOUT: Duration = Duration::from_secs(30);

/// 错误信息里保留的输出尾部长度（字符数）。
const OUTPUT_TAIL_CHARS: usize = 1_500;

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

/// 统一入口：参数走数组、不经过 shell；stdin 关闭、输出走管道、带硬超时。
/// `git` 缺失或目录不可用时分别映射为 `GitUnavailable` / `GitError::Io`。
pub(crate) fn run_git(
    root: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<GitOutput, GitError> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C");

    // Windows 上不闪控制台窗口（与 runtime / task lane 同约定）。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            GitError::Unavailable {
                message: format!("git executable not found: {error}"),
            }
        } else {
            GitError::Io {
                path: root.display().to_string(),
                message: error.to_string(),
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
                    // 超时路径不 join：钩子进程可能继承管道，join 会把超时拖成
                    // 无限等待；读取线程在管道关闭后自行结束。
                    drop(stdout_reader);
                    drop(stderr_reader);
                    return Err(GitError::Timeout {
                        args: args.join(" "),
                        timeout_secs: timeout.as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                drop(stdout_reader);
                drop(stderr_reader);
                return Err(GitError::Io {
                    path: root.display().to_string(),
                    message: error.to_string(),
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

/// stdout/stderr 合并后的尾部，供错误信息使用。
pub(crate) fn output_detail(out: &GitOutput) -> String {
    let detail = match (out.stdout.trim(), out.stderr.trim()) {
        ("", stderr) => stderr.to_string(),
        (stdout, "") => stdout.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    };
    tail(&detail, OUTPUT_TAIL_CHARS)
}

/// 非零退出码 → `GitRejected`（stderr 尾部进入 detail）。
pub(crate) fn rejected(operation: &str, out: &GitOutput) -> GitError {
    GitError::Rejected {
        operation: operation.to_string(),
        detail: output_detail(out),
    }
}

/// 文本尾部（按字符，避免切坏 UTF-8）。
pub(crate) fn tail(text: &str, max_chars: usize) -> String {
    let mut chars: Vec<char> = text.chars().rev().take(max_chars).collect();
    chars.reverse();
    chars.into_iter().collect::<String>().trim().to_string()
}
