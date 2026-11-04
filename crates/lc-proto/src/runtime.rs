//! 运行时探测相关 DTO（Node / Shell），语义见 docs/RUNTIME.md。

/// Node 候选来源，优先级 Tier1–4 + 手动指定 + 便携版。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum NodeSource {
    /// Tier 1 —— 官方安装器（%ProgramFiles%\nodejs 等）
    OfficialInstaller,
    /// Tier 2 —— 版本管理器（nvm / fnm / Volta）
    VersionManager,
    /// Tier 3 —— 包管理器（choco / scoop）
    PackageManager,
    /// Tier 4 —— 注册表 PATH
    RegistryPath,
    /// 用户在 UI 手动指定的路径
    Manual,
    /// 应用下载的便携版
    Portable,
}

/// 版本号。独立 struct 而非字符串，避免下游各自解析。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 版本标签：只影响 UI 展示，不阻断（docs/RUNTIME.md §2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum NodeHealth {
    /// ≥ 22
    Supported,
    /// 18–21
    Unverified,
    /// < 18
    Legacy,
}

/// 单个候选的失败原因（探测矩阵见 docs/RUNTIME.md §4/§9）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum NodeFailureReason {
    Missing,
    NotAFile,
    /// Microsoft Store 别名桩（0 字节 reparse point）
    StoreAlias,
    SpawnFailed,
    Unparsable,
    /// exe 在但 DLL 缺失，二次自检失败
    GhostInstall,
    Broken,
}

/// 一次失败的探测记录。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct NodeProbeAttempt {
    pub path: String,
    pub source: NodeSource,
    pub reason: NodeFailureReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 一个可用的 Node 运行时。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct NodeRuntimeInfo {
    pub path: String,
    pub version: SemVer,
    pub source: NodeSource,
    pub health: NodeHealth,
}

/// Node 探测结果。`NeedManual` 是合法终态（降级模式），不是错误。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum NodeStatus {
    Ready { runtime: NodeRuntimeInfo },
    NeedManual { attempts: Vec<NodeProbeAttempt> },
}

/// shell 种类。单一 shell 原则见 docs/ARCHITECTURE.md §5.4.1。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub enum ShellKind {
    /// Windows：PowerShell 7（pwsh.exe）
    Pwsh,
    /// Linux：bash
    Bash,
}

/// 探测到的 shell。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ShellInfo {
    pub path: String,
    pub kind: ShellKind,
    /// pwsh 要求 major ≥ 7
    pub version_major: u32,
}
