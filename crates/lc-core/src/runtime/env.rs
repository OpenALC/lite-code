//! [`EnvSnapshot`] 的真实实现。
//!
//! Tier 4 读注册表而不是进程 `PATH`：用户改环境变量后，已运行进程
//! 继承的是旧值——这是“我明明装好了却找不到”的头号原因（docs/RUNTIME.md §3）。

use super::{EnvSnapshot, RegistryHive};

/// 真实环境。unix 下注册表永远为空（Tier 4 仅 Windows 有意义）。
pub struct RealEnv;

impl EnvSnapshot for RealEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
    }

    fn registry_path(&self, hive: RegistryHive) -> Vec<String> {
        #[cfg(windows)]
        {
            windows_registry_path(hive)
        }
        #[cfg(not(windows))]
        {
            let _ = hive;
            Vec::new()
        }
    }
}

#[cfg(windows)]
fn windows_registry_path(hive: RegistryHive) -> Vec<String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let predef = match hive {
        RegistryHive::CurrentUser => HKEY_CURRENT_USER,
        RegistryHive::LocalMachine => HKEY_LOCAL_MACHINE,
    };
    let subkey = match hive {
        // HKCU\Environment
        RegistryHive::CurrentUser => "Environment",
        // HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment
        RegistryHive::LocalMachine => {
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
        }
    };
    let Ok(root) = RegKey::predef(predef).open_subkey(subkey) else {
        return Vec::new();
    };
    let Ok(path): Result<String, _> = root.get_value("Path") else {
        return Vec::new();
    };
    path.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
