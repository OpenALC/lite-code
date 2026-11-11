// 插件系统 Tauri 命令层：只做转发，业务逻辑在 `lc_core::ext`。
// 命令由 `run()` 的 `generate_handler!` 注册。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use lc_core::ext::marketplace::{
    BUILTIN_SOURCE_NAME, MarketSearchResult, RegistryList, RegistrySource,
};
use lc_core::ext::{ExtError, ExtHost, ExtPaths, PluginList};
use lc_proto::{CommandResult, ErrorBody, ErrorCode};
use serde::Serialize;
use serde_json::Value;

use crate::WorkspaceState;

/// 默认插件源地址：官方 marketplace 仓库的 release 资产（占位常量，发布前
/// 落定；同 updater 的 DEFAULT_MANIFEST_URL 模式，
/// docs/plans/plugin-marketplace.md §3.3/§7）。
pub(crate) const DEFAULT_REGISTRY_URL: &str =
    "https://github.com/OpenALC/lite-code-marketplace/releases/download/plugin-index/latest.json";

/// 源 URL 覆盖环境变量（测试缝，同 LC_UPDATE_MANIFEST_URL）。
pub(crate) const REGISTRY_URL_ENV: &str = "LC_PLUGIN_REGISTRY_URL";

/// 源 URL：显式环境变量优先，否则用固定滚动地址。
pub(crate) fn registry_url() -> String {
    std::env::var(REGISTRY_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string())
}

/// 内置官方源（桌面层持有常量，lc-core 侧可同名覆盖/禁用）。
pub(crate) fn builtin_registry_source() -> RegistrySource {
    RegistrySource::new(BUILTIN_SOURCE_NAME, registry_url())
}

/// `ext_execute_command` 的返回：`{ "result": <插件返回值> }`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtCommandResult {
    pub result: Value,
}

/// `ext_read_theme` 的返回：`{ "json": <主题文件内容> }`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtThemeResult {
    pub json: Value,
}

/// `ext_read_grammar` 的返回：`{ "json": <语法文件内容> }`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtGrammarResult {
    pub json: Value,
}

/// `ext_view_children` 的返回：`{ "nodes": [<视图节点>] }`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtViewChildrenResult {
    pub nodes: Vec<Value>,
}

/// `ext_ui_values` 的返回：`{ "values": <节归一值 JSON> }`（list→`{ rows }`、
/// kv→`{ items }`，宿主已按节声明归一；markdown/form 节 v1 无回读）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtUiValuesResult {
    pub values: Value,
}

/// 单宿主进程常驻：所有命令与 Agent 会话共享同一个 [`ExtHost`]，
/// 首次使用时探测 Node。返回 Arc 以便 Agent worker 的工具执行器长期持有。
pub(crate) fn shared_host() -> Arc<Mutex<ExtHost>> {
    static HOST: OnceLock<Arc<Mutex<ExtHost>>> = OnceLock::new();
    HOST.get_or_init(|| Arc::new(Mutex::new(ExtHost::detect())))
        .clone()
}

fn host_lock_error() -> ErrorBody {
    ErrorBody::new(ErrorCode::Internal, "插件宿主锁中毒，请重启应用")
}

fn reset_host() -> Result<(), ErrorBody> {
    shared_host().lock().map_err(|_| host_lock_error())?.reset();
    Ok(())
}

fn host_node_path() -> Result<Option<PathBuf>, ErrorBody> {
    let host = shared_host();
    let host = host.lock().map_err(|_| host_lock_error())?;
    Ok(host.node_path().map(Path::to_path_buf))
}

/// 宿主/侧车调用可能持续数十秒，一律放到阻塞线程池，不卡 Tauri 主线程。
async fn blocking<T>(
    action: impl FnOnce() -> Result<T, ErrorBody> + Send + 'static,
) -> Result<T, ErrorBody>
where
    T: Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(action).await {
        Ok(result) => result,
        Err(error) => Err(ErrorBody::new(
            ErrorCode::Internal,
            format!("插件命令线程失败：{error}"),
        )),
    }
}

#[tauri::command]
pub(crate) async fn ext_list(
    state: tauri::State<'_, WorkspaceState>,
) -> Result<PluginList, ErrorBody> {
    let root = state.get();
    let node = host_node_path()?;
    blocking(move || {
        lc_core::ext::list_plugins_with_node(root.as_deref(), node.as_deref())
            .map_err(|error| error.to_error_body())
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_set_enabled(
    id: String,
    enabled: bool,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<PluginList, ErrorBody> {
    let root = state.get();
    let node = host_node_path()?;
    blocking(move || {
        let list =
            lc_core::ext::set_enabled_with_node(root.as_deref(), &id, enabled, node.as_deref())
                .map_err(|error| error.to_error_body())?;
        reset_host()?;
        Ok(list)
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_reload(
    state: tauri::State<'_, WorkspaceState>,
) -> Result<PluginList, ErrorBody> {
    let root = state.get();
    blocking(move || {
        let node = {
            let host = shared_host();
            let mut host = host.lock().map_err(|_| host_lock_error())?;
            host.refresh_node();
            host.node_path().map(Path::to_path_buf)
        };
        lc_core::ext::list_plugins_with_node(root.as_deref(), node.as_deref())
            .map_err(|error| error.to_error_body())
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_execute_command(
    command: String,
    args: Option<Value>,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<ExtCommandResult, ErrorBody> {
    let root = state.get().ok_or_else(crate::workspace_unavailable)?;
    blocking(move || {
        let host = shared_host();
        let mut host = host.lock().map_err(|_| host_lock_error())?;
        let result = host
            .execute(
                &root,
                &command,
                args.unwrap_or_else(|| serde_json::json!({})),
            )
            .map_err(|error| error.to_error_body())?;
        Ok(ExtCommandResult { result })
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_read_theme(
    plugin_id: String,
    path: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<ExtThemeResult, ErrorBody> {
    let root = state.get();
    blocking(move || {
        let json = lc_core::ext::read_theme(root.as_deref(), &plugin_id, &path)
            .map_err(|error| error.to_error_body())?;
        Ok(ExtThemeResult { json })
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_read_grammar(
    plugin_id: String,
    path: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<ExtGrammarResult, ErrorBody> {
    let root = state.get();
    blocking(move || {
        let json = lc_core::ext::read_grammar(root.as_deref(), &plugin_id, &path)
            .map_err(|error| error.to_error_body())?;
        Ok(ExtGrammarResult { json })
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_view_children(
    plugin_id: String,
    view_id: String,
    node_id: Option<String>,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<ExtViewChildrenResult, ErrorBody> {
    let root = state.get().ok_or_else(crate::workspace_unavailable)?;
    blocking(move || {
        let host = shared_host();
        let mut host = host.lock().map_err(|_| host_lock_error())?;
        let value = host
            .view_children(&root, &plugin_id, &view_id, node_id.as_deref())
            .map_err(|error| error.to_error_body())?;
        let nodes = value.as_array().cloned().ok_or_else(|| {
            ErrorBody::new(
                ErrorCode::Internal,
                "ext_view_children 返回了非预期载荷（缺少节点数组）",
            )
        })?;
        Ok(ExtViewChildrenResult { nodes })
    })
    .await
}

/// 读取插件 UI 视图的节值（docs/plans/plugin-webview.md §3.3）：薄转发
/// `ExtHost::ui_values`。`section_id` 为清单稳定节 id（M2-R，不再是数组
/// 序号）；视图未声明/插件禁用/非 node runtime → not_found；节不存在 →
/// not_found；节非 `source:"provider"` → invalid-values。错误经
/// `to_error_body` 复用既有 ErrorCode 映射，不新增错误码；鸿蒙入口隐藏。
#[tauri::command]
pub(crate) async fn ext_ui_values(
    plugin_id: String,
    view_id: String,
    section_id: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<ExtUiValuesResult, ErrorBody> {
    let root = state.get().ok_or_else(crate::workspace_unavailable)?;
    blocking(move || {
        let host = shared_host();
        let mut host = host.lock().map_err(|_| host_lock_error())?;
        let values = host
            .ui_values(&root, &plugin_id, &view_id, &section_id)
            .map_err(|error| error.to_error_body())?;
        Ok(ExtUiValuesResult { values })
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_open_dir() -> Result<CommandResult, ErrorBody> {
    blocking(|| {
        let paths = ExtPaths::default_for(None).map_err(|error| error.to_error_body())?;
        let dir = ensure_extensions_dir(&paths).map_err(|error| error.to_error_body())?;
        open_dir(&dir).map_err(|error| {
            ErrorBody::new(
                ErrorCode::Io,
                format!("打开插件目录 {} 失败：{error}", dir.display()),
            )
        })?;
        Ok(CommandResult::Ack)
    })
    .await
}

// ==== 插件市场命令（docs/plans/plugin-marketplace.md §3.4）：薄转发，业务在
// `lc_core::ext::marketplace`，DTO camelCase，错误码复用既有 ErrorCode。 ====

#[tauri::command]
pub(crate) async fn ext_registry_list() -> Result<RegistryList, ErrorBody> {
    let builtin = builtin_registry_source();
    blocking(move || {
        lc_core::ext::marketplace::list_registries(&builtin).map_err(|error| error.to_error_body())
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_registry_add(
    name: String,
    url: String,
    require_signatures: Option<bool>,
    pubkey: Option<String>,
    mirrors: Option<Vec<String>>,
) -> Result<RegistryList, ErrorBody> {
    let builtin = builtin_registry_source();
    blocking(move || {
        lc_core::ext::marketplace::add_registry(
            &builtin,
            &name,
            &url,
            require_signatures.unwrap_or(false),
            pubkey.as_deref(),
            mirrors.as_deref(),
        )
        .map_err(|error| error.to_error_body())
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_registry_remove(name: String) -> Result<RegistryList, ErrorBody> {
    let builtin = builtin_registry_source();
    blocking(move || {
        lc_core::ext::marketplace::remove_registry(&builtin, &name)
            .map_err(|error| error.to_error_body())
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_search(
    query: Option<String>,
    refresh: Option<bool>,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<MarketSearchResult, ErrorBody> {
    let root = state.get();
    let node = host_node_path()?;
    let builtin = builtin_registry_source();
    blocking(move || {
        let paths =
            ExtPaths::default_for(root.as_deref()).map_err(|error| error.to_error_body())?;
        lc_core::ext::marketplace::search(
            &paths,
            &builtin,
            query.as_deref(),
            refresh.unwrap_or(false),
            node.is_some(),
        )
        .map_err(|error| error.to_error_body())
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_install(
    id: String,
    version: Option<String>,
    source: Option<String>,
    force: Option<bool>,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<PluginList, ErrorBody> {
    let root = state.get();
    let node = host_node_path()?;
    let builtin = builtin_registry_source();
    blocking(move || {
        let paths =
            ExtPaths::default_for(root.as_deref()).map_err(|error| error.to_error_body())?;
        lc_core::ext::marketplace::install(
            &paths,
            &builtin,
            &id,
            version.as_deref(),
            source.as_deref(),
            force.unwrap_or(false),
            node.is_some(),
        )
        .map_err(|error| error.to_error_body())
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_update(
    id: Option<String>,
    force: Option<bool>,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<PluginList, ErrorBody> {
    let root = state.get();
    let node = host_node_path()?;
    let builtin = builtin_registry_source();
    blocking(move || {
        let paths =
            ExtPaths::default_for(root.as_deref()).map_err(|error| error.to_error_body())?;
        lc_core::ext::marketplace::update(
            &paths,
            &builtin,
            id.as_deref(),
            force.unwrap_or(false),
            node.is_some(),
        )
        .map_err(|error| error.to_error_body())
    })
    .await
}

#[tauri::command]
pub(crate) async fn ext_uninstall(
    id: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<PluginList, ErrorBody> {
    let root = state.get();
    let node = host_node_path()?;
    blocking(move || {
        let paths =
            ExtPaths::default_for(root.as_deref()).map_err(|error| error.to_error_body())?;
        lc_core::ext::marketplace::uninstall(&paths, &id, node.is_some())
            .map_err(|error| error.to_error_body())
    })
    .await
}

fn ensure_extensions_dir(paths: &ExtPaths) -> Result<PathBuf, ExtError> {
    std::fs::create_dir_all(&paths.user_dir).map_err(|error| {
        ExtError::Io(format!(
            "创建插件目录 {} 失败：{error}",
            paths.user_dir.display()
        ))
    })?;
    Ok(paths.user_dir.clone())
}

fn open_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        command.arg(path);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(path);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        command
    };
    command.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lc-desktop-ext-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn command_result_serializes_result_field() {
        let value = serde_json::to_value(ExtCommandResult {
            result: json!({"ok": true}),
        })
        .expect("serialize");
        assert_eq!(value["result"]["ok"], true);
    }

    #[test]
    fn theme_result_serializes_json_field() {
        let value = serde_json::to_value(ExtThemeResult {
            json: json!({"name": "Dusk", "colors": {"--bg": "#101014"}}),
        })
        .expect("serialize");
        assert_eq!(value["json"]["name"], "Dusk");
        assert_eq!(value["json"]["colors"]["--bg"], "#101014");
    }

    #[test]
    fn grammar_result_serializes_json_field() {
        let value = serde_json::to_value(ExtGrammarResult {
            json: json!({"tokenizer": {"root": [["\\w+", "keyword"]]}}),
        })
        .expect("serialize");
        assert_eq!(value["json"]["tokenizer"]["root"][0][1], "keyword");
    }

    #[test]
    fn view_children_result_serializes_nodes_field() {
        let value = serde_json::to_value(ExtViewChildrenResult {
            nodes: vec![json!({"id": "n1", "label": "Node 1"})],
        })
        .expect("serialize");
        assert_eq!(value["nodes"][0]["id"], "n1");
        assert_eq!(value["nodes"][0]["label"], "Node 1");
    }

    #[test]
    fn ui_values_result_serializes_values_field() {
        let value = serde_json::to_value(ExtUiValuesResult {
            values: json!({"rows": [{"title": "t1"}]}),
        })
        .expect("serialize");
        assert_eq!(value["values"]["rows"][0]["title"], "t1");
    }

    #[test]
    fn ensure_extensions_dir_creates_directory() {
        let temp = temp_dir("open-dir");
        let paths = ExtPaths::new(temp.join("extensions"), None, temp.join("extensions.json"));
        let dir = ensure_extensions_dir(&paths).expect("create dir");
        assert!(dir.is_dir());
        assert_eq!(dir, paths.user_dir);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn ext_error_maps_to_contract_error_body() {
        let body = ExtError::NodeUnavailable.to_error_body();
        assert_eq!(body.code, ErrorCode::NodeUnavailable);
        assert!(body.message.contains("Node"));
    }

    #[test]
    fn registry_url_env_overrides_builtin_default() {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old = std::env::var_os(REGISTRY_URL_ENV);
        // SAFETY: 测试进程内临时覆盖 URL 环境变量，同一把 ENV_LOCK 串行化，
        // 断言结束立刻恢复。
        unsafe {
            std::env::set_var(REGISTRY_URL_ENV, "file:///tmp/test-index.json");
        }
        assert_eq!(registry_url(), "file:///tmp/test-index.json");
        let source = builtin_registry_source();
        assert_eq!(source.name, BUILTIN_SOURCE_NAME);
        assert_eq!(source.url, "file:///tmp/test-index.json");
        assert!(source.enabled);
        unsafe {
            match old {
                Some(value) => std::env::set_var(REGISTRY_URL_ENV, value),
                None => std::env::remove_var(REGISTRY_URL_ENV),
            }
        }
        assert_eq!(registry_url(), DEFAULT_REGISTRY_URL);
        assert!(DEFAULT_REGISTRY_URL.starts_with("https://"));
    }
}
