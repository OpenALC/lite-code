//! 手动指定 Node（docs/RUNTIME.md §5 手动兜底）。
//!
//! 复用 §4 的同一套 `validate`（`--version` + 二次自检），语义与探测完全一致：
//! Missing / NotAFile / StoreAlias / SpawnFailed / Unparsable / GhostInstall。
//! **校验通过才写 config**；唯一的落盘动作是 `~/.lite-code/config.json`——
//! 永不写系统 PATH、不修改 `.bashrc` / 注册表（docs/RUNTIME.md §5 红线，
//! 由构造保证：本模块没有任何 PATH / 注册表写入路径）。

use std::path::Path;

use lc_proto::{NodeFailureReason, NodeRuntimeInfo, NodeSource};

use crate::config::{
    ConfigError, ConfigStore, JsonConfigStore, NodeRuntime, now_rfc3339, store_node_runtime,
};

use super::{DirLister, ProcessRunner, validate};

#[derive(Debug, thiserror::Error)]
pub enum ManualError {
    /// 校验未通过。`1` 为失败原因，`2` 为原始细节（spawn 错误串 / 原始输出，
    /// UI 直接展示，docs/RUNTIME.md §5“失败显示原始 stderr”）。
    #[error("node 校验失败（{0:?}）")]
    Invalid(NodeFailureReason, Option<String>),
    #[error("配置写入失败：{0}")]
    Persist(#[from] ConfigError),
}

/// 校验用户手动指定的 node 路径（docs/RUNTIME.md §5：选中后立即在同一弹窗内校验）。
/// 与探测共用 [`validate`]，Store 别名桩同样被拒（不执行）。
pub fn validate_manual(
    path: &Path,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
) -> Result<NodeRuntimeInfo, ManualError> {
    validate(path, NodeSource::Manual, lister, runner)
        .map_err(|(reason, detail)| ManualError::Invalid(reason, detail))
}

/// set_manual：校验通过才写 config（docs/RUNTIME.md §5）。
/// `resolved_at` 在此生成（UTC RFC3339）。
pub fn set_manual(
    path: &Path,
    store: &dyn ConfigStore,
    lister: &dyn DirLister,
    runner: &dyn ProcessRunner,
) -> Result<NodeRuntime, ManualError> {
    let info = validate_manual(path, lister, runner)?;
    let rt = config_node_runtime(&info);
    store_node_runtime(store, rt.clone()).map_err(ManualError::Persist)?;
    Ok(rt)
}

/// 校验结果（契约 DTO）→ 配置段。source 原样保留（探测结果可为
/// 任意来源，此处入口固定产生 `Manual`）。
pub fn config_node_runtime(info: &NodeRuntimeInfo) -> NodeRuntime {
    NodeRuntime {
        path: info.path.clone(),
        version: info.version,
        source: info.source,
        resolved_at: now_rfc3339(),
    }
}

/// 真实入口（lc-cli / Tauri 壳共用）：默认 config 路径 + 真实探测组件，
/// 超时与探测一致（[`super::PROBE_TIMEOUT`]，3s）。
pub fn set_manual_real(path: &Path) -> Result<NodeRuntime, ManualError> {
    let store = JsonConfigStore::at_default_path()?;
    let lister = super::RealDirLister;
    let runner = super::RealProcessRunner::new(super::PROBE_TIMEOUT);
    set_manual(path, &store, &lister, &runner)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};

    use lc_proto::{NodeFailureReason, NodeSource, SemVer};

    use super::super::{DirLister, RunnerError};
    use super::*;

    // ---- 最小 fake（与 runtime/tests.rs 同构，独立声明避免跨测试模块耦合）----

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
        fn list_dir(&self, _path: &Path) -> Vec<String> {
            Vec::new()
        }
        fn glob_files(&self, _pattern: &Path) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    struct FakeRunner {
        versions: HashMap<String, Result<String, RunnerError>>,
        ghost: HashSet<String>,
    }

    impl FakeRunner {
        fn new(entries: &[(&str, &str)]) -> Self {
            Self {
                versions: entries
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
            let key = exe.to_string_lossy().into_owned();
            if args.first() == Some(&"-e") {
                return if self.ghost.contains(&key) {
                    Err(RunnerError::Spawn("selfcheck failed".into()))
                } else {
                    Ok(String::new())
                };
            }
            self.versions
                .get(&key)
                .cloned()
                .unwrap_or_else(|| Err(RunnerError::Spawn("no entry".into())))
        }
    }

    /// 唯一临时目录，测试结束清理。写真实 FS 但不写真实 `~/.lite-code`。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("lc-manual-test-{}-{tag}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const GOOD: &str = r"C:\tools\node\node.exe";

    // ---- 测试矩阵 ----

    /// 校验通过 → 写配置，source=Manual，resolved_at 是合法 RFC3339。
    #[test]
    fn valid_path_writes_config_with_manual_source() {
        let dir = TempDir::new("ok");
        let store = JsonConfigStore::new(dir.0.join("config.json"));
        let lister = FakeLister::new(&[GOOD]);
        let runner = FakeRunner::new(&[(GOOD, "v22.14.0")]);

        let rt = set_manual(Path::new(GOOD), &store, &lister, &runner).expect("set_manual");

        assert_eq!(rt.path, GOOD);
        assert_eq!(rt.version, SemVer::new(22, 14, 0));
        assert_eq!(rt.source, NodeSource::Manual);
        assert_eq!(rt.resolved_at.len(), 20, "{}", rt.resolved_at);
        assert!(rt.resolved_at.ends_with('Z'));

        let stored = store.load().unwrap().node_runtime.expect("已写入");
        assert_eq!(stored, rt);
    }

    /// 校验失败（路径不存在）→ 不写配置。
    #[test]
    fn missing_path_leaves_config_untouched() {
        let dir = TempDir::new("missing");
        let store = JsonConfigStore::new(dir.0.join("config.json"));
        let lister = FakeLister::new(&[]);
        let runner = FakeRunner::new(&[]);

        let err = set_manual(Path::new(GOOD), &store, &lister, &runner).unwrap_err();
        let ManualError::Invalid(reason, detail) = err else {
            panic!("expected Invalid, got {err:?}")
        };
        assert_eq!(reason, NodeFailureReason::Missing);
        assert!(detail.is_none());

        assert!(store.load().unwrap().node_runtime.is_none(), "不应写入");
    }

    /// exe 在但二次自检失败（GhostInstall）→ 拒绝写配置。
    #[test]
    fn ghost_install_rejected() {
        let dir = TempDir::new("ghost");
        let store = JsonConfigStore::new(dir.0.join("config.json"));
        let lister = FakeLister::new(&[GOOD]);
        let runner = FakeRunner::new(&[(GOOD, "v22.14.0")]).with_ghost(&[GOOD]);

        let err = set_manual(Path::new(GOOD), &store, &lister, &runner).unwrap_err();
        let ManualError::Invalid(reason, detail) = err else {
            panic!("expected Invalid, got {err:?}")
        };
        assert_eq!(reason, NodeFailureReason::GhostInstall);
        // detail 是 RunnerError 的原始 Display（docs/RUNTIME.md §5：显示原始 stderr）
        assert_eq!(detail.as_deref(), Some("spawn failed: selfcheck failed"));

        assert!(store.load().unwrap().node_runtime.is_none());
    }

    /// Store 别名桩：路径含 WindowsApps 直接拒绝，即使 fake 输出正常也不执行。
    #[test]
    fn store_alias_stub_rejected() {
        let dir = TempDir::new("storealias");
        let store = JsonConfigStore::new(dir.0.join("config.json"));
        let stub = r"C:\Users\t\AppData\Local\Microsoft\WindowsApps\node.exe";
        let lister = FakeLister::new(&[stub]);
        let runner = FakeRunner::new(&[(stub, "v22.14.0")]);

        let err = set_manual(Path::new(stub), &store, &lister, &runner).unwrap_err();
        let ManualError::Invalid(reason, _) = err else {
            panic!("expected Invalid, got {err:?}")
        };
        assert_eq!(reason, NodeFailureReason::StoreAlias);

        assert!(store.load().unwrap().node_runtime.is_none());
    }

    /// `--version` 输出解析不出版本 → Unparsable，原始输出进 detail。
    #[test]
    fn unparsable_version_carries_raw_output() {
        let dir = TempDir::new("unparsable");
        let store = JsonConfigStore::new(dir.0.join("config.json"));
        let lister = FakeLister::new(&[GOOD]);
        let runner = FakeRunner::new(&[(GOOD, "hello world")]);

        let err = set_manual(Path::new(GOOD), &store, &lister, &runner).unwrap_err();
        let ManualError::Invalid(NodeFailureReason::Unparsable, detail) = err else {
            panic!("expected Unparsable")
        };
        assert_eq!(detail.as_deref(), Some("hello world"));
    }

    /// config_node_runtime：字段逐项拷贝 + 生成时间戳。
    #[test]
    fn config_node_runtime_copies_fields() {
        let info = NodeRuntimeInfo {
            path: GOOD.into(),
            version: SemVer::new(24, 15, 0),
            source: NodeSource::Portable,
            health: lc_proto::NodeHealth::Supported,
        };
        let rt = config_node_runtime(&info);
        assert_eq!(rt.path, info.path);
        assert_eq!(rt.version, info.version);
        assert_eq!(rt.source, NodeSource::Portable);
        assert_eq!(rt.resolved_at.len(), 20);
    }
}
