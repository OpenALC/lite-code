//! Agent 会话历史：`<data_dir>/agent-history/<id>.json`。
//!
//! 仅保存前端生成的会话快照（消息、工具、usage、turnStats），Rust 侧不做 schema
//! 解释；列表只读取顶层元信息。文件大小与 id 都做上限，避免任意路径写入。

use std::fs;
use std::path::{Path, PathBuf};

use lc_proto::{ErrorBody, ErrorCode};
use serde::Serialize;
use serde_json::Value;

const MAX_ID_LEN: usize = 128;
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub title: String,
    pub saved_at: u64,
    pub workspace: String,
    pub mode: String,
    pub model: String,
    pub turn: u32,
    pub messages: usize,
}

fn history_dir() -> Result<PathBuf, ErrorBody> {
    lc_core::config::data_dir()
        .map(|base| base.join("agent-history"))
        .map_err(|error| {
            ErrorBody::new(
                ErrorCode::NotFound,
                format!("无法定位应用数据目录：{error}"),
            )
        })
}

fn invalid(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::InvalidArgument, message)
}

fn io_error(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::Io, message)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && !id.contains("..")
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn path_for(dir: &Path, id: &str) -> Result<PathBuf, ErrorBody> {
    if !valid_id(id) {
        return Err(invalid("会话 id 非法"));
    }
    Ok(dir.join(format!("{id}.json")))
}

fn summarize(id: String, payload: &Value) -> HistoryEntry {
    let agent = payload.get("agent").unwrap_or(&Value::Null);
    HistoryEntry {
        id,
        title: payload
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("未命名会话")
            .to_string(),
        saved_at: payload.get("savedAt").and_then(Value::as_u64).unwrap_or(0),
        workspace: payload
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        mode: payload
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .to_string(),
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        turn: agent.get("turn").and_then(Value::as_u64).unwrap_or(0) as u32,
        messages: agent
            .get("messages")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    }
}

fn list_in(dir: &Path) -> Result<Vec<HistoryEntry>, ErrorBody> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in
        fs::read_dir(dir).map_err(|error| io_error(format!("读取会话目录失败：{error}")))?
    {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !valid_id(&id) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        out.push(summarize(id, &payload));
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.saved_at));
    Ok(out)
}

fn load_in(dir: &Path, id: &str) -> Result<Value, ErrorBody> {
    let path = path_for(dir, id)?;
    let text =
        fs::read_to_string(&path).map_err(|error| io_error(format!("读取会话失败：{error}")))?;
    serde_json::from_str(&text).map_err(|error| invalid(format!("会话 JSON 无效：{error}")))
}

fn save_in(dir: &Path, id: &str, payload: &Value) -> Result<(), ErrorBody> {
    if !payload.is_object() {
        return Err(invalid("会话数据必须是 JSON 对象"));
    }
    let path = path_for(dir, id)?;
    let bytes =
        serde_json::to_vec(payload).map_err(|error| invalid(format!("序列化会话失败：{error}")))?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(invalid(format!(
            "会话数据超过 {} MiB",
            MAX_PAYLOAD_BYTES / 1024 / 1024
        )));
    }
    fs::create_dir_all(dir).map_err(|error| io_error(format!("创建会话目录失败：{error}")))?;
    fs::write(&path, bytes).map_err(|error| io_error(format!("写入会话失败：{error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn delete_in(dir: &Path, id: &str) -> Result<(), ErrorBody> {
    let path = path_for(dir, id)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(format!("删除会话失败：{error}"))),
    }
}

/// 旧版本历史没有 workspace 字段；启动时把它们归入当前工作区。
fn migrate_workspace_in(dir: &Path, workspace: &str) -> Result<usize, ErrorBody> {
    if workspace.trim().is_empty() || !dir.exists() {
        return Ok(0);
    }
    let mut moved = 0;
    for entry in
        fs::read_dir(dir).map_err(|error| io_error(format!("读取会话目录失败：{error}")))?
    {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !valid_id(&id) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut payload) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(object) = payload.as_object_mut() else {
            continue;
        };
        let missing = object
            .get("workspace")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty);
        if !missing {
            continue;
        }
        object.insert("workspace".into(), Value::String(workspace.into()));
        save_in(dir, &id, &payload)?;
        moved += 1;
    }
    Ok(moved)
}

#[tauri::command]
pub fn history_list() -> Result<Vec<HistoryEntry>, ErrorBody> {
    list_in(&history_dir()?)
}

#[tauri::command]
pub fn history_load(id: String) -> Result<Value, ErrorBody> {
    load_in(&history_dir()?, &id)
}

#[tauri::command]
pub fn history_save(id: String, payload: Value) -> Result<(), ErrorBody> {
    save_in(&history_dir()?, &id, &payload)
}

#[tauri::command]
pub fn history_delete(id: String) -> Result<(), ErrorBody> {
    delete_in(&history_dir()?, &id)
}

#[tauri::command]
pub fn history_migrate_workspace(workspace: String) -> Result<usize, ErrorBody> {
    migrate_workspace_in(&history_dir()?, &workspace)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lc-history-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn payload(title: &str, saved_at: u64) -> Value {
        serde_json::json!({
            "title": title,
            "savedAt": saved_at,
            "workspace": "/work/project",
            "mode": "auto",
            "model": "gpt-5",
            "turnStats": [],
            "agent": {
                "turn": 3,
                "messages": [{"role": "user"}, {"role": "assistant"}]
            }
        })
    }

    #[test]
    fn save_list_load_delete_roundtrip() {
        let dir = temp_dir("roundtrip");
        save_in(&dir, "s-1", &payload("第一个会话", 10)).unwrap();
        save_in(&dir, "s-2", &payload("第二个会话", 20)).unwrap();

        let list = list_in(&dir).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "s-2");
        assert_eq!(list[0].title, "第二个会话");
        assert_eq!(list[0].workspace, "/work/project");
        assert_eq!(list[0].turn, 3);
        assert_eq!(list[0].messages, 2);

        let loaded = load_in(&dir, "s-1").unwrap();
        assert_eq!(loaded["title"], "第一个会话");

        delete_in(&dir, "s-1").unwrap();
        assert_eq!(list_in(&dir).unwrap().len(), 1);
        delete_in(&dir, "s-1").unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_id_is_rejected() {
        let dir = temp_dir("bad-id");
        assert!(save_in(&dir, "../escape", &payload("x", 1)).is_err());
        assert!(load_in(&dir, "a/b").is_err());
        assert!(delete_in(&dir, "").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_object_payload_is_rejected() {
        let dir = temp_dir("bad-payload");
        assert!(save_in(&dir, "s-1", &serde_json::json!("text")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_assigns_missing_workspace_once() {
        let dir = temp_dir("migrate");
        save_in(
            &dir,
            "s-1",
            &serde_json::json!({
                "title": "旧会话",
                "savedAt": 1,
                "agent": {"turn": 0, "messages": []}
            }),
        )
        .unwrap();

        assert_eq!(migrate_workspace_in(&dir, "D:/work/a").unwrap(), 1);
        assert_eq!(list_in(&dir).unwrap()[0].workspace, "D:/work/a");
        assert_eq!(migrate_workspace_in(&dir, "D:/work/b").unwrap(), 0);
        assert_eq!(list_in(&dir).unwrap()[0].workspace, "D:/work/a");
        let _ = fs::remove_dir_all(&dir);
    }
}
