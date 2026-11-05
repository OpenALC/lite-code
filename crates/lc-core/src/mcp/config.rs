//! MCP 配置（Claude Code 兼容格式）。
//!
//! 读取并合并两个文件，项目级同名 server 覆盖用户级：
//! - `<root>/.mcp.json`
//! - `~/.lite-code/mcp.json`
//!
//! ```json
//! { "mcpServers": { "filesystem": {
//!     "command": "npx",
//!     "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
//!     "env": { "KEY": "VALUE" },
//!     "enabled": true
//! } } }
//! ```
//!
//! 只支持 `command` 型 stdio server。`type` 为 `http`/`sse` 或只给了 `url`
//! 的条目会被显式忽略并写入 [`McpConfig::warnings`]——不静默假装支持。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::APP_DIR_NAME;

use super::McpError;

/// 项目级配置文件名（放在工作区根目录）。
pub const PROJECT_FILE_NAME: &str = ".mcp.json";
/// 用户级配置文件名（放在 `~/.lite-code/`）。
pub const USER_FILE_NAME: &str = "mcp.json";

/// 单个 stdio MCP server 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// 缺省 `true`；`false` 的 server 保留在配置中但不连接。
    pub enabled: bool,
}

impl McpServerConfig {
    /// 构造一个启用的 stdio server。
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
        }
    }
}

/// 合并后的 MCP 配置与加载期 warning。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpConfig {
    /// 按「用户配置在前、项目配置覆盖同名项」的顺序保存（含 disabled）。
    pub servers: Vec<McpServerConfig>,
    /// 被忽略的 HTTP/SSE 条目、无法定位 home 等非致命问题。
    pub warnings: Vec<String>,
}

impl McpConfig {
    /// 读取项目与用户配置并合并。文件不存在视为空配置。
    pub fn load(root: &Path) -> Result<Self, McpError> {
        let user_path = dirs::home_dir().map(|home| home.join(APP_DIR_NAME).join(USER_FILE_NAME));
        let mut config =
            Self::load_with_user_file(&root.join(PROJECT_FILE_NAME), user_path.as_deref())?;
        if user_path.is_none() {
            config
                .warnings
                .push("无法定位用户主目录，跳过 ~/.lite-code/mcp.json".into());
        }
        Ok(config)
    }

    /// 显式指定两个配置路径（测试与自定义布局使用）。`user_path` 为 `None`
    /// 时只读项目文件。项目级同名 server 整体覆盖用户级条目。
    pub fn load_with_user_file(
        project_path: &Path,
        user_path: Option<&Path>,
    ) -> Result<Self, McpError> {
        let mut warnings = Vec::new();
        let user = match user_path {
            Some(path) => parse_file(path, &mut warnings)?,
            None => Vec::new(),
        };
        let project = parse_file(project_path, &mut warnings)?;
        Ok(Self {
            servers: merge_servers(user, project),
            warnings,
        })
    }

    /// 所有 server（含 disabled）。
    pub fn servers(&self) -> &[McpServerConfig] {
        &self.servers
    }

    /// 实际需要连接的 server。
    pub fn enabled_servers(&self) -> impl Iterator<Item = &McpServerConfig> {
        self.servers.iter().filter(|server| server.enabled)
    }

    /// 加载期 warning（HTTP/SSE 被忽略、home 不可得等）。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 按名字查找（含 disabled），便于上层展示连接状态。
    pub fn get(&self, name: &str) -> Option<&McpServerConfig> {
        self.servers.iter().find(|server| server.name == name)
    }
}

#[derive(Debug, Deserialize)]
struct RawFile {
    #[serde(default, rename = "mcpServers")]
    servers: BTreeMap<String, RawServer>,
}

#[derive(Debug, Deserialize)]
struct RawServer {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "enabled_default")]
    enabled: bool,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

fn enabled_default() -> bool {
    true
}

fn parse_file(path: &Path, warnings: &mut Vec<String>) -> Result<Vec<McpServerConfig>, McpError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(McpError::Config(format!(
                "读取 {} 失败：{error}",
                path.display()
            )));
        }
    };
    let raw: RawFile = serde_json::from_str(&text)
        .map_err(|error| McpError::Config(format!("解析 {} 失败：{error}", path.display())))?;

    let mut servers = Vec::new();
    for (name, server) in raw.servers {
        let kind = server
            .kind
            .as_deref()
            .unwrap_or("stdio")
            .to_ascii_lowercase();
        if matches!(kind.as_str(), "http" | "sse")
            || (server.command.is_none() && server.url.is_some())
        {
            warnings.push(format!(
                "忽略 MCP server `{name}`：仅支持 stdio command，HTTP/SSE 尚未实现（{}）",
                path.display()
            ));
            continue;
        }
        let Some(command) = server
            .command
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty())
        else {
            return Err(McpError::Config(format!(
                "MCP server `{name}` 缺少 command（{}）",
                path.display()
            )));
        };
        servers.push(McpServerConfig {
            name,
            command,
            args: server.args,
            env: server.env,
            enabled: server.enabled,
        });
    }
    Ok(servers)
}

/// 项目级同名项整体覆盖用户级项；其余保持「用户在前、项目追加」。
fn merge_servers(
    mut user: Vec<McpServerConfig>,
    project: Vec<McpServerConfig>,
) -> Vec<McpServerConfig> {
    for server in project {
        match user
            .iter_mut()
            .find(|existing| existing.name == server.name)
        {
            Some(existing) => *existing = server,
            None => user.push(server),
        }
    }
    user
}

/// 便捷入口：直接给出用户配置路径（测试/嵌入方使用）。
pub fn user_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(APP_DIR_NAME).join(USER_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lc-mcp-config-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write(path: &Path, value: serde_json::Value) {
        std::fs::write(path, value.to_string()).expect("write config");
    }

    #[test]
    fn parses_stdio_server_with_defaults() {
        let dir = temp_dir("parse");
        let project = dir.join(PROJECT_FILE_NAME);
        write(
            &project,
            json!({"mcpServers": {"filesystem": {
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
                "env": {"KEY": "VALUE"}
            }}}),
        );

        let config = McpConfig::load_with_user_file(&project, None).expect("load");
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.servers.len(), 1);
        let server = &config.servers[0];
        assert_eq!(server.name, "filesystem");
        assert_eq!(server.command, "npx");
        assert_eq!(server.args.len(), 3);
        assert_eq!(server.env.get("KEY").map(String::as_str), Some("VALUE"));
        assert!(server.enabled);
        assert_eq!(config.enabled_servers().count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_overrides_user_server() {
        let dir = temp_dir("override");
        let user = dir.join(USER_FILE_NAME);
        let project = dir.join(PROJECT_FILE_NAME);
        write(
            &user,
            json!({"mcpServers": {
                "shared": {"command": "user-cmd"},
                "user-only": {"command": "keep"}
            }}),
        );
        write(
            &project,
            json!({"mcpServers": {"shared": {"command": "project-cmd", "args": ["--x"]}}}),
        );

        let config = McpConfig::load_with_user_file(&project, Some(&user)).expect("load");
        assert_eq!(config.servers.len(), 2);
        let shared = config.get("shared").expect("shared");
        assert_eq!(shared.command, "project-cmd");
        assert_eq!(shared.args, ["--x"]);
        assert_eq!(
            config.get("user-only").map(|s| s.command.as_str()),
            Some("keep")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_server_is_retained_but_not_connected() {
        let dir = temp_dir("disabled");
        let project = dir.join(PROJECT_FILE_NAME);
        write(
            &project,
            json!({"mcpServers": {"off": {"command": "x", "enabled": false}}}),
        );

        let config = McpConfig::load_with_user_file(&project, None).expect("load");
        assert_eq!(config.servers.len(), 1);
        assert!(!config.servers[0].enabled);
        assert_eq!(config.enabled_servers().count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn http_and_sse_servers_are_ignored_with_warnings() {
        let dir = temp_dir("http");
        let project = dir.join(PROJECT_FILE_NAME);
        write(
            &project,
            json!({"mcpServers": {
                "remote-http": {"type": "http", "url": "https://example.com/mcp"},
                "remote-sse": {"type": "sse", "url": "https://example.com/sse"},
                "stdio": {"command": "node", "args": ["server.js"]}
            }}),
        );

        let config = McpConfig::load_with_user_file(&project, None).expect("load");
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].name, "stdio");
        assert_eq!(config.warnings.len(), 2);
        assert!(config.warnings.iter().all(|w| w.contains("HTTP/SSE")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_command_is_rejected() {
        let dir = temp_dir("no-command");
        let project = dir.join(PROJECT_FILE_NAME);
        write(&project, json!({"mcpServers": {"broken": {}}}));

        let error = McpConfig::load_with_user_file(&project, None).expect_err("must reject");
        assert!(matches!(error, McpError::Config(_)));
        assert!(error.to_string().contains("command"), "{error}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_yield_empty_config() {
        let dir = temp_dir("missing");
        let config = McpConfig::load_with_user_file(
            &dir.join("nope.json"),
            Some(&dir.join("also-nope.json")),
        )
        .expect("load");
        assert!(config.servers.is_empty());
        assert!(config.warnings.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_json_is_reported() {
        let dir = temp_dir("invalid");
        let project = dir.join(PROJECT_FILE_NAME);
        std::fs::write(&project, "{ not json").expect("write");
        let error = McpConfig::load_with_user_file(&project, None).expect_err("must reject");
        assert!(matches!(error, McpError::Config(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
