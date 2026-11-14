//! lc-core::pty 单测：fake pty 驱动状态机（headless CI 双平台可跑），
//! 覆盖验收要求的启动/写入/读回/退出、缓冲上限与重放、重连一次语义、
//! close 幂等与退出回收。

use std::io;
use std::sync::Arc;
use std::time::Duration;

use std::sync::atomic::Ordering;

use super::{
    MAX_SCROLLBACK, PtyError, PtyEvent, PtyFactory, RingBuf, Session, SessionManager, Spec, State,
    tests_support::FakePty,
};

/// 测试工厂：按脚本依次产出 fake pty；第 N 次 spawn 记录在 spawn_index。
struct FactoryScript {
    ptys: std::sync::Mutex<Vec<FakePty>>,
    specs: std::sync::Mutex<Vec<(Spec, u16, u16)>>,
}

impl FactoryScript {
    fn new(count: usize) -> Self {
        Self {
            ptys: std::sync::Mutex::new((0..count).map(|_| FakePty::new()).collect()),
            specs: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn pty(&self, index: usize) -> FakePty {
        self.ptys.lock().unwrap()[index].clone()
    }

    fn into_factory(self: Arc<Self>) -> Box<PtyFactory> {
        Box::new(move |spec, cols, rows| {
            let mut specs = self.specs.lock().unwrap();
            let index = specs.len();
            specs.push((spec.clone(), cols, rows));
            drop(specs);
            let ptys = self.ptys.lock().unwrap();
            if index >= ptys.len() {
                return Err(io::Error::other("factory exhausted"));
            }
            Ok(ptys[index].clone().handles())
        })
    }
}

/// 事件记录器与回调（测试共用形状）。
type EventLog = Arc<std::sync::Mutex<Vec<PtyEvent>>>;
type EventSinkFn = Box<dyn Fn(PtyEvent) + Send + Sync>;

fn recording() -> (EventLog, EventSinkFn) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (
        events,
        Box::new(move |event| sink.lock().unwrap().push(event)),
    )
}

/// 等待条件成立（读线程是异步的）。
fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("condition not met within 5s: {what}");
}

fn open_local(
    script: &Arc<FactoryScript>,
    events: Box<dyn Fn(PtyEvent) + Send + Sync>,
) -> (SessionManager, Session) {
    let manager = SessionManager::new(script.clone().into_factory(), events);
    let session = manager.open(Spec::Local, 80, 24).expect("open session");
    (manager, session)
}

#[test]
fn local_session_writes_and_reads_back() {
    let script = Arc::new(FactoryScript::new(1));
    let (events, sink) = recording();
    let (_manager, session) = open_local(&script, sink);

    session.write(b"echo hi\r\n".as_slice());
    script
        .pty(0)
        .output
        .lock()
        .unwrap()
        .push(b"hi\r\n".to_vec());
    wait_for("first output", || !session.scrollback().is_empty());
    assert_eq!(session.scrollback(), b"hi\r\n");
    assert_eq!(
        script.pty(0).written.lock().unwrap().as_slice(),
        b"echo hi\r\n"
    );
    let events = events.lock().unwrap();
    assert!(events.iter().any(
        |e| matches!(e, PtyEvent::Data { data, .. } if data.as_slice() == b"hi\r\n".as_slice())
    ));
}

#[test]
fn open_failure_reports_error_without_registering() {
    let script = Arc::new(FactoryScript::new(0)); // factory exhausted → spawn 失败
    let (_events, sink) = recording();
    let manager = SessionManager::new(script.clone().into_factory(), sink);
    let message = match manager.open(Spec::Local, 80, 24) {
        Err(PtyError::Spawn(message)) => message,
        _ => panic!("expected spawn error"),
    };
    assert!(message.contains("exhausted"), "{message}");
    assert!(manager.list().is_empty());
}

#[test]
fn unexpected_exit_reconnects_once_with_replay_then_closes() {
    let script = Arc::new(FactoryScript::new(2));
    let (events, sink) = recording();
    let (_manager, session) = open_local(&script, sink);

    // 第一段输出进缓冲；随后进程自己退出（非用户关闭）
    script
        .pty(0)
        .output
        .lock()
        .unwrap()
        .push(b"before".to_vec());
    wait_for("before output", || session.scrollback() == b"before");
    *script.pty(0).exit.lock().unwrap() = Some(3);

    // 重连：第二次 spawn 用同参数，缓冲保留
    wait_for("second spawn", || script.specs.lock().unwrap().len() == 2);
    wait_for("reconnected", || session.state() == State::Connected);
    assert_eq!(session.scrollback(), b"before", "重连保留滚动缓冲");
    {
        let specs = script.specs.lock().unwrap();
        assert_eq!(specs[0], (Spec::Local, 80, 24));
        assert_eq!(specs[1], (Spec::Local, 80, 24));
    }

    // 重放：重连成功的会话把缓冲作为一条 Data 事件发出
    wait_for("replay data", || {
        events.lock().unwrap().iter().any(
            |e| matches!(e, PtyEvent::Data { data, .. } if data.as_slice() == b"before".as_slice()),
        )
    });

    // 再次退出（重连额度已用完）→ 终态
    *script.pty(1).exit.lock().unwrap() = Some(0);
    wait_for("closed", || session.state() == State::Closed);
    let events = events.lock().unwrap();
    let states: Vec<State> = events
        .iter()
        .filter_map(|e| match e {
            PtyEvent::State { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert!(
        states
            .windows(2)
            .any(|w| w[0] == State::Reconnecting && w[1] == State::Connected),
        "reconnecting → connected：{states:?}"
    );
    assert_eq!(states.last().copied(), Some(State::Closed));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PtyEvent::Exit { code, .. } if *code == 3))
    );
}

#[test]
fn reconnect_failure_falls_to_closed() {
    let script = Arc::new(FactoryScript::new(1)); // 只有 1 个 pty，重连 spawn 失败
    let (_events, sink) = recording();
    let (_manager, session) = open_local(&script, sink);

    *script.pty(0).exit.lock().unwrap() = Some(1);
    wait_for("closed", || session.state() == State::Closed);
    let states: Vec<State> = {
        // 事件回调被 manager 持有；直接看状态序列断言在上一条测试里做，
        // 这里只验证终态与不再追加 spawn。
        let _ = session.state();
        vec![]
    };
    let _ = states;
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(script.specs.lock().unwrap().len(), 2, "重连尝试恰好一次");
}

#[test]
fn user_close_does_not_reconnect_and_is_idempotent() {
    let script = Arc::new(FactoryScript::new(2));
    let (events, sink) = recording();
    let (manager, session) = open_local(&script, sink);

    manager.close(session.id()).expect("first close");
    manager
        .close(session.id())
        .expect("second close idempotent");
    assert!(script.pty(0).killed.load(Ordering::SeqCst));
    assert_eq!(session.state(), State::Closed);

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        script.specs.lock().unwrap().len(),
        1,
        "用户关闭不得触发重连"
    );
    let events = events.lock().unwrap();
    assert!(!events.iter().any(|e| matches!(
        e,
        PtyEvent::State {
            state: State::Reconnecting,
            ..
        }
    )));
}

#[test]
fn resize_is_forwarded_and_used_for_reconnect() {
    let script = Arc::new(FactoryScript::new(2));
    let (_events, sink) = recording();
    let (_manager, session) = open_local(&script, sink);

    session.resize(120, 40);
    wait_for("resize forwarded", || {
        *script.pty(0).sizes.lock().unwrap() == vec![(120, 40)]
    });

    *script.pty(0).exit.lock().unwrap() = Some(0);
    wait_for("second spawn", || script.specs.lock().unwrap().len() == 2);
    let specs = script.specs.lock().unwrap();
    assert_eq!(specs[1], (Spec::Local, 120, 40), "重连按最新尺寸重建");
}

#[test]
fn write_after_close_is_dropped_silently() {
    let script = Arc::new(FactoryScript::new(1));
    let (_events, sink) = recording();
    let (manager, session) = open_local(&script, sink);
    manager.close(session.id()).expect("close");
    session.write(b"ignored");
    assert!(script.pty(0).written.lock().unwrap().is_empty());
}

#[test]
fn kill_all_terminates_every_session_and_is_idempotent() {
    let script = Arc::new(FactoryScript::new(2));
    let (_events, sink) = recording();
    let manager = SessionManager::new(script.clone().into_factory(), sink);
    let first = manager.open(Spec::Local, 80, 24).expect("first");
    let second = manager
        .open(
            Spec::Ssh {
                host: "example.com".into(),
                port: 2222,
                user: Some("dev".into()),
            },
            100,
            30,
        )
        .expect("second");

    manager.kill_all();
    manager.kill_all(); // 幂等
    assert!(script.pty(0).killed.load(Ordering::SeqCst));
    assert!(script.pty(1).killed.load(Ordering::SeqCst));
    assert_eq!(first.state(), State::Closed);
    assert_eq!(second.state(), State::Closed);
}

#[test]
fn manager_drop_kills_children() {
    let script = Arc::new(FactoryScript::new(1));
    let (_events, sink) = recording();
    let manager = SessionManager::new(script.clone().into_factory(), sink);
    let _session = manager.open(Spec::Local, 80, 24).expect("open");
    drop(manager);
    wait_for("killed", || script.pty(0).killed.load(Ordering::SeqCst));
}

#[test]
fn ssh_specs_carry_kind_and_defaults() {
    let ssh = Spec::Ssh {
        host: "example.com".into(),
        port: 22,
        user: None,
    };
    assert_eq!(ssh.kind(), "ssh");
    assert_eq!(Spec::Local.kind(), "local");
    assert_eq!(State::Connected.as_str(), "connected");
    assert_eq!(State::Reconnecting.as_str(), "reconnecting");
    assert_eq!(State::Closed.as_str(), "closed");
    assert_eq!(
        ssh,
        Spec::Ssh {
            host: "example.com".into(),
            port: 22,
            user: None
        }
    );
    assert_eq!(
        super::ssh_command("example.com", 22, None),
        vec!["ssh".to_string(), "example.com".to_string()]
    );
}

#[test]
fn list_returns_all_sessions_with_metadata() {
    let script = Arc::new(FactoryScript::new(2));
    let (_events, sink) = recording();
    let manager = SessionManager::new(script.clone().into_factory(), sink);
    let local = manager.open(Spec::Local, 80, 24).expect("local");
    let ssh = manager
        .open(
            Spec::Ssh {
                host: "example.com".into(),
                port: 22,
                user: Some("dev".into()),
            },
            80,
            24,
        )
        .expect("ssh");

    let listed = manager.list();
    assert_eq!(listed.len(), 2);
    let ids: Vec<&str> = listed.iter().map(|s| s.id()).collect();
    assert!(ids.contains(&local.id()) && ids.contains(&ssh.id()));
    assert_eq!(
        listed.iter().map(|s| s.spec().kind()).collect::<Vec<_>>(),
        ["local", "ssh"]
    );
    assert!(listed.iter().all(|s| s.state() == State::Connected));
    assert_eq!(ssh.title(), "ssh://example.com");

    manager.close(local.id()).expect("close");
    // closed 会话仍在表里（重开窗口恢复标签 + replay）
    assert_eq!(manager.list().len(), 2);
}

#[test]
fn ring_buf_caps_at_max_scrollback() {
    let mut buf = RingBuf::new(MAX_SCROLLBACK);
    buf.push(&vec![b'a'; MAX_SCROLLBACK + 1024]);
    assert_eq!(buf.len(), MAX_SCROLLBACK);
    assert!(buf.snapshot().iter().all(|b| *b == b'a'));

    buf.push(b"tail");
    assert_eq!(buf.len(), MAX_SCROLLBACK);
    assert_eq!(&buf.snapshot()[buf.len() - 4..], b"tail");
}

#[test]
fn ssh_spec_default_port_and_user_normalize() {
    // 22 端口不出现在命令行；user 为空串视同未提供
    assert_eq!(
        super::ssh_command("h", 22, Some("")),
        vec!["ssh".to_string(), "h".to_string()]
    );
    assert_eq!(
        super::ssh_command("h", 2222, Some("dev")),
        vec![
            "ssh".to_string(),
            "-p".to_string(),
            "2222".to_string(),
            "-l".to_string(),
            "dev".to_string(),
            "h".to_string()
        ]
    );
    assert_eq!(
        super::ssh_command("h", 2222, None),
        vec![
            "ssh".to_string(),
            "-p".to_string(),
            "2222".to_string(),
            "h".to_string()
        ]
    );
}
