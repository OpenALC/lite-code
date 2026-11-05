//! P4 Task 命令层（docs/TASKS.md §4、§7）。
//!
//! 业务逻辑全部在 `lc_core::task::TaskStore`：本模块只负责三件事——
//! `Mutex` 包装（`TaskState`）、把状态机错误映射成契约 `ErrorBody`、把每次
//! 状态迁移转成 `task:*` 事件。事件走与 agent 相同的 `core:event` 单通道，
//! 载荷是契约 [`EventEnvelope`]；seq 由本模块自己的 [`TaskEvents`] 分配，
//! 与 agent 的序列互不影响（UI 不依赖跨 lane 的连续性）。
//!
//! 锁约定：`TaskState` 只在单次命令内短暂持有。`session_create` 先取任务根
//! 与状态快照，释放锁后再做 provider/MCP/AgentLoop 等重操作，最后重新加锁
//! 完成绑定与启动。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lc_core::task::{TaskDiff, TaskError, TaskStore};
use lc_proto::{
    CommandResult, CoreEvent, ErrorBody, ErrorCode, EventEnvelope, TaskInfo, TaskStatus,
};
use tauri::Emitter;

use crate::WorkspaceState;
use crate::agent::{CORE_EVENT_CHANNEL, EventSink};

const TASK_STATE_UNAVAILABLE: &str = "任务存储不可用（锁中毒），请重启应用";

// ==== 应用级状态 ====

/// 任务存储的应用级状态。首次命令按当前 workspace 根惰性加载；根切换
/// （`workspace_open`）时显式 [`TaskState::reset`]，下一次命令重新加载。
///
/// 内部是 `Arc<Mutex<..>>`，因此可以克隆给会话 worker：worker 只在状态迁移
/// 时短暂加锁，不做 LLM/网络/磁盘大操作。
#[derive(Default, Clone)]
pub(crate) struct TaskState {
    inner: Arc<Mutex<TaskStoreSlot>>,
}

#[derive(Default)]
struct TaskStoreSlot {
    root: Option<PathBuf>,
    store: Option<TaskStore>,
}

impl TaskState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 切换 workspace 后丢弃内存中的 store；旧 `tasks.json` 保留在磁盘，
    /// 新根会重新加载（而不是继续使用旧根的 repo_root）。
    pub(crate) fn reset(&self) {
        if let Ok(mut slot) = self.inner.lock() {
            *slot = TaskStoreSlot::default();
        }
    }

    fn with_store<T>(
        &self,
        root: &Path,
        action: impl FnOnce(&mut TaskStore) -> Result<T, ErrorBody>,
    ) -> Result<T, ErrorBody> {
        let mut slot = self.lock()?;
        let store = slot.ensure_with(root, |root| TaskStore::load_default(root))?;
        action(store)
    }

    /// 不要求 workspace 根：worker 在会话创建时已通过 `with_store`
    /// 初始化 store，后续状态迁移只复用同一实例（也保证与 `session_create`
    /// 的绑定/启动写入同一份内存状态）。
    pub(crate) fn with_current<T>(
        &self,
        action: impl FnOnce(&mut TaskStore) -> Result<T, TaskError>,
    ) -> Result<T, ErrorBody> {
        let mut slot = self.lock()?;
        let store = slot.store.as_mut().ok_or_else(|| {
            ErrorBody::new(
                ErrorCode::Internal,
                "任务存储尚未初始化：请先创建或打开任务",
            )
        })?;
        action(store).map_err(|error| error.to_error_body())
    }

    fn mark_status(
        &self,
        task_id: impl AsRef<str>,
        status: TaskStatus,
    ) -> Result<TaskInfo, ErrorBody> {
        self.with_current(move |store| store.set_status(task_id, status))
    }

    /// 审批挂起：进入 `Blocked`，端口与 worktree 保持不变。
    pub(crate) fn mark_needs_input(&self, task_id: impl AsRef<str>) -> Result<TaskInfo, ErrorBody> {
        self.mark_status(task_id, TaskStatus::Blocked)
    }

    pub(crate) fn mark_running(&self, task_id: impl AsRef<str>) -> Result<TaskInfo, ErrorBody> {
        self.mark_status(task_id, TaskStatus::Running)
    }

    pub(crate) fn mark_review(&self, task_id: impl AsRef<str>) -> Result<TaskInfo, ErrorBody> {
        self.mark_status(task_id, TaskStatus::Review)
    }

    pub(crate) fn mark_failed(&self, task_id: impl AsRef<str>) -> Result<TaskInfo, ErrorBody> {
        self.mark_status(task_id, TaskStatus::Failed)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, TaskStoreSlot>, ErrorBody> {
        self.inner
            .lock()
            .map_err(|_| ErrorBody::new(ErrorCode::Internal, TASK_STATE_UNAVAILABLE))
    }

    /// 测试入口：直接注入已构造的 store（生产路径走 `with_store` +
    /// `TaskStore::load_default`，避免测试读写用户的真实数据目录）。
    #[cfg(test)]
    pub(crate) fn init_for_test(&self, store: TaskStore) {
        let mut slot = self.lock().expect("task state lock");
        slot.root = Some(store.repo_root().to_path_buf());
        slot.store = Some(store);
    }
}

impl TaskStoreSlot {
    /// 根变化即丢弃旧 store；加载失败时不缓存半成品，下次命令会重试。
    fn ensure_with(
        &mut self,
        root: &Path,
        load: impl FnOnce(&Path) -> Result<TaskStore, TaskError>,
    ) -> Result<&mut TaskStore, ErrorBody> {
        if self.root.as_deref() != Some(root) {
            self.store = None;
            self.root = Some(root.to_path_buf());
        }
        if self.store.is_none() {
            self.store = Some(load(root).map_err(|error| error.to_error_body())?);
        }
        Ok(self.store.as_mut().expect("store initialized above"))
    }
}

/// task lane 的事件序号。`TaskEvents` 可克隆（内部共享同一计数器），
/// 与 agent 的 seq 相互独立。
#[derive(Default, Clone)]
pub(crate) struct TaskEvents {
    seq: Arc<AtomicU64>,
}

impl TaskEvents {
    fn next_envelope(&self, event: CoreEvent) -> EventEnvelope {
        EventEnvelope {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            event,
        }
    }

    pub(crate) fn sink(&self, app: tauri::AppHandle) -> Arc<dyn EventSink> {
        Arc::new(TaskEventSink {
            app,
            events: self.clone(),
        })
    }
}

struct TaskEventSink {
    app: tauri::AppHandle,
    events: TaskEvents,
}

impl EventSink for TaskEventSink {
    fn emit(&self, event: CoreEvent) {
        // 窗口销毁/重建期间 emit 可能失败；事件流不是命令通道，忽略即可。
        let _ = self
            .app
            .emit(CORE_EVENT_CHANNEL, self.events.next_envelope(event));
    }
}

// ==== Tauri 命令 ====

fn workspace_root(workspace: &tauri::State<'_, WorkspaceState>) -> Result<PathBuf, ErrorBody> {
    workspace.get().ok_or_else(crate::workspace_unavailable)
}

/// 任务列表：按创建时间（同一毫秒再按 task_id）排序，供工作台首屏加载。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn task_list(
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, TaskState>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    let tasks = state.with_store(&root, |store| -> Result<_, ErrorBody> {
        Ok(sorted_tasks(store))
    })?;
    Ok(CommandResult::Tasks { tasks })
}

/// 创建任务：git 项目同时创建 worktree；事件为 `task:created` + `task:updated`。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn task_create(
    title: String,
    session_id: Option<String>,
    app: tauri::AppHandle,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, TaskState>,
    events: tauri::State<'_, TaskEvents>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    let sink = events.sink(app);
    let task = state.with_store(&root, |store| {
        create_task(store, sink.as_ref(), &title, session_id)
    })?;
    Ok(CommandResult::Task { task })
}

/// 启动任务：并发槽满时任务进入 `Queued`（仍返回任务快照，不报错）。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn task_start(
    task_id: String,
    app: tauri::AppHandle,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, TaskState>,
    events: tauri::State<'_, TaskEvents>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    let sink = events.sink(app);
    let task = state.with_store(&root, |store| start_task(store, sink.as_ref(), &task_id))?;
    Ok(CommandResult::Task { task })
}

/// 取消任务：worktree 保留供 Review；随后按创建顺序补满并发槽。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn task_cancel(
    task_id: String,
    app: tauri::AppHandle,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, TaskState>,
    events: tauri::State<'_, TaskEvents>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    let sink = events.sink(app);
    let task = state.with_store(&root, |store| cancel_task(store, sink.as_ref(), &task_id))?;
    Ok(CommandResult::Task { task })
}

/// 应用任务：合并成功返回提交后的 TaskInfo，并下发 `task:merged`。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn task_apply(
    task_id: String,
    app: tauri::AppHandle,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, TaskState>,
    events: tauri::State<'_, TaskEvents>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    let sink = events.sink(app);
    let task = state.with_store(&root, |store| apply_task(store, sink.as_ref(), &task_id))?;
    Ok(CommandResult::Task { task })
}

/// 丢弃任务：worktree 有未提交改动时由 TaskStore 拒绝（DirtyWorktree）。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn task_discard(
    task_id: String,
    app: tauri::AppHandle,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, TaskState>,
    events: tauri::State<'_, TaskEvents>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    let sink = events.sink(app);
    state.with_store(&root, |store| discard_task(store, sink.as_ref(), &task_id))?;
    Ok(CommandResult::Ack)
}

/// 读取任务 diff（同时刷新 changes）。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn task_diff(
    task_id: String,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, TaskState>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    let diff: TaskDiff = state.with_store(&root, |store| {
        store.diff(&task_id).map_err(|error| error.to_error_body())
    })?;
    Ok(CommandResult::TaskDiff {
        task_id: diff.task_id,
        patch: diff.patch,
        files: diff.files,
    })
}

/// 绑定/解绑 Agent 会话（`session_id = None` 即解绑）。
#[tauri::command(rename_all = "snake_case")]
pub(crate) fn task_bind_session(
    task_id: String,
    session_id: Option<String>,
    app: tauri::AppHandle,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, TaskState>,
    events: tauri::State<'_, TaskEvents>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    let sink = events.sink(app);
    let task = state.with_store(&root, |store| {
        bind_task_session(store, sink.as_ref(), &task_id, session_id)
    })?;
    Ok(CommandResult::Task { task })
}

// ==== 命令纯逻辑（单测直接覆盖，不启动 Tauri 运行时） ====

fn sorted_tasks(store: &TaskStore) -> Vec<TaskInfo> {
    let mut tasks = store.list();
    tasks.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
    tasks
}

fn not_found(task_id: &str) -> ErrorBody {
    TaskError::NotFound {
        task_id: task_id.to_string(),
    }
    .to_error_body()
}

fn create_task(
    store: &mut TaskStore,
    sink: &dyn EventSink,
    title: &str,
    session_id: Option<String>,
) -> Result<TaskInfo, ErrorBody> {
    let task = store
        .create(title, session_id)
        .map_err(|error| error.to_error_body())?;
    sink.emit(CoreEvent::TaskCreated {
        task_id: task.task_id.clone(),
        title: task.title.clone(),
    });
    emit_updated(sink, &task);
    Ok(task)
}

fn start_task(
    store: &mut TaskStore,
    sink: &dyn EventSink,
    task_id: &str,
) -> Result<TaskInfo, ErrorBody> {
    let task = store
        .start(task_id)
        .map_err(|error| error.to_error_body())?;
    emit_updated(sink, &task);
    Ok(task)
}

fn cancel_task(
    store: &mut TaskStore,
    sink: &dyn EventSink,
    task_id: &str,
) -> Result<TaskInfo, ErrorBody> {
    let task = store
        .cancel(task_id)
        .map_err(|error| error.to_error_body())?;
    emit_updated(sink, &task);
    promote_queued(store, sink);
    Ok(task)
}

fn apply_task(
    store: &mut TaskStore,
    sink: &dyn EventSink,
    task_id: &str,
) -> Result<TaskInfo, ErrorBody> {
    let outcome = match store.apply(task_id) {
        Ok(outcome) => outcome,
        Err(error) => {
            // 合并冲突会把任务改成 Review；命令虽然失败，UI 仍需要新快照。
            if let Some(task) = store.get(task_id) {
                emit_updated(sink, &task);
            }
            return Err(error.to_error_body());
        }
    };
    let task = store.get(task_id).ok_or_else(|| not_found(task_id))?;
    emit_updated(sink, &task);
    sink.emit(CoreEvent::TaskMerged {
        task_id: task.task_id.clone(),
        commit_sha: outcome.commit,
    });
    promote_queued(store, sink);
    Ok(task)
}

fn discard_task(
    store: &mut TaskStore,
    sink: &dyn EventSink,
    task_id: &str,
) -> Result<TaskInfo, ErrorBody> {
    let task = store
        .discard(task_id)
        .map_err(|error| error.to_error_body())?;
    emit_updated(sink, &task);
    sink.emit(CoreEvent::TaskDiscarded {
        task_id: task.task_id.clone(),
    });
    promote_queued(store, sink);
    Ok(task)
}

fn bind_task_session(
    store: &mut TaskStore,
    sink: &dyn EventSink,
    task_id: &str,
    session_id: Option<String>,
) -> Result<TaskInfo, ErrorBody> {
    let task = store
        .bind_session(task_id, session_id)
        .map_err(|error| error.to_error_body())?;
    emit_updated(sink, &task);
    Ok(task)
}

fn emit_updated(sink: &dyn EventSink, task: &TaskInfo) {
    sink.emit(CoreEvent::TaskUpdated { task: task.clone() });
}

/// `promote_queued` 每次只启动队首一个；循环调用直到并发槽填满。
/// 提升失败不改变已成功命令的结果，只记录日志（任务仍保留在 Queued）。
fn promote_queued(store: &mut TaskStore, sink: &dyn EventSink) {
    loop {
        match store.promote_queued() {
            Ok(Some(task)) => emit_updated(sink, &task),
            Ok(None) => break,
            Err(error) => {
                eprintln!("[task] 队列提升失败：{error}");
                break;
            }
        }
    }
}

// ==== session_create 的任务根与绑定 ====

/// `session_create` 需要的工作区信息：AgentLoop/MCP/内存的根，以及任务快照。
pub(crate) struct SessionTaskTarget {
    pub root: PathBuf,
    pub task: TaskInfo,
}

/// 任务在本次会话中的绑定结果。`started` 表示状态迁移是本命令触发的，
/// 会话创建失败时只回滚这类任务，避免误取消原本就在运行的任务。
#[derive(Debug)]
pub(crate) struct BoundTask {
    pub task: TaskInfo,
    pub started: bool,
}

/// 读取任务快照并解析会话根：有 worktree 用 worktree，无 worktree（非 git）
/// 回退 workspace 根。未知任务返回 NotFound。
pub(crate) fn session_target(
    state: &TaskState,
    workspace_root: &Path,
    task_id: &str,
) -> Result<SessionTaskTarget, ErrorBody> {
    state.with_store(workspace_root, |store| {
        let root = resolve_session_root(store, workspace_root, Some(task_id))?;
        let task = store.get(task_id).ok_or_else(|| not_found(task_id))?;
        ensure_startable(&task)?;
        Ok(SessionTaskTarget { root, task })
    })
}

/// 绑定会话并把任务置 Running：未启动的先 `start`；并发槽满时任务保持
/// Queued 且不写入 session_id（该会话尚未创建），返回 InvalidArgument。
pub(crate) fn bind_session_to_task(
    state: &TaskState,
    workspace_root: &Path,
    task_id: &str,
    session_id: &str,
) -> Result<BoundTask, ErrorBody> {
    state.with_store(workspace_root, |store| {
        bind_session_in_store(store, task_id, session_id)
    })
}

fn bind_session_in_store(
    store: &mut TaskStore,
    task_id: &str,
    session_id: &str,
) -> Result<BoundTask, ErrorBody> {
    let current = store.get(task_id).ok_or_else(|| not_found(task_id))?;
    ensure_startable(&current)?;

    if current.status == TaskStatus::Running {
        let task = store
            .bind_session(task_id, Some(session_id.to_string()))
            .map_err(|error| error.to_error_body())?;
        return Ok(BoundTask {
            task,
            started: false,
        });
    }

    let task = store
        .start(task_id)
        .map_err(|error| error.to_error_body())?;
    if task.status != TaskStatus::Running {
        // 队列满：任务保持 Queued，且不绑定这个尚未创建成功的会话。
        return Err(ErrorBody::new(
            ErrorCode::InvalidArgument,
            format!("并发槽已满，任务 {task_id} 已排队：请先取消或应用其他任务后重试"),
        ));
    }
    let task = store
        .bind_session(task_id, Some(session_id.to_string()))
        .map_err(|error| error.to_error_body())?;
    Ok(BoundTask {
        task,
        started: true,
    })
}

/// 会话创建失败时回滚本命令启动的任务：worker 线程没起来，不能留 Running，
/// 也不能让任务继续指向一个不存在的会话。
pub(crate) fn cancel_task_by_id(
    state: &TaskState,
    workspace_root: &Path,
    task_id: &str,
    sink: &dyn EventSink,
) -> Result<TaskInfo, ErrorBody> {
    state.with_store(workspace_root, |store| {
        store
            .cancel(task_id)
            .map_err(|error| error.to_error_body())?;
        let task = store
            .bind_session(task_id, None)
            .map_err(|error| error.to_error_body())?;
        emit_updated(sink, &task);
        promote_queued(store, sink);
        Ok(task)
    })
}

/// 有 worktree 用 worktree 绝对路径；非 git 任务回退 workspace 根。
/// `task_id = None` 时直接返回 workspace 根。
fn resolve_session_root(
    store: &TaskStore,
    workspace_root: &Path,
    task_id: Option<&str>,
) -> Result<PathBuf, ErrorBody> {
    let Some(task_id) = task_id else {
        return Ok(workspace_root.to_path_buf());
    };
    let task = store.get(task_id).ok_or_else(|| not_found(task_id))?;
    match task.worktree_path.as_deref() {
        Some(path) => {
            let worktree = PathBuf::from(path);
            if !worktree.is_dir() {
                return Err(TaskError::WorktreeMissing {
                    task_id: task.task_id,
                    path: path.to_string(),
                }
                .to_error_body());
            }
            Ok(worktree)
        }
        None => Ok(workspace_root.to_path_buf()),
    }
}

fn ensure_startable(task: &TaskInfo) -> Result<(), ErrorBody> {
    if matches!(
        task.status,
        TaskStatus::Planning | TaskStatus::Queued | TaskStatus::Running
    ) {
        Ok(())
    } else {
        Err(TaskError::InvalidState {
            task_id: task.task_id.clone(),
            status: task.status,
            action: "session_create",
            expected: "planning, queued or running",
        }
        .to_error_body())
    }
}

// ==== 单测：不启动 Tauri 运行时，记录型 sink + 临时 git 仓库 ====

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::sync::Mutex;

    use lc_proto::{ChangeStatus, FileChangeDto, TaskCheck, TaskStepState};

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

        fn clear(&self) {
            self.events.lock().expect("recording sink").clear();
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lc-desktop-task-{}-{}-{}",
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
        run_git(dir, &["config", "user.email", "desktop-task@example.com"]);
        run_git(dir, &["config", "user.name", "Desktop Task Test"]);
        run_git(dir, &["config", "core.autocrlf", "false"]);
        run_git(dir, &["config", "commit.gpgsign", "false"]);
        fs::write(dir.join("README.md"), "hello\n").expect("write README");
        run_git(dir, &["add", "-A"]);
        run_git(dir, &["commit", "-q", "-m", "init"]);
    }

    fn store_for(base: &Path, repo: &Path, max_running: usize) -> TaskStore {
        TaskStore::with_max_running(base.join("data").join("tasks.json"), repo, max_running)
    }

    #[test]
    fn resolve_session_root_uses_task_worktree() {
        let base = temp_root("root-worktree");
        let repo = base.join("repo");
        init_repo(&repo);
        let mut store = store_for(&base, &repo, 2);
        let task = store.create("use worktree", None).expect("create");
        let worktree = task.worktree_path.clone().expect("worktree");

        let root = resolve_session_root(&store, &repo, Some(&task.task_id)).expect("resolve root");
        assert_eq!(root, PathBuf::from(&worktree));
        assert!(root.is_dir());
    }

    #[test]
    fn resolve_session_root_falls_back_without_worktree() {
        let base = temp_root("root-fallback");
        let plain = base.join("plain");
        fs::create_dir_all(&plain).expect("plain dir");
        let mut store = store_for(&base, &plain, 2);
        let task = store.create("serial task", None).expect("create");
        assert!(task.worktree_path.is_none());

        let root = resolve_session_root(&store, &plain, Some(&task.task_id)).expect("resolve root");
        assert_eq!(root, plain);
    }

    #[test]
    fn resolve_session_root_unknown_task_is_not_found() {
        let base = temp_root("root-unknown");
        let plain = base.join("plain");
        fs::create_dir_all(&plain).expect("plain dir");
        let store = store_for(&base, &plain, 2);

        let error =
            resolve_session_root(&store, &plain, Some("task-ghost")).expect_err("unknown task");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains("task-ghost"), "{}", error.message);
    }

    #[test]
    fn resolve_session_root_none_returns_workspace() {
        let base = temp_root("root-none");
        let plain = base.join("plain");
        fs::create_dir_all(&plain).expect("plain dir");
        let store = store_for(&base, &plain, 2);

        let root = resolve_session_root(&store, &plain, None).expect("resolve root");
        assert_eq!(root, plain);
    }

    #[test]
    fn create_task_emits_created_then_updated() {
        let base = temp_root("create-events");
        let repo = base.join("repo");
        init_repo(&repo);
        let mut store = store_for(&base, &repo, 2);
        let sink = RecordingSink::default();

        let task =
            create_task(&mut store, &sink, "fix parser", Some("s-1".into())).expect("create task");
        let events = sink.events();
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(matches!(
            &events[0],
            CoreEvent::TaskCreated { task_id, title }
                if task_id == &task.task_id && title == "fix parser"
        ));
        assert!(matches!(
            &events[1],
            CoreEvent::TaskUpdated { task: updated }
                if updated.task_id == task.task_id
                    && updated.status == TaskStatus::Planning
                    && updated.session_id.as_deref() == Some("s-1")
        ));
    }

    #[test]
    fn apply_emits_merged_and_promotes_queued() {
        let base = temp_root("apply-events");
        let repo = base.join("repo");
        init_repo(&repo);
        let mut store = store_for(&base, &repo, 1);
        let sink = RecordingSink::default();

        let first = create_task(&mut store, &sink, "first", None).expect("create first");
        let second = create_task(&mut store, &sink, "second", None).expect("create second");
        start_task(&mut store, &sink, &first.task_id).expect("start first");
        let queued = start_task(&mut store, &sink, &second.task_id).expect("start second");
        assert_eq!(queued.status, TaskStatus::Queued);
        sink.clear();

        let applied = apply_task(&mut store, &sink, &first.task_id).expect("apply first");
        assert_eq!(applied.status, TaskStatus::Done);
        assert!(applied.worktree_path.is_none());

        let events = sink.events();
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(matches!(
            &events[0],
            CoreEvent::TaskUpdated { task }
                if task.task_id == first.task_id && task.status == TaskStatus::Done
        ));
        assert!(matches!(
            &events[1],
            CoreEvent::TaskMerged { task_id, commit_sha }
                if task_id == &first.task_id && !commit_sha.is_empty()
        ));
        assert!(matches!(
            &events[2],
            CoreEvent::TaskUpdated { task }
                if task.task_id == second.task_id && task.status == TaskStatus::Running
        ));
        assert_eq!(
            store.get(&second.task_id).expect("second").status,
            TaskStatus::Running
        );
    }

    #[test]
    fn discard_emits_discarded_and_promotes_queued() {
        let base = temp_root("discard-events");
        let repo = base.join("repo");
        init_repo(&repo);
        let mut store = store_for(&base, &repo, 1);
        let sink = RecordingSink::default();

        let first = create_task(&mut store, &sink, "first", None).expect("create first");
        let second = create_task(&mut store, &sink, "second", None).expect("create second");
        start_task(&mut store, &sink, &first.task_id).expect("start first");
        let queued = start_task(&mut store, &sink, &second.task_id).expect("start second");
        assert_eq!(queued.status, TaskStatus::Queued);
        sink.clear();

        let discarded = discard_task(&mut store, &sink, &first.task_id).expect("discard first");
        assert_eq!(discarded.status, TaskStatus::Failed);
        assert!(discarded.worktree_path.is_none());

        let events = sink.events();
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(matches!(
            &events[0],
            CoreEvent::TaskUpdated { task }
                if task.task_id == first.task_id && task.status == TaskStatus::Failed
        ));
        assert!(matches!(
            &events[1],
            CoreEvent::TaskDiscarded { task_id } if task_id == &first.task_id
        ));
        assert!(matches!(
            &events[2],
            CoreEvent::TaskUpdated { task }
                if task.task_id == second.task_id && task.status == TaskStatus::Running
        ));
        assert_eq!(
            store.get(&second.task_id).expect("second").status,
            TaskStatus::Running
        );
    }

    #[test]
    fn cancel_emits_updated_and_promotes_queued() {
        let base = temp_root("cancel-events");
        let repo = base.join("repo");
        init_repo(&repo);
        let mut store = store_for(&base, &repo, 1);
        let sink = RecordingSink::default();

        let first = create_task(&mut store, &sink, "first", None).expect("create first");
        let second = create_task(&mut store, &sink, "second", None).expect("create second");
        start_task(&mut store, &sink, &first.task_id).expect("start first");
        let queued = start_task(&mut store, &sink, &second.task_id).expect("start second");
        assert_eq!(queued.status, TaskStatus::Queued);
        sink.clear();

        let cancelled = cancel_task(&mut store, &sink, &first.task_id).expect("cancel first");
        assert_eq!(cancelled.status, TaskStatus::Failed);

        let events = sink.events();
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(matches!(
            &events[0],
            CoreEvent::TaskUpdated { task }
                if task.task_id == first.task_id && task.status == TaskStatus::Failed
        ));
        assert!(matches!(
            &events[1],
            CoreEvent::TaskUpdated { task }
                if task.task_id == second.task_id && task.status == TaskStatus::Running
        ));
    }

    #[test]
    fn session_binding_starts_task_and_keeps_queue_unbound() {
        let base = temp_root("bind-session");
        let repo = base.join("repo");
        init_repo(&repo);
        let mut store = store_for(&base, &repo, 1);

        let first = store.create("first", None).expect("create first");
        let second = store.create("second", None).expect("create second");

        let bound = bind_session_in_store(&mut store, &first.task_id, "s-1").expect("bind first");
        assert!(bound.started);
        assert_eq!(bound.task.status, TaskStatus::Running);
        assert_eq!(bound.task.session_id.as_deref(), Some("s-1"));

        let error = bind_session_in_store(&mut store, &second.task_id, "s-2")
            .expect_err("queue must be full");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        let queued = store.get(&second.task_id).expect("second");
        assert_eq!(queued.status, TaskStatus::Queued);
        assert_eq!(
            queued.session_id, None,
            "queue-full must not bind the session"
        );

        // 已在运行的任务再次绑定不应重复 start。
        let rebound = bind_session_in_store(&mut store, &first.task_id, "s-3").expect("rebind");
        assert!(!rebound.started);
        assert_eq!(rebound.task.session_id.as_deref(), Some("s-3"));
    }

    fn sample_task(task_id: &str, status: TaskStatus) -> TaskInfo {
        TaskInfo {
            task_id: task_id.into(),
            title: "sample".into(),
            session_id: Some("s-1".into()),
            status,
            worktree_path: Some("/tmp/wt".into()),
            branch: Some("lc/sample".into()),
            port: Some(4300),
            base_sha: Some("abc123".into()),
            steps: Vec::new(),
            changes: Vec::new(),
            checks: Vec::new(),
            created_at: 1_700_000_000_000,
        }
    }

    #[test]
    fn task_event_envelopes_are_sequenced_and_snake_case() {
        let events = TaskEvents::default();
        let created = events.next_envelope(CoreEvent::TaskCreated {
            task_id: "t1".into(),
            title: "fix".into(),
        });
        assert_eq!(created.seq, 0);
        let json = serde_json::to_value(&created).expect("serialize created");
        assert_eq!(json["event"]["type"], "task:created");
        assert_eq!(json["event"]["task_id"], "t1");

        let updated = events.next_envelope(CoreEvent::TaskUpdated {
            task: sample_task("t1", TaskStatus::Running),
        });
        assert_eq!(updated.seq, 1);
        let json = serde_json::to_value(&updated).expect("serialize updated");
        assert_eq!(json["event"]["type"], "task:updated");
        assert_eq!(json["event"]["task"]["status"], "running");
        assert_eq!(json["event"]["task"]["task_id"], "t1");

        let cases = vec![
            (
                CoreEvent::TaskStatus {
                    task_id: "t1".into(),
                    status: "running".into(),
                },
                "task:status",
            ),
            (
                CoreEvent::TaskStepEvent {
                    task_id: "t1".into(),
                    index: 0,
                    title: "plan".into(),
                    state: TaskStepState::Running,
                },
                "task:step",
            ),
            (
                CoreEvent::TaskChangeEvent {
                    task_id: "t1".into(),
                    change: FileChangeDto {
                        path: "a.rs".into(),
                        status: ChangeStatus::Modified,
                        applied: false,
                        added: 1,
                        removed: 2,
                    },
                },
                "task:change",
            ),
            (
                CoreEvent::TaskCheckEvent {
                    task_id: "t1".into(),
                    check: TaskCheck {
                        name: "unit".into(),
                        ok: true,
                        output_tail: String::new(),
                        duration_ms: 5,
                    },
                },
                "task:check",
            ),
            (
                CoreEvent::TaskNeedsInput {
                    task_id: "t1".into(),
                    kind: "approval".into(),
                },
                "task:needs_input",
            ),
            (
                CoreEvent::TaskMerged {
                    task_id: "t1".into(),
                    commit_sha: "abc".into(),
                },
                "task:merged",
            ),
            (
                CoreEvent::TaskDiscarded {
                    task_id: "t1".into(),
                },
                "task:discarded",
            ),
        ];
        for (offset, (event, expected)) in cases.into_iter().enumerate() {
            let envelope = events.next_envelope(event);
            assert_eq!(envelope.seq, 2 + offset as u64);
            let json = serde_json::to_value(&envelope).expect("serialize envelope");
            assert_eq!(json["event"]["type"], expected);
        }
    }

    #[test]
    fn task_store_slot_reloads_on_root_switch() {
        let base = temp_root("store-slot");
        let repo_a = base.join("repo-a");
        let repo_b = base.join("repo-b");
        init_repo(&repo_a);
        init_repo(&repo_b);
        let data_a = base.join("data-a");
        let data_b = base.join("data-b");

        let mut slot = TaskStoreSlot::default();
        let created = {
            let store = slot
                .ensure_with(&repo_a, |root| {
                    Ok(TaskStore::new(data_a.join("tasks.json"), root))
                })
                .expect("load root a");
            assert!(store.list().is_empty());
            let task = store.create("root a task", None).expect("create");
            task.task_id
        };

        {
            let store = slot
                .ensure_with(&repo_b, |root| {
                    Ok(TaskStore::new(data_b.join("tasks.json"), root))
                })
                .expect("load root b");
            assert!(store.list().is_empty(), "root b must not see root a tasks");
        }

        {
            let store = slot
                .ensure_with(&repo_a, |root| {
                    Ok(TaskStore::new(data_a.join("tasks.json"), root))
                })
                .expect("reload root a");
            let tasks = store.list();
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].task_id, created);
        }
    }

    #[test]
    fn task_store_slot_maps_load_failure_to_contract_error() {
        let base = temp_root("store-failure");
        let repo = base.join("repo");
        init_repo(&repo);
        let mut slot = TaskStoreSlot::default();

        let error = slot
            .ensure_with(&repo, |_| {
                Err(TaskError::StoreCorrupt {
                    message: "boom".into(),
                })
            })
            .expect_err("load must fail");
        assert_eq!(error.code, ErrorCode::Io);
        assert!(error.message.contains("boom"), "{}", error.message);
    }

    #[test]
    fn task_state_clone_shares_store_and_requires_initialization() {
        let base = temp_root("state-clone");
        let repo = base.join("repo");
        init_repo(&repo);

        let state = TaskState::new();
        let clone = state.clone();

        // 未初始化（worker 先于 session_create 绑定）→ Internal，命令层不 panic。
        let error = clone
            .with_current(|store| Ok(store.list().len()))
            .expect_err("store is not initialized");
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("尚未初始化"), "{}", error.message);

        let mut store = store_for(&base, &repo, 2);
        let task = store
            .create("shared state", Some("s-shared".into()))
            .expect("create");
        store.start(&task.task_id).expect("start");
        state.init_for_test(store);

        // 克隆共享同一 store：任一 handle 的迁移对另一个立即可见。
        assert_eq!(
            clone
                .with_current(|store| Ok(store.list().len()))
                .expect("clone sees store"),
            1
        );
        let blocked = clone.mark_needs_input(&task.task_id).expect("mark blocked");
        assert_eq!(blocked.status, TaskStatus::Blocked);
        let seen = state
            .with_current(|store| {
                store.get(&task.task_id).ok_or_else(|| TaskError::NotFound {
                    task_id: task.task_id.clone(),
                })
            })
            .expect("original sees clone mutation");
        assert_eq!(seen.status, TaskStatus::Blocked);

        // reset 同样作用于所有克隆。
        clone.reset();
        let error = state
            .with_current(|store| Ok(store.list().len()))
            .expect_err("reset clears shared slot");
        assert_eq!(error.code, ErrorCode::Internal);
    }
}
