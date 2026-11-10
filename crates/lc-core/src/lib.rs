//! lc-core —— Lite Code 核心服务层。
//!
//! 分层见 docs/ARCHITECTURE.md：本 crate 是 L1，零 tauri 依赖，
//! `lc-cli` 与 Tauri 薄壳都是它的前端。

pub mod agent;
pub mod config;
pub mod db;
pub mod ext;
pub mod fs;
pub mod git;
pub mod llm;
pub mod lsp;
pub mod mcp;
pub mod process;
pub mod runtime;
pub mod task;
pub mod version;

mod untrusted;

/// 核心版本号（workspace 统一管理）。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
