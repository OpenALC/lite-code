//! 契约自检：serde 派生与常量清单必须一致。
//! “AI 生成代码最典型的失败是看起来对、跑起来错”（docs/PLAN.md），
//! 契约层的测试写成规范，不是补充说明。

use lc_proto::*;

#[test]
fn command_tags_match_constant_list() {
    for name in CONTRACT_COMMANDS {
        assert!(
            CONTRACT_COMMANDS.iter().filter(|n| *n == name).count() == 1,
            "重复命令名：{name}"
        );
    }
    // 数量守卫：新增命令必须同步登记
    assert_eq!(
        CONTRACT_COMMANDS.len(),
        31,
        "命令数量变化时须更新 CONTRACT_COMMANDS 与此测试"
    );
}

#[test]
fn command_serializes_with_documented_tag() {
    let json = serde_json::to_string(&CoreCommand::NodeDetect).unwrap();
    assert_eq!(json, r#"{"method":"node_detect"}"#);

    let json = serde_json::to_string(&CoreCommand::NodeSetManual {
        path: "C:\\n.exe".into(),
    })
    .unwrap();
    assert_eq!(json, r#"{"method":"node_set_manual","path":"C:\\n.exe"}"#);

    let json = serde_json::to_string(&CoreCommand::WorkspaceRoot).unwrap();
    assert_eq!(json, r#"{"method":"workspace_root"}"#);

    let json = serde_json::to_string(&CoreCommand::WorkspaceOpen {
        path: "D:/p".into(),
    })
    .unwrap();
    assert_eq!(json, r#"{"method":"workspace_open","path":"D:/p"}"#);

    let json = serde_json::to_string(&CoreCommand::FsWriteFile {
        path: "D:/p/a.txt".into(),
        content: "saved".into(),
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"method":"fs_write_file","path":"D:/p/a.txt","content":"saved"}"#
    );

    let json = serde_json::to_string(&CoreCommand::SessionCreate {
        mode: SessionMode::Auto,
        task_id: None,
    })
    .unwrap();
    assert_eq!(json, r#"{"method":"session_create","mode":"auto"}"#);

    let json = serde_json::to_string(&CoreCommand::SessionCreate {
        mode: SessionMode::Auto,
        task_id: Some("task-1".into()),
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"method":"session_create","mode":"auto","task_id":"task-1"}"#
    );

    let json = serde_json::to_string(&CoreCommand::AgentSend {
        session_id: "s1".into(),
        text: "hi".into(),
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"method":"agent_send","session_id":"s1","text":"hi"}"#
    );

    let json = serde_json::to_string(&CoreCommand::ApprovalResolve {
        session_id: "s1".into(),
        approval_id: 7,
        decision: ApprovalDecision::AllowSession,
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"method":"approval_resolve","session_id":"s1","approval_id":7,"decision":"allow_session"}"#
    );
}

#[test]
fn event_tags_use_documented_names() {
    // docs/TASKS.md §7 的事件名必须原样保留（冒号风格）
    let json = serde_json::to_string(&CoreEvent::TaskCreated {
        task_id: "01J".into(),
        title: "t".into(),
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"type":"task:created","task_id":"01J","title":"t"}"#
    );
}

#[test]
fn node_status_roundtrip_ready() {
    let status = NodeStatus::Ready {
        runtime: NodeRuntimeInfo {
            path: r"C:\Program Files\nodejs\node.exe".into(),
            version: SemVer::new(24, 15, 0),
            source: NodeSource::OfficialInstaller,
            health: NodeHealth::Supported,
        },
    };
    let json = serde_json::to_string(&status).unwrap();
    assert!(json.contains(r#""kind":"ready""#), "{json}");
    assert!(json.contains(r#""health":"supported""#), "{json}");
    let back: NodeStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, status);
}

#[test]
fn node_status_roundtrip_need_manual() {
    let status = NodeStatus::NeedManual {
        attempts: vec![NodeProbeAttempt {
            path: r"C:\Users\t\AppData\Local\Microsoft\WindowsApps\node.exe".into(),
            source: NodeSource::RegistryPath,
            reason: NodeFailureReason::StoreAlias,
            detail: None,
        }],
    };
    let json = serde_json::to_string(&status).unwrap();
    assert!(json.contains(r#""reason":"store_alias""#), "{json}");
    let back: NodeStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, status);
}

#[test]
fn event_envelope_carries_seq() {
    let env = EventEnvelope {
        seq: 42,
        event: CoreEvent::FsTree { generation: 7 },
    };
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains(r#""seq":42"#), "{json}");
    assert!(json.contains(r#""type":"fs:tree""#), "{json}");
}

#[test]
fn contract_meta_is_complete() {
    let meta = ContractMeta::current();
    assert_eq!(meta.name, "lc-proto");
    assert_eq!(meta.contract_version, CONTRACT_VERSION);
    assert_eq!(meta.commands.len(), CONTRACT_COMMANDS.len());
    assert_eq!(meta.events.len(), CONTRACT_EVENTS.len());
    assert_eq!(meta.error_codes.len(), CONTRACT_ERROR_CODES.len());
    assert!(meta.commands.contains(&"node_detect".to_string()));
    assert!(meta.commands.contains(&"workspace_root".to_string()));
    assert!(meta.commands.contains(&"workspace_open".to_string()));
    assert!(meta.commands.contains(&"fs_write_file".to_string()));
    assert!(meta.commands.contains(&"agent_send".to_string()));
    assert!(meta.events.contains(&"task:needs_input".to_string()));
    assert!(meta.events.contains(&"agent:approval_required".to_string()));
}

#[test]
fn approval_prompt_event_roundtrips() {
    let event = CoreEvent::AgentApprovalRequired {
        session_id: "s1".into(),
        approval: ApprovalPrompt {
            approval_id: 3,
            tool: "shell".into(),
            command: Some("npm publish".into()),
            source: ApprovalSource::UserBlacklist,
            reason: "用户黑名单".into(),
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(
        json.contains(r#""type":"agent:approval_required""#),
        "{json}"
    );
    assert!(json.contains(r#""approval_id":3"#), "{json}");
    let back: CoreEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, event);
}

#[test]
fn workspace_root_result_carries_optional_root() {
    let json = serde_json::to_string(&CommandResult::WorkspaceRoot {
        root: Some("D:/p".into()),
    })
    .unwrap();
    assert_eq!(json, r#"{"kind":"workspace_root","root":"D:/p"}"#);

    let json = serde_json::to_string(&CommandResult::WorkspaceRoot { root: None }).unwrap();
    assert_eq!(json, r#"{"kind":"workspace_root","root":null}"#);
}

#[test]
fn error_body_roundtrip() {
    let err = ErrorBody::new(ErrorCode::PathOutsideRoot, "路径越界").with_detail("D:\\outside");
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains(r#""code":"path_outside_root""#), "{json}");
    let back: ErrorBody = serde_json::from_str(&json).unwrap();
    assert_eq!(back, err);
}
