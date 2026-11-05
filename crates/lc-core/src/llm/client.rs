//! OpenAI Responses 流式客户端。传输层抽象为 [`HttpSseTransport`]，
//! 便于用 fake 做协议回归；真实实现走 ureq + SSE 行解析。

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::Value;

use super::openai::parse_event;
use super::request::LlmRequest;
use super::sse::SseParser;
use super::types::{LlmError, LlmEvent};

pub trait HttpSseTransport: Send + Sync {
    /// 发送请求，按 SSE `data` 逐条回调；回调返回错误即中止。
    fn stream(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &Value,
        on_data: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<(), LlmError>;
}

pub struct OpenAiResponsesClient {
    base_url: String,
    api_key: String,
    transport: Box<dyn HttpSseTransport>,
}

impl OpenAiResponsesClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        transport: Box<dyn HttpSseTransport>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            transport,
        }
    }

    pub fn stream(
        &self,
        request: &LlmRequest,
        sink: &mut dyn FnMut(LlmEvent),
    ) -> Result<(), LlmError> {
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        let headers = vec![(
            "Authorization".to_string(),
            format!("Bearer {}", self.api_key),
        )];
        let body = request.to_openai_body();
        self.transport.stream(&url, &headers, &body, &mut |data| {
            for event in parse_event(data)? {
                sink(event);
            }
            Ok(())
        })
    }
}

pub struct UreqSseTransport {
    agent: ureq::Agent,
}

impl Default for UreqSseTransport {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(600)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl HttpSseTransport for UreqSseTransport {
    fn stream(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &Value,
        on_data: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<(), LlmError> {
        let mut request = self
            .agent
            .post(url)
            .header("Content-Type", "application/json");
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let mut response = request
            .send(body.to_string())
            .map_err(|e| LlmError::Http(e.to_string()))?;
        let reader = response.body_mut().as_reader();
        let mut reader = BufReader::new(reader);
        let mut parser = SseParser::new();
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader
                .read_line(&mut line)
                .map_err(|e| LlmError::Http(e.to_string()))?;
            if read == 0 {
                break;
            }
            for event in parser.push(&line) {
                on_data(&event.data)?;
            }
        }
        if let Some(event) = parser.finish() {
            on_data(&event.data)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::request::LlmInput;
    use crate::llm::types::StopReason;
    use serde_json::json;

    struct FakeTransport {
        chunks: Vec<&'static str>,
    }

    impl HttpSseTransport for FakeTransport {
        fn stream(
            &self,
            _url: &str,
            _headers: &[(String, String)],
            _body: &Value,
            on_data: &mut dyn FnMut(&str) -> Result<(), LlmError>,
        ) -> Result<(), LlmError> {
            for chunk in &self.chunks {
                on_data(chunk)?;
            }
            Ok(())
        }
    }

    #[test]
    fn streams_text_and_done_events() {
        let transport = FakeTransport {
            chunks: vec![
                r#"{"type":"response.output_text.delta","output_index":0,"delta":"hi"}"#,
                r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":3,"output_tokens":1},"output":[]}}"#,
            ],
        };
        let client = OpenAiResponsesClient::new("https://example/v1", "k", Box::new(transport));
        let mut request = LlmRequest::new("gpt-5");
        request.input.push(LlmInput::new("user", "hello"));
        let mut events = Vec::new();
        client
            .stream(&request, &mut |event| events.push(event))
            .unwrap();

        assert_eq!(
            events[0],
            LlmEvent::TextDelta {
                index: 0,
                text: "hi".into()
            }
        );
        assert!(matches!(
            events.last(),
            Some(LlmEvent::Done {
                stop_reason: StopReason::EndTurn
            })
        ));
    }

    #[test]
    fn malformed_data_surfaces_error() {
        let transport = FakeTransport {
            chunks: vec!["not json"],
        };
        let client = OpenAiResponsesClient::new("https://example/v1", "k", Box::new(transport));
        let mut events = Vec::new();
        let err = client
            .stream(&LlmRequest::new("m"), &mut |event| events.push(event))
            .unwrap_err();
        assert!(matches!(err, LlmError::Json(_)));
    }

    #[test]
    fn body_and_auth_are_passed_to_transport() {
        use std::sync::Mutex;
        type Seen = (String, Vec<(String, String)>, Value);
        struct Capture {
            seen: Mutex<Option<Seen>>,
        }
        impl HttpSseTransport for Capture {
            fn stream(
                &self,
                url: &str,
                headers: &[(String, String)],
                body: &Value,
                _on_data: &mut dyn FnMut(&str) -> Result<(), LlmError>,
            ) -> Result<(), LlmError> {
                *self.seen.lock().unwrap() =
                    Some((url.to_string(), headers.to_vec(), body.clone()));
                Ok(())
            }
        }
        let capture = std::sync::Arc::new(Capture {
            seen: Mutex::new(None),
        });
        struct ArcCapture(std::sync::Arc<Capture>);
        impl HttpSseTransport for ArcCapture {
            fn stream(
                &self,
                url: &str,
                headers: &[(String, String)],
                body: &Value,
                on_data: &mut dyn FnMut(&str) -> Result<(), LlmError>,
            ) -> Result<(), LlmError> {
                self.0.stream(url, headers, body, on_data)
            }
        }
        let client = OpenAiResponsesClient::new(
            "https://example/v1/",
            "secret",
            Box::new(ArcCapture(capture.clone())),
        );
        client
            .stream(&LlmRequest::new("gpt-5"), &mut |_| {})
            .unwrap();
        let (url, headers, body) = capture.seen.lock().unwrap().clone().unwrap();
        assert_eq!(url, "https://example/v1/responses");
        assert_eq!(headers[0].0, "Authorization");
        assert_eq!(headers[0].1, "Bearer secret");
        assert_eq!(body["stream"], json!(true));
    }
}
