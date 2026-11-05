//! 文件系统服务：路径校验、按层列出、全树遍历、变更监听。
//!
//! 设计约束（docs/ARCHITECTURE.md §5.2、docs/PLAN.md R3）：
//! - 目录**按层返回**，不整树序列化（docs/UI.md：文件树按需加载，一次一层）；
//! - 全树遍历用 `ignore` crate，天然尊重 `.gitignore` / `.ignore`；
//! - 读路径先 canonicalize 再校验必须落在项目根内，越界返回
//!   [`lc_proto::ErrorCode::PathOutsideRoot`] 语义错误；
//! - 监听用 `notify` + `notify-debouncer-full`（去抖 200ms），回调聚合为
//!   `(generation, paths)`，generation 跳变即重拉（与事件信封 `seq` 同语义）。
//!
//! 测试策略与 runtime 模块一致：校验逻辑写成**纯函数**、文件系统访问经
//! [`PathResolver`] trait 注入，单测不碰真实文件系统；遍历与监听用真实
//! 临时目录做集成测试。

pub mod listing;
pub mod paths;
pub mod walk;
pub mod watch;
pub mod write;

#[cfg(test)]
mod tests;

pub use listing::{DirLevel, read_dir_level};
pub use paths::{PathResolver, RealPathResolver, display_path, ensure_within_root};
pub use walk::walk_tree;
pub use watch::{DEBOUNCE, FileWatcher, WatchEvent, WatchSink};
pub use write::write_file_text;

use std::path::Path;

use lc_proto::{ErrorBody, ErrorCode};

/// fs lane 的错误集合。[`FsError::code`] 映射到契约错误码
/// （crates/lc-proto/src/error.rs），传输层包装成 `ErrorBody` 返回 UI。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    /// 路径校验失败：目标在项目根之外（docs/PLAN.md R3 安全边界）。
    #[error("path outside project root: {path}")]
    PathOutsideRoot { path: String },
    #[error("path not found: {path}")]
    NotFound { path: String },
    #[error("io error on {path}: {message}")]
    Io { path: String, message: String },
}

impl FsError {
    /// 对应的契约错误码。lane 私有的细节不进契约，调用方需要时放入
    /// `ErrorBody::detail`。
    pub fn code(&self) -> ErrorCode {
        match self {
            FsError::PathOutsideRoot { .. } => ErrorCode::PathOutsideRoot,
            FsError::NotFound { .. } => ErrorCode::NotFound,
            FsError::Io { .. } => ErrorCode::Io,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        ErrorBody::new(self.code(), self.to_string())
    }
}

/// `std::io::Error` → [`FsError`]：`NotFound` 单列（契约有对应码），其余归 `Io`。
pub(crate) fn io_error(path: &Path, e: std::io::Error) -> FsError {
    let path = display_path(path);
    if e.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound { path }
    } else {
        FsError::Io {
            path,
            message: e.to_string(),
        }
    }
}
