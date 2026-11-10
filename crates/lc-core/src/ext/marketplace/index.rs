//! index.json 线格式解析与校验（docs/plans/plugin-marketplace.md §3.1）。
//!
//! 校验粒度分两档：
//! - **整份 fail-closed**（`contract_mismatch`）：线版本 ≠ 1、同 index 重复 id、
//!   响应超 4 MiB、条目数超 4096、JSON 整体无法解析；
//! - **条目级跳过并计数**：semver 解析失败、sha256 非 64hex、dist.url 不在
//!   白名单（https / 回环 http / file）、内嵌 manifest 与条目 id/version 漂移。
//!   坏条目不毒化整份 index（§3.1「防混淆攻击」只升级重复 id 一档）。

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::version::Version;

use super::{MarketError, is_allowed_url};
use crate::ext::manifest;

/// index 线格式版本。客户端只接受 `1`（fail-closed，同 SqlDialect 闸门风格）。
pub(crate) const INDEX_LINE_VERSION: u32 = 1;
/// 单源 index 响应上限（4 MiB，同 updater 的 MAX_MANIFEST_BYTES 上限模式）。
pub(crate) const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;
/// 单源条目上限（4096；超出按整份无效处理，防恶意巨型响应）。
pub(crate) const MAX_INDEX_ENTRIES: usize = 4096;

/// 签名状态徽标（§3.4）。`Enforced` 为 M3 强制档预留，M1 只会产出
/// `Signed`（signature 非空）/ `Unsigned`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SignatureState {
    Signed,
    Unsigned,
    Enforced,
}

/// 市场搜索结果条目（`ext_search` 的 `results` 元素，custom DTO camelCase）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketPlugin {
    pub id: String,
    pub version: String,
    pub description: String,
    pub runtime: String,
    /// `engines` 冗余字段（安装前 fail-closed 预检用；与 manifest 不一致时
    /// 以 manifest 为准，见 `IndexEntry::effective_engines`）。
    pub engines: BTreeMap<String, String>,
    /// 依赖插件 id 列表（M2 起安装时按此自动补装缺失依赖，见 install.rs）。
    pub deps: Vec<String>,
    /// 提示用权限列表（Node 插件本质是可信代码，§7）。
    pub permissions: Vec<String>,
    /// 命中该条目的源名（多源按 id 去重后 semver 最高的来源）。
    pub source: String,
    /// dist.url 为 `file://` 时 `true`——UI 须标「本地源，自行承担信任」。
    pub file_source: bool,
    pub signature_state: SignatureState,
    /// 本地已安装版本；未安装为 `null`。
    pub installed_version: Option<String>,
    /// 已安装且 index 版本严格大于本地版本。
    pub update_available: bool,
}

/// `ext_search` 返回体：合并去重后的结果 + 暂时不可用的源名（UI 提示
/// 「源 X 暂时不可用」，§3.3「某源失败不毒化结果」）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSearchResult {
    pub results: Vec<MarketPlugin>,
    pub stale_sources: Vec<String>,
}

/// 解析后的 index 条目（内部表示，已过完条目级校验）。
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub id: String,
    pub version: String,
    /// 解析后的版本，供 semver 排序/比较（更新「严格大于」判定）。
    pub parsed_version: Version,
    /// §1 plugin.json 的逐字内嵌副本（存在性 + id/version 一致性已校验）。
    pub manifest: Value,
    pub dist_url: String,
    pub sha256: String,
    /// minisign 签名（trusted comment + base64）；空串 = 未签名（M1 不校验）。
    pub signature: String,
    pub file_source: bool,
    pub(crate) raw_engines: BTreeMap<String, String>,
    pub(crate) raw_deps: Vec<String>,
}

impl IndexEntry {
    /// 展示名：内嵌 manifest 的 `name`，缺省回落 id（同 manifest 校验链规则）。
    pub fn name(&self) -> &str {
        self.manifest
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.id)
    }

    pub fn description(&self) -> &str {
        self.manifest
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    pub fn runtime(&self) -> &str {
        self.manifest
            .get("runtime")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    pub fn permissions(&self) -> Vec<String> {
        self.manifest
            .get("permissions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// engines 以 manifest 为准（§3.1），条目级 `engines` 仅作冗余备份。
    pub fn effective_engines(&self) -> BTreeMap<String, String> {
        manifest_string_map(self.manifest.get("engines"))
            .unwrap_or_else(|| self.raw_engines.clone())
    }

    /// deps 以 manifest 的 `extensionDependencies` 为准，条目级 `deps` 备份。
    pub fn effective_deps(&self) -> Vec<String> {
        self.manifest
            .get("extensionDependencies")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .filter(|deps: &Vec<String>| !deps.is_empty())
            .unwrap_or_else(|| self.raw_deps.clone())
    }
}

/// 解析成功的一份 index。`skipped` 记录被条目级校验跳过的数量（诊断用）。
#[derive(Debug, Clone)]
pub struct ParsedIndex {
    pub entries: Vec<IndexEntry>,
    pub skipped: usize,
}

#[derive(Debug, serde::Deserialize)]
struct RawIndex {
    version: u32,
    #[serde(default)]
    plugins: Vec<Value>,
}

#[derive(Debug, serde::Deserialize)]
struct RawEntry {
    id: String,
    version: String,
    manifest: Value,
    dist: RawDist,
    #[serde(default)]
    signature: String,
    #[serde(default)]
    engines: BTreeMap<String, String>,
    #[serde(default)]
    deps: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct RawDist {
    url: String,
    sha256: String,
}

/// 解析并校验 index JSON（§4 测试矩阵：合法/缺字段/重复 id/非法 semver/
/// sha256 非 64hex/manifest 漂移/超大/条目数超限/线版本 ≠ 1）。
pub fn parse_index(json: &[u8]) -> Result<ParsedIndex, MarketError> {
    if json.len() as u64 > MAX_INDEX_BYTES {
        return Err(MarketError::InvalidIndex(format!(
            "index 超过 {} MiB 上限（{} 字节），请换源或升级客户端",
            MAX_INDEX_BYTES / (1024 * 1024),
            json.len()
        )));
    }
    let raw: RawIndex = serde_json::from_slice(json)
        .map_err(|error| MarketError::InvalidIndex(format!("JSON 解析失败:{error}")))?;
    if raw.version != INDEX_LINE_VERSION {
        return Err(MarketError::InvalidIndex(format!(
            "线格式版本 {} 不受支持（客户端只接受 {INDEX_LINE_VERSION}）",
            raw.version
        )));
    }
    if raw.plugins.len() > MAX_INDEX_ENTRIES {
        return Err(MarketError::InvalidIndex(format!(
            "条目数 {} 超过上限 {MAX_INDEX_ENTRIES}",
            raw.plugins.len()
        )));
    }

    let mut entries = Vec::new();
    let mut skipped = 0_usize;
    for value in raw.plugins {
        match parse_entry(value) {
            Ok(entry) => {
                // 同 index 内重复 id → 整份无效（防混淆攻击）。
                if entries
                    .iter()
                    .any(|existing: &IndexEntry| existing.id == entry.id)
                {
                    return Err(MarketError::InvalidIndex(format!(
                        "重复插件 id `{}`（同 index 内禁止重复）",
                        entry.id
                    )));
                }
                entries.push(entry);
            }
            Err(()) => skipped += 1,
        }
    }
    Ok(ParsedIndex { entries, skipped })
}

/// 条目级校验：任何不合规 → `Err(())`（调用方跳过并计数）。
fn parse_entry(value: Value) -> Result<IndexEntry, ()> {
    let raw: RawEntry = serde_json::from_value(value).map_err(|_| ())?;
    let id = raw.id.trim();
    if !manifest::valid_plugin_id(id) {
        return Err(());
    }
    // semver 解析失败 → 跳过该条目，不毒化整份 index。
    let Some(parsed_version) = Version::parse(&raw.version) else {
        return Err(());
    };
    // sha256 64 位十六进制，必填。
    let sha256 = raw.dist.sha256.trim().to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(());
    }
    // dist.url 白名单：https / 回环 http / file（§3.1）。
    let dist_url = raw.dist.url.trim();
    if !is_allowed_url(dist_url) {
        return Err(());
    }
    // 内嵌 manifest 必须是对象，且 id/version 与条目一致（客户端侧双向校验）。
    let manifest = &raw.manifest;
    if !manifest.is_object() {
        return Err(());
    }
    if manifest.get("id").and_then(Value::as_str).map(str::trim) != Some(id) {
        return Err(());
    }
    if manifest
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        != Some(raw.version.trim())
    {
        return Err(());
    }
    let file_source = dist_url.starts_with("file://");
    Ok(IndexEntry {
        id: id.to_string(),
        version: raw.version.trim().to_string(),
        parsed_version,
        manifest: raw.manifest,
        dist_url: dist_url.to_string(),
        sha256,
        signature: raw.signature.trim().to_string(),
        file_source,
        raw_engines: raw.engines,
        raw_deps: raw.deps,
    })
}

fn manifest_string_map(value: Option<&Value>) -> Option<BTreeMap<String, String>> {
    let object = value?.as_object()?;
    Some(
        object
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|s| (key.clone(), s.to_string())))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ext::marketplace::is_file_url;

    fn entry(id: &str, version: &str) -> Value {
        json!({
            "id": id,
            "version": version,
            "manifest": {
                "id": id,
                "version": version,
                "name": id,
                "runtime": "declarative",
                "description": "test plugin"
            },
            "dist": {
                "url": "https://example.com/plugin.zip",
                "sha256": "a".repeat(64)
            }
        })
    }

    fn index(plugins: impl Into<Value>) -> String {
        json!({"version": 1, "plugins": plugins.into()}).to_string()
    }

    #[test]
    fn parses_valid_index_and_counts_skipped_entries() {
        let plugins = json!([
            entry("demo.one", "0.1.0"),
            {
                "id": "demo.bad-semver",
                "version": "not-a-version",
                "manifest": {"id": "demo.bad-semver", "version": "not-a-version"},
                "dist": {"url": "https://example.com/a.zip", "sha256": "b".repeat(64)}
            },
            {
                "id": "demo.bad-hash",
                "version": "0.1.0",
                "manifest": {"id": "demo.bad-hash", "version": "0.1.0"},
                "dist": {"url": "https://example.com/b.zip", "sha256": "zz".repeat(32)}
            },
            {
                "id": "demo.bad-url",
                "version": "0.1.0",
                "manifest": {"id": "demo.bad-url", "version": "0.1.0"},
                "dist": {"url": "ftp://example.com/c.zip", "sha256": "c".repeat(64)}
            },
            {
                "id": "demo.drifted",
                "version": "0.1.0",
                "manifest": {"id": "demo.other", "version": "0.1.0"},
                "dist": {"url": "https://example.com/d.zip", "sha256": "d".repeat(64)}
            }
        ]);
        let parsed = parse_index(index(plugins).as_bytes()).expect("valid index");
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].id, "demo.one");
        assert_eq!(parsed.skipped, 4);
    }

    #[test]
    fn rejects_line_version_other_than_one_fail_closed() {
        let parsed = parse_index(index(json!([entry("demo.one", "0.1.0")])).as_bytes()).unwrap();
        assert_eq!(parsed.entries.len(), 1);

        let wrong = json!({"version": 2, "plugins": [entry("demo.one", "0.1.0")]}).to_string();
        let err = parse_index(wrong.as_bytes()).expect_err("version 2 rejected");
        assert!(matches!(err, MarketError::InvalidIndex(_)));

        let missing = json!({"plugins": [entry("demo.one", "0.1.0")]}).to_string();
        assert!(parse_index(missing.as_bytes()).is_err());
        let broken = b"{not json";
        assert!(parse_index(broken).is_err());
    }

    #[test]
    fn duplicate_id_invalidates_whole_index() {
        let plugins = json!([entry("demo.one", "0.1.0"), entry("demo.one", "0.2.0")]);
        let err = parse_index(index(plugins).as_bytes()).expect_err("dup id");
        assert!(err.to_string().contains("重复插件 id"));
    }

    #[test]
    fn rejects_oversized_and_overcounted_index() {
        let big = vec![b' '; (MAX_INDEX_BYTES + 1) as usize];
        let err = parse_index(&big).expect_err("oversize");
        assert!(err.to_string().contains("上限"));

        let plugins = vec![entry("demo.one", "0.1.0"); MAX_INDEX_ENTRIES + 1];
        let err = parse_index(index(plugins).as_bytes()).expect_err("too many entries");
        assert!(err.to_string().contains("条目数"));
    }

    #[test]
    fn entry_caps_and_file_source_flag() {
        let mut file_entry = entry("demo.file", "0.1.0");
        file_entry["dist"]["url"] = json!("file:///tmp/demo.zip");
        file_entry["manifest"]["permissions"] = json!(["workspace.read"]);
        let parsed = parse_index(index(json!([file_entry])).as_bytes()).unwrap();
        assert!(parsed.entries[0].file_source);
        assert!(is_file_url(&parsed.entries[0].dist_url));
        assert_eq!(parsed.entries[0].permissions(), vec!["workspace.read"]);

        let http_entry = entry("demo.http", "0.1.0");
        let parsed = parse_index(index(json!([http_entry])).as_bytes()).unwrap();
        assert!(!parsed.entries[0].file_source);
    }

    #[test]
    fn manifest_first_engines_and_deps_with_entry_fallback() {
        let mut full = entry("demo.full", "0.1.0");
        full["engines"] = json!({"lite-code": ">=99.0.0"});
        full["deps"] = json!(["stale.dep"]);
        full["manifest"]["engines"] = json!({"lite-code": ">=0.6.1"});
        full["manifest"]["extensionDependencies"] = json!(["real.dep"]);
        let parsed = parse_index(index(json!([full])).as_bytes()).unwrap();
        let entry = &parsed.entries[0];
        assert_eq!(
            entry
                .effective_engines()
                .get("lite-code")
                .map(String::as_str),
            Some(">=0.6.1")
        );
        assert_eq!(entry.effective_deps(), vec!["real.dep"]);
    }

    #[test]
    fn search_dto_serializes_camel_case() {
        let dto = MarketPlugin {
            id: "demo.one".into(),
            version: "0.1.0".into(),
            description: "desc".into(),
            runtime: "declarative".into(),
            engines: BTreeMap::new(),
            deps: vec![],
            permissions: vec![],
            source: "official".into(),
            file_source: false,
            signature_state: SignatureState::Unsigned,
            installed_version: None,
            update_available: false,
        };
        let value = serde_json::to_value(&dto).expect("serialize");
        assert_eq!(value["signatureState"], "unsigned");
        assert_eq!(value["installedVersion"], serde_json::Value::Null);
        assert_eq!(value["fileSource"], false);

        let result = MarketSearchResult {
            results: vec![dto],
            stale_sources: vec!["mirror-a".into()],
        };
        let value = serde_json::to_value(&result).expect("serialize");
        assert_eq!(value["staleSources"][0], "mirror-a");
        assert!(value["results"][0].get("updateAvailable").is_some());
    }
}
