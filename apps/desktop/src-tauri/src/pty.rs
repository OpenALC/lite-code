//! P5 终端会话命令层（issue #63）：`pty_*` 命令 + `pty:*` 事件。
//!
//! lc-core::pty 持有会话状态机与子进程；本模块只做参数解析（custom DTO
//! camelCase，与既有 ext 命令同惯例，不进 lc-proto 契约）、base64 编解码
//! 与事件下发（AppHandle.emit，事件名固定 `pty:data` / `pty:state` /
//! `pty:exit`）。会话活在 Rust 侧：`PtyState` 是 app 级 State，窗口销毁/
//! 重建不影响；重开窗口用 `pty_list` + `pty_replay` 恢复。

use base64::Engine as _;
#[cfg(test)]
use lc_core::pty::State;
use lc_core::pty::{PtyError, PtyEvent, PtyFactory, PtyHandles, SessionManager, Spec, spawn_real};
use lc_proto::{ErrorBody, ErrorCode};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// 事件通道名（前端 `listen` 逐字使用）。
pub(crate) const PTY_DATA_EVENT: &str = "pty:data";
pub(crate) const PTY_STATE_EVENT: &str = "pty:state";
pub(crate) const PTY_EXIT_EVENT: &str = "pty:exit";

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// `pty_open` 的入参（camelCase，验收契约逐字）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyOpenRequest {
    kind: String,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

/// `pty_open` 的返回（`{ sessionId }`）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyOpenResult {
    session_id: String,
}

/// `pty_list` 的条目（`{ sessionId, kind, state, title }`）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtySessionInfo {
    session_id: String,
    kind: String,
    state: String,
    title: String,
}

/// `pty_list` 的返回。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyListResult {
    sessions: Vec<PtySessionInfo>,
}

fn pty_error_body(error: PtyError) -> ErrorBody {
    match error {
        PtyError::UnknownSession(_) => ErrorBody::new(ErrorCode::NotFound, error.to_string()),
        PtyError::Spawn(_) => ErrorBody::new(ErrorCode::ShellUnavailable, error.to_string()),
    }
}

fn unknown_session_body() -> ErrorBody {
    ErrorBody::new(ErrorCode::NotFound, "会话不存在")
}

/// 事件转发：lc-core 会话线程 → `AppHandle::emit`。窗口销毁期间 emit
/// 可能失败；事件流不是命令通道，忽略即可（会话线程照常缓冲）。
fn forward_event(app: &AppHandle, event: PtyEvent) {
    match event {
        PtyEvent::Data { session_id, data } => {
            let payload = serde_json::json!({
                "sessionId": session_id,
                "data": base64::engine::general_purpose::STANDARD.encode(data),
            });
            let _ = app.emit(PTY_DATA_EVENT, payload);
        }
        PtyEvent::State { session_id, state } => {
            let payload = serde_json::json!({
                "sessionId": session_id,
                "state": state.as_str(),
            });
            let _ = app.emit(PTY_STATE_EVENT, payload);
        }
        PtyEvent::Exit { session_id, code } => {
            let payload = serde_json::json!({
                "sessionId": session_id,
                "code": code,
            });
            let _ = app.emit(PTY_EXIT_EVENT, payload);
        }
    }
}

/// app 级会话管理器（Tauri State）。
pub(crate) struct PtyState {
    manager: SessionManager,
}

impl PtyState {
    pub(crate) fn new(app: AppHandle) -> Self {
        let emitter = app.clone();
        let factory: Box<PtyFactory> = Box::new(
            move |spec: &Spec, cols, rows| -> std::io::Result<PtyHandles> {
                spawn_real(spec, cols, rows)
            },
        );
        let on_event = Box::new(move |event: PtyEvent| forward_event(&emitter, event));
        Self {
            manager: SessionManager::new(factory, on_event),
        }
    }

    /// 应用退出钩子：回收全部 pty 子进程（验收第 5 条）。
    pub(crate) fn shutdown(&self) {
        self.manager.kill_all();
    }
}

fn parse_spec(request: &PtyOpenRequest) -> Result<Spec, ErrorBody> {
    match request.kind.as_str() {
        "local" => Ok(Spec::Local),
        "ssh" => {
            let host = request
                .host
                .as_deref()
                .map(str::trim)
                .filter(|host| !host.is_empty())
                .ok_or_else(|| {
                    ErrorBody::new(ErrorCode::InvalidArgument, "ssh 会话必须提供 host")
                })?;
            Ok(Spec::Ssh {
                host: host.to_string(),
                port: request.port.unwrap_or(22),
                user: request.user.clone().filter(|user| !user.is_empty()),
            })
        }
        other => Err(ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("未知的终端类型：{other}（local | ssh）"),
        )),
    }
}

/// 打开终端会话。本地与 SSH 统一抽象；SSH 在 pty 里启动系统 `ssh` 二进制，
/// 主机密钥确认与密码交互走 ssh 自身提示（TERM=xterm-256color 已设）。
#[tauri::command]
pub(crate) fn pty_open(
    request: PtyOpenRequest,
    state: tauri::State<'_, PtyState>,
) -> Result<PtyOpenResult, ErrorBody> {
    let spec = parse_spec(&request)?;
    let cols = request.cols.unwrap_or(DEFAULT_COLS).max(1);
    let rows = request.rows.unwrap_or(DEFAULT_ROWS).max(1);
    let session = state
        .manager
        .open(spec, cols, rows)
        .map_err(pty_error_body)?;
    Ok(PtyOpenResult {
        session_id: session.id().to_string(),
    })
}

/// 写入（data 为 base64 编码的字节；web 用 Uint8Array 解开）。
#[tauri::command]
pub(crate) fn pty_write(
    session_id: String,
    data: String,
    state: tauri::State<'_, PtyState>,
) -> Result<(), ErrorBody> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| ErrorBody::new(ErrorCode::InvalidArgument, "data 不是合法的 base64"))?;
    let session = state
        .manager
        .get(&session_id)
        .ok_or_else(unknown_session_body)?;
    session.write(&bytes);
    Ok(())
}

/// resize（cols/rows 下限 1）。
#[tauri::command]
pub(crate) fn pty_resize(
    session_id: String,
    cols: u16,
    rows: u16,
    state: tauri::State<'_, PtyState>,
) -> Result<(), ErrorBody> {
    let session = state
        .manager
        .get(&session_id)
        .ok_or_else(unknown_session_body)?;
    session.resize(cols.max(1), rows.max(1));
    Ok(())
}

/// 用户关闭（幂等）。
#[tauri::command]
pub(crate) fn pty_close(
    session_id: String,
    state: tauri::State<'_, PtyState>,
) -> Result<(), ErrorBody> {
    state.manager.close(&session_id).map_err(pty_error_body)
}

/// 列出会话（重开窗口恢复标签用；含 closed 会话）。
#[tauri::command]
pub(crate) fn pty_list(state: tauri::State<'_, PtyState>) -> PtyListResult {
    PtyListResult {
        sessions: state
            .manager
            .list()
            .into_iter()
            .map(|session| PtySessionInfo {
                session_id: session.id().to_string(),
                kind: session.spec().kind().to_string(),
                state: session.state().as_str().to_string(),
                title: session.title(),
            })
            .collect(),
    }
}

/// 取全量滚动缓冲（base64；追加命令，web 端写入后即为当前屏幕）。
#[tauri::command]
pub(crate) fn pty_replay(
    session_id: String,
    state: tauri::State<'_, PtyState>,
) -> Result<String, ErrorBody> {
    let session = state
        .manager
        .get(&session_id)
        .ok_or_else(unknown_session_body)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(session.scrollback()))
}

/// 终端状态机语义的映射测试（字段名契约回归）。
#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: &str, host: Option<&str>, port: Option<u16>) -> PtyOpenRequest {
        PtyOpenRequest {
            kind: kind.to_string(),
            host: host.map(str::to_string),
            port,
            user: None,
            cols: None,
            rows: None,
        }
    }

    #[test]
    fn parse_spec_maps_local_and_ssh() {
        assert_eq!(
            parse_spec(&request("local", None, None)).unwrap(),
            Spec::Local
        );
        assert_eq!(
            parse_spec(&request("ssh", Some("example.com"), None)).unwrap(),
            Spec::Ssh {
                host: "example.com".into(),
                port: 22,
                user: None,
            }
        );
        assert_eq!(
            parse_spec(&request("ssh", Some("example.com"), Some(2222))).unwrap(),
            Spec::Ssh {
                host: "example.com".into(),
                port: 2222,
                user: None,
            }
        );
        assert!(parse_spec(&request("telnet", None, None)).is_err());
        assert!(parse_spec(&request("ssh", None, None)).is_err());
        assert!(parse_spec(&request("ssh", Some("  "), None)).is_err());
    }

    #[test]
    fn session_state_strings_match_contract() {
        assert_eq!(State::Connected.as_str(), "connected");
        assert_eq!(State::Reconnecting.as_str(), "reconnecting");
        assert_eq!(State::Closed.as_str(), "closed");
        assert_eq!(Spec::Local.kind(), "local");
        assert_eq!(
            Spec::Ssh {
                host: "h".into(),
                port: 22,
                user: None
            }
            .kind(),
            "ssh"
        );
    }

    #[test]
    fn event_names_are_fixed() {
        assert_eq!(PTY_DATA_EVENT, "pty:data");
        assert_eq!(PTY_STATE_EVENT, "pty:state");
        assert_eq!(PTY_EXIT_EVENT, "pty:exit");
    }
}
