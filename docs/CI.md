# CI 与发布流程

Windows 一等公民，Linux 顺带，macOS 不构建。

## 1. 工作流总览

| 文件 | 触发 | 目标 | 阻断 |
|---|---|---|---|
| `.github/workflows/ci.yml` | PR、push `main` | 正确性 | 是 |
| `.github/workflows/webview-e2e.yml` | push `main`、手动 | webview 沙箱 Linux/WebKitGTK 运行时复验（xvfb + `--webview-selftest`） | 是 |
| `.github/workflows/perf.yml` | 手动、`perf` 标签、每日 03:00 UTC | 性能回归 | 是（超 15%） |
| `.github/workflows/release.yml` | tag `v*` | 打包与发布 | 是 |
| `.github/workflows/updater-manifest.yml` | release `published` | 把 `latest.json` 提升到滚动 release | 是 |
| `.github/workflows/nightly.yml` | 每日 02:00 UTC | 集成构建与冒烟 | 否（有告警） |

并发：同分支新提交取消上一次（`concurrency.cancel-in-progress: true`），`release.yml` 除外；
`updater-manifest.yml` 串行执行，避免后发布的版本被前一次覆盖。

## 2. `ci.yml`

### 前置 job `prep`

仓库处于"文档阶段"，还没有 `Cargo.toml` / `package.json`。直接写死会导致 PR 一进来就红。所以先用 `prep` 探测能力，用 job 级 `outputs` 传到下游：

| 输出 | 依据 | 影响 |
|---|---|---|
| `rust` | 存在 `Cargo.toml` | 跳过 Rust 全部 job |
| `web` | 存在 `package.json` | 跳过前端与 audit job |
| `lanes` | `scripts/check-lanes.sh` 可执行 | 决定是否跑 lane 检查 |
| `deny` | 存在 `deny.toml` | 决定是否跑 cargo-deny |

探测用 shell 而非 `hashFiles()`——后者在未 checkout 的 job 里不可用，这是最常见的坑。

### job 矩阵

| job | 运行器 | 内容 |
|---|---|---|
| `rust-fmt` | ubuntu-22.04 | `cargo fmt --check` |
| `rust-clippy` | windows-latest + ubuntu-22.04 | `clippy --all-targets --locked -- -D warnings` |
| `rust-test` | windows-latest + ubuntu-22.04 | `cargo test --workspace --locked` |
| `contract` | ubuntu-22.04 | 重新生成 TS 绑定，`git diff --exit-code` |
| `lanes` | ubuntu-22.04 | `scripts/check-lanes.sh` 越界 import 检查 |
| `web` | ubuntu-22.04 | `pnpm install --frozen-lockfile` / lint / typecheck / test / build |
| `db-integration` | ubuntu-22.04 | PG/MySQL 服务容器上跑 `cargo test --locked db_integration`（完整 sidecar 管道；缺服务自动跳过） |
| `audit` | ubuntu-22.04 | `pnpm audit --prod --audit-level high --registry=https://registry.npmjs.org`；发现 high/critical 漏洞或网络失败均 fail |
| `updater-manifest` | ubuntu-22.04 | `node --test scripts/gen-update-manifest.test.mjs` |
| `deny` | ubuntu-22.04 | `cargo deny check`（licenses / bans / advisories / sources） |

`summary` job 通过 `needs` 汇总以上全部 job；任一 failure/cancelled 即让 `ci` 失败。

**为什么 Ubuntu 用 22.04 而不是 latest**：glibc 版本决定产物的最低系统要求。用 latest 构建出的二进制在旧发行版上跑不起来，这个问题在用户上报之前很难发现。

**Windows 必须进矩阵**：路径分隔符、`\\?\` 长路径、文件名大小写、CRLF 都会在 Linux 上测不出来。

### 缓存

`Swatinem/rust-cache@v2`，`key` 包含 `rust-toolchain.toml` 的哈希；前端用 pnpm store（`pnpm/action-setup` 自带缓存）。禁止缓存 `target/` 跨 job 复用——命中率低且会掩盖增量构建问题。

### 一致性检查（重要）

`prep` 里断言三处版本一致，不一致即 fail：

```
rust-toolchain.toml 的 channel
Cargo.toml 的 rust-version
ci.yml env.RUST_TOOLCHAIN
```

这是 R8（MSRV 漂移）的第一道闸。这个检查要**能故意失败一次**再合入，否则无法确认它真的生效（P0 出口标准第 4 条）。

## 3. `webview-e2e.yml`

插件 webview iframe 沙箱的 Linux/WebKitGTK 运行时复验（docs/plans/plugin-webview.md §6
M3 双平台启用前提；Windows WebView2 侧证据已在 STATUS.md 记录）。

- 触发：push `main` / 手动。刻意独立于 `ci.yml`：有显示环境依赖、构建重，不拖慢主链路。
- 链路：`pnpm --filter @lite-code/web build` → `cargo build -p lc-desktop --features
  custom-protocol`（dev 等价物，`frontendDist` 嵌二进制，不打包安装包）→ demo 插件拷入
  `$HOME/.lite-code/extensions/` → `xvfb-run ./target/debug/lc-desktop --webview-selftest`。
- 判定：自测模式内 Rust 注入幂等脚本走真实 UI（活动栏「插件」→ webview「打开视图」→
  iframe 沙箱），断言 demo 插件经握手 + MessageChannel 桥写入的 greet 值在 Rust 状态
  存储可读回；`WEBVIEW_SELFTEST: PASS` 标记行 + 退出码 0 双断言，60s 硬超时强退 1。
- WebKitGTK 无 GPU 环境变量：`WEBKIT_DISABLE_COMPOSITING_MODE=1` /
  `WEBKIT_DISABLE_DMABUF_RENDERER=1`；失败上传 `webview-selftest.log` artifact。

## 4. `perf.yml`

只在 Windows 上跑（一等公民），四个指标与 `PLAN.md` §4 一致：

| 指标 | 目标 | 采集 |
|---|---|---|
| 托盘常驻 RSS | < 60 MB | 窗口销毁后 10s 采样 |
| 窗口活跃 RSS | < 400 MB | 3 文件 + 终端场景脚本 |
| 冷启动到可交互 | < 3s 首次 / < 1.5s 热 | 应用内打点，输出到 stdout |
| 20 次开关后的残留进程 | 0 | 进程快照 diff |

- 基线存为 artifact（`perf-baseline.json`），PR 与基线比对，**回归 > 15% fail**。
- 更新基线需要 `perf-baseline` 标签 + 人工合并，不允许 PR 自动改基线——否则"改基线让 CI 绿"会成为默认操作。
- `perf.yml` 失败不阻塞合并（`ci.yml` 才阻塞），但会挂 PR 评论 + `performance` 标签。P2 起改为阻塞。

## 5. `release.yml`

```
tag v* ──▶ verify（复用 ci 的 fmt/clippy/test/audit/manifest 测试）
       ──▶ build-windows     (x86_64-pc-windows-msvc)
       ──▶ build-linux       (x86_64-unknown-linux-gnu, ubuntu-22.04)
       ──▶ build-linux-arm64 (aarch64-unknown-linux-gnu, ubuntu-24.04-arm)
       ──▶ sbom              (cargo-cyclonedx + pnpm licenses)
       ──▶ sign              (SIGNPATH_ENABLED=true 时 fail closed)
       ──▶ publish           (draft release + latest.json + SHA256SUMS)

release published ──▶ updater-manifest.yml（把 latest.json 提升到滚动 release）
```

Linux arm64 使用 GitHub 托管的 `ubuntu-24.04-arm` 原生构建（公共仓库免费；私有仓库走付费额度），
不交叉编译。该 job 是 P6 A 类门禁：`publish` 要求它 success，且 AppImage / deb / CLI 缺一即失败，
不会发出少资产的 release。发布资产里 arm64 CLI 命名为 `lc-cli-arm64`，避免与 x64 的 `lc-cli` 重名。

规则：

1. **只发 draft**，人工点发布。签名失败时发布页不出现，而不是发一个未签名的包再撤回。
2. 产物附 `SHA256SUMS`，README 与 release notes 都指向它；上传前会逐个重算哈希，
   确认 `SHA256SUMS` 覆盖全部发布资产（win / linux x64 / linux arm64 / `latest.json` / sbom）。
3. `--locked` 贯穿所有构建；`Cargo.lock` 与 `pnpm-lock.yaml` 必须进仓。
4. 版本号单一来源：tag。规则与校验见 [VERSIONING.md](VERSIONING.md) §6：`0.x` 必须带 `-preview.N`，正式版必须是非零主版本，违规 tag 在 `prep` 阶段直接失败。三处版本（`Cargo.toml` / `package.json` / `tauri.conf.json`）与 tag 不一致也会失败。
5. `--prerelease` 由 tag 后缀自动决定，不靠人勾选。
6. 发布说明取 `docs/releases/<version>.md`，缺失时回退到 `release-notes-template.md` 并告警。
7. 发布说明必须包含**已知问题**，数量为 0 时也要写明"本版本无已知高风险问题"。
8. `publish` 同时生成更新清单 `latest.json` 并挂到 draft；**draft 未发布时不得更新
   滚动 `updater-manifest`**，避免清单 URL 指向尚不可访问的资产。提升动作由
   `updater-manifest.yml` 在 `release: published` 时完成。

### 权限与供应链

- workflow 级 `permissions: contents: read`；只有 `publish` job 单独声明 `contents: write`。
- `release.yml` 的所有 checkout 使用 `persist-credentials: false`，构建 job 不把
  `GITHUB_TOKEN` 留在 `.git/config` 里；发布使用显式 `GH_TOKEN`。
- Actions 目前按主版本标签引用（`@v4` 等），尚未固定到 commit SHA；这是后续供应链
  收紧项，不是当前发布门禁。

### 签名（fail closed）

| 变量 / secret | 用途 | 缺失或未接入时 |
|---|---|---|
| `SIGNPATH_ENABLED`（仓库变量） | 是否要求 Windows Authenticode 签名 | `false`：`sign` job 跳过，发布未签名包并在日志告警；`true`：`sign` job **故意失败**（真实 SignPath 流程未接入），从而阻止 `publish` |
| `SIGNPATH_API_TOKEN` | SignPath 凭据 | 真实签名接入前仅检查是否存在，占位流程不再“打印后成功” |
| `TAURI_SIGNING_PRIVATE_KEY` / `..._PASSWORD` | 更新清单 `.sig`（Tauri minisign） | 仍生成 `latest.json` 并写 `signed:false`；日志与 step summary 明确提示，不伪造签名 |
| `AZURE_TENANT_ID` / `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET` | Azure Trusted Signing（备选） | 未接入，同上 |

**签名 secret 只在 tag 构建的 job 中可见**，PR 拿不到，避免 fork 通过 PR 摸到发布凭据。
`SIGNPATH_ENABLED=true` 而真实签名未接入时，工作流宁可失败也不会发布未签名安装包。

### 更新器清单 `latest.json`

1. `publish` 在生成 `SHA256SUMS` 前运行 `node scripts/gen-update-manifest.mjs`，
   参数见 [UPDATER.md](UPDATER.md) §4；上一版清单来自滚动 release
   `updater-manifest`（不存在则首次发布，无 `previous`）。
2. 生成器从安装包旁的 `.sig` 读取签名；没有 `.sig` 时写 `signed:false`、`signature:""`。
3. `latest.json` 与 `SHA256SUMS` 一起进入 draft，并被 `SHA256SUMS` 覆盖。
4. 维护者发布 draft 后，`updater-manifest.yml` 从该 tag 下载 `latest.json`，校验
   `version`/`platforms`/`signed`，再创建或覆盖滚动 release `updater-manifest`。
5. 构建侧尚未把 `TAURI_SIGNING_PRIVATE_KEY` 注入 `pnpm tauri build`；在接入
   `bundle.createUpdaterArtifacts` 或 `tauri signer sign` 之前，清单会保持 `signed:false`。

### 下一次 tag 发布检查清单

- [ ] `Cargo.toml` / `package.json` / `tauri.conf.json` 与 tag 完全一致，`docs/releases/<version>.md` 已提交。
- [ ] `SIGNPATH_ENABLED` 与真实签名状态一致：未接入真实签名时必须为 `false`。
- [ ] 如需 `signed:true`，确认 `TAURI_SIGNING_PRIVATE_KEY` 已配置，且构建产物旁确实有 `.sig`。
- [ ] `updater-manifest.yml` 已在默认分支（`release` 事件只使用默认分支上的工作流文件）。
- [ ] draft 资产包含 Windows NSIS、linux-x64 AppImage/deb、linux-arm64 AppImage/deb、
      三个 CLI、`SHA256SUMS`、`latest.json`。
- [ ] 人工发布后再确认 `updater-manifest` release 的 `latest.json` 版本等于新 tag，
      且 `previous` 指向上一版。

## 6. `nightly.yml`

- 每天 02:00 UTC 全量构建 + 冒烟（启动、打开仓库、编辑保存、Agent 一次 headless 任务）。
- 产物保留 7 天，不发布。
- 失败开 issue（tag `nightly`），同一失败只开一次。

## 7. 分支与合并

- trunk-based，`main` 受保护：必须 `ci.yml` 绿 + 1 个 review。
- 不要求线性历史，但禁止 merge commit 之外的直接 push。
- lane 分支命名 `lane/<name>-<topic>`，合入前必须过 `lanes` 检查。
- 契约变更单独 PR，标题前缀 `[contract]`，需要 Contracts 所有者批准。

## 8. Secrets 清单

| 名称 | 作用域 | 说明 |
|---|---|---|
| `SIGNPATH_ENABLED`（仓库变量） | release | `true` 要求真实 Windows 签名；未接入时 sign job 直接失败 |
| `SIGNPATH_API_TOKEN` | release | SignPath 凭据（真实流程接入前只做存在性检查） |
| `AZURE_*` | release | 备选签名方案 |
| `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)` | release | 更新清单 `.sig`；缺失时 `latest.json` 写 `signed:false` |
| `LLM_TEST_API_KEY` | perf / nightly | headless agent 冒烟，用独立额度 |
| — | — | PR 环境**不需要**任何 secret；缺 secret 的 job 自动跳过 |

## 9. 本地复现

```bash
# 与 ci.yml 等价
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test
pnpm audit --prod --audit-level high --registry=https://registry.npmjs.org
node --test scripts/gen-update-manifest.test.mjs
scripts/check-lanes.sh
cargo deny check
node scripts/check-version.mjs
```

`scripts/check-lanes.sh` 规则：`crates/lc-core/<lane>` 之间不允许互相引用内部模块，只允许引用 `lc_proto` 与同 crate 的 `pub` 接口。实现用 `cargo metadata` + grep，不引入额外工具。

## 10. 明确不做

- **不建 macOS 矩阵**，即使 runner 免费。
- 不做 Windows 的 `aarch64` 构建。
- 不在 PR 上跑完整打包（耗时且浪费额度）；打包只在 tag 与 nightly。
- 不给 PR 注入签名凭据。
- 不缓存 `target/` 跨 runner 传递。
