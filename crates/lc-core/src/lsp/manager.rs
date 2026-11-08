//! 多 server 生命周期与文档路由。
//!
//! 每个 server 独立 `LspClient`；启动失败只记录在 [`LspStartReport::errors`]，
//! 不影响其它 server。`didOpen` 按 `languageIds` 路由，`didChange` / `didClose`
//! 只发给真正打开过该路径的 server。单次发送失败会在本轮结束后移除该 server，
//! 其余 server 继续服务。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::LspError;
use super::client::LspClient;
use super::config::{LspServerConfig, LspServerInfo};
use super::diagnostic::DiagnosticSink;

/// 单个 server 启动/运行失败。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspStartFailure {
    pub id: String,
    pub message: String,
}

/// `start_all` 结果：成功启动的 server 与隔离后的错误。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspStartReport {
    pub servers: Vec<LspServerInfo>,
    pub errors: Vec<LspStartFailure>,
}

impl LspStartReport {
    /// 所有配置都成功启动。
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// 按 server id 管理一组 stdio LSP server。
#[derive(Default)]
pub struct LspManager {
    clients: BTreeMap<String, LspClient>,
    configs: BTreeMap<String, LspServerConfig>,
}

impl LspManager {
    /// 停止现有 server 后按 `configs` 全量启动。
    ///
    /// 同一个 id 重复出现时后者被拒绝；空 id / 空 command / spawn 失败 /
    /// `initialize` 超时都只写入 `errors`，不会短路后续 server。
    pub fn start_all(
        &mut self,
        configs: Vec<LspServerConfig>,
        workspace_root: &Path,
        sink: Arc<dyn DiagnosticSink>,
    ) -> LspStartReport {
        self.stop_all();

        let mut report = LspStartReport::default();
        let mut seen = BTreeSet::new();
        for config in configs {
            if config.id.trim().is_empty() {
                report.errors.push(LspStartFailure {
                    id: String::new(),
                    message: "server id 为空".into(),
                });
                continue;
            }
            if !seen.insert(config.id.clone()) {
                report.errors.push(LspStartFailure {
                    id: config.id.clone(),
                    message: "重复的 server id".into(),
                });
                continue;
            }

            let info = LspServerInfo {
                id: config.id.clone(),
                language_ids: config.language_ids.clone(),
            };
            match LspClient::spawn(&config, workspace_root, Arc::clone(&sink)) {
                Ok(mut client) => match client.initialize() {
                    Ok(()) => {
                        report.servers.push(info);
                        self.clients.insert(config.id.clone(), client);
                        self.configs.insert(config.id.clone(), config);
                    }
                    Err(error) => {
                        client.stop();
                        report.errors.push(LspStartFailure {
                            id: config.id.clone(),
                            message: error.to_string(),
                        });
                    }
                },
                Err(error) => report.errors.push(LspStartFailure {
                    id: config.id.clone(),
                    message: error.to_string(),
                }),
            }
        }
        report
    }

    /// 停止并回收全部 server；幂等。
    pub fn stop_all(&mut self) {
        let clients = std::mem::take(&mut self.clients);
        self.configs.clear();
        for (_, mut client) in clients {
            client.stop();
        }
    }

    /// 按 language id 打开文档；返回实际发送的 server 数（0 表示无匹配）。
    ///
    /// 若多个 server 匹配，单个失败只记录并移除该 server；全部失败才返回错误。
    pub fn did_open(
        &mut self,
        path: &str,
        language_id: &str,
        text: &str,
    ) -> Result<usize, LspError> {
        let ids: Vec<String> = self
            .configs
            .iter()
            .filter(|(id, config)| {
                self.clients.contains_key(*id)
                    && config
                        .language_ids
                        .iter()
                        .any(|candidate| candidate == language_id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        self.dispatch("didOpen", ids, |client| {
            client.did_open(path, language_id, text)
        })
    }

    /// 把全量变更发给所有打开过该路径的 server。
    pub fn did_change(&mut self, path: &str, text: &str) -> Result<usize, LspError> {
        let ids = self.open_servers(path);
        self.dispatch("didChange", ids, |client| client.did_change(path, text))
    }

    /// 通知所有打开过该路径的 server 关闭文档。
    pub fn did_close(&mut self, path: &str) -> Result<usize, LspError> {
        let ids = self.open_servers(path);
        self.dispatch("didClose", ids, |client| client.did_close(path))
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.clients.get(id).is_some_and(LspClient::is_running)
    }

    /// 当前仍在运行的 server 摘要。
    pub fn running_servers(&self) -> Vec<LspServerInfo> {
        self.clients
            .keys()
            .filter_map(|id| self.configs.get(id))
            .map(|config| LspServerInfo {
                id: config.id.clone(),
                language_ids: config.language_ids.clone(),
            })
            .collect()
    }

    fn open_servers(&self, path: &str) -> Vec<String> {
        self.clients
            .iter()
            .filter(|(_, client)| client.is_open(path))
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn dispatch(
        &mut self,
        cause: &str,
        ids: Vec<String>,
        mut action: impl FnMut(&mut LspClient) -> Result<(), LspError>,
    ) -> Result<usize, LspError> {
        let mut delivered = 0;
        let mut first_error = None;
        let mut failed = Vec::new();
        for id in ids {
            let Some(client) = self.clients.get_mut(&id) else {
                continue;
            };
            match action(client) {
                Ok(()) => delivered += 1,
                Err(error) => {
                    eprintln!("lsp: {cause} 发送失败（{id}）：{error}");
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    failed.push(id);
                }
            }
        }

        for id in failed {
            self.clients.remove(&id);
            self.configs.remove(&id);
        }

        if delivered == 0
            && let Some(error) = first_error
        {
            return Err(error);
        }
        Ok(delivered)
    }
}
