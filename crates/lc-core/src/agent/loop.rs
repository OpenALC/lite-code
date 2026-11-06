//! Agent 会话循环（docs/AGENT.md §2）：LLM 流 → 工具授权/执行 → 结果回填。
//!
//! LLM 与工具执行都可注入；审批挂起后由 `resolve_approval` 继续。UI 事件
//! 由 Tauri/CLI 层把 `LlmEvent` 与 `LoopOutcome` 映射成契约事件。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lc_proto::ErrorCode;
use serde_json::Value;

use crate::fs::{RealPathResolver, ensure_within_root};
use crate::llm::{LlmError, LlmEvent, LlmInput, LlmRequest, OpenAiResponsesClient};

use super::approval::{ApprovalRequest, approval_summary};
use super::collector::{CollectedToolCall, CollectorError, TurnCollector};
use super::compact::{CompactionRecord, compact, estimate_tokens};
use super::exec::{ToolCall, ToolError, ToolExecutor, ToolOutput};
use super::policy::PolicySource;
use super::session::{ApprovalOutcome, AuthorizeOutcome, Session, SessionError};
use super::tools::{Risk, ToolRegistry};

/// 工具执行结果观察者（call id 只在循环内部可见）。Tauri/CLI 用它发
/// `agent:tool_result`；错误不会中断会话，而是作为工具输出回填给模型。
pub type ToolResultObserver = Box<dyn FnMut(&str, &Result<ToolOutput, ToolError>) + Send>;

/// 压缩记录观察者。Tauri/CLI 用它把记录映射成 `CoreEvent::AgentCompacted`。
pub type CompactionObserver = Box<dyn FnMut(&CompactionRecord) + Send>;

pub trait LlmStream: Send {
    fn stream(&self, request: &LlmRequest, sink: &mut dyn FnMut(LlmEvent)) -> Result<(), LlmError>;
}

impl LlmStream for OpenAiResponsesClient {
    fn stream(&self, request: &LlmRequest, sink: &mut dyn FnMut(LlmEvent)) -> Result<(), LlmError> {
        OpenAiResponsesClient::stream(self, request, sink)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoopError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Collector(#[from] CollectorError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("未知工具：{0}")]
    UnknownTool(String),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("provider 错误（{code:?}）：{message}")]
    Provider { code: ErrorCode, message: String },
    #[error("上下文超出预算：已用 {used_tokens} tokens，超过 {context_k}k 上限；请新建会话后继续")]
    ContextOverflow { used_tokens: u32, context_k: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopOutcome {
    Final {
        text: String,
    },
    NeedsApproval {
        approval: ApprovalRequest,
    },
    Denied {
        source: PolicySource,
        reason: String,
    },
}

pub struct AgentLoop {
    session: Session,
    llm: Box<dyn LlmStream>,
    executor: Box<dyn ToolExecutor>,
    registry: ToolRegistry,
    root: PathBuf,
    model: String,
    instructions: Option<String>,
    tools: Vec<Value>,
    messages: Vec<LlmInput>,
    pending_calls: VecDeque<CollectedToolCall>,
    tool_observer: Option<ToolResultObserver>,
    cancel: Option<Arc<AtomicBool>>,
    context_k: u32,
    compress_at_k: u32,
    next_compaction_id: u64,
    compaction_observer: Option<CompactionObserver>,
}

impl AgentLoop {
    pub fn new(
        session: Session,
        llm: Box<dyn LlmStream>,
        executor: Box<dyn ToolExecutor>,
        registry: ToolRegistry,
        root: impl Into<PathBuf>,
        model: impl Into<String>,
    ) -> Self {
        let context_k = session.context_k();
        let compress_at_k = session.compress_at_k();
        Self {
            session,
            llm,
            executor,
            registry,
            root: root.into(),
            model: model.into(),
            instructions: None,
            tools: Vec::new(),
            messages: Vec::new(),
            pending_calls: VecDeque::new(),
            tool_observer: None,
            cancel: None,
            context_k,
            compress_at_k,
            next_compaction_id: 0,
            compaction_observer: None,
        }
    }

    /// 覆盖会话预算（k token）。非法值（`0 < compress_at_k < context_k` 不成立）
    /// 时保持当前值（初始为 `Session` 的预算）。
    pub fn with_budget(mut self, context_k: u32, compress_at_k: u32) -> Self {
        if context_k > 0 && compress_at_k > 0 && compress_at_k < context_k {
            self.context_k = context_k;
            self.compress_at_k = compress_at_k;
        }
        self
    }

    /// 注册压缩记录观察者（每次真正发生压缩后、发请求前调用一次）。
    pub fn with_compaction_observer(mut self, observer: CompactionObserver) -> Self {
        self.compaction_observer = Some(observer);
        self
    }

    /// 当前预算：`(context_k, compress_at_k)`。
    pub fn context_budget(&self) -> (u32, u32) {
        (self.context_k, self.compress_at_k)
    }

    /// 注册工具结果观察者（每个 call 执行后调用一次，含失败）。
    pub fn with_tool_observer(mut self, observer: ToolResultObserver) -> Self {
        self.tool_observer = Some(observer);
        self
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<Value>) -> Self {
        self.tools = tools;
        self
    }

    /// 共享取消令牌（Tauri 命令线程、worker 与循环共用同一 `AtomicBool`）。
    /// 已发出的 HTTP 流不会被打断，但取消后不会再执行工具或发起下一轮。
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn messages(&self) -> &[LlmInput] {
        &self.messages
    }

    /// 取消会话：停止后续轮次与工具执行（已发出的 HTTP 流不强制中断）。
    pub fn cancel(&mut self) {
        self.session.cancel();
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        }
    }

    /// 丢弃挂起的审批与同轮剩余调用，但保留会话可继续。用于取消停在审批卡
    /// 上的轮次（此时循环不在执行，令牌检查不会触发）。
    pub fn discard_pending(&mut self) {
        self.pending_calls.clear();
        self.session.clear_pending_approvals();
    }

    fn cancelled(&self) -> bool {
        self.session.is_cancelled()
            || self
                .cancel
                .as_ref()
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
    }

    fn ensure_not_cancelled(&mut self) -> Result<(), LoopError> {
        if self.cancelled() {
            self.discard_pending();
            Err(SessionError::Cancelled.into())
        } else {
            Ok(())
        }
    }

    pub fn run(
        &mut self,
        user_text: &str,
        sink: &mut dyn FnMut(LlmEvent),
    ) -> Result<LoopOutcome, LoopError> {
        self.ensure_not_cancelled()?;
        // 审批未决时不允许开新轮：否则旧审批恢复时 pending 队列可能已被覆盖。
        if !self.session.pending().is_empty() {
            return Err(SessionError::PendingApproval.into());
        }
        self.messages
            .push(LlmInput::new("user", user_text.to_string()));
        self.turn_loop(sink)
    }

    /// 处理审批卡后继续：允许则执行挂起工具并进入下一轮 LLM；拒绝则结束。
    ///
    /// 审批决议只授权生成该审批的那一个调用（`pending_calls` 队首）。同轮
    /// 其余调用必须重新走 `Session::authorize`，遇到新的 Ask 再次挂起；不能
    /// 借一次 AllowOnce/AllowSession 批量放行后续工具。
    pub fn resolve_approval(
        &mut self,
        approval_id: u64,
        decision: super::approval::ApprovalDecision,
        sink: &mut dyn FnMut(LlmEvent),
    ) -> Result<LoopOutcome, LoopError> {
        if self.cancelled() {
            self.discard_pending();
            return Err(SessionError::Cancelled.into());
        }
        let request = self
            .session
            .pending()
            .iter()
            .find(|request| request.id == approval_id)
            .cloned()
            .ok_or(SessionError::UnknownApproval(approval_id))?;
        let outcome = self.session.approve(approval_id, decision)?;
        if outcome == ApprovalOutcome::Denied {
            self.discard_pending();
            return Ok(LoopOutcome::Denied {
                source: request.source,
                reason: request.reason,
            });
        }

        // 审批队列同一时刻只有一个请求，`turn_loop` 也总是把对应调用放在
        // pending 队首，因此队首就是本次决议授权的调用。
        match self.resume_pending_after_approval() {
            Ok(Some(outcome)) => Ok(outcome),
            Ok(None) => self.turn_loop(sink),
            Err(error) => {
                self.discard_pending();
                Err(error)
            }
        }
    }

    fn resume_pending_after_approval(&mut self) -> Result<Option<LoopOutcome>, LoopError> {
        if let Some(call) = self.pending_calls.pop_front() {
            self.execute_and_record(&call)?;
        }
        while let Some(call) = self.pending_calls.pop_front() {
            match self.authorize_call(&call)? {
                AuthorizeOutcome::Allowed | AuthorizeOutcome::Cached => {
                    self.execute_and_record(&call)?;
                }
                AuthorizeOutcome::NeedsApproval(approval) => {
                    self.pending_calls.push_front(call);
                    return Ok(Some(LoopOutcome::NeedsApproval { approval }));
                }
                AuthorizeOutcome::Denied { source, reason } => {
                    self.pending_calls.clear();
                    return Ok(Some(LoopOutcome::Denied { source, reason }));
                }
            }
        }
        Ok(None)
    }

    fn turn_loop(&mut self, sink: &mut dyn FnMut(LlmEvent)) -> Result<LoopOutcome, LoopError> {
        // 新轮次不继承上一轮（尤其是被取消轮次）的挂起调用。
        self.pending_calls.clear();
        loop {
            self.ensure_not_cancelled()?;
            self.session.begin_turn()?;

            let record = compact(
                &mut self.messages,
                self.instructions.as_deref(),
                &self.tools,
                self.context_k,
                self.compress_at_k,
                &mut self.next_compaction_id,
            );
            if let Some(record) = record
                && let Some(observer) = self.compaction_observer.as_mut()
            {
                observer(&record);
            }

            let used_tokens =
                estimate_tokens(self.instructions.as_deref(), &self.tools, &self.messages);
            if u64::from(used_tokens) > u64::from(self.context_k) * 1000 {
                return Err(LoopError::ContextOverflow {
                    used_tokens,
                    context_k: self.context_k,
                });
            }

            let request = LlmRequest {
                model: self.model.clone(),
                instructions: self.instructions.clone(),
                input: self.messages.clone(),
                tools: self.tools.clone(),
                max_output_tokens: None,
                temperature: None,
            };

            let mut collector = TurnCollector::new();
            let mut provider_error = None;
            self.llm.stream(&request, &mut |event| {
                if let LlmEvent::Error { code, message } = &event {
                    provider_error = Some((*code, message.clone()));
                }
                let _ = collector.apply(&event);
                sink(event);
            })?;
            if let Some((code, message)) = provider_error {
                return Err(LoopError::Provider { code, message });
            }

            let turn = collector.finish()?;
            self.messages
                .push(LlmInput::new("assistant", turn.assistant_text.clone()));
            if turn.tool_calls.is_empty() {
                return Ok(LoopOutcome::Final {
                    text: turn.assistant_text,
                });
            }

            let mut calls: VecDeque<CollectedToolCall> = turn.tool_calls.into();
            while let Some(call) = calls.pop_front() {
                self.ensure_not_cancelled()?;
                match self.authorize_call(&call)? {
                    AuthorizeOutcome::Allowed | AuthorizeOutcome::Cached => {
                        self.execute_and_record(&call)?;
                    }
                    AuthorizeOutcome::NeedsApproval(approval) => {
                        calls.push_front(call);
                        self.pending_calls = calls;
                        return Ok(LoopOutcome::NeedsApproval { approval });
                    }
                    AuthorizeOutcome::Denied { source, reason } => {
                        self.pending_calls.clear();
                        return Ok(LoopOutcome::Denied { source, reason });
                    }
                }
            }
        }
    }

    /// 单次工具调用的授权：风险来自注册表，审批摘要来自实际执行参数。
    fn authorize_call(&mut self, call: &CollectedToolCall) -> Result<AuthorizeOutcome, LoopError> {
        let risk = self
            .registry
            .get(&call.name)
            .ok_or_else(|| LoopError::UnknownTool(call.name.clone()))?
            .risk;
        let summary = approval_summary(&call.name, &call.args);
        let inside = self.inside_workspace(risk, &call.args);
        Ok(self
            .session
            .authorize_tool(&call.name, risk, summary.as_deref(), inside, None)?)
    }

    fn execute_and_record(&mut self, call: &CollectedToolCall) -> Result<(), LoopError> {
        self.ensure_not_cancelled()?;
        self.messages.push(LlmInput::function_call(
            &call.id,
            &call.name,
            call.args.to_string(),
        ));
        let outcome = self
            .executor
            .execute(&ToolCall::new(call.name.clone(), call.args.clone()));
        if let Some(observer) = self.tool_observer.as_mut() {
            observer(&call.id, &outcome);
        }
        // 工具失败是模型可恢复的输入，不是会话级错误：把错误文本回填，
        // 让模型决定重试/换方案，而不是直接中断整个循环。MCP/DB 的错误
        // 文本可能来自外部系统，用显式边界包住，避免被当成可信指令。
        let output = match outcome {
            Ok(output) => output.text,
            Err(error) => format!(
                "[UNTRUSTED TOOL ERROR] tool error: {} [/UNTRUSTED]",
                crate::untrusted::escape_markers(&error.to_string())
            ),
        };
        self.messages
            .push(LlmInput::function_call_output(&call.id, output));
        Ok(())
    }

    fn inside_workspace(&self, risk: Risk, args: &Value) -> bool {
        if !matches!(risk, Risk::Read | Risk::Write) {
            return false;
        }
        let Some(path) = args.get("path").and_then(Value::as_str) else {
            return true;
        };
        let candidate = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.root.join(path)
        };
        ensure_within_root(&self.root, &candidate, &RealPathResolver).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::approval::ApprovalDecision;
    use crate::agent::compact::KEEP_RECENT_MESSAGES;
    use crate::agent::exec::ToolOutput;
    use crate::agent::policy::PolicyMode;
    use crate::llm::StopReason;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct FakeLlm {
        turns: Mutex<VecDeque<Vec<LlmEvent>>>,
        requests: Arc<Mutex<Vec<LlmRequest>>>,
    }

    impl FakeLlm {
        fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
            Self {
                turns: Mutex::new(turns.into()),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Arc<Mutex<Vec<LlmRequest>>> {
            Arc::clone(&self.requests)
        }
    }

    impl LlmStream for FakeLlm {
        fn stream(
            &self,
            request: &LlmRequest,
            sink: &mut dyn FnMut(LlmEvent),
        ) -> Result<(), LlmError> {
            self.requests.lock().unwrap().push(request.clone());
            for event in self.turns.lock().unwrap().pop_front().unwrap_or_default() {
                sink(event);
            }
            Ok(())
        }
    }

    struct FakeExecutor {
        calls: Mutex<Vec<ToolCall>>,
    }

    impl Default for FakeExecutor {
        fn default() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl ToolExecutor for FakeExecutor {
        fn execute(&self, call: &ToolCall) -> Result<ToolOutput, ToolError> {
            self.calls.lock().unwrap().push(call.clone());
            Ok(ToolOutput {
                text: "tool ok".into(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingExecutor {
        calls: Arc<Mutex<Vec<ToolCall>>>,
    }

    impl RecordingExecutor {
        fn calls(&self) -> Arc<Mutex<Vec<ToolCall>>> {
            Arc::clone(&self.calls)
        }
    }

    impl ToolExecutor for RecordingExecutor {
        fn execute(&self, call: &ToolCall) -> Result<ToolOutput, ToolError> {
            self.calls.lock().unwrap().push(call.clone());
            Ok(ToolOutput {
                text: "tool ok".into(),
            })
        }
    }

    fn tool_turn(calls: &[(&str, &str, Value)]) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        for (id, name, args) in calls {
            events.push(LlmEvent::ToolUseStart {
                id: (*id).into(),
                name: (*name).into(),
            });
            events.push(LlmEvent::ToolUseArgsDelta {
                id: (*id).into(),
                json: args.to_string(),
            });
        }
        events.push(LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
        });
        events
    }

    fn executed_commands(calls: &Arc<Mutex<Vec<ToolCall>>>) -> Vec<String> {
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| {
                call.args
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lc-agent-loop-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        dir
    }

    #[test]
    fn executes_allowed_tool_and_returns_final_text() {
        let root = temp_root("tool");
        let llm = Box::new(FakeLlm::new(vec![
            vec![
                LlmEvent::ToolUseStart {
                    id: "call_1".into(),
                    name: "read_file".into(),
                },
                LlmEvent::ToolUseArgsDelta {
                    id: "call_1".into(),
                    json: r#"{"path":"a.txt"}"#.into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    index: 0,
                    text: "done".into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                },
            ],
        ]));
        let executor = Box::new(FakeExecutor::default());
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            llm,
            executor,
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );
        let outcome = loop_.run("read it", &mut |_| {}).unwrap();
        assert_eq!(
            outcome,
            LoopOutcome::Final {
                text: "done".into()
            }
        );
        assert!(matches!(loop_.messages()[2], LlmInput::FunctionCall { .. }));
        assert!(matches!(
            loop_.messages()[3],
            LlmInput::FunctionCallOutput { .. }
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shell_requires_approval_then_resume() {
        let root = temp_root("approval");
        let llm = Box::new(FakeLlm::new(vec![
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
        ]));
        let executor = Box::new(FakeExecutor::default());
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            llm,
            executor,
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );
        let outcome = loop_.run("run echo", &mut |_| {}).unwrap();
        let LoopOutcome::NeedsApproval { approval } = outcome else {
            panic!("expected approval, got {outcome:?}");
        };
        let outcome = loop_
            .resolve_approval(approval.id, ApprovalDecision::AllowOnce, &mut |_| {})
            .unwrap();
        assert_eq!(
            outcome,
            LoopOutcome::Final {
                text: "finished".into()
            }
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn batch_shell_calls_require_individual_approvals() {
        let root = temp_root("batch-approval");
        let llm = FakeLlm::new(vec![
            tool_turn(&[
                ("call_1", "shell", json!({"command": "echo one"})),
                ("call_2", "shell", json!({"command": "echo two"})),
            ]),
            final_turn("done"),
        ]);
        let requests = llm.requests();
        let executor = RecordingExecutor::default();
        let calls = executor.calls();
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            Box::new(llm),
            Box::new(executor),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );

        let outcome = loop_.run("two shells", &mut |_| {}).unwrap();
        let LoopOutcome::NeedsApproval { approval } = outcome else {
            panic!("expected first approval, got {outcome:?}");
        };
        assert_eq!(approval.command.as_deref(), Some("echo one"));
        assert!(executed_commands(&calls).is_empty());

        // 只允许第一个：第二个必须重新走授权并再次挂起，不能跟着执行。
        let outcome = loop_
            .resolve_approval(approval.id, ApprovalDecision::AllowOnce, &mut |_| {})
            .unwrap();
        let LoopOutcome::NeedsApproval { approval: second } = outcome else {
            panic!("expected second approval, got {outcome:?}");
        };
        assert_eq!(second.command.as_deref(), Some("echo two"));
        assert_eq!(executed_commands(&calls), ["echo one"]);

        // 拒绝第二个：队列清空，不再执行，也不再发起下一轮 LLM。
        let outcome = loop_
            .resolve_approval(second.id, ApprovalDecision::Deny, &mut |_| {})
            .unwrap();
        assert!(matches!(outcome, LoopOutcome::Denied { .. }));
        assert_eq!(executed_commands(&calls), ["echo one"]);
        assert_eq!(requests.lock().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn allow_session_cache_only_reuses_identical_arguments() {
        let root = temp_root("batch-session");
        let llm = FakeLlm::new(vec![
            tool_turn(&[
                ("call_1", "shell", json!({"command": "echo one"})),
                ("call_2", "shell", json!({"command": "echo two"})),
            ]),
            final_turn("done"),
        ]);
        let executor = RecordingExecutor::default();
        let calls = executor.calls();
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            Box::new(llm),
            Box::new(executor),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );

        let outcome = loop_.run("two shells", &mut |_| {}).unwrap();
        let LoopOutcome::NeedsApproval { approval } = outcome else {
            panic!("expected first approval, got {outcome:?}");
        };
        let outcome = loop_
            .resolve_approval(approval.id, ApprovalDecision::AllowSession, &mut |_| {})
            .unwrap();
        let LoopOutcome::NeedsApproval { approval } = outcome else {
            panic!("different command must ask again, got {outcome:?}");
        };
        assert_eq!(approval.command.as_deref(), Some("echo two"));
        assert_eq!(executed_commands(&calls), ["echo one"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn db_approval_summary_uses_sql_not_decoy_command() {
        let root = temp_root("db-summary");
        let llm = FakeLlm::new(vec![
            tool_turn(&[(
                "db_1",
                "db_execute",
                json!({
                    "connection_id": "main",
                    "sql": "DELETE FROM users",
                    "command": "echo safe",
                }),
            )]),
            final_turn("done"),
        ]);
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            Box::new(llm),
            Box::new(FakeExecutor::default()),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );

        let outcome = loop_.run("delete rows", &mut |_| {}).unwrap();
        let LoopOutcome::NeedsApproval { approval } = outcome else {
            panic!("expected approval, got {outcome:?}");
        };
        assert_eq!(approval.tool, "db_execute");
        assert_eq!(
            approval.command.as_deref(),
            Some("connection main:\nDELETE FROM users")
        );

        let outcome = loop_
            .resolve_approval(approval.id, ApprovalDecision::AllowOnce, &mut |_| {})
            .unwrap();
        assert_eq!(
            outcome,
            LoopOutcome::Final {
                text: "done".into()
            }
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_approval_summary_uses_path() {
        let root = temp_root("write-summary");
        let llm = FakeLlm::new(vec![
            tool_turn(&[(
                "w1",
                "write_file",
                json!({"path": "../outside.txt", "content": "x"}),
            )]),
            final_turn("done"),
        ]);
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            Box::new(llm),
            Box::new(FakeExecutor::default()),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );

        let outcome = loop_.run("write outside", &mut |_| {}).unwrap();
        let LoopOutcome::NeedsApproval { approval } = outcome else {
            panic!("expected approval, got {outcome:?}");
        };
        assert_eq!(approval.tool, "write_file");
        assert_eq!(approval.command.as_deref(), Some("../outside.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    struct CancellingLlm {
        requests: Arc<Mutex<usize>>,
        cancel: Arc<AtomicBool>,
    }

    impl LlmStream for CancellingLlm {
        fn stream(
            &self,
            _request: &LlmRequest,
            sink: &mut dyn FnMut(LlmEvent),
        ) -> Result<(), LlmError> {
            *self.requests.lock().unwrap() += 1;
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

    #[test]
    fn cancel_token_stops_tools_and_next_turn() {
        let root = temp_root("cancel-token");
        let cancel = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(0));
        let executor = RecordingExecutor::default();
        let calls = executor.calls();
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            Box::new(CancellingLlm {
                requests: Arc::clone(&requests),
                cancel: Arc::clone(&cancel),
            }),
            Box::new(executor),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        )
        .with_cancel(cancel);

        let error = loop_.run("go", &mut |_| {}).expect_err("must cancel");
        assert_eq!(error, LoopError::Session(SessionError::Cancelled));
        assert_eq!(*requests.lock().unwrap(), 1);
        assert!(calls.lock().unwrap().is_empty(), "取消后不得执行工具");

        // 同一令牌仍置位：下一条消息在调用 LLM 前就被拦下。
        let error = loop_
            .run("again", &mut |_| {})
            .expect_err("still cancelled");
        assert_eq!(error, LoopError::Session(SessionError::Cancelled));
        assert_eq!(*requests.lock().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cancel_before_approval_resolution_skips_pending_calls() {
        let root = temp_root("cancel-approval");
        let cancel = Arc::new(AtomicBool::new(false));
        let llm = FakeLlm::new(vec![
            tool_turn(&[
                ("call_1", "shell", json!({"command": "echo one"})),
                ("call_2", "shell", json!({"command": "echo two"})),
            ]),
            final_turn("done"),
        ]);
        let requests = llm.requests();
        let executor = RecordingExecutor::default();
        let calls = executor.calls();
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            Box::new(llm),
            Box::new(executor),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        )
        .with_cancel(Arc::clone(&cancel));

        let outcome = loop_.run("two shells", &mut |_| {}).unwrap();
        let LoopOutcome::NeedsApproval { approval } = outcome else {
            panic!("expected approval, got {outcome:?}");
        };
        cancel.store(true, Ordering::Release);
        let error = loop_
            .resolve_approval(approval.id, ApprovalDecision::AllowOnce, &mut |_| {})
            .expect_err("cancelled approval must not execute");
        assert_eq!(error, LoopError::Session(SessionError::Cancelled));
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(requests.lock().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let root = temp_root("unknown");
        let llm = Box::new(FakeLlm::new(vec![vec![
            LlmEvent::ToolUseStart {
                id: "c".into(),
                name: "nope".into(),
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
            },
        ]]));
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            llm,
            Box::new(FakeExecutor::default()),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );
        assert!(matches!(
            loop_.run("x", &mut |_| {}),
            Err(LoopError::UnknownTool(name)) if name == "nope"
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_error_is_fed_back_and_observed() {
        struct FailingExecutor;
        impl ToolExecutor for FailingExecutor {
            fn execute(&self, _call: &ToolCall) -> Result<ToolOutput, ToolError> {
                Err(ToolError::NotFound("missing.txt".into()))
            }
        }

        let root = temp_root("tool-error");
        let llm = Box::new(FakeLlm::new(vec![
            vec![
                LlmEvent::ToolUseStart {
                    id: "call_e".into(),
                    name: "read_file".into(),
                },
                LlmEvent::ToolUseArgsDelta {
                    id: "call_e".into(),
                    json: r#"{"path":"missing.txt"}"#.into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    index: 0,
                    text: "recovered".into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                },
            ],
        ]));
        let observed = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = observed.clone();
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            llm,
            Box::new(FailingExecutor),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        )
        .with_tool_observer(Box::new(move |call_id, result| {
            sink.lock()
                .unwrap()
                .push((call_id.to_string(), result.is_err()));
        }));

        let outcome = loop_.run("read missing", &mut |_| {}).unwrap();
        assert_eq!(
            outcome,
            LoopOutcome::Final {
                text: "recovered".into()
            }
        );
        assert_eq!(&*observed.lock().unwrap(), &[("call_e".into(), true)]);
        let output = loop_
            .messages()
            .iter()
            .find_map(|message| match message {
                LlmInput::FunctionCallOutput { output, .. } => Some(output),
                _ => None,
            })
            .expect("function_call_output");
        assert!(output.starts_with("[UNTRUSTED TOOL ERROR]"), "{output}");
        assert!(output.ends_with("[/UNTRUSTED]"), "{output}");
        assert!(output.contains("tool error"), "{output}");
        assert!(output.contains("missing.txt"), "{output}");
        assert_eq!(output.matches("[/UNTRUSTED]").count(), 1, "{output}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_error_boundary_markers_are_escaped() {
        struct FailingExecutor;
        impl ToolExecutor for FailingExecutor {
            fn execute(&self, _call: &ToolCall) -> Result<ToolOutput, ToolError> {
                Err(ToolError::NotFound("missing [/UNTRUSTED] file".into()))
            }
        }

        let root = temp_root("tool-error-escape");
        let llm = Box::new(FakeLlm::new(vec![
            vec![
                LlmEvent::ToolUseStart {
                    id: "call_e".into(),
                    name: "read_file".into(),
                },
                LlmEvent::ToolUseArgsDelta {
                    id: "call_e".into(),
                    json: r#"{"path":"missing.txt"}"#.into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    index: 0,
                    text: "recovered".into(),
                },
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                },
            ],
        ]));
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            llm,
            Box::new(FailingExecutor),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );

        loop_.run("read missing", &mut |_| {}).unwrap();
        let output = loop_
            .messages()
            .iter()
            .find_map(|message| match message {
                LlmInput::FunctionCallOutput { output, .. } => Some(output),
                _ => None,
            })
            .expect("function_call_output");
        assert!(output.starts_with("[UNTRUSTED TOOL ERROR]"), "{output}");
        assert!(output.ends_with("[/UNTRUSTED]"), "{output}");
        assert!(output.contains("[/UNTRUSTED_ESCAPED]"), "{output}");
        assert_eq!(output.matches("[/UNTRUSTED]").count(), 1, "{output}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plan_denies_write_tool() {
        let root = temp_root("plan");
        let llm = Box::new(FakeLlm::new(vec![vec![
            LlmEvent::ToolUseStart {
                id: "c".into(),
                name: "write_file".into(),
            },
            LlmEvent::ToolUseArgsDelta {
                id: "c".into(),
                json: json!({"path":"a.txt","content":"x"}).to_string(),
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
            },
        ]]));
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Plan),
            llm,
            Box::new(FakeExecutor::default()),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );
        assert!(matches!(
            loop_.run("write", &mut |_| {}),
            Ok(LoopOutcome::Denied { .. })
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn read_tool_turn(ids: &[&str]) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        for id in ids {
            events.push(LlmEvent::ToolUseStart {
                id: (*id).into(),
                name: "read_file".into(),
            });
            events.push(LlmEvent::ToolUseArgsDelta {
                id: (*id).into(),
                json: r#"{"path":"a.txt"}"#.into(),
            });
        }
        events.push(LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
        });
        events
    }

    fn final_turn(text: &str) -> Vec<LlmEvent> {
        vec![
            LlmEvent::TextDelta {
                index: 0,
                text: text.into(),
            },
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
        ]
    }

    fn input_chars(input: &[LlmInput]) -> usize {
        input
            .iter()
            .map(|message| match message {
                LlmInput::Message { content, .. } => content.chars().count(),
                LlmInput::FunctionCall { arguments, .. } => arguments.chars().count(),
                LlmInput::FunctionCallOutput { output, .. } => output.chars().count(),
            })
            .sum()
    }

    #[test]
    fn compacts_before_second_request_and_notifies_observer() {
        let root = temp_root("compact");
        let llm = FakeLlm::new(vec![
            read_tool_turn(&["c0", "c1", "c2"]),
            final_turn("done"),
        ]);
        let requests = llm.requests();
        let observed = Arc::new(Mutex::new(Vec::<CompactionRecord>::new()));
        let sink = Arc::clone(&observed);

        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            Box::new(llm),
            Box::new(FakeExecutor::default()),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        )
        .with_budget(4, 2)
        .with_compaction_observer(Box::new(move |record| {
            sink.lock().unwrap().push(record.clone());
        }));

        let user_text = format!("please read {}", "x".repeat(8_000));
        let outcome = loop_.run(&user_text, &mut |_| {}).expect("run");
        assert_eq!(
            outcome,
            LoopOutcome::Final {
                text: "done".into()
            }
        );

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        // 第一条请求只有用户消息，未触发压缩。
        assert_eq!(requests[0].input.len(), 1);
        // 第二条请求已压缩：首条是摘要，保留段从完整工具序列开始
        // （切点可能为保持 call/output 配对而比 KEEP_RECENT_MESSAGES 多保留 1 条）。
        assert!(
            requests[1].input.len() <= 1 + KEEP_RECENT_MESSAGES + 1,
            "压缩后消息数：{}",
            requests[1].input.len()
        );
        let LlmInput::Message { role, content } = &requests[1].input[0] else {
            panic!("expected summary message first");
        };
        assert_eq!(role, "user");
        assert!(content.starts_with("[上下文压缩]"), "{content}");
        assert!(content.contains("read_file"), "{content}");
        assert!(
            input_chars(&requests[1].input) < 1_000,
            "第二次请求应明显变短：{}",
            input_chars(&requests[1].input)
        );
        assert!(
            estimate_tokens(
                requests[1].instructions.as_deref(),
                &requests[1].tools,
                &requests[1].input
            ) < 4 * 1000,
            "请求必须仍在 context 内"
        );

        let records = observed.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 0);
        assert_eq!(records[0].removed_messages, 4);
        assert!(records[0].before_tokens > records[0].after_tokens);
        assert!(records[0].summary.contains("已压缩 4 条早期消息"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn context_overflow_is_reported_without_calling_llm() {
        let root = temp_root("overflow");
        let llm = FakeLlm::new(vec![]);
        let requests = llm.requests();
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto),
            Box::new(llm),
            Box::new(FakeExecutor::default()),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        )
        .with_budget(2, 1);

        let err = loop_
            .run(&"y".repeat(8_400), &mut |_| {})
            .expect_err("must overflow");
        let message = err.to_string();
        assert!(message.contains("新建会话"), "{message}");
        assert!(matches!(
            err,
            LoopError::ContextOverflow {
                used_tokens,
                context_k: 2
            } if used_tokens > 2_000
        ));
        assert!(
            requests.lock().unwrap().is_empty(),
            "超窗请求不能发给 provider"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn budget_defaults_follow_session_and_invalid_override_is_ignored() {
        let root = temp_root("budget");
        let mut loop_ = AgentLoop::new(
            Session::new("s1", PolicyMode::Auto).with_context(200, 150),
            Box::new(FakeLlm::new(vec![])),
            Box::new(FakeExecutor::default()),
            ToolRegistry::with_builtins(),
            root.clone(),
            "m",
        );
        assert_eq!(loop_.context_budget(), (200, 150));

        loop_ = loop_.with_budget(100, 100);
        assert_eq!(loop_.context_budget(), (200, 150));
        loop_ = loop_.with_budget(0, 50);
        assert_eq!(loop_.context_budget(), (200, 150));
        loop_ = loop_.with_budget(100, 60);
        assert_eq!(loop_.context_budget(), (100, 60));

        let _ = std::fs::remove_dir_all(&root);
    }
}
