//! 数据库出口闸门（P5，docs/MILESTONES.md §8）。
//!
//! 本 lane 只做**不依赖数据库驱动**的纯逻辑 SQL 分类与改写：
//! - 分类（Read / Write / Ddl / Unknown，未知一律 fail-closed）；
//! - 单语句检查（顶层 `;` 切分，字符串/注释/括号内的分号不算）；
//! - 写语句必须有顶层 `WHERE`；
//! - 只读连接拒绝 Write / Ddl / Unknown；
//! - 静态黑名单（在去掉字符串与注释后的 token 上匹配）；
//! - `SELECT` / `WITH` 读查询的 `LIMIT` 注入。
//!
//! 明确不在本 lane：执行、超时、只读/写事务、协议级只读（sidecar 后续），
//! 以及查询结果的 `trust: untrusted` 标注。库内数据（包括“忽略之前指令”
//! 一类文本）永远是不可信数据，执行层负责转义/隔离，分类器不会因为字符串
//! 内容改变判定。

mod sql;

pub use sql::{GuardedSql, SqlError, SqlGuard, SqlKind};
