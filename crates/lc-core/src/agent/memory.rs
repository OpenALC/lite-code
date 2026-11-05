//! 长期记忆读取（docs/AGENT.md §7）。P3 只做 Lite Code 自有格式与项目
//! `AGENTS.md` / `CLAUDE.md` 的只读注入；写入 API 保留原子替换 + `.bak`，
//! 供后续记忆面板使用。Claude Code / Codex 的完整适配在 P5。

use std::path::{Path, PathBuf};

use crate::config::write_atomic;

pub const MAX_FILE_CHARS: usize = 8_000;
pub const MAX_TOTAL_CHARS: usize = 16_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFile {
    pub label: String,
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryBundle {
    pub files: Vec<MemoryFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryError {
    #[error("记忆文件 I/O 失败：{0}")]
    Io(String),
}

impl MemoryBundle {
    /// 读取全局 → 项目 → 根目录说明文件；单文件与总量都有上限。
    /// 文件不存在不是错误。
    pub fn load(root: &Path) -> Self {
        let mut candidates = Vec::new();
        if let Some(home) = dirs::home_dir() {
            candidates.push(("全局记忆", home.join(".lite-code").join("memory.md")));
        }
        candidates.push(("项目记忆", root.join(".lite-code").join("memory.md")));
        candidates.push(("项目说明（AGENTS.md）", root.join("AGENTS.md")));
        candidates.push(("项目说明（CLAUDE.md）", root.join("CLAUDE.md")));

        let mut files = Vec::new();
        let mut total = 0usize;
        for (label, path) in candidates {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let remaining = MAX_TOTAL_CHARS.saturating_sub(total);
            if remaining == 0 {
                break;
            }
            let limit = MAX_FILE_CHARS.min(remaining);
            let content = truncate_chars(content.trim(), limit);
            if content.is_empty() {
                continue;
            }
            total += content.chars().count();
            files.push(MemoryFile {
                label: label.to_string(),
                path,
                content,
            });
        }
        Self { files }
    }

    /// 注入 system prompt 的 Markdown；无记忆时返回 `None`。
    pub fn render(&self) -> Option<String> {
        if self.files.is_empty() {
            return None;
        }
        let mut out = String::from("## 会话记忆\n\n");
        for file in &self.files {
            out.push_str(&format!("### {}\n\n", file.label));
            out.push_str(&file.content);
            out.push_str("\n\n");
        }
        Some(out.trim_end().to_string())
    }
}

/// 追加一段记忆；保留原内容，缺少结尾换行时补上，写入走 `.bak` + 原子替换。
pub fn append(path: &Path, text: &str) -> Result<(), MemoryError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let mut content = match std::fs::read_to_string(path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(MemoryError::Io(error.to_string())),
    };
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(text);
    content.push('\n');
    write_atomic(path, &content).map_err(|error| MemoryError::Io(error.to_string()))
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut out: String = value.chars().take(limit).collect();
    out.push_str("\n…[memory truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lc-memory-{tag}-{}-{}",
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
    fn loads_project_memory_and_agents_file() {
        let root = temp_root("load");
        std::fs::create_dir_all(root.join(".lite-code")).unwrap();
        std::fs::write(root.join(".lite-code/memory.md"), "project fact").unwrap();
        std::fs::write(root.join("AGENTS.md"), "agent rules").unwrap();

        let bundle = MemoryBundle::load(&root);
        let labels: Vec<&str> = bundle
            .files
            .iter()
            .map(|file| file.label.as_str())
            .collect();
        assert!(labels.contains(&"项目记忆"));
        assert!(labels.contains(&"项目说明（AGENTS.md）"));
        let rendered = bundle.render().expect("render");
        assert!(rendered.starts_with("## 会话记忆"));
        assert!(rendered.contains("project fact"));
        assert!(rendered.contains("agent rules"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_memory_renders_none() {
        let root = temp_root("empty");
        assert_eq!(MemoryBundle::load(&root).render(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn large_file_is_truncated_with_marker() {
        let root = temp_root("cap");
        std::fs::create_dir_all(root.join(".lite-code")).unwrap();
        std::fs::write(
            root.join(".lite-code/memory.md"),
            "x".repeat(MAX_FILE_CHARS + 100),
        )
        .unwrap();
        let bundle = MemoryBundle::load(&root);
        assert_eq!(bundle.files.len(), 1);
        assert!(bundle.files[0].content.ends_with("[memory truncated]"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn append_preserves_existing_and_writes_backup() {
        let root = temp_root("append");
        let path = root.join(".lite-code/memory.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "first line").unwrap();

        append(&path, "second line").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "first line\nsecond line\n");
        assert!(root.join(".lite-code/memory.md.bak").exists());

        append(&path, "   ").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        let _ = std::fs::remove_dir_all(&root);
    }
}
