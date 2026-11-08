//! 诊断模型、下发回调与按 path 聚合的缓存。
//!
//! LSP 的 position 是 0-based；这里统一转换为 1-based，前端 Monaco 的
//! `IMarkerData` 同为 1-based。severity 沿用 LSP 的 1–4（1 = Error）。

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

/// 单条诊断；行列均从 1 开始。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// LSP severity：1 Error / 2 Warning / 3 Information / 4 Hint。
    pub severity: u8,
    pub message: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// 诊断下发回调。实现方必须可跨读线程共享。
///
/// `diagnostics` 为空表示 server 已清除该文件在它名下的全部标记。
/// 同一 path 可能收到多个 server 的独立事件；需要合并时用
/// [`DiagnosticStore`]。
pub trait DiagnosticSink: Send + Sync {
    fn publish(&self, server_id: &str, path: &str, diagnostics: &[Diagnostic]);
}

impl<F> DiagnosticSink for F
where
    F: Fn(&str, &str, &[Diagnostic]) + Send + Sync,
{
    fn publish(&self, server_id: &str, path: &str, diagnostics: &[Diagnostic]) {
        self(server_id, path, diagnostics);
    }
}

/// 内存诊断缓存：`path -> serverId -> diagnostics`。
///
/// 每个 server 的 `publishDiagnostics` 会整体替换它在某个 path 下的集合；
/// 空数组只清除该 server，只有所有 server 都清空后才删除 path。可直接作为
/// [`DiagnosticSink`] 传给 [`LspManager::start_all`](super::LspManager::start_all)。
#[derive(Default)]
pub struct DiagnosticStore {
    inner: Mutex<BTreeMap<String, BTreeMap<String, Vec<Diagnostic>>>>,
}

impl DiagnosticStore {
    /// 合并所有 server 在 `path` 下的诊断；无记录时返回空数组。
    pub fn for_path(&self, path: &str) -> Vec<Diagnostic> {
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .get(path)
            .map(|servers| {
                servers
                    .values()
                    .flat_map(|diagnostics| diagnostics.iter().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 某个 server 在 `path` 下的诊断；无记录时返回空数组。
    pub fn for_server(&self, server_id: &str, path: &str) -> Vec<Diagnostic> {
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .get(path)
            .and_then(|servers| servers.get(server_id))
            .cloned()
            .unwrap_or_default()
    }

    /// 当前全部 path 的合并快照（按 path 排序）。
    pub fn snapshot(&self) -> BTreeMap<String, Vec<Diagnostic>> {
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .iter()
            .map(|(path, servers)| {
                let merged = servers
                    .values()
                    .flat_map(|diagnostics| diagnostics.iter().cloned())
                    .collect();
                (path.clone(), merged)
            })
            .collect()
    }

    /// 清空所有缓存。
    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        guard.clear();
    }
}

impl DiagnosticSink for DiagnosticStore {
    fn publish(&self, server_id: &str, path: &str, diagnostics: &[Diagnostic]) {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if diagnostics.is_empty() {
            if let Some(servers) = guard.get_mut(path) {
                servers.remove(server_id);
                if servers.is_empty() {
                    guard.remove(path);
                }
            }
            return;
        }
        guard
            .entry(path.to_string())
            .or_default()
            .insert(server_id.to_string(), diagnostics.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(message: &str) -> Diagnostic {
        Diagnostic {
            severity: 1,
            message: message.to_string(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 2,
        }
    }

    #[test]
    fn diagnostic_serializes_camel_case_end_positions() {
        let value = serde_json::to_value(diagnostic("boom")).expect("serialize diagnostic");
        assert_eq!(value["endLine"], 1);
        assert_eq!(value["endColumn"], 2);
        assert!(value.get("end_line").is_none());
    }

    #[test]
    fn store_merges_servers_and_clears_one_at_a_time() {
        let store = DiagnosticStore::default();
        store.publish("a", "/tmp/x.rs", &[diagnostic("from-a")]);
        store.publish("b", "/tmp/x.rs", &[diagnostic("from-b")]);

        assert_eq!(store.for_path("/tmp/x.rs").len(), 2);
        assert_eq!(store.for_server("a", "/tmp/x.rs")[0].message, "from-a");

        store.publish("a", "/tmp/x.rs", &[]);
        let remaining = store.for_path("/tmp/x.rs");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].message, "from-b");

        store.publish("b", "/tmp/x.rs", &[]);
        assert!(store.for_path("/tmp/x.rs").is_empty());
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn closure_can_be_used_as_sink() {
        let seen = Mutex::new(Vec::new());
        let sink = |server: &str, path: &str, diagnostics: &[Diagnostic]| {
            seen.lock().expect("seen").push((
                server.to_string(),
                path.to_string(),
                diagnostics.len(),
            ));
        };
        sink.publish("fake", "/tmp/x", &[diagnostic("x")]);
        assert_eq!(
            seen.lock().expect("seen").as_slice(),
            &[("fake".to_string(), "/tmp/x".to_string(), 1)]
        );
    }
}
