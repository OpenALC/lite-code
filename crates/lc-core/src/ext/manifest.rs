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
use serde_json::Value;

use crate::fs::{FsError, RealPathResolver, display_path, ensure_within_root};

use super::{
    ExtError, ExtPaths, PluginCommand, PluginInfo, PluginLanguage, PluginRuntime, PluginTheme,
};

pub(crate) const MANIFEST_FILE: &str = "plugin.json";
pub(crate) const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// 主题文件大小上限（docs/EXTENSIONS.md §1）：读取时由 `ext_read_theme` 校验。
pub(crate) const MAX_THEME_BYTES: u64 = 64 * 1024;
/// 语法文件大小上限（docs/EXTENSIONS.md §1）：读取时由 `ext_read_grammar` 校验。
pub(crate) const MAX_GRAMMAR_BYTES: u64 = 64 * 1024;

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
            let listed = group
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join("、");
            mark_plugin_error(
                plugin,
                format!("extensionDependencies 存在循环依赖：{listed}"),
            );
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

/// Tarjan 强连通分量：返回依赖图中所有大小 > 1 的分量（即环），
/// 供环上插件整体报错并退出下发。
fn dependency_cycles(graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
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

    // 激活事件校验（docs/EXTENSIONS.md §4）：v1 仅支持 `*` 与
    // `onCommand:<commandId>`；非法事件只把该插件标记为 error。
    let mut activation_events = Vec::new();
    for event in &manifest.activation_events {
        let event = event.trim();
        if event != "*" && !event.starts_with("onCommand:") {
            return Err(format!(
                "activationEvents 无效：`{event}`（仅支持 `*` 或 `onCommand:<commandId>`）"
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

fn valid_plugin_id(id: &str) -> bool {
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
