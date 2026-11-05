//! 文本文件保存（P1 Editor UI lane 的写路径）。
//!
//! 安全边界与读路径一致（docs/PLAN.md R3）：目标必须落在 workspace 根内。
//! 读路径要求目标已存在；保存允许新建文件，因此目标不存在时改为校验父目录，
//! 已存在时仍先 canonicalize（解析符号链接）再校验，防止根内软链指向根外。
//!
//! 落盘走同目录临时文件 + fsync + rename：R3 要求敏感写入原子替换，
//! 也避免保存中途失败把源文件截断。

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::paths::{PathResolver, display_path, ensure_within_root};
use super::{FsError, io_error};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

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
    write_atomic(&target, contents)?;
    Ok(display_path(&target))
}

fn write_atomic(target: &Path, contents: &str) -> Result<(), FsError> {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = target.with_file_name(format!(".{name}.lc-tmp-{}-{seq}", std::process::id()));

    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        // rename 会替换目标；保留原文件权限（Unix 可执行位、Windows 只读位）。
        if let Ok(meta) = std::fs::metadata(target) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, target)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| io_error(target, e))
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
            // 悬空软链：canonicalize 失败但 symlink_metadata 成功。此时不能按
            // “新文件”处理——std::fs::write/rename 会跟随链接，可能在根外落盘。
            if std::fs::symlink_metadata(&candidate).is_ok() {
                return Err(FsError::PathOutsideRoot {
                    path: display_path(&candidate),
                });
            }
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
