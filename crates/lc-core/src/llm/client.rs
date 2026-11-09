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

impl UreqSseTransport {
    /// 自定义连接/DNS/TLS 握手超时。流式请求的整体超时仍为 600s；鸿蒙宿主
    /// 用它避免 provider 离线时长时间停在“思考中”。
    pub fn with_connect_timeout(connect: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(connect))
            .timeout_global(Some(Duration::from_secs(600)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
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

    // ---- loopback HTTP/1.1 SSE（不访问外网）----

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    struct Loopback {
        addr: SocketAddr,
        handle: JoinHandle<()>,
    }

    impl Loopback {
        /// 起一个只接受一次连接的本地服务；`chunked` 选择响应传输方式。
        fn start(payload: &str, chunked: bool) -> Self {
            let payload = payload.to_string();
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let addr = listener.local_addr().expect("loopback addr");
            let handle = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept loopback");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("read timeout");
                read_request(&mut stream);

                let response = if chunked {
                    chunked_response(payload.as_bytes(), 17)
                } else {
                    content_length_response(payload.as_bytes())
                };
                // 分两次写，覆盖任意 TCP 分片下的 SSE 行缓冲。
                let split = response.len() / 2;
                stream.write_all(&response[..split]).expect("write part 1");
                stream.flush().expect("flush part 1");
                thread::sleep(Duration::from_millis(10));
                stream.write_all(&response[split..]).expect("write part 2");
                stream.flush().expect("flush part 2");
            });
            Self { addr, handle }
        }

        fn url(&self) -> String {
            format!("http://{}/v1/responses", self.addr)
        }

        fn join(self) {
            self.handle.join().expect("loopback thread");
        }
    }

    fn read_request(stream: &mut TcpStream) {
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("read request") == 0 {
                break;
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        if content_length > 0 {
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).expect("read request body");
        }
    }

    fn content_length_response(payload: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        )
        .into_bytes();
        response.extend_from_slice(payload);
        response
    }

    fn chunked_response(payload: &[u8], chunk_size: usize) -> Vec<u8> {
        let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        for chunk in payload.chunks(chunk_size) {
            response.extend_from_slice(format!("{:X}\r\n", chunk.len()).as_bytes());
            response.extend_from_slice(chunk);
            response.extend_from_slice(b"\r\n");
        }
        response.extend_from_slice(b"0\r\n\r\n");
        response
    }

    fn loopback_agent() -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(10)))
            .build();
        ureq::Agent::new_with_config(config)
    }

    fn openai_sse_payload() -> String {
        concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hello\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1},\"output\":[]}}\n\n",
        )
        .to_string()
    }

    #[test]
    fn ureq_transport_streams_content_length_sse_from_loopback() {
        let server = Loopback::start(&openai_sse_payload(), false);
        let client = OpenAiResponsesClient::new(
            format!("http://{}/v1", server.addr),
            "test-key",
            Box::new(UreqSseTransport {
                agent: loopback_agent(),
            }),
        );
        let mut request = LlmRequest::new("gpt-5");
        request.input.push(LlmInput::new("user", "hi"));

        let mut events = Vec::new();
        client
            .stream(&request, &mut |event| events.push(event))
            .expect("stream loopback");
        server.join();

        assert_eq!(
            events,
            vec![
                LlmEvent::TextDelta {
                    index: 0,
                    text: "hello".into()
                },
                LlmEvent::Usage {
                    input: 3,
                    output: 1,
                    cached: 0
                },
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn
                }
            ]
        );
    }

    #[test]
    fn ureq_transport_reads_chunked_sse() {
        let server = Loopback::start("data: one\n\ndata: two\n\n", true);
        let transport = UreqSseTransport {
            agent: loopback_agent(),
        };
        let mut chunks = Vec::new();
        transport
            .stream(&server.url(), &[], &json!({"stream": true}), &mut |data| {
                chunks.push(data.to_string());
                Ok(())
            })
            .expect("stream chunked loopback");
        server.join();

        assert_eq!(chunks, vec!["one".to_string(), "two".to_string()]);
    }
}
