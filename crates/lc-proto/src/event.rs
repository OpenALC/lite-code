//! 事件契约。核心 → UI 单向下行，全部带序号；UI 只订阅不查询（除首次加载）。
//! task:* 事件名与 docs/TASKS.md §7 保持一致。

use crate::agent::{ApprovalPrompt, SessionInfo};
use crate::error::ErrorBody;
use crate::runtime::{NodeStatus, ShellInfo};
use crate::task::{FileChangeDto, TaskCheck, TaskInfo, TaskStepState};

/// 事件信封。`seq` 在连接内单调递增，前端发现跳变即重拉（generation 语义）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct EventEnvelope {
    pub seq: u64,
    pub event: CoreEvent,
}

/// 核心事件集。v0 固定事件名与载荷骨架，lane 实现时不得改名字段。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum CoreEvent {
    #[serde(rename = "node:status")]
    NodeStatus { status: NodeStatus },

    #[serde(rename = "shell:status")]
    ShellStatus { shell: Option<ShellInfo> },

    /// 文件树整体代数变化；前端发现跳变即重拉
    #[serde(rename = "fs:tree")]
    FsTree { generation: u64 },

    #[serde(rename = "fs:entry")]
    FsEntry { path: String },

    #[serde(rename = "task:created")]
    TaskCreated { task_id: String, title: String },

    #[serde(rename = "task:status")]
    TaskStatus { task_id: String, status: String },

    /// 任务完整快照：创建/状态/步骤/变更/检查任意变化后下发，UI 直接替换
    #[serde(rename = "task:updated")]
    TaskUpdated { task: TaskInfo },

    #[serde(rename = "task:step")]
    TaskStepEvent {
        task_id: String,
        index: u32,
        title: String,
        state: TaskStepState,
    },

    #[serde(rename = "task:change")]
    TaskChangeEvent {
        task_id: String,
        change: FileChangeDto,
    },

    #[serde(rename = "task:check")]
    TaskCheckEvent { task_id: String, check: TaskCheck },

    #[serde(rename = "task:discarded")]
    TaskDiscarded { task_id: String },

    /// 触发状态栏“待确认”计数
    #[serde(rename = "task:needs_input")]
    TaskNeedsInput { task_id: String, kind: String },

    #[serde(rename = "task:merged")]
    TaskMerged { task_id: String, commit_sha: String },

    // ---- Agent 事件（P3 实装；契约先行，见 AGENT.md §3 流式事件）----
    #[serde(rename = "agent:text_delta")]
    AgentTextDelta {
        session_id: String,
        index: u32,
        text: String,
    },

    #[serde(rename = "agent:thinking_delta")]
    AgentThinkingDelta { session_id: String, text: String },

    #[serde(rename = "agent:tool_start")]
    AgentToolStart {
        session_id: String,
        call_id: String,
        name: String,
    },

    #[serde(rename = "agent:tool_result")]
    AgentToolResult {
        session_id: String,
        call_id: String,
        ok: bool,
        text: String,
    },

    #[serde(rename = "agent:approval_required")]
    AgentApprovalRequired {
        session_id: String,
        approval: ApprovalPrompt,
    },

    #[serde(rename = "agent:session_state")]
    AgentSessionState { session: SessionInfo },

    #[serde(rename = "agent:usage")]
    AgentUsage {
        session_id: String,
        input: u32,
        output: u32,
        cached: u32,
    },

    /// 上下文压缩记录（docs/AGENT.md §6：不静默截断，每次压缩留可展开记录）
    #[serde(rename = "agent:compacted")]
    AgentCompacted {
        session_id: String,
        record_id: u64,
        before_tokens: u32,
        after_tokens: u32,
        removed_messages: u32,
        summary: String,
    },

    #[serde(rename = "agent:done")]
    AgentDone {
        session_id: String,
        stop_reason: String,
    },

    /// 统一错误事件；高频流式错误用对应 agent 事件承载
    #[serde(rename = "error")]
    Error { error: ErrorBody },
}
