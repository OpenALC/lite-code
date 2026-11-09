//! 鸿蒙宿主：纯 JSON 入口，host 上可单测，不依赖 NAPI。
//!
//! 首版命令面（docs/plans/ohos-shell.md §5.4）：
//! - `workspace_root` / `fs_read_dir` / `fs_read_file` 复用 lc-core 的路径校验与按层读取；
//! - `session_create` 强制 `plan` 模式（只读 + 对话）；
//! - `agent_send` / `agent_cancel` 是 stub：20 个 `text_delta` 后 `done`，取消可中断；
//! - 其余命令返回 `not_implemented`（契约没有独立的 `unsupported` 错误码）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use lc_core::fs::{RealPathResolver, display_path, ensure_within_root, read_dir_level};
use lc_proto::{
    CommandResult, CoreEvent, ErrorBody, ErrorCode, EventEnvelope, SessionInfo, SessionMode,
};
use serde_json::Value;

pub mod ffi;

pub type EventSink = Arc<dyn Fn(String) + Send + Sync>;

const STUB_DELTAS: u32 = 20;
const STUB_DELAY: Duration = Duration::from_millis(25);

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub files_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub providers_file: PathBuf,
}

pub struct Host {
    root: PathBuf,
    root_display: String,
    sessions: Mutex<HashMap<String, SessionHandle>>,
    sink: Mutex<Option<EventSink>>,
    seq: Arc<AtomicU64>,
    next_session: AtomicU64,
}

#[derive(Clone)]
struct SessionHandle {
    info: Arc<Mutex<SessionInfo>>,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
}

impl SessionHandle {
    fn new(info: SessionInfo) -> Self {
        Self {
            info: Arc::new(Mutex::new(info)),
            cancel: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Host {
    pub fn init(config: HostConfig) -> Result<Host, ErrorBody> {
        let root = ensure_within_root(
            &config.workspace_root,
            &config.workspace_root,
            &RealPathResolver,
        )
        .map_err(|error| error.to_error_body())?;
        if !root.is_dir() {
            return Err(invalid(format!(
                "workspace root is not a directory: {}",
                display_path(&root)
            )));
        }
        fs::create_dir_all(&config.files_dir).map_err(|error| io_body(&config.files_dir, error))?;
        if let Some(parent) = config.providers_file.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| io_body(parent, error))?;
        }
        Ok(Host {
            root_display: display_path(&root),
            root,
            sessions: Mutex::new(HashMap::new()),
            sink: Mutex::new(None),
            seq: Arc::new(AtomicU64::new(0)),
            next_session: AtomicU64::new(0),
        })
    }

    /// 命令入口：成功返回契约结果的 JSON，失败返回 `ErrorBody`。
    pub fn dispatch(&self, cmd: &str, args_json: &str) -> Result<String, ErrorBody> {
        let args = parse_args(args_json)?;
        let result = match cmd {
            "workspace_root" => CommandResult::WorkspaceRoot {
                root: Some(self.root_display.clone()),
            },
            "fs_read_dir" => {
                let path = self.resolve_path(&args)?;
                let level = read_dir_level(&self.root, &path, &RealPathResolver)
                    .map_err(|error| error.to_error_body())?;
                CommandResult::DirListing {
                    path: level.path,
                    entries: level.entries,
                    generation: 0,
                }
            }
            "fs_read_file" => {
                let path = self.resolve_path(&args)?;
                let canonical = ensure_within_root(&self.root, &path, &RealPathResolver)
                    .map_err(|error| error.to_error_body())?;
                let bytes = fs::read(&canonical).map_err(|error| io_body(&canonical, error))?;
                let text = String::from_utf8(bytes).map_err(|_| {
                    ErrorBody::new(
                        ErrorCode::Io,
                        format!("file is not valid UTF-8: {}", display_path(&canonical)),
                    )
                })?;
                CommandResult::FileContent {
                    path: display_path(&canonical),
                    text,
                }
            }
            "session_create" => CommandResult::Session {
                session: self.create_session(),
            },
            "agent_send" => {
                self.agent_send(&args)?;
                CommandResult::Ack
            }
            "agent_cancel" => {
                self.agent_cancel(&args)?;
                CommandResult::Ack
            }
            _ => return Err(unsupported(cmd)),
        };
        serde_json::to_string(&result).map_err(|error| {
            ErrorBody::new(ErrorCode::Internal, format!("serialize result: {error}"))
        })
    }

    /// 原生事件出口：`EventEnvelope` JSON 字符串。可重复设置（页面重建后换 sink）。
    pub fn set_event_sink(&self, sink: EventSink) {
        *self.sink_slot() = Some(sink);
    }

    fn create_session(&self) -> SessionInfo {
        let number = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        let info = SessionInfo {
            session_id: format!("ohos-{number}"),
            mode: SessionMode::Plan,
            turn: 0,
            max_turns: 25,
            context_k: 400,
            compress_at_k: 300,
            cancelled: false,
        };
        self.sessions_slot()
            .insert(info.session_id.clone(), SessionHandle::new(info.clone()));
        info
    }

    fn agent_send(&self, args: &Value) -> Result<(), ErrorBody> {
        let session_id = required_string(args, "session_id")?;
        let _text = required_string(args, "text")?;
        let handle = self.session(&session_id)?;
        if handle.running.swap(true, Ordering::AcqRel) {
            return Err(invalid(format!("agent turn already running: {session_id}")));
        }
        handle.cancel.store(false, Ordering::Release);
        {
            let mut info = handle
                .info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            info.turn = info.turn.saturating_add(1);
        }
        let sink = self.sink_slot().clone();
        let seq = Arc::clone(&self.seq);
        thread::spawn(move || run_stub_agent(session_id, handle, sink, seq));
        Ok(())
    }

    fn agent_cancel(&self, args: &Value) -> Result<(), ErrorBody> {
        let session_id = required_string(args, "session_id")?;
        let handle = self.session(&session_id)?;
        if handle.running.load(Ordering::Acquire) {
            handle.cancel.store(true, Ordering::Release);
        }
        Ok(())
    }

    fn session(&self, id: &str) -> Result<SessionHandle, ErrorBody> {
        self.sessions_slot()
            .get(id)
            .cloned()
            .ok_or_else(|| ErrorBody::new(ErrorCode::NotFound, format!("session not found: {id}")))
    }

    fn resolve_path(&self, args: &Value) -> Result<PathBuf, ErrorBody> {
        let raw = required_string(args, "path")?;
        let path = PathBuf::from(raw);
        Ok(if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        })
    }

    fn sessions_slot(&self) -> MutexGuard<'_, HashMap<String, SessionHandle>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn sink_slot(&self) -> MutexGuard<'_, Option<EventSink>> {
        self.sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn run_stub_agent(
    session_id: String,
    handle: SessionHandle,
    sink: Option<EventSink>,
    seq: Arc<AtomicU64>,
) {
    for index in 0..STUB_DELTAS {
        if handle.cancel.load(Ordering::Acquire) {
            finish_cancelled(&session_id, &handle, &sink, &seq);
            return;
        }
        emit(
            &sink,
            &seq,
            CoreEvent::AgentTextDelta {
                session_id: session_id.clone(),
                index,
                text: format!("[stub] delta {index}\n"),
            },
        );
        thread::sleep(STUB_DELAY);
    }
    if handle.cancel.load(Ordering::Acquire) {
        finish_cancelled(&session_id, &handle, &sink, &seq);
        return;
    }
    emit(
        &sink,
        &seq,
        CoreEvent::AgentDone {
            session_id,
            stop_reason: "stop".to_owned(),
        },
    );
    handle.running.store(false, Ordering::Release);
}

fn finish_cancelled(
    session_id: &str,
    handle: &SessionHandle,
    sink: &Option<EventSink>,
    seq: &Arc<AtomicU64>,
) {
    let session = {
        let mut info = handle
            .info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        info.cancelled = true;
        info.clone()
    };
    emit(sink, seq, CoreEvent::AgentSessionState { session });
    emit(
        sink,
        seq,
        CoreEvent::AgentDone {
            session_id: session_id.to_owned(),
            stop_reason: "cancelled".to_owned(),
        },
    );
    handle.running.store(false, Ordering::Release);
}

fn emit(sink: &Option<EventSink>, seq: &AtomicU64, event: CoreEvent) {
    let Some(sink) = sink else { return };
    let envelope = EventEnvelope {
        seq: seq.fetch_add(1, Ordering::SeqCst) + 1,
        event,
    };
    if let Ok(json) = serde_json::to_string(&envelope) {
        sink(json);
    }
}

fn parse_args(args_json: &str) -> Result<Value, ErrorBody> {
    if args_json.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let value: Value = serde_json::from_str(args_json)
        .map_err(|error| invalid(format!("invalid args json: {error}")))?;
    if !value.is_object() {
        return Err(invalid("args must be a JSON object"));
    }
    Ok(value)
}

fn required_string(args: &Value, key: &str) -> Result<String, ErrorBody> {
    args.get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(format!("missing string argument `{key}`")))
}

fn invalid(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::InvalidArgument, message)
}

fn io_body(path: &Path, error: std::io::Error) -> ErrorBody {
    let code = if error.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::NotFound
    } else {
        ErrorCode::Io
    };
    ErrorBody::new(code, format!("io error on {}: {error}", display_path(path)))
}

fn unsupported(cmd: &str) -> ErrorBody {
    ErrorBody::new(
        ErrorCode::NotImplemented,
        format!("unsupported command on ohos shell: {cmd}"),
    )
    .with_detail(
        "supported: workspace_root, fs_read_dir, fs_read_file, session_create, agent_send, agent_cancel",
    )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::*;

    fn temp_base() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "lc-ohos-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn setup() -> (Host, PathBuf) {
        let base = temp_base();
        let root = base.join("workspace");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(base.join("outside.txt"), "secret\n").unwrap();
        let host = Host::init(HostConfig {
            files_dir: base.join("files"),
            workspace_root: root,
            providers_file: base.join("files/providers.json"),
        })
        .unwrap();
        (host, base)
    }

    fn parse(raw: &str) -> Value {
        serde_json::from_str(raw).unwrap()
    }

    fn collect_events(host: &Host) -> Arc<Mutex<Vec<Value>>> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        host.set_event_sink(Arc::new(move |json| {
            sink_events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(serde_json::from_str(&json).unwrap());
        }));
        events
    }

    fn wait_for(
        events: &Arc<Mutex<Vec<Value>>>,
        predicate: impl Fn(&[Value]) -> bool,
    ) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if predicate(&snapshot) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "timeout waiting for events: {snapshot:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn create_session(host: &Host) -> String {
        let value = parse(
            &host
                .dispatch(
                    "session_create",
                    &json!({"mode": "plan", "model": null, "thinking": null}).to_string(),
                )
                .unwrap(),
        );
        assert_eq!(value["kind"], "session");
        assert_eq!(value["session"]["mode"], "plan");
        value["session"]["session_id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn fs_commands_return_contract_shapes() {
        let (host, base) = setup();
        let root = parse(&host.dispatch("workspace_root", "{}").unwrap());
        assert_eq!(root["kind"], "workspace_root");
        let root_path = root["root"].as_str().unwrap().to_owned();
        assert!(Path::new(&root_path).is_dir());

        let listing = parse(
            &host
                .dispatch("fs_read_dir", &json!({"path": "src"}).to_string())
                .unwrap(),
        );
        assert_eq!(listing["kind"], "dir_listing");
        assert_eq!(
            listing["path"],
            Path::new(&root_path).join("src").to_string_lossy().as_ref()
        );
        assert_eq!(listing["generation"], 0);
        let entries = listing["entries"].as_array().unwrap();
        let main = entries
            .iter()
            .find(|entry| entry["name"] == "main.rs")
            .expect("src/main.rs should be listed");
        assert_eq!(main["kind"], "file");
        assert_eq!(
            main["path"],
            Path::new(&root_path)
                .join("src")
                .join("main.rs")
                .to_string_lossy()
                .as_ref()
        );

        let file = parse(
            &host
                .dispatch("fs_read_file", &json!({"path": main["path"]}).to_string())
                .unwrap(),
        );
        assert_eq!(file["kind"], "file_content");
        assert_eq!(file["text"], "fn main() {}\n");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn path_traversal_is_rejected() {
        let (host, base) = setup();
        let outside = base.join("outside.txt");

        let error = host
            .dispatch("fs_read_file", &json!({"path": outside}).to_string())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PathOutsideRoot);

        let error = host
            .dispatch(
                "fs_read_file",
                &json!({"path": "../outside.txt"}).to_string(),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PathOutsideRoot);

        let error = host
            .dispatch("fs_read_dir", &json!({"path": ".."}).to_string())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PathOutsideRoot);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn error_body_shape_is_stable() {
        let (host, base) = setup();

        let error = host.dispatch("git_status", "{}").unwrap_err();
        assert_eq!(error.code, ErrorCode::NotImplemented);
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(value["code"], "not_implemented");
        assert!(value["message"].as_str().unwrap().contains("unsupported"));
        assert!(value["detail"].as_str().unwrap().contains("workspace_root"));

        let error = host.dispatch("fs_read_dir", "{}").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("path"));

        let error = host.dispatch("fs_read_dir", "not json").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn stub_agent_emits_twenty_deltas_then_done() {
        let (host, base) = setup();
        let events = collect_events(&host);
        let session_id = create_session(&host);

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "hello"}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let deltas: Vec<&Value> = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .collect();
        assert_eq!(deltas.len(), 20);
        for (index, delta) in deltas.iter().enumerate() {
            assert_eq!(
                delta["event"]["session_id"].as_str(),
                Some(session_id.as_str())
            );
            assert_eq!(delta["event"]["index"], index as u64);
        }

        let seqs: Vec<u64> = snapshot
            .iter()
            .map(|event| event["seq"].as_u64().unwrap())
            .collect();
        assert!(seqs.windows(2).all(|pair| pair[0] + 1 == pair[1]));

        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["type"], "agent:done");
        assert_eq!(done["event"]["stop_reason"], "stop");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn agent_cancel_interrupts_with_cancelled_events() {
        let (host, base) = setup();
        let events = collect_events(&host);
        let session_id = create_session(&host);

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "hello"}).to_string(),
        )
        .unwrap();
        wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:text_delta")
        });

        host.dispatch(
            "agent_cancel",
            &json!({"session_id": session_id}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let cancelled = snapshot
            .iter()
            .find(|event| event["event"]["type"] == "agent:session_state")
            .expect("cancel should emit session_state");
        assert_eq!(cancelled["event"]["session"]["cancelled"], true);

        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["type"], "agent:done");
        assert_eq!(done["event"]["stop_reason"], "cancelled");

        let deltas = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .count();
        assert!(deltas < 20, "cancel should interrupt before all deltas");

        let _ = fs::remove_dir_all(&base);
    }
}
