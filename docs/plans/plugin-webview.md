# 插件 Webview UI 实施设计

> 状态：设计定稿，待执行（插件系统 v0 收官基线，docs/EXTENSIONS.md §7/§8；
> 市场设计见 docs/plans/plugin-marketplace.md，章节结构与其一致）。
> 范围：插件 UI v1——数据驱动的声明式视图（结构在清单、动态数据走扩展宿主、
> 宿主组件渲染）；沙箱 iframe 作为 v2 可选路径仅在本文规格化（§3.8），不随 v1 实施。
> 前提：树视图（EXTENSIONS.md §4.2）已验证「清单声明 → provider 数据 →
> 双端白名单归一 → 宿主组件渲染 → 命令回路」全链路，本设计是其直接延伸。

## 0. 结论（先行）

- **推荐方案：C（数据驱动 UI，无插件 HTML）**。插件在 `contributes.ui.views` 中只声明
  结构化节数据（`markdown | form | list | kv`），宿主用既有组件渲染；动态数据经新宿主
  协议 `ui/values`（`view/children` 的同构扩展）回读，交互回传复用既有命令回路
  （`ext_execute_command`）。渲染进程收到的插件产物只有「Rust 校验过的清单 JSON +
  宿主归一过的值 JSON」，插件代码（HTML/JS/CSS）零字节进入主 webview——
  EXTENSIONS.md §7 红线字面与精神同时满足，且全链路零新增重型依赖。
- **最大风险**：
  1. **表达力天花板**：数据驱动覆盖设置表单/列表/键值/说明文档（编辑器插件 UI 的
     绝对主流），但做不了富交互（canvas、拖拽、三方组件库）。若 M1/M2 后此类需求
     高频出现，v2 的 iframe 路径（§3.8）必须立项——而它需要修订 §7 红线字面，是
     政治成本最高的一步；
  2. **XSS/注入面**：markdown 节是渲染层唯一 `v-html` 面，复用
     `apps/web/src/services/markdown.ts` 的「先转义全部文本、再插入固定标签」契约；
     渲染实现若在任何回退路径上引入第二个 v-html 点、或渲染插件提供的
     URL/图片，就会打开 `javascript:` 与数据外泄面——用渲染层测试把守；
  3. **命令/bridge 越权**：表单提交与行点击全部走插件命令面，安全前提是
     「命令必须在该插件清单 `contributes.commands` 中声明」（tools.command 闭包
     校验 `crates/lc-core/src/ext/manifest.rs:638-653` 同款）；M3 的 postMessage
     bridge 白名单若漂移，沙箱内代码可借 bridge 触发越权动作——两处都必须双端测试。

## 1. 现状基线

| 层 | 位置 | 可复用度 |
|---|---|---|
| 清单校验 | `crates/lc-core/src/ext/manifest.rs`：`Contributes` 结构 `:62-84`、各栏手写校验（views `:767-785`、themes `:659-695`）、`MAX_MANIFEST_BYTES` 256 KiB `:23` | `contributes.ui` 校验直接加在 `validate_manifest`，风格照抄；失败只标该插件 `error` |
| 声明→DTO 下发 | `PluginInfo`/`PluginList`（`crates/lc-core/src/ext/mod.rs`，经 `ext_list` 全量下发） | `PluginInfo` 增 `uiViews`（含 sections），与 themes/languages/views 同生命周期 |
| 树视图全链路 | EXTENSIONS.md §4.2；`sidecars/ext-host/src/host.mjs` `view/children` `:569-652`、`normalizeViewNodes` `:193-215`；`sidecars/ext-host/src/views.mjs` 注册表（注册范围受限 `:31-37`） | 本设计的直接模板：`ui/values` 照抄其派发/超时/错误 reason 全集 |
| 双端白名单归一 | `apps/web/src/services/pluginViews.ts` `normalizeViewNode` `:79-93`（多余键一律丢弃）；`apps/web/src/services/extensions.ts` DTO 守卫 `isPluginView` `:150-152`、`isPluginInfo` `:162-184` | `isUiSection`/`normalizeUiValues` 同风格 |
| 命令执行回路 | `apps/desktop/src-tauri/src/ext.rs` `ext_execute_command` `:152-172`（全局 command→plugin 映射，声明期已校验归属）；palette 注入 `apps/web/src/App.vue:515-520` | 表单提交/行点击复用；`openCommand` 别名在 palette 层拦截 |
| 安全 markdown | `apps/web/src/services/markdown.ts`（`:1-72`，先转义后插标签，无第三方解析器，测试在 `markdown.spec.ts`） | markdown 节渲染直接复用；v1 子集无链接语法 → 零 `<a>` 面 |
| 标签页容器先例 | `ExtensionsPanel` 作编辑器区标签 `apps/web/src/App.vue:1402-1406`（`__extensions__`） | UI 视图落点照抄：`__ui__:<pluginId>:<viewId>` 标签 |
| 宿主协议 | `host.mjs` ctx `:282-294`、30s 超时 `VIEW_TIMEOUT_MS` `:16-19`（env 注入短超时供测试）；Rust `ExtHost` `crates/lc-core/src/ext/host.rs` `VIEW_TIMEOUT` `:34` | 新增 `ctx.ui.register`、`ui/values`、`pendingUiViews` 映射 |
| Tauri 命令注册 | `apps/desktop/src-tauri/src/lib.rs:820-834`；薄转发模式 `ext.rs:204-227`（`ext_view_children`） | `ext_ui_values` 照抄；ohos shim 不接线（现状 ext 命令亦未转发） |
| CSP | `apps/desktop/src-tauri/tauri.conf.json:24`（现值含 `frame-src http: https:`） | v1 不动；M3 iframe 需增 `frame-src lc-plugin:` |
| 安全红线 | EXTENSIONS.md §7（`:357-364`）：插件代码永不进渲染进程、Node 插件可信、WASM 强隔离、`scrub_secret_env` | 本设计不削弱任何一条，§4 显式论证 |
| WebView 成本 | `docs/ARCHITECTURE.md` §6（销毁 WebView 释放 100MB+） | 否决方案 B 的依据之一 |

## 2. 方案对比

### A. 主 webview 内嵌 sandbox iframe

- **做法**：编辑器区嵌入 `<iframe sandbox="allow-scripts" src="lc-plugin://<pluginId>/<entry>">`，资源走手注册 custom protocol，iframe 与父文档之间仅经白名单 postMessage bridge（`getState/setState/postCommand`）通信（完整规格见 §3.8）。
- **优点**：表达力=完整 Web 平台，与 VS Code webview 心智对齐；bridge 收敛后能力面可控；UX 嵌入式。
- **缺点与红线妥协度**：**字面违反 §7**——`allow-scripts` iframe 的插件 JS 在主 webview 的渲染进程内执行（WebView2 默认不启 site-per-process，与主文档同进程；WebKitGTK 无同等隔离）。sandbox + opaque origin 确实阻断其触碰 `window.__TAURI_INTERNALS__`、父 DOM、存储与网络——**精神（能力隔离）基本满足**；但插件控制的 HTML/JS 接进了渲染器的 JS 引擎/解码器 exploit 面，且「代码进渲染进程」这一事实本身破了字面。若做，需修订 §7、安装确认 UI 明示、CSP/协议/桥三层把守，测试面三者中最大。

### B. 每插件独立 webview 窗口

- **做法**：`WebviewWindow` 每插件一个原生窗口（或 Tauri v2 单窗多 webview 嵌入变体）。
- **优点**：真隔离（独立 webview、独立存储分区），主 webview 攻击面零增加。
- **缺点**：UX 是弹窗式而非嵌入式，编辑器布局（dockview/标签系统）无承载点；窗口管理成本（焦点/z-order/DPI/多显示器/最小化）；每 webview 重量级（ARCHITECTURE.md §6，100MB+）；嵌入变体跨平台一致性最差——WebKitGTK 对一窗多 webview 的支持与 WebView2 不对等，鸿蒙 ArkWeb 更无此能力，三端矩阵里唯一让 ohos 彻底出局的路径。
- **否决**（v1/v2 均不做）。

### C. 数据驱动 UI（推荐）

- **做法**：插件只声明结构化节数据（§3.1）；宿主 Vue 组件渲染（§3.4）；动态数据经 `ui/values` 协议回读（§3.2）；交互走既有命令回路（§3.5）。
- **优点**：**红线零妥协**（字面+精神：渲染进程收到的只有 Rust 校验过的声明 JSON 与宿主归一过的值 JSON，与树视图「数据可以进、代码永不进」同一边界）；攻击面趋零；零新增依赖；三端可移植（数据面不依赖 webview 特性，ohos 后续可接）；主题一致性 by construction（无插件 CSS，全走 `var(--*)` token）。
- **缺点**：表达力受限（无自定义布局/富交互）；宿主组件维护成本（每新增节类型=一个受控组件）；复杂 UI 会撑清单（`ui` 块 ≤32 KiB 上限兜住）。
- **简化变体 C1（拒绝）**：数据回读也复用 `ext_execute_command`（节声明 `valuesCommand`），不新增 `ui/values`。拒绝理由：命令结果任意 JSON、无宿主侧形状校验；120s 命令超时与 UI 数据 30s 语义不符；用户命令与数据回读生命周期混同。最终划分：**提交/行点击=命令（复用），数据回读=独立协议（新增）**。

**结论：采用 C 为 v1 主线；A 规格化但仅在 v2、§7 修订后启用（§3.8）；B 否决并记录。**

## 3. 推荐方案详细设计

### 3.1 声明形状与校验（`contributes.ui.views`）

```jsonc
"contributes": {
  "ui": {
    "views": [
      {
        "id": "task-board",                  // 视图 id：小写字母/数字/线，插件内唯一（同 views.id 规则）
        "name": "任务看板",                   // 标签页标题（非空，trim 后）
        "openCommand": "demo.tasks.board",   // 可空；命令 id 必须出现在 contributes.commands
        "sections": [                        // ≤16 个；整棵 ui 块序列化 ≤32 KiB
          { "kind": "markdown", "text": "# 说明\n……" },              // text ≤16 KiB；静态
          {
            "kind": "form", "title": "筛选", "command": "demo.tasks.filter",
            "fields": [                                                // ≤32 个
              { "key": "q",     "label": "关键词", "type": "text", "placeholder": "……" },
              { "key": "limit", "label": "条数",   "type": "number", "min": 1, "max": 200, "default": 50 },
              { "key": "done",  "label": "含已完成", "type": "boolean", "default": false },
              { "key": "sort",  "label": "排序",   "type": "select", "options": ["new", "old"], "default": "new" },
              { "key": "token", "label": "令牌",   "type": "password" }
            ]
          },
          {
            "kind": "list", "title": "任务", "valuesCommand": "demo.tasks.list",
            "columns": [                                               // ≤8 列；width=1..5 权重
              { "key": "title", "label": "标题", "width": 3 },
              { "key": "state", "label": "状态", "width": 1 }
            ],
            "rowCommand": "demo.tasks.open"                            // 可空；点击行 → 命令，args=行对象
          },
          { "kind": "kv", "title": "统计", "valuesCommand": "demo.tasks.stats" }
        ]
      }
    ]
  }
}
```

- **节类型是闭集**（`markdown|form|list|kv`），未知 kind 在校验期拒绝；字段类型 `text|number|boolean|select|password` 同为闭集，`default` 必须是该类型的合法 JSON 值；`command`/`valuesCommand`/`rowCommand`/`openCommand` 一律做「出现在 `contributes.commands`」闭包校验（tools.command 模式，`manifest.rs:638-653`）。
- **校验实现**：`manifest.rs` 新增 `validate_ui`（风格同 views `:767-785`）；`activationEvents` 校验白名单增 `onUiView:<viewId>`（`:543-555` 扩展）。任一失败只把该插件标记 `error`，不影响其它插件（既有全局语义）。
- **下发**：sections 随 `PluginList` 下发（`PluginInfo` 增 `uiViews`，camelCase，无 lc-proto 契约 bump），不新增拉取命令；体积由 32 KiB `ui` 块上限兜住。

### 3.2 数据回读协议：`ui/values`（`view/children` 同协议扩展）

- **宿主注册**：`ctx.ui.register(viewId, provider)`，provider = `{ getValues(sectionId): Value | Promise<Value> }`；注册范围受限清单声明（照抄 `views.mjs:31-37`）。
- **JSON-RPC 方法**：`ui/values { pluginId, viewId, sectionId }` → result 为按节 kind 白名单归一的 JSON（宿主侧 `normalizeUiValues`，照抄 `normalizeViewNodes` `host.mjs:193-215`）：
  - form 节：`{ "values": { [fieldKey]: json } }`（动态初始值；缺省字段回落 `default`）
  - list 节：`{ "rows": [ { [columnKey]: 标量 } ] }`（≤500 行，单元格仅 string/number/boolean）
  - kv 节：`{ "items": [ { "key": string, "value": string } ] }`（≤64 对）
  - markdown 节：无回读（纯静态）
- **超时与错误语义完全照抄** `view/children`（EXTENSIONS.md §4.2）：默认 30s（`LC_EXT_HOST_VIEW_TIMEOUT_MS` 注入短超时供测试）；`-32601` reason ∈ `plugin-not-loaded`/`view-not-declared`/`view-not-registered`；`-32000` reason ∈ `provider-timeout`/`invalid-values`。
- **懒激活**：`onUiView:<viewId>` 为隐式激活事件，宿主 `pendingUiViews` 映射命中先懒激活（幂等、失败 `-32601 activation-failed`，语义与 `onView:` 一致）；`initialize` 载荷 `HostPlugin` 增 `uiViews: [viewId]`（`host.rs:47-63`）。
- **runtime 约束**：任何 runtime 均可声明（同 views 策略），但 v1 只在 node runtime 分发数据——declarative 插件的 markdown 节静态渲染，form 静态 default 可渲染、提交需 node 时节内注记，list/kv 节显示「需要 Node 运行时」。

### 3.3 命令面（Tauri custom DTO，薄转发）

| 命令 | 签名 | 说明 |
|---|---|---|
| `ext_ui_values` | `(plugin_id, view_id, section_id) -> { values: Value }` | 照抄 `ext_view_children`（`ext.rs:204-227`）：视图必须已声明、插件启用且 node runtime，否则 `not_found`；经 `ExtHost::ui_values` 转发（30s，照 `host.rs` `VIEW_TIMEOUT`）；鸿蒙 shim 未转发时 web 按 reject ErrorBody 走节错误态 |

注册点 `apps/desktop/src-tauri/src/lib.rs:820-834` 区域；错误码复用既有 `ErrorCode`，不新增。

### 3.4 渲染设计

- **落点：编辑器区标签页**。标签 id `__ui__:<pluginId>:<viewId>`、标题=视图 `name`（先例：`ExtensionsPanel` 的 `__extensions__` 标签，`App.vue:1402-1406`）。否决右侧功能栏新 tab（现有 agent/workbench/git/db 已满、宽度窄）与设置页分区（设置页是应用配置语义，非插件运行时面板）。
- **入口**：① 命令面板「插件视图」分组（`App.vue:515-520` 的 `pluginCommands` 旁，label=视图 name、detail=插件名）；② `ExtensionsPanel` 插件卡片「打开视图」按钮；③ `openCommand` 别名——palette 中该命令的 action 改为打开标签页而非 `ext_execute_command`（**纯 web 侧拦截，宿主 handler 不执行**；插件若需打开时初始化数据，应放在 `ui/values` provider 中，由首次回读触发懒激活）。
- **组件**：`apps/web/src/components/PluginUiView.vue` + `apps/web/src/services/pluginUi.ts`（镜像 `PluginViews.vue`/`pluginViews.ts` 分层：服务层管状态/归一/错误态，组件层纯渲染）。
- **注入防御（渲染层红线）**：
  - 所有节文本/值一律 `{{ }}` 插值渲染；全组件**唯一** `v-html` 点是 markdown 节，且必须走 `services/markdown.ts` 的 `renderMarkdown`（先转义后插固定标签）；v1 markdown 子集**不含链接语法** → 零 `<a>`/`javascript:` 面。
  - 不渲染插件提供的任何 URL/图片（`icon` 仅 emoji 短文本，≤4 字符）；不注入插件 CSS——主题一致性 by construction，全部继承 `var(--bg)/var(--fg)/var(--accent)` 等 token。
  - web 侧收到 `PluginList`/`values` 一律过守卫归一（`isUiSection`/`normalizeUiValues`，多余键丢弃、形状非法→节错误态），不信任 IPC 对端（同 `extensions.ts` 守卫风格）。

### 3.5 交互回传

- **表单提交** → `ext_execute_command(section.command, { values })`（values=按 field.type 强制转换后的对象；password 值不进日志）。ok → 节内成功注记 + 可选触发回读刷新；error → 错误注记并保留用户输入。
- **list 行点击** → `rowCommand(row)`（整行对象作 args，单元格为归一标量）。
- **kv** 只读。**所有交互失败** → 节级错误态可重试（同树视图错误态，`PluginViews.vue:100-115`）。
- **打开视图本身零宿主往返**（入口全是 web 侧动作），首次数据回读才懒激活插件。

### 3.6 生命周期

App 启动与插件增删后 `syncPluginUiViews()`（照抄 `syncPluginViews` 挂点 `App.vue:476`/`:987`）；插件禁用/删除 → 其视图标签关闭、状态清理；`ui` 块声明变化 → 标签内容热更新（保留未受影响节的输入态）。M1 起 UI 标签不跨会话持久化（重启即关，列为假设）。

### 3.7 代码落点

- `crates/lc-core/src/ext/manifest.rs`：`Contributes` 增 `ui`；`validate_ui`（kind 闭集、命令闭包、规模上限、32 KiB `ui` 块）；`onUiView` 事件校验；`crates/lc-core/src/ext/mod.rs` 的 `PluginInfo` 增 `uiViews`。
- `crates/lc-core/src/ext/host.rs`：`HostPlugin` 增 `ui_views`；`ExtHost::ui_values` 转发（照 `view_children`）。
- `sidecars/ext-host/src/ui.mjs`（新，照 `views.mjs`）+ `host.mjs`：`ctx.ui`、dispatch 增 `ui/values`、`normalizeUiValues`、`pendingUiViews` 懒激活。
- `apps/desktop/src-tauri/src/ext.rs`：`ext_ui_values` 薄转发；`lib.rs` 注册。
- `apps/web`：`services/extensions.ts` 守卫增 `isUiView`/`isUiSection`；`services/pluginUi.ts`（新）；`components/PluginUiView.vue`；`App.vue` 标签接线 + palette 分组 + `openCommand` 别名拦截；`ExtensionsPanel.vue` 打开按钮；`locales/zh-CN.json`/`en.json`。
- 测试：`crates/lc-core/src/ext/tests.rs` 增清单校验用例；`sidecars/ext-host/test/host.test.mjs` 增 `ui/values` 用例；`apps/web/src/services/pluginUi.spec.ts`（新）。

### 3.8 iframe 沙箱路径（v2 可选，M3；以 §4 红线修订为前提）

- **声明**：`contributes.ui.webviews: [{ id, entry, assets? }]`——entry/assets 必须落插件目录内且已存在（canonicalize 模式照 `themes.path`，`manifest.rs:676-695`）。
- **sandbox 精确值**：`sandbox="allow-scripts"`——**不带 `allow-same-origin`**：opaque origin、无 localStorage/IndexedDB、禁父文档访问。Vue 中 `:sandbox` 绑定，测试断言属性精确值。
- **custom protocol**：`lc-plugin://<pluginId>/<asset>` 经 `tauri::Builder::register_uri_scheme_protocol` 手注册（桌面壳现无 custom protocol 先例，为新增）。handler 规则：authority=插件 id 必须启用且声明过 webview；path 规范化后 `ensure_within_root` 插件目录（拒 `..`/绝对/盘符前缀）；Content-Type 白名单（`text/html|text/css|application/javascript|image/png|jpeg|gif|webp|font/woff2`）；一律 `X-Content-Type-Options: nosniff`；目录列表与非 GET 拒绝；html 响应带 CSP 头。
- **CSP**：主 webview `tauri.conf.json:24` 的 `frame-src` 增 `lc-plugin:`（现值仅 `http: https:`，不放行即被拦）；iframe 文档 CSP：`default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'none'; base-uri 'none'; form-action 'none'`（`'self'`=该插件的 lc-plugin opaque origin）。
- **postMessage bridge（父侧持有 `PluginWebviewBridge` 服务）**：iframe 先握手 `{ source:"lc-plugin", pluginId, protocol:1 }`；op 白名单仅四个——`ready` / `getState(key)` / `setState(key,value)` / `postCommand(command,args)`；state 存 Rust 侧 per-plugin JSON（键 ≤64 字符、单值 ≤64 KiB、每插件 ≤256 KiB、用户数据目录持久化，**不用 webview storage**）；`postCommand` 校验命令 ∈ 该插件清单声明后经 `shared_host`（`ext.rs:66`）转发；**无 fs/network/env/凭据直通**；响应帧 `{ source:"lc-host", seq, ok, ... }`；非白名单 op 静默丢弃 + warn；origin 严格校验 `event.origin === "lc-plugin://<pluginId>"`。通信方向：iframe→父单向发起，父→iframe 仅按 seq 应答。

## 4. 与 EXTENSIONS.md §7 红线的关系

- **方案 C 字面满足，且是树视图边界的原样延伸**：渲染进程收到的插件产物只有三类——① Rust 校验过的清单 JSON（节结构）；② 宿主 `normalizeUiValues` 归一过的值 JSON；③ 经 `renderMarkdown` 转义后的文本。插件可执行代码仍只在扩展宿主（Node）/WASM/LSP 子进程中执行，进程模型零变化。「数据可以进、代码永不进」就是树视图已建立、本文沿用的边界。
- **方案 A 精神满足、字面违反**：sandbox（无 `allow-same-origin`）+ opaque origin + custom protocol + 白名单 bridge 四层叠加后，插件代码拿不到 `__TAURI_INTERNALS__`、父 DOM、存储与网络，能力隔离在工程上成立；但代码物理运行在渲染进程内（WebView2 默认同进程），字面即「插件代码永远不进入 Tauri 渲染进程」不成立。
- **若 v2 启用 A，§7 建议改写为**：「插件可执行代码永不进入主 webview 的**特权上下文**；插件 UI 资源仅可经 `lc-plugin` 协议在 sandbox iframe（opaque origin、`allow-scripts` only）中渲染，且仅能经白名单 postMessage bridge（`ready`/`getState`/`setState`/`postCommand`）通信。」同时保留数据驱动为默认渲染方式，iframe 为清单显式 opt-in + 安装确认 UI 固定文案明示。
- M1/M2（本文切片）**不触发** §7 任何修订；B 方案因真隔离反而满足字面，但 UX/平台成本否决（§2）。

## 5. 测试策略

- **清单校验**（`crates/lc-core/src/ext/tests.rs` 风格）：各 kind 合法/缺字段/kind 枚举外/field type 闭集外；`command`/`valuesCommand`/`rowCommand`/`openCommand` 引用未声明命令；规模超限（sections>16、fields>32、columns>8、text>16 KiB、`ui` 块>32 KiB）；`onUiView` 合法与非法事件；任一失败只标 `error` 不毒化列表。
- **ext_ui_values**：视图未声明/插件禁用/wasm runtime → `not_found`；宿主 JSON-RPC 错误原样映射。
- **宿主**（`sidecars/ext-host/test/host.test.mjs`）：`ui/values` 派发与 reason 全集（`plugin-not-loaded`/`view-not-declared`/`view-not-registered`/`provider-timeout`/`invalid-values`）；`onUiView` 懒激活（幂等、失败不重试）；`LC_EXT_HOST_VIEW_TIMEOUT_MS` 注入短超时验证超时路径；`ctx.ui.register` 注册范围受限（未声明 id 抛错）。
- **web**（`apps/web/src/services/pluginUi.spec.ts`，仿 `pluginViews.spec.ts`）：`isUiSection`/`normalizeUiValues` 纯函数（含 `<script>`/`javascript:` 载荷、多余键丢弃、形状非法→null）；store 同步/错误态/重试；表单提交 args 组装；**`openCommand` 别名断言零 `invoke` 调用**（纯 web 拦截）；markdown 节 `<script>` 转义断言（复用 `markdown.spec.ts` 模式）；demo 模式 fixtures（照 `extensions.ts` DEMO 段）。
- **M3 iframe**：protocol handler 穿越（`../evil`/绝对/盘符）、未声明资源、Content-Type 白名单外 → 404/405；sandbox 属性精确值断言；bridge 非白名单 op、错误 origin、超尺寸 state 拒绝；CSP 响应头断言。
- 全部网络用例走 loopback/file 或无网，CI 离线可跑（同市场约束）。

## 6. 里程碑切片（每片独立可发布）

| 切片 | 内容 | 估点 | 独立发布条件 |
|---|---|---|---|
| **M1 声明+渲染** | `contributes.ui` 校验 + `onUiView` 事件；`PluginInfo.uiViews` 下发；`pluginUi.ts` + `PluginUiView.vue`；编辑器区标签容器；palette 分组 + `ExtensionsPanel` 按钮 + `openCommand` 别名；markdown/kv/form 静态渲染 | 4 | declarative 插件静态视图端到端可打开；非法清单只标 `error`；插件管理页与树视图零回归 |
| **M2 交互回传** | `ctx.ui.register` + `ui/values` 协议 + `normalizeUiValues` + `pendingUiViews` 懒激活；`ExtHost::ui_values` + `ext_ui_values`；表单提交/行点击/回读刷新；节错误态可重试 | 3 | node 插件表单提交到 handler 并回显结果；30s 超时与非法值错误态可重试；懒激活语义与 `onView:` 一致 |
| **M3（可选，v2）iframe 沙箱** | `lc-plugin` protocol handler；`PluginWebviewBridge`；sandbox 组件 + CSP；§7 修订 + 安装确认文案 | 5 | 穿越/未声明资源 404；非白名单 bridge 消息拒；安全评审通过 |

估点单位：1 点 ≈ 一个 agent 半天；含测试与文档（同市场口径）。

## 7. 风险表

| 风险 | 等级 | 缓解 |
|---|---|---|
| 红线妥协（M3 iframe 字面违反 §7） | **高**（仅当做 M3） | §4 修订先行 + 数据驱动保持默认 + iframe 显式 opt-in + 安装确认固定文案；M1/M2 不触发修订 |
| markdown XSS/渲染层注入蔓延 | 中 | 唯一 v-html 点强制 `renderMarkdown`；v1 子集无链接；渲染层测试禁止第二 v-html 点/URL 渲染回退 |
| 命令/bridge 越权 | 中 | 命令归属声明期闭包校验；bridge op 四件白名单 + origin 校验双端测试；bridge 无 fs/network/env 直通 |
| 表达力天花板引发的需求外溢 | 中 | M1 后收集真实需求；富交互需求集中出现再评估 M3；期间引导插件用「视图+命令+工具」组合表达 |
| 主题一致性 | 低 | by construction：无插件 CSS，全 token；节组件继承设置页/树视图样式基线 |
| 清单膨胀拖慢 ext_list | 低 | `ui` 块 ≤32 KiB、sections ≤16/view 上限；声明非法 fail-closed |
| ohos 功能缺口 | 低 | 桌面先行（同市场策略）；数据面不依赖 webview 特性，shim 补转发即可接 |
| 每插件一窗口的性能/UX（方案 B） | —（已否决） | 决策记录于 §2；若未来 reconsider，需先解决嵌入布局与三端 webview 一致性 |

## 8. 假设（已自行裁定，待主 agent 确认）

- 节结构**静态声明**（清单内定死），运行时增删节/动态结构不在 v1，出现真实需求随 M3 一并评估。
- **不引入 JSON Schema 校验依赖**：清单校验沿用 Rust 手写校验器风格（同 themes/languages/views），「防注入」由「kind 闭集 + 命令闭包 + 双端白名单归一」承担，零新增依赖。
- sections 随 `PluginList` 全量下发（不新增拉取命令）；体积由 32 KiB `ui` 块上限约束，实测超标再拆 `ext_ui_view`。
- `ui/values` 数据面 v1 仅 node runtime（同视图 v1 策略）；declarative 插件仅静态节渲染。
- UI 标签 M1 起不跨会话持久化（重启即关）；需要持久化时在 tabs 体系内另立小切片。
- M3 是否立项取决于 M1/M2 交付后的真实需求密度，且以 §4 的 §7 修订合入为硬前提；默认不做。
- `lc-ohos` 不接线（同市场计划），`PluginUiView` 的 ohos 降级=入口隐藏。
