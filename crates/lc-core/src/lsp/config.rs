//! LSP server 配置（插件清单 `contributes.languageServers`，docs/EXTENSIONS.md §1）。
//!
//! 插件 JSON 使用 camelCase（`languageIds`），Rust 侧直接反序列化；启动命令与
//! 参数原样交给 std `Command`，工作目录为 workspace 根。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 单个 stdio LSP server 配置。
///
/// ```json
/// {
///   "id": "typescript",
///   "languageIds": ["typescript", "javascript"],
///   "command": "typescript-language-server",
///   "args": ["--stdio"],
///   "env": {}
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerConfig {
    /// 全局唯一的 server id（插件层应做命名空间隔离）。
    pub id: String,
    /// 该 server 处理的语言 id；`didOpen` 按此路由。
    #[serde(default)]
    pub language_ids: Vec<String>,
    /// 可执行文件或 PATH 中的命令。
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// 显式传给子进程的环境变量；父进程 provider 密钥已被 `scrub_secret_env` 清除。
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl LspServerConfig {
    /// 构造一个无参数、无额外环境变量的 server 配置。
    pub fn new(id: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            language_ids: Vec::new(),
            command: command.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }
}

/// 启动成功后回报给前端的 server 摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerInfo {
    pub id: String,
    pub language_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_uses_camel_case_language_ids() {
        let config: LspServerConfig = serde_json::from_value(json!({
            "id": "typescript",
            "languageIds": ["typescript", "javascript"],
            "command": "typescript-language-server",
            "args": ["--stdio"],
            "env": {"TS_LOG": "1"}
        }))
        .expect("deserialize config");

        assert_eq!(config.id, "typescript");
        assert_eq!(config.language_ids, ["typescript", "javascript"]);
        assert_eq!(config.args, ["--stdio"]);
        assert_eq!(config.env.get("TS_LOG").map(String::as_str), Some("1"));

        let value = serde_json::to_value(&config).expect("serialize config");
        assert!(value.get("languageIds").is_some());
        assert!(value.get("language_ids").is_none());
    }

    #[test]
    fn optional_fields_default_to_empty() {
        let config: LspServerConfig = serde_json::from_value(json!({
            "id": "minimal",
            "command": "server"
        }))
        .expect("deserialize minimal config");

        assert!(config.language_ids.is_empty());
        assert!(config.args.is_empty());
        assert!(config.env.is_empty());
    }
}
