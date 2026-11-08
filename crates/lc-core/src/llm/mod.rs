//! LLM 网关基础层（docs/AGENT.md §3）：SSE 解析、OpenAI Responses 与
//! Anthropic Messages 事件映射、ureq 流式传输。

pub mod anthropic;
pub mod client;
pub mod openai;
pub mod provider;
pub mod request;
pub mod runtime;
pub mod sse;
pub mod types;

pub use anthropic::AnthropicDecoder;
pub use client::{HttpSseTransport, OpenAiResponsesClient, UreqSseTransport};
pub use provider::{ModelConfig, ProviderConfig, ProviderError, ProviderKind, ProvidersFile};
pub use request::{LlmInput, LlmRequest};
pub use runtime::{
    AgentRuntime, RuntimeError, load_default, load_for_model, load_from, providers_path,
};
pub use sse::{SseEvent, SseParser};
pub use types::{LlmError, LlmEvent, StopReason};
