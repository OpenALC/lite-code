//! 数据库 DTO 与查询结果（docs/MILESTONES.md P5、docs/ARCHITECTURE.md §5.5）。
//!
//! 结果网格可能来自不可信数据库内容：`DbQueryResult.untrusted` 恒为 true，
//! 前端只能作为数据展示，不能把单元格内容当指令。大结果集只回样本 +
//! `handle`，完整结果由执行层保留（不在契约里传整表）。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum DbKind {
    Postgres,
    Mysql,
    Mssql,
    Sqlite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum DbConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DbConnectionInfo {
    pub connection_id: String,
    pub name: String,
    pub kind: DbKind,
    pub read_only: bool,
    pub state: DbConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// 远程端口；0 表示缺省/未设置（序列化省略），UI 回落 kind 默认端口。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub port: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// 凭证引用（`env:NAME` 等）；空串不携带。camelCase 与 TS 绑定一致。
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "secretRef")]
    pub secret_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum DbSchemaNodeKind {
    Database,
    Schema,
    Table,
    View,
    Column,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DbSchemaNode {
    pub name: String,
    pub kind: DbSchemaNodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub children: Vec<DbSchemaNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DbColumn {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
}

/// 查询结果样本。`rows` 只含前 N 行（默认 20），完整结果用 `handle` 分页取；
/// 每格用 `Option<String>`，NULL 与字符串 "NULL" 必须可区分。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DbQueryResult {
    pub columns: Vec<DbColumn>,
    pub rows: Vec<Vec<Option<String>>>,
    pub row_count: u64,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    pub elapsed_ms: u64,
    /// 数据库内容不可信，前端不得当指令执行（P5 出口 #4）。
    pub untrusted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DbExecResult {
    pub affected_rows: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DbExplainResult {
    pub plan: String,
    pub elapsed_ms: u64,
}

/// 连接配置（密码/密钥不在契约里，由 SecretStore 按 `secret_ref` 解析）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct DbConnectionConfig {
    pub connection_id: String,
    pub name: String,
    pub kind: DbKind,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    /// `keyring://...` / `env:NAME`；空串表示 SQLite 或无认证。
    pub secret_ref: String,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
}
