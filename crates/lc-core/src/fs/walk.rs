//! 全树遍历（docs/ARCHITECTURE.md §5.2）：`ignore` crate，尊重 `.gitignore` / `.ignore`。
//!
//! 供后续树构建与 `search_code`（docs/AGENT.md §4：WalkDir + 正则）使用。输出为
//! 一次性向量而非流：两个调用方都需要排序与随机访问，流式收益为零；条目不带
//! `size`（遍历不做 per-file stat，对齐 docs/MILESTONES.md §4 的 1 万文件门槛）。
//!
//! `ignore` crate 默认 `require_git = true`：`.gitignore` 只在 git 仓库内生效，
//! 与 ripgrep 语义一致；`.ignore` 在任意目录生效。隐藏文件（`.` 开头）默认跳过，
//! `.git` 目录因此天然不出现在结果里。

use std::path::Path;

use ignore::WalkBuilder;
use lc_proto::DirEntryDto;

use super::FsError;
use super::listing::entry_kind;
use super::paths::{PathResolver, display_path, ensure_within_root};

/// 全树遍历。返回根下全部未被忽略的条目（不含根本身——它是树的锚点），
/// 按 `path` 排序。单个条目失败（权限 / 竞态）跳过，不中断整棵树。
pub fn walk_tree(root: &Path, resolver: &dyn PathResolver) -> Result<Vec<DirEntryDto>, FsError> {
    let canonical = ensure_within_root(root, root, resolver)?;
    if !canonical.is_dir() {
        return Err(FsError::Io {
            path: display_path(&canonical),
            message: "not a directory".into(),
        });
    }

    let mut out = Vec::new();
    for entry in WalkBuilder::new(&canonical).build() {
        let Ok(de) = entry else { continue };
        if de.path() == canonical.as_path() {
            continue;
        }
        let Some(ft) = de.file_type() else { continue };
        out.push(DirEntryDto {
            name: de.file_name().to_string_lossy().into_owned(),
            path: display_path(de.path()),
            kind: entry_kind(&ft),
            size: None,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
