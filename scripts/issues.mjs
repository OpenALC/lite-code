// 一次性脚本：把 docs/MILESTONES.md 展开成 GitHub issues。
// 依赖按声明顺序解析，因此被依赖项必须排在前面。
export const MILESTONES = [
  { code: 'P0', title: 'P0 地基' },
  { code: 'P1', title: 'P1 编辑器模式 alpha' },
  { code: 'P2', title: 'P2 首个 pre-release' },
  { code: 'P3', title: 'P3 Agent 模式可用' },
  { code: 'P4', title: 'P4 工作台与并行' },
  { code: 'P5', title: 'P5 数据库与周边' },
  { code: 'P6', title: 'P6 稳定化与鸿蒙' }
];

export const LABELS = [
  { name: 'lane:contracts', color: '8b8b8b', desc: 'lc-proto / 契约' },
  { name: 'lane:runtime',   color: '8b8b8b', desc: 'Node 运行时探测' },
  { name: 'lane:fs',        color: '8b8b8b', desc: '文件系统与监听' },
  { name: 'lane:editor',    color: '8b8b8b', desc: '编辑器与标签页' },
  { name: 'lane:lifecycle', color: '8b8b8b', desc: '窗口、托盘、进程' },
  { name: 'lane:ci',        color: '8b8b8b', desc: '构建、打包、发布' },
  { name: 'lane:agent',     color: 'd97a70', desc: 'Agent harness' },
  { name: 'lane:ui',        color: 'e8875a', desc: '前端与交互' },
  { name: 'lane:db',        color: '79b98a', desc: '数据库' },
  { name: 'lane:harmony',   color: '6f8fd0', desc: 'HarmonyOS / ArkWeb' },
  { name: 'kind:feat',      color: '0e8a16', desc: '新功能' },
  { name: 'kind:fix',       color: 'd73a4a', desc: '缺陷修复' },
  { name: 'kind:docs',      color: '0075ca', desc: '文档' },
  { name: 'kind:infra',     color: '5319e7', desc: '工程基建' },
  { name: 'milestone-exit', color: 'fbca04', desc: '里程碑出口条件，关闭前必须完成' }
];

const DOD = `## 完成定义（Definition of Done）

- [ ] \`cargo test --locked\` / \`clippy -D warnings\` 通过（涉及 Rust 时）
- [ ] \`pnpm lint\` / \`typecheck\` / \`test\` 通过（涉及前端时）
- [ ] 契约未变更，或已 bump 并以独立 \`[contract]\` PR 合并
- [ ] 对应 \`lc-cli\` 命令有 golden 输出（涉及核心逻辑时）
- [ ] 文档同步更新`;

/** key 用于依赖引用；deps 里填 key。 */
export const ISSUES = [
  // ---------------- P0 ----------------
  {
    key: 'p0-workspace', m: 'P0', lane: 'lane:contracts', kind: 'kind:infra',
    title: '初始化 Cargo workspace 与工具链锁定',
    body: `## 目标

建立可构建的 Rust 工作区，并把工具链版本写死，作为后续所有 lane 的地基。

## 范围

- \`Cargo.toml\` workspace：\`crates/lc-proto\`、\`crates/lc-core\`、\`crates/lc-cli\`
- \`rust-toolchain.toml\`：\`channel = "1.99.1"\`
- 根 \`Cargo.toml\` 的 \`rust-version\` 同步为 1.99
- 提交 \`Cargo.lock\`，所有 CI 命令带 \`--locked\`

## 验收标准

- [ ] \`cargo build --locked\` 在 Windows 与 Ubuntu 22.04 通过
- [ ] \`cargo test --workspace --locked\` 通过（可为空测试）
- [ ] \`rust-toolchain.toml\`、\`Cargo.toml\`、\`ci.yml\` 三处版本一致，\`prep\` 的断言能故意失败一次再恢复

${DOD}

参考：docs/MILESTONES.md §3，docs/CI.md §2`
  },
  {
    key: 'p0-repofiles', m: 'P0', lane: 'lane:ci', kind: 'kind:infra',
    title: '仓库规范文件：rustfmt / cargo-deny / editorconfig / 许可证',
    deps: ['p0-workspace'],
    body: `## 目标

先把规矩立好，避免 60+ 个 issue 落地后返工。

## 范围

- \`rustfmt.toml\`（edition 2021，默认风格）
- \`deny.toml\`：licenses（只允许 Apache-2.0 / MIT / BSD / ISC）、bans、advisories、sources
- \`.editorconfig\`：UTF-8、LF、Rust 4 空格、TS 2 空格
- \`LICENSE-MIT\` + \`LICENSE-APACHE\`
- \`CONTRIBUTING.md\`：写明不得提升 MSRV（PLAN.md R8）

## 验收标准

- [ ] \`cargo fmt --check\` 通过
- [ ] \`cargo deny check\` 通过（\`deny.yml\` 的 job 从跳过变为真实执行）
- [ ] 许可证文件与 \`package.json\` / \`Cargo.toml\` 的 \`license\` 字段一致

${DOD}

参考：docs/CI.md §2`
  },
  {
    key: 'p0-proto', m: 'P0', lane: 'lane:contracts', kind: 'kind:feat',
    title: 'lc-proto v0 契约草案',
    deps: ['p0-workspace'],
    body: `## 目标

定下 UI 与核心之间唯一的同步点。定稿后只读，变更必须走独立 \`[contract]\` PR。

## 范围

- 命令枚举：\`fs.*\` \`node.*\` \`git.*\` \`agent.*\`
- DTO：\`FileNode\` \`RuntimeStatus\` \`SessionState\` \`ErrorCode\`
- 事件枚举：带单调序号，UI 只订阅
- \`ErrorCode\` 与可读消息分离（README 约定：错误码放 tooltip / 日志）

## 验收标准

- [ ] 所有类型可 serde 序列化，有往返单测
- [ ] 事件序号单调性有属性测试
- [ ] \`lc-cli\` 能输出其中至少一个 DTO 的 JSON

${DOD}

参考：docs/ARCHITECTURE.md §3，docs/README.md「三条不可退让的架构约束」`
  },
  {
    key: 'p0-codegen', m: 'P0', lane: 'lane:contracts', kind: 'kind:infra',
    title: 'TS 绑定 codegen 与漂移检查',
    deps: ['p0-proto'],
    body: `## 目标

让前端拿到的类型永远与契约同步，且漂移能在 CI 被发现（而不是在运行时）。

## 范围

- \`lc-proto\` 增加 \`codegen\` feature 与二进制
- 生成产物进仓（\`apps/web/src/proto/\`）
- CI \`contract\` job 用 \`git diff --exit-code\` 校验

注意：\`ci.yml\` 里当前是占位命令，需按实际二进制名修改。

## 验收标准

- [ ] 手动改一处 DTO 但不忘重新生成时，CI \`contract\` job 失败
- [ ] 重新生成后 CI 绿
- [ ] 生成的前端类型可被 TS 编译器消费，无 \`any\`

${DOD}

参考：docs/CI.md §2`
  },
  {
    key: 'p0-cli', m: 'P0', lane: 'lane:contracts', kind: 'kind:feat',
    title: 'lc-cli 骨架与 node detect 命令',
    deps: ['p0-proto'],
    body: `## 目标

核心必须具备 headless 前端，这是 CI 回归与 Agent 自证的前提。

## 范围

- clap 子命令结构，\`--json\` 全局开关
- \`lc-cli node detect --json\`：返回成功 / 路径 / 版本 / 状态
- 三个示例命令打通命令注册到 DTO 输出的链路

## 验收标准

- [ ] \`lc-cli node detect --json\` 在本机输出合法 JSON
- [ ] golden 输出测试（版本号等易变字段做脱敏）
- [ ] 退出码约定：0 成功、2 可恢复错误、3 内部错误

${DOD}

参考：docs/ARCHITECTURE.md §1（lc-cli 是并行验证杠杆）`
  },
  {
    key: 'p0-ci', m: 'P0', lane: 'lane:ci', kind: 'kind:infra',
    title: 'ci.yml 从跳过态切换为真实执行并跑绿',
    deps: ['p0-workspace', 'p0-repofiles'],
    body: `## 目标

工作流已存在，但仓库还没有 \`Cargo.toml\` 与 \`package.json\`，所有 job 都在跳过。本 issue 负责让它们真正跑起来。

## 验收标准

- [ ] fmt / clippy（windows + ubuntu）/ test 双平台全绿
- [ ] \`contract\` 与 \`lanes\` job 从 skip 变为 run
- [ ] \`cargo-deny\` job 真实执行
- [ ] 故意引入 \`clippy\` warning，确认 CI 变红后恢复
- [ ] 缓存命中（第二次运行 clippy job < 3 分钟）

${DOD}

参考：docs/CI.md §2`
  },
  {
    key: 'p0-perf-harness', m: 'P0', lane: 'lane:ci', kind: 'kind:infra',
    title: '性能测量 harness 与基线文件',
    deps: ['p0-ci'],
    body: `## 目标

\`perf.yml\` 引用的脚本尚不存在，本 issue 补齐，让四个指标可以被测量。

## 范围

- \`scripts/perf/run.ps1\`：输出 \`{ trayRssMb, activeRssMb, coldStartMs, warmStartMs, residualProcs }\`
- \`scripts/perf/compare.ps1\`：与 baseline 比对，超过 \`REGRESSION_PCT\` 失败
- \`scripts/perf/comment.ps1\`：把四个数字发到 PR 评论
- \`scripts/perf/baseline.json\`

## 验收标准

- [ ] 手动触发 \`perf.yml\` 能产出 metrics.json
- [ ] 基线更新必须人工审批（PR 改 baseline 不被自动接受）
- [ ] 指标缺失时给出 \`::warning\` 而不是静默通过

${DOD}

参考：docs/CI.md §3，docs/PLAN.md §4`
  },
  {
    key: 'p0-nightly', m: 'P0', lane: 'lane:ci', kind: 'kind:infra',
    title: 'nightly 工作流验证',
    deps: ['p0-ci'],
    body: `## 目标

确认每日构建 + 冒烟 + 失败开 issue 的链路可用（\`nightly.yml\` 已写好但从未真实跑过 schedule）。

## 验收标准

- [ ] \`workflow_dispatch\` 手动触发成功
- [ ] 冒烟脚本缺失时输出 notice 而不是失败
- [ ] 人为制造失败，确认 issue 被创建且重复失败只追加评论

${DOD}

参考：docs/CI.md §5`
  },
  {
    key: 'p0-lanes', m: 'P0', lane: 'lane:ci', kind: 'kind:infra',
    title: 'scripts/check-lanes.sh 目录边界检查',
    deps: ['p0-workspace'],
    body: `## 目标

lane 之间只允许通过 \`lc-proto\` 交互。AI 比人更容易越界，这条检查优先于别的检查。

## 验收标准

- [ ] 用 \`cargo metadata\` 取依赖图，与允许矩阵比对
- [ ] \`crates/lc-core/{agent}\` 引用 \`fs\` 的内部模块时脚本失败
- [ ] 引用 \`lc_proto\` 与同 lane 的 \`pub\` 接口时通过
- [ ] 在 CI 中真实执行（\`lanes\` job 不再 skip）

${DOD}

参考：docs/PLAN.md §3，docs/CI.md §8`
  },
  {
    key: 'p0-web-shell', m: 'P0', lane: 'lane:ui', kind: 'kind:feat',
    title: '前端壳：Vue 3 + TS 5.9 + Vite 8 + Monaco',
    deps: ['p0-workspace'],
    body: `## 目标

前端能启动、打开并编辑保存一个文件。不追求功能，追求链路通。

## 范围

- \`apps/web\`：Vue 3 + TypeScript 5.9 + Vite 8（不使用 JSX，不用 TS7）
- Monaco 动态加载（首屏不含编辑器代码，docs/UI.md §6）
- \`apps/desktop\`：Tauri 2 薄壳，注册一个 \`fs.readFile\` 命令

## 验收标准

- [ ] \`pnpm dev\` 可启动，\`pnpm build\` 产物可被 Tauri 加载
- [ ] 打开一个文件、编辑、保存，磁盘内容变化
- [ ] 首次加载的 JS 体积不含 Monaco chunk

${DOD}

参考：docs/ARCHITECTURE.md §2，docs/UI.md §6`
  },
  {
    key: 'p0-layout-profile', m: 'P0', lane: 'lane:ui', kind: 'kind:feat',
    title: 'LayoutProfile 骨架与模式切换器',
    deps: ['p0-web-shell'],
    body: `## 目标

为 P3 的双模式打地基：一个壳装两种布局，而不是两套 UI。

## 范围

- \`LayoutProfile\`：\`rail / side / main / agent / bottom\` 槽位配置
- 切换器放在标签栏左侧，快捷键 \`Ctrl/Cmd+Shift+M\`
- Agent 视图先给占位（会话流用静态块）

## 验收标准

- [ ] 切换时**不重建** Monaco、不重载工作区
- [ ] 切换后焦点落在新主区域的第一个可交互元素
- [ ] 布局偏好按 workspace 持久化

${DOD}

参考：docs/UI.md §1`
  },
  {
    key: 'p0-demo-import', m: 'P0', lane: 'lane:ui', kind: 'kind:docs',
    title: 'UI demo 迁入真实壳并抽取 token',
    deps: ['p0-web-shell'],
    body: `## 目标

把 \`demo/index.html\` 作为视觉基线落进真实项目，避免后续 UI 走形。

## 范围

- \`demo/index.html\` 的 CSS 变量抽为 \`apps/web/src/styles/tokens.css\`
- 颜色、间距、字号、圆角、动效时长全部 token 化
- 深色 / 浅色两套值原样保留

## 验收标准

- [ ] 真实壳与 demo 的并排截图一致（允许 1px 以内偏差）
- [ ] 无硬编码色值残留（\`grep -n "#[0-9a-f]\\{6\\}" apps/web/src\` 为空）
- [ ] \`prefers-reduced-motion\` 规则仍在

${DOD}

参考：docs/UI.md §2`
  },
  {
    key: 'p0-signpath', m: 'P0', lane: 'lane:ci', kind: 'kind:infra',
    title: '提交 SignPath Foundation 签名申请',
    deps: [],
    body: `## 目标

签名审批可能数周，越早提交越好（docs/PLAN.md R10）。

## 验收标准

- [ ] 申请已提交，记录申请链接与状态
- [ ] 在仓库变量中登记 \`SIGNPATH_ENABLED=false\`（拿到凭据前不启用）
- [ ] 若被拒，评估 Azure Trusted Signing 作为备选并记录结论

${DOD}`
  },
  {
    key: 'p0-first-tag', m: 'P0', lane: 'lane:ci', kind: 'kind:fix',
    title: '首次发版 v0.0.1-preview.1（能装但功能为空）',
    deps: ['p0-ci', 'p0-web-shell', 'p0-layout-profile'],
    body: `## 目标

P0 的最终出口：\`git tag\` 出一个**可安装**的 Windows 包。

## 范围

- 修通 \`release.yml\` 的 \`verify\` job（\`uses: ./.github/workflows/ci.yml\` 的复用路径）
- 三处版本号同步为 \`0.0.1-preview.1\`
- 建 \`docs/releases/0.0.1-preview.1.md\`

## 验收标准

- [ ] tag 触发全流程，产出 draft release 且自动标记 prerelease
- [ ] NSIS 包在干净 Win11 虚拟机上可安装、可启动
- [ ] \`SHA256SUMS\` 随发布附带
- [ ] 版本校验能拒绝非法 tag（例如 \`v0.0.1\` 不带 preview）

${DOD}

参考：docs/VERSIONING.md，docs/CI.md §4`
  },

  // ---------------- P1 ----------------
  {
    key: 'p1-node-fsm', m: 'P1', lane: 'lane:runtime', kind: 'kind:feat',
    title: 'Node 探测状态机',
    body: `## 目标

系统 Node 不可控（PLAN.md R2），探测必须是状态机而不是一次性检查。

## 范围

- 探测顺序：\`NODE_PATH\` 环境 → PATH → 常见安装目录 → 便携版目录
- 状态：\`unknown / probing / ok / outdated / broken / missing\`
- 记录版本、路径、来源、\`NODE_OPTIONS\` 是否被污染

## 验收标准

- [ ] 每类状态都有可复现的注入方式与对应单测
- [ ] 损坏的 npm 不导致应用崩溃，只降级
- [ ] 探测不超过 300ms，且可取消

${DOD}

参考：docs/RUNTIME.md`
  },
  {
    key: 'p1-runtime-settings', m: 'P1', lane: 'lane:runtime', kind: 'kind:feat',
    title: '手动指定 Node 与设置页',
    deps: ['p1-node-fsm'],
    body: `## 目标

自动探测失败时，用户必须能手动兜底。

## 验收标准

- [ ] 设置页可选择 Node 路径，立即重新探测并给出结果
- [ ] 手动路径持久化，重启后仍生效
- [ ] 路径指向非可执行文件时给可读提示（不暴露错误码）

${DOD}

参考：docs/RUNTIME.md`
  },
  {
    key: 'p1-portable-node', m: 'P1', lane: 'lane:runtime', kind: 'kind:feat',
    title: '便携版 Node 下载与安装',
    deps: ['p1-node-fsm'],
    body: `## 目标

未安装 Node 的机器也能使用依赖 Node 的可选能力。

## 验收标准

- [ ] 下载前显示体积与来源，需用户确认
- [ ] 校验 SHA256，失败不落盘
- [ ] 解压到数据目录，不污染系统 PATH
- [ ] 网络失败给出重试与手动下载引导

${DOD}

参考：docs/RUNTIME.md`
  },
  {
    key: 'p1-degraded', m: 'P1', lane: 'lane:runtime', kind: 'kind:feat',
    title: '降级模式：无 Node 时的提示与能力边界',
    deps: ['p1-node-fsm'],
    body: `## 目标

P1 的出口标准之一：未装 Node 的干净 Win11 上不崩溃、提示清楚。

## 验收标准

- [ ] 状态栏 Node 项显示灰色"未配置"，点击进设置
- [ ] 依赖 Node 的功能（MCP、db-bridge）隐藏或置灰并说明原因
- [ ] 编辑器 / 文件树 / Git 完全可用

${DOD}

参考：docs/UI.md §3 状态栏，docs/RUNTIME.md`
  },
  {
    key: 'p1-fs-walk', m: 'P1', lane: 'lane:fs', kind: 'kind:feat',
    title: '文件遍历：ignore 规则与按层返回',
    body: `## 范围

- \`ignore\` crate：尊重 \`.gitignore\` / \`.ignore\`
- 按层返回（一次一层），不一次性扫全树
- 事件序号，按目录增量更新

## 验收标准

- [ ] 1 万文件目录首次展开 < 200ms
- [ ] \`.gitignore\` 中的路径不出现
- [ ] 遍历可取消，取消后无后台线程残留

${DOD}

参考：docs/ARCHITECTURE.md，docs/UI.md §6`
  },
  {
    key: 'p1-fs-watch', m: 'P1', lane: 'lane:fs', kind: 'kind:feat',
    title: '文件监听与事件序号',
    deps: ['p1-fs-walk'],
    body: `## 验收标准

- [ ] \`notify\` 事件合并去抖（同一文件 100ms 内多次写入只上报一次）
- [ ] 事件带单调序号，UI 可检测丢事件并全量刷新
- [ ] 删除/重命名事件正确映射到树节点
- [ ] 监听 root 之外的变化不上报

${DOD}`
  },
  {
    key: 'p1-path-guard', m: 'P1', lane: 'lane:fs', kind: 'kind:fix',
    title: '路径校验：canonicalize 后必须落在根内',
    deps: ['p1-fs-walk'],
    body: `## 目标

安全边界（PLAN.md R3）：任一校验疏漏即本机 RCE。

## 验收标准

- [ ] \`..\` 逃逸、符号链接逃逸、Windows 8.3 短名、UNC 路径全部被拒
- [ ] 大小写不敏感文件系统的重名冲突被测到（Windows）
- [ ] 所有文件写入走同一校验函数，有单测覆盖上述五类
- [ ] 拒绝时返回可读消息 + 建议动作

${DOD}`
  },
  {
    key: 'p1-monaco', m: 'P1', lane: 'lane:editor', kind: 'kind:feat',
    title: 'Monaco 集成与标签页',
    deps: ['p0-web-shell'],
    body: `## 验收标准

- [ ] 打开/关闭/切换标签，中键关闭，dirty 点显示
- [ ] 关闭 dirty 标签时二次确认
- [ ] 同一文件被打开两次时聚焦已有标签
- [ ] 滚动位置在切换后保留

${DOD}

参考：docs/UI.md §3`
  },
  {
    key: 'p1-large-file', m: 'P1', lane: 'lane:editor', kind: 'kind:feat',
    title: '大文件分级策略',
    deps: ['p1-monaco'],
    body: `## 验收标准

- [ ] > 5MB：关闭语法高亮
- [ ] > 20MB：只读预览 + "用系统编辑器打开"
- [ ] > 100MB：拒绝打开并说明原因
- [ ] 阈值可配置，有单测

${DOD}

参考：docs/ARCHITECTURE.md §5`
  },
  {
    key: 'p1-save-conflict', m: 'P1', lane: 'lane:editor', kind: 'kind:fix',
    title: '保存与外部修改冲突检测',
    deps: ['p1-monaco'],
    body: `## 验收标准

- [ ] 保存前比对 mtime + size，被外部修改时提示三选一（覆盖 / 重新加载 / 另存）
- [ ] 原子替换写入，不产生半截文件
- [ ] 冲突时不静默覆盖

${DOD}`
  },
  {
    key: 'p1-window-lifecycle', m: 'P1', lane: 'lane:lifecycle', kind: 'kind:feat',
    title: '窗口销毁与重建',
    body: `## 目标

最小化后释放内存是"轻量"叙事的核心（PLAN.md R1/R4）。

## 验收标准

- [ ] 最小化 60s 后销毁 WebView，托盘常驻 RSS < 60MB
- [ ] 唤回后重建，1s 内恢复：打开的文件、标签、滚动位置、终端会话
- [ ] 销毁时正在进行的 Monaco 操作不产生未捕获异常
- [ ] 20 次开关窗口，无残留进程、无内存持续增长

${DOD}

参考：docs/ARCHITECTURE.md §5.4`
  },
  {
    key: 'p1-tray-single', m: 'P1', lane: 'lane:lifecycle', kind: 'kind:feat',
    title: '托盘与单实例',
    deps: ['p1-window-lifecycle'],
    body: `## 验收标准

- [ ] 托盘菜单：打开 / 新建窗口 / 退出
- [ ] 第二次启动唤起已有实例并聚焦，不新起进程
- [ ] 关闭窗口不退出应用（可在设置中改为退出）
- [ ] 托盘退出时回收所有子进程

${DOD}`
  },
  {
    key: 'p1-process-supervisor', m: 'P1', lane: 'lane:lifecycle', kind: 'kind:feat',
    title: 'ProcessSupervisor：统一子进程生命周期',
    body: `## 目标

进程泄漏（PLAN.md R5）是口碑杀手，必须统一收口。

## 范围

- Windows：全部 Job Object
- Linux：进程组 + \`PR_SET_PDEATHSIG\`
- 提供"诊断"面板列出所有子进程 PID

## 验收标准

- [ ] 父进程被强杀后，子进程 1s 内退出
- [ ] 诊断面板可看到 PID / 命令行 / 启动时间
- [ ] 20 次开关窗口后进程快照 diff 为 0

${DOD}

参考：docs/ARCHITECTURE.md §3`
  },
  {
    key: 'p1-windows-package', m: 'P1', lane: 'lane:ci', kind: 'kind:infra',
    title: 'Windows NSIS 打包',
    deps: ['p1-window-lifecycle'],
    body: `## 验收标准

- [ ] NSIS 安装包与便携 zip 都能产出
- [ ] 安装到含空格与中文的路径可用
- [ ] 卸载不残留数据目录（配置保留策略可配）
- [ ] 图标与版本信息正确

${DOD}

参考：docs/CI.md §4`
  },
  {
    key: 'p1-perf-in-ci', m: 'P1', lane: 'lane:ci', kind: 'kind:infra',
    title: '性能四项进 CI 评论',
    deps: ['p0-perf-harness', 'p1-window-lifecycle'],
    body: `## 验收标准

- [ ] 每次 PR 输出四项数字与基线对比
- [ ] 回归 > 15% fail
- [ ] 冷启动与 RSS 的测量脚本在 CI 上稳定（连续三次运行方差 < 10%）

${DOD}

参考：docs/CI.md §3`
  },
  {
    key: 'p1-linux-build', m: 'P1', lane: 'lane:ci', kind: 'kind:infra',
    title: 'Linux 构建（不出包）',
    deps: ['p1-windows-package'],
    body: `## 验收标准

- [ ] ubuntu-22.04 上 \`cargo build\` 与 headless 测试通过
- [ ] \`webkit2gtk-4.1\` 依赖在 CI 中正确安装
- [ ] 不产出安装包，只验证构建

${DOD}`
  },

  // ---------------- P2 ----------------
  {
    key: 'p2-smoke', m: 'P2', lane: 'lane:ci', kind: 'kind:infra',
    title: '五 lane 合流与端到端冒烟清单',
    deps: ['p1-node-fsm', 'p1-fs-walk', 'p1-monaco', 'p1-window-lifecycle'],
    body: `## 范围

- 冒烟清单：启动 / 打开仓库 / 新建文件 / 编辑 / 保存 / 切分支 / 托盘 / 退出
- 在干净 Win11 虚拟机上执行并记录

## 验收标准

- [ ] 清单全部通过，逐条附截图
- [ ] 冒烟脚本可重复执行（脚本化程度 ≥ 70%）

${DOD}`
  },
  {
    key: 'p2-perf-gate', m: 'P2', lane: 'lane:ci', kind: 'kind:infra',
    title: '性能门槛改为阻断',
    deps: ['p1-perf-in-ci'],
    body: `## 验收标准

- [ ] 性能 job 失败即阻塞合并
- [ ] 基线更新仍需人工合并
- [ ] 在分支保护中登记为 required check

${DOD}`
  },
  {
    key: 'p2-readme-known', m: 'P2', lane: 'lane:docs', kind: 'kind:docs',
    title: 'README 与已知问题清单',
    deps: ['p2-smoke'],
    body: `## 验收标准

- [ ] README 写清安装方式、首次运行提示未知发布者、降级行为
- [ ] 已知问题不少于 5 条（诚实优先）
- [ ] 附 SHA256 校验说明

${DOD}`
  },
  {
    key: 'p2-release', m: 'P2', lane: 'lane:ci', kind: 'kind:feat',
    title: '发布 v0.2.1-preview.1',
    deps: ['p2-smoke', 'p2-perf-gate', 'p2-readme-known'],
    body: `## 验收标准

- [ ] Windows NSIS + Linux AppImage 双产物
- [ ] release notes 来自 \`docs/releases/0.2.1-preview.1.md\`
- [ ] 自动标记 prerelease
- [ ] 干净虚拟机上可安装可启动

${DOD}

参考：docs/VERSIONING.md`
  },

  // ---------------- P3 ----------------
  {
    key: 'p3-session-fsm', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: '会话状态机（Rust 侧）',
    body: `## 目标

会话必须活在 Rust 侧，窗口销毁不影响（PLAN.md R4）。

## 范围

- 状态：\`idle / streaming / tool-running / awaiting-approval / cancelled / failed\`
- 取消语义：正在执行的工具收到取消信号，已批准的命令不追溯
- 会话记录持久化

## 验收标准

- [ ] 状态转移非法时 panic 而非静默（开发构建）
- [ ] 取消后 500ms 内回到 idle，无悬挂工具
- [ ] 窗口销毁重建后会话完整恢复

${DOD}

参考：docs/AGENT.md §2`
  },
  {
    key: 'p3-context-budget', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: '上下文预算与压缩',
    deps: ['p3-session-fsm'],
    body: `## 验收标准

- [ ] 预算按 \`contextK\` 计算，\`contextK\` 为空的模型不允许启用
- [ ] 跨过 \`compressAtK\` 触发压缩，保留最近 6 条 + 摘要
- [ ] 发出 \`session:compacted { from, to }\` 事件
- [ ] 恰好等于阈值时不触发（边界测试）
- [ ] 压缩后可继续对话，不丢工具调用配对

${DOD}

参考：docs/AGENT.md §3.2 / §6`
  },
  {
    key: 'p3-tool-registry', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: '工具注册表与并发控制',
    deps: ['p3-session-fsm'],
    body: `## 范围

- 工具声明：名称、参数 schema、风险等级、是否只读
- 工具总数 ≤ 12（PLAN.md R6）
- 并发：读工具可并行，写工具串行

## 验收标准

- [ ] 单个工具失败不影响其他并发工具
- [ ] 同一文件两个写工具有顺序保证
- [ ] 工具超时可取消

${DOD}

参考：docs/AGENT.md §4`
  },
  {
    key: 'p3-policy', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: '三层判定与 PolicySet 四种模式',
    deps: ['p3-tool-registry'],
    body: `## 范围

- 静态黑名单 ∪ 用户黑名单 ∪ 模型判定高风险
- \`plan / chat / auto / yolo\` 四种 PolicySet
- \`yolo\` 每会话首次确认，且**每次应用启动**重新确认

## 验收标准

- [ ] 四种模式的工具可用性有矩阵测试
- [ ] 计划模式无任何写工具
- [ ] \`yolo\` 下黑名单仍拦截
- [ ] 模式切换写入会话审计记录

${DOD}

参考：docs/AGENT.md §5.1 / §5.2`
  },
  {
    key: 'p3-llm-responses', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: 'LLM 网关：OpenAI Responses',
    deps: ['p3-session-fsm'],
    body: `## 验收标准

- [ ] 流式 SSE：文本增量与工具调用增量都能解析
- [ ] 用量统计回填到会话预算
- [ ] 429 / 5xx 有退避重试，可取消
- [ ] 端点与密钥来自供应商配置，密钥不落日志

${DOD}

参考：docs/AGENT.md §3`
  },
  {
    key: 'p3-llm-messages', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: 'LLM 网关：Anthropic Messages',
    deps: ['p3-llm-responses'],
    body: `## 验收标准

- [ ] 与 Responses 共用归一化后的内部消息格式
- [ ] 工具调用语义差异在网关层抹平，不外泄到 harness
- [ ] 两家的 golden 响应各一份测试夹具

${DOD}

参考：docs/AGENT.md §3`
  },
  {
    key: 'p3-mcp', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: 'MCP Host',
    deps: ['p3-tool-registry'],
    body: `## 验收标准

- [ ] 支持 stdio 与 http 两种传输
- [ ] MCP 工具并入注册表，命名加 server 前缀避免冲突
- [ ] 连接失败在状态栏变红并可查看详情
- [ ] server 崩溃自动重连一次，之后标记异常

${DOD}

参考：docs/AGENT.md §8`
  },
  {
    key: 'p3-memory', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: '长期记忆适配',
    deps: ['p3-context-budget'],
    body: `## 验收标准

- [ ] 读取外部 Claude Code / Codex 配置时容错，结构变化只提示不崩溃
- [ ] 记忆条目的写入可开关
- [ ] 路径与格式变化时给出"适配可能不完整"提示

${DOD}

参考：docs/AGENT.md §7，PLAN.md R11`
  },
  {
    key: 'p3-stream-ui', m: 'P3', lane: 'lane:ui', kind: 'kind:feat',
    title: '会话流 UI（尺寸无关组件）',
    deps: ['p0-layout-profile', 'p3-session-fsm'],
    body: `## 目标

会话流必须能在右侧 360px 栏与主区全宽两种尺寸下工作，P4 才不需要重写。

## 验收标准

- [ ] 容器宽度变化时无重排闪烁，代码块自适应
- [ ] 用户消息右对齐窄栏，助手全宽
- [ ] 长会话滚动性能：1000 条消息不卡
- [ ] 流式输出时不抖动（占位高度稳定）

${DOD}

参考：docs/UI.md §3`
  },
  {
    key: 'p3-tool-cards', m: 'P3', lane: 'lane:ui', kind: 'kind:feat',
    title: '工具调用卡片',
    deps: ['p3-stream-ui'],
    body: `## 验收标准

- [ ] 显示工具名、参数摘要、耗时、结果前 5 行
- [ ] 可展开全文，展开状态在切换会话后保留
- [ ] 参数过长省略，hover 显示完整
- [ ] 失败工具用红色状态但不用红色图标

${DOD}

参考：docs/UI.md §3`
  },
  {
    key: 'p3-permission-card', m: 'P3', lane: 'lane:ui', kind: 'kind:feat',
    title: '内联审批卡',
    deps: ['p3-policy', 'p3-stream-ui'],
    body: `## 验收标准

- [ ] 完整命令 + 目标路径 + 预计耗时
- [ ] 三按钮：拒绝 / 本次会话允许同类 / 允许
- [ ] 决定后卡片就地变形为结果，不弹模态框
- [ ] 后台会话请求审批不抢焦点，只增加状态栏计数

${DOD}

参考：docs/UI.md §3，docs/AGENT.md §5.4`
  },
  {
    key: 'p3-composer', m: 'P3', lane: 'lane:ui', kind: 'kind:feat',
    title: 'Composer：模式 / 模型 / 思考与上下文刻度条',
    deps: ['p3-context-budget'],
    body: `## 验收标准

- [ ] 三个常驻控件带一行说明，不进设置页
- [ ] \`完全自主\` 橙色描边 + 警告图标，首次切换确认
- [ ] 不支持挡位置灰并说明原因
- [ ] 上下文条带 \`compressAtK\` 刻度，跨阈值变黄，压缩后闪烁一次
- [ ] 默认模型与当前会话模型分开显示

${DOD}

参考：docs/UI.md §3`
  },
  {
    key: 'p3-dual-mode', m: 'P3', lane: 'lane:ui', kind: 'kind:feat',
    title: '双模式切换完整实现与无损 handoff',
    deps: ['p3-stream-ui', 'p0-layout-profile'],
    body: `## 验收标准

- [ ] 编辑器 ↔ Agent 切换时会话、审批状态、上下文用量、未应用变更全部保留
- [ ] 切换器显示运行态点，状态栏显示进度
- [ ] 无过场动画，仅主体 150ms 淡入
- [ ] 每个 workspace 记住上次模式

${DOD}

参考：docs/UI.md §1`
  },
  {
    key: 'p3-headless', m: 'P3', lane: 'lane:agent', kind: 'kind:feat',
    title: 'lc-cli agent headless 回归',
    deps: ['p3-llm-responses', 'p3-tool-registry'],
    body: `## 验收标准

- [ ] \`lc-cli agent run --prompt ... --headless\` 完成一次读文件 + 改文件
- [ ] 输出结构化 JSON（步骤、工具、耗时、token）
- [ ] 在 nightly 中作为冒烟运行（需 \`LLM_TEST_API_KEY\`）
- [ ] 失败时输出最后一个失败的步骤与错误码

${DOD}`
  },

  // ---------------- P4 ----------------
  {
    key: 'p4-task-entity', m: 'P4', lane: 'lane:agent', kind: 'kind:feat',
    title: 'Task 实体与事件',
    deps: ['p3-session-fsm'],
    body: `## 范围

见 docs/TASKS.md §2：\`Task\` / \`TaskStatus\` / \`Worktree\` / \`FileChange\` / \`Check\` 与七类事件。

## 验收标准

- [ ] 事件增量更新 changeset 与 checks，不靠结束后整体扫描
- [ ] 重启后 \`Queued\` / \`Review\` 保留，\`Running\` 标记为 \`Failed(interrupted)\`
- [ ] 事件带序号，UI 可检测丢失

${DOD}

参考：docs/TASKS.md`
  },
  {
    key: 'p4-worktree', m: 'P4', lane: 'lane:agent', kind: 'kind:feat',
    title: 'worktree 隔离',
    deps: ['p4-task-entity'],
    body: `## 验收标准

- [ ] worktree 建在数据目录，不在项目内
- [ ] 进 watcher 忽略表与文件树过滤表（P4 出口标准）
- [ ] 写工具路径校验落在 worktree 内
- [ ] 删除前检查未提交改动，有则转 Review
- [ ] 非 git 项目降级为串行并标注原因

${DOD}

参考：docs/TASKS.md §3`
  },
  {
    key: 'p4-three-column', m: 'P4', lane: 'lane:ui', kind: 'kind:feat',
    title: 'Agent 模式三栏布局',
    deps: ['p3-dual-mode'],
    body: `## 范围

左：项目 + 会话 + 功能；中：对话（760–820px 居中）；右：工作台标签。

## 验收标准

- [ ] 中栏内容宽度锁定并居中，代码块可突破
- [ ] 左栏会话列表跨项目可见但按项目折叠，带状态点
- [ ] 切换会话恢复右侧标签组
- [ ] 底部面板在 Agent 模式合并进右侧

${DOD}`
  },
  {
    key: 'p4-view-target', m: 'P4', lane: 'lane:ui', kind: 'kind:feat',
    title: 'ViewTarget 工作台：code / web / term / doc',
    deps: ['p4-three-column'],
    body: `## 验收标准

- [ ] 同一 target 只开一份（重复打开聚焦已有标签）
- [ ] 工具结果可推送到右侧而不塞进气泡
- [ ] 终端不随会话切换清空（是进程不是视图）
- [ ] 标签组按会话持久化

${DOD}`
  },
  {
    key: 'p4-preview', m: 'P4', lane: 'lane:ui', kind: 'kind:feat',
    title: 'Web 预览：iframe 默认 + 可弹出独立窗口',
    deps: ['p4-view-target'],
    body: `## 验收标准

- [ ] 默认 iframe，不遮挡菜单
- [ ] \`X-Frame-Options\` 拒绝时提示并提供"弹出窗口"（独立系统 WebView）
- [ ] dev server 启动需审批，端口由任务分配
- [ ] 后台任务预览销毁，唤回 1s 内重建并恢复 URL
- [ ] 预览面板：URL、刷新、外开、缩放、清缓存

${DOD}`
  },
  {
    key: 'p4-changeset', m: 'P4', lane: 'lane:ui', kind: 'kind:feat',
    title: '变更清单与检查面板',
    deps: ['p4-task-entity'],
    body: `## 验收标准

- [ ] 逐文件 diff，支持应用 / 丢弃 / 打回重做
- [ ] 检查面板聚合测试结果，失败项可展开尾部输出
- [ ] 应用前主工作区必须 clean，否则提示 stash（不自动 stash）
- [ ] 基线漂移提示：\`基于 abc123，主分支已前进 4 个提交\`

${DOD}

参考：docs/TASKS.md §5`
  },
  {
    key: 'p4-merge', m: 'P4', lane: 'lane:agent', kind: 'kind:feat',
    title: '合并协议与冲突 handoff',
    deps: ['p4-changeset'],
    body: `## 验收标准

- [ ] 一次只应用一个，\`--no-ff\`
- [ ] 冲突转编辑器解决，解决后回到任务
- [ ] 不自动变基
- [ ] 合并成功后删除 worktree 与分支

${DOD}

参考：docs/TASKS.md §5`
  },
  {
    key: 'p4-queue', m: 'P4', lane: 'lane:agent', kind: 'kind:feat',
    title: '并发队列、端口分配与数据库写锁',
    deps: ['p4-worktree'],
    body: `## 验收标准

- [ ] 并发上限 2（可调，硬上限 4），超出进入 Queued 并明示原因
- [ ] 端口 4300–4399，任务结束释放
- [ ] 同一 \`connectionId\` 的写操作串行，无交叉事务
- [ ] 后台任务请求审批不抢焦点

${DOD}

参考：docs/TASKS.md §4`
  },

  // ---------------- P5 ----------------
  {
    key: 'p5-db-bridge', m: 'P5', lane: 'lane:db', kind: 'kind:feat',
    title: 'db-bridge sidecar',
    body: `## 范围

\`pg\` / \`mysql2\` / \`mssql\` / \`node:sqlite\`（低于 Node 22.5 降级 \`sql.js\`），esbuild 预打包为单文件，运行时不需 \`node_modules\`。

## 验收标准

- [ ] 四种数据库各跑通连接 / 查询 / 事务
- [ ] sidecar 崩溃不影响主进程，可重启
- [ ] 通过 stdin/stdout JSON 协议通信，有协议单测
- [ ] SSH 隧道复用现有 ssh 模块

${DOD}

参考：docs/ARCHITECTURE.md §5.5`
  },
  {
    key: 'p5-conn-manager', m: 'P5', lane: 'lane:db', kind: 'kind:feat',
    title: '连接管理与密钥存储',
    deps: ['p5-db-bridge'],
    body: `## 验收标准

- [ ] 项目级连接配置，密码进 \`SecretStore\`，不落明文、不进日志
- [ ] 连接级 \`read\` / \`write\` 开关，默认 read
- [ ] 切换为 write 时提示"agent 将可以在该库执行变更"
- [ ] 测试连接按钮返回可读错误

${DOD}`
  },
  {
    key: 'p5-db-panel', m: 'P5', lane: 'lane:ui', kind: 'kind:feat',
    title: '数据库面板：schema 树 / SQL 编辑器 / 结果网格',
    deps: ['p5-conn-manager', 'p4-view-target'],
    body: `## 验收标准

- [ ] schema 树按需展开，大库不一次全拉
- [ ] 结果网格虚拟化，10 万行滚动流畅
- [ ] 单元格编辑需主键；无主键表只读并注明原因
- [ ] 编辑生成 UPDATE 预览，确认后才执行
- [ ] agent 打开的视图带 LIMIT 角标

${DOD}`
  },
  {
    key: 'p5-db-tools', m: 'P5', lane: 'lane:db', kind: 'kind:feat',
    title: 'db_* 工具集与不可信结果标记',
    deps: ['p5-db-bridge', 'p3-tool-registry'],
    body: `## 验收标准

- [ ] \`db_list / db_schema / db_query / db_explain / db_execute / db_open\`
- [ ] 结果只回 20 行样本 + 句柄，不进上下文
- [ ] 结果与 schema 标 \`trust: untrusted\`，注入样本不改变工具行为
- [ ] schema 查询按会话缓存 TTL

${DOD}

参考：docs/AGENT.md §4`
  },
  {
    key: 'p5-sql-gate', m: 'P5', lane: 'lane:db', kind: 'kind:feat',
    title: 'SQL 闸门：解析分类 / 黑名单 / 限流',
    deps: ['p5-db-tools', 'p3-policy'],
    body: `## 验收标准

- [ ] \`sqlparser\` 分类，不用关键字匹配
- [ ] 多语句拒绝，除非走"允许脚本"二次确认
- [ ] 黑名单：DROP / TRUNCATE / ALTER DROP / 无 WHERE 写 / GRANT / SET ROLE，\`yolo\` 下仍拦
- [ ] 无 LIMIT 查询包 \`LIMIT 200\`；默认 8s 超时
- [ ] 只读连接在协议层拒绝写，有单测证明

${DOD}`
  },
  {
    key: 'p5-git', m: 'P5', lane: 'lane:fs', kind: 'kind:feat',
    title: 'Git 面板：status / diff / stage / commit / log',
    deps: ['p1-fs-walk'],
    body: `## 验收标准

- [ ] 调用系统 git，不捆绑
- [ ] diff 在 Worker 计算，不阻塞主线程
- [ ] stage / unstage / commit / 日志浏览
- [ ] 大幅 diff（> 5MB）降级为摘要

${DOD}`
  },
  {
    key: 'p5-ssh-term', m: 'P5', lane: 'lane:fs', kind: 'kind:feat',
    title: 'SSH 终端（portable-pty + xterm.js）',
    body: `## 验收标准

- [ ] 本地与 SSH 会话统一抽象
- [ ] 会话活在 Rust 侧，窗口销毁不中断
- [ ] 断开自动重连一次并保留滚动缓冲
- [ ] 退出应用时 pty 全部回收

${DOD}`
  },
  {
    key: 'p5-lsp', m: 'P5', lane: 'lane:editor', kind: 'kind:feat',
    title: 'TS/JS LSP（Monaco 内置 worker）',
    deps: ['p1-monaco'],
    body: `## 验收标准

- [ ] 补全、悬停、转到定义
- [ ] 只做 TS/JS，不捆绑 language server
- [ ] worker 崩溃可恢复，不白屏

${DOD}

参考：docs/PLAN.md R7`
  },

  // ---------------- P6 ----------------
  {
    key: 'p6-audit', m: 'P6', lane: 'lane:ci', kind: 'kind:infra',
    title: '第三方安全审计与高危整改',
    deps: ['p5-sql-gate', 'p4-merge'],
    body: `## 验收标准

- [ ] 审计覆盖：路径校验、子进程、SQL、MCP、更新器
- [ ] 高危清零，中危有处置计划
- [ ] 报告摘要进仓库

${DOD}

参考：docs/PLAN.md R3`
  },
  {
    key: 'p6-observability', m: 'P6', lane: 'lane:lifecycle', kind: 'kind:feat',
    title: '崩溃上报、更新器与回滚通道',
    deps: ['p1-window-lifecycle'],
    body: `## 验收标准

- [ ] 崩溃转储本地保存并可一键导出
- [ ] 更新器校验签名，失败自动回滚
- [ ] 30 天观测：无 P0/P1 缺陷，崩溃率 < 0.1% 会话

${DOD}`
  },
  {
    key: 'p6-signing', m: 'P6', lane: 'lane:ci', kind: 'kind:infra',
    title: '代码签名落地',
    deps: ['p0-signpath'],
    body: `## 验收标准

- [ ] SignPath 或 Azure Trusted Signing 接入 \`release.yml\`
- [ ] 签名失败时发布页不出现
- [ ] SmartScreen 声誉监测记录

${DOD}

参考：docs/CI.md §4`
  },
  {
    key: 'p6-linux-arm', m: 'P6', lane: 'lane:ci', kind: 'kind:infra',
    title: 'Linux arm64 正式包',
    deps: ['p1-linux-build'],
    body: `## 验收标准

- [ ] 原生 ARM runner 构建 AppImage
- [ ] 在 arm64 机器上冒烟通过

${DOD}`
  },
  {
    key: 'p6-i18n', m: 'P6', lane: 'lane:ui', kind: 'kind:infra',
    title: 'i18n 完整性检查进 CI',
    deps: ['p3-composer'],
    body: `## 验收标准

- [ ] 所有用户可见文案走 key，无硬编码
- [ ] \`zh-CN\` 与 \`en\` 键集合一致，缺键 CI 失败
- [ ] 不拼接句子片段

${DOD}

参考：docs/UI.md §4`
  },
  {
    key: 'p6-arkweb-spike', m: 'P6', lane: 'lane:harmony', kind: 'kind:feat',
    title: 'ArkWeb spike（2 周，只做可行性验证）',
    body: `## 目标

只验证三件事，不承诺交付。

## 验收标准

- [ ] ArkWeb 能加载现有前端产物
- [ ] NAPI 能承载 \`CoreHandle\` 的关键调用
- [ ] Monaco 在 ArkWeb 上的内存与输入法可用
- [ ] 三项结论写入 \`docs/ARCHITECTURE.md\`，无论成败

判定：全过 → 建独立外壳；只过前两项 → 交付"只读 + 对话"；不过 → 停止。

${DOD}

参考：docs/PLAN.md R9`
  },
  {
    key: 'p6-v1', m: 'P6', lane: 'lane:ci', kind: 'kind:feat',
    title: '发布 v1.0.0',
    deps: ['p6-audit', 'p6-observability', 'p6-signing', 'p6-linux-arm', 'p6-i18n'],
    body: `## 验收标准

- [ ] 维护者明确声明正式版
- [ ] 无 \`-preview\` 后缀，主版本非零
- [ ] 双平台签名产物 + SHA256SUMS + SBOM
- [ ] release notes 含平台支持矩阵与已知问题

${DOD}

参考：docs/VERSIONING.md §4`
  }
];
