//! golden 输出测试（PLAN.md Definition of Done：命令必须有 golden 输出）。
//! 输入是合成的探测结果，输出必须与基线逐字节一致。

use std::path::Path;

use lc_cli::{
    contract_meta_json, node_info_json, node_status_json, set_manual_report_json, shell_report_json,
};
use lc_core::config::NodeRuntime;
use lc_core::runtime::manual::ManualError;
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

// ---- node info / node set-manual（docs/RUNTIME.md §5 手动兜底）----

/// 合成的已解析 node 运行时（配置段，字段即契约形状）。
fn manual_node_runtime() -> NodeRuntime {
    NodeRuntime {
        path: r"C:\tools\node-v22.14.0\node.exe".into(),
        version: SemVer::new(22, 14, 0),
        source: NodeSource::Manual,
        resolved_at: "2026-11-05T08:09:10Z".into(),
    }
}

#[test]
fn node_info_none_matches_golden() {
    assert_eq!(node_info_json(None), read_golden("node-info-none.json"));
}

#[test]
fn node_info_manual_matches_golden() {
    let rt = manual_node_runtime();
    assert_eq!(
        node_info_json(Some(&rt)),
        read_golden("node-info-manual.json")
    );
}

#[test]
fn node_set_manual_ok_matches_golden() {
    let rt = manual_node_runtime();
    assert_eq!(
        set_manual_report_json(&Ok(rt)),
        read_golden("node-set-manual-ok.json")
    );
}

#[test]
fn node_set_manual_fail_matches_golden() {
    // 失败原因用契约枚举的 serde 标签，detail 保留原始输出
    let err = Err(ManualError::Invalid(
        NodeFailureReason::GhostInstall,
        Some("selfcheck failed".into()),
    ));
    assert_eq!(
        set_manual_report_json(&err),
        read_golden("node-set-manual-fail.json")
    );
}
