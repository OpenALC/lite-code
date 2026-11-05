//! 文本文件保存（P1 Editor UI lane 的写路径）。
//!
//! 安全边界与读路径一致（docs/PLAN.md R3）：目标必须落在 workspace 根内。
//! 读路径要求目标已存在；保存允许新建文件，因此目标不存在时改为校验父目录，
//! 已存在时仍先 canonicalize（解析符号链接）再校验，防止根内软链指向根外。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::paths::{PathResolver, display_path, ensure_within_root};
use super::{FsError, io_error};

/// 写入文本文件，返回校验后的展示路径。
///
/// 相对路径按 `root` 解析；绝对路径必须是 `root` 内（含 root 本身）。
pub fn write_file_text(
    root: &Path,
    path: &Path,
    contents: &str,
    resolver: &dyn PathResolver,
) -> Result<String, FsError> {
    let target = resolve_write_target(root, path, resolver)?;
    std::fs::write(&target, contents).map_err(|e| io_error(&target, e))?;
    Ok(display_path(&target))
}

fn resolve_write_target(
    root: &Path,
    path: &Path,
    resolver: &dyn PathResolver,
) -> Result<PathBuf, FsError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    match resolver.canonicalize(&candidate) {
        Ok(_) => ensure_within_root(root, &candidate, resolver),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let parent = candidate.parent().ok_or_else(|| FsError::PathOutsideRoot {
                path: display_path(&candidate),
            })?;
            let name = candidate
                .file_name()
                .ok_or_else(|| FsError::PathOutsideRoot {
                    path: display_path(&candidate),
                })?;
            let parent = ensure_within_root(root, parent, resolver)?;
            Ok(parent.join(name))
        }
        Err(e) => Err(io_error(&candidate, e)),
    }
}
