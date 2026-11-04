#!/usr/bin/env bash
# lane 边界检查（docs/CI.md §8 / docs/MILESTONES.md §10）。
#
# 规则：crates/lc-core 下的 lane 目录之间不允许直接互相引用——
# lane 之间只通过 lc-proto 契约交互；同 crate 的跨 lane 复用必须
# 走 crate 根（lib.rs）的 re-export，不允许 `crate::<其他lane>::…`。
#
# 实现用 grep（按 docs/CI.md §8 的约定，不引入额外工具）。
# AI 比人更容易越界，这条检查优先于别的检查。
set -euo pipefail
cd "$(dirname "$0")/.."

CORE_SRC="crates/lc-core/src"
if [ ! -d "$CORE_SRC" ]; then
  echo "::notice::lc-core 尚不存在，跳过 lane 检查"
  exit 0
fi

# 收集 lane 目录（每个一级子目录 = 一个 lane）
lanes=()
while IFS= read -r d; do
  lanes+=("$(basename "$d")")
done < <(find "$CORE_SRC" -mindepth 1 -maxdepth 1 -type d | sort)

if [ "${#lanes[@]}" -le 1 ]; then
  echo "::notice::lane 目录不足 2 个（${lanes[*]-无}），无需检查"
  exit 0
fi

violations=0
for a in "${lanes[@]}"; do
  for b in "${lanes[@]}"; do
    [ "$a" = "$b" ] && continue
    # lane a 内部引用 lane b：`crate::b` / `crate::b::`（use、路径、类型位置都命中）
    hits=$(grep -rnE "crate::${b}\b" "$CORE_SRC/$a" --include='*.rs' 2>/dev/null || true)
    if [ -n "$hits" ]; then
      echo "::error::lane 越界：'$a' 引用了 '$b' 的内部路径（应走 lc-proto 契约或 crate 根 re-export）"
      echo "$hits" | sed 's/^/    /'
      violations=$((violations + 1))
    fi
  done
done

if [ "$violations" -gt 0 ]; then
  echo ""
  echo "lane 边界检查失败：$violations 处越界。"
  echo "跨 lane 复用的正确姿势：把共享类型放进 lc-proto，或由 lib.rs re-export 明确的 pub 接口。"
  exit 1
fi

echo "lane 边界检查通过（lanes: ${lanes[*]}）"
