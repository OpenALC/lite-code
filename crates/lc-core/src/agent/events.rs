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
}
