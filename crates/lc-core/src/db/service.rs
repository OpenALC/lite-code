//! 数据库连接管理与服务层（P5）。
//!
//! [`DbService`] 把 [`DbSidecar`] 与连接配置组合在一起：
//! - 配置持久化到 `data_dir()/db.json`，写入复用
//!   [`crate::config::write_atomic`]（临时文件 + rename，并保留 `.bak`）；
//! - 配置结构里没有密码字段，`secret_ref` 只是密钥引用，落盘内容不包含密码；
//! - 用户 SQL 一律先过 [`SqlGuard`]，再把 `guarded.sql` 交给 sidecar；
//!   只读连接由 sidecar 的 SQLite `readOnly` 句柄再拒绝一次；
//! - Node 不可用时 list/save/delete 仍然可用，connect/query 返回
//!   [`DbServiceError::DbUnavailable`]。
//!
//! 连接状态以 sidecar 内实际建立的连接为准：`active` 集合在 connect 成功时
//! 加入、disconnect/失败/shutdown 时移除；sidecar 超时或退出后会整体丢弃，
//! 已连接项统一标记为 `Error`，下次 connect 重新启动子进程。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use lc_proto::{
    DbConnectionConfig, DbConnectionInfo, DbConnectionState, DbExecResult, DbExplainResult, DbKind,
    DbQueryResult, DbSchemaNode, ErrorBody, ErrorCode,
};
use serde::{Deserialize, Serialize};

use super::{DbError, DbSidecar, SqlError, SqlGuard, SqlKind};

/// 默认配置文件（`data_dir()/db.json`）。
pub const CONFIG_FILE_NAME: &str = "db.json";
/// 配置格式版本；未来不兼容变更递增。
const CONFIG_VERSION: u32 = 1;
/// `db_query` 未指定 `limit` 时的默认行数。
pub const DEFAULT_QUERY_LIMIT: u32 = 100;
/// `db_query` 允许的最大行数（与 [`SqlGuard`] 的上限一致）。
pub const MAX_QUERY_LIMIT: u32 = 1000;

/// 服务层错误。`code()` 映射到契约错误码，`to_error_body()` 供命令层包装。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DbServiceError {
    /// 连接 id 不在配置中。
    #[error("数据库连接不存在：{0}")]
    NotFound(String),
    /// 配置/请求参数不合法。
    #[error("数据库参数不合法：{0}")]
    InvalidArgument(String),
    /// Node/sidecar 不可用，或连接尚未建立。
    #[error("数据库不可用：{0}")]
    DbUnavailable(String),
    /// SQL 闸门拒绝，或 SQLite 拒绝执行。
    #[error("数据库拒绝执行：{0}")]
    DbRejected(String),
    /// 只读连接上的写操作。
    #[error("只读数据库连接拒绝写操作：{0}")]
    DbReadOnly(String),
    /// sidecar 响应/连接超时。
    #[error("数据库操作超时：{0}")]
    Timeout(String),
    /// 本地配置或 sidecar I/O 失败。
    #[error("数据库 I/O 失败：{0}")]
    Io(String),
}

impl DbServiceError {
    pub fn code(&self) -> ErrorCode {
        match self {
            DbServiceError::NotFound(_) => ErrorCode::NotFound,
            DbServiceError::InvalidArgument(_) => ErrorCode::InvalidArgument,
            DbServiceError::DbUnavailable(_) => ErrorCode::DbUnavailable,
            DbServiceError::DbRejected(_) => ErrorCode::DbRejected,
            DbServiceError::DbReadOnly(_) => ErrorCode::DbReadOnly,
            DbServiceError::Timeout(_) => ErrorCode::Timeout,
            DbServiceError::Io(_) => ErrorCode::Io,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        ErrorBody::new(self.code(), self.to_string())
    }
}

impl From<DbError> for DbServiceError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Unavailable(message) | DbError::Closed(message) => {
                DbServiceError::DbUnavailable(message)
            }
            DbError::Rejected(message) => DbServiceError::DbRejected(message),
            DbError::ReadOnly(message) => DbServiceError::DbReadOnly(message),
            DbError::InvalidArgument(message) => DbServiceError::InvalidArgument(message),
            DbError::Timeout { method, timeout_ms } => {
                DbServiceError::Timeout(format!("{method} 超时（{timeout_ms} ms）"))
            }
            DbError::Io(message) => DbServiceError::Io(message),
            DbError::Protocol(message) => {
                DbServiceError::Io(format!("数据库 sidecar 协议错误：{message}"))
            }
        }
    }
}

impl From<SqlError> for DbServiceError {
    fn from(error: SqlError) -> Self {
        match error {
            SqlError::ReadOnly { .. } => DbServiceError::DbReadOnly(error.to_string()),
            SqlError::MissingWhere { .. }
            | SqlError::Blacklisted { .. }
            | SqlError::UnknownStatement { .. } => DbServiceError::DbRejected(error.to_string()),
            SqlError::EmptyStatement
            | SqlError::MultipleStatements { .. }
            | SqlError::Unterminated { .. }
            | SqlError::UnsupportedSyntax { .. } => {
                DbServiceError::InvalidArgument(error.to_string())
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DbFile {
    #[serde(default = "config_version")]
    version: u32,
    #[serde(default)]
    connections: Vec<DbConnectionConfig>,
}

fn config_version() -> u32 {
    CONFIG_VERSION
}

impl DbFile {
    fn from_configs(configs: &HashMap<String, DbConnectionConfig>) -> Self {
        let mut connections: Vec<DbConnectionConfig> = configs.values().cloned().collect();
        connections.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.connection_id.cmp(&right.connection_id))
        });
        Self {
            version: CONFIG_VERSION,
            connections,
        }
    }
}

/// 数据库连接服务。
///
/// 一个 service 只启动一个 sidecar（内部可持有多个 SQLite 连接）。构造时
/// 注入 `db.json` 路径与 Node 路径，便于测试隔离；生产入口是
/// [`DbService::load_default`]。
pub struct DbService {
    config_path: PathBuf,
    node_path: Option<PathBuf>,
    sidecar: Option<DbSidecar>,
    configs: HashMap<String, DbConnectionConfig>,
    infos: HashMap<String, DbConnectionInfo>,
    active: HashSet<String>,
    load_warning: Option<DbServiceError>,
}

impl DbService {
    /// 用显式路径构造（测试/嵌入方使用）。配置文件不存在视为空配置；
    /// 解析/读取失败不会 panic，而是记录在 [`DbService::load_warning`]。
    pub fn new(config_path: impl AsRef<Path>, node_path: Option<PathBuf>) -> Self {
        let mut service = Self {
            config_path: config_path.as_ref().to_path_buf(),
            node_path,
            sidecar: None,
            configs: HashMap::new(),
            infos: HashMap::new(),
            active: HashSet::new(),
            load_warning: None,
        };
        service.load();
        service
    }

    /// 生产入口：`data_dir()/db.json`，Node 路径由 [`crate::runtime::probe_real`]
    /// 决定。Node 不可用时仍返回可用服务（connect 时再报 `DbUnavailable`）。
    pub fn load_default() -> Result<Self, DbServiceError> {
        let config_path = crate::config::data_dir()
            .map_err(|error| DbServiceError::Io(format!("无法定位数据目录：{error}")))?
            .join(CONFIG_FILE_NAME);
        let node_path = match crate::runtime::probe_real() {
            lc_proto::NodeStatus::Ready { runtime } => Some(PathBuf::from(runtime.path)),
            lc_proto::NodeStatus::NeedManual { .. } => None,
        };
        Ok(Self::new(config_path, node_path))
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn node_path(&self) -> Option<&Path> {
        self.node_path.as_deref()
    }

    /// 配置加载期的非致命错误（损坏的 `db.json` 等）。
    pub fn load_warning(&self) -> Option<&DbServiceError> {
        self.load_warning.as_ref()
    }

    /// 连接列表，按 `name`、`connection_id` 排序。
    pub fn list(&self) -> Vec<DbConnectionInfo> {
        let mut infos: Vec<DbConnectionInfo> = self.infos.values().cloned().collect();
        infos.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.connection_id.cmp(&right.connection_id))
        });
        infos
    }

    /// 新增或更新连接配置（按 `connection_id` upsert）。
    ///
    /// SQLite 允许 `host`/`port` 为空；其它驱动要求 `host` 非空且 `port != 0`。
    /// 已连接且配置发生变化时先断开旧连接，避免继续使用过期参数。
    pub fn save_connection(
        &mut self,
        config: DbConnectionConfig,
    ) -> Result<DbConnectionInfo, DbServiceError> {
        validate_config(&config)?;
        let connection_id = config.connection_id.clone();
        let previous = self.configs.get(&connection_id).cloned();
        let changed = previous.as_ref().is_some_and(|old| old != &config);

        let mut configs = self.configs.clone();
        configs.insert(connection_id.clone(), config.clone());
        self.persist_configs(&configs)?;
        self.configs = configs;

        if changed
            && self.active.remove(&connection_id)
            && let Some(sidecar) = self.sidecar.as_mut()
        {
            let _ = sidecar.disconnect(&connection_id);
        }

        let keep_connected = !changed && self.active.contains(&connection_id);
        let info = if keep_connected {
            if let Some(info) = self.infos.get_mut(&connection_id) {
                info.name = config.name.clone();
                info.kind = config.kind;
                info.read_only = config.read_only;
                info.host = host_option(&config);
                info.database = database_option(&config);
                info.clone()
            } else {
                let info = info_from_config(&config);
                self.infos.insert(connection_id, info.clone());
                info
            }
        } else {
            let info = info_from_config(&config);
            self.infos.insert(connection_id, info.clone());
            info
        };
        Ok(info)
    }

    /// 删除连接配置；已连接时先断开 sidecar 内的句柄。
    pub fn delete_connection(&mut self, connection_id: &str) -> Result<(), DbServiceError> {
        if !self.configs.contains_key(connection_id) {
            return Err(DbServiceError::NotFound(connection_id.to_string()));
        }

        let mut configs = self.configs.clone();
        configs.remove(connection_id);
        self.persist_configs(&configs)?;
        self.configs = configs;
        self.infos.remove(connection_id);

        if self.active.remove(connection_id)
            && let Some(sidecar) = self.sidecar.as_mut()
        {
            let _ = sidecar.disconnect(connection_id);
        }
        Ok(())
    }

    /// 建立连接（幂等）。Node 不可用、驱动未打包或 SQLite 打开失败时返回
    /// 错误，同时把连接状态标记为 `Error`（可通过 [`DbService::list`] 查看）。
    pub fn connect(&mut self, connection_id: &str) -> Result<DbConnectionInfo, DbServiceError> {
        let config = self
            .configs
            .get(connection_id)
            .cloned()
            .ok_or_else(|| DbServiceError::NotFound(connection_id.to_string()))?;

        if self.active.contains(connection_id) {
            return self
                .infos
                .get(connection_id)
                .cloned()
                .ok_or_else(|| DbServiceError::NotFound(connection_id.to_string()));
        }

        self.ensure_sidecar(connection_id)?;
        if let Some(info) = self.infos.get_mut(connection_id) {
            info.state = DbConnectionState::Connecting;
            info.error = None;
        }

        let Some(sidecar) = self.sidecar.as_mut() else {
            let error = DbServiceError::DbUnavailable("数据库 sidecar 未启动".to_string());
            self.mark_connection_error(connection_id, &error.to_string());
            return Err(error);
        };
        let result = sidecar.connect(&config);
        match result {
            Ok(()) => {
                self.active.insert(connection_id.to_string());
                let info = self
                    .infos
                    .get_mut(connection_id)
                    .ok_or_else(|| DbServiceError::NotFound(connection_id.to_string()))?;
                info.state = DbConnectionState::Connected;
                info.error = None;
                Ok(info.clone())
            }
            Err(error) => {
                let fatal = is_fatal_sidecar_error(&error);
                let mapped = DbServiceError::from(error);
                self.mark_connection_error(connection_id, &mapped.to_string());
                if fatal {
                    self.reset_sidecar(&mapped.to_string());
                }
                Err(mapped)
            }
        }
    }

    /// 断开连接（幂等）。sidecar 内的句柄已不存在时也会把本地状态清为
    /// `Disconnected`，不把清理失败暴露给调用方。
    pub fn disconnect(&mut self, connection_id: &str) -> Result<DbConnectionInfo, DbServiceError> {
        if !self.configs.contains_key(connection_id) && !self.infos.contains_key(connection_id) {
            return Err(DbServiceError::NotFound(connection_id.to_string()));
        }

        if self.active.remove(connection_id)
            && let Some(sidecar) = self.sidecar.as_mut()
        {
            let _ = sidecar.disconnect(connection_id);
        }
        let info = self
            .infos
            .get_mut(connection_id)
            .ok_or_else(|| DbServiceError::NotFound(connection_id.to_string()))?;
        info.state = DbConnectionState::Disconnected;
        info.error = None;
        Ok(info.clone())
    }

    /// 执行只读查询。`limit` 夹到 `1..=1000`，未指定时使用默认 100。
    pub fn query(
        &mut self,
        connection_id: &str,
        sql: &str,
        limit: Option<u32>,
    ) -> Result<DbQueryResult, DbServiceError> {
        let read_only = self.connection(connection_id)?.read_only;
        let limit = limit.map(|value| value.clamp(1, MAX_QUERY_LIMIT));
        let default_limit = limit.unwrap_or(DEFAULT_QUERY_LIMIT) as usize;
        let guarded = SqlGuard::with_default_limit(default_limit).check(sql, read_only)?;
        if guarded.kind != SqlKind::Read {
            return Err(DbServiceError::DbRejected(format!(
                "db_query 只接受只读语句（{:?}）",
                guarded.kind
            )));
        }

        let Some(sidecar) = self.sidecar.as_mut() else {
            let error = DbServiceError::DbUnavailable("数据库 sidecar 未启动".to_string());
            self.mark_connection_error(connection_id, &error.to_string());
            return Err(error);
        };
        let result = sidecar.query(connection_id, &guarded.sql, limit);
        self.finish(connection_id, result)
    }

    /// 执行写/DDL。仍先过 [`SqlGuard`]（无 WHERE 的 UPDATE/DELETE 会被拒绝）。
    pub fn execute(
        &mut self,
        connection_id: &str,
        sql: &str,
    ) -> Result<DbExecResult, DbServiceError> {
        let read_only = self.connection(connection_id)?.read_only;
        let guarded = SqlGuard::default().check(sql, read_only)?;

        let Some(sidecar) = self.sidecar.as_mut() else {
            let error = DbServiceError::DbUnavailable("数据库 sidecar 未启动".to_string());
            self.mark_connection_error(connection_id, &error.to_string());
            return Err(error);
        };
        let result = sidecar.execute(connection_id, &guarded.sql);
        self.finish(connection_id, result)
    }

    /// 读取 schema 树（database → table/view → column）。
    pub fn schema(&mut self, connection_id: &str) -> Result<Vec<DbSchemaNode>, DbServiceError> {
        self.connection(connection_id)?;
        let Some(sidecar) = self.sidecar.as_mut() else {
            let error = DbServiceError::DbUnavailable("数据库 sidecar 未启动".to_string());
            self.mark_connection_error(connection_id, &error.to_string());
            return Err(error);
        };
        let result = sidecar.schema(connection_id);
        self.finish(connection_id, result)
    }

    /// 查看执行计划；用户 SQL 先过闸门，sidecar 只执行
    /// `EXPLAIN QUERY PLAN <guarded.sql>`。
    pub fn explain(
        &mut self,
        connection_id: &str,
        sql: &str,
    ) -> Result<DbExplainResult, DbServiceError> {
        let read_only = self.connection(connection_id)?.read_only;
        let guarded = SqlGuard::default().check(sql, read_only)?;

        let Some(sidecar) = self.sidecar.as_mut() else {
            let error = DbServiceError::DbUnavailable("数据库 sidecar 未启动".to_string());
            self.mark_connection_error(connection_id, &error.to_string());
            return Err(error);
        };
        let result = sidecar.explain(connection_id, &guarded.sql);
        self.finish(connection_id, result)
    }

    /// 终止 sidecar 并把所有连接标记为 `Disconnected`。幂等。
    pub fn shutdown(&mut self) {
        if let Some(mut sidecar) = self.sidecar.take() {
            sidecar.shutdown();
        }
        self.active.clear();
        for info in self.infos.values_mut() {
            info.state = DbConnectionState::Disconnected;
            info.error = None;
        }
    }

    fn load(&mut self) {
        let text = match std::fs::read_to_string(&self.config_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.load_warning = Some(DbServiceError::Io(format!(
                    "读取 {} 失败：{error}",
                    self.config_path.display()
                )));
                return;
            }
        };
        if text.trim().is_empty() {
            return;
        }

        match serde_json::from_str::<DbFile>(&text) {
            Ok(file) if file.version <= CONFIG_VERSION => {
                for config in file.connections {
                    let info = info_from_config(&config);
                    self.infos.insert(config.connection_id.clone(), info);
                    self.configs.insert(config.connection_id.clone(), config);
                }
            }
            Ok(file) => {
                self.load_warning = Some(DbServiceError::InvalidArgument(format!(
                    "{}: 不支持的配置版本 {}（期望 <= {CONFIG_VERSION}）",
                    self.config_path.display(),
                    file.version
                )));
            }
            Err(error) => {
                self.load_warning = Some(DbServiceError::InvalidArgument(format!(
                    "{} 解析失败：{error}",
                    self.config_path.display()
                )));
            }
        }
    }

    fn persist_configs(
        &self,
        configs: &HashMap<String, DbConnectionConfig>,
    ) -> Result<(), DbServiceError> {
        let file = DbFile::from_configs(configs);
        let mut text = serde_json::to_string_pretty(&file).map_err(|error| {
            DbServiceError::Io(format!(
                "序列化 {} 失败：{error}",
                self.config_path.display()
            ))
        })?;
        text.push('\n');
        crate::config::write_atomic(&self.config_path, &text).map_err(|error| {
            DbServiceError::Io(format!("写入 {} 失败：{error}", self.config_path.display()))
        })
    }

    fn connection(&self, connection_id: &str) -> Result<&DbConnectionConfig, DbServiceError> {
        let config = self
            .configs
            .get(connection_id)
            .ok_or_else(|| DbServiceError::NotFound(connection_id.to_string()))?;
        if !self.active.contains(connection_id) {
            return Err(DbServiceError::DbUnavailable(format!(
                "数据库连接未建立：{connection_id}"
            )));
        }
        Ok(config)
    }

    fn ensure_sidecar(&mut self, connection_id: &str) -> Result<(), DbServiceError> {
        if self.sidecar.is_some() {
            return Ok(());
        }
        let Some(node_path) = self.node_path.clone() else {
            let error = DbServiceError::DbUnavailable(
                "Node 运行时不可用（未探测到可用的 node）".to_string(),
            );
            self.mark_connection_error(connection_id, &error.to_string());
            return Err(error);
        };

        let spawn = DbSidecar::spawn(&node_path).and_then(|mut sidecar| {
            sidecar.initialize()?;
            Ok(sidecar)
        });
        match spawn {
            Ok(sidecar) => {
                self.sidecar = Some(sidecar);
                Ok(())
            }
            Err(error) => {
                let mapped = DbServiceError::from(error);
                self.mark_connection_error(connection_id, &mapped.to_string());
                Err(mapped)
            }
        }
    }

    fn mark_connection_error(&mut self, connection_id: &str, message: &str) {
        self.active.remove(connection_id);
        if let Some(info) = self.infos.get_mut(connection_id) {
            info.state = DbConnectionState::Error;
            info.error = Some(message.to_string());
        }
    }

    fn reset_sidecar(&mut self, message: &str) {
        if let Some(mut sidecar) = self.sidecar.take() {
            sidecar.shutdown();
        }
        let active: Vec<String> = self.active.drain().collect();
        for connection_id in active {
            if let Some(info) = self.infos.get_mut(&connection_id) {
                info.state = DbConnectionState::Error;
                info.error = Some(message.to_string());
            }
        }
    }

    /// sidecar 错误分类：SQL 拒绝/只读/参数错误不断开连接；进程退出、超时、
    /// 协议错误或本地连接丢失会把 sidecar 整体丢弃，等待下次 connect 重建。
    fn finish<T>(
        &mut self,
        connection_id: &str,
        result: Result<T, DbError>,
    ) -> Result<T, DbServiceError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let fatal = is_fatal_sidecar_error(&error);
                let lost = matches!(error, DbError::Unavailable(_));
                let mapped = DbServiceError::from(error);
                if fatal {
                    self.reset_sidecar(&mapped.to_string());
                } else if lost {
                    self.mark_connection_error(connection_id, &mapped.to_string());
                }
                Err(mapped)
            }
        }
    }
}

fn is_fatal_sidecar_error(error: &DbError) -> bool {
    matches!(
        error,
        DbError::Closed(_) | DbError::Timeout { .. } | DbError::Protocol(_)
    )
}

fn validate_config(config: &DbConnectionConfig) -> Result<(), DbServiceError> {
    if config.connection_id.trim().is_empty() {
        return Err(DbServiceError::InvalidArgument(
            "connection_id 不能为空".to_string(),
        ));
    }
    if config.database.trim().is_empty() {
        return Err(DbServiceError::InvalidArgument(
            "database 不能为空".to_string(),
        ));
    }
    if config.kind != DbKind::Sqlite {
        if config.host.trim().is_empty() {
            return Err(DbServiceError::InvalidArgument(format!(
                "{} 连接需要 host",
                kind_label(config.kind)
            )));
        }
        if config.port == 0 {
            return Err(DbServiceError::InvalidArgument(format!(
                "{} 连接需要有效 port",
                kind_label(config.kind)
            )));
        }
    }
    Ok(())
}

fn kind_label(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Postgres => "postgres",
        DbKind::Mysql => "mysql",
        DbKind::Mssql => "mssql",
        DbKind::Sqlite => "sqlite",
    }
}

fn host_option(config: &DbConnectionConfig) -> Option<String> {
    (!config.host.is_empty()).then(|| config.host.clone())
}

fn database_option(config: &DbConnectionConfig) -> Option<String> {
    (!config.database.is_empty()).then(|| config.database.clone())
}

fn info_from_config(config: &DbConnectionConfig) -> DbConnectionInfo {
    DbConnectionInfo {
        connection_id: config.connection_id.clone(),
        name: config.name.clone(),
        kind: config.kind,
        read_only: config.read_only,
        state: DbConnectionState::Disconnected,
        host: host_option(config),
        database: database_option(config),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_node() -> Option<PathBuf> {
        let node = std::env::var_os("LITE_CODE_TEST_NODE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("node"));
        match Command::new(&node)
            .arg("-e")
            .arg("require('node:sqlite')")
            .output()
        {
            Ok(output) if output.status.success() => Some(node),
            _ => {
                eprintln!("跳过数据库服务测试：找不到可用的 node（node:sqlite）");
                None
            }
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "lite-code-db-service-{}-{tag}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("创建临时目录");
            Self { path }
        }

        fn config_path(&self) -> PathBuf {
            self.path.join(CONFIG_FILE_NAME)
        }

        fn db_path(&self) -> PathBuf {
            self.path.join("test.sqlite")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn sqlite_config(id: &str, name: &str, database: &Path, read_only: bool) -> DbConnectionConfig {
        DbConnectionConfig {
            connection_id: id.to_string(),
            name: name.to_string(),
            kind: DbKind::Sqlite,
            host: String::new(),
            port: 0,
            database: database.to_string_lossy().into_owned(),
            user: String::new(),
            secret_ref: String::new(),
            read_only,
            extra: None,
        }
    }

    fn saved(
        service: &mut DbService,
        id: &str,
        name: &str,
        database: &Path,
        read_only: bool,
    ) -> DbConnectionInfo {
        service
            .save_connection(sqlite_config(id, name, database, read_only))
            .expect("保存连接")
    }

    /// 关闭连接时 sidecar 仍持有 SQLite 文件；显式 shutdown 后再删临时目录。
    fn shutdown(service: &mut DbService) {
        service.shutdown();
    }

    #[test]
    fn save_list_delete_and_reload() {
        let dir = TempDir::new("crud");
        let mut service = DbService::new(dir.config_path(), None);

        saved(&mut service, "b", "zebra", &dir.db_path(), false);
        saved(&mut service, "a", "Alpha", &dir.db_path(), true);

        let list = service.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].connection_id, "a");
        assert_eq!(list[0].name, "Alpha");
        assert_eq!(list[0].state, DbConnectionState::Disconnected);
        assert_eq!(list[1].connection_id, "b");

        let text = fs::read_to_string(dir.config_path()).expect("读取 db.json");
        assert!(text.contains("\"connections\""));
        assert!(!text.contains("password"), "配置文件不得包含密码字段");

        service.delete_connection("b").expect("删除连接");
        assert_eq!(service.list().len(), 1);
        assert!(
            matches!(
                service.delete_connection("b"),
                Err(DbServiceError::NotFound(_))
            ),
            "重复删除必须返回 NotFound"
        );

        drop(service);
        let reloaded = DbService::new(dir.config_path(), None);
        assert!(reloaded.load_warning().is_none());
        let list = reloaded.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].connection_id, "a");
        assert_eq!(list[0].name, "Alpha");
        assert_eq!(list[0].state, DbConnectionState::Disconnected);
        assert!(list[0].read_only);
    }

    #[test]
    fn save_validates_and_upserts_same_id() {
        let dir = TempDir::new("validate");
        let mut service = DbService::new(dir.config_path(), None);
        let database = dir.db_path();

        let info = saved(&mut service, "main", "Main", &database, false);
        assert_eq!(info.connection_id, "main");
        assert!(info.host.is_none(), "SQLite 允许空 host");
        assert_eq!(info.database, Some(database.to_string_lossy().into_owned()));

        let updated = saved(&mut service, "main", "Renamed", &database, false);
        assert_eq!(updated.name, "Renamed");
        assert_eq!(service.list().len(), 1, "同 id 必须 upsert");

        let mut empty_db = sqlite_config("x", "x", Path::new(""), false);
        empty_db.database = "   ".to_string();
        assert!(matches!(
            service.save_connection(empty_db),
            Err(DbServiceError::InvalidArgument(_))
        ));

        let mut empty_id = sqlite_config("", "x", &database, false);
        empty_id.connection_id = String::new();
        assert!(matches!(
            service.save_connection(empty_id),
            Err(DbServiceError::InvalidArgument(_))
        ));

        let mut postgres = sqlite_config("pg", "pg", Path::new("ignored"), false);
        postgres.kind = DbKind::Postgres;
        postgres.database = "app".to_string();
        postgres.host = String::new();
        postgres.port = 5432;
        assert!(matches!(
            service.save_connection(postgres.clone()),
            Err(DbServiceError::InvalidArgument(message)) if message.contains("host")
        ));

        postgres.host = "localhost".to_string();
        postgres.port = 0;
        assert!(matches!(
            service.save_connection(postgres),
            Err(DbServiceError::InvalidArgument(message)) if message.contains("port")
        ));

        let mysql = DbConnectionConfig {
            connection_id: "my".to_string(),
            name: "My".to_string(),
            kind: DbKind::Mysql,
            host: "localhost".to_string(),
            port: 3306,
            database: "app".to_string(),
            user: "app".to_string(),
            secret_ref: "env:MYSQL_PASSWORD".to_string(),
            read_only: true,
            extra: Some("{\"ssl\":true}".to_string()),
        };
        assert!(service.save_connection(mysql).is_ok());
    }

    #[test]
    fn node_unavailable_keeps_config_api_but_connect_fails() {
        let dir = TempDir::new("nonode");
        let mut service = DbService::new(dir.config_path(), None);
        assert!(service.node_path().is_none());

        let info = saved(&mut service, "main", "Main", &dir.db_path(), false);
        assert_eq!(info.state, DbConnectionState::Disconnected);
        assert_eq!(service.list().len(), 1);

        let error = service.connect("main").expect_err("Node 缺失必须报错");
        assert!(
            matches!(&error, DbServiceError::DbUnavailable(message) if message.contains("Node"))
        );
        assert_eq!(error.code(), ErrorCode::DbUnavailable);
        assert!(error.to_error_body().message.contains("Node"));

        let list = service.list();
        assert_eq!(list[0].state, DbConnectionState::Error);
        assert!(
            list[0]
                .error
                .as_deref()
                .is_some_and(|text| text.contains("Node"))
        );

        let disconnected = service.disconnect("main").expect("清理错误状态");
        assert_eq!(disconnected.state, DbConnectionState::Disconnected);
        assert!(disconnected.error.is_none());

        service.delete_connection("main").expect("删除仍可用");
        drop(service);

        let mut reloaded = DbService::new(dir.config_path(), None);
        assert!(reloaded.list().is_empty());
        shutdown(&mut reloaded);
    }

    #[test]
    fn connect_query_execute_schema_explain_disconnect_roundtrip() {
        let Some(node) = test_node() else { return };
        let dir = TempDir::new("roundtrip");
        let mut service = DbService::new(dir.config_path(), Some(node));
        saved(&mut service, "main", "Main", &dir.db_path(), false);

        let info = service.connect("main").expect("连接 SQLite");
        assert_eq!(info.state, DbConnectionState::Connected);
        assert!(info.error.is_none());
        assert_eq!(
            service.connect("main").expect("幂等 connect").state,
            DbConnectionState::Connected
        );

        service
            .execute(
                "main",
                "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, note TEXT)",
            )
            .expect("建表");
        let inserted = service
            .execute(
                "main",
                "INSERT INTO items (id, name, note) VALUES (1, 'alpha', NULL), (2, 'beta', 'x')",
            )
            .expect("插入");
        assert_eq!(inserted.affected_rows, 2);

        let result = service
            .query(
                "main",
                "SELECT id, name, note FROM items ORDER BY id",
                Some(10),
            )
            .expect("查询");
        assert_eq!(
            result
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "name", "note"]
        );
        assert_eq!(
            result.rows[0],
            vec![Some("1".to_string()), Some("alpha".to_string()), None]
        );
        assert_eq!(result.rows[1][2], Some("x".to_string()));
        assert_eq!(result.row_count, 2);
        assert!(result.untrusted);

        let nodes = service.schema("main").expect("schema");
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].children.iter().any(|table| table.name == "items"));

        let plan = service
            .explain("main", "SELECT * FROM items WHERE id = 1")
            .expect("explain");
        assert!(
            plan.plan.to_lowercase().contains("items"),
            "plan: {}",
            plan.plan
        );

        let info = service.disconnect("main").expect("断开连接");
        assert_eq!(info.state, DbConnectionState::Disconnected);
        assert!(matches!(
            service.query("main", "SELECT 1", None),
            Err(DbServiceError::DbUnavailable(_))
        ));

        shutdown(&mut service);
        assert_eq!(service.list()[0].state, DbConnectionState::Disconnected);
    }

    #[test]
    fn read_only_connection_rejects_write() {
        let Some(node) = test_node() else { return };
        let dir = TempDir::new("readonly");
        let mut service = DbService::new(dir.config_path(), Some(node));
        let database = dir.db_path();
        saved(&mut service, "rw", "RW", &database, false);
        saved(&mut service, "ro", "RO", &database, true);

        service.connect("rw").expect("读写连接");
        service
            .execute("rw", "CREATE TABLE items (id INTEGER, name TEXT)")
            .expect("建表");
        service.connect("ro").expect("只读连接");

        let read = service
            .query("ro", "SELECT * FROM items", None)
            .expect("只读查询");
        assert_eq!(read.row_count, 0);

        let error = service
            .execute("ro", "INSERT INTO items (id, name) VALUES (1, 'x')")
            .expect_err("只读连接必须拒绝写");
        assert!(matches!(error, DbServiceError::DbReadOnly(_)));
        assert_eq!(error.code(), ErrorCode::DbReadOnly);

        let ddl = service
            .execute("ro", "CREATE TABLE other (id INTEGER)")
            .expect_err("只读连接必须拒绝 DDL");
        assert!(matches!(ddl, DbServiceError::DbReadOnly(_)));

        // 拒绝写之后连接仍可用（sidecar 不因 SQL 拒绝而退出）。
        assert!(
            service
                .query("ro", "SELECT COUNT(*) AS n FROM items", None)
                .is_ok()
        );
        shutdown(&mut service);
    }

    #[test]
    fn update_without_where_is_rejected() {
        let Some(node) = test_node() else { return };
        let dir = TempDir::new("nowhere");
        let mut service = DbService::new(dir.config_path(), Some(node));
        saved(&mut service, "main", "Main", &dir.db_path(), false);
        service.connect("main").expect("连接");

        service
            .execute("main", "CREATE TABLE items (id INTEGER, name TEXT)")
            .expect("建表");
        service
            .execute("main", "INSERT INTO items (id, name) VALUES (1, 'keep')")
            .expect("插入");

        let error = service
            .execute("main", "UPDATE items SET name = 'changed'")
            .expect_err("无 WHERE 的 UPDATE 必须被拒绝");
        assert!(matches!(&error, DbServiceError::DbRejected(message) if message.contains("WHERE")));
        assert_eq!(error.code(), ErrorCode::DbRejected);

        let unchanged = service
            .query("main", "SELECT name FROM items WHERE id = 1", None)
            .expect("复查");
        assert_eq!(unchanged.rows, vec![vec![Some("keep".to_string())]]);
        shutdown(&mut service);
    }

    #[test]
    fn unknown_and_unconnected_connections_report_errors() {
        let dir = TempDir::new("errors");
        let mut service = DbService::new(dir.config_path(), None);

        assert!(matches!(
            service.query("missing", "SELECT 1", None),
            Err(DbServiceError::NotFound(_))
        ));
        assert!(matches!(
            service.connect("missing"),
            Err(DbServiceError::NotFound(_))
        ));
        assert!(matches!(
            service.schema("missing"),
            Err(DbServiceError::NotFound(_))
        ));
        assert!(matches!(
            service.disconnect("missing"),
            Err(DbServiceError::NotFound(_))
        ));
        assert!(matches!(
            service.delete_connection("missing"),
            Err(DbServiceError::NotFound(_))
        ));

        saved(&mut service, "main", "Main", &dir.db_path(), false);
        assert!(matches!(
            service.query("main", "SELECT 1", None),
            Err(DbServiceError::DbUnavailable(_))
        ));
        assert!(matches!(
            service.execute("main", "CREATE TABLE t (id INTEGER)"),
            Err(DbServiceError::DbUnavailable(_))
        ));
        assert!(matches!(
            service.explain("main", "SELECT 1"),
            Err(DbServiceError::DbUnavailable(_))
        ));
        assert!(matches!(
            service.connect("main"),
            Err(DbServiceError::DbUnavailable(_))
        ));
    }

    #[test]
    fn invalid_sql_is_rejected_before_sidecar() {
        let Some(node) = test_node() else { return };
        let dir = TempDir::new("guard");
        let mut service = DbService::new(dir.config_path(), Some(node));
        saved(&mut service, "main", "Main", &dir.db_path(), false);
        service.connect("main").expect("连接");
        service
            .execute("main", "CREATE TABLE items (id INTEGER, name TEXT)")
            .expect("建表");

        let multiple = service
            .query("main", "SELECT 1; SELECT 2", None)
            .expect_err("多语句必须被拒绝");
        assert!(matches!(multiple, DbServiceError::InvalidArgument(_)));
        assert_eq!(multiple.code(), ErrorCode::InvalidArgument);

        let drop_db = service
            .execute("main", "DROP DATABASE production")
            .expect_err("黑名单必须被拒绝");
        assert!(matches!(drop_db, DbServiceError::DbRejected(_)));
        assert_eq!(drop_db.code(), ErrorCode::DbRejected);

        let no_where = service
            .execute("main", "DELETE FROM items")
            .expect_err("无 WHERE DELETE 必须被拒绝");
        assert!(matches!(no_where, DbServiceError::DbRejected(_)));

        let write_via_query = service
            .query("main", "UPDATE items SET name = 'x' WHERE id = 1", None)
            .expect_err("db_query 不接受写语句");
        assert!(matches!(write_via_query, DbServiceError::DbRejected(_)));

        // 以上拒绝都发生在 SQL 层，连接仍然可用。
        assert!(service.query("main", "SELECT 1", None).is_ok());
        shutdown(&mut service);
    }

    #[test]
    fn query_limit_is_applied() {
        let Some(node) = test_node() else { return };
        let dir = TempDir::new("limit");
        let mut service = DbService::new(dir.config_path(), Some(node));
        saved(&mut service, "main", "Main", &dir.db_path(), false);
        service.connect("main").expect("连接");
        service
            .execute("main", "CREATE TABLE numbers (n INTEGER)")
            .expect("建表");

        let values = (1..=50)
            .map(|number| format!("({number})"))
            .collect::<Vec<_>>()
            .join(", ");
        service
            .execute("main", &format!("INSERT INTO numbers (n) VALUES {values}"))
            .expect("插入 50 行");

        let result = service
            .query("main", "SELECT n FROM numbers ORDER BY n", Some(5))
            .expect("限制查询");
        assert_eq!(result.rows.len(), 5);
        assert_eq!(result.row_count, 5);
        assert_eq!(result.rows[0][0].as_deref(), Some("1"));
        assert_eq!(result.rows[4][0].as_deref(), Some("5"));
        shutdown(&mut service);
    }

    #[test]
    fn deleting_connected_connection_closes_it() {
        let Some(node) = test_node() else { return };
        let dir = TempDir::new("delete-connected");
        let mut service = DbService::new(dir.config_path(), Some(node));
        saved(&mut service, "main", "Main", &dir.db_path(), false);
        service.connect("main").expect("连接");
        service
            .execute("main", "CREATE TABLE items (id INTEGER)")
            .expect("建表");

        service.delete_connection("main").expect("删除已连接项");
        assert!(service.list().is_empty());
        assert!(matches!(
            service.connect("main"),
            Err(DbServiceError::NotFound(_))
        ));

        // 同一 id 重新保存/连接：旧 sidecar 句柄必须已经释放。
        saved(&mut service, "main", "Main", &dir.db_path(), false);
        let info = service.connect("main").expect("重新连接");
        assert_eq!(info.state, DbConnectionState::Connected);
        shutdown(&mut service);
    }

    #[test]
    fn corrupt_config_is_reported_without_panicking() {
        let dir = TempDir::new("corrupt");
        fs::write(dir.config_path(), "{ not json").expect("写入损坏配置");
        let mut service = DbService::new(dir.config_path(), None);
        assert!(matches!(
            service.load_warning(),
            Some(DbServiceError::InvalidArgument(_))
        ));
        assert!(service.list().is_empty());

        // 损坏文件不影响后续保存：新配置覆盖旧文件。
        saved(&mut service, "main", "Main", &dir.db_path(), false);
        assert_eq!(service.list().len(), 1);
        shutdown(&mut service);
    }
}
