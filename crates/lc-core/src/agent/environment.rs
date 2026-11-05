//! 环境契约（docs/AGENT.md §4.1）：把平台、shell 方言、路径分隔符与工作区
//! 一次性写进 system prompt，避免模型在 PowerShell 里写 POSIX 命令。
//!
//! 该段由 Rust 侧生成并随 instructions 下发；用户消息不能覆盖。

use std::path::{Path, PathBuf};

use lc_proto::ShellInfo;

use crate::runtime::detect_shell_real;

use super::memory::MemoryBundle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentContract {
    pub platform: String,
    pub arch: String,
    pub shell: Option<ShellInfo>,
    pub workspace_root: String,
    pub home: Option<String>,
    pub temp_dir: String,
    pub available: Vec<String>,
}

impl EnvironmentContract {
    pub fn detect(root: &Path) -> Self {
        Self {
            platform: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            shell: detect_shell_real(),
            workspace_root: root.display().to_string(),
            home: dirs::home_dir().map(|path| path.display().to_string()),
            temp_dir: std::env::temp_dir().display().to_string(),
            available: detect_available(&["git", "node", "npm", "cargo", "rg"]),
        }
    }

    /// 稳定、可缓存的 XML 片段（每个会话重建一次）。
    pub fn render(&self) -> String {
        let (shell, shell_version, shell_path, dialect, separator) = match &self.shell {
            Some(shell) => (
                match shell.kind {
                    lc_proto::ShellKind::Pwsh => "powershell",
                    lc_proto::ShellKind::Bash => "bash",
                },
                shell.version_major.to_string(),
                shell.path.clone(),
                match shell.kind {
                    lc_proto::ShellKind::Pwsh => "pwsh",
                    lc_proto::ShellKind::Bash => "bash",
                },
                match shell.kind {
                    lc_proto::ShellKind::Pwsh => ";",
                    lc_proto::ShellKind::Bash => "&&",
                },
            ),
            None => ("unavailable", "0".into(), String::new(), "unknown", ";"),
        };
        let path_separator = if cfg!(windows) { r"\" } else { "/" };
        let case_sensitive = if cfg!(windows) { "false" } else { "true" };
        let available = if self.available.is_empty() {
            "[]".to_string()
        } else {
            format!("[{}]", self.available.join(", "))
        };
        format!(
            "<environment>\n\
             platform: {}\n\
             arch: {}\n\
             shell: {}\n\
             shell_version: \"{}\"\n\
             shell_path: \"{}\"\n\
             shell_dialect: {}\n\
             path_separator: \"{}\"\n\
             command_separator: \"{}\"\n\
             case_sensitive_paths: {}\n\
             default_encoding: utf-8\n\
             home: \"{}\"\n\
             workspace_root: \"{}\"\n\
             temp_dir: \"{}\"\n\
             available: {}\n\
             </environment>",
            escape(&self.platform),
            escape(&self.arch),
            shell,
            escape(&shell_version),
            escape(&shell_path),
            dialect,
            escape(path_separator),
            escape(separator),
            case_sensitive,
            escape(self.home.as_deref().unwrap_or("")),
            escape(&self.workspace_root),
            escape(&self.temp_dir),
            available,
        )
    }
}

/// 会话 instructions = 项目记忆 + 环境契约 + 调用方给的默认指令。
pub fn session_instructions(root: &Path, base: &str) -> String {
    let mut out = String::new();
    if let Some(memory) = MemoryBundle::load(root).render() {
        out.push_str(&memory);
        out.push_str("\n\n");
    }
    out.push_str(&EnvironmentContract::detect(root).render());
    out.push_str("\n\n");
    out.push_str(base);
    out
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn detect_available(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter(|name| find_in_path(name).is_some())
        .map(|name| (*name).to_string())
        .collect()
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        #[cfg(windows)]
        {
            let exts = std::env::var_os("PATHEXT")
                .map(|value| {
                    value
                        .to_string_lossy()
                        .split(';')
                        .map(|ext| ext.to_ascii_lowercase())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec![".exe".into(), ".cmd".into(), ".bat".into()]);
            for ext in std::iter::once(String::new()).chain(exts) {
                let candidate = dir.join(format!("{name}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lc-environment-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn render_contains_platform_shell_and_workspace() {
        let root = temp_root("render");
        let contract = EnvironmentContract::detect(&root);
        let rendered = contract.render();
        assert!(rendered.starts_with("<environment>"));
        assert!(rendered.contains("workspace_root:"));
        assert!(rendered.contains(&root.display().to_string()));
        assert!(rendered.contains("command_separator:"));
        assert!(rendered.contains("default_encoding: utf-8"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn renders_missing_shell_as_unavailable() {
        let contract = EnvironmentContract {
            platform: "windows".into(),
            arch: "x86_64".into(),
            shell: None,
            workspace_root: r"C:\work & play".into(),
            home: None,
            temp_dir: "C:\\Temp".into(),
            available: vec!["git".into()],
        };
        let rendered = contract.render();
        assert!(rendered.contains("shell: unavailable"));
        assert!(rendered.contains("C:\\work &amp; play"));
        assert!(rendered.contains("available: [git]"));
    }

    #[test]
    fn instructions_put_memory_before_environment_and_base() {
        let root = temp_root("instructions");
        std::fs::create_dir_all(root.join(".lite-code")).unwrap();
        std::fs::write(root.join(".lite-code/memory.md"), "use tabs").unwrap();
        let text = session_instructions(&root, "base instructions");
        let memory_at = text.find("use tabs").expect("memory");
        let env_at = text.find("<environment>").expect("environment");
        let base_at = text.find("base instructions").expect("base");
        assert!(memory_at < env_at && env_at < base_at);
        let _ = std::fs::remove_dir_all(&root);
    }
}
