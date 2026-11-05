//! Task 实体与工作台 DTO（docs/TASKS.md §2）。与 lc-core 的 task 存储对齐，
//! 只承载 UI 可见字段；worktree 路径用展示路径（Windows 反斜杠已按应用约定处理）。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum TaskStatus {
    Planning,
    Running,
    Blocked,
    Review,
    Done,
    Failed,
    Queued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum ChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum TaskStepState {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct TaskStep {
    pub index: u32,
    pub title: String,
    pub state: TaskStepState,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct FileChangeDto {
    pub path: String,
    pub status: ChangeStatus,
    pub applied: bool,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct TaskCheck {
    pub name: String,
    pub ok: bool,
    pub output_tail: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct TaskInfo {
    pub task_id: String,
    pub title: String,
    pub session_id: Option<String>,
    pub status: TaskStatus,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub port: Option<u16>,
    pub base_sha: Option<String>,
    pub steps: Vec<TaskStep>,
    pub changes: Vec<FileChangeDto>,
    pub checks: Vec<TaskCheck>,
    pub created_at: i64,
}
