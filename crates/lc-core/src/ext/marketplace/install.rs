//! 安装 / 更新 / 卸载 / GC（docs/plans/plugin-marketplace.md §3.2/§3.5/§3.6）。
//!
//! 安装主链：条目定位（version 缺省取 semver 最高，source 缺省取源序第一）
//! → engines 预检（force 跳过）→ deps 解析自动装（M2：同源优先/深度 8/环检测）
//! → 下载到 `<user_dir>/.tmp/`（64 MiB 帽）
//! → sha256 校验 → zip 解压（单一顶层 `<id>/`、zip-slip 防御、stored/deflate
//! 白名单、256 MiB 解压帽、4096 条目帽）→ 包内 plugin.json 与 index 条目比对
//! （不一致 `contract_mismatch`）→ 三级配额核算 → tmp+rename 原子落位
//! （Windows 先改名旧目录到 `*.uninstall-bak`，失败可回滚）→ GC。
//!
//! 半安装恢复：任何中断点只剩 `.tmp/` 或 `*.uninstall-bak` 垃圾，
//! `gc_stale` 惰性清理（.tmp 超 24h、*.uninstall-bak 一律清）。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use sha2::Digest;

use super::index::IndexEntry;
use super::source::fetch_url;
use super::source::{
    self, FetchLimits, IndexCache, MergedEntry, RegistrySource, SourceIndex, collect_indexes,
    merge_indexes,
};
use super::{MarketError, safe_zip_target};
use crate::ext::{ExtPaths, ExtensionsState, PluginList};
use crate::version::{Version, satisfies_range};

/// 单包下载上限（§3.6 三级上限第一档）。
pub const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
/// 单插件解压后体积上限（第二档）。
pub const MAX_EXTRACT_BYTES: u64 = 256 * 1024 * 1024;
/// 单包条目数上限。
pub const MAX_PACKAGE_ENTRIES: usize = 4096;
/// extensions 目录总量配额默认值（第三档，1 GiB；env `LC_EXT_QUOTA_MB` 覆盖）。
pub const DEFAULT_QUOTA_MB: u64 = 1024;
/// 配额覆盖环境变量名。
pub const QUOTA_ENV: &str = "LC_EXT_QUOTA_MB";
/// 下载/解压临时目录（`<user_dir>/.tmp/`，同分区保证 rename 原子）。
pub const TMP_DIR_NAME: &str = ".tmp";
/// 卸载/覆盖安装的回滚备份后缀。
pub const UNINSTALL_BAK_SUFFIX: &str = ".uninstall-bak";
/// `.tmp` 垃圾的超龄阈值（GC 清理 >24h 的条目）。
pub const GC_TMP_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// 包下载超时：index 用 10s（§3.3），64 MiB 包体给 60s。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// 安装市场插件（`ext_install`）。返回 `PluginList` 复用既有 UI 刷新链路
/// （同 `ext_reload` 返回约定，§3.4）。
#[allow(clippy::too_many_arguments)]
pub fn install(
    paths: &ExtPaths,
    builtin: &RegistrySource,
    id: &str,
    version: Option<&str>,
    source: Option<&str>,
    force: bool,
    node_available: bool,
) -> Result<PluginList, MarketError> {
    install_at(
        paths,
        builtin,
        &source::registries_path()?,
        source::global_cache(),
        FetchLimits::default(),
        id,
        version,
        source,
        force,
        node_available,
        quota_from_env(),
    )
}

/// 带依赖注入的安装实现（测试缝：隔离源清单/缓存/配额/时限）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_at(
    paths: &ExtPaths,
    builtin: &RegistrySource,
    registries_file: &Path,
    cache: &Mutex<IndexCache>,
    limits: FetchLimits,
    id: &str,
    version: Option<&str>,
    source_name: Option<&str>,
    force: bool,
    node_available: bool,
    quota_mb: u64,
) -> Result<PluginList, MarketError> {
    gc_stale(paths);
    let collected = collect_and_merge(builtin, registries_file, cache, limits)?;
    let merged = merge_indexes(&collected);
    let resolved = resolve_entry(&merged, id, version, source_name)?;
    if !force {
        preflight_engines(&resolved.entry)?;
        resolve_deps(
            paths,
            resolved,
            &collected,
            &merged,
            node_available,
            quota_mb,
        )?;
    }
    install_package(paths, &resolved.entry, quota_mb)?;
    list(paths, node_available)
}

/// 更新（`ext_update`，§3.5）：对每个已安装插件取 index 最高版本，
/// semver 严格大于才更新；走 install 的交换路径，**不动 disabled 集合**。
#[allow(clippy::too_many_arguments)]
pub fn update(
    paths: &ExtPaths,
    builtin: &RegistrySource,
    id: Option<&str>,
    force: bool,
    node_available: bool,
) -> Result<PluginList, MarketError> {
    update_at(
        paths,
        builtin,
        &source::registries_path()?,
        source::global_cache(),
        FetchLimits::default(),
        id,
        force,
        node_available,
        quota_from_env(),
    )
}

#[allow(clippy::too_many_arguments)]
fn update_at(
    paths: &ExtPaths,
    builtin: &RegistrySource,
    registries_file: &Path,
    cache: &Mutex<IndexCache>,
    limits: FetchLimits,
    id: Option<&str>,
    force: bool,
    node_available: bool,
    quota_mb: u64,
) -> Result<PluginList, MarketError> {
    gc_stale(paths);
    let collected = collect_and_merge(builtin, registries_file, cache, limits)?;
    let merged = merge_indexes(&collected);
    let installed = installed_versions_sorted(paths, node_available)?;

    let targets: Vec<(String, String)> = match id {
        Some(raw) => {
            let pid = raw.trim();
            let local = installed
                .iter()
                .find(|(plugin_id, _)| plugin_id == pid)
                .map(|(_, version)| version.clone())
                .ok_or_else(|| MarketError::NotFound(format!("插件 `{pid}` 未安装")))?;
            vec![(pid.to_string(), local)]
        }
        None => installed,
    };

    for (pid, local_version) in &targets {
        let local_parsed = Version::parse(local_version).ok_or_else(|| {
            MarketError::InvalidArgument(format!(
                "本地插件 `{pid}` 版本号 `{local_version}` 无法解析"
            ))
        })?;
        let candidate = merged
            .iter()
            .filter(|merged_entry| merged_entry.entry.id == *pid)
            .max_by(|a, b| a.entry.parsed_version.cmp(&b.entry.parsed_version));
        let Some(best) = candidate else { continue };
        if best.entry.parsed_version > local_parsed {
            if !force {
                preflight_engines(&best.entry)?;
                resolve_deps(paths, best, &collected, &merged, node_available, quota_mb)?;
            }
            install_package(paths, &best.entry, quota_mb)?;
        }
    }
    list(paths, node_available)
}

/// 卸载（`ext_uninstall`，§3.5）：路径白名单（规范化后必须等于
/// `<user_dir>/<id>`，拒绝一切穿越与 project 级）；改名 `*.uninstall-bak`
/// → 清 disabled → 删 bak（任一步失败回滚）。
pub fn uninstall(
    paths: &ExtPaths,
    id: &str,
    node_available: bool,
) -> Result<PluginList, MarketError> {
    gc_stale(paths);
    let id = id.trim();
    // id 拒绝分隔符/空白/`.`/`..`（复用清单 id 规则）→ join 必落在 user_dir 内。
    if !crate::ext::manifest::valid_plugin_id(id) {
        return Err(MarketError::InvalidArgument(format!("插件 id `{id}` 非法")));
    }
    let target = paths.user_dir.join(id);
    if !target.is_dir() {
        return Err(MarketError::NotFound(format!(
            "插件 `{id}` 不存在于 {}",
            paths.user_dir.display()
        )));
    }
    let bak = paths.user_dir.join(format!("{id}{UNINSTALL_BAK_SUFFIX}"));
    if bak.exists() {
        std::fs::remove_dir_all(&bak).map_err(|error| {
            MarketError::Io(format!("清理遗留备份 {} 失败:{error}", bak.display()))
        })?;
    }
    std::fs::rename(&target, &bak)
        .map_err(|error| MarketError::Io(format!("卸载改名失败（插件目录可能被占用）:{error}")))?;
    let state = ExtensionsState::new(paths.state_path.clone());
    if let Err(error) = state.set_enabled(id, true) {
        // 状态写入失败 → 回滚改名，目录恢复原状。
        let _ = std::fs::rename(&bak, &target);
        return Err(MarketError::Io(format!("更新启用状态失败:{error}")));
    }
    // 删 bak 失败不阻断卸载结果（GC 兜底清理）。
    let _ = std::fs::remove_dir_all(&bak);
    list(paths, node_available)
}

/// 惰性 GC（§3.5/§3.6）：清 `.tmp` 内超龄条目（默认 >24h，测试可注入
/// `max_age` 缩小窗口）与一切 `*.uninstall-bak`。best-effort，不报错。
pub fn gc_stale(paths: &ExtPaths) {
    gc_stale_with(paths, GC_TMP_MAX_AGE);
}

pub(crate) fn gc_stale_with(paths: &ExtPaths, max_age: Duration) {
    let Ok(children) = std::fs::read_dir(&paths.user_dir) else {
        return;
    };
    for child in children.flatten() {
        let name = child.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = child.path();
        if name == TMP_DIR_NAME {
            clean_tmp_dir(&path, max_age);
        } else if name.ends_with(UNINSTALL_BAK_SUFFIX) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

fn clean_tmp_dir(tmp: &Path, max_age: Duration) {
    let Ok(children) = std::fs::read_dir(tmp) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for child in children.flatten() {
        let stale = child
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age > max_age);
        if stale {
            if child.file_type().is_ok_and(|kind| kind.is_dir()) {
                let _ = std::fs::remove_dir_all(child.path());
            } else {
                let _ = std::fs::remove_file(child.path());
            }
        }
    }
}

// ==== 内部步骤 ====

fn collect_and_merge(
    builtin: &RegistrySource,
    registries_file: &Path,
    cache: &Mutex<IndexCache>,
    limits: FetchLimits,
) -> Result<Vec<SourceIndex>, MarketError> {
    let file_sources = source::load_file(registries_file)?;
    let sources = source::merged_sources(builtin, &file_sources);
    Ok(collect_indexes(&sources, cache, false, limits))
}

/// 条目定位（§3.5 步骤 1）：version 缺省取该 id 在所有源的 semver 最高；
/// source 限定来源；version 精确匹配按解析后相等判定。
pub(crate) fn resolve_entry<'a>(
    merged: &'a [MergedEntry],
    id: &str,
    version: Option<&str>,
    source_name: Option<&str>,
) -> Result<&'a MergedEntry, MarketError> {
    let id = id.trim();
    let source_name = source_name.map(str::trim);
    let candidates: Vec<&MergedEntry> = merged
        .iter()
        .filter(|merged_entry| merged_entry.entry.id == id)
        .filter(|merged_entry| source_name.is_none_or(|name| merged_entry.source == name))
        .collect();
    if candidates.is_empty() {
        return Err(match source_name {
            Some(source) => MarketError::NotFound(format!(
                "源 `{source}` 中不存在插件 `{id}`（或该源暂时不可用）"
            )),
            None => MarketError::NotFound(format!("插件 `{id}` 不在任何源中")),
        });
    }
    match version {
        Some(raw) => {
            let want = Version::parse(raw.trim()).ok_or_else(|| {
                MarketError::InvalidArgument(format!("版本号 `{raw}` 不是合法 semver"))
            })?;
            candidates
                .into_iter()
                .find(|merged_entry| merged_entry.entry.parsed_version == want)
                .ok_or_else(|| {
                    MarketError::NotFound(format!("插件 `{id}` 的版本 `{raw}` 不在索引中"))
                })
        }
        None => candidates
            .into_iter()
            .max_by(|a, b| a.entry.parsed_version.cmp(&b.entry.parsed_version))
            .ok_or_else(|| MarketError::NotFound(format!("插件 `{id}` 不在任何源中"))),
    }
}

/// 安装前预检（§3.5 步骤 2）：engines fail-closed（force 跳过）。
/// deps 不在此预检——M2 起缺失自动安装（见 `resolve_deps`），本地与所有
/// 启用源都缺失时报错并列出缺失 id。
fn preflight_engines(entry: &IndexEntry) -> Result<(), MarketError> {
    let engines = entry.effective_engines();
    if let Some(constraint) = engines.get("lite-code")
        && !satisfies_range(crate::version(), constraint)
    {
        return Err(MarketError::InvalidArgument(format!(
            "插件 `{}` 要求 lite-code {constraint}（当前 {}），可确认后 force 安装",
            entry.id,
            crate::version()
        )));
    }
    Ok(())
}

/// 依赖自动安装深度上限（§3.5 M2）：主插件计第 1 层，依赖链最深 8 层。
const MAX_DEP_DEPTH: usize = 8;

/// 依赖解析与自动安装（§3.5 M2）：
/// - 本地已满足（合法安装）的依赖直接跳过，零下载；已安装版本更低也不
///   自动升级（M2 最小语义，升级走 `ext_update`）；
/// - 缺失依赖按「同源优先」解析条目：先取要求方命中源（`RegistrySource`）
///   内的同 id 条目，未命中再以任意启用源兜底——取跨源去重后的 semver
///   最高版本（`merged` 按 id 去重保留的即最高版本条目）；
/// - 深度上限 [`MAX_DEP_DEPTH`]，超限 `invalid_argument` 并列出从主插件
///   到越界节点的链；环检测复用 §1 的 Tarjan 语义与报错文案
///   （`manifest::dependency_cycles` / `dependency_cycle_message`）；
/// - 链中任一依赖安装失败：已装依赖保留（每个依赖本身是合法插件），
///   主插件不落位，报错点名失败节点（错误变体保留，命令层错误码不变）。
fn resolve_deps<'a>(
    paths: &ExtPaths,
    root: &'a MergedEntry,
    collected: &'a [SourceIndex],
    merged: &'a [MergedEntry],
    node_available: bool,
    quota_mb: u64,
) -> Result<(), MarketError> {
    let installed = super::source::installed_versions(paths, node_available)?;
    let mut planner = DepPlanner {
        collected,
        merged,
        visited: installed.keys().cloned().collect(),
        raw_deps: HashMap::new(),
        plan: Vec::new(),
    };
    let mut missing: Vec<String> = Vec::new();
    let mut chain: Vec<String> = Vec::new();
    planner.visit(&root.entry, &root.source, 1, &mut chain, &mut missing)?;
    // post-order 最后入列的是主插件本身，安装计划只含依赖（依赖在前）。
    planner.plan.pop();

    // §1 语义：缺失依赖优先于环报错。
    if !missing.is_empty() {
        let listed = missing
            .iter()
            .map(|id| format!("`{id}`"))
            .collect::<Vec<_>>()
            .join("、");
        return Err(MarketError::InvalidArgument(format!(
            "插件 `{}` 依赖缺失:{listed}（本地与所有启用源均不存在，无法自动安装）",
            root.entry.id
        )));
    }

    // 环检测图：节点 = 主插件 + 待装依赖（本地已满足的是链终点，不进图），
    // 边 = 声明 deps 落在节点集内的部分——与 §1 磁盘校验同一构图语义。
    let graph: HashMap<String, Vec<String>> = planner
        .raw_deps
        .iter()
        .map(|(id, deps)| {
            let edges: Vec<String> = deps
                .iter()
                .filter(|dep| planner.raw_deps.contains_key(dep.as_str()))
                .cloned()
                .collect();
            (id.clone(), edges)
        })
        .collect();
    let mut cycles = crate::ext::manifest::dependency_cycles(&graph);
    if !cycles.is_empty() {
        cycles.sort();
        let mut group = cycles.remove(0);
        group.sort();
        return Err(MarketError::InvalidArgument(
            crate::ext::manifest::dependency_cycle_message(&group),
        ));
    }

    // deps 各自独立走完整安装流程（共享 tmp+rename 原子落位），
    // 主插件最后落位；任一失败即中止，已装依赖保留。
    for entry in &planner.plan {
        preflight_engines(entry)?;
        install_package(paths, entry, quota_mb)
            .map_err(|error| dep_install_error(&entry.id, &entry.version, error))?;
    }
    Ok(())
}

/// 依赖安装失败的上下文包装：保留原错误变体（命令层错误码不变），
/// 前缀点名失败节点（§3.5 M2「报错说明失败节点」）。
fn dep_install_error(id: &str, version: &str, error: MarketError) -> MarketError {
    let context = format!("依赖 `{id}`（{version}）安装失败");
    match error {
        MarketError::Io(message) => MarketError::Io(format!("{context}:{message}")),
        MarketError::Http(message) => MarketError::Http(format!("{context}:{message}")),
        MarketError::InvalidIndex(message) => {
            MarketError::InvalidIndex(format!("{context}:{message}"))
        }
        MarketError::NotFound(message) => MarketError::NotFound(format!("{context}:{message}")),
        MarketError::InvalidArgument(message) => {
            MarketError::InvalidArgument(format!("{context}:{message}"))
        }
        MarketError::ContractMismatch(message) => {
            MarketError::ContractMismatch(format!("{context}:{message}"))
        }
        MarketError::QuotaExceeded(message) => {
            MarketError::QuotaExceeded(format!("{context}:{message}"))
        }
        MarketError::NoHomeDir | MarketError::Config(_) => error,
    }
}

/// 依赖条目定位（§3.5 M2 同源优先）：先取要求方命中源内的同 id 条目；
/// 未命中再兜底跨源合并的最高 semver 版本。
fn pick_dep_entry<'a>(
    dep_id: &str,
    source: &'a str,
    collected: &'a [SourceIndex],
    merged: &'a [MergedEntry],
) -> Option<(&'a IndexEntry, &'a str)> {
    for source_index in collected {
        if source_index.source.name != source {
            continue;
        }
        let Ok(index) = &source_index.result else {
            continue;
        };
        if let Some(entry) = index.entries.iter().find(|entry| entry.id == dep_id) {
            return Some((entry, source));
        }
    }
    merged
        .iter()
        .find(|merged_entry| merged_entry.entry.id == dep_id)
        .map(|merged_entry| (&merged_entry.entry, merged_entry.source.as_str()))
}

struct DepPlanner<'a> {
    collected: &'a [SourceIndex],
    merged: &'a [MergedEntry],
    /// 本地已满足或已入计划的 id：DFS visited，防重复展开（零重复下载）。
    visited: HashSet<String>,
    /// 节点 → 声明的 deps（建环检测图用；边在构图时过滤到节点集）。
    raw_deps: HashMap<String, Vec<String>>,
    /// post-order 安装计划（依赖在前）；根（主插件）也在其中，调用方弹出。
    plan: Vec<IndexEntry>,
}

impl<'a> DepPlanner<'a> {
    /// DFS 展开单个节点。深度超限即报错（列出从主插件到越界节点的链）；
    /// 缺失（本地与所有源都没有）记入 `missing`；已 visited 的 id 是链终点。
    fn visit(
        &mut self,
        entry: &IndexEntry,
        source: &'a str,
        depth: usize,
        chain: &mut Vec<String>,
        missing: &mut Vec<String>,
    ) -> Result<(), MarketError> {
        chain.push(entry.id.clone());
        if depth > MAX_DEP_DEPTH {
            return Err(MarketError::InvalidArgument(format!(
                "依赖链深度超过 {MAX_DEP_DEPTH} 层上限:{}",
                chain.join(" -> ")
            )));
        }
        self.visited.insert(entry.id.clone());
        let deps = entry.effective_deps();
        self.raw_deps.insert(entry.id.clone(), deps.clone());
        for dep in deps {
            if self.visited.contains(&dep) {
                continue;
            }
            match pick_dep_entry(&dep, source, self.collected, self.merged) {
                Some((dep_entry, dep_source)) => {
                    self.visit(dep_entry, dep_source, depth + 1, chain, missing)?;
                }
                None => {
                    if !missing.contains(&dep) {
                        missing.push(dep);
                    }
                }
            }
        }
        chain.pop();
        self.plan.push(entry.clone());
        Ok(())
    }
}

/// 包安装主链（下载 → sha256 → 解压 → 一致性 → 配额 → 落位），
/// tmp 产物无论成败都清理。
pub(crate) fn install_package(
    paths: &ExtPaths,
    entry: &IndexEntry,
    quota_mb: u64,
) -> Result<(), MarketError> {
    std::fs::create_dir_all(&paths.user_dir)
        .map_err(|error| MarketError::Io(format!("创建插件目录失败:{error}")))?;
    let tmp_root = paths.user_dir.join(TMP_DIR_NAME);
    std::fs::create_dir_all(&tmp_root)
        .map_err(|error| MarketError::Io(format!("创建临时目录失败:{error}")))?;

    let tag = tmp_tag(&entry.id, &entry.version);
    let zip_path = tmp_root.join(format!("{tag}.zip"));
    let extract_root = tmp_root.join(format!("extract-{tag}"));

    let result = install_inner(paths, entry, quota_mb, &zip_path, &extract_root);
    // tmp 全删，最终目录零变更或完整新装。
    let _ = std::fs::remove_file(&zip_path);
    let _ = std::fs::remove_dir_all(&extract_root);
    result
}

fn install_inner(
    paths: &ExtPaths,
    entry: &IndexEntry,
    quota_mb: u64,
    zip_path: &Path,
    extract_root: &Path,
) -> Result<(), MarketError> {
    download_package(entry, zip_path, DOWNLOAD_TIMEOUT, MAX_PACKAGE_BYTES)?;
    verify_sha256(zip_path, &entry.sha256)?;
    let extracted = extract_package(
        zip_path,
        extract_root,
        &entry.id,
        MAX_PACKAGE_ENTRIES,
        MAX_EXTRACT_BYTES,
    )?;
    check_package_manifest(&extracted, entry)?;
    enforce_quota(paths, &entry.id, &extracted, quota_mb)?;
    place_plugin(paths, extracted, &entry.id)
}

fn tmp_tag(id: &str, version: &str) -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}-{}",
        id,
        version,
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn download_package(
    entry: &IndexEntry,
    dest: &Path,
    timeout: Duration,
    max_bytes: u64,
) -> Result<(), MarketError> {
    let bytes = fetch_url(&entry.dist_url, timeout, max_bytes)?;
    std::fs::write(dest, bytes).map_err(|error| MarketError::Io(format!("写入临时包失败:{error}")))
}

/// sha256 校验：与 index 声明不一致 → `contract_mismatch`（传输篡改/源被
/// 攻陷的 fail-closed 闸门，§3.7）。
pub(crate) fn verify_sha256(path: &Path, expected: &str) -> Result<(), MarketError> {
    let mut file =
        File::open(path).map_err(|error| MarketError::Io(format!("打开下载包失败:{error}")))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|error| MarketError::Io(format!("读取下载包失败:{error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let actual = hex_lower(&hasher.finalize());
    if actual != expected.to_ascii_lowercase() {
        return Err(MarketError::ContractMismatch(format!(
            "包 sha256 与 index 声明不符（预期 {expected}，实际 {actual}），已拒绝安装"
        )));
    }
    Ok(())
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// zip 解压（§3.2）：单一顶层目录 `<id>/`、zip-slip 防御（safe_zip_target）、
/// 仅 stored/deflate、拒绝加密与符号链接条目、4096 条目帽、256 MiB 解压帽。
/// 返回解压出的 `<extract_root>/<id>/` 目录。
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_package(
    zip_path: &Path,
    extract_root: &Path,
    expected_id: &str,
    max_entries: usize,
    max_bytes: u64,
) -> Result<PathBuf, MarketError> {
    let file =
        File::open(zip_path).map_err(|error| MarketError::Io(format!("打开包失败:{error}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| MarketError::InvalidArgument(format!("插件包不是合法 zip:{error}")))?;
    if archive.len() > max_entries {
        return Err(MarketError::InvalidArgument(format!(
            "包内条目数 {} 超过上限 {max_entries}",
            archive.len()
        )));
    }
    std::fs::create_dir_all(extract_root)
        .map_err(|error| MarketError::Io(format!("创建解压目录失败:{error}")))?;

    let mut top: Option<String> = None;
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let mut item = archive
            .by_index(index)
            .map_err(|error| MarketError::InvalidArgument(format!("读取包条目失败:{error}")))?;
        if item.encrypted() {
            return Err(MarketError::ContractMismatch(format!(
                "包内条目加密（M1 拒绝加密条目）:{}",
                item.name().map(|n| n.into_owned()).unwrap_or_default()
            )));
        }
        if !matches!(
            item.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        ) {
            return Err(MarketError::ContractMismatch(format!(
                "包内条目压缩法不受支持（仅 stored/deflate）:{}",
                item.name().map(|n| n.into_owned()).unwrap_or_default()
            )));
        }
        // 符号链接条目拒绝：手写解压只落地普通文件/目录，防止链出 tmp。
        if item
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(MarketError::ContractMismatch(
                "包内条目是符号链接，已拒绝".to_string(),
            ));
        }
        let name = item
            .name()
            .map_err(|error| MarketError::InvalidArgument(format!("包内条目名非法:{error}")))?;
        let clean = name.trim_end_matches('/');
        if clean.is_empty() {
            continue;
        }
        let target = safe_zip_target(extract_root, clean)?;
        let first = Path::new(clean)
            .components()
            .next()
            .and_then(|component| match component {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                _ => None,
            })
            .ok_or_else(|| MarketError::ContractMismatch(format!("条目 `{clean}` 非法")))?;
        match &top {
            None => top = Some(first),
            Some(existing) if existing == &first => {}
            Some(existing) => {
                return Err(MarketError::ContractMismatch(format!(
                    "包内必须是单一顶层目录 `<{expected_id}>/`，实际同时存在 `{existing}` 与 `{first}`"
                )));
            }
        }
        if item.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| MarketError::Io(format!("创建目录失败:{error}")))?;
            continue;
        }
        total = total.saturating_add(item.size());
        if total > max_bytes {
            return Err(MarketError::InvalidArgument(format!(
                "解压后体积超过 {max_bytes} 字节上限"
            )));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| MarketError::Io(format!("创建目录失败:{error}")))?;
        }
        let mut out = File::create(&target)
            .map_err(|error| MarketError::Io(format!("创建文件失败:{error}")))?;
        std::io::copy(&mut item, &mut out)
            .map_err(|error| MarketError::Io(format!("解压条目失败:{error}")))?;
    }

    let top = top.ok_or_else(|| MarketError::ContractMismatch("空包（无任何条目）".to_string()))?;
    if top != expected_id {
        return Err(MarketError::ContractMismatch(format!(
            "包内顶层目录 `{top}` 与 index id `{expected_id}` 不符"
        )));
    }
    let plugin_dir = extract_root.join(&top);
    let manifest_name = crate::ext::manifest::MANIFEST_FILE;
    if !plugin_dir.join(manifest_name).is_file() {
        return Err(MarketError::ContractMismatch(format!(
            "包内缺少 `{top}/{manifest_name}`"
        )));
    }
    Ok(plugin_dir)
}

/// 包内 plugin.json 与 index 条目比对 id/version（§3.5 步骤 4，index-包
/// 一致性绑定；不一致 `contract_mismatch` 拒绝）。
fn check_package_manifest(extracted_dir: &Path, entry: &IndexEntry) -> Result<(), MarketError> {
    let manifest_path = extracted_dir.join(crate::ext::manifest::MANIFEST_FILE);
    let file = File::open(&manifest_path)
        .map_err(|error| MarketError::Io(format!("读取包内清单失败:{error}")))?;
    let mut limited = file.take(crate::ext::manifest::MAX_MANIFEST_BYTES + 1);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| MarketError::Io(format!("读取包内清单失败:{error}")))?;
    if bytes.len() as u64 > crate::ext::manifest::MAX_MANIFEST_BYTES {
        return Err(MarketError::ContractMismatch(
            "包内 plugin.json 超过 256 KiB 上限".to_string(),
        ));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        MarketError::ContractMismatch(format!("包内 plugin.json 不是合法 JSON:{error}"))
    })?;
    let id = value.get("id").and_then(Value::as_str).map(str::trim);
    if id != Some(entry.id.as_str()) {
        return Err(MarketError::ContractMismatch(format!(
            "包内 plugin.json 的 id（{:?}）与 index 条目 `{}` 不一致",
            id, entry.id
        )));
    }
    let version = value.get("version").and_then(Value::as_str).map(str::trim);
    if version != Some(entry.version.as_str()) {
        return Err(MarketError::ContractMismatch(format!(
            "包内 plugin.json 的 version（{:?}）与 index 条目 `{}` 不一致",
            version, entry.version
        )));
    }
    Ok(())
}

/// 三级配额核算（§3.6）：落位前统计「解压后新体积 - 被替换旧体积 +
/// 现 extensions 用量（不含 .tmp 与 *.uninstall-bak）」。
fn enforce_quota(
    paths: &ExtPaths,
    id: &str,
    extracted_dir: &Path,
    quota_mb: u64,
) -> Result<(), MarketError> {
    let usage = extensions_usage(&paths.user_dir);
    let old = {
        let target = paths.user_dir.join(id);
        if target.is_dir() {
            dir_size(&target)
        } else {
            0
        }
    };
    let new_size = dir_size(extracted_dir);
    let quota_bytes = quota_mb.saturating_mul(1024 * 1024);
    let projected = usage.saturating_sub(old).saturating_add(new_size);
    if projected > quota_bytes {
        return Err(MarketError::QuotaExceeded(format!(
            "安装 `{id}` 后扩展目录将达 {} MiB，超出 {} MiB 配额（可调整 {QUOTA_ENV} 或先卸载插件）",
            projected / (1024 * 1024),
            quota_mb
        )));
    }
    Ok(())
}

/// extensions 目录用量：只计已落位的插件目录（排除 .tmp 与 *.uninstall-bak）。
pub(crate) fn extensions_usage(user_dir: &Path) -> u64 {
    let Ok(children) = std::fs::read_dir(user_dir) else {
        return 0;
    };
    children
        .flatten()
        .filter(|child| {
            let name = child.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            name != TMP_DIR_NAME && !name.ends_with(UNINSTALL_BAK_SUFFIX)
        })
        .map(|child| dir_size(&child.path()))
        .sum()
}

/// 目录体积（符号链接计 0，不跟随）。
pub(crate) fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    let Ok(children) = std::fs::read_dir(path) else {
        return 0;
    };
    children
        .flatten()
        .map(|child| dir_size(&child.path()))
        .sum()
}

/// 原子落位（§3.5 步骤 5）：tmp+rename 同分区原子；Windows rename 不覆盖，
/// 先把旧目录改名 `*.uninstall-bak`（失败可回滚），新目录就位后再删 bak。
fn place_plugin(paths: &ExtPaths, extracted_dir: PathBuf, id: &str) -> Result<(), MarketError> {
    let target = paths.user_dir.join(id);
    let bak = paths.user_dir.join(format!("{id}{UNINSTALL_BAK_SUFFIX}"));
    if bak.exists() {
        std::fs::remove_dir_all(&bak)
            .map_err(|error| MarketError::Io(format!("清理遗留备份失败:{error}")))?;
    }
    let mut swapped = false;
    if target.exists() {
        std::fs::rename(&target, &bak).map_err(|error| {
            MarketError::Io(format!(
                "旧版本目录改名失败（插件可能被宿主占用，可先重试）:{error}"
            ))
        })?;
        swapped = true;
    }
    match std::fs::rename(&extracted_dir, &target) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&bak);
            Ok(())
        }
        Err(error) => {
            if swapped {
                let _ = std::fs::rename(&bak, &target);
            }
            Err(MarketError::Io(format!(
                "落位失败（临时目录已回滚清理）:{error}"
            )))
        }
    }
}

/// 配额 env 读取：非正整数回落默认 1 GiB。
pub(crate) fn quota_from_env() -> u64 {
    std::env::var(QUOTA_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_QUOTA_MB)
}

fn list(paths: &ExtPaths, node_available: bool) -> Result<PluginList, MarketError> {
    crate::ext::list_plugins_at(paths, crate::ext::project_allowed(), node_available)
        .map_err(|error| MarketError::Io(error.to_string()))
}

fn installed_versions_sorted(
    paths: &ExtPaths,
    node_available: bool,
) -> Result<Vec<(String, String)>, MarketError> {
    let mut installed: Vec<(String, String)> =
        super::source::installed_versions(paths, node_available)?
            .into_iter()
            .collect();
    installed.sort();
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;
    use crate::ext::marketplace::testutil::{
        TempDir, build_zip, file_url, http_ok_bytes, serve_fn,
    };

    fn sha256_hex(bytes: &[u8]) -> String {
        hex_lower(&sha2::Sha256::digest(bytes))
    }

    fn plugin_manifest(id: &str, version: &str) -> Value {
        json!({
            "id": id,
            "version": version,
            "name": id,
            "runtime": "declarative",
            "description": "test plugin"
        })
    }

    struct Fixture {
        _temp: TempDir,
        paths: ExtPaths,
        reg_path: PathBuf,
        builtin: RegistrySource,
        index_file: PathBuf,
        package_file: PathBuf,
    }

    /// 标准夹具：file:// 单源 + 单一 declarative 插件包（`<id>/plugin.json`）。
    fn fixture(tag: &str, id: &str, version: &str) -> Fixture {
        let manifest = plugin_manifest(id, version);
        let zip = build_zip(&[(
            format!("{id}/plugin.json"),
            manifest.to_string().into_bytes(),
        )]);
        fixture_with_dist(tag, id, version, manifest, &zip, None)
    }

    /// 自定义 dist 字节（坏 zip / 坏清单 / sha256 场景）与 index 覆盖（deps/engines）。
    fn fixture_with_dist(
        tag: &str,
        id: &str,
        version: &str,
        manifest: Value,
        dist_bytes: &[u8],
        index_extra: Option<Value>,
    ) -> Fixture {
        let temp = TempDir::new(tag);
        let paths = ExtPaths::new(
            temp.path().join("extensions"),
            None,
            temp.path().join("extensions.json"),
        );
        let package_file = temp.path().join("pkg.zip");
        std::fs::write(&package_file, dist_bytes).expect("write package");
        let mut entry = json!({
            "id": id,
            "version": version,
            "manifest": manifest,
            "dist": {"url": file_url(&package_file), "sha256": sha256_hex(dist_bytes)}
        });
        if let Some(extra) = index_extra {
            merge_json(&mut entry, extra);
        }
        let index_file = temp.path().join("index.json");
        std::fs::write(
            &index_file,
            json!({"version": 1, "plugins": [entry]}).to_string(),
        )
        .expect("write index");
        Fixture {
            builtin: RegistrySource::new("official", file_url(&index_file)),
            reg_path: temp.path().join("reg.json"),
            _temp: temp,
            paths,
            index_file,
            package_file,
        }
    }

    /// 浅合并：把 extra 的键并入 target（仅一层，测试夹具用）。
    fn merge_json(target: &mut Value, extra: Value) {
        let Some(extra_obj) = extra.as_object() else {
            return;
        };
        let Some(target_obj) = target.as_object_mut() else {
            return;
        };
        for (key, value) in extra_obj {
            target_obj.insert(key.clone(), value.clone());
        }
    }

    fn run_install(
        fx: &Fixture,
        id: &str,
        version: Option<&str>,
        source: Option<&str>,
        force: bool,
        quota_mb: u64,
    ) -> Result<PluginList, MarketError> {
        install_at(
            &fx.paths,
            &fx.builtin,
            &fx.reg_path,
            &Mutex::new(IndexCache::new()),
            FetchLimits::default(),
            id,
            version,
            source,
            force,
            false,
            quota_mb,
        )
    }

    fn run_update(
        fx: &Fixture,
        id: Option<&str>,
        force: bool,
        quota_mb: u64,
    ) -> Result<PluginList, MarketError> {
        update_at(
            &fx.paths,
            &fx.builtin,
            &fx.reg_path,
            &Mutex::new(IndexCache::new()),
            FetchLimits::default(),
            id,
            force,
            false,
            quota_mb,
        )
    }

    fn installed_plugin(fx: &Fixture, id: &str) -> Option<crate::ext::PluginInfo> {
        list(&fx.paths, false)
            .ok()?
            .plugins
            .into_iter()
            .find(|p| p.id == id)
    }

    #[test]
    fn installs_from_file_source_end_to_end() {
        let manifest = json!({
            "id": "demo.app",
            "version": "0.1.0",
            "name": "Demo App",
            "runtime": "declarative",
            "description": "demo"
        });
        let zip = build_zip(&[
            ("demo.app/plugin.json", manifest.to_string().as_bytes()),
            ("demo.app/data/readme.txt", b"hello"),
        ]);
        let fx = fixture_with_dist("e2e", "demo.app", "0.1.0", manifest, &zip, None);

        let list =
            run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB).expect("install");
        let plugin = list
            .plugins
            .iter()
            .find(|p| p.id == "demo.app")
            .expect("plugin listed");
        assert!(
            plugin.error.is_none(),
            "list_plugins 校验链接管：error = {:?}",
            plugin.error
        );
        assert_eq!(plugin.version, "0.1.0");
        assert_eq!(plugin.name, "Demo App");
        let data = fx.paths.user_dir.join("demo.app/data/readme.txt");
        assert_eq!(std::fs::read_to_string(data).expect("data file"), "hello");
        // tmp 清理干净。
        let tmp = fx.paths.user_dir.join(TMP_DIR_NAME);
        assert!(
            tmp.read_dir().is_ok_and(|mut it| it.next().is_none()),
            "tmp cleaned"
        );
    }

    #[test]
    fn rejects_sha256_mismatch_and_places_nothing() {
        let manifest = plugin_manifest("demo.app", "0.1.0");
        let zip = build_zip(&[("demo.app/plugin.json", manifest.to_string().as_bytes())]);
        let fx = fixture_with_dist("bad-sha", "demo.app", "0.1.0", manifest.clone(), &zip, None);
        // 篡改 index 声明的 sha256。
        let index: Value =
            serde_json::from_str(&std::fs::read_to_string(&fx.index_file).expect("read"))
                .expect("json");
        let mut index = index;
        index["plugins"][0]["dist"]["sha256"] = json!("0".repeat(64));
        std::fs::write(&fx.index_file, index.to_string()).expect("rewrite index");

        let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("sha256 mismatch rejected");
        assert!(matches!(err, MarketError::ContractMismatch(_)));
        assert!(err.to_string().contains("sha256"));
        assert!(
            !fx.paths.user_dir.join("demo.app").exists(),
            "nothing placed"
        );
    }

    #[test]
    fn rejects_bad_zip() {
        let manifest = plugin_manifest("demo.app", "0.1.0");
        let fx = fixture_with_dist(
            "bad-zip",
            "demo.app",
            "0.1.0",
            manifest,
            b"not a zip at all",
            None,
        );
        let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("bad zip rejected");
        assert!(matches!(err, MarketError::InvalidArgument(_)));
    }

    #[test]
    fn rejects_zip_slip_entries() {
        for (tag, evil) in [
            ("slip-parent", "../evil"),
            ("slip-abs", "/abs"),
            ("slip-drive", "C:\\evil"),
        ] {
            let manifest = plugin_manifest("demo.app", "0.1.0");
            let zip = build_zip(&[
                ("demo.app/plugin.json", manifest.to_string().as_bytes()),
                (evil, b"owned"),
            ]);
            let fx = fixture_with_dist(tag, "demo.app", "0.1.0", manifest, &zip, None);
            let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
                .expect_err("zip slip rejected");
            assert!(
                matches!(err, MarketError::ContractMismatch(_)),
                "{evil} rejected as contract_mismatch: {err}"
            );
            assert!(!fx.paths.user_dir.join("demo.app").exists());
        }
    }

    #[test]
    fn rejects_manifest_drift_between_package_and_index() {
        let index_manifest = plugin_manifest("demo.app", "0.1.0");
        let package_manifest = json!({
            "id": "demo.app",
            "version": "0.2.0",
            "runtime": "declarative"
        });
        let zip = build_zip(&[(
            "demo.app/plugin.json",
            package_manifest.to_string().as_bytes(),
        )]);
        let fx = fixture_with_dist("drift", "demo.app", "0.1.0", index_manifest, &zip, None);
        let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("drift rejected");
        assert!(matches!(err, MarketError::ContractMismatch(_)));
        assert!(err.to_string().contains("version"));
    }

    #[test]
    fn rejects_wrong_top_level_dir() {
        let manifest = plugin_manifest("demo.app", "0.1.0");
        let zip = build_zip(&[("other.app/plugin.json", manifest.to_string().as_bytes())]);
        let fx = fixture_with_dist("topdir", "demo.app", "0.1.0", manifest, &zip, None);
        let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("top dir mismatch rejected");
        assert!(matches!(err, MarketError::ContractMismatch(_)));
        assert!(err.to_string().contains("demo.app"));
    }

    #[test]
    fn rejects_missing_manifest_inside_package() {
        let manifest = plugin_manifest("demo.app", "0.1.0");
        let zip = build_zip(&[("demo.app/only-data.txt", b"x")]);
        let fx = fixture_with_dist("nomanifest", "demo.app", "0.1.0", manifest, &zip, None);
        let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("missing plugin.json rejected");
        assert!(matches!(err, MarketError::ContractMismatch(_)));
    }

    #[test]
    fn rejects_unsupported_compression_method() {
        // bzip2 条目：zip crate 未启用该 feature 生成不了，把构造好的包字节里
        // Local File Header（签名 +8）与 Central Directory（签名 +10）的
        // compression method 改成 12（bzip2）触发拒绝路径（拒绝发生在解压前）。
        let manifest = plugin_manifest("demo.app", "0.1.0");
        let mut zip = build_zip(&[("demo.app/plugin.json", manifest.to_string().as_bytes())]);
        for (signature, offset) in [
            ([0x50, 0x4b, 0x03, 0x04], 8),
            ([0x50, 0x4b, 0x01, 0x02], 10),
        ] {
            let at = zip
                .windows(4)
                .position(|window| window == signature)
                .expect("zip header")
                + offset;
            zip[at] = 12; // bzip2
            zip[at + 1] = 0;
        }

        let fx = fixture_with_dist("bzip2", "demo.app", "0.1.0", manifest, &zip, None);
        let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("bzip2 entry rejected");
        assert!(matches!(err, MarketError::ContractMismatch(_)));
        assert!(err.to_string().contains("压缩法"));
    }

    #[test]
    fn gc_cleans_aged_tmp_and_uninstall_bak() {
        let fx = fixture("gc", "demo.app", "0.1.0");
        std::fs::create_dir_all(&fx.paths.user_dir).expect("user dir");
        let tmp = fx.paths.user_dir.join(TMP_DIR_NAME);
        std::fs::create_dir_all(&tmp).expect("tmp dir");
        std::fs::write(tmp.join("stale.zip"), b"garbage").expect("stale file");
        std::fs::create_dir_all(tmp.join("stale-extract")).expect("stale dir");
        let bak = fx.paths.user_dir.join("demo.app.uninstall-bak");
        std::fs::create_dir_all(&bak).expect("bak dir");

        gc_stale_with(&fx.paths, Duration::from_millis(0));
        assert!(!tmp.join("stale.zip").exists(), "aged tmp file cleaned");
        assert!(!tmp.join("stale-extract").exists(), "aged tmp dir cleaned");
        assert!(!bak.exists(), "uninstall-bak cleaned");
    }

    #[test]
    fn gc_keeps_fresh_tmp_within_default_window() {
        let fx = fixture("gc-fresh", "demo.app", "0.1.0");
        let tmp = fx.paths.user_dir.join(TMP_DIR_NAME);
        std::fs::create_dir_all(&tmp).expect("tmp dir");
        std::fs::write(tmp.join("fresh.zip"), b"garbage").expect("fresh file");

        gc_stale_with(&fx.paths, GC_TMP_MAX_AGE);
        assert!(tmp.join("fresh.zip").exists(), "fresh tmp kept");
    }

    #[test]
    fn package_download_cap_rejects_oversize() {
        let server = serve_fn(|_, _| vec![b'x'; 128]);
        let dist_url = format!("{}/big.zip", server.url);
        let manifest = plugin_manifest("demo.app", "0.1.0");
        let entry = IndexEntry {
            id: "demo.app".into(),
            version: "0.1.0".into(),
            parsed_version: Version::parse("0.1.0").unwrap(),
            manifest: manifest.clone(),
            dist_url,
            sha256: sha256_hex(&[b'x'; 128]),
            signature: String::new(),
            file_source: false,
            raw_engines: Default::default(),
            raw_deps: Vec::new(),
        };
        // 直接测下载档：cap 16 字节 < 128 字节响应。
        let temp = TempDir::new("dl-cap");
        let err = download_package(
            &entry,
            &temp.path().join("cap.zip"),
            Duration::from_secs(5),
            16,
        )
        .expect_err("oversize package rejected");
        assert!(matches!(err, MarketError::Http(_)));
    }

    #[test]
    fn extract_size_cap_rejects() {
        let manifest = plugin_manifest("demo.app", "0.1.0");
        let zip = build_zip(&[("demo.app/plugin.json", manifest.to_string().as_bytes())]);
        let temp = TempDir::new("extract-cap");
        let zip_path = temp.path().join("pkg.zip");
        std::fs::write(&zip_path, &zip).expect("write zip");
        let err = extract_package(
            &zip_path,
            &temp.path().join("out"),
            "demo.app",
            MAX_PACKAGE_ENTRIES,
            8,
        )
        .expect_err("extract cap rejected");
        assert!(matches!(err, MarketError::InvalidArgument(_)));
    }

    #[test]
    fn quota_cap_rejects_install() {
        let fx = fixture("quota", "demo.app", "0.1.0");
        let err = run_install(&fx, "demo.app", None, None, false, 0).expect_err("quota rejected");
        assert!(matches!(err, MarketError::QuotaExceeded(_)));
        assert!(!fx.paths.user_dir.join("demo.app").exists());
    }

    #[test]
    fn deps_missing_rejected_with_ids_and_force_skips() {
        let manifest = plugin_manifest("demo.app", "0.1.0");
        let zip = build_zip(&[("demo.app/plugin.json", manifest.to_string().as_bytes())]);
        let fx = fixture_with_dist(
            "deps",
            "demo.app",
            "0.1.0",
            manifest,
            &zip,
            Some(json!({"deps": ["ghost.one", "ghost.two"]})),
        );
        let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("deps missing rejected");
        assert!(matches!(err, MarketError::InvalidArgument(_)));
        assert!(err.to_string().contains("ghost.one"));
        assert!(err.to_string().contains("ghost.two"));

        let list = run_install(&fx, "demo.app", None, None, true, DEFAULT_QUOTA_MB)
            .expect("force installs");
        assert!(list.plugins.iter().any(|p| p.id == "demo.app"));
    }

    #[test]
    fn deps_satisfied_by_index_entry_passes() {
        let dep_manifest = plugin_manifest("demo.dep", "1.0.0");
        let dep_zip = build_zip(&[("demo.dep/plugin.json", dep_manifest.to_string().as_bytes())]);
        let app_manifest = plugin_manifest("demo.app", "0.1.0");
        let app_zip = build_zip(&[("demo.app/plugin.json", app_manifest.to_string().as_bytes())]);

        let temp = TempDir::new("deps-ok");
        let paths = ExtPaths::new(
            temp.path().join("extensions"),
            None,
            temp.path().join("extensions.json"),
        );
        let dep_pkg = temp.path().join("dep.zip");
        let app_pkg = temp.path().join("app.zip");
        std::fs::write(&dep_pkg, &dep_zip).expect("dep zip");
        std::fs::write(&app_pkg, &app_zip).expect("app zip");
        let index_file = temp.path().join("index.json");
        std::fs::write(
            &index_file,
            json!({"version": 1, "plugins": [
                {"id": "demo.dep", "version": "1.0.0", "manifest": dep_manifest,
                 "dist": {"url": file_url(&dep_pkg), "sha256": sha256_hex(&dep_zip)}},
                {"id": "demo.app", "version": "0.1.0", "manifest": app_manifest,
                 "deps": ["demo.dep"],
                 "dist": {"url": file_url(&app_pkg), "sha256": sha256_hex(&app_zip)}}
            ]})
            .to_string(),
        )
        .expect("index");
        let builtin = RegistrySource::new("official", file_url(&index_file));
        let reg_path = temp.path().join("reg.json");

        install_at(
            &paths,
            &builtin,
            &reg_path,
            &Mutex::new(IndexCache::new()),
            FetchLimits::default(),
            "demo.dep",
            None,
            None,
            false,
            false,
            DEFAULT_QUOTA_MB,
        )
        .expect("dep");
        install_at(
            &paths,
            &builtin,
            &reg_path,
            &Mutex::new(IndexCache::new()),
            FetchLimits::default(),
            "demo.app",
            None,
            None,
            false,
            false,
            DEFAULT_QUOTA_MB,
        )
        .expect("app with dep");
    }

    #[test]
    fn engines_unsatisfied_rejected_and_force_skips() {
        let manifest = json!({
            "id": "demo.app",
            "version": "0.1.0",
            "runtime": "declarative",
            "engines": {"lite-code": ">=99.0.0"}
        });
        let zip = build_zip(&[("demo.app/plugin.json", manifest.to_string().as_bytes())]);
        let fx = fixture_with_dist("engines", "demo.app", "0.1.0", manifest, &zip, None);
        let err = run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("engines rejected");
        assert!(matches!(err, MarketError::InvalidArgument(_)));
        assert!(err.to_string().contains(">=99.0.0"));

        run_install(&fx, "demo.app", None, None, true, DEFAULT_QUOTA_MB)
            .expect("force skips engines");
    }

    #[test]
    fn version_and_source_resolution_errors_are_specific() {
        let fx = fixture("resolve", "demo.app", "0.1.0");
        let err = run_install(
            &fx,
            "demo.app",
            Some("9.9.9"),
            None,
            false,
            DEFAULT_QUOTA_MB,
        )
        .expect_err("unknown version");
        assert!(matches!(err, MarketError::NotFound(_)));
        let err = run_install(
            &fx,
            "demo.app",
            Some("not-semver"),
            None,
            false,
            DEFAULT_QUOTA_MB,
        )
        .expect_err("bad version arg");
        assert!(matches!(err, MarketError::InvalidArgument(_)));
        let err = run_install(&fx, "demo.app", None, Some("nope"), false, DEFAULT_QUOTA_MB)
            .expect_err("unknown source");
        assert!(matches!(err, MarketError::NotFound(_)));
        let err = run_install(&fx, "ghost.app", None, None, false, DEFAULT_QUOTA_MB)
            .expect_err("unknown plugin");
        assert!(matches!(err, MarketError::NotFound(_)));
        // 精确版本命中可装。
        run_install(
            &fx,
            "demo.app",
            Some("0.1.0"),
            None,
            false,
            DEFAULT_QUOTA_MB,
        )
        .expect("exact version");
    }

    #[test]
    fn reinstall_replaces_and_keeps_disabled() {
        let fx = fixture("reinstall", "demo.app", "0.1.0");
        run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB).expect("first");
        ExtensionsState::new(fx.paths.state_path.clone())
            .set_enabled("demo.app", false)
            .expect("disable");
        run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB).expect("reinstall");
        let plugin = installed_plugin(&fx, "demo.app").expect("still installed");
        assert!(!plugin.enabled, "install 不动 disabled 集合");
    }

    #[test]
    fn invalid_package_manifest_surfaces_via_list_validation() {
        // 包内清单与 index 一致、JSON 合法，但过不了 manifest 校验链
        // （runtime=node 缺 main）→ 落位成功，list 以 error 字段暴露。
        let manifest = json!({
            "id": "demo.app",
            "version": "0.1.0",
            "runtime": "node"
        });
        let zip = build_zip(&[("demo.app/plugin.json", manifest.to_string().as_bytes())]);
        let fx = fixture_with_dist("badmanifest", "demo.app", "0.1.0", manifest, &zip, None);
        run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB).expect("placement ok");
        let plugin = installed_plugin(&fx, "demo.app").expect("listed");
        assert!(plugin.error.is_some(), "坏包以 error 字段暴露而不破坏宿主");
    }

    #[test]
    fn uninstall_removes_dir_and_disabled_entry() {
        let fx = fixture("uninstall", "demo.app", "0.1.0");
        run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB).expect("install");
        ExtensionsState::new(fx.paths.state_path.clone())
            .set_enabled("demo.app", false)
            .expect("disable");
        assert!(fx.paths.state_path.exists(), "disabled 状态已持久化");

        uninstall(&fx.paths, "demo.app", false).expect("uninstall");
        assert!(!fx.paths.user_dir.join("demo.app").exists(), "dir removed");
        let disabled = ExtensionsState::new(fx.paths.state_path.clone())
            .load()
            .expect("load state");
        assert!(!disabled.contains("demo.app"), "disabled cleaned");
        assert!(!fx.paths.user_dir.join("demo.app.uninstall-bak").exists());
    }

    #[test]
    fn uninstall_rejects_bad_id_and_unknown_plugin() {
        let fx = fixture("uninstall-guard", "demo.app", "0.1.0");
        let err = uninstall(&fx.paths, "../evil", false).expect_err("traversal id rejected");
        assert!(matches!(err, MarketError::InvalidArgument(_)));
        let err = uninstall(&fx.paths, "ghost.app", false).expect_err("unknown plugin");
        assert!(matches!(err, MarketError::NotFound(_)));
    }

    #[test]
    fn update_installs_only_strictly_newer_and_keeps_disabled() {
        let fx = fixture("update", "demo.app", "0.1.0");
        run_install(&fx, "demo.app", None, None, false, DEFAULT_QUOTA_MB).expect("install 0.1.0");
        ExtensionsState::new(fx.paths.state_path.clone())
            .set_enabled("demo.app", false)
            .expect("disable");

        // 源升级到 0.2.0（覆写包与 index；新缓存避开旧 sha）。
        let manifest = plugin_manifest("demo.app", "0.2.0");
        let zip = build_zip(&[("demo.app/plugin.json", manifest.to_string().as_bytes())]);
        std::fs::write(&fx.package_file, &zip).expect("rewrite package");
        std::fs::write(
            &fx.index_file,
            json!({"version": 1, "plugins": [{
                "id": "demo.app", "version": "0.2.0", "manifest": manifest,
                "dist": {"url": file_url(&fx.package_file), "sha256": sha256_hex(&zip)}
            }]})
            .to_string(),
        )
        .expect("rewrite index");

        let list = run_update(&fx, None, false, DEFAULT_QUOTA_MB).expect("update all");
        let plugin = list
            .plugins
            .iter()
            .find(|p| p.id == "demo.app")
            .expect("listed");
        assert_eq!(plugin.version, "0.2.0");
        assert!(!plugin.enabled, "update 保留 disabled");

        // 再更新：无严格更新版本 → no-op，仍是 0.2.0。
        let list = run_update(&fx, None, false, DEFAULT_QUOTA_MB).expect("update again no-op");
        assert_eq!(
            list.plugins
                .iter()
                .find(|p| p.id == "demo.app")
                .unwrap()
                .version,
            "0.2.0"
        );

        // 指定 id：已是最新 → no-op。
        run_update(&fx, Some("demo.app"), false, DEFAULT_QUOTA_MB).expect("specific no-op");
        // 指定未安装 id → NotFound。
        let err = run_update(&fx, Some("ghost.app"), false, DEFAULT_QUOTA_MB).expect_err("unknown");
        assert!(matches!(err, MarketError::NotFound(_)));
    }

    #[test]
    fn download_via_loopback_http_end_to_end() {
        let manifest = plugin_manifest("demo.net", "0.3.0");
        let zip = build_zip(&[("demo.net/plugin.json", manifest.to_string().as_bytes())]);
        let sha = sha256_hex(&zip);
        let server = serve_fn(move |_, _| http_ok_bytes(&zip));
        let temp = TempDir::new("net-install");
        let paths = ExtPaths::new(
            temp.path().join("extensions"),
            None,
            temp.path().join("extensions.json"),
        );
        let index_file = temp.path().join("index.json");
        std::fs::write(
            &index_file,
            json!({"version": 1, "plugins": [{
                "id": "demo.net", "version": "0.3.0", "manifest": manifest,
                "dist": {"url": format!("{}/pkg.zip", server.url), "sha256": sha}
            }]})
            .to_string(),
        )
        .expect("index");
        let builtin = RegistrySource::new("official", file_url(&index_file));
        install_at(
            &paths,
            &builtin,
            &temp.path().join("reg.json"),
            &Mutex::new(IndexCache::new()),
            FetchLimits::default(),
            "demo.net",
            None,
            None,
            false,
            false,
            DEFAULT_QUOTA_MB,
        )
        .expect("install over loopback http");
        assert!(paths.user_dir.join("demo.net/plugin.json").is_file());
    }

    // ==== M2：deps 自动安装（§3.5） ====

    /// 多插件夹具描述：id/version/deps + 可自定义的包字节（坏包失败用例）。
    struct MultiPlugin {
        id: String,
        version: String,
        deps: Vec<String>,
        dist: Option<Vec<u8>>,
    }

    fn mp(id: &str, version: &str, deps: &[&str]) -> MultiPlugin {
        MultiPlugin {
            id: id.to_string(),
            version: version.to_string(),
            deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
            dist: None,
        }
    }

    /// 多插件 file:// 夹具：一份 index + 每插件一个包，可再注册镜像源。
    struct MultiFixture {
        _temp: TempDir,
        paths: ExtPaths,
        reg_path: PathBuf,
        builtin: RegistrySource,
        index_file: PathBuf,
    }

    impl MultiFixture {
        fn new(tag: &str, plugins: &[MultiPlugin]) -> Self {
            let temp = TempDir::new(tag);
            let paths = ExtPaths::new(
                temp.path().join("extensions"),
                None,
                temp.path().join("extensions.json"),
            );
            let index_file = temp.path().join("index.json");
            write_index(&index_file, temp.path(), plugins);
            MultiFixture {
                builtin: RegistrySource::new("official", file_url(&index_file)),
                reg_path: temp.path().join("reg.json"),
                _temp: temp,
                paths,
                index_file,
            }
        }

        /// 追加一份镜像源 index（多源用例），返回其路径。
        fn add_index(&self, tag: &str, plugins: &[MultiPlugin]) -> PathBuf {
            let file = self._temp.path().join(format!("index-{tag}.json"));
            write_index(&file, self._temp.path(), plugins);
            file
        }

        /// 覆写主 index（源升级用例）。
        fn rewrite_index(&self, plugins: &[MultiPlugin]) {
            write_index(&self.index_file, self._temp.path(), plugins);
        }

        fn install(&self, id: &str) -> Result<PluginList, MarketError> {
            install_at(
                &self.paths,
                &self.builtin,
                &self.reg_path,
                &Mutex::new(IndexCache::new()),
                FetchLimits::default(),
                id,
                None,
                None,
                false,
                false,
                DEFAULT_QUOTA_MB,
            )
        }

        fn update_all(&self) -> Result<PluginList, MarketError> {
            update_at(
                &self.paths,
                &self.builtin,
                &self.reg_path,
                &Mutex::new(IndexCache::new()),
                FetchLimits::default(),
                None,
                false,
                false,
                DEFAULT_QUOTA_MB,
            )
        }

        fn installed(&self, id: &str) -> Option<crate::ext::PluginInfo> {
            list(&self.paths, false)
                .ok()?
                .plugins
                .into_iter()
                .find(|plugin| plugin.id == id)
        }
    }

    /// 写源清单文件（多源用例注册镜像源）。
    fn write_sources(reg_path: &Path, sources: &[RegistrySource]) {
        std::fs::write(reg_path, json!({"sources": sources}).to_string())
            .expect("write registries");
    }

    /// 写 index + 每个插件的 file:// 包；deps 同时写进 manifest 的
    /// extensionDependencies（§1 权威位置）与条目级 deps（§3.1 冗余字段）。
    fn write_index(index_file: &Path, root: &Path, plugins: &[MultiPlugin]) {
        let mut entries = Vec::new();
        for plugin in plugins {
            let mut manifest = plugin_manifest(&plugin.id, &plugin.version);
            if !plugin.deps.is_empty() {
                manifest["extensionDependencies"] = json!(plugin.deps);
            }
            let zip = plugin.dist.clone().unwrap_or_else(|| {
                build_zip(&[(
                    format!("{}/plugin.json", plugin.id),
                    manifest.to_string().into_bytes(),
                )])
            });
            let package_file = root.join(format!("{}-{}.zip", plugin.id, plugin.version));
            std::fs::write(&package_file, &zip).expect("write package");
            let mut entry = json!({
                "id": plugin.id,
                "version": plugin.version,
                "manifest": manifest,
                "dist": {"url": file_url(&package_file), "sha256": sha256_hex(&zip)}
            });
            if !plugin.deps.is_empty() {
                entry["deps"] = json!(plugin.deps);
            }
            entries.push(entry);
        }
        std::fs::write(
            index_file,
            json!({"version": 1, "plugins": entries}).to_string(),
        )
        .expect("write index");
    }

    /// 生成 n 级依赖链 chain.1 → chain.2 → … → chain.n。
    fn chain_plugins(prefix: &str, count: usize) -> Vec<MultiPlugin> {
        (1..=count)
            .map(|index| MultiPlugin {
                id: format!("{prefix}.{index}"),
                version: "1.0.0".to_string(),
                deps: if index < count {
                    vec![format!("{prefix}.{}", index + 1)]
                } else {
                    Vec::new()
                },
                dist: None,
            })
            .collect()
    }

    #[test]
    fn deps_chain_installs_transitively() {
        let fx = MultiFixture::new(
            "m2-chain",
            &[
                mp("chain.a", "1.0.0", &["chain.b"]),
                mp("chain.b", "1.0.0", &["chain.c"]),
                mp("chain.c", "1.0.0", &[]),
            ],
        );
        let list = fx.install("chain.a").expect("install a");
        for id in ["chain.a", "chain.b", "chain.c"] {
            let plugin = list
                .plugins
                .iter()
                .find(|plugin| plugin.id == id)
                .expect("listed");
            assert!(plugin.error.is_none(), "{id} 应通过校验:{:?}", plugin.error);
        }
        assert!(fx.paths.user_dir.join("chain.c/plugin.json").is_file());
    }

    #[test]
    fn deps_prefer_same_source_over_higher_version_elsewhere() {
        let fx = MultiFixture::new(
            "m2-same-src",
            &[
                mp("demo.app", "1.0.0", &["demo.dep"]),
                mp("demo.dep", "1.0.0", &[]),
            ],
        );
        // 镜像源有更高版本，但非同源 → 不应被选中。
        let mirror_index = fx.add_index("mirror", &[mp("demo.dep", "2.0.0", &[])]);
        write_sources(
            &fx.reg_path,
            &[RegistrySource::new("mirror", file_url(&mirror_index))],
        );

        fx.install("demo.app").expect("install app");
        let dep = fx.installed("demo.dep").expect("dep installed");
        assert_eq!(
            dep.version, "1.0.0",
            "同源优先：装命中源内的版本，而非跨源最高 2.0.0"
        );
    }

    #[test]
    fn deps_fall_back_to_highest_version_across_sources() {
        let fx = MultiFixture::new("m2-fallback", &[mp("demo.app", "1.0.0", &["demo.dep"])]);
        let mirror_index = fx.add_index("mirror", &[mp("demo.dep", "2.0.0", &[])]);
        write_sources(
            &fx.reg_path,
            &[RegistrySource::new("mirror", file_url(&mirror_index))],
        );

        fx.install("demo.app").expect("install app");
        let dep = fx.installed("demo.dep").expect("dep installed");
        assert_eq!(
            dep.version, "2.0.0",
            "命中源未命中 → 兜底任意启用源的跨源最高版本"
        );
    }

    #[test]
    fn deps_already_installed_lower_version_not_upgraded() {
        let fx = MultiFixture::new(
            "m2-noupgrade",
            &[
                mp("demo.app", "1.0.0", &["demo.dep"]),
                mp("demo.dep", "2.0.0", &[]),
            ],
        );
        // 本地预置更低版本（合法 declarative 清单）→ 视为已满足。
        let dep_dir = fx.paths.user_dir.join("demo.dep");
        std::fs::create_dir_all(&dep_dir).expect("dep dir");
        std::fs::write(
            dep_dir.join("plugin.json"),
            plugin_manifest("demo.dep", "1.0.0").to_string(),
        )
        .expect("dep manifest");

        fx.install("demo.app").expect("install app");
        let dep = fx.installed("demo.dep").expect("dep listed");
        assert_eq!(
            dep.version, "1.0.0",
            "M2 不自动升级已安装依赖（升级走 ext_update）"
        );
        assert!(fx.installed("demo.app").is_some());
    }

    #[test]
    fn deps_depth_nine_rejected_listing_chain() {
        let fx = MultiFixture::new("m2-depth9", &chain_plugins("deep", 9));
        let err = fx.install("deep.1").expect_err("depth 9 rejected");
        assert!(matches!(err, MarketError::InvalidArgument(_)));
        let message = err.to_string();
        assert!(message.contains("深度"), "{message}");
        assert!(message.contains("deep.1"), "{message}");
        assert!(message.contains("deep.9"), "{message}");
        // 规划期即拒绝：链上任何插件都不落位。
        assert!(!fx.paths.user_dir.join("deep.1").exists());
        assert!(!fx.paths.user_dir.join("deep.9").exists());
    }

    #[test]
    fn deps_depth_eight_allowed() {
        let fx = MultiFixture::new("m2-depth8", &chain_plugins("deep", 8));
        let list = fx.install("deep.1").expect("depth 8 ok");
        assert_eq!(list.plugins.len(), 8, "8 层链全部装齐");
        for plugin in &list.plugins {
            assert!(plugin.error.is_none(), "{:?}", plugin.error);
        }
    }

    #[test]
    fn deps_cycle_rejected_with_ring_members() {
        let fx = MultiFixture::new(
            "m2-cycle",
            &[
                mp("loop.a", "1.0.0", &["loop.b"]),
                mp("loop.b", "1.0.0", &["loop.a"]),
            ],
        );
        let err = fx.install("loop.a").expect_err("cycle rejected");
        assert!(matches!(err, MarketError::InvalidArgument(_)));
        let message = err.to_string();
        assert!(message.contains("循环依赖"), "{message}");
        assert!(message.contains("`loop.a`"), "{message}");
        assert!(message.contains("`loop.b`"), "{message}");
        assert!(!fx.paths.user_dir.join("loop.a").exists(), "环上插件不落位");
        assert!(!fx.paths.user_dir.join("loop.b").exists());
    }

    #[test]
    fn deps_failure_keeps_installed_siblings_and_skips_main() {
        let bad = MultiPlugin {
            id: "demo.bad".to_string(),
            version: "1.0.0".to_string(),
            deps: Vec::new(),
            dist: Some(b"not a zip at all".to_vec()),
        };
        let fx = MultiFixture::new(
            "m2-fail",
            &[
                mp("demo.main", "1.0.0", &["demo.good", "demo.bad"]),
                mp("demo.good", "1.0.0", &[]),
                bad,
            ],
        );
        let err = fx.install("demo.main").expect_err("bad dep fails");
        let message = err.to_string();
        assert!(message.contains("demo.bad"), "报错说明失败节点:{message}");
        assert!(fx.installed("demo.good").is_some(), "先装成功的依赖保留");
        assert!(
            !fx.paths.user_dir.join("demo.bad").exists(),
            "失败依赖不落位"
        );
        assert!(
            !fx.paths.user_dir.join("demo.main").exists(),
            "主插件不落位"
        );
    }

    #[test]
    fn deps_satisfied_locally_skips_package_download() {
        let temp = TempDir::new("m2-zero-dl");
        let paths = ExtPaths::new(
            temp.path().join("extensions"),
            None,
            temp.path().join("extensions.json"),
        );
        let dep_manifest = plugin_manifest("demo.dep", "1.0.0");
        let dep_zip = build_zip(&[("demo.dep/plugin.json", dep_manifest.to_string().as_bytes())]);
        let mut app_manifest = plugin_manifest("demo.app", "1.0.0");
        app_manifest["extensionDependencies"] = json!(["demo.dep"]);
        let app_zip = build_zip(&[("demo.app/plugin.json", app_manifest.to_string().as_bytes())]);

        // 每插件一个 loopback server：hits 即包下载次数（index 走 file://）。
        let dep_server = serve_fn({
            let dep_zip = dep_zip.clone();
            move |_, _| http_ok_bytes(&dep_zip)
        });
        let app_server = serve_fn({
            let app_zip = app_zip.clone();
            move |_, _| http_ok_bytes(&app_zip)
        });
        let index_file = temp.path().join("index.json");
        std::fs::write(
            &index_file,
            json!({"version": 1, "plugins": [
                {"id": "demo.app", "version": "1.0.0", "manifest": app_manifest,
                 "deps": ["demo.dep"],
                 "dist": {"url": format!("{}/pkg.zip", app_server.url),
                          "sha256": sha256_hex(&app_zip)}},
                {"id": "demo.dep", "version": "1.0.0", "manifest": dep_manifest,
                 "dist": {"url": format!("{}/pkg.zip", dep_server.url),
                          "sha256": sha256_hex(&dep_zip)}}
            ]})
            .to_string(),
        )
        .expect("index");
        let builtin = RegistrySource::new("official", file_url(&index_file));
        let reg_path = temp.path().join("reg.json");
        let install = |id: &str| {
            install_at(
                &paths,
                &builtin,
                &reg_path,
                &Mutex::new(IndexCache::new()),
                FetchLimits::default(),
                id,
                None,
                None,
                false,
                false,
                DEFAULT_QUOTA_MB,
            )
        };

        install("demo.dep").expect("install dep");
        assert_eq!(dep_server.hits.load(Ordering::Relaxed), 1, "dep 首次下载");
        install("demo.app").expect("install app");
        assert_eq!(
            dep_server.hits.load(Ordering::Relaxed),
            1,
            "本地已满足 → dep 零下载"
        );
        assert_eq!(app_server.hits.load(Ordering::Relaxed), 1, "app 下载一次");
        assert!(paths.user_dir.join("demo.app/plugin.json").is_file());
    }

    #[test]
    fn update_backfills_missing_deps() {
        let fx = MultiFixture::new(
            "m2-update",
            &[mp("demo.app", "0.1.0", &[]), mp("demo.dep", "1.0.0", &[])],
        );
        fx.install("demo.app").expect("install 0.1.0");
        assert!(fx.installed("demo.dep").is_none());
        // 源升级：0.2.0 新增依赖 demo.dep（本地缺失）→ update 补装。
        fx.rewrite_index(&[
            mp("demo.app", "0.2.0", &["demo.dep"]),
            mp("demo.dep", "1.0.0", &[]),
        ]);

        let list = fx.update_all().expect("update");
        let app = list
            .plugins
            .iter()
            .find(|plugin| plugin.id == "demo.app")
            .expect("app listed");
        assert_eq!(app.version, "0.2.0");
        assert!(
            fx.installed("demo.dep").is_some(),
            "update 路径同样补装缺失依赖"
        );
    }
}
