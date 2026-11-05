//! Agent harness（docs/AGENT.md）：工具注册表与权限策略先行，
//! 会话循环 / LLM 网关 / 压缩 / MCP 由后续 lane 接入。

pub mod approval;
pub mod collector;
pub mod exec;
pub mod r#loop;
pub mod policy;
pub mod session;
pub mod shell;
pub mod tools;

pub use approval::{ApprovalCache, ApprovalDecision, ApprovalId, ApprovalQueue, ApprovalRequest};
pub use collector::{CollectedToolCall, CollectedTurn, CollectorError, TurnCollector};
pub use exec::{LocalToolExecutor, ToolCall, ToolError, ToolExecutor, ToolOutput};
pub use r#loop::{AgentLoop, LlmStream, LoopError, LoopOutcome};
pub use policy::{Policy, PolicyDecision, PolicyError, PolicyMode, PolicyRequest, PolicySource};
pub use session::{
    ApprovalOutcome, AuthorizeOutcome, DEFAULT_MAX_TURNS, ModeChange, Session, SessionError,
};
pub use shell::{DEFAULT_TIMEOUT_MS, MAX_STREAM_BYTES, MAX_TIMEOUT_MS};
pub use tools::{
    MAX_TOOLS, Risk, ToolRegistry, ToolRegistryError, ToolSpec, builtin_tool_definitions,
};
