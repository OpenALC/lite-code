//! 归一化请求 → OpenAI Responses body（docs/AGENT.md §3）。
//! 支持普通消息、function_call 与 function_call_output 三种输入项。

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmInput {
    Message {
        role: String,
        content: String,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
}

impl LlmInput {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Message {
            role: role.into(),
            content: content.into(),
        }
    }

    pub fn function_call(
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self::FunctionCall {
            call_id: call_id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    pub fn function_call_output(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self::FunctionCallOutput {
            call_id: call_id.into(),
            output: output.into(),
        }
    }

    fn to_openai(&self) -> Value {
        match self {
            Self::Message { role, content } => json!({"role": role, "content": content}),
            Self::FunctionCall {
                call_id,
                name,
                arguments,
            } => json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
            }),
            Self::FunctionCallOutput { call_id, output } => json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmRequest {
    pub model: String,
    pub instructions: Option<String>,
    pub input: Vec<LlmInput>,
    pub tools: Vec<Value>,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

impl LlmRequest {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            instructions: None,
            input: Vec::new(),
            tools: Vec::new(),
            max_output_tokens: None,
            temperature: None,
        }
    }

    pub fn to_openai_body(&self) -> Value {
        let input: Vec<Value> = self.input.iter().map(LlmInput::to_openai).collect();
        let mut body = json!({
            "model": self.model,
            "input": input,
            "stream": true,
            "tools": self.tools,
        });
        if let Some(instructions) = &self.instructions {
            body["instructions"] = json!(instructions);
        }
        if let Some(max) = self.max_output_tokens {
            body["max_output_tokens"] = json!(max);
        }
        if let Some(temperature) = self.temperature {
            body["temperature"] = json!(temperature);
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_responses_body_with_stream_and_tools() {
        let mut request = LlmRequest::new("gpt-5");
        request.instructions = Some("be brief".into());
        request.input = vec![LlmInput::new("user", "hi")];
        request.tools = vec![json!({"type":"function","name":"read_file"})];
        request.max_output_tokens = Some(128);
        request.temperature = Some(0.2);

        let body = request.to_openai_body();
        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "be brief");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"], "hi");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["max_output_tokens"], 128);
    }

    #[test]
    fn function_call_and_output_are_distinct_items() {
        let mut request = LlmRequest::new("m");
        request.input = vec![
            LlmInput::function_call("call_1", "read_file", r#"{"path":"a"}"#),
            LlmInput::function_call_output("call_1", "file text"),
        ];
        let body = request.to_openai_body();
        assert_eq!(body["input"][0]["type"], "function_call");
        assert_eq!(body["input"][0]["call_id"], "call_1");
        assert_eq!(body["input"][1]["type"], "function_call_output");
        assert_eq!(body["input"][1]["output"], "file text");
    }

    #[test]
    fn omits_optional_fields_when_none() {
        let request = LlmRequest::new("m");
        let body = request.to_openai_body();
        assert!(body.get("instructions").is_none());
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("temperature").is_none());
    }
}
