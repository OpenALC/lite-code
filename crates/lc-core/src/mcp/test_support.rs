//! MCP 单测共享的 fake transport：按脚本顺序返回响应，记录请求/通知/超时。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::McpError;
use super::client::McpTransport;

/// 一次 fake 响应。
pub(crate) enum FakeReply {
    Result(Value),
    Error { code: i64, message: String },
}

/// 按顺序消费 `replies` 的 transport；消息与超时全部记录。
pub(crate) struct FakeTransport {
    server: String,
    pub(crate) requests: Arc<Mutex<Vec<Value>>>,
    pub(crate) notifications: Arc<Mutex<Vec<Value>>>,
    pub(crate) timeouts: Arc<Mutex<Vec<Duration>>>,
    replies: VecDeque<FakeReply>,
}

impl FakeTransport {
    pub(crate) fn new(server: &str, replies: Vec<FakeReply>) -> Self {
        Self {
            server: server.to_string(),
            requests: Arc::new(Mutex::new(Vec::new())),
            notifications: Arc::new(Mutex::new(Vec::new())),
            timeouts: Arc::new(Mutex::new(Vec::new())),
            replies: replies.into(),
        }
    }
}

impl McpTransport for FakeTransport {
    fn request(&mut self, message: &Value, timeout: Duration) -> Result<Value, McpError> {
        self.requests
            .lock()
            .expect("fake requests lock")
            .push(message.clone());
        self.timeouts
            .lock()
            .expect("fake timeouts lock")
            .push(timeout);
        match self.replies.pop_front() {
            Some(FakeReply::Result(value)) => Ok(value),
            Some(FakeReply::Error { code, message }) => Err(McpError::Rpc {
                server: self.server.clone(),
                code,
                message,
            }),
            None => Err(McpError::Protocol {
                server: self.server.clone(),
                message: "fake transport 没有更多响应".into(),
            }),
        }
    }

    fn notify(&mut self, message: &Value) -> Result<(), McpError> {
        self.notifications
            .lock()
            .expect("fake notifications lock")
            .push(message.clone());
        Ok(())
    }
}
