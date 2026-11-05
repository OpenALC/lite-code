#!/usr/bin/env bash
# lane 边界检查（docs/CI.md §8 / docs/MILESTONES.md §10）。
#
# 规则（与 docs/CI.md §8 对齐）：
#   - lane 之间只通过 lc-proto 契约交互；
#   - 同 crate 的跨 lane 复用只允许目标 lane 根（mod.rs）的 pub 面；
#   - `crate::<lane>::<子模块>::…` 一律越界（pub 面只允许单跳）。
#
# 实现用 grep/awk（按 docs/CI.md §8 的约定，不引入额外工具）。
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

# 目标 lane 根里声明的子模块名（含私有 mod）：用于识别深路径。
lane_modules() {
  local mod_file="$CORE_SRC/$1/mod.rs"
  [ -f "$mod_file" ] || return 0
  grep -oE '^[[:space:]]*(pub([[:space:]]+\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*' "$mod_file" \
    | awk '{print $NF}' | sort -u || true
}

# 目标 lane 根的 pub 面：pub 项 + pub use 重导出的末段标识符。
lane_pub_surface() {
  local mod_file="$CORE_SRC/$1/mod.rs"
  [ -f "$mod_file" ] || return 0
  {
    grep -oE '^[[:space:]]*pub[[:space:]]+(fn|struct|enum|trait|mod|type|const|static)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*' "$mod_file" \
      | awk '{print $NF}' || true
    grep -E '^[[:space:]]*pub[[:space:]]+use[[:space:]]' "$mod_file" \
      | sed -E 's/^[[:space:]]*pub[[:space:]]+use[[:space:]]+//; s/;[[:space:]]*$//' \
      | awk -F',' '{
          for (i = 1; i <= NF; i++) {
            seg = $i
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", seg)
            gsub(/[{}]/, "", seg)
            if (match(seg, /[[:space:]]as[[:space:]]+[A-Za-z_][A-Za-z0-9_]*$/)) {
              alias = substr(seg, RSTART)
              sub(/^[[:space:]]*as[[:space:]]+/, "", alias)
              print alias
            } else if (match(seg, /[A-Za-z_][A-Za-z0-9_]*$/)) {
              print substr(seg, RSTART)
            }
          }
        }' || true
  } | sort -u
}

# 从单个文件中提取对 lane b 的引用，每行输出 “ITEM <name>” 或 “PATH <name>”：
#   ITEM —— 单跳引用（crate::b::X / crate::b::{X, Y}）
#   PATH —— 后接 :: 的引用（crate::b::X::…），由调用方区分类型关联路径与子模块深路径
refs_in_file() {
  local other="$1" file="$2"
  tr '\n' ' ' < "$file" | awk -v b="$other" '
    {
      s = $0
      pat = "crate::" b "::"
      while ((i = index(s, pat)) != 0) {
        rest = substr(s, i + length(pat))
        if (substr(rest, 1, 1) == "{") {
          depth = 0; body = ""; closed = 0
          for (j = 1; j <= length(rest); j++) {
            c = substr(rest, j, 1)
            if (c == "{") { depth++; if (depth == 1) continue }
            else if (c == "}") { depth--; if (depth == 0) { closed = j; break } }
            if (depth >= 1) body = body c
          }
          if (closed == 0) { print "PATH ?"; break }
          n = split(body, parts, ",")
          for (k = 1; k <= n; k++) {
            seg = parts[k]
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", seg)
            sub(/[[:space:]]+as[[:space:]].*/, "", seg)
            if (seg == "self") continue
            if (match(seg, /[A-Za-z_][A-Za-z0-9_]*$/)) print "ITEM " substr(seg, RSTART)
          }
          s = substr(rest, closed + 1)
        } else if (match(rest, /^[A-Za-z_][A-Za-z0-9_]*/)) {
          item = substr(rest, 1, RLENGTH)
          after = substr(rest, RLENGTH + 1)
          if (substr(after, 1, 2) == "::") print "PATH " item
          else print "ITEM " item
          s = after
        } else {
          s = substr(rest, 2)
        }
      }
    }'
}

violations=0
for a in "${lanes[@]}"; do
  for b in "${lanes[@]}"; do
    [ "$a" = "$b" ] && continue

    pubset=$(lane_pub_surface "$b")
    modules=$(lane_modules "$b")

    while IFS= read -r file; do
      bad=()
      while IFS= read -r ref; do
        [ -z "$ref" ] && continue
        item=${ref#* }
        if printf '%s\n' "$modules" | grep -qxF -- "$item"; then
          bad+=("crate::$b::$item::…（子模块内部不可跨 lane 引用）")
        elif ! printf '%s\n' "$pubset" | grep -qxF -- "$item"; then
          bad+=("crate::$b::$item（目标 lane 根未导出为 pub）")
        fi
      done < <(refs_in_file "$b" "$file")

      if [ "${#bad[@]}" -gt 0 ]; then
        echo "::error::lane 越界：'$a' 引用了 '$b' 的内部路径（只允许目标 lane 根的 pub 面）"
        printf '    %s\n' "${bad[@]}"
        grep -nE "crate::${b}(::|\b)" "$file" | sed 's/^/    /' || true
        violations=$((violations + ${#bad[@]}))
      fi
    done < <(find "$CORE_SRC/$a" -type f -name '*.rs' | sort)
  done
done

if [ "$violations" -gt 0 ]; then
  echo ""
  echo "lane 边界检查失败：$violations 处越界。"
  echo "跨 lane 复用的正确姿势：把共享类型放进 lc-proto，或由目标 lane 根的 pub 面导出。"
  exit 1
fi

echo "lane 边界检查通过（lanes: ${lanes[*]}）"
