# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号规则见 [docs/VERSIONING.md](docs/VERSIONING.md)：`0.<里程碑>.<发版序号>-preview.<重建号>`。
发版时由 Conventional Commits 标题生成初稿，人工合并同类项。

## [Unreleased]

### Added

- Cargo workspace 骨架：`lc-proto` / `lc-core` / `lc-cli` / `lc-desktop`（Tauri 薄壳）
- `lc-proto` 契约 v0：命令、DTO、`ErrorCode`、事件枚举；TS 绑定生成与漂移检查
- `lc-proto` 契约 v0.1：新增 `workspace_root` / `fs_write_file`；TS 绑定改为带 `export` 的 ES 模块，web 直接 `import type`
- `lc-proto` 契约 v0.2：新增 `workspace_open`；工作区根由 Tauri 状态持有并持久化到 `config.json`
- 打开工作区：顶栏输入目录路径，校验后切换文件树根；无工作区时编辑器仍可用
- `lc-core::runtime`：Node 探测状态机（Tier 1–4 候选、Store 别名桩识别、GhostInstall 自检）与 Shell 探测（PowerShell 7 / bash），纯函数设计可注入测试
- `lc-cli`：`node detect` / `shell detect` / `contract info`，golden 输出测试
- `apps/web`：Vue 3 + Vite 8 + TS 5.9 + Monaco 起壳，`LayoutProfile` 骨架与模式切换器
- CI：fmt / clippy / test（Windows + Ubuntu）、契约漂移、lane 边界、cargo-deny、web 四件套
- `lc-proto` 契约 v0.4：`SessionInfo.contextK/compressAtK`、`agent:compacted` 事件、`permission_denied` 错误码
- Agent harness：工具注册表（≤12）、plan/chat/auto/yolo 四模式、审批队列、read/write/edit/glob/grep/shell 执行
- LLM 网关：OpenAI Responses SSE 与 Anthropic Messages 解码、ureq 流式传输、`providers.json` + 环境变量密钥兜底
- `AgentLoop`：LLM 流 → 授权 → 工具执行 → 结果回填 → 下一轮；工具失败回填而非中断，支持审批 resume
- Tauri Agent 命令：`session_create` / `agent_send` / `agent_cancel` / `approval_resolve`，每会话 worker 线程与 `core:event` 事件信封
- `lc-cli agent run`：headless 多轮任务、JSON 事件流、`--yes` 自动审批、退出码 0/1/2
- Agent UI：会话流、工具卡、内联审批三按钮、composer、yolo 二次确认、上下文刻度条与可展开压缩记录
- 上下文压缩：工具输出瘦身 → 早期摘要 → 激进压缩；切点保证 `function_call`/`output` 配对，压缩后继续当前任务
- MCP Host：stdio JSON-RPC、`.mcp.json`/用户配置合并、动态工具注册与执行、超时/进程树回收
- 会话 instructions：注入平台/shell/工作区环境契约，以及全局/项目 memory.md、AGENTS.md/CLAUDE.md 记忆
- Task 工作台：`Planning/Running/Blocked/Review/Done/Failed/Queued` 状态机、并发 2 队列、端口与任务持久化
- worktree 隔离：数据目录下 `worktrees/<project>/<task>`，分支 `lc/<task>-<slug>`，主工作区保持 clean
- 合并协议：主工作区 clean / 基线漂移 / `--no-ff` / 冲突保留与解决后收尾；同文件不同行并行合并
- Tauri task 命令与 `task:*` 事件；`session_create` 绑定任务后在 worktree 内运行 Agent
- 工作台 UI：任务列表、变更/检查/Diff/预览标签、应用/丢弃、后台审批待确认计数
- 预览与标签恢复：iframe 随任务销毁/重建，工作台标签按任务记忆
- 契约 v0.6：数据库连接/查询/schema DTO、9 个 `db_*` 命令、`db:status` 事件、`db_*` 错误码
- SQL 闸门：单语句、读写/DDL 分类、无 WHERE 拦截、静态黑名单、LIMIT 注入、只读连接拒绝
- SQLite sidecar：Node `node:sqlite` NDJSON JSON-RPC、connect/query/execute/schema/explain、超时与进程回收
- DB 服务与 Agent 工具：连接配置持久化、`db_list/db_schema/db_query/db_explain/db_execute`；结果标 `untrusted`
- 数据库面板：连接列表、schema 树、SQL 编辑器、结果网格、EXPLAIN、只读徽标
- Git 面板：主工作区 status（staged/unstaged、ahead/behind、merge/rebase）、diff、暂存/取消暂存、提交与最近提交
- Agent DB 闭环：真实 SQLite 建表 → 写入 → 查询，结果回填后继续
- 安全边界：DB/MCP 工具输出加不可信数据标记；Tauri CSP 允许预览 iframe 并收紧 object/base
- HarmonyOS spike：ArkWeb + NAPI 通过；Monaco 内存/IME 未过，首版降级只读+对话
- P6 A 类：Linux arm64（ubuntu-24.04-arm）AppImage/deb/CLI 进入发布矩阵
- P6 A 类：`update_check`、`latest.json` 生成器、updater-manifest 与回滚文档
- 安全修复：MCP 项目配置默认不执行、SQL 无 WHERE 绕过、批量工具审批绕过、VCS 写保护、取消、记忆路径/大小、task 按工作区隔离与 ignored 文件保护
- 发布加固：签名 fail-closed、最小权限、资产/checksums 一致性校验、CI audit 与 updater manifest 测试
- 依赖：`dompurify` override 至 3.4.17，`pnpm audit --prod` 清零

