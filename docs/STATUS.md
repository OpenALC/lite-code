# 工作进度快照（交接文档）

> 本文件是会话间的工作状态交接，由主 agent 维护；里程碑的权威定义仍在
> MILESTONES.md。快照时间：2026-11-11，main HEAD `9f2a67b`（见 `git log`）；发布版本 `0.6.1-preview.66`，契约 **v0.9**（v0.8 LSP DTO / v0.9 db 远程字段）。
> P3 发布：`v0.3.1-preview.1` draft 已产出 6 个资产。
> P4 发布：`v0.4.1-preview.2` draft 已产出 6 个资产（NSIS/AppImage/deb/两个 CLI/SHA256SUMS）。
> P0 发布线：tag `v0.0.1-preview.2` + draft release 已产出。
> P1 发布线：`v0.1.1-preview.1` draft 已按正确资产清单重建。
> P2 发布线：`v0.2.1-preview.1` release 成功，6 个资产 draft 已产出（发布流程修复后首个自动 draft）。
> P6 发布线：`v0.6.1-preview.4` 已发布（10 资产，含 arm64）；`preview.4/7/8/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31/32/33/34/35/36/37/38/39/41/42/43/44/45/46/47/48/49/50/51/52/53/54/55/56/57/58/59/60/61/62/63/64` 已发布；`.40` 因 linuxdeploy 故障跳过（内容并入 .41）；滚动清单 updater-manifest 已提升到 .64；鸿蒙手机/平板模拟器端到端全绿（PSS 已实测）。

## 当前位置一览

| 事项 | 状态 |
|---|---|
| P0 地基代码 | ✅ 全量完成 |
| P0 发布 | ✅ `v0.0.1-preview.2` draft：Windows NSIS、Linux AppImage/deb、两个 `lc-cli`、SHA256SUMS |
| P1 第一批三 lane（fs / editor / ci-perf） | ✅ 实现+审查+已提交 |
| P1 第二批三 lane（process / runtime / bridge） | ✅ 实现+审查完成 |
| 契约（lc-proto） | ✅ v0.2：`workspace_root`、`workspace_open`、`fs_write_file`、export TS 绑定 |
| 编辑器保存 | ✅ 真实文件打开、编辑、窗口级 Ctrl+S、dirty、fallback 提示 |
| 大文件分级 | ✅ >5MB 降级能力、>20MB 只读预览并阻止保存 |
| per-tab model | ✅ 每标签独立 Monaco model，undo/光标/滚动保留；关闭即释放 |
| dirty 基线 | ✅ `savedContent` 差异推导 dirty；撤销回已保存内容自动变干净 |
| 布局 | ✅ 活动栏 + 侧栏 + 编辑器 + 右侧功能栏 + 底部面板；分隔条可拖拽、可折叠；localStorage 持久化 |
| 打开工作区 | ✅ 契约 v0.2 `workspace_open`；根由 Rust 状态持有并持久化；`workspace_pick` 原生文件夹选择器；侧栏显示项目名 |
| 窗口生命周期 | ✅ 关闭销毁 WebView、托盘/单实例按配置重建；标签内容在销毁前持久化并在重建后恢复 |
| 构建正确性 | ✅ `lc-desktop` 启用 `tauri/custom-protocol`；perf.yml 先构建前端 dist，避免空 WebView |
| 性能 | ✅ 托盘 RSS 29.5MB / 冷启动 48ms / 热启动 47ms / 20 次开关残留 0；activeRss 无脚本入口未测 |
| 10k 文件展开 | ✅ `lc-cli fs read-dir` 首层 1 万文件 87/96/97ms（目标 <200ms） |
| headless CLI | ✅ `fs read-dir` 复用 UI 同一实现，输出契约 JSON |
| P1 发布 | ✅ `v0.1.1-preview.1` draft：NSIS、AppImage、deb、两个 `lc-cli`、SHA256SUMS（6 个资产） |
| P2 发布 | ✅ `v0.2.1-preview.1` draft：NSIS、AppImage、deb、两个 `lc-cli`、SHA256SUMS（6 个资产） |
| 发布流程 | ✅ 修复 AppDir 递归上传与重名；checksums/资产名统一；P2 自动 publish 成功 |
| P3 Tools+Policy | ✅ 工具注册表（≤12）+ 四模式策略 + 三层判定的纯函数骨架与单测 |
| P3 会话审批 | ✅ 模式/yolo 确认、轮次上限、取消、审批队列、AllowOnce/AllowSession/Deny |
| P3 工具执行 | ✅ read/write/edit/glob/grep（根内校验、输出截断）；shell 待 PTY lane |
| P3 LLM 解析 | ✅ SSE 增量解析 + OpenAI Responses 事件归一化（文本/思考/工具/usage/停止原因） |
| P3 LLM 传输 | ✅ provider/model 配置校验、Responses 请求体、可注入客户端 + ureq 流式传输 |
| P3 契约 v0.3 | ✅ session_create/agent_send/agent_cancel/approval_resolve + thinking/tool_result/approval/session_state 事件 |
| P3 事件聚合 | ✅ TurnCollector：文本/思考、工具参数拼装、usage、stop reason |
| P3 Anthropic | ✅ Messages SSE 映射 + 状态化解码器；ureq loopback 传输测试 |
| P3 AgentLoop | ✅ user → LLM 流 → TurnCollector → 授权 → 工具执行 → function_call_output 回填；审批 resume |
| P3 shell | ✅ PowerShell 7 / bash、超时杀进程树、stdout/stderr 截断、Job/进程组回收 |
| P3 Agent UI 状态 | ✅ agentStore reducer（文本/思考/工具/审批/usage/done/compacted）+ 测试 |
| P3 Tauri 命令层 | ✅ 每会话 worker 线程、四条命令、`core:event` 信封、取消/审批 resume |
| P3 CLI | ✅ `lc-cli agent run --prompt ... --json/--yes`，headless 审批与退出码 |
| P3 Agent UI 视图 | ✅ 会话流、工具卡、内联审批三按钮、composer、yolo 确认、上下文刻度条 |
| P3 上下文压缩 | ✅ 预算/阈值、工具输出瘦身、早期摘要、激进压缩、`agent:compacted` 记录；工具配对安全 |
| P3 MCP | ✅ `.mcp.json`/用户配置、stdio JSON-RPC、握手/工具列表/调用、动态工具注册与执行 |
| P3 环境契约/记忆 | ✅ 平台/shell/工作区 XML 注入；全局+项目 memory.md、AGENTS.md/CLAUDE.md 读取 |
| P3 发布 | ✅ `v0.3.1-preview.1` draft：NSIS、AppImage、deb、两个 `lc-cli`、SHA256SUMS（6 资产） |
| P4 契约 v0.5 | ✅ Task DTO、8 个 task 命令、`task:*` 事件；`session_create` 可绑定 task |
| P4 Task 核心 | ✅ 持久化状态机、队列（默认 2）、端口、worktree、changes/diff/apply/discard |
| P4 合并协议 | ✅ 主工作区 clean、基线漂移、`--no-ff`、冲突保留、解决后再次 apply 收尾 |
| P4 Tauri 命令层 | ✅ 8 个 task 命令与事件；session 在 worktree 内运行；后台审批转 Blocked + needs_input |
| P4 工作台 UI | ✅ 任务列表、三栏布局、变更/检查/Diff/预览标签、应用/丢弃、状态栏待确认 |
| P4 预览/标签 | ✅ 标签组按任务恢复；iframe 切换销毁、回到任务用原 URL 重建 |
| P4 发布 | ✅ `v0.4.1-preview.2` draft：NSIS、AppImage、deb、两个 `lc-cli`、SHA256SUMS |
| P5 契约 v0.6 | ✅ DB DTO、9 个 db 命令、`db:status`、`db_*` 错误码；TS 绑定已导出 |
| P5 SQL 闸门 | ✅ 单语句、分类、无 WHERE 拦截、静态黑名单、LIMIT 注入、只读拒绝（60 测试） |
| P5 SQLite sidecar | ✅ Node `node:sqlite` NDJSON JSON-RPC；connect/query/execute/schema/explain；超时/回收 |
| P5 DB 服务/工具 | ✅ DbService 配置持久化、连接管理；Agent `db_*` 工具与执行器路由 |
| P5 DB UI | ✅ 连接列表、schema 树、SQL 编辑器、结果网格、EXPLAIN、untrusted 提示 |
| P5 Agent DB 闭环 | ✅ 真实 SQLite：Agent 建表 → 写入 → 查询（`lc-core/tests/db_agent.rs`） |
| P5 Git | ✅ status/diff/stage/unstage/commit/log 服务与 Tauri 命令；Git 面板 |
| P5 发布 | ✅ `v0.5.1-preview.1` **已发布**（非 draft）：6 资产 |
| P6 鸿蒙 | ✅ 外壳 MVP + NAPI 真实 core + providers + 真实 Agent 对话 + 离线回退 ~1.3s + 会话回收 + 历史持久/迁移/管理 + UI 抛光（preview.51–.63）：手机/平板模拟器端到端全绿（平板三栏 2880×1920 验证通过）；PSS 实测：冷启动 ~300MiB（ArkWeb 引擎基线 ~134MiB 结构性）、3 大文件 ~357MiB、5 轮对话 ~377MiB（原 150MiB 目标不可达，改按增量管理）；待办：真机签名（维护者，walkthrough 已备 `scripts/ohos/signing.md`）、2in1 形态（镜像就绪，DevEco GUI 创建设备）、lc-host 抽取 M3（TurnRunner+desktop，M1/M2 已完成） |
| P6 本地安全整改 | ✅ CSP frame-src/object-src；DB/MCP 输出不可信边界；审计范围写入 `docs/SECURITY-AUDIT.md` |
| P6 A 类 arm64 | ✅ `v0.6.1-preview.3` tag 构建成功：aarch64 AppImage/deb、`lc-cli-arm64` 已发布 |
| P6 A 类 更新器 | ✅ `update_check` + `latest.json` 生成器 + 回滚文档；无签名密钥时 `signed:false`，签名门禁未完全关闭 |
| P6 审计修复 | ✅ Critical：MCP 项目配置默认不执行、SQL 无 WHERE 绕过、批量工具审批绕过；High：VCS 写保护、取消、记忆路径、发布校验、DB 审批按连接隔离、子进程密钥清理 |
| P6 审计 Medium | ✅ CREATE TRIGGER DML 检查、LIMIT 识别、bridge 流式取样、不可信边界转义、task key 稳定化、写文件审批内容摘要 |
| P6 依赖审计 | ✅ `dompurify` override 3.4.17；`pnpm audit --prod` 无已知漏洞 |
| P6 发布 | ✅ `preview.4/7/8/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31/32/33/34/35/36/37/38/39/41/42/43/44/45/46` 已发布；`.40` 跳过（内容并入 .41）；`.47` 发布中；滚动清单 updater-manifest 已提升到 .46 |
| UI 重构 | ✅ VS Code 式活动栏/侧栏/编辑器/右侧功能栏；自定义标题栏、亮暗主题、缩放、设置页、快速打开、原生文件夹选择、打开编辑器列表、文件树拖拽与右键、编辑器中文右键 |
| 工作区搜索 | ✅ `fs_search`（字面量/正则/大小写、gitignore、2MiB/二进制跳过、500 行上限；异步不阻塞 UI）+ 搜索侧栏 + 命中跳转 |
| 设置页 | ✅ 编辑器区独立页签：主题/语言/缩放/字号/Tab/换行，实时应用并持久化；版本号由 Vite 注入 |
| DB 驱动 | ✅ PostgreSQL（pg）/ MySQL（mysql2）/ SQL Server（mssql）打入 2.3MiB 单文件 bundle；SQLite 保留 |
| P6 其余 | ⏳ B 类（soak、第三方审计、签名、Win11 验收）由维护者关闭；更新器 `signed:false` |
| Runtime 设置页 | ✅ Ready/NeedManual、候选失败原因、手动路径、便携版下载 |
| main CI | ✅ preview.23–.63 CI 全绿 |
| UI 收尾 | ✅ `81526e3`：截断文案按实际条数展示、快速打开 Tab 失焦关闭；已并入 preview.8 |
| 供应商管理 | ✅ 左侧活动栏「供应商」页签：Base URL/API Key/模型列表/默认与 Subagent 模型；密钥脱敏、tmp 0600、备份脱敏 |
| Agent 模型/思考 | ✅ 会话前选择模型与思考挡位，`session_create` 透传；无会话显示上下文预算与新会话按钮 |
| Agent 附件 | ✅ `agent_pick_files` 读取 UTF-8 文本，10 个/256 KiB/总 1 MiB；chips 展示与移除 |
| 选区加入 Agent | ✅ 编辑器右键“添加到 Agent 对话”，路径:行号 + 围栏代码块；演示缓冲区回退标题、20k 截断 |
| Agent UI（Demo 对齐） | ✅ 会话头/消息流/工具卡/审批卡/压缩行/一体化 composer（pill + 上下文条）/附件 chips；AI 消息安全渲染 Markdown |
| Agent 流滚动 | ✅ `.stream` 子项 `flex: none`，工具卡/审批卡多时不再被压缩；23 条会话滚到底验证 |
| Agent 会话历史 | ✅ 快照落盘 `data_dir/agent-history`，列表/切换/删除；历史视图只读，续聊新建会话并注入最近 12 条上下文 |
| Agent 状态与用量 | ✅ 思考/生成/工作/等待审批/完成/出错；每轮输入/输出/缓存 token 与耗时 |
| 双模式布局 | ✅ 活动栏「编辑器 / Agent」切换并持久化；Agent 模式为会话列表 + 全宽对话 |
| Agent 工作区 | ✅ 左侧工作区列表（打开/切换/移除），会话按 workspace 归档；旧历史启动时迁移；运行/审批中禁止切换 |
| Agent UI（Kimi） | ✅ 中性色板、无活动栏/状态栏、项目分组对话 + 搜索、48px 顶栏、860px 居中对话与 composer |
| 编辑器 UI（VSC） | ✅ 48px 活动栏、34px 标签/顶部强调、悬停关闭、面包屑、34px 面板标签、标题栏模式切换 |
| Agent 功能移植 | ✅ 运行中排队并自动续发、对话重命名/归档/恢复、上下文详情浮层、助手消息复制 |
| Agent Git 侧栏 | ✅ 头部 Git 按钮打开 420px 变更栏（复用 GitPanel：暂存/提交/日志/Diff）；工作区切换刷新 |
| 队列编辑 | ✅ 点击排队消息载回 composer（含附件）并从队列移除 |
| @ 文件引用 | ✅ composer 输入 @ 按需索引工作区（≤2000/深度 6），跳过工具/构建目录；键盘选择插入相对路径 |
| Agent 预览侧栏 | ✅ http(s) URL、协议校验、iframe sandbox、刷新/清除、最近地址 localStorage |
| 消息分叉 | ✅ 用户消息编辑并分叉、助手消息分叉重试；截断上下文/清理工具与审批，原会话保留 |
| Agent 偏好 | ✅ 设置页发送键/密度/字号 12–18；订阅实时应用，持久化 lc.ui.prefs |

## 本轮完成

1. **契约 v0.1**（`7b0ce1e`）
   - `workspace_root` 由 Rust 注入，fs 命令不再信任前端 root。
   - `fs_write_file` 允许新建，校验父目录/符号链接根内；codegen 改为 `export type`。
2. **编辑器保存**（`0243d6d`）
   - 标签 store 持有 `path/content/dirty`；CodeEditor 跟随激活标签；窗口级 Ctrl+S。
3. **Runtime 设置页**（`6566ffb`）
   - Tauri `node_set_manual` / `node_download_portable`；设置面板与 bridge 包装。
4. **安全与形状修复**（`1e0f742`）
   - 保存改为临时文件 + fsync + rename，保留原权限；拒绝悬空软链写穿。
   - Tauri `node_detect` / `shell_detect` / `contract_info` 统一返回 `CommandResult`，修复 bridge 形状错配。
   - 浏览器 fallback 不清 dirty；release publish 逐文件上传并允许 sbom 缺失。
5. **P0 release**
   - `preview.2` 构建成功但 workflow publish 因 sbom 缺失失败；已用现有产物手动创建 draft，并修正 main 的 publish 逻辑供后续版本使用。
6. **大文件分级接线**（`4f4b1e9`）
   - 标签携带 `EditorFileStrategy`；>5MB 关闭 occurrences/折叠/minimap，>20MB 只读并阻止 Ctrl+S 写盘。
7. **per-tab Monaco model**（`4ee477e`）
   - 每标签独立 model、view state 与内容监听；切标签 setModel，关闭/卸载释放。
   - 浏览器实测：切标签后内容保留，Ctrl+Z 可撤销切标签前的输入；占位树不再永远“读取中”。
8. **dirty 基线**（`b4dedce`）
   - `savedContent` 作为落盘基线，`dirty = content !== savedContent`；撤销回已保存内容自动变干净。
9. **布局**（`2a6818b`）
   - `LayoutPrefs`：侧栏/Agent/底部尺寸与折叠；clamp + localStorage 持久化。
   - 三个拖拽分隔条、Agent/底部折叠与恢复；dockview 留到 P4 工作台。
10. **打开工作区**（`ce62f52`）
   - 契约 v0.2 `workspace_open`；Tauri `WorkspaceState` 持有根并持久化到 config。
   - 顶栏路径输入 → 校验目录 → 文件树切换根；fs 命令统一从状态取根。
11. **窗口销毁/重建**（`4dc4a0b`）
   - 关闭请求 `window.destroy()` 释放 WebView2；托盘/二次启动按配置重建。
   - 无窗口时阻止 ExitRequested；标签内容/基线/档位在销毁前持久化，重建后恢复。
12. **构建正确性**（`9d23c4a`）
   - `lc-desktop` 增加 `custom-protocol` feature，生产包嵌入前端资源。
   - perf.yml 先构建 dist，再用 `-p lc-desktop --features custom-protocol`；裸 `cargo build` 的空 WebView 不再用于测量。
13. **headless fs / 10k 文件**（`efe8425`）
   - `lc-cli fs read-dir` 复用 UI 同一 `read_dir_level`，输出契约 JSON。
   - 本机 1 万文件首层列举 87/96/97ms，低于 200ms 目标。
14. **P1 性能收尾**（`7ce641d` / `1d68600`）
   - 生产构建（custom-protocol）实测：托盘 29.5MB、冷启动 48ms、热启动 47ms、20 次开关残留 0。
   - `scripts/perf/RESULTS.md` 记录口径与构建证据；baseline 已收录四项非 null 指标。
15. **P3 Tools+Policy 骨架**（`9ef9fda`）
   - `ToolRegistry`：≤12、静态风险、MCP 动态工具占位。
   - `Policy`：plan/chat/auto/yolo；静态+用户黑名单；写内/写外；模型风险 fail-closed。
   - 纯函数与 10 个单测；Rust 侧闸门先于 LLM loop。
16. **P3 会话审批**（`5506d1d`）
   - `Session`：模式（yolo 二次确认）、轮次上限、取消、待审批阻塞。
   - `ApprovalQueue` + `ApprovalCache`：AllowOnce / AllowSession（tool+command 同类）/ Deny。
   - 9 个状态机单测；任何工具执行前必须先 authorize。
17. **P3 工具执行**（`37deae5`）
   - `LocalToolExecutor`：read/write/edit/glob/grep，全部先经根内校验，输出截断。
   - edit 拒绝 0/多次匹配，`replace_all` 显式开启；shell 明确 Unsupported 待 PTY lane。
   - 5 个临时目录集成测试；lane 脚本修复 `pub use {}` 展开误报。
18. **P3 LLM 解析**（`8b5df1d`）
   - `SseParser`：增量 chunk、CRLF、多行 data、注释、finish 兜底。
   - OpenAI Responses 映射：text/thinking、tool start/args、usage、stop reason、failed。
   - 8 个协议/映射单测；网络传输与 Anthropic Messages 留后续 lane。
19. **P3 LLM 传输**（`438452e`）
   - provider/model 配置（camelCase、预算/重复/引用校验）。
   - `LlmRequest` → Responses body；`OpenAiResponsesClient` + 可注入 `HttpSseTransport`；ureq 真流式读取。
20. **P3 契约 v0.3**（`72326e0`）
   - session/agent/approval 命令；thinking/tool_result/approval_required/session_state 事件。
   - DTO 绑定与 golden 同步，命令数 14。
21. **P3 事件聚合**（`1844e71`）
   - `TurnCollector`：assistant/thinking、工具 start+args、usage、stop reason，单测覆盖错误路径。
22. **P3 Anthropic / loopback**（`d92fa8b`，subagent 交付）
   - `AnthropicDecoder`：message/content_block/message_delta/stop 关联，tool id 状态化。
   - `ProviderKind::AnthropicMessages`；ureq 本地 loopback（Content-Length + chunked）测试。
   - 14 个新增测试；真实 Anthropic 端点未做外网集成。
23. **P3 shell 执行**（`64baaf8`，subagent 交付，合入 `2c19d60`）
   - PowerShell 7 / bash 探测与执行；默认 30s、上限 120s，超时杀进程树。
   - stdout/stderr 采集与截断；Windows Job Object / Unix 进程组回收。
24. **P3 AgentLoop**（`ecf31e0`）
   - LLM 流 → TurnCollector → 授权 → 工具执行 → `function_call_output` 回填。
   - 审批 resume、取消、轮次上限；测试用 FakeLlm/FakeExecutor 覆盖工具轮与终止路径。
25. **P3 Agent UI 状态层**（`8b53b75`，subagent 交付）
   - agentStore reducer：文本/思考 delta、工具卡片、审批、usage、done、session_state。
   - 23 个 reducer 单测；尚未接 React 组件。
26. **provider 运行时与工具定义**（`b85fe64`）
   - `providers.json` + `LC_PROVIDERS_FILE`/`LC_API_KEY`/`LC_BASE_URL`/`LC_MODEL` 兜底。
   - `ToolRegistry::llm_tools()` 输出 Responses API 的 6 个工具 schema，CLI/Tauri 共用。
27. **事件映射与工具失败恢复**（`56b698a` / `659d99f`）
   - `permission_denied` 错误码；`llm_event`/`loop_outcome`/`session_info` 统一映射。
   - 工具失败回填 `function_call_output` 并继续，observer 暴露 call id 供 `agent:tool_result`。
28. **Tauri 命令层 + CLI + UI 接线**（`0c222a3` / `3dab4cb` / `5318535`）
   - 每会话 worker 线程；`session_create`/`agent_send`/`agent_cancel`/`approval_resolve`；`core:event` 信封。
   - `lc-cli agent run`：JSON 事件流、`--yes`、退出码 0/1/2。
   - AgentPanel：模式栏、工具卡、审批三按钮、composer、yolo 确认、浏览器 fallback。
29. **契约 v0.4 与上下文压缩**（`058840e` / `f33f516` / `81abd8d` / `28e86b6`）
   - `SessionInfo.context_k/compress_at_k`、`agent:compacted`。
   - 压缩顺序：旧工具输出瘦身 → 早期摘要 → 激进压缩；切点保证 call/output 配对。
   - UI 阈值刻度、超阈值变黄、压缩后闪烁与可展开记录；CLI/Tauri 均发射压缩事件。
   - 本地 mock Responses 二轮闭环：读两个 12KB 文件 → 压缩 6572→3583 tokens → 继续并 done。
30. **MCP Host**（`d2951f6`，subagent 交付）
   - `.mcp.json` + `~/.lite-code/mcp.json` 合并；stdio JSON-RPC 握手、`tools/list`、`tools/call`、超时与进程树回收。
   - `mcp_<server>_<tool>` 动态注册（`ToolRegistry::with_mcp_tools` 替换 `mcp_tool` 占位）；`McpExecutor` 路由 + 内置执行器 fallback。
   - Python stdio 假 server 集成测试；CLI 闭环：模型请求 `mcp_echo_echo` → `echo:hi` → `done`。
31. **环境契约与记忆**（`774e7dd`）
   - system prompt 注入 `<environment>`（平台/架构/shell 方言/路径分隔符/工作区/PATH 工具）。
   - 全局 `~/.lite-code/memory.md`、项目 `.lite-code/memory.md`、`AGENTS.md`/`CLAUDE.md` 读取并注入；追加写入走 `.bak` + 原子替换。
32. **P3 发布**（`v0.3.1-preview.1`）
   - 6 个 draft 资产：Windows NSIS、Linux AppImage/deb、两个 `lc-cli`、SHA256SUMS。
33. **P4 契约与 Task 核心**（`3b69c26` / `e064d99`）
   - 契约 v0.5：Task DTO、8 个 task 命令、`task:*` 事件；`session_create` 支持 task 绑定。
   - TaskStore：持久化、并发 2、端口、worktree、changes/diff/apply/discard、队列提升。
34. **P4 Tauri 与工作台 UI**（`a8a0c92` / `40473bd`）
   - 8 个 task 命令与事件；session 在任务 worktree 内运行；后台审批 → Blocked + `task:needs_input`。
   - 任务列表、三栏工作台、变更/检查/Diff/预览、应用/丢弃、状态栏待确认。
35. **P4 冲突收尾与预览存活**（`cd9b782` / `cfde355` / `853d605`）
   - 冲突解决/仅暂存后再次 apply 可收尾；同文件不同行并行合并；基线漂移计数。
   - 工作台标签按任务恢复；iframe 切换销毁、回到任务用原 URL 重建。
36. **P4 发布**（`v0.4.1-preview.2`）
   - 首次 preview.1 因 CI Windows 路径断言失败；改为 canonicalize 比较后 preview.2 成功，6 个 draft 资产。
37. **P5 契约 v0.6 + SQL 闸门**（`4e12255` / `cbb740a`）
   - DB DTO/9 命令/`db:status`/`db_*` 错误码；TS 绑定导出。
   - 单语句、读写/DDL 分类、无 WHERE 拦截、黑名单、LIMIT 注入、只读拒绝；60 测试。
38. **P5 SQLite sidecar**（`a2af30c`）
   - Node `node:sqlite` NDJSON JSON-RPC；connect/query/execute/schema/explain；超时与进程回收；14 测试。
39. **P5 DB 服务/工具/Tauri/UI**（`70aabfc` / `f80cd2a` / `436f621`）
   - DbService 配置持久化与连接管理；Agent `db_*` 工具；Tauri 9 命令；连接列表/schema/SQL/结果网格。
40. **P5 Git 服务与面板**（`2bd28cb` / `ad0b696`）
   - 主工作区 status（staged/unstaged、ahead/behind、merge/rebase）、diff、stage/unstage、commit、log；Tauri 6 命令与 Git 面板。
41. **P5 Agent DB 闭环**（`crates/lc-core/tests/db_agent.rs`）
   - 真实 SQLite：Agent 调 `db_execute` 建表/写入、`db_query` 查询，输出回填后正常结束。

## 已知 P1 缺口（审查结果，按优先级）

1. 无外部修改冲突检测（savedContent 基线已有；mtime/hash 契约字段未做）。
2. 托盘“退出”绕过 webview 脏检查，可能丢未保存缓冲。
3. 原生文件夹选择器未接；当前打开工作区为路径输入。
4. `activeRssMb` 缺 UI 自动化入口未测；cold/warm 终点是“窗口可见”而非应用内“可交互”。
5. `@lc-proto` 目前只有 tsconfig 别名，未来若生成运行时常量需补 Vite alias。

## P6 审计与门禁进展（2026-11-06）

- 独立审计（只读）发现 3 Critical / 6 High / 10 Medium / 6 Low；完整报告在会话记录，范围见 `docs/SECURITY-AUDIT.md`。
- 已合入修复：
  - MCP：项目 `.mcp.json` 默认忽略（`LC_MCP_ALLOW_PROJECT=1` 显式开启）、环境 allowlist、server 上限 8、1MiB 上限。
  - SQL：SQLite 方言（`\` 不再转义单引号）、sidecar 强制行数上限、PRAGMA 只读白名单。
  - Agent：审批逐项重新授权、真实参数摘要、`.git`/`.hg`/`.svn` 写保护、取消 token、工具错误不可信边界。
  - 记忆：根校验 + 拒绝 symlink/reparse + 1MiB 上限 + 不可信边界。
  - Task：按 workspace 隔离 `tasks.json`、apply/discard 保护 ignored 文件。
  - 发布：签名 fail-closed、最小权限、更新器 `latest.json`、资产/checksums 一致性校验；CI 增加 audit 与 updater manifest 测试。
- 依赖：`dompurify` 覆盖至 3.4.17，`pnpm audit --prod` 清零。
- 仍待 tag 验证：arm64 runner、release 资产命名、`latest.json` 上传、签名关闭时的 `signed:false` 提示。

## preview.5：UI 打磨与工作区搜索

1. **全文搜索**：`fs_search`（lc-core `fs::search` + Tauri 命令 + bridge + `SearchView`）。
   - 字面量/正则、大小写开关；`ignore` 遍历尊重 `.gitignore` / `.ignore`。
   - 2 MiB / 二进制跳过，单文件 100 行、总计 500 行；预览按字符窗口截取并补省略号。
   - 侧栏分组、命中高亮、行号；点击打开文件并通过 `editorReveal` 定位到命中行。
   - 顶部快速打开新增“在文件中搜索”，把关键词带入搜索侧栏（`seed` 立即生效）。
2. **设置页**：从窄侧栏移到编辑器区独立页签（`__settings__`），关闭/切换行为与普通标签一致。
   - 字号 11–20、Tab 2/4/8、自动换行；`CodeEditor` 实时应用，已有 model 同步 tabSize。
   - 语言切换同步设置页签标题；版本号由 Vite `__APP_VERSION__` 注入。
3. **修复**：刷新按钮误开工作区弹窗、语言按钮无效、状态栏 “main main”、项目名大写/重复、
   Agent 头部裁切、中文界面残留英文、minimap 视觉噪声。
4. **验证**：Tauri/WebView2 真机通过原生选择器打开工作区、搜索命中跳转、右键菜单、
   拖拽移动、主题/语言/缩放、设置页；Git/DB 在右栏不遮挡编辑器。

## preview.8：收尾构建

preview.7 的收尾：搜索结果截断文案按实际 `totalMatches` 展示（单文件 100 行上限不再误写
“前 500 条”）；快速打开支持 Tab 失焦关闭。其余功能与 preview.7 相同。

## preview.7：快速打开关闭行为

preview.6 的基础上补一处交互修复：快速打开面板打开后点击编辑器/文件树/侧栏不会关闭，
会遮住标签栏并保留旧查询。现在用 document `pointerdown`（捕获阶段）判断 `.quick-wrap`
之外的区域，收起并清空查询；面板内条目点击不受影响。

## preview.6：审计修复

独立只读审计确认 2 High / 6 Medium / 2 Low，已逐项修复：

1. `fs_search` 补 `rename_all = "snake_case"`：大小写与 max_results 真正生效；改 async +
   `spawn_blocking`，搜索不再阻塞 Tauri 主线程。
2. Ctrl+, / 帮助 / 状态栏设置入口统一到编辑器区设置页签。
3. 搜索侧栏常驻挂载保留状态；seed 带 seq；root 变化清空结果；工作区切换不显示旧命中。
4. 编辑器定位请求消费一次，设置页返回不重放旧命中。
5. CodeEditor 卸载移除窗口监听，异步 import 后检查 disposed。
6. `truncated` 多探一条，满额与单文件上限判定准确；非法正则错误本地化。

真机复验：大小写 11→0→11，Ctrl+, 设置页，视图切换保留查询；其余 UI 项沿用 preview.5 验证。

## preview.10：供应商安全加固

独立审计 1 High / 3 Medium / 4 Low，已修复：

1. `.tmp` 创建即 0600；加载/运行时收紧旧文件权限；`.bak` 只保留非密钥字段；
   `clearApiKey` 同时清 `apiKeyRef`；`ProviderConfig` Debug 脱敏；短 Key 全掩码。
2. 保存期间 fieldset 禁用防竞态；供应商页常驻挂载保留草稿；模型行稳定 key；
   保存后保留选中供应商；Anthropic 后缀与 Subagent 文案 i18n/如实描述。

真机复验：临时 `LC_PROVIDERS_FILE` 新建/保存/重启/保留/清除/脱敏/Tab 切换。
Linux 额外跑 `saved_files_are_private_and_backup_has_no_key`。

## preview.9：供应商管理

1. **页面**：活动栏 Agent 入口替换为「供应商」，打开编辑器区 `__providers__` 页签；
   供应商列表 + 配置卡 + 模型卡片 + 默认/Subagent 模型。
2. **配置**：`providers.json` 新增可选 `apiKey`，与 `apiKeyRef` 并存；新增
   `subagentModel` / `subagentThinking`。保存走 `write_atomic` + `.bak`，Unix 0600。
3. **安全**：`providers_load` 只返回 `hasApiKey` 与末四位提示；空白 Key 保留旧值，
   `clearApiKey` 显式删除；运行时 `LC_API_KEY` > `apiKey` > `apiKeyRef`。
4. **验证**：临时 `LC_PROVIDERS_FILE` 真机走通新建/保存/重启恢复/保留/清除/脱敏；
   中英文、设置页与 Agent 空态入口正常。Anthropic 仍 fail-closed，Subagent 字段仅持久化。

## preview.11：插件系统 v0

1. **ext lane**：`~/.lite-code/extensions` 清单发现/校验、启用状态、Node 宿主、
   WASM runner、MCP 合并；项目插件默认忽略。
2. **Node 宿主**：独立进程 JSON-RPC，命令/workspace.readFile/storage/log；
   `examples/plugins/hello-node` 真机执行通过。
3. **WASM**：WASI preview1 command，无 preopen/网络，`examples/plugins/hello-wasm`；
   15 项独立测试 + 真机命令通过。
4. **MCP/LSP**：插件 `mcpServers` 合并进 Agent；`languageServers` 启动 stdio LSP，
   `lsp:diagnostics` → Monaco markers，fake server 真机验证。
5. **UI**：活动栏「插件」页、启停/重载/打开目录、命令执行、Ctrl+P 命令；
   设置页/供应商页不回归。
6. **边界**：不承诺 VS Code API；无市场/签名/Webview/树视图；Node 插件为可信代码。

## RELEASE-GATES 复审（随 .45–.63 打 tag 执行，2026-11-10）

- **A 类两项已关闭并持续保持**：#5 arm64（.3 起入矩阵，资产三平台验证）；#6 更新器
  （latest.json + 滚动清单 .63 + workflow_dispatch 人工兜底 + 版本守卫测试；
  `signed:false` 与已知限制逐版披露，豁免条件第 4 条满足）。
- §4 审计豁免条件 1–3：维持满足（SECURITY-AUDIT.md §2/§5、cargo-deny/pnpm audit/
  lane 边界全绿、安全路径有测试）。
- B 类 #1–4、#7 维持「等待维护者」，未触碰 RELEASE-GATES.md 状态列（按其 §6 要求）。

## 审查前自审（09:00 大规模审查输入，05:10 快照）

- **M2 语义等价证据**：39 处机械迁移全部经脚本唯一匹配断言；lc-ohos 36 项
  测试不改语义全过；锁序 1:1（insert+reclaim 同锁、`begin_turn` 查表+CAS
  同锁）；错误文案 `session not found: {id}` / `agent turn already running: {id}`
  逐字保留；alive 迟到抑制/回收只挑 `!running` 不变式由 lc-host 9 项新单测
  钉住（含「全 running 时新会话插入即回收」的真实可达语义）。
- **模拟器证据（M2 HAP）**：b（工具流 716ms end_turn）、c（三轮复用 298ms）、
  d（取消 cancelled）、f（40 会话：ohos-5 not_found、ohos-41 ack）、
  e-online（mock 流式 9 delta 318ms）、e-offline（会话内 5s 在途回退 20 stub）
  + 新会话 800ms 探测回退 803ms 首 delta——两条回退路径分工符合设计
  （预探路径 <2.5s 成立）；hilog 无 panic。
- **M3a 唯一行为差异**：桌面信封 seq 0→1 基；已核实 web 不消费 envelope seq，
  两端形状不变；桌面 100 项测试全绿（两个 seq 断言随语义更新）。
- **RELEASE-GATES**：A 类维持关闭（check-all 含 lanes/cargo-deny 全绿）；
  B 类（soak/审计/签名/Win11）未触碰，按 §6 等待维护者。
- **发布纪律**：M2/M3a 均未推 tag；preview.65 与 M3b/3c 留审查后。

## preview.67 发版中（2026-11-11）

- tag `v0.6.1-preview.67` 打在 `18f7db9`（版本变更+notes，CI 绿、check-version
  通过）——打包插件系统六块收官 + db M3 契约 0.9；notes
  `docs/releases/0.6.1-preview.67.md`；workspace 714 项发版前实测全绿。
  后台发版脚本已修 `/tmp` 路径坑（上轮 step4 假循环根因）。
- **市场 M1 核心已合**（`a1cc559`）：index/源/安装三模块 + version.rs
  单一来源（~3900 行，lc-core 551 项，check-all 全量绿）；GC 挂启动/
  web UI 已随 M1b 落地（`d073d2a`：浏览 tab+§3.7 确认对话框+563 项；
  包大小暂显「未知」待 index 线补 size 字段）；Ubuntu CI 红已修
  （`8173a52` file_url_to_path Unix 绝对路径——教训：file:// 解析必须
  按平台分支断言）。M2 deps 自动装（`5a17798`）/M3 签名强制档+多镜像（`c8ce356`：
  minisign-verify 0.3.0 过 deny，sha256 文本签名+trusted comment 三重
  绑定，镜像 sha 不符也回退）——**市场线 M1-M3 全关**（marketplace
  67 项）。前置：
  **插件市场设计定稿**
  （`docs/plans/plugin-marketplace.md`，agent-92）：
  静态 index+多源镜像（与 updater-manifest 同构，零服务端零新增重型依赖——
  ureq/zip/sha2 已在 lc-core）；签名两档（警告/强制 minisign 分钥）；
  curated PR 制审查；M1 index+安装/M2 更新卸载/M3 签名多镜像（13 点）。
- **preview.67 已发布**（2026-11-11 凌晨）：10 资产 draft=false，
  updater-manifest 提升到 .67（previous=.66，脚本自验真实通过）——
  插件系统六块收官 + db M3 契约 0.9 全量交付。三连发 .65/.66/.67 完成。
- **preview.66 已发布**（前情）：10 资产 + updater-manifest 提升到 .66
  （previous=.65），插件延迟激活随该版交付。

## preview.66 发版中（2026-11-11）

- tag `v0.6.1-preview.66` 打在 `b8041b1`（版本变更+notes 提交，CI 绿、
  check-version --release 通过）——打包 swarm 四轨道 + 设置模态浮层 +
  lc-host M4 关闭；release notes `docs/releases/0.6.1-preview.66.md`；
  workspace 692 项发版前复测全绿。后台发版任务完成：**v0.6.1-preview.66 已发布**（10 资产，SHA256SUMS
  随附；updater-manifest 提升到 .66，previous=.65 回滚通道，signed:false
  与既有披露一致）。插件延迟激活已合（`5bf0c2b`，activationEvents 生效：
  onCommand 懒激活 + `*` 向后兼容，ext 29/29 + host 11/11）。

## 审查回合（2026-11-10 上午，main `30ec40c`）

- **M3b 已按审查合并**（`be84ab6`）：冲突按意见解决（3b 取分支版 lc-core 落点
  + 三终态语义明示；3c 取主线「不做」版）；合并后 workspace 667 全绿，远端 CI 绿。
- **preview.65 notes 按审查修正**（`73373f9`）：亮点区分「M2 鸿蒙会话表迁移」与
  「M3a 两端事件信封复用」；M3c 改「决定不做」；补 M3b 条目与 667 计数。
- **审查意见文档入库** `docs/reviews/2026-11-10-main-merge.md`。
- **设置页重做**（`9b8a51d`，用户直接要求）：改为模式无关模态浮层——任何
  入口（活动栏/状态栏/菜单/Ctrl+,/Agent footer）打开设置不再跳转 VSC 模式；
  新增「供应商」分区内嵌 ProvidersPanel；Agent 侧栏 footer 供应商/设置直达
  浮层。桌面 dev 运行时 e2e 全过（Agent 模式开浮层保持模式、Esc 关闭）；
  web 四件套 + build:ohos 全绿；demo 同步模态化。
- **复审四项已修**（`a31c8cc`，复审文档 `docs/reviews/2026-11-10-main-rereview.md`）：
  供应商编辑单实例（编辑器区页签移除，活动栏入口直达浮层模型分区）；
  demo `?view=settings` 启动中断（模态标记移到脚本前）；浮层焦点管理
  （移入/Tab 锁定/屏蔽后台快捷键/还原焦点）；持久化遗留
  `__settings__`/`__providers__` 标签恢复过滤（+1 单测，web 432 项）。
  运行时 e2e 全过；release notes 验证记录按复审更正（CI 取消链说明、
  web 变更列明、432 计数）。
- **readiness 复核收口**（`83bb73c` 键盘隔离 + `c0ff64f` 版本变更）：捕获阶段
  守卫拦截后台全局快捷键（Ctrl+S/Ctrl+F 等，CodeEditor/AgentPanel 独立监听
  一并隔离；输入框原生编辑键不受影响）；焦点陷阱修正（打开聚焦首控件、
  立即 Shift+Tab 回绕）；e2e 五项全过（含开/关对照实验与 Agent 模式 Ctrl+F）。
  版本变更 .64→.65 独立提交，`check-version --release --tag v0.6.1-preview.65`
  通过，CI 绿——**打标前置条件全部满足，等「发版」指令**。
- **lc-host M4 收尾完成**（`bafdd06`）：判定 factory 组合在 M2/M3 修订形态下
  已自然成立（stub 回退在 turn 层、TCP 预探在 factory 内），无编译死代码
  （clippy 全绿），孤儿组件扫描唯一命中 `RuntimeSettings.vue`（414 行零引用）
  已删除——lc-host 抽取计划 M1→M4 全部关闭。
- **三路子代理进行中**（限额已恢复；约定只实现不提交，主 agent 统一验收）：
  Track A 插件级 Agent 工具（agent-81）；Track C LSP 补全/悬停/跳转
  （agent-82，含契约 bump）；Track B MySQL/PG 设计文档（agent-83）。
- **preview.65 发版中**：tag `v0.6.1-preview.65` 打在 `b69cf33`（final 复审
  建议提交），release 工作流后台执行（等 draft→发布→updater-manifest）；
- **preview.65 已发布**（2026-11-11）：tag `v0.6.1-preview.65`（打在
  `b69cf33`，final 复审建议提交）；release 工作流 10 资产已发布（draft=false，
  SHA256SUMS 随附）；updater-manifest 已提升到 .65（signed:false 与既有披露
  一致，3 平台含回滚通道 previous=.64）。
- **Track B-M1+M3 已合**（`50a81e3` + M3 契约 0.9）：MySQL/PG 连接管理
  UI——远程表单（host/port 校验/env 密码名 secret_ref/数据库必填）、
  dbStore 放开四 kind、dbConnectionForm 纯函数 +35 单测；契约 0.9 后
  DbConnectionInfo 携带 port/user/secretRef（可选字段向后兼容），编辑回填
  全等往返；connect 错误分类引导（Node 缺失/连接类分流提示，web 488 项
  全绿）。M1 时的已知限制（回填回落默认）已随 M3 关闭。
- **声明式插件主题已合**（`fb1b65a`）：contributes.themes 端到端——清单校验
  （label 唯一/目录内 .json/64KiB）、ext_read_theme 防穿越、主题 JSON
  --vars 白名单防注入、设置页 select（暗/亮+插件 optgroup）、失效回落
  dark；web 502 项全绿。EXTENSIONS §8 语言声明已随 `f29d86a` 收官——v0 未实现仅剩
  市场/Webview/签名/API 兼容；扩展 lane 累计六块全落地（工具/延迟激活/
  主题/语言/依赖/树视图，lc-core 504 项，web 532）。
- **流程教训**（依赖切片 CI 曾红）：Rust 切片 brief 与验收必须含
  `clippy --all-targets -D warnings`（agent-90 brief 只要求 fmt+test，
  漏 ptr_arg；已修 `c2e5866`）——后续子代理 brief 已 implicit 此要求，
  主 agent 验收恢复跑全量 check-all。
  注意：本轮子代理并发期间发生 stash 误操作事故（agent-84 误 git stash
  卷走他轨在途改动，后已被 pop 还原），主 agent 已外科手术隔离提交，
  各轨道最终状态以验收时全量门禁为准。
- **子代理 swarm 四轨道全部落地**（限额恢复后首轮，main `c2b3902`）：
  Track A 插件级 Agent 工具（`002cf1a`，21 文件+2117，check-all 全绿）；
  Track C LSP 补全/悬停/跳转（`74dcf3e`，契约 v0.8，web 477）；
  Track B-M1 MySQL/PG 连接 UI（`50a81e3`）；B-M2 字节截断+CI 集成
  （`65e0b14`+断言修复 `f040548`+clippy `9422b1e`——真实 PG/MySQL 集成
  首跑验证通过：roundtrip/read-only/query/explain 全绿）。lc-host M4
  关闭（`bafdd06`）；preview.65 已发布（10 资产+manifest .65）。
- **swarm 后鸿蒙回归全绿**（新 HAP 于重拉的 Relay_Phone_API26 模拟器）：
  切片 H 子集 b/c/d/f 全过（工具流 786ms、三轮复用 303ms、取消 cancelled、
  40 会话回收 ohos-5 not_found、零 JS 错误）——lc-host Sessions 经 swarm
  大改后鸿蒙端行为不变。ohos 专用前端不受桌面设置浮层影响（无该入口）。
  事故记录：agent-84 误 git stash 卷走他轨 WIP（已 pop 还原，主 agent
  外科手术隔离提交，未污染任何轨道最终状态）。
  perf.yml 已在最新 main 触发（run 42858384352），观测 final 复审记录
  的窗口开关残留计数问题。
- **perf 残留观测结论**（run 42858384352，main `dea9568`）：窗口开关 20 次
  残留进程 **0**（基线 0；final 复审在 `b69cf33` 亦为 0），托盘 RSS 25.6MB、
  冷启动 34ms、热启动 32ms、`c2db89b` 的单实例残留 1 未复现——暂按偶发
  记录，继续靠 nightly perf 观测，不阻塞发版。

## main(2)：lc-host M3a 事件出口统一（f8a4cce）

- lc-host envelope 新增 `CoreEventSink` trait（CoreEvent 级，桌面形状）+
  `envelope()` 构造 + `Seq::from_u64` 种子；桌面 `agent.rs`/`db.rs` 切换：
  本地 trait 改重导出，`next_envelope`/`AtomicU64` seq 删除，统一 1 起
  （桌面首信封 seq 0→1，web 不消费 envelope seq，已核实无影响）。
- 验证：lc-host 15、lc-desktop 100、workspace 全绿；check-all 全量门禁
  通过；桌面 dev 重编运行正常。lc-ohos 未动（additive only）。M2 diff 已
  全量复审（原子序/锁序/错误文案逐 hunk 核对，修复 1 处遗留悬空 doc-link）；
  M3a 桌面运行时 e2e 已补：经 `__TAURI_INTERNALS__` 挂 `core:event` 监听，
  真实建会话+发消息（供应商周限额已尽，走到 error 事件）——捕获信封
  seq 2,3 连续（1 基 Seq 生效，seq 1 为监听前应用 init 消耗）、形状不变。
- 发布：未推 tag，同 M2 留 09:00 审查后。

## main：lc-host M2 会话编排骨架下沉

- lc-host 新增 `sessions` 模块：`SessionHandle`（info/cancel/running/alive
  原语 + model/thinking 元数据）、`SessionEntry<P>`（泛型宿主负载）、
  `Sessions`（map + limit + ordinal，插入即回收，`begin_turn` 同锁查表+CAS）。
  lc-ohos 本地 SessionHandle/reclaim/begin_turn/MAX_SESSIONS/next_session 全删，
  `AgentLoop` 槽位以 `AgentSlot = Arc<Mutex<Option<AgentLoop>>>` 进 payload；
  错误文案与锁序 1:1 保留，36 项测试不改语义全过。
- 验证：lc-host 12 项（+9 新 Sessions 单测：回收序/running 保留/alive 抑制/
  并发轮次拒绝/unknown not_found）；workspace 663 全绿；check-all 全量门禁通过
  （fmt/clippy/test/web 431/lanes/deny）；HAP 27.35MiB（Rust 重编确认），
  模拟器切片 H 子集 b/c/d/f 全过（工具流/三轮复用/取消 cancelled/40 会话
  回收 ohos-5 已 not_found、ohos-41 ack），hilog 无 panic。
- 发布：未推 tag——留 09:00 审查后发 preview.65（M2 随该版发布或单独 .65，
  按审查结论定）。

## preview.64：lc-host M1 共享事件出口

- 新 crate `lc-host`（envelope：EventSink/Seq/emit_envelope +3 单测）；
  lc-ohos 切换共享实现，无行为变化。
- 验证：workspace 654、clippy/fmt、web 431、lanes、HAP 27.33MiB；
  模拟器冒烟（session_create/agent_send/seq 连续）通过；real-path 流式
  中断为当晚宿主网络降级所致（.63 对照同样表现，非 M1 回归，已排查留证）。
- 发布：`v0.6.1-preview.64` **已发布**（10 资产，deny 修复后重跑成功）；滚动清单已提升到 .64。

## preview.63：鸿蒙 UI 抛光
- 供应商 sheet 系统返回键关闭（ArkTS onBackPress 消费 + setSheetOpen 状态
  同步）、Tab 焦点陷阱、Escape；归档开关零计数无 N。
- web 431 项（+7）；模拟器 uinput 返回键 e2e：sheet 关闭 Ability 不后台、
  关闭后返回键走默认。
- 发布：`v0.6.1-preview.63` **已发布**（10 资产）；滚动清单已提升到 .63。

## preview.62：鸿蒙历史面板管理入口

- ohos 分支重命名（内联）/归档/取消归档/显示已归档开关；historyView 纯函数
  +7 单测（web 424）；桌面不变。
- 模拟器 e2e：重命名重启保留、归档折叠、偏好记忆、取消归档恢复。
- 发布：`v0.6.1-preview.62` 发布中。

## preview.61：鸿蒙历史命令补齐与迁移

- history_rename/archive 原生（titleLocked 防覆盖）；localStorage 一次性
  幂等迁移（全成功才清 key、共享 Promise 去重）。
- lc-ohos 36 项、web 417 项单测；模拟器迁移 e2e 通过；HAP 27.27MiB。
- 待办：历史面板重命名/归档 UI 入口（需要时补）。
- 发布：`v0.6.1-preview.61` **已发布**（10 资产）；滚动清单已提升到 .61。

## preview.60：鸿蒙历史原生持久化

- lc-ohos history.rs：原生 list/save/load/delete（0600、50 条驱逐、序号续接）；
  shim 优先原生、回退 localStorage；lc-ohos 32 项单测。
- 模拟器 e2e：force-stop 重启后历史仍在、localStorage 无 history key。
- 待办：rename/archive 原生命令、localStorage 一次性迁移。
- 发布：`v0.6.1-preview.60` **已发布**（10 资产）；滚动清单已提升到 .60。

## preview.59：providers 逻辑下沉 lc-core

- 视图/合并共享化：删桌面/鸿蒙 355 行重复；lc-core +6 单测（密钥不泄漏、
  clearApiKey/ref 优先级、budget 拒绝）；薄适配层注释保留。
- 验证：workspace 全绿（desktop 100/ohos 23/lc-core +6）、check-all 全套。
- 发布：`v0.6.1-preview.59` **已发布**（10 资产）；滚动清单已提升到 .59。

## preview.58：demo 同步与 CI 补链

- demo/index.html 高保真重写（Kimi 风格/单行统计/设置页/统一菜单/双主题）；
  CI web job 增加 build:ohos + 无 Monaco 断言。
- 验证：demo 7 视图截图通过；check-all 全套通过。
- 发布：`v0.6.1-preview.58` 发布中。

## preview.57：鸿蒙供应商编辑入口

- 复用桌面 ProvidersPanel（tauriShim 白名单打通 providers_load/save），
  OhosApp 顶栏按钮 + 全屏 sheet；保存→providers:changed→对话页刷新全通。
- 模拟器端到端：改 baseUrl/contextK → 保存 → 重启持久 → 0/120k 生效。
- 验证：check-all 全套 + HAP 27.17MiB 通过。
- 待办：真机签名（维护者）、lc-host 抽取。
- 发布：`v0.6.1-preview.57` **已发布**（10 资产）；滚动清单已提升到 .57。

## preview.56：鸿蒙会话回收与 DNS 预算

- 会话表上限 32（按创建序回收已终止、running 绝不回收、alive 标记使迟到事件
  no-op）；agent_send 锁内 begin_turn。
- DNS 解析 800ms 预算化（IP 字面量零线程；域名分离线程 + recv_timeout），
  慢 DNS 直接回退 stub。
- lc-ohos 23 项单测；check-all 全套 + HAP 27.15MiB 通过。
- 待办：模拟器回归（切片 H a–f 全过，已归档）、供应商编辑 UI、真机签名。
- 发布：`v0.6.1-preview.56` **已发布**（10 资产）；滚动清单已提升到 .56。

## preview.55：鸿蒙离线回退提速

- provider baseUrl 800ms TCP 预探（probe_target/unreachable_target），不可达直接
  stub：~5.5s → ~1.3s；lc-ohos 19 项单测（含耗时断言）。
- 验证：check-all 全套通过（627 cargo / 413 web / lanes / deny）；本机黑盒 2.92s → 1.33s。
- 待办：DNS 解析预算化、供应商编辑 UI、真机签名。
- 发布：`v0.6.1-preview.55` **已发布**（10 资产）；滚动清单已提升到 .55。

## preview.54：Agent 用量统计修复

- 根因：统计按追加而非按轮覆盖 + 迟到/重复终态事件；修复为按轮号 upsert、
  终态不清累计器、loadHistory 隔离在途轮次。
- 新增 5 项去重单测（web 413）；桌面 mock 与鸿蒙 CDP 验证一轮一行。
- 验证：check-all 全套（fmt/clippy/test/web/lanes/deny）通过。
- 发布：`v0.6.1-preview.54` **已发布**（10 资产）；滚动清单已提升到 .54。

## preview.53：鸿蒙真实 Agent 对话

- `agent_send` 接入 lc-core 真实 AgentLoop（plan 只读工具 + Responses 流式 +
  5s 超时），失败回退 stub 并标注；`mock-llm.mjs` 开发验证；lc-ohos 15 项单测。
- 模拟器端到端：真实流式 + read_file 工具实读文件 + thinking/usage；
  断网回退 `[stub] delta`；取消可继续。
- 验证：cargo 623、workspace clippy/fmt、web 408、lanes、HAP 27.04MiB。
- 待办：turnStats 双行复现、秒级离线回退、approval_resolve、会话回收。
- 发布：`v0.6.1-preview.53` **已发布**（10 资产）；滚动清单已提升到 .53。

## preview.52：鸿蒙 providers 命令

- `providers_load/save` 复用 lc-core（0600 + 校验 + camelCase 视图对齐桌面）；
  `session_create` 预算跟随所选模型（100k/80k）。
- 模拟器验证：供应商就绪、模型 pill `stub-chat`、0/100k 预算、20 delta 对话；
  providers.json 0600；重启后状态保持。
- 验证：cargo 618、workspace clippy、web 408、lanes、HAP 26.92MiB。
- 发布：`v0.6.1-preview.52` **已发布**（10 资产，fmt 修复后重跑成功）；滚动清单已提升到 .52。

## preview.51：鸿蒙 NAPI 桥接真实 core

- `lc-ohos` C ABI + NAPI async work + threadsafe 事件通道 + ArkTS NativeBridge；
  x86_64/aarch64 OHOS 交叉编译入 HAP（26.75MiB）；build.mjs 自动构建双 target。
- 模拟器端到端：真实 fs 文件树、三级目录中文只读预览、20 delta 对话 637ms、
  取消 14ms、刷新重订阅无崩溃；`apps/ohos/README.md` 同步现状。
- 验证：cargo 613、web 408、lanes、build:ohos 无 Monaco、全量 HAP 管线绿。
- 待办：providers 命令（已随 .52 完成）、lc-core feature 化、真机签名。
- 发布：`v0.6.1-preview.51` **已发布**（10 资产，clippy 修复后重跑成功）；滚动清单已提升到 .51。

## preview.50：鸿蒙首版外壳 MVP

- `apps/ohos` 工程（dev.litecode.ohos）+ 虚拟 origin 资源管线（onInterceptRequest、MIME、
  路径防护、404）+ `scripts/ohos/build.mjs`；HAP 2.89MiB。
- `apps/web` ohos 只读入口（build:ohos 318KiB 无 Monaco）；`crates/lc-ohos` 宿主
  （fs 命令复用 lc-core 校验、plan 锁定、stub agent，5 项新测试）。
- 验证：cargo 613、web 408、lanes 12 全绿；模拟器 API26 安装/启动/渲染通过
  （修复 MIME charset 致 HTML 纯文本问题）；真机拒未签名 HAP（预期）。
- 待办：NAPI 胶水（P3，进行中）、lc-core OHOS 交叉编译探针（已通过 x86_64/aarch64）、真机签名。
- 发布：`v0.6.1-preview.50` **已发布**（10 资产）；滚动清单已提升到 .50。

## preview.49：顶栏菜单并入统一视觉

- `.menu-pop` 并入全局 menu.css（保留向上绝对定位）；删除 App.vue 本地定义。
- 验证：Web 408（28 文件）；真机顶栏「文件」下拉截图正常。
- 发布：`v0.6.1-preview.49` **已发布**（10 资产）；滚动清单已提升到 .49。

## preview.48：右键菜单视觉统一

- 新增全局 `styles/menu.css`，文件树/编辑器/标签栏右键菜单统一容器、行高、hover、分隔线；
  删除三处 scoped 重复定义。
- 验证：Web 408（28 文件）；真机两处右键菜单截图一致。
- 发布：`v0.6.1-preview.48` **已发布**（10 资产）；滚动清单已提升到 .48。

## preview.47：Agent composer 图标 Kimi 化

- 停止按钮 → `kimi-stop`；审批/权限盾牌 → `kimi-shield-question`；模式 pill → `kimi-thinking` 灯泡。
- 验证：Web 408（28 文件）；真机 Agent 模式 composer 截图正常。
- 发布：`v0.6.1-preview.47` **已发布**（10 资产）；滚动清单已提升到 .47。

## preview.46：活动栏/工具栏图标 Kimi 化

- 从 Kimi Code Desktop 资源包静态提取 91 个 `kimi-*` 图标（与线上 DOM 路径校验一致）。
- 替换：文件树、任务、终端、文件、铅笔、加号、关闭、归档、箭头、chevron 等 11 个
  为 24×24 填充路径；git/db/供应商/插件保留线性图标。
- 验证：Web 408（28 文件）；真机活动栏截图正常。
- 发布：`v0.6.1-preview.46` **已发布**（10 资产）。

## preview.45：Kimi 风格设置页

- 设置页重构为 184px 左导航（通用/编辑器/Agent/快捷键/关于）+ 16px 圆角分组卡片，
  行高 52px；≤760px 导航转横向滚动。
- 打开设置页自动收起右侧功能栏与底部面板，设置获得完整编辑器宽度；行标签 flex 基准宽度，
  窄容器不再逐字竖排。
- 设置/搜索/Agent/快捷键/关于图标换为 Kimi Code Desktop 的 24×24、1.8 stroke 路径；
  应用 Logo 不变；重新生成 macOS `icon.icns`。
- 验证：Web 408（28 文件，无新增测试）、Rust 608 基线；真机 1280×800 卡片 708px、
  四行横向、快捷键 10 行单行、5 项导航图标正常。
- 发布：`v0.6.1-preview.45` **已发布**（10 资产，manifest 待 updater-manifest 提升）。

## preview.44：固定标签

- 右键「固定标签/取消固定」；固定标签移到最左并显示图钉，状态随标签持久化。
- 关闭其他/关闭右侧/关闭已保存均保护固定标签；固定标签仍可单独关闭。
- 验证：Web 408（28 文件，新增固定排序/批量关闭 2 项、持久化 1 项）、Rust 608；
  真机固定 index.html 后移到首位并显示图钉，取消后恢复。
- 发布：`v0.6.1-preview.44` **已发布**（10 资产，manifest 已更新到 .44）。

## preview.43：全局搜索替换

- 搜索框可展开替换输入；两步确认（显示文件数，5 秒超时），按当前大小写/正则选项替换。
- 跳过 dirty 标签与读写失败文件；非 dirty 打开标签同步更新并标记已保存；完成后重跑搜索。
- 验证：Web 405（28 文件）、Rust 608；隔离沙箱 1 文件 2 处替换成功，
  确认文案「确认替换 1 个文件」与写回 `beta beta beta` 通过；误改的 3 个 vendor 文件已反向恢复。
- 发布：`v0.6.1-preview.43` **已发布**（10 资产，manifest 已更新到 .43）。

## preview.42：文件树 type-ahead

- 文件树聚焦后直接输入字母/数字，600ms 内累积前缀，跳到下一个匹配项并循环。
- 大小写不敏感、前缀匹配；空格与修饰键不参与；切换工作区/刷新/卸载清空缓冲。
- 验证：Web 405（28 文件）、Rust 608；真机 `.claude` 输入 `app` 跳到 `tangshan/app`。
- 发布：`v0.6.1-preview.42` **已发布**（10 资产，manifest 已更新到 .42）。

## preview.41：面包屑导航

- 路径段点击弹固定菜单：目录列出自身内容，文件列出父目录；目录下钻、文件打开/激活标签。
- 点击外部/Escape/滚动/缩放关闭；最多显示末 5 段，悬停显示完整路径。
- 验证：Web 405（28 文件）、Rust 608；真机 lite-code 面包屑弹出 dist 内容、
  下钻 assets 列出产物、Escape 关闭，随后切回 tangshan。
- 发布：`v0.6.1-preview.41` **已发布**（10 资产，manifest 已更新到 .41；.40 内容并入本版）。

## preview.40：Problems 严重级筛选

- 问题面板新增「全部/错误/警告」筛选与数量；列表按筛选展示，页签/状态栏仍为总数。
- 无匹配时显示「当前筛选没有问题」；新增 `filterProblems` 纯函数与 1 项测试。
- 验证：Web 405（28 文件）、Rust 608；真机 4 条诊断显示 4/2/1，列表 4→2→1→4 通过。
- 发布：跳过。linuxdeploy 连续失败，内容并入 `v0.6.1-preview.41`。

## preview.39：Ctrl+P 空查询入口

- 空查询顺序：2 工作区 → 2 未归档会话 → 4 文件 → 2 命令，共 10 项。
- 选择工作区切换并进 Agent；选择会话切换工作区并恢复消息流；归档会话需关键词检索。
- 验证：Web 404（28 文件）、Rust 608；真机空列表实际显示 tangshan/lite-code、
  两条 tangshan 会话、4 文件、2 命令。
- 发布：`v0.6.1-preview.39` **已发布**（10 资产，manifest 已更新到 .39）。

## preview.38：Agent 拖拽附件

- composer 支持拖入文本文件：蓝框提示、松开添加；限制 10 个 / 256 KiB / 总量 1 MiB。
- 二进制（含 NUL）与读取失败跳过并提示；composer 外 drop 被阻止，避免 WebView2 打开文件。
- 验证：Web 404（28 文件）、Rust 608；真机拖入 note.txt 成功、300 KiB big.txt 报错，刷新清理。
- 发布：`v0.6.1-preview.38` **已发布**（10 资产，manifest 已更新到 .38）。

## preview.37：文件树键盘导航

- roving tabindex + 蓝色焦点；↑/↓ 移动、→ 展开目录、← 折叠或跳父目录、Home/End 首尾。
- 切换工作区/刷新文件树重置焦点；Enter/Space 沿用原生打开/展开。
- 验证：Web 404（28 文件）、Rust 608；真机 22 行，↓ 移动焦点、→/← 切换 aria-expanded 通过。
- 发布：`v0.6.1-preview.37` **已发布**（10 资产，manifest 已更新到 .37）。

## preview.36：Agent 回到底部

- 跟随输出：距底 48px 内自动滚到底；用户上翻后停止强制滚动。
- 不在底部且有消息时显示圆形「回到底部」按钮，点击恢复跟随；发送/新建/切换会话时重置。
- 验证：Web 404（28 文件）、Rust 608；真机 48 条消息滚动区 4239/557，
  顶部显示按钮，点击后 scrollTop=3682 且按钮隐藏。
- 发布：`v0.6.1-preview.36` **已发布**（10 资产，manifest 已更新到 .36）。

## preview.35：VSC / Agent 主题统一

- 删除 Agent 中性色覆盖，两模式共用同一套背景/面板/边框/圆角/间距；Agent 仍隐藏活动栏。
- 强调色统一为 Logo 蓝 `#0A84FF`（深浅色各自的 hover/soft/line）；用户气泡蓝边蓝底、
  模式切换蓝底、composer/搜索聚焦蓝边。
- Monaco 背景对齐面板令牌，光标/选区/链接改蓝；欢迎页标识用应用 Logo。
- 验证：Web 404（28 文件）、Rust 608；深浅色 × Agent/VSC 四张截图确认卡片、圆角、
  间距与蓝色强调一致，刷新后主题偏好保留。
- 发布：`v0.6.1-preview.35` **已发布**（10 资产，AppRun 备件 + NSIS 恢复后补发成功，
  manifest 已更新到 .35）。

## preview.34：Ctrl+P 前缀语法

- `>` 只搜命令（空查询显示最近命令并补齐）；`:30` 跳当前文件第 30 行。
- Ctrl+P 选择文件结果自动切编辑器模式，修复 Agent 模式下只激活隐藏标签。
- `parseQuickQuery` 纯函数；quickPalette 新增 3 项前缀测试。
- 验证：Web 404（28 文件）、Rust 608；真机 `>`、`>设置`、`:30` 与普通查询通过。
- 发布：`v0.6.1-preview.34` **已发布**（10 资产，AppRun 备件 + NSIS 恢复后补发成功）。

## preview.33：Problems 面板

- 底部「终端/输出/问题」可切换，问题页签显示诊断计数；状态栏 `⚠ N` 可直达。
- 诊断按路径/行/列摊平，显示严重级色点、消息与相对路径:行:列；点击激活/打开文件并定位到行。
- 新增 `flattenDiagnostics` 纯函数；lspDiagnostics 新增 2 项测试。
- 验证：Web 401（28 文件）、Rust 608；真机注入 error/warning 两条诊断，计数、列表、
  状态栏与点击跳转 `index.html` 通过，清空后显示「没有发现问题」。
- 发布：`v0.6.1-preview.33` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.33`。

## preview.32：Ctrl+P 最近使用

- 空查询显示最多 6 个最近文件 + 4 个最近命令，不足时用其他标签/常用命令回填。
- 激活真实文件标签记录文件，执行命令面板命令记录命令；去重置顶、各保留 10 条，
  持久化到 `lc.quick.recent`。
- 输入后匹配结果按最近使用排序，其余保持原顺序。
- 验证：Web 399（28 文件，quickRecent 3 项）、Rust 608；真机空列表 5+4、
  选择文件与执行设置命令后各自置顶通过。
- 发布：`v0.6.1-preview.32` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.32`。

## preview.31：Agent 会话内 Ctrl+F 查找

- Agent 全宽模式 Ctrl+F 打开查找条：匹配 text/thinking，大小写不敏感，Enter/Shift+Enter 循环，
  Escape 关闭；命中消息滚动居中并描边高亮。
- 新增 `agentFind` 纯函数与 3 项测试；切换会话自动关闭查找。
- 验证：Web 396（27 文件）、Rust 608；真机注入 3 条消息验证 1/2 → 2/2、Escape、刷新清理通过。
- 发布：`v0.6.1-preview.31` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.31`。

## preview.30：标签右键菜单与品牌标识

- 标签右键菜单：关闭/关闭其他/关闭右侧/关闭已保存/复制路径/在资源管理器显示。
- 批量关闭只关干净标签，dirty 保留；「关闭其他」后始终激活保留标签；
  菜单 fixed 定位、点击外部或 Escape 关闭。
- 新增 `services/clipboard.ts`，Agent 消息复制与复制路径共用 execCommand 实现。
- 标题栏 `.brand-mark` 改用新 Logo，移除 Agent/亮色主题的灰色渐变。
- 验证：Web 393（26 文件，tabs 49 项）、Rust 608；真机菜单 6 项、复制路径内容、
  品牌标记背景与截图通过。
- 发布：`v0.6.1-preview.30` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.30`。

## preview.29：应用 Logo 换新

- 1254 原图 → 1024 母版，225px 圆角蒙版裁切，四角透明；`tauri icon` 重新生成全套
  ICO/ICNS/PNG/Store/Android/iOS 资产。
- Web 新增 `public/favicon.png` 与 `<link rel="icon">`；`scripts/dev/icon-source.png` 同步替换。
- 验证：32/128/256/512 输出四角 alpha=0；重新链接后的 exe 提取图标为新蓝色 Logo；
  favicon 经 dev server 返回 200。
- 发布：`v0.6.1-preview.29` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.29`。

## preview.28：Ctrl+P 检索历史会话正文

- `history_list` 为每个会话生成最多 4000 字符 `excerpt`（按消息顺序拼接 text，跳过空白）。
- Ctrl+P 会话匹配新增正文字段；浏览器演示模式由 localStorage 快照计算同样摘要。
- Agent 侧栏会话过滤同步匹配正文与工作区路径。
- 验证：Web 390（26 文件）、Rust 608（history 7 项，新增摘要拼接 1 项）；真机
  Ctrl+P 搜 `TSPL2`（仅出现在助手回复）命中两条 tangshan 会话。
- 发布：`v0.6.1-preview.28` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.28`。

## preview.27：编辑器快捷键与快捷键总览

- Ctrl+W 关闭当前编辑器，复用标签栏 dirty 二次确认；文件菜单关闭标签同步修复。
- Ctrl+Tab / Ctrl+Shift+Tab 正向/反向循环切换，少于两个标签无操作。
- 设置页新增「快捷键」卡片：10 组键位，随语言包切换。
- 状态栏接入 Git：分支/更改数/ahead-behind，启动与切换工作区自动刷新；无仓库显示「无仓库」。
- 验证：Web 390（26 文件，tabs 新增 2 项）、Rust 607；真机循环切换、关闭/重开、
  快捷键卡片、状态栏 `main/4 个更改/↑3` 通过。
- 发布：`v0.6.1-preview.27` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.27`。

## preview.26：标签交互与代码块复制

- Agent 代码块加语言标签与「复制」按钮，`execCommand` 避免 WebView2 权限条。
- 标签拖拽排序（左/右半区决定插入位置），顺序随标签持久化。
- Ctrl+Shift+T 重新打开最近关闭的编辑器：LIFO 最多 10 个，原位恢复内容/dirty/策略；
  文件删除导致的关闭不入栈。命令面板同步新增入口。
- 标签栏溢出横向滚动、滚轮横向滚动、中键关闭；dirty 标签仍二次确认。
- 验证：Web 388（26 文件）、Rust 607；真机复制文本、拖拽、重开位置、320px 滚轮滚动通过。
- 发布：`v0.6.1-preview.26` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.26`。

## preview.25：Ctrl+P 搜索工作区与历史会话

- 快速打开新增工作区/历史会话分组：工作区匹配名称与根路径，会话匹配标题/模型/模式/工作区。
- 选中会话先切工作区，再读取 app-local 存档并还原消息流，直接进入 Agent 模式。
- 结果上限 10 条（工作区/会话各 4 条，余下给标签/命令/在文件中搜索）；不检索消息正文。
- 验证：Web 380（26 文件，新增 quickPalette 3 项）、Rust 607；真机搜索「继续」恢复 tangshan 会话、
  搜索「lite-code」切换工作区通过。
- 发布：`v0.6.1-preview.25` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.25`。

## preview.24：Agent 偏好设置

- 设置页 Agent 卡片：发送键 Enter/Ctrl+Enter、消息密度舒适/紧凑、字号 12–18。
- 通过 subscribePrefs 实时应用；CSS 变量 `--agent-font` 与 density class；持久化 lc.ui.prefs。
- 默认 Enter 发送/Shift+Enter 换行；Ctrl+Enter 始终发送。
- 验证：Web 377（25 文件）、Rust 607；真机密度/字号/发送键切换与恢复通过。

## preview.23：消息编辑/重试分叉

- 用户消息「编辑并分叉」：截断该消息前上下文，恢复草稿；助手消息「分叉重试」直接重发。
- 分叉快照清理工具/审批/压缩/usage，turnStats 只保留该轮之前；原会话保留。
- 验证：Web 376（25 文件，新增 forkSession 3 项）、Rust 607；真机编辑分叉通过。
- 发布：`v0.6.1-preview.23` **已发布**（10 资产，校验通过，manifest 已更新）。

## preview.22：Agent 预览侧栏

- 头部预览按钮与 Git 共用右侧 rail；URL 输入、http(s) 校验、iframe sandbox、刷新/清除。
- 缺协议自动补 http；javascript/file/data 返回可读错误；合法地址写入 localStorage。
- 验证：Web 373（24 文件，新增 previewUrl 4 项）、Rust 607；真机加载/拦截/清除通过。

## preview.21：composer @ 文件引用

- 输入 `@` 按需索引工作区（≤2000 文件/深度 6），跳过 node_modules/target/vendor 与 .claude 等目录。
- 片段过滤、↑/↓、Enter 插入、Esc 关闭；插入 `@相对路径 `，由 Agent read_file 读取。
- 修复 vue-i18n 把 `@` 当 linked message 的启动崩溃（`{'@'}` 字面量）。
- 验证：Web 369（23 文件）、Rust 607；真机 @read 过滤与插入通过。

## preview.20：Agent Git 侧栏 + 队列编辑

- Agent 头部 Git 按钮打开 420px 右侧变更栏，复用 GitPanel（分支/暂存/提交/日志/Diff）。
- 切换工作区刷新 Git；非 Git 工作区显示可读错误，不影响对话。
- 点击排队消息载回 composer（含附件）并移出队列。
- 验证：Web 364、Rust 607；真机 Git rail 与队列编辑通过。
- 发布：`v0.6.1-preview.20` **已发布**（10 资产，SHA256SUMS 校验通过，manifest 已更新）。

## preview.19：Kimi Agent 功能移植（核心不变）

- 排队发送：运行/审批中发送进入本地队列，done 后自动续发；可移除/清空；建会话期间不丢队列。
- 对话管理：内联重命名、归档/恢复、已归档折叠区；Tauri `history_rename/history_archive`；
  后续保存保留标题与归档元数据。
- 上下文详情：点击上下文条查看预算/阈值/最近请求/缓存/累计每轮 token。
- 消息复制：execCommand 回退，避免 WebView2 剪贴板权限条。
- 验证：Web 364、Rust 607；真机双轮排队、重命名/归档/恢复、浮层与复制通过。

## preview.18：Agent（Kimi）/ 编辑器（新版 VSC）双模式视觉重构

- Agent：中性黑白 token、隐藏活动栏/状态栏、272px 侧栏 + 项目分组对话 + 搜索、
  48px 顶栏、860px 居中对话/composer、侧栏底部供应商/设置。
- 编辑器：48px 活动栏/20px 图标、34px 标签 + 活动顶部强调 + 悬停关闭、面包屑、
  34px 面板标签；模式切换移到标题栏。
- 参考 Kimi Code Desktop 的 tokens/侧栏/生产式布局与 microsoft/vscode 的 parts 结构。
- 修复 `:global()` 选择器误套 `.shell` 导致窗口压窄；Web 360、Rust 605 全绿。
- 发布：`v0.6.1-preview.18` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.18`。

## preview.17：Agent 工作区 + 按工作区归档对话

- Agent 模式左侧新增工作区列表：打开文件夹、点击切换、两次点击移除；列表存 localStorage。
- 切换工作区同步 Rust 根、文件树、LSP、Git/DB/任务状态；运行中或待审批时拒绝切换。
- 历史快照新增 `workspace` 字段，对话列表按当前工作区过滤；启动时迁移无 workspace 的旧记录。
- 参考 Kimi Code Desktop 的 Projects/Chats 与 ZCode 的 workspace 状态设计。
- 验证：Web 360（22 文件）、Rust 605；真机两工作区切换与对话过滤通过。

## preview.16：会话历史 / 活动状态 / 每轮 token / 双模式

- 历史：快照落盘到 `data_dir/agent-history`，头部/侧栏列出会话，支持切换与两次点击删除；
  载入为只读视图，发送时新建会话并注入最近 12 条上下文。
- 状态：思考中/生成中/工作中/等待审批/已完成/出错/历史会话；头部呼吸提示。
- 用量：每轮结束记录输入、输出、缓存 token 与耗时；usage 按 LLM 调用累加。
- 双模式：活动栏顶部「编辑器 / Agent」切换并写入 localStorage；Agent 模式为会话列表 + 全宽对话。
- 验证：Web 356（21 文件）、Rust 604；真机历史保存/切换/续聊、状态与用量、亮暗主题通过。
- 发布：`v0.6.1-preview.16` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.16`。

## preview.15：Agent 消息流滚动修复

- `.stream` 直接子项统一 `flex: none`：工具卡/审批卡较多时不再被压成细条，内容超出后滚动。
- 真机 23 条会话验证 `scrollHeight 2950 / clientHeight 522`，滚到底正常；Web 352 项全绿。
- 发布：见 P6 发布线。

## preview.14：Agent 面板按 Demo 重构

- 会话头、消息流、工具卡、审批卡、压缩行对齐 `demo/index.html`；用户气泡、助手正文、状态点与轮次。
- composer 一体化：模式/模型/思考 pill + 附件 chips + 图标发送 + 上下文条（阈值刻度）。
- AI 消息安全渲染 Markdown（标题/列表/行内代码/加粗/围栏代码块，先转义全文）。
- 验证：Web 352（21 文件）；真机模型/思考菜单、Markdown、shell 审批拒绝、附件与暗色主题通过。
- 发布：`v0.6.1-preview.14` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.14`。

## preview.13：Agent 模型/思考/附件/选区

- 模型与思考：Agent 头部下拉读取供应商配置并随 `session_create(model, thinking)` 透传；
  `max` 归一 `high`，`off` 不发送 reasoning 字段。
- 上下文：无会话时按所选模型显示 `0 / 1000k` 与压缩阈值；新增“新会话”重置消息流。
- 附件：原生选择器读取 UTF-8 文本，10 个 / 256 KiB / 总 1 MiB，以 `<attachment>` 注入会话。
- 选区：编辑器右键“添加到 Agent 对话”，追加 `路径:行号` + 围栏代码块；演示缓冲区回退标题。
- 修复：`sessionCreate` 适配层曾把模型当 `taskId`，真机报 `task not found`；已显式转接。
- 验证：Rust 601、Web 347；真机发送 `请只回复：OK` 成功返回，轮次 1/25、用量 1.4k/1000k。
- 发布：`v0.6.1-preview.13` **已发布**（非 draft，10 资产），SHA256SUMS 校验通过，
  updater manifest 已更新到 `0.6.1-preview.13`。

## preview.12：Agent 就绪状态修复

Agent 空态读取供应商配置：启用供应商 + 模型 + 密钥时显示“供应商已就绪”和默认模型；
供应商保存广播 `providers:changed`，Agent 面板即时刷新；401/403 追加 Base URL/API Key
不匹配提示。401 本身是供应商凭据/端点配置问题，不是会话创建失败。

## P6 外部依赖（阻塞项）

P6 出口要求包含无法在本会话/本机完成的项，需维护者或外部环境：

1. **30 天无 P0/P1 缺陷、崩溃率 < 0.1%**：需要真实用户/长周期遥测；当前仓库无遥测后端。
2. **第三方安全审计高危清零**：需要外部审计方与报告。
3. **代码签名（SignPath / Azure Trusted Signing）**：需要证书/账号与仓库 secrets；当前安装包未签名。
4. **HarmonyOS 产品外壳**：spike 已完成（`spikes/harmony-arkweb`，DevEco 26/API26 模拟器验证 ArkWeb+NAPI）；剩余产品化（只读+对话首版）需排期。
5. **Linux arm64**：已进发布矩阵（CI 构建 aarch64 AppImage/deb/lc-cli 并发布）；剩余真机运行验收。
6. **更新器与回滚通道**：需要签名密钥与发布端点。

可本地完成的部分（i18n 完整性、cargo-deny、clippy/test、性能回归）已纳入 CI。

## 待办（按序）

1. P5 周边：Git 面板（status/diff/stage/commit/log）、SSH 终端、TS/JS LSP。
2. P5 出口验收：真实 SQLite 项目「schema → 查询 → 审批更新 → 再查验证」；10 万行样本/handle；同连接写串行。
3. 干净 Win11 无 Node 的人工验收（代码路径已有 core/UI 测试覆盖）。
4. P6：稳定化、鸿蒙与正式包。
5. 已知限制：pg/mysql/mssql 驱动已打包但未做真实服务器端到端；db_query 无分页 handle；搜索不支持替换；MCP 仅 stdio 与单工具槽位；真实 OpenAI/Anthropic 端点兼容性未覆盖。

## 本地验证命令（与 CI 等价）

一条命令跑完全部（等价于下面逐条执行）：

```bash
bash scripts/check-all.sh
```

逐条等价命令：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm.cmd lint && pnpm.cmd typecheck && pnpm.cmd test -- --run && pnpm.cmd build
C:/Git/usr/bin/bash.exe -c "export PATH=/usr/bin:$PATH; exec scripts/check-lanes.sh"
cargo deny --locked check
```

## 发布基础设施备注

- Linux/Windows 打包依赖 GitHub `tauri-apps/binary-releases` 下载 AppRun/NSIS。
  2026-11-09 该端点持续 504：AppRun 已内置 `scripts/release/AppRun-*` 并在打包脚本中预置到
  Tauri 缓存；AppImage 单独重试 3 次并设置 `APPIMAGE_EXTRACT_AND_RUN=1`。
- `v0.6.1-preview.34/35` 标签已重打到带修复的提交；NSIS 仍需该端点，待 GitHub 恢复后
  对失败的 release run 执行 `gh run rerun --failed`，先 .34 后 .35。

## 环境备忘（本机特有，避免重新踩坑）

- `world.run`/spawn 下 pnpm 必须写 **`pnpm.cmd`**（无扩展名 shim 是 ENOENT）
- `bash` 解析到 WSL；Git Bash 在 `C:\Git\usr\bin\bash.exe`，需
  `-c "export PATH=/usr/bin:$PATH; exec <script>"`
- 本机 cargo-deny 0.20.2 的参数顺序：`cargo deny --locked check`
- cargo-deny-action 要显式 `rust-version`（仓库 rust-toolchain.toml 会 override 其 musl 工具链）
- `gh run watch --exit-status` 的退出码别被后续 `echo`/管道吞掉
- 工作流（dynamic-workflows）：门禁命令必须编译期字面量；`git add -A` 会卷入
  subagent 进行中的文件——提交一律用精确路径
- pwsh 7.6.9 为 Store 版（WindowsApps），版本校验通过；rustup pinned 1.99.1 已装

## 测试基线（本机 Windows，最近一次全绿）

lc-core 462 · lc-cli 33 · lc-proto 10 · lc-desktop 99 · lsp stdio 2 · web 408（28 文件）· 合计 **1016**（含 `db_agent` 2、`fs::search` 14、ext 18、lsp 18、lspDiagnostics 7、history 7、quickPalette 6、tabs 51、agentFind 3、quickRecent 3）。
Linux 额外跑 `write_file_rejects_dangling_symlink_escape` 与权限保留测试。
