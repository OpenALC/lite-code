//! P3 Agent 会话命令层（docs/AGENT.md §2）。
//!
//! 每个会话拥有独立的 `std::thread` 与 `AgentLoop`：Tauri 主线程只做命令
//! 转发/状态查询，流式调用期间不持任何会话锁。命令层与 worker 通过
//! `WorkerCommand` 通道交互，取消用 `Arc<AtomicBool>` 立即置位，sink 与工具
//! 观察者据此抑制取消后的事件。
//!
//! 所有下行事件统一走 `core:event` 单通道（与 apps/web 的 bridge 约定一致），
//! 载荷是契约 [`EventEnvelope`]；`seq` 由 lc-host `Seq` 统一分配（1 起，
//! 与鸿蒙端同一语义），多会话共享同一序列。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use lc_core::agent::{
    AgentLoop, ApprovalDecision as CoreApprovalDecision, CompactionRecord, LocalToolExecutor,
    LoopError, LoopOutcome, Session, SessionError, ToolError, ToolOutput, ToolRegistry,
    completion_events, llm_event, mode_from_contract, session_info, session_instructions,
};
use lc_core::llm::{LlmEvent, RuntimeError, load_for_model};
use lc_core::mcp::{McpConfig, McpExecutor, McpHost};
use lc_core::task::TaskError;
use lc_proto::{
    ApprovalDecision, CommandResult, CoreEvent, ErrorBody, ErrorCode, SessionMode, TaskInfo,
    TaskStatus,
};
use tauri::Emitter;

use crate::db::{self, DbState};
use crate::task::{self, TaskEvents, TaskState};
use crate::{WorkspaceState, workspace_unavailable};

/// UI 侧 `CORE_EVENT_CHANNEL` 必须与此一致（apps/web/src/services/bridge.ts）。
pub(crate) const CORE_EVENT_CHANNEL: &str = "core:event";

const DEFAULT_INSTRUCTIONS: &str =
    "你是 Lite Code 的编码助手。先阅读相关文件再动手修改；使用工具完成请求；用中文简洁回答。";

const CANCELLED_STOP_REASON: &str = "cancelled";

// ==== 会话注册表 ====

/// 应用级会话状态：id 分配、seq 分配与会话句柄表。
///
/// 内部用 `Arc` 以便 worker 线程在退出时从注册表移除自己；worker 只持有
/// `Weak`，避免与句柄中的 Sender 形成引用环导致线程永不结束。
#[derive(Default, Clone)]
pub(crate) struct AgentSessions {
    inner: Arc<SessionsInner>,
}

#[derive(Default)]
struct SessionsInner {
    sessions: Mutex<HashMap<String, SessionHandle>>,
    seq: lc_host::envelope::Seq,
    counter: AtomicU64,
}

/// worker 命令。`ResolveApproval` 携带的是 lc-core 的决策类型（契约类型在
/// 命令入口转换），这样 worker 不需要理解 lc-proto。
#[derive(Debug)]
enum WorkerCommand {
    Send(String),
    ResolveApproval {
        approval_id: u64,
        decision: CoreApprovalDecision,
    },
    Cancel,
}

/// 命令层持有的会话句柄。字段都可在命令线程克隆，worker 侧共享同一组原子量。
#[derive(Debug, Clone)]
struct SessionHandle {
    id: String,
    sender: Sender<WorkerCommand>,
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
}

impl AgentSessions {
    fn allocate_id(&self) -> String {
        let n = self.inner.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0);
        session_id(millis, n)
    }

    fn event_sink(&self, app: tauri::AppHandle) -> Arc<dyn EventSink> {
        Arc::new(AppEventSink {
            app,
            seq: self.inner.seq.clone(),
        })
    }

    fn register(&self, handle: SessionHandle) -> Result<(), ErrorBody> {
        let mut sessions = self.registry()?;
        sessions.insert(handle.id.clone(), handle);
        Ok(())
    }

    fn handle(&self, id: &str) -> Result<SessionHandle, ErrorBody> {
        self.registry()?
            .get(id)
            .cloned()
            .ok_or_else(|| unknown_session(id))
    }

    fn remove(&self, id: &str) {
        if let Ok(mut sessions) = self.inner.sessions.lock() {
            sessions.remove(id);
        }
    }

    /// 锁中毒也走契约错误体，命令层不 panic。
    fn registry(&self) -> Result<MutexGuard<'_, HashMap<String, SessionHandle>>, ErrorBody> {
        self.inner.sessions.lock().map_err(|_| {
            ErrorBody::new(
                ErrorCode::Internal,
                "会话注册表不可用（锁中毒），请重启应用",
            )
        })
    }
}

impl SessionHandle {
    /// `agent_send` 的忙闲迁移：只允许一个在途轮次。
    fn begin_send(&self) -> Result<(), ErrorBody> {
        match self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(_) => Err(ErrorBody::new(ErrorCode::InvalidArgument, "会话正在运行")),
        }
    }

    fn end_send(&self) {
        self.busy.store(false, Ordering::Release);
    }

    fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn session_id(millis: u128, n: u64) -> String {
    format!("s-{millis}-{n}")
}

fn unknown_session(id: &str) -> ErrorBody {
    ErrorBody::new(ErrorCode::NotFound, format!("会话不存在：{id}"))
}

// ==== 插件级 Agent 工具装配（docs/EXTENSIONS.md §4）====

/// 插件工具装配层：`specs` 进 ToolRegistry，`llm_tools` 进 LLM 请求，
/// `executor` 包在既有执行器链之外（路由按工具名分发，层级顺序不影响行为）。
struct PluginToolLayer {
    specs: Vec<lc_core::agent::ToolSpec>,
    llm_tools: Vec<serde_json::Value>,
    executor: Arc<dyn lc_core::agent::ToolExecutor>,
}

/// 查询启用插件的 Agent 工具并包一层执行器；任何一步失败都降级为空层
/// （宿主未运行/Node 缺失/无启用工具时不注册插件工具，不阻断会话）。
fn plugin_tool_layer(
    root: &Path,
    executor: Arc<dyn lc_core::agent::ToolExecutor>,
) -> PluginToolLayer {
    let empty = PluginToolLayer {
        specs: Vec::new(),
        llm_tools: Vec::new(),
        executor,
    };
    let tools = lc_core::ext::plugin_tools(root);
    if tools.is_empty() {
        return empty;
    }
    let runner = match lc_core::ext::HostToolRunner::new(crate::ext::shared_host(), root) {
        Ok(runner) => runner,
        Err(error) => {
            eprintln!("[ext] 插件工具不可用：{error}");
            return empty;
        }
    };
    build_plugin_tool_layer(tools, Arc::new(runner), empty.executor)
}

/// 纯装配（单测直接覆盖）：runner 可注入 fake，tools 手工构造。
fn build_plugin_tool_layer(
    tools: Vec<lc_core::ext::PluginTool>,
    runner: Arc<dyn lc_core::ext::PluginToolRunner>,
    executor: Arc<dyn lc_core::agent::ToolExecutor>,
) -> PluginToolLayer {
    if tools.is_empty() {
        return PluginToolLayer {
            specs: Vec::new(),
            llm_tools: Vec::new(),
            executor,
        };
    }
    let plugin_executor =
        lc_core::ext::ExtToolExecutor::new(runner, &tools).with_fallback(executor);
    PluginToolLayer {
        specs: plugin_executor.tool_specs(),
        llm_tools: plugin_executor.llm_tools(),
        executor: Arc::new(plugin_executor),
    }
}

// ==== 事件发射 ====

/// 事件下行抽象：lc-host `CoreEventSink` 的桌面重导出（M3a 下沉，两端
/// 统一 trait 形状与 seq 语义）。生产实现是 Tauri `AppHandle::emit`；单测
/// 用记录型 sink，不启动 Tauri 运行时。
pub(crate) use lc_host::envelope::CoreEventSink as EventSink;

struct AppEventSink {
    app: tauri::AppHandle,
    seq: lc_host::envelope::Seq,
}

impl EventSink for AppEventSink {
    fn emit(&self, event: CoreEvent) {
        let envelope = lc_host::envelope::envelope(&self.seq, event);
        // 窗口销毁/重建期间 emit 可能失败；事件流不是命令通道，忽略即可。
        let _ = self.app.emit(CORE_EVENT_CHANNEL, envelope);
    }
}

/// worker 侧的事件出口：负责会话 id 注入与取消后的抑制。
#[derive(Clone)]
struct WorkerEmitter {
    sink: Arc<dyn EventSink>,
    session_id: String,
    cancel: Arc<AtomicBool>,
}

impl WorkerEmitter {
    fn emit(&self, event: CoreEvent) {
        self.sink.emit(event);
    }

    /// LLM 流事件：取消后不再下发文本/思考/工具开始/usage。
    fn forward_llm(&self, event: &LlmEvent) {
        if self.cancel.load(Ordering::Acquire) {
            return;
        }
        if let Some(event) = llm_event(&self.session_id, event) {
            self.emit(event);
        }
    }

    /// 工具结果由 `with_tool_observer` 回调，取消后同样抑制。
    fn forward_tool_result(&self, call_id: &str, result: &Result<ToolOutput, ToolError>) {
        if self.cancel.load(Ordering::Acquire) {
            return;
        }
        self.emit(tool_result_event(&self.session_id, call_id, result));
    }

    /// 上下文压缩记录（docs/AGENT.md §6：不静默截断）。
    fn forward_compaction(&self, record: &CompactionRecord) {
        if self.cancel.load(Ordering::Acquire) {
            return;
        }
        self.emit(compaction_event(&self.session_id, record));
    }

    /// 取消终态。`Session::cancel` 是永久状态且会清空审批队列，直接调用会让
    /// 同一会话无法继续；这里用快照把本轮标记为 cancelled，latch 由 worker
    /// 在完成后复位，保证下一条消息仍可运行。
    fn cancelled(&self, session: &Session) {
        let mut info = session_info(session);
        info.cancelled = true;
        self.emit(CoreEvent::AgentSessionState { session: info });
        self.emit(CoreEvent::AgentDone {
            session_id: self.session_id.clone(),
            stop_reason: CANCELLED_STOP_REASON.into(),
        });
    }
}

fn tool_result_event(
    session_id: &str,
    call_id: &str,
    result: &Result<ToolOutput, ToolError>,
) -> CoreEvent {
    let (ok, text) = match result {
        Ok(output) => (true, output.text.clone()),
        Err(error) => (false, error.to_string()),
    };
    CoreEvent::AgentToolResult {
        session_id: session_id.into(),
        call_id: call_id.into(),
        ok,
        text,
    }
}

fn compaction_event(session_id: &str, record: &CompactionRecord) -> CoreEvent {
    CoreEvent::AgentCompacted {
        session_id: session_id.into(),
        record_id: record.id,
        before_tokens: record.before_tokens,
        after_tokens: record.after_tokens,
        removed_messages: record.removed_messages,
        summary: record.summary.clone(),
    }
}

/// 审批挂起时本轮没有结束，`busy` 保持到 `approval_resolve` 之后。
fn keeps_busy(outcome: &LoopOutcome) -> bool {
    matches!(outcome, LoopOutcome::NeedsApproval { .. })
}

// ==== worker 线程 ====

/// Worker 与任务状态的连接：会话创建时从 `session_create` 传入，后续只在
/// 状态迁移时短暂加锁。
struct TaskBinding {
    task_id: String,
    state: TaskState,
}

impl TaskBinding {
    /// 迁移到指定状态并返回最新快照；仅接受审批联动用到的四个运行期状态。
    /// 失败只记录日志：任务存储异常不能中断 Agent 会话与审批 UI。
    fn mark(&self, status: TaskStatus) -> Option<TaskInfo> {
        let result = match status {
            TaskStatus::Blocked => self.state.mark_needs_input(&self.task_id),
            TaskStatus::Running => self.state.mark_running(&self.task_id),
            TaskStatus::Review => self.state.mark_review(&self.task_id),
            TaskStatus::Failed => self.state.mark_failed(&self.task_id),
            TaskStatus::Planning | TaskStatus::Queued | TaskStatus::Done => return None,
        };
        match result {
            Ok(task) => Some(task),
            Err(error) => {
                eprintln!(
                    "[task] 任务 {} 迁移到 {status:?} 失败：{}",
                    self.task_id, error.message
                );
                None
            }
        }
    }

    /// 审批 resolve 开始：若任务仍是 Blocked，先回到 Running，避免 UI 在
    /// 后续 LLM 流期间一直显示“待确认”。非 Blocked（例如取消已置 Failed）
    /// 不迁移，尊重显式终态。
    fn resume_if_blocked(&self) -> Option<TaskInfo> {
        let status = self
            .state
            .with_current(|store| {
                store.get(&self.task_id).ok_or_else(|| TaskError::NotFound {
                    task_id: self.task_id.clone(),
                })
            })
            .ok()?
            .status;
        if status == TaskStatus::Blocked {
            self.mark(TaskStatus::Running)
        } else {
            None
        }
    }
}

struct Worker {
    id: String,
    agent: AgentLoop,
    emitter: WorkerEmitter,
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    registry: Weak<SessionsInner>,
    /// 绑定任务时，round 结束/取消都要同步任务状态（P4 审批联动）。
    task: Option<TaskBinding>,
    /// 会话结束时终止 MCP 子进程；与 AgentLoop 内的执行器共享同一 host。
    mcp: Option<McpExecutor>,
}

impl Worker {
    fn run(mut self, receiver: Receiver<WorkerCommand>) {
        while let Ok(command) = receiver.recv() {
            match command {
                WorkerCommand::Send(text) => self.run_prompt(text),
                WorkerCommand::ResolveApproval {
                    approval_id,
                    decision,
                } => self.resolve(approval_id, decision),
                WorkerCommand::Cancel => self.cancel_idle(),
            }
        }
        // 通道断开（应用退出）时移除自己；只拿一次注册表锁，且此时不持其他锁。
        if let Some(inner) = self.registry.upgrade()
            && let Ok(mut sessions) = inner.sessions.lock()
        {
            sessions.remove(&self.id);
        }
        if let Some(mcp) = self.mcp.take() {
            mcp.shutdown();
        }
    }

    fn run_prompt(&mut self, text: String) {
        if self.cancel.swap(false, Ordering::AcqRel) {
            self.finish_cancelled();
            return;
        }
        let emitter = self.emitter.clone();
        let result = self
            .agent
            .run(&text, &mut |event| emitter.forward_llm(&event));
        self.complete(result);
    }

    fn resolve(&mut self, approval_id: u64, decision: CoreApprovalDecision) {
        if self.cancel.swap(false, Ordering::AcqRel) {
            self.finish_cancelled();
            return;
        }
        // 审批卡已处理：先解除 Blocked，再继续执行被挂起的工具与下一轮 LLM。
        if let Some(task) = self.task.as_ref().and_then(TaskBinding::resume_if_blocked) {
            self.emitter.emit(CoreEvent::TaskUpdated { task });
        }
        let emitter = self.emitter.clone();
        let result = self
            .agent
            .resolve_approval(approval_id, decision, &mut |event| {
                emitter.forward_llm(&event)
            });
        self.complete(result);
    }

    /// 取消命令在 worker 空闲时到达：没有在途轮次就清掉 latch，避免下一条
    /// 消息被误判为取消。若 `busy` 为真但循环不在跑，说明停在审批卡上：
    /// 丢弃挂起审批并结束本轮，否则会话会一直挂着旧审批无法继续。
    fn cancel_idle(&mut self) {
        if !self.busy.load(Ordering::Acquire) {
            self.cancel.store(false, Ordering::Release);
            return;
        }
        if self.agent.session().pending().is_empty() {
            // 忙但尚未进入审批挂起（例如 Send 已入队）：保留取消 latch，
            // 由 worker 处理该 Send 时统一收尾。
            return;
        }
        self.agent.discard_pending();
        self.cancel.store(false, Ordering::Release);
        self.finish_cancelled();
    }

    fn complete(&mut self, result: Result<LoopOutcome, LoopError>) {
        if self.cancel.swap(false, Ordering::AcqRel) {
            self.finish_cancelled();
            return;
        }
        match result {
            Ok(outcome) => {
                match &outcome {
                    LoopOutcome::NeedsApproval { .. } => self.mark_needs_input(),
                    LoopOutcome::Final { .. } => self.mark_task(TaskStatus::Review),
                    LoopOutcome::Denied { .. } => self.mark_task(TaskStatus::Failed),
                }
                for event in completion_events(self.agent.session(), &outcome) {
                    self.emitter.emit(event);
                }
                if !keeps_busy(&outcome) {
                    self.busy.store(false, Ordering::Release);
                }
            }
            Err(error) => {
                self.mark_task(TaskStatus::Failed);
                self.emitter.emit(CoreEvent::Error {
                    error: loop_error_body(&error),
                });
                self.busy.store(false, Ordering::Release);
            }
        }
    }

    /// 审批挂起：先落 Blocked，再发 `task:needs_input`（每轮一次）。事件走
    /// `core:event`，与 agent 事件共用 agent sink 的 seq，不额外抢焦点。
    fn mark_needs_input(&self) {
        let Some(binding) = self.task.as_ref() else {
            return;
        };
        binding.mark(TaskStatus::Blocked);
        self.emitter.emit(CoreEvent::TaskNeedsInput {
            task_id: binding.task_id.clone(),
            kind: "approval".into(),
        });
    }

    fn mark_task(&self, status: TaskStatus) {
        if let Some(task) = self.task.as_ref().and_then(|binding| binding.mark(status)) {
            self.emitter.emit(CoreEvent::TaskUpdated { task });
        }
    }

    fn finish_cancelled(&self) {
        // 取消是终止态：契约没有 Canceled，任务统一记为 Failed（worktree 保留
        // 供 Review/Discard）。
        self.mark_task(TaskStatus::Failed);
        self.emitter.cancelled(self.agent.session());
        self.busy.store(false, Ordering::Release);
    }
}

fn spawn_worker(
    sessions: &AgentSessions,
    id: String,
    agent: AgentLoop,
    sink: Arc<dyn EventSink>,
    cancel: Arc<AtomicBool>,
    task: Option<TaskBinding>,
    mcp: Option<McpExecutor>,
) -> Result<SessionHandle, ErrorBody> {
    let (sender, receiver) = mpsc::channel();
    let busy = Arc::new(AtomicBool::new(false));
    let handle = SessionHandle {
        id: id.clone(),
        sender,
        cancel: cancel.clone(),
        busy: busy.clone(),
    };
    sessions.register(handle.clone())?;

    let emitter = WorkerEmitter {
        sink,
        session_id: id.clone(),
        cancel: cancel.clone(),
    };
    // 工具结果只在循环内部可见 call id：用观察者补发 agent:tool_result。
    let observer = emitter.clone();
    let compaction_observer = emitter.clone();
    let agent = agent
        .with_cancel(cancel.clone())
        .with_tool_observer(Box::new(move |call_id, result| {
            observer.forward_tool_result(call_id, result)
        }))
        .with_compaction_observer(Box::new(move |record| {
            compaction_observer.forward_compaction(record)
        }));

    let worker = Worker {
        id: id.clone(),
        agent,
        emitter,
        cancel,
        busy,
        registry: Arc::downgrade(&sessions.inner),
        task,
        mcp,
    };
    let spawned = thread::Builder::new()
        .name(format!("agent-{id}"))
        .spawn(move || worker.run(receiver));
    if let Err(error) = spawned {
        sessions.remove(&id);
        return Err(ErrorBody::new(
            ErrorCode::Internal,
            format!("无法启动会话线程：{error}"),
        ));
    }
    Ok(handle)
}

// ==== Tauri 命令 ====

/// 创建 P3 会话：解析 provider 配置、构建 AgentLoop 并启动 worker。
///
/// `task_id` 存在时以任务的 worktree 作为工作区根（MCP/记忆/工具边界一致），
/// 非 git 任务回退 workspace 根；随后把会话绑定到任务并置 Running。
/// provider/MCP/AgentLoop 构建期间不持有 `TaskState` 锁。
#[allow(clippy::too_many_arguments)]
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn session_create(
    mode: SessionMode,
    task_id: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    app: tauri::AppHandle,
    workspace: tauri::State<'_, WorkspaceState>,
    sessions: tauri::State<'_, AgentSessions>,
    tasks: tauri::State<'_, TaskState>,
    task_events: tauri::State<'_, TaskEvents>,
    db: tauri::State<'_, DbState>,
) -> Result<CommandResult, ErrorBody> {
    let workspace_root = workspace.get().ok_or_else(workspace_unavailable)?;
    let target = match task_id.as_deref() {
        Some(task_id) => Some(task::session_target(&tasks, &workspace_root, task_id)?),
        None => None,
    };
    let root = target
        .as_ref()
        .map_or_else(|| workspace_root.clone(), |target| target.root.clone());

    let runtime = load_for_model(model.as_deref()).map_err(runtime_error_body)?;
    let reasoning = reasoning_effort(thinking.as_deref(), &runtime.thinking)?;
    let client = runtime.openai_client().map_err(runtime_error_body)?;

    let session = new_session(sessions.allocate_id(), mode)
        .with_context(runtime.context_k, runtime.compress_at_k);
    let info = session_info(&session);

    let registry = ToolRegistry::with_builtins();
    let mut mcp_config = McpConfig::load(&root).unwrap_or_else(|error| {
        eprintln!("[mcp] 配置加载失败：{error}");
        McpConfig::default()
    });
    // 插件声明的 MCP server 与用户配置合并（名称带 plugin.<id> 前缀）。
    mcp_config
        .servers
        .extend(lc_core::ext::plugin_mcp_servers(&root));
    let mcp_executor = McpExecutor::from_host(McpHost::connect(&mcp_config, &root));
    for warning in mcp_executor.warnings() {
        eprintln!("[mcp] {warning}");
    }
    let mcp_shutdown = mcp_executor.clone();
    let registry = registry.with_mcp_tools(mcp_executor.tool_specs());
    let mut tools = registry.llm_tools();
    for tool in mcp_executor.llm_tools() {
        let registered = tool
            .get("name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| registry.get(name).is_some());
        if registered {
            tools.push(tool);
        }
    }
    let executor: Arc<dyn lc_core::agent::ToolExecutor> =
        Arc::new(mcp_executor.with_fallback(Box::new(LocalToolExecutor::new(root.clone()))));

    // 插件级 Agent 工具（docs/EXTENSIONS.md §4）：启用 node 插件的
    // contributes.tools 一律按 Risk::Mutating 注册，进 AgentLoop 后由既有
    // 策略层审批；宿主/Node 不可用或没有启用工具时静默降级，不阻断会话。
    let plugin_layer = plugin_tool_layer(&root, executor);
    let registry = registry.with_plugin_tools(plugin_layer.specs.clone());
    for tool in &plugin_layer.llm_tools {
        let registered = tool
            .get("name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| registry.get(name).is_some());
        if registered {
            tools.push(tool.clone());
        }
    }
    let executor = plugin_layer.executor;

    // DB 工具优先：db_* 由 DbExecutor 处理，其余交给 插件 → MCP → 本地执行器。
    // 初始化失败只让 db_* 返回 ToolError::Database，不阻止普通会话。
    let executor = db::agent_executor(db.inner(), executor);
    let instructions = session_instructions(&root, DEFAULT_INSTRUCTIONS);
    let agent = AgentLoop::new(
        session,
        Box::new(client),
        executor,
        registry,
        root,
        runtime.model_id,
    )
    .with_instructions(instructions)
    .with_reasoning(reasoning)
    .with_tools(tools);

    let sink = sessions.event_sink(app.clone());
    let task_sink = task_events.sink(app);

    let bound = match target {
        Some(target) => match task::bind_session_to_task(
            &tasks,
            &workspace_root,
            &target.task.task_id,
            &info.session_id,
        ) {
            Ok(bound) => Some(bound),
            Err(error) => {
                mcp_shutdown.shutdown();
                return Err(error);
            }
        },
        None => None,
    };

    // 克隆共享的 TaskState（Arc）交给 worker：会话内的审批/结束/取消都能
    // 直接迁移任务状态，不需要再次解析 workspace 根。
    let task_binding = bound.as_ref().map(|bound| TaskBinding {
        task_id: bound.task.task_id.clone(),
        state: tasks.inner().clone(),
    });

    if let Err(error) = spawn_worker(
        &sessions,
        info.session_id.clone(),
        agent,
        sink.clone(),
        Arc::new(AtomicBool::new(false)),
        task_binding,
        Some(mcp_shutdown),
    ) {
        if let Some(bound) = bound.as_ref()
            && bound.started
        {
            match task::cancel_task_by_id(
                &tasks,
                &workspace_root,
                &bound.task.task_id,
                task_sink.as_ref(),
            ) {
                Ok(task) => {
                    eprintln!("[task] 会话创建失败，任务 {} 已取消", task.task_id);
                }
                Err(rollback) => {
                    eprintln!("[task] 会话创建失败且回滚失败：{}", rollback.message);
                }
            }
        }
        return Err(error);
    }
    // 创建后立即下发一次会话状态；worker 在下一条命令前不会产生事件。
    sink.emit(CoreEvent::AgentSessionState {
        session: info.clone(),
    });
    if let Some(bound) = bound {
        task_sink.emit(CoreEvent::TaskUpdated { task: bound.task });
    }

    Ok(CommandResult::Session { session: info })
}

/// 发送用户消息：置 busy 后立即返回 Ack，内容走 `agent:*` 事件。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn agent_send(
    session_id: String,
    text: String,
    sessions: tauri::State<'_, AgentSessions>,
) -> Result<CommandResult, ErrorBody> {
    let handle = sessions.handle(&session_id)?;
    handle.begin_send()?;
    if handle.sender.send(WorkerCommand::Send(text)).is_err() {
        handle.end_send();
        sessions.remove(&session_id);
        return Err(unknown_session(&session_id));
    }
    Ok(CommandResult::Ack)
}

/// 处理审批卡：转发给 worker，成功受理即返回 Ack。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn approval_resolve(
    session_id: String,
    approval_id: u64,
    decision: ApprovalDecision,
    sessions: tauri::State<'_, AgentSessions>,
) -> Result<CommandResult, ErrorBody> {
    let handle = sessions.handle(&session_id)?;
    let command = WorkerCommand::ResolveApproval {
        approval_id,
        decision: approval_decision(decision),
    };
    if handle.sender.send(command).is_err() {
        sessions.remove(&session_id);
        return Err(unknown_session(&session_id));
    }
    Ok(CommandResult::Ack)
}

/// 取消当前轮：先置位（sink 立即停止下发），再通知 worker 收尾。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn agent_cancel(
    session_id: String,
    sessions: tauri::State<'_, AgentSessions>,
) -> Result<CommandResult, ErrorBody> {
    let handle = sessions.handle(&session_id)?;
    handle.request_cancel();
    if handle.sender.send(WorkerCommand::Cancel).is_err() {
        sessions.remove(&session_id);
        return Err(unknown_session(&session_id));
    }
    Ok(CommandResult::Ack)
}

// ==== 纯逻辑（单测直接覆盖） ====

fn new_session(id: impl Into<String>, mode: SessionMode) -> Session {
    let mut session = Session::new(id, mode_from_contract(mode));
    if mode == SessionMode::Yolo {
        // UI 在切换 yolo 前已二次确认；命令层仍按显式请求记录确认。
        session.confirm_yolo();
    }
    session
}

fn approval_decision(decision: ApprovalDecision) -> CoreApprovalDecision {
    match decision {
        ApprovalDecision::AllowOnce => CoreApprovalDecision::AllowOnce,
        ApprovalDecision::AllowSession => CoreApprovalDecision::AllowSession,
        ApprovalDecision::Deny => CoreApprovalDecision::Deny,
    }
}

/// provider 配置失败 → 可操作的契约错误。失败发生在会话创建前，码限定在
/// `InvalidArgument` / `NotFound`（不是运行时 IO 事件）。
/// 思考挡位 → Responses `reasoning.effort`；`off` 不发送，`max` 归一到 high。
fn reasoning_effort(
    thinking: Option<&str>,
    supported: &[String],
) -> Result<Option<String>, ErrorBody> {
    let Some(level) = thinking.map(str::trim).filter(|level| !level.is_empty()) else {
        return Ok(None);
    };
    if !supported.is_empty() && !supported.iter().any(|item| item == level) {
        return Err(ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("思考挡位 {level} 不在模型支持的列表中"),
        ));
    }
    match level {
        "off" => Ok(None),
        "low" => Ok(Some("low".into())),
        "medium" => Ok(Some("medium".into())),
        "high" | "max" => Ok(Some("high".into())),
        other => Err(ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("不支持的思考挡位：{other}"),
        )),
    }
}

fn runtime_error_body(error: RuntimeError) -> ErrorBody {
    match error {
        RuntimeError::Io(detail) => ErrorBody::new(
            ErrorCode::NotFound,
            format!(
                "无法读取 providers 配置（{detail}）：请检查 ~/.lite-code/providers.json \
                 或 LC_PROVIDERS_FILE 指向的文件"
            ),
        ),
        RuntimeError::Config(error) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("providers 配置无效（{error}）：请检查 JSON 与必填字段"),
        ),
        RuntimeError::NoModel => ErrorBody::new(
            ErrorCode::InvalidArgument,
            "providers 配置没有可用模型：请在 models 中添加模型并设置 defaultModel",
        ),
        RuntimeError::UnknownModel(model) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("模型 {model} 不存在：请在供应商页面选择已配置的模型"),
        ),
        RuntimeError::ProviderUnavailable(provider) => ErrorBody::new(
            ErrorCode::NotFound,
            format!(
                "provider {provider} 不存在或未启用：请检查 providers.json 的 enabled \
                 与 defaultModel"
            ),
        ),
        RuntimeError::UnresolvedKeyRef(reference) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("无法解析密钥引用 {reference}：当前仅支持 env:NAME，或设置环境变量 LC_API_KEY"),
        ),
        RuntimeError::MissingEnv(name) => ErrorBody::new(
            ErrorCode::NotFound,
            format!("环境变量 {name} 未设置：请导出该变量，或设置 LC_API_KEY"),
        ),
        RuntimeError::MissingApiKey => ErrorBody::new(
            ErrorCode::InvalidArgument,
            "供应商未配置 API Key：请在左侧“供应商”页面填写，或设置 LC_API_KEY",
        ),
        RuntimeError::UnsupportedKind(kind) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!(
                "provider 类型 {kind:?} 暂不支持流式会话：请把 defaultModel 指向 \
                 openai-responses 的模型（Anthropic 传输层尚未实现）"
            ),
        ),
    }
}

/// 循环错误 → 契约错误码。已知终止条件给专用码，其余 `Internal` 兜底。
fn loop_error_body(error: &LoopError) -> ErrorBody {
    match error {
        LoopError::Llm(error) => ErrorBody::new(ErrorCode::Internal, error.to_string()),
        LoopError::Collector(error) => ErrorBody::new(ErrorCode::Internal, error.to_string()),
        LoopError::Session(SessionError::Cancelled) => {
            ErrorBody::new(ErrorCode::Cancelled, "会话已取消")
        }
        LoopError::Session(SessionError::TurnLimit { max }) => ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("已达到轮次上限 {max}：请把任务拆小后重试"),
        ),
        LoopError::Session(error) => ErrorBody::new(ErrorCode::InvalidArgument, error.to_string()),
        LoopError::UnknownTool(name) => {
            ErrorBody::new(ErrorCode::NotFound, format!("模型请求了未知工具：{name}"))
        }
        LoopError::Tool(error) => tool_error_body(error),
        LoopError::Provider { code, message } => ErrorBody::new(*code, message.clone()),
        LoopError::ContextOverflow { .. } => {
            ErrorBody::new(ErrorCode::InvalidArgument, error.to_string())
        }
    }
}

fn tool_error_body(error: &ToolError) -> ErrorBody {
    let code = match error {
        ToolError::Timeout { .. } => ErrorCode::Timeout,
        ToolError::ShellUnavailable(_) => ErrorCode::ShellUnavailable,
        ToolError::PathOutsideRoot(_) => ErrorCode::PathOutsideRoot,
        ToolError::NotFound(_) => ErrorCode::NotFound,
        ToolError::InvalidArgs(_) => ErrorCode::InvalidArgument,
        ToolError::ProtectedPath(_) => ErrorCode::PermissionDenied,
        // 服务错误在 ToolError::Database 里只剩文本：闸门拒绝与不可用分开映射，
        // 其余（连接不存在、未连接、Node 缺失）统一按不可用处理。
        ToolError::Database(message) if message.contains("拒绝") => ErrorCode::DbRejected,
        ToolError::Database(_) => ErrorCode::DbUnavailable,
        _ => ErrorCode::Internal,
    };
    ErrorBody::new(code, error.to_string())
}

// ==== 单测：不启动 Tauri 运行时，用记录型 sink + fake LLM/executor ====

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};

    use lc_core::agent::{
        ApprovalRequest, LlmStream, PolicyMode, PolicySource, Risk, ToolCall, ToolExecutor,
        ToolRegistry,
    };
    use lc_core::llm::ProviderKind;
    use lc_core::llm::{LlmError, LlmRequest, StopReason};
    use lc_core::task::TaskStore;
    use lc_proto::{ApprovalSource, SessionInfo, SessionMode};

    use super::*;

    #[derive(Default, Clone)]
    struct RecordingSink {
        events: Arc<Mutex<Vec<CoreEvent>>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event: CoreEvent) {
            self.events.lock().expect("recording sink").push(event);
        }
    }

    impl RecordingSink {
        fn events(&self) -> Vec<CoreEvent> {
            self.events.lock().expect("recording sink").clone()
        }
    }

    struct FakeLlm {
        turns: Mutex<VecDeque<Vec<LlmEvent>>>,
    }

    impl FakeLlm {
        fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
            Self {
                turns: Mutex::new(turns.into()),
            }
        }
    }

    impl LlmStream for FakeLlm {
        fn stream(
            &self,
            _request: &LlmRequest,
            sink: &mut dyn FnMut(LlmEvent),
        ) -> Result<(), LlmError> {
            for event in self
                .turns
                .lock()
                .expect("fake llm")
                .pop_front()
                .unwrap_or_default()
            {
                sink(event);
            }
            Ok(())
        }
    }

    struct FakeExecutor;

    impl ToolExecutor for FakeExecutor {
        fn execute(&self, _call: &ToolCall) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                text: "tool ok".into(),
            })
        }
    }

    fn test_loop(id: &str, llm: Box<dyn LlmStream>) -> AgentLoop {
        AgentLoop::new(
            Session::new(id, PolicyMode::Auto),
            llm,
            Box::new(FakeExecutor),
            ToolRegistry::with_builtins(),
            std::env::temp_dir(),
            "test-model",
        )
    }

    fn spawn_test_worker(
        sink: &RecordingSink,
        llm: Box<dyn LlmStream>,
    ) -> (AgentSessions, SessionHandle) {
        let sessions = AgentSessions::default();
        let id = sessions.allocate_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = spawn_worker(
            &sessions,
            id.clone(),
            test_loop(&id, llm),
            Arc::new(sink.clone()),
            cancel,
            None,
            None,
        )
        .expect("spawn worker");
        (sessions, handle)
    }

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("condition not met within 2s");
    }

    fn text_count(events: &[CoreEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, CoreEvent::AgentTextDelta { .. }))
            .count()
    }

    // ---- 任务联动测试夹具：临时 git 仓库 + 已 start 的任务 ----

    struct TaskFixture {
        state: TaskState,
        task_id: String,
    }

    impl TaskFixture {
        fn binding(&self) -> TaskBinding {
            TaskBinding {
                task_id: self.task_id.clone(),
                state: self.state.clone(),
            }
        }

        fn task(&self) -> TaskInfo {
            self.state
                .with_current(|store| {
                    store.get(&self.task_id).ok_or_else(|| TaskError::NotFound {
                        task_id: self.task_id.clone(),
                    })
                })
                .expect("task snapshot")
        }

        fn status(&self) -> TaskStatus {
            self.task().status
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lc-desktop-agent-{}-{}-{}",
            tag,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .output()
            .unwrap_or_else(|error| panic!("spawn git {args:?}: {error}"));
        assert!(
            out.status.success(),
            "git {args:?} failed in {}:\nstdout: {}\nstderr: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir).expect("create repo dir");
        run_git(dir, &["init", "-q"]);
        run_git(dir, &["config", "user.email", "agent-task@example.com"]);
        run_git(dir, &["config", "user.name", "Agent Task Test"]);
        run_git(dir, &["config", "core.autocrlf", "false"]);
        run_git(dir, &["config", "commit.gpgsign", "false"]);
        fs::write(dir.join("README.md"), "hello\n").expect("write README");
        run_git(dir, &["add", "-A"]);
        run_git(dir, &["commit", "-q", "-m", "init"]);
    }

    fn task_fixture(tag: &str, session_id: &str) -> TaskFixture {
        let base = temp_root(tag);
        let repo = base.join("repo");
        init_repo(&repo);
        let mut store = TaskStore::with_max_running(base.join("data").join("tasks.json"), &repo, 2);
        let task = store
            .create("approval link", Some(session_id.into()))
            .expect("create task");
        let started = store.start(&task.task_id).expect("start task");
        assert_eq!(started.status, TaskStatus::Running);
        assert!(started.port.is_some());
        assert!(started.worktree_path.is_some());

        let state = TaskState::new();
        state.init_for_test(store);
        TaskFixture {
            state,
            task_id: task.task_id,
        }
    }

    fn task_updated_statuses(events: &[CoreEvent]) -> Vec<TaskStatus> {
        events
            .iter()
            .filter_map(|event| match event {
                CoreEvent::TaskUpdated { task } => Some(task.status),
                _ => None,
            })
            .collect()
    }

    fn needs_input_count(events: &[CoreEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, CoreEvent::TaskNeedsInput { .. }))
            .count()
    }

    #[test]
    fn session_ids_are_prefixed_and_unique() {
        assert_eq!(session_id(1_700_000_000_000, 1), "s-1700000000000-1");

        let sessions = AgentSessions::default();
        let first = sessions.allocate_id();
        let second = sessions.allocate_id();
        assert!(first.starts_with("s-"), "{first}");
        assert!(second.starts_with("s-"), "{second}");
        assert_ne!(first, second);
    }

    #[test]
    fn busy_gate_allows_one_in_flight_turn() {
        let (sender, _receiver) = mpsc::channel();
        let handle = SessionHandle {
            id: "s-test".into(),
            sender,
            cancel: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(false)),
        };

        handle.begin_send().expect("first send");
        let error = handle.begin_send().expect_err("second send rejected");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(error.message, "会话正在运行");

        handle.end_send();
        handle.begin_send().expect("idle again");
    }

    #[test]
    fn session_modes_map_and_yolo_is_confirmed() {
        assert_eq!(new_session("a", SessionMode::Auto).mode(), PolicyMode::Auto);
        assert_eq!(new_session("p", SessionMode::Plan).mode(), PolicyMode::Plan);
        assert_eq!(new_session("c", SessionMode::Chat).mode(), PolicyMode::Chat);
        // Session::new(Yolo) 本身不会确认；命令层必须显式 confirm_yolo。
        assert_eq!(new_session("y", SessionMode::Yolo).mode(), PolicyMode::Yolo);
    }

    #[test]
    fn runtime_errors_are_actionable_contract_errors() {
        let io = runtime_error_body(RuntimeError::Io("denied".into()));
        assert_eq!(io.code, ErrorCode::NotFound);
        assert!(io.message.contains("providers.json"), "{}", io.message);

        let missing = runtime_error_body(RuntimeError::MissingEnv("OPENAI_API_KEY".into()));
        assert_eq!(missing.code, ErrorCode::NotFound);
        assert!(
            missing.message.contains("LC_API_KEY"),
            "{}",
            missing.message
        );

        let key_ref = runtime_error_body(RuntimeError::UnresolvedKeyRef("keyring://x".into()));
        assert_eq!(key_ref.code, ErrorCode::InvalidArgument);
        assert!(key_ref.message.contains("env:NAME"), "{}", key_ref.message);

        let anthropic = runtime_error_body(RuntimeError::UnsupportedKind(
            ProviderKind::AnthropicMessages,
        ));
        assert_eq!(anthropic.code, ErrorCode::InvalidArgument);
        assert!(
            anthropic.message.contains("openai-responses"),
            "{}",
            anthropic.message
        );

        let no_model = runtime_error_body(RuntimeError::NoModel);
        assert_eq!(no_model.code, ErrorCode::InvalidArgument);
        assert!(
            no_model.message.contains("defaultModel"),
            "{}",
            no_model.message
        );

        let unavailable = runtime_error_body(RuntimeError::ProviderUnavailable("x".into()));
        assert_eq!(unavailable.code, ErrorCode::NotFound);
    }

    #[test]
    fn loop_errors_map_to_contract_codes() {
        assert_eq!(
            loop_error_body(&LoopError::Session(SessionError::Cancelled)).code,
            ErrorCode::Cancelled
        );

        let limit = loop_error_body(&LoopError::Session(SessionError::TurnLimit { max: 3 }));
        assert_eq!(limit.code, ErrorCode::InvalidArgument);
        assert!(limit.message.contains('3'), "{}", limit.message);

        assert_eq!(
            loop_error_body(&LoopError::UnknownTool("nope".into())).code,
            ErrorCode::NotFound
        );
        assert_eq!(
            loop_error_body(&LoopError::Provider {
                code: ErrorCode::Timeout,
                message: "slow".into(),
            })
            .code,
            ErrorCode::Timeout
        );
        assert_eq!(
            loop_error_body(&LoopError::Llm(LlmError::Http("boom".into()))).code,
            ErrorCode::Internal
        );
        assert_eq!(
            loop_error_body(&LoopError::Tool(ToolError::Timeout { timeout_ms: 30 })).code,
            ErrorCode::Timeout
        );
        assert_eq!(
            loop_error_body(&LoopError::Tool(ToolError::ShellUnavailable("pwsh".into()))).code,
            ErrorCode::ShellUnavailable
        );

        let overflow = loop_error_body(&LoopError::ContextOverflow {
            used_tokens: 500_000,
            context_k: 400,
        });
        assert_eq!(overflow.code, ErrorCode::InvalidArgument);
        assert!(overflow.message.contains("500000"), "{}", overflow.message);
        assert!(overflow.message.contains("400k"), "{}", overflow.message);
    }

    #[test]
    fn tool_database_errors_map_to_db_codes() {
        let rejected = tool_error_body(&ToolError::Database(
            "数据库拒绝执行：只允许单条语句".into(),
        ));
        assert_eq!(rejected.code, ErrorCode::DbRejected);
        assert!(rejected.message.contains("拒绝"), "{}", rejected.message);

        let unavailable = tool_error_body(&ToolError::Database("数据库连接未建立：main".into()));
        assert_eq!(unavailable.code, ErrorCode::DbUnavailable);
        assert!(
            unavailable.message.contains("未建立"),
            "{}",
            unavailable.message
        );

        assert_eq!(
            loop_error_body(&LoopError::Tool(ToolError::Database(
                "数据库 sidecar 未启动".into(),
            )))
            .code,
            ErrorCode::DbUnavailable
        );
    }

    #[test]
    fn compaction_record_maps_to_contract_event() {
        let record = CompactionRecord {
            id: 3,
            before_tokens: 12_300,
            after_tokens: 4_100,
            removed_messages: 7,
            summary: "已压缩 7 条早期消息".into(),
        };
        assert_eq!(
            compaction_event("s1", &record),
            CoreEvent::AgentCompacted {
                session_id: "s1".into(),
                record_id: 3,
                before_tokens: 12_300,
                after_tokens: 4_100,
                removed_messages: 7,
                summary: "已压缩 7 条早期消息".into(),
            }
        );
    }

    #[test]
    fn completion_events_cover_all_outcomes() {
        let session = Session::new("s1", PolicyMode::Auto);

        let final_events = completion_events(&session, &LoopOutcome::Final { text: "ok".into() });
        assert!(matches!(
            &final_events[0],
            CoreEvent::AgentSessionState { session } if session.session_id == "s1"
        ));
        assert_eq!(
            final_events[1],
            CoreEvent::AgentDone {
                session_id: "s1".into(),
                stop_reason: "end_turn".into(),
            }
        );
        assert!(!keeps_busy(&LoopOutcome::Final {
            text: String::new()
        }));

        let approval = LoopOutcome::NeedsApproval {
            approval: ApprovalRequest {
                id: 7,
                tool: "shell".into(),
                command: Some("echo hi".into()),
                source: PolicySource::ExecuteReview,
                reason: "review".into(),
            },
        };
        let approval_events = completion_events(&session, &approval);
        assert!(matches!(
            &approval_events[1],
            CoreEvent::AgentApprovalRequired { approval, .. } if approval.approval_id == 7
        ));
        assert!(keeps_busy(&approval));

        let denied = completion_events(
            &session,
            &LoopOutcome::Denied {
                source: PolicySource::Mode,
                reason: "plan 模式禁止写入".into(),
            },
        );
        assert!(matches!(
            &denied[1],
            CoreEvent::Error { error } if error.code == ErrorCode::PermissionDenied
        ));
    }

    #[test]
    fn tool_results_and_llm_forwarding_obey_cancel() {
        let ok = tool_result_event(
            "s1",
            "call_1",
            &Ok(ToolOutput {
                text: "tool ok".into(),
            }),
        );
        assert_eq!(
            ok,
            CoreEvent::AgentToolResult {
                session_id: "s1".into(),
                call_id: "call_1".into(),
                ok: true,
                text: "tool ok".into(),
            }
        );
        let failed = tool_result_event(
            "s1",
            "call_2",
            &Err(ToolError::NotFound("missing.txt".into())),
        );
        let CoreEvent::AgentToolResult { ok, text, .. } = failed else {
            panic!("expected tool result");
        };
        assert!(!ok);
        assert!(text.contains("missing.txt"), "{text}");

        let sink = RecordingSink::default();
        let cancel = Arc::new(AtomicBool::new(true));
        let emitter = WorkerEmitter {
            sink: Arc::new(sink.clone()),
            session_id: "s1".into(),
            cancel: cancel.clone(),
        };
        emitter.forward_llm(&LlmEvent::TextDelta {
            index: 0,
            text: "hidden".into(),
        });
        emitter.forward_tool_result(
            "call_1",
            &Ok(ToolOutput {
                text: "hidden".into(),
            }),
        );
        assert!(sink.events().is_empty());

        cancel.store(false, Ordering::Release);
        emitter.forward_llm(&LlmEvent::TextDelta {
            index: 0,
            text: "visible".into(),
        });
        assert_eq!(
            sink.events(),
            vec![CoreEvent::AgentTextDelta {
                session_id: "s1".into(),
                index: 0,
                text: "visible".into(),
            }]
        );
    }

    #[test]
    fn envelopes_are_sequenced_and_contract_shaped() {
        assert_eq!(CORE_EVENT_CHANNEL, "core:event");

        // M3a：seq 语义统一到 lc-host `Seq`（1 起，种子 n 的下一个为 n+1；
        // 原 AtomicU64 0 基 fetch_add 的 41/42 对应现在的 42/43）。
        let seq = lc_host::envelope::Seq::from_u64(41);
        let first = lc_host::envelope::envelope(
            &seq,
            CoreEvent::AgentThinkingDelta {
                session_id: "s1".into(),
                text: "think".into(),
            },
        );
        let second = lc_host::envelope::envelope(
            &seq,
            CoreEvent::AgentDone {
                session_id: "s1".into(),
                stop_reason: "end_turn".into(),
            },
        );
        assert_eq!(first.seq, 42);
        assert_eq!(second.seq, 43);

        let json = serde_json::to_value(&second).expect("serialize envelope");
        assert_eq!(json["seq"], 43);
        assert_eq!(json["event"]["type"], "agent:done");
        assert_eq!(json["event"]["stop_reason"], "end_turn");
    }

    #[test]
    fn registry_lookup_and_remove() {
        let sessions = AgentSessions::default();
        let id = sessions.allocate_id();
        let (sender, _receiver) = mpsc::channel();
        sessions
            .register(SessionHandle {
                id: id.clone(),
                sender,
                cancel: Arc::new(AtomicBool::new(false)),
                busy: Arc::new(AtomicBool::new(false)),
            })
            .expect("register");
        assert_eq!(sessions.handle(&id).expect("lookup").id, id);

        sessions.remove(&id);
        let error = sessions.handle(&id).expect_err("removed");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains(&id), "{}", error.message);
    }

    #[test]
    fn worker_streams_turn_and_clears_busy() {
        let sink = RecordingSink::default();
        let llm = FakeLlm::new(vec![vec![
            LlmEvent::TextDelta {
                index: 0,
                text: "hi".into(),
            },
            LlmEvent::Usage {
                input: 3,
                output: 1,
                cached: 0,
            },
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
        ]]);
        let (_sessions, handle) = spawn_test_worker(&sink, Box::new(llm));

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("hello".into()))
            .expect("send");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        let events = sink.events();
        assert!(matches!(
            &events[0],
            CoreEvent::AgentTextDelta { text, .. } if text == "hi"
        ));
        assert!(matches!(&events[1], CoreEvent::AgentUsage { input: 3, .. }));
        assert!(matches!(
            &events[2],
            CoreEvent::AgentSessionState { session } if session.session_id == handle.id
        ));
        assert!(matches!(
            events.last(),
            Some(CoreEvent::AgentDone { stop_reason, .. }) if stop_reason == "end_turn"
        ));
    }

    #[test]
    fn worker_cancel_suppresses_rest_of_stream() {
        struct CancellingLlm {
            cancel: Arc<AtomicBool>,
        }

        impl LlmStream for CancellingLlm {
            fn stream(
                &self,
                _request: &LlmRequest,
                sink: &mut dyn FnMut(LlmEvent),
            ) -> Result<(), LlmError> {
                sink(LlmEvent::TextDelta {
                    index: 0,
                    text: "first".into(),
                });
                self.cancel.store(true, Ordering::Release);
                sink(LlmEvent::TextDelta {
                    index: 0,
                    text: "second".into(),
                });
                sink(LlmEvent::Usage {
                    input: 9,
                    output: 9,
                    cached: 0,
                });
                Ok(())
            }
        }

        let sink = RecordingSink::default();
        let sessions = AgentSessions::default();
        let id = sessions.allocate_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = spawn_worker(
            &sessions,
            id.clone(),
            test_loop(
                &id,
                Box::new(CancellingLlm {
                    cancel: cancel.clone(),
                }),
            ),
            Arc::new(sink.clone()),
            cancel,
            None,
            None,
        )
        .expect("spawn");

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("hello".into()))
            .expect("send");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        let events = sink.events();
        assert_eq!(text_count(&events), 1, "{events:?}");
        assert!(matches!(&events[0], CoreEvent::AgentTextDelta { text, .. } if text == "first"));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, CoreEvent::AgentUsage { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            CoreEvent::AgentSessionState { session } if session.cancelled
        )));
        assert!(matches!(
            events.last(),
            Some(CoreEvent::AgentDone { stop_reason, .. }) if stop_reason == "cancelled"
        ));
        // latch 复位：同一会话的下一条消息仍可运行。
        assert!(!handle.cancel.load(Ordering::Acquire));
    }

    #[test]
    fn worker_send_after_cancel_reports_cancelled_without_running() {
        let sink = RecordingSink::default();
        let llm = FakeLlm::new(vec![vec![
            LlmEvent::TextDelta {
                index: 0,
                text: "should not appear".into(),
            },
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
        ]]);
        let (_sessions, handle) = spawn_test_worker(&sink, Box::new(llm));

        handle.request_cancel();
        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("hello".into()))
            .expect("send");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        let events = sink.events();
        assert_eq!(text_count(&events), 0, "{events:?}");
        assert!(matches!(
            events.last(),
            Some(CoreEvent::AgentDone { stop_reason, .. }) if stop_reason == "cancelled"
        ));
        assert!(!handle.cancel.load(Ordering::Acquire));
    }

    #[test]
    fn worker_cancel_prevents_loop_from_executing_tools() {
        struct CancelBeforeToolLlm {
            cancel: Arc<AtomicBool>,
        }

        impl LlmStream for CancelBeforeToolLlm {
            fn stream(
                &self,
                _request: &LlmRequest,
                sink: &mut dyn FnMut(LlmEvent),
            ) -> Result<(), LlmError> {
                sink(LlmEvent::ToolUseStart {
                    id: "call_1".into(),
                    name: "read_file".into(),
                });
                sink(LlmEvent::ToolUseArgsDelta {
                    id: "call_1".into(),
                    json: r#"{"path":"a.txt"}"#.into(),
                });
                sink(LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                });
                self.cancel.store(true, Ordering::Release);
                Ok(())
            }
        }

        struct CountingExecutor {
            calls: Arc<Mutex<usize>>,
        }

        impl ToolExecutor for CountingExecutor {
            fn execute(&self, _call: &ToolCall) -> Result<ToolOutput, ToolError> {
                *self.calls.lock().expect("calls") += 1;
                Ok(ToolOutput {
                    text: "tool ok".into(),
                })
            }
        }

        let sink = RecordingSink::default();
        let sessions = AgentSessions::default();
        let id = sessions.allocate_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(Mutex::new(0));
        let agent = AgentLoop::new(
            Session::new(id.clone(), PolicyMode::Auto),
            Box::new(CancelBeforeToolLlm {
                cancel: Arc::clone(&cancel),
            }),
            Box::new(CountingExecutor {
                calls: Arc::clone(&calls),
            }),
            ToolRegistry::with_builtins(),
            std::env::temp_dir(),
            "test-model",
        );
        // 关键：AgentLoop 没有自己 with_cancel，必须由 spawn_worker 接入
        // 命令层共享的同一令牌。
        let handle = spawn_worker(
            &sessions,
            id,
            agent,
            Arc::new(sink.clone()),
            cancel,
            None,
            None,
        )
        .expect("spawn");

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("go".into()))
            .expect("send");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        assert_eq!(*calls.lock().expect("calls"), 0, "取消后不得执行工具");
        assert!(sink.events().iter().any(|event| matches!(
            event,
            CoreEvent::AgentDone { stop_reason, .. } if stop_reason == CANCELLED_STOP_REASON
        )));
    }

    #[test]
    fn worker_keeps_busy_while_approval_pending_then_resumes() {
        let sink = RecordingSink::default();
        let llm = FakeLlm::new(vec![
            vec![
                LlmEvent::ToolUseStart {
                    id: "call_1".into(),
                    name: "shell".into(),
                },
                LlmEvent::ToolUseArgsDelta {
                    id: "call_1".into(),
                    json: r#"{"command":"echo hi"}"#.into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    index: 0,
                    text: "finished".into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                },
            ],
        ]);
        let (_sessions, handle) = spawn_test_worker(&sink, Box::new(llm));

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("run echo".into()))
            .expect("send");
        wait_for(|| {
            sink.events()
                .iter()
                .any(|event| matches!(event, CoreEvent::AgentApprovalRequired { .. }))
        });
        // 审批挂起期间保持 busy：新的 agent_send 会被拒绝。
        assert!(handle.busy.load(Ordering::Acquire));
        assert_eq!(
            handle.begin_send().expect_err("still running").code,
            ErrorCode::InvalidArgument
        );

        let approval_id = sink
            .events()
            .iter()
            .find_map(|event| match event {
                CoreEvent::AgentApprovalRequired { approval, .. } => Some(approval.approval_id),
                _ => None,
            })
            .expect("approval id");
        handle
            .sender
            .send(WorkerCommand::ResolveApproval {
                approval_id,
                decision: CoreApprovalDecision::AllowOnce,
            })
            .expect("resolve");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        let events = sink.events();
        assert!(events.iter().any(|event| matches!(
            event,
            CoreEvent::AgentToolResult { ok: true, text, .. } if text == "tool ok"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            CoreEvent::AgentTextDelta { text, .. } if text == "finished"
        )));
        assert!(matches!(
            events.last(),
            Some(CoreEvent::AgentDone { stop_reason, .. }) if stop_reason == "end_turn"
        ));
    }

    #[test]
    fn worker_cancel_at_approval_clears_pending_and_allows_next_turn() {
        let sink = RecordingSink::default();
        let llm = FakeLlm::new(vec![
            vec![
                LlmEvent::ToolUseStart {
                    id: "call_1".into(),
                    name: "shell".into(),
                },
                LlmEvent::ToolUseArgsDelta {
                    id: "call_1".into(),
                    json: r#"{"command":"echo hi"}"#.into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    index: 0,
                    text: "after cancel".into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                },
            ],
        ]);
        let (_sessions, handle) = spawn_test_worker(&sink, Box::new(llm));

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("run echo".into()))
            .expect("send");
        wait_for(|| {
            sink.events()
                .iter()
                .any(|event| matches!(event, CoreEvent::AgentApprovalRequired { .. }))
        });
        assert!(handle.busy.load(Ordering::Acquire));

        handle.request_cancel();
        handle.sender.send(WorkerCommand::Cancel).expect("cancel");
        wait_for(|| !handle.busy.load(Ordering::Acquire));
        assert!(!handle.cancel.load(Ordering::Acquire));
        assert!(matches!(
            sink.events().last(),
            Some(CoreEvent::AgentDone { stop_reason, .. }) if stop_reason == CANCELLED_STOP_REASON
        ));

        // 旧审批已随取消丢弃，下一条消息不需要先 resolve 即可运行。
        handle.begin_send().expect("next send");
        handle
            .sender
            .send(WorkerCommand::Send("next".into()))
            .expect("send next");
        wait_for(|| {
            sink.events().iter().any(|event| {
                matches!(
                    event,
                    CoreEvent::AgentTextDelta { text, .. } if text == "after cancel"
                )
            })
        });
        wait_for(|| !handle.busy.load(Ordering::Acquire));
        assert!(matches!(
            sink.events().last(),
            Some(CoreEvent::AgentDone { stop_reason, .. }) if stop_reason == "end_turn"
        ));
    }

    #[test]
    fn approval_decision_conversion_preserves_variants() {
        assert_eq!(
            approval_decision(ApprovalDecision::AllowOnce),
            CoreApprovalDecision::AllowOnce
        );
        assert_eq!(
            approval_decision(ApprovalDecision::AllowSession),
            CoreApprovalDecision::AllowSession
        );
        assert_eq!(
            approval_decision(ApprovalDecision::Deny),
            CoreApprovalDecision::Deny
        );
    }

    /// 契约字段回归：UI 直接读取这些名字，改名需要同步 bridge。
    #[test]
    fn session_info_and_approval_shapes_are_snake_case() {
        let info = SessionInfo {
            session_id: "s1".into(),
            mode: SessionMode::Auto,
            turn: 1,
            max_turns: 25,
            context_k: 400,
            compress_at_k: 300,
            cancelled: false,
        };
        let json = serde_json::to_value(&info).expect("serialize");
        assert_eq!(json["session_id"], "s1");
        assert_eq!(json["max_turns"], 25);
        assert_eq!(json["context_k"], 400);
        assert_eq!(json["compress_at_k"], 300);

        let prompt = lc_proto::ApprovalPrompt {
            approval_id: 3,
            tool: "shell".into(),
            command: Some("echo hi".into()),
            source: ApprovalSource::ExecuteReview,
            reason: "review".into(),
        };
        let json = serde_json::to_value(&prompt).expect("serialize");
        assert_eq!(json["approval_id"], 3);
        assert_eq!(json["source"], "execute_review");
    }

    // ---- 审批 → 任务状态联动 ----

    #[test]
    fn worker_approval_marks_blocked_then_review() {
        let sink = RecordingSink::default();
        let sessions = AgentSessions::default();
        let id = sessions.allocate_id();
        let fixture = task_fixture("approval-link", &id);
        let llm = FakeLlm::new(vec![
            vec![
                LlmEvent::ToolUseStart {
                    id: "call_1".into(),
                    name: "shell".into(),
                },
                LlmEvent::ToolUseArgsDelta {
                    id: "call_1".into(),
                    json: r#"{"command":"echo hi"}"#.into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    index: 0,
                    text: "finished".into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                },
            ],
        ]);
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = spawn_worker(
            &sessions,
            id.clone(),
            test_loop(&id, Box::new(llm)),
            Arc::new(sink.clone()),
            cancel,
            Some(fixture.binding()),
            None,
        )
        .expect("spawn task worker");

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("run echo".into()))
            .expect("send");
        wait_for(|| {
            sink.events()
                .iter()
                .any(|event| matches!(event, CoreEvent::AgentApprovalRequired { .. }))
        });

        // NeedsApproval：Blocked + needs_input 各一次；端口/worktree 保留。
        let blocked = fixture.task();
        assert_eq!(blocked.status, TaskStatus::Blocked);
        assert!(blocked.port.is_some(), "Blocked 不释放端口");
        assert!(blocked.worktree_path.is_some(), "Blocked 不释放 worktree");
        let events = sink.events();
        assert_eq!(needs_input_count(&events), 1, "{events:?}");
        assert!(events.iter().any(|event| matches!(
            event,
            CoreEvent::TaskNeedsInput { task_id, kind }
                if task_id == &fixture.task_id && kind == "approval"
        )));
        assert!(handle.busy.load(Ordering::Acquire), "审批期间保持 busy");

        let approval_id = events
            .iter()
            .find_map(|event| match event {
                CoreEvent::AgentApprovalRequired { approval, .. } => Some(approval.approval_id),
                _ => None,
            })
            .expect("approval id");
        handle
            .sender
            .send(WorkerCommand::ResolveApproval {
                approval_id,
                decision: CoreApprovalDecision::AllowOnce,
            })
            .expect("resolve");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        // resolve：Blocked → Running → Review；needs_input 不重复。
        assert_eq!(fixture.status(), TaskStatus::Review);
        let events = sink.events();
        assert_eq!(
            task_updated_statuses(&events),
            vec![TaskStatus::Running, TaskStatus::Review],
            "{events:?}"
        );
        assert_eq!(needs_input_count(&events), 1, "{events:?}");
    }

    #[test]
    fn worker_loop_error_marks_task_failed() {
        struct FailingLlm;

        impl LlmStream for FailingLlm {
            fn stream(
                &self,
                _request: &LlmRequest,
                _sink: &mut dyn FnMut(LlmEvent),
            ) -> Result<(), LlmError> {
                Err(LlmError::Http("boom".into()))
            }
        }

        let sink = RecordingSink::default();
        let sessions = AgentSessions::default();
        let id = sessions.allocate_id();
        let fixture = task_fixture("error-link", &id);
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = spawn_worker(
            &sessions,
            id.clone(),
            test_loop(&id, Box::new(FailingLlm)),
            Arc::new(sink.clone()),
            cancel,
            Some(fixture.binding()),
            None,
        )
        .expect("spawn task worker");

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("hello".into()))
            .expect("send");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        assert_eq!(fixture.status(), TaskStatus::Failed);
        let events = sink.events();
        let updated = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    CoreEvent::TaskUpdated { task } if task.status == TaskStatus::Failed
                )
            })
            .unwrap_or_else(|| panic!("missing failed task:updated: {events:?}"));
        let error = events
            .iter()
            .position(|event| matches!(event, CoreEvent::Error { .. }))
            .unwrap_or_else(|| panic!("missing agent error: {events:?}"));
        assert!(updated < error, "task snapshot precedes agent error");
    }

    #[test]
    fn worker_cancel_marks_task_failed() {
        struct CancelDuringStream {
            cancel: Arc<AtomicBool>,
        }

        impl LlmStream for CancelDuringStream {
            fn stream(
                &self,
                _request: &LlmRequest,
                sink: &mut dyn FnMut(LlmEvent),
            ) -> Result<(), LlmError> {
                sink(LlmEvent::TextDelta {
                    index: 0,
                    text: "first".into(),
                });
                self.cancel.store(true, Ordering::Release);
                sink(LlmEvent::TextDelta {
                    index: 0,
                    text: "hidden".into(),
                });
                Ok(())
            }
        }

        let sink = RecordingSink::default();
        let sessions = AgentSessions::default();
        let id = sessions.allocate_id();
        let fixture = task_fixture("cancel-link", &id);
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = spawn_worker(
            &sessions,
            id.clone(),
            test_loop(
                &id,
                Box::new(CancelDuringStream {
                    cancel: cancel.clone(),
                }),
            ),
            Arc::new(sink.clone()),
            cancel,
            Some(fixture.binding()),
            None,
        )
        .expect("spawn task worker");

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("hello".into()))
            .expect("send");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        assert_eq!(fixture.status(), TaskStatus::Failed);
        let events = sink.events();
        assert!(
            events.iter().any(|event| matches!(
                event,
                CoreEvent::TaskUpdated { task } if task.status == TaskStatus::Failed
            )),
            "{events:?}"
        );
        assert!(matches!(
            events.last(),
            Some(CoreEvent::AgentDone { stop_reason, .. }) if stop_reason == "cancelled"
        ));
    }

    #[test]
    fn task_link_events_have_contract_shape() {
        // M3a：seq 1 基（Seq::from_u64(9) 的下一个为 10；原 0 基 fetch_add 的
        // 9/10 对应现在的 10/11）。
        let seq = lc_host::envelope::Seq::from_u64(9);
        let needs = lc_host::envelope::envelope(
            &seq,
            CoreEvent::TaskNeedsInput {
                task_id: "task-1".into(),
                kind: "approval".into(),
            },
        );
        let json = serde_json::to_value(&needs).expect("serialize needs_input");
        assert_eq!(json["seq"], 10);
        assert_eq!(json["event"]["type"], "task:needs_input");
        assert_eq!(json["event"]["task_id"], "task-1");
        assert_eq!(json["event"]["kind"], "approval");

        let updated = lc_host::envelope::envelope(
            &seq,
            CoreEvent::TaskUpdated {
                task: TaskInfo {
                    task_id: "task-1".into(),
                    title: "approval link".into(),
                    session_id: Some("s-1".into()),
                    status: TaskStatus::Review,
                    worktree_path: Some("/tmp/wt".into()),
                    branch: Some("lc/task-1".into()),
                    port: Some(4300),
                    base_sha: None,
                    steps: Vec::new(),
                    changes: Vec::new(),
                    checks: Vec::new(),
                    created_at: 1,
                },
            },
        );
        let json = serde_json::to_value(&updated).expect("serialize updated");
        assert_eq!(json["seq"], 11);
        assert_eq!(json["event"]["type"], "task:updated");
        assert_eq!(json["event"]["task"]["task_id"], "task-1");
        assert_eq!(json["event"]["task"]["status"], "review");
        assert_eq!(json["event"]["task"]["session_id"], "s-1");
    }

    #[test]
    fn reasoning_effort_maps_thinking_levels() {
        let supported = ["off", "low", "medium", "high", "max"].map(String::from);
        assert_eq!(reasoning_effort(None, &supported).expect("none"), None);
        assert_eq!(
            reasoning_effort(Some("off"), &supported).expect("off"),
            None
        );
        assert_eq!(
            reasoning_effort(Some("low"), &supported).expect("low"),
            Some("low".into())
        );
        assert_eq!(
            reasoning_effort(Some("medium"), &supported).expect("medium"),
            Some("medium".into())
        );
        assert_eq!(
            reasoning_effort(Some("high"), &supported).expect("high"),
            Some("high".into())
        );
        assert_eq!(
            reasoning_effort(Some("max"), &supported).expect("max"),
            Some("high".into())
        );
    }

    #[test]
    fn reasoning_effort_rejects_unsupported_level() {
        let supported = ["off", "high"].map(String::from);
        let err = reasoning_effort(Some("low"), &supported).expect_err("unsupported");
        assert_eq!(err.code, lc_proto::ErrorCode::InvalidArgument);
    }

    // ---- 插件级 Agent 工具装配（docs/EXTENSIONS.md §4）----

    use lc_core::ext::{ExtToolExecutor, PluginTool, PluginToolRunner};

    struct FakePluginRunner {
        results:
            std::collections::HashMap<String, Result<serde_json::Value, lc_core::ext::ExtError>>,
        calls: Arc<Mutex<Vec<(String, String, serde_json::Value)>>>,
    }

    impl FakePluginRunner {
        fn new(results: Vec<(&str, Result<serde_json::Value, lc_core::ext::ExtError>)>) -> Self {
            Self {
                results: results
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl PluginToolRunner for FakePluginRunner {
        fn run_tool(
            &self,
            plugin_id: &str,
            tool: &str,
            args: serde_json::Value,
        ) -> Result<serde_json::Value, lc_core::ext::ExtError> {
            self.calls
                .lock()
                .expect("calls")
                .push((plugin_id.to_string(), tool.to_string(), args));
            self.results.get(tool).cloned().unwrap_or_else(|| {
                Err(lc_core::ext::ExtError::ToolNotFound {
                    plugin: plugin_id.to_string(),
                    tool: tool.to_string(),
                })
            })
        }
    }

    fn sample_plugin_tool() -> PluginTool {
        PluginTool {
            plugin_id: "example.desktop".into(),
            name: "example.search".into(),
            description: "搜索工作区文本".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
            command: "example.search".into(),
        }
    }

    #[test]
    fn plugin_tool_layer_registers_mutating_tools_and_routes() {
        let runner = Arc::new(FakePluginRunner::new(vec![(
            "example.search",
            Ok(serde_json::Value::String("hit".into())),
        )]));
        let base: Arc<dyn ToolExecutor> = Arc::new(FakeExecutor);
        let layer =
            build_plugin_tool_layer(vec![sample_plugin_tool()], runner.clone(), base.clone());

        // spec 进注册表：一律 Risk::Mutating；LLM 定义透传清单 description/parameters。
        let registry = ToolRegistry::with_builtins()
            .with_mcp_tools([])
            .with_plugin_tools(layer.specs.clone());
        assert_eq!(
            registry.get("example.search").map(|spec| spec.risk),
            Some(Risk::Mutating)
        );
        assert_eq!(layer.llm_tools.len(), 1);
        assert_eq!(layer.llm_tools[0]["name"], "example.search");
        assert_eq!(layer.llm_tools[0]["description"], "搜索工作区文本");
        assert_eq!(
            layer.llm_tools[0]["parameters"]["required"],
            serde_json::json!(["query"])
        );

        // 执行器路由到插件通道，未命中回退到原执行器。
        let output = layer
            .executor
            .execute(&ToolCall::new(
                "example.search",
                serde_json::json!({"query": "x"}),
            ))
            .expect("plugin tool");
        assert!(output.text.contains("hit"), "{}", output.text);
        assert!(
            output.text.starts_with("[UNTRUSTED PLUGIN CONTENT"),
            "{}",
            output.text
        );
        let output = layer
            .executor
            .execute(&ToolCall::new(
                "read_file",
                serde_json::json!({"path": "a"}),
            ))
            .expect("fallback");
        assert_eq!(output.text, "tool ok");
        assert_eq!(
            runner.calls.lock().expect("calls").len(),
            1,
            "只有插件工具走插件通道"
        );

        // 空工具列表：原样返回，不包插件层。
        let layer = build_plugin_tool_layer(vec![], runner.clone(), base.clone());
        assert!(layer.specs.is_empty());
        assert!(layer.llm_tools.is_empty());
        let output = layer
            .executor
            .execute(&ToolCall::new("read_file", serde_json::json!({})))
            .expect("pass through");
        assert_eq!(output.text, "tool ok");
    }

    #[test]
    fn worker_runs_plugin_tool_after_approval() {
        let sink = RecordingSink::default();
        let sessions = AgentSessions::default();
        let id = sessions.allocate_id();
        let llm = FakeLlm::new(vec![
            vec![
                LlmEvent::ToolUseStart {
                    id: "call_1".into(),
                    name: "example.search".into(),
                },
                LlmEvent::ToolUseArgsDelta {
                    id: "call_1".into(),
                    json: r#"{"query":"hi"}"#.into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    index: 0,
                    text: "finished".into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                },
            ],
        ]);
        let runner = Arc::new(FakePluginRunner::new(vec![(
            "example.search",
            Ok(serde_json::Value::String("result-from-plugin".into())),
        )]));
        let base: Arc<dyn ToolExecutor> = Arc::new(FakeExecutor);
        let plugin_executor =
            ExtToolExecutor::new(runner.clone(), &[sample_plugin_tool()]).with_fallback(base);
        let registry = ToolRegistry::with_builtins()
            .with_mcp_tools([])
            .with_plugin_tools(plugin_executor.tool_specs());
        let cancel = Arc::new(AtomicBool::new(false));
        let agent = AgentLoop::new(
            Session::new(id.clone(), PolicyMode::Auto),
            Box::new(llm),
            Box::new(plugin_executor),
            registry,
            std::env::temp_dir(),
            "test-model",
        );
        let handle = spawn_worker(
            &sessions,
            id,
            agent,
            Arc::new(sink.clone()),
            cancel,
            None,
            None,
        )
        .expect("spawn");

        handle.begin_send().expect("begin");
        handle
            .sender
            .send(WorkerCommand::Send("搜一下".into()))
            .expect("send");

        // 插件工具 = Mutating：auto 模式先挂审批，runner 尚未执行。
        wait_for(|| {
            sink.events()
                .iter()
                .any(|event| matches!(event, CoreEvent::AgentApprovalRequired { .. }))
        });
        assert!(runner.calls.lock().expect("calls").is_empty());
        let approval = sink
            .events()
            .iter()
            .find_map(|event| match event {
                CoreEvent::AgentApprovalRequired { approval, .. } => Some(approval.clone()),
                _ => None,
            })
            .expect("approval");
        assert_eq!(approval.tool, "example.search");
        assert_eq!(approval.source, ApprovalSource::NetworkTool);
        assert!(approval.reason.contains("插件"), "{}", approval.reason);

        handle
            .sender
            .send(WorkerCommand::ResolveApproval {
                approval_id: approval.approval_id,
                decision: CoreApprovalDecision::AllowOnce,
            })
            .expect("resolve");
        wait_for(|| !handle.busy.load(Ordering::Acquire));

        let events = sink.events();
        assert!(events.iter().any(|event| matches!(
            event,
            CoreEvent::AgentToolResult { ok: true, text, .. }
                if text.contains("result-from-plugin") && text.starts_with("[UNTRUSTED PLUGIN CONTENT")
        )));
        assert_eq!(
            runner.calls.lock().expect("calls").len(),
            1,
            "审批允许后恰好执行一次"
        );
        assert!(matches!(
            events.last(),
            Some(CoreEvent::AgentDone { stop_reason, .. }) if stop_reason == "end_turn"
        ));
    }
}
