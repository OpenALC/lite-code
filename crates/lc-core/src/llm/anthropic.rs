//! Anthropic Messages SSE 事件映射（docs/AGENT.md §3：v1.x 端点）。
//!
//! 纯映射，不做 I/O。Anthropic 的 `content_block_delta` 只带内容块 `index`，
//! 不带 `tool_use` id，因此 [`AnthropicDecoder`] 记住 `content_block_start`
//! 建立的 index → id 关系；单条 [`parse_event`] 无状态，关联不到时把 index
//! 当作 id。未知事件忽略，坏 JSON 返回 [`LlmError::Json`]。

use std::collections::HashMap;

use lc_proto::ErrorCode;
use serde_json::Value;

use super::types::{LlmError, LlmEvent, StopReason};

fn as_u32(value: Option<&Value>) -> u32 {
    value
        .and_then(Value::as_u64)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

fn text_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn stop_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("end_turn") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("stop_sequence") => StopReason::StopSequence,
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::Other("missing stop_reason".into()),
    }
}

/// 跨事件有状态解码器：维护 `content_block_start` 建立的 tool_use 关联。
#[derive(Debug, Default)]
pub struct AnthropicDecoder {
    tool_ids: HashMap<u32, String>,
    done: bool,
}

impl AnthropicDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一条 SSE `data` JSON，返回 0..n 个归一化事件。
    pub fn push(&mut self, data: &str) -> Result<Vec<LlmEvent>, LlmError> {
        let value: Value = serde_json::from_str(data).map_err(|e| LlmError::Json(e.to_string()))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or(LlmError::MissingType)?;

        match kind {
            "message_start" => {
                let usage = value
                    .get("message")
                    .and_then(|message| message.get("usage"))
                    .unwrap_or(&Value::Null);
                Ok(vec![LlmEvent::Usage {
                    input: as_u32(usage.get("input_tokens")),
                    output: 0,
                    cached: as_u32(usage.get("cache_read_input_tokens")),
                }])
            }
            "content_block_start" => {
                let block = value.get("content_block").unwrap_or(&Value::Null);
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    return Ok(vec![]);
                }
                let index = as_u32(value.get("index"));
                let id = text_field(block, "id");
                let name = text_field(block, "name");
                self.tool_ids.insert(index, id.clone());
                Ok(vec![LlmEvent::ToolUseStart { id, name }])
            }
            "content_block_delta" => {
                let index = as_u32(value.get("index"));
                let delta = value.get("delta").unwrap_or(&Value::Null);
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => Ok(vec![LlmEvent::TextDelta {
                        index,
                        text: text_field(delta, "text"),
                    }]),
                    Some("input_json_delta") => Ok(vec![LlmEvent::ToolUseArgsDelta {
                        id: self
                            .tool_ids
                            .get(&index)
                            .cloned()
                            .unwrap_or_else(|| index.to_string()),
                        json: text_field(delta, "partial_json"),
                    }]),
                    Some("thinking_delta") => Ok(vec![LlmEvent::ThinkingDelta {
                        text: text_field(delta, "thinking"),
                    }]),
                    _ => Ok(vec![]),
                }
            }
            "message_delta" => {
                let usage = value.get("usage").unwrap_or(&Value::Null);
                let reason = value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str);
                self.done = true;
                Ok(vec![
                    LlmEvent::Usage {
                        input: 0,
                        output: as_u32(usage.get("output_tokens")),
                        cached: 0,
                    },
                    LlmEvent::Done {
                        stop_reason: stop_reason(reason),
                    },
                ])
            }
            "message_stop" => {
                // message_delta 已带 stop_reason 并发出 Done；正常流里
                // message_stop 只是终止标记，不重复发 Done。
                if self.done {
                    return Ok(vec![]);
                }
                self.done = true;
                Ok(vec![LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                }])
            }
            "error" => {
                let error = value.get("error").unwrap_or(&Value::Null);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| error.get("type").and_then(Value::as_str))
                    .unwrap_or("Anthropic 流式错误");
                self.done = true;
                Ok(vec![LlmEvent::Error {
                    code: ErrorCode::Internal,
                    message: message.to_string(),
                }])
            }
            _ => Ok(vec![]),
        }
    }
}

/// 单条无状态映射（未知事件返回空）。tool_use 参数增量若没有先经过
/// [`AnthropicDecoder::push`] 的 `content_block_start`，id 退化为块索引。
pub fn parse_event(data: &str) -> Result<Vec<LlmEvent>, LlmError> {
    AnthropicDecoder::new().push(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_start_reports_input_and_cached_tokens() {
        let events = parse_event(
            r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":25,"cache_read_input_tokens":12,"cache_creation_input_tokens":4,"output_tokens":1}}}"#,
        )
        .unwrap();
        assert_eq!(
            events,
            vec![LlmEvent::Usage {
                input: 25,
                output: 0,
                cached: 12
            }]
        );
    }

    #[test]
    fn text_and_thinking_deltas_use_block_index() {
        let text = parse_event(
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Hello"}}"#,
        )
        .unwrap();
        assert_eq!(
            text,
            vec![LlmEvent::TextDelta {
                index: 2,
                text: "Hello".into()
            }]
        );

        let thinking = parse_event(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}"#,
        )
        .unwrap();
        assert_eq!(
            thinking,
            vec![LlmEvent::ThinkingDelta {
                text: "plan".into()
            }]
        );
    }

    #[test]
    fn tool_use_start_and_args_delta_share_the_same_id() {
        let mut decoder = AnthropicDecoder::new();
        let start = decoder
            .push(
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}"#,
            )
            .unwrap();
        assert_eq!(
            start,
            vec![LlmEvent::ToolUseStart {
                id: "toolu_1".into(),
                name: "read_file".into()
            }]
        );

        let args = decoder
            .push(
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            )
            .unwrap();
        assert_eq!(
            args,
            vec![LlmEvent::ToolUseArgsDelta {
                id: "toolu_1".into(),
                json: "{\"path\":".into()
            }]
        );

        // 未注册的内容块按索引回退，不丢增量。
        let orphan = decoder
            .push(
                r#"{"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
            )
            .unwrap();
        assert_eq!(
            orphan,
            vec![LlmEvent::ToolUseArgsDelta {
                id: "3".into(),
                json: "{}".into()
            }]
        );
    }

    #[test]
    fn non_tool_blocks_and_unknown_deltas_are_ignored() {
        assert!(
            parse_event(
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            parse_event(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"x"}}"#,
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn message_delta_maps_all_stop_reasons_and_output_usage() {
        let cases = [
            ("end_turn", StopReason::EndTurn),
            ("tool_use", StopReason::ToolUse),
            ("max_tokens", StopReason::MaxTokens),
            ("stop_sequence", StopReason::StopSequence),
        ];
        for (raw, expected) in cases {
            let data = format!(
                r#"{{"type":"message_delta","delta":{{"stop_reason":"{raw}"}},"usage":{{"output_tokens":15}}}}"#
            );
            assert_eq!(
                parse_event(&data).unwrap(),
                vec![
                    LlmEvent::Usage {
                        input: 0,
                        output: 15,
                        cached: 0
                    },
                    LlmEvent::Done {
                        stop_reason: expected
                    }
                ],
                "{raw}"
            );
        }
    }

    #[test]
    fn unknown_stop_reason_falls_back_to_other() {
        let events = parse_event(
            r#"{"type":"message_delta","delta":{"stop_reason":"future_reason"},"usage":{"output_tokens":1}}"#,
        )
        .unwrap();
        assert_eq!(
            events.last(),
            Some(&LlmEvent::Done {
                stop_reason: StopReason::Other("future_reason".into())
            })
        );
    }

    #[test]
    fn message_stop_emits_done_only_when_needed() {
        let mut decoder = AnthropicDecoder::new();
        let delta = decoder
            .push(r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}"#)
            .unwrap();
        assert_eq!(delta.len(), 2);
        assert!(
            decoder
                .push(r#"{"type":"message_stop"}"#)
                .unwrap()
                .is_empty()
        );

        // 没有 message_delta 时，message_stop 单独作为终止事件。
        assert_eq!(
            parse_event(r#"{"type":"message_stop"}"#).unwrap(),
            vec![LlmEvent::Done {
                stop_reason: StopReason::EndTurn
            }]
        );
    }

    #[test]
    fn error_event_maps_to_internal() {
        assert_eq!(
            parse_event(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            )
            .unwrap(),
            vec![LlmEvent::Error {
                code: ErrorCode::Internal,
                message: "Overloaded".into()
            }]
        );
        // 缺 message 时退化为 error.type。
        assert_eq!(
            parse_event(r#"{"type":"error","error":{"type":"api_error"}}"#).unwrap(),
            vec![LlmEvent::Error {
                code: ErrorCode::Internal,
                message: "api_error".into()
            }]
        );
    }

    #[test]
    fn unknown_events_are_ignored() {
        for data in [
            r#"{"type":"ping"}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"future_event","payload":{}}"#,
        ] {
            assert!(parse_event(data).unwrap().is_empty(), "{data}");
        }
    }

    #[test]
    fn bad_json_and_missing_type_report_errors() {
        assert!(matches!(parse_event("not json"), Err(LlmError::Json(_))));
        assert!(matches!(
            parse_event(r#"{"foo":"bar"}"#),
            Err(LlmError::MissingType)
        ));
    }
}
