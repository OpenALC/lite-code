//! 核心 Agent 事件 → 契约事件（lc-proto v0.3）。
//!
//! LLM 流事件与循环终态在这里统一映射，Tauri 与 CLI 只负责传输，
//! 避免两端对事件语义各自解释。

use lc_proto::{ApprovalPrompt, ApprovalSource, CoreEvent, ErrorBody, SessionInfo, SessionMode};

use super::approval::ApprovalRequest;
use super::r#loop::LoopOutcome;
use super::policy::{PolicyMode, PolicySource};
use super::session::Session;
use crate::llm::LlmEvent;

pub fn mode_from_contract(mode: SessionMode) -> PolicyMode {
    match mode {
        SessionMode::Plan => PolicyMode::Plan,
        SessionMode::Chat => PolicyMode::Chat,
        SessionMode::Auto => PolicyMode::Auto,
        SessionMode::Yolo => PolicyMode::Yolo,
    }
}

pub fn mode_to_contract(mode: PolicyMode) -> SessionMode {
    match mode {
        PolicyMode::Plan => SessionMode::Plan,
        PolicyMode::Chat => SessionMode::Chat,
        PolicyMode::Auto => SessionMode::Auto,
        PolicyMode::Yolo => SessionMode::Yolo,
    }
}

fn approval_source(source: PolicySource) -> ApprovalSource {
    match source {
        PolicySource::Mode => ApprovalSource::Mode,
        PolicySource::StaticBlacklist => ApprovalSource::StaticBlacklist,
        PolicySource::UserBlacklist => ApprovalSource::UserBlacklist,
        PolicySource::ModelRisk => ApprovalSource::ModelRisk,
        PolicySource::OutsideWorkspace => ApprovalSource::OutsideWorkspace,
        PolicySource::NetworkTool => ApprovalSource::NetworkTool,
        PolicySource::ExecuteReview => ApprovalSource::ExecuteReview,
    }
}

pub fn session_info(session: &Session) -> SessionInfo {
    SessionInfo {
        session_id: session.id.clone(),
        mode: mode_to_contract(session.mode()),
        turn: session.turn(),
        max_turns: session.max_turns(),
        context_k: session.context_k(),
        compress_at_k: session.compress_at_k(),
        cancelled: session.is_cancelled(),
    }
}

pub fn approval_prompt(request: &ApprovalRequest) -> ApprovalPrompt {
    ApprovalPrompt {
        approval_id: request.id,
        tool: request.tool.clone(),
        command: request.command.clone(),
        source: approval_source(request.source),
        reason: request.reason.clone(),
    }
}

/// LLM 流事件映射。`ToolUseArgsDelta` 与 `Done` 不直接下发：
/// 前者只用于参数拼装，后者由 `LoopOutcome` 决定最终 `agent:done`。
pub fn llm_event(session_id: &str, event: &LlmEvent) -> Option<CoreEvent> {
    Some(match event {
        LlmEvent::TextDelta { index, text } => CoreEvent::AgentTextDelta {
            session_id: session_id.into(),
            index: *index,
            text: text.clone(),
        },
        LlmEvent::ThinkingDelta { text } => CoreEvent::AgentThinkingDelta {
            session_id: session_id.into(),
            text: text.clone(),
        },
        LlmEvent::ToolUseStart { id, name } => CoreEvent::AgentToolStart {
            session_id: session_id.into(),
            call_id: id.clone(),
            name: name.clone(),
        },
        LlmEvent::ToolUseArgsDelta { .. } | LlmEvent::Done { .. } => return None,
        LlmEvent::Usage {
            input,
            output,
            cached,
        } => CoreEvent::AgentUsage {
            session_id: session_id.into(),
            input: *input,
            output: *output,
            cached: *cached,
        },
        LlmEvent::Error { code, message } => CoreEvent::Error {
            error: ErrorBody::new(*code, message.clone()),
        },
    })
}

/// 循环终态映射。拒绝使用契约错误码 `permission_denied`，
/// 让 UI 与 headless CLI 都能区分“策略拒绝”和网络/解析失败。
pub fn loop_outcome(session_id: &str, outcome: &LoopOutcome) -> Vec<CoreEvent> {
    match outcome {
        LoopOutcome::Final { .. } => vec![CoreEvent::AgentDone {
            session_id: session_id.into(),
            stop_reason: "end_turn".into(),
        }],
        LoopOutcome::NeedsApproval { approval } => vec![CoreEvent::AgentApprovalRequired {
            session_id: session_id.into(),
            approval: approval_prompt(approval),
        }],
        LoopOutcome::Denied { reason, .. } => vec![CoreEvent::Error {
            error: ErrorBody::new(lc_proto::ErrorCode::PermissionDenied, reason.clone()),
        }],
    }
}

/// 一轮正常结束（Ok 分支）的完成事件序列：`session_state` 快照在前，
/// `loop_outcome` 事件随后。两端共享的排序契约（M3b 从桌面 `agent.rs`
/// 下移）；鸿蒙 lc-ohos 的 Ok 分支因 `session_state` 写回与 Plan 模式
/// 强约束交织在 emitter 内、不批量组装，且 plan 模式收到 NeedsApproval/
/// Denied 时补发 `agent:done`（桌面审批挂起不补，busy 保持）——差异留在
/// 宿主，本函数只钉住公共前缀顺序。
pub fn completion_events(session: &Session, outcome: &LoopOutcome) -> Vec<CoreEvent> {
    let mut events = vec![CoreEvent::AgentSessionState {
        session: session_info(session),
    }];
    events.extend(loop_outcome(&session.id, outcome));
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ApprovalRequest, PolicySource};

    #[test]
    fn modes_roundtrip_between_contract_and_core() {
        for mode in [
            PolicyMode::Plan,
            PolicyMode::Chat,
            PolicyMode::Auto,
            PolicyMode::Yolo,
        ] {
            assert_eq!(mode_from_contract(mode_to_contract(mode)), mode);
        }
    }

    #[test]
    fn llm_deltas_map_to_agent_events() {
        let text = llm_event(
            "s1",
            &LlmEvent::TextDelta {
                index: 3,
                text: "hi".into(),
            },
        )
        .unwrap();
        assert_eq!(
            text,
            CoreEvent::AgentTextDelta {
                session_id: "s1".into(),
                index: 3,
                text: "hi".into()
            }
        );

        let usage = llm_event(
            "s1",
            &LlmEvent::Usage {
                input: 10,
                output: 2,
                cached: 4,
            },
        )
        .unwrap();
        assert_eq!(
            usage,
            CoreEvent::AgentUsage {
                session_id: "s1".into(),
                input: 10,
                output: 2,
                cached: 4
            }
        );

        assert!(
            llm_event(
                "s1",
                &LlmEvent::Done {
                    stop_reason: crate::llm::StopReason::EndTurn
                }
            )
            .is_none()
        );
    }

    #[test]
    fn approval_prompt_carries_full_command_and_source() {
        let request = ApprovalRequest {
            id: 7,
            tool: "shell".into(),
            command: Some("rm -rf build".into()),
            source: PolicySource::ExecuteReview,
            reason: "执行类工具需确认".into(),
        };
        assert_eq!(
            approval_prompt(&request),
            ApprovalPrompt {
                approval_id: 7,
                tool: "shell".into(),
                command: Some("rm -rf build".into()),
                source: ApprovalSource::ExecuteReview,
                reason: "执行类工具需确认".into(),
            }
        );
    }

    #[test]
    fn denial_maps_to_permission_denied_error() {
        let events = loop_outcome(
            "s1",
            &LoopOutcome::Denied {
                source: PolicySource::Mode,
                reason: "plan 模式禁止写入".into(),
            },
        );
        assert_eq!(
            events,
            vec![CoreEvent::Error {
                error: ErrorBody::new(lc_proto::ErrorCode::PermissionDenied, "plan 模式禁止写入")
            }]
        );
    }

    #[test]
    fn final_maps_to_done_and_session_info_is_contract_shaped() {
        let session = Session::new("s1", PolicyMode::Auto).with_max_turns(5);
        let info = session_info(&session);
        assert_eq!(info.session_id, "s1");
        assert_eq!(info.mode, SessionMode::Auto);
        assert_eq!(info.max_turns, 5);
        assert!(!info.cancelled);
        assert_eq!(
            loop_outcome("s1", &LoopOutcome::Final { text: "ok".into() }),
            vec![CoreEvent::AgentDone {
                session_id: "s1".into(),
                stop_reason: "end_turn".into()
            }]
        );
    }

    #[test]
    fn completion_events_put_session_state_first_for_all_outcomes() {
        let session = Session::new("s1", PolicyMode::Auto).with_max_turns(5);

        let final_events = completion_events(&session, &LoopOutcome::Final { text: "ok".into() });
        assert!(matches!(
            &final_events[0],
            CoreEvent::AgentSessionState { session } if session.session_id == "s1"
        ));
        assert_eq!(
            final_events[1],
            CoreEvent::AgentDone {
                session_id: "s1".into(),
                stop_reason: "end_turn".into(),
            }
        );

        let approval = LoopOutcome::NeedsApproval {
            approval: ApprovalRequest {
                id: 7,
                tool: "shell".into(),
                command: Some("echo hi".into()),
                source: PolicySource::ExecuteReview,
                reason: "review".into(),
            },
        };
        let approval_events = completion_events(&session, &approval);
        assert!(matches!(
            &approval_events[1],
            CoreEvent::AgentApprovalRequired { approval, .. } if approval.approval_id == 7
        ));
        // 共享契约：审批挂起只发 session_state + approval_required，不补
        // agent:done（桌面 busy 保持到 resolve 之后；鸿蒙 plan 模式的 done 由
        // 宿主按自己的语义补发）。
        assert_eq!(approval_events.len(), 2);

        let denied = completion_events(
            &session,
            &LoopOutcome::Denied {
                source: PolicySource::Mode,
                reason: "plan 模式禁止写入".into(),
            },
        );
        assert!(matches!(
            &denied[1],
            CoreEvent::Error { error } if error.code == lc_proto::ErrorCode::PermissionDenied
        ));
        assert_eq!(denied.len(), 2);
    }
}
