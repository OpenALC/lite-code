//! db lane 服务容器集成测试（docs/plans/db-mysql-pg.md §5.2）。
//!
//! 与 `src/db/sidecar.rs` 的单元测试互补：这里走**完整 sidecar 管道**
//! （`DbSidecar::spawn` 安装内嵌 bundle → node 子进程 → `initialize` 握手 →
//! `connect`（密码只经 `secret_ref` 环境引用解析）→ query / execute /
//! schema / explain），后端是真实 PostgreSQL / MySQL 服务。CI 由
//! db-integration job 提供服务容器；本机缺 node、缺密码环境变量或端口不可达
//! 时跳过，探测模式与 `sidecar.rs` 的 `test_node()` 一致。

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use lc_core::db::{DbError, DbSidecar};
use lc_proto::{DbConnectionConfig, DbKind, DbQueryResult, DbSchemaNode, DbSchemaNodeKind};

// CI 服务容器映射（.github/workflows/ci.yml db-integration job）：
// postgres:16 → 127.0.0.1:5432（库 postgres 由镜像自带，密码 test）；
// mysql:8.0 → 127.0.0.1:3306（root 密码 test，MYSQL_DATABASE=test）。
const PG_PORT: u16 = 5432;
const PG_DATABASE: &str = "postgres";
const PG_USER: &str = "postgres";
const MYSQL_PORT: u16 = 3306;
const MYSQL_DATABASE: &str = "test";
const MYSQL_USER: &str = "root";
const HOST: &str = "127.0.0.1";

/// 与 `sidecar.rs` 的 `test_node()` 相同：探测可用的 node（node:sqlite）。
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
            eprintln!("跳过 db 集成测试：找不到可用的 node（node:sqlite）");
            None
        }
    }
}

/// 密码环境变量已设置且服务端口可连通，才认为集成测试可以跑。
fn service_available(secret_var: &str, port: u16) -> bool {
    if std::env::var_os(secret_var).is_none() {
        eprintln!("跳过 db 集成测试：未设置 {secret_var}");
        return false;
    }
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
        Ok(_) => true,
        Err(error) => {
            eprintln!("跳过 db 集成测试：{HOST}:{port} 不可达（{error}）");
            false
        }
    }
}

fn remote_config(
    id: &str,
    kind: DbKind,
    port: u16,
    database: &str,
    user: &str,
    secret_ref: &str,
    read_only: bool,
) -> DbConnectionConfig {
    DbConnectionConfig {
        connection_id: id.to_string(),
        name: id.to_string(),
        kind,
        host: HOST.to_string(),
        port,
        database: database.to_string(),
        user: user.to_string(),
        secret_ref: secret_ref.to_string(),
        read_only,
        extra: None,
    }
}

fn start(node: &Path) -> DbSidecar {
    let mut sidecar = DbSidecar::spawn(node).expect("启动数据库 sidecar");
    sidecar.initialize().expect("初始化数据库 sidecar");
    sidecar
}

fn cell<'a>(result: &'a DbQueryResult, row: usize, column: usize) -> &'a str {
    result.rows[row][column]
        .as_deref()
        .expect("单元格不应为 NULL")
}

/// 在 schema 树里递归找表/视图节点（PG 是 database → schema → table 三层，
/// MySQL 是 database → table 两层）。
fn find_table<'a>(nodes: &'a [DbSchemaNode], name: &str) -> Option<&'a DbSchemaNode> {
    nodes
        .iter()
        .find(|node| {
            node.name == name
                && matches!(node.kind, DbSchemaNodeKind::Table | DbSchemaNodeKind::View)
        })
        .or_else(|| {
            nodes
                .iter()
                .find_map(|node| find_table(&node.children, name))
        })
}

fn pg_config(id: &str, read_only: bool) -> DbConnectionConfig {
    remote_config(
        id,
        DbKind::Postgres,
        PG_PORT,
        PG_DATABASE,
        PG_USER,
        "env:PGPASS",
        read_only,
    )
}

fn mysql_config(id: &str, read_only: bool) -> DbConnectionConfig {
    remote_config(
        id,
        DbKind::Mysql,
        MYSQL_PORT,
        MYSQL_DATABASE,
        MYSQL_USER,
        "env:MYSQL_PWD",
        read_only,
    )
}

#[test]
fn db_integration_postgres_query_execute_schema_explain() {
    let Some(node) = test_node() else { return };
    if !service_available("PGPASS", PG_PORT) {
        return;
    }
    let mut sidecar = start(&node);
    sidecar
        .connect(&pg_config("pg", false))
        .expect("连接 postgres");

    sidecar
        .execute("pg", "DROP TABLE IF EXISTS m2_pg_items")
        .expect("清理旧表");
    sidecar
        .execute(
            "pg",
            "CREATE TABLE m2_pg_items (id INTEGER PRIMARY KEY, name TEXT, note TEXT)",
        )
        .expect("建表");
    let inserted = sidecar
        .execute(
            "pg",
            "INSERT INTO m2_pg_items (id, name, note) VALUES (1, 'alpha', NULL), (2, 'beta', 'x')",
        )
        .expect("插入");
    assert_eq!(inserted.affected_rows, 2);

    let result = sidecar
        .query(
            "pg",
            "SELECT id, name, note FROM m2_pg_items ORDER BY id",
            None,
        )
        .expect("查询");
    assert_eq!(result.row_count, 2);
    assert!(!result.truncated);
    assert!(result.untrusted);
    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name", "note"]
    );
    assert_eq!(cell(&result, 0, 0), "1");
    assert_eq!(cell(&result, 0, 1), "alpha");
    assert_eq!(result.rows[0][2], None, "NULL 必须保真");
    assert_eq!(cell(&result, 1, 2), "x");

    // 单元格字节截断（§3.5）走完整管道：5000 字节 TEXT 截到 4 KiB 内并带后缀。
    let big = "p".repeat(5000);
    sidecar
        .execute(
            "pg",
            &format!("INSERT INTO m2_pg_items (id, name) VALUES (3, '{big}')"),
        )
        .expect("插入大文本");
    let big_result = sidecar
        .query("pg", "SELECT name FROM m2_pg_items WHERE id = 3", None)
        .expect("查询大文本");
    let value = cell(&big_result, 0, 0);
    assert!(
        value.ends_with('…'),
        "大单元格应带省略后缀：{}",
        &value[value.len().saturating_sub(32)..]
    );
    assert!(
        value.len() <= 4096,
        "单元格应截断到 4 KiB 内，实际 {} 字节",
        value.len()
    );

    // 结果集累计字节预算：40 行 × 4000 字节，第 33 行超限停取并标记 truncated。
    let row_text = "q".repeat(4000);
    for id in 10..50 {
        sidecar
            .execute(
                "pg",
                &format!("INSERT INTO m2_pg_items (id, name) VALUES ({id}, '{row_text}')"),
            )
            .expect("插入预算行");
    }
    let budget = sidecar
        .query(
            "pg",
            "SELECT name FROM m2_pg_items WHERE id >= 10 ORDER BY id",
            None,
        )
        .expect("预算查询");
    assert_eq!(budget.row_count, 32, "128 KiB 预算只放得下 32 行");
    assert!(budget.truncated);

    // schema 树：database → schema(public) → table → column。
    let schema = sidecar.schema("pg").expect("schema");
    assert_eq!(schema[0].kind, DbSchemaNodeKind::Database);
    let table = find_table(&schema, "m2_pg_items").expect("schema 树应有 m2_pg_items");
    assert!(
        table.children.iter().any(|column| column.name == "name"),
        "m2_pg_items 应有 name 列"
    );

    // explain：PG 返回 EXPLAIN 计划文本，不执行原语句。
    let explain = sidecar
        .explain("pg", "SELECT * FROM m2_pg_items WHERE id = 1")
        .expect("explain");
    assert!(!explain.plan.is_empty());
}

#[test]
fn db_integration_postgres_read_only_rejects_write() {
    let Some(node) = test_node() else { return };
    if !service_available("PGPASS", PG_PORT) {
        return;
    }
    let mut sidecar = start(&node);
    sidecar
        .connect(&pg_config("pg-ro", true))
        .expect("连接只读 postgres（bridge 侧建立只读会话）");

    sidecar
        .query("pg-ro", "SELECT 1 AS one", None)
        .expect("只读查询");

    let error = sidecar
        .execute("pg-ro", "CREATE TABLE m2_pg_ro (id INTEGER)")
        .expect_err("只读连接必须拒绝写");
    assert!(
        matches!(&error, DbError::ReadOnly(_) | DbError::Rejected(_)),
        "意外的错误类型：{error}"
    );
}

#[test]
fn db_integration_mysql_query_execute_schema_explain() {
    let Some(node) = test_node() else { return };
    if !service_available("MYSQL_PWD", MYSQL_PORT) {
        return;
    }
    let mut sidecar = start(&node);
    sidecar
        .connect(&mysql_config("my", false))
        .expect("连接 mysql");

    sidecar
        .execute("my", "DROP TABLE IF EXISTS m2_my_items")
        .expect("清理旧表");
    sidecar
        .execute(
            "my",
            "CREATE TABLE m2_my_items (id INTEGER PRIMARY KEY, name TEXT, note TEXT)",
        )
        .expect("建表");
    let inserted = sidecar
        .execute(
            "my",
            "INSERT INTO m2_my_items (id, name, note) VALUES (1, 'alpha', NULL), (2, 'beta', 'x')",
        )
        .expect("插入");
    assert_eq!(inserted.affected_rows, 2);

    let result = sidecar
        .query(
            "my",
            "SELECT id, name, note FROM m2_my_items ORDER BY id",
            None,
        )
        .expect("查询");
    assert_eq!(result.row_count, 2);
    assert!(!result.truncated);
    assert_eq!(cell(&result, 0, 1), "alpha");
    assert_eq!(result.rows[0][2], None, "NULL 必须保真");

    // 单元格字节截断走完整管道。
    let big = "m".repeat(5000);
    sidecar
        .execute(
            "my",
            &format!("INSERT INTO m2_my_items (id, name) VALUES (3, '{big}')"),
        )
        .expect("插入大文本");
    let big_result = sidecar
        .query("my", "SELECT name FROM m2_my_items WHERE id = 3", None)
        .expect("查询大文本");
    let value = cell(&big_result, 0, 0);
    assert!(value.ends_with('…'), "大单元格应带省略后缀");
    assert!(
        value.len() <= 4096,
        "单元格应截断到 4 KiB 内，实际 {} 字节",
        value.len()
    );

    // 结果集累计字节预算：40 行 × 4000 字节，超限停取并标记 truncated。
    let row_text = "n".repeat(4000);
    for id in 10..50 {
        sidecar
            .execute(
                "my",
                &format!("INSERT INTO m2_my_items (id, name) VALUES ({id}, '{row_text}')"),
            )
            .expect("插入预算行");
    }
    let budget = sidecar
        .query(
            "my",
            "SELECT name FROM m2_my_items WHERE id >= 10 ORDER BY id",
            None,
        )
        .expect("预算查询");
    assert_eq!(budget.row_count, 32, "128 KiB 预算只放得下 32 行");
    assert!(budget.truncated);

    // schema 树：MySQL 是扁平的 database → table → column。
    let schema = sidecar.schema("my").expect("schema");
    assert_eq!(schema[0].kind, DbSchemaNodeKind::Database);
    assert_eq!(schema[0].name, "test");
    let table = find_table(&schema, "m2_my_items").expect("schema 树应有 m2_my_items");
    assert!(
        table.children.iter().any(|column| column.name == "name"),
        "m2_my_items 应有 name 列"
    );

    let explain = sidecar
        .explain("my", "SELECT * FROM m2_my_items WHERE id = 1")
        .expect("explain");
    assert!(!explain.plan.is_empty());
}

#[test]
fn db_integration_mysql_read_only_rejects_write() {
    let Some(node) = test_node() else { return };
    if !service_available("MYSQL_PWD", MYSQL_PORT) {
        return;
    }
    let mut sidecar = start(&node);
    sidecar
        .connect(&mysql_config("my-ro", true))
        .expect("连接只读 mysql（bridge 侧建立只读会话）");

    sidecar
        .query("my-ro", "SELECT 1 AS one", None)
        .expect("只读查询");

    let error = sidecar
        .execute("my-ro", "CREATE TABLE m2_my_ro (id INTEGER)")
        .expect_err("只读连接必须拒绝写");
    assert!(
        matches!(&error, DbError::ReadOnly(_) | DbError::Rejected(_)),
        "意外的错误类型：{error}"
    );
}
