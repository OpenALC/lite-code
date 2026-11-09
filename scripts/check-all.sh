#!/usr/bin/env bash
# 发版前全量本地门禁（与 CI verify 等价；docs/STATUS.md「本地验证命令」的脚本化）。
# 用法：bash scripts/check-all.sh
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n==> %s\n' "$*"; }

step "cargo fmt --all --check"
cargo fmt --all --check

step "cargo clippy --workspace --all-targets --locked -- -D warnings"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "cargo test --workspace --locked"
cargo test --workspace --locked

step "pnpm lint / typecheck / test / build"
pnpm lint
pnpm typecheck
pnpm test
pnpm build

step "lane 边界检查"
if [[ -x /c/Git/usr/bin/bash.exe ]]; then
  /c/Git/usr/bin/bash.exe -c 'export PATH=/usr/bin:$PATH; exec scripts/check-lanes.sh'
else
  scripts/check-lanes.sh
fi

step "cargo deny --locked check"
cargo deny --locked check

printf '\n全量门禁通过。\n'
