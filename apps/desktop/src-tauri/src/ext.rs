// 插件系统 Tauri 命令层：只做转发，业务逻辑在 `lc_core::ext`。
// 命令由 `run()` 的 `generate_handler!` 注册。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use lc_core::ext::{ExtError, ExtHost, ExtPaths, PluginList};
use lc_proto::{CommandResult, ErrorBody, ErrorCode};
use serde::Serialize;
use serde_json::Value;

use crate::WorkspaceState;

/// `ext_execute_command` 的返回：`{ "result": <插件返回值> }`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtCommandResult {
    pub result: Value,
}

/// 单宿主进程常驻：所有命令共享同一个 [`ExtHost`]，首次使用时探测 Node。
fn shared_host() -> &'static Mutex<ExtHost> {
    static HOST: OnceLock<Mutex<ExtHost>> = OnceLock::new();
    HOST.get_or_init(|| Mutex::new(ExtHost::detect()))
}

fn lock_host() -> Result<MutexGuard<'static, ExtHost>, ErrorBody> {
    shared_host()
        .lock()
        .map_err(|_| ErrorBody::new(ErrorCode::Internal, "插件宿主锁中毒，请重启应用"))
}

fn reset_host() -> Result<(), ErrorBody> {
    lock_host()?.reset();
    Ok(())
}

fn host_node_path() -> Result<Option<PathBuf>, ErrorBody> {
    let host = lock_host()?;
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
            let mut host = lock_host()?;
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
        let mut host = lock_host()?;
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
}
