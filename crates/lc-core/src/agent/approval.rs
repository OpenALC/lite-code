//! 审批队列与"本次会话允许同类"缓存（docs/AGENT.md §5.4）。
//!
//! 纯状态：UI 只负责展示 `ApprovalRequest` 并回传 `ApprovalDecision`；
//! 任何工具执行前必须先经过 `Session::authorize`。

use std::collections::HashSet;

use serde_json::Value;

use super::policy::PolicySource;

pub type ApprovalId = u64;

/// 审批摘要：来自工具的实际执行参数，用于审批卡展示与 `AllowSession`
/// 缓存键。**不能**直接采信请求里的 `command` 字段——`db_*` 与 MCP 工具
/// 的真实参数不是它，伪造的 `command` 会让审批卡与缓存键偏离实际执行内容。
///
/// - `shell`：`command`
/// - `db_*`：`connection_id` + `sql`（无 SQL 的 `db_list` / `db_schema` 回退到完整参数）
/// - `write_file` / `edit_file`：`path`
/// - 其它工具（含 MCP）：紧凑 JSON 参数
///
/// 参数缺失或类型不对时回退到紧凑 JSON，保证摘要始终反映实际参数。
pub(crate) fn approval_summary(tool: &str, args: &Value) -> Option<String> {
    if tool.starts_with("db_")
        && let Some(sql) = args.get("sql").and_then(Value::as_str)
    {
        let connection = args
            .get("connection_id")
            .and_then(Value::as_str)
            .unwrap_or("(unknown)");
        return Some(format!("connection {connection}:\n{sql}"));
    }
    let field = match tool {
        "shell" => Some("command"),
        "write_file" | "edit_file" => Some("path"),
        _ => None,
    };
    if let Some(field) = field
        && let Some(value) = args.get(field).and_then(Value::as_str)
    {
        return Some(value.to_string());
    }
    if args.is_null() {
        // MCP 客户端会把 null 参数规范化为 {}；摘要保持一致。
        return Some("{}".to_string());
    }
    Some(args.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub tool: String,
    /// 审批摘要（见 [`approval_summary`]）：shell 命令原文、SQL、文件路径或
    /// 紧凑参数 JSON。契约字段名保持 `command`，但不再等同于"命令"。
    pub command: Option<String>,
    pub source: PolicySource,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// 只允许这一次。
    AllowOnce,
    /// 本次会话允许同类（tool + 完整审批摘要完全相同）。
    AllowSession,
    Deny,
}

/// 同类判定键：工具名 + 完整审批摘要。参数不同的调用不会互相放行。
fn class_key(tool: &str, command: Option<&str>) -> (String, Option<String>) {
    (tool.to_string(), command.map(str::to_string))
}

#[derive(Debug, Default, Clone)]
pub struct ApprovalCache {
    allowed: HashSet<(String, Option<String>)>,
}

impl ApprovalCache {
    pub fn allow_session(&mut self, tool: &str, command: Option<&str>) {
        self.allowed.insert(class_key(tool, command));
    }

    pub fn allows(&self, tool: &str, command: Option<&str>) -> bool {
        self.allowed.contains(&class_key(tool, command))
    }

    pub fn len(&self) -> usize {
        self.allowed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }
}

#[derive(Debug, Default, Clone)]
pub struct ApprovalQueue {
    next_id: ApprovalId,
    pending: Vec<ApprovalRequest>,
}

impl ApprovalQueue {
    pub fn push(
        &mut self,
        tool: impl Into<String>,
        command: Option<&str>,
        source: PolicySource,
        reason: impl Into<String>,
    ) -> ApprovalRequest {
        let id = self.next_id;
        self.next_id += 1;
        let request = ApprovalRequest {
            id,
            tool: tool.into(),
            command: command.map(str::to_string),
            source,
            reason: reason.into(),
        };
        self.pending.push(request.clone());
        request
    }

    pub fn pending(&self) -> &[ApprovalRequest] {
        &self.pending
    }

    pub fn take(&mut self, id: ApprovalId) -> Option<ApprovalRequest> {
        let index = self.pending.iter().position(|r| r.id == id)?;
        Some(self.pending.remove(index))
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_ids_are_monotonic_and_take_removes() {
        let mut queue = ApprovalQueue::default();
        let first = queue.push("shell", Some("ls"), PolicySource::ExecuteReview, "review");
        let second = queue.push(
            "write_file",
            None,
            PolicySource::OutsideWorkspace,
            "outside",
        );
        assert_eq!(first.id, 0);
        assert_eq!(second.id, 1);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.take(first.id), Some(first));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.take(99), None);
    }

    #[test]
    fn session_cache_matches_tool_and_command_exactly() {
        let mut cache = ApprovalCache::default();
        cache.allow_session("shell", Some("npm test"));
        assert!(cache.allows("shell", Some("npm test")));
        assert!(!cache.allows("shell", Some("npm publish")));
        assert!(!cache.allows("write_file", None));

        cache.allow_session("write_file", None);
        assert!(cache.allows("write_file", None));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn db_session_cache_is_scoped_to_connection() {
        use serde_json::json;
        let dev = approval_summary(
            "db_execute",
            &json!({"connection_id": "dev", "sql": "DELETE FROM users WHERE id=1"}),
        )
        .expect("dev summary");
        let prod = approval_summary(
            "db_execute",
            &json!({"connection_id": "prod", "sql": "DELETE FROM users WHERE id=1"}),
        )
        .expect("prod summary");
        assert_ne!(dev, prod);

        let mut cache = ApprovalCache::default();
        cache.allow_session("db_execute", Some(&dev));
        assert!(cache.allows("db_execute", Some(&dev)));
        assert!(!cache.allows("db_execute", Some(&prod)));
    }

    #[test]
    fn approval_summary_uses_actual_execution_arguments() {
        use serde_json::json;

        assert_eq!(
            approval_summary(
                "shell",
                &json!({"command": "rm -rf build", "timeout_ms": 1000})
            )
            .as_deref(),
            Some("rm -rf build")
        );
        // db_* 的实际参数是 sql；伪造的 command 不能影响审批卡或缓存键。
        assert_eq!(
            approval_summary(
                "db_execute",
                &json!({"connection_id": "main", "sql": "DELETE FROM t", "command": "echo safe"})
            )
            .as_deref(),
            Some("connection main:\nDELETE FROM t")
        );
        assert_eq!(
            approval_summary(
                "db_query",
                &json!({"connection_id": "main", "sql": "SELECT 1", "command": "echo safe"})
            )
            .as_deref(),
            Some("connection main:\nSELECT 1")
        );
        // 无 SQL 的数据库工具按完整参数归类。
        assert_eq!(
            approval_summary("db_schema", &json!({"connection_id": "main"})).as_deref(),
            Some(r#"{"connection_id":"main"}"#)
        );
        assert_eq!(
            approval_summary("write_file", &json!({"path": "src/a.ts", "content": "x"})).as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(
            approval_summary("edit_file", &json!({"path": "a.ts", "old_string": "a"})).as_deref(),
            Some("a.ts")
        );
        // MCP 等动态工具：完整紧凑 JSON，参数变化即换缓存键。
        assert_eq!(
            approval_summary("mcp_fs_read", &json!({"path": "a"})).as_deref(),
            Some(r#"{"path":"a"}"#)
        );
        assert_eq!(
            approval_summary("db_list", &json!({})).as_deref(),
            Some("{}")
        );
    }
}
