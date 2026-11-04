//! [`ProcessRunner`] 的真实实现：带超时的子进程执行。
//!
//! Windows 全部 CREATE_NO_WINDOW（0x08000000），探测不允许闪黑框
//! （docs/RUNTIME.md §4/§6）。

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{ProcessRunner, RunnerError};

pub struct RealProcessRunner {
    timeout: Duration,
}

impl RealProcessRunner {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl ProcessRunner for RealProcessRunner {
    fn run(&self, exe: &Path, args: &[&str]) -> Result<String, RunnerError> {
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        let mut child = cmd.spawn().map_err(|e| RunnerError::Spawn(e.to_string()))?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    // 进程已退出，安全读取全部输出
                    let out = child
                        .wait_with_output()
                        .map_err(|e| RunnerError::Spawn(e.to_string()))?;
                    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                    text.push('\n');
                    text.push_str(&String::from_utf8_lossy(&out.stderr));
                    return Ok(text);
                }
                Ok(None) => {
                    if start.elapsed() >= self.timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(RunnerError::Timeout(self.timeout));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => return Err(RunnerError::Spawn(e.to_string())),
            }
        }
    }
}
