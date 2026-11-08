//! 真实 stdio LSP 端到端测试：Node fake server 验证 initialize、didOpen、
//! publishDiagnostics、didChange、didClose 与 stop。
//!
//! 无 Node 时用例跳过（打印原因），不标记失败；CI 未装 Node 仍可跑其余测试。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lc_core::lsp::{Diagnostic, DiagnosticSink, LspManager, LspServerConfig, LspStartReport};

const WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
struct Published {
    server_id: String,
    path: String,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<Published>>,
    ready: Condvar,
}

impl DiagnosticSink for RecordingSink {
    fn publish(&self, server_id: &str, path: &str, diagnostics: &[Diagnostic]) {
        {
            let mut events = self.events.lock().expect("events");
            events.push(Published {
                server_id: server_id.to_string(),
                path: path.to_string(),
                diagnostics: diagnostics.to_vec(),
            });
        }
        self.ready.notify_all();
    }
}

impl RecordingSink {
    fn wait_for(
        &self,
        predicate: impl Fn(&Published) -> bool,
        timeout: Duration,
    ) -> Option<Published> {
        let deadline = Instant::now() + timeout;
        let mut events = self.events.lock().expect("events");
        loop {
            if let Some(event) = events.iter().find(|event| predicate(event)) {
                return Some(event.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, result) = self
                .ready
                .wait_timeout(events, deadline - now)
                .expect("condvar");
            events = guard;
            if result.timed_out() {
                return events.iter().find(|event| predicate(event)).cloned();
            }
        }
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lc-lsp-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_lsp_server.mjs")
}

fn fake_config(id: &str, log: &Path) -> LspServerConfig {
    LspServerConfig {
        id: id.to_string(),
        language_ids: vec!["plaintext".to_string()],
        command: "node".to_string(),
        args: vec![
            fixture_path().to_string_lossy().into_owned(),
            "--log".to_string(),
            log.to_string_lossy().into_owned(),
        ],
        env: BTreeMap::new(),
    }
}

fn read_log(log: &Path) -> String {
    fs::read_to_string(log).unwrap_or_default()
}

fn wait_for_log(log: &Path, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if read_log(log).contains(needle) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn start_fake(
    manager: &mut LspManager,
    root: &Path,
    log: &Path,
    sink: Arc<RecordingSink>,
) -> LspStartReport {
    manager.start_all(vec![fake_config("fake", log)], root, sink)
}

#[test]
fn lsp_stdio_initialize_open_change_close_and_stop() {
    if !node_available() {
        eprintln!("skipping lsp stdio test: node not found in PATH");
        return;
    }

    let dir = TempDir::new("stdio");
    let log = dir.path().join("fake-lsp.log");
    let document_path = dir.path().join("sample.txt");
    fs::write(&document_path, "alpha").expect("write sample");
    let document_path = document_path.to_string_lossy().into_owned();

    let sink = Arc::new(RecordingSink::default());
    let mut manager = LspManager::default();
    let report = start_fake(&mut manager, dir.path(), &log, Arc::clone(&sink));
    assert!(report.is_ok(), "start errors: {:?}", report.errors);
    assert_eq!(report.servers.len(), 1);
    assert_eq!(report.servers[0].id, "fake");
    assert_eq!(report.servers[0].language_ids, ["plaintext"]);
    assert!(manager.is_running("fake"));

    assert!(
        wait_for_log(&log, "\"method\":\"initialize\"", WAIT),
        "initialize was not sent: {}",
        read_log(&log)
    );
    assert!(
        wait_for_log(&log, "\"method\":\"initialized\"", WAIT),
        "initialized was not sent: {}",
        read_log(&log)
    );

    let delivered = manager
        .did_open(&document_path, "plaintext", "alpha")
        .expect("didOpen");
    assert_eq!(delivered, 1);
    let opened = sink
        .wait_for(
            |event| event.path == document_path && !event.diagnostics.is_empty(),
            WAIT,
        )
        .expect("publishDiagnostics for didOpen");
    assert_eq!(opened.server_id, "fake");
    assert_eq!(
        opened.diagnostics,
        vec![Diagnostic {
            severity: 1,
            message: "fake: unknown token".into(),
            line: 2,
            column: 3,
            end_line: 2,
            end_column: 8,
        }]
    );
    assert!(
        wait_for_log(&log, "\"method\":\"textDocument/didOpen\"", WAIT),
        "didOpen was not sent: {}",
        read_log(&log)
    );

    let delivered = manager
        .did_change(&document_path, "beta")
        .expect("didChange");
    assert_eq!(delivered, 1);
    let changed = sink
        .wait_for(
            |event| {
                event.path == document_path
                    && event
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.message == "changed:beta")
            },
            WAIT,
        )
        .expect("publishDiagnostics for didChange");
    assert_eq!(changed.diagnostics[0].severity, 2);
    assert!(
        wait_for_log(&log, "\"method\":\"textDocument/didChange\"", WAIT),
        "didChange was not sent: {}",
        read_log(&log)
    );

    let delivered = manager.did_close(&document_path).expect("didClose");
    assert_eq!(delivered, 1);
    let closed = sink
        .wait_for(
            |event| event.path == document_path && event.diagnostics.is_empty(),
            WAIT,
        )
        .expect("empty publishDiagnostics for didClose");
    assert!(closed.diagnostics.is_empty());
    assert!(
        wait_for_log(&log, "\"method\":\"textDocument/didClose\"", WAIT),
        "didClose was not sent: {}",
        read_log(&log)
    );

    manager.stop_all();
    assert!(!manager.is_running("fake"));
    assert!(
        wait_for_log(&log, "\"method\":\"shutdown\"", WAIT),
        "shutdown was not sent: {}",
        read_log(&log)
    );
    assert!(
        wait_for_log(&log, "\"method\":\"exit\"", WAIT),
        "exit was not sent: {}",
        read_log(&log)
    );
}

#[test]
fn lsp_start_failure_is_isolated_from_other_servers() {
    if !node_available() {
        eprintln!("skipping lsp isolation test: node not found in PATH");
        return;
    }

    let dir = TempDir::new("isolation");
    let log = dir.path().join("fake-lsp.log");
    let sink = Arc::new(RecordingSink::default());
    let mut manager = LspManager::default();

    let mut missing = LspServerConfig::new("missing", "lc-lsp-command-that-does-not-exist-7f4a1c");
    missing.language_ids = vec!["plaintext".to_string()];

    let report = manager.start_all(vec![missing, fake_config("fake", &log)], dir.path(), sink);
    assert_eq!(report.servers.len(), 1);
    assert_eq!(report.servers[0].id, "fake");
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].id, "missing");
    assert!(manager.is_running("fake"));
    assert!(!manager.is_running("missing"));

    manager.stop_all();
    assert!(
        wait_for_log(&log, "\"method\":\"exit\"", WAIT),
        "fake server did not stop: {}",
        read_log(&log)
    );
}
