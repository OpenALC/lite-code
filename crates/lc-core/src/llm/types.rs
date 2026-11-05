//! LLM 流式事件（docs/AGENT.md §3）。provider 适配层负责映射到这些事件。

use lc_proto::ErrorCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmEvent {
    TextDelta {
        index: u32,
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolUseStart {
        id: String,
        name: String,
    },
    ToolUseArgsDelta {
        id: String,
        json: String,
    },
    Usage {
        input: u32,
        output: u32,
        cached: u32,
    },
    Done {
        stop_reason: StopReason,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    #[error("SSE/JSON 解析失败：{0}")]
    Json(String),
    #[error("事件缺少 type 字段")]
    MissingType,
    #[error("事件字段无效：{0}")]
    Invalid(String),
}
