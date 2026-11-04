//! 端到端 CLI 冒烟：直接运行编译出的二进制。

use std::process::Command;

fn run_cli(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_lc-cli"))
        .args(args)
        .output()
        .expect("spawn lc-cli");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn version_flag_outputs_workspace_version() {
    let (code, stdout, stderr) = run_cli(&["--version"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        format!("lc-cli {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn contract_info_json_is_deterministic_and_matches_golden() {
    let (code, stdout, stderr) = run_cli(&["contract", "info", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let golden = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/contract-info.json"),
    )
    .unwrap();
    assert_eq!(stdout, golden);
}

#[test]
fn node_detect_json_outputs_contract_shape_and_exits_zero() {
    // 关键约束（nightly smoke 依赖）：探测失败也必须退出 0
    let (code, stdout, stderr) = run_cli(&["node", "detect", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("输出必须是合法 JSON");
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .expect("kind 字段必须存在");
    assert!(kind == "ready" || kind == "need_manual", "kind={kind}");
    if kind == "ready" {
        let rt = v.get("runtime").expect("ready 必须带 runtime");
        assert!(rt.get("path").is_some());
        assert!(rt.get("version").is_some());
        assert!(rt.get("source").is_some());
        assert!(rt.get("health").is_some());
    }
}

#[test]
fn shell_detect_json_outputs_shell_field() {
    let (code, stdout, stderr) = run_cli(&["shell", "detect", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v.get("shell").is_some(), "必须有 shell 字段（可为 null）");
}
