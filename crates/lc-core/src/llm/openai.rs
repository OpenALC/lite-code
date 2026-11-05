//! OpenAI Responses API 事件映射（docs/AGENT.md §3：v1 只做该端点）。
//!
//! 输入是一条 SSE 的 `data` JSON；未知事件忽略，已知事件严格解析字段。

use lc_proto::ErrorCode;
use serde_json::Value;

use super::types::{LlmError, LlmEvent, StopReason};

fn as_u32(value: Option<&Value>) -> u32 {
    value
        .and_then(Value::as_u64)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

fn text_delta(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 解析一条 `data:` JSON，返回 0..n 个归一化事件。
pub fn parse_event(data: &str) -> Result<Vec<LlmEvent>, LlmError> {
    if data.trim() == "[DONE]" {
        return Ok(vec![]);
    }
    let value: Value = serde_json::from_str(data).map_err(|e| LlmError::Json(e.to_string()))?;
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or(LlmError::MissingType)?;

    let events = match kind {
        "response.output_text.delta" => vec![LlmEvent::TextDelta {
            index: as_u32(value.get("output_index")),
            text: text_delta(value.get("delta")),
        }],
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            vec![LlmEvent::ThinkingDelta {
                text: text_delta(value.get("delta")),
            }]
        }
        "response.output_item.added" => {
            let item = value.get("item").unwrap_or(&Value::Null);
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                vec![]
            } else {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("call_id").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                vec![LlmEvent::ToolUseStart { id, name }]
            }
        }
        "response.function_call_arguments.delta" => vec![LlmEvent::ToolUseArgsDelta {
            id: value
                .get("item_id")
                .and_then(Value::as_str)
                .or_else(|| value.get("call_id").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string(),
            json: text_delta(value.get("delta")),
        }],
        "response.completed" => {
            let response = value.get("response").unwrap_or(&Value::Null);
            let usage = response.get("usage").unwrap_or(&Value::Null);
            let mut events = vec![LlmEvent::Usage {
                input: as_u32(usage.get("input_tokens")),
                output: as_u32(usage.get("output_tokens")),
                cached: as_u32(
                    usage
                        .get("input_tokens_details")
                        .and_then(|details| details.get("cached_tokens")),
                ),
            }];

            let status = response.get("status").and_then(Value::as_str);
            let stop_reason = if status == Some("incomplete") {
                match response
                    .get("incomplete_details")
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str)
                {
                    Some("max_output_tokens") => StopReason::MaxTokens,
                    Some(reason) => StopReason::Other(reason.to_string()),
                    None => StopReason::Other("incomplete".into()),
                }
            } else if response
                .get("output")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item.get("type").and_then(Value::as_str) == Some("function_call")
                    })
                })
            {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            };
            events.push(LlmEvent::Done { stop_reason });
            events
        }
        "response.failed" => {
            let response = value.get("response").unwrap_or(&Value::Null);
            let message = response
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("response failed")
                .to_string();
            vec![LlmEvent::Error {
                code: ErrorCode::Internal,
                message,
            }]
        }
        "error" => vec![LlmEvent::Error {
            code: ErrorCode::Internal,
            message: text_delta(value.get("message")),
        }],
        _ => vec![],
    };

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_thinking_deltas() {
        let text =
            parse_event(r#"{"type":"response.output_text.delta","output_index":2,"delta":"hi"}"#)
                .unwrap();
        assert_eq!(
            text,
            vec![LlmEvent::TextDelta {
                index: 2,
                text: "hi".into()
            }]
        );

        let thinking =
            parse_event(r#"{"type":"response.reasoning_summary_text.delta","delta":"plan"}"#)
                .unwrap();
        assert_eq!(
            thinking,
            vec![LlmEvent::ThinkingDelta {
                text: "plan".into()
            }]
        );
    }

    #[test]
    fn tool_call_start_and_args_delta() {
        let start = parse_event(
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"item_1","call_id":"call_1","name":"read_file"}}"#,
        )
        .unwrap();
        assert_eq!(
            start,
            vec![LlmEvent::ToolUseStart {
                id: "item_1".into(),
                name: "read_file".into()
            }]
        );

        let args = parse_event(
            r#"{"type":"response.function_call_arguments.delta","item_id":"item_1","delta":"{\"path\":"}"#,
        )
        .unwrap();
        assert_eq!(
            args,
            vec![LlmEvent::ToolUseArgsDelta {
                id: "item_1".into(),
                json: "{\"path\":".into()
            }]
        );
    }

    #[test]
    fn completed_reports_usage_and_stop_reason() {
        let events = parse_event(
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":10,"output_tokens":4,"input_tokens_details":{"cached_tokens":3}},"output":[{"type":"function_call"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            events,
            vec![
                LlmEvent::Usage {
                    input: 10,
                    output: 4,
                    cached: 3
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse
                }
            ]
        );
    }

    #[test]
    fn incomplete_max_tokens_and_failure() {
        let events = parse_event(
            r#"{"type":"response.completed","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":1,"output_tokens":2},"output":[]}}"#,
        )
        .unwrap();
        assert_eq!(
            events.last(),
            Some(&LlmEvent::Done {
                stop_reason: StopReason::MaxTokens
            })
        );

        let failure =
            parse_event(r#"{"type":"response.failed","response":{"error":{"message":"boom"}}}"#)
                .unwrap();
        assert_eq!(
            failure,
            vec![LlmEvent::Error {
                code: ErrorCode::Internal,
                message: "boom".into()
            }]
        );
    }

    #[test]
    fn unknown_events_are_ignored_and_bad_json_reports() {
        assert!(
            parse_event(r#"{"type":"response.created"}"#)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(parse_event("not json"), Err(LlmError::Json(_))));
        assert!(matches!(
            parse_event(r#"{"foo":"bar"}"#),
            Err(LlmError::MissingType)
        ));
    }
}
