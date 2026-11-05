//! P5 出口 #1：Agent 通过 db_* 工具完成「建表 → 写入 → 查询」闭环。
//! 使用真实 Node `node:sqlite` sidecar；Node 不可用时跳过。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lc_core::agent::{
    AgentLoop, LlmStream, LocalToolExecutor, LoopOutcome, PolicyMode, Session, ToolRegistry,
};
use lc_core::db::{DbExecutor, DbService};
use lc_core::llm::{LlmError, LlmEvent, LlmRequest, StopReason};
use lc_core::runtime::probe_real;
use lc_proto::{DbConnectionConfig, DbKind, NodeStatus};

struct ScriptedLlm {
    turns: Mutex<VecDeque<Vec<LlmEvent>>>,
}

impl ScriptedLlm {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns: Mutex::new(turns.into()),
        }
    }
}

impl LlmStream for ScriptedLlm {
    fn stream(
        &self,
        _request: &LlmRequest,
        sink: &mut dyn FnMut(LlmEvent),
    ) -> Result<(), LlmError> {
        for event in self
            .turns
            .lock()
            .expect("scripted llm")
            .pop_front()
            .unwrap_or_default()
        {
            sink(event);
        }
        Ok(())
    }
}

fn tool_turn(id: &str, name: &str, args: serde_json::Value) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUseStart {
            id: id.into(),
            name: name.into(),
        },
        LlmEvent::ToolUseArgsDelta {
            id: id.into(),
            json: args.to_string(),
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
        },
    ]
}

#[test]
fn agent_completes_sqlite_flow_with_db_tools() {
    let node = match probe_real() {
        NodeStatus::Ready { runtime } => PathBuf::from(runtime.path),
        _ => {
            eprintln!("skip: Node runtime unavailable");
            return;
        }
    };

    let dir = std::env::temp_dir().join(format!(
        "lc-db-agent-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let db_path = dir.join("demo.sqlite");
    let config_path = dir.join("db.json");

    let mut service = DbService::new(&config_path, Some(node));
    let connection_id = "c1".to_string();
    service
        .save_connection(DbConnectionConfig {
            connection_id: connection_id.clone(),
            name: "demo".into(),
            kind: DbKind::Sqlite,
            host: String::new(),
            port: 0,
            database: db_path.display().to_string(),
            user: String::new(),
            secret_ref: String::new(),
            read_only: false,
            extra: None,
        })
        .expect("save connection");
    service.connect(&connection_id).expect("connect sqlite");

    let service = Arc::new(Mutex::new(service));
    let executor = DbExecutor::new(Arc::clone(&service))
        .with_fallback(Arc::new(LocalToolExecutor::new(dir.clone())));
    let registry = ToolRegistry::with_builtins();
    let tools = registry.llm_tools();

    let llm = ScriptedLlm::new(vec![
        tool_turn(
            "e1",
            "db_execute",
            serde_json::json!({
                "connection_id": connection_id,
                "sql": "CREATE TABLE people (id INTEGER, name TEXT)"
            }),
        ),
        tool_turn(
            "e2",
            "db_execute",
            serde_json::json!({
                "connection_id": connection_id,
                "sql": "INSERT INTO people VALUES (1, 'alice')"
            }),
        ),
        tool_turn(
            "q1",
            "db_query",
            serde_json::json!({
                "connection_id": connection_id,
                "sql": "SELECT id, name FROM people ORDER BY id"
            }),
        ),
        vec![
            LlmEvent::TextDelta {
                index: 0,
                text: "done".into(),
            },
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
        ],
    ]);

    let mut agent = AgentLoop::new(
        Session::new("s1", PolicyMode::Yolo),
        Box::new(llm),
        Box::new(executor),
        registry,
        dir.clone(),
        "scripted",
    )
    .with_tools(tools);

    let outcome = agent
        .run("create, insert, query", &mut |_| {})
        .expect("run");
    assert_eq!(
        outcome,
        LoopOutcome::Final {
            text: "done".into()
        }
    );

    let outputs = agent
        .messages()
        .iter()
        .filter_map(|message| match message {
            lc_core::llm::LlmInput::FunctionCallOutput { output, .. } => Some(output.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        outputs.contains("alice"),
        "query output missing row: {outputs}"
    );
    assert!(
        outputs.contains("affected_rows") || outputs.contains("1"),
        "execute output missing affected rows: {outputs}"
    );

    service.lock().expect("service").shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parallel_writes_serialize_on_one_connection() {
    let node = match probe_real() {
        NodeStatus::Ready { runtime } => PathBuf::from(runtime.path),
        _ => {
            eprintln!("skip: Node runtime unavailable");
            return;
        }
    };
    let dir = std::env::temp_dir().join(format!(
        "lc-db-serial-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut service = DbService::new(dir.join("db.json"), Some(node));
    let id = "c1".to_string();
    service
        .save_connection(DbConnectionConfig {
            connection_id: id.clone(),
            name: "demo".into(),
            kind: DbKind::Sqlite,
            host: String::new(),
            port: 0,
            database: dir.join("demo.sqlite").display().to_string(),
            user: String::new(),
            secret_ref: String::new(),
            read_only: false,
            extra: None,
        })
        .expect("save");
    service.connect(&id).expect("connect");
    service
        .execute(&id, "CREATE TABLE t (name TEXT)")
        .expect("create");

    let service = Arc::new(Mutex::new(service));
    let handles: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|name| {
            let service = Arc::clone(&service);
            let id = id.clone();
            std::thread::spawn(move || {
                service
                    .lock()
                    .expect("service")
                    .execute(&id, &format!("INSERT INTO t VALUES ('{name}')"))
                    .expect("insert");
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("join");
    }

    let result = service
        .lock()
        .expect("service")
        .query(&id, "SELECT name FROM t ORDER BY name", Some(10))
        .expect("query");
    let rows: Vec<String> = result
        .rows
        .iter()
        .filter_map(|row| row.first().cloned().flatten())
        .collect();
    assert_eq!(rows, vec!["a".to_string(), "b".to_string()]);

    service.lock().expect("service").shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
