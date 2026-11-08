//! ext lane 单测：清单校验、启用状态、MCP 合并与 Node/WASM 宿主协议。
//!
//! 宿主测试使用临时 fake Node 脚本；找不到 Node 时跳过并打印原因，
//! 不让 CI 因缺少运行时失败。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::mcp::plugin_mcp_servers_at;
use super::*;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lc-ext-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("创建临时目录");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn test_paths(temp: &TempDir) -> ExtPaths {
    ExtPaths::new(
        temp.path.join("extensions"),
        Some(temp.path.join("project-extensions")),
        temp.path.join("extensions.json"),
    )
}

fn write_plugin(root: &Path, dir: &str, manifest: Value, files: &[&str]) -> PathBuf {
    let plugin_dir = root.join(dir);
    fs::create_dir_all(&plugin_dir).expect("创建插件目录");
    fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_vec_pretty(&manifest).expect("序列化清单"),
    )
    .expect("写清单");
    for file in files {
        let path = plugin_dir.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("创建文件父目录");
        }
        fs::write(path, b"").expect("写插件文件");
    }
    plugin_dir
}

fn node_manifest(id: &str, main: &str, commands: &[&str]) -> Value {
    json!({
        "id": id,
        "name": id,
        "version": "0.1.0",
        "description": "",
        "runtime": "node",
        "main": main,
        "contributes": {
            "commands": commands
                .iter()
                .map(|command| json!({"command": command, "title": command}))
                .collect::<Vec<_>>(),
        },
    })
}

fn write_node_plugin(paths: &ExtPaths, dir: &str, id: &str, commands: &[&str]) {
    write_plugin(
        &paths.user_dir,
        dir,
        node_manifest(id, "./index.mjs", commands),
        &["index.mjs"],
    );
}

fn test_node() -> Option<PathBuf> {
    let node = std::env::var_os("LITE_CODE_TEST_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("node"));
    match Command::new(&node).arg("--version").output() {
        Ok(output) if output.status.success() => Some(node),
        _ => {
            eprintln!("跳过扩展宿主集成测试：找不到可用的 node");
            None
        }
    }
}

const FAKE_HOST: &str = r#"
import readline from 'node:readline';
import fs from 'node:fs';
const rl = readline.createInterface({ input: process.stdin });
let plugins = [];
let workspaceRoot = null;
rl.on('line', (line) => {
  const message = JSON.parse(line);
  const reply = (result) => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }) + '\n');
  if (message.method === 'initialize') {
    plugins = message.params.plugins;
    workspaceRoot = message.params.workspaceRoot;
    fs.appendFileSync(__INIT_MARKER__, 'init\n');
    reply({ errors: [{ pluginId: 'bad.plugin', message: 'load failed' }] });
  } else if (message.method === 'command/execute') {
    if (message.params.command === 'test.echo') {
      reply({ pluginId: message.params.pluginId, args: message.params.args, plugins, workspaceRoot });
    } else if (message.params.command === 'test.fail') {
      process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: message.id, error: { code: -32603, message: 'boom', data: { why: 'test' } } }) + '\n');
    } else if (message.params.command === 'test.hang') {
      // 故意不回复，测试 Rust 侧超时。
    } else if (message.params.command === 'test.exit') {
      process.exit(0);
    }
  } else if (message.method === 'shutdown') {
    reply({});
    process.exit(0);
  }
});
"#;

fn write_fake_host(temp: &TempDir, marker: &Path) -> PathBuf {
    let marker_literal =
        serde_json::to_string(marker.to_string_lossy().as_ref()).expect("marker JSON");
    let script = temp.path.join("fake-host.mjs");
    fs::write(
        &script,
        FAKE_HOST.replace("__INIT_MARKER__", &marker_literal),
    )
    .expect("写 fake host");
    script
}

const FAKE_WASM_HOST: &str = r#"
const args = process.argv.slice(2);
const get = (name) => args[args.indexOf(name) + 1];
const result = {
  wasm: get('--wasm'),
  command: get('--command'),
  args: JSON.parse(get('--args')),
  workspace: get('--workspace'),
};
process.stdout.write(JSON.stringify(result));
"#;

fn write_fake_wasm_host(temp: &TempDir) -> PathBuf {
    let script = temp.path.join("fake-wasm-host.mjs");
    fs::write(&script, FAKE_WASM_HOST).expect("写 fake wasm host");
    script
}

#[test]
fn manifest_parses_contract_fields_and_serializes_camel_case() {
    let temp = TempDir::new("parse");
    let paths = test_paths(&temp);
    write_plugin(
        &paths.user_dir,
        "example.hello",
        json!({
            "id": "example.hello",
            "name": "Hello",
            "version": "0.1.0",
            "description": "示例插件",
            "engines": { "lite-code": ">=0.6.1" },
            "runtime": "node",
            "main": "./index.mjs",
            "activationEvents": ["onCommand:example.hello"],
            "contributes": {
                "commands": [{ "command": "example.hello", "title": "Hello from plugin" }],
                "mcpServers": {
                    "filesystem": {
                        "command": "npx",
                        "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]
                    }
                },
                "languageServers": [{
                    "id": "typescript",
                    "languageIds": ["typescript", "javascript"],
                    "command": "typescript-language-server",
                    "args": ["--stdio"],
                    "env": {}
                }]
            },
            "permissions": ["workspace.read"]
        }),
        &["index.mjs"],
    );

    let list = list_plugins_at(&paths, false, false).expect("list");
    assert_eq!(list.plugins.len(), 1);
    let info = &list.plugins[0];
    assert_eq!(info.id, "example.hello");
    assert_eq!(info.name, "Hello");
    assert_eq!(info.version, "0.1.0");
    assert_eq!(info.runtime, PluginRuntime::Node);
    assert!(info.enabled);
    assert!(info.error.is_none());
    assert_eq!(info.commands[0].command, "example.hello");
    assert_eq!(info.commands[0].title, "Hello from plugin");
    assert_eq!(info.mcp_servers, vec!["filesystem"]);
    assert_eq!(info.language_servers, vec!["typescript"]);

    let serialized = serde_json::to_value(&list).expect("serialize");
    assert_eq!(serialized["nodeAvailable"], false);
    assert_eq!(serialized["dir"], crate::fs::display_path(&paths.user_dir));
    assert_eq!(serialized["plugins"][0]["mcpServers"][0], "filesystem");
    assert_eq!(serialized["plugins"][0]["languageServers"][0], "typescript");
    assert_eq!(serialized["plugins"][0]["error"], Value::Null);
    assert_eq!(serialized["plugins"][0]["runtime"], "node");
}

#[test]
fn manifest_requires_main_for_node_and_wasm() {
    let temp = TempDir::new("main-required");
    let paths = test_paths(&temp);
    write_plugin(
        &paths.user_dir,
        "example.nomain",
        json!({
            "id": "example.nomain",
            "name": "No main",
            "version": "0.1.0",
            "runtime": "node",
            "contributes": { "commands": [{ "command": "example.nomain" }] }
        }),
        &[],
    );

    let list = list_plugins_at(&paths, false, false).expect("list");
    assert_eq!(list.plugins.len(), 1);
    let error = list.plugins[0].error.as_deref().expect("error");
    assert!(error.contains("需要 main"), "{error}");
    assert!(!list.plugins[0].enabled);
}

#[test]
fn main_cannot_escape_plugin_dir() {
    let temp = TempDir::new("escape");
    let paths = test_paths(&temp);
    fs::create_dir_all(&paths.user_dir).expect("create extensions");
    let outside = paths.user_dir.join("outside.mjs");
    fs::write(&outside, b"").expect("write outside");
    write_plugin(
        &paths.user_dir,
        "example.escape",
        node_manifest("example.escape", "../outside.mjs", &[]),
        &[],
    );

    let list = list_plugins_at(&paths, false, false).expect("list");
    assert_eq!(list.plugins.len(), 1);
    let error = list.plugins[0].error.as_deref().expect("error");
    assert!(error.contains("插件目录内"), "{error}");
    assert!(!list.plugins[0].enabled);
}

#[test]
fn oversized_and_invalid_manifests_are_reported_per_plugin() {
    let temp = TempDir::new("bad-manifest");
    let paths = test_paths(&temp);

    let oversized_dir = paths.user_dir.join("big");
    fs::create_dir_all(&oversized_dir).expect("create big dir");
    fs::write(
        oversized_dir.join("plugin.json"),
        vec![b'x'; usize::try_from(MAX_MANIFEST_BYTES).expect("limit fits usize") + 1],
    )
    .expect("write oversized");

    let invalid_dir = paths.user_dir.join("invalid");
    fs::create_dir_all(&invalid_dir).expect("create invalid dir");
    fs::write(invalid_dir.join("plugin.json"), "{ not json").expect("write invalid");

    let list = list_plugins_at(&paths, false, false).expect("list");
    assert_eq!(list.plugins.len(), 2);
    assert!(list.plugins.iter().any(|plugin| {
        plugin
            .error
            .as_deref()
            .is_some_and(|error| error.contains("256 KiB"))
    }));
    assert!(list.plugins.iter().any(|plugin| {
        plugin
            .error
            .as_deref()
            .is_some_and(|error| error.contains("合法 JSON"))
    }));
}

#[test]
fn duplicate_plugin_ids_keep_the_first_entry_and_mark_the_rest() {
    let temp = TempDir::new("duplicate-id");
    let paths = test_paths(&temp);
    write_node_plugin(&paths, "first", "duplicate.id", &["duplicate.first"]);
    write_node_plugin(&paths, "second", "duplicate.id", &["duplicate.second"]);

    let list = list_plugins_at(&paths, false, false).expect("list");
    assert_eq!(list.plugins.len(), 2);
    assert_eq!(list.plugins[0].id, "duplicate.id");
    assert!(list.plugins[0].error.is_none());
    let duplicate = &list.plugins[1];
    assert_ne!(duplicate.id, "duplicate.id");
    assert!(duplicate.id.starts_with("duplicate.id#"));
    assert!(
        duplicate
            .error
            .as_deref()
            .is_some_and(|error| error.contains("重复"))
    );
    assert!(!duplicate.enabled);
}

#[test]
fn duplicate_commands_are_rejected_within_and_across_plugins() {
    let temp = TempDir::new("duplicate-command");
    let paths = test_paths(&temp);
    write_node_plugin(
        &paths,
        "inside",
        "example.inside",
        &["same.command", "same.command"],
    );

    let list = list_plugins_at(&paths, false, false).expect("list");
    assert_eq!(list.plugins.len(), 1);
    assert!(
        list.plugins[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("命令 id 重复"))
    );

    let temp = TempDir::new("duplicate-command-cross");
    let paths = test_paths(&temp);
    write_node_plugin(&paths, "a-first", "example.first", &["shared.command"]);
    write_node_plugin(&paths, "b-second", "example.second", &["shared.command"]);

    let list = list_plugins_at(&paths, false, false).expect("list");
    assert_eq!(list.plugins.len(), 2);
    assert!(list.plugins[0].error.is_none());
    assert!(
        list.plugins[1]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("已由插件") && error.contains("shared.command"))
    );
}

#[test]
fn enable_state_persists_and_preserves_unknown_fields() {
    let temp = TempDir::new("state");
    let paths = test_paths(&temp);
    write_node_plugin(&paths, "one", "example.one", &["one.run"]);
    write_node_plugin(&paths, "two", "example.two", &["two.run"]);

    let list = set_enabled_at(&paths, false, "example.one", false, false).expect("disable");
    let one = list
        .plugins
        .iter()
        .find(|plugin| plugin.id == "example.one")
        .expect("one");
    let two = list
        .plugins
        .iter()
        .find(|plugin| plugin.id == "example.two")
        .expect("two");
    assert!(!one.enabled);
    assert!(two.enabled);

    let state: Value =
        serde_json::from_str(&fs::read_to_string(&paths.state_path).expect("read state"))
            .expect("parse state");
    assert_eq!(state["disabled"], json!(["example.one"]));

    let list = list_plugins_at(&paths, false, false).expect("list after disable");
    assert!(
        !list
            .plugins
            .iter()
            .find(|plugin| plugin.id == "example.one")
            .expect("one")
            .enabled
    );

    set_enabled_at(&paths, false, "example.one", true, false).expect("enable");
    assert!(
        list_plugins_at(&paths, false, false)
            .expect("list after enable")
            .plugins
            .iter()
            .find(|plugin| plugin.id == "example.one")
            .expect("one")
            .enabled
    );

    fs::write(
        &paths.state_path,
        r#"{"disabled":[],"future":{"keep":true}}"#,
    )
    .expect("write state with unknown field");
    set_enabled_at(&paths, false, "example.two", false, false).expect("disable two");
    let state: Value =
        serde_json::from_str(&fs::read_to_string(&paths.state_path).expect("read state"))
            .expect("parse state");
    assert_eq!(state["future"]["keep"], true);
    assert_eq!(state["disabled"], json!(["example.two"]));

    let unknown =
        set_enabled_at(&paths, false, "missing.plugin", false, false).expect_err("unknown");
    assert!(matches!(unknown, ExtError::UnknownPlugin { .. }));
}

#[test]
fn project_plugins_require_allow_flag_and_user_ids_win() {
    let temp = TempDir::new("project");
    let paths = test_paths(&temp);
    write_node_plugin(&paths, "user", "user.plugin", &["user.run"]);
    let project_dir = paths.project_dir.as_deref().expect("project dir");
    write_node_plugin(
        &ExtPaths::new(project_dir.to_path_buf(), None, paths.state_path.clone()),
        "project",
        "project.plugin",
        &["project.run"],
    );
    write_node_plugin(
        &ExtPaths::new(project_dir.to_path_buf(), None, paths.state_path.clone()),
        "shadow",
        "user.plugin",
        &["shadow.run"],
    );

    let without_project = list_plugins_at(&paths, false, false).expect("list default");
    assert_eq!(
        without_project
            .plugins
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>(),
        vec!["user.plugin"]
    );

    let with_project = list_plugins_at(&paths, true, false).expect("list project");
    assert_eq!(with_project.plugins.len(), 3);
    assert_eq!(
        with_project
            .plugins
            .iter()
            .filter(|plugin| plugin.id == "user.plugin")
            .count(),
        1
    );
    assert!(with_project.plugins.iter().any(|plugin| {
        plugin
            .error
            .as_deref()
            .is_some_and(|error| error.contains("重复"))
    }));
}

#[test]
fn plugin_mcp_servers_are_prefixed_filtered_and_disabled_aware() {
    let temp = TempDir::new("mcp");
    let paths = test_paths(&temp);
    let mut manifest = node_manifest("example.mcp", "./index.mjs", &[]);
    manifest["contributes"]["mcpServers"] = json!({
        "filesystem": {
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
            "env": {"KEY": "VALUE"}
        },
        "off": { "command": "off-cmd", "enabled": false },
        "remote": { "type": "http", "url": "https://example.com/mcp" },
        "empty": {}
    });
    write_plugin(&paths.user_dir, "mcp", manifest, &["index.mjs"]);

    let mut disabled_manifest = node_manifest("example.disabled", "./index.mjs", &[]);
    disabled_manifest["contributes"]["mcpServers"] = json!({"hidden": {"command": "hidden-cmd"}});
    write_plugin(
        &paths.user_dir,
        "disabled",
        disabled_manifest,
        &["index.mjs"],
    );
    set_enabled_at(&paths, false, "example.disabled", false, false).expect("disable");

    let servers = plugin_mcp_servers_at(&paths, false);
    assert!(
        servers
            .iter()
            .any(|server| server.name == "plugin.example.mcp.filesystem")
    );
    assert!(
        !servers
            .iter()
            .any(|server| server.name == "plugin.example.mcp.remote")
    );
    assert!(
        !servers
            .iter()
            .any(|server| server.name == "plugin.example.mcp.empty")
    );
    assert!(
        !servers
            .iter()
            .any(|server| server.name.starts_with("plugin.example.disabled"))
    );

    let filesystem = servers
        .iter()
        .find(|server| server.name == "plugin.example.mcp.filesystem")
        .expect("filesystem");
    assert_eq!(filesystem.command, "npx");
    assert_eq!(filesystem.args.len(), 3);
    assert_eq!(filesystem.env.get("KEY").map(String::as_str), Some("VALUE"));
    assert!(filesystem.enabled);

    let off = servers
        .iter()
        .find(|server| server.name == "plugin.example.mcp.off")
        .expect("off");
    assert!(!off.enabled);
}

#[test]
fn undeclared_commands_and_declarative_plugins_are_rejected() {
    let temp = TempDir::new("declarative");
    let paths = test_paths(&temp);
    write_plugin(
        &paths.user_dir,
        "decl",
        json!({
            "id": "example.decl",
            "name": "Declarative",
            "version": "0.1.0",
            "runtime": "declarative",
            "contributes": { "commands": [{ "command": "decl.hello", "title": "Hello" }] }
        }),
        &[],
    );

    let mut host = ExtHost::new(None);
    let error = host
        .execute_with_paths(&paths, false, &temp.path, "decl.hello", json!({}))
        .expect_err("declarative command");
    assert_eq!(error, ExtError::DeclarativeNotExecutable);
    assert_eq!(error.code(), ErrorCode::NotImplemented);

    let missing = host
        .execute_with_paths(&paths, false, &temp.path, "missing.command", json!({}))
        .expect_err("missing command");
    assert!(matches!(missing, ExtError::CommandNotFound { .. }));
    assert_eq!(missing.code(), ErrorCode::NotFound);
}

#[test]
fn node_plugin_reports_node_unavailable_before_spawning() {
    let temp = TempDir::new("no-node");
    let paths = test_paths(&temp);
    write_node_plugin(&paths, "node", "example.node", &["node.run"]);

    let mut host = ExtHost::new(None);
    let error = host
        .execute_with_paths(&paths, false, &temp.path, "node.run", json!({}))
        .expect_err("node unavailable");
    assert_eq!(error, ExtError::NodeUnavailable);
    assert_eq!(error.code(), ErrorCode::NodeUnavailable);
}

#[test]
fn node_host_initializes_executes_keeps_one_process_and_maps_errors() {
    let Some(node) = test_node() else {
        return;
    };
    let temp = TempDir::new("host");
    let paths = test_paths(&temp);
    write_node_plugin(
        &paths,
        "host",
        "example.host",
        &["test.echo", "test.fail", "test.hang", "test.exit"],
    );
    let workspace = temp.path.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");
    let marker = temp.path.join("init-marker.txt");
    let script = write_fake_host(&temp, &marker);

    let mut host = ExtHost::new(Some(node));
    host.set_host_script(Some(script));

    let result = host
        .execute_with_paths(&paths, false, &workspace, "test.echo", json!({"value": 1}))
        .expect("execute echo");
    assert_eq!(result["pluginId"], "example.host");
    assert_eq!(result["args"]["value"], 1);
    assert_eq!(result["workspaceRoot"], crate::fs::display_path(&workspace));
    assert_eq!(
        result["plugins"][0]["commands"],
        json!(["test.echo", "test.fail", "test.hang", "test.exit"])
    );
    assert_eq!(result["plugins"][0]["id"], "example.host");

    host.execute_with_paths(&paths, false, &workspace, "test.echo", json!({}))
        .expect("second execute");
    assert_eq!(
        fs::read_to_string(&marker)
            .expect("read marker")
            .lines()
            .count(),
        1,
        "单宿主只 initialize 一次"
    );
    assert_eq!(host.load_errors()[0], "bad.plugin: load failed");

    let error = host
        .execute_with_paths(&paths, false, &workspace, "test.fail", json!({}))
        .expect_err("plugin error");
    match error {
        ExtError::HostRpc {
            code,
            message,
            data,
        } => {
            assert_eq!(code, -32603);
            assert!(message.contains("boom"), "{message}");
            assert!(
                data.as_deref().is_some_and(|data| data.contains("test")),
                "{data:?}"
            );
        }
        other => panic!("expected HostRpc, got {other:?}"),
    }

    host.execute_with_paths(&paths, false, &workspace, "test.echo", json!({}))
        .expect("host survives plugin errors");

    let exited = host
        .execute_with_paths(&paths, false, &workspace, "test.exit", json!({}))
        .expect_err("process exit");
    assert!(matches!(exited, ExtError::HostExited(_)), "{exited:?}");

    host.execute_with_paths(&paths, false, &workspace, "test.echo", json!({}))
        .expect("host restarts after exit");
    host.shutdown();
}

#[test]
fn real_host_end_to_end_uses_bundled_host() {
    let Some(node) = test_node() else {
        return;
    };
    let temp = TempDir::new("real-host");
    let paths = test_paths(&temp);
    let plugin_dir = paths.user_dir.join("real");
    fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "example.real",
            "name": "Real host",
            "version": "0.1.0",
            "runtime": "node",
            "main": "./index.mjs",
            "contributes": {
                "commands": [{ "command": "real.echo", "title": "Echo" }]
            }
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    fs::write(
        plugin_dir.join("index.mjs"),
        r#"
export async function activate(ctx) {
  ctx.commands.register("real.echo", async (args) => ({
    echoed: args,
    root: ctx.workspace.root,
  }));
}
export function deactivate() {}
"#,
    )
    .expect("write plugin module");

    let workspace = temp.path.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/ext-host.mjs");
    let host_source = fs::read_to_string(&script).unwrap_or_default();
    if !host_source.contains("command/execute") {
        eprintln!("跳过真实扩展宿主集成测试：assets/ext-host.mjs 仍是 placeholder");
        return;
    }
    let mut host = ExtHost::new(Some(node));
    host.set_host_script(Some(script));

    let result = host
        .execute_with_paths(&paths, false, &workspace, "real.echo", json!({"n": 7}))
        .expect("real host execute");
    assert_eq!(result["echoed"]["n"], 7);
    assert_eq!(result["root"], crate::fs::display_path(&workspace));
    assert!(host.load_errors().is_empty(), "{:?}", host.load_errors());
    host.shutdown();
}

#[test]
fn host_restarts_when_plugin_set_changes() {
    let Some(node) = test_node() else {
        return;
    };
    let temp = TempDir::new("host-reload");
    let paths = test_paths(&temp);
    write_node_plugin(&paths, "a", "example.a", &["test.echo"]);
    let workspace = temp.path.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");
    let marker = temp.path.join("init-marker.txt");
    let script = write_fake_host(&temp, &marker);

    let mut host = ExtHost::new(Some(node));
    host.set_host_script(Some(script));
    host.execute_with_paths(&paths, false, &workspace, "test.echo", json!({}))
        .expect("first execute");
    assert_eq!(
        fs::read_to_string(&marker)
            .expect("read marker")
            .lines()
            .count(),
        1
    );

    write_node_plugin(&paths, "b", "example.b", &["test.other"]);
    host.execute_with_paths(&paths, false, &workspace, "test.echo", json!({}))
        .expect("execute after plugin added");
    assert_eq!(
        fs::read_to_string(&marker)
            .expect("read marker")
            .lines()
            .count(),
        2,
        "插件集合变化应重启宿主"
    );
    host.shutdown();
}

#[test]
fn host_timeout_maps_to_timeout_and_recovers() {
    let Some(node) = test_node() else {
        return;
    };
    let temp = TempDir::new("host-timeout");
    let paths = test_paths(&temp);
    write_node_plugin(
        &paths,
        "timeout",
        "example.timeout",
        &["test.hang", "test.echo"],
    );
    let workspace = temp.path.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");
    let marker = temp.path.join("init-marker.txt");
    let script = write_fake_host(&temp, &marker);

    let mut host = ExtHost::new(Some(node));
    host.set_host_script(Some(script));
    host.set_command_timeout(Duration::from_millis(150));
    let error = host
        .execute_with_paths(&paths, false, &workspace, "test.hang", json!({}))
        .expect_err("timeout");
    assert!(
        matches!(error, ExtError::HostTimeout { timeout_ms: 150 }),
        "{error:?}"
    );
    assert_eq!(error.code(), ErrorCode::Timeout);

    host.set_command_timeout(Duration::from_secs(5));
    host.execute_with_paths(&paths, false, &workspace, "test.echo", json!({}))
        .expect("host recovers after timeout");
    host.shutdown();
}

#[test]
fn wasm_command_runs_host_with_documented_arguments() {
    let Some(node) = test_node() else {
        return;
    };
    let temp = TempDir::new("wasm");
    let paths = test_paths(&temp);
    write_plugin(
        &paths.user_dir,
        "wasm",
        json!({
            "id": "example.wasm",
            "name": "Wasm",
            "version": "0.1.0",
            "runtime": "wasm",
            "main": "./plugin.wasm",
            "contributes": { "commands": [{ "command": "wasm.run", "title": "Run" }] }
        }),
        &["plugin.wasm"],
    );
    let workspace = temp.path.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");
    let script = write_fake_wasm_host(&temp);

    let mut host = ExtHost::new(Some(node));
    host.set_wasm_script(Some(script));
    let result = host
        .execute_with_paths(&paths, false, &workspace, "wasm.run", json!({"answer": 42}))
        .expect("wasm execute");
    assert_eq!(result["command"], "wasm.run");
    assert_eq!(result["args"]["answer"], 42);
    assert_eq!(result["workspace"], crate::fs::display_path(&workspace));
    assert!(
        result["wasm"]
            .as_str()
            .is_some_and(|path| path.ends_with("plugin.wasm")),
        "{result}"
    );

    let mut no_node = ExtHost::new(None);
    no_node.set_wasm_script(Some(write_fake_wasm_host(&temp)));
    let error = no_node
        .execute_with_paths(&paths, false, &workspace, "wasm.run", json!({}))
        .expect_err("node unavailable for wasm");
    assert_eq!(error, ExtError::NodeUnavailable);
}

#[cfg(windows)]
#[test]
fn windows_main_accepts_backslash_relative_paths() {
    let temp = TempDir::new("windows-path");
    let paths = test_paths(&temp);
    write_plugin(
        &paths.user_dir,
        "win",
        node_manifest("example.win", ".\\index.mjs", &["win.run"]),
        &["index.mjs"],
    );

    let list = list_plugins_at(&paths, false, false).expect("list");
    assert_eq!(list.plugins.len(), 1);
    assert!(
        list.plugins[0].error.is_none(),
        "{:?}",
        list.plugins[0].error
    );
    assert!(!list.plugins[0].path.starts_with(r"\\?\"));
    assert!(list.plugins[0].path.ends_with("win"));
}

#[test]
fn project_switch_requires_exactly_one() {
    assert!(project_allowed_value(Some("1")));
    assert!(project_allowed_value(Some(" 1 ")));
    assert!(!project_allowed_value(Some("true")));
    assert!(!project_allowed_value(Some("0")));
    assert!(!project_allowed_value(Some("")));
    assert!(!project_allowed_value(None));
}
