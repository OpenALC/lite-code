//! Git 面板 DTO（docs/MILESTONES.md P5 周边）。只读写主工作区，
//! 任务 worktree 的 diff 由 `task_diff` 提供。

use crate::task::ChangeStatus;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct GitFileStatus {
    pub path: String,
    pub status: ChangeStatus,
    pub staged: bool,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct GitStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub clean: bool,
    pub merge_in_progress: bool,
    pub rebase_in_progress: bool,
    pub files: Vec<GitFileStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct GitCommit {
    pub sha: String,
    pub short_sha: String,
    pub author: String,
    /// 第一行摘要（完整 message 不进列表，详情按需读取）。
    pub message: String,
    /// Unix 秒。
    pub timestamp: i64,
}
