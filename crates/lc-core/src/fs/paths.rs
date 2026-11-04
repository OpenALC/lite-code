//! 路径校验（docs/PLAN.md R3 安全边界）。
//!
//! 读路径先 canonicalize 再校验必须落在项目根内。canonicalize 一步完成三件事：
//! 1. 折叠 `..` / `.` —— `../` 逃逸在文本层就被还原成真实位置；
//! 2. 解析符号链接 —— 根内的软链接指向根外同样会被拒；
//! 3. 归一化 Windows verbatim 前缀 —— 根与目标用同一规则产出，可安全做前缀比较。
//!
//! 校验本身是**纯函数**，文件系统访问全部经 [`PathResolver`] 注入。

use std::path::{Path, PathBuf};

use super::{FsError, io_error};

/// 路径解析抽象。测试用 fake 实现，真实实现走 [`RealPathResolver`]。
pub trait PathResolver: Send + Sync {
    /// 语义同 `std::fs::canonicalize`：路径必须存在，返回规范化绝对路径。
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
}

/// 真实实现。
pub struct RealPathResolver;

impl PathResolver for RealPathResolver {
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
}

/// 校验 `path` 落在 `root` 内（含 root 本身），返回 canonicalize 后的路径。
/// `root` 无法 canonicalize 视为根不存在；`path` 无法 canonicalize 视为路径不存在。
pub fn ensure_within_root(
    root: &Path,
    path: &Path,
    resolver: &dyn PathResolver,
) -> Result<PathBuf, FsError> {
    let root = resolver.canonicalize(root).map_err(|e| io_error(root, e))?;
    let canonical = resolver.canonicalize(path).map_err(|e| io_error(path, e))?;
    if !contained_in(&canonical, &root) {
        return Err(FsError::PathOutsideRoot {
            path: display_path(path),
        });
    }
    Ok(canonical)
}

/// 前缀包含判断。Windows 路径大小写不敏感
/// （docs/AGENT.md §4.1：`case_sensitive_paths: false`），其余平台按组件精确比较。
fn contained_in(canonical: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let p = canonical.to_string_lossy().to_lowercase();
        let r = root
            .to_string_lossy()
            .to_lowercase()
            .trim_end_matches('\\')
            .to_string();
        // canonicalize 产物统一为 `\` 分隔的绝对路径，字符串前缀比较足够；
        // 补分隔符避免 `D:\proj2` 命中 `D:\proj`。
        p == r || p.starts_with(&format!("{r}\\"))
    }
    #[cfg(not(windows))]
    {
        // `Path::starts_with` 按组件比较，`/root2` 不会命中 `/root`
        canonical.starts_with(root)
    }
}

/// 规范路径 → 展示路径：去掉 Windows verbatim 前缀（`\\?\C:\…` → `C:\…`）。
/// 契约里的 path 字段（`DirEntryDto.path` 等）一律走这里，前端不做二次处理。
pub fn display_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    }
}
