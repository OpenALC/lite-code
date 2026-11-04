# 贡献指南

## 环境要求

| 工具 | 版本 | 说明 |
|---|---|---|
| Rust | **1.99.1**（由 `rust-toolchain.toml` 钉定） | 不得提升 MSRV（PLAN.md R8） |
| Node | ≥ 22 | 前端构建 |
| pnpm | 10.x | 以根 `package.json` 的 `packageManager` 为准 |
| cargo-deny | 最新 | 本地复现 CI 的 deny 检查 |

## 本地验证（与 CI 等价，提交前全绿）

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test -- --run && pnpm build
scripts/check-lanes.sh
cargo deny check --locked
```

## 规则速查（详见 docs/）

- **契约（`crates/lc-proto`）只读**：变更走独立 `[contract]` PR 并 bump 契约版本；
  改完运行 `pnpm contract:codegen` 重新生成 TS 绑定并提交，漂移检查会拦截遗漏。
- **lane 边界**：`crates/lc-core/<lane>` 之间禁止互相 import 内部路径；共享下沉到 `lc-proto` 或 crate 根 re-export。
- **提交**：一个 PR 一个 squash commit，标题用 Conventional Commits（`feat(agent): …` / `fix(db): …`）。
- **golden**：核心命令必须有 golden 输出测试；变更输出必须显式更新基线。
- **分支**：trunk-based；lane 分支命名 `lane/<name>-<topic>`。
