//! 插件语言服务器接线：把启用插件的 `contributes.languageServers` 转成
//! `LspServerConfig`，交给 LSP lane 启动。
//!
//! 这是薄壳层：插件发现/校验在 lc-core ext lane，LSP 生命周期在 lc-core lsp lane；
//! 这里只做 DTO 转译，避免两个 lane 互相依赖。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lc_core::lsp::{LspServerConfig, LspStartReport};
use lc_proto::{ErrorBody, ErrorCode};
use serde::Deserialize;

use crate::{WorkspaceState, lsp};

#[derive(Debug, Deserialize)]
struct PluginManifest {
    #[serde(default)]
    contributes: Option<Contributes>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contributes {
    #[serde(default)]
    language_servers: Vec<LanguageServerDecl>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LanguageServerDecl {
    id: String,
    #[serde(default)]
    language_ids: Vec<String>,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// 读取启用插件的语言服务器声明。单个清单损坏只跳过该插件。
pub(crate) fn plugin_lsp_configs(root: Option<&Path>) -> Result<Vec<LspServerConfig>, ErrorBody> {
    let list = lc_core::ext::list_plugins(root).map_err(|error| error.to_error_body())?;
    let mut configs = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for plugin in list.plugins {
        if !plugin.enabled || plugin.error.is_some() {
            continue;
        }
        let manifest_path = PathBuf::from(&plugin.path).join("plugin.json");
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<PluginManifest>(&text) else {
            continue;
        };
        let Some(contributes) = manifest.contributes else {
            continue;
        };
        for server in contributes.language_servers {
            if server.id.trim().is_empty() || server.command.trim().is_empty() {
                continue;
            }
            let id = format!("plugin.{}.{}", plugin.id, server.id);
            if !seen.insert(id.clone()) {
                continue;
            }
            configs.push(LspServerConfig {
                id,
                language_ids: server.language_ids,
                command: resolve_command(&plugin.path, &server.command),
                args: server.args,
                env: server.env,
            });
        }
    }
    Ok(configs)
}

/// 裸命令交给 PATH；显式相对路径（含分隔符）相对插件目录解析。
fn resolve_command(plugin_dir: &str, command: &str) -> String {
    if command.contains('/') || command.contains('\\') {
        let path = Path::new(command);
        if path.is_relative() {
            return Path::new(plugin_dir)
                .join(path)
                .to_string_lossy()
                .into_owned();
        }
    }
    command.to_string()
}

/// 启动全部插件语言服务器；配置来自当前工作区的启用插件。
#[tauri::command]
pub(crate) async fn lsp_start_plugins(
    app: tauri::AppHandle,
    lsp_state: tauri::State<'_, lsp::LspState>,
    workspace: tauri::State<'_, WorkspaceState>,
) -> Result<LspStartReport, ErrorBody> {
    let root = workspace
        .get()
        .ok_or_else(|| ErrorBody::new(ErrorCode::InvalidArgument, "请先打开工作区"))?;
    let configs = plugin_lsp_configs(Some(&root))?;
    lsp_state.configure(configs)?;
    lsp::lsp_start_all(None, app, lsp_state, workspace).await
}

#[cfg(test)]
mod tests {
    use super::resolve_command;

    #[test]
    fn resolves_relative_commands_against_plugin_dir() {
        let dir = r"D:\plugins\example";
        assert_eq!(
            resolve_command(dir, "bin/server.exe"),
            std::path::Path::new(dir)
                .join("bin/server.exe")
                .to_string_lossy()
        );
        assert_eq!(
            resolve_command(dir, "typescript-language-server"),
            "typescript-language-server"
        );
    }
}
