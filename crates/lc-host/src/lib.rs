//! 共享宿主编排（docs/plans/lc-host-extraction.md）。
//!
//! M1：事件出口原语——`EventSink`、单调 seq 的 `EventEnvelope` 封装。
//! 桌面（Tauri 事件插件）与鸿蒙（NAPI → ArkTS → WebView）共用同一信封
//! 形状与 seq 语义；后续 M2/M3 在此之上叠加 SessionRegistry/TurnRunner。

pub mod envelope {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use lc_proto::{CoreEvent, EventEnvelope};

    /// 事件出口：接收 `EventEnvelope` 的 JSON 字符串。宿主各自实现
    /// （Tauri 事件插件 / NAPI threadsafe function → runJavaScript）。
    pub type EventSink = Arc<dyn Fn(String) + Send + Sync>;

    /// 单调 seq（1 起）。进程内唯一，跨会话连续；web 侧按 seq 检测丢序。
    /// 内部 Arc：`Seq` 可 clone，跨 worker 线程共享同一计数器。
    #[derive(Default, Clone)]
    pub struct Seq(Arc<AtomicU64>);

    impl Seq {
        pub fn next(&self) -> u64 {
            self.0.fetch_add(1, Ordering::SeqCst) + 1
        }
    }

    /// 封装一条契约事件为信封 JSON 并推给 sink；sink 缺失或序列化失败为 no-op。
    pub fn emit_envelope(sink: &Option<EventSink>, seq: &Seq, event: CoreEvent) {
        let Some(sink) = sink else { return };
        let envelope = EventEnvelope {
            seq: seq.next(),
            event,
        };
        if let Ok(json) = serde_json::to_string(&envelope) {
            sink(json);
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::{Arc, Mutex};

        use lc_proto::{CoreEvent, ErrorBody, ErrorCode};

        use super::{EventSink, Seq, emit_envelope};

        fn collecting() -> (EventSink, Arc<Mutex<Vec<String>>>) {
            let out = Arc::new(Mutex::new(Vec::new()));
            let clone = Arc::clone(&out);
            (Arc::new(move |json| clone.lock().unwrap().push(json)), out)
        }

        #[test]
        fn seq_is_monotonic_from_one() {
            let seq = Seq::default();
            assert_eq!(seq.next(), 1);
            assert_eq!(seq.next(), 2);
            assert_eq!(seq.next(), 3);
        }

        #[test]
        fn envelope_carries_seq_and_event() {
            let (sink, out) = collecting();
            let seq = Seq::default();
            emit_envelope(
                &Some(sink),
                &seq,
                CoreEvent::Error {
                    error: ErrorBody::new(ErrorCode::Internal, "boom"),
                },
            );
            let json = &out.lock().unwrap()[0];
            assert!(json.contains("\"seq\":1"), "seq 应从 1 开始: {json}");
            assert!(json.contains("\"boom\""));
        }

        #[test]
        fn missing_sink_is_noop_and_does_not_consume_seq() {
            let seq = Seq::default();
            emit_envelope(
                &None,
                &seq,
                CoreEvent::AgentDone {
                    session_id: "s".into(),
                    stop_reason: "end_turn".into(),
                },
            );
            assert_eq!(seq.next(), 1, "无 sink 时不得消耗 seq");
        }
    }
}
