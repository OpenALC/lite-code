# 工作进度快照（交接文档）

> 本文件是会话间的工作状态交接，由主 agent 维护；里程碑的权威定义仍在
> MILESTONES.md。快照时间：2026-11-09，main HEAD：preview.36 发布提交（见 `git log`）；版本 `0.6.1-preview.36`；契约 v0.7。
> P3 发布：`v0.3.1-preview.1` draft 已产出 6 个资产。
> P4 发布：`v0.4.1-preview.2` draft 已产出 6 个资产（NSIS/AppImage/deb/两个 CLI/SHA256SUMS）。
> P0 发布线：tag `v0.0.1-preview.2` + draft release 已产出。
> P1 发布线：`v0.1.1-preview.1` draft 已按正确资产清单重建。
> P2 发布线：`v0.2.1-preview.1` release 成功，6 个资产 draft 已产出（发布流程修复后首个自动 draft）。
> P6 发布线：`v0.6.1-preview.4` 已发布（10 资产，含 arm64）；`preview.4/7/8/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31/32/33/34/35` 已发布；`preview.36` 发布中。

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
| P6 鸿蒙 | ✅ spike 完成（DevEco 26/API26）：ArkWeb 跑通前端、NAPI 传 handle；Monaco 内存/IME 未过 → 首版只读+对话 |
| P6 本地安全整改 | ✅ CSP frame-src/object-src；DB/MCP 输出不可信边界；审计范围写入 `docs/SECURITY-AUDIT.md` |
| P6 A 类 arm64 | ✅ `v0.6.1-preview.3` tag 构建成功：aarch64 AppImage/deb、`lc-cli-arm64` 已发布 |
| P6 A 类 更新器 | ✅ `update_check` + `latest.json` 生成器 + 回滚文档；无签名密钥时 `signed:false`，签名门禁未完全关闭 |
| P6 审计修复 | ✅ Critical：MCP 项目配置默认不执行、SQL 无 WHERE 绕过、批量工具审批绕过；High：VCS 写保护、取消、记忆路径、发布校验、DB 审批按连接隔离、子进程密钥清理 |
| P6 审计 Medium | ✅ CREATE TRIGGER DML 检查、LIMIT 识别、bridge 流式取样、不可信边界转义、task key 稳定化、写文件审批内容摘要 |
| P6 依赖审计 | ✅ `dompurify` override 3.4.17；`pnpm audit --prod` 无已知漏洞 |
| P6 发布 | ✅ `preview.4/7/8/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31/32/33/34/35` 已发布；`preview.36` 发布中 |
| UI 重构 | ✅ VS Code 式活动栏/侧栏/编辑器/右侧功能栏；自定义标题栏、亮暗主题、缩放、设置页、快速打开、原生文件夹选择、打开编辑器列表、文件树拖拽与右键、编辑器中文右键 |
| 工作区搜索 | ✅ `fs_search`（字面量/正则/大小写、gitignore、2MiB/二进制跳过、500 行上限；异步不阻塞 UI）+ 搜索侧栏 + 命中跳转 |
| 设置页 | ✅ 编辑器区独立页签：主题/语言/缩放/字号/Tab/换行，实时应用并持久化；版本号由 Vite 注入 |
| DB 驱动 | ✅ PostgreSQL（pg）/ MySQL（mysql2）/ SQL Server（mssql）打入 2.3MiB 单文件 bundle；SQLite 保留 |
| P6 其余 | ⏳ B 类（soak、第三方审计、签名、Win11 验收）由维护者关闭；更新器 `signed:false` |
| Runtime 设置页 | ✅ Ready/NeedManual、候选失败原因、手动路径、便携版下载 |
| main CI | ✅ preview.23–36 CI 全绿 |
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

## preview.36：Agent 回到底部

- 跟随输出：距底 48px 内自动滚到底；用户上翻后停止强制滚动。
- 不在底部且有消息时显示圆形「回到底部」按钮，点击恢复跟随；发送/新建/切换会话时重置。
- 验证：Web 404（28 文件）、Rust 608；真机 48 条消息滚动区 4239/557，
  顶部显示按钮，点击后 scrollTop=3682 且按钮隐藏。
- 发布：`v0.6.1-preview.36` 发布中。

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
4. **HarmonyOS spike**：需要 DevEco/ArkWeb SDK 与真机；本机未安装。
5. **Linux arm64 正式包**：需要 arm64 构建/运行环境与发布矩阵扩展。
6. **更新器与回滚通道**：需要签名密钥与发布端点。

可本地完成的部分（i18n 完整性、cargo-deny、clippy/test、性能回归）已纳入 CI。

## 待办（按序）

1. P5 周边：Git 面板（status/diff/stage/commit/log）、SSH 终端、TS/JS LSP。
2. P5 出口验收：真实 SQLite 项目「schema → 查询 → 审批更新 → 再查验证」；10 万行样本/handle；同连接写串行。
3. 干净 Win11 无 Node 的人工验收（代码路径已有 core/UI 测试覆盖）。
4. P6：稳定化、鸿蒙与正式包。
5. 已知限制：pg/mysql/mssql 驱动已打包但未做真实服务器端到端；db_query 无分页 handle；搜索不支持替换；MCP 仅 stdio 与单工具槽位；真实 OpenAI/Anthropic 端点兼容性未覆盖。

## 本地验证命令（与 CI 等价）

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

lc-core 462 · lc-cli 33 · lc-proto 10 · lc-desktop 99 · lsp stdio 2 · web 404（28 文件）· 合计 **1012**（含 `db_agent` 2、`fs::search` 14、ext 18、lsp 18、lspDiagnostics 6、history 7、quickPalette 6、tabs 49、agentFind 3、quickRecent 3）。
Linux 额外跑 `write_file_rejects_dangling_symlink_escape` 与权限保留测试。
