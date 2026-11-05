//! 配置与数据目录（P1 Runtime lane：docs/AGENT.md §7、docs/ARCHITECTURE.md §3 `PathProvider`）。
//!
//! 职责：`~/.lite-code/config.json` 的读写——原子替换 + `.bak` 备份
//! （docs/AGENT.md §7：写入前备份，临时文件 + rename）。
//!
//! lane 纪律：本模块只依赖 lc-proto 与 std/白名单依赖；不引用其他 lane 内部路径。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lc_proto::{NodeSource, SemVer};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// 配置/数据目录定位失败（home 目录不可得）或读写失败。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("无法定位用户主目录")]
    NoHomeDir,
    #[error("config.json 解析失败：{0}")]
    Parse(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 应用目录名。AGENT.md §7 的约定与 Claude `~/.claude` / Codex `~/.codex` 同构。
pub const APP_DIR_NAME: &str = ".lite-code";
pub const CONFIG_FILE_NAME: &str = "config.json";

/// `~/.lite-code/config.json` 的 node 段（docs/RUNTIME.md §5：
/// 校验通过写 `{ path, version, source: "manual", resolvedAt }`）。
///
/// `resolved_at` 用 RFC3339 UTC 字符串：配置文件是给人看的，
/// 裸 epoch 秒可读性差；格式由 [`format_rfc3339`] 保证。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRuntime {
    /// node 可执行文件绝对路径（argv[0] 直接可用，docs/RUNTIME.md §6）
    pub path: String,
    pub version: SemVer,
    /// 契约枚举（lc-proto `NodeSource`）：manual / portable
    pub source: NodeSource,
    /// 解析时间，RFC3339 UTC（如 `2026-11-05T08:09:10Z`）
    pub resolved_at: String,
}

/// config.json 顶层结构。起步只有 node 段；`extra` 原样保留
/// 未知字段——用户/未来版本的配置不因降级读写而丢失（AGENT.md §7 的精神）。
/// （只有 PartialEq：`serde_json::Value` 含 f64，无 Eq。）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_runtime: Option<NodeRuntime>,
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// 全局配置路径 `~/.lite-code/config.json`（docs/AGENT.md §7 表格钉死的约定）。
///
/// 选 [`dirs::home_dir()`] 而非 `config_dir()`：Windows 的 `config_dir()` 是
/// `%APPDATA%`（Roaming），拼出来是 `AppData\Roaming\.lite-code\…`，与文档的
/// `~/` 前缀不符；`home_dir()` 三平台都返回 `~`，Linux 下 `~/.lite-code` 与
/// XDG 点目录约定同构（同表里 `~/.claude.json` / `~/.codex/config.toml`）。
pub fn config_path() -> Result<PathBuf, ConfigError> {
    let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
    Ok(home.join(APP_DIR_NAME).join(CONFIG_FILE_NAME))
}

/// 数据目录（docs/ARCHITECTURE.md §3 `PathProvider::data_dir`）。
///
/// Windows 用 `%LOCALAPPDATA%\lite-code`（[`dirs::data_local_dir`]）而不是
/// `data_dir()` 的 Roaming：便携版 Node 是几十 MB 的本机二进制，不随域配置漫游；
/// unix 走 XDG（`$XDG_DATA_HOME`/lite-code，默认 `~/.local/share`）。
pub fn data_dir() -> Result<PathBuf, ConfigError> {
    #[cfg(windows)]
    let base = dirs::data_local_dir();
    #[cfg(not(windows))]
    let base = dirs::data_dir();
    base.ok_or(ConfigError::NoHomeDir)
        .map(|d| d.join("lite-code"))
}

// ---- 时间戳：白名单依赖里没有 chrono/time，自己实现并单测 ----

/// unix 秒 → RFC3339 UTC（纯函数）。civil 历法算法出自
/// Howard Hinnant `civil_from_days`（公有领域）。
pub fn format_rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// 当前时刻，RFC3339 UTC。系统时钟早于 epoch 时按 0 处理（不 panic）。
pub fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_rfc3339(secs)
}

/// 天数（1970-01-01 = 0）→（年，月，日）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---- 序列化 / 反序列化 ----

pub fn parse_config(text: &str) -> Result<AppConfig, ConfigError> {
    serde_json::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))
}

/// pretty JSON + 结尾换行（与 golden 文件风格一致）。
pub fn serialize_config(cfg: &AppConfig) -> String {
    let mut s = serde_json::to_string_pretty(cfg).expect("AppConfig 序列化不可能失败");
    s.push('\n');
    s
}

// ---- 存取抽象（trait 注入；测试用临时目录或内存实现）----

/// 配置存取抽象。真实实现 [`JsonConfigStore`]，测试可注入内存实现。
pub trait ConfigStore: Send + Sync {
    fn load(&self) -> Result<AppConfig, ConfigError>;
    fn save(&self, cfg: &AppConfig) -> Result<(), ConfigError>;
}

/// 文件系统实现：缺文件视为空配置，写入走 [`write_atomic`]。
pub struct JsonConfigStore {
    path: PathBuf,
}

impl JsonConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// 默认路径 `~/.lite-code/config.json`。
    pub fn at_default_path() -> Result<Self, ConfigError> {
        Ok(Self::new(config_path()?))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ConfigStore for JsonConfigStore {
    fn load(&self) -> Result<AppConfig, ConfigError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => parse_config(&text),
            // 尚无配置 ≠ 错误：node_runtime 为 None 即未解析状态
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, cfg: &AppConfig) -> Result<(), ConfigError> {
        write_atomic(&self.path, &serialize_config(cfg))?;
        Ok(())
    }
}

/// 原子写入（docs/AGENT.md §7）：目标已存在时先复制为 `<名>.bak`，
/// 再写同目录临时文件，最后 rename 覆盖。rename 在同一卷上是原子的
/// （Windows 下 std::fs::rename 即 MoveFileExW + MOVEFILE_REPLACE_EXISTING）。
///
/// 注意：不保留用户的手工 key 顺序/空白——本文件是应用生成的机器配置
/// （结构化 serde pretty），AGENT.md §7 的“最小 diff”约束针对的是用户手写的
/// 记忆/供应商配置（toml_edit 等），此处按任务约定走备份 + 整体重写。
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let bak = with_suffix(path, "bak");
    if path.exists() {
        std::fs::copy(path, &bak)?;
    }

    let tmp = with_suffix(path, "tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `config.json` → `config.json.<suffix>`（同目录兄弟文件，保证 rename 同卷）。
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os: std::ffi::OsString = path.as_os_str().to_owned();
    os.push(".");
    os.push(suffix);
    PathBuf::from(os)
}

// ---- 供 runtime::manual / runtime::portable 共用的语义操作 ----

/// 校验通过后写入 node 段（docs/RUNTIME.md §5：manual 与便携版写同一配置）。
pub fn store_node_runtime(store: &dyn ConfigStore, rt: NodeRuntime) -> Result<(), ConfigError> {
    let mut cfg = store.load()?;
    cfg.node_runtime = Some(rt);
    store.save(&cfg)
}

/// `node info` 读取入口：读出当前已解析的 node 运行时（可能为 None）。
pub fn load_node_runtime(store: &dyn ConfigStore) -> Result<Option<NodeRuntime>, ConfigError> {
    Ok(store.load()?.node_runtime)
}
