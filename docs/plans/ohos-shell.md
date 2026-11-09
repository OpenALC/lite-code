# 鸿蒙（HarmonyOS）首版「只读 + 对话」外壳实施方案

已实际阅读 `spikes/harmony-arkweb/`（README、`Index.ets`、`module.json5`、`build-profile.json5`、NAPI C++）、`apps/web/src/App.vue`、`apps/web/src/services/bridge.ts`、`apps/web/vite.config.ts`、`apps/web/src/components/CodeEditor.vue`、`crates/lc-core`（fs/agent/policy/config）、`apps/desktop/src-tauri/src/{lib.rs,agent.rs}`、`docs/MILESTONES.md` P6 节与 `docs/STATUS.md` 中的鸿蒙行。

## 0. 结论摘要

1. **工程落点**：新建 `apps/ohos/`，从 spike 复制骨架改造；spike 原样冻结为验证证据。`apps/ohos` 不加入 pnpm workspace；ohpm/hvigor 独立。Rust 宿主逻辑放 `crates/lc-ohos`（root cargo workspace 成员，host 可编译、可单测），C++ NAPI 胶水留在 `apps/ohos/entry/src/main/cpp`。
2. **前端**：新增 `ohos.html` + `src/ohos/*` 专用入口，**不**在 `App.vue` 里加 `?mode=readonly` 分支。App.vue 目前只有 `editor|agent` 两态和 `?root=`（`apps/web/src/App.vue:133-144`、`:280`），没有只读分支；而它的 `onMounted` 会扇出 `workspaceRoot`、git、task、db、LSP、history 等大量 Tauri 命令（`App.vue:897-924`），用 query 分支会逼着我们把整套桌面命令都 shim 出来。专用入口的共享改动控制在 30 行以内。
3. **资源加载**：产品用「虚拟 https origin + `onInterceptRequest` 从 rawfile 供给」，这是 ArkWeb 官方文档给出的本地资源跨域解法（[转述与示例](https://www.seaxiang.com/blog/qEKFw5)）；dev 继续用 `http://10.0.2.2:5173/`。不依赖“关闭 web 安全设置”——ArkWeb 没有公开的此类开关，不应作为方案前提。内嵌 HTTP server 只作 fallback。
4. **core 通信**：保持 Tauri IPC 形状的 JS shim → ArkTS `javaScriptProxy` → NAPI C++ → `lc-ohos`（Rust）→ `lc-core`。首版命令面 7 条 + 事件 9 类；会话强制 `plan` 模式，`plan` 只允许只读工具（`crates/lc-core/src/agent/policy.rs:129-137`），正好对应“只读 + 对话”。
5. **不把首版赌在 lc-core 交叉编译上**：MVP 用 `lc-ohos` 里的 stub agent 打通 NAPI 事件链；真 core 作为 Phase 4 门禁，由 0.5 天的构建探针决定。失败则降级为 std fs + stub agent，并记录依赖债。
6. **签名**：未签名 HAP 可进模拟器（spike 已验证）；真机需要调试/发布签名，属 Phase 5，CI 先只出 unsigned HAP artifact。

## 0.1 事实基线

| 事实 | 证据 |
|---|---|
| ArkWeb 可跑现有前端、NAPI 可传 handle | `spikes/harmony-arkweb/README.md:37-44` |
| rawfile 直载 Vite 产物被 CORS 拒绝（origin null） | `spikes/harmony-arkweb/README.md:40`、`docs/ARCHITECTURE.md:218` |
| Monaco 渲染 OK，但内存/IME 未过 → 首版只读+对话 | `spikes/harmony-arkweb/README.md:45-53`、`docs/MILESTONES.md:184-189`、`docs/STATUS.md:68` |
| 门禁原文：只过前两项 → 交付“只读 + 对话” | `docs/MILESTONES.md:184-190` |
| 前端无只读分支，只有 editor/agent 与 `?root=` | `apps/web/src/App.vue:133-144`、`:280` |
| bridge 用 `window.__TAURI_INTERNALS__.invoke` 探测 Tauri | `apps/web/src/services/bridge.ts:207-210` |
| 事件走 `core:event`，接受 `{seq,event}` 信封 | `apps/web/src/services/bridge.ts:1294-1363` |
| Monaco 是动态 import，未挂载不会加载 | `apps/web/src/components/CodeEditor.vue:311` |
| 文档明确“单编辑器方案，不引入 CodeMirror” | `docs/README.md:31` |
| plan 模式允许只读工具、拒绝写/执行/网络 | `crates/lc-core/src/agent/policy.rs:125-137` |
| agent 的 EventSink 抽象已存在，但只为 Tauri 实现 | `apps/desktop/src-tauri/src/agent.rs:162-179` |
| 桌面命令面很大（60+ 条） | `apps/desktop/src-tauri/src/lib.rs:798-865` |

---

## 1. 分阶段总览

| 阶段 | 工期 | 内容 | 出口标准 |
|---|---|---|---|
| **P0 探针** | 0.5–1d | 虚拟 origin 加载 ESM/dynamic import；lc-core OHOS 交叉编译；NAPI async/tsfn | 三项各有明确 go/no-go，写入 `docs/ARCHITECTURE.md` 附录 |
| **P1 外壳工程** | 1–2d | `apps/ohos/` 骨架、web 同步脚本、虚拟 origin 资源服务、HAP 安装 | 模拟器离线加载 `ohos.html`，不依赖宿主 Vite |
| **P2 只读 UI** | 2–3d | `OhosApp.vue`、只读预览、文件树、模式锁、移动布局 | 不加载 Monaco；文件树/预览/对话三块可用 |
| **P3 NAPI 桥** | 3–4d | `crates/lc-ohos`、C++ NAPI、JS shim、事件通道 | `fs_read_dir/read_file` 真数据；stub agent 流式事件贯通 |
| **P4 真 core/对话** | 4–6d | lc-core 编译/裁剪、`lc-host` 抽取、`plan` 会话、provider 配置 | 真实 provider（或宿主 mock）跑通一轮多轮对话 |
| **P5 产品化** | 3–5d | 工作区供给、历史持久化、签名/真机、文档与 CI | 真机安装；STATUS/MILESTONES/ARCHITECTURE 更新 |

P0–P3 合计约 7–10 个工作日；P4 是否成立取决于 P0 的交叉编译探针。

---

## 2. 工程落点

### 2.1 目录结构

```
apps/ohos/                          # 新工程，从 spike 复制骨架
  AppScope/app.json5                # bundleName: dev.litecode.ohos
  build-profile.json5               # signingConfigs: []（不提交密钥）
  hvigorfile.ts
  hvigor/hvigor-config.json5        # 固定 hvigor 版本
  hvigorw / hvigorw.bat             # 从 DevEco 模板补，spike 缺
  oh-package.json5
  entry/
    build-profile.json5             # externalNativeOptions + abiFilters arm64-v8a/x86_64
    hvigorfile.ts
    src/main/module.json5           # INTERNET；deviceTypes: phone/tablet/2in1
    src/main/ets/entryability/EntryAbility.ets
    src/main/ets/pages/Index.ets    # Web + onInterceptRequest
    src/main/ets/native/NativeBridge.ets
    src/main/cpp/CMakeLists.txt     # 链接 Rust staticlib
    src/main/cpp/napi_init.cpp      # NAPI 胶水
    src/main/cpp/types/libentry/{index.d.ts,oh-package.json5}
    src/main/resources/rawfile/web/        # 构建产物，gitignore
    src/main/resources/rawfile/sample-workspace/  # 可选示例工作区
  native/README.md                  # Rust 交叉编译命令
  README.md
  .gitignore

crates/lc-ohos/                     # root cargo workspace 成员
  Cargo.toml                        # crate-type = ["staticlib", "rlib"]
  src/lib.rs                        # Host、JSON dispatch、stub agent

apps/web/
  ohos.html                         # 新 MPA 入口
  src/ohos/{main.ts,OhosApp.vue,ReadonlyCodeView.vue,tauriShim.ts,native.d.ts,ohos.css}
  vite.config.ts                    # mode=ohos → input=ohos.html, outDir=dist-ohos
  package.json                      # build:ohos

scripts/ohos/
  build.mjs                         # 构建 → 同步 rawfile → hvigorw → 打印 HAP
  env.md                            # DEVECO_SDK_HOME / HVIGOR_USER_HOME / hdc
```

### 2.2 与 pnpm / ohpm / hvigor / cargo 的关系

| 工具 | 关系 | 处理 |
|---|---|---|
| pnpm workspace | `apps/ohos` 不进 workspace | `pnpm-workspace.yaml` 只列 `apps/db-bridge`、`apps/web`、`sidecars/*`，`apps/ohos` 天然被忽略；不改文件 |
| ohpm | 独立包管理 | `apps/ohos/oh-package.json5`、`oh-package-lock.json5`；首版零第三方依赖，仍执行 `ohpm install` 保证可复现 |
| hvigor | 独立构建 | 不经过 pnpm scripts；由 `scripts/ohos/build.mjs` 调用，CI 不把它塞进 `pnpm build` |
| cargo | 共享 root workspace | `crates/lc-ohos` 是 workspace 成员，host 上 `cargo test -p lc-ohos` 可跑；OHOS 目标只影响交叉编译产物 |
| 前端产物 | 单向复制 | `apps/web/dist-ohos` → `apps/ohos/entry/src/main/resources/rawfile/web/`，rawfile/web 进 `.gitignore`（spike 已有同样约定：`spikes/harmony-arkweb/.gitignore:6`） |

**为什么新建而不是直接改 spike**：spike 的 `bundleName` 是 `dev.litecode.spike`（`AppScope/app.json5:3`），`signingConfigs` 为空（`build-profile.json5:3`），没有 hvigor/ohpm wrapper，也没有产品结构。保留 spike 作为门禁证据，避免“验证工程”和“产品工程”混在一起。

**Rust 静态库放 root workspace 的理由**：`lc-ohos` 本身不依赖 NAPI（NAPI 在 C++ 侧），host 可编译可单测；这样 MSRV、`Cargo.lock`、`cargo-deny`、`check-lanes` 都保持一致。只有 C++ 胶水和 OHOS 交叉编译是平台专属。

### 2.3 构建脚本职责（`scripts/ohos/build.mjs`）

1. 校验 `DEVECO_SDK_HOME`、`hvigorw` 路径、`ohpm` 路径，缺失时给出可操作的错误。
2. `pnpm --filter @lite-code/web build:ohos`。
3. 校验 `dist-ohos/ohos.html` 存在；断言没有 `monaco` 大 chunk。
4. 清空并复制到 `rawfile/web/`。
5. `ohpm install`（如可用）。
6. `hvigorw assembleHap --mode module -p product=default -p module=entry@default -p buildMode=debug --no-daemon`。
7. 打印 HAP 路径与 `hdc install` 命令。

命令与 spike README 完全一致（`spikes/harmony-arkweb/README.md:22-28`），只是路径换成产品工程。

---

## 3. 前端改造

### 3.1 入口方案对比

| 方案 | 改动量 | 问题 | 结论 |
|---|---|---|---|
| **专用 MPA entry**（`ohos.html` + `src/ohos/*`） | 新增约 600–700 行；共享改动 <30 行 | 需要维护第二套壳布局 | **推荐** |
| `App.vue` 读 `?mode=readonly` 分支 | 看似最小，实际要逐个隐藏菜单/侧栏/状态栏，并 shim 全部桌面命令 | `onMounted` 触发 git/task/db/LSP/history 等命令（`App.vue:897-924`）；`App.vue` 会静态 import `CodeEditor`，虽不会挂载 Monaco，但桌面逻辑全在 | 仅作临时 demo 兜底 |
| 复制一份 `App.vue` | 大 | 两套逻辑漂移 | 不推荐 |

专用入口的核心是**不 import `App.vue`、不 import `CodeEditor.vue`**，因此 Monaco 完全不进入构建图（`CodeEditor.vue:311` 是动态 import，但父组件不引入就不会有 chunk）。

### 3.2 新增文件与职责

| 文件 | 职责 |
|---|---|
| `apps/web/ohos.html` | viewport / color-scheme；`<script type="module" src="/src/ohos/main.ts">` |
| `src/ohos/main.ts` | `import './tauriShim'` → `createApp(OhosApp).use(i18n).mount('#app')`；安装 tabs 持久化 |
| `src/ohos/OhosApp.vue` | 顶部精简栏 + 移动端三视图（文件/预览/对话）；组合 `FileTree`、`ReadonlyCodeView`、`AgentPanel`；`createAgentSession` + `createHistoryRecorder` |
| `src/ohos/ReadonlyCodeView.vue` | 订阅 `tabsStore`，渲染 `<pre>` + 行号；大文件截断提示；无编辑能力 |
| `src/ohos/tauriShim.ts` | 定义 `window.__TAURI_INTERNALS__` / `__TAURI_EVENT_PLUGIN_INTERNALS__`，把命令转发到 `window.__lcNative`，事件回灌 `core:event`；history 用 localStorage；命令白名单 |
| `src/ohos/native.d.ts` | `window.__lcNative` 类型 |
| `src/ohos/ohos.css` | 移动布局、底部导航、只读预览样式；复用 `styles/tokens.css` |

### 3.3 共享改动（最小侵入）

| 文件 | 改动 | 目的 |
|---|---|---|
| `apps/web/src/services/bridge.ts` | 新增 `shellKind(): 'tauri'|'ohos'|'browser'`、`isReadOnly()`；`isTauri()` 不变 | 让 UI 知道当前是只读外壳；不破坏现有桥接语义 |
| `apps/web/src/components/FileTree.vue` | `canMutate` 增加 `&& !isReadOnly()` | 隐藏新建/重命名/删除/拖拽移动 |
| `apps/web/src/components/AgentPanel.vue` | 新增 `forceMode?: SessionMode`；`MODES` 按它过滤；附件按钮改为 `shellKind() === 'tauri'` | 锁定 `plan`；隐藏无实现的附件选择 |
| `apps/web/vite.config.ts` | 增加 `mode === 'ohos'` 的 `build.rollupOptions.input` 与 `outDir: 'dist-ohos'` | 独立产物，不污染桌面 dist |
| `apps/web/package.json` | `"build:ohos": "vue-tsc --noEmit && vite build --mode ohos"` | CI 可直接跑 |

`AgentPanel` 的 `isTauri()` 在 shim 注入后会为 true，因此附件按钮必须用 `shellKind()` 而不是 `isTauri()` 判断。

### 3.4 只读渲染选型

| 方案 | 运行时成本 | 结论 |
|---|---|---|
| **纯 `<pre>` + CSS 行号** | ~0 | **MVP 用这个**；先保证文件树/对话可用 |
| `highlight.js`（精选语言） | ~120KB，无 worker | Phase 2 推荐；配合已有 DOMPurify 渲染 |
| 只读 Monaco | 渲染进程 PSS ~275MB（spike 模拟器数据，`README.md:45`） | 仅当真机测量通过时考虑；不作为首版 |
| CodeMirror 6 | 中等 | **与 `docs/README.md:31`“单编辑器方案，不引入 CodeMirror”冲突**；需要先改文档决策，不应由实现者默认引入 |
| Shiki | WASM/大依赖 | ArkWeb 上风险高，不推荐 |

行号可用 CSS counter 或左侧 `<span>` 列；大文件复用 `resolveEditorStrategy` 的阈值，超过 1MB 只显示前 N 行并提示。

### 3.5 布局

- **手机竖屏**：顶部 36px 精简栏（项目名、刷新、主题）+ 底部三段式切换「文件 / 预览 / 对话」。点文件自动切到预览；对话全宽。
- **平板 / 2in1**：左侧文件树（240px）+ 中间预览 + 右侧对话（可折叠）。
- 不渲染桌面菜单栏、活动栏、Git/DB/Task 面板、状态栏、窗口控件。
- 复用 `tokens.css` 的明暗主题与 `vue-i18n`；不新增文案体系。

### 3.6 构建验证

`pnpm --filter @lite-code/web build:ohos` 后检查：

- 产物只有 `ohos.html` + `assets/*`，无 `monaco-editor` 大 chunk；
- `grep -R "monaco" dist-ohos/assets/*.js` 为空；
- 首屏 JS gzip 目标 < 250KB（Vue + i18n + markdown + DOMPurify）。

---

## 4. 资源加载方案对比

### 4.1 五种方案

| 方案 | 原理 | 优点 | 缺点 | 结论 |
|---|---|---|---|---|
| **A. HTTP dev（`10.0.2.2:5173`）** | 宿主 Vite 服务 | HMR、零拷贝、spike 已验证 | 依赖开发机；不能发布；模拟器专用 | **仅 dev** |
| **B. 虚拟 https origin + `onInterceptRequest`** | `https://app.litecode.example/ohos.html`，拦截后从 `rawfile/web/` 返回 | 同源，ESM/CSS/fonts 全通过；离线；官方文档方案；可继续用 Vite 的绝对 `/assets` 路径 | 需实现 MIME 表、路径穿越防护、404；每个资源经拦截层 | **产品推荐** |
| **C. 内嵌 HTTP server** | NAPI/ArkTS 在 127.0.0.1 起 TCP server 服务 rawfile | 真 http origin；Range/缓存自然 | 端口生命周期、后台挂起、loopback 安全、额外线程；非必要 | **fallback**（B 的模块/worker 探针失败时） |
| **D. `setPathAllowingUniversalAccess` + `file://`** | 允许指定目录跨域 | 官方文档方案，代码少 | 需要 `base: './'`；file:// 下 ESM/worker 行为差异；只允许 `filesDir`/`resourceDir` 子目录 | 不推荐（备选） |
| **E. 单文件内联（vite-plugin-singlefile）** | 所有 JS/CSS 内联进 HTML | 无子资源请求，绕过 CORS | 包体巨大、无缓存、动态 chunk/worker 全废 | 仅应急 |

**关于“关闭 web 安全设置”**：ArkWeb 没有公开的“关闭 web security”开关；官方给的两条路就是 B（虚拟域名 + 拦截）和 D（路径白名单）。不要把它写进方案。

**关于 inline script**：内联脚本本身能跑，但 Vite 的 ESM 产物是外链模块；只内联入口、不内联所有 chunk 仍会触发 CORS。全量内联就是方案 E。

### 4.2 推荐实现（方案 B）

`Index.ets` 核心形状：

```ts
Web({ src: 'https://app.litecode.example/ohos.html', controller: this.controller })
  .javaScriptAccess(true)
  .domStorageAccess(true)
  .fileAccess(false)          // 不再依赖 file://
  .imageAccess(true)
  .onInterceptRequest((event) => {
    const url = event?.request.getRequestUrl() ?? '';
    const path = url.startsWith(ORIGIN + '/') ? url.slice(ORIGIN.length + 1) : '';
    if (!isSafeAssetPath(path)) return notFound();
    const response = new WebResourceResponse();
    response.setResponseData($rawfile(`web/${path}`));
    response.setResponseMimeType(mimeOf(path));
    response.setResponseEncoding('utf-8');
    response.setResponseCode(200);
    response.setReasonMessage('OK');
    response.setResponseIsReady(true);
    return response;
  })
```

要点：

- **origin 用保留域名**（`app.litecode.example`），避免与真实域名冲突；不要用 `lc.local` 这类 mDNS 域。
- **MIME 表必须显式**：html/js/mjs/css/json/svg/png/woff2/wasm/map；未知扩展返回 `application/octet-stream`。
- **路径防护**：`decodeURIComponent` 后拒绝 `..`、`\`、`%00`、绝对路径；只允许 `web/` 前缀。
- **未命中必须返回 404 response**，不能返回 `null`，否则会落到真实网络（DNS 失败会表现为白屏）。
- `$rawfile` 适合中小资源；大文件可改用 `resourceManager.getRawFileContent` 或 fd 形式（实现时按 API 26 能力选择）。
- 调试期打开 `webview.WebviewController.setWebDebuggingAccess(true)`，release 关闭。
- `rawfile/web/` 与 `apps/web/dist-ohos` 一一对应；`ohos.html` 保持在根路径，Vite 的 `/assets/...` 正好映射到 `rawfile/web/assets/...`，**不需要改 `base`**。

### 4.3 dev/prod 切换

- debug：`src` 指向 `http://10.0.2.2:5173/ohos.html`，不注册拦截（或只拦截产品 origin）。
- release：`src` 指向产品 origin。
- 用 `build-profile.json5` 的 `buildOption.arkOptions.buildProfileFields` 生成 `BuildProfile` 常量，或 MVP 阶段直接用一个 `const USE_DEV_SERVER = true` 并在 README 写明；不要用运行时环境变量。

### 4.4 必须验证的边界

| 项 | 为什么 | 时间盒 |
|---|---|---|
| dynamic `import()` chunk | 未来引入按需模块时会用 | 30min |
| Web Worker | Monaco/高亮若回归会用到 | 30min |
| `modulepreload` / `crossorigin` | Vite 产物默认带 | 30min |
| 字体/图片 MIME | 图标与主题字体 | 15min |
| 404 行为 | 避免白屏和真实网络请求 | 15min |

---

## 5. core 通信

### 5.1 分层

```
Web (Vue)
  └─ apps/web/src/ohos/tauriShim.ts        // 伪装 window.__TAURI_INTERNALS__
       └─ window.__lcNative.invoke(cmd, argsJson): Promise<string>
            └─ ArkTS NativeBridge（javaScriptProxy，异步方法放 asyncMethodList）
                 └─ NAPI C++（libentry.so）
                      └─ crates/lc-ohos（Rust staticlib）
                           └─ lc-core（fs / agent / llm / policy）

事件：lc-ohos → C 回调 → napi_threadsafe_function → ArkTS 队列
      → WebMessagePort（优先）或 runJavaScript 批推 → shim 回灌 core:event
```

### 5.2 JS shim 的契约

shim 必须实现：

- `window.__TAURI_INTERNALS__.invoke(cmd, args)`；
- `window.__TAURI_INTERNALS__.transformCallback(cb)`；
- `window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener(event, id)`；
- 内部处理 `plugin:event|listen` / `plugin:event|unlisten`，把原生事件按 `{ event, id, payload }` 交给注册的回调；
- 命令白名单：只转发 `workspace_root`、`fs_read_dir`、`fs_read_file`、`session_create`、`agent_send`、`agent_cancel`、`approval_resolve`、`providers_load`、`providers_save`；其余（`git_*`、`db_*`、`task_*`、`lsp_*`、`ext_*`、`fs_write_*`、`workspace_pick`）直接抛 `unsupported`。
- `history_*` 用 localStorage 实现（形状对齐 `bridge.ts` 的 `HistorySummary`），先避免原生历史存储。

这样 `bridge.ts` 的 20 多个 `isTauri()` 分支全部保持原样，`isTauri()` 返回 true，命令包装、错误信封、事件校验都能复用。

### 5.3 NAPI 接口（C ABI）

```c
// 初始化：传入 sandbox 路径与配置路径；返回 handle id
uint64_t lc_ohos_init(const char* config_json);
// 同步阻塞调用，由 C++ 的 async work 线程调用
int32_t  lc_ohos_call(uint64_t handle, const char* cmd, const char* args_json,
                      char** out_json, char** out_error);
// 注册事件回调；回调在 Rust 线程触发，C++ 内部转 threadsafe function
void     lc_ohos_set_event_sink(uint64_t handle, lc_event_cb cb, void* user);
void     lc_ohos_dispose(uint64_t handle);
void     lc_ohos_free(char* ptr);
```

C++ 侧：

- `call` 用 `napi_create_async_work` 或受控线程池执行，完成后在 JS 线程 resolve/reject；
- 事件用 `napi_create_threadsafe_function`；`napi_add_env_cleanup_hook` 里 dispose，避免 ArkWeb 页面销毁后回调悬空；
- handle 用 `napi_external`/对象包裹，不把裸指针交给 JS；
- JS 线程永不阻塞；`agent_send` 立即返回 `ack`，内容走事件。

### 5.4 首版最小命令子集

| 优先级 | 命令 | 参数 | 返回 | 说明 |
|---|---|---|---|---|
| P0 | `workspace_root` | `{}` | `{kind:'workspace_root', root}` | 返回沙箱工作区根 |
| P0 | `fs_read_dir` | `{path}` | `{kind:'dir_listing', path, entries, generation:0}` | `lc_core::fs::read_dir_level`，先 `ensure_within_root` |
| P0 | `fs_read_file` | `{path}` | `{kind:'file_content', path, text}` | 先 `ensure_within_root`；非 UTF-8 报 `io` |
| P0 | `session_create` | `{mode:'plan', model?, thinking?}` | `{kind:'session', session}` | 不传 task_id；不启 MCP |
| P0 | `agent_send` | `{session_id, text}` | `{kind:'ack'}` | 内容走事件 |
| P0 | `agent_cancel` | `{session_id}` | `{kind:'ack'}` | |
| P1 | `providers_load` | `{}` | `ProvidersView`（camelCase） | AgentPanel 需要模型列表才会启用输入框 |
| P1 | `providers_save` | `{input}` | `ProvidersView` | 首版可只读预置，Phase 4 再接 UI |
| P2 | `approval_resolve` | `{session_id, approval_id, decision}` | `{kind:'ack'}` | plan 模式下读工具自动放行，暂不需要；保留接口 |
| P2 | `fs_search` | `{query,...}` | 非契约 `SearchResult` | 搜索侧栏不在首版 |
| 明确不做 | `workspace_pick`、`fs_write_*`、`fs_move/create/delete/rename`、`git_*`、`db_*`、`task_*`、`lsp_*`、`ext_*`、`node_*`、`update_*` | — | — | 首版关闭 |

### 5.5 事件子集（信封 `{seq, event}`）

- `agent:text_delta { session_id, index, text }`
- `agent:thinking_delta { session_id, text }`
- `agent:tool_start { session_id, call_id, name }`
- `agent:tool_result { session_id, call_id, ok, text }`
- `agent:session_state { session }`
- `agent:usage { session_id, input, output, cached }`
- `agent:compacted { ... }`
- `agent:done { session_id, stop_reason }`
- `error { error }`
- 未来：`agent:approval_required`

`bridge.ts:1573-1642` 的 `isCoreEvent` 已覆盖这些类型，原生侧只要按 `lc_proto::EventEnvelope` serde 序列化即可。

### 5.6 会话模式与运行环境

- **强制 `plan`**：`SessionMode::Plan` 映射到 `PolicyMode::Plan`，只读工具允许、写/执行/网络拒绝（`policy.rs:129-137`），不需要审批 UI。
- **禁用 MCP**：桌面 `session_create` 会 `McpHost::connect` 并可能 spawn Node（`apps/desktop/src-tauri/src/agent.rs:573-583`）；鸿蒙宿主必须走 `enable_mcp: false`，否则会产生无法回收的子进程。
- **禁用 task/db**：OHOS 宿主不注入 `TaskState`/`DbState`，只构造 `AgentLoop` + `ToolRegistry::with_builtins()`（写/执行工具会被 plan 策略拒绝）。
- **环境探测**：`EnvironmentContract::detect` 在无 shell 时输出 `unavailable`（`crates/lc-core/src/agent/environment.rs:40-57`），不会阻塞会话；`detect_available` 的探测失败应静默。
- **路径**：`providers_path()` 默认基于 `dirs::home_dir()` + `~/.lite-code`（`crates/lc-core/src/config/mod.rs:68-70`、`llm/runtime.rs:75-81`）。NAPI init 时 `setenv("LC_PROVIDERS_FILE", "<filesDir>/providers.json")`，并设置 `TMPDIR` 到 `filesDir/tmp`；不要依赖 OHOS 的 `HOME`。
- **配置模板**：首版把 `providers.json` 模板放 `rawfile`，首次启动复制到 `filesDir`；MVP 可用宿主 mock provider（`http://10.0.2.2:<port>/v1`）避免真实密钥。

### 5.7 宿主 crate 与复用策略

`crates/lc-ohos` 提供：

```rust
pub struct HostConfig { pub files_dir, pub workspace_root, pub providers_file }
pub struct Host { /* workspace root, sessions, event sink */ }

impl Host {
    pub fn init(cfg: HostConfig) -> Result<Host, ErrorBody>;
    pub fn dispatch(&self, cmd: &str, args_json: &str) -> Result<String, ErrorBody>;
    pub fn set_event_sink(&self, sink: Arc<dyn Fn(String) + Send + Sync>);
}
```

- `dispatch` 是纯 JSON 入口，host 上可单测；不需要 NAPI 就能覆盖 fs 路径校验、错误码、stub agent 事件序列。
- **推荐（Phase 4）**：把 Tauri `agent.rs` 里的 `AgentSessions`/`Worker`/`spawn_worker` 抽到 `crates/lc-host`，`EventSink` trait 已经是现成抽象（`agent.rs:162-179`）；Tauri 侧只保留 `AppEventSink` 与 task/db 钩子。这样鸿蒙与桌面共用同一套会话编排。
- **时间紧的临时方案**：在 `lc-ohos` 复制最小 worker（约 300 行），显式标注技术债，Phase 5 前合并回 `lc-host`。不要长期保留两份。

### 5.8 lc-core 的 OHOS 编译与依赖风险

Rust 官方把 `aarch64/armv7/x86_64-unknown-linux-ohos` 列为 Tier 2（带 host tools），`rustup target add` 可用，但需要 OpenHarmony SDK 的 clang/sysroot 与 `ohos-openssl`（[rustc book](https://doc.rust-lang.org/rustc/platform-support/openharmony.html)）。`lc-core` 当前依赖：

- `ignore`/`globset`/`regex`：纯 Rust，风险低；
- `notify`/`notify-debouncer-full`：inotify，可能可编译，但 OHOS 不需要 watch；
- `ureq` + `rustls`/`ring`：**最大风险**，TLS 后端可能需要换成 OpenSSL；
- `zip`/`sha2`：Node 下载用，OHOS 不需要；
- `dirs`：`home_dir()` 在 OHOS 可能为 None，已用 `LC_PROVIDERS_FILE` 规避。

**Phase 4 的前置动作**：给 `lc-core` 加 feature（`fs`、`watch`、`agent`、`runtime`、`db`、`git`、`lsp`），让 `lc-ohos` 只开 `fs` + `agent`；`agent` 的 `ureq` TLS 后端按探针结果选择 `rustls` 或 `native-tls`/OpenSSL。若两者都失败，最后一招是让 LLM Transport 经 NAPI 回调 ArkTS `@ohos.net.http`，但同步/流式桥接复杂，不建议首版做。

**探针命令**（在 `apps/ohos/native/README.md` 固化）：

```bash
rustup target add x86_64-unknown-linux-ohos aarch64-unknown-linux-ohos
cargo build --release --target x86_64-unknown-linux-ohos -p lc-ohos
cargo build --release --target aarch64-unknown-linux-ohos -p lc-ohos
```

产物 `target/<triple>/release/liblc_ohos.a` 由 CMake 按 `OHOS_ARCH` 链接。

---

## 6. 构建与发布

### 6.1 HAP 构建步骤

```bash
export DEVECO_SDK_HOME='D:\ide\Huawei\DevEco Studio\sdk'
export HVIGOR_USER_HOME="$TEMP/lc-hvigor"

# 1) 前端
pnpm --filter @lite-code/web build:ohos

# 2) 同步资源
node scripts/ohos/build.mjs --skip-web   # 或让脚本一起跑

# 3) ohpm（零依赖也应执行，保证 lock 可复现）
'D:\ide\Huawei\DevEco Studio\tools\ohpm\bin\ohpm.bat' install

# 4) HAP
'D:\ide\Huawei\DevEco Studio\tools\hvigor\bin\hvigorw.bat' assembleHap \
  --mode module -p product=default -p module=entry@default -p buildMode=debug --no-daemon

# 5) 安装到模拟器
HDC='D:\ide\Huawei\DevEco Studio\sdk\default\openharmony\toolchains\hdc.exe'
"$HDC" install -r entry/build/default/outputs/default/entry-default-unsigned.hap
"$HDC" shell aa start -a EntryAbility -b dev.litecode.ohos
```

与 spike 的差异只有 bundleName 和路径；`assembleHap` 参数保持 `README.md:22-24` 的原样。

### 6.2 签名

| 目标 | 是否需要签名 | 做法 |
|---|---|---|
| 模拟器（`Relay_Phone_API26`） | 否 | `hdc install -r` 装 unsigned HAP（spike 已验证） |
| 真机 debug | 是 | DevEco “自动签名”（华为账号 + 设备 UDID）；证书/profile 落本地，**不进仓库** |
| 真机 release / 上架 | 是 | AGC 发布证书 + profile + AppGallery 审核；P6 不做 |
| CI | 首版不做 | 只上传 unsigned HAP artifact；签名密钥走 secrets 是 Phase 5 的独立任务 |

`apps/ohos/build-profile.json5` 保持 `"signingConfigs": []`。DevEco 自动签名可能会把材料写回该文件，建议：

- 提交前检查 `git diff` 是否出现 `storePassword`/`keyPassword`；
- CI 增加一个 `grep` 守卫，发现签名材料字段即 fail；
- 文档写明“本地签名后再还原 build-profile.json5”。

### 6.3 CI 建议

| 工作流 | 触发 | 内容 | 是否阻断 |
|---|---|---|---|
| `ci.yml` web job | 现有 | 追加 `pnpm --filter @lite-code/web build:ohos` | 是（纯 Vite，无 SDK 依赖） |
| `.github/workflows/ohos.yml` | `workflow_dispatch` / 可选 nightly | 自托管 Windows runner：`build.mjs` → `hvigorw assembleHap` → 上传 HAP | 否 |
| `release.yml` | tag | 暂不加入 HAP | — |

理由：DevEco SDK 体积大、安装涉及许可，GitHub 托管 runner 不适合；自托管 runner 需预装 DevEco 26/API 26 并固定版本。HAP 不进 release assets，直到签名与分发流程确定。

缓存：`~/.ohpm`、`HVIGOR_USER_HOME`；不要缓存 `entry/build`（增量构建容易掩盖资源同步问题）。

### 6.4 版本

- `AppScope/app.json5` 的 `versionName` 与 root `package.json` 对齐；
- `versionCode` 用固定规则（如 `1000000 + preview 序号`），写在 `scripts/ohos/build.mjs`；
- 扩展 `scripts/check-version.mjs` 读取 `apps/ohos/AppScope/app.json5`（用 regex 避免引入 JSON5 解析依赖），否则版本漂移不会被 CI 抓到。

---

## 7. 风险与未知项

| # | 风险 | 影响 | 缓解 | 验证 |
|---|---|---|---|---|
| 1 | `lc-core` 在 OHOS 交叉编译失败（ring/rustls、notify、zip） | 真 core/对话被阻断 | P0 0.5d 探针；feature 裁剪；TLS 换 OpenSSL；最终 fallback stub agent | 两条 target 的 `cargo build` |
| 2 | NAPI threadsafe function 生命周期/页面重载 | 崩溃、事件丢失 | `napi_add_env_cleanup_hook`；页面 detach 时缓冲事件；`dispose` 幂等 | 反复前后台/刷新 20 次 |
| 3 | 文件访问模型：picker 返回 URI，拿不到 POSIX 路径 | 无法打开用户真实仓库 | MVP 用沙箱 sample workspace；Phase 5 做 URI-aware fs 或“导入到沙箱” | 真机 picker 探针 |
| 4 | 对话输入框的真实 IME | 中文输入不可用 = 产品不可用 | 早期真机验证；不要用 `uitest uiInput` 作为唯一证据 | 真机人工输入中文 |
| 5 | Monaco 替代渲染 | 代码可读性差 | MVP `<pre>`；Phase 2 `highlight.js`；不引入 CodeMirror（与 `docs/README.md:31` 冲突） | 真机滚动/大文件 |
| 6 | 内存 | 手机 OOM/后台被杀 | 不加载 Monaco；只读预览截断；测量 PSS | 模拟器 + 真机 PSS |
| 7 | 虚拟 origin 拦截边界（dynamic import/worker/fonts/404） | 白屏 | P0 逐项探针；失败则内嵌 HTTP server fallback | 断网 + 清缓存启动 |
| 8 | 签名/分发 | 只能模拟器 | 首版明确只交付模拟器；真机签名独立排期 | unsigned 安装 + 真机签名各一次 |
| 9 | DevEco/hvigor 版本漂移 | 构建不可复现 | 提交 hvigor wrapper、固定 `modelVersion`、README 写死版本 | 干净机器构建一次 |
| 10 | 事件在后台/重载后丢失 | 对话看起来“断” | 首版前台使用；Phase 5 加事件缓冲/会话快照重放 | 切后台 30s 再回来 |
| 11 | JS bridge 安全边界 | 任意命令/路径穿越 | shim 白名单 + NAPI 白名单 + `ensure_within_root` + 拦截层路径防护 | 代码审查 + 单测 |
| 12 | 明文 API Key 存沙箱 | 密钥泄露面 | 首版不落真实密钥（mock provider）；Phase 4 做密钥存储评审 | 安全审计清单 |

**Monaco 替代的明确结论**：首版不要 Monaco，也不要 CodeMirror。`<pre>` 先跑通；需要高亮时用 `highlight.js` + 已有 DOMPurify。若未来要回到 Monaco，只读模式仍需先过真机内存测量，且要接受 275MB 渲染进程的量级。

**待验证清单（按优先级）**：

1. 虚拟 origin 下 dynamic import / worker / `modulepreload`（P0，1h）。
2. `lc-core` 两条 OHOS target 编译（P0，2h）。
3. `napi_create_async_work` + tsfn + Promise resolve（P0，1h）。
4. `javaScriptProxy` 异步方法必须放 `asyncMethodList`（P0，30min）。
5. WebMessagePort vs `runJavaScript` 事件推送（P0，1h）。
6. 真机 IME（P4，需设备）。
7. picker URI 与 `fileIo` 读取（P5，需设备）。

---

## 8. 1–2 天 MVP 切片

### 目标

在 API 26 模拟器上，交付一个**离线可启动、只读、能对话**的 HAP，证明「资源管线 + 只读 UI + NAPI 命令/事件」三条链路，不追求真实 LLM。

### 范围

**做**：

- `apps/ohos` 骨架 + web 同步脚本 + 虚拟 origin 加载；
- `OhosApp` 只读壳：文件树 + `<pre>` 预览 + `AgentPanel`；
- NAPI `workspace_root` / `fs_read_dir` / `fs_read_file`；
- NAPI 事件通道 + **stub agent**（线程按 20 个 delta 推送，再 `done`；`agent_cancel` 能中断）；
- `plan` 模式锁定、附件按钮隐藏、写操作隐藏；
- 明暗主题、i18n 复用；
- 离线验收 + PSS 记录。

**不做**：

- 真实 provider/LLM；真实工作区选择器；历史原生持久化；语法高亮；搜索；Git/DB/Task/LSP/Extensions；签名/真机；AppGallery。

### 两天安排

| 时间 | 任务 | 产出 |
|---|---|---|
| D1 AM | 复制 spike 骨架 → `apps/ohos`；改 bundleName/deviceTypes；补 hvigor wrapper；实现 `onInterceptRequest` + MIME + 404 | 模拟器能离线打开一个最小 `ohos.html` |
| D1 PM | `build:ohos` + `scripts/ohos/build.mjs`；`OhosApp` + `ReadonlyCodeView`；sample workspace（3 个文件） | 文件树/预览可见（数据可先写死） |
| D2 AM | `crates/lc-ohos` + C++ NAPI + `tauriShim`；接通 `workspace_root`/`fs_read_dir`/`fs_read_file` | 真 fs 数据渲染 |
| D2 PM | tsfn 事件 + stub agent + cancel；验收、PSS、记录 go/no-go | 对话流式可见；HAP 可安装 |

### 验收清单

1. 干净环境执行 `node scripts/ohos/build.mjs` 产出 HAP；`hdc install -r` 成功。
2. 关闭宿主 Vite、模拟器断网，App 仍能启动并渲染（证明 rawfile 虚拟 origin）。
3. 文件树至少展开 3 层，打开文件显示只读内容，无保存/重命名入口。
4. 输入 prompt 后 ≥20 个 delta 到达并以 `done` 结束；取消能在 1s 内产生 `session_state(cancelled)` + `done(cancelled)`。
5. `grep monaco dist-ohos` 为空；PSS 目标：应用 + ArkWeb render 合计 < 150MB（模拟器）。
6. 前后台切换 10 次不崩溃；页面刷新后重新订阅事件不崩。
7. `docs/STATUS.md` 增加一行“P6 鸿蒙外壳 MVP：只读+stub 对话，模拟器验收”，并记录未解项。

### 演示脚本

1. 启动 App → 看到项目名与文件树；
2. 点 `src/main.rs` → 预览代码（只读）；
3. 切到对话 → 输入“解释这个项目” → 看到流式 stub 回复；
4. 点取消 → 流停止、状态复位；
5. `hdc shell` 查看 `libentry.so` 已加载、handle 正常。

### Go / No-Go

- 虚拟 origin 失败 → 转方案 C（内嵌 HTTP server），MVP 延长 1–2 天；
- tsfn 崩溃 → 先退化为 `runJavaScript` 轮询事件，记录风险；
- `lc-core` 编译失败 → MVP 用 std fs + stub，Phase 4 先做依赖裁剪/TLS 选型；
- 上述都过 → 进入 P2/P3 正式排期。

---

## 9. 假设与待确认

**假设**：

- 开发机与 spike 相同：DevEco 26 / API 26，模拟器 `Relay_Phone_API26`（x86_64），真机为 arm64；
- P6 不要求 AppGallery 上架，只要求可安装、可演示；
- 首版设备形态为 phone/tablet/2in1，不做 wearable；
- 首版不引入第三方 ohpm 依赖；
- 中文输入法在真机可用是产品前提，若不可用需要重新评估“对话优先”的交互（例如改用 ArkTS 原生输入框覆盖层）。

**待探索（若继续推进，建议交给 explore/实现 agent）**：

- `apps/web` 是否有现成的移动端样式断点可复用（目前只看到 `@media (max-width: 1100px)` 的右侧栏浮层，`App.vue:2549`）；
- `AgentPanel` 在窄屏下的最小可用布局，是否需要拆分出 `AgentComposer` 子组件；
- `lc-core` feature 化的具体拆分边界（`fs/watch`、`agent/llm`、`runtime`）与 `crates/lc-ohos` 的 feature 组合；
- 真机 picker 的 URI 能否直接交给 `fileIo` 读取，以及是否需要“导入到沙箱”的产品决策。

---

## 10. 参考

- spike 结论与命令：`spikes/harmony-arkweb/README.md:22-53`
- 鸿蒙门禁与降级决策：`docs/MILESTONES.md:184-190`；`docs/STATUS.md:68`；`docs/ARCHITECTURE.md:210-226`
- 现有前端桥接/事件：`apps/web/src/services/bridge.ts:207-210`、`:1294-1363`
- Monaco 动态加载：`apps/web/src/components/CodeEditor.vue:311`
- App.vue 模式与启动扇出：`apps/web/src/App.vue:133-144`、`:280`、`:897-924`
- 只读策略：`crates/lc-core/src/agent/policy.rs:125-137`
- agent 事件 sink：`apps/desktop/src-tauri/src/agent.rs:162-179`、`:544-583`
- 桌面命令面：`apps/desktop/src-tauri/src/lib.rs:798-865`
- fs 按层读取：`crates/lc-core/src/fs/listing.rs:25-56`
- ArkWeb 本地资源跨域官方解法（虚拟域名 + `onInterceptRequest` / `setPathAllowingUniversalAccess`）：[seaxiang 转述](https://www.seaxiang.com/blog/qEKFw5)
- Rust OHOS target 与工具链要求：[rustc book: `*-unknown-linux-ohos`](https://doc.rust-lang.org/rustc/platform-support/openharmony.html)
- 未签名 HAP 仅模拟器、真机需签名：[社区构建 runbook](https://github.com/Xaiver03/heyta/blob/main/docs/runbooks/multi-platform-build.md)
