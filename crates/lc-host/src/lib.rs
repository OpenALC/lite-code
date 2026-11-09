//! 共享宿主编排（docs/plans/lc-host-extraction.md）。
//!
//! M1：事件出口原语——`EventSink`、单调 seq 的 `EventEnvelope` 封装。
//! M2：会话编排骨架——`SessionHandle` 状态原语 + `Sessions` 会话表
//! （map/limit/ordinal，插入即回收，取句柄与置 running 同锁）。
//! 桌面（Tauri 事件插件）与鸿蒙（NAPI → ArkTS → WebView）共用同一信封
//! 形状、seq 语义与会话生命周期语义；后续 M3 在此之上叠加 TurnRunner。

pub mod envelope {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use lc_proto::{CoreEvent, EventEnvelope};

    /// 事件出口：接收 `EventEnvelope` 的 JSON 字符串。宿主各自实现
    /// （Tauri 事件插件 / NAPI threadsafe function → runJavaScript）。
    pub type EventSink = Arc<dyn Fn(String) + Send + Sync>;

    /// CoreEvent 级事件出口（桌面形状，M3a 下沉）：生产实现把事件推到底层
    /// 通道（Tauri 事件插件）。鸿蒙的 JSON fn-pointer 出口见 [`EventSink`]；
    /// 两形状经 [`envelope`] 统一信封与 seq 语义。
    pub trait CoreEventSink: Send + Sync {
        fn emit(&self, event: CoreEvent);
    }

    /// 单调 seq（1 起）。进程内唯一，跨会话连续；web 侧按 seq 检测丢序。
    /// 内部 Arc：`Seq` 可 clone，跨 worker 线程共享同一计数器。
    #[derive(Default, Clone)]
    pub struct Seq(Arc<AtomicU64>);

    impl Seq {
        pub fn next(&self) -> u64 {
            self.0.fetch_add(1, Ordering::SeqCst) + 1
        }

        /// 从已有计数续接（种子 n：下一个返回 n+1）。桌面迁移用：替换
        /// `AtomicU64::new(n)` 种子场景（测试断言首信封 seq 连续）。
        pub fn from_u64(seed: u64) -> Self {
            Self(Arc::new(AtomicU64::new(seed)))
        }
    }

    /// 封装一条契约事件为信封 JSON 并推给 sink；sink 缺失或序列化失败为 no-op。
    pub fn emit_envelope(sink: &Option<EventSink>, seq: &Seq, event: CoreEvent) {
        let Some(sink) = sink else { return };
        let envelope = envelope(seq, event);
        if let Ok(json) = serde_json::to_string(&envelope) {
            sink(json);
        }
    }

    /// 包一条契约事件为信封（seq 取共享 [`Seq`] 的下一个值，1 起，SeqCst）。
    /// 桌面 `next_envelope` 的下移替代：两端信封形状与 seq 语义由此统一。
    pub fn envelope(seq: &Seq, event: CoreEvent) -> EventEnvelope {
        EventEnvelope {
            seq: seq.next(),
            event,
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::{Arc, Mutex};

        use lc_proto::{CoreEvent, ErrorBody, ErrorCode};

        use super::{CoreEventSink, EventSink, Seq, emit_envelope, envelope};

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
        fn seq_from_u64_continues_after_seed() {
            let seq = Seq::from_u64(41);
            assert_eq!(seq.next(), 42);
            assert_eq!(seq.next(), 43);
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

        #[test]
        fn envelope_wraps_seq_and_event() {
            let seq = Seq::default();
            let first = envelope(
                &seq,
                CoreEvent::AgentDone {
                    session_id: "s".into(),
                    stop_reason: "end_turn".into(),
                },
            );
            assert_eq!(first.seq, 1);
            let second = envelope(
                &seq,
                CoreEvent::Error {
                    error: ErrorBody::new(ErrorCode::Internal, "boom"),
                },
            );
            assert_eq!(second.seq, 2, "envelope() 与 emit_envelope() 同一 seq 语义");
        }

        #[test]
        fn core_event_sink_trait_is_implementable() {
            struct Recording {
                events: Mutex<Vec<CoreEvent>>,
            }
            impl CoreEventSink for Recording {
                fn emit(&self, event: CoreEvent) {
                    self.events.lock().unwrap().push(event);
                }
            }
            let recording = Arc::new(Recording {
                events: Mutex::new(Vec::new()),
            });
            let boxed: Arc<dyn CoreEventSink> = recording.clone();
            boxed.emit(CoreEvent::AgentDone {
                session_id: "s".into(),
                stop_reason: "end_turn".into(),
            });
            assert_eq!(recording.events.lock().unwrap().len(), 1);
        }
    }
}

/// M2：会话编排骨架（docs/plans/lc-host-extraction.md §3 M2）。
///
/// 从 lc-ohos 原样下沉的会话生命周期语义：句柄状态原语（info/cancel/running/
/// alive）+ 会话表（map + 上限 + 创建序号）。宿主侧私有负载（lc-ohos 的
/// `Arc<Mutex<Option<AgentLoop>>>` 槽位）由 `SessionEntry<P>` 的泛型承载，
/// 本模块不依赖 lc-core。
pub mod sessions {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use lc_proto::{ErrorBody, ErrorCode, SessionInfo};

    /// 会话表默认上限。超出后按创建顺序回收已终止会话；`running`（含已请求
    /// 取消、worker 尚未退出的会话）绝不回收。单测/嵌入式可用
    /// [`Sessions::new`] 覆盖（limit 最小为 1）。
    pub const DEFAULT_MAX_SESSIONS: usize = 32;

    #[derive(Clone, Debug)]
    pub struct SessionHandle {
        info: Arc<Mutex<SessionInfo>>,
        cancel: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
        alive: Arc<AtomicBool>,
        /// 创建序号（来自 `Sessions::next_ordinal`），回收时按此从老到新。
        created: u64,
        model: Option<String>,
        thinking: Option<String>,
    }

    impl SessionHandle {
        pub fn new(
            info: SessionInfo,
            created: u64,
            model: Option<String>,
            thinking: Option<String>,
        ) -> Self {
            Self {
                info: Arc::new(Mutex::new(info)),
                cancel: Arc::new(AtomicBool::new(false)),
                running: Arc::new(AtomicBool::new(false)),
                alive: Arc::new(AtomicBool::new(true)),
                created,
                model,
                thinking,
            }
        }

        /// 会话信息锁：宿主在轮次开始时间自增 turn、事件出口写回快照。
        pub fn info_lock(&self) -> MutexGuard<'_, SessionInfo> {
            self.info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        pub fn info_snapshot(&self) -> SessionInfo {
            self.info_lock().clone()
        }

        /// 取消令牌：宿主把它接进自己的 agent 运行时（`with_cancel` 等），
        /// 自行 load/store/swap；`Sessions` 不解释其语义。
        pub fn cancel_token(&self) -> &Arc<AtomicBool> {
            &self.cancel
        }

        pub fn is_running(&self) -> bool {
            self.running.load(Ordering::Acquire)
        }

        /// worker 完全退出（含 panic 收尾）后调用；`running = false` 即表示
        /// 该会话不会再有事件推送，回收侧即可把它视为已终止。
        pub fn finish_running(&self) {
            self.running.store(false, Ordering::Release);
        }

        /// 只有 worker 完全退出才可回收。取消中的 worker 在发完取消/完成事件
        /// 前仍是 running，因此这里无需单独看 `cancel`；该标志在完成竞态下可能
        /// 残留，不能作为依据。
        pub fn is_reclaimable(&self) -> bool {
            !self.is_running()
        }

        /// 从会话表回收时由 `Sessions` 调用：所有 clone 共享，worker 侧迟到
        /// 事件据此变成 no-op。
        pub fn reclaim(&self) {
            self.alive.store(false, Ordering::Release);
        }

        pub fn is_alive(&self) -> bool {
            self.alive.load(Ordering::Acquire)
        }

        pub fn model(&self) -> Option<&str> {
            self.model.as_deref()
        }

        pub fn thinking(&self) -> Option<&str> {
            self.thinking.as_deref()
        }

        fn created(&self) -> u64 {
            self.created
        }
    }

    /// 句柄 + 宿主私有负载。lc-host 不依赖 lc-core；负载类型由调用方决定
    /// （lc-ohos = `Arc<Mutex<Option<AgentLoop>>>` 槽位，桌面 M3 = worker 句柄）。
    #[derive(Clone, Debug)]
    pub struct SessionEntry<P> {
        pub handle: SessionHandle,
        pub payload: P,
    }

    impl<P> SessionEntry<P> {
        pub fn new(handle: SessionHandle, payload: P) -> Self {
            Self { handle, payload }
        }
    }

    /// 会话表：map + 上限 + 创建序号。插入即回收；`begin_turn` 在同一把
    /// map 锁内查表并原子置 `running`，拿到的句柄不可能在置位前被回收。
    pub struct Sessions<P> {
        map: Mutex<HashMap<String, SessionEntry<P>>>,
        limit: usize,
        ordinal: AtomicU64,
    }

    impl<P> Sessions<P> {
        /// `initial_ordinal`：进程重启后从已落盘历史续接序号（宿主传入历史
        /// 目录中的最大序号），避免新会话覆盖同名旧记录。
        pub fn new(limit: usize, initial_ordinal: u64) -> Self {
            Self {
                map: Mutex::new(HashMap::new()),
                limit: limit.max(1),
                ordinal: AtomicU64::new(initial_ordinal),
            }
        }

        /// 下一个创建序号（1 起，单调）。
        pub fn next_ordinal(&self) -> u64 {
            self.ordinal.fetch_add(1, Ordering::Relaxed) + 1
        }

        pub fn len(&self) -> usize {
            self.map_lock().len()
        }

        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }

        /// 插入新会话；超出上限时按创建顺序回收已终止会话。
        pub fn insert(&self, id: String, entry: SessionEntry<P>) {
            let mut map = self.map_lock();
            map.insert(id, entry);
            self.reclaim_terminated(&mut map);
        }

        fn map_lock(&self) -> MutexGuard<'_, HashMap<String, SessionEntry<P>>> {
            self.map
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        /// 超出上限时按创建序号从老到新回收已终止会话。running（含取消中的
        /// worker）的会话跳过；若全部在跑，会话表暂时超限，等后续创建再回收。
        fn reclaim_terminated(&self, map: &mut HashMap<String, SessionEntry<P>>) {
            if map.len() <= self.limit {
                return;
            }
            let mut candidates: Vec<(u64, String)> = map
                .iter()
                .filter(|(_, entry)| entry.handle.is_reclaimable())
                .map(|(id, entry)| (entry.handle.created(), id.clone()))
                .collect();
            candidates.sort_unstable_by_key(|(created, _)| *created);
            let excess = map.len() - self.limit;
            for (_, id) in candidates.into_iter().take(excess) {
                if let Some(entry) = map.remove(&id) {
                    entry.handle.reclaim();
                }
            }
        }
    }

    impl<P: Clone> Sessions<P> {
        pub fn get(&self, id: &str) -> Option<SessionEntry<P>> {
            self.map_lock().get(id).cloned()
        }

        /// 取句柄并原子置 `running`；与回收共用同一把 map 锁，避免刚取到的
        /// 句柄在被置位前就被回收（回收只挑 `running = false` 的会话）。
        pub fn begin_turn(&self, id: &str) -> Result<SessionEntry<P>, ErrorBody> {
            let map = self.map_lock();
            let entry = map.get(id).cloned().ok_or_else(|| not_found(id))?;
            entry
                .handle
                .running
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| {
                    ErrorBody::new(
                        ErrorCode::InvalidArgument,
                        format!("agent turn already running: {id}"),
                    )
                })?;
            Ok(entry)
        }
    }

    fn not_found(id: &str) -> ErrorBody {
        ErrorBody::new(ErrorCode::NotFound, format!("session not found: {id}"))
    }

    #[cfg(test)]
    mod tests {
        use lc_proto::{ErrorCode, SessionInfo, SessionMode};

        use super::{DEFAULT_MAX_SESSIONS, SessionEntry, SessionHandle, Sessions};

        fn entry(ordinal: u64) -> SessionEntry<()> {
            SessionEntry::new(handle(ordinal), ())
        }

        fn handle(ordinal: u64) -> SessionHandle {
            SessionHandle::new(
                SessionInfo {
                    session_id: format!("s-{ordinal}"),
                    mode: SessionMode::Plan,
                    turn: 0,
                    max_turns: 25,
                    context_k: 400,
                    compress_at_k: 300,
                    cancelled: false,
                },
                ordinal,
                None,
                None,
            )
        }

        fn insert(sessions: &Sessions<()>, ordinal: u64) -> SessionHandle {
            let entry = entry(ordinal);
            let handle = entry.handle.clone();
            sessions.insert(format!("s-{ordinal}"), entry);
            handle
        }

        #[test]
        fn default_limit_is_32() {
            assert_eq!(DEFAULT_MAX_SESSIONS, 32);
        }

        #[test]
        fn ordinal_starts_after_initial_and_increments() {
            let sessions: Sessions<()> = Sessions::new(4, 41);
            assert_eq!(sessions.next_ordinal(), 42);
            assert_eq!(sessions.next_ordinal(), 43);
        }

        #[test]
        fn insert_beyond_limit_reclaims_oldest_terminated() {
            let sessions: Sessions<()> = Sessions::new(2, 0);
            insert(&sessions, 1);
            insert(&sessions, 2);
            insert(&sessions, 3);
            assert_eq!(sessions.len(), 2);
            assert!(sessions.get("s-1").is_none(), "最老的已终止会话被回收");
            assert!(sessions.get("s-2").is_some());
            assert!(sessions.get("s-3").is_some());
        }

        #[test]
        fn reclaim_keeps_running_sessions_and_terminates_reclaimed() {
            // 与 lc-ohos 回收语义一致：running 的会话绝不回收；新插入的已终止
            // 会话若超上限，立即按创建序回收（此处即回收新会话自己）。
            let sessions: Sessions<()> = Sessions::new(1, 0);
            let running = insert(&sessions, 1);
            sessions.begin_turn("s-1").unwrap();
            assert!(running.is_running());
            insert(&sessions, 2);
            assert_eq!(sessions.len(), 1, "running 会话保留，新会话被立即回收");
            assert!(sessions.get("s-1").is_some());
            assert!(sessions.get("s-2").is_none());
            running.finish_running();
            insert(&sessions, 3);
            assert_eq!(sessions.len(), 1);
            assert!(sessions.get("s-1").is_none(), "finish 后按创建序回收");
            assert!(sessions.get("s-3").is_some());
        }

        #[test]
        fn all_running_sessions_keep_newest_terminated_reclaimable() {
            // 上限内的会话全部 running 时，新建会话一生成即超上限，且它是唯一
            // 可回收的，插入即被回收（lc-ohos create 路径的真实语义）。
            let sessions: Sessions<()> = Sessions::new(2, 0);
            insert(&sessions, 1);
            sessions.begin_turn("s-1").unwrap();
            insert(&sessions, 2);
            sessions.begin_turn("s-2").unwrap();
            insert(&sessions, 3);
            assert_eq!(sessions.len(), 2, "新会话是唯一可回收项，插入即回收");
            assert!(sessions.get("s-1").is_some());
            assert!(sessions.get("s-2").is_some());
            assert!(sessions.get("s-3").is_none());
            sessions.get("s-1").unwrap().handle.finish_running();
            insert(&sessions, 4);
            assert_eq!(sessions.len(), 2);
            assert!(sessions.get("s-1").is_none(), "最老的已终止会话先回收");
            assert!(sessions.get("s-4").is_some());
        }

        #[test]
        fn reclaimed_handle_is_not_alive_so_late_events_drop() {
            let sessions: Sessions<()> = Sessions::new(1, 0);
            let stale = insert(&sessions, 1);
            assert!(stale.is_alive());
            insert(&sessions, 2);
            assert!(!stale.is_alive(), "回收后 alive=false，迟到事件变 no-op");
            assert!(sessions.get("s-1").is_none());
        }

        #[test]
        fn begin_turn_sets_running_and_rejects_concurrent_turn() {
            let sessions: Sessions<()> = Sessions::new(4, 0);
            insert(&sessions, 1);
            assert!(sessions.begin_turn("s-1").is_ok());
            let error = sessions.begin_turn("s-1").unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidArgument);
            assert!(
                error.message.contains("already running"),
                "并发轮次拒绝文案: {}",
                error.message
            );
        }

        #[test]
        fn begin_turn_unknown_session_returns_not_found() {
            let sessions: Sessions<()> = Sessions::new(4, 0);
            let error = sessions.begin_turn("ghost").unwrap_err();
            assert_eq!(error.code, ErrorCode::NotFound);
            assert!(error.message.contains("session not found: ghost"));
        }

        #[test]
        fn entry_carries_handle_and_payload() {
            let sessions: Sessions<String> = Sessions::new(4, 0);
            let entry = SessionEntry::new(
                SessionHandle::new(
                    SessionInfo {
                        session_id: "s-9".into(),
                        mode: SessionMode::Plan,
                        turn: 7,
                        max_turns: 25,
                        context_k: 400,
                        compress_at_k: 300,
                        cancelled: false,
                    },
                    9,
                    Some("m".into()),
                    Some("high".into()),
                ),
                "payload".to_owned(),
            );
            assert_eq!(entry.handle.model(), Some("m"));
            assert_eq!(entry.handle.thinking(), Some("high"));
            assert_eq!(entry.handle.info_snapshot().turn, 7);
            sessions.insert("s-9".into(), entry);
            assert_eq!(sessions.get("s-9").unwrap().payload, "payload");
        }
    }
}
