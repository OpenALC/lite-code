# Lite Code 文档索引

轻量级桌面 Agent 工作台。Tauri 2 + Rust 核心 + Vue 3 WebView 前端。

**定位**：不是 Claude Code / Codex 的 GUI 套壳，而是用 Rust 重写 Agent harness 本身——摆脱 Bun 运行时依赖与 Electron 运行时开销，同时把编辑器、Git、SSH、数据库、MCP、Agent 记忆收进同一个 WebView。

| 文档 | 内容 |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | 分层、crate 划分、模块选型、构建产物 |
| [RUNTIME.md](RUNTIME.md) | Node 探测状态机、手动兜底、便携版下载、进程注入 |
| [AGENT.md](AGENT.md) | Agent harness 设计、工具执行、权限、MCP、记忆适配 |
| [PLAN.md](PLAN.md) | 并行开发组织、lane 划分、风险清单、工程量 |
| [MILESTONES.md](MILESTONES.md) | 里程碑 P0–P6、平台支持矩阵、各阶段出口标准 |
| [VERSIONING.md](VERSIONING.md) | 版本号规则、tag 校验、正式版流程、契约版本 |
| [CI.md](CI.md) | GitHub Actions 工作流、缓存、签名、secrets、本地复现 |
| [TASKS.md](TASKS.md) | Task 实体、worktree 隔离、并发配额、合并协议 |
| [UI.md](UI.md) | 布局、视觉基调、交互约定 |
| [release-notes-template.md](release-notes-template.md) | 发版说明模板 |

---

## 技术栈基线（已锁定）

| 项 | 值 | 说明 |
|---|---|---|
| Rust | 1.99.1 | `rust-toolchain.toml` + `rust-version`，不跟随 stable |
| Tauri | 2.x | 薄壳，只做窗口/托盘/命令注册 |
| 前端 | Vue 3 + TypeScript 5.9 + Vite 8 | 不使用 JSX，不用 TS7（与 Vue 3 不兼容） |
| 编辑器 | Monaco Editor | 单编辑器方案，不引入 CodeMirror |
| Linux GUI | GTK3 + `webkit2gtk-4.1` | Tauri 2 稳定版后端；GTK4 待上游支持 |
| HarmonyOS | ArkWeb + NAPI | 复用同一前端产物，独立外壳 |
| Node | 系统已安装，优先 v22+ | 仅用于可选集成，harness 不依赖 |

## 已定基调（不要反复推翻）

| 决定 | 结论 |
|---|---|
| 许可证 | `Apache-2.0 OR MIT` 双许可 |
| LLM 端点 | 先做 **OpenAI Responses**，随后 **Anthropic Messages**；其他端点不做 |
| i18n | 官方只维护 `zh-CN`；语言包可替换，社区自译 |
| 定位 | 自建 harness，**不接受 Electron 套壳**，不依赖 Bun |
| 开发节奏 | 先规划到位再开工；当前阶段只产出文档 + UI demo |
| 轻量底线 | 资源占用不失控即可（见 PLAN.md 性能门槛） |

## UI demo

[`demo/index.html`](../demo/index.html) — 单文件，浏览器直接打开，无外部依赖。用于确认布局、密度与视觉基调，非最终实现。P0 已迁入真实壳：`apps/web`（Vue 3 + Monaco）。

## 三条不可退让的架构约束

1. **`lc-core` 不依赖 Tauri**。所有服务通过 `CoreHandle` trait 暴露，Tauri 命令只是转发层。这是独立进程化、headless CLI、鸿蒙适配的前提。
2. **`lc-proto` 是唯一契约**。并行开发时只读不写，变更必须走独立 PR 并 bump 版本。
3. **Agent 会话状态在 Rust 侧**。窗口销毁只销毁渲染进程，PTY、子进程、会话历史全部由核心持有。

## 术语

- **harness**：Agent 主循环——调度 LLM、执行工具、管理上下文与权限。本项目用 Rust 实现。
- **lane**：并行开发的工作流，按目录切分，互不交叉。
- **降级模式**：Node 不可用时应用仍可启动的状态，编辑器/文件树/Git 可用。
