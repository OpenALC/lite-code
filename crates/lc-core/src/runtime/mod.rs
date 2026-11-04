//! Node / Shell 运行时探测。
//!
//! 设计约束（docs/RUNTIME.md）：
//! - 探测逻辑写成**纯函数**，由 [`EnvSnapshot`] / [`DirLister`] / [`ProcessRunner`]
//!   三个 trait 驱动，单测不碰真实文件系统；
//! - 候选按 Tier 1–4 优先级枚举，同档内按版本号降序；
//! - **串行**探测（Windows Defender 逐个扫描，并发 spawn 反而更慢）；
//! - 不通过进程 `PATH` 找 node，Tier 4 读注册表。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lc_proto::{
    NodeFailureReason, NodeHealth, NodeProbeAttempt, NodeRuntimeInfo, NodeSource, NodeStatus,
    SemVer,
};

pub mod env;
pub mod lister;
pub mod runner;
pub mod shell;

#[cfg(test)]
mod tests;

pub use env::RealEnv;
pub use lister::RealDirLister;
pub use runner::RealProcessRunner;

/// 探测超时（docs/RUNTIME.md §4：3s）。
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// 注册表 hive（Tier 4）。进程 PATH 可能是陈旧值，必须读注册表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryHive {
    CurrentUser,
    LocalMachine,
}

/// 宿主环境抽象。测试用 fake 实现，真实实现走 [`RealEnv`]。
pub trait EnvSnapshot: Send + Sync {
    fn var(&self, name: &str) -> Option<String>;
    /// Tier 4：按 hive 读取注册表 PATH（unix 实现返回空）。
    fn registry_path(&self, hive: RegistryHive) -> Vec<String>;
}

/// 文件系统抽象。
pub trait DirLister: Send + Sync {
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
    /// 列出目录下的条目名（不递归）。
    fn list_dir(&self, path: &Path) -> Vec<String>;
    /// 单段通配 `*` 的模式匹配（Tier 2 版本目录扫描）。
    /// 实现方委托 [`glob_walk`]（trait 方法内无法把 `&Self` 转成 `&dyn`）。
    fn glob_files(&self, pattern: &Path) -> Vec<PathBuf>;
}

/// 子进程执行抽象。超时语义统一在此实现，调用方不处理等待细节。
pub trait ProcessRunner: Send + Sync {
    /// 运行 `<exe> args...`，stdout+stderr 合并返回；超时/启动失败返回 Err。
    fn run(&self, exe: &Path, args: &[&str]) -> Result<String, RunnerError>;

    /// `<exe> --version`
    fn run_version(&self, exe: &Path) -> Result<String, RunnerError> {
        self.run(exe, &["--version"])
    }

    /// 二次自检：排除“exe 在但 DLL 缺失”的 GhostInstall。
    fn run_selfcheck(&self, exe: &Path) -> Result<(), RunnerError> {
        self.run(exe, &["-e", "require('fs');process.exit(0)"])
            .map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunnerError {
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("probe timeout")]
    Timeout(std::time::Duration),
}

/// 候选模式。`raw` 中 `%VAR%` 由 [`EnvSnapshot`] 展开；`*` 为单段通配。
#[derive(Debug, Clone)]
pub struct CandidatePattern {
    pub tier: u8,
    pub source: NodeSource,
    pub raw: &'static str,
}

const TIER1_OFFICIAL: NodeSource = NodeSource::OfficialInstaller;
const TIER2_MANAGER: NodeSource = NodeSource::VersionManager;
const TIER3_PKG: NodeSource = NodeSource::PackageManager;

/// Windows 候选模式（docs/RUNTIME.md §3）。
pub const WINDOWS_PATTERNS: &[CandidatePattern] = &[
    // Tier 1 —— 官方安装器
    CandidatePattern {
        tier: 1,
        source: TIER1_OFFICIAL,
        raw: "%ProgramFiles%\\nodejs\\node.exe",
    },
    CandidatePattern {
        tier: 1,
        source: TIER1_OFFICIAL,
        raw: "%ProgramFiles(x86)%\\nodejs\\node.exe",
    },
    CandidatePattern {
        tier: 1,
        source: TIER1_OFFICIAL,
        raw: "%LOCALAPPDATA%\\Programs\\nodejs\\node.exe",
    },
    // Tier 2 —— 版本管理器（glob 匹配，同档按版本号降序）
    CandidatePattern {
        tier: 2,
        source: TIER2_MANAGER,
        raw: "%NVM_HOME%\\v*\\node.exe",
    },
    CandidatePattern {
        tier: 2,
        source: TIER2_MANAGER,
        raw: "%APPDATA%\\nvm\\v*\\node.exe",
    },
    CandidatePattern {
        tier: 2,
        source: TIER2_MANAGER,
        raw: "%NVM_SYMLINK%\\node.exe",
    },
    CandidatePattern {
        tier: 2,
        source: TIER2_MANAGER,
        raw: "%FNM_DIR%\\node-versions\\v*\\installation\\node.exe",
    },
    CandidatePattern {
        tier: 2,
        source: TIER2_MANAGER,
        raw: "%APPDATA%\\fnm\\node-versions\\v*\\installation\\node.exe",
    },
    CandidatePattern {
        tier: 2,
        source: TIER2_MANAGER,
        raw: "%LOCALAPPDATA%\\Volta\\tools\\image\\node\\*\\node.exe",
    },
    // Tier 3 —— 包管理器
    CandidatePattern {
        tier: 3,
        source: TIER3_PKG,
        raw: "%ChocolateyInstall%\\bin\\node.exe",
    },
    CandidatePattern {
        tier: 3,
        source: TIER3_PKG,
        raw: "%SCOOP%\\apps\\nodejs*\\current\\node.exe",
    },
    CandidatePattern {
        tier: 3,
        source: TIER3_PKG,
        raw: "%USERPROFILE%\\scoop\\apps\\nodejs\\current\\node.exe",
    },
];

/// Linux 候选（docs/RUNTIME.md §3）：桌面环境 PATH 可能不含 nvm 的 shell 初始化结果，
/// 因此必须扫描版本目录，不能只靠裸命令。
pub const LINUX_PATTERNS: &[CandidatePattern] = &[
    CandidatePattern {
        tier: 1,
        source: TIER2_MANAGER,
        raw: "$NVM_DIR/versions/node/v*/bin/node",
    },
    CandidatePattern {
        tier: 1,
        source: TIER2_MANAGER,
        raw: "$HOME/nvm/versions/node/v*/bin/node",
    },
    CandidatePattern {
        tier: 1,
        source: TIER2_MANAGER,
        raw: "~/.nvm/versions/node/v*/bin/node",
    },
    CandidatePattern {
        tier: 1,
        source: TIER2_MANAGER,
        raw: "$N_PREFIX/n*/bin/node",
    },
    CandidatePattern {
        tier: 1,
        source: TIER2_MANAGER,
        raw: "/usr/local/n/versions/node/*/bin/node",
    },
    CandidatePattern {
        tier: 2,
        source: TIER1_OFFICIAL,
        raw: "/usr/bin/node",
    },
    CandidatePattern {
        tier: 2,
        source: TIER1_OFFICIAL,
        raw: "/usr/local/bin/node",
    },
    CandidatePattern {
        tier: 2,
        source: TIER1_OFFICIAL,
        raw: "/snap/bin/node",
    },
];

pub fn platform_patterns() -> &'static [CandidatePattern] {
    if cfg!(windows) {
        WINDOWS_PATTERNS
    } else {
        LINUX_PATTERNS
    }
}

/// 一个待验证的候选。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    source: NodeSource,
    path: PathBuf,
}

/// 展开模式中的 `%VAR%`（Windows）或 `$VAR` / `~`（unix）。
/// 任一变量未设置 → 整个候选不可用。
pub(crate) fn expand_env(raw: &str, env: &dyn EnvSnapshot) -> Option<PathBuf> {
    if raw.starts_with('~') {
        let home = env.var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
        return Some(PathBuf::from(home).join(raw.trim_start_matches("~/")));
    }
    if let Some(rest) = raw.strip_prefix('$') {
        // $VAR/... 形式（unix 风格）
        let (name, tail) = rest.split_once('/')?;
        let v = env.var(name)?;
        return Some(PathBuf::from(v).join(tail));
    }
    // %VAR%\... 形式（Windows 风格）
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        out.push_str(&env.var(&after[..end])?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(PathBuf::from(out))
}

/// 按模式枚举候选，tier 升序，同档内按路径中的版本号降序。
fn enumerate(
    env: &dyn EnvSnapshot,
    lister: &dyn DirLister,
    patterns: &[CandidatePattern],
) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let max_tier = patterns.iter().map(|p| p.tier).max().unwrap_or(0);

    for tier in 1..=max_tier {
        let tier_start = candidates.len();
        for pat in patterns.iter().filter(|p| p.tier == tier) {
            let Some(root) = expand_env(pat.raw, env) else {
                continue;
            };
            for hit in lister.glob_files(&root) {
                candidates.push(Candidate {
                    source: pat.source,
                    path: hit,
                });
            }
        }
        sort_versions_desc(&mut candidates[tier_start..]);
    }

    // Tier 4 —— 注册表 PATH（读注册表而非进程 PATH，docs/RUNTIME.md §3）
    for hive in [RegistryHive::CurrentUser, RegistryHive::LocalMachine] {
        for dir in env.registry_path(hive) {
            let dir = dir.trim();
            if dir.is_empty() {
                continue;
            }
            let p = Path::new(dir).join("node.exe");
            if lister.exists(&p) {
                candidates.push(Candidate {
                    source: NodeSource::RegistryPath,
                    path: p,
                });
            }
        }
    }

    // 去重：canonicalize 对 reparse point 有效，shim 转发器后续走真实路径
    let mut seen = HashSet::new();
    candidates.retain(|c| {
        let key = lister
            .canonicalize(&c.path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| c.path.to_string_lossy().into_owned());
        seen.insert(key)
    });
    candidates
}

/// 从路径中提取版本号（任意段的 `vN.N.N` 或 `N.N.N`），用于同档排序。
fn semver_from_path(path: &Path) -> Option<SemVer> {
    path.components().rev().find_map(|c| {
        let s = c.as_os_str().to_string_lossy();
        parse_version(&s).filter(|_| s.contains('.'))
    })
}

/// 同档内版本号降序；解析不出版本的排最后（保持稳定）。
fn sort_versions_desc(batch: &mut [Candidate]) {
    batch.sort_by(
        |a, b| match (semver_from_path(&a.path), semver_from_path(&b.path)) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        },
    );
}

/// 从文本中解析第一处 `vN.N.N`（或裸 `N.N.N`）。
pub fn parse_version(text: &str) -> Option<SemVer> {
    let b = text.as_bytes();
    for i in 0..b.len() {
        let j = match b[i] {
            b'v' | b'V' => i + 1,
            _ => i,
        };
        if let Some((maj, j2)) = read_u32(b, j)
            && j2 < b.len()
            && b[j2] == b'.'
            && let Some((min, j3)) = read_u32(b, j2 + 1)
            && j3 < b.len()
            && b[j3] == b'.'
            && let Some((pat, _)) = read_u32(b, j3 + 1)
        {
            return Some(SemVer::new(maj, min, pat));
        }
    }
    None
}

fn read_u32(b: &[u8], mut i: usize) -> Option<(u32, usize)> {
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    std::str::from_utf8(&b[start..i])
        .ok()?
        .parse()
        .ok()
        .map(|n| (n, i))
}

/// 版本 → 健康标签（docs/RUNTIME.md §2：低版本不阻断，只影响标签）。
fn health_of(v: SemVer) -> NodeHealth {
    match v.major {
        22.. => NodeHealth::Supported,
        18..=21 => NodeHealth::Unverified,
        _ => NodeHealth::Legacy,
    }
}

type ValidateError = (NodeFailureReason, Option<String>);

/// 单个候选的校验流程（docs/RUNTIME.md §4）。
fn validate(
    path: &Path,
    source: NodeSource,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
) -> Result<NodeRuntimeInfo, ValidateError> {
    if !lister.exists(path) {
        return Err((NodeFailureReason::Missing, None));
    }
    if lister.is_dir(path) {
        return Err((NodeFailureReason::NotAFile, None));
    }
    // Store 别名桩：0 字节 reparse point，路径含 WindowsApps 直接标记，不执行
    if path.to_string_lossy().contains("WindowsApps") {
        return Err((NodeFailureReason::StoreAlias, None));
    }

    let out = runner.run_version(path).map_err(|e| {
        (
            NodeFailureReason::SpawnFailed,
            Some(match e {
                RunnerError::Timeout(_) => "timeout".into(),
                RunnerError::Spawn(msg) => msg,
            }),
        )
    })?;

    let Some(v) = parse_version(&out) else {
        return Err((NodeFailureReason::Unparsable, Some(out.trim().to_string())));
    };

    runner
        .run_selfcheck(path)
        .map_err(|e| (NodeFailureReason::GhostInstall, Some(e.to_string())))?;

    Ok(NodeRuntimeInfo {
        path: path.to_string_lossy().into_owned(),
        version: v,
        source,
        health: health_of(v),
    })
}

/// 探测主入口：用当前平台的候选模式。
pub fn resolve(
    env: &dyn EnvSnapshot,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
) -> NodeStatus {
    resolve_with(env, lister, runner, platform_patterns())
}

/// 可注入候选模式的探测入口（测试用，也供将来“用户自定义探测”扩展）。
pub fn resolve_with(
    env: &dyn EnvSnapshot,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
    patterns: &[CandidatePattern],
) -> NodeStatus {
    // 串行而非并发（docs/RUNTIME.md §4：Defender 逐个扫描，并发更慢）；
    // 每个失败候选记录进 attempts，全部失败时作为 NeedManual 的证据。
    let mut attempts = Vec::new();
    for candidate in enumerate(env, lister, patterns) {
        match validate(&candidate.path, candidate.source, lister, runner) {
            Ok(info) => return NodeStatus::Ready { runtime: info },
            Err((reason, detail)) => attempts.push(NodeProbeAttempt {
                path: candidate.path.to_string_lossy().into_owned(),
                source: candidate.source,
                reason,
                detail,
            }),
        }
    }
    NodeStatus::NeedManual { attempts }
}

/// 真实探测入口（lc-cli / Tauri 壳共用）。
pub fn probe_real() -> NodeStatus {
    let env = RealEnv;
    let lister = RealDirLister;
    let runner = RealProcessRunner::new(PROBE_TIMEOUT);
    resolve(&env, &lister, &runner)
}

pub use shell::detect_shell_real;
