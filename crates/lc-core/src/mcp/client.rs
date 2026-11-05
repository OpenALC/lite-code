//! MCP JSON-RPC 2.0 客户端与 stdio transport。
//!
//! stdio 采用 newline-delimited JSON：每条消息一行。握手顺序：
//! `initialize` → 等响应 → `notifications/initialized` → `tools/list`
//! （支持 `nextCursor` 分页）。
//!
//! 超时由 reader 线程 + `mpsc::recv_timeout` 实现；初始化/列表 10s、
//! 工具调用 120s。超时立即终止子进程。transport 抽象成
//! [`McpTransport`]，测试可注入 fake，无需真实进程。

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agent::ToolOutput;

use super::McpError;
use super::config::McpServerConfig;

/// MCP 协议版本（2024-11-05）。
pub const PROTOCOL_VERSION: &str = "2024-11-05";
/// clientInfo.version。
pub const CLIENT_VERSION: &str = "0.2.1";

/// `initialize` 超时。
pub const INIT_TIMEOUT: Duration = Duration::from_secs(10);
/// `tools/list` 超时。
pub const LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// `tools/call` 超时。
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);

const MAX_STDERR_LINES: usize = 40;

/// `tools/list` 返回的远端工具。
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// transport 抽象：测试注入 fake，真实实现走 [`StdioTransport`]。
///
/// `message` 是完整 JSON-RPC 请求（含 id）；`request` 返回响应中的
/// `result` 成员。JSON-RPC `error` 映射为 [`McpError::Rpc`]。
pub trait McpTransport: Send {
    fn request(&mut self, message: &Value, timeout: Duration) -> Result<Value, McpError>;
    fn notify(&mut self, message: &Value) -> Result<(), McpError>;
}

/// 单个 stdio MCP server 客户端（握手 + 工具发现 + 调用）。
pub struct McpClient {
    server: String,
    transport: Box<dyn McpTransport>,
    next_id: u64,
    tools: Vec<RemoteTool>,
}

impl McpClient {
    pub fn new(server: impl Into<String>, transport: Box<dyn McpTransport>) -> Self {
        Self {
            server: server.into(),
            transport,
            next_id: 0,
            tools: Vec::new(),
        }
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// 已完成握手并缓存的工具列表。
    pub fn tools(&self) -> &[RemoteTool] {
        &self.tools
    }

    /// 完整握手：initialize → notifications/initialized → tools/list。
    pub fn initialize(&mut self) -> Result<(), McpError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "lite-code", "version": CLIENT_VERSION}
        });
        let result = self.request("initialize", params, INIT_TIMEOUT)?;
        if !result.is_object() {
            return Err(McpError::Protocol {
                server: self.server.clone(),
                message: "initialize 响应 result 不是对象".into(),
            });
        }
        self.notify("notifications/initialized")?;
        self.tools = self.fetch_tools()?;
        Ok(())
    }

    /// 调用远端工具，把 MCP `content` 数组渲染为文本。
    ///
    /// `isError: true` 返回 [`McpError::Tool`]，让上层观察者拿到 `ok=false`
    /// （与本地工具失败回填语义一致）。
    pub fn call_tool(&mut self, tool: &str, args: Value) -> Result<ToolOutput, McpError> {
        let arguments = if args.is_null() { json!({}) } else { args };
        let result = self.request(
            "tools/call",
            json!({"name": tool, "arguments": arguments}),
            CALL_TIMEOUT,
        )?;
        let text = render_content(&result);
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let message = if text.is_empty() {
                "工具返回 isError=true".to_string()
            } else {
                text
            };
            return Err(McpError::Tool {
                tool: tool.to_string(),
                message,
            });
        }
        Ok(ToolOutput { text })
    }

    fn fetch_tools(&mut self) -> Result<Vec<RemoteTool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        loop {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let result = self.request("tools/list", params, LIST_TIMEOUT)?;
            let Some(items) = result.get("tools").and_then(Value::as_array) else {
                return Err(McpError::Protocol {
                    server: self.server.clone(),
                    message: "tools/list 响应缺少 tools 数组".into(),
                });
            };
            for item in items {
                let Some(name) = item.get("name").and_then(Value::as_str) else {
                    continue;
                };
                tools.push(RemoteTool {
                    name: name.to_string(),
                    description: item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input_schema: item
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                });
            }
            match result.get("nextCursor").and_then(Value::as_str) {
                Some(next) => {
                    let next = next.to_string();
                    if !seen_cursors.insert(next.clone()) {
                        return Err(McpError::Protocol {
                            server: self.server.clone(),
                            message: format!("tools/list 分页 cursor 重复：{next}"),
                        });
                    }
                    cursor = Some(next);
                }
                None => break,
            }
        }
        Ok(tools)
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.next_id = self.next_id.saturating_add(1);
        let message = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params
        });
        self.transport.request(&message, timeout)
    }

    fn notify(&mut self, method: &str) -> Result<(), McpError> {
        let message = json!({"jsonrpc": "2.0", "method": method});
        self.transport.notify(&message)
    }
}

/// 拼接 MCP `content`：`text` 原样，其他类型用 `[type]` 占位。
fn render_content(result: &Value) -> String {
    let Some(content) = result.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    let parts: Vec<String> = content
        .iter()
        .map(|item| {
            let kind = item
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if kind == "text" {
                item.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            } else {
                format!("[{kind}]")
            }
        })
        .collect();
    parts.join("\n")
}

/// 真实 stdio transport：spawn 子进程，stdin 写 JSON 行，reader 线程收 stdout。
///
/// 回收：`drop` / [`shutdown`](Self::shutdown) / 超时都会终止子进程——
/// Windows 为每个 server 建一个 `KILL_ON_JOB_CLOSE` Job Object（关闭句柄
/// 即整树终止），unix 用独立进程组 + `SIGKILL`。显式 kill 后 `wait()` 收割。
pub struct StdioTransport {
    server: String,
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    responses: Receiver<Result<Value, String>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

impl StdioTransport {
    /// 以 `root` 为工作目录启动 server。
    pub fn spawn(server: &McpServerConfig, root: &Path) -> Result<Self, McpError> {
        if server.command.trim().is_empty() {
            return Err(McpError::Spawn {
                server: server.name.clone(),
                message: "command 为空".into(),
            });
        }

        let mut command = Command::new(&server.command);
        command
            .args(&server.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &server.env {
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

        let mut child = command.spawn().map_err(|error| McpError::Spawn {
            server: server.name.clone(),
            message: error.to_string(),
        })?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        #[cfg(windows)]
        let job = match attach_job(&child) {
            Ok(job) => Some(job),
            Err(message) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(McpError::Spawn {
                    server: server.name.clone(),
                    message,
                });
            }
        };

        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(McpError::Spawn {
                server: server.name.clone(),
                message: "未能取得 stdio 管道".into(),
            });
        };

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = stderr {
            spawn_stderr_reader(server.name.clone(), stderr, Arc::clone(&stderr_tail));
        }
        let (sender, responses) = mpsc::channel();
        spawn_stdout_reader(server.name.clone(), stdout, sender);

        Ok(Self {
            server: server.name.clone(),
            child,
            stdin: Some(stdin),
            responses,
            stderr_tail,
            #[cfg(windows)]
            job,
        })
    }

    /// 显式终止子进程（`drop` 也走同一路径）。幂等。
    pub fn shutdown(&mut self) {
        self.terminate();
    }

    fn terminate(&mut self) {
        self.stdin.take();
        #[cfg(unix)]
        {
            let _ = kill_process_group(self.child.id());
        }
        let _ = self.child.kill();
        #[cfg(windows)]
        {
            // Job 句柄关闭 → KILL_ON_JOB_CLOSE → 整树终止。
            self.job.take();
        }
        let _ = self.child.wait();
    }

    fn write_message(&mut self, message: &Value) -> Result<(), McpError> {
        let server = self.server.clone();
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(McpError::Closed {
                server,
                message: "stdin 已关闭".into(),
            });
        };
        let mut bytes = serde_json::to_vec(message).map_err(|error| McpError::Protocol {
            server: server.clone(),
            message: format!("请求序列化失败：{error}"),
        })?;
        bytes.push(b'\n');
        if let Err(error) = stdin.write_all(&bytes).and_then(|()| stdin.flush()) {
            self.terminate();
            return Err(McpError::Io {
                server,
                message: error.to_string(),
            });
        }
        Ok(())
    }

    fn closed_error(&self) -> McpError {
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
        McpError::Closed {
            server: self.server.clone(),
            message,
        }
    }
}

impl McpTransport for StdioTransport {
    fn request(&mut self, message: &Value, timeout: Duration) -> Result<Value, McpError> {
        self.write_message(message)?;
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.responses.recv_timeout(remaining) {
                Ok(Ok(value)) => {
                    // 忽略 server 发来的 notification / 反向请求。
                    if value.get("method").is_some() {
                        continue;
                    }
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
                        return Err(McpError::Rpc {
                            server: self.server.clone(),
                            code,
                            message,
                        });
                    }
                    return value
                        .get("result")
                        .cloned()
                        .ok_or_else(|| McpError::Protocol {
                            server: self.server.clone(),
                            message: "JSON-RPC 响应缺少 result".into(),
                        });
                }
                Ok(Err(message)) => {
                    return Err(McpError::Protocol {
                        server: self.server.clone(),
                        message,
                    });
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.terminate();
                    return Err(McpError::Timeout {
                        server: self.server.clone(),
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

    fn notify(&mut self, message: &Value) -> Result<(), McpError> {
        self.write_message(message)
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn spawn_stdout_reader(server: String, stdout: ChildStdout, sender: Sender<Result<Value, String>>) {
    let _ = std::thread::Builder::new()
        .name(format!("mcp-stdout-{server}"))
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ = sender.send(Err(format!("读取 {server} stdout 失败：{error}")));
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        if sender.send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        if sender
                            .send(Err(format!("{server} 输出不是合法 JSON-RPC：{error}")))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
}

fn spawn_stderr_reader(server: String, stderr: ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    let _ = std::thread::Builder::new()
        .name(format!("mcp-stderr-{server}"))
        .spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let mut guard = tail.lock().unwrap_or_else(PoisonError::into_inner);
                if guard.len() >= MAX_STDERR_LINES {
                    guard.pop_front();
                }
                guard.push_back(line);
            }
        });
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

/// 对子进程所在进程组发 SIGKILL；组不存在（已退出）视为成功。
#[cfg(unix)]
fn kill_process_group(pid: u32) -> std::io::Result<()> {
    let rc = unsafe { unix_ffi::kill(-(pid as i32), unix_ffi::SIGKILL) };
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
    use crate::mcp::test_support::{FakeReply, FakeTransport};
    use serde_json::json;

    fn initialize_reply() -> FakeReply {
        FakeReply::Result(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake", "version": "0"}
        }))
    }

    fn tools_reply(tools: Value) -> FakeReply {
        FakeReply::Result(json!({"tools": tools}))
    }

    #[test]
    fn handshake_sends_initialize_then_initialized_then_tools_list() {
        let transport = FakeTransport::new(
            "fake",
            vec![
                initialize_reply(),
                tools_reply(json!([
                    {"name": "echo", "description": "Echo", "inputSchema": {"type": "object"}}
                ])),
            ],
        );
        let requests = Arc::clone(&transport.requests);
        let notifications = Arc::clone(&transport.notifications);
        let timeouts = Arc::clone(&transport.timeouts);
        let mut client = McpClient::new("fake", Box::new(transport));

        client.initialize().expect("initialize");

        let requests = requests.lock().expect("requests");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["method"], "initialize");
        assert_eq!(requests[0]["params"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(requests[0]["params"]["clientInfo"]["name"], "lite-code");
        assert_eq!(requests[0]["params"]["clientInfo"]["version"], "0.2.1");
        assert_eq!(requests[1]["method"], "tools/list");

        let notifications = notifications.lock().expect("notifications");
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0]["method"], "notifications/initialized");
        assert!(notifications[0].get("id").is_none());

        assert_eq!(client.tools().len(), 1);
        assert_eq!(client.tools()[0].name, "echo");

        let timeouts = timeouts.lock().expect("timeouts");
        assert_eq!(&*timeouts, &[INIT_TIMEOUT, LIST_TIMEOUT]);
    }

    #[test]
    fn tools_list_follows_cursor_pages() {
        let transport = FakeTransport::new(
            "fake",
            vec![
                initialize_reply(),
                FakeReply::Result(json!({
                    "tools": [{"name": "first"}],
                    "nextCursor": "page-2"
                })),
                FakeReply::Result(json!({"tools": [{"name": "second"}]})),
            ],
        );
        let requests = Arc::clone(&transport.requests);
        let mut client = McpClient::new("fake", Box::new(transport));

        client.initialize().expect("initialize");

        let names: Vec<&str> = client
            .tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(names, ["first", "second"]);
        let requests = requests.lock().expect("requests");
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2]["method"], "tools/list");
        assert_eq!(requests[2]["params"]["cursor"], "page-2");
    }

    #[test]
    fn tool_call_concatenates_text_and_marks_non_text() {
        let transport = FakeTransport::new(
            "fake",
            vec![
                initialize_reply(),
                tools_reply(json!([])),
                FakeReply::Result(json!({"content": [
                    {"type": "text", "text": "alpha"},
                    {"type": "image", "data": "..."},
                    {"type": "text", "text": "beta"}
                ]})),
            ],
        );
        let requests = Arc::clone(&transport.requests);
        let timeouts = Arc::clone(&transport.timeouts);
        let mut client = McpClient::new("fake", Box::new(transport));
        client.initialize().expect("initialize");

        let output = client
            .call_tool("echo", json!({"text": "hi"}))
            .expect("call");
        assert_eq!(output.text, "alpha\n[image]\nbeta");

        let requests = requests.lock().expect("requests");
        let call = requests.last().expect("call request");
        assert_eq!(call["method"], "tools/call");
        assert_eq!(call["params"]["name"], "echo");
        assert_eq!(call["params"]["arguments"], json!({"text": "hi"}));
        let timeouts = timeouts.lock().expect("timeouts");
        assert_eq!(timeouts.last(), Some(&CALL_TIMEOUT));
    }

    #[test]
    fn tool_call_is_error_when_is_error_true() {
        let transport = FakeTransport::new(
            "fake",
            vec![
                initialize_reply(),
                tools_reply(json!([])),
                FakeReply::Result(json!({
                    "isError": true,
                    "content": [{"type": "text", "text": "boom"}]
                })),
            ],
        );
        let mut client = McpClient::new("fake", Box::new(transport));
        client.initialize().expect("initialize");

        let error = client.call_tool("fail", json!({})).expect_err("must fail");
        assert_eq!(
            error,
            McpError::Tool {
                tool: "fail".into(),
                message: "boom".into()
            }
        );
    }

    #[test]
    fn json_rpc_error_is_propagated() {
        let transport = FakeTransport::new(
            "fake",
            vec![
                initialize_reply(),
                FakeReply::Error {
                    code: -32601,
                    message: "method not found".into(),
                },
            ],
        );
        let mut client = McpClient::new("fake", Box::new(transport));

        let error = client.initialize().expect_err("must fail");
        assert_eq!(
            error,
            McpError::Rpc {
                server: "fake".into(),
                code: -32601,
                message: "method not found".into()
            }
        );
    }

    #[test]
    fn malformed_tools_list_is_rejected() {
        let transport = FakeTransport::new(
            "fake",
            vec![
                initialize_reply(),
                FakeReply::Result(json!({"notTools": []})),
            ],
        );
        let mut client = McpClient::new("fake", Box::new(transport));
        let error = client.initialize().expect_err("must fail");
        assert!(matches!(error, McpError::Protocol { .. }));
    }
}
