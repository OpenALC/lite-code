//! 探测逻辑测试矩阵（docs/RUNTIME.md §9）。
//! 全部走 fake 实现，不碰真实文件系统。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use lc_proto::{NodeFailureReason, NodeHealth, NodeSource, NodeStatus};

use super::{CandidatePattern, DirLister, EnvSnapshot, ProcessRunner, RegistryHive, RunnerError};

// ---- fake 实现 ----

struct FakeEnv {
    vars: HashMap<String, String>,
    registry: Vec<String>,
}

impl FakeEnv {
    fn new(vars: &[(&str, &str)]) -> Self {
        Self {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            registry: Vec::new(),
        }
    }

    fn with_registry(mut self, dirs: &[&str]) -> Self {
        self.registry = dirs.iter().map(|s| s.to_string()).collect();
        self
    }
}

impl EnvSnapshot for FakeEnv {
    fn var(&self, name: &str) -> Option<String> {
        self.vars.get(name).cloned()
    }

    fn registry_path(&self, _hive: RegistryHive) -> Vec<String> {
        self.registry.clone()
    }
}

/// 用精确路径集合模拟文件系统；glob 靠 [`super::lister::glob_walk`] 对
/// `list_dir` 的逐段遍历实现，与真实行为同构。
struct FakeLister {
    files: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
}

impl FakeLister {
    fn new(paths: &[&str]) -> Self {
        let mut files = HashSet::new();
        let mut dirs = HashSet::new();
        for p in paths {
            let pb = PathBuf::from(p);
            files.insert(pb.clone());
            for d in pb.ancestors().skip(1) {
                dirs.insert(d.to_path_buf());
            }
        }
        Self { files, dirs }
    }
}

impl DirLister for FakeLister {
    fn exists(&self, path: &Path) -> bool {
        self.files.contains(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.dirs.contains(path)
    }

    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        if self.files.contains(path) || self.dirs.contains(path) {
            Ok(path.to_path_buf())
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "nope"))
        }
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in &self.files {
            if f.parent() == Some(path) {
                out.push(f.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        for d in &self.dirs {
            if d.parent() == Some(path) {
                out.push(d.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        out.sort();
        out
    }

    fn glob_files(&self, pattern: &Path) -> Vec<PathBuf> {
        super::lister::glob_walk(pattern, self)
    }
}

struct FakeRunner {
    /// exe 路径 → `--version` 输出
    versions: HashMap<String, Result<String, RunnerError>>,
    /// 自检失败的 exe
    ghost: HashSet<String>,
}

impl FakeRunner {
    fn new(versions: &[(&str, &str)]) -> Self {
        Self {
            versions: versions
                .iter()
                .map(|(p, v)| (p.to_string(), Ok(v.to_string())))
                .collect(),
            ghost: HashSet::new(),
        }
    }

    fn with_ghost(mut self, paths: &[&str]) -> Self {
        self.ghost.extend(paths.iter().map(|s| s.to_string()));
        self
    }
}

impl ProcessRunner for FakeRunner {
    fn run(&self, exe: &Path, args: &[&str]) -> Result<String, RunnerError> {
        // 自检调用：`-e require('fs');process.exit(0)`
        if args.first() == Some(&"-e") {
            let key = exe.to_string_lossy().into_owned();
            return if self.ghost.contains(&key) {
                Err(RunnerError::Spawn("selfcheck failed".into()))
            } else {
                Ok(String::new())
            };
        }
        // shell 校验调用（pwsh 取 major）
        if args.contains(&"$PSVersionTable.PSVersion.Major") {
            let v = self
                .versions
                .get(&exe.to_string_lossy().into_owned())
                .ok_or_else(|| RunnerError::Spawn("no shell entry".into()))?
                .clone()?;
            return Ok(v);
        }
        self.versions
            .get(&exe.to_string_lossy().into_owned())
            .cloned()
            .unwrap_or_else(|| Err(RunnerError::Spawn("no entry".into())))
    }
}

// ---- 测试辅助 ----

/// 与 WINDOWS_PATTERNS 等价、但只包含测试用到的模式（平台无关）。
fn test_patterns() -> Vec<CandidatePattern> {
    use lc_proto::NodeSource::*;
    vec![
        CandidatePattern {
            tier: 1,
            source: OfficialInstaller,
            raw: "%ProgramFiles%\\nodejs\\node.exe",
        },
        CandidatePattern {
            tier: 2,
            source: VersionManager,
            raw: "%APPDATA%\\nvm\\v*\\node.exe",
        },
        CandidatePattern {
            tier: 2,
            source: VersionManager,
            raw: "%NVM_HOME%\\v*\\node.exe",
        },
    ]
}

/// 注册表 PATH 目录的跨平台夹具：与 enumerate 的 Tier 4 构造同构
/// （`Path::join` 在 unix 插 `/`，夹具必须走同一条构造路径）。
fn registry_dir(path: &str) -> String {
    if cfg!(windows) {
        path.to_string()
    } else {
        path.replace('\\', "/")
    }
}

fn resolve(env: &FakeEnv, lister: &FakeLister, runner: &FakeRunner) -> NodeStatus {
    let patterns = test_patterns();
    super::resolve_with(env, lister, runner, &patterns)
}

fn ready(status: NodeStatus) -> lc_proto::NodeRuntimeInfo {
    match status {
        NodeStatus::Ready { runtime } => runtime,
        NodeStatus::NeedManual { attempts } => {
            panic!("expected Ready, got NeedManual with {attempts:?}")
        }
    }
}

fn manual(status: NodeStatus) -> Vec<lc_proto::NodeProbeAttempt> {
    match status {
        NodeStatus::Ready { runtime } => panic!("expected NeedManual, got Ready {runtime:?}"),
        NodeStatus::NeedManual { attempts } => attempts,
    }
}

// ---- 测试矩阵 ----

/// RUNTIME.md §9 原始用例：nvm 多版本取最高。
#[test]
fn nvm_highest_version_wins() {
    let env = FakeEnv::new(&[("APPDATA", r"C:\Users\t\AppData\Roaming")]);
    let lister = FakeLister::new(&[
        r"C:\Users\t\AppData\Roaming\nvm\v20.11.0\node.exe",
        r"C:\Users\t\AppData\Roaming\nvm\v22.4.0\node.exe",
    ]);
    let runner = FakeRunner::new(&[
        (
            r"C:\Users\t\AppData\Roaming\nvm\v22.4.0\node.exe",
            "v22.4.0",
        ),
        (
            r"C:\Users\t\AppData\Roaming\nvm\v20.11.0\node.exe",
            "v20.11.0",
        ),
    ]);
    let r = ready(resolve(&env, &lister, &runner));
    assert_eq!(r.version.to_string(), "22.4.0");
    assert_eq!(r.source, NodeSource::VersionManager);
    assert_eq!(r.health, NodeHealth::Supported);
}

/// 同档内按版本号降序：高版本 spawn 失败后回落到低版本，顺序可从 attempts 验证。
#[test]
fn same_tier_ordered_version_desc() {
    let env = FakeEnv::new(&[("APPDATA", r"C:\Users\t\AppData\Roaming")]);
    let v22 = r"C:\Users\t\AppData\Roaming\nvm\v22.4.0\node.exe";
    let v20 = r"C:\Users\t\AppData\Roaming\nvm\v20.11.0\node.exe";
    let lister = FakeLister::new(&[v22, v20]);
    let mut runner = FakeRunner::new(&[(v22, "v22.4.0"), (v20, "v20.11.0")]);
    // v22 --version 失败（SpawnFailed），v20 版本正常但自检失败（GhostInstall）
    runner.versions.insert(
        v22.to_string(),
        Err(RunnerError::Spawn("access denied".into())),
    );
    let runner = runner.with_ghost(&[v20]);
    let attempts = manual(resolve(&env, &lister, &runner));
    assert_eq!(attempts.len(), 2);
    assert!(
        attempts[0].path.ends_with("v22.4.0\\node.exe"),
        "{:?}",
        attempts
    );
    assert_eq!(attempts[0].reason, NodeFailureReason::SpawnFailed);
    assert!(
        attempts[1].path.ends_with("v20.11.0\\node.exe"),
        "{:?}",
        attempts
    );
    assert_eq!(attempts[1].reason, NodeFailureReason::GhostInstall);
}

/// 只有 Store 别名桩 → NeedManual，标记 StoreAlias，不执行。
/// 别名桩只可能经注册表 PATH（Tier 4）枚举到。
#[test]
fn store_alias_only() {
    let dir = registry_dir(r"C:\Users\t\AppData\Local\Microsoft\WindowsApps");
    let exe = Path::new(&dir).join("node.exe");
    let env = FakeEnv::new(&[]).with_registry(&[dir.as_str()]);
    let lister = FakeLister::new(&[exe.to_str().unwrap()]);
    let runner = FakeRunner::new(&[]);
    let attempts = manual(resolve(&env, &lister, &runner));
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].reason, NodeFailureReason::StoreAlias);
    assert_eq!(attempts[0].source, NodeSource::RegistryPath);
}

/// exe 存在但自检失败（GhostInstall）→ 跳过，尝试下一个。
#[test]
fn ghost_install_skipped_and_next_candidate_wins() {
    let env = FakeEnv::new(&[("ProgramFiles", r"C:\Program Files")]);
    let ghost = r"C:\Program Files\nodejs\node.exe";
    let good = r"C:\nvm\v22.0.0\node.exe";
    let lister = FakeLister::new(&[ghost, good]);
    // ghost 的 --version 正常，但二次自检失败 —— 这正是 GhostInstall 的定义
    let runner = FakeRunner::new(&[(ghost, "v22.0.0"), (good, "v22.0.0")]).with_ghost(&[ghost]);
    // good 通过 NVM_HOME 到达（tier 2，晚于 tier 1 的 ghost）
    let env = env_with(&env, "NVM_HOME", r"C:\nvm");
    let r = ready(resolve(&env, &lister, &runner));
    assert!(r.path.ends_with(r"v22.0.0\node.exe"), "{}", r.path);
}

/// 只有 ghost 候选时，失败原因必须是 GhostInstall 而非 SpawnFailed。
#[test]
fn ghost_only_reports_ghost_install() {
    let env = FakeEnv::new(&[("ProgramFiles", r"C:\Program Files")]);
    let ghost = r"C:\Program Files\nodejs\node.exe";
    let lister = FakeLister::new(&[ghost]);
    let runner = FakeRunner::new(&[(ghost, "v22.0.0")]).with_ghost(&[ghost]);
    let attempts = manual(resolve(&env, &lister, &runner));
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].reason, NodeFailureReason::GhostInstall);
}

/// 版本 16 → Ready/Legacy，不阻断。
#[test]
fn version_16_is_ready_legacy() {
    let env = FakeEnv::new(&[("ProgramFiles", r"C:\Program Files")]);
    let lister = FakeLister::new(&[r"C:\Program Files\nodejs\node.exe"]);
    let runner = FakeRunner::new(&[(r"C:\Program Files\nodejs\node.exe", "v16.20.2")]);
    let r = ready(resolve(&env, &lister, &runner));
    assert_eq!(r.health, NodeHealth::Legacy);
    assert_eq!(r.version, lc_proto::SemVer::new(16, 20, 2));
}

/// 全部失败 → NeedManual，attempts 按候选顺序。
#[test]
fn all_fail_reports_attempts() {
    let env = FakeEnv::new(&[("ProgramFiles", r"C:\Program Files")]);
    let lister = FakeLister::new(&[r"C:\Program Files\nodejs\node.exe"]);
    let runner = FakeRunner::new(&[]); // 无版本条目 → SpawnFailed
    let attempts = manual(resolve(&env, &lister, &runner));
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].reason, NodeFailureReason::SpawnFailed);
}

/// 什么都没有 → NeedManual，空 attempts。
#[test]
fn nothing_installed() {
    let env = FakeEnv::new(&[]);
    let lister = FakeLister::new(&[]);
    let runner = FakeRunner::new(&[]);
    let attempts = manual(resolve(&env, &lister, &runner));
    assert!(attempts.is_empty(), "{attempts:?}");
}

/// Tier 1 命中即返回，不看 Tier 2/4（首个可用即返回）。
#[test]
fn tier1_beats_registry() {
    let registry = registry_dir(r"D:\registry-node");
    let registry_exe = Path::new(&registry).join("node.exe");
    let env =
        FakeEnv::new(&[("ProgramFiles", r"C:\Program Files")]).with_registry(&[registry.as_str()]);
    let lister = FakeLister::new(&[
        r"C:\Program Files\nodejs\node.exe",
        registry_exe.to_str().unwrap(),
    ]);
    let runner = FakeRunner::new(&[(r"C:\Program Files\nodejs\node.exe", "v24.15.0")]);
    let r = ready(resolve(&env, &lister, &runner));
    assert_eq!(r.source, NodeSource::OfficialInstaller);
}

/// PATH 陈旧场景：环境变量缺失时，以注册表为准（Tier 4）。
#[test]
fn registry_used_when_env_var_missing() {
    let dir = registry_dir(r"D:\nvm-portable");
    let exe = Path::new(&dir).join("node.exe");
    let env = FakeEnv::new(&[]).with_registry(&[dir.as_str()]);
    let lister = FakeLister::new(&[exe.to_str().unwrap()]);
    let runner = FakeRunner::new(&[(exe.to_str().unwrap(), "v18.19.0")]);
    let r = ready(resolve(&env, &lister, &runner));
    assert_eq!(r.source, NodeSource::RegistryPath);
    assert_eq!(r.health, NodeHealth::Unverified);
}

/// PATH 陈旧场景（反例）：环境变量在，注册表也在 —— Tier 2 仍优先于 Tier 4。
#[test]
fn env_var_beats_stale_registry() {
    let registry = registry_dir(r"D:\stale-node");
    let registry_exe = Path::new(&registry).join("node.exe");
    let env = FakeEnv::new(&[("NVM_HOME", r"C:\nvm")]).with_registry(&[registry.as_str()]);
    let lister = FakeLister::new(&[r"C:\nvm\v20.0.0\node.exe", registry_exe.to_str().unwrap()]);
    let runner = FakeRunner::new(&[(r"C:\nvm\v20.0.0\node.exe", "v20.0.0")]);
    let r = ready(resolve(&env, &lister, &runner));
    assert_eq!(r.source, NodeSource::VersionManager);
}

/// 超时视为 SpawnFailed，继续下一个候选。
#[test]
fn timeout_is_spawn_failed_and_continues() {
    let env = FakeEnv::new(&[("NVM_HOME", r"C:\nvm")]);
    let broken = r"C:\nvm\v21.0.0\node.exe";
    let good = r"C:\nvm\v20.0.0\node.exe";
    let lister = FakeLister::new(&[broken, good]);
    let mut runner = FakeRunner::new(&[(good, "v20.0.0")]);
    runner.versions.insert(
        broken.to_string(),
        Err(RunnerError::Timeout(super::PROBE_TIMEOUT)),
    );
    let r = ready(resolve(&env, &lister, &runner));
    assert!(r.path.ends_with("v20.0.0\\node.exe"));
}

// ---- 辅助：构造时补充单个变量（保持用例可读）----

fn env_with(base: &FakeEnv, key: &str, val: &str) -> FakeEnv {
    let mut vars = base.vars.clone();
    vars.insert(key.to_string(), val.to_string());
    FakeEnv {
        vars,
        registry: base.registry.clone(),
    }
}

// ---- parse_version 单测 ----

#[test]
fn parse_version_variants() {
    use super::parse_version;
    assert_eq!(
        parse_version("v24.15.0"),
        Some(lc_proto::SemVer::new(24, 15, 0))
    );
    assert_eq!(
        parse_version("node v18.0.1 here"),
        Some(lc_proto::SemVer::new(18, 0, 1))
    );
    assert_eq!(
        parse_version("16.20.2"),
        Some(lc_proto::SemVer::new(16, 20, 2))
    );
    assert_eq!(parse_version("v1.2"), None, "两段不算版本");
    assert_eq!(parse_version("no digits"), None);
    assert_eq!(parse_version(""), None);
}

// ---- shell 探测（直接调用平台分支，保证测试平台无关）----

#[test]
fn shell_pwsh_found_with_major_check() {
    use super::shell::detect_pwsh;
    let env = FakeEnv::new(&[("ProgramFiles", r"C:\Program Files")]);
    let exe = r"C:\Program Files\PowerShell\7\pwsh.exe";
    let lister = FakeLister::new(&[exe]);
    let runner = FakeRunner::new(&[(exe, "7")]);
    let s = detect_pwsh(&env, &lister, &runner).expect("pwsh should be found");
    assert_eq!(s.version_major, 7);
    assert_eq!(s.kind, lc_proto::ShellKind::Pwsh);
}

#[test]
fn shell_pwsh_major_below_7_rejected() {
    use super::shell::detect_pwsh;
    let env = FakeEnv::new(&[("ProgramFiles", r"C:\Program Files")]);
    let exe = r"C:\Program Files\PowerShell\7\pwsh.exe";
    let lister = FakeLister::new(&[exe]);
    let runner = FakeRunner::new(&[(exe, "5")]);
    assert!(detect_pwsh(&env, &lister, &runner).is_none());
}

#[test]
fn shell_bash_on_unix_style_candidates() {
    use super::shell::detect_bash;
    let lister = FakeLister::new(&["/bin/bash"]);
    let runner = FakeRunner::new(&[("/bin/bash", "GNU bash, version 5.2.15(1)-release")]);
    let s = detect_bash(&lister, &runner).expect("bash should be found");
    assert_eq!(s.version_major, 5);
    assert_eq!(s.kind, lc_proto::ShellKind::Bash);
}
