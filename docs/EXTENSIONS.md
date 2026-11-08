# 插件系统（v0 设计）

> 目标：让第三方用 **Node/JavaScript、WASM 或外部进程（MCP/LSP）** 扩展 Lite Code，
> 不在 Tauri 渲染进程里执行插件代码，也不要求插件作者写 Rust。
> v0 只做可用的最小闭环，不承诺 VS Code API 兼容。

## 1. 插件目录与清单

用户级：`~/.lite-code/extensions/<publisher>.<name>/plugin.json`
项目级：`<workspace>/.lite-code/extensions/`（默认忽略，`LC_EXT_ALLOW_PROJECT=1` 才读取）。
启用状态：`~/.lite-code/extensions.json`，形如 `{ "disabled": ["id"] }`。

```jsonc
{
  "id": "example.hello",              // 全局唯一，反向域名
  "name": "Hello",
  "version": "0.1.0",
  "description": "示例插件",
  "engines": { "lite-code": ">=0.6.1" },
  "runtime": "node",                   // node | wasm | declarative
  "main": "./index.mjs",               // node: JS 入口；wasm: .wasm；declarative 可省略
  "activationEvents": ["onCommand:example.hello"],
  "contributes": {
    "commands": [
      { "command": "example.hello", "title": "Hello from plugin" }
    ],
    "mcpServers": {
      "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] }
    },
    "languageServers": [
      {
        "id": "typescript",
        "languageIds": ["typescript", "javascript"],
        "command": "typescript-language-server",
        "args": ["--stdio"],
        "env": {}
      }
    ]
  },
  "permissions": ["workspace.read"]    // 提示用；Node 插件本质是可信代码
}
```

校验：`id` 唯一、`version` 非空、`runtime` 合法、`main` 必须落在插件目录内且存在
（declarative 除外）、命令 id 唯一、清单大小 ≤ 256 KiB。

## 2. 运行形态

| runtime | 执行方式 | 适合 |
|---|---|---|
| `node` | 常驻 Node 扩展宿主（stdio JSON-RPC） | 命令、工作区读写、状态、生态复用 |
| `wasm` | 每次命令启动一个 WASI 模块（无文件/网络权限） | 不可信工具、算法、跨语言插件 |
| `declarative` | 不执行代码，只读取 `contributes` | 主题、MCP/LSP 声明、命令占位 |

Node 扩展宿主由 Rust 侧按需启动；无 Node 时 node 插件禁用，declarative/MCP 插件仍可用。
WASM 运行器也依赖 Node，但只使用 `node:wasi`，不把 Node API 暴露给 wasm。

## 3. Rust 命令（Tauri，custom DTO）

```text
ext_list() -> PluginList
ext_set_enabled(id, enabled) -> PluginList
ext_reload() -> PluginList
ext_execute_command(command, args?) -> { result: any }
ext_open_dir() -> ack
lsp_start_plugins() -> { servers: [{ id, languageIds }], errors: [] }
lsp_start_all(configs?) -> { servers, errors }
lsp_did_open(path, language_id, text) -> ack
lsp_did_change(path, text) -> ack
lsp_did_close(path) -> ack
lsp_stop_all() -> ack
```

`PluginList`：

```jsonc
{
  "plugins": [{
    "id": "example.hello",
    "name": "Hello",
    "version": "0.1.0",
    "description": "示例插件",
    "runtime": "node",
    "enabled": true,
    "path": "C:\\Users\\...\\extensions\\example.hello",
    "error": null,
    "commands": [{ "command": "example.hello", "title": "Hello" }],
    "mcpServers": ["filesystem"],
    "languageServers": ["typescript"]
  }],
  "dir": "C:\\Users\\...\\extensions",
  "nodeAvailable": true
}
```

事件：`lsp:diagnostics`，载荷
`{ serverId, path, diagnostics: [{ severity, message, line, column, endLine, endColumn }] }`。
行列从 1 开始；severity 用 LSP 的 1–4。

## 4. Node 扩展宿主协议

Rust 写入内置 `ext-host.mjs` 到数据目录（或 `LC_EXT_HOST` 指向源文件），启动：

```text
node ext-host.mjs
```

stdin/stdout 每行一个 JSON-RPC 2.0 消息（UTF-8，单行）。

- Rust → host：
  - `initialize` `{ workspaceRoot, plugins: [{ id, path, main, commands }] }`
  - `command/execute` `{ pluginId, command, args }`
  - `shutdown` `{}`
- host → Rust：只使用 JSON-RPC response；v0 不反向调用 Rust。
- 插件模块导出 `activate(ctx)` / `deactivate()`；ctx 提供：
  - `commands.register(id, handler)`
  - `workspace.root`、`workspace.readFile(relPath)`（限制在 workspace 内）
  - `storage.get(key)` / `storage.set(key, value)`（宿主内存 + 插件级 JSON）
  - `window.showMessage(text)`（v0 记录日志）
  - `log(...)`
- v0 启动时加载所有启用插件；`activationEvents` 仅记录，延迟激活后续实现。
- 单个插件加载失败不阻止其它插件；错误写入 `initialize` 的 `errors`。

## 5. WASM 运行协议

```text
node wasm-host.mjs --wasm <plugin.wasm> --command <id> --args <json> --workspace <root>
```

- 模块按 WASI preview1 command 执行；stdin 不传数据，stdout 必须是单个 JSON 值；
  stderr 作为错误详情。
- 无 preopen、无网络、无宿主环境变量；超时由 Rust 侧控制。
- 插件通过命令行参数 `/ argv` 获取 `command`、`args`、`workspace`；输出 JSON 结果。
- 示例：Rust/C/Go 编译到 `wasm32-wasip1`，`_start` 读取 argv，写 stdout。

## 6. MCP / LSP

- `mcpServers` 与 `.mcp.json` 同格式；Rust 侧合并时把名称改为
  `plugin.<pluginId>.<name>`，避免与用户配置冲突；项目级插件默认不加载。
- `languageServers` 由 LSP lane 启动；v0 只做：
  `initialize` / `initialized` / `textDocument/didOpen` / `didChange` / `didClose` /
  `publishDiagnostics`。补全、悬停、跳转后续接入。
- LSP 服务器按插件启用状态启动；stdio 子进程不继承 provider 密钥。

## 7. 安全边界

- 插件代码永远不进入 Tauri 渲染进程；Node 插件在独立扩展宿主进程中执行。
- 清单路径必须落在插件目录内；项目级插件默认关闭。
- Node 插件是**可信代码**（与 VS Code 相同），`permissions` 只用于安装提示；
  WASM 才是强隔离层。
- 扩展宿主与 LSP/WASM 子进程使用 `scrub_secret_env` 风格的环境清理；不自动注入
  `LC_API_KEY`/`OPENAI_API_KEY`。
- v0 不支持插件市场、签名、自动更新；只支持本地目录安装。

## 8. v0 范围

已实现：清单发现/校验、启用禁用、Node 命令、WASM 命令、MCP 声明、LSP 诊断、
插件管理页、命令面板入口。

未实现：VS Code API 兼容、插件市场、Webview UI、树视图、插件级 Agent 工具、
延迟激活、插件签名、插件依赖、主题/语言声明。
