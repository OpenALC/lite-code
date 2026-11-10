//! P5 数据库命令层（docs/MILESTONES.md §8、docs/ARCHITECTURE.md §5.5）。
//!
//! 业务逻辑全部在 `lc_core::db::DbService`：本模块负责 `Mutex` 包装
//! （[`DbState`]）、把服务错误映射成契约 `ErrorBody`、把状态迁移转成
//! `db:status` 事件，以及为 Agent 组件执行器（[`agent_executor`]）。
//!
//! 锁约定：`DbState` 的外层锁只用于惰性初始化与取共享句柄，拿到
//! `Arc<Mutex<DbService>>` 后立即释放；`query`/`execute` 等 sidecar 调用
//! 持服务锁执行（可能数十秒），因此命令一律走 `async` + `spawn_blocking`，
//! 不阻塞 Tauri 主线程。同一时刻只有一个数据库操作（单 sidecar 串行）。
//!
//! 事件走与 agent/task 相同的 `core:event` 单通道，载荷是契约
//! [`EventEnvelope`]；seq 由本模块自己的 [`DbEvents`] 分配。只有改变连接
//! 状态的命令（connect/disconnect/save/delete）下发 `db:status`，
//! list/schema/query/execute/explain 不发状态事件。

use std::sync::{Arc, Mutex};

use lc_core::agent::{ToolCall, ToolError, ToolExecutor, ToolOutput};
use lc_core::db::{DbExecutor, DbService, DbServiceError};
use lc_proto::{
    CommandResult, CoreEvent, DbConnectionConfig, DbConnectionState, ErrorBody, ErrorCode,
    EventEnvelope,
};
use tauri::Emitter;

use crate::agent::{CORE_EVENT_CHANNEL, EventSink};

const DB_STATE_UNAVAILABLE: &str = "数据库服务不可用（锁中毒），请重启应用";

type SharedDb = Arc<Mutex<DbService>>;
type DbSlot = Arc<Mutex<Option<SharedDb>>>;
type DbLoader = Arc<dyn Fn() -> Result<DbService, DbServiceError> + Send + Sync>;

// ==== 应用级状态 ====

/// 数据库服务的应用级状态。首次数据库命令/会话创建时惰性
/// [`DbService::load_default`]；克隆共享同一服务，`workspace_open` 不重置
/// （连接是用户级配置，不随工作区切换）。
#[derive(Clone)]
pub(crate) struct DbState {
    inner: DbSlot,
    loader: DbLoader,
}

impl Default for DbState {
    fn default() -> Self {
        Self::with_loader(DbService::load_default)
    }
}

impl DbState {
    /// 注入服务构造器（测试用临时 `db.json`；生产走 [`DbState::default`]）。
    pub(crate) fn with_loader(
        loader: impl Fn() -> Result<DbService, DbServiceError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
            loader: Arc::new(loader),
        }
    }

    /// 共享服务句柄：首次调用构造 `DbService`，之后复用同一实例。
    /// 构造失败不缓存（下次命令会重试），错误按服务错误映射。
    pub(crate) fn shared(&self) -> Result<SharedDb, ErrorBody> {
        let mut slot = self
            .inner
            .lock()
            .map_err(|_| ErrorBody::new(ErrorCode::Internal, DB_STATE_UNAVAILABLE))?;
        if let Some(service) = slot.as_ref() {
            return Ok(Arc::clone(service));
        }
        let service = (self.loader)().map_err(|error| error.to_error_body())?;
        let shared = Arc::new(Mutex::new(service));
        *slot = Some(Arc::clone(&shared));
        Ok(shared)
    }

    /// 持服务锁执行一次服务调用；锁中毒同样走契约错误，不 panic。
    fn with_service<T>(
        &self,
        action: impl FnOnce(&mut DbService) -> Result<T, DbServiceError>,
    ) -> Result<T, ErrorBody> {
        let shared = self.shared()?;
        let mut service = shared
            .lock()
            .map_err(|_| ErrorBody::new(ErrorCode::Internal, DB_STATE_UNAVAILABLE))?;
        action(&mut service).map_err(|error| error.to_error_body())
    }
}

/// db lane 的事件序号。`DbEvents` 可克隆（内部共享同一计数器），
/// 与 agent/task 的 seq 相互独立。
#[derive(Default, Clone)]
pub(crate) struct DbEvents {
    seq: lc_host::envelope::Seq,
}

impl DbEvents {
    fn next_envelope(&self, event: CoreEvent) -> EventEnvelope {
        lc_host::envelope::envelope(&self.seq, event)
    }

    pub(crate) fn sink(&self, app: tauri::AppHandle) -> Arc<dyn EventSink> {
        Arc::new(DbEventSink {
            app,
            events: self.clone(),
        })
    }
}

struct DbEventSink {
    app: tauri::AppHandle,
    events: DbEvents,
}

impl EventSink for DbEventSink {
    fn emit(&self, event: CoreEvent) {
        // 窗口销毁/重建期间 emit 可能失败；事件流不是命令通道，忽略即可。
        let _ = self
            .app
            .emit(CORE_EVENT_CHANNEL, self.events.next_envelope(event));
    }
}

// ==== 命令纯逻辑（单测直接覆盖，不启动 Tauri 运行时） ====

fn list_connections(state: &DbState) -> Result<CommandResult, ErrorBody> {
    let connections = state.with_service(|service| Ok(service.list()))?;
    Ok(CommandResult::Connections { connections })
}

fn connect(
    state: &DbState,
    sink: &dyn EventSink,
    connection_id: &str,
) -> Result<CommandResult, ErrorBody> {
    let connection = state.with_service(|service| service.connect(connection_id))?;
    sink.emit(CoreEvent::DbStatus {
        connection: connection.clone(),
    });
    Ok(CommandResult::Connection { connection })
}

fn disconnect(
    state: &DbState,
    sink: &dyn EventSink,
    connection_id: &str,
) -> Result<CommandResult, ErrorBody> {
    let connection = state.with_service(|service| service.disconnect(connection_id))?;
    sink.emit(CoreEvent::DbStatus {
        connection: connection.clone(),
    });
    Ok(CommandResult::Connection { connection })
}

fn schema(state: &DbState, connection_id: &str) -> Result<CommandResult, ErrorBody> {
    let nodes = state.with_service(|service| service.schema(connection_id))?;
    Ok(CommandResult::DbSchema {
        connection_id: connection_id.to_string(),
        nodes,
    })
}

fn query(
    state: &DbState,
    connection_id: &str,
    sql: &str,
    limit: Option<u32>,
) -> Result<CommandResult, ErrorBody> {
    let result = state.with_service(|service| service.query(connection_id, sql, limit))?;
    Ok(CommandResult::DbQuery { result })
}

fn execute(state: &DbState, connection_id: &str, sql: &str) -> Result<CommandResult, ErrorBody> {
    let result = state.with_service(|service| service.execute(connection_id, sql))?;
    Ok(CommandResult::DbExec { result })
}

fn explain(state: &DbState, connection_id: &str, sql: &str) -> Result<CommandResult, ErrorBody> {
    let result = state.with_service(|service| service.explain(connection_id, sql))?;
    Ok(CommandResult::DbExplain { result })
}

fn save_connection(
    state: &DbState,
    sink: &dyn EventSink,
    connection: DbConnectionConfig,
) -> Result<CommandResult, ErrorBody> {
    let connection = state.with_service(|service| service.save_connection(connection))?;
    sink.emit(CoreEvent::DbStatus {
        connection: connection.clone(),
    });
    Ok(CommandResult::Connection { connection })
}

/// 删除配置。契约没有 `db:deleted` 事件，`db:status` 携带删除前的快照
/// （统一标为 `Disconnected`）作为状态迁移通知；命令结果仍是 `Ack`，
/// UI 以 `Ack` 为准从列表移除。
fn delete_connection(
    state: &DbState,
    sink: &dyn EventSink,
    connection_id: &str,
) -> Result<CommandResult, ErrorBody> {
    let removed = state.with_service(|service| {
        let info = service
            .list()
            .into_iter()
            .find(|connection| connection.connection_id == connection_id);
        service.delete_connection(connection_id)?;
        Ok(info)
    })?;
    if let Some(mut connection) = removed {
        connection.state = DbConnectionState::Disconnected;
        connection.error = None;
        sink.emit(CoreEvent::DbStatus { connection });
    }
    Ok(CommandResult::Ack)
}

// ==== 命令层异步包装 ====

/// sidecar 调用可能持续数十秒；命令在阻塞线程池执行，避免卡住主线程。
async fn blocking<T>(
    action: impl FnOnce() -> Result<T, ErrorBody> + Send + 'static,
) -> Result<T, ErrorBody>
where
    T: Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(action).await {
        Ok(result) => result,
        Err(error) => Err(ErrorBody::new(
            ErrorCode::Internal,
            format!("数据库命令线程失败：{error}"),
        )),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_list(state: tauri::State<'_, DbState>) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    blocking(move || list_connections(&db)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_connect(
    connection_id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, DbState>,
    events: tauri::State<'_, DbEvents>,
) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    let sink = events.sink(app);
    blocking(move || connect(&db, sink.as_ref(), &connection_id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_disconnect(
    connection_id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, DbState>,
    events: tauri::State<'_, DbEvents>,
) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    let sink = events.sink(app);
    blocking(move || disconnect(&db, sink.as_ref(), &connection_id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_schema(
    connection_id: String,
    state: tauri::State<'_, DbState>,
) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    blocking(move || schema(&db, &connection_id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_query(
    connection_id: String,
    sql: String,
    limit: Option<u32>,
    state: tauri::State<'_, DbState>,
) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    blocking(move || query(&db, &connection_id, &sql, limit)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_execute(
    connection_id: String,
    sql: String,
    state: tauri::State<'_, DbState>,
) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    blocking(move || execute(&db, &connection_id, &sql)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_explain(
    connection_id: String,
    sql: String,
    state: tauri::State<'_, DbState>,
) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    blocking(move || explain(&db, &connection_id, &sql)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_save_connection(
    connection: DbConnectionConfig,
    app: tauri::AppHandle,
    state: tauri::State<'_, DbState>,
    events: tauri::State<'_, DbEvents>,
) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    let sink = events.sink(app);
    blocking(move || save_connection(&db, sink.as_ref(), connection)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn db_delete_connection(
    connection_id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, DbState>,
    events: tauri::State<'_, DbEvents>,
) -> Result<CommandResult, ErrorBody> {
    let db = state.inner().clone();
    let sink = events.sink(app);
    blocking(move || delete_connection(&db, sink.as_ref(), &connection_id)).await
}

// ==== Agent 执行器组合 ====

const DB_TOOL_NAMES: [&str; 5] = [
    "db_list",
    "db_schema",
    "db_query",
    "db_explain",
    "db_execute",
];

fn is_db_tool(name: &str) -> bool {
    DB_TOOL_NAMES.contains(&name)
}

/// `session_create` 的执行器链：`db_*` 先由 [`DbExecutor`] 处理，其余工具
/// 交给 `fallback`（MCP → 本地执行器）。数据库初始化失败不阻止会话：
/// 用一个只对 `db_*` 返回 `ToolError::Database` 的包装器，其它工具照常走
/// fallback。
pub(crate) fn agent_executor(
    state: &DbState,
    fallback: Arc<dyn ToolExecutor>,
) -> Box<dyn ToolExecutor> {
    match state.shared() {
        Ok(service) => Box::new(DbExecutor::new(service).with_fallback(fallback)),
        Err(error) => {
            eprintln!("[db] Agent 数据库工具不可用：{}", error.message);
            Box::new(UnavailableDbExecutor { error, fallback })
        }
    }
}

struct UnavailableDbExecutor {
    error: ErrorBody,
    fallback: Arc<dyn ToolExecutor>,
}

impl ToolExecutor for UnavailableDbExecutor {
    fn execute(&self, call: &ToolCall) -> Result<ToolOutput, ToolError> {
        if is_db_tool(&call.name) {
            return Err(ToolError::Database(self.error.message.clone()));
        }
        self.fallback.execute(call)
    }
}

// ==== 单测：不启动 Tauri 运行时，记录型 sink + 临时配置/SQLite ====

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use lc_core::agent::ToolCall;
    use lc_proto::{DbConnectionInfo, DbKind, DbSchemaNodeKind};
    use serde_json::json;

    use super::*;

    #[derive(Default, Clone)]
    struct RecordingSink {
        events: Arc<Mutex<Vec<CoreEvent>>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event: CoreEvent) {
            self.events.lock().expect("recording sink").push(event);
        }
    }

    impl RecordingSink {
        fn events(&self) -> Vec<CoreEvent> {
            self.events.lock().expect("recording sink").clone()
        }

        fn clear(&self) {
            self.events.lock().expect("recording sink").clear();
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "lc-desktop-db-{}-{}-{}",
                tag,
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("创建临时目录");
            Self { path }
        }

        fn config_path(&self) -> PathBuf {
            self.path.join("db.json")
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
                eprintln!("跳过桌面数据库集成测试：找不到可用的 node（node:sqlite）");
                None
            }
        }
    }

    fn sqlite_config(id: &str, database: &Path) -> DbConnectionConfig {
        DbConnectionConfig {
            connection_id: id.to_string(),
            name: id.to_string(),
            kind: DbKind::Sqlite,
            host: String::new(),
            port: 0,
            database: database.to_string_lossy().into_owned(),
            user: String::new(),
            secret_ref: String::new(),
            read_only: false,
            extra: None,
        }
    }

    struct Fixture {
        dir: TempDir,
        state: DbState,
        loads: Arc<AtomicU64>,
    }

    fn fixture(tag: &str, node: Option<PathBuf>) -> Fixture {
        let dir = TempDir::new(tag);
        let config_path = dir.config_path();
        let loads = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&loads);
        let state = DbState::with_loader(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(DbService::new(&config_path, node.clone()))
        });
        Fixture { dir, state, loads }
    }

    fn shutdown(fixture: &Fixture) {
        fixture
            .state
            .with_service(|service| {
                service.shutdown();
                Ok(())
            })
            .expect("shutdown");
    }

    fn connection_of(result: CommandResult) -> DbConnectionInfo {
        match result {
            CommandResult::Connection { connection } => connection,
            other => panic!("expected connection result: {other:?}"),
        }
    }

    #[test]
    fn state_is_lazy_and_clone_shares_the_service() {
        let fixture = fixture("lazy", None);
        let clone = fixture.state.clone();

        assert_eq!(fixture.loads.load(Ordering::SeqCst), 0, "尚未初始化");
        let config = sqlite_config("main", &fixture.dir.db_path());
        let sink = RecordingSink::default();
        let saved = connection_of(save_connection(&clone, &sink, config).expect("save via clone"));
        assert_eq!(saved.connection_id, "main");
        assert_eq!(fixture.loads.load(Ordering::SeqCst), 1, "只初始化一次");

        let listed = list_connections(&fixture.state).expect("list via original");
        let CommandResult::Connections { connections } = listed else {
            panic!("expected connections");
        };
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].connection_id, "main");
        assert_eq!(fixture.loads.load(Ordering::SeqCst), 1, "克隆复用同一服务");
    }

    #[test]
    fn loader_failure_maps_to_db_unavailable_and_is_retried() {
        let loads = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&loads);
        let state = DbState::with_loader(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(DbServiceError::DbUnavailable(
                "Node 运行时不可用（未探测到可用的 node）".into(),
            ))
        });

        let first = list_connections(&state).expect_err("loader must fail");
        assert_eq!(first.code, ErrorCode::DbUnavailable);
        assert!(first.message.contains("Node"), "{}", first.message);
        let second = list_connections(&state).expect_err("loader retried");
        assert_eq!(second.code, ErrorCode::DbUnavailable);
        assert_eq!(loads.load(Ordering::SeqCst), 2, "失败不缓存");
    }

    #[test]
    fn poisoned_state_lock_maps_to_internal() {
        let fixture = fixture("poison-state", None);
        let poison = fixture.state.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poison.inner.lock().expect("inner lock");
            panic!("poison the state lock");
        })
        .join();

        let error = list_connections(&fixture.state).expect_err("poisoned lock");
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("锁中毒"), "{}", error.message);
    }

    #[test]
    fn poisoned_service_lock_maps_to_internal() {
        let fixture = fixture("poison-service", None);
        let shared = fixture.state.shared().expect("initialize");
        let _ = std::thread::spawn(move || {
            let _guard = shared.lock().expect("service lock");
            panic!("poison the service lock");
        })
        .join();

        let error = list_connections(&fixture.state).expect_err("poisoned service");
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("锁中毒"), "{}", error.message);
    }

    #[test]
    fn list_save_delete_map_contract_results_and_events() {
        let fixture = fixture("crud", None);
        let sink = RecordingSink::default();

        let saved = save_connection(
            &fixture.state,
            &sink,
            sqlite_config("main", &fixture.dir.db_path()),
        )
        .expect("save");
        assert_eq!(
            serde_json::to_value(&saved).expect("serialize")["kind"],
            json!("connection")
        );
        let connection = connection_of(saved);
        assert_eq!(connection.connection_id, "main");
        assert_eq!(connection.state, DbConnectionState::Disconnected);
        assert_eq!(sink.events().len(), 1, "save 下发 db:status");
        assert!(matches!(
            &sink.events()[0],
            CoreEvent::DbStatus { connection } if connection.connection_id == "main"
        ));

        let listed = list_connections(&fixture.state).expect("list");
        let CommandResult::Connections { connections } = &listed else {
            panic!("expected connections");
        };
        assert_eq!(connections.len(), 1);
        assert_eq!(
            serde_json::to_value(&listed).expect("serialize")["kind"],
            json!("connections")
        );

        sink.clear();
        assert_eq!(
            delete_connection(&fixture.state, &sink, "main").expect("delete"),
            CommandResult::Ack
        );
        let events = sink.events();
        assert_eq!(events.len(), 1, "delete 下发 db:status");
        let CoreEvent::DbStatus { connection } = &events[0] else {
            panic!("expected db:status");
        };
        assert_eq!(connection.connection_id, "main");
        assert_eq!(connection.state, DbConnectionState::Disconnected);
        assert!(
            matches!(
                list_connections(&fixture.state).expect("list after delete"),
                CommandResult::Connections { connections } if connections.is_empty()
            ),
            "删除后列表为空"
        );

        let missing = delete_connection(&fixture.state, &sink, "missing").expect_err("unknown");
        assert_eq!(missing.code, ErrorCode::NotFound);
    }

    #[test]
    fn connect_and_disconnect_map_events_and_errors() {
        let fixture = fixture("connect", None);
        let sink = RecordingSink::default();
        save_connection(
            &fixture.state,
            &sink,
            sqlite_config("main", &fixture.dir.db_path()),
        )
        .expect("save");
        sink.clear();

        // 无 Node：connect 必须报 DbUnavailable，且不下发状态事件。
        let error = connect(&fixture.state, &sink, "main").expect_err("no node");
        assert_eq!(error.code, ErrorCode::DbUnavailable);
        assert!(error.message.contains("Node"), "{}", error.message);
        assert!(sink.events().is_empty(), "失败不迁移状态");

        let unknown = connect(&fixture.state, &sink, "ghost").expect_err("unknown");
        assert_eq!(unknown.code, ErrorCode::NotFound);

        let disconnected =
            connection_of(disconnect(&fixture.state, &sink, "main").expect("disconnect"));
        assert_eq!(disconnected.state, DbConnectionState::Disconnected);
        let events = sink.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(matches!(
            &events[0],
            CoreEvent::DbStatus { connection }
                if connection.connection_id == "main"
                    && connection.state == DbConnectionState::Disconnected
        ));

        let missing = disconnect(&fixture.state, &sink, "ghost").expect_err("unknown");
        assert_eq!(missing.code, ErrorCode::NotFound);
    }

    #[test]
    fn read_commands_preserve_service_error_codes() {
        let fixture = fixture("read-errors", None);
        let sink = RecordingSink::default();
        save_connection(
            &fixture.state,
            &sink,
            sqlite_config("main", &fixture.dir.db_path()),
        )
        .expect("save");

        assert_eq!(
            schema(&fixture.state, "ghost")
                .expect_err("unknown schema")
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            schema(&fixture.state, "main")
                .expect_err("unconnected schema")
                .code,
            ErrorCode::DbUnavailable
        );
        assert_eq!(
            query(&fixture.state, "ghost", "SELECT 1", None)
                .expect_err("unknown query")
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            query(&fixture.state, "main", "SELECT 1", None)
                .expect_err("unconnected query")
                .code,
            ErrorCode::DbUnavailable
        );
        assert_eq!(
            execute(&fixture.state, "main", "CREATE TABLE t (id INTEGER)")
                .expect_err("unconnected execute")
                .code,
            ErrorCode::DbUnavailable
        );
        assert_eq!(
            explain(&fixture.state, "main", "SELECT 1")
                .expect_err("unconnected explain")
                .code,
            ErrorCode::DbUnavailable
        );
        assert!(sink.events().len() <= 1, "读命令不发状态事件");
    }

    #[test]
    fn sqlite_roundtrip_maps_schema_query_execute_explain() {
        let Some(node) = test_node() else { return };
        let fixture = fixture("roundtrip", Some(node));
        let sink = RecordingSink::default();
        save_connection(
            &fixture.state,
            &sink,
            sqlite_config("main", &fixture.dir.db_path()),
        )
        .expect("save");

        let connected = connection_of(connect(&fixture.state, &sink, "main").expect("connect"));
        assert_eq!(connected.state, DbConnectionState::Connected);
        assert!(connected.error.is_none());

        let created = execute(
            &fixture.state,
            "main",
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)",
        )
        .expect("create table");
        assert!(matches!(created, CommandResult::DbExec { .. }));

        let inserted = execute(
            &fixture.state,
            "main",
            "INSERT INTO items (id, name) VALUES (1, 'alpha'), (2, 'beta')",
        )
        .expect("insert");
        let CommandResult::DbExec { result } = inserted else {
            panic!("expected db_exec");
        };
        assert_eq!(result.affected_rows, 2);

        let queried = query(
            &fixture.state,
            "main",
            "SELECT id, name FROM items ORDER BY id",
            Some(1),
        )
        .expect("query");
        let CommandResult::DbQuery { result } = queried else {
            panic!("expected db_query");
        };
        assert_eq!(result.row_count, 1);
        assert_eq!(result.rows.len(), 1);
        assert!(result.untrusted);
        assert_eq!(result.rows[0][1].as_deref(), Some("alpha"));

        let schema_result = schema(&fixture.state, "main").expect("schema");
        let CommandResult::DbSchema {
            connection_id,
            nodes,
        } = schema_result
        else {
            panic!("expected db_schema");
        };
        assert_eq!(connection_id, "main");
        assert_eq!(nodes[0].kind, DbSchemaNodeKind::Database);
        assert!(nodes[0].children.iter().any(|table| table.name == "items"));

        let explained =
            explain(&fixture.state, "main", "SELECT * FROM items WHERE id = 1").expect("explain");
        let CommandResult::DbExplain { result } = explained else {
            panic!("expected db_explain");
        };
        assert!(
            result.plan.to_lowercase().contains("items"),
            "{}",
            result.plan
        );

        let rejected =
            execute(&fixture.state, "main", "DELETE FROM items").expect_err("delete without where");
        assert_eq!(rejected.code, ErrorCode::DbRejected);

        shutdown(&fixture);
    }

    #[test]
    fn db_status_envelopes_are_sequenced_and_snake_case() {
        let events = DbEvents::default();
        let connection = DbConnectionInfo {
            connection_id: "main".into(),
            name: "Main".into(),
            kind: DbKind::Sqlite,
            read_only: false,
            state: DbConnectionState::Connected,
            host: None,
            port: 0,
            user: None,
            secret_ref: None,
            database: Some("app.sqlite".into()),
            error: None,
        };
        let first = events.next_envelope(CoreEvent::DbStatus {
            connection: connection.clone(),
        });
        // M3a：seq 语义统一到 lc-host `Seq`（1 起；原 0 基 fetch_add 的
        // 0/1 对应现在的 1/2）。
        assert_eq!(first.seq, 1);
        let json = serde_json::to_value(&first).expect("serialize db status");
        assert_eq!(json["seq"], 1);
        assert_eq!(json["event"]["type"], "db:status");
        assert_eq!(json["event"]["connection"]["connection_id"], "main");
        assert_eq!(json["event"]["connection"]["state"], "connected");
        assert_eq!(json["event"]["connection"]["read_only"], false);
        assert_eq!(json["event"]["connection"]["kind"], "sqlite");

        let second = events.next_envelope(CoreEvent::DbStatus { connection });
        assert_eq!(second.seq, 2);
    }

    #[test]
    fn cloned_event_handles_share_the_sequence() {
        let events = DbEvents::default();
        let clone = events.clone();
        let first = events.next_envelope(CoreEvent::DbStatus {
            connection: DbConnectionInfo {
                connection_id: "a".into(),
                name: "a".into(),
                kind: DbKind::Sqlite,
                read_only: false,
                state: DbConnectionState::Disconnected,
                host: None,
                port: 0,
                user: None,
                secret_ref: None,
                database: None,
                error: None,
            },
        });
        let second = clone.next_envelope(CoreEvent::DbStatus {
            connection: DbConnectionInfo {
                connection_id: "b".into(),
                name: "b".into(),
                kind: DbKind::Sqlite,
                read_only: false,
                state: DbConnectionState::Disconnected,
                host: None,
                port: 0,
                user: None,
                secret_ref: None,
                database: None,
                error: None,
            },
        });
        // M3a：1 基 seq（原 0 基的 0/1 对应现在的 1/2）。
        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2, "克隆共享同一 seq 计数器");
    }

    struct Fallback;

    impl ToolExecutor for Fallback {
        fn execute(&self, call: &ToolCall) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                text: format!("fallback:{}", call.name),
            })
        }
    }

    #[test]
    fn agent_executor_routes_db_tools_and_falls_back() {
        let fixture = fixture("agent-chain", None);
        let sink = RecordingSink::default();
        save_connection(
            &fixture.state,
            &sink,
            sqlite_config("main", &fixture.dir.db_path()),
        )
        .expect("save");

        let executor = agent_executor(&fixture.state, Arc::new(Fallback));
        let listed = executor
            .execute(&ToolCall::new("db_list", json!({})))
            .expect("db_list routed to DbExecutor");
        let value: serde_json::Value = serde_json::from_str(&listed.text).expect("db_list JSON");
        assert_eq!(value[0]["connection_id"], "main");

        let fallback = executor
            .execute(&ToolCall::new("read_file", json!({"path": "a.txt"})))
            .expect("non-db tool");
        assert_eq!(fallback.text, "fallback:read_file");

        let missing = executor
            .execute(&ToolCall::new(
                "db_query",
                json!({"connection_id": "ghost", "sql": "SELECT 1"}),
            ))
            .expect_err("db error must not fall back");
        assert!(matches!(missing, ToolError::Database(_)));
    }

    #[test]
    fn agent_executor_keeps_session_usable_when_db_init_fails() {
        let state = DbState::with_loader(|| {
            Err(DbServiceError::DbUnavailable(
                "无法定位数据目录".to_string(),
            ))
        });
        let executor = agent_executor(&state, Arc::new(Fallback));

        let db_error = executor
            .execute(&ToolCall::new("db_list", json!({})))
            .expect_err("db tool must report unavailable");
        assert!(matches!(&db_error, ToolError::Database(message) if message.contains("无法定位")));

        let local = executor
            .execute(&ToolCall::new("shell", json!({"command": "echo hi"})))
            .expect("non-db tool still works");
        assert_eq!(local.text, "fallback:shell");
    }
}
