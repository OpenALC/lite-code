# 工作进度快照（交接文档）

> 本文件是会话间的工作状态交接，由主 agent 维护；里程碑的权威定义仍在
> MILESTONES.md。快照时间：2026-11-05，HEAD：`6aecc1b`（main，已推送）。

## 当前位置一览

| 事项 | 状态 |
|---|---|
| P0 地基代码 | ✅ 全量完成；本地 fmt/clippy/test 全绿；NSIS 本地打包成功（4.62 MiB） |
| P1 第一批三 lane（fs / editor / ci-perf） | ✅ 实现+人工审查+已提交（`2f46498`） |
| P1 第二批三 lane（process / runtime / bridge） | ✅ 实现完成、lane 门禁全过、已随 `6aecc1b` 提交；**人工审查部分完成**（process supervisor 与 runtime portable 已读，质量高；剩余见「P1 第二批三 lane 推进报告」产物的 reviewPoints） |
| CI（GitHub Actions） | ❌ HEAD 红两处，根因已定位、修复配方见下 |
| 契约（lc-proto） | 冻结未动；桥接 lane 提出 3 条变更请求待用户决策 |

## CI 红因与修复配方（HEAD `6aecc1b`，按序执行）

### 1. `test (ubuntu-22.04)`：2 个测试失败（run 41937372562）

根因：`crates/lc-core/src/runtime/mod.rs` Tier-4 用 `Path::new(dir).join("node.exe")`
构造候选——unix 上 join 插 `/`，而测试夹具用 `\` 存路径，字符串不等 → 候选没枚举到。
真实实现无问题（unix 无注册表），是**夹具平台不纯粹**。受影响的只有依赖 Tier-4 的两个测试
（`tier1_beats_registry`/`env_var_beats_stale_registry` 因 Tier1/2 先命中而侥幸通过）。

修复：在 `crates/lc-core/src/runtime/tests.rs` 加夹具助手并改写两个测试：

```rust
/// 注册表 PATH 目录的跨平台夹具：与 enumerate 的 Tier 4 构造同构
/// （`Path::join` 在 unix 插 `/`，夹具必须走同一条构造路径）。
fn registry_dir(path: &str) -> String {
    if cfg!(windows) { path.to_string() } else { path.replace('\\', "/") }
}

#[test]
fn store_alias_only() {
    let dir = registry_dir(r"C:\Users\t\AppData\Local\Microsoft\WindowsApps");
    let exe = Path::new(&dir).join("node.exe");
    let env = FakeEnv::new(&[]).with_registry(&[dir.as_str()]);
    let lister = FakeLister::new(&[exe.to_str().unwrap()]);
    let runner = FakeRunner::new(&[]);
    let attempts = manual(resolve(&env, &lister, &runner));
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].reason, NodeFailureReason::StoreAlias);
    assert_eq!(attempts[0].source, NodeSource::RegistryPath);
}

// registry_used_when_env_var_missing 同理：
//   dir = registry_dir(r"D:\nvm-portable")；exe = Path::new(&dir).join("node.exe")
//   FakeLister::new(&[exe.to_str().unwrap()])、FakeRunner::new(&[(exe.to_str().unwrap(), "v18.19.0")])
//   断言 source=RegistryPath、health=Unverified 不变。
```

### 2. `lane boundaries`：check-lanes.sh 比文档严（两次工作流升级均因此）

根因：脚本禁止一切跨 lane `crate::<lane>::` 引用，但 docs/CI.md §8 允许引用
"**同 crate 的 pub 接口**"（Rust 可见性本身只放行 pub 项）。runtime→config 的
`use crate::config::{ConfigError, ConfigStore, ...}` 是合法 pub 引用（config/mod.rs
根级全 pub：ConfigError/NodeRuntime/AppConfig/config_path/data_dir 等）。

修复：重写 `scripts/check-lanes.sh` 违规判定为两条（对齐 §8）：

- 深路径（`crate::<lane>::<子模块>::`）一律越界——pub 面只允许单跳；
- 单跳引用（`crate::<lane>::<Item>`，含 `use crate::lane::{A, B}` 花括号展开）：
  Item 必须出现在目标 lane 根（`$CORE_SRC/<lane>/mod.rs`）的 pub 面中
  （提取规则：`pub fn|struct|enum|trait|mod|type|const|static X` 的 X +
  `pub use` 行去括号逗号后的末段标识符）；
- 保留原有：lane 内部引用自己 ✓、lc_proto 引用 ✓、模块整体导入
  （`use crate::config;` 后经 `config::X` 使用）grep 不可见，属已知盲区（编译器兜底）。

### 3. `cargo-deny`：已在 `6aecc1b` 修复，待 CI 验证

CC0-1.0 放行 + 内部依赖补 `version`（消通配）+ action 显式 `rust-version: "1.99.1"`。

## 待办（按序，前两步完成后 CI 应全绿）

1. 应用上面修复 1+2 → 本地验证（见下命令）→ commit + push
2. CI 全绿后打 P0 收尾 tag：`git tag v0.0.1-preview.1 && git push origin v0.0.1-preview.1`
   → release.yml 自动出 draft（版本三处已对齐 `0.0.1-preview.1`；签名 secret 缺省自动跳过）
3. 完成第二批 lane 人工审查（重点：Job Object 回收测试的真实性、zip-slip 防护逐行、
   托盘/单实例对 nightly 无头环境的影响、bridge.ts 的 fallback 标注）
4. **契约变更请求 ×3（需用户决策，走独立 [contract] PR + bump）**：
   - `FsWriteFile { path, content }`（保存功能前置）
   - workspace 根查询命令（如 `workspace_root`/`config_get`，文件树需要根来源）
   - codegen 产物改为带 export 的声明（web 可直接 import bindings，删 contract.ts 跳板）
5. 用户事项：SignPath Foundation 申请、本地 `cargo install --locked cargo-deny`
6. P1 后续：editor 保存（等契约）、Runtime 设置页 UI、`LayoutProfile` 与 dockview、
   P1 出口标准的性能门槛实测（harness 已就绪：scripts/perf/）

## 本地验证命令（与 CI 等价）

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm.cmd lint && pnpm.cmd typecheck && pnpm.cmd test -- --run && pnpm.cmd build   # 仓库根
C:/Git/usr/bin/bash.exe -c "export PATH=/usr/bin:$PATH; exec scripts/check-lanes.sh"
cargo deny check --locked   # 需先安装 cargo-deny
```

## 环境备忘（本机特有，避免重新踩坑）

- `world.run`/spawn 下 pnpm 必须写 **`pnpm.cmd`**（无扩展名 shim 是 ENOENT）
- `bash` 解析到 WSL；Git Bash 在 `C:\Git\usr\bin\bash.exe`，需
  `-c "export PATH=/usr/bin:$PATH; exec <script>"`
- cargo-deny-action 要显式 `rust-version`（仓库 rust-toolchain.toml 会 override 其 musl 工具链）
- `gh run watch --exit-status` 的退出码别被后续 `echo`/管道吞掉
- 工作流（dynamic-workflows）：门禁命令必须编译期字面量；`git add -A` 会卷入
  subagent 进行中的文件——提交一律用精确路径
- 工作流脚本样例（已验证可复用）：`.zcode/workflow-drafts/*.dwf.ts`
  （三 lane 并行 + 字面量门禁 + 自修复循环 + board/markdown 产物）；新会话使用前需重新加载
  dynamic-workflows skill
- pwsh 7.6.9 为 Store 版（WindowsApps），版本校验通过；rustup pinned 1.99.1 已装

## 测试基线（本机 Windows，最近一次全绿）

lc-core 28（runtime 17 + fs 11）· lc-cli 9（golden 5 + cli 4）· lc-proto 8 ·
web 48（5 文件）· 合计 **93**；第二批后新增 process/runtime/config/bridge 测试待入基线。
