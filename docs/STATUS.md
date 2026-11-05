# 工作进度快照（交接文档）

> 本文件是会话间的工作状态交接，由主 agent 维护；里程碑的权威定义仍在
> MILESTONES.md。快照时间：2026-11-05，HEAD：`5765db7`（main，CI 全绿）。

## 当前位置一览

| 事项 | 状态 |
|---|---|
| P0 地基代码 | ✅ 全量完成；本地 fmt/clippy/test 全绿；NSIS 本地打包成功（4.62 MiB） |
| P1 第一批三 lane（fs / editor / ci-perf） | ✅ 实现+人工审查+已提交（`2f46498`） |
| P1 第二批三 lane（process / runtime / bridge） | ✅ 实现完成、lane 门禁全过、已提交（`6aecc1b`）；**人工审查部分完成**（process supervisor 与 runtime portable 已读，质量高；剩余见「P1 第二批三 lane 推进报告」产物的 reviewPoints） |
| CI（GitHub Actions） | ✅ 全绿（run `41959914939`，HEAD `5765db7`） |
| 契约（lc-proto） | 冻结未动；桥接 lane 提出 3 条变更请求待用户决策 |

## 本轮 CI 修复（`5765db7`，已验证全绿）

1. `scripts/check-lanes.sh` 对齐 docs/CI.md §8：单跳引用按目标 lane 根的
   pub 面校验，`crate::<lane>::<子模块>::…` 一律拒绝；支持跨行 `use {...}`。
2. `runtime` Tier-4 测试夹具改用 `registry_dir()`，与 `Path::join` 同构，
   修复 ubuntu 上 `store_alias_only` / `registry_used_when_env_var_missing`。
3. `zip` 关闭默认特性只保留 `deflate`，移除 bzip2 许可证依赖；`deny.toml`
   显式忽略 4 条上游未维护、无升级路径的 RUSTSEC 通告。
4. watcher burst 测试回到生产 200ms 窗口，不再假设两次写入必然同批
   （并行负载下事件到达时间不可控）；改为校验两路径送达 + generation
   单调，并把路径排序去重抽成纯函数单测。

## 待办（按序）

1. 打 P0 收尾 tag：`git tag v0.0.1-preview.1 && git push origin v0.0.1-preview.1`
   → release.yml 自动出 draft（版本三处已对齐 `0.0.1-preview.1`；签名 secret 缺省自动跳过）
2. 完成第二批 lane 人工审查（重点：Job Object 回收测试的真实性、zip-slip 防护逐行、
   托盘/单实例对 nightly 无头环境的影响、bridge.ts 的 fallback 标注）
3. **契约变更请求 ×3（需用户决策，走独立 [contract] PR + bump）**：
   - `FsWriteFile { path, content }`（保存功能前置）
   - workspace 根查询命令（如 `workspace_root`/`config_get`，文件树需要根来源）
   - codegen 产物改为带 export 的声明（web 可直接 import bindings，删 contract.ts 跳板）
4. 用户事项：SignPath Foundation 申请（本机 cargo-deny 已安装）
5. P1 后续：editor 保存（等契约）、Runtime 设置页 UI、`LayoutProfile` 与 dockview、
   P1 出口标准的性能门槛实测（harness 已就绪：scripts/perf/）

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
- 工作流脚本样例（已验证可复用）：`.zcode/workflow-drafts/*.dwf.ts`
  （三 lane 并行 + 字面量门禁 + 自修复循环 + board/markdown 产物）；新会话使用前需重新加载
  dynamic-workflows skill
- pwsh 7.6.9 为 Store 版（WindowsApps），版本校验通过；rustup pinned 1.99.1 已装

## 测试基线（本机 Windows，最近一次全绿）

lc-core 66 · lc-cli 15 · lc-proto 8 · lc-desktop 8 · web 70（7 文件）· 合计 **167**。
