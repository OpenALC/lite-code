//! Server-Sent Events 增量解析（docs/AGENT.md §3 的流式前提）。
//!
//! 只做协议层：任意 chunk 边界、CRLF、多行 data、注释与空行分派。
//! provider 事件到 [`super::LlmEvent`] 的映射在 provider 模块。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入任意 chunk，返回本次已完整分派的事件。
    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let mut line = self.buffer[..pos].to_string();
            self.buffer.drain(..=pos);
            if line.ends_with('\r') {
                line.pop();
            }
            self.line(&line, &mut out);
        }
        out
    }

    /// 连接结束时提交没有空行结尾的最后一个事件（宽松处理）。
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            let mut out = Vec::new();
            let line = line.trim_end_matches('\r');
            self.line(line, &mut out);
            if let Some(event) = out.pop() {
                return Some(event);
            }
        }
        self.dispatch()
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if let Some(event) = self.dispatch() {
                out.push(event);
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        let event = SseEvent {
            event: self.event.take(),
            data: self.data.join("\n"),
        };
        self.data.clear();
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_chunks_and_multiline_data() {
        let mut parser = SseParser::new();
        assert!(
            parser
                .push("event: response.output_text.delta\nda")
                .is_empty()
        );
        let events = parser.push("ta: {\"a\":1}\ndata: second\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event.as_deref(),
            Some("response.output_text.delta")
        );
        assert_eq!(events[0].data, "{\"a\":1}\nsecond");
    }

    #[test]
    fn handles_crlf_comments_and_multiple_events() {
        let mut parser = SseParser::new();
        let events = parser.push(": ping\r\ndata: one\r\n\r\ndata: two\r\n\r\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "one");
        assert_eq!(events[1].data, "two");
        assert_eq!(events[0].event, None);
    }

    #[test]
    fn finish_flushes_trailing_event() {
        let mut parser = SseParser::new();
        assert!(parser.push("event: done\ndata: {}").is_empty());
        let event = parser.finish().expect("trailing event");
        assert_eq!(event.event.as_deref(), Some("done"));
        assert_eq!(event.data, "{}");
        assert_eq!(parser.finish(), None);
    }
}
