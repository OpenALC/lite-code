//! 审批队列与"本次会话允许同类"缓存（docs/AGENT.md §5.4）。
//!
//! 纯状态：UI 只负责展示 `ApprovalRequest` 并回传 `ApprovalDecision`；
//! 任何工具执行前必须先经过 `Session::authorize`。

use std::collections::HashSet;

use super::policy::PolicySource;

pub type ApprovalId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub tool: String,
    /// shell/SQL 的完整命令原文；文件工具为 None。
    pub command: Option<String>,
    pub source: PolicySource,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// 只允许这一次。
    AllowOnce,
    /// 本次会话允许同类（tool + command 完全相同）。
    AllowSession,
    Deny,
}

/// 同类判定键：工具名 + 命令原文。文件工具没有命令，按工具名归类。
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
}
