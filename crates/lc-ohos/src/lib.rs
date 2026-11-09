//! 鸿蒙宿主：纯 JSON 入口，host 上可单测，不依赖 NAPI。
//!
//! 首版命令面（docs/plans/ohos-shell.md §5.4）：
//! - `workspace_root` / `fs_read_dir` / `fs_read_file` 复用 lc-core 的路径校验与按层读取；
//! - `session_create` 强制 `plan` 模式（只读 + 对话），记录模型/思考挡位；
//!   会话表默认上限 32，超出后按创建顺序回收已终止会话（running 的绝不回收）；
//! - `agent_send` 首次发送时按 `providers_file` 构造 lc-core `AgentLoop`：plan 策略
//!   + `read_file`/`glob`/`grep` 只读工具，事件逐条映射成契约 `EventEnvelope`；
//!     构造前先用 800ms 预算预探 provider 的 host:port（DNS 解析与 TCP 连接共用），
//!     不通直接回退 stub，不再等 ureq 的 5s 连接超时；provider 缺失或流传输失败时
//!     回退到 20 个 `[stub]` delta（`done.stop_reason = "stub"`）；
//! - `agent_cancel` 置位 `AgentLoop` 共享的取消令牌；
//! - `providers_load` / `providers_save` 读写 `HostConfig.providers_file`，形状与桌面
//!   `providers.rs` 的 `ProvidersView` 一致（密钥只回传 `hasApiKey`/末四位提示）；
//! - `history_list` / `history_save` / `history_load` / `history_delete` 把会话快照
//!   持久化到 `<files_dir>/history/<id>.json`（0600、tmp/rename、上限 50 条），
//!   形状与桌面 `history.rs` 及前端 `HistorySummary` 一致；
//! - 其余命令返回 `not_implemented`（契约没有独立的 `unsupported` 错误码）。

use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use lc_core::agent::{
    AgentLoop, CompactionRecord, LocalToolExecutor, LoopError, LoopOutcome, Risk, Session,
    ToolError, ToolOutput, ToolRegistry, ToolSpec, llm_event, loop_outcome, session_info,
    session_instructions,
};
use lc_core::fs::{RealPathResolver, display_path, ensure_within_root, read_dir_level};
use lc_core::llm::runtime::load_from_model;
use lc_core::llm::{
    AgentRuntime, LlmEvent, OpenAiResponsesClient, ProviderError, ProviderKind, ProvidersFile,
    ProvidersSaveInput, RuntimeError, UreqSseTransport, merge_and_validate, view_from_file,
};
use lc_proto::{
    CommandResult, CoreEvent, ErrorBody, ErrorCode, EventEnvelope, SessionInfo, SessionMode,
};
use serde_json::Value;

pub mod ffi;

mod history;

/// 视图类型保留在 crate 根，避免破坏 lc-ohos 既有公开接口。
pub use lc_core::llm::{ProviderView, ProvidersView};

pub type EventSink = Arc<dyn Fn(String) + Send + Sync>;

const STUB_DELTAS: u32 = 20;
const STUB_DELAY: Duration = Duration::from_millis(25);
const STUB_STOP_REASON: &str = "stub";
const CANCELLED_STOP_REASON: &str = "cancelled";

/// 会话表默认上限。超出后按创建顺序回收已终止会话；`running`（含已请求取消、
/// worker 尚未退出的会话）绝不回收。单测/嵌入式可用
/// [`Host::init_with_factory_and_limit`] 覆盖。
const MAX_SESSIONS: usize = 32;

/// 构建 `AgentLoop` 前的 TCP 预探上限。ureq 的连接超时是 5s；离线时先用
/// 800ms（DNS 解析 + TCP 连接）判断 host:port 是否可达，避免首条消息停 5s
/// 才回退 stub。
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

const DEFAULT_INSTRUCTIONS: &str =
    "你是 Lite Code 的编码助手。先阅读相关文件再回答；使用工具完成请求；用中文简洁回答。";

/// 模板里的 `apiKeyRef` 在模拟器上通常没有对应环境变量。缺省时用占位 key
/// 真正发出请求：宿主 mock 忽略鉴权，真实 provider 会返回 401 并回退 stub，
/// 不会把占位值当成真实密钥。
const PLACEHOLDER_API_KEY: &str = "lc-ohos-local";

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub files_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub providers_file: PathBuf,
}

/// 一轮会话构建 `AgentLoop` 所需的只读输入。
pub struct AgentBuildRequest<'a> {
    pub session_id: &'a str,
    pub workspace_root: &'a Path,
    pub providers_file: &'a Path,
    pub model: Option<&'a str>,
    pub thinking: Option<&'a str>,
    pub max_turns: u32,
    /// 与 `AgentLoop::with_cancel` 共享；注入的传输可据此提前结束流。
    pub cancel: Arc<AtomicBool>,
}

/// `AgentLoop` 工厂。生产实现解析 `providers_file`；单测注入 fake LLM，
/// 不启动网络。
pub trait AgentFactory: Send + Sync {
    fn create(&self, request: &AgentBuildRequest<'_>) -> Result<AgentLoop, ErrorBody>;
}

pub struct Host {
    root: PathBuf,
    root_display: String,
    providers_file: PathBuf,
    history_dir: PathBuf,
    factory: Arc<dyn AgentFactory>,
    sessions: Mutex<HashMap<String, SessionHandle>>,
    max_sessions: usize,
    sink: Mutex<Option<EventSink>>,
    seq: Arc<AtomicU64>,
    next_session: AtomicU64,
}

#[derive(Clone)]
struct SessionHandle {
    info: Arc<Mutex<SessionInfo>>,
    /// 首次 `agent_send` 成功构建后常驻，用于保留多轮上下文。
    agent: Arc<Mutex<Option<AgentLoop>>>,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    /// 从会话表回收时置 `false`；所有 clone 共享，worker 侧迟到事件据此变成 no-op。
    alive: Arc<AtomicBool>,
    /// 创建序号（来自 `next_session`），回收时按此从老到新。
    created: u64,
    model: Option<String>,
    thinking: Option<String>,
}

impl SessionHandle {
    fn new(
        info: SessionInfo,
        created: u64,
        model: Option<String>,
        thinking: Option<String>,
    ) -> Self {
        Self {
            info: Arc::new(Mutex::new(info)),
            agent: Arc::new(Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            alive: Arc::new(AtomicBool::new(true)),
            created,
            model,
            thinking,
        }
    }

    fn info_snapshot(&self) -> SessionInfo {
        self.info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 只有 worker 完全退出（`running = false`）才可回收。取消中的 worker 在
    /// 发完 `session_state(cancelled)`/`done(cancelled)` 前仍是 running，因此
    /// 这里无需单独看 `cancel`；该标志在完成竞态下可能残留，不能作为依据。
    fn is_reclaimable(&self) -> bool {
        !self.running.load(Ordering::Acquire)
    }

    fn reclaim(&self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl Host {
    pub fn init(config: HostConfig) -> Result<Host, ErrorBody> {
        Self::init_with_factory(config, Arc::new(RealAgentFactory))
    }

    /// 注入自定义 `AgentLoop` 工厂；生产走 [`Host::init`]。
    pub fn init_with_factory(
        config: HostConfig,
        factory: Arc<dyn AgentFactory>,
    ) -> Result<Host, ErrorBody> {
        Self::init_with_factory_and_limit(config, factory, MAX_SESSIONS)
    }

    /// 注入工厂并指定会话上限；生产默认 [`MAX_SESSIONS`]。
    pub fn init_with_factory_and_limit(
        config: HostConfig,
        factory: Arc<dyn AgentFactory>,
        max_sessions: usize,
    ) -> Result<Host, ErrorBody> {
        let root = ensure_within_root(
            &config.workspace_root,
            &config.workspace_root,
            &RealPathResolver,
        )
        .map_err(|error| error.to_error_body())?;
        if !root.is_dir() {
            return Err(invalid(format!(
                "workspace root is not a directory: {}",
                display_path(&root)
            )));
        }
        fs::create_dir_all(&config.files_dir).map_err(|error| io_body(&config.files_dir, error))?;
        if let Some(parent) = config.providers_file.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| io_body(parent, error))?;
        }
        let history_dir = config.files_dir.join("history");
        fs::create_dir_all(&history_dir).map_err(|error| io_body(&history_dir, error))?;
        // 进程重启后从已落盘历史续接序号，避免新会话覆盖同名旧记录。
        let next_session = history::max_session_ordinal(&history_dir);
        Ok(Host {
            root_display: display_path(&root),
            root,
            providers_file: config.providers_file,
            history_dir,
            factory,
            sessions: Mutex::new(HashMap::new()),
            max_sessions: max_sessions.max(1),
            sink: Mutex::new(None),
            seq: Arc::new(AtomicU64::new(0)),
            next_session: AtomicU64::new(next_session),
        })
    }

    /// 命令入口：成功返回契约结果的 JSON，失败返回 `ErrorBody`。
    pub fn dispatch(&self, cmd: &str, args_json: &str) -> Result<String, ErrorBody> {
        let args = parse_args(args_json)?;
        let result = match cmd {
            "workspace_root" => CommandResult::WorkspaceRoot {
                root: Some(self.root_display.clone()),
            },
            "fs_read_dir" => {
                let path = self.resolve_path(&args)?;
                let level = read_dir_level(&self.root, &path, &RealPathResolver)
                    .map_err(|error| error.to_error_body())?;
                CommandResult::DirListing {
                    path: level.path,
                    entries: level.entries,
                    generation: 0,
                }
            }
            "fs_read_file" => {
                let path = self.resolve_path(&args)?;
                let canonical = ensure_within_root(&self.root, &path, &RealPathResolver)
                    .map_err(|error| error.to_error_body())?;
                let bytes = fs::read(&canonical).map_err(|error| io_body(&canonical, error))?;
                let text = String::from_utf8(bytes).map_err(|_| {
                    ErrorBody::new(
                        ErrorCode::Io,
                        format!("file is not valid UTF-8: {}", display_path(&canonical)),
                    )
                })?;
                CommandResult::FileContent {
                    path: display_path(&canonical),
                    text,
                }
            }
            "session_create" => CommandResult::Session {
                session: self.create_session(&args),
            },
            "agent_send" => {
                self.agent_send(&args)?;
                CommandResult::Ack
            }
            "agent_cancel" => {
                self.agent_cancel(&args)?;
                CommandResult::Ack
            }
            "providers_load" => return self.providers_load(),
            "providers_save" => return self.providers_save(&args),
            "history_list" => return self.history_list(),
            "history_load" => return self.history_load(&args),
            "history_save" => return self.history_save(&args),
            "history_delete" => return self.history_delete(&args),
            _ => return Err(unsupported(cmd)),
        };
        serde_json::to_string(&result).map_err(|error| {
            ErrorBody::new(ErrorCode::Internal, format!("serialize result: {error}"))
        })
    }

    /// 原生事件出口：`EventEnvelope` JSON 字符串。可重复设置（页面重建后换 sink）。
    pub fn set_event_sink(&self, sink: EventSink) {
        *self.sink_slot() = Some(sink);
    }

    fn create_session(&self, args: &Value) -> SessionInfo {
        let number = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        let model = optional_string(args, "model");
        let thinking = optional_string(args, "thinking");
        let (context_k, compress_at_k) = self.model_budget(model.as_deref());
        let info = SessionInfo {
            session_id: format!("ohos-{number}"),
            mode: SessionMode::Plan,
            turn: 0,
            max_turns: 25,
            context_k,
            compress_at_k,
            cancelled: false,
        };
        {
            let mut sessions = self.sessions_slot();
            sessions.insert(
                info.session_id.clone(),
                SessionHandle::new(info.clone(), number, model, thinking),
            );
            reclaim_terminated_sessions(&mut sessions, self.max_sessions);
        }
        info
    }

    /// 会话预算跟随所选模型的 `contextK/compressAtK`；缺省模型或配置缺失时回退 400/300。
    fn model_budget(&self, model: Option<&str>) -> (u32, u32) {
        let Some(model) = model else {
            return (400, 300);
        };
        let Ok(Some(file)) = ProvidersFile::load_path(&self.providers_file) else {
            return (400, 300);
        };
        for config in &file.models {
            if config.id == model {
                let context_k = config.context_k.max(1);
                return (context_k, config.compress_at_k.clamp(1, context_k));
            }
        }
        (400, 300)
    }

    fn agent_send(&self, args: &Value) -> Result<(), ErrorBody> {
        let session_id = required_string(args, "session_id")?;
        let text = required_string(args, "text")?;
        // 取句柄与置 running 在同一把 sessions 锁内完成：拿到的句柄不可能在
        // 置位前被回收；置位后回收侧会因 running=true 跳过它。
        let handle = self.begin_turn(&session_id)?;
        handle.cancel.store(false, Ordering::Release);
        {
            let mut info = handle
                .info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            info.turn = info.turn.saturating_add(1);
        }
        let sink = self.sink_slot().clone();
        let seq = Arc::clone(&self.seq);
        let factory = Arc::clone(&self.factory);
        let root = self.root.clone();
        let providers_file = self.providers_file.clone();
        let thread_handle = handle.clone();
        let thread_session = session_id.clone();
        let spawned = thread::Builder::new()
            .name(format!("agent-{session_id}"))
            .spawn(move || {
                // 守卫覆盖 worker 的全部事件（含 panic 分支的 error + done）：
                // running=false 即表示该会话不会再有事件推送，可安全回收。
                let _running = RunningGuard(thread_handle.clone());
                let emitter = TurnEmitter::new(thread_session, thread_handle.clone(), sink, seq);
                let panic_emitter = emitter.clone();
                let turn = AgentTurn {
                    text,
                    handle: thread_handle,
                    root,
                    providers_file,
                    factory,
                };
                let outcome = catch_unwind(AssertUnwindSafe(|| run_agent_turn(&emitter, turn)));
                if outcome.is_err() {
                    eprintln!("[ohos-agent] worker panicked; emitting error + done");
                    panic_emitter
                        .error(ErrorBody::new(ErrorCode::Internal, "agent worker panicked"));
                    panic_emitter.done("error");
                }
            });
        if let Err(error) = spawned {
            handle.running.store(false, Ordering::Release);
            return Err(ErrorBody::new(
                ErrorCode::Internal,
                format!("failed to start agent worker: {error}"),
            ));
        }
        Ok(())
    }

    fn agent_cancel(&self, args: &Value) -> Result<(), ErrorBody> {
        let session_id = required_string(args, "session_id")?;
        let handle = self.session(&session_id)?;
        if handle.running.load(Ordering::Acquire) {
            // 只置位共享令牌，不调用 `AgentLoop::cancel()`：后者会把 lc-core
            // `Session` 永久标记为已取消，同一会话无法继续。worker 在本轮结束
            // 后复位令牌，并补发 session_state(cancelled) + done(cancelled)。
            handle.cancel.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// 读取供应商配置。文件缺失或 JSON 损坏时返回空视图：壳内已有供应商
    /// 编辑入口（ProvidersPanel 经 shim 调用本命令），空视图让前端显示
    /// 「请先配置供应商」引导，而不是整页报错。
    fn providers_load(&self) -> Result<String, ErrorBody> {
        let file = match ProvidersFile::load_path(&self.providers_file) {
            Ok(Some(file)) => file,
            Ok(None) | Err(ProviderError::Parse(_)) => ProvidersFile::default(),
            Err(error) => return Err(provider_error(error)),
        };
        serialize_view(file)
    }

    /// 保存供应商配置。入参形状与桌面 `providers_save` 一致：`{ input }`，
    /// 空 `apiKey` 表示保留旧值；写入复用 lc-core 的 0600 + tmp/rename。
    fn providers_save(&self, args: &Value) -> Result<String, ErrorBody> {
        let input = args
            .get("input")
            .ok_or_else(|| invalid("missing object argument `input`"))?;
        let input: ProvidersSaveInput = serde_json::from_value(input.clone())
            .map_err(|error| invalid(format!("invalid providers input: {error}")))?;
        // 鸿蒙契约对空 models 的报错文案要求包含 `models`，与 lc-core
        // `ProviderError::Empty` 的通用文案不同；保留这层薄适配以维持现有契约。
        if input.models.is_empty() {
            return Err(invalid("models must be a non-empty array"));
        }
        let existing = ProvidersFile::load_path(&self.providers_file).map_err(provider_error)?;
        let file = merge_and_validate(existing, input).map_err(provider_error)?;
        file.save_path(&self.providers_file)
            .map_err(provider_error)?;
        serialize_view(file)
    }

    fn history_list(&self) -> Result<String, ErrorBody> {
        serialize_json(&history::list(&self.history_dir)?)
    }

    fn history_load(&self, args: &Value) -> Result<String, ErrorBody> {
        let id = required_string(args, "id")?;
        serialize_json(&history::load(&self.history_dir, &id)?)
    }

    fn history_save(&self, args: &Value) -> Result<String, ErrorBody> {
        let id = required_string(args, "id")?;
        let payload = args
            .get("payload")
            .ok_or_else(|| invalid("missing object argument `payload`"))?;
        history::save(&self.history_dir, &id, payload)?;
        Ok("null".to_string())
    }

    fn history_delete(&self, args: &Value) -> Result<String, ErrorBody> {
        let id = required_string(args, "id")?;
        history::delete(&self.history_dir, &id)?;
        Ok("null".to_string())
    }

    fn session(&self, id: &str) -> Result<SessionHandle, ErrorBody> {
        self.sessions_slot()
            .get(id)
            .cloned()
            .ok_or_else(|| not_found_session(id))
    }

    /// 取句柄并原子置 `running`；与回收共用 sessions 锁，避免刚取到的句柄
    /// 在被置位前就被回收（回收只挑 `running = false` 的会话）。
    fn begin_turn(&self, id: &str) -> Result<SessionHandle, ErrorBody> {
        let sessions = self.sessions_slot();
        let handle = sessions
            .get(id)
            .cloned()
            .ok_or_else(|| not_found_session(id))?;
        handle
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| invalid(format!("agent turn already running: {id}")))?;
        Ok(handle)
    }

    fn resolve_path(&self, args: &Value) -> Result<PathBuf, ErrorBody> {
        let raw = required_string(args, "path")?;
        let path = PathBuf::from(raw);
        Ok(if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        })
    }

    fn sessions_slot(&self) -> MutexGuard<'_, HashMap<String, SessionHandle>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn sink_slot(&self) -> MutexGuard<'_, Option<EventSink>> {
        self.sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn not_found_session(id: &str) -> ErrorBody {
    ErrorBody::new(ErrorCode::NotFound, format!("session not found: {id}"))
}

/// 超出上限时按创建序号从老到新回收已终止会话。running（含取消中的 worker）
/// 的会话跳过；若全部在跑，会话表暂时超限，等后续创建再回收。
fn reclaim_terminated_sessions(sessions: &mut HashMap<String, SessionHandle>, max_sessions: usize) {
    if sessions.len() <= max_sessions {
        return;
    }
    let mut candidates: Vec<(u64, String)> = sessions
        .iter()
        .filter(|(_, handle)| handle.is_reclaimable())
        .map(|(id, handle)| (handle.created, id.clone()))
        .collect();
    candidates.sort_unstable_by_key(|(created, _)| *created);
    let excess = sessions.len() - max_sessions;
    for (_, id) in candidates.into_iter().take(excess) {
        if let Some(handle) = sessions.remove(&id) {
            handle.reclaim();
        }
    }
}

fn serialize_json<T: serde::Serialize>(value: &T) -> Result<String, ErrorBody> {
    serde_json::to_string(value)
        .map_err(|error| ErrorBody::new(ErrorCode::Internal, format!("serialize result: {error}")))
}

fn serialize_view(file: ProvidersFile) -> Result<String, ErrorBody> {
    serde_json::to_string(&view_from_file(file)).map_err(|error| {
        ErrorBody::new(
            ErrorCode::Internal,
            format!("serialize providers view: {error}"),
        )
    })
}

fn provider_error(error: ProviderError) -> ErrorBody {
    let code = match error {
        ProviderError::Io(_) => ErrorCode::Io,
        _ => ErrorCode::InvalidArgument,
    };
    ErrorBody::new(code, error.to_string())
}

/// 生产工厂：`providers_file` → `AgentRuntime` → ureq SSE client。
struct RealAgentFactory;

impl AgentFactory for RealAgentFactory {
    fn create(&self, request: &AgentBuildRequest<'_>) -> Result<AgentLoop, ErrorBody> {
        let runtime = resolve_runtime(request.providers_file, request.model, |name| {
            std::env::var(name).ok()
        })
        .map_err(runtime_error_body)?;
        let client = match runtime.kind {
            ProviderKind::OpenAiResponses => {
                if let Some((host, port)) = unreachable_target(&runtime.base_url) {
                    return Err(ErrorBody::new(
                        ErrorCode::Io,
                        format!("provider is unreachable at {host}:{port} (TCP probe failed)"),
                    ));
                }
                OpenAiResponsesClient::new(
                    &runtime.base_url,
                    &runtime.api_key,
                    Box::new(UreqSseTransport::with_connect_timeout(Duration::from_secs(
                        5,
                    ))),
                )
            }
            ProviderKind::AnthropicMessages => {
                return Err(runtime_error_body(RuntimeError::UnsupportedKind(
                    runtime.kind,
                )));
            }
        };
        let reasoning = reasoning_effort(request.thinking, &runtime.thinking);
        let registry = read_only_registry();
        let session = Session::new(request.session_id, lc_core::agent::PolicyMode::Plan)
            .with_max_turns(request.max_turns)
            .with_context(runtime.context_k, runtime.compress_at_k);
        let instructions = session_instructions(request.workspace_root, DEFAULT_INSTRUCTIONS);
        Ok(AgentLoop::new(
            session,
            Box::new(client),
            Box::new(LocalToolExecutor::new(request.workspace_root)),
            registry,
            request.workspace_root,
            runtime.model_id,
        )
        .with_instructions(instructions)
        .with_reasoning(reasoning)
        .with_tools(read_only_registry().llm_tools()))
    }
}

/// plan 模式只暴露读工具；不注册 MCP、`task` 或 `db_*`，模型不会看到这些定义。
fn read_only_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for (name, risk) in [
        ("read_file", Risk::Read),
        ("glob", Risk::Read),
        ("grep", Risk::Read),
    ] {
        registry
            .register(ToolSpec::new(name, risk))
            .expect("只读工具清单必须合法");
    }
    registry
}

/// 读取 provider 配置。文件不存在时不回退环境变量（鸿蒙宿主始终有显式
/// `providers_file`），这样“未配置 provider”能稳定回退 stub。
fn resolve_runtime(
    path: &Path,
    model: Option<&str>,
    env: impl Fn(&str) -> Option<String> + Copy,
) -> Result<AgentRuntime, RuntimeError> {
    match ProvidersFile::load_path(path) {
        Ok(None) => return Err(RuntimeError::NoModel),
        Ok(Some(_)) => {}
        Err(error) => return Err(RuntimeError::Config(error)),
    }
    load_from_model(path, model, |name| {
        env(name).or_else(|| {
            // `LC_API_KEY` 是全局覆盖；未设置时继续走 apiKey/ref，不用占位值
            // 遮蔽用户保存的本地密钥。
            if name == "LC_API_KEY" {
                None
            } else {
                Some(PLACEHOLDER_API_KEY.to_owned())
            }
        })
    })
}

/// `off` 不发送 reasoning；`max` 归一到 Responses 支持的 `high`。
fn reasoning_effort(thinking: Option<&str>, supported: &[String]) -> Option<String> {
    let level = thinking.map(str::trim).filter(|level| !level.is_empty())?;
    if !supported.is_empty() && !supported.iter().any(|item| item == level) {
        eprintln!("[ohos-agent] thinking {level} is not advertised by the model; ignoring");
        return None;
    }
    match level {
        "off" => None,
        "low" => Some("low".into()),
        "medium" => Some("medium".into()),
        "high" | "max" => Some("high".into()),
        other => {
            eprintln!("[ohos-agent] unknown thinking level {other}; ignoring");
            None
        }
    }
}

fn runtime_error_body(error: RuntimeError) -> ErrorBody {
    match error {
        RuntimeError::Io(detail) => ErrorBody::new(
            ErrorCode::NotFound,
            format!("cannot read providers config ({detail})"),
        ),
        RuntimeError::Config(error) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("invalid providers config: {error}"),
        ),
        RuntimeError::NoModel => ErrorBody::new(
            ErrorCode::InvalidArgument,
            "providers config has no usable model",
        ),
        RuntimeError::UnknownModel(model) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("model {model} is not configured"),
        ),
        RuntimeError::ProviderUnavailable(provider) => ErrorBody::new(
            ErrorCode::NotFound,
            format!("provider {provider} is missing or disabled"),
        ),
        RuntimeError::UnresolvedKeyRef(reference) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("cannot resolve apiKeyRef {reference}: only env:NAME is supported"),
        ),
        RuntimeError::MissingEnv(name) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("environment variable {name} is not set"),
        ),
        RuntimeError::MissingApiKey => ErrorBody::new(
            ErrorCode::InvalidArgument,
            "provider has no API key (set apiKey, LC_API_KEY or apiKeyRef)",
        ),
        RuntimeError::UnsupportedKind(kind) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("provider kind {kind:?} has no streaming client"),
        ),
    }
}

/// 从 http(s) `baseUrl` 提取 `(host, port)`。协议不符或结构无法识别时返回
/// `None`，调用方按“可达”处理，把错误留给真正的 HTTP 传输。
fn probe_target(base_url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = base_url.trim().split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    // 去掉 userinfo（user:pass@host），只探测真正的 host:port。
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, tail) = bracketed.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        let port = match tail.strip_prefix(':') {
            Some(raw) => raw.parse().ok()?,
            None if tail.is_empty() => default_port,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, raw_port)) if !host.is_empty() => {
            Some((host.to_owned(), raw_port.parse().ok()?))
        }
        Some(_) => None,
        None => Some((authority.to_owned(), default_port)),
    }
}

/// 在 `PROBE_TIMEOUT` 内做一次 TCP 预探（https 也只到 TCP 层，不验 TLS）。
/// 返回 `Some((host, port))` 表示不可达；URL 无法解析时返回 `None`，把错误
/// 留给真正的 HTTP 传输。DNS 解析也受同一预算约束：解析超时/失败按不可达
/// 处理，直接回退 stub，避免 resolver 把首条消息拖过 800ms。
fn unreachable_target(base_url: &str) -> Option<(String, u16)> {
    let (host, port) = probe_target(base_url)?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let remaining = deadline.saturating_duration_since(Instant::now());
    let Some(addrs) = resolve_bounded(&host, port, remaining, resolve_host) else {
        eprintln!("[ohos-agent] cannot resolve {host}:{port} within probe budget");
        return Some((host, port));
    };
    for addr in addrs {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        if remaining.is_zero() {
            break;
        }
        if TcpStream::connect_timeout(&addr, remaining).is_ok() {
            return None;
        }
    }
    Some((host, port))
}

fn resolve_host(host: String, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    let addrs = (host.as_str(), port).to_socket_addrs()?;
    Ok(addrs.collect())
}

/// 在 `timeout` 内解析 `host:port`。IP 字面量直接构造地址，不创建线程；
/// 域名解析放到单个分离线程，调用方用 channel 超时等待。解析线程即使卡住
/// 也只会持有自己的 host 字符串，不影响宿主状态；OHOS 上与其他 worker 同样
/// 走 std thread。
fn resolve_bounded(
    host: &str,
    port: u16,
    timeout: Duration,
    resolve: impl FnOnce(String, u16) -> std::io::Result<Vec<SocketAddr>> + Send + 'static,
) -> Option<Vec<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(vec![SocketAddr::new(ip, port)]);
    }
    let (sender, receiver) = mpsc::channel();
    let name = host.to_owned();
    let spawned = thread::Builder::new()
        .name("lc-ohos-dns".into())
        .spawn(move || {
            // 接收端超时后已丢弃；发送失败无需处理。
            let _ = sender.send(resolve(name, port));
        });
    if spawned.is_err() {
        return None;
    }
    match receiver.recv_timeout(timeout) {
        Ok(Ok(addrs)) => Some(addrs),
        Ok(Err(_)) | Err(_) => None,
    }
}

struct AgentTurn {
    text: String,
    handle: SessionHandle,
    root: PathBuf,
    providers_file: PathBuf,
    factory: Arc<dyn AgentFactory>,
}

/// 单轮执行：惰性构建/复用 `AgentLoop`，接收 LLM 事件并在失败时回退 stub。
fn run_agent_turn(emitter: &TurnEmitter, turn: AgentTurn) {
    let AgentTurn {
        text,
        handle,
        root,
        providers_file,
        factory,
    } = turn;

    let mut agent_slot = handle
        .agent
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if agent_slot.is_none() {
        let request = AgentBuildRequest {
            session_id: &emitter.session_id,
            workspace_root: &root,
            providers_file: &providers_file,
            model: handle.model.as_deref(),
            thinking: handle.thinking.as_deref(),
            max_turns: handle.info_snapshot().max_turns,
            cancel: Arc::clone(&handle.cancel),
        };
        match factory.create(&request) {
            Ok(agent) => {
                let tools = emitter.clone();
                let compaction = emitter.clone();
                *agent_slot = Some(
                    agent
                        .with_cancel(Arc::clone(&handle.cancel))
                        .with_tool_observer(Box::new(move |call_id, result| {
                            tools.tool_result(call_id, result)
                        }))
                        .with_compaction_observer(Box::new(move |record| {
                            compaction.compacted(record)
                        })),
                );
            }
            Err(error) => {
                eprintln!(
                    "[ohos-agent] provider unavailable, falling back to stub: {}",
                    error.message
                );
                drop(agent_slot);
                run_stub_agent(&handle, emitter);
                return;
            }
        }
    }

    let agent = agent_slot.as_mut().expect("agent slot is populated above");
    let mut visible_output = false;
    let result = {
        let mut on_event = |event: LlmEvent| {
            if matches!(
                event,
                LlmEvent::TextDelta { .. } | LlmEvent::ToolUseStart { .. }
            ) {
                visible_output = true;
            }
            emitter.forward_llm(&event);
        };
        agent.run(&text, &mut on_event)
    };
    let session_after = session_info(agent.session());
    drop(agent_slot);

    if handle.cancel.swap(false, Ordering::AcqRel) {
        emitter.cancelled();
        return;
    }

    match result {
        Ok(outcome) => {
            emitter.session_state(session_after);
            let mut events = loop_outcome(&emitter.session_id, &outcome);
            let stop_reason = match &outcome {
                LoopOutcome::Final { .. } => None,
                LoopOutcome::NeedsApproval { .. } => Some("approval"),
                LoopOutcome::Denied { .. } => Some("denied"),
            };
            for event in events.drain(..) {
                emitter.event(event);
            }
            if let Some(stop_reason) = stop_reason {
                emitter.done(stop_reason);
            }
        }
        Err(error) => {
            // 传输/解析失败且尚未产生任何可见输出：回退 stub，保证离线 demo 可用。
            if matches!(error, LoopError::Llm(_)) && !visible_output {
                eprintln!("[ohos-agent] LLM transport failed, falling back to stub: {error}");
                run_stub_agent(&handle, emitter);
                return;
            }
            emitter.error(loop_error_body(&error));
            emitter.session_state(session_after);
            emitter.done("error");
        }
    }
}

fn loop_error_body(error: &LoopError) -> ErrorBody {
    match error {
        LoopError::Session(lc_core::agent::SessionError::Cancelled) => {
            ErrorBody::new(ErrorCode::Cancelled, "会话已取消")
        }
        LoopError::Session(error) => ErrorBody::new(ErrorCode::InvalidArgument, error.to_string()),
        LoopError::ContextOverflow { .. } => {
            ErrorBody::new(ErrorCode::InvalidArgument, error.to_string())
        }
        LoopError::UnknownTool(name) => {
            ErrorBody::new(ErrorCode::NotFound, format!("模型请求了未知工具：{name}"))
        }
        LoopError::Provider { code, message } => ErrorBody::new(*code, message.clone()),
        other => ErrorBody::new(ErrorCode::Internal, other.to_string()),
    }
}

/// 在 worker 闭包内持有，覆盖本轮全部事件（含 panic 收尾）；drop 后
/// `running = false`，回收侧即可把该会话视为已终止。
struct RunningGuard(SessionHandle);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
    }
}

/// 事件出口：注入会话 id、统一 seq，并在取消或会话被回收后抑制迟到事件。
#[derive(Clone)]
struct TurnEmitter {
    session_id: String,
    handle: SessionHandle,
    sink: Option<EventSink>,
    seq: Arc<AtomicU64>,
}

impl TurnEmitter {
    fn new(
        session_id: String,
        handle: SessionHandle,
        sink: Option<EventSink>,
        seq: Arc<AtomicU64>,
    ) -> Self {
        Self {
            session_id,
            handle,
            sink,
            seq,
        }
    }

    fn event(&self, event: CoreEvent) {
        // 会话已被回收：worker 可能仍持有句柄（例如 panic 后的收尾），所有
        // 后续推送都必须是 no-op，避免向已移除会话发事件。
        if !self.handle.alive.load(Ordering::Acquire) {
            return;
        }
        emit(&self.sink, &self.seq, event);
    }

    fn forward_llm(&self, event: &LlmEvent) {
        if self.handle.cancel.load(Ordering::Acquire) {
            return;
        }
        if let Some(event) = llm_event(&self.session_id, event) {
            self.event(event);
        }
    }

    fn tool_result(&self, call_id: &str, result: &Result<ToolOutput, ToolError>) {
        if self.handle.cancel.load(Ordering::Acquire) {
            return;
        }
        let (ok, text) = match result {
            Ok(output) => (true, output.text.clone()),
            Err(error) => (false, error.to_string()),
        };
        self.event(CoreEvent::AgentToolResult {
            session_id: self.session_id.clone(),
            call_id: call_id.to_owned(),
            ok,
            text,
        });
    }

    fn compacted(&self, record: &CompactionRecord) {
        if self.handle.cancel.load(Ordering::Acquire) {
            return;
        }
        self.event(CoreEvent::AgentCompacted {
            session_id: self.session_id.clone(),
            record_id: record.id,
            before_tokens: record.before_tokens,
            after_tokens: record.after_tokens,
            removed_messages: record.removed_messages,
            summary: record.summary.clone(),
        });
    }

    fn session_state(&self, mut info: SessionInfo) {
        if !self.handle.alive.load(Ordering::Acquire) {
            return;
        }
        info.session_id = self.session_id.clone();
        info.mode = SessionMode::Plan;
        {
            let mut stored = self
                .handle
                .info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *stored = info.clone();
        }
        self.event(CoreEvent::AgentSessionState { session: info });
    }

    fn cancelled(&self) {
        if !self.handle.alive.load(Ordering::Acquire) {
            return;
        }
        let mut info = self.handle.info_snapshot();
        info.session_id = self.session_id.clone();
        info.mode = SessionMode::Plan;
        info.cancelled = true;
        {
            let mut stored = self
                .handle
                .info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *stored = info.clone();
        }
        self.event(CoreEvent::AgentSessionState { session: info });
        self.done(CANCELLED_STOP_REASON);
    }

    fn error(&self, error: ErrorBody) {
        self.event(CoreEvent::Error { error });
    }

    fn done(&self, stop_reason: &str) {
        self.event(CoreEvent::AgentDone {
            session_id: self.session_id.clone(),
            stop_reason: stop_reason.to_owned(),
        });
    }
}

fn run_stub_agent(handle: &SessionHandle, emitter: &TurnEmitter) {
    for index in 0..STUB_DELTAS {
        if handle.cancel.load(Ordering::Acquire) {
            finish_cancelled(handle, emitter);
            return;
        }
        emitter.event(CoreEvent::AgentTextDelta {
            session_id: emitter.session_id.clone(),
            index,
            text: format!("[stub] delta {index}\n"),
        });
        thread::sleep(STUB_DELAY);
    }
    if handle.cancel.load(Ordering::Acquire) {
        finish_cancelled(handle, emitter);
        return;
    }
    emitter.session_state(handle.info_snapshot());
    emitter.done(STUB_STOP_REASON);
}

fn finish_cancelled(handle: &SessionHandle, emitter: &TurnEmitter) {
    handle.cancel.store(false, Ordering::Release);
    emitter.cancelled();
}

fn emit(sink: &Option<EventSink>, seq: &AtomicU64, event: CoreEvent) {
    let Some(sink) = sink else { return };
    let envelope = EventEnvelope {
        seq: seq.fetch_add(1, Ordering::SeqCst) + 1,
        event,
    };
    if let Ok(json) = serde_json::to_string(&envelope) {
        sink(json);
    }
}

fn parse_args(args_json: &str) -> Result<Value, ErrorBody> {
    if args_json.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let value: Value = serde_json::from_str(args_json)
        .map_err(|error| invalid(format!("invalid args json: {error}")))?;
    if !value.is_object() {
        return Err(invalid("args must be a JSON object"));
    }
    Ok(value)
}

fn required_string(args: &Value, key: &str) -> Result<String, ErrorBody> {
    args.get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(format!("missing string argument `{key}`")))
}

fn optional_string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn invalid(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::InvalidArgument, message)
}

fn io_body(path: &Path, error: std::io::Error) -> ErrorBody {
    let code = if error.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::NotFound
    } else {
        ErrorCode::Io
    };
    ErrorBody::new(code, format!("io error on {}: {error}", display_path(path)))
}

fn unsupported(cmd: &str) -> ErrorBody {
    ErrorBody::new(
        ErrorCode::NotImplemented,
        format!("unsupported command on ohos shell: {cmd}"),
    )
    .with_detail(
        "supported: workspace_root, fs_read_dir, fs_read_file, session_create, agent_send, agent_cancel, providers_load, providers_save, history_list, history_save, history_load, history_delete",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    use lc_core::agent::{LlmStream, PolicyMode};
    use lc_core::llm::{LlmError, LlmRequest, StopReason};
    use serde_json::json;

    use super::*;

    fn temp_base() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "lc-ohos-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn setup_with_factory(factory: Option<Arc<dyn AgentFactory>>) -> (Host, PathBuf) {
        setup_with_limit(factory, MAX_SESSIONS)
    }

    fn setup_with_limit(
        factory: Option<Arc<dyn AgentFactory>>,
        max_sessions: usize,
    ) -> (Host, PathBuf) {
        let base = temp_base();
        let root = base.join("workspace");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(base.join("outside.txt"), "secret\n").unwrap();
        let config = HostConfig {
            files_dir: base.join("files"),
            workspace_root: root,
            providers_file: base.join("files/providers.json"),
        };
        let factory = factory.unwrap_or_else(|| Arc::new(RealAgentFactory));
        let host = Host::init_with_factory_and_limit(config, factory, max_sessions).unwrap();
        (host, base)
    }

    fn setup() -> (Host, PathBuf) {
        setup_with_factory(None)
    }

    fn parse(raw: &str) -> Value {
        serde_json::from_str(raw).unwrap()
    }

    fn collect_events(host: &Host) -> Arc<Mutex<Vec<Value>>> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        host.set_event_sink(Arc::new(move |json| {
            sink_events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(serde_json::from_str(&json).unwrap());
        }));
        events
    }

    fn wait_for(
        events: &Arc<Mutex<Vec<Value>>>,
        predicate: impl Fn(&[Value]) -> bool,
    ) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if predicate(&snapshot) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "timeout waiting for events: {snapshot:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn create_session(host: &Host) -> String {
        create_session_with_model(host, None)
    }

    fn create_session_with_model(host: &Host, model: Option<&str>) -> String {
        let value = parse(
            &host
                .dispatch(
                    "session_create",
                    &json!({"mode": "plan", "model": model, "thinking": null}).to_string(),
                )
                .unwrap(),
        );
        assert_eq!(value["kind"], "session");
        assert_eq!(value["session"]["mode"], "plan");
        value["session"]["session_id"].as_str().unwrap().to_owned()
    }

    struct FakeLlm {
        turns: Mutex<VecDeque<Vec<LlmEvent>>>,
        requests: Arc<Mutex<Vec<LlmRequest>>>,
    }

    impl LlmStream for FakeLlm {
        fn stream(
            &self,
            request: &LlmRequest,
            sink: &mut dyn FnMut(LlmEvent),
        ) -> Result<(), LlmError> {
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(request.clone());
            let events = self
                .turns
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_default();
            for event in events {
                sink(event);
            }
            Ok(())
        }
    }

    struct FakeFactory {
        turns: Mutex<VecDeque<Vec<LlmEvent>>>,
        requests: Arc<Mutex<Vec<LlmRequest>>>,
    }

    impl FakeFactory {
        fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
            Self {
                turns: Mutex::new(turns.into()),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Arc<Mutex<Vec<LlmRequest>>> {
            Arc::clone(&self.requests)
        }
    }

    impl AgentFactory for FakeFactory {
        fn create(&self, request: &AgentBuildRequest<'_>) -> Result<AgentLoop, ErrorBody> {
            let turns = std::mem::take(
                &mut *self
                    .turns
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
            Ok(AgentLoop::new(
                Session::new(request.session_id, PolicyMode::Plan)
                    .with_max_turns(request.max_turns)
                    .with_context(100, 80),
                Box::new(FakeLlm {
                    turns: Mutex::new(turns),
                    requests: Arc::clone(&self.requests),
                }),
                Box::new(LocalToolExecutor::new(request.workspace_root)),
                read_only_registry(),
                request.workspace_root,
                "fake-model",
            )
            .with_tools(read_only_registry().llm_tools()))
        }
    }

    fn read_turn(ids: &[&str], path: &str) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        for id in ids {
            events.push(LlmEvent::ToolUseStart {
                id: (*id).into(),
                name: "read_file".into(),
            });
            events.push(LlmEvent::ToolUseArgsDelta {
                id: (*id).into(),
                json: json!({ "path": path }).to_string(),
            });
        }
        events.push(LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
        });
        events
    }

    fn final_turn(text: &str) -> Vec<LlmEvent> {
        vec![
            LlmEvent::TextDelta {
                index: 0,
                text: text.into(),
            },
            LlmEvent::Usage {
                input: 42,
                output: 7,
                cached: 3,
            },
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
        ]
    }

    #[test]
    fn fs_commands_return_contract_shapes() {
        let (host, base) = setup();
        let root = parse(&host.dispatch("workspace_root", "{}").unwrap());
        assert_eq!(root["kind"], "workspace_root");
        let root_path = root["root"].as_str().unwrap().to_owned();
        assert!(Path::new(&root_path).is_dir());

        let listing = parse(
            &host
                .dispatch("fs_read_dir", &json!({"path": "src"}).to_string())
                .unwrap(),
        );
        assert_eq!(listing["kind"], "dir_listing");
        assert_eq!(
            listing["path"],
            Path::new(&root_path).join("src").to_string_lossy().as_ref()
        );
        assert_eq!(listing["generation"], 0);
        let entries = listing["entries"].as_array().unwrap();
        let main = entries
            .iter()
            .find(|entry| entry["name"] == "main.rs")
            .expect("src/main.rs should be listed");
        assert_eq!(main["kind"], "file");
        assert_eq!(
            main["path"],
            Path::new(&root_path)
                .join("src")
                .join("main.rs")
                .to_string_lossy()
                .as_ref()
        );

        let file = parse(
            &host
                .dispatch("fs_read_file", &json!({"path": main["path"]}).to_string())
                .unwrap(),
        );
        assert_eq!(file["kind"], "file_content");
        assert_eq!(file["text"], "fn main() {}\n");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn path_traversal_is_rejected() {
        let (host, base) = setup();
        let outside = base.join("outside.txt");

        let error = host
            .dispatch("fs_read_file", &json!({"path": outside}).to_string())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PathOutsideRoot);

        let error = host
            .dispatch(
                "fs_read_file",
                &json!({"path": "../outside.txt"}).to_string(),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PathOutsideRoot);

        let error = host
            .dispatch("fs_read_dir", &json!({"path": ".."}).to_string())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PathOutsideRoot);

        let _ = fs::remove_dir_all(&base);
    }

    fn stub_save_input() -> Value {
        json!({
            "input": {
                "providers": [{
                    "id": "local-stub",
                    "kind": "openai-responses",
                    "label": "Local Stub",
                    "baseUrl": "http://10.0.2.2:8765/v1",
                    "enabled": true,
                    "apiKeyRef": "env:LC_STUB_API_KEY"
                }],
                "models": [{
                    "id": "stub-chat",
                    "provider": "local-stub",
                    "modelId": "stub-chat",
                    "thinking": ["off"],
                    "contextK": 100,
                    "compressAtK": 80,
                    "capabilities": []
                }],
                "defaultModel": "stub-chat",
                "defaultThinking": "off",
                "subagentModel": null,
                "subagentThinking": null
            }
        })
    }

    #[test]
    fn session_create_uses_model_budget() {
        let (host, base) = setup();
        host.dispatch("providers_save", &stub_save_input().to_string())
            .unwrap();

        let with_model = parse(
            &host
                .dispatch("session_create", r#"{"mode":"plan","model":"stub-chat"}"#)
                .unwrap(),
        );
        assert_eq!(with_model["session"]["context_k"], 100);
        assert_eq!(with_model["session"]["compress_at_k"], 80);

        let without_model = parse(&host.dispatch("session_create", "{}").unwrap());
        assert_eq!(without_model["session"]["context_k"], 400);
        assert_eq!(without_model["session"]["compress_at_k"], 300);

        let unknown = parse(
            &host
                .dispatch("session_create", r#"{"model":"no-such-model"}"#)
                .unwrap(),
        );
        assert_eq!(unknown["session"]["context_k"], 400);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn providers_load_missing_file_returns_empty_view() {
        let (host, base) = setup();

        let view = parse(&host.dispatch("providers_load", "{}").unwrap());
        assert_eq!(view["providers"], json!([]));
        assert_eq!(view["models"], json!([]));
        assert!(view["defaultModel"].is_null());
        assert!(view["defaultThinking"].is_null());
        assert!(view["subagentModel"].is_null());
        assert!(view["subagentThinking"].is_null());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn providers_load_corrupt_json_returns_empty_view() {
        let (host, base) = setup();
        let providers = base.join("files/providers.json");
        fs::write(&providers, "{ this is not json").unwrap();

        let view = parse(&host.dispatch("providers_load", "{}").unwrap());
        assert_eq!(view["providers"], json!([]));
        assert_eq!(view["models"], json!([]));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn providers_save_round_trips_through_load() {
        let (host, base) = setup();
        let providers = base.join("files/providers.json");

        let saved = parse(
            &host
                .dispatch("providers_save", &stub_save_input().to_string())
                .unwrap(),
        );
        assert_eq!(saved["providers"][0]["id"], "local-stub");
        assert_eq!(saved["providers"][0]["hasApiKey"], false);
        assert!(saved["providers"][0]["apiKeyHint"].is_null());
        assert_eq!(saved["providers"][0]["apiKeyRef"], "env:LC_STUB_API_KEY");
        assert_eq!(saved["models"][0]["id"], "stub-chat");
        assert_eq!(saved["models"][0]["contextK"], 100);
        assert_eq!(saved["defaultModel"], "stub-chat");
        assert_eq!(saved["defaultThinking"], "off");

        let disk: Value = serde_json::from_str(&fs::read_to_string(&providers).unwrap()).unwrap();
        assert_eq!(disk["providers"][0]["baseUrl"], "http://10.0.2.2:8765/v1");
        assert!(disk["providers"][0].get("apiKey").is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&providers).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "providers.json must be private");
        }

        let loaded = parse(&host.dispatch("providers_load", "{}").unwrap());
        assert_eq!(loaded, saved);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn providers_save_rejects_invalid_input() {
        let (host, base) = setup();
        let providers = base.join("files/providers.json");

        let error = host.dispatch("providers_save", "{}").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("input"));

        let mut empty_models = stub_save_input();
        empty_models["input"]["models"] = json!([]);
        let error = host
            .dispatch("providers_save", &empty_models.to_string())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("models"));

        let mut unknown_provider = stub_save_input();
        unknown_provider["input"]["models"][0]["provider"] = json!("missing");
        let error = host
            .dispatch("providers_save", &unknown_provider.to_string())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("missing"));

        let mut bad_budget = stub_save_input();
        bad_budget["input"]["models"][0]["compressAtK"] = json!(100);
        let error = host
            .dispatch("providers_save", &bad_budget.to_string())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("compressAtK"));

        assert!(!providers.exists(), "rejected input must not be written");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn error_body_shape_is_stable() {
        let (host, base) = setup();

        let error = host.dispatch("git_status", "{}").unwrap_err();
        assert_eq!(error.code, ErrorCode::NotImplemented);
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(value["code"], "not_implemented");
        assert!(value["message"].as_str().unwrap().contains("unsupported"));
        assert!(value["detail"].as_str().unwrap().contains("workspace_root"));

        let error = host.dispatch("fs_read_dir", "{}").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("path"));

        let error = host.dispatch("fs_read_dir", "not json").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn stub_agent_emits_twenty_deltas_then_done() {
        let (host, base) = setup();
        let events = collect_events(&host);
        let session_id = create_session(&host);

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "hello"}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let deltas: Vec<&Value> = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .collect();
        assert_eq!(deltas.len(), 20);
        for (index, delta) in deltas.iter().enumerate() {
            assert_eq!(
                delta["event"]["session_id"].as_str(),
                Some(session_id.as_str())
            );
            assert_eq!(delta["event"]["index"], index as u64);
        }

        let seqs: Vec<u64> = snapshot
            .iter()
            .map(|event| event["seq"].as_u64().unwrap())
            .collect();
        assert!(seqs.windows(2).all(|pair| pair[0] + 1 == pair[1]));

        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["type"], "agent:done");
        assert_eq!(done["event"]["stop_reason"], "stub");
        assert!(
            snapshot
                .iter()
                .any(|event| event["event"]["type"] == "agent:session_state"),
            "stub fallback must still report session state"
        );
        assert!(
            snapshot
                .iter()
                .all(|event| event["event"]["type"] != "error"),
            "missing provider is a fallback, not an error event"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn agent_cancel_interrupts_with_cancelled_events() {
        let (host, base) = setup();
        let events = collect_events(&host);
        let session_id = create_session(&host);

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "hello"}).to_string(),
        )
        .unwrap();
        wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:text_delta")
        });

        host.dispatch(
            "agent_cancel",
            &json!({"session_id": session_id}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let cancelled = snapshot
            .iter()
            .find(|event| event["event"]["type"] == "agent:session_state")
            .expect("cancel should emit session_state");
        assert_eq!(cancelled["event"]["session"]["cancelled"], true);

        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["type"], "agent:done");
        assert_eq!(done["event"]["stop_reason"], "cancelled");

        let deltas = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .count();
        assert!(deltas < 20, "cancel should interrupt before all deltas");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_runtime_without_env_key_uses_placeholder() {
        let base = temp_base();
        let path = base.join("providers.json");
        fs::write(
            &path,
            r#"{
              "providers": [{"id":"local-stub","kind":"openai-responses","label":"Local Stub",
                             "baseUrl":"http://10.0.2.2:8765/v1","apiKeyRef":"env:LC_STUB_API_KEY","enabled":true}],
              "models": [{"id":"stub-chat","provider":"local-stub","modelId":"stub-chat",
                          "thinking":["off"],"contextK":100,"compressAtK":80,"capabilities":[]}],
              "defaultModel": "stub-chat"
            }"#,
        )
        .unwrap();

        let runtime = resolve_runtime(&path, Some("stub-chat"), |_| None).expect("runtime");
        assert_eq!(runtime.base_url, "http://10.0.2.2:8765/v1");
        assert_eq!(runtime.model_id, "stub-chat");
        assert_eq!(runtime.api_key, PLACEHOLDER_API_KEY);
        assert_eq!(runtime.context_k, 100);
        assert_eq!(runtime.compress_at_k, 80);

        let missing = base.join("missing.json");
        assert_eq!(
            resolve_runtime(&missing, None, |_| None).err(),
            Some(RuntimeError::NoModel)
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn unreachable_provider_falls_back_to_stub_within_budget() {
        let (host, base) = setup();
        host.dispatch("providers_save", &stub_save_input().to_string())
            .unwrap();
        let events = collect_events(&host);
        let session_id = create_session_with_model(&host, Some("stub-chat"));
        save_unreachable_provider(&host);

        let started = Instant::now();
        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "read src/main.rs"}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let elapsed = started.elapsed();
        let deltas: Vec<&Value> = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .collect();
        assert_eq!(deltas.len(), STUB_DELTAS as usize);
        assert!(deltas.iter().all(|delta| {
            delta["event"]["text"]
                .as_str()
                .unwrap()
                .starts_with("[stub]")
        }));
        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["stop_reason"], "stub");
        assert!(
            snapshot
                .iter()
                .all(|event| event["event"]["type"] != "error"),
            "probe failure is a fallback, not an error event"
        );
        // 预探 800ms + 20×25ms stub 约 1.3s；2.5s 给 CI 留余量，同时确保
        // 不会再等 ureq 的 5s 连接超时。
        assert!(
            elapsed < Duration::from_millis(2500),
            "stub fallback took {elapsed:?}, expected < 2.5s"
        );

        let _ = fs::remove_dir_all(&base);
    }

    fn save_unreachable_provider(host: &Host) {
        let mut input = stub_save_input();
        input["input"]["providers"][0]["baseUrl"] = json!("http://127.0.0.1:1/v1");
        input["input"]["providers"][0]["apiKey"] = json!("sk-test");
        host.dispatch("providers_save", &input.to_string()).unwrap();
    }

    #[test]
    fn probe_target_parses_http_urls_and_skips_unknown_schemes() {
        assert_eq!(
            probe_target("http://example.test:8765/v1"),
            Some(("example.test".into(), 8765))
        );
        assert_eq!(
            probe_target("https://api.openai.com/v1"),
            Some(("api.openai.com".into(), 443))
        );
        assert_eq!(
            probe_target("http://127.0.0.1/v1?x=1"),
            Some(("127.0.0.1".into(), 80))
        );
        assert_eq!(
            probe_target(" https://[::1]:8443/v1 "),
            Some(("::1".into(), 8443))
        );
        assert_eq!(probe_target("http:///v1"), None);
        assert_eq!(probe_target("http://example.test:bad/v1"), None);
        assert_eq!(probe_target("ftp://example.test/v1"), None);
        assert_eq!(probe_target("not a url"), None);
    }

    #[test]
    fn tcp_probe_detects_reachable_and_refused_ports() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(
            unreachable_target(&format!("http://127.0.0.1:{port}/v1")),
            None,
            "listening port must be treated as reachable"
        );
        drop(listener);

        let started = Instant::now();
        assert_eq!(
            unreachable_target(&format!("http://127.0.0.1:{port}/v1")),
            Some(("127.0.0.1".into(), port)),
            "closed port must be treated as unreachable"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// 极小 HTTP mock：忽略 TCP 预探连接（只连不发），对真正的 POST 返回固定状态。
    fn spawn_http_status_mock(status: &'static str) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        if !read_http_request(&mut stream) {
                            continue;
                        }
                        let response =
                            format!("{status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        let _ = stream.write_all(response.as_bytes());
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        (port, handle)
    }

    fn read_http_request(stream: &mut TcpStream) -> bool {
        let mut data = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => return false,
                Ok(read) => {
                    data.extend_from_slice(&buf[..read]);
                    let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&data[..end]);
                    let content_length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let received = data.len().saturating_sub(end + 4);
                    if received < content_length {
                        let mut rest = vec![0u8; content_length - received];
                        return stream.read_exact(&mut rest).is_ok();
                    }
                    return true;
                }
                Err(_) => return false,
            }
        }
    }

    #[test]
    fn reachable_provider_with_http_401_still_falls_back_to_stub() {
        let (port, server) = spawn_http_status_mock("HTTP/1.1 401 Unauthorized");
        let (host, base) = setup();
        let mut input = stub_save_input();
        input["input"]["providers"][0]["baseUrl"] = json!(format!("http://127.0.0.1:{port}/v1"));
        input["input"]["providers"][0]["apiKey"] = json!("sk-test");
        host.dispatch("providers_save", &input.to_string()).unwrap();
        let events = collect_events(&host);
        let session_id = create_session_with_model(&host, Some("stub-chat"));

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "hello"}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let deltas: Vec<&Value> = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .collect();
        assert_eq!(deltas.len(), STUB_DELTAS as usize);
        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["stop_reason"], "stub");
        assert!(
            snapshot
                .iter()
                .all(|event| event["event"]["type"] != "error"),
            "HTTP 401 before any visible output is a fallback, not an error event"
        );
        server.join().unwrap();

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn fake_agent_runs_plan_tool_call_and_streams_events() {
        let factory = Arc::new(FakeFactory::new(vec![
            read_turn(&["call_1"], "src/main.rs"),
            final_turn("main.rs 只有一个空的 main 函数。"),
        ]));
        let requests = factory.requests();
        let (host, base) = setup_with_factory(Some(factory));
        let events = collect_events(&host);
        let session_id = create_session_with_model(&host, Some("fake-model"));

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "读取 src/main.rs 并解释"}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });

        let types: Vec<&str> = snapshot
            .iter()
            .map(|event| event["event"]["type"].as_str().unwrap())
            .collect();
        let tool_start = types
            .iter()
            .position(|kind| *kind == "agent:tool_start")
            .expect("tool_start");
        let tool_result = types
            .iter()
            .position(|kind| *kind == "agent:tool_result")
            .expect("tool_result");
        let text = types
            .iter()
            .position(|kind| *kind == "agent:text_delta")
            .expect("text_delta");
        assert!(tool_start < tool_result && tool_result < text, "{types:?}");

        let start = &snapshot[tool_start]["event"];
        assert_eq!(start["name"], "read_file");
        assert_eq!(start["session_id"], session_id);
        let result = &snapshot[tool_result]["event"];
        assert_eq!(result["ok"], true);
        assert!(result["text"].as_str().unwrap().contains("fn main() {}"));

        let text: String = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .filter_map(|event| event["event"]["text"].as_str())
            .collect();
        assert_eq!(text, "main.rs 只有一个空的 main 函数。");
        assert!(
            snapshot
                .iter()
                .any(|event| event["event"]["type"] == "agent:usage"
                    && event["event"]["input"] == 42
                    && event["event"]["cached"] == 3)
        );

        let state = snapshot
            .iter()
            .rev()
            .find(|event| event["event"]["type"] == "agent:session_state")
            .expect("session_state");
        // turn 计的是 LLM 迭代次数：工具轮 + 最终回答轮。
        assert_eq!(state["event"]["session"]["turn"], 2);
        assert_eq!(state["event"]["session"]["mode"], "plan");
        assert_eq!(state["event"]["session"]["context_k"], 100);

        let done_count = types.iter().filter(|kind| **kind == "agent:done").count();
        assert_eq!(
            done_count, 1,
            "one user turn must emit exactly one done: {types:?}"
        );
        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["type"], "agent:done");
        assert_eq!(done["event"]["stop_reason"], "end_turn");

        let seqs: Vec<u64> = snapshot
            .iter()
            .map(|event| event["seq"].as_u64().unwrap())
            .collect();
        assert!(seqs.windows(2).all(|pair| pair[0] + 1 == pair[1]));
        assert!(
            snapshot
                .iter()
                .all(|event| event["event"]["type"] != "error"),
            "plan read tools must not error: {snapshot:?}"
        );

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].input.iter().any(|input| matches!(
            input,
            lc_core::llm::LlmInput::FunctionCallOutput { output, .. }
                if output.contains("fn main() {}")
        )));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn fake_agent_rejects_write_tool_in_read_only_registry() {
        let factory = Arc::new(FakeFactory::new(vec![vec![
            LlmEvent::ToolUseStart {
                id: "write_1".into(),
                name: "write_file".into(),
            },
            LlmEvent::ToolUseArgsDelta {
                id: "write_1".into(),
                json: json!({"path": "src/main.rs", "content": "x"}).to_string(),
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
            },
        ]]));
        let (host, base) = setup_with_factory(Some(factory));
        let events = collect_events(&host);
        let session_id = create_session_with_model(&host, Some("fake-model"));

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "覆盖 main.rs"}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let error = snapshot
            .iter()
            .find(|event| event["event"]["type"] == "error")
            .expect("write tool should be rejected");
        // 只读注册表不含 write_file，模型即使幻觉出该调用也只会得到未知工具错误。
        assert_eq!(error["event"]["error"]["code"], "not_found");
        assert!(
            error["event"]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("write_file")
        );
        assert!(
            snapshot
                .iter()
                .all(|event| event["event"]["type"] != "agent:tool_result"),
            "denied tool must not execute"
        );
        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["stop_reason"], "error");
        assert_eq!(
            fs::read_to_string(base.join("workspace/src/main.rs")).unwrap(),
            "fn main() {}\n",
            "plan mode must not modify the file"
        );

        let _ = fs::remove_dir_all(&base);
    }

    struct SlowLlm {
        cancel: Arc<AtomicBool>,
    }

    impl LlmStream for SlowLlm {
        fn stream(
            &self,
            _request: &LlmRequest,
            sink: &mut dyn FnMut(LlmEvent),
        ) -> Result<(), LlmError> {
            for index in 0..STUB_DELTAS {
                if self.cancel.load(Ordering::Acquire) {
                    break;
                }
                sink(LlmEvent::TextDelta {
                    index,
                    text: format!("real {index} "),
                });
                thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        }
    }

    struct SlowFactory {
        cancel: Arc<AtomicBool>,
    }

    impl AgentFactory for SlowFactory {
        fn create(&self, request: &AgentBuildRequest<'_>) -> Result<AgentLoop, ErrorBody> {
            Ok(AgentLoop::new(
                Session::new(request.session_id, PolicyMode::Plan),
                Box::new(SlowLlm {
                    cancel: Arc::clone(&self.cancel),
                }),
                Box::new(LocalToolExecutor::new(request.workspace_root)),
                read_only_registry(),
                request.workspace_root,
                "fake-model",
            )
            .with_tools(read_only_registry().llm_tools()))
        }
    }

    struct PartialErrorLlm;

    impl LlmStream for PartialErrorLlm {
        fn stream(
            &self,
            _request: &LlmRequest,
            sink: &mut dyn FnMut(LlmEvent),
        ) -> Result<(), LlmError> {
            sink(LlmEvent::TextDelta {
                index: 0,
                text: "partial answer".into(),
            });
            Err(LlmError::Http("connection reset".into()))
        }
    }

    struct PartialErrorFactory;

    impl AgentFactory for PartialErrorFactory {
        fn create(&self, request: &AgentBuildRequest<'_>) -> Result<AgentLoop, ErrorBody> {
            Ok(AgentLoop::new(
                Session::new(request.session_id, PolicyMode::Plan),
                Box::new(PartialErrorLlm),
                Box::new(LocalToolExecutor::new(request.workspace_root)),
                read_only_registry(),
                request.workspace_root,
                "fake-model",
            )
            .with_tools(read_only_registry().llm_tools()))
        }
    }

    #[test]
    fn agent_cancel_interrupts_real_loop() {
        let cancel = Arc::new(AtomicBool::new(false));
        let factory = Arc::new(SlowFactory {
            cancel: Arc::clone(&cancel),
        });
        let (host, base) = setup_with_factory(Some(factory));
        let events = collect_events(&host);
        let session_id = create_session_with_model(&host, Some("fake-model"));

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "go"}).to_string(),
        )
        .unwrap();
        wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:text_delta")
        });

        host.dispatch(
            "agent_cancel",
            &json!({"session_id": session_id}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let state = snapshot
            .iter()
            .rev()
            .find(|event| event["event"]["type"] == "agent:session_state")
            .expect("cancelled session state");
        assert_eq!(state["event"]["session"]["cancelled"], true);
        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["stop_reason"], "cancelled");
        let deltas = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .count();
        assert!(
            deltas < STUB_DELTAS as usize,
            "cancel should stop the stream"
        );
        assert!(
            snapshot
                .iter()
                .all(|event| event["event"]["type"] != "agent:done"
                    || event["event"]["stop_reason"] == "cancelled")
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn transport_error_after_visible_output_is_not_masked_as_stub() {
        let (host, base) = setup_with_factory(Some(Arc::new(PartialErrorFactory)));
        let events = collect_events(&host);
        let session_id = create_session_with_model(&host, Some("fake-model"));

        host.dispatch(
            "agent_send",
            &json!({"session_id": session_id, "text": "go"}).to_string(),
        )
        .unwrap();

        let snapshot = wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        let deltas: Vec<&Value> = snapshot
            .iter()
            .filter(|event| event["event"]["type"] == "agent:text_delta")
            .collect();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["event"]["text"], "partial answer");
        assert!(
            snapshot
                .iter()
                .any(|event| event["event"]["type"] == "error"),
            "transport error after visible output must surface as an error"
        );
        let done = snapshot.last().unwrap();
        assert_eq!(done["event"]["type"], "agent:done");
        assert_eq!(done["event"]["stop_reason"], "error");

        let _ = fs::remove_dir_all(&base);
    }

    fn wait_until(mut predicate: impl FnMut() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(Instant::now() < deadline, "timeout waiting for {what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// 阻塞到测试显式放行；用于让会话在整个回收断言期间保持 running。
    struct GateLlm {
        release: Arc<AtomicBool>,
    }

    impl LlmStream for GateLlm {
        fn stream(
            &self,
            _request: &LlmRequest,
            sink: &mut dyn FnMut(LlmEvent),
        ) -> Result<(), LlmError> {
            while !self.release.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            sink(LlmEvent::TextDelta {
                index: 0,
                text: "released".into(),
            });
            sink(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
            });
            Ok(())
        }
    }

    struct GateFactory {
        release: Arc<AtomicBool>,
    }

    impl AgentFactory for GateFactory {
        fn create(&self, request: &AgentBuildRequest<'_>) -> Result<AgentLoop, ErrorBody> {
            Ok(AgentLoop::new(
                Session::new(request.session_id, PolicyMode::Plan)
                    .with_max_turns(request.max_turns)
                    .with_context(100, 80),
                Box::new(GateLlm {
                    release: Arc::clone(&self.release),
                }),
                Box::new(LocalToolExecutor::new(request.workspace_root)),
                read_only_registry(),
                request.workspace_root,
                "fake-model",
            )
            .with_tools(read_only_registry().llm_tools()))
        }
    }

    #[test]
    fn session_reclaim_evicts_oldest_terminated_and_keeps_running() {
        const LIMIT: usize = 3;
        let release = Arc::new(AtomicBool::new(false));
        let factory = Arc::new(GateFactory {
            release: Arc::clone(&release),
        });
        let (host, base) = setup_with_limit(Some(factory), LIMIT);
        let events = collect_events(&host);

        let running = create_session_with_model(&host, Some("fake-model"));
        host.dispatch(
            "agent_send",
            &json!({"session_id": running, "text": "hold"}).to_string(),
        )
        .unwrap();
        wait_until(
            || {
                host.session(&running)
                    .unwrap()
                    .running
                    .load(Ordering::Acquire)
            },
            "session to start running",
        );

        // running(1) + LIMIT(3) = 4 > 3：最老的已终止会话被回收。
        let terminated: Vec<String> = (0..LIMIT).map(|_| create_session(&host)).collect();
        assert_eq!(host.sessions_slot().len(), LIMIT);
        assert!(
            host.session(&running).is_ok(),
            "running session must survive reclamation"
        );
        assert!(
            host.session(&terminated[0]).is_err(),
            "oldest terminated session must be reclaimed"
        );
        assert!(host.session(&terminated[1]).is_ok());

        // 后续创建继续按创建顺序回收。
        let newest = create_session(&host);
        assert!(
            host.session(&terminated[1]).is_err(),
            "reclamation follows creation order"
        );
        assert!(host.session(&terminated[2]).is_ok());
        assert!(host.session(&newest).is_ok());
        assert_eq!(host.sessions_slot().len(), LIMIT);

        // worker 结束后 running 会话才变成可回收，且因为最老而先被回收。
        release.store(true, Ordering::Release);
        wait_for(&events, |events| {
            events
                .iter()
                .any(|event| event["event"]["type"] == "agent:done")
        });
        wait_until(
            || {
                !host
                    .session(&running)
                    .unwrap()
                    .running
                    .load(Ordering::Acquire)
            },
            "running flag to clear",
        );
        create_session(&host);
        assert!(
            host.session(&running).is_err(),
            "finished session becomes reclaimable"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn reclaimed_session_ignores_late_worker_events() {
        const LIMIT: usize = 1;
        let (host, base) = setup_with_limit(None, LIMIT);
        let first = create_session(&host);
        let stale = host.session(&first).unwrap();

        let late = Arc::new(Mutex::new(Vec::new()));
        let late_sink = Arc::clone(&late);
        let emitter = TurnEmitter::new(
            first.clone(),
            stale.clone(),
            Some(Arc::new(move |json| {
                late_sink
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(serde_json::from_str::<Value>(&json).unwrap());
            })),
            Arc::clone(&host.seq),
        );
        emitter.done("stub");
        assert_eq!(
            late.lock().unwrap().len(),
            1,
            "emitter must work before reclamation"
        );

        let second = create_session(&host);
        assert!(host.session(&first).is_err());
        assert!(host.session(&second).is_ok());

        // 模拟持有旧句柄的 worker：会话回收后继续推送必须是 no-op。
        emitter.done("stub");
        emitter.error(ErrorBody::new(ErrorCode::Internal, "late"));
        emitter.session_state(stale.info_snapshot());
        assert_eq!(
            late.lock().unwrap().len(),
            1,
            "events for a reclaimed session must be dropped"
        );

        let error = host
            .dispatch(
                "agent_send",
                &json!({"session_id": first, "text": "late"}).to_string(),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn slow_dns_resolution_is_bounded_by_probe_budget() {
        let started = Instant::now();
        let resolved = resolve_bounded("slow.invalid", 80, Duration::from_millis(100), |_, _| {
            thread::sleep(Duration::from_secs(2));
            Ok(vec![SocketAddr::from(([127, 0, 0, 1], 80))])
        });
        assert!(resolved.is_none(), "timed-out resolution must be None");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "DNS resolution must honor the probe budget, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn ip_literals_bypass_the_resolver_thread() {
        let started = Instant::now();
        let resolved = resolve_bounded("127.0.0.1", 8080, Duration::from_millis(50), |_, _| {
            thread::sleep(Duration::from_secs(2));
            Ok(Vec::new())
        });
        assert_eq!(
            resolved,
            Some(vec![SocketAddr::from(([127, 0, 0, 1], 8080))])
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "IP literal must skip DNS"
        );
    }

    #[test]
    fn history_commands_roundtrip_through_dispatch() {
        let (host, base) = setup();
        let payload = json!({
            "id": "ohos-1",
            "title": "修复登录",
            "savedAt": 123,
            "workspace": "/work/project",
            "mode": "plan",
            "model": "gpt-5",
            "turnStats": [],
            "agent": {
                "turn": 2,
                "messages": [
                    {"role": "user", "text": "请修复登录问题"},
                    {"role": "assistant", "text": "我先看看代码"}
                ]
            }
        });
        let saved = host
            .dispatch(
                "history_save",
                &json!({"id": "ohos-1", "payload": payload}).to_string(),
            )
            .unwrap();
        assert_eq!(saved, "null");

        let listed = parse(&host.dispatch("history_list", "{}").unwrap());
        assert_eq!(listed[0]["id"], "ohos-1");
        assert_eq!(listed[0]["title"], "修复登录");
        assert_eq!(listed[0]["savedAt"], 123);
        assert_eq!(listed[0]["turn"], 2);
        assert_eq!(listed[0]["messages"], 2);
        assert_eq!(listed[0]["excerpt"], "请修复登录问题\n我先看看代码");

        let loaded = parse(
            &host
                .dispatch("history_load", &json!({"id": "ohos-1"}).to_string())
                .unwrap(),
        );
        assert_eq!(loaded["title"], "修复登录");

        assert_eq!(
            host.dispatch("history_delete", &json!({"id": "ohos-1"}).to_string())
                .unwrap(),
            "null"
        );
        assert_eq!(
            parse(&host.dispatch("history_list", "{}").unwrap())
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let missing = host
            .dispatch("history_load", &json!({"id": "ohos-1"}).to_string())
            .unwrap_err();
        assert_eq!(missing.code, ErrorCode::NotFound);

        let escaped = host
            .dispatch(
                "history_save",
                &json!({"id": "../escape", "payload": payload}).to_string(),
            )
            .unwrap_err();
        assert_eq!(escaped.code, ErrorCode::InvalidArgument);
        assert!(!host.history_dir.join("../escape.json").exists());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn restart_resumes_session_ordinal_from_history() {
        let base = temp_base();
        let root = base.join("workspace");
        fs::create_dir_all(&root).unwrap();
        let config = || HostConfig {
            files_dir: base.join("files"),
            workspace_root: root.clone(),
            providers_file: base.join("files/providers.json"),
        };

        let host = Host::init_with_factory(config(), Arc::new(RealAgentFactory)).unwrap();
        let first = create_session(&host);
        host.dispatch(
            "history_save",
            &json!({
                "id": first,
                "payload": {"title": "重启前的会话", "savedAt": 1, "agent": {"messages": []}}
            })
            .to_string(),
        )
        .unwrap();
        drop(host);

        let host = Host::init_with_factory(config(), Arc::new(RealAgentFactory)).unwrap();
        let second = create_session(&host);
        assert_ne!(second, first);
        let loaded = parse(
            &host
                .dispatch("history_load", &json!({"id": first}).to_string())
                .unwrap(),
        );
        assert_eq!(loaded["title"], "重启前的会话");
        let listed = parse(&host.dispatch("history_list", "{}").unwrap());
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["title"], "重启前的会话");

        let _ = fs::remove_dir_all(&base);
    }
}
