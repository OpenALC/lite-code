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

#[test]
fn node_set_manual_missing_path_fails_with_json_report() {
    // set-manual 是显式动作：校验失败退出非 0（与 detect 的“永远 0”不同），
    // 但失败也输出 JSON 报告，reason 用契约标签
    let (code, stdout, stderr) = run_cli(&[
        "node",
        "set-manual",
        r"C:\definitely\missing\node.exe",
        "--json",
    ]);
    assert_ne!(code, 0, "校验失败必须非零退出；stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("失败也要输出 JSON 报告");
    assert_eq!(v.get("ok").and_then(|o| o.as_bool()), Some(false));
    assert_eq!(v.get("reason").and_then(|r| r.as_str()), Some("missing"));
}

#[test]
fn fs_read_dir_json_lists_entries() {
    let root = std::env::temp_dir().join(format!("lc-cli-fs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();

    let root_arg = root.to_str().unwrap();
    let (code, stdout, stderr) =
        run_cli(&["fs", "read-dir", root_arg, "--root", root_arg, "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("必须是契约 JSON");
    assert_eq!(v.get("kind").and_then(|k| k.as_str()), Some("dir_listing"));
    let entries = v
        .get("entries")
        .and_then(|e| e.as_array())
        .expect("entries");
    assert!(
        entries
            .iter()
            .any(|e| e.get("name").and_then(|n| n.as_str()) == Some("a.txt"))
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn node_info_json_outputs_node_field_and_exits_zero() {
    // 无配置 → {"node": null}，仍是合法输出且退出 0
    let (code, stdout, stderr) = run_cli(&["node", "info", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v.get("node").is_some(), "必须有 node 字段（可为 null）");
}
