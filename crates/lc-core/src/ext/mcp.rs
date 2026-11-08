//! 插件声明的 MCP server 合并（docs/EXTENSIONS.md §6）。
//!
//! 只读取**启用且清单有效**的插件；名称统一加前缀
//! `plugin.<pluginId>.<name>`，避免与 `~/.lite-code/mcp.json` / `.mcp.json`
//! 的用户配置冲突。只支持 stdio `command` 型条目，HTTP/SSE 显式跳过。

use std::collections::HashSet;
use std::path::Path;

use crate::mcp::McpServerConfig;

use super::{ExtPaths, enabled_plugins, project_allowed};

/// 合并当前工作区下启用插件的 MCP 声明。任何配置错误都降级为空列表——
/// MCP 不可用不应阻断主流程。
pub fn plugin_mcp_servers(root: &Path) -> Vec<McpServerConfig> {
    let Ok(paths) = ExtPaths::default_for(Some(root)) else {
        return Vec::new();
    };
    plugin_mcp_servers_at(&paths, project_allowed())
}

pub(crate) fn plugin_mcp_servers_at(paths: &ExtPaths, allow_project: bool) -> Vec<McpServerConfig> {
    let Ok(plugins) = enabled_plugins(paths, allow_project) else {
        return Vec::new();
    };

    let mut servers = Vec::new();
    let mut names = HashSet::new();
    for plugin in plugins {
        if !plugin.info.enabled {
            continue;
        }
        let Some(manifest) = plugin.manifest else {
            continue;
        };
        for (name, declared) in &manifest.contributes.mcp_servers {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let kind = declared
                .kind
                .as_deref()
                .unwrap_or("stdio")
                .to_ascii_lowercase();
            if matches!(kind.as_str(), "http" | "sse")
                || (declared.command.is_none() && declared.url.is_some())
            {
                continue;
            }
            let Some(command) = declared
                .command
                .as_deref()
                .map(str::trim)
                .filter(|command| !command.is_empty())
            else {
                continue;
            };
            let qualified = format!("plugin.{}.{}", plugin.info.id, name);
            if !names.insert(qualified.clone()) {
                continue;
            }
            servers.push(McpServerConfig {
                name: qualified,
                command: command.to_string(),
                args: declared.args.clone(),
                env: declared.env.clone(),
                enabled: declared.enabled,
            });
        }
    }
    servers
}
