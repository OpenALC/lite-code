# P1 性能测量结果（Windows）

- 日期：2026-11-05
- 代码：main@9d23c4a（合并进 perf/p1-measure），release 构建
- 构建：`pnpm --filter @lite-code/web build` + `cargo build --release --locked -p lc-desktop --features custom-protocol`
- 环境：Windows 10.0.26200 x64 / Rust 1.99.1 / pwsh 7.6.9
- 命令：`pwsh -NoProfile -File scripts/perf/run.ps1 -Out scripts/perf/out/metrics.json`
- 轮次：两轮完整运行（每轮 3 次冷启动、关闭后静置 10s、3 次热启动、20 次窗口开关）

## 结果

| 指标 | 第 1 轮 | 第 2 轮 | 目标（PLAN §4） | 结论 |
|---|---|---|---|---|
| 托盘常驻 RSS（窗口已销毁） | 29.6 MB | 29.5 MB | < 60 MB | PASS |
| 窗口活跃 RSS（3 文件 + 终端） | n/a | n/a | < 400 MB | 未测（无 UI 自动化入口） |
| 冷启动到主窗口可见 | 47 ms | 47 ms | < 3000 ms | PASS（近似口径） |
| 热启动（销毁后二次启动到可见） | 56 ms | 61 ms | < 1500 ms | PASS（近似口径） |
| 20 次开关后残留进程 | 0 | 0 | 0 | PASS |

托盘 RSS 拆解（关闭窗口、静置 10s 后）：`lc-desktop` 主进程 29.5–29.6 MB，WebView2 子进程 0 个（0 MB）。
R4 之前（关闭仅隐藏）同一 harness 测得 367.5–369.5 MB / 6 个 WebView2 子进程；改为 destroy 后回落到目标以内。

## 产物含前端的证据

- `target/release/lc-desktop.exe` 为 14,439,936 B；未启用 `custom-protocol` 的裸 `cargo build` 产物为 11,426,304 B。
- 二进制包含 `apps/web/dist` 的资源键：`index-Ch101u7y.js`、`vs-BBGKm3wj.js`、`editor.api-1FvVBcqP.js`。
- 运行期 WebView2 History 记录 `http://tauri.localhost/`（title `Lite Code`），测量期间未启动任何 dev server。

## 口径与限制

- `coldStartMs` / `warmStartMs` 的终点是主窗口可见，不是应用内"可交互"埋点；真实可交互耗时仍需应用侧启动打点。
- RSS 为进程树 `WorkingSet64` 求和（含共享页，属上限估计）。
- `activeRssMb` 仍为 null：缺少可脚本化的"打开 3 文件 + 终端"入口。
- `scripts/perf/baseline.json` 未改：按 docs/CI.md §3，更新基线需要 `perf-baseline` 标签 + 人工合并。在基线收录 `trayRssMb` / `warmStartMs` 之前 compare.ps1 会跳过这两项；`coldStartMs` 基线 79 ms 来自旧口径，本次 47 ms。
