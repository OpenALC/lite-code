# 插件 Webview UI 实施设计

> 状态：M1/M2 已实现，2026-11-11 审查后修订；§3.1/§3.2/§3.6 的协议及
> 生命周期整改列为 **M2-R，待实施**。本文更新方案，不代表代码已完成整改。
> 实现记录见 docs/STATUS.md「Webview M1+M2 收口」，市场设计见
> docs/plans/plugin-marketplace.md；安全边界仍以 docs/EXTENSIONS.md §7 为准。
> 范围：插件 UI v1——数据驱动的声明式视图（结构在清单、动态数据走扩展宿主、
> 宿主组件渲染）；沙箱 iframe 为 v2 可选设计（§3.8），尚未实现或跨平台验证，
> 不随 v1 启用。
> 前提：树视图（EXTENSIONS.md §4.2）已验证「清单声明 → provider 数据 →
> 双端白名单归一 → 宿主组件渲染 → 命令回路」全链路，本设计是其直接延伸。

## 0. 结论（先行）

- **推荐方案：C（数据驱动 UI，无插件 HTML）**。插件在 `contributes.ui.views` 中只声明
  结构化节数据（`markdown | form | list | kv`），宿主用既有组件渲染；动态数据经宿主
  协议 `ui/values`（`view/children` 的同构扩展）回读，交互回传复用既有命令回路
  （`ext_execute_command`）。渲染进程收到的插件产物只有「Rust 校验过的清单 JSON +
  宿主归一过的值 JSON」，插件代码（HTML/JS/CSS）零字节进入主 webview——
  EXTENSIONS.md §7 红线字面与精神同时满足，且全链路零新增重型依赖。
- **最大风险**：
  1. **表达力天花板**：数据驱动覆盖设置表单/列表/键值/说明文档（编辑器插件 UI 的
     绝对主流），但做不了富交互（canvas、拖拽、三方组件库）。若 M1/M2 后此类需求
     高频出现，再评估 v2 的 iframe 路径（§3.8）；启用前必须修订 §7 并完成
     桌面双平台的协议、资源加载和桥接验证；
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
| 清单校验 | `crates/lc-core/src/ext/manifest.rs`：`Contributes.ui`、`validate_ui`、`MAX_MANIFEST_BYTES` 256 KiB | 已实现；M2-R 补稳定节 id 与 `source`，迁移旧 `valuesCommand` 声明 |
| 声明→DTO 下发 | `PluginInfo.uiViews` / `PluginList`（`crates/lc-core/src/ext/mod.rs`，经 `ext_list` 全量下发） | 已实现；M2-R 同步节描述及初始化 DTO |
| 树视图全链路 | EXTENSIONS.md §4.2；`sidecars/ext-host/src/host.mjs` `view/children` `:569-652`、`normalizeViewNodes` `:193-215`；`sidecars/ext-host/src/views.mjs` 注册表（注册范围受限 `:31-37`） | 本设计的直接模板：`ui/values` 照抄其派发/超时/错误 reason 全集 |
| 双端白名单归一 | `services/pluginUi.ts` 与 `sidecars/ext-host/src/host.mjs` 已有 `normalizeUiValues` | 当前宿主只获得 `sectionKinds`，不能按字段/列声明完整过滤；整改见 §3.2 |
| 命令执行回路 | `apps/desktop/src-tauri/src/ext.rs` `ext_execute_command` `:152-172`（全局 command→plugin 映射，声明期已校验归属）；palette 注入 `apps/web/src/App.vue:515-520` | 表单提交/行点击复用；`openCommand` 别名在 palette 层拦截 |
| 安全 markdown | `apps/web/src/services/markdown.ts`（`:1-72`，先转义后插标签，无第三方解析器，测试在 `markdown.spec.ts`） | markdown 节渲染直接复用；v1 子集无链接语法 → 零 `<a>` 面 |
| 标签页容器先例 | `ExtensionsPanel` 作编辑器区标签 `apps/web/src/App.vue:1402-1406`（`__extensions__`） | UI 视图落点照抄：`__ui__:<pluginId>:<viewId>` 标签 |
| 宿主协议 | `ctx.ui.register`、`ui/values`、`pendingUiViews`、Rust `ExtHost::ui_values` 已实现，默认超时 30s | 当前 sectionId 是数组序号字符串；M2-R 改用稳定 id 和完整节描述，见 §3.2 |
| Tauri 命令注册 | `apps/desktop/src-tauri/src/ext.rs` 的 `ext_ui_values` 及 `lib.rs` 注册 | 已有薄转发；ohos shim 未接线、入口隐藏 |
| CSP | `apps/desktop/src-tauri/tauri.conf.json`（现值含 `frame-src http: https:`） | v1 不动；v2 按平台生成实际资源 URL 与 CSP，见 §3.8 |
| 安全红线 | EXTENSIONS.md §7（`:357-364`）：插件代码永不进渲染进程、Node 插件可信、WASM 强隔离、`scrub_secret_env` | 本设计不削弱任何一条，§4 显式论证 |
| WebView 成本 | `docs/ARCHITECTURE.md` §6（销毁 WebView 释放 100MB+） | 否决方案 B 的依据之一 |

## 2. 方案对比

### A. 主 webview 内嵌 sandbox iframe

- **做法**：编辑器区嵌入 `sandbox="allow-scripts"` 的 iframe，`src` 由宿主按平台生成；资源走 custom protocol，父子间通过一次性握手建立专属 MessageChannel（§3.8）。
- **优点**：表达力=完整 Web 平台，与 VS Code webview 心智对齐；bridge 收敛后能力面可控；UX 嵌入式。
- **缺点与红线妥协度**：iframe 不能保证独立渲染进程，故不能满足 §7 的进程边界。sandbox + opaque origin 阻止直接访问父 DOM、父上下文与同源存储；网络资源及导航另需 CSP 和宿主策略约束，不能把 sandbox 当成断网机制。插件 HTML/JS 仍进入渲染器的解析器与 JS 引擎攻击面。启用前需修订 §7、明确安装提示，并验证 CSP、协议及桥接边界。

### B. 每插件独立 webview 窗口

- **做法**：`WebviewWindow` 每插件一个原生窗口（或 Tauri v2 单窗多 webview 嵌入变体）。
- **优点**：可提供单独的 webview 与配置的数据分区；是否独立进程仍取决于平台和运行时，不能仅凭独立窗口认定进程隔离。
- **缺点**：UX 是弹窗式而非嵌入式，编辑器布局（dockview/标签系统）无承载点；窗口管理成本（焦点/z-order/DPI/多显示器/最小化）；每 webview 重量级（ARCHITECTURE.md §6，100MB+）；嵌入变体跨平台一致性最差——WebKitGTK 对一窗多 webview 的支持与 WebView2 不对等，鸿蒙 ArkWeb 更无此能力，三端矩阵里唯一让 ohos 彻底出局的路径。
- **否决**（v1/v2 均不做）。

### C. 数据驱动 UI（推荐）

- **做法**：插件只声明结构化节数据（§3.1）；宿主 Vue 组件渲染（§3.4）；动态数据经 `ui/values` 协议回读（§3.2）；交互走既有命令回路（§3.5）。
- **优点**：**红线零妥协**（字面+精神：渲染进程收到的只有 Rust 校验过的声明 JSON 与宿主归一过的值 JSON，与树视图「数据可以进、代码永不进」同一边界）；不新增插件 HTML/JS 执行面；零新增依赖；数据面不依赖平台 webview 特性，ohos 后续可接；无插件 CSS，全走 `var(--*)` token。
- **缺点**：表达力受限（无自定义布局/富交互）；宿主组件维护成本（每新增节类型=一个受控组件）；复杂 UI 会撑清单（`ui` 块 ≤32 KiB 上限兜住）。
- **简化变体 C1（拒绝）**：数据回读也复用 `ext_execute_command`（节声明 `valuesCommand`），不新增 `ui/values`。拒绝理由：命令结果任意 JSON、无宿主侧形状校验；120s 命令超时与 UI 数据 30s 语义不符；用户命令与数据回读生命周期混同。最终划分：**提交/行点击=命令（复用），数据回读=独立协议（新增）**。

**结论：采用 C 为 v1 主线；A 为 v2 候选设计，须在 §7 修订及双平台验证后启用（§3.8）；B 不纳入本轮。**

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
          { "id": "intro", "kind": "markdown", "text": "# 说明\n……" }, // text ≤16 KiB；静态
          {
            "id": "filter", "kind": "form", "title": "筛选", "command": "demo.tasks.filter",
            "fields": [                                                // ≤32 个
              { "key": "q",     "label": "关键词", "type": "text", "placeholder": "……" },
              { "key": "limit", "label": "条数",   "type": "number", "min": 1, "max": 200, "default": 50 },
              { "key": "done",  "label": "含已完成", "type": "boolean", "default": false },
              { "key": "sort",  "label": "排序",   "type": "select", "options": ["new", "old"], "default": "new" },
              { "key": "token", "label": "令牌",   "type": "password" }
            ]
          },
          {
            "id": "tasks", "kind": "list", "title": "任务", "source": "provider",
            "columns": [                                               // ≤8 列；width=1..5 权重
              { "key": "title", "label": "标题", "width": 3 },
              { "key": "state", "label": "状态", "width": 1 }
            ],
            "rowCommand": "demo.tasks.open"                            // 可空；点击行 → 命令，args=行对象
          },
          { "id": "stats", "kind": "kv", "title": "统计", "source": "provider" }
        ]
      }
    ]
  }
}
```

- **稳定身份**：每节必须有 `id`，格式为 1–64 个小写字母、数字或连字符，首字符为字母，视图内唯一；field/column 的 `key` 在所属节内唯一。数组只决定显示顺序，不承担身份。注册表、请求和状态以 `(pluginId, viewId, sectionId)` 定位，不要求不同插件的 viewId 全局唯一。
- **节类型是闭集**（`markdown|form|list|kv`），未知 kind 在校验期拒绝；字段类型 `text|number|boolean|select|password` 同为闭集，`default` 必须匹配类型：文本/密码为字符串、数值为有限数且符合 min/max、布尔为 boolean、select 为 options 中的一项；options ≤32 个、kv 静态 items ≤64 对。`command`/`rowCommand`/`openCommand` 做「出现在该插件 `contributes.commands`」闭包校验。
- **数据源**：`source` 仅允许 list/kv 节使用，唯一动态值为 `"provider"`；list 必填，kv 省略时只渲染静态 `items`，存在时动态结果优先。markdown/form 不接受 `source`，form v1 仅使用清单 default。移除 `valuesCommand`：回读从不调用命令 handler，插件只需为视图注册一个 provider，不再为读取数据声明虚设命令。
- **校验实现**：在现有 `manifest.rs::validate_ui` 中补以上规则；沿用已有 `onUiView:<viewId>` 激活事件校验。任一失败只把该插件标记 `error`，不影响其它插件。
- **下发**：sections 随既有 `PluginInfo.uiViews` 下发（camelCase，Tauri custom DTO，不改 lc-proto 契约），不新增拉取命令；体积由 32 KiB `ui` 块上限兜住。

### 3.2 数据回读协议：`ui/values`（`view/children` 同协议扩展）

- **宿主注册**：`ctx.ui.register(viewId, provider)`，provider = `{ getValues(sectionId): Value | Promise<Value> }`；注册范围受限该插件清单声明。上述示例中 provider 收到 `"tasks"` 或 `"stats"`，按稳定 id 返回对应数据；不存在 `demo.tasks.list` 命令调用。
- **initialize 节描述 DTO**：`HostPlugin.uiViews` 下发下面的完整描述，替代当前的 `sectionKinds` 数组。Rust 从已校验清单构造、Node 按该描述归一、web 按同一清单再次归一；不得从 provider 返回值推断结构。

  ```ts
  type HostUiField = {
    key: string; type: 'text' | 'number' | 'boolean' | 'select' | 'password';
    default?: string | number | boolean; options?: string[]; min?: number; max?: number;
  }
  type HostUiSection =
    | { id: string; kind: 'markdown' }
    | { id: string; kind: 'form'; fields: HostUiField[] }
    | { id: string; kind: 'list'; source: 'provider'; columns: { key: string }[] }
    | { id: string; kind: 'kv'; source?: 'provider' }
  // HostPlugin.uiViews: { id: string; sections: HostUiSection[] }[]
  ```

- **JSON-RPC 方法**：`ui/values { pluginId, viewId, sectionId }`，sectionId 是清单稳定 id，不再接受节序号。仅向 `source: "provider"` 的节派发；未知节或不支持回读的节在调用 provider 前拒绝。
  - list：`{ "rows": [ { [columnKey]: 标量 } ] }`，≤500 行。仅保留声明的列键；未知键丢弃，已声明键的对象/数组/null/非有限数导致 `invalid-values`；缺列按空单元格渲染。标量只允许 string/有限 number/boolean。
  - kv：`{ "items": [ { "key": string, "value": string } ] }`，≤64 对；只保留 key/value，key 必须非空；非法项导致 `invalid-values`。
  - markdown/form：v1 无回读。表单只从 default 初始化，再由用户编辑和提交；动态初值另立切片，届时必须定义刷新与未保存输入的冲突处理。
  - 动态值规范化后序列化总量 ≤256 KiB，单字符串 UTF-8 ≤16 KiB，超限报 `invalid-values`。这是动态响应的限制，不能用 32 KiB 静态清单上限替代。归一与命令参数构造只枚举声明字段，不把 provider 对象直接合入宿主状态。
- **超时与错误语义完全照抄** `view/children`（EXTENSIONS.md §4.2）：默认 30s（`LC_EXT_HOST_VIEW_TIMEOUT_MS` 注入短超时供测试）；`-32601` reason ∈ `plugin-not-loaded`/`view-not-declared`/`view-not-registered`；`-32000` reason ∈ `provider-timeout`/`invalid-values`。
- **懒激活**：`onUiView:<viewId>` 为隐式激活事件，`pendingUiViews` 按 `(pluginId, viewId)` 命中后先激活（幂等、失败 `-32601 activation-failed`）。激活成功再按上述 DTO 检查节并调用 provider，语义与 `onView:` 一致。
- **runtime 约束**：任何 runtime 均可声明；v1 只在 node runtime 回读。非 node 的动态 list/kv 显示「需要 Node 运行时」，静态 kv/markdown 正常渲染；form 无 command 时只读，有 command 时使用该 runtime 已支持的命令回路，不能把可渲染误写成必定可提交。

**迁移边界（M2-R）**：现有 M1/M2 用数组序号字符串、`sectionKinds` 和作为回读开关的 `valuesCommand`；上述目标协议尚未实施。Rust 清单/DTO、内置 Node 宿主、web 服务/组件以及仓库示例、文档和测试在一个整改切片中迁移；不同时保留序号与稳定 id 两套语义。旧插件需显式补节 id，将 list/kv 的 `valuesCommand` 改为 `source: "provider"`，provider 按稳定 id 分派；旧声明或不匹配的初始化描述应报清单错误，不猜测转换。M2-R 完成前不宣称热更新输入隔离和声明级动态值归一已验收。

### 3.3 命令面（Tauri custom DTO，薄转发）

| 命令 | 签名 | 说明 |
|---|---|---|
| `ext_ui_values` | `(plugin_id, view_id, section_id) -> { values: Value }` | section_id 为稳定 id；插件须启用且为 node、视图及节已声明；无插件/视图或非 node 返回 `not_found`，不存在的节或非动态节拒绝回读并映射 `invalid-values`；经 `ExtHost::ui_values` 转发（30s），外层 values 包装归一后的 rows/items 对象；鸿蒙入口隐藏 |

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

- **表单提交** → `ext_execute_command(section.command, { values })`：只发送声明的 fieldKey，按 field.type 转换并校验（空 number 不隐式转为 0，min/max 与 select options 同清单约束），非法输入留在表单提示；password 值不进日志。ok → 节内成功注记，并刷新同视图所有 provider 数据节；error → 错误注记并保留用户输入。刷新不重置表单，不引入 form 动态初值。
- **list 行点击** → `rowCommand(row)`（仅声明列组成的归一行对象作 args）；成功后刷新该动态节，失败保留行数据并显示节内注记。
- **kv** 只读。**所有交互失败** → 节级错误态可重试（同树视图错误态，`PluginViews.vue:100-115`）。
- **打开视图本身零宿主往返**（入口全是 web 侧动作），首次数据回读才懒激活插件。

### 3.6 生命周期

- App 启动与插件增删后 `syncPluginUiViews()`；插件禁用/删除 → 关闭其标签并清理草稿、数据、错误态和在途请求的接收资格。UI 标签不跨会话持久化。
- 组件 key、草稿及节数据的身份均为 `(pluginId, viewId, sectionId)`，不使用显示序号。插入或重排节不改变已有节身份；删除节清掉其状态，同 id 的 kind、字段/列、命令、数据源或 default 等语义声明变化则重置该节。仅标题/label/显示顺序变化可以保留输入，其余未受影响节不重置。
- 每次成功应用插件列表都推进同步代次。异步回读、提交、行命令完成时，只有插件/视图/节仍有效、同步代次仍匹配且它仍是该节最新请求，才写入结果或触发刷新。旧结果直接丢弃；不能因 provider 仍运行而恢复已删除状态。副作用命令不会因 UI 丢弃结果而撤销，禁止自动重放。
- 同步作废旧请求时同时复位其 loading/busy 状态，避免丢弃响应后控件永久禁用；已有动态节按新声明重新加载，保留的表单草稿可以再次由用户提交。关闭视图时清理该视图运行时状态，重开从清单 default 和新 provider 数据开始。不得把关闭前的 password 草稿保存在全局或持久化存储中。

### 3.7 代码落点

以下模块均已存在；本表只列 M2-R 的修改范围：

- Rust `ext/manifest.rs` / `ext/mod.rs`：节 id、source 及闭集校验，PluginInfo 节声明同步；`ext/host.rs` 下发完整节描述，按稳定 id 检查回读资格。
- Node `sidecars/ext-host/src/ui.mjs` / `host.mjs`：按插件及视图定位注册表，按描述过滤列和约束动态值，provider 接收稳定 sectionId。
- 桌面 `src/ext.rs`：保持 `ext_ui_values` 薄转发，更新参数含义及错误映射。
- Web `services/extensions.ts` / `pluginUi.ts` / `components/PluginUiView.vue`：迁移 DTO 守卫、回读开关、稳定组件 key、草稿/数据状态以及同步代次检查；更新示例和双语言文案。既有 App 标签入口和 openCommand 行为沿用。
- 验收用例在既有 `ext/tests.rs`、`sidecars/ext-host/test/host.test.mjs`、`pluginUi.spec.ts` 中扩展；组件渲染和热更新须用真实应用补验，不只依赖纯函数测试。

### 3.8 iframe 沙箱路径（v2 可选，M3；以 §4 红线修订为前提）

- **声明**：`contributes.ui.webviews: [{ id, entry, assets? }]`——entry/assets 必须落插件目录内且已存在。entry 与显式 assets 构成可访问文件白名单；省略 assets 只允许 entry，不代表开放整个插件目录。响应时仍做 canonicalize 和根内校验，不允许通过重定向或符号链接扩大可访问范围。
- **sandbox 精确值**：`sandbox="allow-scripts"`——**不带 `allow-same-origin`**：opaque origin、无 localStorage/IndexedDB、禁父文档访问。Vue 中 `:sandbox` 绑定，测试断言属性精确值。
- **统一 URL 与路由**：只允许宿主 `pluginAssetUrl(pluginId, webviewId, asset)` 生成 URL，逻辑路径固定为 `/<pluginId>/<webviewId>/<asset>`，pluginId/webviewId 不放 authority。Linux 为 `lc-plugin://localhost/<pluginId>/<webviewId>/<asset>`；Windows 默认为 `http://lc-plugin.localhost/<pluginId>/<webviewId>/<asset>`，若启用 HTTPS 映射则同一生成器输出 HTTPS。CSP 与 handler 共用此规则，不能另拼字符串。[Tauri 官方平台说明](https://docs.rs/tauri/latest/tauri/struct.Builder.html#method.register_uri_scheme_protocol)
- **protocol handler**：通过 `register_uri_scheme_protocol` 注册；按平台解析受支持的请求 URI，校验预期 scheme/authority，再从 path 的前两段提取 pluginId/webviewId。必须启用且声明了该 webview，asset 必须在该视图文件白名单内。统一解码一次，拒绝编码斜杠、反斜杠、NUL、点段、绝对路径及盘符前缀，再做根内检查。仅 GET；目录与不存在/未声明资产 404、其它方法 405。Content-Type 白名单为 `text/html|text/css|application/javascript|image/png|image/jpeg|image/gif|image/webp|font/woff2`，统一 `nosniff`，HTML 响应附 CSP。
- **CSP**：父文档 frame-src 需覆盖生成器产出的实际平台地址；不能只增 `lc-plugin:` 就宣称 Windows 可用。子文档 `default-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'`，script/style/img/font 各使用宿主生成的该视图资源目录完整 URL 前缀（含尾斜杠），不使用任意 `http:`/`https:`、通配符或把 opaque origin 写成可比较的 `'self'`。v2 初版脚本/样式外置到白名单资产，禁止内联代码、eval、远程资源及重定向。此为待验证策略；须实际验证 sandbox 下的经典脚本、样式、图片与字体加载，ES modules 不在首版承诺内。仅 CSP 不能阻止 iframe 自身导航；宿主还须拒绝子 frame 离开批准的入口 URL，导航/重载使桥失效。若任一目标平台无法约束导航和资源访问，M3 不得启用。
- **一次性握手与身份绑定**：父侧为每个 iframe 实例生成随机 nonce 并绑定插件、视图、contentWindow 和加载代次；nonce 通过入口 fragment 交给子页。子页发 `{ source:"lc-plugin", protocol:1, op:"ready", nonce }`。父侧同时检查 `event.source === iframe.contentWindow`、nonce、代次和帧形状；无 allow-same-origin 的消息 `event.origin` 为 `"null"`，它只是预期格式，不能用来区分插件，消息中的 pluginId 也不能授权。[Chrome 官方 opaque-origin 说明](https://developer.chrome.com/docs/modern-web-guidance/guides/security/validate-origins)
- **专属 MessageChannel**：握手只接受一次；父侧向已校验的 contentWindow 转移 MessagePort（opaque 接收方需要 targetOrigin `"*"`），仅发送初始化帧和端口，不发送状态/凭据。后续所有请求及响应只走此端口；收到的每个请求均用父侧绑定的插件身份重新授权。iframe 重载、导航、卸载或插件禁用时关闭端口、清除 nonce、丢弃在途响应；重新加载须重新握手，旧端口不得复用。
- **bridge 能力**：握手 ready 之外，仅 `getState(key)` / `setState(key,value)` / `postCommand(command,args)`；请求含 seq，响应 `{ source:"lc-host", seq, ok, ... }`，参数形状与错误响应固定，未知 op 拒绝。state 为 Rust 侧 per-plugin JSON，键 ≤64 字符、单值 ≤64 KiB、每插件 ≤256 KiB，用户数据目录持久化；不用 webview storage。postCommand 的命令必须属于绑定插件的清单，Rust 端再次校验归属，不能只按全局 command 映射执行。无 fs/network/env/凭据直通，父侧不主动推送状态。

## 4. 与 EXTENSIONS.md §7 红线的关系

- **方案 C 字面满足，且是树视图边界的原样延伸**：渲染进程收到的插件产物只有三类——① Rust 校验过的清单 JSON（节结构）；② 宿主 `normalizeUiValues` 归一过的值 JSON；③ 经 `renderMarkdown` 转义后的文本。插件可执行代码仍只在扩展宿主（Node）/WASM/LSP 子进程中执行，进程模型零变化。「数据可以进、代码永不进」就是树视图已建立、本文沿用的边界。
- **方案 A 尚未验证能力隔离，且不满足现有进程边界**：sandbox + opaque origin 限制父上下文访问；资源、导航、IPC 和桥接需按 §3.8 在 Windows/WebView2 与 Linux/WebKitGTK 分别验证。不能以 sandbox 或独立 iframe 证明独立进程、断网或无 IPC 权限。
- **若 v2 启用 A，§7 建议改写为**：「插件可执行代码永不进入主 webview 的**特权上下文**；插件 UI 资源仅可经 `lc-plugin` 协议在 sandbox iframe（opaque origin、`allow-scripts` only）中渲染，且仅能经白名单 postMessage bridge（`ready`/`getState`/`setState`/`postCommand`）通信。」同时保留数据驱动为默认渲染方式，iframe 为清单显式 opt-in + 安装确认 UI 固定文案明示。
- M1/M2 与 M2-R **不触发** §7 任何修订；B 的独立窗口也不能自动证明满足进程隔离，且因 UX/平台成本不纳入本轮（§2）。

## 5. 测试策略

- **清单校验**（`crates/lc-core/src/ext/tests.rs`）：各 kind 合法/缺字段/kind 或 field type 非法；稳定节 id 缺失、格式非法、视图内重复；同名 viewId 可存在于不同插件；`source` 非法或用于 form/markdown、旧 `valuesCommand` 拒绝；命令引用越出插件声明；default 类型/边界/options 不符与规模超限；`onUiView` 合法与非法事件。任一失败只标该插件 `error`。
- **ext_ui_values**：视图未声明/插件禁用/非 node → `not_found`；未知节和静态节在 provider 执行前拒绝；Tauri 的 values 包装与 Node 的 rows/items 结果一致；宿主错误原样映射。
- **宿主**（`sidecars/ext-host/test/host.test.mjs`）：DTO 中完整保留节 id、kind、source、fields/columns；稳定 id 派发、无命令 handler 调用、不同插件同名视图互不覆盖；多余列丢弃、已声明列值非法拒绝、字符串/总响应超限；reason 全集含 activation-failed；懒激活幂等、失败不重试；provider 超时；未声明视图注册抛错。
- **web**（`apps/web/src/services/pluginUi.spec.ts`，仿 `pluginViews.spec.ts`）：`isUiSection`/`normalizeUiValues` 纯函数（含 `<script>`/`javascript:` 载荷、多余键丢弃、形状非法→null）；store 同步/错误态/重试；表单提交 args 组装；**`openCommand` 别名断言零 `invoke` 调用**（纯 web 拦截）；markdown 节 `<script>` 转义断言（复用 `markdown.spec.ts` 模式）；demo 模式 fixtures（照 `extensions.ts` DEMO 段）。
- **M2-R 生命周期验收**：表单中有未保存值时插入/重排其它节，输入仍归属原节；同 id 节语义变化只重置该节；禁用/删除/重载插件后旧回读和命令响应不写回、不自动重放副作用；乱序返回仅采用最新请求；关闭并重开不恢复密码草稿；form 不回读且提交后仅刷新 provider 节。
- **M3 iframe**：Windows/Linux 的生成 URL → handler 路由 → CSP → 实际资源加载闭环；拒绝编码穿越/其它视图资产/未声明资源及非 GET；精确 sandbox 属性；`origin="null"` 但错误 source/nonce/代次的握手拒绝，正确帧完成 MessagePort 往返；重复握手、旧端口、跨插件命令与超尺寸 state 拒绝；父 CSP、子 CSP、导航/重载后桥失效实测。未启用 M3 时这些属于验收计划，不计为现有安全证据。
- 全部网络用例走 loopback/file 或无网，CI 离线可跑（同市场约束）。

## 6. 里程碑切片与整改状态

| 切片 | 内容 | 估点 | 独立发布条件 |
|---|---|---|---|
| **M1 声明+渲染（已实现）** | `f5e1d10`：清单/DTO、编辑器标签、palette/插件管理入口、静态节渲染 | 4（原估） | 已有运行时验证记录，见 STATUS；不等于已完成 M2-R |
| **M2 交互回传（已实现）** | `101ab3c`；`4a17c5c`/`220c739`/`7ff026e` 修复回读刷新及响应式更新 | 3（原估） | 已有表单提交/行点击/动态节回读记录；当前仍是序号 sectionId、sectionKinds 与 valuesCommand 开关 |
| **M2-R 审查整改（待实施）** | 稳定节 id；完整初始化 DTO；source/provider 协议；声明级归一；草稿和在途请求生命周期；示例/文档同步迁移 | 3 | §5 的协议与生命周期用例通过，真实应用重排/变更/禁用过程中不串值；旧清单明确报错，运行代码与本文一致 |
| **M3（可选，v2，未立项）iframe 沙箱** | 平台 URL 生成/handler/CSP；一次性握手 + MessageChannel；导航边界；§7 修订及安装提示 | 5（须立项时重估） | 双平台 §5 全部通过、安全评审通过且 §7 修订合入；任一边界无法实现则不启用 |

估点单位：1 点 ≈ 一个 agent 半天；含测试与文档（同市场口径）。

## 7. 风险表

| 风险 | 等级 | 缓解 |
|---|---|---|
| 红线妥协（M3 iframe 字面违反 §7） | **高**（仅当做 M3） | §4 修订先行 + 数据驱动保持默认 + iframe 显式 opt-in + 安装确认固定文案；M1/M2 不触发修订 |
| markdown XSS/渲染层注入蔓延 | 中 | 唯一 v-html 点强制 `renderMarkdown`；v1 子集无链接；渲染层测试禁止第二 v-html 点/URL 渲染回退 |
| 命令/bridge 越权 | 中 | v1 命令闭包；v2 source/一次性 nonce/代次绑定 + 专属端口 + Rust 命令归属校验，不能以 null origin 授权 |
| 热更新串值或旧响应恢复已删除状态 | 高（M2-R 待关闭） | 稳定节 id、语义变更重置及同步代次/请求检查；真实应用覆盖重排、禁用、乱序响应 |
| 声明与动态回读结构漂移 | 中（M2-R 待关闭） | 完整节描述 DTO、source/provider 单一语义、按声明过滤及动态响应体积限制 |
| 表达力天花板引发的需求外溢 | 中 | M1 后收集真实需求；富交互需求集中出现再评估 M3；期间引导插件用「视图+命令+工具」组合表达 |
| 主题一致性 | 低 | by construction：无插件 CSS，全 token；节组件继承设置页/树视图样式基线 |
| 清单膨胀拖慢 ext_list | 低 | `ui` 块 ≤32 KiB、sections ≤16/view 上限；声明非法 fail-closed |
| ohos 功能缺口 | 低 | 桌面先行（同市场策略）；数据面不依赖 webview 特性，shim 补转发即可接 |
| 每插件一窗口的性能/UX（方案 B） | —（已否决） | 决策记录于 §2；若未来 reconsider，需先解决嵌入布局与三端 webview 一致性 |

## 8. 范围决定

- 节结构由清单静态声明；插件 provider 不能动态增删节。插件更新/重载后的清单变化按 §3.6 同步，与 provider 运行时变结构区分。
- **不引入 JSON Schema 校验依赖**：清单校验沿用 Rust 手写校验器风格（同 themes/languages/views），「防注入」由「kind 闭集 + 命令闭包 + 双端白名单归一」承担，零新增依赖。
- sections 随 `PluginList` 全量下发（不新增拉取命令）；体积由 32 KiB `ui` 块上限约束，实测超标再拆 `ext_ui_view`。
- `ui/values` 数据面 v1 仅 node runtime（同视图 v1 策略）；declarative 插件仅静态节渲染。
- form v1 仅用 default 初始化；动态初值另立切片，不因底层曾接受 `{ values }` 对象就宣称功能已交付。
- UI 标签 M1 起不跨会话持久化（重启即关）；需要持久化时在 tabs 体系内另立小切片。
- M3 是否立项取决于 M1/M2 交付后的真实需求密度，且以 §4 的 §7 修订合入为硬前提；默认不做。
- `lc-ohos` 不接线（同市场计划），`PluginUiView` 的 ohos 降级=入口隐藏。

## 9. 本次审查修订记录（2026-11-11）

- 稳定节身份与草稿/旧请求失效规则：§3.1、§3.6。
- 完整初始化描述及双端按声明归一：§3.2。
- 移除虚设 valuesCommand，明确 provider 路由和 form 动态初值不在 v1：§3.1–§3.5。
- 修正 opaque origin 身份判断，规定一次性握手及端口生命周期：§3.8。
- Windows/Linux URL、handler 路由与 CSP 共用平台规则：§3.8。
- M1/M2 实现事实与 M2-R 待办分开记录：§1、§6；上述修改仅为方案修订，不自动关闭代码整改项。
