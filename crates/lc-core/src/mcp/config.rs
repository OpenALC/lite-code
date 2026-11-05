//! MCP 配置（Claude Code 兼容格式）。
//!
//! 安全默认下只读取用户级配置：
//! - `~/.lite-code/mcp.json`（始终读取）
//! - `<root>/.mcp.json`（默认忽略；仅当 `LC_MCP_ALLOW_PROJECT=1` 时读取）
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
//! 项目级配置是不可信输入：同名 server 不覆盖用户级条目（冲突写入
//! [`McpConfig::warnings`]），文件超过 [`MAX_PROJECT_FILE_BYTES`] 时整体忽略。
//! 只支持 `command` 型 stdio server。`type` 为 `http`/`sse` 或只给了 `url`
//! 的条目会被显式忽略并写入 warning——不静默假装支持。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::APP_DIR_NAME;

use super::McpError;

/// 项目级配置文件名（放在工作区根目录）。
pub const PROJECT_FILE_NAME: &str = ".mcp.json";
/// 用户级配置文件名（放在 `~/.lite-code/`）。
pub const USER_FILE_NAME: &str = "mcp.json";
/// 显式启用项目级配置的环境变量；只有值为 `1` 时生效。
pub const ALLOW_PROJECT_ENV: &str = "LC_MCP_ALLOW_PROJECT";
/// 默认忽略项目级配置时写入的 warning。
pub const PROJECT_IGNORED_WARNING: &str =
    "项目 .mcp.json 已忽略；设置 LC_MCP_ALLOW_PROJECT=1 显式启用";
/// 项目级配置文件的字节上限（1 MiB）。
pub const MAX_PROJECT_FILE_BYTES: u64 = 1024 * 1024;
/// 单次会话最多连接的 MCP server 数量。
pub const MAX_SERVERS: usize = 8;

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
    /// 用户配置在前；项目配置只追加用户级没有的同名项（含 disabled）。
    pub servers: Vec<McpServerConfig>,
    /// 被忽略的 HTTP/SSE 条目、项目配置冲突、无法定位 home 等非致命问题。
    pub warnings: Vec<String>,
}

impl McpConfig {
    /// 读取用户配置；项目配置仅在 `LC_MCP_ALLOW_PROJECT=1` 时读取。
    /// 文件不存在视为空配置。
    pub fn load(root: &Path) -> Result<Self, McpError> {
        let user_path = dirs::home_dir().map(|home| home.join(APP_DIR_NAME).join(USER_FILE_NAME));
        let mut config = Self::load_with_paths(
            &root.join(PROJECT_FILE_NAME),
            user_path.as_deref(),
            project_config_allowed(),
        )?;
        if user_path.is_none() {
            config
                .warnings
                .push("无法定位用户主目录，跳过 ~/.lite-code/mcp.json".into());
        }
        Ok(config)
    }

    /// 显式指定两个配置路径与是否允许项目配置（测试与自定义布局使用）。
    /// `user_path` 为 `None` 时没有用户级配置。项目级同名 server 不覆盖
    /// 用户级条目，冲突会写入 warning。
    pub fn load_with_paths(
        project_path: &Path,
        user_path: Option<&Path>,
        allow_project: bool,
    ) -> Result<Self, McpError> {
        let mut warnings = Vec::new();
        let user = match user_path {
            Some(path) => parse_file(path, None, &mut warnings)?,
            None => Vec::new(),
        };
        let project = if allow_project {
            parse_file(project_path, Some(MAX_PROJECT_FILE_BYTES), &mut warnings)?
        } else {
            if project_path.exists() {
                warnings.push(PROJECT_IGNORED_WARNING.into());
            }
            Vec::new()
        };
        Ok(Self {
            servers: merge_servers(user, project, &mut warnings),
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

    /// 加载期 warning（HTTP/SSE 被忽略、项目配置冲突、home 不可得等）。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 按名字查找（含 disabled），便于上层展示连接状态。
    pub fn get(&self, name: &str) -> Option<&McpServerConfig> {
        self.servers.iter().find(|server| server.name == name)
    }
}

/// 项目级配置是否被显式启用。只接受 `LC_MCP_ALLOW_PROJECT=1`（允许首尾空白）。
pub fn project_config_allowed() -> bool {
    project_config_allowed_value(std::env::var(ALLOW_PROJECT_ENV).ok().as_deref())
}

fn project_config_allowed_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim() == "1")
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

enum ConfigBytes {
    Missing,
    TooLarge,
    Present(Vec<u8>),
}

fn parse_file(
    path: &Path,
    max_bytes: Option<u64>,
    warnings: &mut Vec<String>,
) -> Result<Vec<McpServerConfig>, McpError> {
    let bytes = match read_config_bytes(path, max_bytes)? {
        ConfigBytes::Missing => return Ok(Vec::new()),
        ConfigBytes::TooLarge => {
            warnings.push(format!(
                "项目 MCP 配置 {} 超过 1 MiB 上限，已忽略",
                path.display()
            ));
            return Ok(Vec::new());
        }
        ConfigBytes::Present(bytes) => bytes,
    };
    let raw: RawFile = serde_json::from_slice(&bytes)
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

/// 有上限时最多读取 `max + 1` 字节，避免为超大文件分配内存。
fn read_config_bytes(path: &Path, max_bytes: Option<u64>) -> Result<ConfigBytes, McpError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigBytes::Missing);
        }
        Err(error) => {
            return Err(McpError::Config(format!(
                "读取 {} 失败：{error}",
                path.display()
            )));
        }
    };

    let mut bytes = Vec::new();
    let read = match max_bytes {
        Some(max) => file.take(max.saturating_add(1)).read_to_end(&mut bytes),
        None => file.read_to_end(&mut bytes),
    };
    read.map_err(|error| McpError::Config(format!("读取 {} 失败：{error}", path.display())))?;

    if max_bytes.is_some_and(|max| bytes.len() as u64 > max) {
        return Ok(ConfigBytes::TooLarge);
    }
    Ok(ConfigBytes::Present(bytes))
}

/// 用户级条目优先：项目级同名项被拒绝，其余按「用户在前、项目追加」合并。
fn merge_servers(
    mut user: Vec<McpServerConfig>,
    project: Vec<McpServerConfig>,
    warnings: &mut Vec<String>,
) -> Vec<McpServerConfig> {
    for server in project {
        if user.iter().any(|existing| existing.name == server.name) {
            warnings.push(format!(
                "MCP server `{}` 在项目 .mcp.json 中与用户配置重名，保留用户配置（拒绝项目覆盖）",
                server.name
            ));
            continue;
        }
        user.push(server);
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
    use crate::mcp::host::McpHost;
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

    /// 一个被 spawn 就会留下 marker 的命令；用于证明默认路径没有启动它。
    #[cfg(windows)]
    fn marker_command(dir: &Path) -> (String, Vec<String>) {
        std::fs::write(
            dir.join("spawn_probe.cmd"),
            "@echo off\r\necho spawned> project-spawned.marker\r\n",
        )
        .expect("write spawn probe");
        (
            "cmd".to_string(),
            vec!["/C".to_string(), "spawn_probe.cmd".to_string()],
        )
    }

    #[cfg(unix)]
    fn marker_command(dir: &Path) -> (String, Vec<String>) {
        std::fs::write(
            dir.join("spawn_probe.sh"),
            "#!/bin/sh\necho spawned > project-spawned.marker\n",
        )
        .expect("write spawn probe");
        ("sh".to_string(), vec!["spawn_probe.sh".to_string()])
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

        let config = McpConfig::load_with_paths(&project, None, true).expect("load");
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
    fn project_config_is_ignored_by_default_but_user_config_loads() {
        let dir = temp_dir("default-ignore");
        let user = dir.join(USER_FILE_NAME);
        let project = dir.join(PROJECT_FILE_NAME);
        let marker = dir.join("project-spawned.marker");
        let (command, args) = marker_command(&dir);
        write(
            &user,
            json!({"mcpServers": {"user-only": {"command": "user-cmd", "enabled": false}}}),
        );
        write(
            &project,
            json!({"mcpServers": {"probe": {"command": command, "args": args}}}),
        );

        let config = McpConfig::load_with_paths(&project, Some(&user), false).expect("load");
        assert_eq!(config.servers.len(), 1);
        assert!(config.get("probe").is_none());
        assert_eq!(
            config
                .get("user-only")
                .map(|server| server.command.as_str()),
            Some("user-cmd")
        );
        assert!(
            config
                .warnings
                .iter()
                .any(|warning| warning == PROJECT_IGNORED_WARNING),
            "{:?}",
            config.warnings
        );

        let host = McpHost::connect(&config, &dir);
        assert!(host.tool_specs().is_empty());
        assert!(!marker.exists(), "默认不得 spawn 项目 .mcp.json 中的命令");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicitly_enabled_project_config_is_loaded() {
        let dir = temp_dir("allow-project");
        let project = dir.join(PROJECT_FILE_NAME);
        let marker = dir.join("project-spawned.marker");
        let (command, args) = marker_command(&dir);
        write(
            &project,
            json!({"mcpServers": {"probe": {"command": command, "args": args}}}),
        );

        let config = McpConfig::load_with_paths(&project, None, true).expect("load");
        assert_eq!(config.servers.len(), 1);
        assert!(config.get("probe").is_some());
        assert!(
            !config
                .warnings
                .iter()
                .any(|warning| warning == PROJECT_IGNORED_WARNING),
            "{:?}",
            config.warnings
        );
        assert!(!marker.exists(), "只加载配置不应立刻 spawn");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_does_not_override_user_server() {
        let dir = temp_dir("no-override");
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
            json!({"mcpServers": {
                "shared": {"command": "project-cmd", "args": ["--x"]},
                "project-only": {"command": "added"}
            }}),
        );

        let config = McpConfig::load_with_paths(&project, Some(&user), true).expect("load");
        assert_eq!(config.servers.len(), 3);
        let shared = config.get("shared").expect("shared");
        assert_eq!(shared.command, "user-cmd");
        assert!(shared.args.is_empty());
        assert_eq!(
            config.get("user-only").map(|s| s.command.as_str()),
            Some("keep")
        );
        assert_eq!(
            config.get("project-only").map(|s| s.command.as_str()),
            Some("added")
        );
        assert!(
            config
                .warnings
                .iter()
                .any(|warning| warning.contains("重名") && warning.contains("保留用户配置")),
            "{:?}",
            config.warnings
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_project_config_is_ignored() {
        let dir = temp_dir("too-large");
        let project = dir.join(PROJECT_FILE_NAME);
        let size = usize::try_from(MAX_PROJECT_FILE_BYTES).expect("limit fits usize") + 1;
        std::fs::write(&project, vec![b'x'; size]).expect("write oversized config");

        let config = McpConfig::load_with_paths(&project, None, true).expect("load");
        assert!(config.servers.is_empty());
        assert!(
            config
                .warnings
                .iter()
                .any(|warning| warning.contains("1 MiB") && warning.contains("已忽略")),
            "{:?}",
            config.warnings
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_switch_requires_exactly_one() {
        assert!(project_config_allowed_value(Some("1")));
        assert!(project_config_allowed_value(Some(" 1 ")));
        assert!(!project_config_allowed_value(Some("true")));
        assert!(!project_config_allowed_value(Some("0")));
        assert!(!project_config_allowed_value(Some("")));
        assert!(!project_config_allowed_value(None));
    }

    #[test]
    fn disabled_server_is_retained_but_not_connected() {
        let dir = temp_dir("disabled");
        let project = dir.join(PROJECT_FILE_NAME);
        write(
            &project,
            json!({"mcpServers": {"off": {"command": "x", "enabled": false}}}),
        );

        let config = McpConfig::load_with_paths(&project, None, true).expect("load");
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

        let config = McpConfig::load_with_paths(&project, None, true).expect("load");
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

        let error = McpConfig::load_with_paths(&project, None, true).expect_err("must reject");
        assert!(matches!(error, McpError::Config(_)));
        assert!(error.to_string().contains("command"), "{error}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_yield_empty_config() {
        let dir = temp_dir("missing");
        let config = McpConfig::load_with_paths(
            &dir.join("nope.json"),
            Some(&dir.join("also-nope.json")),
            true,
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
        let error = McpConfig::load_with_paths(&project, None, true).expect_err("must reject");
        assert!(matches!(error, McpError::Config(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
