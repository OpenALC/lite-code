//! 插件清单发现与校验（docs/EXTENSIONS.md §1）。
//!
//! 发现顺序：用户目录在前、项目目录在后；同一 `id` 只允许第一个有效条目，
//! 后续条目以 `error` 形式保留在列表里，便于 UI 暴露问题但不影响其它插件。
//! 单个目录的清单错误不会让整个列表失败。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::fs::{FsError, RealPathResolver, display_path, ensure_within_root};

use super::{
    ExtError, ExtPaths, PluginCommand, PluginInfo, PluginLanguage, PluginRuntime, PluginTheme,
    PluginUiView, PluginView,
};

pub(crate) const MANIFEST_FILE: &str = "plugin.json";
pub(crate) const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// 主题文件大小上限（docs/EXTENSIONS.md §1）：读取时由 `ext_read_theme` 校验。
pub(crate) const MAX_THEME_BYTES: u64 = 64 * 1024;
/// 语法文件大小上限（docs/EXTENSIONS.md §1）：读取时由 `ext_read_grammar` 校验。
pub(crate) const MAX_GRAMMAR_BYTES: u64 = 64 * 1024;
/// UI 视图节数上限（docs/plans/plugin-webview.md §3.1）。
pub(crate) const MAX_UI_SECTIONS: usize = 16;
/// form 节字段数上限（docs/plans/plugin-webview.md §3.1）。
pub(crate) const MAX_UI_FIELDS: usize = 32;
/// list 节列数上限（docs/plans/plugin-webview.md §3.1）。
pub(crate) const MAX_UI_COLUMNS: usize = 8;
/// markdown 节文本上限（docs/plans/plugin-webview.md §3.1）。
pub(crate) const MAX_UI_TEXT_BYTES: usize = 16 * 1024;
/// kv 节静态/动态条目上限（docs/plans/plugin-webview.md §3.2）。
pub(crate) const MAX_UI_ITEMS: usize = 64;
/// select 字段 options 上限（docs/plans/plugin-webview.md §3.1）。
pub(crate) const MAX_UI_OPTIONS: usize = 32;
/// 整棵 `contributes.ui` 块序列化上限（docs/plans/plugin-webview.md §3.1）。
pub(crate) const MAX_UI_BLOCK_BYTES: usize = 32 * 1024;

/// 一个已发现条目。`manifest` 为 `None` 表示该目录的清单无效或 id 冲突，
/// 只用于列表展示，不能执行命令或参与 MCP 合并。
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredPlugin {
    pub info: PluginInfo,
    pub manifest: Option<PluginManifest>,
    pub dir: PathBuf,
    pub main_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PluginManifest {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub runtime: PluginRuntime,
    #[serde(default)]
    pub main: Option<String>,
    /// 激活事件（docs/EXTENSIONS.md §4）：空数组或含 `*` 立即激活；
    /// 仅 `onCommand:<commandId>` 时延迟到首次命令/工具调用。
    #[serde(default, rename = "activationEvents")]
    pub activation_events: Vec<String>,
    /// 依赖的其它插件 id（docs/EXTENSIONS.md §1）：宿主保证依赖先于
    /// 本插件加载。条目格式与插件 id 同规则（小写字母/数字/点/线）。
    #[serde(default, rename = "extensionDependencies")]
    pub extension_dependencies: Vec<String>,
    #[serde(default)]
    pub contributes: Contributes,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Contributes {
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerDecl>,
    #[serde(default)]
    pub language_servers: Vec<LanguageServerDecl>,
    /// Agent 可见的插件工具（docs/EXTENSIONS.md §1/§4）。
    #[serde(default)]
    pub tools: Vec<PluginToolDecl>,
    /// 配色主题声明（docs/EXTENSIONS.md §1）：UI 经 `ext_read_theme` 拉取 JSON。
    #[serde(default)]
    pub themes: Vec<ThemeDecl>,
    /// 语言声明（docs/EXTENSIONS.md §1）：UI 注册 Monaco 语言并拉取语法 JSON。
    #[serde(default)]
    pub languages: Vec<LanguageDecl>,
    /// 树视图声明（docs/EXTENSIONS.md §1）：UI 在资源管理器侧栏渲染可折叠
    /// 分区，节点数据由插件 `ctx.views.register` 提供（§4）。
    #[serde(default)]
    pub views: Vec<ViewDecl>,
    /// UI 视图声明（docs/plans/plugin-webview.md §3.1）：编辑器区标签页的
    /// 声明式节集合（markdown/form/list/kv 闭集），静态节 M1 渲染，动态
    /// 数据走 ui/values（M2）。
    #[serde(default)]
    pub ui: UiContrib,
}

/// UI 视图块（`contributes.ui`）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiContrib {
    #[serde(default)]
    pub views: Vec<UiViewDecl>,
}

/// UI 视图声明。`id` 插件内唯一（小写字母/数字/线，与 views.id 同规则）；
/// `name` 是标签页标题（非空）；`open_command` 可空，声明时必须出现在
/// contributes.commands（palette 别名拦截用，纯 web 打开本视图）。
/// `sections` 的原始 JSON 由 validate_ui 白名单校验后原样进 DTO。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiViewDecl {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub open_command: Option<String>,
    #[serde(default)]
    pub sections: Value,
}

/// 树视图声明。`id` 是插件内唯一的视图标识（小写字母/数字/线，与语言
/// id 同规则）；`name` 是侧栏分区的展示名（非空）。节点数据不在清单里，
/// 由插件 activate 时注册的 provider 提供（docs/EXTENSIONS.md §4）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ViewDecl {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// 语言声明。`id` 是 Monaco language id（小写字母/数字/线，插件内唯一）；
/// `extensions` 是带点的扩展名列表（插件间去重，冲突方在校验错误中说明）；
/// `grammar` 是相对插件目录的语法 JSON（校验时须已存在且在目录内，
/// 读取时另限 64 KiB），缺省表示不提供高亮。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LanguageDecl {
    pub id: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub grammar: Option<String>,
}

/// 主题声明。`path` 是相对插件目录的主题 JSON（校验时须已存在且在目录内，
/// 读取时另限 64 KiB）；`ui_theme` 校验后只可能是 `dark`/`light`。
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ThemeDecl {
    pub label: String,
    pub path: String,
    #[serde(rename = "uiTheme")]
    pub ui_theme: String,
}

/// 插件声明的 Agent 工具。`command` 映射到插件内注册的命令 handler，
/// 缺省与 `name` 一致；执行时优先派发到 `ctx.tools.register` 注册的
/// handler，未注册则回退到该命令 handler（docs/EXTENSIONS.md §4）。
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PluginToolDecl {
    pub name: String,
    pub description: String,
    /// JSON Schema（object 类型），原样进入 LLM 工具定义。
    pub parameters: Value,
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LanguageServerDecl {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct McpServerDecl {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

fn enabled_default() -> bool {
    true
}

pub(crate) fn discover(
    paths: &ExtPaths,
    allow_project: bool,
    disabled: &BTreeSet<String>,
) -> Result<Vec<DiscoveredPlugin>, ExtError> {
    let mut plugins = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut claimed_commands: HashMap<String, String> = HashMap::new();
    let mut claimed_extensions: HashMap<String, String> = HashMap::new();
    scan_root(
        &paths.user_dir,
        disabled,
        &mut plugins,
        &mut seen_ids,
        &mut claimed_commands,
        &mut claimed_extensions,
    )?;
    if allow_project
        && let Some(project_dir) = paths.project_dir.as_deref()
        && project_dir != paths.user_dir
    {
        scan_root(
            project_dir,
            disabled,
            &mut plugins,
            &mut seen_ids,
            &mut claimed_commands,
            &mut claimed_extensions,
        )?;
    }
    validate_dependencies(&mut plugins);
    Ok(plugins)
}

/// 依赖跨插件校验（docs/EXTENSIONS.md §1）：被依赖 id 必须存在于已发现
/// 插件中；依赖图存在环时环上插件整体标记 error。两者都需要完整插件
/// 列表，因此在 discover 收尾时统一处理（参考 claimed_extensions 的
/// 跨插件校验模式）。缺失依赖优先于环报错（一个插件只标一个错误）。
fn validate_dependencies(plugins: &mut [DiscoveredPlugin]) {
    let ids: HashSet<&str> = plugins
        .iter()
        .filter_map(|plugin| {
            plugin
                .manifest
                .as_ref()
                .map(|manifest| manifest.id.as_str())
        })
        .collect();

    let mut missing: HashMap<String, Vec<String>> = HashMap::new();
    for plugin in plugins.iter() {
        let Some(manifest) = plugin.manifest.as_ref() else {
            continue;
        };
        let absent: Vec<String> = manifest
            .extension_dependencies
            .iter()
            .filter(|dependency| !ids.contains(dependency.as_str()))
            .cloned()
            .collect();
        if !absent.is_empty() {
            missing.insert(manifest.id.clone(), absent);
        }
    }

    // 环检测只在有效清单的子图上进行（自环已被自依赖校验拒绝）。
    let graph: HashMap<String, Vec<String>> = plugins
        .iter()
        .filter_map(|plugin| plugin.manifest.as_ref())
        .map(|manifest| {
            (
                manifest.id.clone(),
                manifest
                    .extension_dependencies
                    .iter()
                    .filter(|dependency| ids.contains(dependency.as_str()))
                    .cloned()
                    .collect(),
            )
        })
        .collect();
    // member → 同环成员列表，错误信息里写明环的构成。
    let mut cycle_members: HashMap<String, Vec<String>> = HashMap::new();
    for mut group in dependency_cycles(&graph) {
        group.sort();
        for member in &group {
            cycle_members.insert(member.clone(), group.clone());
        }
    }

    for plugin in plugins.iter_mut() {
        let Some(manifest) = plugin.manifest.as_ref() else {
            continue;
        };
        if let Some(absent) = missing.get(&manifest.id) {
            let listed = absent
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join("、");
            mark_plugin_error(
                plugin,
                format!("extensionDependencies 引用的插件不存在：{listed}"),
            );
        } else if let Some(group) = cycle_members.get(&manifest.id) {
            mark_plugin_error(plugin, dependency_cycle_message(group));
        }
    }
}

/// 把已入列的插件标记为 error（跨插件校验后置用）：保留列表展示信息，
/// 清空 manifest/main，使其不参与命令执行、MCP 合并与宿主下发。
fn mark_plugin_error(plugin: &mut DiscoveredPlugin, message: String) {
    plugin.info.error = Some(message);
    plugin.info.enabled = false;
    plugin.manifest = None;
    plugin.main_path = None;
}

/// 依赖环报错文案（§1 语义）。marketplace 安装链的环检测复用同一格式
/// （docs/plans/plugin-marketplace.md §3.5 M2）。
pub(crate) fn dependency_cycle_message(group: &[String]) -> String {
    let listed = group
        .iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join("、");
    format!("extensionDependencies 存在循环依赖：{listed}")
}

/// Tarjan 强连通分量：返回依赖图中所有大小 > 1 的分量（即环），
/// 供环上插件整体报错并退出下发。marketplace 依赖解析（M2）复用。
pub(crate) fn dependency_cycles(graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    struct Search<'a> {
        graph: &'a HashMap<String, Vec<String>>,
        index_of: HashMap<&'a str, usize>,
        lowlink: HashMap<&'a str, usize>,
        on_stack: HashSet<&'a str>,
        stack: Vec<&'a str>,
        next: usize,
        cycles: Vec<Vec<String>>,
    }

    impl<'a> Search<'a> {
        fn strong_connect(&mut self, node: &'a str) {
            let graph = self.graph;
            self.index_of.insert(node, self.next);
            self.lowlink.insert(node, self.next);
            self.next += 1;
            self.stack.push(node);
            self.on_stack.insert(node);

            let edges: Vec<&'a str> = graph
                .get(node)
                .map(|dependencies| dependencies.iter().map(String::as_str).collect())
                .unwrap_or_default();
            for next_node in edges {
                if !self.index_of.contains_key(next_node) {
                    self.strong_connect(next_node);
                    let lowlink = self.lowlink[next_node].min(self.lowlink[node]);
                    self.lowlink.insert(node, lowlink);
                } else if self.on_stack.contains(next_node) {
                    let lowlink = self.index_of[next_node].min(self.lowlink[node]);
                    self.lowlink.insert(node, lowlink);
                }
            }

            if self.lowlink[node] == self.index_of[node] {
                let mut component = Vec::new();
                loop {
                    let member = self.stack.pop().expect("Tarjan 栈非空");
                    self.on_stack.remove(member);
                    component.push(member.to_string());
                    if member == node {
                        break;
                    }
                }
                if component.len() > 1 {
                    self.cycles.push(component);
                }
            }
        }
    }

    let mut search = Search {
        graph,
        index_of: HashMap::new(),
        lowlink: HashMap::new(),
        on_stack: HashSet::new(),
        stack: Vec::new(),
        next: 0,
        cycles: Vec::new(),
    };
    for start in graph.keys() {
        if !search.index_of.contains_key(start.as_str()) {
            search.strong_connect(start.as_str());
        }
    }
    search.cycles
}

fn scan_root(
    root: &Path,
    disabled: &BTreeSet<String>,
    plugins: &mut Vec<DiscoveredPlugin>,
    seen_ids: &mut HashSet<String>,
    claimed_commands: &mut HashMap<String, String>,
    claimed_extensions: &mut HashMap<String, String>,
) -> Result<(), ExtError> {
    let read_dir = match std::fs::read_dir(root) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ExtError::Io(format!(
                "读取插件目录 {} 失败：{error}",
                root.display()
            )));
        }
    };

    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for entry in read_dir.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest_path = dir.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            continue;
        }
        candidates.push((entry.file_name().to_string_lossy().into_owned(), dir));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    for (dir_name, raw_dir) in candidates {
        let dir = match std::fs::canonicalize(&raw_dir) {
            Ok(dir) => dir,
            Err(error) => {
                push_error(
                    plugins,
                    seen_ids,
                    None,
                    &dir_name,
                    &raw_dir,
                    format!("插件目录不可用：{error}"),
                );
                continue;
            }
        };

        let manifest_path = dir.join(MANIFEST_FILE);
        let raw_value = match read_manifest_value(&manifest_path) {
            Ok(value) => value,
            Err(message) => {
                push_error(plugins, seen_ids, None, &dir_name, &dir, message);
                continue;
            }
        };

        let parsed = match serde_json::from_value::<PluginManifest>(raw_value.clone()) {
            Ok(parsed) => parsed,
            Err(error) => {
                push_error(
                    plugins,
                    seen_ids,
                    Some(&raw_value),
                    &dir_name,
                    &dir,
                    format!("清单字段无效：{error}"),
                );
                continue;
            }
        };

        let (manifest, main_path) = match validate_manifest(parsed, &dir) {
            Ok(valid) => valid,
            Err(message) => {
                push_error(
                    plugins,
                    seen_ids,
                    Some(&raw_value),
                    &dir_name,
                    &dir,
                    message,
                );
                continue;
            }
        };

        let id = manifest.id.clone();
        if !seen_ids.insert(id.clone()) {
            push_error(
                plugins,
                seen_ids,
                Some(&raw_value),
                &dir_name,
                &dir,
                format!("插件 id `{id}` 重复；保留先发现的插件"),
            );
            continue;
        }

        if let Some((command, other)) = manifest.contributes.commands.iter().find_map(|command| {
            claimed_commands
                .get(&command.command)
                .map(|other| (command.command.clone(), other.clone()))
        }) {
            push_error(
                plugins,
                seen_ids,
                Some(&raw_value),
                &dir_name,
                &dir,
                format!("命令 id `{command}` 已由插件 `{other}` 声明"),
            );
            continue;
        }

        // 语言扩展名插件间去重（docs/EXTENSIONS.md §1）：先发现者占用，
        // 冲突扩展名连同语言 id、冲突方一起写进校验错误。
        if let Some((language_id, extension, other)) =
            manifest.contributes.languages.iter().find_map(|language| {
                language.extensions.iter().find_map(|extension| {
                    claimed_extensions
                        .get(extension)
                        .map(|other| (language.id.clone(), extension.clone(), other.clone()))
                })
            })
        {
            push_error(
                plugins,
                seen_ids,
                Some(&raw_value),
                &dir_name,
                &dir,
                format!("语言 `{language_id}` 的扩展名 `{extension}` 已由插件 `{other}` 声明"),
            );
            continue;
        }

        for command in &manifest.contributes.commands {
            claimed_commands.insert(command.command.clone(), id.clone());
        }
        for language in &manifest.contributes.languages {
            for extension in &language.extensions {
                claimed_extensions.insert(extension.clone(), id.clone());
            }
        }

        let enabled = !disabled.contains(&id);
        plugins.push(DiscoveredPlugin {
            info: info_from_manifest(&manifest, &dir, enabled),
            manifest: Some(manifest),
            dir,
            main_path,
        });
    }
    Ok(())
}

fn validate_manifest(
    mut manifest: PluginManifest,
    dir: &Path,
) -> Result<(PluginManifest, Option<PathBuf>), String> {
    manifest.id = manifest.id.trim().to_string();
    if !valid_plugin_id(&manifest.id) {
        return Err(format!("插件 id 无效：`{}`", manifest.id));
    }

    manifest.version = manifest.version.trim().to_string();
    if manifest.version.is_empty() {
        return Err("version 不能为空".into());
    }

    manifest.name = manifest.name.trim().to_string();
    if manifest.name.is_empty() {
        manifest.name = manifest.id.clone();
    }

    // 激活事件校验（docs/EXTENSIONS.md §4 / docs/plans/plugin-webview.md §3.2）：
    // v1 仅支持 `*`、`onCommand:<commandId>`、`onView:<viewId>` 与
    // `onUiView:<viewId>`；非法事件只把该插件标记为 error。
    let mut activation_events = Vec::new();
    for event in &manifest.activation_events {
        let event = event.trim();
        if event != "*"
            && !event.starts_with("onCommand:")
            && !event.starts_with("onView:")
            && !event.starts_with("onUiView:")
        {
            return Err(format!(
                "activationEvents 无效：`{event}`（仅支持 `*`、`onCommand:<commandId>`、`onView:<viewId>` 或 `onUiView:<viewId>`）"
            ));
        }
        if !activation_events.iter().any(|existing| existing == event) {
            activation_events.push(event.to_string());
        }
    }
    manifest.activation_events = activation_events;

    // 依赖声明校验（docs/EXTENSIONS.md §1）：条目必须是合法插件 id 形态
    // （小写字母/数字/点/线，与工具名字符集一致），trim 后去重；
    // 自依赖直接拒绝。被依赖插件是否存在于插件目录属跨插件校验，
    // 在 discover 收尾时统一检查（与环检测一起）。
    let mut seen_dependencies = HashSet::new();
    let mut extension_dependencies = Vec::new();
    for dependency in &manifest.extension_dependencies {
        let dependency = dependency.trim();
        if !valid_tool_name(dependency) {
            return Err(format!(
                "extensionDependencies 无效：`{dependency}`（仅允许小写字母、数字、点、线）"
            ));
        }
        if dependency == manifest.id {
            return Err(format!(
                "extensionDependencies 不能依赖自身：`{dependency}`"
            ));
        }
        if seen_dependencies.insert(dependency.to_string()) {
            extension_dependencies.push(dependency.to_string());
        }
    }
    manifest.extension_dependencies = extension_dependencies;

    let mut command_ids = HashSet::new();
    for command in &mut manifest.contributes.commands {
        command.command = command.command.trim().to_string();
        if !valid_command_id(&command.command) {
            return Err(format!("命令 id 无效：`{}`", command.command));
        }
        if !command_ids.insert(command.command.clone()) {
            return Err(format!("命令 id 重复：`{}`", command.command));
        }
        command.title = command.title.trim().to_string();
        if command.title.is_empty() {
            command.title = command.command.clone();
        }
    }

    let mut language_ids = HashSet::new();
    for server in &mut manifest.contributes.language_servers {
        server.id = server.id.trim().to_string();
        if server.id.is_empty() {
            return Err("languageServers.id 不能为空".into());
        }
        if !language_ids.insert(server.id.clone()) {
            return Err(format!("language server id 重复：`{}`", server.id));
        }
    }

    // Agent 工具校验（docs/EXTENSIONS.md §1）：名字合法且插件内唯一、
    // 描述非空、parameters 是 object 类型 JSON Schema；command 必须出现在
    // contributes.commands 中，或与 name 一致（缺省即 name）。
    let command_ids: HashSet<&str> = manifest
        .contributes
        .commands
        .iter()
        .map(|command| command.command.as_str())
        .collect();
    let mut tool_names = HashSet::new();
    for tool in &mut manifest.contributes.tools {
        tool.name = tool.name.trim().to_string();
        if !valid_tool_name(&tool.name) {
            return Err(format!(
                "工具名无效：`{}`（仅允许小写字母、数字、点、线）",
                tool.name
            ));
        }
        if !tool_names.insert(tool.name.clone()) {
            return Err(format!("工具名重复：`{}`", tool.name));
        }
        tool.description = tool.description.trim().to_string();
        if tool.description.is_empty() {
            return Err(format!("工具 `{}` 缺少描述", tool.name));
        }
        if !valid_tool_parameters(&tool.parameters) {
            return Err(format!(
                "工具 `{}` 的 parameters 必须是 object 类型 JSON Schema",
                tool.name
            ));
        }
        let command = tool
            .command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty());
        let command = match command {
            Some(command) => command.to_string(),
            None => tool.name.clone(),
        };
        if command != tool.name && !command_ids.contains(command.as_str()) {
            return Err(format!(
                "工具 `{}` 映射的命令 `{command}` 未在 contributes.commands 中声明",
                tool.name
            ));
        }
        tool.command = Some(command);
    }

    // 主题声明校验（docs/EXTENSIONS.md §1）：label 非空且插件内唯一、
    // uiTheme 只能是 dark/light、path 必须是插件目录内已存在的 .json
    // （复用 main 的 canonicalize 模式，防 `..`/软链逃逸）；大小在读取时另校。
    let mut theme_labels = HashSet::new();
    for theme in &mut manifest.contributes.themes {
        theme.label = theme.label.trim().to_string();
        if theme.label.is_empty() {
            return Err("themes.label 不能为空".into());
        }
        if !theme_labels.insert(theme.label.clone()) {
            return Err(format!("themes.label 重复：`{}`", theme.label));
        }
        let ui_theme = theme.ui_theme.trim().to_string();
        if ui_theme != "dark" && ui_theme != "light" {
            return Err(format!(
                "themes.uiTheme 非法：`{}`（仅支持 dark 或 light）",
                theme.ui_theme
            ));
        }
        theme.ui_theme = ui_theme;
        let raw_path = theme.path.trim();
        if !raw_path.ends_with(".json") {
            return Err(format!("themes.path 必须是 .json 文件：`{raw_path}`"));
        }
        let candidate = dir.join(raw_path);
        let resolved =
            ensure_within_root(dir, &candidate, &RealPathResolver).map_err(
                |error| match error {
                    FsError::NotFound { .. } => format!("themes.path 不存在：`{raw_path}`"),
                    FsError::PathOutsideRoot { .. } => {
                        format!("themes.path 必须落在插件目录内：`{raw_path}`")
                    }
                    other => format!("themes.path 校验失败：{other}"),
                },
            )?;
        if !resolved.is_file() {
            return Err(format!("themes.path 不是文件：`{raw_path}`"));
        }
        theme.path = raw_path.to_string();
    }

    // 语言声明校验（docs/EXTENSIONS.md §1）：id 合法（小写字母/数字/线）
    // 且插件内唯一；extensions 是非空 `.ext` 列表（插件内去重，统一转小写）；
    // grammar 必须是插件目录内已存在的 .json（复用 themes.path 的模式）。
    let mut language_ids = HashSet::new();
    for language in &mut manifest.contributes.languages {
        language.id = language.id.trim().to_string();
        if !valid_language_id(&language.id) {
            return Err(format!(
                "languages.id 无效：`{}`（仅允许小写字母、数字、线）",
                language.id
            ));
        }
        if !language_ids.insert(language.id.clone()) {
            return Err(format!("languages.id 重复：`{}`", language.id));
        }
        let mut seen_extensions = HashSet::new();
        let mut extensions = Vec::new();
        for extension in &language.extensions {
            let extension = extension.trim().to_lowercase();
            if !valid_language_extension(&extension) {
                return Err(format!(
                    "languages.extensions 非法：`{extension}`（必须是 `.ext` 形式）"
                ));
            }
            if seen_extensions.insert(extension.clone()) {
                extensions.push(extension);
            }
        }
        language.extensions = extensions;
        let mut seen_aliases = HashSet::new();
        language.aliases = language
            .aliases
            .iter()
            .map(|alias| alias.trim().to_string())
            .filter(|alias| !alias.is_empty())
            .filter(|alias| seen_aliases.insert(alias.clone()))
            .collect();
        let grammar = language
            .grammar
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty());
        let grammar = match grammar {
            Some(raw_path) => {
                if !raw_path.ends_with(".json") {
                    return Err(format!("languages.grammar 必须是 .json 文件：`{raw_path}`"));
                }
                let candidate = dir.join(raw_path);
                let resolved =
                    ensure_within_root(dir, &candidate, &RealPathResolver).map_err(|error| {
                        match error {
                            FsError::NotFound { .. } => {
                                format!("languages.grammar 不存在：`{raw_path}`")
                            }
                            FsError::PathOutsideRoot { .. } => {
                                format!("languages.grammar 必须落在插件目录内：`{raw_path}`")
                            }
                            other => format!("languages.grammar 校验失败：{other}"),
                        }
                    })?;
                if !resolved.is_file() {
                    return Err(format!("languages.grammar 不是文件：`{raw_path}`"));
                }
                Some(raw_path.to_string())
            }
            None => None,
        };
        language.grammar = grammar;
    }

    // 树视图声明校验（docs/EXTENSIONS.md §1）：id 合法（小写字母/数字/线，
    // 与语言 id 同规则）且插件内唯一；name 非空（trim 后）。
    let mut view_ids = HashSet::new();
    for view in &mut manifest.contributes.views {
        view.id = view.id.trim().to_string();
        if !valid_language_id(&view.id) {
            return Err(format!(
                "views.id 无效：`{}`（仅允许小写字母、数字、线）",
                view.id
            ));
        }
        if !view_ids.insert(view.id.clone()) {
            return Err(format!("views.id 重复：`{}`", view.id));
        }
        view.name = view.name.trim().to_string();
        if view.name.is_empty() {
            return Err(format!("views.name 不能为空：`{}`", view.id));
        }
    }

    // UI 视图声明校验（docs/plans/plugin-webview.md §3.1）：节 kind 闭集、
    // field type 闭集、命令闭包、规模上限；失败只把该插件标记为 error。
    validate_ui(&mut manifest.contributes.ui, &command_ids)?;

    let mut mcp_names = HashSet::new();
    let mut normalized_mcp = BTreeMap::new();
    for (name, declared) in std::mem::take(&mut manifest.contributes.mcp_servers) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("mcpServers 名称不能为空".into());
        }
        if !mcp_names.insert(name.clone()) {
            return Err(format!("mcpServers 名称重复：`{name}`"));
        }
        normalized_mcp.insert(name, declared);
    }
    manifest.contributes.mcp_servers = normalized_mcp;

    let main = manifest
        .main
        .as_deref()
        .map(str::trim)
        .filter(|main| !main.is_empty());
    let main_path = match main {
        Some(main) => {
            let candidate = dir.join(main);
            let resolved = ensure_within_root(dir, &candidate, &RealPathResolver).map_err(
                |error| match error {
                    FsError::NotFound { .. } => format!("main 不存在：`{main}`"),
                    FsError::PathOutsideRoot { .. } => {
                        format!("main 必须落在插件目录内：`{main}`")
                    }
                    other => format!("main 校验失败：{other}"),
                },
            )?;
            if !resolved.is_file() {
                return Err(format!("main 不是文件：`{main}`"));
            }
            Some(resolved)
        }
        None if manifest.runtime != PluginRuntime::Declarative => {
            return Err(format!(
                "runtime `{}` 需要 main",
                runtime_name(manifest.runtime)
            ));
        }
        None => None,
    };

    Ok((manifest, main_path))
}

/// UI 视图声明校验（docs/plans/plugin-webview.md §3.1）：视图 id 合法
/// （小写字母/数字/线，与 views.id 同规则）且插件内唯一、name 非空；
/// openCommand 必须出现在 contributes.commands；节 kind 闭集
/// （markdown|form|list|kv），field type 闭集（text|number|boolean|select|
/// password）且 default 类型合法；command/valuesCommand/rowCommand 一律
/// 闭包校验（tools.command 模式）；规模上限 sections≤16、fields≤32、
/// columns≤8、kv items≤64、markdown text≤16 KiB，整棵 ui 块序列化 ≤32 KiB。
/// 节在校验时按 kind 重建为白名单键集合，多余键丢弃；任一失败只把该插件
/// 标记为 error（既有 validate_manifest 全局语义）。
fn validate_ui(ui: &mut UiContrib, command_ids: &HashSet<&str>) -> Result<(), String> {
    // 整棵 ui 块序列化上限：校验形状前先兜体积，避免超大块浪费校验开销。
    let block = serde_json::to_string(&*ui).map_err(|error| format!("ui 块序列化失败：{error}"))?;
    if block.len() > MAX_UI_BLOCK_BYTES {
        return Err(format!(
            "contributes.ui 块超过 {} KiB 上限",
            MAX_UI_BLOCK_BYTES / 1024
        ));
    }

    let mut view_ids = HashSet::new();
    for view in &mut ui.views {
        view.id = view.id.trim().to_string();
        if !valid_language_id(&view.id) {
            return Err(format!(
                "ui.views.id 无效：`{}`（仅允许小写字母、数字、线）",
                view.id
            ));
        }
        if !view_ids.insert(view.id.clone()) {
            return Err(format!("ui.views.id 重复：`{}`", view.id));
        }
        view.name = view.name.trim().to_string();
        if view.name.is_empty() {
            return Err(format!("ui.views.name 不能为空：`{}`", view.id));
        }
        let open_command = view
            .open_command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty());
        match open_command {
            Some(command) => {
                if !command_ids.contains(command) {
                    return Err(format!(
                        "ui.views.openCommand 引用的命令 `{command}` 未在 contributes.commands 中声明"
                    ));
                }
                view.open_command = Some(command.to_string());
            }
            None => view.open_command = None,
        }
        validate_ui_sections(&view.id, &mut view.sections, command_ids)?;
    }
    Ok(())
}

/// 视图节数组校验：闭集 kind 分派 + 按 kind 白名单重建。节序号（1 起）进
/// 错误文案，方便插件作者定位。
fn validate_ui_sections(
    view_id: &str,
    sections: &mut Value,
    command_ids: &HashSet<&str>,
) -> Result<(), String> {
    let Some(list) = sections.as_array_mut() else {
        return Err(format!("ui.views `{view_id}` 的 sections 必须是数组"));
    };
    if list.len() > MAX_UI_SECTIONS {
        return Err(format!(
            "ui.views `{view_id}` 的 sections 超过 {MAX_UI_SECTIONS} 个上限"
        ));
    }
    for (index, section) in list.iter_mut().enumerate() {
        let label = format!("ui.views `{view_id}` 第 {} 节", index + 1);
        let Some(kind) = section.get("kind").and_then(Value::as_str) else {
            return Err(format!("{label} 缺少 kind"));
        };
        let kind = kind.trim();
        let normalized = match kind {
            "markdown" => validate_ui_markdown(&label, section)?,
            "form" => validate_ui_form(&label, section, command_ids)?,
            "list" => validate_ui_list(&label, section, command_ids)?,
            "kv" => validate_ui_kv(&label, section, command_ids)?,
            other => {
                return Err(format!(
                    "{label} kind 非法：`{other}`（仅支持 markdown/form/list/kv）"
                ));
            }
        };
        *section = normalized;
    }
    Ok(())
}

/// 取可空字符串字段：trim 后为空视为缺省（调用方丢弃该键）。
fn optional_string(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 命令引用闭包校验（tools.command 模式）：trim 后必须出现在
/// contributes.commands；空串视为缺省。
fn optional_command(
    map: &serde_json::Map<String, Value>,
    key: &str,
    label: &str,
    command_ids: &HashSet<&str>,
) -> Result<Option<String>, String> {
    let command = map.get(key).and_then(Value::as_str).map(str::trim);
    match command {
        Some("") | None => Ok(None),
        Some(command) => {
            if !command_ids.contains(command) {
                return Err(format!(
                    "{label} 的 `{key}` 引用的命令 `{command}` 未在 contributes.commands 中声明"
                ));
            }
            Ok(Some(command.to_string()))
        }
    }
}

/// markdown 节：text 必须是字符串且 ≤16 KiB；无回读（§3.2），纯静态渲染。
fn validate_ui_markdown(label: &str, section: &Value) -> Result<Value, String> {
    let Some(map) = section.as_object() else {
        return Err(format!("{label} 必须是对象"));
    };
    let text = map
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label}（markdown）缺少 text"))?;
    if text.len() > MAX_UI_TEXT_BYTES {
        return Err(format!(
            "{label}（markdown）的 text 超过 {} KiB 上限",
            MAX_UI_TEXT_BYTES / 1024
        ));
    }
    Ok(json!({ "kind": "markdown", "text": text }))
}

/// form 节：fields 1..=32 个；field key/label 非空、key 节内唯一；type 闭集；
/// default 必须是该类型的合法 JSON 值；select 必须有 1..=32 个非空字符串
/// options；number 的 min/max 必须是数字；command 闭包校验。
fn validate_ui_form(
    label: &str,
    section: &Value,
    command_ids: &HashSet<&str>,
) -> Result<Value, String> {
    let Some(map) = section.as_object() else {
        return Err(format!("{label} 必须是对象"));
    };
    let title = optional_string(map, "title");
    let command = optional_command(map, "command", label, command_ids)?;
    let fields = map
        .get("fields")
        .and_then(Value::as_array)
        .filter(|fields| !fields.is_empty())
        .ok_or_else(|| format!("{label}（form）的 fields 必须是非空数组"))?;
    if fields.len() > MAX_UI_FIELDS {
        return Err(format!(
            "{label}（form）的 fields 超过 {MAX_UI_FIELDS} 个上限"
        ));
    }

    let mut field_keys = HashSet::new();
    let mut normalized_fields = Vec::with_capacity(fields.len());
    for field in fields {
        let field_label = format!("{label}（form）字段");
        let Some(field_map) = field.as_object() else {
            return Err(format!("{field_label}必须是对象"));
        };
        let key = field_map
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| format!("{field_label}缺少 key"))?;
        if !field_keys.insert(key.to_string()) {
            return Err(format!("{field_label}`{key}` 重复"));
        }
        let field_label_name = field_map
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("{field_label}`{key}` 缺少 label"))?;
        let field_type = field_map
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .ok_or_else(|| format!("{field_label}`{key}` 缺少 type"))?;
        match field_type {
            "text" | "number" | "boolean" | "select" | "password" => {}
            other => {
                return Err(format!(
                    "{field_label}`{key}` 的 type 非法：`{other}`（仅支持 text/number/boolean/select/password）"
                ));
            }
        }

        let mut normalized = serde_json::Map::new();
        normalized.insert("key".into(), json!(key));
        normalized.insert("label".into(), json!(field_label_name));
        normalized.insert("type".into(), json!(field_type));
        if let Some(placeholder) = optional_string(field_map, "placeholder") {
            normalized.insert("placeholder".into(), json!(placeholder));
        }
        // default 必须是该 field type 的合法 JSON 值（§3.1）。
        if let Some(default) = field_map.get("default") {
            let valid = match field_type {
                "text" | "password" | "select" => default.is_string(),
                "number" => default.is_number(),
                "boolean" => default.is_boolean(),
                _ => unreachable!("field type 闭集已校验"),
            };
            if !valid {
                return Err(format!(
                    "{field_label}`{key}` 的 default 必须是 {field_type} 类型的 JSON 值"
                ));
            }
            normalized.insert("default".into(), default.clone());
        }
        if field_type == "select" {
            let options = field_map
                .get("options")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("{field_label}`{key}`（select）缺少 options"))?;
            if options.is_empty() || options.len() > MAX_UI_OPTIONS {
                return Err(format!(
                    "{field_label}`{key}`（select）的 options 必须是 1..={MAX_UI_OPTIONS} 个字符串"
                ));
            }
            let mut seen = HashSet::new();
            let mut normalized_options = Vec::with_capacity(options.len());
            for option in options {
                let option = option
                    .as_str()
                    .map(str::trim)
                    .filter(|option| !option.is_empty())
                    .ok_or_else(|| {
                        format!("{field_label}`{key}`（select）的 options 必须是非空字符串")
                    })?;
                if seen.insert(option.to_string()) {
                    normalized_options.push(option);
                }
            }
            normalized.insert("options".into(), json!(normalized_options));
        }
        // min/max 只对 number 有意义：非 number 字段静默丢弃；number 字段
        // 类型非法则报错。
        if field_type == "number" {
            for bound in ["min", "max"] {
                match field_map.get(bound) {
                    Some(value) if !value.is_number() => {
                        return Err(format!("{field_label}`{key}` 的 {bound} 必须是数字"));
                    }
                    Some(value) => {
                        normalized.insert(bound.to_string(), value.clone());
                    }
                    None => {}
                }
            }
        }
        normalized_fields.push(Value::Object(normalized));
    }

    let mut normalized = serde_json::Map::new();
    normalized.insert("kind".into(), json!("form"));
    if let Some(title) = title {
        normalized.insert("title".into(), json!(title));
    }
    if let Some(command) = command {
        normalized.insert("command".into(), json!(command));
    }
    normalized.insert("fields".into(), json!(normalized_fields));
    Ok(Value::Object(normalized))
}

/// list 节：columns 1..=8 个，key/label 非空、key 节内唯一，width 缺省 1、
/// 声明时必须是 1..=5 的整数权重；valuesCommand/rowCommand 闭包校验。
fn validate_ui_list(
    label: &str,
    section: &Value,
    command_ids: &HashSet<&str>,
) -> Result<Value, String> {
    let Some(map) = section.as_object() else {
        return Err(format!("{label} 必须是对象"));
    };
    let title = optional_string(map, "title");
    let values_command = optional_command(map, "valuesCommand", label, command_ids)?;
    let row_command = optional_command(map, "rowCommand", label, command_ids)?;
    let columns = map
        .get("columns")
        .and_then(Value::as_array)
        .filter(|columns| !columns.is_empty())
        .ok_or_else(|| format!("{label}（list）的 columns 必须是非空数组"))?;
    if columns.len() > MAX_UI_COLUMNS {
        return Err(format!(
            "{label}（list）的 columns 超过 {MAX_UI_COLUMNS} 个上限"
        ));
    }

    let mut column_keys = HashSet::new();
    let mut normalized_columns = Vec::with_capacity(columns.len());
    for column in columns {
        let column_label = format!("{label}（list）列");
        let Some(column_map) = column.as_object() else {
            return Err(format!("{column_label}必须是对象"));
        };
        let key = column_map
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| format!("{column_label}缺少 key"))?;
        if !column_keys.insert(key.to_string()) {
            return Err(format!("{column_label}`{key}` 重复"));
        }
        let column_name = column_map
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("{column_label}`{key}` 缺少 label"))?;
        // width=1..5 权重（§3.1），缺省 1；JSON 1.0 这类浮点整数按 as_f64
        // 收敛后取整接受。
        let width = match column_map.get("width") {
            Some(value) => {
                let Some(width) = value.as_f64() else {
                    return Err(format!("{column_label}`{key}` 的 width 必须是数字"));
                };
                if width.fract() != 0.0 || !(1.0..=5.0).contains(&width) {
                    return Err(format!(
                        "{column_label}`{key}` 的 width 必须是 1..=5 的整数"
                    ));
                }
                width as i64
            }
            None => 1,
        };
        normalized_columns.push(json!({
            "key": key,
            "label": column_name,
            "width": width,
        }));
    }

    let mut normalized = serde_json::Map::new();
    normalized.insert("kind".into(), json!("list"));
    if let Some(title) = title {
        normalized.insert("title".into(), json!(title));
    }
    if let Some(command) = values_command {
        normalized.insert("valuesCommand".into(), json!(command));
    }
    normalized.insert("columns".into(), json!(normalized_columns));
    if let Some(command) = row_command {
        normalized.insert("rowCommand".into(), json!(command));
    }
    Ok(Value::Object(normalized))
}

/// kv 节：静态 items（0..=64 对，key 非空字符串、value 字符串）与
/// valuesCommand（M2 动态回读）至少结构合法；M1 只渲染静态 items。
fn validate_ui_kv(
    label: &str,
    section: &Value,
    command_ids: &HashSet<&str>,
) -> Result<Value, String> {
    let Some(map) = section.as_object() else {
        return Err(format!("{label} 必须是对象"));
    };
    let title = optional_string(map, "title");
    let values_command = optional_command(map, "valuesCommand", label, command_ids)?;
    let mut normalized = serde_json::Map::new();
    normalized.insert("kind".into(), json!("kv"));
    if let Some(title) = title {
        normalized.insert("title".into(), json!(title));
    }
    if let Some(items) = map.get("items") {
        let items = items
            .as_array()
            .ok_or_else(|| format!("{label}（kv）的 items 必须是数组"))?;
        if items.len() > MAX_UI_ITEMS {
            return Err(format!("{label}（kv）的 items 超过 {MAX_UI_ITEMS} 个上限"));
        }
        let mut normalized_items = Vec::with_capacity(items.len());
        for item in items {
            let item_label = format!("{label}（kv）条目");
            let Some(item_map) = item.as_object() else {
                return Err(format!("{item_label}必须是对象"));
            };
            let key = item_map
                .get("key")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .ok_or_else(|| format!("{item_label}的 key 必须是非空字符串"))?;
            let value = item_map
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{item_label}`{key}` 的 value 必须是字符串"))?;
            normalized_items.push(json!({ "key": key, "value": value }));
        }
        normalized.insert("items".into(), json!(normalized_items));
    }
    if let Some(command) = values_command {
        normalized.insert("valuesCommand".into(), json!(command));
    }
    Ok(Value::Object(normalized))
}

/// 插件 id 形态（反向域名）：非空、≤128 字符、非 `.`/`..`、不含空白/控制字符
/// 与路径分隔符。marketplace 索引条目复用同一规则。
pub(crate) fn valid_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && !id
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '/' | '\\'))
}

fn valid_command_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(|ch| ch.is_whitespace() || ch.is_control())
}

/// 工具名是 Agent 可见标识：小写字母 / 数字 / 点 / 线（反域名风格）。
fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '-'))
}

/// 语言 id 即 Monaco language id：小写字母 / 数字 / 线。
fn valid_language_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

/// 扩展名必须是 `.ext` 形式：点开头、后续不含空白/控制字符。
fn valid_language_extension(extension: &str) -> bool {
    extension.len() >= 2
        && extension.starts_with('.')
        && !extension[1..]
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '/' | '\\'))
}

/// parameters 必须是 JSON object；带 `type` 字段时必须声明为 object
/// （缺省 type 的 properties/required 简写也接受）。
fn valid_tool_parameters(parameters: &Value) -> bool {
    if !parameters.is_object() {
        return false;
    }
    parameters
        .get("type")
        .map(|kind| kind.as_str() == Some("object"))
        .unwrap_or(true)
}

fn runtime_name(runtime: PluginRuntime) -> &'static str {
    match runtime {
        PluginRuntime::Node => "node",
        PluginRuntime::Wasm => "wasm",
        PluginRuntime::Declarative => "declarative",
    }
}

fn info_from_manifest(manifest: &PluginManifest, dir: &Path, enabled: bool) -> PluginInfo {
    let mut seen_mcp = HashSet::new();
    let mcp_servers = manifest
        .contributes
        .mcp_servers
        .keys()
        .filter_map(|name| {
            let name = name.trim();
            if name.is_empty() || !seen_mcp.insert(name.to_string()) {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect();
    let mut seen = HashSet::new();
    let language_servers = manifest
        .contributes
        .language_servers
        .iter()
        .filter_map(|server| {
            if seen.insert(server.id.clone()) {
                Some(server.id.clone())
            } else {
                None
            }
        })
        .collect();
    PluginInfo {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        runtime: manifest.runtime,
        enabled,
        path: display_path(dir),
        error: None,
        commands: manifest.contributes.commands.clone(),
        mcp_servers,
        language_servers,
        themes: manifest
            .contributes
            .themes
            .iter()
            .map(|theme| PluginTheme {
                label: theme.label.clone(),
                path: theme.path.clone(),
                ui_theme: theme.ui_theme.clone(),
            })
            .collect(),
        languages: manifest
            .contributes
            .languages
            .iter()
            .map(language_from_decl)
            .collect(),
        views: manifest
            .contributes
            .views
            .iter()
            .map(|view| PluginView {
                id: view.id.clone(),
                name: view.name.clone(),
            })
            .collect(),
        ui_views: manifest
            .contributes
            .ui
            .views
            .iter()
            .map(|view| PluginUiView {
                id: view.id.clone(),
                name: view.name.clone(),
                open_command: view.open_command.clone(),
                sections: view
                    .sections
                    .as_array()
                    .map(|sections| sections.to_vec())
                    .unwrap_or_default(),
            })
            .collect(),
    }
}

/// 语言声明 → 展示 DTO（`PluginInfo.languages` 元素）。
fn language_from_decl(language: &LanguageDecl) -> PluginLanguage {
    PluginLanguage {
        id: language.id.clone(),
        extensions: language.extensions.clone(),
        aliases: language.aliases.clone(),
        grammar: language.grammar.clone(),
    }
}

fn push_error(
    plugins: &mut Vec<DiscoveredPlugin>,
    seen_ids: &mut HashSet<String>,
    raw_value: Option<&Value>,
    dir_name: &str,
    dir: &Path,
    message: String,
) {
    let mut info = error_info(raw_value, dir_name, dir, message);
    info.id = unique_listing_id(&info.id, seen_ids);
    plugins.push(DiscoveredPlugin {
        info,
        manifest: None,
        dir: dir.to_path_buf(),
        main_path: None,
    });
}

fn unique_listing_id(base: &str, seen_ids: &mut HashSet<String>) -> String {
    let base = if base.trim().is_empty() {
        "plugin"
    } else {
        base
    };
    if seen_ids.insert(base.to_string()) {
        return base.to_string();
    }
    let mut index = 2u32;
    loop {
        let candidate = format!("{base}#{index}");
        if seen_ids.insert(candidate.clone()) {
            return candidate;
        }
        index = index.saturating_add(1);
    }
}

fn error_info(
    raw_value: Option<&Value>,
    dir_name: &str,
    dir: &Path,
    message: String,
) -> PluginInfo {
    let object = raw_value.and_then(Value::as_object);
    let id = object
        .and_then(|object| object.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(dir_name)
        .to_string();
    let name = object
        .and_then(|object| object.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&id)
        .to_string();
    let version = object
        .and_then(|object| object.get("version"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let description = object
        .and_then(|object| object.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let runtime = object
        .and_then(|object| object.get("runtime"))
        .and_then(Value::as_str)
        .and_then(|runtime| serde_json::from_value(Value::String(runtime.to_string())).ok())
        .unwrap_or(PluginRuntime::Declarative);

    let mut commands = Vec::new();
    let mut seen_commands = HashSet::new();
    if let Some(items) = object
        .and_then(|object| object.get("contributes"))
        .and_then(|contributes| contributes.get("commands"))
        .and_then(Value::as_array)
    {
        for item in items {
            let Some(command) = item.get("command").and_then(Value::as_str) else {
                continue;
            };
            let command = command.trim();
            if command.is_empty() || !seen_commands.insert(command.to_string()) {
                continue;
            }
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(command);
            commands.push(PluginCommand {
                command: command.to_string(),
                title: title.to_string(),
            });
        }
    }

    let mut seen_mcp = HashSet::new();
    let mcp_servers = object
        .and_then(|object| object.get("contributes"))
        .and_then(|contributes| contributes.get("mcpServers"))
        .and_then(Value::as_object)
        .map(|servers| {
            servers
                .keys()
                .filter_map(|name| {
                    let name = name.trim();
                    if name.is_empty() || !seen_mcp.insert(name.to_string()) {
                        None
                    } else {
                        Some(name.to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let mut seen_language_servers = HashSet::new();
    let language_servers = object
        .and_then(|object| object.get("contributes"))
        .and_then(|contributes| contributes.get("languageServers"))
        .and_then(Value::as_array)
        .map(|servers| {
            servers
                .iter()
                .filter_map(|server| server.get("id").and_then(Value::as_str))
                .map(str::trim)
                .filter(|id| !id.is_empty() && seen_language_servers.insert(id.to_string()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    PluginInfo {
        id,
        name,
        version,
        description,
        runtime,
        enabled: false,
        path: display_path(dir),
        error: Some(message),
        commands,
        mcp_servers,
        language_servers,
        themes: Vec::new(),
        languages: Vec::new(),
        views: Vec::new(),
        ui_views: Vec::new(),
    }
}

fn read_manifest_value(path: &Path) -> Result<Value, String> {
    let file =
        File::open(path).map_err(|error| format!("读取 {} 失败：{error}", display_path(path)))?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取 {} 失败：{error}", display_path(path)))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!("清单超过 {} KiB 上限", MAX_MANIFEST_BYTES / 1024));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} 不是合法 JSON：{error}", display_path(path)))
}
