//! 插件市场（docs/plans/plugin-marketplace.md，M1：静态 index + 多源镜像下的
//! 搜索 / 安装 / 更新 / 卸载）。
//!
//! - `index`：index.json 线格式解析与校验（§3.1，fail-closed）；
//! - `source`：`~/.lite-code/plugin-registries.json` 源清单读写、多源并行拉取、
//!   TTL 内存缓存、按 id 去重取 semver 最高（§3.3）；
//! - `install`：包下载 / sha256 / zip 防穿越解压 / index-包一致性比对 /
//!   tmp+rename 原子落位 / 三级配额 / GC / 卸载 / 更新（§3.2/§3.5/§3.6）。
//!
//! 安全模型（§3.7）：M1 为警告档——HTTPS + sha256 完整性，签名强制档（M3）
//! 落地前 `signature` 只透传状态不做密码学校验。

mod index;
mod install;
mod source;

use std::path::{Component, Path, PathBuf};

pub use index::{MarketPlugin, MarketSearchResult, ParsedIndex, SignatureState};
pub use install::{
    DEFAULT_QUOTA_MB, GC_TMP_MAX_AGE, MAX_EXTRACT_BYTES, MAX_PACKAGE_BYTES, MAX_PACKAGE_ENTRIES,
    QUOTA_ENV, TMP_DIR_NAME, UNINSTALL_BAK_SUFFIX, gc_stale, install, uninstall, update,
};
use lc_proto::{ErrorBody, ErrorCode};
pub use source::{
    BUILTIN_SOURCE_NAME, FetchLimits, IndexCache, PER_SOURCE_TIMEOUT, RegistryList, RegistrySource,
    RegistrySourceInfo, SEARCH_BUDGET, SOURCE_CACHE_TTL, add_registry, list_registries,
    remove_registry, search, set_registry_enabled,
};

/// 市场错误集合。每个变体映射到契约 [`ErrorCode`]，命令层包装成 [`ErrorBody`]；
/// 不新增契约码（§3.4）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MarketError {
    #[error("无法定位用户主目录")]
    NoHomeDir,
    #[error("插件源配置错误：{0}")]
    Config(String),
    #[error("插件市场文件读写失败：{0}")]
    Io(String),
    #[error("插件市场网络请求失败：{0}")]
    Http(String),
    #[error("插件源清单(index)无效：{0}")]
    InvalidIndex(String),
    #[error("插件 `{0}` 不存在")]
    NotFound(String),
    #[error("参数无效：{0}")]
    InvalidArgument(String),
    #[error("插件包与 index 不一致：{0}")]
    ContractMismatch(String),
    #[error("超出扩展存储配额：{0}")]
    QuotaExceeded(String),
}

impl MarketError {
    pub fn code(&self) -> ErrorCode {
        match self {
            MarketError::NoHomeDir | MarketError::Config(_) | MarketError::Http(_) => {
                ErrorCode::Internal
            }
            MarketError::Io(_) => ErrorCode::Io,
            MarketError::InvalidIndex(_) | MarketError::ContractMismatch(_) => {
                ErrorCode::ContractMismatch
            }
            MarketError::NotFound(_) => ErrorCode::NotFound,
            MarketError::InvalidArgument(_) => ErrorCode::InvalidArgument,
            MarketError::QuotaExceeded(_) => ErrorCode::PermissionDenied,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        ErrorBody::new(self.code(), self.to_string())
    }
}

/// 只允许 https；回环地址允许 http（同 updater 的约束模式，
/// docs/plans/plugin-marketplace.md §3.1）。
pub(crate) fn is_https_or_loopback(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        return !rest.is_empty() && !rest.starts_with('/') && !rest.contains(char::is_whitespace);
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = match authority.find(']') {
        Some(end) => &authority[..=end],
        None => authority.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "[::1]"
}

/// `file://` 源（§3.1：允许但 UI 需标「本地源，自行承担信任」）。
pub(crate) fn is_file_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("file://") else {
        return false;
    };
    if let Some(stripped) = rest.strip_prefix('/') {
        // file:///abs（Windows 盘符形态 file:///C:/... 同理）。
        return !stripped.is_empty();
    }
    // file://host/abs —— 只允许 localhost。
    match rest.find('/') {
        Some(end) => rest[..end].eq_ignore_ascii_case("localhost") && end + 1 < rest.len(),
        None => false,
    }
}

/// index 条目 dist.url 与源清单共用的白名单：https / 回环 http / file。
pub(crate) fn is_allowed_url(url: &str) -> bool {
    is_https_or_loopback(url) || is_file_url(url)
}

/// `file://` URL → 本地路径。支持 `file:///abs`：Windows 盘符（`/C:/...`）
/// 剥一层前导斜杠，Unix 绝对路径（`/tmp/...`）保留前导斜杠；也支持
/// `file://localhost/abs`；拒绝 `..` 段，不做百分号解码（M1 约定路径不含
/// 空格，生成端与测试均使用原始路径）。
pub(crate) fn file_url_to_path(url: &str) -> Option<PathBuf> {
    if !is_file_url(url) {
        return None;
    }
    let rest = url.strip_prefix("file://")?;
    let path = if let Some(stripped) = rest.strip_prefix('/') {
        // Windows 盘符（`/C:/...`）才剥前导斜杠；`/tmp/...` 这类 Unix 绝对
        // 路径必须保留，否则解析成相对路径后文件打开失败（Ubuntu CI 实测）。
        let is_windows_drive = stripped.len() >= 2
            && stripped.as_bytes()[0].is_ascii_alphabetic()
            && stripped.as_bytes()[1] == b':';
        if is_windows_drive { stripped } else { rest }
    } else {
        let end = rest.find('/')?;
        if !rest[..end].eq_ignore_ascii_case("localhost") {
            return None;
        }
        &rest[end + 1..]
    };
    if path.is_empty() || path.split('/').any(|segment| segment == "..") {
        return None;
    }
    Some(PathBuf::from(path))
}

/// zip 条目名防穿越（§3.2）：拒绝对路径、`..` 组件与盘符前缀（`C:\` / `C:` /
/// `/`），其余组件拼回收窄在 `root` 下的目标路径。
pub(crate) fn safe_zip_target(root: &Path, name: &str) -> Result<PathBuf, MarketError> {
    if name.is_empty() || name.contains(char::is_control) {
        return Err(MarketError::ContractMismatch(format!(
            "包内条目 `{name}` 非法（空名或含控制字符）"
        )));
    }
    // 盘符前缀跨平台显式拒绝：Windows 上 `C:foo` 是 Prefix 组件，其他平台的
    // Component 会把它当普通段放过，但同一包解压到 Windows 即成盘符穿越。
    if name.len() >= 2 && name.as_bytes()[0].is_ascii_alphabetic() && name.as_bytes()[1] == b':' {
        return Err(MarketError::ContractMismatch(format!(
            "包内条目 `{name}` 越界（拒绝盘符前缀）"
        )));
    }
    let mut relative = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            _ => {
                return Err(MarketError::ContractMismatch(format!(
                    "包内条目 `{name}` 越界（拒绝绝对路径与 `..`）"
                )));
            }
        }
    }
    Ok(root.join(relative))
}

#[cfg(test)]
pub(crate) mod testutil {
    //! 市场 lane 共享测试工具：临时目录、极简 loopback HTTP server
    //! （docs/plans/plugin-marketplace.md §4「零新依赖」约束）、zip 正样本构造。

    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::Duration;

    /// 进程内唯一临时目录（Drop 清理）。
    pub(crate) struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "lc-market-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("创建临时目录");
            Self { path }
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// loopback 假 HTTP server：每个连接交给 handler，返回的响应字节由
    /// server 统一写出（hits 为累计连接数）。
    pub(crate) struct TestServer {
        pub url: String,
        pub hits: Arc<AtomicUsize>,
    }

    pub(crate) fn serve_fn(
        handler: impl Fn(TcpStream, usize) -> Vec<u8> + Send + Sync + 'static,
    ) -> TestServer {
        serve_raw(move |stream, hit| {
            let response = handler(stream.try_clone().expect("clone stream"), hit);
            write_raw(&stream, &response);
        })
    }

    /// handler 自行写响应的版本（Content-Length 撒谎/截断等异常用例）。
    /// 先排空请求头再交给 handler：服务器抢在客户端写完请求前关连接会触发
    /// RST 丢弃已写响应（Windows 上尤其明显，updater 假 server 也先读后写）。
    pub(crate) fn serve_raw(
        handler: impl Fn(TcpStream, usize) + Send + Sync + 'static,
    ) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let thread_hits = hits.clone();
        std::thread::spawn(move || {
            let handler = handler;
            while let Ok((stream, _)) = listener.accept() {
                let hit = thread_hits.fetch_add(1, Ordering::Relaxed);
                drain_request(&stream);
                handler(stream, hit);
            }
        });
        TestServer {
            url: format!("http://127.0.0.1:{port}"),
            hits,
        }
    }

    /// 读取请求头直到空行（带 2s 读超时兜底，避免坏客户端挂死 accept 线程）。
    fn drain_request(stream: &TcpStream) {
        use std::io::Read;

        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = [0_u8; 2048];
        let mut total = 0_usize;
        let mut reader = stream;
        loop {
            match reader.read(&mut buf[total..]) {
                Ok(0) => break,
                Ok(read) => {
                    total += read;
                    let header_end = buf[..total]
                        .windows(4)
                        .any(|window| window == [13, 10, 13, 10]);
                    if header_end || total == buf.len() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    impl TestServer {
        /// 恒 404 的服务器（失败源用例）。
        pub(crate) fn not_found() -> Self {
            serve_fn(|_, _| {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            })
        }
    }

    pub(crate) fn http_ok(body: &str) -> Vec<u8> {
        http_ok_bytes(body.as_bytes())
    }

    /// 200 OK + 任意二进制体（zip 包等）。
    pub(crate) fn http_ok_bytes(body: &[u8]) -> Vec<u8> {
        let mut response =
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
        response.extend_from_slice(body);
        response
    }

    pub(crate) fn write_raw(stream: &TcpStream, bytes: &[u8]) {
        let mut stream = stream;
        let _ = stream.write_all(bytes);
        let _ = stream.flush();
    }

    /// 本地路径 → file:// URL（Windows 盘符路径补前导斜杠）。
    pub(crate) fn file_url(path: &Path) -> String {
        let raw = path.display().to_string();
        if raw.contains("://") {
            return raw;
        }
        let with_leading = if raw.starts_with('/') {
            raw.clone()
        } else {
            format!("/{raw}")
        };
        format!("file://{with_leading}")
    }

    /// 用 zip crate 构造正样本包（stored，零压缩依赖）。
    pub(crate) fn build_zip<S: AsRef<str>, B: AsRef<[u8]>>(entries: &[(S, B)]) -> Vec<u8> {
        use std::io::Write as _;

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer
                .start_file(name.as_ref(), options)
                .expect("start zip entry");
            writer.write_all(bytes.as_ref()).expect("write zip entry");
        }
        let cursor = writer.finish().expect("finish zip");
        cursor.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_whitelist_accepts_https_loopback_and_file() {
        assert!(is_https_or_loopback("https://example.com/index.json"));
        assert!(is_https_or_loopback("http://127.0.0.1:8080/index.json"));
        assert!(is_https_or_loopback("http://localhost/index.json"));
        assert!(is_https_or_loopback("http://[::1]:8080/index.json"));
        assert!(!is_https_or_loopback("http://example.com/index.json"));
        assert!(!is_https_or_loopback("ftp://example.com/index.json"));
        assert!(!is_https_or_loopback("https:///index.json"));

        assert!(is_file_url("file:///tmp/index.json"));
        assert!(is_file_url("file://localhost/tmp/index.json"));
        assert!(!is_file_url("file://evil host/x"));
        assert!(!is_file_url("https://example.com/index.json"));
    }

    #[test]
    fn file_url_round_trips_to_local_path() {
        // Unix 绝对路径必须保留前导斜杠（回归：曾被剥成相对路径导致 CI 红）。
        let unix = file_url_to_path("file:///tmp/lite-code/index.json").expect("parse unix url");
        assert_eq!(unix, Path::new("/tmp/lite-code/index.json"));

        let windows = file_url_to_path("file:///C:/tmp/index.json").expect("parse windows url");
        assert!(windows.ends_with(Path::new("C:/tmp/index.json")));

        assert!(file_url_to_path("file:///tmp/../secret.json").is_none());
        assert!(file_url_to_path("http://example.com/index.json").is_none());
    }

    #[test]
    fn zip_slip_rejects_traversal_absolute_and_drive_prefix() {
        let root = Path::new("/tmp/root");
        let err = safe_zip_target(root, "../evil").expect_err(".. rejected");
        assert_eq!(err.code(), ErrorCode::ContractMismatch);
        assert!(safe_zip_target(root, "/abs").is_err());
        assert!(safe_zip_target(root, "C:\\evil").is_err());
        assert!(safe_zip_target(root, "C:/evil").is_err());
        assert!(safe_zip_target(root, "C:evil").is_err());
        assert!(safe_zip_target(root, "pkg/../../evil").is_err());

        let ok = safe_zip_target(root, "pkg/plugin.json").expect("relative ok");
        assert!(ok.ends_with(Path::new("root/pkg/plugin.json")));
    }

    #[test]
    fn market_error_maps_to_documented_codes() {
        assert_eq!(
            MarketError::NotFound("x".into()).code(),
            ErrorCode::NotFound
        );
        assert_eq!(
            MarketError::InvalidIndex("x".into()).code(),
            ErrorCode::ContractMismatch
        );
        assert_eq!(
            MarketError::ContractMismatch("x".into()).code(),
            ErrorCode::ContractMismatch
        );
        assert_eq!(
            MarketError::InvalidArgument("x".into()).code(),
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            MarketError::QuotaExceeded("x".into()).code(),
            ErrorCode::PermissionDenied
        );
        assert_eq!(MarketError::Http("x".into()).code(), ErrorCode::Internal);
        assert_eq!(MarketError::Io("x".into()).code(), ErrorCode::Io);
    }
}
