//! 源清单与多源拉取（docs/plans/plugin-marketplace.md §3.3）。
//!
//! - 源清单 `~/.lite-code/plugin-registries.json`（write_atomic 原子写，未知
//!   顶层字段保留，同 ext state 先例）；builtin 源由调用方（桌面层）注入，
//!   文件内同名条目可覆盖其 `enabled`/`require_signatures`/`pubkey`
//!   ——即「builtin 不可删除、可禁用」；
//! - URL 白名单：https / 回环 http / `file://`（file 源 UI 须标「本地源」）；
//! - `ext_search` 并行拉取全部启用源（std thread per source，per-source
//!   10s 超时、总预算 30s），TTL 5 分钟内存缓存（`refresh=true` 强制），
//!   按 id 去重取 semver 最高；失败源不毒化结果，记入 `staleSources`。

use std::collections::{BTreeMap, HashMap};
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

/// 单个插件源。文件键为 snake_case（`require_signatures`/`pubkey`/`enabled`），
/// 与 §3.3 线格式一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RegistrySource {
    pub name: String,
    pub url: String,
    /// M3 强制档开关；M1 只透传配置不做签名校验。
    #[serde(default)]
    pub require_signatures: bool,
    /// minisign 公钥（base64），仅来自本地配置，禁止网络获取（无 TOFU）。
    #[serde(default)]
    pub pubkey: String,
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
            enabled: true,
        }
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
/// `enabled`/`require_signatures`/`pubkey`（URL 非空时也覆盖），
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

/// 新增源。URL 走白名单校验；builtin 同名拒绝覆盖。
pub fn add_registry(
    builtin: &RegistrySource,
    name: &str,
    url: &str,
    require_signatures: bool,
    pubkey: Option<&str>,
) -> Result<RegistryList, MarketError> {
    add_registry_at(
        &registries_path()?,
        builtin,
        name,
        url,
        require_signatures,
        pubkey,
    )
}

fn add_registry_at(
    path: &Path,
    builtin: &RegistrySource,
    name: &str,
    url: &str,
    require_signatures: bool,
    pubkey: Option<&str>,
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
    let mut file_sources = load_file(path)?;
    if file_sources.iter().any(|source| source.name == name) {
        return Err(MarketError::InvalidArgument(format!("源 `{name}` 已存在")));
    }
    file_sources.push(RegistrySource {
        name: name.to_string(),
        url: url.to_string(),
        require_signatures,
        pubkey: pubkey.unwrap_or_default().trim().to_string(),
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
                let fetched = fetch_index(&source.url, limits.per_source);
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

        let list = list_registries_at(&reg_path, &builtin).expect("list");
        assert_eq!(list.sources.len(), 1);
        assert!(list.sources[0].builtin);
        assert!(!list.sources[0].require_signatures);

        let list = add_registry_at(
            &reg_path,
            &builtin,
            "mirror-a",
            "file:///tmp/mirror/index.json",
            true,
            Some("pubkey-b64"),
        )
        .expect("add mirror");
        assert_eq!(list.sources.len(), 2);
        assert!(list.sources[1].require_signatures);
        assert!(list.sources[1].has_pubkey);

        let value = serde_json::to_value(&list).expect("serialize");
        assert_eq!(value["sources"][1]["name"], "mirror-a");
        assert_eq!(value["sources"][1]["requireSignatures"], true);
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
        )
        .expect("add second");
        let raw = std::fs::read_to_string(&reg_path).expect("read back");
        let file: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(file["future_field"]["k"], 1);

        // 重复名 / builtin 同名 / 非法 URL 拒绝。
        assert!(
            add_registry_at(
                &reg_path,
                &builtin,
                "mirror-a",
                "https://b.example.com",
                false,
                None
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
                None
            )
            .is_err()
        );
        assert!(
            add_registry_at(&reg_path, &builtin, "bad", "ftp://example.com", false, None).is_err()
        );

        let err = remove_registry_at(&reg_path, &builtin, BUILTIN_SOURCE_NAME)
            .expect_err("builtin protected");
        assert_eq!(err.code(), lc_proto::ErrorCode::InvalidArgument);
        assert!(remove_registry_at(&reg_path, &builtin, "missing").is_err());

        let list = remove_registry_at(&reg_path, &builtin, "mirror-a").expect("remove");
        assert_eq!(list.sources.len(), 2);
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
}
