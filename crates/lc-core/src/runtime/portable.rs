//! 便携版下载（docs/RUNTIME.md §5 便携版下载）。
//!
//! 流程：解析版本 → 下载官方 zip → **SHASUMS256.txt 校验（不匹配删除并报错）**
//! → zip 解压到 `data_dir()/runtime/node/`（zip-slip 防护）→ 与探测同一
//! [`validate`] → 写 config（与手动指定同一份）。
//!
//! 纪律：
//! - 网络函数保持薄（[`http_agent`] / [`fetch_text`] / [`download_to_file`]），
//!   无重试无缓存，不带逻辑；所有可测逻辑都是纯函数并有单测；
//! - 单测不访问网络（zip 用本地构造的真实归档验证）；
//! - 镜像地址参数化（`base`），默认 [`DEFAULT_DIST_BASE`]（docs/RUNTIME.md §5
//!   “镜像地址可配置，默认官方源”）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lc_proto::{NodeFailureReason, NodeSource, SemVer};
use sha2::{Digest, Sha256};

use crate::config::{
    ConfigError, ConfigStore, JsonConfigStore, NodeRuntime, data_dir, store_node_runtime,
};

use super::{DirLister, ProcessRunner, parse_version, validate};

/// 官方分发源。传镜像时替换整个前缀（如 `https://npmmirror.com/mirrors/node`）。
pub const DEFAULT_DIST_BASE: &str = "https://nodejs.org/dist";

/// 基准 Node 大版本（docs/RUNTIME.md §2：基准 Node 22）。
/// `node portable download` 不带 --version 时解析 `latest-v22.x`。
pub const DEFAULT_MAJOR: u32 = 22;

/// 单请求/整体超时：官方 win-x64 zip 约 30MB，给慢网络留足余量。
const HTTP_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, thiserror::Error)]
pub enum PortableError {
    /// 官方 Linux/macOS 产物是 .tar.xz / .tar.gz，当前依赖（zip）解不了；
    /// 明确报不支持，而不是静默下载一个解不开的包。
    #[error("便携版下载仅支持 Windows zip 产物（{0}）")]
    UnsupportedPlatform(String),
    #[error("网络错误：{0}")]
    Http(String),
    /// docs/RUNTIME.md §5：校验不匹配 → 删除并报错。
    #[error("SHA256 不匹配：{file}（期望 {expected}，实际 {actual}）")]
    HashMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("SHASUMS256.txt 缺少产物条目：{0}")]
    MissingShasum(String),
    #[error("无法从 {0} 解析出版本（未找到匹配的产物行）")]
    ResolveFailed(String),
    /// zip-slip 防护（docs/RUNTIME.md §5 安全要求）：`..` / 绝对路径 / 盘符。
    #[error("zip 条目路径非法（zip-slip 防护拒绝）：{0}")]
    ZipSlip(String),
    #[error("zip 处理失败：{0}")]
    Zip(String),
    #[error("node 校验失败（{0:?}）")]
    Validate(NodeFailureReason, Option<String>),
    #[error("配置写入失败：{0}")]
    Persist(#[from] ConfigError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// ---- 版本参数（纯函数）----

/// `--version` 参数：完整 `22.14.0`，或主版本 `22`（解析 `latest-v22.x`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    Exact(SemVer),
    Latest(u32),
}

/// 解析用户输入：容忍 `v` 前缀；`22.14` 这类两段式不算版本。
pub fn parse_version_arg(arg: &str) -> Option<VersionSpec> {
    let a = arg.trim().trim_start_matches(['v', 'V']);
    if a.contains('.') {
        parse_version(a).map(VersionSpec::Exact)
    } else {
        a.parse::<u32>().ok().map(VersionSpec::Latest)
    }
}

// ---- SHASUMS256.txt（纯函数）----

/// SHASUMS256.txt 的一行：`<64 位 hex>  <文件名>`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShasumEntry {
    pub hash: String,
    pub file: String,
}

/// 解析 SHASUMS256.txt。宽容处理：空行、缺 hash、hash 非 64 位十六进制的
/// 行跳过（官方文件只有条目行，宽容只为抵抗手工编辑的镜像副本）。
pub fn parse_shasums(text: &str) -> Vec<ShasumEntry> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (hash, rest) = line.split_once(char::is_whitespace)?;
            let hash = hash.trim();
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let file = rest.trim();
            if file.is_empty() {
                return None;
            }
            Some(ShasumEntry {
                hash: hash.to_ascii_lowercase(),
                file: file.to_string(),
            })
        })
        .collect()
}

/// 按产物名精确查找期望 hash。
pub fn lookup_hash<'a>(entries: &'a [ShasumEntry], file: &str) -> Option<&'a str> {
    entries
        .iter()
        .find(|e| e.file == file)
        .map(|e| e.hash.as_str())
}

/// 在 SHASUMS 文本里找匹配后缀的产物行，解析出其精确版本。
/// 用于 `latest-v22.x/SHASUMS256.txt` → `22.x.y`。
pub fn resolve_version_from_shasums(text: &str, artifact_suffix: &str) -> Option<SemVer> {
    parse_shasums(text)
        .into_iter()
        .find(|e| e.file.ends_with(artifact_suffix))
        .and_then(|e| parse_version(&e.file))
}

// ---- 哈希（纯函数）----

/// 小写 hex 编码。
pub fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

/// 流式哈希（30MB 级 zip 不整块进内存）。
pub fn sha256_hex_reader(r: &mut dyn Read) -> std::io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex_lower(&h.finalize()))
}

// ---- zip-slip 防护（纯函数）----

/// zip 条目名 → 解压目标相对路径组件。
///
/// 拒绝（docs/RUNTIME.md §5 安全要求，zip-slip）：
/// - 任何 `..` 段（整个条目拒绝，不做折叠）；
/// - 绝对路径（`/…`，含归一化后变 `//…` 的 Windows UNC）；
/// - 盘符与冒号（`C:`、NTFS 备用数据流 `file:ads`）；
/// - 空名与 NUL 字节。
///
/// `\` 归一为 `/`；`.` 与空段丢弃。返回的组件可安全 `dest.join(...)`。
pub fn sanitize_entry_name(name: &str) -> Option<Vec<String>> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    let norm = name.replace('\\', "/");
    if norm.starts_with('/') {
        return None;
    }
    let mut out = Vec::new();
    for seg in norm.split('/') {
        match seg {
            "" | "." => {}
            ".." => return None,
            s if s.contains(':') => return None,
            s => out.push(s.to_string()),
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

// ---- 平台产物命名（纯函数）----

/// 当前 Windows 架构的官方命名 tag。非 Windows / 非已知架构 → 不支持
/// （官方 Linux 产物是 .tar.xz，依赖里没有 tar/xz 解码器）。
pub fn win_arch_tag() -> Result<&'static str, PortableError> {
    if !cfg!(windows) {
        return Err(PortableError::UnsupportedPlatform(
            "当前平台官方产物是 .tar.xz".into(),
        ));
    }
    match std::env::consts::ARCH {
        "x86_64" => Ok("x64"),
        "aarch64" => Ok("arm64"),
        other => Err(PortableError::UnsupportedPlatform(format!(
            "无 {other} 架构产物的命名映射"
        ))),
    }
}

/// 官方 zip 产物名，如 `node-v22.14.0-win-x64.zip`（docs/RUNTIME.md §5）。
pub fn artifact_name(v: SemVer) -> Result<String, PortableError> {
    let arch = win_arch_tag()?;
    Ok(format!("node-v{v}-win-{arch}.zip"))
}

/// SHASUMS 里按后缀匹配产物的模式。
fn artifact_suffix() -> Result<String, PortableError> {
    let arch = win_arch_tag()?;
    Ok(format!("-win-{arch}.zip"))
}

// ---- 网络函数：薄，无逻辑，不进单测 ----

fn http_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

fn http_err(e: ureq::Error) -> PortableError {
    PortableError::Http(e.to_string())
}

fn fetch_text(agent: &ureq::Agent, url: &str) -> Result<String, PortableError> {
    agent
        .get(url)
        .call()
        .map_err(http_err)?
        .body_mut()
        .read_to_string()
        .map_err(|e| PortableError::Http(e.to_string()))
}

/// 流式下载到文件（不整块进内存）。
fn download_to_file(agent: &ureq::Agent, url: &str, dest: &Path) -> Result<(), PortableError> {
    let mut resp = agent.get(url).call().map_err(http_err)?;
    let mut reader = resp.body_mut().as_reader();
    let mut out = std::fs::File::create(dest)?;
    std::io::copy(&mut reader, &mut out)?;
    Ok(())
}

/// 版本解析。`Latest(major)` 拉 `{base}/latest-v{major}.x/SHASUMS256.txt`
/// （官方保证该目录指向该主版本线的最新版），从产物行解析精确版本。
fn resolve_version(
    agent: &ureq::Agent,
    base: &str,
    spec: &VersionSpec,
) -> Result<SemVer, PortableError> {
    match spec {
        VersionSpec::Exact(v) => Ok(*v),
        VersionSpec::Latest(major) => {
            let url = format!("{base}/latest-v{major}.x/SHASUMS256.txt");
            let suffix = artifact_suffix()?;
            let text = fetch_text(agent, &url)?;
            resolve_version_from_shasums(&text, &suffix).ok_or(PortableError::ResolveFailed(url))
        }
    }
}

// ---- 解压 ----

struct Entry {
    index: usize,
    /// sanitize 后的相对组件
    parts: Vec<String>,
    is_dir: bool,
}

/// 全部条目共享同一第一段、且没有“裸文件”落在根上时返回该段。
/// 官方 zip 的所有内容都在 `node-v<ver>-win-x64/` 下，剥离后 node.exe
/// 直接落在 `runtime/node/`（docs/RUNTIME.md §5 的目标布局）。
fn common_root(entries: &[Entry]) -> Option<String> {
    let first = entries.first()?.parts.first()?.clone();
    let uniform = entries
        .iter()
        .all(|e| e.parts.first() == Some(&first) && (e.is_dir || e.parts.len() >= 2));
    uniform.then_some(first)
}

/// 解压 node 官方 zip 到 `dest`。逐条目过 [`sanitize_entry_name`]，
/// 任一条目非法 → [`PortableError::ZipSlip`]，立即停止。
/// 返回写入的文件数。
pub fn extract_node_zip(archive_path: &Path, dest: &Path) -> Result<usize, PortableError> {
    let zip_err = |e: zip::result::ZipError| PortableError::Zip(e.to_string());
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(zip_err)?;

    let mut entries = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let zf = archive.by_index(i).map_err(zip_err)?;
        let name = zf.name().map_err(zip_err)?.to_string();
        let is_dir = name.ends_with('/');
        let Some(parts) = sanitize_entry_name(&name) else {
            return Err(PortableError::ZipSlip(name));
        };
        entries.push(Entry {
            index: i,
            parts,
            is_dir,
        });
    }

    let strip = common_root(&entries);

    let mut written = 0usize;
    for e in &entries {
        let parts: &[String] = match &strip {
            Some(root) if e.parts.first() == Some(root) => &e.parts[1..],
            _ => &e.parts[..],
        };
        if parts.is_empty() {
            // 只剩根目录自身的条目（"pkg/"）
            std::fs::create_dir_all(dest)?;
            continue;
        }
        let target = parts.iter().fold(dest.to_path_buf(), |p, s| p.join(s));
        if e.is_dir {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut zf = archive.by_index(e.index).map_err(zip_err)?;
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut zf, &mut out)?;
        written += 1;
    }
    Ok(written)
}

// ---- 安装编排 ----

/// 便携版安装根目录：`data_dir()/runtime/node/`（docs/RUNTIME.md §5）。
pub fn install_dir() -> Result<PathBuf, ConfigError> {
    Ok(data_dir()?.join("runtime").join("node"))
}

/// 校验下载产物与 SHASUMS256.txt。不匹配 → 报错（调用方负责删除）。
fn verify_shasums(
    zip_path: &Path,
    artifact: &str,
    shasums_url: &str,
    agent: &ureq::Agent,
) -> Result<(), PortableError> {
    let text = fetch_text(agent, shasums_url)?;
    let expected = lookup_hash(&parse_shasums(&text), artifact)
        .ok_or_else(|| PortableError::MissingShasum(artifact.to_string()))?
        .to_string();
    let mut f = std::fs::File::open(zip_path)?;
    let actual = sha256_hex_reader(&mut f)?;
    if actual != expected {
        return Err(PortableError::HashMismatch {
            file: artifact.to_string(),
            expected,
            actual,
        });
    }
    Ok(())
}

/// 完整安装（docs/RUNTIME.md §5 便携版下载）：
/// 解析版本 → 下载 → SHASUMS256 校验（不匹配删除并报错）→ 清空旧安装 →
/// 解压（zip-slip 防护）→ 同一 [`validate`] → 写 config。
pub fn install(
    spec: &VersionSpec,
    base: &str,
    store: &dyn ConfigStore,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
) -> Result<NodeRuntime, PortableError> {
    let dir = install_dir()?;
    let agent = http_agent();

    let v = resolve_version(&agent, base, spec)?;
    let artifact = artifact_name(v)?;
    let zip_url = format!("{base}/v{v}/{artifact}");
    let shasums_url = format!("{base}/v{v}/SHASUMS256.txt");

    // 安装包与 runtime/node/ 同级，装完即删
    let runtime_root = dir
        .parent()
        .ok_or_else(|| PortableError::Zip("install dir 无父目录".into()))?
        .to_path_buf();
    let zip_path = runtime_root.join(&artifact);
    std::fs::create_dir_all(&dir)?;
    if zip_path.exists() {
        std::fs::remove_file(&zip_path)?; // 上次失败残留
    }

    download_to_file(&agent, &zip_url, &zip_path)?;

    // SHASUMS256 校验；不匹配 → 删除并报错（docs/RUNTIME.md §5）
    if let Err(e) = verify_shasums(&zip_path, &artifact, &shasums_url, &agent) {
        let _ = std::fs::remove_file(&zip_path);
        return Err(e);
    }

    // 覆盖旧安装：先清空再解压（解压中途失败时下次重装会再次清空）
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }

    let extracted = extract_node_zip(&zip_path, &dir);
    let _ = std::fs::remove_file(&zip_path); // 无论成败不留安装包
    let files = extracted?;

    if files == 0 {
        return Err(PortableError::Zip("归档中没有文件条目".into()));
    }

    // 与探测/手动指定走同一 validate（docs/RUNTIME.md §5）
    let node_exe = dir.join("node.exe");
    let info = validate(&node_exe, NodeSource::Portable, lister, runner)
        .map_err(|(reason, detail)| PortableError::Validate(reason, detail))?;

    let rt = super::manual::config_node_runtime(&info);
    store_node_runtime(store, rt.clone())?;
    Ok(rt)
}

/// 真实入口（lc-cli / Tauri 壳共用）。`base` 缺省官方源。
pub fn install_real(spec: &VersionSpec, base: Option<&str>) -> Result<NodeRuntime, PortableError> {
    let store = JsonConfigStore::at_default_path()?;
    let lister = super::RealDirLister;
    let runner = super::RealProcessRunner::new(super::PROBE_TIMEOUT);
    install(
        spec,
        base.unwrap_or(DEFAULT_DIST_BASE),
        &store,
        &lister,
        &runner,
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};

    use super::*;

    // ---- SHASUMS 解析 ----

    const SHASUMS: &str = "\
b8f0e60b2692b3a56ff7d1a2ff3e0a3b0aa0f0ff000000000000000000000005  node-v22.14.0-win-x64.zip
deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  node-v22.14.0-linux-x64.tar.xz
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  node-v22.14.0.tar.gz
";

    #[test]
    fn parse_shasums_official_format() {
        let entries = parse_shasums(SHASUMS);
        assert_eq!(entries.len(), 3, "{entries:?}");
        assert_eq!(entries[0].file, "node-v22.14.0-win-x64.zip");
        assert_eq!(
            entries[0].hash,
            "b8f0e60b2692b3a56ff7d1a2ff3e0a3b0aa0f0ff000000000000000000000005"
        );
        // hash 统一小写
        let mixed = parse_shasums(
            "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789  f.txt",
        );
        assert_eq!(
            mixed[0].hash,
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
    }

    #[test]
    fn parse_shasums_skips_garbage_lines() {
        let text = "\n\nshort  f.txt\nzzzz  f.txt\nok_only_hash_no_file\n  \n";
        assert!(parse_shasums(text).is_empty());
    }

    #[test]
    fn lookup_hash_hit_and_miss() {
        let entries = parse_shasums(SHASUMS);
        assert_eq!(
            lookup_hash(&entries, "node-v22.14.0-linux-x64.tar.xz"),
            Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
        );
        assert!(lookup_hash(&entries, "node-v99.0.0-win-x64.zip").is_none());
    }

    #[test]
    fn resolve_version_from_latest_shasums() {
        assert_eq!(
            resolve_version_from_shasums(SHASUMS, "-win-x64.zip"),
            Some(SemVer::new(22, 14, 0))
        );
        assert_eq!(
            resolve_version_from_shasums(SHASUMS, "-linux-x64.tar.xz"),
            Some(SemVer::new(22, 14, 0))
        );
        assert_eq!(
            resolve_version_from_shasums(SHASUMS, "-darwin-arm64.tar.gz"),
            None
        );
    }

    // ---- 哈希 ----

    #[test]
    fn hex_lower_basic() {
        assert_eq!(hex_lower(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(hex_lower(&[]), "");
    }

    #[test]
    fn sha256_known_vectors() {
        // FIPS 180-2 标准向量
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 空输入
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_reader_matches_slice_version() {
        let data = b"portable node zip payload".repeat(1000);
        let mut cursor = Cursor::new(data.clone());
        assert_eq!(sha256_hex_reader(&mut cursor).unwrap(), sha256_hex(&data));
    }

    // ---- zip-slip 防护 ----

    #[test]
    fn sanitize_accepts_normal_paths() {
        assert_eq!(
            sanitize_entry_name("node-v22.14.0-win-x64/node.exe"),
            Some(vec!["node-v22.14.0-win-x64".into(), "node.exe".into()])
        );
        // 反斜杠归一
        assert_eq!(
            sanitize_entry_name("pkg\\lib\\x.txt"),
            Some(vec!["pkg".into(), "lib".into(), "x.txt".into()])
        );
        // `.` 段与重复分隔符丢弃
        assert_eq!(
            sanitize_entry_name("a/./b//c.txt"),
            Some(vec!["a".into(), "b".into(), "c.txt".into()])
        );
        // 目录条目
        assert_eq!(sanitize_entry_name("pkg/"), Some(vec!["pkg".into()]));
    }

    #[test]
    fn sanitize_rejects_zip_slip() {
        // 上跳
        assert_eq!(sanitize_entry_name("../evil.exe"), None);
        assert_eq!(sanitize_entry_name("a/../../evil.exe"), None);
        assert_eq!(sanitize_entry_name("..\\..\\evil.exe"), None);
        // 绝对路径（含 UNC 归一化后以 / 开头）
        assert_eq!(sanitize_entry_name("/abs/evil.txt"), None);
        assert_eq!(sanitize_entry_name("\\\\server\\share\\evil.exe"), None);
        // 盘符与备用数据流
        assert_eq!(sanitize_entry_name("C:\\evil.txt"), None);
        assert_eq!(sanitize_entry_name("evil.exe:ads"), None);
        // 空与 NUL
        assert_eq!(sanitize_entry_name(""), None);
        assert_eq!(sanitize_entry_name("ev\0il"), None);
    }

    // ---- 版本参数 ----

    #[test]
    fn parse_version_arg_matrix() {
        use VersionSpec::*;
        assert_eq!(
            parse_version_arg("22.14.0"),
            Some(Exact(SemVer::new(22, 14, 0)))
        );
        assert_eq!(
            parse_version_arg("v24.15.0"),
            Some(Exact(SemVer::new(24, 15, 0)))
        );
        assert_eq!(parse_version_arg(" 22 "), Some(Latest(22)));
        assert_eq!(parse_version_arg("v22"), Some(Latest(22)));
        assert_eq!(parse_version_arg("22.14"), None, "两段不算版本");
        assert_eq!(parse_version_arg("abc"), None);
        assert_eq!(parse_version_arg(""), None);
    }

    // ---- 产物命名 ----
    // 平台差异用 cfg 拆测试而非运行时 assert!(cfg!(windows))：
    // cfg! 是编译期常量，运行时恒真/恒假会触发 clippy::assertions_on_constants；
    // clippy 提示的 const { assert!(..) } 会把平台断言变成编译期断言，Linux CI 直接编译失败，不可采用。

    #[cfg(windows)]
    #[test]
    fn artifact_name_matches_platform_windows() {
        let name = artifact_name(SemVer::new(22, 14, 0)).expect("Windows 上应成功");
        let tag = win_arch_tag().unwrap();
        assert_eq!(name, format!("node-v22.14.0-win-{tag}.zip"));
        assert!(tag == "x64" || tag == "arm64", "tag={tag}");
    }

    #[cfg(not(windows))]
    #[test]
    fn artifact_name_matches_platform_unsupported() {
        let e = artifact_name(SemVer::new(22, 14, 0))
            .expect_err("非 Windows 应返回 UnsupportedPlatform");
        assert!(matches!(e, PortableError::UnsupportedPlatform(_)));
    }

    // ---- 解压（本地构造真实 zip，不访问网络）----

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("lc-portable-test-{}-{tag}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_zip(path: &Path, files: &[(&str, &[u8])], dirs: &[&str]) {
        let f = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        for d in dirs {
            w.add_directory(*d, zip::write::SimpleFileOptions::default())
                .unwrap();
        }
        for (name, data) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn extract_strips_common_root() {
        let dir = TempDir::new("extract");
        let zip_path = dir.0.join("good.zip");
        let dest = dir.0.join("out");
        write_zip(
            &zip_path,
            &[
                ("node-v22.14.0-win-x64/node.exe", b"fake node"),
                ("node-v22.14.0-win-x64/README.txt", b"readme"),
                ("node-v22.14.0-win-x64/lib/a.txt", b"a"),
            ],
            &["node-v22.14.0-win-x64/"],
        );

        let n = extract_node_zip(&zip_path, &dest).unwrap();
        assert_eq!(n, 3);
        // 根目录被剥离：node.exe 直接落在 dest 下（docs/RUNTIME.md §5 布局）
        assert_eq!(std::fs::read(dest.join("node.exe")).unwrap(), b"fake node");
        assert_eq!(std::fs::read(dest.join("README.txt")).unwrap(), b"readme");
        assert_eq!(std::fs::read(dest.join("lib").join("a.txt")).unwrap(), b"a");
    }

    #[test]
    fn extract_rejects_parent_escape() {
        let dir = TempDir::new("zipslip");
        let zip_path = dir.0.join("evil.zip");
        let dest = dir.0.join("out");
        write_zip(
            &zip_path,
            &[("pkg/ok.txt", b"ok"), ("../evil.exe", b"evil")],
            &[],
        );

        let err = extract_node_zip(&zip_path, &dest).unwrap_err();
        assert!(
            matches!(err, PortableError::ZipSlip(ref n) if n == "../evil.exe"),
            "{err}"
        );
        // 恶意文件未逃逸出 dest
        assert!(!dir.0.join("evil.exe").exists(), "zip-slip 逃逸！");
        assert!(!dest.join("evil.exe").exists());
    }

    #[test]
    fn extract_rejects_absolute_and_drive_paths() {
        let dir = TempDir::new("abs");
        for evil in ["/abs/evil.txt", "C:\\evil.txt"] {
            let zip_path = dir.0.join("evil.zip");
            let dest = dir.0.join("out2");
            write_zip(&zip_path, &[(evil, b"x")], &[]);
            let err = extract_node_zip(&zip_path, &dest).unwrap_err();
            assert!(matches!(err, PortableError::ZipSlip(_)), "{evil}: {err}");
            let _ = std::fs::remove_file(&zip_path);
            let _ = std::fs::remove_dir_all(&dest);
        }
    }

    #[test]
    fn extract_no_common_root_keeps_layout() {
        let dir = TempDir::new("noroot");
        let zip_path = dir.0.join("flat.zip");
        let dest = dir.0.join("out");
        write_zip(
            &zip_path,
            &[("loose.txt", b"l"), ("sub/inner.txt", b"i")],
            &[],
        );
        let n = extract_node_zip(&zip_path, &dest).unwrap();
        assert_eq!(n, 2);
        assert!(dest.join("loose.txt").exists());
        assert!(dest.join("sub").join("inner.txt").exists());
    }

    #[test]
    fn extract_corrupt_zip_reports_error() {
        let dir = TempDir::new("corrupt");
        let zip_path = dir.0.join("bad.zip");
        std::fs::write(&zip_path, b"this is not a zip").unwrap();
        let err = extract_node_zip(&zip_path, &dir.0.join("out")).unwrap_err();
        assert!(matches!(err, PortableError::Zip(_)), "{err}");
    }

    // ---- install_dir 布局 ----

    #[test]
    fn install_dir_is_data_dir_runtime_node() {
        let dir = install_dir().unwrap();
        let data = data_dir().unwrap();
        assert_eq!(dir, data.join("runtime").join("node"));
    }
}
