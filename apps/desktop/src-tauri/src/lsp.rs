//! LSP Tauri 命令层（docs/EXTENSIONS.md §3、§6）。
//!
//! 本模块只做三件事：`Mutex` 包装（[`LspState`]）、把 lc-core 的 LSP 错误映射成
//! 契约 `ErrorBody`、把诊断转成 `lsp:diagnostics` 事件。协议、进程与路由逻辑
//! 全部在 `lc_core::lsp`。
//!
//! ## 接线（由 `lib.rs` / ext 命令层完成，本模块不注册命令）
//!
//! 1. `.manage(lsp::LspState::new())`；
//! 2. `.invoke_handler(tauri::generate_handler![
//!    lsp::lsp_start_all, lsp::lsp_did_open, lsp::lsp_did_change,
//!    lsp::lsp_did_close, lsp::lsp_stop_all, ...])`；
//! 3. ext 清单加载后，把各插件 `contributes.languageServers` 转成
//!    `Vec<LspServerConfig>`，调用 `LspState::configure(configs)`（或等价的
//!    公开入口）。随后前端/主流程调用 `lsp_start_all`；命令不传 `configs`
//!    时使用 `configure` 暂存的值，传了则覆盖暂存值并以本次配置启动。
//!
//! 诊断事件载荷（camelCase）：
//! `{ "serverId": "...", "path": "...", "diagnostics": [{ "severity", "message",
//! "line", "column", "endLine", "endColumn" }] }`；空数组表示清除标记。
//!
//! 命令注册在 `lib.rs`；本模块不自行注册。
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use lc_core::lsp::{
    Diagnostic, DiagnosticSink, LspError, LspManager, LspServerConfig, LspStartReport,
};
use lc_proto::{CommandResult, ErrorBody, ErrorCode};
use serde::Serialize;
use tauri::Emitter;

use crate::{WorkspaceState, workspace_unavailable};

const LSP_DIAGNOSTICS_EVENT: &str = "lsp:diagnostics";
const LSP_STATE_UNAVAILABLE: &str = "LSP 状态不可用（锁中毒），请重启应用";

/// 应用级 LSP 状态。内部 `Arc<Mutex<..>>` 便于 `lsp_start_all` /
/// `lsp_stop_all` 把可能阻塞的 spawn / shutdown 移到 `spawn_blocking`。
#[derive(Default, Clone)]
pub(crate) struct LspState {
    inner: Arc<Mutex<LspStateInner>>,
}

#[derive(Default)]
struct LspStateInner {
    manager: LspManager,
    /// ext 清单转换出的待启动配置；`lsp_start_all(Some(configs))` 会覆盖它。
    configs: Vec<LspServerConfig>,
}

impl LspState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 暂存插件清单中的 language server 配置（不启动）。供 ext 命令层在
    /// 加载/刷新插件后调用；启动由 `lsp_start_all` 触发。
    // 由 main/ext 接线使用；接线完成前允许暂未引用。
    #[allow(dead_code)]
    pub(crate) fn configure(&self, configs: Vec<LspServerConfig>) -> Result<(), ErrorBody> {
        self.lock()?.configs = configs;
        Ok(())
    }

    /// 当前暂存配置的快照。
    #[allow(dead_code)]
    pub(crate) fn configured(&self) -> Result<Vec<LspServerConfig>, ErrorBody> {
        Ok(self.lock()?.configs.clone())
    }

    fn start_all(
        &self,
        configs: Vec<LspServerConfig>,
        workspace_root: &Path,
        sink: Arc<dyn DiagnosticSink>,
    ) -> Result<LspStartReport, ErrorBody> {
        let mut inner = self.lock()?;
        inner.configs = configs.clone();
        Ok(inner.manager.start_all(configs, workspace_root, sink))
    }

    fn start_configured(
        &self,
        workspace_root: &Path,
        sink: Arc<dyn DiagnosticSink>,
    ) -> Result<LspStartReport, ErrorBody> {
        let mut inner = self.lock()?;
        let configs = inner.configs.clone();
        Ok(inner.manager.start_all(configs, workspace_root, sink))
    }

    fn stop_all(&self) -> Result<(), ErrorBody> {
        let mut inner = self.lock()?;
        inner.manager.stop_all();
        Ok(())
    }

    fn did_open(&self, path: &str, language_id: &str, text: &str) -> Result<usize, ErrorBody> {
        self.lock()?
            .manager
            .did_open(path, language_id, text)
            .map_err(lsp_error)
    }

    fn did_change(&self, path: &str, text: &str) -> Result<usize, ErrorBody> {
        self.lock()?
            .manager
            .did_change(path, text)
            .map_err(lsp_error)
    }

    fn did_close(&self, path: &str) -> Result<usize, ErrorBody> {
        self.lock()?.manager.did_close(path).map_err(lsp_error)
    }

    fn lock(&self) -> Result<MutexGuard<'_, LspStateInner>, ErrorBody> {
        self.inner
            .lock()
            .map_err(|_| ErrorBody::new(ErrorCode::Internal, LSP_STATE_UNAVAILABLE))
    }
}

fn lsp_error(error: LspError) -> ErrorBody {
    error.to_error_body()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LspDiagnosticsEvent {
    server_id: String,
    path: String,
    diagnostics: Vec<Diagnostic>,
}

/// 把 `LspClient` 读线程收到的诊断转成 Tauri 事件。
struct TauriDiagnosticSink {
    app: tauri::AppHandle,
}

impl DiagnosticSink for TauriDiagnosticSink {
    fn publish(&self, server_id: &str, path: &str, diagnostics: &[Diagnostic]) {
        // 窗口销毁/重建期间 emit 可能失败；诊断不是命令通道，忽略即可。
        let _ = self.app.emit(
            LSP_DIAGNOSTICS_EVENT,
            LspDiagnosticsEvent {
                server_id: server_id.to_string(),
                path: path.to_string(),
                diagnostics: diagnostics.to_vec(),
            },
        );
    }
}

/// 启动全部语言服务器。
///
/// `configs` 缺省时使用 [`LspState::configure`] 暂存的配置；显式传入时覆盖
/// 暂存值并启动。单个 server 失败写入返回值的 `errors`，不影响其它 server。
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn lsp_start_all(
    configs: Option<Vec<LspServerConfig>>,
    app: tauri::AppHandle,
    state: tauri::State<'_, LspState>,
    workspace: tauri::State<'_, WorkspaceState>,
) -> Result<LspStartReport, ErrorBody> {
    let workspace_root = workspace.get().ok_or_else(workspace_unavailable)?;
    let state = state.inner().clone();
    let sink: Arc<dyn DiagnosticSink> = Arc::new(TauriDiagnosticSink { app });

    tauri::async_runtime::spawn_blocking(move || match configs {
        Some(configs) => state.start_all(configs, &workspace_root, sink),
        None => state.start_configured(&workspace_root, sink),
    })
    .await
    .map_err(|error| ErrorBody::new(ErrorCode::Internal, format!("LSP 启动任务失败：{error}")))?
}

/// 打开文档；`language_id` 没有匹配的 server 时返回 Ack（前端无需处理）。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn lsp_did_open(
    path: String,
    language_id: String,
    text: String,
    state: tauri::State<'_, LspState>,
) -> Result<CommandResult, ErrorBody> {
    state.did_open(&path, &language_id, &text)?;
    Ok(CommandResult::Ack)
}

/// 全量文本变更。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn lsp_did_change(
    path: String,
    text: String,
    state: tauri::State<'_, LspState>,
) -> Result<CommandResult, ErrorBody> {
    state.did_change(&path, &text)?;
    Ok(CommandResult::Ack)
}

/// 关闭文档；对应 server 会收到 `textDocument/didClose`。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn lsp_did_close(
    path: String,
    state: tauri::State<'_, LspState>,
) -> Result<CommandResult, ErrorBody> {
    state.did_close(&path)?;
    Ok(CommandResult::Ack)
}

/// 停止全部语言服务器；`configure` 暂存的配置保留，便于再次启动。
#[tauri::command]
pub(crate) async fn lsp_stop_all(
    state: tauri::State<'_, LspState>,
) -> Result<CommandResult, ErrorBody> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.stop_all())
        .await
        .map_err(|error| ErrorBody::new(ErrorCode::Internal, format!("LSP 停止任务失败：{error}")))?
        .map(|()| CommandResult::Ack)
}
