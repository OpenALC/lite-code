//! Agent 会话与审批 DTO（docs/AGENT.md §5）。与 lc-core 的 Policy/Session
//! 语义对齐，但只承载 UI 可见字段。

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum SessionMode {
    Plan,
    Chat,
    Auto,
    Yolo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum ApprovalDecision {
    AllowOnce,
    AllowSession,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum ApprovalSource {
    Mode,
    StaticBlacklist,
    UserBlacklist,
    ModelRisk,
    OutsideWorkspace,
    NetworkTool,
    ExecuteReview,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct SessionInfo {
    pub session_id: String,
    pub mode: SessionMode,
    pub turn: u32,
    pub max_turns: u32,
    /// 模型上下文窗口（k token），UI 用量条分母。
    pub context_k: u32,
    /// 压缩阈值（k token），UI 刻度线。
    pub compress_at_k: u32,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ApprovalPrompt {
    pub approval_id: u64,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub source: ApprovalSource,
    pub reason: String,
}
