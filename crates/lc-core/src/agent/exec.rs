//! 内置工具执行（docs/AGENT.md §4）：read_file / write_file / edit_file /
//! glob / grep。所有路径先经 `ensure_within_root`；shell 等执行类工具留给
//! PTY/ProcessSupervisor lane，当前明确返回 Unsupported。

use std::path::{Path, PathBuf};

use globset::Glob;
use lc_proto::EntryKind;
use regex::Regex;
use serde_json::Value;

use crate::fs::{
    FsError, RealPathResolver, display_path, ensure_within_root, walk_tree, write_file_text,
};

const MAX_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub args: Value,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, args: Value) -> Self {
        Self {
            name: name.into(),
            args,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolError {
    #[error("参数错误：{0}")]
    InvalidArgs(String),
    #[error("路径不在工作区内：{0}")]
    PathOutsideRoot(String),
    #[error("文件不存在：{0}")]
    NotFound(String),
    #[error("I/O 错误：{0}")]
    Io(String),
    #[error("未找到待替换文本")]
    EditNotMatched,
    #[error("待替换文本出现 {0} 次，需显式 replace_all")]
    AmbiguousMatch(usize),
    #[error("正则无效：{0}")]
    Regex(String),
    #[error("工具暂未支持：{0}")]
    Unsupported(String),
}

fn fs_error(error: FsError) -> ToolError {
    match error {
        FsError::PathOutsideRoot { path } => ToolError::PathOutsideRoot(path),
        FsError::NotFound { path } => ToolError::NotFound(path),
        FsError::Io { path, message } => ToolError::Io(format!("{path}: {message}")),
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArgs(format!("缺少字符串参数 {key}")))
}

fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn truncate(mut text: String) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text;
    }
    let cut = text
        .char_indices()
        .take_while(|(i, _)| *i < MAX_OUTPUT_BYTES)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    text.truncate(cut);
    text.push_str("\n…[truncated]");
    text
}

/// 工具执行抽象：真实实现 [`LocalToolExecutor`]，测试可注入 fake。
pub trait ToolExecutor: Send + Sync {
    fn execute(&self, call: &ToolCall) -> Result<ToolOutput, ToolError>;
}

#[derive(Debug, Clone)]
pub struct LocalToolExecutor {
    root: PathBuf,
}

impl LocalToolExecutor {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn read(&self, path: &str) -> Result<(PathBuf, String), ToolError> {
        let candidate = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.root.join(path)
        };
        let canonical =
            ensure_within_root(&self.root, &candidate, &RealPathResolver).map_err(fs_error)?;
        let text = std::fs::read_to_string(&canonical)
            .map_err(|e| ToolError::Io(format!("{}: {e}", display_path(&canonical))))?;
        Ok((canonical, text))
    }

    fn canonical_root(&self) -> Result<PathBuf, ToolError> {
        ensure_within_root(&self.root, &self.root, &RealPathResolver).map_err(fs_error)
    }

    fn read_file(&self, args: &Value) -> Result<ToolOutput, ToolError> {
        let path = arg_str(args, "path")?;
        let (_, text) = self.read(path)?;
        Ok(ToolOutput {
            text: truncate(text),
        })
    }

    fn write_file(&self, args: &Value) -> Result<ToolOutput, ToolError> {
        let path = arg_str(args, "path")?;
        let content = arg_str(args, "content")?;
        let written = write_file_text(&self.root, Path::new(path), content, &RealPathResolver)
            .map_err(fs_error)?;
        Ok(ToolOutput {
            text: format!("written {written} ({} bytes)", content.len()),
        })
    }

    fn edit_file(&self, args: &Value) -> Result<ToolOutput, ToolError> {
        let path = arg_str(args, "path")?;
        let old = arg_str(args, "old_string")?;
        let new = arg_str(args, "new_string")?;
        let replace_all = arg_bool(args, "replace_all", false);
        if old.is_empty() {
            return Err(ToolError::InvalidArgs("old_string 不能为空".into()));
        }

        let (canonical, content) = self.read(path)?;
        let count = content.matches(old).count();
        if count == 0 {
            return Err(ToolError::EditNotMatched);
        }
        if count > 1 && !replace_all {
            return Err(ToolError::AmbiguousMatch(count));
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            content.replacen(old, new, 1)
        };
        let written = write_file_text(&self.root, &canonical, &updated, &RealPathResolver)
            .map_err(fs_error)?;
        Ok(ToolOutput {
            text: format!("edited {written}: {count} replacement(s)"),
        })
    }

    fn glob(&self, args: &Value) -> Result<ToolOutput, ToolError> {
        let pattern = arg_str(args, "pattern")?;
        let matcher = Glob::new(pattern)
            .map_err(|e| ToolError::Regex(e.to_string()))?
            .compile_matcher();
        let root = self.canonical_root()?;
        let root_display = display_path(&root);

        let mut out = Vec::new();
        for entry in walk_tree(&self.root, &RealPathResolver).map_err(fs_error)? {
            let path = Path::new(&entry.path);
            let relative = path.strip_prefix(&root_display).unwrap_or(path);
            if matcher.is_match(relative) || matcher.is_match(&entry.name) {
                out.push(entry.path);
                if out.len() >= MAX_ENTRIES {
                    break;
                }
            }
        }
        Ok(ToolOutput {
            text: if out.is_empty() {
                "no matches".into()
            } else {
                truncate(out.join("\n"))
            },
        })
    }

    fn grep(&self, args: &Value) -> Result<ToolOutput, ToolError> {
        let pattern = arg_str(args, "pattern")?;
        let re = Regex::new(pattern).map_err(|e| ToolError::Regex(e.to_string()))?;
        let root = self.canonical_root()?;
        let root_display = display_path(&root);

        let mut out = Vec::new();
        let mut stopped = false;
        'files: for entry in walk_tree(&self.root, &RealPathResolver).map_err(fs_error)? {
            if entry.kind != EntryKind::File {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&entry.path) else {
                continue;
            };
            let relative = Path::new(&entry.path)
                .strip_prefix(&root_display)
                .unwrap_or(Path::new(&entry.path))
                .display()
                .to_string();
            for (index, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    out.push(format!("{relative}:{}:{line}", index + 1));
                    if out.len() >= MAX_ENTRIES {
                        stopped = true;
                        break 'files;
                    }
                }
            }
        }
        if out.is_empty() {
            return Ok(ToolOutput {
                text: "no matches".into(),
            });
        }
        let mut text = out.join("\n");
        if stopped {
            text.push_str("\n…[truncated]");
        }
        Ok(ToolOutput {
            text: truncate(text),
        })
    }
}

impl ToolExecutor for LocalToolExecutor {
    fn execute(&self, call: &ToolCall) -> Result<ToolOutput, ToolError> {
        match call.name.as_str() {
            "read_file" => self.read_file(&call.args),
            "write_file" => self.write_file(&call.args),
            "edit_file" => self.edit_file(&call.args),
            "glob" => self.glob(&call.args),
            "grep" => self.grep(&call.args),
            other => Err(ToolError::Unsupported(format!(
                "{other}（执行类工具待 PTY/ProcessSupervisor lane）"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lc-agent-exec-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_write_edit_roundtrip() {
        let root = temp_root("rw");
        let executor = LocalToolExecutor::new(&root);

        let write = executor
            .execute(&ToolCall::new(
                "write_file",
                json!({"path": "a.txt", "content": "hello world\n"}),
            ))
            .expect("write");
        assert!(write.text.contains("written"), "{}", write.text);

        let read = executor
            .execute(&ToolCall::new("read_file", json!({"path": "a.txt"})))
            .expect("read");
        assert_eq!(read.text, "hello world\n");

        let edit = executor
            .execute(&ToolCall::new(
                "edit_file",
                json!({"path": "a.txt", "old_string": "world", "new_string": "agent"}),
            ))
            .expect("edit");
        assert!(edit.text.contains("1 replacement"), "{}", edit.text);
        assert_eq!(
            fs::read_to_string(root.join("a.txt")).unwrap(),
            "hello agent\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_rejects_ambiguous_and_missing_matches() {
        let root = temp_root("edit");
        fs::write(root.join("a.txt"), "x x").unwrap();
        let executor = LocalToolExecutor::new(&root);

        let err = executor
            .execute(&ToolCall::new(
                "edit_file",
                json!({"path": "a.txt", "old_string": "x", "new_string": "y"}),
            ))
            .unwrap_err();
        assert_eq!(err, ToolError::AmbiguousMatch(2));

        let err = executor
            .execute(&ToolCall::new(
                "edit_file",
                json!({"path": "a.txt", "old_string": "zzz", "new_string": "y"}),
            ))
            .unwrap_err();
        assert_eq!(err, ToolError::EditNotMatched);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_outside_root() {
        let root = temp_root("guard");
        let executor = LocalToolExecutor::new(&root);
        let outside = std::env::temp_dir().join("lc-agent-outside.txt");
        let err = executor
            .execute(&ToolCall::new(
                "write_file",
                json!({"path": outside.to_str().unwrap(), "content": "x"}),
            ))
            .unwrap_err();
        assert!(matches!(err, ToolError::PathOutsideRoot(_)));
        assert!(!outside.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn glob_and_grep_respect_tree() {
        let root = temp_root("search");
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "fn main() {}\n// TODO: fix\n").unwrap();
        fs::write(root.join("b.txt"), "TODO text\n").unwrap();
        let executor = LocalToolExecutor::new(&root);

        let glob = executor
            .execute(&ToolCall::new("glob", json!({"pattern": "**/*.rs"})))
            .expect("glob");
        assert!(glob.text.contains("a.rs"), "{}", glob.text);

        let grep = executor
            .execute(&ToolCall::new("grep", json!({"pattern": "TODO"})))
            .expect("grep");
        assert!(grep.text.contains("a.rs:2"), "{}", grep.text);
        assert!(grep.text.contains("b.txt:1"), "{}", grep.text);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn shell_is_explicitly_unsupported_for_now() {
        let root = temp_root("shell");
        let executor = LocalToolExecutor::new(&root);
        let err = executor
            .execute(&ToolCall::new("shell", json!({"command": "echo hi"})))
            .unwrap_err();
        assert!(matches!(err, ToolError::Unsupported(_)));
        let _ = fs::remove_dir_all(&root);
    }
}
