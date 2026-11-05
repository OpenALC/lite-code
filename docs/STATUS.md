# 工作进度快照（交接文档）

> 本文件是会话间的工作状态交接，由主 agent 维护；里程碑的权威定义仍在
> MILESTONES.md。快照时间：2026-11-05，main HEAD：`6566ffb`。
> P0 发布线单独在 `release/p0-preview2`（tag `v0.0.1-preview.2`）。

## 当前位置一览

| 事项 | 状态 |
|---|---|
| P0 地基代码 | ✅ 全量完成 |
| P0 发布 | ⚠️ `preview.1` 在 publish 失败（Windows 产物路径错误）；`preview.2` 已修复工作流并重打，Windows/Linux 构建中 |
| P1 第一批三 lane（fs / editor / ci-perf） | ✅ 实现+审查+已提交 |
| P1 第二批三 lane（process / runtime / bridge） | ✅ 实现+人工审查完成；Windows 单子进程 kill 的孙进程限制已记录 |
| 契约（lc-proto） | ✅ v0.1：`workspace_root`、`fs_write_file`、export TS 绑定 |
| 编辑器保存 | ✅ 真实文件打开、编辑、Ctrl+S 保存、dirty 复位、fallback 提示 |
| Runtime 设置页 | ✅ Ready/NeedManual、候选失败原因、手动路径、便携版下载 |
| main CI | ✅ `8e2bf00` 全绿；`6566ffb` 的 CI 进行中 |

## 本轮完成

1. **契约 v0.1**（`7b0ce1e`）
   - `workspace_root`：根由 Rust 传输层注入，fs 命令不再信任前端 root。
   - `fs_write_file`：根内写路径校验（新建校验父目录、软链逃逸拒绝）。
   - codegen 产物改为 `export type`，web 通过 `@lc-proto` 直接 import，删除 `contract.ts`。
2. **编辑器保存**（`0243d6d`）
   - 标签 store 持有 `path/content/dirty`；文件树读取成功后注入内容。
   - CodeEditor 跟随激活标签切换内容与语言；Ctrl+S 调 `fs_write_file`。
   - 保存期间继续输入时保留 dirty；浏览器 dev 返回 fallback 提示。
3. **发布工作流修复**（`fcad1a0` + `release/p0-preview2`）
   - Windows 上传路径改为 `target/x86_64-pc-windows-msvc/release/bundle/**`。
   - 两个平台显式构建并上传 `lc-cli`。
   - 构建失败后的重打按 VERSIONING §3 使用 `-preview.2`。
4. **Runtime 设置页**（`6566ffb`）
   - Tauri 新增 `node_set_manual` / `node_download_portable`。
   - 顶栏入口 → 面板展示状态、候选失败原因、手动校验、便携版下载。
   - 浏览器 dev 的 fallback 不会伪装成保存/安装成功。

## 待办（按序）

1. 确认 `v0.0.1-preview.2` release draft 产出（Windows NSIS + Linux AppImage/deb + SHA256SUMS）。
2. `LayoutProfile` 完整实现与 dockview 可停靠布局。
3. P1 性能四项门槛实测（scripts/perf/）：冷启动/托盘 RSS/窗口 RSS/残留进程。
4. P1 出口验收：干净 Win11 无 Node、20 次开关窗口、1 万文件目录。
5. P2：五 lane 合流冒烟，发布 `v0.1.0-pre`（Windows NSIS + Linux AppImage）。

## 本地验证命令（与 CI 等价）

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm.cmd lint && pnpm.cmd typecheck && pnpm.cmd test -- --run && pnpm.cmd build   # 仓库根
C:/Git/usr/bin/bash.exe -c "export PATH=/usr/bin:$PATH; exec scripts/check-lanes.sh"
cargo deny --locked check
```

## 环境备忘（本机特有，避免重新踩坑）

- `world.run`/spawn 下 pnpm 必须写 **`pnpm.cmd`**（无扩展名 shim 是 ENOENT）
- `bash` 解析到 WSL；Git Bash 在 `C:\Git\usr\bin\bash.exe`，需
  `-c "export PATH=/usr/bin:$PATH; exec <script>"`
- 本机 cargo-deny 0.20.2 的参数顺序：`cargo deny --locked check`
  （`check --locked` 会报 unexpected argument）
- cargo-deny-action 要显式 `rust-version`（仓库 rust-toolchain.toml 会 override 其 musl 工具链）
- `gh run watch --exit-status` 的退出码别被后续 `echo`/管道吞掉
- 工作流（dynamic-workflows）：门禁命令必须编译期字面量；`git add -A` 会卷入
  subagent 进行中的文件——提交一律用精确路径
- pwsh 7.6.9 为 Store 版（WindowsApps），版本校验通过；rustup pinned 1.99.1 已装

## 测试基线（本机 Windows，最近一次全绿）

lc-core 70 · lc-cli 15 · lc-proto 9 · lc-desktop 11 · web 84（8 文件）· 合计 **189**。
