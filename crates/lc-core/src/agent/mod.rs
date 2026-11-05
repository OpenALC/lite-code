//! Agent harness（docs/AGENT.md）：工具注册表与权限策略先行，
//! 会话循环 / LLM 网关 / 压缩 / MCP 由后续 lane 接入。

pub mod policy;
pub mod tools;

pub use policy::{Policy, PolicyDecision, PolicyError, PolicyMode, PolicyRequest, PolicySource};
pub use tools::{MAX_TOOLS, Risk, ToolRegistry, ToolRegistryError, ToolSpec};
