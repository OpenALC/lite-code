//! Agent harness（docs/AGENT.md）：工具注册表、会话循环 / LLM 网关 / 压缩。
//! MCP 是独立的 `crate::mcp` lane，通过 [`ToolRegistry::with_mcp_tools`] 接入。

pub mod approval;
pub mod collector;
pub mod compact;
pub mod environment;
pub mod events;
pub mod exec;
pub mod r#loop;
pub mod memory;
pub mod policy;
pub mod session;
pub mod shell;
pub mod tools;

pub use approval::{ApprovalCache, ApprovalDecision, ApprovalId, ApprovalQueue, ApprovalRequest};
pub use collector::{CollectedToolCall, CollectedTurn, CollectorError, TurnCollector};
pub use compact::{CompactionRecord, KEEP_RECENT_MESSAGES, compact, estimate_tokens};
pub use environment::{EnvironmentContract, session_instructions};
pub use events::{
    approval_prompt, llm_event, loop_outcome, mode_from_contract, mode_to_contract, session_info,
};
pub use exec::{LocalToolExecutor, ToolCall, ToolError, ToolExecutor, ToolOutput};
pub use r#loop::{
    AgentLoop, CompactionObserver, LlmStream, LoopError, LoopOutcome, ToolResultObserver,
};
pub use memory::{MemoryBundle, MemoryError, append as append_memory};
pub use policy::{Policy, PolicyDecision, PolicyError, PolicyMode, PolicyRequest, PolicySource};
pub use session::{
    ApprovalOutcome, AuthorizeOutcome, DEFAULT_MAX_TURNS, ModeChange, Session, SessionError,
};
pub use shell::{DEFAULT_TIMEOUT_MS, MAX_STREAM_BYTES, MAX_TIMEOUT_MS};
pub use tools::MAX_TOOLS;
pub use tools::Risk;
pub use tools::ToolRegistry;
pub use tools::ToolRegistryError;
pub use tools::ToolSpec;
pub use tools::builtin_tool_definitions;
