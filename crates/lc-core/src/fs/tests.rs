//! fs 模块测试矩阵。
//!
//! 两层（对齐 docs/RUNTIME.md §9 的分层思路）：
//! - 路径校验：**纯函数单测**，走 [`FakeResolver`]，不碰真实文件系统；
//! - 遍历与监听：**真实目录集成测试**。临时目录用 `std::env::temp_dir` +
//!   进程内唯一子目录实现——任务提到 tempfile，但依赖白名单仅限
//!   ignore / notify / notify-debouncer-full，故等价手写，不引白名单外依赖。

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lc_proto::ErrorCode;

use super::listing::read_dir_level;
use super::paths::{RealPathResolver, ensure_within_root};
use super::watch::{FileWatcher, WatchEvent, WatchSink};
use super::{FsError, PathResolver, walk_tree};

// ==== fake resolver（纯函数单测用，与 runtime/tests.rs 的 FakeLister 同思路）====

/// 记录"存在"的规范化路径集合；canonicalize = 文本折叠 + 集合查询。
struct FakeResolver {
    known: Vec<PathBuf>,
}

impl PathResolver for FakeResolver {
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        let norm = normalize(path);
        if self.known.contains(&norm) {
            Ok(norm)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "fake: nope",
            ))
        }
    }
}

/// 文本层折叠 `.` / `..`（fake 只需覆盖单测用到的路径形态）。
fn normalize(path: &Path) -> PathBuf {
    let mut stack: Vec<OsString> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = stack.last().is_some_and(|s| s != "..");
                if can_pop {
                    stack.pop();
                } else if stack.is_empty() {
                    stack.push(OsString::from(".."));
                }
            }
            other => stack.push(other.as_os_str().to_os_string()),
        }
    }
    let out: PathBuf = stack.iter().collect();
    out
}

/// 平台无关的"绝对根"：Windows `C:\proj`，unix `/proj`。
fn fake_base() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\proj")
    } else {
        PathBuf::from("/proj")
    }
}

fn fake_resolver(known: &[PathBuf]) -> FakeResolver {
    FakeResolver {
        known: known.to_vec(),
    }
}

// ==== ensure_within_root：纯函数单测 ====

/// 根内合法路径（含根本身、冗余 `..` 折叠后仍在根内）。
#[test]
fn within_root_accepts_root_and_nested() {
    let base = fake_base();
    let resolver = fake_resolver(&[
        base.clone(),
        base.join("sub"),
        base.join("sub").join("file.txt"),
    ]);

    let ok = ensure_within_root(&base, &base, &resolver).expect("root itself");
    assert_eq!(ok, base);

    let nested = base.join("sub").join("file.txt");
    assert_eq!(
        ensure_within_root(&base, &nested, &resolver).expect("nested"),
        nested
    );

    let redundant = base.join("sub").join("..").join("sub").join("file.txt");
    assert!(
        ensure_within_root(&base, &redundant, &resolver).is_ok(),
        "折叠后仍在根内"
    );
}

/// `../` 逃逸：文本折叠后落在根外 → PathOutsideRoot。
#[test]
fn escape_via_parent_dir_rejected() {
    let base = fake_base();
    let secret = base.parent().unwrap().join("secret.txt");
    let resolver = fake_resolver(&[base.clone(), base.join("sub"), secret]);

    let escape = base.join("sub").join("..").join("..").join("secret.txt");
    let err = ensure_within_root(&base, &escape, &resolver).unwrap_err();
    assert!(matches!(err, FsError::PathOutsideRoot { .. }), "{err:?}");
    assert_eq!(err.code(), ErrorCode::PathOutsideRoot);
}

/// 不带 `..` 的绝对路径同样必须落在根内。
#[test]
fn absolute_outside_path_rejected() {
    let base = fake_base();
    let sibling = base.parent().unwrap().join("other");
    let resolver = fake_resolver(&[base.clone(), sibling.clone(), sibling.join("x.txt")]);

    let err = ensure_within_root(&base, &sibling.join("x.txt"), &resolver).unwrap_err();
    assert!(matches!(err, FsError::PathOutsideRoot { .. }), "{err:?}");
}

/// 不存在的路径 → NotFound（根不存在与目标不存在同语义）。
#[test]
fn nonexistent_path_is_not_found() {
    let base = fake_base();
    let resolver = fake_resolver(&[base.clone(), base.join("sub")]);

    let err = ensure_within_root(&base, &base.join("ghost.txt"), &resolver).unwrap_err();
    assert!(matches!(err, FsError::NotFound { .. }), "{err:?}");
    assert_eq!(err.code(), ErrorCode::NotFound);

    let err = ensure_within_root(&base.join("ghost-root"), &base, &resolver).unwrap_err();
    assert!(matches!(err, FsError::NotFound { .. }), "{err:?}");
}

/// Windows 大小写不敏感（docs/AGENT.md §4.1：case_sensitive_paths: false）。
#[cfg(windows)]
#[test]
fn windows_path_compare_is_case_insensitive() {
    let base = fake_base();
    let resolver = fake_resolver(&[base.clone(), PathBuf::from(r"c:\proj\file.txt")]);
    assert!(ensure_within_root(&base, Path::new(r"c:\proj\file.txt"), &resolver).is_ok());
    // 深一层的兄弟目录不得命中前缀
    let resolver = fake_resolver(&[base.clone(), PathBuf::from(r"C:\proj2\file.txt")]);
    let err = ensure_within_root(&base, Path::new(r"C:\proj2\file.txt"), &resolver).unwrap_err();
    assert!(matches!(err, FsError::PathOutsideRoot { .. }), "{err:?}");
}

/// unix 大小写敏感。
#[cfg(unix)]
#[test]
fn unix_path_compare_is_case_sensitive() {
    let base = fake_base();
    let resolver = fake_resolver(&[base.clone(), PathBuf::from("/PROJ/file.txt")]);
    let err = ensure_within_root(&base, Path::new("/PROJ/file.txt"), &resolver).unwrap_err();
    assert!(matches!(err, FsError::PathOutsideRoot { .. }), "{err:?}");
}

// ==== 真实目录集成测试辅助 ====

/// 真实临时根：`%TEMP%/lc-fs-<tag>-<pid>-<seq>`，进程内唯一。
fn temp_root(tag: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lc-fs-{tag}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("create temp root");
    dir
}

// ==== read_dir_level：真实目录 ====

#[test]
fn read_dir_level_lists_one_layer() {
    let root = temp_root("list");
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "12345").unwrap();
    fs::write(root.join("b.txt"), "").unwrap();
    fs::write(root.join(".hidden"), "").unwrap();

    let level = read_dir_level(&root, &root, &RealPathResolver).expect("one layer");
    // 一层语义：只含直接子项，排序大小写不敏感且确定
    let names: Vec<&str> = level.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec![".hidden", "a.txt", "b.txt", "sub"], "{names:?}");

    let file = &level.entries[1];
    assert_eq!(file.kind, lc_proto::EntryKind::File);
    assert_eq!(file.size, Some(5));
    let dir = &level.entries[3];
    assert_eq!(dir.kind, lc_proto::EntryKind::Dir);
    assert_eq!(dir.size, None);

    // 契约 path 字段必须是展示路径（无 `\\?\` verbatim 前缀）且可对应回文件名
    assert!(!level.path.starts_with(r"\\?\"), "{}", level.path);
    for e in &level.entries {
        assert!(
            Path::new(&e.path).ends_with(&e.name),
            "{} vs {}",
            e.path,
            e.name
        );
    }

    // 子层内容不出现：再写一层，仍只返回一层
    fs::write(root.join("sub").join("deep.txt"), "").unwrap();
    let again = read_dir_level(&root, &root, &RealPathResolver).unwrap();
    assert_eq!(again.entries.len(), 4, "不递归：{names:?}");
    assert!(!again.entries.iter().any(|e| e.name == "deep.txt"));

    // 冗余 `..` 的合法路径照常工作
    let redundant = root.join("sub").join("..");
    let via_redundant = read_dir_level(&root, &redundant, &RealPathResolver).unwrap();
    assert_eq!(via_redundant.path, level.path);

    let _ = fs::remove_dir_all(&root);
}

/// R3 真实文件系统下的逃逸与不存在路径。
#[test]
fn read_dir_level_rejects_escape_and_missing() {
    let root = temp_root("list-root");
    let outside = temp_root("list-outside");
    fs::write(outside.join("outside.txt"), "x").unwrap();

    let escape = root
        .join("..")
        .join(outside.file_name().unwrap())
        .join("outside.txt");
    let err = read_dir_level(&root, &escape, &RealPathResolver).unwrap_err();
    assert!(matches!(err, FsError::PathOutsideRoot { .. }), "{err:?}");
    assert_eq!(err.code(), ErrorCode::PathOutsideRoot);

    let err = read_dir_level(&root, &root.join("ghost.txt"), &RealPathResolver).unwrap_err();
    assert!(matches!(err, FsError::NotFound { .. }), "{err:?}");
    assert_eq!(err.code(), ErrorCode::NotFound);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
}

// ==== walk_tree：真实目录 + ignore 规则 ====

#[test]
fn walk_tree_respects_ignore_and_hidden() {
    let root = temp_root("walk");
    fs::write(root.join(".ignore"), "secret*\nbuild/\n").unwrap();
    fs::write(root.join("a.txt"), "").unwrap();
    fs::write(root.join("secret.txt"), "").unwrap();
    fs::write(root.join(".hidden"), "").unwrap();
    fs::create_dir_all(root.join("build").join("inner")).unwrap();
    fs::write(root.join("build").join("o.js"), "").unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src").join("main.rs"), "").unwrap();

    let entries = walk_tree(&root, &RealPathResolver).expect("walk");
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();

    // 命中：普通文件、目录、嵌套文件
    assert_eq!(
        entries.len(),
        3,
        "恰好 a.txt/src/main.rs，根锚点不返回：{paths:?}"
    );
    assert!(paths.iter().any(|p| p.ends_with("a.txt")), "{paths:?}");
    let src = entries
        .iter()
        .find(|e| e.name == "src")
        .expect("src 目录应出现");
    assert_eq!(src.kind, lc_proto::EntryKind::Dir);
    let main_rs = entries
        .iter()
        .find(|e| e.name == "main.rs")
        .expect("main.rs 应出现");
    assert_eq!(main_rs.kind, lc_proto::EntryKind::File);
    // 忽略：.ignore 规则 + 隐藏文件 + 根本身
    assert!(!paths.iter().any(|p| p.contains("secret")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("build")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.ends_with(".hidden")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.ends_with(".ignore")), "{paths:?}");
    assert_eq!(
        entries.iter().filter(|e| e.name == "a.txt").count(),
        1,
        "根本身不返回"
    );

    // 输出按 path 排序（树构建方依赖确定性）
    assert!(
        entries.windows(2).all(|w| w[0].path <= w[1].path),
        "未排序：{paths:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

// ==== 写文件：保存路径（P1 Editor UI lane）====

#[test]
fn write_file_creates_and_overwrites() {
    let root = temp_root("write");
    let target = root.join("note.txt");

    let written =
        super::write_file_text(&root, &target, "v1", &RealPathResolver).expect("create file");
    assert!(written.ends_with("note.txt"), "{written}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "v1");

    super::write_file_text(&root, &target, "v2", &RealPathResolver).expect("overwrite file");
    assert_eq!(fs::read_to_string(&target).unwrap(), "v2");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn write_file_rejects_outside_root() {
    let root = temp_root("write-outside");
    let outside = std::env::temp_dir().join("lc-fs-write-outside.txt");
    let _ = fs::remove_file(&outside);

    let err = super::write_file_text(&root, &outside, "x", &RealPathResolver)
        .expect_err("outside path must be rejected");
    assert_eq!(err.code(), ErrorCode::PathOutsideRoot);

    let _ = fs::remove_file(&outside);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn write_file_rejects_dot_dot_escape() {
    let root = temp_root("write-escape");
    let target = root.join("..").join("lc-fs-write-escape.txt");

    let err = super::write_file_text(&root, &target, "x", &RealPathResolver)
        .expect_err("dot-dot escape must be rejected");
    assert_eq!(err.code(), ErrorCode::PathOutsideRoot);

    let _ = fs::remove_file(&target);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn write_file_missing_parent_is_not_found() {
    let root = temp_root("write-missing");
    let target = root.join("missing").join("a.txt");

    let err = super::write_file_text(&root, &target, "x", &RealPathResolver)
        .expect_err("missing parent must fail");
    assert_eq!(err.code(), ErrorCode::NotFound);

    let _ = fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn write_file_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let root = temp_root("write-symlink");
    let outside_root = temp_root("write-symlink-outside");
    let outside_file = outside_root.join("secret.txt");
    fs::write(&outside_file, "old").unwrap();
    let link = root.join("link.txt");
    symlink(&outside_file, &link).unwrap();

    let err = super::write_file_text(&root, &link, "new", &RealPathResolver)
        .expect_err("symlink escape must be rejected");
    assert_eq!(err.code(), ErrorCode::PathOutsideRoot);
    assert_eq!(fs::read_to_string(&outside_file).unwrap(), "old");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside_root);
}

#[cfg(unix)]
#[test]
fn write_file_rejects_dangling_symlink_escape() {
    use std::os::unix::fs::symlink;

    let root = temp_root("write-dangling");
    let outside_root = temp_root("write-dangling-outside");
    let outside_file = outside_root.join("created.txt");
    let link = root.join("link.txt");
    symlink(&outside_file, &link).unwrap();

    let err = super::write_file_text(&root, &link, "new", &RealPathResolver)
        .expect_err("dangling symlink escape must be rejected");
    assert_eq!(err.code(), ErrorCode::PathOutsideRoot);
    assert!(!outside_file.exists(), "根外文件不得被创建");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside_root);
}

#[cfg(unix)]
#[test]
fn write_file_preserves_existing_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root("write-mode");
    let target = root.join("run.sh");
    fs::write(&target, "old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();

    super::write_file_text(&root, &target, "new", &RealPathResolver).expect("write");
    let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);

    let _ = fs::remove_dir_all(&root);
}

// ==== FileWatcher：真实目录 + notify ====

/// 监听超时上限：去抖 200ms + Windows/Linux CI 波动余量。
const WATCH_TIMEOUT: Duration = Duration::from_secs(15);

/// 记录收到的批次，供测试在截止时间内轮询断言。
struct RecordingSink {
    batches: Mutex<Vec<WatchEvent>>,
    signal: Condvar,
}

impl RecordingSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            batches: Mutex::new(Vec::new()),
            signal: Condvar::new(),
        })
    }

    /// 从 `start` 号批次起等首个满足 `pred` 的批次，返回（绝对序号，批次）。
    fn wait_for(
        &self,
        start: usize,
        timeout: Duration,
        pred: &impl Fn(&WatchEvent) -> bool,
    ) -> Option<(usize, WatchEvent)> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.batches.lock().expect("batches poisoned");
        loop {
            if let Some(hit) = guard
                .iter()
                .enumerate()
                .skip(start)
                .find(|(_, b)| pred(b))
                .map(|(i, b)| (i, b.clone()))
            {
                return Some(hit);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, _) = self
                .signal
                .wait_timeout(guard, deadline - now)
                .expect("signal poisoned");
            guard = next;
        }
    }
}

impl WatchSink for RecordingSink {
    fn on_batch(&self, event: WatchEvent) {
        let mut guard = self.batches.lock().expect("batches poisoned");
        guard.push(event);
        self.signal.notify_all();
    }
}

/// 批次里是否出现名为 `name` 的路径（basename 比对，规避 verbatim 前缀差异）。
fn contains_path(batch: &WatchEvent, name: &str) -> bool {
    batch
        .paths
        .iter()
        .any(|p| Path::new(p).file_name().is_some_and(|f| f == name))
}

/// 创建 / 修改 / 删除都收到事件，且 generation 单调递增。
#[test]
fn watcher_reports_create_modify_delete() {
    let root = temp_root("watch-cmd");
    let sink = RecordingSink::new();
    let watcher = FileWatcher::new(&root, sink.clone()).expect("watcher");

    let a = root.join("a.txt");
    fs::write(&a, "v1").expect("create");
    let (i1, first) = sink
        .wait_for(0, WATCH_TIMEOUT, &|b| contains_path(b, "a.txt"))
        .expect("create batch");
    assert_eq!(first.generation, 1, "首个批次 generation = 1");
    assert_eq!(first.generation, watcher.generation());

    fs::write(&a, "v2-longer").expect("modify");
    let (i2, second) = sink
        .wait_for(i1 + 1, WATCH_TIMEOUT, &|b| contains_path(b, "a.txt"))
        .expect("modify batch");
    assert!(
        second.generation > first.generation,
        "{first:?} -> {second:?}"
    );

    fs::remove_file(&a).expect("delete");
    let (_, third) = sink
        .wait_for(i2 + 1, WATCH_TIMEOUT, &|b| contains_path(b, "a.txt"))
        .expect("delete batch");
    assert!(
        third.generation > second.generation,
        "{second:?} -> {third:?}"
    );
    assert_eq!(third.generation, watcher.generation());

    drop(watcher); // 先停监听再清理，Windows 上持有句柄时目录删不掉
    let _ = fs::remove_dir_all(&root);
}

/// 短时间内的多次变更：去抖器可能合并为一批，也可能在负载下拆成多批；
/// 两条路径都必须送达，且批次 generation 严格递增。
#[test]
fn watcher_delivers_burst_paths() {
    let root = temp_root("watch-burst");
    let sink = RecordingSink::new();
    let watcher = FileWatcher::new(&root, sink.clone()).expect("watcher");

    fs::write(root.join("b.txt"), "1").expect("write b");
    fs::write(root.join("c.txt"), "2").expect("write c");

    let deadline = Instant::now() + WATCH_TIMEOUT;
    loop {
        {
            let batches = sink.batches.lock().expect("batches poisoned");
            let has_b = batches.iter().any(|b| contains_path(b, "b.txt"));
            let has_c = batches.iter().any(|b| contains_path(b, "c.txt"));
            if has_b && has_c {
                assert!(
                    batches
                        .windows(2)
                        .all(|w| w[0].generation < w[1].generation),
                    "{batches:?}"
                );
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "burst paths not delivered within {WATCH_TIMEOUT:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(watcher);
    let _ = fs::remove_dir_all(&root);
}

/// 同批多路径排序去重（与去抖时序无关的纯逻辑）。
#[test]
fn batch_paths_are_sorted_and_deduped() {
    let a = PathBuf::from("a.txt");
    let b = PathBuf::from("b.txt");
    let paths = super::watch::collect_paths([b.as_path(), a.as_path(), b.as_path(), a.as_path()]);
    assert_eq!(paths, ["a.txt", "b.txt"]);
    assert!(super::watch::collect_paths(std::iter::empty::<&Path>()).is_empty());
}

/// 监听目标不存在 / 不是目录 → 契约错误语义。
#[test]
fn watcher_rejects_missing_root_and_file_root() {
    let root = temp_root("watch-guard");
    let sink = RecordingSink::new();

    let missing = root.join("ghost");
    let err = FileWatcher::new(&missing, sink.clone()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);

    let file = root.join("file.txt");
    fs::write(&file, "x").unwrap();
    let err = FileWatcher::new(&file, sink.clone()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Io);

    let _ = fs::remove_dir_all(&root);
}
