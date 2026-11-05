//! `shell` 工具执行（docs/AGENT.md §4/§5.3、docs/ARCHITECTURE.md §5.4）：
//! 非 PTY、一次性命令，shell 固定为 Windows PowerShell 7（`pwsh`）/ Linux bash。
//!
//! - 命令构造、超时解析、输出裁剪、错误映射都是可单测的纯函数；
//! - 真实执行走 [`RealProcessSupervisor::spawn_captured`]，每次执行一个独立
//!   supervisor：超时 `kill_all`（Windows Job Object / unix 进程组）后 drop，
//!   整棵进程树（含孙进程）被回收，管道随之关闭，不留下孤儿；
//! - stdout/stderr 各自截断 [`MAX_STREAM_BYTES`]；非零退出码是正常结果，
//!   只有 spawn 失败、shell 缺失和超时才是错误；
//! - shell 缺失返回 [`ToolError::ShellUnavailable`]，不降级到 cmd.exe / sh。

use std::io::{self, Read};
use std::path::Path;
use std::process::ExitStatus;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use lc_proto::{ShellInfo, ShellKind};
use serde_json::Value;

use crate::process::{ProcessError, ProcessSupervisor, RealProcessSupervisor};
use crate::runtime::detect_shell_real;

use super::exec::{ToolError, ToolOutput};

/// `timeout_ms` 缺省值（docs/AGENT.md §4：命令执行默认 30s）。
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// `timeout_ms` 上限 120s；超出按上限截断。
pub const MAX_TIMEOUT_MS: u64 = 120_000;
/// stdout / stderr 各自的最大保留字节数。
pub const MAX_STREAM_BYTES: usize = 128 * 1024;

const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 解析后的 shell 调用：程序绝对路径 + 参数数组（不经二次拼接，避免转义）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    program: String,
    args: Vec<String>,
}

/// 单路输出（已按上限裁剪）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamText {
    text: String,
    truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandOutput {
    exit_code: Option<i32>,
    stdout: StreamText,
    stderr: StreamText,
}

/// `shell` 工具入口：解析 `command` / `timeout_ms`，探测 shell 后执行。
pub fn execute_shell(cwd: &Path, args: &Value) -> Result<ToolOutput, ToolError> {
    let command = command_arg(args)?;
    let timeout_ms = parse_timeout(args)?;
    let shell = resolve_shell()?;
    run_command(&shell, cwd, command, timeout_ms)
}

fn command_arg(args: &Value) -> Result<&str, ToolError> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArgs("缺少字符串参数 command".into()))?;
    if command.trim().is_empty() {
        return Err(ToolError::InvalidArgs("command 不能为空".into()));
    }
    Ok(command)
}

/// `timeout_ms` 缺省 30s；0 / 非整数报错；超过 120s 截断到上限。
fn parse_timeout(args: &Value) -> Result<u64, ToolError> {
    match args.get("timeout_ms") {
        None | Some(Value::Null) => Ok(DEFAULT_TIMEOUT_MS),
        Some(value) => {
            let millis = value.as_u64().ok_or_else(|| {
                ToolError::InvalidArgs("timeout_ms 必须是非负整数（毫秒）".into())
            })?;
            if millis == 0 {
                return Err(ToolError::InvalidArgs("timeout_ms 必须大于 0".into()));
            }
            Ok(millis.min(MAX_TIMEOUT_MS))
        }
    }
}

/// 探测并缓存可用 shell。只缓存成功结果：用户中途安装后无需重启即可恢复。
fn resolve_shell() -> Result<ShellInfo, ToolError> {
    static DETECTED: OnceLock<ShellInfo> = OnceLock::new();
    if let Some(shell) = DETECTED.get() {
        return Ok(shell.clone());
    }
    match detect_shell_real() {
        Some(shell) => {
            let _ = DETECTED.set(shell.clone());
            Ok(shell)
        }
        None => Err(ToolError::ShellUnavailable(missing_shell_message())),
    }
}

fn missing_shell_message() -> String {
    if cfg!(windows) {
        "未找到 PowerShell 7（pwsh）。安装：winget install --id Microsoft.PowerShell -e \
         --source winget；安装后重试（不自动降级到 cmd.exe 或 5.1）"
            .into()
    } else {
        "未找到 bash。请安装 bash（例如 apt install bash）；未自动降级到 sh".into()
    }
}

/// shell 方言固定：pwsh 走 `-NoProfile -NonInteractive -ExecutionPolicy Bypass`，
/// bash 走 `--noprofile --norc -c`（docs/ARCHITECTURE.md §5.4.3）。
fn build_invocation(shell: &ShellInfo, command: &str) -> Invocation {
    match shell.kind {
        ShellKind::Pwsh => Invocation {
            program: shell.path.clone(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                pwsh_script(command),
            ],
        },
        ShellKind::Bash => Invocation {
            program: shell.path.clone(),
            args: vec![
                "--noprofile".into(),
                "--norc".into(),
                "-c".into(),
                command.into(),
            ],
        },
    }
}

/// PowerShell 包装（docs/ARCHITECTURE.md §5.4.4）：强制 UTF-8 输出，并把
/// `$LASTEXITCODE` 收敛为进程退出码；纯 PowerShell 异常统一 exit 1。
fn pwsh_script(command: &str) -> String {
    format!(
        "[Console]::OutputEncoding = [Text.UTF8Encoding]::new()\n\
         $OutputEncoding = [Text.UTF8Encoding]::new()\n\
         try {{\n\
         {command}\n\
         if ($null -ne $LASTEXITCODE) {{ exit $LASTEXITCODE }} else {{ exit 0 }}\n\
         }} catch {{\n\
         [Console]::Error.WriteLine($_.ToString())\n\
         exit 1\n\
         }}\n"
    )
}

fn map_process_error(error: ProcessError) -> ToolError {
    match error {
        ProcessError::NotFound { program } => {
            ToolError::ShellUnavailable(format!("shell 不存在：{program}"))
        }
        ProcessError::Spawn { program, message } => {
            ToolError::Io(format!("启动 {program} 失败：{message}"))
        }
        other => ToolError::Io(other.to_string()),
    }
}

fn run_command(
    shell: &ShellInfo,
    cwd: &Path,
    command: &str,
    timeout_ms: u64,
) -> Result<ToolOutput, ToolError> {
    let invocation = build_invocation(shell, command);
    let args: Vec<&str> = invocation.args.iter().map(String::as_str).collect();

    // 每条命令独立 supervisor：kill_all / drop 只影响本条命令的进程树。
    let mut supervisor = RealProcessSupervisor::new().map_err(map_process_error)?;
    let mut child = supervisor
        .spawn_captured(Path::new(&invocation.program), &args, Some(cwd))
        .map_err(map_process_error)?;

    let stdout = child
        .take_stdout()
        .ok_or_else(|| ToolError::Io("未取得 shell stdout 管道".into()))?;
    let stderr = child
        .take_stderr()
        .ok_or_else(|| ToolError::Io("未取得 shell stderr 管道".into()))?;
    let stdout_handle = thread::spawn(move || read_capped(stdout, MAX_STREAM_BYTES));
    let stderr_handle = thread::spawn(move || read_capped(stderr, MAX_STREAM_BYTES));

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let status: Option<ExitStatus> = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                let _ = supervisor.kill_all();
                drop(supervisor);
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return Err(map_process_error(error));
            }
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(POLL_INTERVAL);
    };

    if let Some(status) = status {
        // 正常退出也要 drop supervisor：命令若留下后台孙进程，整组一并回收，
        // 继承的管道才能关闭，读取线程不会永久阻塞。
        drop(supervisor);
        let stdout = join_stream(stdout_handle)?;
        let stderr = join_stream(stderr_handle)?;
        return Ok(format_output(CommandOutput {
            exit_code: status.code(),
            stdout,
            stderr,
        }));
    }

    // 超时：先整组终止，再 drop 兜底（kill_all 失败也不会留下阻塞的 wait）。
    let kill_result = supervisor.kill_all();
    drop(supervisor);
    let _status = child.wait().map_err(map_process_error)?;
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    match kill_result {
        Ok(()) => Err(ToolError::Timeout { timeout_ms }),
        Err(error) => Err(ToolError::Io(format!("命令超时，终止进程树失败：{error}"))),
    }
}

/// 读取一路输出，最多保留 `limit` 字节；超出部分继续读空以防管道阻塞。
fn read_capped(mut reader: impl Read, limit: usize) -> io::Result<StreamText> {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut buf = [0u8; 8192];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(kept.len());
        if remaining == 0 {
            truncated = true;
            continue;
        }
        let take = remaining.min(read);
        kept.extend_from_slice(&buf[..take]);
        if take < read {
            truncated = true;
        }
    }
    Ok(StreamText {
        text: String::from_utf8_lossy(&kept).into_owned(),
        truncated,
    })
}

fn join_stream(
    handle: thread::JoinHandle<io::Result<StreamText>>,
) -> Result<StreamText, ToolError> {
    match handle.join() {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(error)) => Err(ToolError::Io(format!("读取 shell 输出失败：{error}"))),
        Err(_) => Err(ToolError::Io("shell 输出读取线程异常退出".into())),
    }
}

fn format_output(output: CommandOutput) -> ToolOutput {
    let exit_code = output
        .exit_code
        .map_or_else(|| "signal".to_string(), |code| code.to_string());
    let mut text = format!("exit_code: {exit_code}\nstdout:\n");
    append_stream(&mut text, &output.stdout);
    text.push_str("\nstderr:\n");
    append_stream(&mut text, &output.stderr);
    ToolOutput { text }
}

fn append_stream(text: &mut String, stream: &StreamText) {
    if stream.text.is_empty() {
        text.push_str("(empty)");
    } else {
        text.push_str(&stream.text);
        if !stream.text.ends_with('\n') {
            text.push('\n');
        }
    }
    if stream.truncated {
        text.push_str("…[truncated]\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::exec::{LocalToolExecutor, ToolCall, ToolExecutor};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lc-agent-shell-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn shell(kind: ShellKind, path: &str) -> ShellInfo {
        ShellInfo {
            path: path.into(),
            kind,
            version_major: 7,
        }
    }

    fn detected_shell() -> Option<ShellInfo> {
        let shell = detect_shell_real();
        if shell.is_none() {
            eprintln!("跳过真实 shell 用例：当前环境未探测到 pwsh/bash");
        }
        shell
    }

    fn echo_command() -> &'static str {
        if cfg!(windows) {
            "Write-Output 'hello'"
        } else {
            "echo hello"
        }
    }

    fn fail_command() -> &'static str {
        if cfg!(windows) {
            "Write-Error 'boom'; exit 3"
        } else {
            "echo boom >&2; exit 3"
        }
    }

    // ---- 纯逻辑 ----

    #[test]
    fn pwsh_invocation_sets_deterministic_flags_and_wrapper() {
        let shell = shell(ShellKind::Pwsh, r"C:\Program Files\PowerShell\7\pwsh.exe");
        let invocation = build_invocation(&shell, "Get-ChildItem");
        assert_eq!(invocation.program, shell.path);
        for flag in [
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ] {
            assert!(invocation.args.iter().any(|arg| arg == flag), "{flag}");
        }
        let script = invocation.args.last().expect("script");
        assert!(script.contains("[Console]::OutputEncoding"), "{script}");
        assert!(script.contains("Get-ChildItem"), "{script}");
        assert!(script.contains("$LASTEXITCODE"), "{script}");
        assert!(script.contains("exit 1"), "{script}");
    }

    #[test]
    fn bash_invocation_uses_noprofile_norc_c() {
        let shell = shell(ShellKind::Bash, "/bin/bash");
        let invocation = build_invocation(&shell, "echo hi && false");
        assert_eq!(invocation.program, "/bin/bash");
        assert_eq!(
            invocation.args,
            vec!["--noprofile", "--norc", "-c", "echo hi && false"]
        );
    }

    #[test]
    fn timeout_defaults_clamps_and_rejects_invalid() {
        assert_eq!(parse_timeout(&json!({})).unwrap(), DEFAULT_TIMEOUT_MS);
        assert_eq!(parse_timeout(&json!({"timeout_ms": 250})).unwrap(), 250);
        assert_eq!(
            parse_timeout(&json!({"timeout_ms": MAX_TIMEOUT_MS + 1})).unwrap(),
            MAX_TIMEOUT_MS
        );
        assert!(matches!(
            parse_timeout(&json!({"timeout_ms": 0})),
            Err(ToolError::InvalidArgs(_))
        ));
        assert!(matches!(
            parse_timeout(&json!({"timeout_ms": -1})),
            Err(ToolError::InvalidArgs(_))
        ));
        assert!(matches!(
            parse_timeout(&json!({"timeout_ms": "30s"})),
            Err(ToolError::InvalidArgs(_))
        ));
    }

    #[test]
    fn command_argument_is_required_and_non_empty() {
        assert!(matches!(
            command_arg(&json!({})),
            Err(ToolError::InvalidArgs(_))
        ));
        assert!(matches!(
            command_arg(&json!({"command": "   "})),
            Err(ToolError::InvalidArgs(_))
        ));
        assert_eq!(
            command_arg(&json!({"command": "echo hi"})).unwrap(),
            "echo hi"
        );
    }

    #[test]
    fn output_is_capped_per_stream() {
        let input = vec![b'x'; 40];
        let stream = read_capped(io::Cursor::new(input), 16).unwrap();
        assert_eq!(stream.text.len(), 16);
        assert!(stream.truncated);

        let short = read_capped(io::Cursor::new(b"hello".to_vec()), 16).unwrap();
        assert_eq!(short.text, "hello");
        assert!(!short.truncated);
    }

    #[test]
    fn formatted_output_contains_exit_code_and_both_streams() {
        let output = CommandOutput {
            exit_code: Some(3),
            stdout: StreamText {
                text: "out\n".into(),
                truncated: false,
            },
            stderr: StreamText {
                text: "err".into(),
                truncated: true,
            },
        };
        let text = format_output(output).text;
        assert!(text.starts_with("exit_code: 3\nstdout:\n"), "{text}");
        assert!(text.contains("out\n"), "{text}");
        assert!(text.contains("\nstderr:\nerr\n…[truncated]"), "{text}");
        assert!(!text.contains("out\n…"), "{text}");
    }

    #[test]
    fn process_errors_map_to_shell_or_io() {
        let not_found = map_process_error(ProcessError::NotFound {
            program: "pwsh".into(),
        });
        assert!(matches!(not_found, ToolError::ShellUnavailable(_)));

        let spawn = map_process_error(ProcessError::Spawn {
            program: "pwsh".into(),
            message: "denied".into(),
        });
        assert!(matches!(spawn, ToolError::Io(_)));
    }

    #[test]
    fn missing_shell_message_is_actionable() {
        let message = missing_shell_message();
        if cfg!(windows) {
            assert!(message.contains("PowerShell 7"), "{message}");
            assert!(message.contains("winget install"), "{message}");
        } else {
            assert!(message.contains("bash"), "{message}");
        }
    }

    // ---- 真实命令（平台感知；shell 缺失时跳过） ----

    #[test]
    fn executes_short_command_and_captures_output() {
        let Some(_shell) = detected_shell() else {
            return;
        };
        let root = temp_root("echo");
        let executor = LocalToolExecutor::new(&root);
        let output = executor
            .execute(&ToolCall::new("shell", json!({"command": echo_command()})))
            .expect("shell command");
        assert!(output.text.contains("exit_code: 0"), "{}", output.text);
        assert!(output.text.contains("hello"), "{}", output.text);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn non_zero_exit_is_a_normal_result_with_stderr() {
        let Some(_shell) = detected_shell() else {
            return;
        };
        let root = temp_root("nonzero");
        let executor = LocalToolExecutor::new(&root);
        let output = executor
            .execute(&ToolCall::new("shell", json!({"command": fail_command()})))
            .expect("non-zero exit must not be an error");
        assert!(output.text.contains("exit_code: 3"), "{}", output.text);
        assert!(output.text.contains("boom"), "{}", output.text);
        let _ = fs::remove_dir_all(&root);
    }

    /// 200ms 超时必须终止整组：脚本先派发一个 1s 后写标记的孙进程，再长睡。
    /// 若只杀直接子进程，孙进程会在超时后写入标记。
    #[test]
    fn timeout_kills_the_whole_process_tree() {
        let Some(_shell) = detected_shell() else {
            return;
        };
        let root = temp_root("timeout");
        let marker = root.join("survivor.txt");
        let script = if cfg!(windows) {
            format!(
                "$null = Start-Job -ScriptBlock {{ Start-Sleep -Milliseconds 1000; \
                 Set-Content -Path '{}' -Value alive }}\nStart-Sleep -Seconds 30",
                marker.display()
            )
        } else {
            format!(
                "(sleep 1; printf alive > '{}') & sleep 30",
                marker.display()
            )
        };

        let executor = LocalToolExecutor::new(&root);
        let start = Instant::now();
        let error = executor
            .execute(&ToolCall::new(
                "shell",
                json!({"command": script, "timeout_ms": 200}),
            ))
            .expect_err("must time out");
        assert!(
            matches!(error, ToolError::Timeout { timeout_ms: 200 }),
            "{error:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "timeout took {:?}",
            start.elapsed()
        );

        // 给"未被杀的孙进程"足够时间写标记
        thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "孙进程逃逸，进程树未被完整回收");
        let _ = fs::remove_dir_all(&root);
    }
}
