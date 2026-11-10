//! 单个 stdio LSP server 客户端：Content-Length framing、JSON-RPC 2.0、
//! 文档同步、诊断接收与补全 / 悬停 / 跳转请求。
//!
//! 消息读取在独立线程完成：响应经 `mpsc` 交给 [`LspClient::initialize`] /
//! `shutdown` 的等待方；`publishDiagnostics` 通知直接分发给 [`DiagnosticSink`]；
//! server 反向请求（`workspace/configuration` 等）用空结果应答，避免 server
//! 阻塞。请求等待用 `recv_timeout` 实现超时，超时立即终止子进程。
//!
//! 进程回收：`stop` / `drop` 先尽力发送 `shutdown` + `exit`，随后关 stdin；
//! 宽限期内未退出则强制终止。Windows 用独立 Job Object（失败时回退
//! `taskkill /T /F`），unix 用独立进程组 + `SIGKILL`。

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use lc_proto::{LspCompletionItem, LspHover, LspHoverContent, LspLocation, LspTextEdit};

use super::LspError;
use super::config::LspServerConfig;
use super::diagnostic::{Diagnostic, DiagnosticSink};
use super::uri::{path_key, path_to_uri, resolve_path, uri_to_path};
use crate::process::scrub_secret_env;

/// `initialize` 请求超时。
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(10);
/// `shutdown` 请求超时；超时后直接杀进程，不阻塞退出流程。
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// completion / hover / definition 请求超时；超时按既有语义终止子进程。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

const STOP_GRACE: Duration = Duration::from_millis(750);
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_STDERR_LINES: usize = 40;

#[derive(Debug)]
struct Document {
    uri: String,
    version: i64,
    text: String,
}

/// 打开文档与 URI → 原始 path 的映射。
///
/// `uri_paths` 在 didClose 后保留：server 随后下发的空诊断仍能映射回前端
/// 使用的原始 path（否则清除标记可能落到另一种路径写法上）。
#[derive(Default)]
struct DocumentStore {
    open: HashMap<String, Document>,
    uri_paths: HashMap<String, String>,
}

impl DocumentStore {
    fn path_for_uri(&self, uri: &str) -> Option<String> {
        if let Some(path) = self.uri_paths.get(uri) {
            return Some(path.clone());
        }
        // server 可能改写盘符大小写或 `/`、`\`；按归一化 path 再匹配一次。
        let decoded = uri_to_path(uri)?;
        let key = path_key(&decoded);
        self.uri_paths
            .values()
            .find(|path| path_key(path) == key)
            .cloned()
    }
}

/// 一个已启动（未必已 initialize）的 LSP server。
pub struct LspClient {
    server_id: String,
    workspace_root: PathBuf,
    child: Child,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    responses: Receiver<Result<Value, String>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    documents: Arc<Mutex<DocumentStore>>,
    next_id: u64,
    /// server 在 initialize 中声明的 `textDocumentSync`（1 Full / 2 Incremental）。
    sync_kind: u8,
    initialized: bool,
    stopped: bool,
    reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

impl LspClient {
    /// 启动 server 子进程并启动读线程；不发送 `initialize`。
    ///
    /// `workspace_root` 同时作为子进程工作目录与 `rootUri`。
    pub fn spawn(
        config: &LspServerConfig,
        workspace_root: &Path,
        sink: Arc<dyn DiagnosticSink>,
    ) -> Result<Self, LspError> {
        let server = config.id.clone();
        let command = config.command.trim();
        if command.is_empty() {
            return Err(LspError::Spawn {
                server,
                message: "command 为空".into(),
            });
        }

        let mut command = Command::new(command);
        command
            .args(&config.args)
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 先清掉父进程 provider 密钥，再合并配置显式声明的 env。
        scrub_secret_env(&mut command);
        for (key, value) in &config.env {
            command.env(key, value);
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(WINDOWS_CREATE_NO_WINDOW);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // 独立进程组：超时/drop 时对 -pid 发 SIGKILL，孙进程一并回收。
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|error| LspError::Spawn {
            server: server.clone(),
            message: error.to_string(),
        })?;

        #[cfg(windows)]
        let job = match attach_job(&child) {
            Ok(job) => Some(job),
            Err(message) => {
                // 进程树回收尽力而为：仍可用 taskkill 回退，不因此拒绝启动。
                eprintln!("lsp: {server} 进程树监控不可用：{message}");
                None
            }
        };

        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LspError::Spawn {
                server,
                message: "未能取得 stdin 管道".into(),
            });
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LspError::Spawn {
                server,
                message: "未能取得 stdout 管道".into(),
            });
        };
        let stderr = child.stderr.take();

        let stdin = Arc::new(Mutex::new(Some(stdin)));
        let documents = Arc::new(Mutex::new(DocumentStore::default()));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));

        let stderr_reader = stderr.and_then(|stderr| {
            let stderr_tail = Arc::clone(&stderr_tail);
            std::thread::Builder::new()
                .name(format!("lsp-stderr-{server}"))
                .spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines() {
                        let Ok(line) = line else { break };
                        let mut guard = stderr_tail.lock().unwrap_or_else(PoisonError::into_inner);
                        if guard.len() >= MAX_STDERR_LINES {
                            guard.pop_front();
                        }
                        guard.push_back(line);
                    }
                })
                .ok()
        });

        let (sender, responses) = mpsc::channel();
        let reader = spawn_stdout_reader(
            server.clone(),
            stdout,
            Arc::clone(&stdin),
            Arc::clone(&documents),
            sink,
            sender,
        )
        .map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            LspError::Spawn {
                server: server.clone(),
                message: format!("启动 stdout 读线程失败：{error}"),
            }
        })?;

        Ok(Self {
            server_id: server,
            workspace_root: workspace_root.to_path_buf(),
            child,
            stdin,
            responses,
            stderr_tail,
            documents,
            next_id: 0,
            sync_kind: 1,
            initialized: false,
            stopped: false,
            reader: Some(reader),
            stderr_reader,
            #[cfg(windows)]
            job,
        })
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// 子进程尚未被本客户端终止（不代表 server 仍健康）。
    pub fn is_running(&self) -> bool {
        !self.stopped
    }

    /// 完整握手：`initialize` → 等响应 → `initialized` 通知。
    pub fn initialize(&mut self) -> Result<(), LspError> {
        if self.initialized {
            return Ok(());
        }

        let root_uri = path_to_uri(&self.workspace_root);
        let workspace_uri = root_uri.clone();
        let root_path = self.workspace_root.to_string_lossy().to_string();
        let workspace_name = self
            .workspace_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_string();
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "lite-code", "version": env!("CARGO_PKG_VERSION")},
            "rootPath": root_path,
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": workspace_uri, "name": workspace_name}],
            "capabilities": {
                "textDocument": {
                    "synchronization": {"dynamicRegistration": false},
                    "publishDiagnostics": {
                        "relatedInformation": false,
                        "versionSupport": false
                    }
                },
                "workspace": {"configuration": false, "workspaceFolders": false}
            }
        });
        let result = self.request("initialize", params, INITIALIZE_TIMEOUT)?;
        if !result.is_object() {
            return Err(LspError::Protocol {
                server: self.server_id.clone(),
                message: "initialize 响应 result 不是对象".into(),
            });
        }
        self.sync_kind = result
            .pointer("/capabilities/textDocumentSync")
            .map(parse_sync_kind)
            .unwrap_or(1);
        self.notify("initialized", None)?;
        self.initialized = true;
        Ok(())
    }

    /// 打开文档（全量文本）。同一路径重复 open 退化为一次全量 didChange。
    pub fn did_open(&mut self, path: &str, language_id: &str, text: &str) -> Result<(), LspError> {
        if !self.initialized {
            return Err(LspError::NotInitialized {
                server: self.server_id.clone(),
            });
        }

        let display = resolve_path(path, &self.workspace_root)
            .to_string_lossy()
            .into_owned();
        let key = path_key(&display);
        let uri = path_to_uri(Path::new(&display));

        let existing = {
            let mut guard = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.open.get_mut(&key).map(|document| {
                document.version = document.version.saturating_add(1);
                (
                    document.uri.clone(),
                    document.version,
                    document.text.clone(),
                )
            })
        };
        if let Some((uri, version, old_text)) = existing {
            let result = self.notify(
                "textDocument/didChange",
                Some(change_params(
                    &uri,
                    version,
                    &old_text,
                    text,
                    self.sync_kind,
                )),
            );
            if result.is_ok() {
                let mut guard = self
                    .documents
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if let Some(document) = guard.open.get_mut(&key) {
                    document.text = text.to_string();
                }
            }
            return result;
        }

        {
            let mut guard = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.uri_paths.insert(uri.clone(), display);
            guard.open.insert(
                key.clone(),
                Document {
                    uri: uri.clone(),
                    version: 1,
                    text: text.to_string(),
                },
            );
        }

        let result = self.notify(
            "textDocument/didOpen",
            Some(json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text
                }
            })),
        );
        if result.is_err() {
            let mut guard = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.open.remove(&key);
        }
        result
    }

    /// 全量文本变更；未打开的路径返回 [`LspError::DocumentNotOpen`]。
    pub fn did_change(&mut self, path: &str, text: &str) -> Result<(), LspError> {
        let display = resolve_path(path, &self.workspace_root)
            .to_string_lossy()
            .into_owned();
        let key = path_key(&display);
        let (uri, version, old_text) = {
            let mut guard = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(document) = guard.open.get_mut(&key) else {
                return Err(LspError::DocumentNotOpen {
                    path: path.to_string(),
                });
            };
            document.version = document.version.saturating_add(1);
            (
                document.uri.clone(),
                document.version,
                document.text.clone(),
            )
        };
        let result = self.notify(
            "textDocument/didChange",
            Some(change_params(
                &uri,
                version,
                &old_text,
                text,
                self.sync_kind,
            )),
        );
        if result.is_ok() {
            let mut guard = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(document) = guard.open.get_mut(&key) {
                document.text = text.to_string();
            }
        }
        result
    }

    /// 关闭文档；未打开时幂等返回 `Ok`。
    pub fn did_close(&mut self, path: &str) -> Result<(), LspError> {
        let display = resolve_path(path, &self.workspace_root)
            .to_string_lossy()
            .into_owned();
        let key = path_key(&display);
        let uri = {
            let mut guard = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.open.remove(&key).map(|document| document.uri)
        };
        let Some(uri) = uri else {
            return Ok(());
        };
        self.notify(
            "textDocument/didClose",
            Some(json!({"textDocument": {"uri": uri}})),
        )
    }

    /// 该路径当前是否已 didOpen。
    pub fn is_open(&self, path: &str) -> bool {
        let display = resolve_path(path, &self.workspace_root)
            .to_string_lossy()
            .into_owned();
        let key = path_key(&display);
        self.documents
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .open
            .contains_key(&key)
    }

    /// `textDocument/completion`；文档未 didOpen 返回 [`LspError::DocumentNotOpen`]。
    ///
    /// `line` / `column` 为 1-based（与前端坐标一致），内部换算成 LSP 的
    /// 0-based UTF-16 position。
    pub fn completion(
        &mut self,
        path: &str,
        line: u32,
        column: u32,
    ) -> Result<Vec<LspCompletionItem>, LspError> {
        let params = self.text_document_position(path, line, column)?;
        let result = self.request("textDocument/completion", params, REQUEST_TIMEOUT)?;
        Ok(parse_completion_result(&result))
    }

    /// `textDocument/hover`；该位置无悬停时 `contents` 为空数组。
    pub fn hover(&mut self, path: &str, line: u32, column: u32) -> Result<LspHover, LspError> {
        let params = self.text_document_position(path, line, column)?;
        let result = self.request("textDocument/hover", params, REQUEST_TIMEOUT)?;
        Ok(parse_hover_result(&result))
    }

    /// `textDocument/definition`；Location / LocationLink 拍平为位置列表，
    /// 空数组表示无定义。
    pub fn definition(
        &mut self,
        path: &str,
        line: u32,
        column: u32,
    ) -> Result<Vec<LspLocation>, LspError> {
        let params = self.text_document_position(path, line, column)?;
        let result = self.request("textDocument/definition", params, REQUEST_TIMEOUT)?;
        Ok(parse_definition_result(&result))
    }

    /// 已打开文档的 URI 与 LSP position 参数；未打开返回 DocumentNotOpen。
    fn text_document_position(
        &self,
        path: &str,
        line: u32,
        column: u32,
    ) -> Result<Value, LspError> {
        let display = resolve_path(path, &self.workspace_root)
            .to_string_lossy()
            .into_owned();
        let key = path_key(&display);
        let uri = {
            let guard = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.open.get(&key).map(|document| document.uri.clone())
        };
        let Some(uri) = uri else {
            return Err(LspError::DocumentNotOpen {
                path: path.to_string(),
            });
        };
        Ok(json!({
            "textDocument": {"uri": uri},
            "position": {
                "line": line.saturating_sub(1),
                "character": column.saturating_sub(1),
            }
        }))
    }

    /// 尽力优雅关闭并回收子进程；幂等。
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        if self.initialized {
            self.initialized = false;
            let _ = self.request("shutdown", Value::Null, SHUTDOWN_TIMEOUT);
            let _ = self.notify("exit", None);
        }
        self.close_stdin();
        self.wait_exit(STOP_GRACE);
        self.terminate();
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        if self.stopped {
            return Err(self.closed_error());
        }
        self.next_id = self.next_id.saturating_add(1);
        let id = json!(self.next_id);
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        if let Err(message) = write_message(&self.stdin, &message) {
            self.terminate();
            return Err(LspError::Io {
                server: self.server_id.clone(),
                message,
            });
        }

        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.responses.recv_timeout(remaining) {
                Ok(Ok(value)) => {
                    if value.get("id") != Some(&id) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("未知 JSON-RPC 错误")
                            .to_string();
                        return Err(LspError::Rpc {
                            server: self.server_id.clone(),
                            code,
                            message,
                        });
                    }
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                Ok(Err(message)) => {
                    self.terminate();
                    return Err(LspError::Protocol {
                        server: self.server_id.clone(),
                        message,
                    });
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.terminate();
                    return Err(LspError::Timeout {
                        server: self.server_id.clone(),
                        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    });
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.terminate();
                    return Err(self.closed_error());
                }
            }
        }
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), LspError> {
        if self.stopped {
            return Err(self.closed_error());
        }
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        if let Err(message) = write_message(&self.stdin, &message) {
            self.terminate();
            return Err(LspError::Io {
                server: self.server_id.clone(),
                message,
            });
        }
        Ok(())
    }

    fn close_stdin(&self) {
        self.stdin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return true,
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn terminate(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.initialized = false;
        self.close_stdin();

        #[cfg(unix)]
        {
            let _ = kill_process_group(self.child.id());
        }
        #[cfg(windows)]
        {
            if self.job.take().is_none() {
                kill_process_tree_windows(self.child.id());
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();

        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_reader.take() {
            let _ = handle.join();
        }
    }

    fn closed_error(&self) -> LspError {
        let tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let message = if tail.is_empty() {
            "子进程 stdout 已关闭".to_string()
        } else {
            format!("子进程 stdout 已关闭；stderr:\n{tail}")
        };
        LspError::Closed {
            server: self.server_id.clone(),
            message,
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_stdout_reader(
    server: String,
    stdout: ChildStdout,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    documents: Arc<Mutex<DocumentStore>>,
    sink: Arc<dyn DiagnosticSink>,
    sender: Sender<Result<Value, String>>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("lsp-stdout-{server}"))
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if let Some(method) = message.get("method").and_then(Value::as_str) {
                            if message.get("id").is_some() {
                                respond_to_server_request(&message, &stdin);
                            } else if method == "textDocument/publishDiagnostics" {
                                handle_publish_diagnostics(
                                    &server,
                                    &message,
                                    &documents,
                                    sink.as_ref(),
                                );
                            }
                        } else if sender.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        })
}

fn respond_to_server_request(message: &Value, stdin: &Mutex<Option<ChildStdin>>) {
    let Some(id) = message.get("id").cloned() else {
        return;
    };
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "workspace/configuration" => {
            let count = message
                .pointer("/params/items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Value::Array(vec![Value::Null; count])
        }
        "workspace/workspaceFolders" => json!([]),
        "workspace/applyEdit" => json!({"applied": false}),
        "window/showDocument" => json!({"success": false}),
        _ => Value::Null,
    };
    let response = json!({"jsonrpc": "2.0", "id": id, "result": result});
    let _ = write_message(stdin, &response);
}

fn handle_publish_diagnostics(
    server: &str,
    message: &Value,
    documents: &Mutex<DocumentStore>,
    sink: &dyn DiagnosticSink,
) {
    let Some(params) = message.get("params") else {
        return;
    };
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return;
    };
    let path = {
        let guard = documents.lock().unwrap_or_else(PoisonError::into_inner);
        guard.path_for_uri(uri)
    }
    .or_else(|| uri_to_path(uri))
    .unwrap_or_else(|| uri.to_string());
    let diagnostics = parse_diagnostics(params);
    sink.publish(server, &path, &diagnostics);
}

fn parse_sync_kind(value: &Value) -> u8 {
    let kind = match value {
        Value::Number(number) => number.as_u64(),
        Value::Object(object) => object.get("change").and_then(Value::as_u64),
        _ => None,
    };
    kind.and_then(|kind| u8::try_from(kind).ok())
        .filter(|kind| *kind <= 2)
        .unwrap_or(1)
}

/// 全量同步 server 发 `{text}`；增量同步 server 发一次覆盖整篇文档的 range
/// 变更（v0 不计算最小编辑区间）。
fn change_params(uri: &str, version: i64, old_text: &str, text: &str, sync_kind: u8) -> Value {
    let change = if sync_kind == 2 {
        let (line, character) = end_position(old_text);
        json!({
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": line, "character": character}
            },
            "text": text
        })
    } else {
        json!({"text": text})
    };
    json!({
        "textDocument": {"uri": uri, "version": version},
        "contentChanges": [change]
    })
}

/// 文本末尾的 LSP position（0-based，character 按 UTF-16 code unit 计数）。
fn end_position(text: &str) -> (u32, u32) {
    let line = u32::try_from(text.matches('\n').count()).unwrap_or(u32::MAX);
    let last_line = text.rsplit('\n').next().unwrap_or_default();
    let character =
        u32::try_from(last_line.trim_end_matches('\r').encode_utf16().count()).unwrap_or(u32::MAX);
    (line, character)
}

fn parse_diagnostics(params: &Value) -> Vec<Diagnostic> {
    params
        .get("diagnostics")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_diagnostic).collect())
        .unwrap_or_default()
}

fn parse_diagnostic(value: &Value) -> Diagnostic {
    let start = value.pointer("/range/start");
    let end = value.pointer("/range/end");
    let line = position(start, "line").unwrap_or(0).saturating_add(1);
    let column = position(start, "character").unwrap_or(0).saturating_add(1);
    let end_line = position(end, "line").map_or(line, |line| line.saturating_add(1));
    let end_column = position(end, "character").map_or(column, |column| column.saturating_add(1));
    let severity = value
        .get("severity")
        .and_then(Value::as_u64)
        .and_then(|severity| u8::try_from(severity).ok())
        .unwrap_or(1)
        .clamp(1, 4);

    Diagnostic {
        severity,
        message: value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        line,
        column,
        end_line,
        end_column,
    }
}

fn position(point: Option<&Value>, key: &str) -> Option<u32> {
    point
        .and_then(|point| point.get(key))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

/// completion 结果：CompletionItem[] | CompletionList | null 三种形态；
/// label 缺失或为空的项跳过，kind 只保留 LSP 规定的 1–25。
fn parse_completion_result(value: &Value) -> Vec<LspCompletionItem> {
    let items: &[Value] = match value {
        Value::Null => &[],
        Value::Array(items) => items,
        Value::Object(_) => value
            .get("items")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice),
        _ => &[],
    };
    items.iter().filter_map(parse_completion_item).collect()
}

fn parse_completion_item(value: &Value) -> Option<LspCompletionItem> {
    let label = value.get("label").and_then(Value::as_str)?;
    if label.is_empty() {
        return None;
    }
    let kind = value
        .get("kind")
        .and_then(Value::as_u64)
        .and_then(|kind| u8::try_from(kind).ok())
        .filter(|kind| (1..=25).contains(kind));
    let detail = value
        .get("detail")
        .and_then(Value::as_str)
        .filter(|detail| !detail.is_empty())
        .map(str::to_string);
    let insert_text = value
        .get("insertText")
        .and_then(Value::as_str)
        .map(str::to_string);
    // TextEdit 与 InsertReplaceEdit 的 v0 统一取 newText（plain insert）。
    let text_edit = value
        .get("textEdit")
        .and_then(|edit| edit.get("newText"))
        .and_then(Value::as_str)
        .map(|new_text| LspTextEdit {
            new_text: new_text.to_string(),
        });
    Some(LspCompletionItem {
        label: label.to_string(),
        kind,
        detail,
        insert_text,
        text_edit,
    })
}

/// hover 结果：null 或无 contents 视为无悬停。
fn parse_hover_result(value: &Value) -> LspHover {
    let contents = value
        .get("contents")
        .map(parse_hover_contents)
        .unwrap_or_default();
    LspHover { contents }
}

/// MarkupContent | MarkedString | MarkedString[] 拍平为段列表。
fn parse_hover_contents(value: &Value) -> Vec<LspHoverContent> {
    match value {
        Value::String(text) => vec![LspHoverContent {
            value: text.clone(),
            is_markdown: true,
        }],
        Value::Array(parts) => parts.iter().flat_map(parse_marked_string).collect(),
        Value::Object(_) => {
            if let Some(kind) = value.get("kind").and_then(Value::as_str) {
                let value = value
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                vec![LspHoverContent {
                    value,
                    is_markdown: kind == "markdown",
                }]
            } else {
                parse_marked_string(value)
            }
        }
        _ => Vec::new(),
    }
}

/// MarkedString：字符串形式按 markdown 处理；`{language, value}` 包成围栏
/// 代码块；其它形状忽略。
fn parse_marked_string(value: &Value) -> Vec<LspHoverContent> {
    if let Some(text) = value.as_str() {
        return vec![LspHoverContent {
            value: text.to_string(),
            is_markdown: true,
        }];
    }
    if let (Some(language), Some(code)) = (
        value.get("language").and_then(Value::as_str),
        value.get("value").and_then(Value::as_str),
    ) {
        return vec![LspHoverContent {
            value: format!("```{language}\n{code}\n```"),
            is_markdown: true,
        }];
    }
    Vec::new()
}

/// definition 结果：Location | Location[] | LocationLink[] | null 拍平为
/// 位置列表；无法转成路径的项跳过。
fn parse_definition_result(value: &Value) -> Vec<LspLocation> {
    match value {
        Value::Null => Vec::new(),
        Value::Array(items) => items.iter().filter_map(parse_location_like).collect(),
        Value::Object(_) => parse_location_like(value).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Location `{uri, range}` 与 LocationLink `{targetUri, targetSelectionRange}`
/// 统一取起始位置，转成 1-based 行列。
fn parse_location_like(value: &Value) -> Option<LspLocation> {
    let (uri, range) = if value.get("uri").is_some() {
        (value.get("uri")?, value.get("range"))
    } else if value.get("targetUri").is_some() {
        let range = value
            .get("targetSelectionRange")
            .or_else(|| value.get("targetRange"));
        (value.get("targetUri")?, range)
    } else {
        return None;
    };
    let path = uri_to_path(uri.as_str()?)?;
    let start = range.and_then(|range| range.get("start"));
    Some(LspLocation {
        path,
        line: position(start, "line").unwrap_or(0).saturating_add(1),
        column: position(start, "character").unwrap_or(0).saturating_add(1),
    })
}

fn read_message<R: BufRead>(reader: &mut R) -> std::io::Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse::<usize>().ok();
        }
    }

    let Some(length) = content_length else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "缺少 Content-Length",
        ));
    };
    if length == 0 || length > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Content-Length 越界：{length}"),
        ));
    }

    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("消息不是合法 JSON：{error}"),
        )
    })
}

fn write_message(stdin: &Mutex<Option<ChildStdin>>, message: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    let length = body.len();
    let mut frame = format!("Content-Length: {length}\r\n\r\n").into_bytes();
    frame.extend_from_slice(&body);

    let mut guard = stdin.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(stream) = guard.as_mut() else {
        return Err("stdin 已关闭".to_string());
    };
    stream
        .write_all(&frame)
        .and_then(|()| stream.flush())
        .map_err(|error| error.to_string())
}

#[cfg(windows)]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn attach_job(child: &Child) -> Result<win32job::Job, String> {
    use std::os::windows::io::AsRawHandle;

    let mut info = win32job::ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&info)
        .map_err(|error| format!("创建 Job Object 失败：{error}"))?;
    job.assign_process(child.as_raw_handle() as isize)
        .map_err(|error| format!("子进程加入 Job Object 失败：{error}"))?;
    Ok(job)
}

#[cfg(windows)]
fn kill_process_tree_windows(pid: u32) {
    use std::os::windows::process::CommandExt;

    let pid = pid.to_string();
    let mut command = Command::new("taskkill");
    command
        .args(["/PID", pid.as_str(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(WINDOWS_CREATE_NO_WINDOW);
    let _ = command.status();
}

/// 对子进程所在进程组发 SIGKILL；组不存在（已退出）视为成功。
#[cfg(unix)]
fn kill_process_group(pid: u32) -> std::io::Result<()> {
    let Ok(pid) = i32::try_from(pid) else {
        return Ok(());
    };
    // SAFETY: kill(2) 是线程安全系统调用；负 pid 表示向该进程组发送信号。
    let rc = unsafe { unix_ffi::kill(-pid, unix_ffi::SIGKILL) };
    if rc == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(unix_ffi::ESRCH) {
        return Ok(());
    }
    Err(error)
}

#[cfg(unix)]
mod unix_ffi {
    use std::ffi::c_int;

    unsafe extern "C" {
        pub fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    pub const SIGKILL: c_int = 9; // signal.h
    pub const ESRCH: c_int = 3; // errno.h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(body: &str) -> Vec<u8> {
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend_from_slice(body.as_bytes());
        frame
    }

    #[test]
    fn reads_content_length_frames_then_eof() {
        let mut input = frame(r#"{"jsonrpc":"2.0","id":1,"result":null}"#);
        input.extend(frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics"}"#,
        ));
        let mut reader = BufReader::new(Cursor::new(input));

        let first = read_message(&mut reader)
            .expect("first frame")
            .expect("message");
        assert_eq!(first["id"], 1);
        let second = read_message(&mut reader)
            .expect("second frame")
            .expect("message");
        assert_eq!(second["method"], "textDocument/publishDiagnostics");
        assert!(read_message(&mut reader).expect("eof").is_none());
    }

    #[test]
    fn rejects_missing_or_oversized_content_length() {
        let mut missing = BufReader::new(Cursor::new(b"Content-Type: x\r\n\r\n{}".to_vec()));
        assert!(read_message(&mut missing).is_err());

        let mut oversized = BufReader::new(Cursor::new(
            format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1).into_bytes(),
        ));
        assert!(read_message(&mut oversized).is_err());

        let mut empty = BufReader::new(Cursor::new(b"Content-Length: 0\r\n\r\n".to_vec()));
        assert!(read_message(&mut empty).is_err());
    }

    #[test]
    fn diagnostics_are_converted_to_one_based_positions() {
        let params = json!({
            "uri": "file:///tmp/x.rs",
            "diagnostics": [{
                "range": {
                    "start": {"line": 1, "character": 2},
                    "end": {"line": 1, "character": 7}
                },
                "severity": 2,
                "message": "unused variable"
            }]
        });
        let diagnostics = parse_diagnostics(&params);
        assert_eq!(
            diagnostics,
            vec![Diagnostic {
                severity: 2,
                message: "unused variable".into(),
                line: 2,
                column: 3,
                end_line: 2,
                end_column: 8,
            }]
        );
    }

    #[test]
    fn missing_severity_range_and_message_use_safe_defaults() {
        let diagnostics = parse_diagnostics(&json!({"diagnostics": [{}]}));
        assert_eq!(
            diagnostics,
            vec![Diagnostic {
                severity: 1,
                message: String::new(),
                line: 1,
                column: 1,
                end_line: 1,
                end_column: 1,
            }]
        );
        let clamped = parse_diagnostics(&json!({
            "diagnostics": [{"severity": 99, "range": {"start": {"line": 0, "character": 0}}}]
        }));
        assert_eq!(clamped[0].severity, 4);
    }

    #[test]
    fn sync_kind_accepts_number_and_object_shapes() {
        assert_eq!(parse_sync_kind(&json!(1)), 1);
        assert_eq!(parse_sync_kind(&json!(2)), 2);
        assert_eq!(parse_sync_kind(&json!({"change": 2})), 2);
        assert_eq!(parse_sync_kind(&json!(99)), 1);
        assert_eq!(parse_sync_kind(&json!(null)), 1);
    }

    #[test]
    fn incremental_change_replaces_the_whole_document() {
        let full = change_params("file:///tmp/x", 2, "alpha", "beta", 1);
        assert_eq!(full["contentChanges"][0]["text"], "beta");
        assert!(full["contentChanges"][0].get("range").is_none());

        let incremental = change_params("file:///tmp/x", 2, "a\nbc", "x", 2);
        assert_eq!(
            incremental["contentChanges"][0]["range"]["start"]["line"],
            0
        );
        assert_eq!(incremental["contentChanges"][0]["range"]["end"]["line"], 1);
        assert_eq!(
            incremental["contentChanges"][0]["range"]["end"]["character"],
            2
        );

        // LSP character 是 UTF-16 code unit：emoji 占 2。
        assert_eq!(end_position("😀"), (0, 2));
    }

    #[test]
    fn completion_parses_array_and_completion_list_shapes() {
        let item = |label: &str, extra: Value| {
            let mut value = json!({"label": label});
            if let (Some(object), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
                object.extend(extra.clone());
            }
            value
        };
        let array = parse_completion_result(&json!([
            item("alpha", json!({"kind": 3, "detail": "fn"})),
            item("beta", json!({"kind": 6, "insertText": "beta()"})),
            item(
                "gamma",
                json!({"textEdit": {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "gamma!"}})
            ),
            json!({"kind": 1}),
            "not-an-object",
        ]));
        assert_eq!(
            array,
            vec![
                LspCompletionItem {
                    label: "alpha".into(),
                    kind: Some(3),
                    detail: Some("fn".into()),
                    insert_text: None,
                    text_edit: None,
                },
                LspCompletionItem {
                    label: "beta".into(),
                    kind: Some(6),
                    detail: None,
                    insert_text: Some("beta()".into()),
                    text_edit: None,
                },
                LspCompletionItem {
                    label: "gamma".into(),
                    kind: None,
                    detail: None,
                    insert_text: None,
                    text_edit: Some(LspTextEdit {
                        new_text: "gamma!".into(),
                    }),
                },
            ]
        );

        let list = parse_completion_result(&json!({
            "isIncomplete": false,
            "items": [item("delta", json!({"kind": 14}))]
        }));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label, "delta");
        assert_eq!(list[0].kind, Some(14));

        assert!(parse_completion_result(&Value::Null).is_empty());
        assert!(parse_completion_result(&json!({"unexpected": true})).is_empty());
    }

    #[test]
    fn completion_kind_is_clamped_to_lsp_range() {
        let items = parse_completion_result(&json!([
            {"label": "zero", "kind": 0},
            {"label": "ok", "kind": 25},
            {"label": "too-big", "kind": 99},
        ]));
        assert_eq!(items[0].kind, None);
        assert_eq!(items[1].kind, Some(25));
        assert_eq!(items[2].kind, None);
    }

    #[test]
    fn hover_flattens_markup_and_marked_string_shapes() {
        let markdown = parse_hover_result(&json!({
            "contents": {"kind": "markdown", "value": "**bold**"}
        }));
        assert_eq!(
            markdown.contents,
            vec![LspHoverContent {
                value: "**bold**".into(),
                is_markdown: true,
            }]
        );

        let plain = parse_hover_result(&json!({
            "contents": {"kind": "plaintext", "value": "a_b"}
        }));
        assert_eq!(
            plain.contents,
            vec![LspHoverContent {
                value: "a_b".into(),
                is_markdown: false,
            }]
        );

        let marked = parse_hover_result(&json!({
            "contents": [
                "raw markdown",
                {"language": "rust", "value": "fn main() {}"},
                {"unsupported": true},
            ]
        }));
        assert_eq!(marked.contents.len(), 2);
        assert_eq!(marked.contents[0].value, "raw markdown");
        assert!(marked.contents[0].is_markdown);
        assert_eq!(marked.contents[1].value, "```rust\nfn main() {}\n```");
        assert!(marked.contents[1].is_markdown);

        assert!(parse_hover_result(&Value::Null).contents.is_empty());
        assert!(parse_hover_result(&json!({})).contents.is_empty());
    }

    #[test]
    fn definition_flattens_location_and_location_link_shapes() {
        #[cfg(windows)]
        let (main_rs, lib_rs, jump_txt) = (r"\tmp\main.rs", r"\tmp\lib.rs", r"\tmp\jump.txt");
        #[cfg(not(windows))]
        let (main_rs, lib_rs, jump_txt) = ("/tmp/main.rs", "/tmp/lib.rs", "/tmp/jump.txt");

        let single = parse_definition_result(&json!({
            "uri": "file:///tmp/main.rs",
            "range": {
                "start": {"line": 0, "character": 4},
                "end": {"line": 0, "character": 8}
            }
        }));
        assert_eq!(
            single,
            vec![LspLocation {
                path: main_rs.into(),
                line: 1,
                column: 5,
            }]
        );

        let mixed = parse_definition_result(&json!([
            {
                "targetUri": "file:///tmp/lib.rs",
                "targetRange": {"start": {"line": 2, "character": 0}},
                "targetSelectionRange": {"start": {"line": 2, "character": 3}}
            },
            {
                "uri": "file:///tmp/jump.txt",
                "range": {"start": {"line": 9, "character": 1}}
            },
        ]));
        assert_eq!(
            mixed,
            vec![
                LspLocation {
                    path: lib_rs.into(),
                    line: 3,
                    column: 4,
                },
                LspLocation {
                    path: jump_txt.into(),
                    line: 10,
                    column: 2,
                },
            ]
        );

        assert!(parse_definition_result(&Value::Null).is_empty());
        assert!(
            parse_definition_result(&json!([
                {"uri": "https://example.com/x.rs", "range": {"start": {"line": 0, "character": 0}}}
            ]))
            .is_empty()
        );
    }
}
