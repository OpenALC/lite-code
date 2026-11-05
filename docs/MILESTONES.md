# 里程碑

本文档是**出口标准的唯一权威**。`PLAN.md` 负责组织原则与风险清单，这里负责"每个阶段交付什么、怎么算完成"。

## 1. 平台支持矩阵

| 平台 | 等级 | 何时可用 | CI | 产物 |
|---|---|---|---|---|
| **Windows 10/11 x64** | 一等公民 | P0 起 | 每个 PR 必跑 | NSIS 安装包、便携 zip |
| **Linux x64** | 顺带 | P1 构建绿，P2 起出包 | 每个 PR 必跑 | AppImage、deb |
| **Linux arm64** | 顺带 | P6 | 打 tag 时交叉编译 | AppImage |
| **macOS** | 不支持 | — | 不构建、不测试 | — |
| **HarmonyOS** | 后续 | P6，spike 通过才做 | 手动 | ArkWeb 外壳 |

**macOS 的决策**：不做。不建 CI 矩阵、不合并 mac 专属路径、不接 mac 分发流程。issue 保留但不修，除非有维护者愿意长期承担。这样做的原因是工程资源有限，而 mac 上 WebView 与进程管理的坑与 Windows 完全不同量级。

**ARM64 Windows**：暂不支持，P6 之后按反馈决定。

## 2. 里程碑总览

| 里程碑 | 主题 | 周 | 累计 | 平台门禁 | 一句话出口 |
|---|---|---|---|---|---|
| **P0** | 地基 | 2 | 2 | Win 必过 | tag 出一个能装、功能为空的 Windows 包 |
| **P1** | 编辑器模式 alpha | 4 | 6 | Win 必过 · Linux 构建绿 | 干净 Win11 上无 Node 也能编辑文件 |
| **P2** | 首个 pre-release | 1 | 7 | 双平台出包 | `v0.2.1-preview.1` 可下载 |
| **P3** | Agent 模式可用 | 6 | 13 | Win 必过 | 完成一次真实多轮任务，可编辑/Agent 双模式无损切换 |
| **P4** | 工作台与并行 | 5 | 18 | 双平台 | 两个任务在各自 worktree 并行，互不干扰 |
| **P5** | 数据库与周边 | 5 | 23 | 双平台 | 查库→改数据（审批）→验证 闭环 |
| **P6** | 稳定化与鸿蒙 | 4+2 | 29 | 双平台正式包 | `v1.0.0`，安全审计高危清零 |

```
P0 ──▶ P1 ──▶ P2 ──▶ P3 ──┬──▶ P4 ──▶ P5 ──▶ P6
                          │
                     （P4/P5 部分 lane 可并行，见各阶段依赖）
```

每个里程碑结束必须产出五件东西：发布说明、`docs/` 对应更新、UI demo 同步、`CHANGELOG.md` 条目、下阶段 lane 分配表。

版本号规则见 [VERSIONING.md](VERSIONING.md)：`0.<P>.<发版序号>-preview.<重建号>`，P 编号直接落到第二部分。

| 里程碑 | 版本线 | 收尾 tag |
|---|---|---|
| P0 | `0.0.x` | `v0.0.N-preview.1` |
| P1 | `0.1.x` | `v0.1.N-preview.1` |
| P2 | `0.2.x` | `v0.2.N-preview.1` |
| P3 | `0.3.x` | `v0.3.N-preview.1` |
| P4 | `0.4.x` | `v0.4.N-preview.1` |
| P5 | `0.5.x` | `v0.5.N-preview.1` |
| P6 | `0.6.x` | `v0.6.N-preview.1` |
| 正式版 | `1.0.0` | `v1.0.0`（仅维护者声明） |

## 3. P0 · 地基（2 周，串行）

**目标**：可构建、可分发、可协作。功能为空是允许的。

| 交付 | lane | 说明 |
|---|---|---|
| Cargo workspace + `rust-toolchain.toml`(1.99.1) | Contracts | `--locked` 可构建 |
| `rustfmt.toml`、`deny.toml`、`.editorconfig` | CI | 先立规矩 |
| `lc-proto` v0 契约 | Contracts | 命令名、DTO、`ErrorCode`、事件枚举 |
| TS 绑定 codegen + 漂移检查 | Contracts | 生成产物进仓，CI 比对 |
| `lc-cli` 骨架 | Contracts | `node detect --json` 可运行 |
| CI 三件套 `ci.yml` / `release.yml` / `perf.yml` | CI | Windows + Ubuntu 矩阵绿 |
| Vue 3 + Vite 8 + Monaco 起壳 | Editor UI | 打开并编辑一个文件 |
| `LayoutProfile` 骨架 + 模式切换器 | Agent UI | 切到 Agent 视图只显示占位 |
| UI demo 迁入真实壳 | Editor UI | `demo/index.html` 作为视觉基线 |
| SignPath Foundation 申请提交 | CI | 审批可能数周，越早越好 |

**出口标准**

1. `cargo build --locked` 与 `cargo test --locked` 在 Windows、Ubuntu 双平台绿。
2. `lc-cli node detect --json` 输出契约字段，有 golden 输出。
3. 契约 TS 绑定生成成功，`git diff --exit-code` 通过（漂移检查生效）。
4. `rust-toolchain.toml` 与 `ci.yml` 的 `RUST_TOOLCHAIN` 一致性检查生效并能故意失败一次（证明检查不是摆设）。
5. tag `v0.0.1` 产出**可安装但功能为空**的 Windows NSIS 包，附 SHA256。
6. `release.yml` 能在 tag 上跑完全流程（签名步骤可跳过）。

**不做**：任何编辑器功能、任何 agent 能力、Linux 包。

## 4. P1 · 编辑器模式 alpha（4 周，五 lane）

| Lane | 范围 |
|---|---|
| **Runtime** | Node 探测状态机、手动指定、便携版下载、设置页、降级提示 |
| **FS** | `ignore` 遍历、`notify` 监听、按层返回、事件序号、路径校验 |
| **Editor UI** | Monaco 集成、标签页、大文件分级（>20MB 只读预览）、保存与冲突检测 |
| **Lifecycle** | 窗口销毁/重建、托盘、单实例、`ProcessSupervisor` |
| **CI/Packaging** | Windows 打包、性能 job 接入、Linux 构建（不出包） |

**出口标准**

1. 未装 Node 的干净 Win11 虚拟机：应用启动、打开仓库、新建/编辑/保存文件、显示降级提示且不崩溃。
2. 托盘常驻 RSS < 60 MB（窗口销毁后 10s 采样）。
3. 冷启动到可交互 < 3s（首次）/ < 1.5s（热）。
4. 连续 20 次开关窗口后残留子进程为 0。
5. 1 万个文件的目录首次展开 < 200ms，滚动不掉帧。
6. Ubuntu 22.04 上构建与 headless 测试通过（不出安装包）。
7. 性能四项指标进 CI 评论，回归 > 15% 直接 fail。

## 5. P2 · 首个 pre-release（1 周）

**目标**：把东西发出去，早拿反馈。

- 五 lane 合流，端到端冒烟清单跑通。
- 性能门槛设为**阻断**（不再是评论）。
- 发布 `v0.2.1-preview.1`：Windows NSIS + Linux AppImage（x64），附 SHA256 与已知问题清单。
- README 写清"首次运行可能提示未知发布者"与降级行为。

**出口标准**：发布页可下载、在干净虚拟机上能装能跑、已知问题清单不少于 5 条（诚实比完整重要）。

## 6. P3 · Agent 模式可用（6 周，四条 lane）

**目标**：双模式真正成立——用户能按 `Cmd/Ctrl+Shift+M` 在"对着代码干活"与"下命令等结果"之间切换，且切换无损。

| Lane | 范围 |
|---|---|
| **Agent Loop** | 会话状态机（Rust 侧）、上下文预算、压缩与事件、取消、重试 |
| **Tools + Policy** | 工具注册表（≤12 个）、并发控制、三层判定、路径校验、`PolicySet` 四种模式 |
| **Integrations** | LLM 网关（Responses 优先、Messages 次之）、SSE 流式、MCP Host、记忆适配 |
| **Agent UI** | 会话流（尺寸无关组件）、工具卡片、内联审批、子代理块、压缩记录、composer 三控件、上下文刻度条、`LayoutProfile` 完整实现 |

**出口标准**

1. 一次真实任务闭环：读文件 → 改文件 → 跑测试 → 汇报，全过程在会话流中可见，中途可取消。
2. 审批卡三按钮行为正确：`拒绝` 不执行、`本次会话允许同类` 不再询问同类、`允许` 仅一次。
3. `完全自主` 每会话首次确认 **且每次应用启动重新确认**。
4. 编辑器 ↔ Agent 切换无损：会话、审批状态、上下文用量、未应用变更全部保留。
5. 上下文跨 `compressAtK` 变黄，压缩后回橙并闪烁一次，留可展开的压缩记录。
6. 闸门规则单测覆盖：路径逃逸、黑名单、无 `WHERE` 写操作、计划模式无写工具。
7. `lc-cli agent run --prompt ... --headless` 能完成同一任务（CI 回归用）。

## 7. P4 · 工作台与并行（5 周）

**目标**：Agent 模式从"一个对话框"变成"工作台"，并且能同时跑多个任务。

| 范围 | 说明 |
|---|---|
| Task 实体 | `status` / `steps` / `changeset` / `checks`，见 `TASKS.md` |
| worktree 隔离 | `<数据目录>/worktrees/<project>/<task>`，分支 `lc/<task>-<slug>` |
| 三栏布局 | 左：项目 + 会话 + 功能；中：对话（760–820px）；右：工作台 |
| `ViewTarget` | `code` / `web` / `term` / `doc`，同 target 单实例 |
| Web 预览 | 默认 iframe，按需弹出独立系统 WebView 窗口；后台任务预览销毁 |
| 变更清单 | 逐文件 diff、应用、丢弃、打回重做；检查面板聚合测试结果 |
| 合并协议 | 一次一个 `--no-ff`；冲突转编辑器解决；基线漂移提示 |

**出口标准**

1. 两个任务并行写文件互不干扰，主工作区 `git status` 保持干净。
2. worktree 不出现在文件树、不被 file watcher 上报。
3. 后台任务不抢焦点：审批以状态栏计数呈现，点击跳转。
4. 切换任务时右侧标签组按会话恢复。
5. 合并冲突能在编辑器内解决并回到任务。
6. 第 3 个任务进入排队态且 UI 明示原因（并发上限 2）。
7. 后台任务被销毁预览后，唤回 1s 内重建并恢复 URL。

## 8. P5 · 数据库与周边（5 周）

| 范围 | 说明 |
|---|---|
| db-bridge | `pg` / `mysql2` / `mssql` / `node:sqlite`，esbuild 单文件，SSH 隧道复用 |
| 连接管理 | 项目级配置、`SecretStore` 存密钥、连接级只读开关 |
| 数据库面 | schema 树、SQL 编辑器、虚拟化结果网格、主键定位的单元格编辑 |
| Agent 工具 | `db_list` / `db_schema` / `db_query` / `db_explain` / `db_execute` / `db_open` |
| 闸门 | SQL 解析后分类、单语句、静态黑名单、`LIMIT` 注入、超时、只读事务、写事务 |
| 周边 | Git 面板、SSH 终端（`portable-pty` + xterm.js）、TS/JS LSP（Monaco worker） |

**出口标准**

1. agent 在真实项目完成：查 schema → 条件查询 →（审批）更新 → 再查验证。
2. 无 `WHERE` 的 `UPDATE`/`DELETE` 在 `完全自主` 下仍被拦截。
3. 10 万行结果不进上下文：`db_query` 只回 20 行样本 + 句柄，网格滚动流畅。
4. 注入样本测试：库内数据含"忽略之前指令"不改变工具行为（`trust: untrusted` 生效）。
5. 只读连接在协议层拒绝写（不只靠分类器），并有单测证明。
6. 并行任务对同一连接的写操作串行化，无交叉事务。

## 9. P6 · 稳定化与鸿蒙（4 周 + 2 周 buffer）

- 第三方安全审计整改，高危清零；v0.2 之前完成审计（见 `PLAN.md` R3）。
- 崩溃率、无响应、进程泄漏的长周期观测；更新器与回滚通道。
- 代码签名落地（SignPath 或 Azure Trusted Signing），SmartScreen 声誉。
- Linux arm64 正式包；i18n 完整性检查进 CI。
- 发布 `v1.0.0`。

**HarmonyOS 门禁（2 周 spike）**

只验证三件事：ArkWeb 能否跑现有前端产物、NAPI 能否承载 `CoreHandle`、Monaco 在 ArkWeb 上的内存与输入法是否可用。

- 三项全过 → 建独立外壳，复用 `apps/web` 产物。
- 只过前两项 → 交付"只读 + 对话"，编辑器降级为代码高亮预览。
- 不过 → 停止，结论写进 `ARCHITECTURE.md`，不牺牲桌面体验。

**出口标准**：30 天无 P0/P1 缺陷；崩溃率 < 0.1% 会话；启动 P95 < 3s；审计高危清零；鸿蒙结论已记录（无论是否继续）。

## 10. 门禁与 Definition of Done

任何里程碑关闭前：

- `cargo test --locked` / `clippy -D warnings` / `pnpm test` / `typecheck` 全绿。
- 契约未变更，或已 bump 并以独立 PR 合并。
- 对应 `lc-cli` 命令有 golden 输出。
- 性能四项未回归（`perf.yml` 阻断）。
- 文档与 demo 同步更新。
- 已知问题清单写进 release notes。

**跨 lane 规则**：lane 之间只通过 `lc-proto` 交互；`scripts/check-lanes.sh` 在 CI 中执行，越界 import 直接 fail。AI 比人更容易越界，这条检查优先于别的检查。

## 11. 风险与里程碑的对应

| 风险（见 PLAN） | 在哪一阶段暴露 | 处置 |
|---|---|---|
| R1 轻量叙事 | P1 / P2 | 性能门槛阻断；对外只讲托盘常驻与冷启动 |
| R2 系统 Node | P1 | 降级模式作为出口条件，不是可选 |
| R3 安全边界 | P3 / P5 / P6 | 闸门在 Rust；P6 前完成审计 |
| R4 窗口状态 | P1 / P3 | 会话与 PTY 活在 Rust 侧（P0 起即约束） |
| R5 进程泄漏 | P1 | 20 次开关的进程快照进 CI |
| R6 harness 失控 | P3 | 工具 ≤12 个，provider ≤3 |
| R7 LSP 蔓延 | P5 | 只做 TS/JS |
| R9 鸿蒙 | P6 | 2 周 spike 即止 |
| R10 签名 | P0 / P6 | P0 提交申请，P6 完成落地 |
| **R12 worktree 污染主工作区** | P4 | worktree 放数据目录 + watcher 忽略 + 文件树过滤 |
| **R13 SQL 注入面** | P5 | 结果标 `untrusted`；分类器 + 协议级只读双保险 |

## 12. 排期假设

- 单 lane 由 1 个 agent + 1 个人复核；lane 内部串行，lane 之间并行。
- P0 必须串行完成，P1 起五 lane 并行。
- 日历时间取决于集成与验证速度，不取决于编码速度（见 `PLAN.md` §6）。
- 每个里程碑预留 20% buffer；P6 的 buffer 已计入。
