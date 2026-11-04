//! Tauri 薄壳（docs/ARCHITECTURE.md L2）。
//!
//! P0 只注册探测类命令做垂直切片：UI → 命令 → lc-core → lc-proto DTO。
//! CoreHandle（EventSink / PathProvider / SecretStore）在 P1 Lifecycle lane 落地，
//! 届时事件下发走 Tauri Channel，本文件只增加实现，不改变分层。

use lc_proto::{ContractMeta, NodeStatus, ShellInfo};

/// 转发 lc-core 的 Node 探测。惰性触发由前端控制，这里不做缓存。
#[tauri::command]
fn node_detect() -> NodeStatus {
    lc_core::runtime::probe_real()
}

/// 转发 shell 探测（终端与 Agent 执行依赖，docs/RUNTIME.md §8）。
#[tauri::command]
fn shell_detect() -> Option<ShellInfo> {
    lc_core::runtime::detect_shell_real()
}

/// 契约元信息（“关于”页与 lc-cli 共用同一来源）。
#[tauri::command]
fn contract_info() -> ContractMeta {
    lc_proto::ContractMeta::current()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            node_detect,
            shell_detect,
            contract_info
        ])
        .run(tauri::generate_context!())
        .expect("error while running lite-code");
}
