//! 启用状态持久化（`~/.lite-code/extensions.json`）。
//!
//! 文件形如 `{ "disabled": ["id"] }`；未知顶层字段原样保留，避免降级
//! 读写丢失未来版本的数据。写入复用 config lane 的原子替换（临时文件 +
//! rename，已有文件先备份 `.bak`）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{ExtError, ExtPaths};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(flatten, default)]
    extra: Map<String, Value>,
}

/// `extensions.json` 的读写句柄。
#[derive(Debug, Clone)]
pub struct ExtensionsState {
    path: PathBuf,
}

impl ExtensionsState {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn at_default() -> Result<Self, ExtError> {
        Ok(Self::new(ExtPaths::default_for(None)?.state_path))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取 disabled 集合；文件不存在视为空集合。
    pub fn load(&self) -> Result<BTreeSet<String>, ExtError> {
        self.load_file()
    }

    /// 设置启用状态并落盘。`enabled = true` 从 disabled 集合移除。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), ExtError> {
        if id.trim().is_empty() {
            return Err(ExtError::UnknownPlugin { id: id.into() });
        }
        let mut disabled = self.load()?;
        if enabled {
            disabled.remove(id);
        } else {
            disabled.insert(id.to_string());
        }
        self.save(&disabled)
    }

    fn load_file(&self) -> Result<BTreeSet<String>, ExtError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeSet::new());
            }
            Err(error) => {
                return Err(ExtError::Config(format!(
                    "读取 {} 失败：{error}",
                    self.path.display()
                )));
            }
        };
        let file: StateFile = serde_json::from_str(&text).map_err(|error| {
            ExtError::Config(format!("{} 解析失败：{error}", self.path.display()))
        })?;
        Ok(file.disabled.into_iter().collect())
    }

    fn save(&self, disabled: &BTreeSet<String>) -> Result<(), ExtError> {
        let mut file = match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str::<StateFile>(&text).map_err(|error| {
                ExtError::Config(format!("{} 解析失败：{error}", self.path.display()))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StateFile::default(),
            Err(error) => {
                return Err(ExtError::Config(format!(
                    "读取 {} 失败：{error}",
                    self.path.display()
                )));
            }
        };
        file.disabled = disabled.iter().cloned().collect();
        let mut text = serde_json::to_string_pretty(&file)
            .map_err(|error| ExtError::Config(format!("序列化扩展状态失败：{error}")))?;
        text.push('\n');
        crate::config::write_atomic(&self.path, &text)
            .map_err(|error| ExtError::Io(format!("写入 {} 失败：{error}", self.path.display())))
    }
}
