//! 一轮 LLM 流的事件聚合：文本/思考、工具调用参数、usage、停止原因。
//! 纯函数式状态机，不依赖网络与 UI；会话循环只消费 [`CollectedTurn`]。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::llm::{LlmEvent, StopReason};

use super::exec::ToolCall;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedTurn {
    pub assistant_text: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_tokens: u32,
    pub stop_reason: Option<StopReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CollectorError {
    #[error("工具调用 {id} 的参数不是合法 JSON：{message}")]
    InvalidToolArgs { id: String, message: String },
    #[error("收到工具调用参数但未收到 start：{0}")]
    ArgsWithoutStart(String),
}

#[derive(Debug, Default, Clone)]
struct PendingCall {
    name: String,
    args: String,
}

#[derive(Debug, Default)]
pub struct TurnCollector {
    assistant_text: String,
    thinking: String,
    calls: BTreeMap<String, PendingCall>,
    input_tokens: u32,
    output_tokens: u32,
    cached_tokens: u32,
    stop_reason: Option<StopReason>,
}

impl TurnCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: &LlmEvent) -> Result<(), CollectorError> {
        match event {
            LlmEvent::TextDelta { text, .. } => self.assistant_text.push_str(text),
            LlmEvent::ThinkingDelta { text } => self.thinking.push_str(text),
            LlmEvent::ToolUseStart { id, name } => {
                self.calls.insert(
                    id.clone(),
                    PendingCall {
                        name: name.clone(),
                        args: String::new(),
                    },
                );
            }
            LlmEvent::ToolUseArgsDelta { id, json } => {
                let call = self
                    .calls
                    .get_mut(id)
                    .ok_or_else(|| CollectorError::ArgsWithoutStart(id.clone()))?;
                call.args.push_str(json);
            }
            LlmEvent::Usage {
                input,
                output,
                cached,
            } => {
                self.input_tokens = *input;
                self.output_tokens = *output;
                self.cached_tokens = *cached;
            }
            LlmEvent::Done { stop_reason } => self.stop_reason = Some(stop_reason.clone()),
            LlmEvent::Error { .. } => {}
        }
        Ok(())
    }

    pub fn finish(self) -> Result<CollectedTurn, CollectorError> {
        let mut tool_calls = Vec::with_capacity(self.calls.len());
        for (id, call) in self.calls {
            let args = if call.args.trim().is_empty() {
                Value::Object(Default::default())
            } else {
                serde_json::from_str(&call.args).map_err(|e| CollectorError::InvalidToolArgs {
                    id: id.clone(),
                    message: e.to_string(),
                })?
            };
            tool_calls.push(ToolCall::new(call.name, args));
        }
        Ok(CollectedTurn {
            assistant_text: self.assistant_text,
            thinking: self.thinking,
            tool_calls,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cached_tokens: self.cached_tokens,
            stop_reason: self.stop_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_text_thinking_usage_and_stop() {
        let mut collector = TurnCollector::new();
        collector
            .apply(&LlmEvent::ThinkingDelta {
                text: "plan".into(),
            })
            .unwrap();
        collector
            .apply(&LlmEvent::TextDelta {
                index: 0,
                text: "hello ".into(),
            })
            .unwrap();
        collector
            .apply(&LlmEvent::TextDelta {
                index: 0,
                text: "world".into(),
            })
            .unwrap();
        collector
            .apply(&LlmEvent::Usage {
                input: 10,
                output: 2,
                cached: 4,
            })
            .unwrap();
        collector
            .apply(&LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
            })
            .unwrap();

        let turn = collector.finish().unwrap();
        assert_eq!(turn.assistant_text, "hello world");
        assert_eq!(turn.thinking, "plan");
        assert_eq!(turn.input_tokens, 10);
        assert_eq!(turn.output_tokens, 2);
        assert_eq!(turn.cached_tokens, 4);
        assert_eq!(turn.stop_reason, Some(StopReason::EndTurn));
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn assembles_tool_call_arguments() {
        let mut collector = TurnCollector::new();
        collector
            .apply(&LlmEvent::ToolUseStart {
                id: "item_1".into(),
                name: "write_file".into(),
            })
            .unwrap();
        for chunk in [r#"{"path":"#, r#""a.txt","content":"#, r#""hi"}"#] {
            collector
                .apply(&LlmEvent::ToolUseArgsDelta {
                    id: "item_1".into(),
                    json: chunk.into(),
                })
                .unwrap();
        }
        collector
            .apply(&LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
            })
            .unwrap();

        let turn = collector.finish().unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "write_file");
        assert_eq!(
            turn.tool_calls[0].args,
            json!({"path":"a.txt","content":"hi"})
        );
    }

    #[test]
    fn empty_args_default_to_object_and_bad_json_errors() {
        let mut collector = TurnCollector::new();
        collector
            .apply(&LlmEvent::ToolUseStart {
                id: "a".into(),
                name: "read_file".into(),
            })
            .unwrap();
        let turn = collector.finish().unwrap();
        assert_eq!(turn.tool_calls[0].args, json!({}));

        let mut collector = TurnCollector::new();
        collector
            .apply(&LlmEvent::ToolUseStart {
                id: "a".into(),
                name: "x".into(),
            })
            .unwrap();
        collector
            .apply(&LlmEvent::ToolUseArgsDelta {
                id: "a".into(),
                json: "{".into(),
            })
            .unwrap();
        assert!(matches!(
            collector.finish(),
            Err(CollectorError::InvalidToolArgs { .. })
        ));
    }

    #[test]
    fn args_without_start_is_an_error() {
        let mut collector = TurnCollector::new();
        assert_eq!(
            collector.apply(&LlmEvent::ToolUseArgsDelta {
                id: "ghost".into(),
                json: "{}".into()
            }),
            Err(CollectorError::ArgsWithoutStart("ghost".into()))
        );
    }
}
