# 开发计划

## 1. 组织原则

落地方式是 vibe coding + 多 Agent 并行，因此规划的**第一目标是减少并行冲突**，而不是排功能顺序。

三条硬约束：

1. **契约先行且单点修改**。`lc-proto` 是唯一同步点，定稿后只读不写，变更走独立 PR 并 bump 版本。
2. **按目录切 lane，一个 lane 一个所有者**。跨 lane 只通过契约交互，禁止直接 import 内部模块。
3. **核心可 headless 运行**。这决定了 AI 产物的自证速度，也是 CI 能脱离 WebView 做回归的前提。

## 2. 里程碑

> 里程碑的权威定义（范围、出口标准、平台门禁、排期）见 [MILESTONES.md](MILESTONES.md)。本节保留阶段划分与并行关系的早期草案。

### P0 地基（1–2 周，串行）

| 任务 | 出口 |
|---|---|
| 工作区、`rust-toolchain.toml`(1.99.1)、Cargo workspace | `cargo build` 通过 |
| `lc-proto` 契约草案：命令名、DTO、错误码、事件 | 生成 TS 绑定成功 |
| `lc-cli` 骨架 + 3 个示例命令 | `lc-cli node detect --json` 可运行 |
| CI 起立：fmt / clippy / test(windows+linux) | 绿灯 |
| **提交 SignPath Foundation 申请** | 已提交（审批可能数周，越早越好） |
| Monaco + Vue 3 + Vite 8 模板验证 | 能打开并编辑一个文件 |

P0 结束时必须能 `git tag` 出一个可安装的 Windows 包——哪怕功能为空。

### P1 五条 lane 并行（3–4 周）

| Lane | 范围 | 依赖 |
|---|---|---|
| **Runtime** | 探测状态机、手动兜底、便携版下载、设置页 | Contracts |
| **FS** | `ignore` 遍历、`notify` 监听、按层返回、事件序号 | Contracts |
| **Editor UI** | Monaco 集成、标签页、大文件分级、保存与冲突检测 | Contracts |
| **Lifecycle** | 窗口销毁/重建、托盘、单实例、`ProcessSupervisor` | Contracts |
| **CI/Packaging** | 三平台矩阵、性能 job、签名流程接入 | 无 |

**出口标准**：在未装 Node 的干净 Win11 虚拟机里，应用能启动、编辑文件、显示降级提示；托盘常驻 RSS < 60MB。

### P2 集成与首个 pre-release（1 周）

- 五条 lane 合流，端到端冒烟
- 性能门槛进 CI
- 发布 `v0.1.0-pre`（只有编辑器 + 文件树 + 运行时，也发）

早期反馈的价值大于晚三个月发完整版。

### P3 Agent 主体（4–6 周，三条 lane）

| Lane | 范围 |
|---|---|
| **Agent Loop** | 会话状态机、上下文预算、压缩、取消 |
| **Tools + Policy** | 工具注册表、并发控制、权限审批、路径校验 |
| **Integrations** | LLM 网关（归一化 + 透传）、MCP Host、记忆适配 |

**出口标准**：能完成一次真实多轮任务——读文件、改文件、跑命令、调 MCP 工具，全过程 UI 可视化。

### P4 周边（4–5 周，可并行）

- Git（status / diff / stage / commit / log）
- SSH 终端（`portable-pty` + xterm.js）
- LSP 宿主（先 TS/JS，用 Monaco 内置 worker）
- Linux x64/arm64 打包（原生 ARM runner）

### P5 扩展（按反馈排序）

- L2 外部 CLI 会话宿主
- 数据库 sidecar
- 通用 LSP（rust-analyzer / gopls / pyright 检测）
- 鸿蒙 spike（2 周，只做可行性验证）

## 3. 并行开发规范

### 目录所有权

```
crates/lc-proto         → Contracts（冻结后仅单人可改）
crates/lc-core/{runtime,fs,git,ssh,agent,mcp,llm,pty}
                        → 各自 lane
crates/lc-cli           → Contracts
apps/web/src/{editor,tabs,tree,agent,settings,services}
                        → Editor UI / Agent UI 分别所有
apps/desktop/src-tauri/{window,tray,process}
                        → Lifecycle
.github/, sidecars/     → CI/Packaging
```

### Definition of Done

不是"写完了"，而是：

- `cargo test -p lc-core` / `pnpm test` / `clippy -D warnings` 全绿
- `lc-cli` 对应命令有 golden 输出
- 契约未变更（或已 bump）

AI 生成代码最典型的失败是"看起来对、跑起来错"，所以测试要写成**规范**（golden + 属性测试），不是补充说明。

### 集成节奏

- trunk-based，功能用 feature flag 隔离
- 每日集成构建，每周一个 pre-release
- 不攒大 PR——并行产物的冲突解决成本会超过重新生成

### 模块边界检查

`cargo-deny`-style 检查：lane 之间不允许跨目录 import。这条对人和对 AI 同样有效，AI 更容易越界。

## 4. 性能门槛

CI job 在每次 PR 跑，四个数字进评论，**回归 > 15% 直接 fail**：

| 指标 | 目标 | 测量方式 |
|---|---|---|
| 托盘常驻 RSS（窗口已销毁） | < 60 MB | 窗口销毁后 10s 采样 |
| 窗口活跃 RSS（3 文件 + 终端） | < 400 MB | 脚本化场景 |
| 冷启动到可交互 | < 3s 首次 / < 1.5s 热 | 标记起止 |
| 20 次开关窗口后的残留进程 | 0 | 进程快照 diff |

第四项最容易失控，也最伤口碑。靠统一的 `ProcessSupervisor` 兜底：Windows 全部 Job Object，Linux 全部进程组。

## 5. 风险清单

按"会毁掉项目"到"会拖慢项目"排序。

### R1 轻量叙事被自己打脸（高）

WebView + Monaco 实测基础内存约 150–250MB，与精简 Electron 应用接近。真正的差距在**最小化后的常驻**与**冷启动**。

→ 把"轻量"定义写死并进 CI；对外宣传只讲托盘常驻和冷启动，不讲"窗口打开时的内存"。

### R2 系统 Node 不可控（高）

`NODE_OPTIONS` 污染、损坏的 npm、企业代理、杀软拦截、ABI 不兼容的原生模块。

→ 只用纯 JS 依赖；sidecar 独立进程；统一错误分类与可读提示；每类失败都有降级方案。

### R3 安全边界（高）

文件读写 + 子进程执行 + 用户提供路径，任一校验疏漏即本机 RCE。且应用要读 Claude Code 的配置，天然接触敏感数据。

→ 严格 CSP、禁远程内容；路径 `canonicalize` 后校验根内；不开放通用执行命令；敏感写入备份 + 原子替换；v0.2 之前做一次第三方安全审计。

### R4 窗口销毁/重建的状态 bug（中高）

销毁时正在进行的 Monaco 操作、未保存缓冲、终端会话、滚动位置。

→ 严格区分可丢弃状态（磁盘）与不可丢弃状态（Rust 侧）。**终端与 Agent 会话必须活在 Rust 侧**，这个决定要在写第一行终端代码之前定下。

### R5 进程泄漏（中高）

每次窗口销毁、LSP 重启、git 调用都可能留下孤儿。

→ 统一 `ProcessSupervisor`；退出时强制回收；提供"诊断"面板列出所有子进程 PID。

### R6 harness 范围失控（中高）

自己做 harness 意味着要处理工具语义、上下文压缩、权限、重试、超时、多 provider 差异——比"套壳"重得多。

→ v1 只支持 2–3 个 provider；工具集控制在 12 个以内；压缩策略先用最简单的"外部化 + 摘要"，不追求最优。

### R7 LSP 蔓延（中）

"转到定义"做全就是一个小型 VS Code。

→ v1 只做 TS/JS；通用 LSP 排 P5；从不捆绑 language server。

### R8 MSRV 漂移（中）

固定 1.99.1 意味着依赖 minor 升级可能直接让构建失败。

→ 提交 `Cargo.lock`；CI 用 `--locked`；Renovate 只提 patch；`CONTRIBUTING.md` 写明不得提升 MSRV。

### R9 鸿蒙不确定性（中）

无官方 Tauri 支持；libnode 可获得性、ArkWeb API 差异、签名上架流程均为未知。

→ spike-first，2 周见分晓；只付抽象层成本，不为鸿蒙牺牲桌面体验。

### R10 签名与分发摩擦（中低）

SignPath 审批周期、SmartScreen 声誉积累、Linux 用户对 AppImage 依赖 webkit 的不满。

→ P0 就提交申请；README 说明首次运行可能提示未知发布者；提供 SHA256。

### R11 上游格式漂移（低但持续）

Claude Code / Codex 的配置路径与格式会变。

→ 适配器完全数据驱动；启动时轻量探测，结构变化时提示"适配可能不完整"，不崩溃不静默。

## 6. 工程量估算

| 模块 | 人月 |
|---|---|
| 编辑器 + 文件树 + 标签页 | 3 |
| 生命周期 + 托盘 + 进程管理 | 1.5 |
| 运行时探测 | 1 |
| Agent harness（loop + tools + policy + llm + mcp + memory） | 6 |
| Git | 2 |
| SSH + 终端 | 1.5 |
| DB | 2 |
| LSP | 2 |
| UI / i18n / 设置 | 2 |
| CI/CD + 三平台 + 签名 | 1.5 |
| **合计** | **约 22.5 人月** |

单 Agent 串行约 22 个月；五 lane 并行、AI 承担大部分编码的情况下，实际日历时间取决于集成与验证速度，而非打字速度。瓶颈在 R4/R5 这类生命周期问题——它们只能靠真实运行发现。
