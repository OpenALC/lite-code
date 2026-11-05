//! lc-cli 命令实现。JSON 输出直接序列化 lc-proto DTO，
//! 保证 CLI 与 Tauri 壳看到的是同一份契约。

use lc_core::config::NodeRuntime;
use lc_core::runtime::manual::ManualError;
use lc_proto::{CommandResult, ContractMeta, NodeFailureReason, NodeStatus, ShellInfo};

/// `node detect --json`：输出契约的 [`NodeStatus`]。
/// 无 Node 也是合法输出（kind=need_manual），永远不报错。
pub fn node_status_json(status: &NodeStatus) -> String {
    pretty(status)
}

/// `shell detect --json`：`{"shell": {...} | null}`。
/// 不用 `json!` 宏——serde_json 的 Map 默认按 key 排序，会打乱契约字段顺序。
pub fn shell_report_json(shell: Option<&ShellInfo>) -> String {
    #[derive(serde::Serialize)]
    struct ShellReport<'a> {
        shell: Option<&'a ShellInfo>,
    }
    pretty(&ShellReport { shell })
}

/// `contract info --json`：契约元信息，内容完全确定，适合 golden。
pub fn contract_meta_json(meta: &ContractMeta) -> String {
    pretty(meta)
}

/// fs 命令的 JSON 输出：直接序列化契约 `CommandResult`，与 Tauri 形状一致。
pub fn command_result_json(result: &CommandResult) -> String {
    pretty(result)
}

/// `node info --json`：`{"node": {...} | null}`，即 config 里的 node_runtime 段
/// （docs/RUNTIME.md §5：手动指定 / 便携版写同一份配置）。
pub fn node_info_json(rt: Option<&NodeRuntime>) -> String {
    #[derive(serde::Serialize)]
    struct NodeInfo<'a> {
        node: Option<&'a NodeRuntime>,
    }
    pretty(&NodeInfo { node: rt })
}

/// `node info` 的人类可读输出。
pub fn node_info_human(rt: Option<&NodeRuntime>) -> String {
    match rt {
        Some(r) => format!(
            "node info: 已解析\n  path:    {}\n  version: {}\n  source:  {:?}\n  resolved_at: {}",
            r.path, r.version, r.source, r.resolved_at
        ),
        None => "node info: 未解析（无手动指定或便携版安装）\n  可用 `node set-manual <path>` 手动指定，或 `node portable download` 安装便携版。".into(),
    }
}

/// `node set-manual` 的 JSON 报告（确定性，适合 golden）：
/// 成功 `{"ok":true,"runtime":…}`；
/// 失败 `{"ok":false,"reason":"<契约 snake_case 标签>","detail":…}`。
/// `reason` 经 serde 取自契约枚举，与 NodeStatus JSON 的标签天然一致；
/// 配置写入失败不是契约失败原因，用 CLI 层标签 `persist_failed`。
pub fn set_manual_report_json(res: &Result<NodeRuntime, ManualError>) -> String {
    #[derive(serde::Serialize)]
    struct SetManualReport<'a> {
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        runtime: Option<&'a NodeRuntime>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<&'a str>,
    }
    let report = match res {
        Ok(rt) => SetManualReport {
            ok: true,
            runtime: Some(rt),
            reason: None,
            detail: None,
        },
        Err(ManualError::Invalid(reason, detail)) => SetManualReport {
            ok: false,
            runtime: None,
            reason: Some(failure_reason_tag(reason)),
            detail: detail.as_deref(),
        },
        Err(ManualError::Persist(e)) => SetManualReport {
            ok: false,
            runtime: None,
            reason: Some("persist_failed".into()),
            detail: Some(&e.to_string()),
        },
    };
    pretty(&report)
}

/// 契约枚举 → snake_case 标签（经 serde，保证与 JSON 输出同源）。
fn failure_reason_tag(reason: &NodeFailureReason) -> String {
    serde_json::to_value(reason)
        .expect("NodeFailureReason 序列化不可能失败")
        .as_str()
        .expect("枚举序列化为字符串")
        .to_string()
}

fn pretty<T: serde::Serialize>(value: &T) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("DTO 序列化不可能失败");
    s.push('\n');
    s
}

/// `node detect` 的人类可读输出。
pub fn node_status_human(status: &NodeStatus) -> String {
    match status {
        NodeStatus::Ready { runtime } => format!(
            "node detect: Ready\n  path:    {}\n  version: {}\n  source:  {:?}\n  health:  {:?}",
            runtime.path, runtime.version, runtime.source, runtime.health
        ),
        NodeStatus::NeedManual { attempts } => {
            let mut out = format!(
                "node detect: NeedManual（{} 个候选均不可用）\n",
                attempts.len()
            );
            for a in attempts {
                out.push_str(&format!("  - {} → {:?}\n", a.path, a.reason));
            }
            out.push_str("  Node 用于 MCP / DB 等可选功能，不装也能使用编辑器与 Git。");
            out
        }
    }
}
