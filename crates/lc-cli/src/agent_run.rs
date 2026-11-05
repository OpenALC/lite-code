//! `agent run` 的可单测逻辑：事件编码、人类可读渲染、审批策略、
//! 退出码与工作区校验。I/O 与参数解析留在 `main.rs`。

use std::path::{Path, PathBuf};

use lc_core::agent::{
    ApprovalDecision, ApprovalRequest, LoopError, LoopOutcome, PolicyMode, Session, ToolError,
    ToolOutput,
};
use lc_proto::{CoreEvent, ErrorBody, ErrorCode, EventEnvelope};

/// 默认系统指令：工具路径相对工作区根，先调查再修改。
pub const DEFAULT_INSTRUCTIONS: &str =
    "你是 lite-code 的编码 agent。工具路径相对工作区根；先用工具调查，再修改。";

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_POLICY_DENIED: i32 = 2;

/// 人类模式下工具结果的最大展示字符数。
pub const TOOL_PREVIEW_CHARS: usize = 500;

/// stdout 的 JSON 事件行写入器：`seq` 从 1 单调递增。
#[derive(Debug, Default)]
pub struct JsonEventLines {
    seq: u64,
}

impl JsonEventLines {
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回一行 JSON（不含换行符），并推进序号。
    pub fn next_line(&mut self, event: &CoreEvent) -> String {
        self.seq += 1;
        let envelope = EventEnvelope {
            seq: self.seq,
            event: event.clone(),
        };
        serde_json::to_string(&envelope).expect("EventEnvelope 序列化不可能失败")
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// 人类模式下一份事件应该写到哪个流。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanOutput {
    Nothing,
    Stdout(String),
    Stderr(String),
}

/// 事件 → 人类可读输出。文本增量不附加换行，保持流式原样；
/// 思考增量只在 `--show-thinking` 时写 stderr。
pub fn human_output(event: &CoreEvent, show_thinking: bool) -> HumanOutput {
    match event {
        CoreEvent::AgentTextDelta { text, .. } => HumanOutput::Stdout(text.clone()),
        CoreEvent::AgentThinkingDelta { text, .. } => {
            if show_thinking {
                HumanOutput::Stderr(text.clone())
            } else {
                HumanOutput::Nothing
            }
        }
        CoreEvent::AgentToolStart { name, .. } => HumanOutput::Stderr(format!("[tool] {name}\n")),
        CoreEvent::AgentToolResult { ok, text, .. } => {
            let label = if *ok { "ok" } else { "error" };
            let shown = truncate_text(text, TOOL_PREVIEW_CHARS);
            HumanOutput::Stderr(format!("[tool {label}] {}\n", shown.trim_end()))
        }
        CoreEvent::AgentUsage {
            input,
            output,
            cached,
            ..
        } => HumanOutput::Stderr(format!(
            "[usage] input={input} output={output} cached={cached}\n"
        )),
        CoreEvent::Error { error } => HumanOutput::Stderr(format!("[error] {}\n", error.message)),
        // 审批由调用方打印完整字段；done/session 等事件只占 JSON 流。
        CoreEvent::AgentApprovalRequired { .. }
        | CoreEvent::AgentDone { .. }
        | CoreEvent::AgentSessionState { .. } => HumanOutput::Nothing,
        // 非 Agent 事件不会出现在 agent run 中。
        _ => HumanOutput::Nothing,
    }
}

/// 工具执行结果 → 契约事件（JSON 模式全量输出，不截断）。
pub fn tool_result_event(
    session_id: &str,
    call_id: &str,
    result: &Result<ToolOutput, ToolError>,
) -> CoreEvent {
    match result {
        Ok(output) => CoreEvent::AgentToolResult {
            session_id: session_id.into(),
            call_id: call_id.into(),
            ok: true,
            text: output.text.clone(),
        },
        Err(error) => CoreEvent::AgentToolResult {
            session_id: session_id.into(),
            call_id: call_id.into(),
            ok: false,
            text: error.to_string(),
        },
    }
}

/// 命令行审批策略：`--yes` 允许一次，否则拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAction {
    AllowOnce,
    Deny,
}

pub fn approval_action(yes: bool) -> ApprovalAction {
    if yes {
        ApprovalAction::AllowOnce
    } else {
        ApprovalAction::Deny
    }
}

impl ApprovalAction {
    pub fn decision(self) -> ApprovalDecision {
        match self {
            ApprovalAction::AllowOnce => ApprovalDecision::AllowOnce,
            ApprovalAction::Deny => ApprovalDecision::Deny,
        }
    }
}

/// 非交互审批被拒时写 stderr 的完整信息。
pub fn approval_details(request: &ApprovalRequest) -> String {
    format!(
        "[approval] 需要确认：\n  tool:    {}\n  command: {}\n  source:  {:?}\n  reason:  {}\n已拒绝（非交互模式；使用 --yes 可自动允许本次审批）。\n",
        request.tool,
        request.command.as_deref().unwrap_or("-"),
        request.source,
        request.reason
    )
}

/// `--yes` 自动允许时的人类提示。
pub fn approval_auto_line(request: &ApprovalRequest) -> String {
    format!(
        "[approval] --yes 自动允许一次：tool={} command={}\n",
        request.tool,
        request.command.as_deref().unwrap_or("-")
    )
}

/// 循环终态 → 退出码。审批未决同样按策略拒绝处理。
pub fn outcome_exit_code(outcome: &LoopOutcome) -> i32 {
    match outcome {
        LoopOutcome::Final { .. } => EXIT_OK,
        LoopOutcome::NeedsApproval { .. } | LoopOutcome::Denied { .. } => EXIT_POLICY_DENIED,
    }
}

/// `LoopError`（含轮次上限）统一退出 1。
pub fn error_exit_code() -> i32 {
    EXIT_ERROR
}

/// `LoopError` → 契约 `error` 事件，保证 `--json` 先输出事件再退出。
pub fn loop_error_event(error: &LoopError) -> CoreEvent {
    let body = match error {
        LoopError::Provider { code, message } => ErrorBody::new(*code, message.clone()),
        LoopError::UnknownTool(name) => {
            ErrorBody::new(ErrorCode::InvalidArgument, format!("未知工具：{name}"))
        }
        other => ErrorBody::new(ErrorCode::Internal, other.to_string()),
    };
    CoreEvent::Error { error: body }
}

/// `agent run --mode` → 策略模式。
pub fn mode_from_str(value: &str) -> Option<PolicyMode> {
    match value {
        "plan" => Some(PolicyMode::Plan),
        "chat" => Some(PolicyMode::Chat),
        "auto" => Some(PolicyMode::Auto),
        "yolo" => Some(PolicyMode::Yolo),
        _ => None,
    }
}

/// 构造会话。yolo 由命令行本身视为显式确认，直接 `confirm_yolo`。
pub fn build_session(id: impl Into<String>, mode: PolicyMode, max_turns: u32) -> Session {
    let mut session = Session::new(id, mode).with_max_turns(max_turns);
    if mode == PolicyMode::Yolo {
        session.confirm_yolo();
    }
    session
}

/// `--instructions` 覆盖默认指令（空字符串也视为显式覆盖）。
pub fn resolve_instructions(override_text: Option<&str>) -> &str {
    override_text.unwrap_or(DEFAULT_INSTRUCTIONS)
}

/// 工作区校验失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    Io { path: String, message: String },
    NotDirectory(String),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceError::Io { path, message } => {
                write!(f, "工作区 {path} 无法解析：{message}")
            }
            WorkspaceError::NotDirectory(path) => write!(f, "工作区 {path} 不是目录"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

/// canonicalize 后必须是已存在的目录。
pub fn resolve_workspace(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let canonical = path.canonicalize().map_err(|error| WorkspaceError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    if !canonical.is_dir() {
        return Err(WorkspaceError::NotDirectory(
            canonical.display().to_string(),
        ));
    }
    Ok(canonical)
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let preview: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lc_core::agent::{ModeChange, PolicySource, SessionError};

    fn approval() -> ApprovalRequest {
        ApprovalRequest {
            id: 7,
            tool: "shell".into(),
            command: Some("rm -rf build".into()),
            source: PolicySource::ExecuteReview,
            reason: "执行类工具需确认".into(),
        }
    }

    fn text_event(text: &str) -> CoreEvent {
        CoreEvent::AgentTextDelta {
            session_id: "s1".into(),
            index: 0,
            text: text.into(),
        }
    }

    #[test]
    fn json_event_lines_are_monotonic_envelopes() {
        let mut lines = JsonEventLines::new();
        let events = [
            text_event("hi"),
            CoreEvent::AgentDone {
                session_id: "s1".into(),
                stop_reason: "end_turn".into(),
            },
            CoreEvent::Error {
                error: ErrorBody::new(ErrorCode::PermissionDenied, "no"),
            },
        ];

        for (index, event) in events.iter().enumerate() {
            let line = lines.next_line(event);
            assert!(!line.contains('\n'), "一行一个 JSON：{line}");
            let envelope: EventEnvelope = serde_json::from_str(&line).unwrap();
            assert_eq!(envelope.seq, index as u64 + 1);
            assert_eq!(&envelope.event, event);
        }
        assert_eq!(lines.seq(), 3);
    }

    #[test]
    fn json_event_lines_keep_contract_type_names() {
        let mut lines = JsonEventLines::new();
        let done = lines.next_line(&CoreEvent::AgentDone {
            session_id: "s1".into(),
            stop_reason: "end_turn".into(),
        });
        let error = lines.next_line(&CoreEvent::Error {
            error: ErrorBody::new(ErrorCode::PermissionDenied, "denied"),
        });
        assert!(done.contains(r#""type":"agent:done""#), "{done}");
        assert!(error.contains(r#""type":"error""#), "{error}");
        assert!(error.contains(r#""code":"permission_denied""#), "{error}");
    }

    #[test]
    fn human_output_streams_text_and_gates_thinking() {
        assert_eq!(
            human_output(&text_event("hel"), false),
            HumanOutput::Stdout("hel".into())
        );

        let thinking = CoreEvent::AgentThinkingDelta {
            session_id: "s1".into(),
            text: "hmm".into(),
        };
        assert_eq!(human_output(&thinking, false), HumanOutput::Nothing);
        assert_eq!(
            human_output(&thinking, true),
            HumanOutput::Stderr("hmm".into())
        );

        let start = CoreEvent::AgentToolStart {
            session_id: "s1".into(),
            call_id: "c1".into(),
            name: "read_file".into(),
        };
        assert_eq!(
            human_output(&start, false),
            HumanOutput::Stderr("[tool] read_file\n".into())
        );
    }

    #[test]
    fn human_tool_result_truncates_to_500_chars() {
        let long = "x".repeat(600);
        let event = tool_result_event("s1", "c1", &Ok(ToolOutput { text: long }));
        let HumanOutput::Stderr(line) = human_output(&event, false) else {
            panic!("工具结果必须写 stderr");
        };
        assert!(line.starts_with("[tool ok] "), "{line}");
        assert_eq!(line.matches('x').count(), TOOL_PREVIEW_CHARS);
        assert!(line.ends_with("…\n"), "{line}");

        let error = tool_result_event("s1", "c1", &Err(ToolError::NotFound("missing.txt".into())));
        assert_eq!(
            human_output(&error, false),
            HumanOutput::Stderr("[tool error] 文件不存在：missing.txt\n".into())
        );
    }

    #[test]
    fn approval_without_yes_denies_and_reports_full_fields() {
        let request = approval();
        let action = approval_action(false);
        assert_eq!(action, ApprovalAction::Deny);
        assert_eq!(action.decision(), ApprovalDecision::Deny);

        let details = approval_details(&request);
        for expected in ["shell", "rm -rf build", "ExecuteReview", "执行类工具需确认"] {
            assert!(details.contains(expected), "{details}");
        }
        assert!(details.contains("--yes"), "{details}");
    }

    #[test]
    fn approval_with_yes_allows_once() {
        let action = approval_action(true);
        assert_eq!(action, ApprovalAction::AllowOnce);
        assert_eq!(action.decision(), ApprovalDecision::AllowOnce);
        assert!(approval_auto_line(&approval()).contains("shell"));
    }

    #[test]
    fn exit_codes_follow_contract() {
        assert_eq!(
            outcome_exit_code(&LoopOutcome::Final { text: "ok".into() }),
            EXIT_OK
        );
        assert_eq!(
            outcome_exit_code(&LoopOutcome::Denied {
                source: PolicySource::Mode,
                reason: "plan".into(),
            }),
            EXIT_POLICY_DENIED
        );
        assert_eq!(
            outcome_exit_code(&LoopOutcome::NeedsApproval {
                approval: approval(),
            }),
            EXIT_POLICY_DENIED
        );
        assert_eq!(error_exit_code(), EXIT_ERROR);
    }

    #[test]
    fn turn_limit_error_maps_to_error_event_and_exit_one() {
        let error = LoopError::Session(SessionError::TurnLimit { max: 25 });
        assert_eq!(error_exit_code(), EXIT_ERROR);
        let CoreEvent::Error { error: body } = loop_error_event(&error) else {
            panic!("LoopError 必须映射为 error 事件");
        };
        assert_eq!(body.code, ErrorCode::Internal);
        assert!(body.message.contains("25"), "{}", body.message);
    }

    #[test]
    fn unknown_tool_maps_to_invalid_argument() {
        let CoreEvent::Error { error } = loop_error_event(&LoopError::UnknownTool("nope".into()))
        else {
            panic!("必须映射为 error 事件");
        };
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("nope"));
    }

    #[test]
    fn workspace_must_be_an_existing_directory() {
        let dir = std::env::temp_dir().join(format!(
            "lc-cli-agent-workspace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let resolved = resolve_workspace(&dir).expect("目录应通过");
        assert_eq!(resolved, dir.canonicalize().unwrap());

        let file = dir.join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(
            resolve_workspace(&file),
            Err(WorkspaceError::NotDirectory(_))
        ));

        let missing = dir.join("missing");
        assert!(matches!(
            resolve_workspace(&missing),
            Err(WorkspaceError::Io { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn yolo_session_is_confirmed_and_max_turns_applied() {
        let mut yolo = build_session("s1", PolicyMode::Yolo, 7);
        assert_eq!(yolo.mode(), PolicyMode::Yolo);
        assert_eq!(yolo.max_turns(), 7);
        assert_eq!(yolo.request_mode(PolicyMode::Yolo), ModeChange::Applied);

        let mut auto = build_session("s2", PolicyMode::Auto, 3);
        assert_eq!(auto.mode(), PolicyMode::Auto);
        assert_eq!(
            auto.request_mode(PolicyMode::Yolo),
            ModeChange::NeedsYoloConfirmation
        );
    }

    #[test]
    fn mode_and_instructions_are_resolved() {
        for (text, mode) in [
            ("plan", PolicyMode::Plan),
            ("chat", PolicyMode::Chat),
            ("auto", PolicyMode::Auto),
            ("yolo", PolicyMode::Yolo),
        ] {
            assert_eq!(mode_from_str(text), Some(mode));
        }
        assert_eq!(mode_from_str("unknown"), None);

        assert_eq!(resolve_instructions(None), DEFAULT_INSTRUCTIONS);
        assert_eq!(resolve_instructions(Some("")), "");
        assert_eq!(resolve_instructions(Some("custom")), "custom");
    }
}
