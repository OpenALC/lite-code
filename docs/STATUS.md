# 工作进度快照（交接文档）

> 本文件是会话间的工作状态交接，由主 agent 维护；里程碑的权威定义仍在
> MILESTONES.md。快照时间：2026-11-05，main HEAD：`fcad1a0`。
> P0 发布线单独在 `release/p0-preview2`（tag `v0.0.1-preview.2`）。

## 当前位置一览

| 事项 | 状态 |
|---|---|
| P0 地基代码 | ✅ 全量完成 |
| P0 发布 | ⚠️ `v0.0.1-preview.1` 在 publish 阶段失败：Windows 产物实际在 `target/<triple>/release`，工作流上传了 `target/release`。已在 `release/p0-preview2` 修复并重打 `v0.0.1-preview.2`（后台构建中） |
| P1 第一批三 lane（fs / editor / ci-perf） | ✅ 实现+审查+已提交 |
| P1 第二批三 lane（process / runtime / bridge） | ✅ 实现完成；人工审查完成，未发现高危问题。已知限制：Windows 单子进程 `kill` 只杀直接子进程，孙进程回收依赖 `kill_all` / supervisor drop |
| 契约（lc-proto） | ✅ v0.1 已落地：`workspace_root`、`fs_write_file`、带 export 的 TS 绑定 |
| 编辑器保存 | ✅ 真实文件打开、编辑、Ctrl+S 保存、dirty 复位、浏览器 fallback 提示 |
| main CI | ✅ 契约 v0.1 run 全绿；编辑器保存 run 进行中 |

## 本轮完成

1. **契约 v0.1**（`7b0ce1e`）
   - `workspace_root`：根由 Rust 传输层注入，fs 命令不再信任前端 root。
   - `fs_write_file`：core 根内写路径校验（新建文件校验父目录、软链逃逸拒绝）。
   - codegen 产物改为 `export type`，web 通过 `@lc-proto` 直接 import，删除 `contract.ts`。
2. **编辑器保存**（`0243d6d`）
   - 标签 store 持有 `path/content/dirty`；文件树读取成功后注入内容。
   - CodeEditor 跟随激活标签切换内容与语言；Ctrl+S 调 `fs_write_file`。
   - 保存期间继续输入时保留 dirty；浏览器 dev 返回 fallback 提示。
   - 新增 `editorLanguage` 纯函数与测试。
3. **发布工作流修复**（`fcad1a0`，同步自 `release/p0-preview2`）
   - Windows 上传路径改为 `target/x86_64-pc-windows-msvc/release/bundle/**`。
   - 两个平台都显式构建并上传 `lc-cli`。
   - 按 VERSIONING §3，构建失败后的重打使用 `-preview.2`。

## 待办（按序）

1. 确认 `v0.0.1-preview.2` release draft 产出（Windows NSIS + Linux AppImage/deb + SHA256SUMS）。
2. Runtime 设置页 UI（Node 状态、手动指定、便携版下载、降级提示）。
3. `LayoutProfile` 完整实现与 dockview 可停靠布局。
4. P1 性能四项门槛实测（scripts/perf/），冷启动/托盘 RSS/窗口 RSS/残留进程。
5. P1 出口验收：干净 Win11 无 Node 环境、20 次开关窗口、1 万文件目录。
6. P2：五 lane 合流冒烟，发布 `v0.1.0-pre`（Windows NSIS + Linux AppImage）。

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

lc-core 70 · lc-cli 15 · lc-proto 9 · lc-desktop 9 · web 81（8 文件）· 合计 **184**。
