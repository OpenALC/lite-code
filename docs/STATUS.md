# 工作进度快照（交接文档）

> 本文件是会话间的工作状态交接，由主 agent 维护；里程碑的权威定义仍在
> MILESTONES.md。快照时间：2026-11-05，main HEAD：`4f4b1e9`。
> P0 发布线：tag `v0.0.1-preview.2` + draft release 已产出。

## 当前位置一览

| 事项 | 状态 |
|---|---|
| P0 地基代码 | ✅ 全量完成 |
| P0 发布 | ✅ `v0.0.1-preview.2` draft：Windows NSIS、Linux AppImage/deb、两个 `lc-cli`、SHA256SUMS |
| P1 第一批三 lane（fs / editor / ci-perf） | ✅ 实现+审查+已提交 |
| P1 第二批三 lane（process / runtime / bridge） | ✅ 实现+审查完成 |
| 契约（lc-proto） | ✅ v0.1：`workspace_root`、`fs_write_file`、export TS 绑定 |
| 编辑器保存 | ✅ 真实文件打开、编辑、窗口级 Ctrl+S、dirty、fallback 提示 |
| 大文件分级 | ✅ >5MB 降级能力、>20MB 只读预览并阻止保存 |
| Runtime 设置页 | ✅ Ready/NeedManual、候选失败原因、手动路径、便携版下载 |
| main CI | ✅ `fff0172` 全绿；最新提交 CI 进行中 |

## 本轮完成

1. **契约 v0.1**（`7b0ce1e`）
   - `workspace_root` 由 Rust 注入，fs 命令不再信任前端 root。
   - `fs_write_file` 允许新建，校验父目录/符号链接根内；codegen 改为 `export type`。
2. **编辑器保存**（`0243d6d`）
   - 标签 store 持有 `path/content/dirty`；CodeEditor 跟随激活标签；窗口级 Ctrl+S。
3. **Runtime 设置页**（`6566ffb`）
   - Tauri `node_set_manual` / `node_download_portable`；设置面板与 bridge 包装。
4. **安全与形状修复**（`1e0f742`）
   - 保存改为临时文件 + fsync + rename，保留原权限；拒绝悬空软链写穿。
   - Tauri `node_detect` / `shell_detect` / `contract_info` 统一返回 `CommandResult`，修复 bridge 形状错配。
   - 浏览器 fallback 不清 dirty；release publish 逐文件上传并允许 sbom 缺失。
5. **P0 release**
   - `preview.2` 构建成功但 workflow publish 因 sbom 缺失失败；已用现有产物手动创建 draft，并修正 main 的 publish 逻辑供后续版本使用。
6. **大文件分级接线**（`4f4b1e9`）
   - 标签携带 `EditorFileStrategy`；>5MB 关闭 occurrences/折叠/minimap，>20MB 只读并阻止 Ctrl+S 写盘。

## 已知 P1 缺口（审查结果，按优先级）

1. 切标签用单 Monaco model，undo/光标不按标签保留（下一步改 per-tab model）。
2. 无保存冲突检测（需要契约加 mtime/hash 或前端 savedContent 基线）。
3. 托盘“退出”绕过 webview 脏检查，可能丢未保存缓冲。
4. workspace 根仍取 CWD/环境变量，尚无“打开文件夹”流程；已校验必须是目录。
5. `@lc-proto` 目前只有 tsconfig 别名，未来若生成运行时常量需补 Vite alias。

## 待办（按序）

1. 编辑器 per-tab Monaco model（undo/光标/滚动保留）。
2. `LayoutProfile` 完整实现与 dockview 可停靠布局。
3. P1 性能四项实测（scripts/perf/）：冷启动/托盘 RSS/窗口 RSS/残留进程。
4. P1 出口验收：干净 Win11 无 Node、20 次开关窗口、1 万文件目录。
5. P2：五 lane 合流冒烟，发布 `v0.1.0-pre`。

## 本地验证命令（与 CI 等价）

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm.cmd lint && pnpm.cmd typecheck && pnpm.cmd test -- --run && pnpm.cmd build
C:/Git/usr/bin/bash.exe -c "export PATH=/usr/bin:$PATH; exec scripts/check-lanes.sh"
cargo deny --locked check
```

## 环境备忘（本机特有，避免重新踩坑）

- `world.run`/spawn 下 pnpm 必须写 **`pnpm.cmd`**（无扩展名 shim 是 ENOENT）
- `bash` 解析到 WSL；Git Bash 在 `C:\Git\usr\bin\bash.exe`，需
  `-c "export PATH=/usr/bin:$PATH; exec <script>"`
- 本机 cargo-deny 0.20.2 的参数顺序：`cargo deny --locked check`
- cargo-deny-action 要显式 `rust-version`（仓库 rust-toolchain.toml 会 override 其 musl 工具链）
- `gh run watch --exit-status` 的退出码别被后续 `echo`/管道吞掉
- 工作流（dynamic-workflows）：门禁命令必须编译期字面量；`git add -A` 会卷入
  subagent 进行中的文件——提交一律用精确路径
- pwsh 7.6.9 为 Store 版（WindowsApps），版本校验通过；rustup pinned 1.99.1 已装

## 测试基线（本机 Windows，最近一次全绿）

lc-core 70 · lc-cli 15 · lc-proto 9 · lc-desktop 11 · web 85（8 文件）· 合计 **190**。
Linux 额外跑 `write_file_rejects_dangling_symlink_escape` 与权限保留测试。
