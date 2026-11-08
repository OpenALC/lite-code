//! LSP lane（docs/EXTENSIONS.md §6）：stdio JSON-RPC 语言服务器 v0。
//!
//! 范围：`initialize` / `initialized` / `textDocument/didOpen` / `didChange` /
//! `didClose` / `publishDiagnostics`。补全、悬停、跳转等请求留给后续版本。
//!
//! 分层：
//! - [`config`]：插件清单 `languageServers` 对应的 [`LspServerConfig`]（camelCase）；
//! - [`diagnostic`]：1-based [`Diagnostic`]、[`DiagnosticSink`] 与按 path 聚合的
//!   [`DiagnosticStore`]；
//! - [`client`]：Content-Length framing、JSON-RPC、子进程与读线程；
//! - [`manager`]：按 server / language id 路由，单个 server 失败不影响其它。
//!
//! 安全：子进程复用 [`crate::process::scrub_secret_env`]，不继承 provider 密钥。
//! Windows 用 Job Object（`KILL_ON_JOB_CLOSE`）尽力回收进程树；unix 用独立
//! 进程组。超时或协议错误会终止子进程，避免留下孤儿。

pub mod client;
pub mod config;
pub mod diagnostic;
pub mod manager;
mod uri;

pub use client::{INITIALIZE_TIMEOUT, LspClient, SHUTDOWN_TIMEOUT};
pub use config::{LspServerConfig, LspServerInfo};
pub use diagnostic::{Diagnostic, DiagnosticSink, DiagnosticStore};
pub use manager::{LspManager, LspStartFailure, LspStartReport};
pub use uri::{path_to_uri, uri_to_path};

use lc_proto::{ErrorBody, ErrorCode};

/// LSP 客户端 / 协议 / 进程错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LspError {
    #[error("LSP server `{server}` 启动失败：{message}")]
    Spawn { server: String, message: String },
    #[error("LSP server `{server}` 协议错误：{message}")]
    Protocol { server: String, message: String },
    #[error("LSP server `{server}` 请求超时（{timeout_ms} ms），已终止子进程")]
    Timeout { server: String, timeout_ms: u64 },
    #[error("LSP server `{server}` 返回 JSON-RPC 错误（code {code}）：{message}")]
    Rpc {
        server: String,
        code: i64,
        message: String,
    },
    #[error("LSP server `{server}` 子进程已退出：{message}")]
    Closed { server: String, message: String },
    #[error("LSP server `{server}` I/O 错误：{message}")]
    Io { server: String, message: String },
    #[error("LSP server `{server}` 尚未完成 initialize")]
    NotInitialized { server: String },
    #[error("文档未打开：{path}")]
    DocumentNotOpen { path: String },
}

impl LspError {
    /// 对应的契约错误码；lane 私有细节保留在 message 中。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Spawn { .. } | Self::Closed { .. } | Self::Io { .. } => ErrorCode::Io,
            Self::Timeout { .. } => ErrorCode::Timeout,
            Self::Protocol { .. } | Self::Rpc { .. } | Self::NotInitialized { .. } => {
                ErrorCode::Internal
            }
            Self::DocumentNotOpen { .. } => ErrorCode::NotFound,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        ErrorBody::new(self.code(), self.to_string())
    }
}
