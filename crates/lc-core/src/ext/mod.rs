//! 插件系统（v0）：清单发现、启用状态、Node/WASM 命令执行与 MCP 声明合并。
//!
//! 契约见 docs/EXTENSIONS.md。本 lane 不把插件代码放进 Tauri 渲染进程：
//! - `node` 插件在常驻扩展宿主中执行（stdio 行分隔 JSON-RPC）；
//! - `wasm` 每次命令启动一个 `wasm-host.mjs` 子进程；
//! - `declarative` 只读取 `contributes`，命令执行返回未实现。
//!
//! 安全边界（docs/EXTENSIONS.md §7）：
//! - 用户目录始终读取；项目级 `<root>/.lite-code/extensions` 仅在
//!   `LC_EXT_ALLOW_PROJECT=1` 时读取；
//! - 清单 `main` 必须 canonicalize 后落在插件目录内；
//! - 宿主与 WASM 子进程都走 [`crate::process::scrub_secret_env`] 风格的
//!   环境清理，不注入 provider 密钥。

mod host;
mod manifest;
mod mcp;
mod state;
mod wasm;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lc_proto::{ErrorBody, ErrorCode, NodeStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use host::ExtHost;
pub use mcp::plugin_mcp_servers;
pub use state::ExtensionsState;

/// 项目级插件目录的显式开关；只有值（去掉首尾空白）等于 `1` 时生效。
pub const PROJECT_ALLOW_ENV: &str = "LC_EXT_ALLOW_PROJECT";
/// 用户级插件目录名（`~/.lite-code/extensions`）。
pub const USER_EXTENSIONS_DIR: &str = "extensions";
/// 项目级插件目录（相对工作区根）。
pub const PROJECT_EXTENSIONS_REL: &str = ".lite-code/extensions";
/// 清单大小上限（256 KiB）。
pub const MAX_MANIFEST_BYTES: u64 = manifest::MAX_MANIFEST_BYTES;

/// 插件运行形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginRuntime {
    Node,
    Wasm,
    Declarative,
}

/// 插件声明的命令。`command` 是全局唯一 id，`title` 供 UI 展示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommand {
    pub command: String,
    #[serde(default)]
    pub title: String,
}

/// 单个插件的展示信息（`PluginList.plugins` 元素）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub runtime: PluginRuntime,
    pub enabled: bool,
    /// 插件目录绝对路径（展示用，已去掉 Windows verbatim 前缀）。
    pub path: String,
    /// 校验/加载错误；有效插件为 `null`。
    pub error: Option<String>,
    pub commands: Vec<PluginCommand>,
    pub mcp_servers: Vec<String>,
    pub language_servers: Vec<String>,
}

/// 插件列表（`ext_list` / `ext_reload` / `ext_set_enabled` 的返回）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginList {
    pub plugins: Vec<PluginInfo>,
    /// 用户级插件目录。
    pub dir: String,
    pub node_available: bool,
}

/// ext lane 的错误集合。每个变体都能映射到契约 [`ErrorCode`]，
/// 传输层包装成 [`ErrorBody`]。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtError {
    #[error("无法定位用户主目录")]
    NoHomeDir,
    #[error("插件配置错误：{0}")]
    Config(String),
    #[error("插件文件读写失败：{0}")]
    Io(String),
    #[error("插件资源不可用：{0}")]
    AssetMissing(String),
    #[error("插件 `{id}` 不存在")]
    UnknownPlugin { id: String },
    #[error("命令 `{command}` 不存在或所属插件已禁用")]
    CommandNotFound { command: String },
    #[error("Node 运行时不可用")]
    NodeUnavailable,
    #[error("扩展宿主启动失败：{0}")]
    HostSpawn(String),
    #[error("扩展宿主协议错误：{0}")]
    HostProtocol(String),
    #[error("扩展宿主进程已退出：{0}")]
    HostExited(String),
    #[error("扩展宿主请求超时（{timeout_ms} ms）")]
    HostTimeout { timeout_ms: u64 },
    #[error("扩展宿主返回 JSON-RPC 错误（code {code}）：{message}")]
    HostRpc {
        code: i64,
        message: String,
        data: Option<String>,
    },
    #[error("WASM 插件执行失败：{0}")]
    WasmFailed(String),
    #[error("WASM 插件执行超时（{timeout_ms} ms）")]
    WasmTimeout { timeout_ms: u64 },
    #[error("declarative 插件不执行代码")]
    DeclarativeNotExecutable,
}

impl ExtError {
    pub fn code(&self) -> ErrorCode {
        match self {
            ExtError::NoHomeDir | ExtError::Config(_) => ErrorCode::Internal,
            ExtError::Io(_) => ErrorCode::Io,
            ExtError::AssetMissing(_) => ErrorCode::NotFound,
            ExtError::UnknownPlugin { .. } | ExtError::CommandNotFound { .. } => {
                ErrorCode::NotFound
            }
            ExtError::NodeUnavailable => ErrorCode::NodeUnavailable,
            ExtError::HostSpawn(_) => ErrorCode::NodeBroken,
            ExtError::HostProtocol(_) | ExtError::HostExited(_) => ErrorCode::Internal,
            ExtError::HostTimeout { .. } => ErrorCode::Timeout,
            ExtError::HostRpc { code, .. } => match code {
                -32601 => ErrorCode::NotImplemented,
                -32600 | -32602 => ErrorCode::InvalidArgument,
                _ => ErrorCode::Internal,
            },
            ExtError::WasmFailed(_) => ErrorCode::Internal,
            ExtError::WasmTimeout { .. } => ErrorCode::Timeout,
            ExtError::DeclarativeNotExecutable => ErrorCode::NotImplemented,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        let body = ErrorBody::new(self.code(), self.to_string());
        match self {
            ExtError::HostRpc {
                data: Some(data), ..
            } => body.with_detail(data.clone()),
            _ => body,
        }
    }
}

/// 插件目录布局。测试与自定义安装位置可直接构造。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtPaths {
    pub user_dir: PathBuf,
    pub project_dir: Option<PathBuf>,
    pub state_path: PathBuf,
}

impl ExtPaths {
    /// 默认布局：`~/.lite-code/extensions`、`~/.lite-code/extensions.json`，
    /// 以及工作区根下的 `.lite-code/extensions`。
    pub fn default_for(workspace_root: Option<&Path>) -> Result<Self, ExtError> {
        let home = dirs::home_dir().ok_or(ExtError::NoHomeDir)?;
        let app_dir = home.join(crate::config::APP_DIR_NAME);
        Ok(Self {
            user_dir: app_dir.join(USER_EXTENSIONS_DIR),
            project_dir: workspace_root.map(|root| root.join(PROJECT_EXTENSIONS_REL)),
            state_path: app_dir.join("extensions.json"),
        })
    }

    pub fn new(user_dir: PathBuf, project_dir: Option<PathBuf>, state_path: PathBuf) -> Self {
        Self {
            user_dir,
            project_dir,
            state_path,
        }
    }
}

/// 真实探测 Node（docs/RUNTIME.md）。探测失败是合法的降级状态。
pub fn probe_node() -> Option<PathBuf> {
    match crate::runtime::probe_real() {
        NodeStatus::Ready { runtime } => Some(PathBuf::from(runtime.path)),
        NodeStatus::NeedManual { .. } => None,
    }
}

/// 项目级插件是否被显式启用。
pub fn project_allowed() -> bool {
    project_allowed_value(std::env::var(PROJECT_ALLOW_ENV).ok().as_deref())
}

fn project_allowed_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim() == "1")
}

/// 读取默认位置的插件列表（会探测 Node）。
pub fn list_plugins(workspace_root: Option<&Path>) -> Result<PluginList, ExtError> {
    let node = probe_node();
    list_plugins_with_node(workspace_root, node.as_deref())
}

/// 读取默认位置的插件列表，复用调用方已探测的 Node。
pub fn list_plugins_with_node(
    workspace_root: Option<&Path>,
    node: Option<&Path>,
) -> Result<PluginList, ExtError> {
    let paths = ExtPaths::default_for(workspace_root)?;
    list_plugins_at(&paths, project_allowed(), node.is_some())
}

/// 读取指定布局的插件列表（测试与嵌入方使用）。
pub fn list_plugins_at(
    paths: &ExtPaths,
    allow_project: bool,
    node_available: bool,
) -> Result<PluginList, ExtError> {
    let disabled = ExtensionsState::new(paths.state_path.clone()).load()?;
    let plugins = manifest::discover(paths, allow_project, &disabled)?;
    Ok(PluginList {
        plugins: plugins.into_iter().map(|plugin| plugin.info).collect(),
        dir: crate::fs::display_path(&paths.user_dir),
        node_available,
    })
}

/// 切换启用状态并返回新列表（会探测 Node）。
pub fn set_enabled(
    workspace_root: Option<&Path>,
    id: &str,
    enabled: bool,
) -> Result<PluginList, ExtError> {
    let node = probe_node();
    set_enabled_with_node(workspace_root, id, enabled, node.as_deref())
}

/// 切换启用状态并返回新列表，复用调用方已探测的 Node。
pub fn set_enabled_with_node(
    workspace_root: Option<&Path>,
    id: &str,
    enabled: bool,
    node: Option<&Path>,
) -> Result<PluginList, ExtError> {
    let paths = ExtPaths::default_for(workspace_root)?;
    set_enabled_at(&paths, project_allowed(), id, enabled, node.is_some())
}

/// 在指定布局下切换启用状态。
pub fn set_enabled_at(
    paths: &ExtPaths,
    allow_project: bool,
    id: &str,
    enabled: bool,
    node_available: bool,
) -> Result<PluginList, ExtError> {
    let state = ExtensionsState::new(paths.state_path.clone());
    let disabled = state.load()?;
    let plugins = manifest::discover(paths, allow_project, &disabled)?;
    if !plugins.iter().any(|plugin| plugin.info.id == id) {
        return Err(ExtError::UnknownPlugin { id: id.to_string() });
    }
    state.set_enabled(id, enabled)?;
    list_plugins_at(paths, allow_project, node_available)
}

/// 内置 sidecar 的定位/落盘：
/// 1. 环境变量覆盖（必须是存在的文件，canonicalize 成绝对路径）；
/// 2. 否则把 `include_str!` 资产写到 `<data_dir>/sidecars/<file_name>` 并复用。
pub(crate) fn resolve_asset(
    env_var: &str,
    file_name: &str,
    builtin: &str,
) -> Result<PathBuf, ExtError> {
    if let Some(raw) = std::env::var_os(env_var).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(raw);
        let canonical = std::fs::canonicalize(&path).map_err(|error| {
            ExtError::AssetMissing(format!(
                "{env_var} 指向的文件不可用：{}（{error}）",
                path.display()
            ))
        })?;
        // Node 不认 Windows verbatim 前缀；给子进程的脚本路径统一用展示路径。
        return Ok(PathBuf::from(crate::fs::display_path(&canonical)));
    }

    let data_dir = crate::config::data_dir()
        .map_err(|error| ExtError::Config(format!("无法定位数据目录：{error}")))?;
    let path = data_dir.join("sidecars").join(file_name);
    let existing = std::fs::read_to_string(&path).ok();
    if existing.as_deref() != Some(builtin) {
        crate::config::write_atomic(&path, builtin)
            .map_err(|error| ExtError::Io(format!("写入 {} 失败：{error}", path.display())))?;
    }
    Ok(path)
}

/// 已启用插件集合（供宿主/MCP 等内部调用）。
pub(crate) fn enabled_plugins(
    paths: &ExtPaths,
    allow_project: bool,
) -> Result<Vec<manifest::DiscoveredPlugin>, ExtError> {
    let disabled = ExtensionsState::new(paths.state_path.clone()).load()?;
    manifest::discover(paths, allow_project, &disabled)
}

/// 供测试与嵌入方检查的启用 id 集合。
pub fn disabled_plugins(paths: &ExtPaths) -> Result<BTreeSet<String>, ExtError> {
    ExtensionsState::new(paths.state_path.clone()).load()
}

/// 保留给调用方构造 `Json` 参数时使用，避免各自复制 `null → {}` 规则。
pub(crate) fn normalize_args(args: Value) -> Value {
    if args.is_null() {
        Value::Object(Default::default())
    } else {
        args
    }
}
