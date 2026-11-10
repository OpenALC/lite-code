//! 命令契约。UI → 核心的全部入口；Tauri 命令只是这里的转发层
//! （docs/README.md：lc-core 不依赖 Tauri）。

use crate::agent::{ApprovalDecision, SessionInfo, SessionMode};
use crate::db::{
    DbConnectionConfig, DbConnectionInfo, DbExecResult, DbExplainResult, DbQueryResult,
    DbSchemaNode,
};
use crate::git::{GitCommit, GitFileStatus, GitStatus};
use crate::runtime::{NodeStatus, ShellInfo};
use crate::task::{FileChangeDto, TaskInfo};

/// 当前契约版本。规则见 docs/VERSIONING.md §8。
pub const CONTRACT_VERSION: &str = "0.8.0";

/// 全部命令名，与 [`CoreCommand`] 的 serde tag 一一对应。
/// 单元测试保证与枚举同步（数量 + 命名）。
pub const CONTRACT_COMMANDS: &[&str] = &[
    "node_detect",
    "node_set_manual",
    "node_download_portable",
    "shell_detect",
    "workspace_root",
    "workspace_open",
    "fs_read_dir",
    "fs_read_file",
    "fs_write_file",
    "session_create",
    "agent_send",
    "agent_cancel",
    "approval_resolve",
    "task_list",
    "task_create",
    "task_start",
    "task_cancel",
    "task_apply",
    "task_discard",
    "task_diff",
    "task_bind_session",
    "db_list",
    "db_connect",
    "db_disconnect",
    "db_schema",
    "db_query",
    "db_execute",
    "db_explain",
    "db_save_connection",
    "db_delete_connection",
    "git_status",
    "git_diff",
    "git_stage",
    "git_unstage",
    "git_commit",
    "git_log",
    "contract_info",
];

/// 全部事件名，与 [`crate::CoreEvent`] 的 serde `type` 值对应。
pub const CONTRACT_EVENTS: &[&str] = &[
    "node:status",
    "shell:status",
    "fs:tree",
    "fs:entry",
    "task:created",
    "task:status",
    "task:step",
    "task:change",
    "task:check",
    "task:updated",
    "task:needs_input",
    "task:merged",
    "task:discarded",
    "db:status",
    "agent:text_delta",
    "agent:thinking_delta",
    "agent:tool_start",
    "agent:tool_result",
    "agent:approval_required",
    "agent:session_state",
    "agent:usage",
    "agent:compacted",
    "agent:done",
    "error",
];

/// 全部错误码名，与 [`crate::ErrorCode`] 的 serde tag 对应。
pub const CONTRACT_ERROR_CODES: &[&str] = &[
    "internal",
    "not_implemented",
    "invalid_argument",
    "timeout",
    "cancelled",
    "path_outside_root",
    "not_found",
    "io",
    "node_unavailable",
    "node_broken",
    "shell_unavailable",
    "contract_mismatch",
    "permission_denied",
    "db_unavailable",
    "db_rejected",
    "db_read_only",
    "git_unavailable",
    "git_rejected",
];

/// 请求信封。`id` 用于关联响应（会话内单调递增）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct CommandEnvelope {
    pub id: u64,
    pub command: CoreCommand,
}

/// 核心命令集。v0.1 覆盖 runtime 探测、workspace 根与 fs 读写；
/// agent 命令在 P3 以 minor 版本加入。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum CoreCommand {
    /// 探测 Node（惰性触发，串行枚举候选，见 docs/RUNTIME.md）
    NodeDetect,
    /// 手动指定 node.exe 路径；选中后立即校验，通过才写配置
    NodeSetManual { path: String },
    /// 下载便携版（校验 SHASUMS256.txt 后解压，见 docs/RUNTIME.md §5）
    NodeDownloadPortable { version: Option<String> },
    /// 探测 shell（启动时执行，见 docs/RUNTIME.md §8）
    ShellDetect,
    /// 当前工作区根（文件树与 fs 命令的安全边界，docs/PLAN.md R3）
    WorkspaceRoot,
    /// 打开一个目录作为工作区根；路径必须存在且是目录
    WorkspaceOpen { path: String },
    /// 目录按层返回，不整树序列化（docs/ARCHITECTURE.md §5.2）
    FsReadDir { path: String },
    /// 读文件。v0 返回文本；P1 传输层改走原始字节（避免 base64 往返）
    FsReadFile { path: String },
    /// 保存文本文件；目标必须落在 workspace 根内，允许新建
    FsWriteFile { path: String, content: String },
    /// 创建 P3 会话（模式在会话级生效）；`task_id` 存在时在任务 worktree 内运行
    SessionCreate {
        mode: SessionMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
    /// 向会话发送用户消息；流式结果走 agent:* 事件
    AgentSend { session_id: String, text: String },
    /// 取消运行中的会话
    AgentCancel { session_id: String },
    /// 处理审批卡
    ApprovalResolve {
        session_id: String,
        approval_id: u64,
        decision: ApprovalDecision,
    },
    /// 任务列表（工作台首次加载 / 重连时拉取）
    TaskList,
    /// 创建任务；有 git 仓库时同时创建 worktree
    TaskCreate {
        title: String,
        session_id: Option<String>,
    },
    /// 启动排队/待规划任务
    TaskStart { task_id: String },
    /// 取消任务；worktree 保留供 Review
    TaskCancel { task_id: String },
    /// 应用任务变更到主工作区（一次一个）
    TaskApply { task_id: String },
    /// 丢弃任务与 worktree
    TaskDiscard { task_id: String },
    /// 读取任务的 unified diff 与文件统计
    TaskDiff { task_id: String },
    /// 绑定/解绑 Agent 会话（None = 解绑）
    TaskBindSession {
        task_id: String,
        session_id: Option<String>,
    },
    /// 数据库连接列表（不含密钥）
    DbList,
    /// 建立连接；密码由 SecretStore / 环境变量解析
    DbConnect { connection_id: String },
    /// 断开连接
    DbDisconnect { connection_id: String },
    /// 读取 schema 树
    DbSchema { connection_id: String },
    /// 执行读查询；结果只回样本 + 句柄
    DbQuery {
        connection_id: String,
        sql: String,
        limit: Option<u32>,
    },
    /// 执行写/DDL；必须通过 SQL 闸门
    DbExecute { connection_id: String, sql: String },
    /// EXPLAIN，不执行
    DbExplain { connection_id: String, sql: String },
    /// 新建/更新连接配置（密钥单独存）
    DbSaveConnection { connection: DbConnectionConfig },
    /// 删除连接配置
    DbDeleteConnection { connection_id: String },
    /// Git 状态（主工作区）
    GitStatus,
    /// Git diff；path 为空时返回全量
    GitDiff { path: Option<String>, staged: bool },
    /// 暂存路径
    GitStage { paths: Vec<String> },
    /// 取消暂存
    GitUnstage { paths: Vec<String> },
    /// 提交（message 必须非空）
    GitCommit { message: String },
    /// 最近提交
    GitLog { limit: Option<u32> },
    /// 契约元信息（lc-cli 与 UI 关于页共用）
    ContractInfo,
}

/// 命令结果。每个命令对应一个 `kind`；错误走统一的 `ErrorBody`（transport 层包装）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum CommandResult {
    NodeStatus {
        status: NodeStatus,
    },
    Shell {
        shell: Option<ShellInfo>,
    },
    Session {
        session: SessionInfo,
    },
    Tasks {
        tasks: Vec<TaskInfo>,
    },
    Task {
        task: TaskInfo,
    },
    TaskDiff {
        task_id: String,
        patch: String,
        files: Vec<FileChangeDto>,
    },
    Connections {
        connections: Vec<DbConnectionInfo>,
    },
    Connection {
        connection: DbConnectionInfo,
    },
    DbSchema {
        connection_id: String,
        nodes: Vec<DbSchemaNode>,
    },
    DbQuery {
        result: DbQueryResult,
    },
    DbExec {
        result: DbExecResult,
    },
    DbExplain {
        result: DbExplainResult,
    },
    GitStatus {
        status: GitStatus,
    },
    GitDiff {
        patch: String,
        files: Vec<GitFileStatus>,
    },
    GitLog {
        commits: Vec<GitCommit>,
    },
    WorkspaceRoot {
        root: Option<String>,
    },
    DirListing {
        path: String,
        entries: Vec<DirEntryDto>,
        generation: u64,
    },
    FileContent {
        path: String,
        text: String,
    },
    Contract {
        meta: ContractMeta,
    },
    Ack,
}

/// 目录项。按层返回，不含子树。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// 契约元信息。lc-cli `contract info` 与 UI“关于”页共用同一来源。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ContractMeta {
    pub name: String,
    pub contract_version: String,
    pub commands: Vec<String>,
    pub events: Vec<String>,
    pub error_codes: Vec<String>,
}

impl ContractMeta {
    pub fn current() -> Self {
        Self {
            name: "lc-proto".into(),
            contract_version: CONTRACT_VERSION.into(),
            commands: CONTRACT_COMMANDS.iter().map(|s| (*s).into()).collect(),
            events: CONTRACT_EVENTS.iter().map(|s| (*s).into()).collect(),
            error_codes: CONTRACT_ERROR_CODES.iter().map(|s| (*s).into()).collect(),
        }
    }
}
