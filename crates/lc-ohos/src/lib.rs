//! 鸿蒙宿主：纯 JSON 入口，host 上可单测，不依赖 NAPI。
//!
//! 首版命令面（docs/plans/ohos-shell.md §5.4）：
//! - `workspace_root` / `fs_read_dir` / `fs_read_file` 复用 lc-core 的路径校验与按层读取；
//! - `session_create` 强制 `plan` 模式（只读 + 对话），记录模型/思考挡位；
//! - `agent_send` 首次发送时按 `providers_file` 构造 lc-core `AgentLoop`：plan 策略
//!   + `read_file`/`glob`/`grep` 只读工具，事件逐条映射成契约 `EventEnvelope`；
//!     provider 缺失或流传输失败时回退到 20 个 `[stub]` delta（`done.stop_reason = "stub"`）；
//! - `agent_cancel` 置位 `AgentLoop` 共享的取消令牌；
//! - `providers_load` / `providers_save` 读写 `HostConfig.providers_file`，形状与桌面
//!   `providers.rs` 的 `ProvidersView` 一致（密钥只回传 `hasApiKey`/末四位提示）；
//! - 其余命令返回 `not_implemented`（契约没有独立的 `unsupported` 错误码）。

use std::collections::HashMap;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use lc_core::agent::{
    AgentLoop, CompactionRecord, LocalToolExecutor, LoopError, LoopOutcome, Risk, Session,
    ToolError, ToolOutput, ToolRegistry, ToolSpec, llm_event, loop_outcome, session_info,
    session_instructions,
};
use lc_core::fs::{RealPathResolver, display_path, ensure_within_root, read_dir_level};
use lc_core::llm::runtime::load_from_model;
use lc_core::llm::{
    AgentRuntime, LlmEvent, ModelConfig, OpenAiResponsesClient, ProviderConfig, ProviderError,
    ProviderKind, ProvidersFile, RuntimeError, UreqSseTransport,
};
use lc_proto::{
    CommandResult, CoreEvent, ErrorBody, ErrorCode, EventEnvelope, SessionInfo, SessionMode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod ffi;

pub type EventSink = Arc<dyn Fn(String) + Send + Sync>;

const STUB_DELTAS: u32 = 20;
const STUB_DELAY: Duration = Duration::from_millis(25);
const STUB_STOP_REASON: &str = "stub";
const CANCELLED_STOP_REASON: &str = "cancelled";

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
    factory: Arc<dyn AgentFactory>,
    sessions: Mutex<HashMap<String, SessionHandle>>,
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
    model: Option<String>,
    thinking: Option<String>,
}

impl SessionHandle {
    fn new(info: SessionInfo, model: Option<String>, thinking: Option<String>) -> Self {
        Self {
            info: Arc::new(Mutex::new(info)),
            agent: Arc::new(Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
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
        Ok(Host {
            root_display: display_path(&root),
            root,
            providers_file: config.providers_file,
            factory,
            sessions: Mutex::new(HashMap::new()),
            sink: Mutex::new(None),
            seq: Arc::new(AtomicU64::new(0)),
            next_session: AtomicU64::new(0),
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
        self.sessions_slot().insert(
            info.session_id.clone(),
            SessionHandle::new(info.clone(), model, thinking),
        );
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
        let handle = self.session(&session_id)?;
        if handle
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(invalid(format!("agent turn already running: {session_id}")));
        }
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

    /// 读取供应商配置。文件缺失或 JSON 损坏时返回空视图：首版壳没有供应商
    /// 编辑入口，空视图让前端继续显示“请先配置供应商”，而不是整页报错。
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
        if input.models.is_empty() {
            return Err(invalid("models must be a non-empty array"));
        }
        let existing = ProvidersFile::load_path(&self.providers_file).map_err(provider_error)?;
        let file = merge_and_validate(existing, input).map_err(provider_error)?;
        file.save_path(&self.providers_file)
            .map_err(provider_error)?;
        serialize_view(file)
    }

    fn session(&self, id: &str) -> Result<SessionHandle, ErrorBody> {
        self.sessions_slot()
            .get(id)
            .cloned()
            .ok_or_else(|| ErrorBody::new(ErrorCode::NotFound, format!("session not found: {id}")))
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

/// 与 `apps/desktop/src-tauri/src/providers.rs` 的 `ProviderView` 同形：
/// `apiKey` 永不回传，只给 `hasApiKey` 与末四位提示。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    pub base_url: String,
    pub enabled: bool,
    pub api_key_ref: String,
    pub has_api_key: bool,
    pub api_key_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersView {
    pub providers: Vec<ProviderView>,
    pub models: Vec<ModelConfig>,
    pub default_model: Option<String>,
    pub default_thinking: Option<String>,
    pub subagent_model: Option<String>,
    pub subagent_thinking: Option<String>,
}

impl From<ProvidersFile> for ProvidersView {
    fn from(file: ProvidersFile) -> Self {
        Self {
            providers: file
                .providers
                .into_iter()
                .map(|provider| {
                    let key = provider
                        .api_key
                        .as_deref()
                        .map(str::trim)
                        .filter(|key| !key.is_empty());
                    ProviderView {
                        id: provider.id,
                        kind: provider.kind,
                        label: provider.label,
                        base_url: provider.base_url,
                        enabled: provider.enabled,
                        api_key_ref: provider.api_key_ref,
                        has_api_key: key.is_some(),
                        api_key_hint: key.map(key_hint),
                    }
                })
                .collect(),
            models: file.models,
            default_model: file.default_model,
            default_thinking: file.default_thinking,
            subagent_model: file.subagent_model,
            subagent_thinking: file.subagent_thinking,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInput {
    id: String,
    kind: ProviderKind,
    label: String,
    base_url: String,
    #[serde(default = "default_true")]
    enabled: bool,
    /// 用户新输入的密钥；空/缺省表示保留旧值。
    #[serde(default)]
    api_key: Option<String>,
    /// 旧配置的环境变量引用；缺省表示沿用旧值。
    #[serde(default)]
    api_key_ref: Option<String>,
    #[serde(default)]
    clear_api_key: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvidersSaveInput {
    providers: Vec<ProviderInput>,
    models: Vec<ModelConfig>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    default_thinking: Option<String>,
    #[serde(default)]
    subagent_model: Option<String>,
    #[serde(default)]
    subagent_thinking: Option<String>,
}

fn default_true() -> bool {
    true
}

/// 合并旧配置里的密钥/引用，再走 lc-core 校验（与桌面命令语义一致）。
fn merge_and_validate(
    existing: Option<ProvidersFile>,
    input: ProvidersSaveInput,
) -> Result<ProvidersFile, ProviderError> {
    let mut old_providers: HashMap<String, ProviderConfig> = existing
        .unwrap_or_default()
        .providers
        .into_iter()
        .map(|provider| (provider.id.clone(), provider))
        .collect();

    let providers = input
        .providers
        .into_iter()
        .map(|provider| {
            let old = old_providers.remove(&provider.id);
            let new_key = provider
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string);
            let api_key = if provider.clear_api_key {
                None
            } else {
                new_key.or_else(|| old.as_ref().and_then(|p| p.api_key.clone()))
            };
            let api_key_ref = if provider.clear_api_key {
                String::new()
            } else {
                provider
                    .api_key_ref
                    .as_deref()
                    .map(str::trim)
                    .filter(|reference| !reference.is_empty())
                    .map(str::to_string)
                    .or_else(|| old.map(|p| p.api_key_ref))
                    .unwrap_or_default()
            };
            ProviderConfig {
                id: provider.id,
                kind: provider.kind,
                label: provider.label,
                base_url: provider.base_url,
                api_key,
                api_key_ref,
                enabled: provider.enabled,
            }
        })
        .collect();

    let file = ProvidersFile {
        providers,
        models: input.models,
        default_model: input.default_model,
        default_thinking: input.default_thinking,
        subagent_model: input.subagent_model,
        subagent_thinking: input.subagent_thinking,
    };
    file.validate()?;
    Ok(file)
}

fn serialize_view(file: ProvidersFile) -> Result<String, ErrorBody> {
    serde_json::to_string(&ProvidersView::from(file)).map_err(|error| {
        ErrorBody::new(
            ErrorCode::Internal,
            format!("serialize providers view: {error}"),
        )
    })
}

/// 只暴露末四位；短密钥整体打码，避免把 4 位以内的密钥直接回显。
fn key_hint(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 8 {
        return "••••".into();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("••••{tail}")
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
            ProviderKind::OpenAiResponses => OpenAiResponsesClient::new(
                &runtime.base_url,
                &runtime.api_key,
                Box::new(UreqSseTransport::with_connect_timeout(Duration::from_secs(
                    5,
                ))),
            ),
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

struct AgentTurn {
    text: String,
    handle: SessionHandle,
    root: PathBuf,
    providers_file: PathBuf,
    factory: Arc<dyn AgentFactory>,
}

/// 单轮执行：惰性构建/复用 `AgentLoop`，接收 LLM 事件并在失败时回退 stub。
fn run_agent_turn(emitter: &TurnEmitter, turn: AgentTurn) {
    let _running = RunningGuard(turn.handle.clone());
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

struct RunningGuard(SessionHandle);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
    }
}

/// 事件出口：注入会话 id、统一 seq，并在取消后抑制迟到事件。
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
        "supported: workspace_root, fs_read_dir, fs_read_file, session_create, agent_send, agent_cancel, providers_load, providers_save",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
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
        let host = match factory {
            Some(factory) => Host::init_with_factory(config, factory),
            None => Host::init(config),
        }
        .unwrap();
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
    fn transport_failure_falls_back_to_stub() {
        let (host, base) = setup();
        host.dispatch("providers_save", &stub_save_input().to_string())
            .unwrap();
        let events = collect_events(&host);
        let session_id = create_session_with_model(&host, Some("stub-chat"));
        save_unreachable_provider(&host);

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

        let _ = fs::remove_dir_all(&base);
    }

    fn save_unreachable_provider(host: &Host) {
        let mut input = stub_save_input();
        input["input"]["providers"][0]["baseUrl"] = json!("http://127.0.0.1:1/v1");
        input["input"]["providers"][0]["apiKey"] = json!("sk-test");
        host.dispatch("providers_save", &input.to_string()).unwrap();
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
}
