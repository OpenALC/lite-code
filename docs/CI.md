# CI 与发布流程

Windows 一等公民，Linux 顺带，macOS 不构建。

## 1. 工作流总览

| 文件 | 触发 | 目标 | 阻断 |
|---|---|---|---|
| `.github/workflows/ci.yml` | PR、push `main` | 正确性 | 是 |
| `.github/workflows/perf.yml` | 手动、`perf` 标签、每日 03:00 UTC | 性能回归 | 是（超 15%） |
| `.github/workflows/release.yml` | tag `v*` | 打包与发布 | 是 |
| `.github/workflows/nightly.yml` | 每日 02:00 UTC | 集成构建与冒烟 | 否（有告警） |

并发：同分支新提交取消上一次（`concurrency.cancel-in-progress: true`），`release.yml` 除外。

## 2. `ci.yml`

### 前置 job `prep`

仓库处于"文档阶段"，还没有 `Cargo.toml` / `package.json`。直接写死会导致 PR 一进来就红。所以先用 `prep` 探测能力，用 job 级 `outputs` 传到下游：

| 输出 | 依据 | 影响 |
|---|---|---|
| `rust` | 存在 `Cargo.toml` | 跳过 Rust 全部 job |
| `web` | 存在 `package.json` | 跳过前端 job |
| `toolchain` | 读 `rust-toolchain.toml` 的 `channel` | 与锁定版本比对，不一致直接 fail |
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
| `deny` | ubuntu-22.04 | `cargo deny check`（licenses / bans / advisories / sources） |
| `audit` | ubuntu-22.04 | `pnpm audit --audit-level high`（仅 PR，`continue-on-error: false`） |

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

## 3. `perf.yml`

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

## 4. `release.yml`

```
tag v* ──▶ verify（复用 ci 的 fmt/clippy/test）
       ──▶ build-windows     (x86_64-pc-windows-msvc)
       ──▶ build-linux       (x86_64-unknown-linux-gnu, ubuntu-22.04)
       ──▶ build-linux-arm64 (aarch64-unknown-linux-gnu, ubuntu-24.04-arm)
       ──▶ sbom              (cargo-cyclonedx + pnpm licenses)
       ──▶ sign              (条件执行，见下)
       ──▶ publish           (draft release + SHA256SUMS + 已知问题)
```

Linux arm64 使用 GitHub 托管的 `ubuntu-24.04-arm` 原生构建（公共仓库免费；私有仓库走付费额度），
不交叉编译。该 job 是 P6 A 类门禁：`publish` 要求它 success，且 AppImage / deb / CLI 缺一即失败，
不会发出少资产的 release。发布资产里 arm64 CLI 命名为 `lc-cli-arm64`，避免与 x64 的 `lc-cli` 重名。

规则：

1. **只发 draft**，人工点发布。签名失败时发布页不出现，而不是发一个未签名的包再撤回。
2. 产物附 `SHA256SUMS`，README 与 release notes 都指向它。
3. `--locked` 贯穿所有构建；`Cargo.lock` 与 `pnpm-lock.yaml` 必须进仓。
4. 版本号单一来源：tag。规则与校验见 [VERSIONING.md](VERSIONING.md) §6：`0.x` 必须带 `-preview.N`，正式版必须是非零主版本，违规 tag 在 `prep` 阶段直接失败。三处版本（`Cargo.toml` / `package.json` / `tauri.conf.json`）与 tag 不一致也会失败。
5. `--prerelease` 由 tag 后缀自动决定，不靠人勾选。
6. 发布说明取 `docs/releases/<version>.md`，缺失时回退到 `release-notes-template.md` 并告警。
7. 发布说明必须包含**已知问题**，数量为 0 时也要写明"本版本无已知高风险问题"。

### 签名（条件执行）

| secret | 用途 | 缺失时 |
|---|---|---|
| `SIGNPATH_API_TOKEN` | SignPath 签名 | 跳过，产物标 `unsigned` |
| `AZURE_TENANT_ID` / `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET` | Azure Trusted Signing | 同上 |
| `TAURI_SIGNING_PRIVATE_KEY` / `..._PASSWORD` | 自动更新签名 | 跳过，不生成更新清单 |

用 `if: ${{ secrets.X != '' }}` 判断。**签名 secret 只在 tag 构建的 job 中可见**，PR 拿不到，避免 fork 通过 PR 摸到发布凭据。

## 5. `nightly.yml`

- 每天 02:00 UTC 全量构建 + 冒烟（启动、打开仓库、编辑保存、Agent 一次 headless 任务）。
- 产物保留 7 天，不发布。
- 失败开 issue（tag `nightly`），同一失败只开一次。

## 6. 分支与合并

- trunk-based，`main` 受保护：必须 `ci.yml` 绿 + 1 个 review。
- 不要求线性历史，但禁止 merge commit 之外的直接 push。
- lane 分支命名 `lane/<name>-<topic>`，合入前必须过 `lanes` 检查。
- 契约变更单独 PR，标题前缀 `[contract]`，需要 Contracts 所有者批准。

## 7. Secrets 清单

| 名称 | 作用域 | 说明 |
|---|---|---|
| `SIGNPATH_API_TOKEN` | release | Windows 签名 |
| `AZURE_*` | release | 备选签名方案 |
| `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)` | release | 更新包签名 |
| `LLM_TEST_API_KEY` | perf / nightly | headless agent 冒烟，用独立额度 |
| — | — | PR 环境**不需要**任何 secret；缺 secret 的 job 自动跳过 |

## 8. 本地复现

```bash
# 与 ci.yml 等价
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test
scripts/check-lanes.sh
cargo deny check
```

`scripts/check-lanes.sh` 规则：`crates/lc-core/<lane>` 之间不允许互相引用内部模块，只允许引用 `lc_proto` 与同 crate 的 `pub` 接口。实现用 `cargo metadata` + grep，不引入额外工具。

## 9. 明确不做

- **不建 macOS 矩阵**，即使 runner 免费。
- 不做 Windows 的 `aarch64` 构建。
- 不在 PR 上跑完整打包（耗时且浪费额度）；打包只在 tag 与 nightly。
- 不给 PR 注入签名凭据。
- 不缓存 `target/` 跨 runner 传递。
