//! lc-cli 命令实现。JSON 输出直接序列化 lc-proto DTO，
//! 保证 CLI 与 Tauri 壳看到的是同一份契约。

use lc_proto::{ContractMeta, NodeStatus, ShellInfo};

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
