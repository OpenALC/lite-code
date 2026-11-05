//! 数据库出口闸门、连接服务与 Agent 工具（P5，docs/MILESTONES.md §8）。
//!
//! `sql.rs` 是**不依赖数据库驱动**的纯逻辑 SQL 分类与改写：
//! - 分类（Read / Write / Ddl / Unknown，未知一律 fail-closed）；
//! - 单语句检查（顶层 `;` 切分，字符串/注释/括号内的分号不算）；
//! - 写语句必须有顶层 `WHERE`；
//! - 只读连接拒绝 Write / Ddl / Unknown；
//! - 静态黑名单（在去掉字符串与注释后的 token 上匹配）；
//! - `SELECT` / `WITH` 读查询的 `LIMIT` 注入。
//!
//! `sidecar.rs` 是 Node `node:sqlite` 执行层（JSON-RPC 2.0 / NDJSON）：
//! 安装 `assets/db-bridge.mjs`、管理子进程与超时、把结果转换为契约 DTO。
//! 执行层在调用 sidecar 前仍会先过 [`SqlGuard`]，并强制
//! `DbQueryResult::untrusted = true`。
//!
//! `service.rs` 负责连接配置持久化（`data_dir()/db.json`）与连接生命周期；
//! `tools.rs` 把服务包装成 Agent 的 `db_*` 工具。库内数据（包括“忽略之前
//! 指令”一类文本）永远是不可信数据，分类器不会因为字符串内容改变判定。

mod service;
mod sidecar;
mod sql;
mod tools;

pub use service::{
    CONFIG_FILE_NAME, DEFAULT_QUERY_LIMIT, DbService, DbServiceError, MAX_QUERY_LIMIT,
};
pub use sidecar::{DbError, DbSidecar};
pub use sql::{GuardedSql, SqlError, SqlGuard, SqlKind};
pub use tools::{DbExecutor, db_tool_definitions, db_tool_specs};
