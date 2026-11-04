//! 目录按层列出（docs/ARCHITECTURE.md §5.2、docs/UI.md §3：按需加载，一次一层）。
//!
//! 不递归、不整树序列化：树由前端逐层展开，`generation` 由监听侧提供
//! （[`super::watch`]），本模块只产出单层快照。DTO 复用契约的
//! [`lc_proto::DirEntryDto`] / [`lc_proto::EntryKind`]，lane 不自造结构。

use std::fs;
use std::path::Path;

use lc_proto::{DirEntryDto, EntryKind};

use super::paths::{PathResolver, display_path, ensure_within_root};
use super::{FsError, io_error};

/// 单层目录快照。`path` 为校验后的展示路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirLevel {
    pub path: String,
    pub entries: Vec<DirEntryDto>,
}

/// 列出 `path` 一层目录。先做根内校验（docs/PLAN.md R3），再 `read_dir`。
/// 条目按名称排序（大小写不敏感）；"目录优先"之类的展示策略属于 UI 层，
/// 这里只保证顺序确定。
pub fn read_dir_level(
    root: &Path,
    path: &Path,
    resolver: &dyn PathResolver,
) -> Result<DirLevel, FsError> {
    let canonical = ensure_within_root(root, path, resolver)?;

    let mut entries = Vec::new();
    for entry in fs::read_dir(&canonical).map_err(|e| io_error(&canonical, e))? {
        // 单个条目失效（竞态删除 / 权限）不影响整层，跳过
        let Ok(entry) = entry else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        let kind = entry_kind(&ft);
        // size 只对普通文件取：目录与符号链接没有稳定的"大小"语义
        let size = if ft.is_file() {
            entry.metadata().ok().map(|m| m.len())
        } else {
            None
        };
        entries.push(DirEntryDto {
            path: display_path(&canonical.join(entry.file_name())),
            name: entry.file_name().to_string_lossy().into_owned(),
            kind,
            size,
        });
    }
    entries.sort_by_cached_key(|e| e.name.to_lowercase());
    Ok(DirLevel {
        path: display_path(&canonical),
        entries,
    })
}

/// 非 `DirEntry` 路径（walk 场景）复用的 kind 归类。
pub(crate) fn entry_kind(ft: &std::fs::FileType) -> EntryKind {
    // DirEntry::file_type 不跟随符号链接：symlink 优先于 dir/file 判定
    if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    }
}
