# 工作进度快照（交接文档）

> 本文件是会话间的工作状态交接，由主 agent 维护；里程碑的权威定义仍在
> MILESTONES.md。快照时间：2026-11-05，main HEAD：`774e7dd`；契约 v0.4。
> P0 发布线：tag `v0.0.1-preview.2` + draft release 已产出。
> P1 发布线：`v0.1.1-preview.1` draft 已按正确资产清单重建。
> P2 发布线：`v0.2.1-preview.1` release 成功，6 个资产 draft 已产出（发布流程修复后首个自动 draft）。

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
| 布局 | ✅ 侧栏/Agent/底部可拖拽、可折叠，localStorage 持久化；dockview 留 P4 |
| 打开工作区 | ✅ 契约 v0.2 `workspace_open`；根由 Rust 状态持有并持久化；当前为路径输入，原生选择器待 P2 |
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
| Runtime 设置页 | ✅ Ready/NeedManual、候选失败原因、手动路径、便携版下载 |
| main CI | ✅ `958d4fe` 全绿；`774e7dd` 由后台 watcher 验证 |

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

## 已知 P1 缺口（审查结果，按优先级）

1. 无外部修改冲突检测（savedContent 基线已有；mtime/hash 契约字段未做）。
2. 托盘“退出”绕过 webview 脏检查，可能丢未保存缓冲。
3. 原生文件夹选择器未接；当前打开工作区为路径输入。
4. `activeRssMb` 缺 UI 自动化入口未测；cold/warm 终点是“窗口可见”而非应用内“可交互”。
5. `@lc-proto` 目前只有 tsconfig 别名，未来若生成运行时常量需补 Vite alias。

## 待办（按序）

1. 干净 Win11 无 Node 的人工验收（代码路径已有 core/UI 测试覆盖）。
2. P3 出口验收：用真实 provider key 完成一次「读文件 → 改文件 → 跑测试 → 汇报」多轮任务；验证审批三按钮、yolo 每次启动确认、编辑器↔Agent 切换无损。
3. P3 发布准备：版本号升 `0.3.1-preview.1`，打 tag 发 draft（真实任务验收通过后再打）。
4. 已知限制：取消不强制中断已发出的 HTTP 流；审批挂起期间取消会留下 pending 审批（UI 正常路径不触发）；MCP 仅支持 stdio 与单工具槽位（保留 task/db_query 占位）。

## 本地验证命令（与 CI 等价）

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm.cmd lint && pnpm.cmd typecheck && pnpm.cmd test -- --run && pnpm.cmd build
C:/Git/usr/bin/bash.exe -c "export PATH=/usr/bin:$PATH; exec scripts/check-lanes.sh"
cargo deny --locked check
```

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

lc-core 204 · lc-cli 33 · lc-proto 10 · lc-desktop 28 · web 166（12 文件）· 合计 **441**。
Linux 额外跑 `write_file_rejects_dangling_symlink_escape` 与权限保留测试。
