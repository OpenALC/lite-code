#!/usr/bin/env bash
# Linux 发布打包：deb 稳定产出；AppImage 依赖的 linuxdeploy 下载/运行偶发失败，
# 单独重试并清理 Tauri 工具缓存。APPIMAGE_EXTRACT_AND_RUN 让 AppImage 工具
# 在无 FUSE 的 runner 上自解压运行。
set -euo pipefail

if [ ! -f apps/desktop/src-tauri/tauri.conf.json ]; then
  cargo build --release --locked
  echo "::notice::Tauri 壳未就绪，仅产出二进制"
  exit 0
fi

pnpm tauri build --bundles deb

built=0
for attempt in 1 2 3; do
  if APPIMAGE_EXTRACT_AND_RUN=1 pnpm tauri build --bundles appimage; then
    built=1
    break
  fi
  echo "::warning::AppImage 打包失败（第 ${attempt} 次），清理缓存后重试"
  rm -rf "$HOME"/.cache/tauri/*linuxdeploy* "$HOME"/.cache/tauri/*AppRun* 2>/dev/null || true
  sleep 5
done

if [ "$built" != 1 ]; then
  echo "::error::AppImage 打包连续失败"
  exit 1
fi
