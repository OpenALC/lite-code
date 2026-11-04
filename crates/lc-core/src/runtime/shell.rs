//! Shell 探测（docs/RUNTIME.md §8）：Windows 找 PowerShell 7，Linux 找 bash。
//!
//! 与 Node 探测语义不同：shell 缺失 = 终端与命令执行不可用，必须走引导安装；
//! 不提供手动填路径。校验按文件名精确匹配（`pwsh.exe` ≠ `powershell.exe`）。

use std::path::{Path, PathBuf};

use lc_proto::{ShellInfo, ShellKind};

use super::{DirLister, EnvSnapshot, ProcessRunner};

/// Windows 候选顺序（docs/ARCHITECTURE.md §5.4.1 / RUNTIME.md §8.1）。
const PWSH_CANDIDATES: &[&str] = &[
    "%ProgramFiles%\\PowerShell\\7\\pwsh.exe",
    "%ProgramFiles%\\PowerShell\\7-preview\\pwsh.exe",
];

const PWSH_STORE_PATH: &str = "%LOCALAPPDATA%\\Microsoft\\WindowsApps\\pwsh.exe";

/// Linux 候选顺序（docs/RUNTIME.md §8.2）。必须精确找 bash，不能用 sh。
const BASH_CANDIDATES: &[&str] = &["/bin/bash", "/usr/bin/bash"];

/// 探测 shell。找不到可用 shell 返回 None（UI 据此弹引导卡）。
pub fn detect_shell(
    env: &dyn EnvSnapshot,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
) -> Option<ShellInfo> {
    if cfg!(windows) {
        detect_pwsh(env, lister, runner)
    } else {
        detect_bash(lister, runner)
    }
}

pub(crate) fn detect_pwsh(
    env: &dyn EnvSnapshot,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
) -> Option<ShellInfo> {
    // 绝对路径优先（不依赖继承的 PATH），最后 `where pwsh`，再 Store 路径兜底
    let mut candidates: Vec<PathBuf> = PWSH_CANDIDATES
        .iter()
        .filter_map(|raw| super::expand_env(raw, env))
        .collect();

    // where pwsh → 取第一条结果
    if let Ok(out) = runner.run(Path::new("where"), &["pwsh"])
        && let Some(first) = out.lines().map(str::trim).find(|l| !l.is_empty())
    {
        candidates.push(PathBuf::from(first));
    }
    if let Some(store) = super::expand_env(PWSH_STORE_PATH, env) {
        candidates.push(store);
    }

    for exe in candidates {
        let Some(info) = verify_pwsh(&exe, lister, runner) else {
            continue;
        };
        return Some(info);
    }
    None
}

/// 校验单个 pwsh 候选：major ≥ 7 才算命中（docs/ARCHITECTURE.md §5.4.1）。
fn verify_pwsh(
    exe: &Path,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
) -> Option<ShellInfo> {
    if !lister.exists(exe) || lister.is_dir(exe) {
        return None;
    }
    let out = runner
        .run(
            exe,
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$PSVersionTable.PSVersion.Major",
            ],
        )
        .ok()?;
    let major = first_int(&out)?;
    (major >= 7).then(|| ShellInfo {
        path: exe.to_string_lossy().into_owned(),
        kind: ShellKind::Pwsh,
        version_major: major,
    })
}

pub(crate) fn detect_bash(lister: &dyn DirLister, runner: &dyn ProcessRunner) -> Option<ShellInfo> {
    let mut candidates: Vec<PathBuf> = BASH_CANDIDATES.iter().map(PathBuf::from).collect();
    // which bash → 兜底候选
    if let Ok(out) = runner.run(Path::new("which"), &["bash"])
        && let Some(first) = out.lines().map(str::trim).find(|l| !l.is_empty())
    {
        candidates.push(PathBuf::from(first));
    }

    for exe in candidates {
        if !lister.exists(&exe) || lister.is_dir(&exe) {
            continue;
        }
        // bash --version → "GNU bash, version 5.2.15(1)-release ..."
        let Ok(out) = runner.run(&exe, &["--version"]) else {
            continue;
        };
        let Some(major) = first_int(&out) else {
            continue;
        };
        return Some(ShellInfo {
            path: exe.to_string_lossy().into_owned(),
            kind: ShellKind::Bash,
            version_major: major,
        });
    }
    None
}

/// 从文本中取第一个整数（pwsh 输出 "7"；bash 输出含 "version 5.2.15"）。
fn first_int(text: &str) -> Option<u32> {
    let mut num: Option<String> = None;
    for c in text.chars() {
        if c.is_ascii_digit() {
            num.get_or_insert_with(String::new).push(c);
        } else if let Some(n) = num.take() {
            return n.parse().ok();
        }
    }
    num?.parse().ok()
}

/// 真实探测入口。
pub fn detect_shell_real() -> Option<ShellInfo> {
    let env = super::RealEnv;
    let lister = super::RealDirLister;
    let runner = super::RealProcessRunner::new(super::PROBE_TIMEOUT);
    detect_shell(&env, &lister, &runner)
}
