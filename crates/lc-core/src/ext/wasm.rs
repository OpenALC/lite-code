//! WASM 运行器（docs/EXTENSIONS.md §5）。
//!
//! 每次命令启动一个 `node wasm-host.mjs --wasm … --command … --args …
//! --workspace … --timeout …` 子进程；stdout 必须是单个 JSON 值，stderr 作为错误详情。
//! WASM 模块本身走 Node 的 `node:wasi`（preview1），不暴露 Node API；
//! 超时由 Rust 侧控制。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::fs::display_path;
use crate::process::scrub_secret_env;

use super::ExtError;

/// 单次 WASM 命令超时。
pub const WASM_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const STDERR_TAIL_CHARS: usize = 4096;

pub(crate) fn run(
    node: &Path,
    script: &Path,
    wasm: &Path,
    command: &str,
    args: &Value,
    workspace: &Path,
) -> Result<Value, ExtError> {
    let args_json = serde_json::to_string(args)
        .map_err(|error| ExtError::WasmFailed(format!("参数序列化失败：{error}")))?;
    let script = PathBuf::from(display_path(script));
    let wasm_arg = display_path(wasm);
    let workspace_arg = display_path(workspace);

    let mut command_line = Command::new(node);
    command_line
        .arg(script)
        .arg("--wasm")
        .arg(&wasm_arg)
        .arg("--command")
        .arg(command)
        .arg("--args")
        .arg(args_json)
        .arg("--workspace")
        .arg(&workspace_arg)
        .arg("--timeout")
        .arg(
            u64::try_from(WASM_TIMEOUT.as_millis())
                .unwrap_or(u64::MAX)
                .to_string(),
        )
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    scrub_secret_env(&mut command_line);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command_line.creation_flags(WINDOWS_CREATE_NO_WINDOW);
    }

    let mut child = command_line.spawn().map_err(|error| {
        ExtError::HostSpawn(format!("无法启动 WASM 宿主 {}：{error}", node.display()))
    })?;

    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ExtError::WasmFailed("未能取得 wasm 宿主 stdio 管道".into()));
    };
    let stdout_reader = read_in_background(stdout);
    let stderr_reader = read_in_background(stderr);

    let deadline = Instant::now()
        .checked_add(WASM_TIMEOUT)
        .unwrap_or_else(Instant::now);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(ExtError::WasmTimeout {
                        timeout_ms: u64::try_from(WASM_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                    });
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ExtError::WasmFailed(format!("等待 wasm 宿主失败：{error}")));
            }
        }
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let stderr_text = tail_text(&String::from_utf8_lossy(&stderr));

    if !status.success() {
        return Err(ExtError::WasmFailed(format!(
            "wasm 宿主退出码 {status}{}",
            if stderr_text.is_empty() {
                String::new()
            } else {
                format!("；stderr:\n{stderr_text}")
            }
        )));
    }

    serde_json::from_slice(&stdout).map_err(|error| {
        ExtError::WasmFailed(format!(
            "wasm 插件 stdout 不是单个 JSON 值：{error}{}",
            if stderr_text.is_empty() {
                String::new()
            } else {
                format!("；stderr:\n{stderr_text}")
            }
        ))
    })
}

fn read_in_background(mut reader: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    })
}

fn tail_text(text: &str) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= STDERR_TAIL_CHARS {
        trimmed.to_string()
    } else {
        trimmed.chars().skip(count - STDERR_TAIL_CHARS).collect()
    }
}
