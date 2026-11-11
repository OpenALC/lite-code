//! 插件 webview UI 的 Rust 侧核心（docs/plans/plugin-webview.md §3.8，M3）。
//!
//! 三块职责，全部做成纯函数/薄 IO，桌面层只做挂接：
//! 1. **URL 生成器** [`plugin_asset_url`]：唯一允许产出插件资源 URL 的地方
//!    （§3.8「统一 URL 与路由」），CSP 与 handler 共用，不另拼字符串。
//!    Linux 为 `lc-plugin://localhost/...`，Windows 默认为
//!    `http://lc-plugin.localhost/...`（WebView2 把自定义 scheme 映射为
//!    `http://<scheme>.localhost`；启用 HTTPS 映射后同一生成器改输出 https）。
//! 2. **protocol handler 核心** [`serve_webview_asset`]：解析 scheme/authority、
//!    path 前两段提取 pluginId/webviewId、统一解码一次并拒绝编码斜杠/反斜杠/
//!    NUL/点段/绝对路径/盘符前缀，canonicalize 后做根内 + 声明白名单校验；
//!    仅 GET（其它 405），目录/不存在/未声明 404；Content-Type 白名单 +
//!    统一 nosniff（响应头由桌面挂接层补）+ HTML 附子文档 CSP。
//! 3. **bridge 状态** [`WebviewStateStore`]：per-plugin JSON，键 ≤64 字符、
//!    单值 ≤64 KiB、每插件 ≤256 KiB，持久化到用户数据目录；不用 webview
//!    storage。postCommand 不新增命令：webview 经既有 `ext_execute_command`
//!    执行，Rust 端用 [`validate_webview_command`] 显式校验命令属于绑定插件的
//!    清单 contributes.commands 后才转发。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::fs::{RealPathResolver, ensure_within_root};

use super::manifest::DiscoveredPlugin;
use super::{ExtError, ExtPaths, enabled_plugins, project_allowed};

/// 资源 URL 的逻辑 scheme（`lc-plugin`）。Windows 上 WebView2 实际以
/// `http(s)://lc-plugin.localhost` 请求，handler 按平台校验预期 scheme/authority。
pub const PLUGIN_ASSET_SCHEME: &str = "lc-plugin";
/// Linux 基地址（WebKitGTK 原生自定义 scheme，Tauri 官方平台说明）。
pub const LINUX_ASSET_BASE: &str = "lc-plugin://localhost";
/// Windows 默认基地址（`use_https_scheme` 后改输出 https，地址主体不变）。
pub const WINDOWS_ASSET_BASE: &str = "http://lc-plugin.localhost";

/// webview bridge 状态键上限（字符数，docs/plans/plugin-webview.md §3.8）。
pub const MAX_WEBVIEW_STATE_KEY_CHARS: usize = 64;
/// webview bridge 单值上限（序列化字节数）。
pub const MAX_WEBVIEW_STATE_VALUE_BYTES: usize = 64 * 1024;
/// webview bridge 每插件状态总量上限（键 + 序列化值合计字节数）。
pub const MAX_WEBVIEW_STATE_TOTAL_BYTES: usize = 256 * 1024;

/// 用户数据目录下的持久化文件名（`<data_dir>/ext-webview-state.json`）。
pub const WEBVIEW_STATE_FILE: &str = "ext-webview-state.json";

/// bridge 状态存储文件顶层形状：per-plugin JSON 对象，未知顶层字段原样
/// 保留（同 extensions.json 的 forward-compat 模式）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct WebviewStateFile {
    #[serde(default)]
    plugins: BTreeMap<String, BTreeMap<String, Value>>,
    #[serde(flatten, default)]
    extra: Map<String, Value>,
}

#[cfg(windows)]
const fn platform_base() -> &'static str {
    WINDOWS_ASSET_BASE
}

#[cfg(not(windows))]
const fn platform_base() -> &'static str {
    LINUX_ASSET_BASE
}

#[cfg(windows)]
const fn platform_scheme() -> &'static str {
    "http"
}

#[cfg(not(windows))]
const fn platform_scheme() -> &'static str {
    PLUGIN_ASSET_SCHEME
}

#[cfg(windows)]
const fn platform_authority() -> &'static str {
    "lc-plugin.localhost"
}

#[cfg(not(windows))]
const fn platform_authority() -> &'static str {
    "localhost"
}

/// §3.8 统一 URL 生成器：`{base}/<pluginId>/<webviewId>/<asset>`，pluginId/
/// webviewId 不放 authority。`asset` 传空串得到该视图资源目录前缀（含尾斜杠），
/// 供子文档 CSP 使用。CSP 与 handler 必须共用本函数。
pub fn plugin_asset_url(plugin_id: &str, webview_id: &str, asset: &str) -> String {
    plugin_asset_url_with_base(platform_base(), plugin_id, webview_id, asset)
}

/// 指定基地址的生成器：测试与 CSP 构造用它覆盖双平台形状，宿主侧一律走
/// [`plugin_asset_url`]。
pub fn plugin_asset_url_with_base(
    base: &str,
    plugin_id: &str,
    webview_id: &str,
    asset: &str,
) -> String {
    format!("{base}/{plugin_id}/{webview_id}/{asset}")
}

/// 子文档 CSP（§3.8「CSP」）：`default-src 'none'; connect-src 'none';
/// base-uri 'none'; form-action 'none'`，script/style/img/font 各使用该视图
/// 资源目录的完整 URL 前缀（含尾斜杠），不使用任意 `http:`/`https:`、通配符
/// 或 `'self'`。仅 HTML 响应携带。
pub fn webview_child_csp(plugin_id: &str, webview_id: &str) -> String {
    let prefix = plugin_asset_url(plugin_id, webview_id, "");
    format!(
        "default-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'; \
         script-src {prefix}; style-src {prefix}; img-src {prefix}; font-src {prefix}"
    )
}

/// handler 响应。桌面挂接层据此构造 `http::Response` 并统一补
/// `X-Content-Type-Options: nosniff`（本核心不拼响应头，保证单测可断言语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebviewAssetResponse {
    pub status: WebviewAssetStatus,
    pub content_type: Option<&'static str>,
    pub body: Vec<u8>,
    /// HTML 200 响应的子文档 CSP（[webview_child_csp] 产物）。
    pub csp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebviewAssetStatus {
    Ok,
    NotFound,
    MethodNotAllowed,
}

impl WebviewAssetResponse {
    fn ok(content_type: &'static str, body: Vec<u8>, csp: Option<String>) -> Self {
        Self {
            status: WebviewAssetStatus::Ok,
            content_type: Some(content_type),
            body,
            csp,
        }
    }

    fn not_found() -> Self {
        Self {
            status: WebviewAssetStatus::NotFound,
            content_type: None,
            body: Vec::new(),
            csp: None,
        }
    }

    fn method_not_allowed() -> Self {
        Self {
            status: WebviewAssetStatus::MethodNotAllowed,
            content_type: None,
            body: Vec::new(),
            csp: None,
        }
    }
}

/// 解析后的插件资源请求（path 前两段 = pluginId/webviewId，其余为 asset
/// 相对路径，统一解码一次后的形状）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPluginAsset {
    pub plugin_id: String,
    pub webview_id: String,
    pub asset: String,
}

/// 按当前平台解析 handler 收到的请求 URI（Windows 预期
/// `http://lc-plugin.localhost/...`，Linux 预期 `lc-plugin://localhost/...`）。
pub fn parse_asset_uri(uri: &str) -> Option<ParsedPluginAsset> {
    // Windows 实测（WebView2/tauri 2.12）：handler 回调收到的是自定义 scheme
    // 原生形式 `lc-plugin://localhost/...`——页面侧 URL 为 http://lc-plugin.
    // localhost（webview 内部做映射），两种形式都接受；Linux 只有原生形式。
    // URL 生成器不受此影响（页面/CSP 仍用平台基地址）。
    #[cfg(windows)]
    {
        parse_asset_uri_for(uri, platform_scheme(), platform_authority())
            .or_else(|| parse_asset_uri_for(uri, PLUGIN_ASSET_SCHEME, "localhost"))
    }
    #[cfg(not(windows))]
    {
        parse_asset_uri_for(uri, platform_scheme(), platform_authority())
    }
}

/// 指定预期 scheme/authority 的解析（测试用它覆盖双平台；scheme/authority
/// 不匹配、路径形状非法、解码引入禁字符一律 None → 404）。
pub fn parse_asset_uri_for(uri: &str, scheme: &str, authority: &str) -> Option<ParsedPluginAsset> {
    let rest = uri.strip_prefix(scheme)?.strip_prefix("://")?;
    let (auth, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    // authority 大小写不敏感；请求不携带用户info/端口（生成器不产出），
    // 出现即与预期不符。
    if !auth.eq_ignore_ascii_case(authority) {
        return None;
    }
    let path = path.split('?').next().unwrap_or(path);
    let path = path.split('#').next().unwrap_or(path);
    let path = path.strip_prefix('/')?;
    let raw_segments: Vec<&str> = path.split('/').collect();
    // 至少 pluginId/webviewId/asset 三段；asset 本身可含子目录（多段）。
    if raw_segments.len() < 3 {
        return None;
    }
    let mut segments = Vec::with_capacity(raw_segments.len());
    for raw in raw_segments {
        let decoded = percent_decode(raw)?;
        if !valid_asset_segment(&decoded) {
            return None;
        }
        segments.push(decoded);
    }
    let asset = segments[2..].join("/");
    // 盘符前缀（`C:` 等）：相对路径不允许以盘符开头。
    if asset.len() >= 2 && asset.as_bytes()[0].is_ascii_alphabetic() && asset.as_bytes()[1] == b':'
    {
        return None;
    }
    Some(ParsedPluginAsset {
        plugin_id: segments[0].clone(),
        webview_id: segments[1].clone(),
        asset,
    })
}

/// 解码后路径段校验（§3.8「统一解码一次」的禁字符集）：拒绝编码斜杠（解码后
/// 出现的 `/`）、反斜杠、NUL、点段（`.`/`..`）与空段——空段同时拦住 `//`、
/// 尾斜杠（目录形态）与绝对路径形态。
fn valid_asset_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.chars().any(|ch| matches!(ch, '/' | '\\' | '\0'))
}

/// 百分比解码（拒绝非法转义与非法 UTF-8）。
fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    if !bytes.contains(&b'%') {
        return Some(raw.to_string());
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = hex_value(*bytes.get(index + 1)?)?;
            let lo = hex_value(*bytes.get(index + 2)?)?;
            out.push(hi * 16 + lo);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Content-Type 白名单（§3.8）：扩展名（小写）→ MIME，不在表内一律不服务
/// （404，fail-closed）。`.htm`/`.svg` 等不在首版承诺内。
pub fn content_type_for(asset: &str) -> Option<&'static str> {
    let name = asset.rsplit('/').next().unwrap_or(asset);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "html" => Some("text/html"),
        "css" => Some("text/css"),
        "js" | "mjs" => Some("application/javascript"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "woff2" => Some("font/woff2"),
        _ => None,
    }
}

/// protocol handler 入口（桌面 `register_uri_scheme_protocol` 薄挂接）：
/// 默认布局发现启用插件 → 白名单/根内校验 → 读文件。任何拒绝都是 404
/// （目录/不存在/未声明/穿越），非 GET 是 405。
pub fn serve_webview_asset(
    workspace_root: Option<&Path>,
    uri: &str,
    method: &str,
) -> WebviewAssetResponse {
    let paths = match ExtPaths::default_for(workspace_root) {
        Ok(paths) => paths,
        Err(_) => return WebviewAssetResponse::not_found(),
    };
    serve_webview_asset_at(&paths, project_allowed(), uri, method)
}

/// 指定布局下的 handler 核心（测试与嵌入方使用，同本模块 `_at` 惯例）。
pub fn serve_webview_asset_at(
    paths: &ExtPaths,
    allow_project: bool,
    uri: &str,
    method: &str,
) -> WebviewAssetResponse {
    let Some(parsed) = parse_asset_uri(uri) else {
        return WebviewAssetResponse::not_found();
    };
    if !method.eq_ignore_ascii_case("GET") {
        return WebviewAssetResponse::method_not_allowed();
    }
    let plugins = match enabled_plugins(paths, allow_project) {
        Ok(plugins) => plugins,
        Err(_) => return WebviewAssetResponse::not_found(),
    };
    serve_parsed_asset(&plugins, &parsed)
}

/// 白名单解析：请求目标 canonicalize 后必须与该视图声明的 entry/assets
/// canonical 路径逐一相等——canonicalize 同时完成根内校验与符号链接解析，
/// 软链/重定向无法把可访问范围扩大到声明文件之外。
fn resolve_whitelisted_asset(
    plugin: &DiscoveredPlugin,
    entry: &str,
    assets: &[String],
    asset: &str,
) -> Option<PathBuf> {
    let target =
        ensure_within_root(&plugin.dir, &plugin.dir.join(asset), &RealPathResolver).ok()?;
    let whitelisted = std::iter::once(entry)
        .chain(assets.iter().map(String::as_str))
        .any(|declared| {
            ensure_within_root(&plugin.dir, &plugin.dir.join(declared), &RealPathResolver)
                .is_ok_and(|canonical| canonical == target)
        });
    if !whitelisted || !target.is_file() {
        return None;
    }
    Some(target)
}

/// 解析后的请求 → 响应。插件必须启用且声明该 webview，asset 必须在
/// entry+assets 白名单内且 Content-Type 受支持。
fn serve_parsed_asset(
    plugins: &[DiscoveredPlugin],
    parsed: &ParsedPluginAsset,
) -> WebviewAssetResponse {
    let Some(plugin) = plugins
        .iter()
        .find(|plugin| plugin.info.id == parsed.plugin_id && plugin.info.enabled)
    else {
        return WebviewAssetResponse::not_found();
    };
    let Some(manifest) = plugin.manifest.as_ref() else {
        return WebviewAssetResponse::not_found();
    };
    let Some(decl) = manifest
        .contributes
        .ui
        .webviews
        .iter()
        .find(|decl| decl.id == parsed.webview_id)
    else {
        return WebviewAssetResponse::not_found();
    };
    let Some(resolved) =
        resolve_whitelisted_asset(plugin, &decl.entry, &decl.assets, &parsed.asset)
    else {
        return WebviewAssetResponse::not_found();
    };
    let Some(content_type) = content_type_for(&parsed.asset) else {
        return WebviewAssetResponse::not_found();
    };
    let Ok(body) = std::fs::read(&resolved) else {
        return WebviewAssetResponse::not_found();
    };
    let csp = (content_type == "text/html")
        .then(|| webview_child_csp(&parsed.plugin_id, &parsed.webview_id));
    WebviewAssetResponse::ok(content_type, body, csp)
}

/// 校验「webview 来源的命令确实属于绑定插件」（§3.8 bridge postCommand）：
/// 插件必须已启用且命令出现在该插件清单 contributes.commands 中；通过后再由
/// 既有全局命令映射执行（execute_with_paths），两道校验串行。
pub fn validate_webview_command(
    paths: &ExtPaths,
    allow_project: bool,
    plugin_id: &str,
    command: &str,
) -> Result<(), ExtError> {
    let plugins = enabled_plugins(paths, allow_project)?;
    let bound = plugins.iter().find(|plugin| {
        plugin.info.id == plugin_id && plugin.info.enabled && plugin.manifest.is_some()
    });
    let Some(bound) = bound else {
        return Err(ExtError::UnknownPlugin {
            id: plugin_id.to_string(),
        });
    };
    let declared = bound.manifest.as_ref().is_some_and(|manifest| {
        manifest
            .contributes
            .commands
            .iter()
            .any(|declared| declared.command == command)
    });
    if !declared {
        return Err(ExtError::CommandNotFound {
            command: command.to_string(),
        });
    }
    Ok(())
}

/// 生成宿主资源 URL 并校验其可达性（webview 入口/资产 URL 一律经宿主生成，
/// §3.8「统一 URL 与路由」）：插件启用、webview 声明、asset 在白名单内才
/// 返回 URL，否则按 ExtError 失败（web 侧据此报错，而非拿到一个 404 地址）。
pub fn checked_asset_url(
    paths: &ExtPaths,
    allow_project: bool,
    plugin_id: &str,
    webview_id: &str,
    asset: &str,
) -> Result<String, ExtError> {
    let plugins = enabled_plugins(paths, allow_project)?;
    let plugin = plugins
        .iter()
        .find(|plugin| plugin.info.id == plugin_id && plugin.info.enabled)
        .ok_or_else(|| ExtError::UnknownPlugin {
            id: plugin_id.to_string(),
        })?;
    let Some(manifest) = plugin.manifest.as_ref() else {
        return Err(ExtError::UnknownPlugin {
            id: plugin_id.to_string(),
        });
    };
    let Some(decl) = manifest
        .contributes
        .ui
        .webviews
        .iter()
        .find(|decl| decl.id == webview_id)
    else {
        return Err(ExtError::WebviewNotFound {
            plugin: plugin_id.to_string(),
            webview: webview_id.to_string(),
        });
    };
    if resolve_whitelisted_asset(plugin, &decl.entry, &decl.assets, asset).is_none() {
        return Err(ExtError::WebviewAssetNotFound {
            plugin: plugin_id.to_string(),
            webview: webview_id.to_string(),
            asset: asset.to_string(),
        });
    }
    Ok(plugin_asset_url(plugin_id, webview_id, asset))
}

/// webview bridge 的 per-plugin 状态存储（§3.8「bridge 能力」）：键 ≤64 字符、
/// 单值 ≤64 KiB、每插件 ≤256 KiB，持久化到用户数据目录，不用 webview storage。
/// 读改写全量文件（状态量小且调用低频，同 extensions.json 模式）。
#[derive(Debug, Clone)]
pub struct WebviewStateStore {
    path: PathBuf,
}

impl WebviewStateStore {
    /// 默认位置 `<data_dir>/ext-webview-state.json`。
    pub fn at_default() -> Result<Self, ExtError> {
        let data_dir = crate::config::data_dir()
            .map_err(|error| ExtError::Config(format!("无法定位数据目录：{error}")))?;
        Ok(Self::new(data_dir.join(WEBVIEW_STATE_FILE)))
    }

    /// 指定路径（测试与嵌入方使用）。
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取单键；插件或键不存在返回 None。
    pub fn get(&self, plugin_id: &str, key: &str) -> Result<Option<Value>, ExtError> {
        validate_state_key(key)?;
        let file = self.load_file()?;
        Ok(file
            .plugins
            .get(plugin_id)
            .and_then(|state| state.get(key))
            .cloned())
    }

    /// 写入单键并落盘；超限（键长/单值/总量）返回 InvalidArgument 且不写文件。
    pub fn set(&self, plugin_id: &str, key: &str, value: &Value) -> Result<(), ExtError> {
        validate_state_key(key)?;
        super::manifest::valid_plugin_id(plugin_id)
            .then_some(())
            .ok_or_else(|| ExtError::UnknownPlugin {
                id: plugin_id.to_string(),
            })?;
        let encoded = serde_json::to_string(value)
            .map_err(|error| ExtError::Config(format!("webview 状态序列化失败：{error}")))?;
        if encoded.len() > MAX_WEBVIEW_STATE_VALUE_BYTES {
            return Err(ExtError::WebviewStateValueTooLarge {
                plugin: plugin_id.to_string(),
            });
        }
        let mut file = self.load_file()?;
        let state = file.plugins.entry(plugin_id.to_string()).or_default();
        state.insert(key.to_string(), value.clone());
        if state_total_bytes(state) > MAX_WEBVIEW_STATE_TOTAL_BYTES {
            return Err(ExtError::WebviewStateTotalTooLarge {
                plugin: plugin_id.to_string(),
            });
        }
        self.save(&file)
    }

    fn load_file(&self) -> Result<WebviewStateFile, ExtError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(WebviewStateFile::default());
            }
            Err(error) => {
                return Err(ExtError::Config(format!(
                    "读取 {} 失败：{error}",
                    self.path.display()
                )));
            }
        };
        serde_json::from_str(&text)
            .map_err(|error| ExtError::Config(format!("{} 解析失败：{error}", self.path.display())))
    }

    fn save(&self, file: &WebviewStateFile) -> Result<(), ExtError> {
        let mut text = serde_json::to_string_pretty(file)
            .map_err(|error| ExtError::Config(format!("webview 状态序列化失败：{error}")))?;
        text.push('\n');
        crate::config::write_atomic(&self.path, &text)
            .map_err(|error| ExtError::Io(format!("写入 {} 失败：{error}", self.path.display())))
    }
}

/// 状态键形态：非空且 ≤64 字符。
fn validate_state_key(key: &str) -> Result<(), ExtError> {
    if key.is_empty() || key.chars().count() > MAX_WEBVIEW_STATE_KEY_CHARS {
        return Err(ExtError::WebviewStateKeyInvalid);
    }
    Ok(())
}

/// 每插件状态总量：键字符数 + 值序列化字节数合计。
fn state_total_bytes(state: &BTreeMap<String, Value>) -> usize {
    state
        .iter()
        .map(|(key, value)| {
            key.len()
                + serde_json::to_string(value)
                    .map(|encoded| encoded.len())
                    .unwrap_or(0)
        })
        .sum()
}
