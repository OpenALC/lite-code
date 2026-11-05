//! lc-proto —— Lite Code 的唯一契约层。
//!
//! 并行开发的所有 lane 只通过这里定义的 DTO / Command / Event / ErrorCode 交互。
//! TS 绑定由 `cargo run -p lc-proto --features codegen --bin codegen` 生成，
//! 产物位于 `bindings/`，进仓并由 CI 做漂移检查。

pub mod agent;
pub mod command;
pub mod error;
pub mod event;
pub mod runtime;
pub mod task;

pub use agent::{ApprovalDecision, ApprovalPrompt, ApprovalSource, SessionInfo, SessionMode};
pub use command::{
    CONTRACT_COMMANDS, CONTRACT_ERROR_CODES, CONTRACT_EVENTS, CONTRACT_VERSION, CommandEnvelope,
    CommandResult, ContractMeta, CoreCommand, DirEntryDto, EntryKind,
};
pub use error::{ErrorBody, ErrorCode};
pub use event::{CoreEvent, EventEnvelope};
pub use runtime::{
    NodeFailureReason, NodeHealth, NodeProbeAttempt, NodeRuntimeInfo, NodeSource, NodeStatus,
    SemVer, ShellInfo, ShellKind,
};
pub use task::{
    ChangeStatus, FileChangeDto, TaskCheck, TaskInfo, TaskStatus, TaskStep, TaskStepState,
};

/// 当前契约版本。新增可选字段/命令 → minor+1；删除/改名/改语义 → major+1（独立 PR）。
pub fn contract_version() -> &'static str {
    CONTRACT_VERSION
}
