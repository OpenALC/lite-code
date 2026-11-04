//! golden 输出测试（PLAN.md Definition of Done：命令必须有 golden 输出）。
//! 输入是合成的探测结果，输出必须与基线逐字节一致。

use std::path::Path;

use lc_cli::{contract_meta_json, node_status_json, shell_report_json};
use lc_proto::{
    ContractMeta, NodeFailureReason, NodeHealth, NodeProbeAttempt, NodeRuntimeInfo, NodeSource,
    NodeStatus, SemVer, ShellInfo, ShellKind,
};

fn read_golden(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("golden")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("read golden {name}: {e}"))
}

#[test]
fn node_detect_ready_matches_golden() {
    let status = NodeStatus::Ready {
        runtime: NodeRuntimeInfo {
            path: r"C:\Program Files\nodejs\node.exe".into(),
            version: SemVer::new(24, 15, 0),
            source: NodeSource::OfficialInstaller,
            health: NodeHealth::Supported,
        },
    };
    assert_eq!(
        node_status_json(&status),
        read_golden("node-detect-ready.json")
    );
}

#[test]
fn node_detect_manual_matches_golden() {
    let status = NodeStatus::NeedManual {
        attempts: vec![NodeProbeAttempt {
            path: r"C:\Users\t\AppData\Local\Microsoft\WindowsApps\node.exe".into(),
            source: NodeSource::RegistryPath,
            reason: NodeFailureReason::StoreAlias,
            detail: None,
        }],
    };
    assert_eq!(
        node_status_json(&status),
        read_golden("node-detect-manual.json")
    );
}

#[test]
fn shell_found_matches_golden() {
    let shell = ShellInfo {
        path: r"C:\Program Files\PowerShell\7\pwsh.exe".into(),
        kind: ShellKind::Pwsh,
        version_major: 7,
    };
    assert_eq!(
        shell_report_json(Some(&shell)),
        read_golden("shell-found.json")
    );
}

#[test]
fn shell_none_matches_golden() {
    assert_eq!(shell_report_json(None), read_golden("shell-none.json"));
}

#[test]
fn contract_info_matches_golden() {
    // ContractMeta::current() 完全确定；契约变更必须显式更新 golden
    assert_eq!(
        contract_meta_json(&ContractMeta::current()),
        read_golden("contract-info.json")
    );
}
