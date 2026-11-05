//! 进程生命周期测试：**真实子进程**（docs/PLAN.md §5：R4/R5 这类生命周期
//! 问题只能靠真实运行发现，fake 验证不了 Job Object / 进程组语义）。
//!
//! 平台分支：windows 用 `cmd /c`，unix 用 `sh -c`。不依赖网络——
//! `ping` 只打 127.0.0.1 回环，仅用作可控时长的存活命令。

use std::io::Read;
use std::path::Path;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use lc_proto::ErrorCode;

use super::{ProcessError, ProcessSupervisor, RealProcessSupervisor, TrackedChild};

// ---- 测试辅助 ----

/// 短进程（~百毫秒）：给"spawn → 入 Job"的窗口留余量，避免秒退竞态翻测试。
fn short_lived_command() -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd".into(),
            vec![
                "/c".into(),
                "ping".into(),
                "-n".into(),
                "1".into(),
                "127.0.0.1".into(),
            ],
        )
    } else {
        ("sh".into(), vec!["-c".into(), "sleep 0.3".into()])
    }
}

/// 长驻进程：`seconds` 秒的存活命令，供 kill / drop 用例回收。
fn long_lived_command(seconds: u32) -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd".into(),
            vec![
                "/c".into(),
                "ping".into(),
                "-n".into(),
                seconds.to_string(),
                "127.0.0.1".into(),
            ],
        )
    } else {
        ("sh".into(), vec!["-c".into(), format!("sleep {seconds}")])
    }
}

fn str_args(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

fn supervisor() -> RealProcessSupervisor {
    RealProcessSupervisor::new().expect("create supervisor")
}

/// 轮询直到子进程退出；超时返回 None（供"必须已死"断言用）。
fn wait_exited(child: &TrackedChild, timeout: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => return None,
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

// ---- 用例 ----

/// 短进程正常退出被记录：wait 收集到 exit code 0，collect_exited 返回
/// (pid, status) 并从跟踪列表移除。
#[test]
fn short_process_exit_is_collected() {
    let mut sup = supervisor();
    let (program, args) = short_lived_command();
    let child = sup
        .spawn_tracked(Path::new(&program), &str_args(&args))
        .expect("spawn short process");

    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(0), "{status:?}");

    let exited = sup.collect_exited().expect("collect exited");
    assert_eq!(exited.len(), 1, "{exited:?}");
    assert_eq!(exited[0].0, child.id());
    assert_eq!(exited[0].1.code(), Some(0));
    assert!(sup.tracked_pids().is_empty(), "{:?}", sup.tracked_pids());
    // 再收一次：已收割的不再出现
    assert!(sup.collect_exited().expect("collect again").is_empty());
}

/// 程序不存在 → `NotFound` 语义错误（契约码映射，对齐 fs lane 的分类方式）。
#[test]
fn spawn_missing_program_is_not_found() {
    let mut sup = supervisor();
    let err = sup
        .spawn_tracked(Path::new("lc-core-definitely-missing-exe-7f3a"), &[])
        .expect_err("must fail");
    assert!(matches!(err, ProcessError::NotFound { .. }), "{err:?}");
    assert_eq!(err.code(), ErrorCode::NotFound);
    assert_eq!(err.to_error_body().code, ErrorCode::NotFound);
}

/// 错误码映射（纯函数，直接构造，同 fs/tests.rs 风格）。
#[test]
fn error_code_mapping() {
    assert_eq!(
        ProcessError::NotFound {
            program: "x".into()
        }
        .code(),
        ErrorCode::NotFound
    );
    assert_eq!(
        ProcessError::Spawn {
            program: "x".into(),
            message: "m".into()
        }
        .code(),
        ErrorCode::Io
    );
    assert_eq!(
        ProcessError::Group {
            pid: None,
            message: "m".into()
        }
        .code(),
        ErrorCode::Internal
    );
    assert_eq!(
        ProcessError::Group {
            pid: Some(1),
            message: "m".into()
        }
        .code(),
        ErrorCode::Internal
    );
    assert_eq!(
        ProcessError::Kill {
            pid: 1,
            message: "m".into()
        }
        .code(),
        ErrorCode::Io
    );
    assert_eq!(
        ProcessError::Wait {
            pid: 1,
            message: "m".into()
        }
        .code(),
        ErrorCode::Io
    );
}

/// kill_all 终止整组：两个长驻进程同时回收、以非零码退出；注册表清空后
/// supervisor 可继续 spawn（Windows：TerminateJobObject 后 Job 对象仍可用）。
#[test]
fn kill_all_terminates_all_and_supervisor_stays_usable() {
    let mut sup = supervisor();
    let (program, args) = long_lived_command(30);
    let a = sup
        .spawn_tracked(Path::new(&program), &str_args(&args))
        .expect("spawn a");
    let b = sup
        .spawn_tracked(Path::new(&program), &str_args(&args))
        .expect("spawn b");
    assert_eq!(sup.tracked_pids(), vec![a.id(), b.id()]);

    sup.kill_all().expect("kill_all");

    for (name, child) in [("a", &a), ("b", &b)] {
        let status = wait_exited(child, Duration::from_secs(10))
            .unwrap_or_else(|| panic!("{name} must be terminated by kill_all"));
        // TerminateJobObject / SIGKILL 都不会表现为正常退出
        assert!(!status.success(), "{name}: {status:?}");
    }
    assert!(sup.tracked_pids().is_empty(), "{:?}", sup.tracked_pids());
    assert!(sup.collect_exited().expect("collect").is_empty());

    // 回收后继续用：spawn → 正常退出
    let (p2, a2) = short_lived_command();
    let c = sup
        .spawn_tracked(Path::new(&p2), &str_args(&a2))
        .expect("spawn after kill_all");
    assert_eq!(c.wait().expect("wait").code(), Some(0));
}

/// drop supervisor 即整组回收（docs/PLAN.md R5）。
///
/// Windows 上这正是"Job Object 关闭即杀"的路径：supervisor drop 关闭 Job
/// 句柄，`KILL_ON_JOB_CLOSE` 终止全组——句柄本身不做任何显式 kill 调用。
/// 证据是**时序**：命令需要 ~29s 而轮询 10s 内就拿到退出状态，自然跑完
/// 不可能。注意退出码不断言：实测 KILL_ON_JOB_CLOSE 终止的进程退出码为
/// 0（Windows 10.0.26200），与 kill_all 里显式 `TerminateJobObject(1)` 的
/// 非零码不同；非零码路径由 kill_all 用例覆盖。
#[test]
fn supervisor_drop_terminates_children() {
    let (program, args) = long_lived_command(30);
    let child = {
        let mut sup = supervisor();
        sup.spawn_tracked(Path::new(&program), &str_args(&args))
            .expect("spawn long process")
    }; // supervisor 在此 drop

    // 命令需 ~29s 才能自然退出；10s 内观测到退出 ⇒ 是 drop 触发的整组回收
    let _status = wait_exited(&child, Duration::from_secs(10))
        .expect("process must be terminated when supervisor is dropped");
}

/// 单句柄 kill 生效且幂等：已退出的进程再次 kill 返回 Ok。
#[test]
fn tracked_child_kill_is_effective_and_idempotent() {
    let mut sup = supervisor();
    let (program, args) = long_lived_command(30);
    let child = sup
        .spawn_tracked(Path::new(&program), &str_args(&args))
        .expect("spawn long process");

    child.kill().expect("kill");
    let status = wait_exited(&child, Duration::from_secs(10)).expect("killed in time");
    assert!(!status.success(), "{status:?}");
    child.kill().expect("kill on already-exited must be Ok");
}

/// captured spawn：stdout/stderr 分离、exit code 与 cwd 均正确传递。
#[test]
fn captured_spawn_collects_streams_and_uses_cwd() {
    let mut sup = supervisor();
    let dir = std::env::temp_dir().join(format!(
        "lc-process-captured-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let (program, args) = if cfg!(windows) {
        (
            "cmd".to_string(),
            vec!["/c".into(), "echo out& echo err 1>&2& exit /b 3".into()],
        )
    } else {
        (
            "sh".to_string(),
            vec!["-c".into(), "printf out; printf err >&2; exit 3".into()],
        )
    };
    let mut child = sup
        .spawn_captured(Path::new(&program), &str_args(&args), Some(&dir))
        .expect("spawn captured");
    let mut stdout = child.take_stdout().expect("stdout pipe");
    let mut stderr = child.take_stderr().expect("stderr pipe");
    let status = child.wait().expect("wait");

    let mut out = String::new();
    stdout.read_to_string(&mut out).expect("read stdout");
    let mut err = String::new();
    stderr.read_to_string(&mut err).expect("read stderr");
    assert_eq!(status.code(), Some(3), "{status:?}");
    assert!(out.contains("out"), "{out}");
    assert!(err.contains("err"), "{err}");

    let (program, args) = if cfg!(windows) {
        ("cmd".to_string(), vec!["/c".into(), "cd".into()])
    } else {
        ("sh".to_string(), vec!["-c".into(), "pwd".into()])
    };
    let mut cwd_child = sup
        .spawn_captured(Path::new(&program), &str_args(&args), Some(&dir))
        .expect("spawn cwd probe");
    let mut cwd_stdout = cwd_child.take_stdout().expect("stdout pipe");
    assert!(cwd_child.take_stderr().is_some());
    assert_eq!(cwd_child.wait().expect("wait").code(), Some(0));
    let mut cwd = String::new();
    cwd_stdout.read_to_string(&mut cwd).expect("read cwd");
    let reported = Path::new(cwd.trim());
    assert!(reported.is_absolute(), "{cwd}");
    assert_eq!(reported.file_name(), dir.file_name(), "{cwd}");

    let _ = std::fs::remove_dir_all(&dir);
}
