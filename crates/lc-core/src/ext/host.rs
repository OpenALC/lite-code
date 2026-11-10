//! Node 扩展宿主客户端（docs/EXTENSIONS.md §4）。
//!
//! 单宿主进程常驻、懒启动：第一次执行 node 插件命令时 spawn
//! `node <ext-host.mjs>`，之后复用；插件集合变化时自动重启。stdin/stdout
//! 为行分隔 JSON-RPC 2.0，请求按 id 匹配，超时/进程退出都会终止宿主，
//! 下一次调用再惰性重启。单个插件抛错只返回 JSON-RPC error，不终止宿主。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::fs::display_path;
use crate::process::scrub_secret_env;

use super::manifest::DiscoveredPlugin;
use super::{
    ExtError, ExtPaths, PluginRuntime, enabled_plugins, normalize_args, project_allowed,
    resolve_asset,
};

/// `initialize` 超时。
pub const INIT_TIMEOUT: Duration = Duration::from_secs(10);
/// `command/execute` 默认超时。
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
/// `tool/execute` 默认超时（插件级 Agent 工具，docs/EXTENSIONS.md §4）。
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const STDERR_TAIL_LINES: usize = 40;
const HOST_ENV: &str = "LC_EXT_HOST";
const WASM_HOST_ENV: &str = "LC_WASM_HOST";

/// 初始化参数里随插件下发的工具声明（名字 → 命令 handler 映射）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostTool {
    name: String,
    command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostPlugin {
    id: String,
    path: String,
    main: String,
    commands: Vec<String>,
    tools: Vec<HostTool>,
}

/// Node 扩展宿主句柄。`execute` 在需要时启动宿主；`reset` 强制下次重启。
pub struct ExtHost {
    node: Option<PathBuf>,
    host_script: Option<PathBuf>,
    wasm_script: Option<PathBuf>,
    process: Option<HostProcess>,
    initialized: Option<Vec<HostPlugin>>,
    load_errors: Vec<String>,
    next_id: u64,
    command_timeout: Duration,
    tool_timeout: Duration,
}

impl ExtHost {
    pub fn new(node: Option<PathBuf>) -> Self {
        Self {
            node,
            host_script: None,
            wasm_script: None,
            process: None,
            initialized: None,
            load_errors: Vec::new(),
            next_id: 0,
            command_timeout: COMMAND_TIMEOUT,
            tool_timeout: TOOL_TIMEOUT,
        }
    }

    /// 真实探测 Node 并构造宿主（探测只发生一次）。
    pub fn detect() -> Self {
        Self::new(super::probe_node())
    }

    /// 重新探测 Node，并丢弃旧宿主（`ext_reload` 用）。
    pub fn refresh_node(&mut self) {
        self.reset();
        self.node = super::probe_node();
    }

    pub fn node_path(&self) -> Option<&Path> {
        self.node.as_deref()
    }

    pub fn node_available(&self) -> bool {
        self.node.is_some()
    }

    /// 测试/嵌入方覆盖宿主脚本（优先于 `LC_EXT_HOST` 与内置资产）。
    pub fn set_host_script(&mut self, path: Option<PathBuf>) {
        self.host_script = path;
    }

    /// 测试/嵌入方覆盖 wasm 宿主脚本。
    pub fn set_wasm_script(&mut self, path: Option<PathBuf>) {
        self.wasm_script = path;
    }

    pub fn set_command_timeout(&mut self, timeout: Duration) {
        self.command_timeout = timeout;
    }

    /// 测试/嵌入方覆盖 `tool/execute` 超时（默认 [`TOOL_TIMEOUT`] = 30s）。
    pub fn set_tool_timeout(&mut self, timeout: Duration) {
        self.tool_timeout = timeout;
    }

    /// `initialize` 返回的插件加载错误（单个插件失败不影响其它插件）。
    pub fn load_errors(&self) -> &[String] {
        &self.load_errors
    }

    /// 丢弃宿主进程；下一次命令按需重启。
    pub fn reset(&mut self) {
        self.stop_host();
    }

    /// 尽力发送 `shutdown` 后终止宿主（drop 时也会走 kill 路径）。
    pub fn shutdown(&mut self) {
        if self.process.as_mut().is_some_and(HostProcess::is_alive) {
            self.next_id = self.next_id.wrapping_add(1);
            let id = self.next_id;
            if let Some(process) = self.process.as_mut() {
                let _ = process.request(id, "shutdown", json!({}), SHUTDOWN_TIMEOUT);
            }
        }
        self.stop_host();
    }

    /// 默认目录布局下执行命令。
    pub fn execute(
        &mut self,
        workspace_root: &Path,
        command: &str,
        args: Value,
    ) -> Result<Value, ExtError> {
        let paths = ExtPaths::default_for(Some(workspace_root))?;
        self.execute_with_paths(&paths, project_allowed(), workspace_root, command, args)
    }

    /// 指定布局下执行命令（测试与嵌入方使用）。
    pub fn execute_with_paths(
        &mut self,
        paths: &ExtPaths,
        allow_project: bool,
        workspace_root: &Path,
        command: &str,
        args: Value,
    ) -> Result<Value, ExtError> {
        let plugins = enabled_plugins(paths, allow_project)?;
        let target = plugins
            .iter()
            .find(|plugin| {
                plugin.info.enabled
                    && plugin.manifest.as_ref().is_some_and(|manifest| {
                        manifest
                            .contributes
                            .commands
                            .iter()
                            .any(|declared| declared.command == command)
                    })
            })
            .ok_or_else(|| ExtError::CommandNotFound {
                command: command.to_string(),
            })?;
        let manifest = target
            .manifest
            .as_ref()
            .ok_or_else(|| ExtError::CommandNotFound {
                command: command.to_string(),
            })?;

        match manifest.runtime {
            PluginRuntime::Node => {
                let node = self.node.clone().ok_or(ExtError::NodeUnavailable)?;
                let host_plugins = host_plugins(&plugins);
                self.ensure_host(&node, workspace_root, &host_plugins)?;
                self.request(
                    "command/execute",
                    json!({
                        "pluginId": target.info.id,
                        "command": command,
                        "args": normalize_args(args),
                    }),
                    self.command_timeout,
                )
            }
            PluginRuntime::Wasm => {
                let node = self.node.clone().ok_or(ExtError::NodeUnavailable)?;
                let main = target
                    .main_path
                    .as_deref()
                    .ok_or_else(|| ExtError::WasmFailed("插件缺少 main（wasm 模块路径）".into()))?;
                let script = self.wasm_script_path()?;
                super::wasm::run(
                    &node,
                    &script,
                    main,
                    command,
                    &normalize_args(args),
                    workspace_root,
                )
            }
            PluginRuntime::Declarative => Err(ExtError::DeclarativeNotExecutable),
        }
    }

    /// 指定布局下执行插件级 Agent 工具（docs/EXTENSIONS.md §4）：
    /// 向扩展宿主发送 `tool/execute`。工具不存在/插件禁用/非 node runtime
    /// 返回 [`ExtError::ToolNotFound`]；宿主未运行时按 node 插件命令的
    /// 既有逻辑惰性启动，Node 缺失返回 [`ExtError::NodeUnavailable`]。
    pub fn execute_tool_with_paths(
        &mut self,
        paths: &ExtPaths,
        allow_project: bool,
        workspace_root: &Path,
        plugin_id: &str,
        tool: &str,
        args: Value,
    ) -> Result<Value, ExtError> {
        let plugins = enabled_plugins(paths, allow_project)?;
        let target = plugins
            .iter()
            .find(|plugin| {
                plugin.info.enabled
                    && plugin.info.id == plugin_id
                    && plugin.manifest.as_ref().is_some_and(|manifest| {
                        manifest
                            .contributes
                            .tools
                            .iter()
                            .any(|declared| declared.name == tool)
                    })
            })
            .ok_or_else(|| ExtError::ToolNotFound {
                plugin: plugin_id.to_string(),
                tool: tool.to_string(),
            })?;
        // v0 工具只在扩展宿主（node runtime）分发；wasm/declarative 插件
        // 声明的工具不参与注册，这里防御性返回未找到。
        if target.manifest.as_ref().map(|manifest| manifest.runtime) != Some(PluginRuntime::Node) {
            return Err(ExtError::ToolNotFound {
                plugin: plugin_id.to_string(),
                tool: tool.to_string(),
            });
        }

        let node = self.node.clone().ok_or(ExtError::NodeUnavailable)?;
        let host_plugins = host_plugins(&plugins);
        self.ensure_host(&node, workspace_root, &host_plugins)?;
        self.request(
            "tool/execute",
            json!({
                "pluginId": target.info.id,
                "tool": tool,
                "args": normalize_args(args),
            }),
            self.tool_timeout,
        )
    }

    fn ensure_host(
        &mut self,
        node: &Path,
        workspace_root: &Path,
        plugins: &[HostPlugin],
    ) -> Result<(), ExtError> {
        if self.initialized.as_deref() != Some(plugins) {
            self.stop_host();
        } else if let Some(process) = self.process.as_mut() {
            if process.is_alive() {
                return Ok(());
            }
            self.stop_host();
        } else {
            self.stop_host();
        }

        let script = self.host_script_path()?;
        let mut process = HostProcess::spawn(node, &script, workspace_root)?;
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let params = json!({
            "workspaceRoot": display_path(workspace_root),
            "plugins": plugins
                .iter()
                .map(|plugin| json!({
                    "id": plugin.id,
                    "path": plugin.path,
                    "main": plugin.main,
                    "commands": plugin.commands,
                    "tools": plugin.tools
                        .iter()
                        .map(|tool| json!({
                            "name": tool.name,
                            "command": tool.command,
                        }))
                        .collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
        });
        let result = process.request(id, "initialize", params, INIT_TIMEOUT)?;
        self.load_errors = parse_load_errors(&result);
        self.process = Some(process);
        self.initialized = Some(plugins.to_vec());
        Ok(())
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ExtError> {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let result = match self.process.as_mut() {
            Some(process) => process.request(id, method, params, timeout),
            None => Err(ExtError::HostExited("扩展宿主未启动".into())),
        };
        if let Err(error) = &result
            && !matches!(error, ExtError::HostRpc { .. })
        {
            self.stop_host();
        }
        result
    }

    fn stop_host(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.terminate();
        }
        self.initialized = None;
        self.load_errors.clear();
    }

    fn host_script_path(&self) -> Result<PathBuf, ExtError> {
        match &self.host_script {
            Some(path) => canonical_script(path),
            None => resolve_asset(
                HOST_ENV,
                "ext-host.mjs",
                include_str!("../../assets/ext-host.mjs"),
            ),
        }
    }

    fn wasm_script_path(&self) -> Result<PathBuf, ExtError> {
        match &self.wasm_script {
            Some(path) => canonical_script(path),
            None => resolve_asset(
                WASM_HOST_ENV,
                "wasm-host.mjs",
                include_str!("../../assets/wasm-host.mjs"),
            ),
        }
    }
}

fn canonical_script(path: &Path) -> Result<PathBuf, ExtError> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        ExtError::AssetMissing(format!("宿主脚本不可用：{}（{error}）", path.display()))
    })?;
    // Node 不认 Windows verbatim 前缀（`\\?\C:\…`），传给 node 的命令行参数
    // 必须用展示路径；canonicalize 仍用于确认文件真实存在。
    Ok(PathBuf::from(display_path(&canonical)))
}

fn host_plugins(plugins: &[DiscoveredPlugin]) -> Vec<HostPlugin> {
    let mut host_plugins: Vec<HostPlugin> = plugins
        .iter()
        .filter_map(|plugin| {
            if !plugin.info.enabled {
                return None;
            }
            let manifest = plugin.manifest.as_ref()?;
            if manifest.runtime != PluginRuntime::Node {
                return None;
            }
            let main = plugin.main_path.as_ref()?;
            Some(HostPlugin {
                id: plugin.info.id.clone(),
                path: display_path(&plugin.dir),
                main: display_path(main),
                commands: manifest
                    .contributes
                    .commands
                    .iter()
                    .map(|command| command.command.clone())
                    .collect(),
                tools: manifest
                    .contributes
                    .tools
                    .iter()
                    .map(|tool| HostTool {
                        name: tool.name.clone(),
                        command: tool.command.clone().unwrap_or_else(|| tool.name.clone()),
                    })
                    .collect(),
            })
        })
        .collect();
    host_plugins.sort_by(|left, right| left.id.cmp(&right.id));
    host_plugins
}

fn parse_load_errors(result: &Value) -> Vec<String> {
    match result.get("errors") {
        Some(Value::Array(items)) => items.iter().map(render_load_error).collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![other.to_string()],
    }
}

fn render_load_error(value: &Value) -> String {
    match value {
        Value::String(message) => message.clone(),
        Value::Object(object) => {
            let plugin = object
                .get("pluginId")
                .or_else(|| object.get("id"))
                .and_then(Value::as_str);
            let message = object
                .get("message")
                .or_else(|| object.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("插件加载失败");
            match plugin {
                Some(plugin) => format!("{plugin}: {message}"),
                None => message.to_string(),
            }
        }
        other => other.to_string(),
    }
}

struct HostProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<Result<Value, String>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

impl HostProcess {
    fn spawn(node: &Path, script: &Path, workspace_root: &Path) -> Result<Self, ExtError> {
        let mut command = Command::new(node);
        command
            .arg(script)
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        scrub_secret_env(&mut command);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(WINDOWS_CREATE_NO_WINDOW);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|error| {
            ExtError::HostSpawn(format!("无法启动扩展宿主 {}：{error}", node.display()))
        })?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        #[cfg(windows)]
        let job = match attach_job(&child) {
            Ok(job) => Some(job),
            Err(message) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ExtError::HostSpawn(message));
            }
        };

        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ExtError::HostSpawn("未能取得扩展宿主 stdio 管道".into()));
        };

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = stderr {
            spawn_stderr_reader(stderr, Arc::clone(&stderr_tail));
        }
        let (sender, responses) = mpsc::channel();
        spawn_stdout_reader(stdout, sender);

        Ok(Self {
            child,
            stdin: Some(stdin),
            responses,
            stderr_tail,
            #[cfg(windows)]
            job,
        })
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ExtError> {
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&message)?;

        let expected = Value::from(id);
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.responses.recv_timeout(remaining) {
                Ok(Ok(value)) => {
                    // 宿主 v0 只回 response；通知/反向请求忽略。
                    if value.get("method").is_some() {
                        continue;
                    }
                    if value.get("id") != Some(&expected) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("未知 JSON-RPC 错误")
                            .to_string();
                        let data = error.get("data").map(|data| {
                            data.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| data.to_string())
                        });
                        return Err(ExtError::HostRpc {
                            code,
                            message,
                            data,
                        });
                    }
                    return value.get("result").cloned().ok_or_else(|| {
                        ExtError::HostProtocol(format!("JSON-RPC 响应缺少 result（id {id}）"))
                    });
                }
                Ok(Err(message)) => return Err(ExtError::HostProtocol(message)),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(ExtError::HostTimeout {
                        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    });
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let status = self
                        .child
                        .try_wait()
                        .ok()
                        .flatten()
                        .map(|status| status.to_string())
                        .unwrap_or_else(|| "stdout 已关闭".into());
                    return Err(ExtError::HostExited(format!(
                        "扩展宿主已退出（{status}）{}",
                        self.stderr_detail()
                    )));
                }
            }
        }
    }

    fn write_message(&mut self, message: &Value) -> Result<(), ExtError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(ExtError::HostExited("扩展宿主 stdin 已关闭".into()));
        };
        let mut bytes = serde_json::to_vec(message)
            .map_err(|error| ExtError::HostProtocol(format!("请求序列化失败：{error}")))?;
        bytes.push(b'\n');
        stdin
            .write_all(&bytes)
            .and_then(|()| stdin.flush())
            .map_err(|error| ExtError::HostExited(format!("写入扩展宿主 stdin 失败：{error}")))
    }

    fn stderr_detail(&self) -> String {
        let tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if tail.is_empty() {
            String::new()
        } else {
            format!(
                "；stderr:\n{}",
                tail.iter().cloned().collect::<Vec<_>>().join("\n")
            )
        }
    }

    fn terminate(&mut self) {
        self.stdin.take();
        #[cfg(unix)]
        {
            let _ = kill_process_group(self.child.id());
        }
        let _ = self.child.kill();
        #[cfg(windows)]
        {
            // Job 句柄关闭 → KILL_ON_JOB_CLOSE → 整树终止。
            self.job.take();
        }
        let _ = self.child.wait();
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn spawn_stdout_reader(stdout: ChildStdout, sender: Sender<Result<Value, String>>) {
    let _ = std::thread::Builder::new()
        .name("ext-host-stdout".into())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ = sender.send(Err(format!("读取扩展宿主 stdout 失败：{error}")));
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        if sender.send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(format!("扩展宿主输出不是合法 JSON-RPC：{error}")));
                        break;
                    }
                }
            }
        });
}

fn spawn_stderr_reader(stderr: ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    let _ = std::thread::Builder::new()
        .name("ext-host-stderr".into())
        .spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let mut guard = tail.lock().unwrap_or_else(PoisonError::into_inner);
                if guard.len() >= STDERR_TAIL_LINES {
                    guard.pop_front();
                }
                guard.push_back(line);
            }
        });
}

#[cfg(windows)]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn attach_job(child: &Child) -> Result<win32job::Job, String> {
    use std::os::windows::io::AsRawHandle;

    let mut info = win32job::ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&info)
        .map_err(|error| format!("创建扩展宿主 Job Object 失败：{error}"))?;
    job.assign_process(child.as_raw_handle() as isize)
        .map_err(|error| format!("扩展宿主加入 Job Object 失败：{error}"))?;
    Ok(job)
}

#[cfg(unix)]
fn kill_process_group(pid: u32) -> std::io::Result<()> {
    let rc = unsafe { unix_ffi::kill(-(pid as i32), unix_ffi::SIGKILL) };
    if rc == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(unix_ffi::ESRCH) {
        return Ok(());
    }
    Err(error)
}

#[cfg(unix)]
mod unix_ffi {
    use std::ffi::c_int;

    unsafe extern "C" {
        pub fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    pub const SIGKILL: c_int = 9;
    pub const ESRCH: c_int = 3;
}
