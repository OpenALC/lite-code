//! 真实 spawn：本地 shell + 系统 ssh（portable-pty，native pty 而非管道）。
//!
//! ssh 参数构造（`ssh_command`）与 Windows shell 候选可单测；spawn 路径
//! 需要 pty 环境，由桌面侧集成验证（headless CI 只跑 lc-core::pty 的
//! fake 会话测试）。

use std::io::{Read, Write};
use std::sync::Mutex;

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, NativePtySystem, PtySize};

use super::{PtyControl, PtyHandles, PtyReader, Spec};

/// TERM 环境值（验收：需设置 `TERM=xterm-256color`）。
pub(crate) const TERM: &str = "xterm-256color";

/// ssh 参数（验收：`ssh [-p port] [-l user] host`，不引 SSH 库）。
pub fn ssh_command(host: &str, port: u16, user: Option<&str>) -> Vec<String> {
    let mut args = vec!["ssh".to_string()];
    if port != 22 {
        args.push("-p".to_string());
        args.push(port.to_string());
    }
    if let Some(user) = user.filter(|u| !u.is_empty()) {
        args.push("-l".to_string());
        args.push(user.to_string());
    }
    args.push(host.to_string());
    args
}

fn child_command(spec: &Spec) -> CommandBuilder {
    let mut command = match spec {
        Spec::Local => local_shell_command(),
        Spec::Ssh { host, port, user } => {
            let args = ssh_command(host, *port, user.as_deref());
            let mut command = CommandBuilder::new(&args[0]);
            command.args(&args[1..]);
            command
        }
    };
    command.env("TERM", TERM);
    command
}

/// 本地 shell（行为要求 1，参照 runtime lane 的探测顺序）：
/// Windows pwsh 7 → powershell.exe → cmd；Unix $SHELL → bash。
/// 找不到候选时回退 `CommandBuilder::new_default_prog`（系统登录 shell）。
fn local_shell_command() -> CommandBuilder {
    if cfg!(windows) {
        for candidate in local_shell_candidates_windows() {
            if !candidate.is_file() {
                continue;
            }
            let is_cmd = candidate
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with("cmd.exe");
            let mut command = CommandBuilder::new(&candidate);
            // powershell/pwsh 去 banner；cmd 不认 -NoLogo，直接进交互 shell。
            if !is_cmd {
                command.arg("-NoLogo");
            }
            return command;
        }
        return CommandBuilder::new_default_prog();
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut command = CommandBuilder::new(shell);
    command.arg("-l");
    command
}

/// Windows shell 候选（pwsh 7 → powershell.exe → cmd）。
fn local_shell_candidates_windows() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(
            std::path::PathBuf::from(program_files)
                .join("PowerShell")
                .join("7")
                .join("pwsh.exe"),
        );
    }
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        let root = std::path::PathBuf::from(system_root);
        candidates.push(
            root.join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe"),
        );
        candidates.push(root.join("System32").join("cmd.exe"));
    }
    candidates
}

/// 按会话参数启动真实进程，拆成命令线程/读线程两半区。
pub fn spawn_real(spec: &Spec, cols: u16, rows: u16) -> std::io::Result<PtyHandles> {
    let pair = portable_pty::PtySystem::openpty(
        &NativePtySystem::default(),
        PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        },
    )
    .map_err(std::io::Error::other)?;
    let child = pair
        .slave
        .spawn_command(child_command(spec))
        .map_err(std::io::Error::other)?;
    drop(pair.slave); // 父侧关掉 slave，子进程退出后 reader 才能读到 EOF
    let killer = child.clone_killer();
    let writer = pair.master.take_writer().map_err(std::io::Error::other)?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(std::io::Error::other)?;
    Ok(PtyHandles {
        control: Box::new(RealControl {
            writer: Some(writer),
            master: pair.master,
            killer,
        }),
        reader: Box::new(RealReader {
            reader,
            child: Mutex::new(child),
        }),
    })
}

struct RealControl {
    writer: Option<Box<dyn Write + Send>>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl PtyControl for RealControl {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self.writer.as_mut() {
            Some(writer) => writer.write_all(data),
            None => Ok(()),
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(std::io::Error::other)
    }

    fn kill(&mut self) {
        let _ = self.writer.take();
        let _ = self.killer.kill();
        // master drop：关 ConPTY 句柄，子进程随 pty 关闭终止。
    }
}

struct RealReader {
    reader: Box<dyn Read + Send>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

impl PtyReader for RealReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }

    fn try_wait(&mut self) -> Option<portable_pty::ExitStatus> {
        self.child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_wait()
            .ok()
            .flatten()
    }
}
