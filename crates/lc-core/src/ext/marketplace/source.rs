//! 源清单与多源拉取（docs/plans/plugin-marketplace.md §3.3）。
//!
//! - 源清单 `~/.lite-code/plugin-registries.json`（write_atomic 原子写，未知
//!   顶层字段保留，同 ext state 先例）；builtin 源由调用方（桌面层）注入，
//!   文件内同名条目可覆盖其 `enabled`/`require_signatures`/`pubkey`
//!   ——即「builtin 不可删除、可禁用」；
//! - URL 白名单：https / 回环 http / `file://`（file 源 UI 须标「本地源」）；
//! - `ext_search` 并行拉取全部启用源（std thread per source，per-source
//!   10s 超时、总预算 30s），TTL 5 分钟内存缓存（`refresh=true` 强制），
//!   按 id 去重取 semver 最高；失败源不毒化结果，记入 `staleSources`；
//! - M3 多镜像：源可配 `mirrors`（同一 index 的备选 URL），index 拉取按
//!   `[url, mirrors…]` 顺序回退（per-mirror 超时 = per-source 10s）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::index::{
    IndexEntry, MAX_INDEX_BYTES, MarketPlugin, MarketSearchResult, ParsedIndex, SignatureState,
    parse_index,
};
use super::{MarketError, file_url_to_path, is_allowed_url, is_file_url, is_https_or_loopback};
use crate::ext::{ExtPaths, PluginList};
use crate::version::Version;

/// 源清单文件名（`~/.lite-code/plugin-registries.json`）。
pub const REGISTRIES_FILE_NAME: &str = "plugin-registries.json";
/// 内置源的约定名字（桌面层注入 builtin 源时使用）。
pub const BUILTIN_SOURCE_NAME: &str = "official";

/// 拉取约束：生产值见各常量；测试用 `FetchLimits` 注入小值压缩用例时间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchLimits {
    /// 内存缓存 TTL（默认 5 分钟）。
    pub cache_ttl: Duration,
    /// 单源超时（默认 10s）。
    pub per_source: Duration,
    /// 整体搜索预算（默认 30s）；超时未回的源记入 staleSources。
    pub total_budget: Duration,
}

impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            cache_ttl: SOURCE_CACHE_TTL,
            per_source: PER_SOURCE_TIMEOUT,
            total_budget: SEARCH_BUDGET,
        }
    }
}

/// 内存缓存 TTL（§3.3）。
pub const SOURCE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// per-source 超时（§3.3，同 updater 的 HTTP_TIMEOUT）。
pub const PER_SOURCE_TIMEOUT: Duration = Duration::from_secs(10);
/// 整体搜索预算（§3.3）。
pub const SEARCH_BUDGET: Duration = Duration::from_secs(30);

/// 单个插件源。文件键为 snake_case（`require_signatures`/`pubkey`/`mirrors`/
/// `enabled`），与 §3.3 线格式一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RegistrySource {
    pub name: String,
    /// 权威 index URL；`mirrors` 是其同内容镜像，拉取按 `[url, mirrors…]` 顺序回退。
    pub url: String,
    /// M3 强制档开关：true 时安装必验 minisign 签名（§3.7，见 install.rs）。
    #[serde(default)]
    pub require_signatures: bool,
    /// minisign 公钥（base64 的 42 字节线格式 blob），仅来自本地配置，
    /// 禁止网络获取（无 TOFU）。
    #[serde(default)]
    pub pubkey: String,
    /// 同一 index 的镜像 URL 列表（M3，§3.3）：与 `url` 等价地位，按序回退。
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl RegistrySource {
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            require_signatures: false,
            pubkey: String::new(),
            mirrors: Vec::new(),
            enabled: true,
        }
    }

    /// index 拉取候选 URL：权威 `url` 在前，`mirrors` 按配置顺序随后
    /// （§3.3「同一 index 可托管多份镜像 URL，回退顺序=列表顺序」）。
    pub fn index_urls(&self) -> Vec<&str> {
        std::iter::once(self.url.as_str())
            .chain(self.mirrors.iter().map(String::as_str))
            .collect()
    }
}

/// `ext_registry_list` 的 `sources` 元素（custom DTO camelCase）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrySourceInfo {
    pub name: String,
    pub url: String,
    /// 桌面层注入的 builtin 源（不可删除；文件内同名条目可禁用它）。
    pub builtin: bool,
    pub require_signatures: bool,
    /// `pubkey` 非空（UI 据此展示是否 pinned 公钥）。
    pub has_pubkey: bool,
    /// 同一 index 的镜像 URL 列表（M3 多镜像回退，§3.3）；无镜像为空数组。
    pub mirrors: Vec<String>,
}

/// `ext_registry_list` / `ext_registry_add` / `ext_registry_remove` 返回体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryList {
    pub sources: Vec<RegistrySourceInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    sources: Vec<RegistrySource>,
    #[serde(flatten, default)]
    extra: Map<String, Value>,
}

/// 源清单默认路径 `~/.lite-code/plugin-registries.json`。
pub fn registries_path() -> Result<PathBuf, MarketError> {
    let home = dirs::home_dir().ok_or(MarketError::NoHomeDir)?;
    Ok(home
        .join(crate::config::APP_DIR_NAME)
        .join(REGISTRIES_FILE_NAME))
}

pub(crate) fn load_file(path: &Path) -> Result<Vec<RegistrySource>, MarketError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(MarketError::Config(format!(
                "读取 {} 失败：{error}",
                path.display()
            )));
        }
    };
    let file: RegistryFile = serde_json::from_str(&text)
        .map_err(|error| MarketError::Config(format!("{} 解析失败：{error}", path.display())))?;
    Ok(file.sources)
}

fn save_file(path: &Path, sources: &[RegistrySource]) -> Result<(), MarketError> {
    let mut file = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<RegistryFile>(&text).map_err(|error| {
            MarketError::Config(format!("{} 解析失败：{error}", path.display()))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => RegistryFile::default(),
        Err(error) => {
            return Err(MarketError::Config(format!(
                "读取 {} 失败：{error}",
                path.display()
            )));
        }
    };
    file.sources = sources.to_vec();
    let mut text = serde_json::to_string_pretty(&file)
        .map_err(|error| MarketError::Config(format!("序列化源清单失败：{error}")))?;
    text.push('\n');
    crate::config::write_atomic(path, &text)
        .map_err(|error| MarketError::Io(format!("写入 {} 失败：{error}", path.display())))
}

/// builtin 源 + 文件源合并：文件内与 builtin 同名的条目覆盖其
/// `enabled`/`require_signatures`/`pubkey`（URL、`mirrors` 非空时也覆盖），
/// 实现「builtin 不可删除、可禁用」。
pub(crate) fn merged_sources(
    builtin: &RegistrySource,
    file_sources: &[RegistrySource],
) -> Vec<RegistrySource> {
    let mut merged = vec![builtin.clone()];
    for source in file_sources {
        match merged.iter_mut().find(|slot| slot.name == source.name) {
            Some(slot) => {
                if !source.url.trim().is_empty() {
                    slot.url = source.url.clone();
                }
                slot.enabled = source.enabled;
                slot.require_signatures = source.require_signatures;
                slot.pubkey = source.pubkey.clone();
                if !source.mirrors.is_empty() {
                    slot.mirrors = source.mirrors.clone();
                }
            }
            None => merged.push(source.clone()),
        }
    }
    merged
}

fn source_info(source: &RegistrySource, builtin_name: &str) -> RegistrySourceInfo {
    RegistrySourceInfo {
        name: source.name.clone(),
        url: source.url.clone(),
        builtin: source.name == builtin_name,
        require_signatures: source.require_signatures,
        has_pubkey: !source.pubkey.trim().is_empty(),
        mirrors: source.mirrors.clone(),
    }
}

/// 列出全部源（builtin 在首位）。
pub fn list_registries(builtin: &RegistrySource) -> Result<RegistryList, MarketError> {
    list_registries_at(&registries_path()?, builtin)
}

fn list_registries_at(path: &Path, builtin: &RegistrySource) -> Result<RegistryList, MarketError> {
    let file_sources = load_file(path)?;
    Ok(RegistryList {
        sources: merged_sources(builtin, &file_sources)
            .iter()
            .map(|source| source_info(source, &builtin.name))
            .collect(),
    })
}

/// 新增源。URL 走白名单校验；builtin 同名拒绝覆盖；M3：pubkey 合法性、
/// 强制档必配 pubkey、镜像 URL 白名单均在入库前拦截（invalid_argument）。
pub fn add_registry(
    builtin: &RegistrySource,
    name: &str,
    url: &str,
    require_signatures: bool,
    pubkey: Option<&str>,
    mirrors: Option<&[String]>,
) -> Result<RegistryList, MarketError> {
    add_registry_at(
        &registries_path()?,
        builtin,
        name,
        url,
        require_signatures,
        pubkey,
        mirrors,
    )
}

/// pubkey 入库前校验（§3.7）：强制档必须配置；非空则必须是 base64 的
/// minisign 公钥线格式（解码后 42 字节 = 算法标识 2 + 密钥号 8 + Ed25519
/// 公钥 32）。只来自本地配置，禁止网络获取（无 TOFU）。
fn validate_pubkey(require_signatures: bool, pubkey: Option<&str>) -> Result<String, MarketError> {
    let trimmed = pubkey.unwrap_or_default().trim();
    if require_signatures && trimmed.is_empty() {
        return Err(MarketError::InvalidArgument(
            "强制档（require_signatures=true）必须配置 pubkey（本地 pinned 的 minisign 公钥）"
                .to_string(),
        ));
    }
    if !trimmed.is_empty() && minisign_verify::PublicKey::from_base64(trimmed).is_err() {
        return Err(MarketError::InvalidArgument(
            "pubkey 非法：须为 base64 编码的 minisign 公钥（解码后 42 字节：算法标识 2 + 密钥号 8 + Ed25519 公钥 32）".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

/// 镜像 URL 入库前校验：与源 URL 同一白名单（§3.3 多镜像）。
fn validate_mirrors(mirrors: Option<&[String]>) -> Result<Vec<String>, MarketError> {
    let mut out = Vec::new();
    for mirror in mirrors.unwrap_or_default() {
        let mirror = mirror.trim();
        if !is_allowed_url(mirror) {
            return Err(MarketError::InvalidArgument(format!(
                "镜像 URL 必须是 https（回环允许 http）或 file://：{mirror}"
            )));
        }
        out.push(mirror.to_string());
    }
    Ok(out)
}

fn add_registry_at(
    path: &Path,
    builtin: &RegistrySource,
    name: &str,
    url: &str,
    require_signatures: bool,
    pubkey: Option<&str>,
    mirrors: Option<&[String]>,
) -> Result<RegistryList, MarketError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 || name.chars().any(char::is_whitespace) {
        return Err(MarketError::InvalidArgument(format!(
            "源名 `{name}` 非法（非空、≤64 字符、不含空白）"
        )));
    }
    if name == builtin.name {
        return Err(MarketError::InvalidArgument(format!(
            "builtin 源 `{}` 不可被同名源覆盖（可禁用它）",
            builtin.name
        )));
    }
    let url = url.trim();
    if !is_allowed_url(url) {
        return Err(MarketError::InvalidArgument(format!(
            "源 URL 必须是 https（回环允许 http）或 file://：{url}"
        )));
    }
    let pubkey = validate_pubkey(require_signatures, pubkey)?;
    let mirrors = validate_mirrors(mirrors)?;
    let mut file_sources = load_file(path)?;
    if file_sources.iter().any(|source| source.name == name) {
        return Err(MarketError::InvalidArgument(format!("源 `{name}` 已存在")));
    }
    file_sources.push(RegistrySource {
        name: name.to_string(),
        url: url.to_string(),
        require_signatures,
        pubkey,
        mirrors,
        enabled: true,
    });
    save_file(path, &file_sources)?;
    list_registries_at(path, builtin)
}

/// 移除源；builtin 源拒绝删除（§3.3）。
pub fn remove_registry(builtin: &RegistrySource, name: &str) -> Result<RegistryList, MarketError> {
    remove_registry_at(&registries_path()?, builtin, name)
}

fn remove_registry_at(
    path: &Path,
    builtin: &RegistrySource,
    name: &str,
) -> Result<RegistryList, MarketError> {
    if name == builtin.name {
        return Err(MarketError::InvalidArgument(format!(
            "builtin 源 `{}` 不可删除（可禁用它）",
            builtin.name
        )));
    }
    let mut file_sources = load_file(path)?;
    let before = file_sources.len();
    file_sources.retain(|source| source.name != name);
    if file_sources.len() == before {
        return Err(MarketError::NotFound(format!("源 `{name}` 不存在")));
    }
    save_file(path, &file_sources)?;
    list_registries_at(path, builtin)
}

/// 启用/禁用源。builtin 名以覆盖条目形式写入文件（不可删除但可禁用）。
/// M1 命令面未暴露，留作 UI 与测试缝。
pub fn set_registry_enabled(
    builtin: &RegistrySource,
    name: &str,
    enabled: bool,
) -> Result<RegistryList, MarketError> {
    set_registry_enabled_at(&registries_path()?, builtin, name, enabled)
}

fn set_registry_enabled_at(
    path: &Path,
    builtin: &RegistrySource,
    name: &str,
    enabled: bool,
) -> Result<RegistryList, MarketError> {
    let mut file_sources = load_file(path)?;
    if name == builtin.name {
        match file_sources.iter_mut().find(|source| source.name == name) {
            Some(slot) => slot.enabled = enabled,
            None => {
                let mut overlay = builtin.clone();
                overlay.enabled = enabled;
                file_sources.push(overlay);
            }
        }
    } else {
        let slot = file_sources
            .iter_mut()
            .find(|source| source.name == name)
            .ok_or_else(|| MarketError::NotFound(format!("源 `{name}` 不存在")))?;
        slot.enabled = enabled;
    }
    save_file(path, &file_sources)?;
    list_registries_at(path, builtin)
}

// ==== 拉取与缓存 ====

struct CachedIndex {
    fetched_at: Instant,
    index: ParsedIndex,
}

/// TTL 内存缓存（按源 URL 键控；`refresh=true` 强制重拉并刷新）。
#[derive(Default)]
pub struct IndexCache {
    map: HashMap<String, CachedIndex>,
}

impl IndexCache {
    pub fn new() -> Self {
        Self::default()
    }
}

/// 全局缓存（OnceLock：const 上下文不依赖 `HashMap::new` 的 const 稳定性）。
pub(crate) fn global_cache() -> &'static Mutex<IndexCache> {
    static CACHE: OnceLock<Mutex<IndexCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(IndexCache::new()))
}

fn cache_get(
    cache: &Mutex<IndexCache>,
    url: &str,
    now: Instant,
    ttl: Duration,
) -> Option<ParsedIndex> {
    let cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let hit = cache.map.get(url)?;
    if now.duration_since(hit.fetched_at) <= ttl {
        Some(hit.index.clone())
    } else {
        None
    }
}

/// 从 URL 拉取字节（file:// / https / 回环 http），上限 `cap` 字节。
pub(crate) fn fetch_url(url: &str, timeout: Duration, cap: u64) -> Result<Vec<u8>, MarketError> {
    let bytes = if is_file_url(url) {
        let path = file_url_to_path(url).ok_or_else(|| {
            MarketError::InvalidArgument(format!("file:// URL 无法解析为本地路径：{url}"))
        })?;
        let file = std::fs::File::open(&path)
            .map_err(|error| MarketError::Io(format!("打开 {} 失败：{error}", path.display())))?;
        let mut limited = file.take(cap + 1);
        let mut bytes = Vec::new();
        limited
            .read_to_end(&mut bytes)
            .map_err(|error| MarketError::Io(format!("读取 {} 失败：{error}", path.display())))?;
        bytes
    } else if is_https_or_loopback(url) {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .build();
        let agent = ureq::Agent::new_with_config(agent);
        let mut response = agent
            .get(url)
            .call()
            .map_err(|error| MarketError::Http(format!("GET {url} 失败：{error}")))?;
        response
            .body_mut()
            .with_config()
            .limit(cap)
            .read_to_vec()
            .map_err(|error| MarketError::Http(format!("读取 {url} 响应失败：{error}")))?
    } else {
        return Err(MarketError::InvalidArgument(format!(
            "URL 必须是 https（回环允许 http）或 file://：{url}"
        )));
    };
    if bytes.len() as u64 > cap {
        return Err(MarketError::Http(format!(
            "响应超过 {cap} 字节上限（恶意或错误响应）"
        )));
    }
    Ok(bytes)
}

/// 拉取并解析单源 index（install 复用同一约束）。
pub(crate) fn fetch_index(url: &str, timeout: Duration) -> Result<ParsedIndex, MarketError> {
    let body = fetch_url(url, timeout, MAX_INDEX_BYTES)?;
    parse_index(&body)
}

/// 多镜像回退拉取（M3，§3.3）：按 `source.index_urls()` 顺序
/// （权威 url → mirrors）逐个尝试，每个候选独立受 per-source 超时约束；
/// 全部失败 → 返回最后一个错误（源整体记 stale，不毒化结果）。
pub(crate) fn fetch_index_with_fallback(
    source: &RegistrySource,
    timeout: Duration,
) -> Result<ParsedIndex, MarketError> {
    let mut last_error = None;
    for url in source.index_urls() {
        match fetch_index(url, timeout) {
            Ok(index) => return Ok(index),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("index_urls 至少含权威 url"))
}

/// 一份源的结果：成功带解析后的 index，失败只记名字（不毒化结果）。
pub(crate) struct SourceIndex {
    pub source: RegistrySource,
    pub result: Result<ParsedIndex, ()>,
}

impl SourceIndex {
    pub fn stale_name(&self) -> Option<&str> {
        match &self.result {
            Ok(_) => None,
            Err(()) => Some(self.source.name.as_str()),
        }
    }
}

/// 并行拉取全部启用源：std thread per source，per-source 超时由 ureq 约束，
/// 总预算用 channel + recv_timeout 收口；超时未回的源记为失败（其线程在
/// ureq 超时后自行退出，向已断开 channel send 落空即结束）。缓存只由主线程
/// 读写（`ParsedIndex` 不跨线程）。
pub(crate) fn collect_indexes(
    sources: &[RegistrySource],
    cache: &Mutex<IndexCache>,
    refresh: bool,
    limits: FetchLimits,
) -> Vec<SourceIndex> {
    let enabled: Vec<RegistrySource> = sources
        .iter()
        .filter(|source| source.enabled)
        .cloned()
        .collect();
    let mut results: BTreeMap<usize, ParsedIndex> = BTreeMap::new();
    let mut failed: Vec<usize> = Vec::new();
    let mut pending = Vec::new();
    for (index, source) in enabled.iter().enumerate() {
        if !refresh
            && let Some(cached) = cache_get(cache, &source.url, Instant::now(), limits.cache_ttl)
        {
            results.insert(index, cached);
        } else {
            pending.push(index);
        }
    }

    if !pending.is_empty() {
        let (sender, receiver) = std::sync::mpsc::channel::<(usize, Result<ParsedIndex, ()>)>();
        let spawned = pending.len();
        for index in pending {
            let source = enabled[index].clone();
            let sender = sender.clone();
            std::thread::spawn(move || {
                let fetched = fetch_index_with_fallback(&source, limits.per_source);
                let _ = sender.send((index, fetched.map_err(|_| ())));
            });
        }
        // 主线程持有 sender 原件会让 recv 永不结束：全部交给子线程克隆体。
        drop(sender);

        let deadline = Instant::now() + limits.total_budget;
        let mut resolved = 0_usize;
        while resolved < spawned {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match receiver.recv_timeout(remaining) {
                Ok((index, result)) => {
                    match result {
                        Ok(index_doc) => {
                            cache_put(cache, &enabled[index].url, index_doc.clone());
                            results.insert(index, index_doc);
                        }
                        Err(()) => failed.push(index),
                    }
                    resolved += 1;
                }
                Err(_) => break,
            }
        }
        // 超时未回的 pending 源保持失败态（spawned - resolved 个）。
    }

    enabled
        .iter()
        .enumerate()
        .map(|(index, source)| SourceIndex {
            source: source.clone(),
            result: match results.remove(&index) {
                Some(index_doc) => Ok(index_doc),
                None => Err(()),
            },
        })
        .collect()
}

fn cache_put(cache: &Mutex<IndexCache>, url: &str, index: ParsedIndex) {
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.map.insert(
        url.to_string(),
        CachedIndex {
            fetched_at: Instant::now(),
            index,
        },
    );
}

/// 合并条目：同一 id 取 semver 最高，平局保持源列表顺序在前者。
#[derive(Debug, Clone)]
pub struct MergedEntry {
    pub entry: IndexEntry,
    pub source: String,
}

pub(crate) fn merge_indexes(collected: &[SourceIndex]) -> Vec<MergedEntry> {
    let mut winners: HashMap<String, usize> = HashMap::new();
    let mut ordered: Vec<MergedEntry> = Vec::new();
    for source_index in collected {
        let Ok(index) = &source_index.result else {
            continue;
        };
        for entry in &index.entries {
            match winners.get(&entry.id) {
                None => {
                    winners.insert(entry.id.clone(), ordered.len());
                    ordered.push(MergedEntry {
                        entry: entry.clone(),
                        source: source_index.source.name.clone(),
                    });
                }
                Some(position) => {
                    let current = &ordered[*position];
                    if entry.parsed_version > current.entry.parsed_version {
                        ordered[*position] = MergedEntry {
                            entry: entry.clone(),
                            source: source_index.source.name.clone(),
                        };
                    }
                }
            }
        }
    }
    ordered
}

fn query_matches(entry: &IndexEntry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    entry.id.to_lowercase().contains(&query)
        || entry.name().to_lowercase().contains(&query)
        || entry.description().to_lowercase().contains(&query)
}

/// 搜索市场（`ext_search`）。`refresh=true` 跳过缓存强制重拉。
pub fn search(
    paths: &ExtPaths,
    builtin: &RegistrySource,
    query: Option<&str>,
    refresh: bool,
    node_available: bool,
) -> Result<MarketSearchResult, MarketError> {
    search_with(
        paths,
        builtin,
        query,
        refresh,
        node_available,
        global_cache(),
        FetchLimits::default(),
        &registries_path()?,
    )
}

/// 带缓存/约束/源清单注入的搜索实现（测试缝：小预算、私有缓存、隔离清单）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn search_with(
    paths: &ExtPaths,
    builtin: &RegistrySource,
    query: Option<&str>,
    refresh: bool,
    node_available: bool,
    cache: &Mutex<IndexCache>,
    limits: FetchLimits,
    registries_file: &Path,
) -> Result<MarketSearchResult, MarketError> {
    let file_sources = load_file(registries_file)?;
    let sources = merged_sources(builtin, &file_sources);
    let collected = collect_indexes(&sources, cache, refresh, limits);
    build_search_result(paths, collected, query, node_available)
}

/// 从拉取结果构造 DTO（install/update 复用合并与已安装信息）。
pub(crate) fn build_search_result(
    paths: &ExtPaths,
    collected: Vec<SourceIndex>,
    query: Option<&str>,
    node_available: bool,
) -> Result<MarketSearchResult, MarketError> {
    let merged = merge_indexes(&collected);
    let stale_sources: Vec<String> = collected
        .iter()
        .filter_map(SourceIndex::stale_name)
        .map(str::to_string)
        .collect();
    // M3 强制档徽标：条目来自 require_signatures=true 的源且带签名 → Enforced
    // （web 侧徽标三态已就位，M1b 已做；未签名条目在强制源装不上，搜索侧仍标
    // Unsigned 供 UI 提示）。
    let enforced_sources: HashSet<String> = collected
        .iter()
        .filter(|source_index| source_index.source.require_signatures)
        .map(|source_index| source_index.source.name.clone())
        .collect();

    let installed = installed_versions(paths, node_available)?;
    let query = query.unwrap_or_default();
    let mut results = Vec::new();
    for merged_entry in merged {
        let entry = &merged_entry.entry;
        if !query_matches(entry, query) {
            continue;
        }
        let installed_version = installed.get(&entry.id).cloned();
        let update_available = match &installed_version {
            Some(local) => Version::parse(local).is_some_and(|local| entry.parsed_version > local),
            None => false,
        };
        results.push(MarketPlugin {
            id: entry.id.clone(),
            version: entry.version.clone(),
            description: entry.description().to_string(),
            runtime: entry.runtime().to_string(),
            engines: entry.effective_engines(),
            deps: entry.effective_deps(),
            permissions: entry.permissions(),
            source: merged_entry.source.clone(),
            file_source: entry.file_source,
            signature_state: if entry.signature.is_empty() {
                SignatureState::Unsigned
            } else if enforced_sources.contains(&merged_entry.source) {
                SignatureState::Enforced
            } else {
                SignatureState::Signed
            },
            installed_version,
            update_available,
        });
    }
    results.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(MarketSearchResult {
        results,
        stale_sources,
    })
}

/// 已安装（且清单校验通过）插件的 id → version 映射。
pub(crate) fn installed_versions(
    paths: &ExtPaths,
    node_available: bool,
) -> Result<HashMap<String, String>, MarketError> {
    let list: PluginList =
        crate::ext::list_plugins_at(paths, crate::ext::project_allowed(), node_available)
            .map_err(|error| MarketError::Io(error.to_string()))?;
    Ok(list
        .plugins
        .into_iter()
        .filter(|plugin| plugin.error.is_none())
        .map(|plugin| (plugin.id, plugin.version))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use serde_json::json;

    use super::*;
    use crate::ext::marketplace::testutil::{
        TempDir, TestServer, http_ok, serve_fn, serve_raw, write_raw,
    };

    fn manifest(id: &str, version: &str) -> Value {
        json!({
            "id": id,
            "version": version,
            "name": id,
            "runtime": "declarative",
            "description": format!("plugin {id}")
        })
    }

    fn index_body(plugins: Value) -> String {
        json!({"version": 1, "plugins": plugins}).to_string()
    }

    fn entry(id: &str, version: &str, url: &str) -> Value {
        json!({
            "id": id,
            "version": version,
            "manifest": manifest(id, version),
            "dist": {"url": url, "sha256": "a".repeat(64)}
        })
    }

    fn temp_paths(tag: &str) -> (TempDir, ExtPaths) {
        let temp = TempDir::new(tag);
        let paths = ExtPaths::new(
            temp.path().join("extensions"),
            None,
            temp.path().join("extensions.json"),
        );
        (temp, paths)
    }

    #[test]
    fn registry_add_remove_and_builtin_protection() {
        let temp = TempDir::new("reg-add");
        let reg_path = temp.path().join("plugin-registries.json");
        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, "https://example.com/index.json");
        let key = crate::ext::marketplace::testutil::MiniSignTestKey::from_seed(1);

        let list = list_registries_at(&reg_path, &builtin).expect("list");
        assert_eq!(list.sources.len(), 1);
        assert!(list.sources[0].builtin);
        assert!(!list.sources[0].require_signatures);
        assert!(list.sources[0].mirrors.is_empty());

        let list = add_registry_at(
            &reg_path,
            &builtin,
            "mirror-a",
            "file:///tmp/mirror/index.json",
            true,
            Some(&key.pubkey_b64()),
            Some(&["https://mirror-a.example.com/index.json".to_string()]),
        )
        .expect("add mirror");
        assert_eq!(list.sources.len(), 2);
        assert!(list.sources[1].require_signatures);
        assert!(list.sources[1].has_pubkey);
        assert_eq!(
            list.sources[1].mirrors,
            vec!["https://mirror-a.example.com/index.json".to_string()]
        );

        let value = serde_json::to_value(&list).expect("serialize");
        assert_eq!(value["sources"][1]["name"], "mirror-a");
        assert_eq!(value["sources"][1]["requireSignatures"], true);
        assert_eq!(
            value["sources"][1]["mirrors"][0],
            "https://mirror-a.example.com/index.json"
        );
        assert_eq!(value["sources"][0]["builtin"], true);

        // 未知顶层字段在写回后保留。
        let raw = std::fs::read_to_string(&reg_path).expect("read registries");
        let mut file: Value = serde_json::from_str(&raw).expect("json");
        file["future_field"] = json!({"k": 1});
        std::fs::write(&reg_path, file.to_string()).expect("extend registries");
        add_registry_at(
            &reg_path,
            &builtin,
            "mirror-b",
            "https://b.example.com/index.json",
            false,
            None,
            None,
        )
        .expect("add second");
        let raw = std::fs::read_to_string(&reg_path).expect("read back");
        let file: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(file["future_field"]["k"], 1);
        assert_eq!(
            file["sources"][0]["mirrors"][0], "https://mirror-a.example.com/index.json",
            "mirrors 持久化为 snake_case 线格式"
        );

        // 重复名 / builtin 同名 / 非法 URL 拒绝。
        assert!(
            add_registry_at(
                &reg_path,
                &builtin,
                "mirror-a",
                "https://b.example.com",
                false,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            add_registry_at(
                &reg_path,
                &builtin,
                "official",
                "https://b.example.com",
                false,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            add_registry_at(
                &reg_path,
                &builtin,
                "bad",
                "ftp://example.com",
                false,
                None,
                None
            )
            .is_err()
        );

        let err = remove_registry_at(&reg_path, &builtin, BUILTIN_SOURCE_NAME)
            .expect_err("builtin protected");
        assert_eq!(err.code(), lc_proto::ErrorCode::InvalidArgument);
        assert!(remove_registry_at(&reg_path, &builtin, "missing").is_err());

        let list = remove_registry_at(&reg_path, &builtin, "mirror-a").expect("remove");
        assert_eq!(list.sources.len(), 2);
    }

    #[test]
    fn registry_add_validates_pubkey_m3_rules() {
        let temp = TempDir::new("reg-pubkey");
        let reg_path = temp.path().join("plugin-registries.json");
        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, "https://example.com/index.json");
        let key = crate::ext::marketplace::testutil::MiniSignTestKey::from_seed(2);

        // 强制档必须配置 pubkey（add 时拦截，§3.7）。
        let err = add_registry_at(
            &reg_path,
            &builtin,
            "enforced-no-key",
            "https://e.example.com/index.json",
            true,
            None,
            None,
        )
        .expect_err("enforced without pubkey rejected");
        assert_eq!(err.code(), lc_proto::ErrorCode::InvalidArgument);
        assert!(err.to_string().contains("必须配置 pubkey"), "{err}");

        // 显式空串 pubkey 同上。
        assert!(
            add_registry_at(
                &reg_path,
                &builtin,
                "enforced-empty-key",
                "https://e.example.com/index.json",
                true,
                Some("   "),
                None,
            )
            .is_err()
        );

        // 非空但非法 base64 → 拒绝。
        let err = add_registry_at(
            &reg_path,
            &builtin,
            "bad-b64",
            "https://e.example.com/index.json",
            false,
            Some("!!!not-base64!!!"),
            None,
        )
        .expect_err("invalid base64 pubkey rejected");
        assert_eq!(err.code(), lc_proto::ErrorCode::InvalidArgument);
        assert!(err.to_string().contains("pubkey 非法"), "{err}");

        // base64 合法但长度不对（41 字节 blob，非 minisign 线格式）→ 拒绝。
        let mut wrong_len = vec![0u8; 41];
        wrong_len[0] = 0x45;
        wrong_len[1] = 0x64;
        let short = crate::ext::marketplace::testutil::base64_encode(&wrong_len);
        let err = add_registry_at(
            &reg_path,
            &builtin,
            "bad-len",
            "https://e.example.com/index.json",
            false,
            Some(&short),
            None,
        )
        .expect_err("wrong-length pubkey rejected");
        assert_eq!(err.code(), lc_proto::ErrorCode::InvalidArgument);

        // 合法 pubkey：警告档可配（不影响安装），强制档可配。
        add_registry_at(
            &reg_path,
            &builtin,
            "warn-key",
            "https://e.example.com/warn.json",
            false,
            Some(&key.pubkey_b64()),
            None,
        )
        .expect("warning tier with pubkey ok");
        add_registry_at(
            &reg_path,
            &builtin,
            "enforced-key",
            "https://e.example.com/enf.json",
            true,
            Some(&key.pubkey_b64()),
            None,
        )
        .expect("enforced with valid pubkey ok");
    }

    #[test]
    fn registry_add_validates_mirrors() {
        let temp = TempDir::new("reg-mirrors");
        let reg_path = temp.path().join("plugin-registries.json");
        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, "https://example.com/index.json");

        // 非法镜像 URL 拒绝并点名。
        let err = add_registry_at(
            &reg_path,
            &builtin,
            "bad-mirror",
            "https://e.example.com/index.json",
            false,
            None,
            Some(&["ftp://evil.example.com/index.json".to_string()]),
        )
        .expect_err("bad mirror url rejected");
        assert_eq!(err.code(), lc_proto::ErrorCode::InvalidArgument);
        assert!(err.to_string().contains("镜像 URL"), "{err}");

        // 合法镜像（含 file://）入库并出现在 list DTO。
        let list = add_registry_at(
            &reg_path,
            &builtin,
            "with-mirrors",
            "https://e.example.com/index.json",
            false,
            None,
            Some(&[
                "https://m1.example.com/index.json".to_string(),
                "file:///tmp/m2/index.json".to_string(),
            ]),
        )
        .expect("mirrors added");
        let info = list
            .sources
            .iter()
            .find(|source| source.name == "with-mirrors")
            .expect("source");
        assert_eq!(info.mirrors.len(), 2);
        assert_eq!(info.mirrors[1], "file:///tmp/m2/index.json");

        // 老文件无 mirrors 键 → 默认空（向后兼容）。
        let mut file: Value =
            serde_json::from_str(&std::fs::read_to_string(&reg_path).expect("read")).expect("json");
        file["sources"][0]
            .as_object_mut()
            .expect("obj")
            .remove("mirrors");
        std::fs::write(&reg_path, file.to_string()).expect("rewrite");
        let list = list_registries_at(&reg_path, &builtin).expect("list");
        let info = list
            .sources
            .iter()
            .find(|source| source.name == "with-mirrors")
            .expect("source");
        assert!(info.mirrors.is_empty(), "缺省 mirrors 向后兼容为空");
    }

    #[test]
    fn registry_disable_builtin_via_overlay_and_toggle_file_source() {
        let temp = TempDir::new("reg-disable");
        let reg_path = temp.path().join("plugin-registries.json");
        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, "https://example.com/index.json");
        let mut mirror = RegistrySource::new("mirror", "file:///tmp/m.json");
        mirror.enabled = false;
        save_file(&reg_path, std::slice::from_ref(&mirror)).expect("save");

        let list = set_registry_enabled_at(&reg_path, &builtin, BUILTIN_SOURCE_NAME, false)
            .expect("disable builtin");
        assert!(list.sources.iter().any(|s| s.name == "official"));
        let file_sources = load_file(&reg_path).expect("load");
        let overlay = file_sources
            .iter()
            .find(|s| s.name == "official")
            .expect("overlay");
        assert!(!overlay.enabled, "builtin disabled persisted as overlay");

        let merged = merged_sources(&builtin, &file_sources);
        assert!(!merged[0].enabled, "builtin disabled via overlay");
        assert!(!merged[1].enabled, "file source disabled");

        set_registry_enabled_at(&reg_path, &builtin, "mirror", true).expect("enable mirror");
        let merged = merged_sources(&builtin, &load_file(&reg_path).expect("load"));
        assert!(
            merged
                .iter()
                .find(|s| s.name == "mirror")
                .expect("mirror")
                .enabled
        );
        set_registry_enabled_at(&reg_path, &builtin, BUILTIN_SOURCE_NAME, true)
            .expect("re-enable builtin");
        let merged = merged_sources(&builtin, &load_file(&reg_path).expect("load"));
        assert!(merged[0].enabled);
    }

    #[test]
    fn search_merges_sources_and_marks_failure_stale() {
        let (_temp, paths) = temp_paths("search-merge");
        let good = serve_fn(move |_, _| {
            http_ok(&index_body(json!([
                entry("demo.one", "1.0.0", "https://example.com/one.zip"),
                entry("demo.two", "0.1.0", "https://example.com/two.zip")
            ])))
        });
        let bad = TestServer::not_found();
        let reg_path = _temp.path().join("reg.json");

        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, format!("{}/index.json", good.url));
        let mut mirror = RegistrySource::new("mirror", format!("{}/index.json", bad.url));
        mirror.enabled = true;
        save_file(&reg_path, std::slice::from_ref(&mirror)).expect("save sources");

        let cache = Mutex::new(IndexCache::new());
        let result = search_with(
            &paths,
            &builtin,
            None,
            true,
            false,
            &cache,
            FetchLimits {
                cache_ttl: Duration::from_secs(300),
                per_source: Duration::from_secs(5),
                total_budget: Duration::from_secs(10),
            },
            &reg_path,
        )
        .expect("search");
        assert_eq!(
            result.results.len(),
            2,
            "stale source does not poison results"
        );
        assert_eq!(result.stale_sources, vec!["mirror".to_string()]);
        assert_eq!(result.results[0].source, "official");
    }

    #[test]
    fn search_picks_highest_version_across_sources() {
        let (_temp, paths) = temp_paths("search-version");
        let reg_path = _temp.path().join("reg.json");
        let low = serve_fn(move |_, _| {
            http_ok(&index_body(json!([entry(
                "demo.one",
                "0.9.0",
                "https://example.com/low.zip"
            )])))
        });
        let high = serve_fn(move |_, _| {
            http_ok(&index_body(json!([entry(
                "demo.one",
                "1.0.0",
                "https://example.com/high.zip"
            )])))
        });

        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, format!("{}/index.json", low.url));
        let mirror = RegistrySource::new("mirror", format!("{}/index.json", high.url));
        save_file(&reg_path, std::slice::from_ref(&mirror)).expect("save sources");

        let cache = Mutex::new(IndexCache::new());
        let result = search_with(
            &paths,
            &builtin,
            None,
            true,
            false,
            &cache,
            FetchLimits::default(),
            &reg_path,
        )
        .expect("search");
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].version, "1.0.0");
        assert_eq!(result.results[0].source, "mirror");
    }

    #[test]
    fn search_uses_cache_until_refresh() {
        let (_temp, paths) = temp_paths("search-cache");
        let reg_path = _temp.path().join("reg.json");
        let server = serve_fn(move |_, _| {
            http_ok(&index_body(json!([entry(
                "demo.one",
                "1.0.0",
                "https://example.com/one.zip"
            )])))
        });
        let builtin =
            RegistrySource::new(BUILTIN_SOURCE_NAME, format!("{}/index.json", server.url));
        let cache = Mutex::new(IndexCache::new());
        let limits = FetchLimits {
            cache_ttl: Duration::from_secs(300),
            per_source: Duration::from_secs(5),
            total_budget: Duration::from_secs(10),
        };

        let first = search_with(
            &paths, &builtin, None, false, false, &cache, limits, &reg_path,
        )
        .expect("first");
        assert_eq!(first.results.len(), 1);
        let second = search_with(
            &paths, &builtin, None, false, false, &cache, limits, &reg_path,
        )
        .expect("cached");
        assert_eq!(second.results.len(), 1);
        assert_eq!(
            server.hits.load(Ordering::Relaxed),
            1,
            "cache hit avoids refetch"
        );

        let third = search_with(
            &paths, &builtin, None, true, false, &cache, limits, &reg_path,
        )
        .expect("refresh");
        assert_eq!(third.results.len(), 1);
        assert_eq!(
            server.hits.load(Ordering::Relaxed),
            2,
            "refresh forces refetch"
        );
    }

    #[test]
    fn search_refetches_after_ttl_expiry() {
        let (_temp, paths) = temp_paths("search-ttl");
        let reg_path = _temp.path().join("reg.json");
        let server = serve_fn(move |_, _| {
            http_ok(&index_body(json!([entry(
                "demo.one",
                "1.0.0",
                "https://example.com/one.zip"
            )])))
        });
        let builtin =
            RegistrySource::new(BUILTIN_SOURCE_NAME, format!("{}/index.json", server.url));
        let cache = Mutex::new(IndexCache::new());
        let ttl = Duration::from_secs(300);
        let limits = FetchLimits {
            cache_ttl: ttl,
            per_source: Duration::from_secs(5),
            total_budget: Duration::from_secs(10),
        };

        search_with(
            &paths, &builtin, None, false, false, &cache, limits, &reg_path,
        )
        .expect("first");
        assert_eq!(server.hits.load(Ordering::Relaxed), 1);

        // 把缓存条目的 fetched_at 拨回 TTL 之前 → 下次搜索必须重拉。
        let url = format!("{}/index.json", server.url);
        {
            let mut guard = cache.lock().unwrap();
            let entry = guard.map.get_mut(&url).expect("cached entry");
            entry.fetched_at = Instant::now()
                .checked_sub(ttl + Duration::from_secs(1))
                .unwrap();
        }
        search_with(
            &paths, &builtin, None, false, false, &cache, limits, &reg_path,
        )
        .expect("after ttl");
        assert_eq!(
            server.hits.load(Ordering::Relaxed),
            2,
            "expired cache refetches"
        );
    }

    #[test]
    fn search_marks_slow_source_stale_within_total_budget() {
        let (_temp, paths) = temp_paths("search-budget");
        let reg_path = _temp.path().join("reg.json");
        let slow = serve_fn(move |_, _| {
            std::thread::sleep(Duration::from_millis(600));
            http_ok(&index_body(json!([entry(
                "demo.slow",
                "1.0.0",
                "https://example.com/s.zip"
            )])))
        });
        let fast = serve_fn(move |_, _| {
            http_ok(&index_body(json!([entry(
                "demo.fast",
                "1.0.0",
                "https://example.com/f.zip"
            )])))
        });

        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, format!("{}/index.json", slow.url));
        let mirror = RegistrySource::new("mirror", format!("{}/index.json", fast.url));
        save_file(&reg_path, std::slice::from_ref(&mirror)).expect("save sources");

        let cache = Mutex::new(IndexCache::new());
        let result = search_with(
            &paths,
            &builtin,
            None,
            true,
            false,
            &cache,
            FetchLimits {
                cache_ttl: Duration::from_secs(300),
                per_source: Duration::from_secs(5),
                total_budget: Duration::from_millis(200),
            },
            &reg_path,
        )
        .expect("search within budget");
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].id, "demo.fast");
        assert_eq!(result.stale_sources, vec!["official".to_string()]);
    }

    #[test]
    fn search_survives_lying_content_length_and_truncated_index() {
        let (_temp, paths) = temp_paths("search-truncate");
        let reg_path = _temp.path().join("reg.json");
        // Content-Length 撒谎（声称 4 字节实际更多 → ureq 只交付 4 字节 → JSON 解析失败）。
        let lying = serve_raw(move |stream, _| {
            let body = index_body(json!([entry(
                "demo.x",
                "1.0.0",
                "https://example.com/x.zip"
            )]));
            write_raw(
                &stream,
                &format!("HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n{body}")
                    .into_bytes(),
            );
        });
        // 截断连接：声称很长但只发一半就断开（可读部分不是合法 JSON）。
        let truncated = serve_raw(move |stream, _| {
            let body = index_body(json!([entry(
                "demo.y",
                "1.0.0",
                "https://example.com/y.zip"
            )]));
            let full_len = body.len();
            let mut head = body.into_bytes();
            head.truncate(head.len() / 2);
            write_raw(
                &stream,
                &format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {full_len}\r\nConnection: close\r\n\r\n"
                )
                .into_bytes(),
            );
            write_raw(&stream, &head);
        });
        let good = serve_fn(move |_, _| {
            http_ok(&index_body(json!([entry(
                "demo.z",
                "1.0.0",
                "https://example.com/z.zip"
            )])))
        });

        let mut sources = vec![
            RegistrySource::new("lying", format!("{}/index.json", lying.url)),
            RegistrySource::new("truncated", format!("{}/index.json", truncated.url)),
            RegistrySource::new("good", format!("{}/index.json", good.url)),
        ];
        sources[1].enabled = true;
        sources[2].enabled = true;
        save_file(&reg_path, &sources).expect("save sources");

        // builtin 名与第一个源对齐（合并语义要求 builtin 在首位）。
        let builtin = sources[0].clone();
        let cache = Mutex::new(IndexCache::new());
        let result = search_with(
            &paths,
            &builtin,
            None,
            true,
            false,
            &cache,
            FetchLimits::default(),
            &reg_path,
        )
        .expect("search survives bad sources");
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].id, "demo.z");
        let mut stale = result.stale_sources.clone();
        stale.sort();
        assert_eq!(stale, vec!["lying".to_string(), "truncated".to_string()]);
    }

    #[test]
    fn search_filters_by_query_and_marks_installed() {
        let (_temp, paths) = temp_paths("search-query");
        let reg_path = _temp.path().join("reg.json");
        std::fs::create_dir_all(paths.user_dir.join("demo.installed")).expect("plugin dir");
        std::fs::write(
            paths.user_dir.join("demo.installed/plugin.json"),
            manifest("demo.installed", "0.5.0").to_string(),
        )
        .expect("write installed plugin");

        let server = serve_fn(move |_, _| {
            http_ok(&index_body(json!([
                entry("demo.installed", "1.0.0", "https://example.com/i.zip"),
                entry("demo.other", "2.0.0", "https://example.com/o.zip")
            ])))
        });
        let builtin =
            RegistrySource::new(BUILTIN_SOURCE_NAME, format!("{}/index.json", server.url));
        let cache = Mutex::new(IndexCache::new());

        let result = search_with(
            &paths,
            &builtin,
            Some("installed"),
            true,
            false,
            &cache,
            FetchLimits::default(),
            &reg_path,
        )
        .expect("query search");
        assert_eq!(result.results.len(), 1);
        let hit = &result.results[0];
        assert_eq!(hit.id, "demo.installed");
        assert_eq!(hit.installed_version.as_deref(), Some("0.5.0"));
        assert!(hit.update_available);

        let all = search_with(
            &paths,
            &builtin,
            None,
            true,
            false,
            &cache,
            FetchLimits::default(),
            &reg_path,
        )
        .expect("all");
        let other = all
            .results
            .iter()
            .find(|r| r.id == "demo.other")
            .expect("other");
        assert_eq!(other.installed_version, None);
        assert!(!other.update_available);
    }

    #[test]
    fn fetch_url_reads_file_scheme() {
        let temp = TempDir::new("fetch-file");
        let file = temp.path().join("index.json");
        std::fs::write(
            &file,
            index_body(json!([entry("demo.f", "1.0.0", "https://e/f.zip")])),
        )
        .expect("write");
        let url = crate::ext::marketplace::testutil::file_url(&file);
        let bytes =
            fetch_url(&url, Duration::from_secs(5), MAX_INDEX_BYTES).expect("fetch file url");
        let parsed = parse_index(&bytes).expect("parse");
        assert_eq!(parsed.entries.len(), 1);

        assert!(fetch_url("ftp://example.com/x", Duration::from_secs(5), 1024).is_err());
    }

    // ==== M3：同 index 多镜像回退（§3.3） ====

    /// 源 url + mirrors 的回退夹具：url/mirror1 挂（404），mirror2 存活。
    struct MirrorFixture {
        _temp: TempDir,
        paths: ExtPaths,
        reg_path: PathBuf,
        good: TestServer,
        dead_url_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        mirror1_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    fn mirror_fixture(tag: &str) -> MirrorFixture {
        let dead = TestServer::not_found();
        let dead_url_hits = dead.hits.clone();
        let mirror1 = TestServer::not_found();
        let mirror1_hits = mirror1.hits.clone();
        let good = serve_fn(move |_, _| {
            http_ok(&index_body(json!([entry(
                "demo.mirror",
                "1.0.0",
                "https://e/m.zip"
            )])))
        });
        let (_temp, paths) = temp_paths(tag);
        let reg_path = _temp.path().join("reg.json");
        // 源：权威 url 与 mirror1 均 404，mirror2 存活；builtin 以 overlay
        // 禁用（测试环境不可达 example.com，避免混入 stale）。
        let mut source = RegistrySource::new("mirrored", format!("{}/index.json", dead.url));
        source.mirrors = vec![
            format!("{}/index.json", mirror1.url),
            format!("{}/index.json", good.url),
        ];
        let mut official = RegistrySource::new(BUILTIN_SOURCE_NAME, "");
        official.enabled = false;
        save_file(&reg_path, &[source, official]).expect("save sources");
        MirrorFixture {
            _temp,
            paths,
            reg_path,
            good,
            dead_url_hits,
            mirror1_hits,
        }
    }

    #[test]
    fn index_fetch_falls_back_across_mirrors_in_order() {
        let fx = mirror_fixture("mirror-fallback");
        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, "https://example.com/index.json");
        let cache = Mutex::new(IndexCache::new());

        let result = search_with(
            &fx.paths,
            &builtin,
            None,
            true,
            false,
            &cache,
            FetchLimits::default(),
            &fx.reg_path,
        )
        .expect("search via mirror fallback");
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].id, "demo.mirror");
        assert!(result.stale_sources.is_empty());
        assert_eq!(
            fx.dead_url_hits.load(Ordering::Relaxed),
            1,
            "权威 url 先尝试一次"
        );
        assert_eq!(
            fx.mirror1_hits.load(Ordering::Relaxed),
            1,
            "mirror1 回退一次"
        );
        assert!(fx.good.hits.load(Ordering::Relaxed) >= 1, "mirror2 命中");
    }

    #[test]
    fn index_fetch_all_mirrors_dead_marks_source_stale_by_name() {
        let (_temp, paths) = temp_paths("mirror-dead");
        let reg_path = _temp.path().join("reg.json");
        let dead_a = TestServer::not_found();
        let dead_b = TestServer::not_found();
        let dead_c = TestServer::not_found();
        let mut source = RegistrySource::new("all-down", format!("{}/index.json", dead_a.url));
        source.mirrors = vec![
            format!("{}/index.json", dead_b.url),
            format!("{}/index.json", dead_c.url),
        ];
        let mut official = RegistrySource::new(BUILTIN_SOURCE_NAME, "");
        official.enabled = false;
        save_file(&reg_path, &[source, official]).expect("save sources");

        let builtin = RegistrySource::new(BUILTIN_SOURCE_NAME, "https://example.com/index.json");
        let cache = Mutex::new(IndexCache::new());
        let result = search_with(
            &paths,
            &builtin,
            None,
            true,
            false,
            &cache,
            FetchLimits::default(),
            &reg_path,
        )
        .expect("search survives all-dead mirrors");
        assert!(result.results.is_empty());
        assert_eq!(
            result.stale_sources,
            vec!["all-down".to_string()],
            "全挂时点名失败源"
        );
        assert_eq!(
            dead_b.hits.load(Ordering::Relaxed),
            1,
            "每个镜像都尝试过（per-mirror 独立回退）"
        );
    }

    #[test]
    fn search_marks_enforced_badge_only_for_signed_entry_in_enforced_source() {
        let (_temp, paths) = temp_paths("search-enforced");
        let reg_path = _temp.path().join("reg.json");
        let key = crate::ext::marketplace::testutil::MiniSignTestKey::from_seed(9);
        let server = serve_fn(move |_, _| {
            let mut signed = entry("demo.enf", "1.0.0", "https://example.com/e.zip");
            signed["signature"] = json!("whatever-non-empty");
            http_ok(&index_body(json!([
                signed,
                entry("demo.plain", "1.0.0", "https://e/p.zip")
            ])))
        });

        // 强制档源：带签名条目 → Enforced 徽标；未签名条目仍 Unsigned。
        let mut enforced =
            RegistrySource::new(BUILTIN_SOURCE_NAME, format!("{}/index.json", server.url));
        enforced.require_signatures = true;
        enforced.pubkey = key.pubkey_b64();
        let result = search_with(
            &paths,
            &enforced,
            None,
            true,
            false,
            &Mutex::new(IndexCache::new()),
            FetchLimits::default(),
            &reg_path,
        )
        .expect("search enforced");
        let states: HashMap<String, SignatureState> = result
            .results
            .iter()
            .map(|plugin| (plugin.id.clone(), plugin.signature_state))
            .collect();
        assert_eq!(states["demo.enf"], SignatureState::Enforced);
        assert_eq!(states["demo.plain"], SignatureState::Unsigned);

        // 同一服务器、警告档源：带签名条目 → Signed（不回退 Unsigned）。
        let warning =
            RegistrySource::new(BUILTIN_SOURCE_NAME, format!("{}/index.json", server.url));
        let result = search_with(
            &paths,
            &warning,
            None,
            true,
            false,
            &Mutex::new(IndexCache::new()),
            FetchLimits::default(),
            &reg_path,
        )
        .expect("search warning");
        let states: HashMap<String, SignatureState> = result
            .results
            .iter()
            .map(|plugin| (plugin.id.clone(), plugin.signature_state))
            .collect();
        assert_eq!(states["demo.enf"], SignatureState::Signed);
        assert_eq!(states["demo.plain"], SignatureState::Unsigned);
    }
}
