//! 更新检查通道（docs/RELEASE-GATES.md A 类第 6 项，docs/UPDATER.md）。
//!
//! 只做「读静态清单 + 版本比较」：不下载、不安装、不执行任何远端代码。
//! 清单由 `scripts/gen-update-manifest.mjs` 生成，托管在固定 URL
//! （release 资产 `updater-manifest/latest.json`，可用 `LC_UPDATE_MANIFEST_URL`
//! 覆盖，便于本地/测试）。签名状态原样透传给前端；没有签名时 `signed=false`，
//! 是否安装由 UI 与后续签名校验决定，本模块不做自动更新。

use std::collections::BTreeMap;
use std::time::Duration;

use lc_proto::{ErrorBody, ErrorCode};
use serde::{Deserialize, Serialize};

/// 固定清单地址：滚动 release 的资产，不受 GitHub `latest` 忽略 prerelease 的影响。
/// 仓库迁移时只改这一处；发布流程见 docs/UPDATER.md。
pub(crate) const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/OpenALC/lite-code/releases/download/updater-manifest/latest.json";

/// 清单响应上限：正常清单只有几 KB，避免恶意/错误响应撑爆内存。
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// 当前支持的平台键（与 Tauri updater 命名一致）。
pub(crate) const SUPPORTED_PLATFORMS: [&str; 3] =
    ["windows-x86_64", "linux-x86_64", "linux-aarch64"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlatformAsset {
    pub url: String,
    /// 空字符串与缺失等价：生成器在未签名时写 `""`。
    #[serde(default)]
    pub signature: Option<String>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PreviousRelease {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub platforms: BTreeMap<String, PlatformAsset>,
}

/// 静态清单 `latest.json` 的线格式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    /// 全局签名声明；为 true 时选中平台的签名必须存在。
    #[serde(default)]
    pub signed: bool,
    #[serde(default)]
    pub platforms: BTreeMap<String, PlatformAsset>,
    /// 回滚通道：保留上一版本的版本号与安装包 URL（不自动降级）。
    #[serde(default)]
    pub previous: Option<PreviousRelease>,
}

/// 回滚目标（已按当前平台解析）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RollbackInfo {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub signature: Option<String>,
    pub sha256: String,
}

/// `update_check` 的返回体。`available=false` 表示当前已是最新或清单更旧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct UpdateInfo {
    pub current_version: String,
    pub available: bool,
    pub version: String,
    pub notes: String,
    pub pub_date: String,
    pub url: String,
    pub signature: Option<String>,
    pub sha256: String,
    /// 选中平台安装包是否带签名；不代表客户端已完成密码学校验。
    pub signed: bool,
    pub previous: Option<RollbackInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateError {
    Http(String),
    InvalidManifest(String),
    InvalidVersion(String),
    UnsupportedPlatform,
    PlatformMissing {
        platform: String,
        available: Vec<String>,
    },
    /// 清单声明 signed=true，但选中平台没有签名。
    SignatureClaimMissing {
        platform: String,
    },
    InvalidAsset {
        platform: String,
        reason: String,
    },
    InvalidUrl {
        url: String,
    },
}

impl UpdateError {
    pub(crate) fn body(self) -> ErrorBody {
        match self {
            Self::Http(message) => {
                ErrorBody::new(ErrorCode::Internal, format!("更新检查失败：{message}"))
            }
            Self::InvalidManifest(message) => ErrorBody::new(
                ErrorCode::ContractMismatch,
                format!("更新清单无效：{message}"),
            ),
            Self::InvalidVersion(version) => ErrorBody::new(
                ErrorCode::ContractMismatch,
                format!("更新清单版本号无效：{version}"),
            ),
            Self::UnsupportedPlatform => ErrorBody::new(
                ErrorCode::NotImplemented,
                "当前平台不支持更新检查（仅 Windows x64 / Linux x64 / Linux arm64）",
            ),
            Self::PlatformMissing {
                platform,
                available,
            } => ErrorBody::new(
                ErrorCode::NotFound,
                format!("更新清单缺少平台 {platform} 的安装包"),
            )
            .with_detail(if available.is_empty() {
                "清单 platforms 为空".to_string()
            } else {
                format!("可用平台：{}", available.join(", "))
            }),
            Self::SignatureClaimMissing { platform } => ErrorBody::new(
                ErrorCode::ContractMismatch,
                format!("清单声明已签名，但平台 {platform} 的安装包缺少 signature"),
            ),
            Self::InvalidAsset { platform, reason } => ErrorBody::new(
                ErrorCode::ContractMismatch,
                format!("更新清单中平台 {platform} 的安装包无效：{reason}"),
            ),
            Self::InvalidUrl { url } => ErrorBody::new(
                ErrorCode::InvalidArgument,
                format!("更新 URL 必须是 https（仅回环地址允许 http）：{url}"),
            ),
        }
    }
}

/// 清单 URL：显式环境变量优先，否则用固定滚动地址。
pub(crate) fn manifest_url() -> String {
    std::env::var("LC_UPDATE_MANIFEST_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_string())
}

pub(crate) fn current_platform_key() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-aarch64")
    } else {
        None
    }
}

/// 拉取并解析清单，再按当前平台生成 `UpdateInfo`。阻塞调用，命令层用
/// `spawn_blocking` 包住，避免卡 WebView。
pub(crate) fn check_url(
    url: &str,
    current_version: &str,
    platform: &str,
) -> Result<UpdateInfo, UpdateError> {
    if !is_https_or_loopback(url) {
        return Err(UpdateError::InvalidUrl {
            url: url.to_string(),
        });
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build();
    let agent = ureq::Agent::new_with_config(agent);
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_MANIFEST_BYTES)
        .read_to_string()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    let manifest = parse_manifest(&body)?;
    check_manifest(&manifest, current_version, platform)
}

pub(crate) fn parse_manifest(json: &str) -> Result<UpdateManifest, UpdateError> {
    serde_json::from_str(json).map_err(|e| UpdateError::InvalidManifest(e.to_string()))
}

/// 纯逻辑：比较版本、选平台、校验 URL/sha256/签名声明、解析回滚目标。
pub(crate) fn check_manifest(
    manifest: &UpdateManifest,
    current_version: &str,
    platform: &str,
) -> Result<UpdateInfo, UpdateError> {
    if !SUPPORTED_PLATFORMS.contains(&platform) {
        return Err(UpdateError::UnsupportedPlatform);
    }
    let current = Version::parse(current_version)
        .ok_or_else(|| UpdateError::InvalidVersion(current_version.to_string()))?;
    let latest = Version::parse(&manifest.version)
        .ok_or_else(|| UpdateError::InvalidVersion(manifest.version.clone()))?;
    let asset = manifest
        .platforms
        .get(platform)
        .ok_or_else(|| UpdateError::PlatformMissing {
            platform: platform.to_string(),
            available: manifest.platforms.keys().cloned().collect(),
        })?;
    validate_asset(platform, asset)?;
    let signature = normalize_signature(asset.signature.as_deref());
    if manifest.signed && signature.is_none() {
        return Err(UpdateError::SignatureClaimMissing {
            platform: platform.to_string(),
        });
    }

    let previous = match &manifest.previous {
        Some(previous) => rollback_info(previous, &latest, platform)?,
        None => None,
    };

    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        available: latest > current,
        version: manifest.version.clone(),
        notes: manifest.notes.clone(),
        pub_date: manifest.pub_date.clone(),
        url: asset.url.clone(),
        signed: signature.is_some(),
        signature,
        sha256: asset.sha256.clone(),
        previous,
    })
}

fn rollback_info(
    previous: &PreviousRelease,
    latest: &Version,
    platform: &str,
) -> Result<Option<RollbackInfo>, UpdateError> {
    let version = Version::parse(&previous.version)
        .ok_or_else(|| UpdateError::InvalidVersion(previous.version.clone()))?;
    if version >= *latest {
        return Err(UpdateError::InvalidManifest(format!(
            "previous 版本 {} 不低于最新版本",
            previous.version
        )));
    }
    // 上一版可能还没有当前平台的产物（例如 arm64 首版），此时没有可用回滚目标，
    // 但不阻断对最新版的检查。
    let Some(asset) = previous.platforms.get(platform) else {
        return Ok(None);
    };
    validate_asset(platform, asset)?;
    Ok(Some(RollbackInfo {
        version: previous.version.clone(),
        notes: previous.notes.clone(),
        url: asset.url.clone(),
        signature: normalize_signature(asset.signature.as_deref()),
        sha256: asset.sha256.clone(),
    }))
}

fn validate_asset(platform: &str, asset: &PlatformAsset) -> Result<(), UpdateError> {
    if !is_https_or_loopback(&asset.url) {
        return Err(UpdateError::InvalidUrl {
            url: asset.url.clone(),
        });
    }
    if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(UpdateError::InvalidAsset {
            platform: platform.to_string(),
            reason: "sha256 必须是 64 位十六进制".to_string(),
        });
    }
    Ok(())
}

fn normalize_signature(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 只允许 https；回环地址允许 http，便于本地清单服务器与测试。
fn is_https_or_loopback(url: &str) -> bool {
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

// ==== SemVer（只实现更新比较需要的部分，不引新 crate） ====

#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<PreId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreId {
    Numeric(u64),
    Alpha(String),
}

impl Version {
    fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        // build metadata 不参与比较，但必须语法合法。
        let (rest, build) = match raw.split_once('+') {
            Some((rest, build)) => (rest, Some(build)),
            None => (raw, None),
        };
        if let Some(build) = build
            && (build.is_empty()
                || !build
                    .split('.')
                    .all(|part| !part.is_empty() && is_identifier(part)))
        {
            return None;
        }
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };
        let mut parts = core.split('.');
        let major = parse_core(parts.next()?)?;
        let minor = parse_core(parts.next()?)?;
        let patch = parse_core(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some(pre) => {
                let mut ids = Vec::new();
                for part in pre.split('.') {
                    if part.is_empty() || !is_identifier(part) {
                        return None;
                    }
                    if part.bytes().all(|b| b.is_ascii_digit()) {
                        if part.len() > 1 && part.starts_with('0') {
                            return None;
                        }
                        ids.push(PreId::Numeric(part.parse().ok()?));
                    } else {
                        ids.push(PreId::Alpha(part.to_string()));
                    }
                }
                ids
            }
            None => Vec::new(),
        };
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

fn parse_core(raw: &str) -> Option<u64> {
    if raw.is_empty()
        || !raw.bytes().all(|b| b.is_ascii_digit())
        || (raw.len() > 1 && raw.starts_with('0'))
    {
        return None;
    }
    raw.parse().ok()
}

fn is_identifier(raw: &str) -> bool {
    raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                (false, false) => cmp_prerelease(&self.pre, &other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn cmp_prerelease(left: &[PreId], right: &[PreId]) -> std::cmp::Ordering {
    for (a, b) in left.iter().zip(right) {
        let ordering = match (a, b) {
            (PreId::Numeric(a), PreId::Numeric(b)) => a.cmp(b),
            (PreId::Numeric(_), PreId::Alpha(_)) => std::cmp::Ordering::Less,
            (PreId::Alpha(_), PreId::Numeric(_)) => std::cmp::Ordering::Greater,
            (PreId::Alpha(a), PreId::Alpha(b)) => a.cmp(b),
        };
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use lc_proto::ErrorCode;

    use super::{
        PlatformAsset, PreviousRelease, SUPPORTED_PLATFORMS, UpdateError, UpdateManifest, Version,
        check_manifest, check_url, current_platform_key, is_https_or_loopback, parse_manifest,
    };

    fn asset(url: &str, signature: Option<&str>) -> PlatformAsset {
        PlatformAsset {
            url: url.to_string(),
            signature: signature.map(str::to_string),
            sha256: "a".repeat(64),
        }
    }

    fn manifest(signed: bool, signature: Option<&str>) -> UpdateManifest {
        let mut platforms = BTreeMap::new();
        platforms.insert(
            "windows-x86_64".to_string(),
            asset(
                "https://github.com/OpenALC/lite-code/releases/download/v0.6.1-preview.1/Lite.Code_0.6.1-preview.1_x64-setup.exe",
                signature,
            ),
        );
        UpdateManifest {
            version: "0.6.1-preview.1".to_string(),
            notes: "preview".to_string(),
            pub_date: "2026-11-05T00:00:00Z".to_string(),
            signed,
            platforms,
            previous: None,
        }
    }

    #[test]
    fn semver_ordering_handles_prerelease_and_build_metadata() {
        let parse = |v: &str| Version::parse(v).unwrap_or_else(|| panic!("parse {v}"));
        assert!(parse("0.6.1-preview.1") > parse("0.5.1-preview.1"));
        assert!(parse("0.6.1-preview.2") > parse("0.6.1-preview.1"));
        assert!(parse("0.6.1") > parse("0.6.1-preview.9"));
        assert!(parse("0.6.1-preview.10") > parse("0.6.1-preview.9"));
        assert_eq!(parse("0.6.1-preview.1+build.7"), parse("0.6.1-preview.1"));
        assert!(parse("1.0.0-alpha") < parse("1.0.0-alpha.1"));
        assert!(Version::parse("0.6").is_none());
        assert!(Version::parse("v0.6.1").is_none());
        assert!(Version::parse("0.6.1-preview.01").is_none());
    }

    #[test]
    fn manifest_parses_full_and_unsigned_payloads() {
        let json = r#"{
          "version": "0.6.1-preview.1",
          "notes": "P6 preview",
          "pub_date": "2026-11-05T00:00:00Z",
          "signed": false,
          "platforms": {
            "windows-x86_64": {
              "url": "https://example.com/Lite%20Code.exe",
              "signature": "",
              "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }
          }
        }"#;
        let parsed = parse_manifest(json).expect("manifest parses");
        assert_eq!(parsed.version, "0.6.1-preview.1");
        assert!(!parsed.signed);
        assert_eq!(
            parsed.platforms["windows-x86_64"].signature.as_deref(),
            Some("")
        );

        let info = check_manifest(&parsed, "0.5.1-preview.1", "windows-x86_64").unwrap();
        assert!(info.available);
        assert!(!info.signed);
        assert_eq!(info.signature, None);
        assert_eq!(
            info.sha256,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn signed_manifest_requires_per_platform_signature() {
        let signed = manifest(true, Some("minisign-signature"));
        let info = check_manifest(&signed, "0.5.1-preview.1", "windows-x86_64").unwrap();
        assert!(info.signed);
        assert_eq!(info.signature.as_deref(), Some("minisign-signature"));

        let claimed = manifest(true, None);
        let err = check_manifest(&claimed, "0.5.1-preview.1", "windows-x86_64")
            .expect_err("signed claim without signature is rejected");
        assert_eq!(err.body().code, ErrorCode::ContractMismatch);

        // signed=false 但实际带签名：按签名存在处理，前端不会误判为可执行未签名包。
        let actual = manifest(false, Some("sig"));
        assert!(
            check_manifest(&actual, "0.5.1-preview.1", "windows-x86_64")
                .unwrap()
                .signed
        );
    }

    #[test]
    fn unsigned_and_older_manifests_report_no_update() {
        let manifest = manifest(false, None);
        let equal = check_manifest(&manifest, "0.6.1-preview.1", "windows-x86_64").unwrap();
        assert!(!equal.available);
        let newer_current = check_manifest(&manifest, "0.7.0", "windows-x86_64").unwrap();
        assert!(!newer_current.available);
    }

    #[test]
    fn previous_release_exposes_rollback_url_for_current_platform() {
        let mut current = manifest(true, Some("new-sig"));
        let mut platforms = BTreeMap::new();
        platforms.insert(
            "windows-x86_64".to_string(),
            asset(
                "https://github.com/OpenALC/lite-code/releases/download/v0.6.0-preview.1/Lite.Code_0.6.0-preview.1_x64-setup.exe",
                Some("old-sig"),
            ),
        );
        current.previous = Some(PreviousRelease {
            version: "0.6.0-preview.1".to_string(),
            notes: "previous".to_string(),
            platforms,
        });

        let info = check_manifest(&current, "0.5.1-preview.1", "windows-x86_64").unwrap();
        let rollback = info.previous.expect("rollback target");
        assert_eq!(rollback.version, "0.6.0-preview.1");
        assert!(rollback.url.contains("v0.6.0-preview.1"));
        assert_eq!(rollback.signature.as_deref(), Some("old-sig"));

        // 上一版没有当前平台产物时不阻断检查，只是没有回滚目标。
        let mut only_x64 = current.clone();
        only_x64.previous = Some(PreviousRelease {
            version: "0.6.0-preview.1".to_string(),
            notes: String::new(),
            platforms: BTreeMap::new(),
        });
        // 平台不存在时 check_manifest 先返回 PlatformMissing；给 linux 平台塞一个当前版本资产。
        only_x64.platforms.insert(
            "linux-aarch64".to_string(),
            asset("https://example.com/aarch64.AppImage", Some("sig")),
        );
        assert!(
            check_manifest(&only_x64, "0.5.1-preview.1", "linux-aarch64")
                .unwrap()
                .previous
                .is_none()
        );
    }

    #[test]
    fn platform_and_asset_validation_is_explicit() {
        let mut missing = manifest(false, None);
        missing.platforms.clear();
        let err = check_manifest(&missing, "0.5.1-preview.1", "windows-x86_64")
            .expect_err("missing platform");
        assert_eq!(err.body().code, ErrorCode::NotFound);

        let mut bad_hash = manifest(false, None);
        bad_hash.platforms.get_mut("windows-x86_64").unwrap().sha256 = "not-a-hash".to_string();
        let err =
            check_manifest(&bad_hash, "0.5.1-preview.1", "windows-x86_64").expect_err("bad hash");
        assert_eq!(err.body().code, ErrorCode::ContractMismatch);

        assert_eq!(
            UpdateError::UnsupportedPlatform.body().code,
            ErrorCode::NotImplemented
        );
        assert_eq!(
            UpdateError::Http("offline".into()).body().code,
            ErrorCode::Internal
        );
    }

    #[test]
    fn urls_require_https_except_loopback() {
        assert!(is_https_or_loopback("https://github.com/a/b"));
        assert!(is_https_or_loopback("http://127.0.0.1:8080/latest.json"));
        assert!(is_https_or_loopback("http://localhost/latest.json"));
        assert!(is_https_or_loopback("http://[::1]:8080/latest.json"));
        assert!(!is_https_or_loopback("http://example.com/latest.json"));
        assert!(!is_https_or_loopback("ftp://example.com/latest.json"));
        assert!(!is_https_or_loopback("https:///latest.json"));
    }

    #[test]
    fn platform_keys_are_the_documented_set() {
        assert_eq!(SUPPORTED_PLATFORMS.len(), 3);
        if let Some(key) = current_platform_key() {
            assert!(SUPPORTED_PLATFORMS.contains(&key));
        }
    }

    #[test]
    fn check_url_fetches_loopback_manifest() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let body = r#"{
              "version": "0.6.1-preview.1",
              "notes": "loopback",
              "pub_date": "2026-11-05T00:00:00Z",
              "signed": false,
              "platforms": {
                "windows-x86_64": {
                  "url": "https://example.com/Lite.Code.exe",
                  "signature": "",
                  "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }
              }
            }"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write response");
        });

        let info = check_url(
            &format!("http://127.0.0.1:{port}/latest.json"),
            "0.5.1-preview.1",
            "windows-x86_64",
        )
        .expect("loopback manifest");
        server.join().unwrap();
        assert!(info.available);
        assert!(!info.signed);
        assert_eq!(info.version, "0.6.1-preview.1");
    }

    #[test]
    fn non_https_manifest_is_rejected_before_network() {
        let err = check_url(
            "http://example.com/latest.json",
            "0.5.1-preview.1",
            "windows-x86_64",
        )
        .expect_err("plain http is rejected");
        assert_eq!(err.body().code, ErrorCode::InvalidArgument);
    }
}
