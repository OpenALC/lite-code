//! 鸿蒙宿主的会话历史：`<files_dir>/history/<session_id>.json`。
//!
//! 形状与桌面 `apps/desktop/src-tauri/src/history.rs` / 前端 `HistorySummary`
//! 保持一致。宿主只做最小 schema 解读（列表读顶层元信息与消息摘要），保存
//! 的是前端生成的完整快照。写入走同目录 tmp + rename 并限制 0600；会话 id
//! 只允许安全字符，避免任意路径写入。历史条数超过 [`MAX_ENTRIES`] 时按
//! `savedAt` 从老到新驱逐。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lc_proto::{ErrorBody, ErrorCode};
use serde::Serialize;
use serde_json::Value;

const MAX_ID_LEN: usize = 128;
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
/// 原生历史保留条数；超出后按 `savedAt` 淘汰最老的记录。
pub(crate) const MAX_ENTRIES: usize = 50;
const EXCERPT_LIMIT: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryEntry {
    pub id: String,
    pub title: String,
    pub saved_at: u64,
    pub workspace: String,
    pub mode: String,
    pub model: String,
    pub turn: u32,
    pub messages: usize,
    pub archived: bool,
    /// 消息正文摘要，供列表检索；限制长度，避免列表响应过大。
    pub excerpt: String,
}

fn invalid(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::InvalidArgument, message)
}

fn io_error(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::Io, message)
}

fn excerpt_of(agent: &Value) -> String {
    let Some(messages) = agent.get("messages").and_then(Value::as_array) else {
        return String::new();
    };
    let mut text = String::new();
    for message in messages {
        let Some(raw) = message.get("text").and_then(Value::as_str) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(raw);
        if text.chars().count() >= EXCERPT_LIMIT {
            break;
        }
    }
    text.chars().take(EXCERPT_LIMIT).collect()
}

/// 只允许文件名安全字符；`/`、`\` 与 `..` 都会被拒绝。
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id.len() <= MAX_ID_LEN
        && !id.contains("..")
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn path_for(dir: &Path, id: &str) -> Result<PathBuf, ErrorBody> {
    if !valid_id(id) {
        return Err(invalid(format!("invalid history id: {id}")));
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
        archived: payload
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        excerpt: excerpt_of(agent),
    }
}

/// 列出可解析的历史摘要，按 `savedAt` 降序。目录不存在返回空列表；损坏的
/// JSON 文件跳过，不影响其他记录。
pub(crate) fn list(dir: &Path) -> Result<Vec<HistoryEntry>, ErrorBody> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in
        fs::read_dir(dir).map_err(|error| io_error(format!("读取历史目录失败：{error}")))?
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
    out.sort_by_key(|entry| std::cmp::Reverse(entry.saved_at));
    Ok(out)
}

pub(crate) fn load(dir: &Path, id: &str) -> Result<Value, ErrorBody> {
    let path = path_for(dir, id)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ErrorBody::new(
                ErrorCode::NotFound,
                format!("history entry not found: {id}"),
            ));
        }
        Err(error) => return Err(io_error(format!("读取历史失败：{error}"))),
    };
    serde_json::from_str(&text).map_err(|error| invalid(format!("历史 JSON 无效：{error}")))
}

fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> Result<(), ErrorBody> {
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("history.json");
    let tmp = dir.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&tmp);
        return Err(io_error(format!("写入历史失败：{error}")));
    }
    Ok(())
}

fn evict_oldest(dir: &Path) -> Result<(), ErrorBody> {
    let mut entries = list(dir)?;
    if entries.len() <= MAX_ENTRIES {
        return Ok(());
    }
    entries.sort_by(|a, b| a.saved_at.cmp(&b.saved_at).then_with(|| a.id.cmp(&b.id)));
    let excess = entries.len() - MAX_ENTRIES;
    for entry in entries.into_iter().take(excess) {
        let path = path_for(dir, &entry.id)?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(format!("淘汰历史失败：{error}"))),
        }
    }
    Ok(())
}

pub(crate) fn save(dir: &Path, id: &str, payload: &Value) -> Result<(), ErrorBody> {
    if !payload.is_object() {
        return Err(invalid("历史数据必须是 JSON 对象"));
    }
    let path = path_for(dir, id)?;
    let bytes =
        serde_json::to_vec(payload).map_err(|error| invalid(format!("序列化历史失败：{error}")))?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(invalid(format!(
            "历史数据超过 {} MiB",
            MAX_PAYLOAD_BYTES / 1024 / 1024
        )));
    }
    fs::create_dir_all(dir).map_err(|error| io_error(format!("创建历史目录失败：{error}")))?;
    write_atomic(dir, &path, &bytes)?;
    evict_oldest(dir)
}

pub(crate) fn delete(dir: &Path, id: &str) -> Result<(), ErrorBody> {
    let path = path_for(dir, id)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(format!("删除历史失败：{error}"))),
    }
}

/// 已落盘历史里最大的 `ohos-<n>` 会话序号。进程重启后 `next_session`
/// 从这里继续，避免新建会话覆盖同名旧历史。
pub(crate) fn max_session_ordinal(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut max = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Some(ordinal) = stem
            .strip_prefix("ohos-")
            .and_then(|rest| rest.parse::<u64>().ok())
        else {
            continue;
        };
        max = max.max(ordinal);
    }
    max
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lc-ohos-history-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn payload(title: &str, saved_at: u64) -> Value {
        json!({
            "title": title,
            "savedAt": saved_at,
            "workspace": "/work/project",
            "mode": "auto",
            "model": "gpt-5",
            "turnStats": [],
            "agent": {
                "turn": 3,
                "messages": [
                    {"role": "user", "text": "请修复登录问题"},
                    {"role": "assistant", "text": "我先看看代码"}
                ]
            }
        })
    }

    #[test]
    fn save_list_load_delete_roundtrip() {
        let dir = temp_dir("roundtrip");
        save(&dir, "ohos-1", &payload("第一个会话", 10)).unwrap();
        save(&dir, "ohos-2", &payload("第二个会话", 20)).unwrap();

        let entries = list(&dir).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "ohos-2");
        assert_eq!(entries[0].title, "第二个会话");
        assert_eq!(entries[0].saved_at, 20);
        assert_eq!(entries[0].workspace, "/work/project");
        assert_eq!(entries[0].turn, 3);
        assert_eq!(entries[0].messages, 2);
        assert_eq!(entries[0].excerpt, "请修复登录问题\n我先看看代码");

        let loaded = load(&dir, "ohos-1").unwrap();
        assert_eq!(loaded["title"], "第一个会话");

        // 同一 id 再保存应覆盖而不是追加（tmp/rename 目标已存在）。
        save(&dir, "ohos-2", &payload("第二个会话（更新）", 21)).unwrap();
        assert_eq!(list(&dir).unwrap().len(), 2);
        assert_eq!(load(&dir, "ohos-2").unwrap()["title"], "第二个会话（更新）");

        delete(&dir, "ohos-1").unwrap();
        assert_eq!(list(&dir).unwrap().len(), 1);
        delete(&dir, "ohos-1").unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_json_is_skipped_by_list_but_not_load() {
        let dir = temp_dir("corrupt");
        save(&dir, "ohos-1", &payload("正常会话", 1)).unwrap();
        fs::write(dir.join("ohos-2.json"), "{not json").unwrap();

        let list = list(&dir).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "ohos-1");
        assert!(load(&dir, "ohos-2").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_missing_and_load_missing_are_stable() {
        let dir = temp_dir("missing");
        delete(&dir, "ohos-9").unwrap();
        let error = load(&dir, "ohos-9").unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn id_must_not_escape_history_dir() {
        let dir = temp_dir("bad-id");
        for id in ["../escape", "a/b", r"a\b", "a..b", "", "."] {
            assert!(save(&dir, id, &payload("x", 1)).is_err(), "id={id:?}");
            assert!(load(&dir, id).is_err(), "id={id:?}");
            assert!(delete(&dir, id).is_err(), "id={id:?}");
        }
        assert!(!dir.parent().unwrap().join("escape.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_object_payload_is_rejected() {
        let dir = temp_dir("bad-payload");
        assert!(save(&dir, "ohos-1", &json!("text")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn over_cap_evicts_oldest() {
        let dir = temp_dir("cap");
        for index in 0..=MAX_ENTRIES {
            save(
                &dir,
                &format!("ohos-{index}"),
                &payload(&format!("会话 {index}"), index as u64),
            )
            .unwrap();
        }
        let list = list(&dir).unwrap();
        assert_eq!(list.len(), MAX_ENTRIES);
        assert!(!list.iter().any(|entry| entry.id == "ohos-0"));
        assert!(list.iter().any(|entry| entry.id == "ohos-50"));
        assert_eq!(list[0].id, "ohos-50");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn max_session_ordinal_reads_saved_history() {
        let dir = temp_dir("ordinal");
        assert_eq!(max_session_ordinal(&dir), 0);
        save(&dir, "ohos-7", &payload("旧会话", 1)).unwrap();
        assert_eq!(max_session_ordinal(&dir), 7);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_private_and_no_tmp_left() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("mode");
        save(&dir, "ohos-1", &payload("私密会话", 1)).unwrap();
        let path = dir.join("ohos-1.json");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
