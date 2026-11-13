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
  "extensionDependencies": ["example.dep"],  // 可空；依赖的其它插件 id，宿主保证依赖先加载
  "contributes": {
    "commands": [
      { "command": "example.hello", "title": "Hello from plugin" }
    ],
    "tools": [
      {
        "name": "example.search",            // Agent 可见的工具名（小写字母/数字/点/线，插件内唯一）
        "description": "搜索工作区文本",      // 给 LLM 的工具描述（非空）
        "parameters": {                      // JSON Schema（object 类型），原样进 LLM 工具定义
          "type": "object",
          "properties": { "query": { "type": "string" } },
          "required": ["query"]
        },
        "command": "example.hello"           // 映射到插件内注册的命令 handler；缺省与 name 一致
      }
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
    ],
    "themes": [
      {
        "label": "Dusk",                  // 设置页展示名（非空、插件内唯一）
        "path": "./themes/dusk.json",     // 插件目录内的主题 JSON（读取时限 64 KiB）
        "uiTheme": "dark"                 // 基色：dark | light
      }
    ],
    "languages": [
      {
        "id": "foo-lang",                 // Monaco language id（小写字母/数字/线，插件内唯一）
        "extensions": [".foo"],           // 带点扩展名（插件间去重，冲突→本插件标记 error）
        "aliases": ["Foo"],               // 可空；展示别名
        "grammar": "./grammars/foo.json"  // 可空；插件目录内的语法 JSON（读取时限 64 KiB）
      }
    ],
    "views": [
      {
        "id": "example-tree",             // 视图 id（小写字母/数字/线，插件内唯一）
        "name": "示例视图"                 // 资源管理器侧栏分区的展示名（非空）
      }
    ]
  },
  "permissions": ["workspace.read"]    // 提示用；Node 插件本质是可信代码
}
```

主题 JSON 形状（定死）：

```jsonc
{
  "name": "Dusk",
  "colors": { "--bg": "#101014", "--fg": "#e6e9ef" }
}
```

`colors` 是 CSS 变量名→值映射（对应 `tokens.css` 的 `--bg`/`--fg`/`--accent` 等）。
UI 侧只白名单 `--` 前缀的合法变量名，其余键一律忽略（防注入其它规则）。应用时
注入/更新一个 `<style data-plugin-theme="<slug>">`，选择器
`:root[data-theme='<slug>']`；slug 规则 `plugin--<pluginId>--<label-slug>`
（label 转小写、非字母数字折叠为 `-`、空则回落 `theme`）。slug 作为 theme 值
持久化进 `lc.ui.prefs`；所属插件禁用/删除后回落 `dark` 并清理样式标签。
任何 runtime（含 declarative）均可声明主题。

语法 JSON 形状（Monarch 子集，原样转发，web 侧白名单）：

```jsonc
{
  "tokenizer": {
    "root": [["\\w+", "keyword"], ["\\s+", ""]]
  }
}
```

UI 侧只白名单 `tokenizer` 键且值必须是纯对象，其余键一律忽略（防注入其它
Monaco 语言定义字段）；形状不符按无高亮处理。语言经
`monaco.languages.register({id, extensions, aliases})` 注册，语法经
`setMonarchTokensProvider` 挂上。**内置优先**：扩展名已被内置映射
（`editorLanguage.ts`）覆盖时插件声明不生效；与内置相同的 language id 不重复
注册、也不覆盖内置高亮。Monaco 没有语言的 unregister，插件禁用/删除/语法读
失败时用空 tokenizer（`{ "tokenizer": {} }`）清高亮，语言注册与扩展名映射
随下一次同步撤掉。任何 runtime（含 declarative）均可声明语言。

树视图（v1）：`contributes.views` 声明的视图在资源管理器侧栏底部渲染为可折叠
分区（分区头=视图 `name`，标注来源插件；不新增活动栏容器）。节点数据由插件
activate 时 `ctx.views.register(viewId, provider)` 提供（§4）：provider 暴露
`getChildren(nodeId?)`，返回 `[{ id, label, icon?, command?, tooltip? }]`——
`id`/`label` 必填非空字符串，`command` 可空（空则节点纯展示），空数组合法
（叶子）。`onView:<viewId>` 是隐式激活事件：视图首次被请求且插件是延迟插件时
懒激活，语义与 `onCommand:` 一致（§4）。节点点击带 `command` 的项执行对应
插件命令；读失败/插件禁用 → 分区错误态可重试。

校验：`id` 唯一、`version` 非空、`runtime` 合法、`main` 必须落在插件目录内且存在
（declarative 除外）、命令 id 唯一、清单大小 ≤ 256 KiB。`activationEvents` 另校验：
每个事件必须是 `*` 或以 `onCommand:` / `onView:` 开头，非法事件只把该插件标记为
`error`（延迟激活语义见 §4）。`contributes.tools` 另校验：
`name` 合法（小写字母/数字/点/线、插件内唯一）、`description` 非空、`parameters`
必须是 object 类型 JSON Schema（带 `type` 字段时必须为 `"object"`）、`command`
必须出现在 `contributes.commands` 中或与 `name` 一致（缺省即 `name`）。
`contributes.themes` 另校验：`label` 非空且插件内唯一、`path` 必须落在插件
目录内且为已存在的 `.json`（复用 main 的 canonicalize 模式）、`uiTheme` 只能
是 `dark`/`light`；主题文件在 `ext_read_theme` 读取时限 64 KiB。
`contributes.languages` 另校验：`id` 合法（小写字母/数字/线）且插件内唯一、
`extensions` 必须是 `.ext` 形式（插件内去重并统一转小写）、`grammar` 必须落在
插件目录内且为已存在的 `.json`（复用 themes.path 的 canonicalize 模式，可空）；
扩展名插件间去重——先发现者占用，冲突插件整体标记 `error`，错误信息说明冲突
扩展名、语言 id 与冲突方插件；语法文件在 `ext_read_grammar` 读取时限 64 KiB。
`contributes.views` 另校验：`id` 合法（小写字母/数字/线，与语言 id 同规则）
且插件内唯一、`name` 非空（trim 后）；节点数据不在清单里，由插件 activate 时
注册的 provider 提供（§4）。任何 runtime（含 declarative）均可声明视图，
但 v1 只在扩展宿主（node runtime）分发视图数据（模式同工具）。
`extensionDependencies` 另校验：条目必须是合法插件 id 形态（小写字母/数字/
点/线），去重、拒绝自依赖；被依赖 id 必须存在于已发现插件中（被禁用但已
发现算存在），缺失只把声明方插件标记为 `error`，错误信息写明缺失 id。依赖
成环时环上插件整体标记 `error`（错误信息写明环的构成）且不参与宿主下发。
宿主按依赖对 initialize 插件列表做拓扑排序（依赖先于被依赖者，无依赖关系
的插件保持既有顺序），列表顺序即宿主 import+activate 顺序。依赖只约束
加载顺序：被依赖插件若是延迟激活（§4），不会因为被依赖而提前激活，v1 不
对此做特殊处理。
任一校验失败只把该插件标记为 `error`，不影响其它插件。

## 2. 运行形态

| runtime | 执行方式 | 适合 |
|---|---|---|
| `node` | 常驻 Node 扩展宿主（stdio JSON-RPC） | 命令、工作区读写、状态、生态复用 |
| `wasm` | 每次命令启动一个 WASI 模块（无文件/网络权限） | 不可信工具、算法、跨语言插件 |
| `declarative` | 不执行代码，只读取 `contributes` | 主题、语言、MCP/LSP 声明、命令占位 |

Node 扩展宿主由 Rust 侧按需启动；无 Node 时 node 插件禁用，declarative/MCP 插件仍可用。
WASM 运行器也依赖 Node，但只使用 `node:wasi`，不把 Node API 暴露给 wasm。

## 3. Rust 命令（Tauri，custom DTO）

```text
ext_list() -> PluginList
ext_set_enabled(id, enabled) -> PluginList
ext_reload() -> PluginList
ext_execute_command(command, args?) -> { result: any }
ext_read_theme(plugin_id, path) -> { json }
ext_read_grammar(plugin_id, path) -> { json }
ext_view_children(plugin_id, view_id, node_id?) -> { nodes: [{ id, label, icon?, command?, tooltip? }] }
ext_open_dir() -> ack
lsp_start_plugins() -> { servers: [{ id, languageIds }], errors: [] }
lsp_start_all(configs?) -> { servers, errors }
lsp_did_open(path, language_id, text) -> ack
lsp_did_change(path, text) -> ack
lsp_did_close(path) -> ack
lsp_stop_all() -> ack
lsp_completion(path, line, column) -> CompletionItem[]
lsp_hover(path, line, column) -> Hover
lsp_definition(path, line, column) -> Location[]
```

补全 / 悬停 / 跳转（`textDocument/completion|hover|definition`）：

- `line` / `column` 为 **1-based**（与 Monaco 及诊断 DTO 一致），Rust 侧换算成
  LSP 的 0-based UTF-16 position；请求超时 10s，超时按 LSP lane 既有语义
  终止子进程。
- 文档未 didOpen（含服务器未启动、文件未路由到任何 server）返回
  `not_found` 结构错误；server 返回 JSON-RPC error 原样映射为 `internal`
  结构错误；一律不 panic。
- `CompletionItem`：`{ label, kind?, detail?, insertText?, textEdit? }`；
  `kind` 为 LSP CompletionItemKind 1–25；`textEdit` 是 plain insert 子集
  `{ newText }`，存在时优先于 `insertText`，`insertText` 缺省用 `label`。
- `Hover`：`{ contents: [{ value, isMarkdown }] }`；contents 为空表示无悬停。
  MarkedString 的 `{language, value}` 形式由 Rust 包成围栏代码块。
- `Location`：`{ path, line, column }`（1-based）；空数组表示无定义。
  Location 与 LocationLink 统一拍平，link 取 `targetSelectionRange` 起点。
- 前端只在「该文件语言有已启动 server」时注册对应 provider（Monaco
  completion/hover/definition provider + `registerEditorOpener` 跨文件跳转
  落到标签系统），无 server 不注册，Monaco 保留内置建议。

`ext_read_theme`：读取插件贡献的主题 JSON（§1 形状）。`path` 必须是该插件
清单 `contributes.themes` 中声明过的相对路径——解析后落在插件目录外、未声明、
插件不存在或已禁用一律 `not_found`（防目录穿越）；文件超过 64 KiB 报
`invalid_argument`；非法 JSON 同样 `invalid_argument`。

`ext_read_grammar`：读取插件贡献的语言语法 JSON（§1 形状，Monarch 子集
原样转发，web 侧白名单校验）。`path` 必须是该插件清单
`contributes.languages` 中声明过的 `grammar` 相对路径——防穿越/未声明/
插件不存在或已禁用一律 `not_found`；超过 64 KiB 或非法 JSON 报
`invalid_argument`（语义与 `ext_read_theme` 完全一致）。鸿蒙 shim 未转发
该命令，web 侧按 reject 的 ErrorBody 走读失败分支（清空该语言高亮）。

`ext_view_children`：读取插件树视图的子节点（§1/§4）。`view_id` 必须是该
插件清单 `contributes.views` 中声明过的视图 id、`plugin_id` 对应插件启用且为
node runtime，否则一律 `not_found`；`node_id` 缺省拉根节点，否则拉该节点的
子节点。请求经扩展宿主 `view/children` 转发（默认超时 30s），返回节点数组
（可空）；宿主侧 JSON-RPC 错误（如视图未注册 `-32601`、provider 超时
`-32000`）原样映射为结构错误。鸿蒙 shim 未转发该命令，web 侧按 reject 的
ErrorBody 走分区错误态（可重试）。

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
    "languageServers": ["typescript"],
    "themes": [{ "label": "Dusk", "path": "./themes/dusk.json", "uiTheme": "dark" }],
    "languages": [{
      "id": "foo-lang",
      "extensions": [".foo"],
      "aliases": ["Foo"],
      "grammar": "./grammars/foo.json"
    }],
    "views": [{ "id": "example-tree", "name": "示例视图" }]
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
  - `initialize` `{ workspaceRoot, plugins: [{ id, path, main, commands, tools: [{ name, command }], views: [viewId], uiViews: [{ id, sections }], activationEvents }] }`——`activationEvents` 为清单原样的字符串数组；空数组（缺省字段）或含 `"*"` 表示立即激活，否则该插件延迟激活。`views` 为清单 `contributes.views` 的 id 列表。`uiViews` 为清单 `contributes.ui.views` 的完整节描述 DTO（markdown `{id,kind}`；form `{id,kind,fields:[{key,type,default?,options?,min?,max?}]}`；list `{id,kind,source:"provider",columns:[{key}]}`；kv `{id,kind,source?}`，见 `docs/plans/plugin-webview.md` §3.2）。
  - `command/execute` `{ pluginId, command, args }`
  - `tool/execute` `{ pluginId, tool, args }` → `{ result: <插件返回值> }`（默认超时 30s）
  - `view/children` `{ pluginId, viewId, nodeId? }` → 节点数组 `[{ id, label, icon?, command?, tooltip? }]`（空数组合法；默认超时 30s）
  - `ui/values` `{ pluginId, viewId, sectionId }` → 按节声明归一的值载荷（list→`{rows}`≤500、kv→`{items}`≤64；markdown/form 节无回读；`sectionId` 为清单稳定节 id——M2-R 起不再是数组序号——仅 `source:"provider"` 的 list/kv 节可回读，未知节与静态节在 provider 执行前拒绝；默认超时 30s；reason 全集与 `view/children` 同构，规格见 `docs/plans/plugin-webview.md` §3.2）
  - `shutdown` {}
- host → Rust：只使用 JSON-RPC response；v0 不反向调用 Rust。
- 插件模块导出 `activate(ctx)` / `deactivate()`；ctx 提供：
  - `commands.register(id, handler)`
  - `tools.register(name, handler)`（插件级 Agent 工具，见下）
  - `views.register(viewId, provider)`（树视图；provider = `{ getChildren(nodeId?): 节点数组 | Promise<节点数组> }`，节点形状见 §1）
  - `workspace.root`、`workspace.readFile(relPath)`（限制在 workspace 内）
  - `storage.get(key)` / `storage.set(key, value)`（宿主内存 + 插件级 JSON）
  - `window.showMessage(text)`（v0 记录日志）
  - `log(...)`
- 延迟激活（v1）：`activationEvents` 生效。无 `activationEvents`（宿主收到空数组）
  或含 `"*"` 的插件在 `initialize` 时立即 import + activate（向后兼容）；仅含
  `onCommand:<commandId>` / `onView:<viewId>` 的插件记为「延迟」——不 import、
  不 activate，宿主只登记其声明的 command/tool/view → pluginId 映射。
  `command/execute`、`tool/execute` 与 `view/children` 命中延迟插件的声明项时先
  懒激活（import + activate）再派发；v1 粒度为插件级，事件串里的 commandId/
  viewId 不参与匹配，任一声明的 command/tool/view 被调用即激活整个插件。
  `onView:<viewId>` 无需插件额外处理：视图首次被请求展开且插件是延迟插件时，
  宿主按 views 映射懒激活（隐式激活事件）。懒激活幂等：同一插件只激活一次
  （并发/重复调用共享同一次激活）；激活失败返回 JSON-RPC `-32601`，
  `data.reason = "activation-failed"`（另含 `stage`/`message`），
  失败状态记录到 stderr 且本会话不重试。未激活的延迟插件没有 deactivate；
  已激活插件的 deactivate 语义不变。延迟插件的加载错误不进 `initialize` 的
  `errors`（那时还没加载），经首次调用的 `-32601` 上报。
- `extensionDependencies` 只约束 initialize 列表顺序（§1），不改动激活时机：
  被依赖的延迟激活插件不会因此提前 import/activate，依赖方激活时不能假设
  被依赖插件已运行（v1 不特殊处理该交互）。
- 单个插件加载失败不阻止其它插件；错误写入 `initialize` 的 `errors`。

### 4.1 插件级 Agent 工具

`contributes.tools` 声明的工具注册进 AgentLoop 的 ToolRegistry，LLM 可直接调用：

- 派发顺序：`tool/execute` 先查 `ctx.tools.register(name, handler)` 注册的 handler；
  未注册时回退到清单 `command` 映射的命令 handler；都没有返回 JSON-RPC `-32601`。
- 延迟激活的插件（§4）其工具仍出现在 `plugin_tools()` 清单里——Agent 看得到、
  首次 `tool/execute` 命中时才懒激活执行。
- v0 仅 `node` runtime 插件可贡献工具（wasm/declarative 声明的工具不参与注册）；
  宿主未运行、插件禁用、Node 缺失或超时（默认 30s）时该工具不注册，静默降级，
  不报错阻塞会话。
- Rust 侧由 `lc_core::ext::{plugin_tools, ExtToolExecutor, HostToolRunner}` 桥接：
  每个工具包成 ToolExecutor，经共享的扩展宿主进程执行；插件工具一律
  `Risk::Mutating`（视为需审批），由 AgentLoop 既有策略层处理，审批队列无需改动。
- 输出按字符边界截断到 128 KiB（与 shell/MCP 一致），再包
  `[UNTRUSTED PLUGIN CONTENT …] [/UNTRUSTED]` 边界后回填给模型。
- 桌面端在 `session_create` 装配（执行器链由外到内：db → 插件 → MCP → 本地，
  按工具名路由分发）；鸿蒙端 plan 只读策略会拒绝 Mutating 工具，接入点见
  `lc-ohos` `read_only_registry` 的 TODO。

### 4.2 树视图节点协议

`view/children` 派发：先查 `ctx.views.register` 注册的 provider；未注册时按
views → pluginId 映射懒激活延迟插件后再查；仍没有返回 JSON-RPC `-32601`，
`data.reason` 区分 `plugin-not-loaded` / `view-not-declared`（清单未声明）/
`view-not-registered`（已激活但 activate 未注册 provider）。provider 可返回
Promise；宿主侧超时默认 30s（Rust 侧 `view_timeout` 另有 recv_timeout 兜底），
超时返回 `-32000`、`data.reason = "provider-timeout"`。节点形状非法（`id`/
`label` 缺字段或非字符串、返回非数组）返回 `-32000`、`data.reason =
"invalid-node"`；provider 抛错同 `-32000`。注册范围受限：插件只能注册清单
`contributes.views` 声明过的视图 id。

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
- `languageServers` 由 LSP lane 启动；已实现：
  `initialize` / `initialized` / `textDocument/didOpen` / `didChange` / `didClose` /
  `publishDiagnostics`，以及按路径请求的补全 / 悬停 / 跳转
  （`textDocument/completion` / `hover` / `definition`，见 §3）。
- LSP 服务器按插件启用状态启动；stdio 子进程不继承 provider 密钥。

## 7. 安全边界

- 插件可执行代码永不进入主 webview 的**特权上下文**；插件 UI 资源仅可经
  `lc-plugin` 协议在 sandbox iframe（opaque origin、`sandbox="allow-scripts"` only）
  中渲染，且仅能经白名单 postMessage bridge（`ready`/`getState`/`setState`/
  `postCommand`）通信（docs/plans/plugin-webview.md §3.8，M3：宿主生成 URL、
  handler 按 entry+显式 assets 白名单服务、bridge 状态为 Rust 侧 per-plugin
  JSON 并持久化、postCommand 校验命令属于绑定插件清单）。
- 数据驱动 UI（`contributes.ui.views` 声明式节）仍是默认渲染方式；iframe
  webview 为清单显式 opt-in（`contributes.ui.webviews`），安装确认 UI 以
  固定文案明示。
- 清单路径必须落在插件目录内；项目级插件默认关闭。
- Node 插件是**可信代码**（与 VS Code 相同），`permissions` 只用于安装提示；
  WASM 才是强隔离层。
- 扩展宿主与 LSP/WASM 子进程使用 `scrub_secret_env` 风格的环境清理；不自动注入
  `LC_API_KEY`/`OPENAI_API_KEY`。
- v0 不支持插件市场、签名、自动更新；只支持本地目录安装。

## 8. v0 范围

已实现：清单发现/校验、启用禁用、Node 命令、WASM 命令、MCP 声明、LSP 诊断、
LSP 补全/悬停/跳转、插件管理页、命令面板入口、插件级 Agent 工具（清单 contributes.tools、
`tool/execute` 协议、桌面端 AgentLoop 接线、一律 Risk::Mutating 走既有审批）、
延迟激活（`activationEvents` 的 `onCommand:` / `onView:` / `*` 三类事件，§4）、
主题声明（`contributes.themes` 校验、`ext_read_theme` 读取、web 设置页切换、
插件禁用/删除后回落 dark 并清理）、
语言声明（`contributes.languages` 校验与插件间扩展名去重、`ext_read_grammar`
读取、Monaco 语言注册与 Monarch 高亮、语法白名单防注入、声明消失/读失败降级
空 tokenizer；内置映射与内置语言 id 优先）、
树视图（`contributes.views` 校验、`ctx.views.register` 注册、`view/children`
协议与 30s 超时、`onView:` 隐式懒激活、`ext_view_children` 转发、web 资源管理器
侧栏底部可折叠分区渲染、节点命令执行、错误态可重试）、
插件依赖（`extensionDependencies` 校验：格式/去重/自依赖/缺失依赖、环依赖
整环报错并退出下发；initialize 插件列表按依赖拓扑排序，依赖先于被依赖者）、
webview UI（`contributes.ui.webviews` 校验、宿主生成 `lc-plugin` 资源 URL、
白名单 handler + Content-Type 闭集 + 子文档 CSP、per-plugin bridge 状态
`ext_webview_state_get/set`、postCommand 命令归属校验，§3.8/§7）。

未实现：VS Code API 兼容（v0 文档首行即不承诺）；插件市场仅缺发布端仓库
（客户端浏览/安装/签名校验/多源/镜像已实现，见
docs/plans/plugin-marketplace.md §3.7）。
