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
- 复验补丁：DB 审批按 connection_id 隔离、shell/DB/git 子进程剥离 provider 密钥、CI audit 缓存修复
- 审计 Medium：CREATE TRIGGER DML 检查、LIMIT/TOP 识别、DB bridge 流式取样、不可信边界转义、task key FNV-1a 64
- 审批 UX：write_file/edit_file 展示内容/替换摘要，AllowSession 按内容隔离
- UI 重构：VS Code 式活动栏/侧栏/编辑器/右侧功能栏、自定义无边框标题栏、亮/暗主题、页面缩放、设置面板、快速打开、原生文件夹选择
- 文件树：项目名、打开的编辑器、右键菜单（中文）、拖拽移动、VCS 目录过滤；编辑器中文右键菜单
- 数据库：PostgreSQL / MySQL / SQL Server 驱动打包为单文件 bundle，与 SQLite 共用 JSON-RPC
- 工作区全文搜索：`fs_search` 支持字面量/正则、大小写与 gitignore，搜索侧栏分组高亮，点击命中跳转编辑器行
- 快速打开：无文件/命令匹配时提供“在文件中搜索”，把关键词带入搜索侧栏
- 设置页：编辑器区独立页签，主题/语言/缩放/字号/Tab/换行实时生效并持久化，版本号动态注入
- UI 修复：刷新按钮不再误开工作区弹窗、语言按钮真正切换、状态栏与项目名重复、Agent 头部裁切、中文文案补全
- 搜索修复：`fs_search` 参数命名（大小写/上限生效）与异步 `spawn_blocking`；结果截断标记准确
- 设置入口修复：Ctrl+, / 帮助 / 状态栏统一打开设置页签；搜索侧栏跨视图保留状态、工作区切换清空旧结果
- 编辑器生命周期：定位请求消费一次、卸载移除窗口监听、Monaco 异步挂载竞态防护；非法正则错误本地化
- 快速打开（Ctrl+P）点击外部关闭并清空查询；此前面板会一直浮在标签栏上方
- 搜索截断文案按实际条数展示；快速打开支持 Tab 失焦关闭
- 供应商管理页：左侧活动栏「供应商」，配置 Base URL/API Key/模型列表/默认与 Subagent 模型
- 供应商密钥：本地 apiKey 脱敏读写、0600、空白保留旧值/显式清除；运行时优先级 env > apiKey > apiKeyRef
- 供应商安全加固：tmp/加载/备份 0600、备份去密钥、clear 同时清 apiKeyRef、Debug 脱敏
- 供应商交互收尾：保存禁用表单、页签常驻保留草稿、稳定模型行 key、短 Key 全掩码
- 插件系统 v0：Node/JS 扩展宿主、WASM WASI 命令、声明式 MCP/LSP、插件管理页与命令面板
- 插件安全边界：项目插件默认关闭、清单路径校验、独立进程、WASM 无文件/网络、密钥环境清理
- Agent 模型与思考：会话创建前选择模型/思考挡位，透传 `session_create`；`max` 归一 `high`
- Agent 上下文预算：无会话时展示所选模型 `contextK` 与压缩阈值；新增“新会话”重置消息流
- Agent 文本附件：原生文件选择器，UTF-8、10 个/256 KiB/总 1 MiB 上限，chips 展示与移除
- 选中代码加入 Agent：编辑器右键菜单追加 `路径:行号` + 围栏代码块；演示缓冲区回退标题，20k 截断
- Agent 修复：`sessionCreate` 适配层曾把模型误传为 `taskId`，真机发送报 `task not found`；已显式转接
- Agent UI 重构（对齐 `demo/index.html`）：会话头、消息流、工具卡/审批卡/压缩行、一体化 composer
  （模式/模型/思考 pill、上下文条）、附件 chips 与图标发送按钮；AI 消息安全渲染 Markdown
- Agent 流滚动修复：`.stream` 子项不再参与 flex 收缩，工具卡/审批卡较多时保持原高度并可滚动
- Agent 会话历史：快照落盘到数据目录，历史列表/切换/删除；载入为只读视图，发送时新建会话并注入最近 12 条上下文
- Agent 活动状态与用量：思考中/生成中/工作中/等待审批/已完成/出错；每轮展示输入/输出/缓存 token 与耗时
- 双模式布局：活动栏顶部「编辑器 / Agent」切换；Agent 模式为会话列表 + 全宽对话，编辑器模式保留 VSC 式布局
- Agent 工作区：Agent 模式左侧工作区列表（打开文件夹/切换/移除），会话按工作区归档；
  切换工作区同步 Rust 根、文件树与 LSP，旧历史启动时归入当前工作区
- Agent UI（Kimi Code Desktop 参考）：中性色板、无活动栏/状态栏、项目分组对话 + 搜索、
  48px 顶栏、居中 860px 对话与 composer、侧栏供应商/设置入口
- 编辑器 UI（新版 VS Code 参考）：48px 活动栏、34px 标签与活动标签顶部强调、悬浮关闭、
  面包屑导航、34px 面板标签、标题栏编辑器/Agent 模式切换
- Agent 功能移植：运行中消息排队并自动续发、对话重命名/归档/恢复、上下文详情浮层、
  助手消息复制（execCommand，避免 WebView2 剪贴板权限条）
- Agent Git 侧栏：Agent 模式头部 Git 按钮打开 420px 变更栏，复用 GitPanel 的分支/暂存/提交/日志/Diff
- 队列消息可点击载回 composer 编辑，原消息从队列移除
