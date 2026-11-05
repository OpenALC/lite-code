//! 会话审批状态机（docs/AGENT.md §5.1/§5.4）。
//!
//! 只负责模式、轮次、审批队列与"本次会话允许同类"；LLM 消息历史与工具
//! 执行由后续 lane 接入。任何工具执行前都必须先拿到 `AuthorizeOutcome::Allowed`
//! 或 `Cached`。

use super::approval::{
    ApprovalCache, ApprovalDecision, ApprovalId, ApprovalQueue, ApprovalRequest,
};
use super::policy::{Policy, PolicyDecision, PolicyMode, PolicyRequest, PolicySource};
use super::tools::Risk;

pub const DEFAULT_MAX_TURNS: u32 = 25;
pub const DEFAULT_CONTEXT_K: u32 = 400;
pub const DEFAULT_COMPRESS_AT_K: u32 = 300;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error("已达到轮次上限 {max}")]
    TurnLimit { max: u32 },
    #[error("已有待审批请求，先处理后再继续")]
    PendingApproval,
    #[error("未知审批 id：{0}")]
    UnknownApproval(ApprovalId),
    #[error("会话已取消")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeChange {
    Applied,
    /// 切到 yolo 必须显式确认；确认前模式不变。
    NeedsYoloConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizeOutcome {
    Allowed,
    /// 命中"本次会话允许同类"缓存，无需再弹审批。
    Cached,
    NeedsApproval(ApprovalRequest),
    Denied {
        source: PolicySource,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Allowed,
    Denied,
}

pub struct Session {
    pub id: String,
    mode: PolicyMode,
    policy: Policy,
    approvals: ApprovalQueue,
    cache: ApprovalCache,
    turn: u32,
    max_turns: u32,
    context_k: u32,
    compress_at_k: u32,
    yolo_confirmed: bool,
    cancelled: bool,
}

impl Session {
    pub fn new(id: impl Into<String>, mode: PolicyMode) -> Self {
        Self::with_policy(id, mode, Policy::with_defaults())
    }

    pub fn with_policy(id: impl Into<String>, mode: PolicyMode, policy: Policy) -> Self {
        Self {
            id: id.into(),
            mode,
            policy,
            approvals: ApprovalQueue::default(),
            cache: ApprovalCache::default(),
            turn: 0,
            max_turns: DEFAULT_MAX_TURNS,
            context_k: DEFAULT_CONTEXT_K,
            compress_at_k: DEFAULT_COMPRESS_AT_K,
            yolo_confirmed: false,
            cancelled: false,
        }
    }

    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns.max(1);
        self
    }

    /// 设置上下文预算（k token）；`compress_at_k` 必须小于 `context_k`，
    /// 非法值回落到默认预算，避免除法/刻度异常。
    pub fn with_context(mut self, context_k: u32, compress_at_k: u32) -> Self {
        if context_k > 0 && compress_at_k > 0 && compress_at_k < context_k {
            self.context_k = context_k;
            self.compress_at_k = compress_at_k;
        }
        self
    }

    pub fn context_k(&self) -> u32 {
        self.context_k
    }

    pub fn compress_at_k(&self) -> u32 {
        self.compress_at_k
    }

    pub fn mode(&self) -> PolicyMode {
        self.mode
    }

    /// 切换模式；yolo 需要 `confirm_yolo`。
    pub fn request_mode(&mut self, mode: PolicyMode) -> ModeChange {
        if mode == PolicyMode::Yolo && !self.yolo_confirmed {
            return ModeChange::NeedsYoloConfirmation;
        }
        self.mode = mode;
        ModeChange::Applied
    }

    /// 用户确认 yolo；每个会话必须显式调用一次。
    pub fn confirm_yolo(&mut self) {
        self.yolo_confirmed = true;
        self.mode = PolicyMode::Yolo;
    }

    pub fn turn(&self) -> u32 {
        self.turn
    }

    pub fn max_turns(&self) -> u32 {
        self.max_turns
    }

    pub fn begin_turn(&mut self) -> Result<u32, SessionError> {
        if self.cancelled {
            return Err(SessionError::Cancelled);
        }
        if self.turn >= self.max_turns {
            return Err(SessionError::TurnLimit {
                max: self.max_turns,
            });
        }
        self.turn += 1;
        Ok(self.turn)
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.approvals.clear();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn pending(&self) -> &[ApprovalRequest] {
        self.approvals.pending()
    }

    /// 丢弃待审批请求，但保留会话可继续（区别于永久终止的 [`Session::cancel`]）。
    pub fn clear_pending_approvals(&mut self) {
        self.approvals.clear();
    }

    pub fn authorize(
        &mut self,
        request: &PolicyRequest<'_>,
    ) -> Result<AuthorizeOutcome, SessionError> {
        if self.cancelled {
            return Err(SessionError::Cancelled);
        }
        if !self.approvals.is_empty() {
            return Err(SessionError::PendingApproval);
        }

        // 会话模式是权威来源，不接受调用方传入的旧模式。
        let request = PolicyRequest {
            mode: self.mode,
            ..request.clone()
        };

        match self.policy.evaluate(&request) {
            PolicyDecision::Allow => Ok(AuthorizeOutcome::Allowed),
            PolicyDecision::Deny { source, reason } => {
                Ok(AuthorizeOutcome::Denied { source, reason })
            }
            PolicyDecision::Ask { source, reason } => {
                if self.cache.allows(request.tool, request.command) {
                    return Ok(AuthorizeOutcome::Cached);
                }
                let approval = self
                    .approvals
                    .push(request.tool, request.command, source, reason);
                Ok(AuthorizeOutcome::NeedsApproval(approval))
            }
        }
    }

    pub fn approve(
        &mut self,
        id: ApprovalId,
        decision: ApprovalDecision,
    ) -> Result<ApprovalOutcome, SessionError> {
        let request = self
            .approvals
            .take(id)
            .ok_or(SessionError::UnknownApproval(id))?;
        match decision {
            ApprovalDecision::AllowOnce => Ok(ApprovalOutcome::Allowed),
            ApprovalDecision::AllowSession => {
                self.cache
                    .allow_session(&request.tool, request.command.as_deref());
                Ok(ApprovalOutcome::Allowed)
            }
            ApprovalDecision::Deny => Ok(ApprovalOutcome::Denied),
        }
    }

    /// 便捷判定：工具 + 风险 + 审批摘要（shell 命令 / SQL / 文件路径 /
    /// 紧凑参数 JSON）+ 工作区信息。
    pub fn authorize_tool(
        &mut self,
        tool: &str,
        risk: Risk,
        command: Option<&str>,
        inside_workspace: bool,
        model_risk_high: Option<bool>,
    ) -> Result<AuthorizeOutcome, SessionError> {
        let request = PolicyRequest {
            mode: self.mode,
            tool,
            risk,
            command,
            inside_workspace,
            model_risk_high,
        };
        self.authorize(&request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(mode: PolicyMode) -> Session {
        Session::new("s1", mode)
    }

    #[test]
    fn plan_blocks_write_without_approval() {
        let mut s = session(PolicyMode::Plan);
        assert_eq!(
            s.authorize_tool("read_file", Risk::Read, None, true, None),
            Ok(AuthorizeOutcome::Allowed)
        );
        assert!(matches!(
            s.authorize_tool("write_file", Risk::Write, None, true, None),
            Ok(AuthorizeOutcome::Denied { .. })
        ));
        assert!(s.pending().is_empty());
    }

    #[test]
    fn auto_write_outside_requires_approval_and_cache_works() {
        let mut s = session(PolicyMode::Auto);
        let first = s
            .authorize_tool("write_file", Risk::Write, None, false, None)
            .expect("authorize");
        let AuthorizeOutcome::NeedsApproval(request) = first else {
            panic!("expected approval, got {first:?}");
        };
        assert_eq!(s.pending().len(), 1);

        // 待审批时不能继续发起工具。
        assert_eq!(
            s.authorize_tool("read_file", Risk::Read, None, true, None),
            Err(SessionError::PendingApproval)
        );

        assert_eq!(
            s.approve(request.id, ApprovalDecision::AllowSession),
            Ok(ApprovalOutcome::Allowed)
        );
        assert!(s.pending().is_empty());

        // 同类命中缓存。
        assert_eq!(
            s.authorize_tool("write_file", Risk::Write, None, false, None),
            Ok(AuthorizeOutcome::Cached)
        );
    }

    #[test]
    fn allow_once_is_not_reused_and_deny_clears() {
        let mut s = session(PolicyMode::Auto);
        let AuthorizeOutcome::NeedsApproval(first) = s
            .authorize_tool("write_file", Risk::Write, None, false, None)
            .expect("authorize")
        else {
            panic!("expected approval");
        };
        assert_eq!(
            s.approve(first.id, ApprovalDecision::AllowOnce),
            Ok(ApprovalOutcome::Allowed)
        );
        let second = match s
            .authorize_tool("write_file", Risk::Write, None, false, None)
            .expect("authorize")
        {
            AuthorizeOutcome::NeedsApproval(request) => request,
            other => panic!("expected approval, got {other:?}"),
        };
        assert_eq!(
            s.approve(second.id, ApprovalDecision::Deny),
            Ok(ApprovalOutcome::Denied)
        );
        assert!(s.pending().is_empty());
    }

    #[test]
    fn unknown_approval_is_an_error() {
        let mut s = session(PolicyMode::Auto);
        assert_eq!(
            s.approve(42, ApprovalDecision::AllowOnce),
            Err(SessionError::UnknownApproval(42))
        );
    }

    #[test]
    fn yolo_needs_confirmation_once() {
        let mut s = session(PolicyMode::Auto);
        assert_eq!(
            s.request_mode(PolicyMode::Yolo),
            ModeChange::NeedsYoloConfirmation
        );
        assert_eq!(s.mode(), PolicyMode::Auto);
        s.confirm_yolo();
        assert_eq!(s.mode(), PolicyMode::Yolo);
        assert_eq!(s.request_mode(PolicyMode::Yolo), ModeChange::Applied);
    }

    #[test]
    fn turn_limit_and_cancel() {
        let mut s = session(PolicyMode::Auto).with_max_turns(2);
        assert_eq!(s.begin_turn(), Ok(1));
        assert_eq!(s.begin_turn(), Ok(2));
        assert_eq!(s.begin_turn(), Err(SessionError::TurnLimit { max: 2 }));

        s.cancel();
        assert!(s.is_cancelled());
        assert_eq!(s.begin_turn(), Err(SessionError::Cancelled));
        assert_eq!(
            s.authorize_tool("read_file", Risk::Read, None, true, None),
            Err(SessionError::Cancelled)
        );
    }

    #[test]
    fn session_mode_overrides_request_mode() {
        let mut s = session(PolicyMode::Plan);
        let request = PolicyRequest {
            mode: PolicyMode::Yolo,
            tool: "write_file",
            risk: Risk::Write,
            command: None,
            inside_workspace: true,
            model_risk_high: None,
        };
        assert!(matches!(
            s.authorize(&request),
            Ok(AuthorizeOutcome::Denied { .. })
        ));
    }

    #[test]
    fn context_budget_defaults_and_validates() {
        let s = session(PolicyMode::Auto);
        assert_eq!(
            (s.context_k(), s.compress_at_k()),
            (DEFAULT_CONTEXT_K, DEFAULT_COMPRESS_AT_K)
        );

        let s = s.with_context(200, 150);
        assert_eq!((s.context_k(), s.compress_at_k()), (200, 150));

        // 非法值保持已有预算，不产生除零或阈值倒挂。
        let s = s.with_context(0, 10);
        assert_eq!((s.context_k(), s.compress_at_k()), (200, 150));
        let s = s.with_context(100, 100);
        assert_eq!((s.context_k(), s.compress_at_k()), (200, 150));
        let s = s.with_context(100, 0);
        assert_eq!((s.context_k(), s.compress_at_k()), (200, 150));
    }
}
