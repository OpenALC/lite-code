//! LLM 网关基础层（docs/AGENT.md §3）：SSE 解析与 OpenAI Responses 事件映射。
//! 网络传输、供应商配置与 Anthropic Messages 由后续 lane 接入。

pub mod client;
pub mod openai;
pub mod provider;
pub mod request;
pub mod sse;
pub mod types;

pub use client::{HttpSseTransport, OpenAiResponsesClient, UreqSseTransport};
pub use provider::{ModelConfig, ProviderConfig, ProviderError, ProviderKind, ProvidersFile};
pub use request::{LlmInput, LlmRequest};
pub use sse::{SseEvent, SseParser};
pub use types::{LlmError, LlmEvent, StopReason};
