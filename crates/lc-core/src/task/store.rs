//! 任务持久化、并发队列与状态迁移（docs/TASKS.md §2、§4、§8）。
//!
//! `TaskStore` 本身不带 `Mutex`：传输层（Tauri）包一层即可；纯方法全部可
//! 在单测里直接调用。所有写盘走 `config::write_atomic`（临时文件 + rename，
//! 并保留 `.bak`）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lc_proto::{TaskInfo, TaskStatus};
use serde::{Deserialize, Serialize};

use crate::fs::display_path;

use super::diff::{TaskDiff, build_diff, scan_changes};
use super::worktree::{self, ApplyOutcome};
use super::{PORT_RANGE_END, PORT_RANGE_START, TaskError, next_task_id, now_millis};

/// 默认并发上限（docs/TASKS.md §4：验收质量优先）。
pub const DEFAULT_MAX_RUNNING: usize = 2;
/// 硬上限，设置项不可越过。
pub const HARD_MAX_RUNNING: usize = 4;

const STORE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    tasks: Vec<TaskRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskRecord {
    task: TaskInfo,
    #[serde(default)]
    notes: Vec<String>,
}

/// 任务存储与队列。构造时注入 `tasks.json` 路径与项目根，便于测试隔离；
/// 生产入口是 [`TaskStore::load_default`]。
#[derive(Debug)]
pub struct TaskStore {
    persist_path: PathBuf,
    data_root: PathBuf,
    repo_root: PathBuf,
    tasks: Vec<TaskRecord>,
    max_running: usize,
    load_warning: Option<TaskError>,
}

impl TaskStore {
    /// 用显式路径构造（测试/嵌入方使用）。加载失败不会 panic，错误可通过
    /// [`TaskStore::load_warning`] 读取；需要严格加载请用
    /// [`TaskStore::load_default`] 或 [`TaskStore::reload`]。
    pub fn new(persist_path: impl AsRef<Path>, repo_root: impl AsRef<Path>) -> Self {
        Self::with_max_running(persist_path, repo_root, DEFAULT_MAX_RUNNING)
    }

    /// 同 [`TaskStore::new`]，并指定并发上限（clamp 到 1..=4）。
    pub fn with_max_running(
        persist_path: impl AsRef<Path>,
        repo_root: impl AsRef<Path>,
        max_running: usize,
    ) -> Self {
        let mut store = Self::build(persist_path, repo_root, max_running);
        if let Err(error) = store.reload() {
            store.load_warning = Some(error);
        }
        store
    }

    /// 生产入口：`lc_core::config::data_dir()/tasks.json`。
    pub fn load_default(repo_root: impl AsRef<Path>) -> Result<Self, TaskError> {
        let data_dir = crate::config::data_dir().map_err(|error| TaskError::Config {
            message: error.to_string(),
        })?;
        let mut store = Self::build(data_dir.join("tasks.json"), repo_root, DEFAULT_MAX_RUNNING);
        store.reload()?;
        Ok(store)
    }

    fn build(
        persist_path: impl AsRef<Path>,
        repo_root: impl AsRef<Path>,
        max_running: usize,
    ) -> Self {
        let persist_path = persist_path.as_ref().to_path_buf();
        let data_root = persist_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            persist_path,
            data_root,
            repo_root: repo_root.as_ref().to_path_buf(),
            tasks: Vec::new(),
            max_running: max_running.clamp(1, HARD_MAX_RUNNING),
            load_warning: None,
        }
    }

    /// 从磁盘重新加载：`Running` → `Failed`（中断），释放其端口；
    /// `Queued` / `Review` 以及其他状态原样保留。仅当发生迁移时回写。
    pub fn reload(&mut self) -> Result<(), TaskError> {
        let file = read_store(&self.persist_path)?;
        if file.version > STORE_VERSION {
            return Err(TaskError::StoreCorrupt {
                message: format!(
                    "{}: unsupported store version {} (expected <= {STORE_VERSION})",
                    display_path(&self.persist_path),
                    file.version
                ),
            });
        }

        let mut changed = false;
        let mut used_ports = HashSet::new();
        let mut tasks = Vec::with_capacity(file.tasks.len());
        for mut record in file.tasks {
            if record.task.status == TaskStatus::Running {
                record.task.status = TaskStatus::Failed;
                record.task.port = None;
                record
                    .notes
                    .push("interrupted: process exited while the task was running".into());
                changed = true;
            }
            if let Some(port) = record.task.port {
                if !(PORT_RANGE_START..=PORT_RANGE_END).contains(&port) {
                    record.task.port = None;
                    changed = true;
                } else if !used_ports.insert(port) {
                    // 文件被手工编辑时可能出现重复端口；保留第一个，其余回收。
                    record.task.port = None;
                    changed = true;
                }
            }
            tasks.push(record);
        }

        self.tasks = tasks;
        if changed {
            self.persist()?;
        }
        self.load_warning = None;
        Ok(())
    }

    /// 首次加载时若文件损坏而走了宽松构造，这里能拿到具体错误。
    pub fn load_warning(&self) -> Option<&TaskError> {
        self.load_warning.as_ref()
    }

    pub fn list(&self) -> Vec<TaskInfo> {
        self.tasks
            .iter()
            .map(|record| record.task.clone())
            .collect()
    }

    pub fn get(&self, task_id: impl AsRef<str>) -> Option<TaskInfo> {
        self.index_of(task_id.as_ref())
            .map(|index| self.tasks[index].task.clone())
    }

    /// 任务的诊断备注（worktree 降级、清理 warning 等），不进入契约 DTO。
    pub fn notes(&self, task_id: impl AsRef<str>) -> Vec<String> {
        self.index_of(task_id.as_ref())
            .map(|index| self.tasks[index].notes.clone())
            .unwrap_or_default()
    }

    pub fn persist_path(&self) -> &Path {
        &self.persist_path
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn max_running(&self) -> usize {
        self.max_running
    }

    /// 调整并发上限（clamp 1..=4）。已有的 Running 任务不受影响，
    /// 只影响后续 `start` / `promote_queued`。
    pub fn set_max_running(&mut self, max_running: usize) {
        self.max_running = max_running.clamp(1, HARD_MAX_RUNNING);
    }

    /// 创建任务：git 项目同时创建 worktree；失败降级为串行（不报错）。
    pub fn create(
        &mut self,
        title: impl AsRef<str>,
        session_id: Option<String>,
    ) -> Result<TaskInfo, TaskError> {
        let title = title.as_ref().trim();
        if title.is_empty() {
            return Err(TaskError::InvalidInput {
                message: "task title must not be empty".into(),
            });
        }

        let existing: HashSet<String> = self
            .tasks
            .iter()
            .map(|record| record.task.task_id.clone())
            .collect();
        let task_id = next_task_id(&existing);
        let created = worktree::create(&self.repo_root, &self.data_root, &task_id, title);
        let info = TaskInfo {
            task_id,
            title: title.to_string(),
            session_id,
            status: TaskStatus::Planning,
            worktree_path: created.path.as_deref().map(display_path),
            branch: created.branch,
            port: None,
            base_sha: created.base_sha,
            steps: Vec::new(),
            changes: Vec::new(),
            checks: Vec::new(),
            created_at: now_millis(),
        };

        self.tasks.push(TaskRecord {
            task: info.clone(),
            notes: created.notes,
        });
        if let Err(error) = self.persist() {
            // 写盘失败就把刚建的 worktree/分支尽力回收，避免泄漏。
            if let Some(record) = self.tasks.pop() {
                let path = record.task.worktree_path.as_deref().map(Path::new);
                let _ = worktree::remove(&self.repo_root, path, record.task.branch.as_deref());
            }
            return Err(error);
        }
        Ok(info)
    }

    /// 启动任务。并发满或（无 worktree 的）串行槽被占用时置 `Queued` 并返回；
    /// 只有 `Planning` / `Queued` 可以启动。
    pub fn start(&mut self, task_id: impl AsRef<str>) -> Result<TaskInfo, TaskError> {
        let task_id = task_id.as_ref();
        let index = self.index_of(task_id).ok_or_else(|| TaskError::NotFound {
            task_id: task_id.to_string(),
        })?;
        let status = self.tasks[index].task.status;
        if !matches!(status, TaskStatus::Planning | TaskStatus::Queued) {
            return Err(invalid_state(
                &self.tasks[index].task,
                "start",
                "planning or queued",
            ));
        }

        if self.running_count() >= self.max_running
            || (self.tasks[index].task.worktree_path.is_none() && self.running_serial_count() > 0)
        {
            self.tasks[index].task.status = TaskStatus::Queued;
            self.persist()?;
            return Ok(self.tasks[index].task.clone());
        }

        let port = match self.tasks[index].task.port {
            Some(port) => port,
            None => self.allocate_port()?,
        };
        self.tasks[index].task.port = Some(port);
        self.tasks[index].task.status = TaskStatus::Running;
        self.persist()?;
        Ok(self.tasks[index].task.clone())
    }

    /// 取消任务：状态置 `Failed`（契约没有 Canceled），worktree 保留供 Review。
    /// 端口立即回收。已 `Done` 的任务拒绝。
    pub fn cancel(&mut self, task_id: impl AsRef<str>) -> Result<TaskInfo, TaskError> {
        let task_id = task_id.as_ref();
        let index = self.index_of(task_id).ok_or_else(|| TaskError::NotFound {
            task_id: task_id.to_string(),
        })?;
        if self.tasks[index].task.status == TaskStatus::Done {
            return Err(invalid_state(
                &self.tasks[index].task,
                "cancel",
                "a task that has not been applied",
            ));
        }
        self.tasks[index].task.status = TaskStatus::Failed;
        self.tasks[index].task.port = None;
        self.persist()?;
        Ok(self.tasks[index].task.clone())
    }

    pub fn bind_session(
        &mut self,
        task_id: impl AsRef<str>,
        session_id: Option<String>,
    ) -> Result<TaskInfo, TaskError> {
        let task_id = task_id.as_ref();
        let index = self.index_of(task_id).ok_or_else(|| TaskError::NotFound {
            task_id: task_id.to_string(),
        })?;
        self.tasks[index].task.session_id = session_id;
        self.persist()?;
        Ok(self.tasks[index].task.clone())
    }

    /// 重新扫描 worktree：更新 `changes`。`checks` 由工具执行事件驱动，
    /// 本层没有 check runner，因此保持原值（通常为空）。
    pub fn refresh(&mut self, task_id: impl AsRef<str>) -> Result<TaskInfo, TaskError> {
        let task_id = task_id.as_ref();
        let index = self.index_of(task_id).ok_or_else(|| TaskError::NotFound {
            task_id: task_id.to_string(),
        })?;
        let Some(path) = self.tasks[index].task.worktree_path.clone() else {
            return Ok(self.tasks[index].task.clone());
        };
        let worktree = PathBuf::from(&path);
        if !worktree.is_dir() {
            return Ok(self.tasks[index].task.clone());
        }

        let scan = scan_changes(&worktree)?;
        self.tasks[index].task.changes = scan.changes;
        self.persist()?;
        Ok(self.tasks[index].task.clone())
    }

    /// 读取任务 diff（同时刷新 changes）。
    pub fn diff(&mut self, task_id: impl AsRef<str>) -> Result<TaskDiff, TaskError> {
        let task_id = task_id.as_ref();
        let index = self.index_of(task_id).ok_or_else(|| TaskError::NotFound {
            task_id: task_id.to_string(),
        })?;
        let Some(path) = self.tasks[index].task.worktree_path.clone() else {
            return Ok(TaskDiff {
                task_id: task_id.to_string(),
                patch: String::new(),
                files: self.tasks[index].task.changes.clone(),
            });
        };
        let worktree = PathBuf::from(&path);
        if !worktree.is_dir() {
            return Ok(TaskDiff {
                task_id: task_id.to_string(),
                patch: String::new(),
                files: self.tasks[index].task.changes.clone(),
            });
        }

        let scan = scan_changes(&worktree)?;
        let patch = build_diff(&worktree, &scan.untracked)?;
        self.tasks[index].task.changes = scan.changes.clone();
        self.persist()?;
        Ok(TaskDiff {
            task_id: task_id.to_string(),
            patch,
            files: scan.changes,
        })
    }

    /// 应用任务到主工作区：
    /// 1. 主工作区必须 clean；
    /// 2. 计算 base drift（不阻止合并）；
    /// 3. worktree 的未提交改动先提交到任务分支（避免 merge 后删除 worktree 丢数据）；
    /// 4. `git merge --no-ff`；冲突保留 merge 状态、任务置 `Review` 并返回错误；
    /// 5. 成功则记录合并提交、删除 worktree/分支、状态 `Done`。
    pub fn apply(&mut self, task_id: impl AsRef<str>) -> Result<ApplyOutcome, TaskError> {
        let task_id = task_id.as_ref();
        let index = self.index_of(task_id).ok_or_else(|| TaskError::NotFound {
            task_id: task_id.to_string(),
        })?;
        let status = self.tasks[index].task.status;
        if matches!(
            status,
            TaskStatus::Planning | TaskStatus::Queued | TaskStatus::Done
        ) {
            return Err(invalid_state(
                &self.tasks[index].task,
                "apply",
                "running, blocked, review or failed",
            ));
        }

        let info = self.tasks[index].task.clone();
        let mut warnings = self.tasks[index].notes.clone();

        let Some(path) = info.worktree_path.as_deref() else {
            // 非 git / worktree 降级：没有可合并的分支，直接完成。
            self.tasks[index].task.status = TaskStatus::Done;
            self.tasks[index].task.port = None;
            self.tasks[index].notes = warnings.clone();
            self.persist()?;
            return Ok(ApplyOutcome {
                commit: String::new(),
                base_drift: 0,
                warnings,
            });
        };
        let Some(branch) = info.branch.clone() else {
            return Err(TaskError::InvalidState {
                task_id: info.task_id.clone(),
                status: info.status,
                action: "apply",
                expected: "a worktree with a branch",
            });
        };
        let worktree = PathBuf::from(path);
        if !worktree.is_dir() {
            return Err(TaskError::WorktreeMissing {
                task_id: task_id.to_string(),
                path: path.to_string(),
            });
        }

        worktree::ensure_main_clean(&self.repo_root)?;

        // 合并前扫描一次，保证 apply 之后 changes 是 worktree 的真实快照；
        // 未跟踪/未提交的改动由 auto_commit 落到任务分支，不会因删除 worktree 丢失。
        let scan = scan_changes(&worktree)?;
        self.tasks[index].task.changes = scan.changes;

        let base_drift = match info.base_sha.as_deref() {
            Some(base_sha) => match worktree::base_drift(&self.repo_root, base_sha) {
                Ok(drift) => drift,
                Err(error) => {
                    warnings.push(format!("base drift unavailable: {error}"));
                    0
                }
            },
            None => 0,
        };

        if worktree::worktree_is_dirty(&worktree)? {
            match worktree::auto_commit_worktree(&worktree, task_id) {
                Ok(true) => {
                    warnings.push("uncommitted worktree changes were committed before merge".into())
                }
                Ok(false) => {}
                Err(error) => return Err(error),
            }
        }

        match worktree::merge_branch(&self.repo_root, &branch, task_id) {
            Ok(()) => {}
            Err(TaskError::MergeConflict { detail, .. }) => {
                self.tasks[index].task.status = TaskStatus::Review;
                self.tasks[index].task.port = None;
                warnings.push(format!("merge conflict left in main worktree: {detail}"));
                self.tasks[index].notes = warnings;
                self.persist()?;
                return Err(TaskError::MergeConflict {
                    task_id: task_id.to_string(),
                    detail,
                });
            }
            Err(error) => return Err(error),
        }

        let commit = worktree::head_sha(&self.repo_root)?;
        warnings.extend(worktree::remove(
            &self.repo_root,
            Some(&worktree),
            Some(&branch),
        ));

        for change in &mut self.tasks[index].task.changes {
            change.applied = true;
        }
        self.tasks[index].task.status = TaskStatus::Done;
        self.tasks[index].task.port = None;
        self.tasks[index].task.worktree_path = None;
        self.tasks[index].task.branch = None;
        self.tasks[index].notes = warnings.clone();
        self.persist()?;

        Ok(ApplyOutcome {
            commit,
            base_drift,
            warnings,
        })
    }

    /// 丢弃任务：worktree 有未提交改动时拒绝（`DirtyWorktree`）；干净则
    /// 删除 worktree 与分支（失败仅记 warning），状态置 `Failed`，端口回收。
    /// 非 git 任务没有 worktree，直接标记。
    pub fn discard(&mut self, task_id: impl AsRef<str>) -> Result<TaskInfo, TaskError> {
        let task_id = task_id.as_ref();
        let index = self.index_of(task_id).ok_or_else(|| TaskError::NotFound {
            task_id: task_id.to_string(),
        })?;
        if self.tasks[index].task.status == TaskStatus::Done {
            return Err(invalid_state(
                &self.tasks[index].task,
                "discard",
                "a task that has not been applied",
            ));
        }

        let info = self.tasks[index].task.clone();
        let mut warnings = self.tasks[index].notes.clone();

        if let Some(path) = info.worktree_path.as_deref() {
            let worktree = PathBuf::from(path);
            if worktree.is_dir() {
                if worktree::worktree_is_dirty(&worktree)? {
                    return Err(TaskError::DirtyWorktree {
                        task_id: task_id.to_string(),
                        path: path.to_string(),
                    });
                }
                warnings.extend(worktree::remove(
                    &self.repo_root,
                    Some(&worktree),
                    info.branch.as_deref(),
                ));
            } else {
                warnings.push(format!("worktree path missing on disk: {path}"));
            }
        }

        self.tasks[index].task.status = TaskStatus::Failed;
        self.tasks[index].task.port = None;
        self.tasks[index].task.worktree_path = None;
        self.tasks[index].task.branch = None;
        self.tasks[index].task.changes.clear();
        self.tasks[index].notes = warnings;
        self.persist()?;
        Ok(self.tasks[index].task.clone())
    }

    /// 按创建顺序启动**下一个**排队任务。并发已满、没有排队任务，或队首被
    /// 串行槽挡住时返回 `None`（后续任务不插队）。调用方在 cancel/done 后
    /// 显式调用（不自动 promote）；要填满多个空槽可循环调用。
    pub fn promote_queued(&mut self) -> Result<Option<TaskInfo>, TaskError> {
        if self.running_count() >= self.max_running {
            return Ok(None);
        }
        let next = self
            .tasks
            .iter()
            .filter(|record| record.task.status == TaskStatus::Queued)
            .min_by(|a, b| {
                a.task
                    .created_at
                    .cmp(&b.task.created_at)
                    .then_with(|| a.task.task_id.cmp(&b.task.task_id))
            })
            .map(|record| record.task.task_id.clone());
        let Some(task_id) = next else {
            return Ok(None);
        };

        let task = self.start(&task_id)?;
        if task.status == TaskStatus::Running {
            Ok(Some(task))
        } else {
            // 队首被串行槽挡住：保持创建顺序，后面的任务不插队。
            Ok(None)
        }
    }

    fn persist(&self) -> Result<(), TaskError> {
        let file = StoreFile {
            version: STORE_VERSION,
            tasks: self.tasks.clone(),
        };
        let mut text =
            serde_json::to_string_pretty(&file).map_err(|error| TaskError::StoreCorrupt {
                message: format!("cannot serialize tasks: {error}"),
            })?;
        text.push('\n');
        crate::config::write_atomic(&self.persist_path, &text).map_err(|error| TaskError::Io {
            path: display_path(&self.persist_path),
            message: error.to_string(),
        })
    }

    fn index_of(&self, task_id: &str) -> Option<usize> {
        self.tasks
            .iter()
            .position(|record| record.task.task_id == task_id)
    }

    fn running_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|record| record.task.status == TaskStatus::Running)
            .count()
    }

    fn running_serial_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|record| {
                record.task.status == TaskStatus::Running && record.task.worktree_path.is_none()
            })
            .count()
    }

    fn allocate_port(&self) -> Result<u16, TaskError> {
        let used: HashSet<u16> = self
            .tasks
            .iter()
            .filter_map(|record| record.task.port)
            .collect();
        (PORT_RANGE_START..=PORT_RANGE_END)
            .find(|port| !used.contains(port))
            .ok_or(TaskError::PortExhausted)
    }
}

fn read_store(path: &Path) -> Result<StoreFile, TaskError> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|error| TaskError::StoreCorrupt {
            message: format!("{}: {error}", display_path(path)),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile {
            version: STORE_VERSION,
            tasks: Vec::new(),
        }),
        Err(error) => Err(TaskError::Io {
            path: display_path(path),
            message: error.to_string(),
        }),
    }
}

fn invalid_state(task: &TaskInfo, action: &'static str, expected: &'static str) -> TaskError {
    TaskError::InvalidState {
        task_id: task.task_id.clone(),
        status: task.status,
        action,
        expected,
    }
}
