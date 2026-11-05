//! MCP Host（docs/AGENT.md §8）：stdio server 连接、工具发现与执行。
//!
//! 本轮只实现 **stdio + command 型 server**；HTTP/SSE 配置显式忽略并记录
//! warning。MCP 工具是外部工具，风险等级恒为 [`crate::agent::Risk::Network`]，
//! 需要用户审批（与内置 `Network` 工具一致）。
//!
//! 分层：
//! - [`config`]：读取 `~/.lite-code/mcp.json`；项目级 `<root>/.mcp.json` 默认
//!   忽略，仅在 `LC_MCP_ALLOW_PROJECT=1` 时读取且不覆盖用户级同名项；
//! - [`client`]：JSON-RPC 2.0 客户端，握手、分页拉取工具、`tools/call`；
//!   stdio 子进程只继承平台 allowlist 环境变量，不继承 provider 密钥；
//! - [`host`]：多 server 生命周期（最多 [`config::MAX_SERVERS`] 个）、工具名
//!   清洗/去重、Responses API 定义、[`McpExecutor`] 执行路由。
//!
//! MCP 不可用不应影响主流程：连接失败只记录 warning，其余 server 继续。

pub mod client;
pub mod config;
pub mod host;

#[cfg(test)]
pub(crate) mod test_support;

pub use client::{
    CALL_TIMEOUT, INIT_TIMEOUT, LIST_TIMEOUT, McpClient, McpTransport, StdioTransport,
};
pub use config::{McpConfig, McpServerConfig};
pub use host::{McpExecutor, McpHost, McpToolSpec};

/// MCP 配置 / 协议 / 传输错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    #[error("MCP 配置错误：{0}")]
    Config(String),
    #[error("MCP server `{server}` 启动失败：{message}")]
    Spawn { server: String, message: String },
    #[error("MCP server `{server}` 协议错误：{message}")]
    Protocol { server: String, message: String },
    #[error("MCP server `{server}` 请求超时（{timeout_ms} ms），已终止子进程")]
    Timeout { server: String, timeout_ms: u64 },
    #[error("MCP server `{server}` 返回 JSON-RPC 错误（code {code}）：{message}")]
    Rpc {
        server: String,
        code: i64,
        message: String,
    },
    #[error("MCP 工具 `{tool}` 返回错误：{message}")]
    Tool { tool: String, message: String },
    #[error("MCP server `{server}` 子进程已退出：{message}")]
    Closed { server: String, message: String },
    #[error("MCP server `{server}` I/O 错误：{message}")]
    Io { server: String, message: String },
}
