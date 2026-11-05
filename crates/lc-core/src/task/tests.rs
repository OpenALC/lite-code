//! task lane 集成测试：真实临时 git 仓库 + 真实 worktree。
//!
//! 全部用 std 与系统 git（Windows / Unix 同一套命令），不引入测试依赖；
//! 每个用例独立临时目录，互不共享仓库状态。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lc_proto::{ChangeStatus, ErrorCode, TaskStatus};

use super::*;

struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let seq = N.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "lc-task-test-{}-{nanos}-{tag}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn repo(&self) -> PathBuf {
        self.0.join("repo")
    }

    fn data(&self) -> PathBuf {
        self.0.join("data")
    }

    fn store(&self, repo: &Path) -> TaskStore {
        TaskStore::new(self.data().join("tasks.json"), repo)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn init_repo(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).expect("create repo dir");
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "task-test@example.com"]);
    git(root, &["config", "user.name", "Task Test"]);
    git(root, &["config", "core.autocrlf", "false"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "hello\n").expect("write README");
    commit_all(root, "init");
    root.to_path_buf()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .output()
        .unwrap_or_else(|error| panic!("spawn git {args:?}: {error}"));
    assert!(
        out.status.success(),
        "git {args:?} failed in {}:\nstdout: {}\nstderr: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_quiet(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("spawn git")
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn write_file(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).expect("write test file");
}

fn worktree_path(task: &lc_proto::TaskInfo) -> PathBuf {
    PathBuf::from(task.worktree_path.clone().expect("task has worktree"))
}

fn worktree_listed(repo: &Path, path: &Path) -> bool {
    let target = std::fs::canonicalize(path).ok();
    let listing = git(repo, &["worktree", "list", "--porcelain"]);
    listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter_map(|listed| std::fs::canonicalize(listed).ok())
        .any(|listed| target.as_deref() == Some(listed.as_path()))
}

fn assert_port_in_range(port: u16) {
    assert!(
        (PORT_RANGE_START..=PORT_RANGE_END).contains(&port),
        "port {port} outside {PORT_RANGE_START}-{PORT_RANGE_END}"
    );
}

// ---- 持久化 ----

#[test]
fn create_list_persist_reload() {
    let td = TestDir::new("create-list");
    let repo = init_repo(&td.repo());
    let persist = td.data().join("tasks.json");
    let mut store = TaskStore::new(&persist, &repo);

    let first = store
        .create("first task", Some("session-1".into()))
        .unwrap();
    let second = store.create("second", None).unwrap();

    assert_eq!(first.status, TaskStatus::Planning);
    assert_eq!(first.session_id.as_deref(), Some("session-1"));
    assert_ne!(first.task_id, second.task_id);
    assert!(first.task_id.starts_with("task-"));
    assert_eq!(store.list().len(), 2);
    assert_eq!(store.list()[0].task_id, first.task_id);
    assert_eq!(store.list()[1].task_id, second.task_id);

    let reloaded = TaskStore::new(&persist, &repo);
    assert_eq!(reloaded.list().len(), 2);
    let again = reloaded.get(&first.task_id).expect("task after reload");
    assert_eq!(again.title, "first task");
    assert_eq!(again.session_id.as_deref(), Some("session-1"));
    assert_eq!(again.status, TaskStatus::Planning);
}

#[test]
fn reload_marks_running_failed_and_keeps_queued() {
    let td = TestDir::new("reload-interrupted");
    let repo = init_repo(&td.repo());
    let persist = td.data().join("tasks.json");
    let mut store = TaskStore::new(&persist, &repo);

    let t1 = store.create("one", None).unwrap();
    let t2 = store.create("two", None).unwrap();
    let t3 = store.create("three", None).unwrap();
    assert_eq!(
        store.start(&t1.task_id).unwrap().status,
        TaskStatus::Running
    );
    assert_eq!(
        store.start(&t2.task_id).unwrap().status,
        TaskStatus::Running
    );
    assert_eq!(store.start(&t3.task_id).unwrap().status, TaskStatus::Queued);
    assert!(store.get(&t1.task_id).unwrap().port.is_some());

    drop(store);
    let mut store = TaskStore::new(&persist, &repo);

    let t1 = store.get(&t1.task_id).unwrap();
    let t2 = store.get(&t2.task_id).unwrap();
    let t3 = store.get(&t3.task_id).unwrap();
    assert_eq!(t1.status, TaskStatus::Failed);
    assert_eq!(t2.status, TaskStatus::Failed);
    assert_eq!(t3.status, TaskStatus::Queued);
    assert!(t1.port.is_none());
    assert!(t2.port.is_none());
    assert!(t3.port.is_none());
    assert!(
        store
            .notes(&t1.task_id)
            .iter()
            .any(|n| n.contains("interrupted"))
    );

    let promoted = store
        .promote_queued()
        .unwrap()
        .expect("queued task promoted");
    assert_eq!(promoted.task_id, t3.task_id);
    assert_eq!(promoted.status, TaskStatus::Running);
    assert_eq!(promoted.port, Some(PORT_RANGE_START));
}

#[test]
fn task_stores_are_isolated_per_workspace() {
    let td = TestDir::new("per-workspace");
    let alpha = init_repo(&td.path().join("alpha"));
    let beta = td.path().join("beta");
    std::fs::create_dir_all(&beta).expect("create beta");

    let mut alpha_store = TaskStore::load_at(&td.data(), &alpha).unwrap();
    assert!(alpha_store.legacy_store_warning().is_none());
    let alpha_task = alpha_store.create("alpha task", None).unwrap();
    let alpha_path = alpha_store.persist_path().to_path_buf();
    assert!(alpha_path.starts_with(td.data().join("tasks")));
    assert!(
        worktree_path(&alpha_task).starts_with(td.data().join("worktrees")),
        "worktrees must stay under <data>/worktrees: {:?}",
        alpha_task.worktree_path
    );
    assert_eq!(
        alpha_path.extension().and_then(|ext| ext.to_str()),
        Some("json")
    );
    drop(alpha_store);

    let beta_store = TaskStore::load_at(&td.data(), &beta).unwrap();
    assert!(
        beta_store.list().is_empty(),
        "beta workspace must not see alpha tasks"
    );
    assert_ne!(beta_store.persist_path(), alpha_path.as_path());
    drop(beta_store);

    let reopened = TaskStore::load_at(&td.data(), &alpha).unwrap();
    assert_eq!(reopened.list().len(), 1);
    assert_eq!(
        reopened.get(&alpha_task.task_id).unwrap().title,
        "alpha task"
    );
    assert_eq!(reopened.persist_path(), alpha_path.as_path());
}

#[test]
fn legacy_global_task_store_warns_without_importing() {
    let td = TestDir::new("legacy-store");
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).expect("create repo");
    std::fs::create_dir_all(td.data()).expect("create data dir");

    let legacy = td.data().join("tasks.json");
    let legacy_json = serde_json::json!({
        "version": 1,
        "tasks": [{
            "task": {
                "task_id": "task-legacy",
                "title": "legacy task",
                "session_id": null,
                "status": "planning",
                "worktree_path": null,
                "branch": null,
                "port": null,
                "base_sha": null,
                "steps": [],
                "changes": [],
                "checks": [],
                "created_at": 1
            },
            "notes": []
        }]
    });
    std::fs::write(&legacy, legacy_json.to_string()).expect("write legacy store");

    let store = TaskStore::load_at(&td.data(), &repo).unwrap();
    assert!(
        store.list().is_empty(),
        "legacy tasks must not leak into the new per-workspace store"
    );
    let warning = store.legacy_store_warning().expect("legacy store warning");
    assert!(warning.contains("tasks.json"), "warning: {warning}");
    assert!(warning.contains("manually"), "warning: {warning}");
    assert!(
        legacy.is_file(),
        "legacy file must not be migrated or deleted"
    );
}

// ---- 非 git / worktree 路径 ----

#[test]
fn non_git_task_has_no_worktree_but_gets_port() {
    let td = TestDir::new("non-git");
    let plain = td.path().join("plain");
    std::fs::create_dir_all(&plain).expect("create plain dir");
    let mut store = td.store(&plain);

    let task = store.create("plain task", None).unwrap();
    assert!(task.worktree_path.is_none());
    assert!(task.branch.is_none());
    assert!(task.base_sha.is_none());
    assert!(
        store
            .notes(&task.task_id)
            .iter()
            .any(|n| n.contains("not a git"))
    );

    let started = store.start(&task.task_id).unwrap();
    assert_eq!(started.status, TaskStatus::Running);
    let port = started.port.expect("port for non-git task");
    assert_port_in_range(port);
    assert!(!plain.join("worktrees").exists());

    // 非 git 任务共用主工作区：串行槽被占时第二个任务排队。
    let second = store.create("second plain task", None).unwrap();
    let queued = store.start(&second.task_id).unwrap();
    assert_eq!(queued.status, TaskStatus::Queued);
    store.cancel(&task.task_id).unwrap();
    let promoted = store
        .promote_queued()
        .unwrap()
        .expect("serial task promoted");
    assert_eq!(promoted.task_id, second.task_id);
    assert_eq!(promoted.status, TaskStatus::Running);
    assert_eq!(promoted.port, Some(port), "released port is reused");

    let discarded = store.discard(&promoted.task_id).unwrap();
    assert_eq!(discarded.status, TaskStatus::Failed);
    assert!(discarded.worktree_path.is_none());
    assert!(discarded.port.is_none());
}

#[test]
fn worktree_lives_in_data_dir_and_branch_is_valid() {
    let td = TestDir::new("worktree-path");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);

    let task = store.create("Fix the login flow", None).unwrap();
    let path = worktree_path(&task);
    assert!(path.is_dir());
    assert!(path.starts_with(td.data()));
    assert!(!path.starts_with(&repo));
    assert!(!path_inside(&path, &repo));
    assert!(path.ends_with(&task.task_id));

    let branch = task.branch.clone().expect("branch");
    assert!(branch.starts_with("lc/"));
    assert!(branch.contains(&task.task_id));
    assert!(branch.ends_with("fix-the-login-flow"));
    git(&repo, &["check-ref-format", "--branch", &branch]);

    let listing = git(&repo, &["worktree", "list", "--porcelain"]);
    assert!(
        worktree_listed(&repo, &path),
        "worktree list must contain {}:\n{listing}",
        path.display()
    );
    assert!(task.base_sha.is_some());
}

// ---- 并发与队列 ----

#[test]
fn concurrency_limit_queues_third_and_promotes_it() {
    let td = TestDir::new("queue-cap");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);

    let t1 = store.create("one", None).unwrap();
    let t2 = store.create("two", None).unwrap();
    let t3 = store.create("three", None).unwrap();

    assert_eq!(
        store.start(&t1.task_id).unwrap().status,
        TaskStatus::Running
    );
    assert_eq!(
        store.start(&t2.task_id).unwrap().status,
        TaskStatus::Running
    );
    let queued = store.start(&t3.task_id).unwrap();
    assert_eq!(queued.status, TaskStatus::Queued);
    assert!(queued.port.is_none());

    let running = store
        .list()
        .into_iter()
        .filter(|task| task.status == TaskStatus::Running)
        .count();
    assert_eq!(running, 2);

    let canceled = store.cancel(&t1.task_id).unwrap();
    assert_eq!(canceled.status, TaskStatus::Failed);
    assert!(canceled.port.is_none());

    let promoted = store
        .promote_queued()
        .unwrap()
        .expect("queued task promoted");
    assert_eq!(promoted.task_id, t3.task_id);
    assert_eq!(promoted.status, TaskStatus::Running);
    assert_eq!(promoted.port, Some(PORT_RANGE_START));
    assert!(worktree_path(&promoted).is_dir());
}

#[test]
fn promote_queued_respects_creation_order() {
    let td = TestDir::new("queue-order");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);

    let t1 = store.create("one", None).unwrap();
    let t2 = store.create("two", None).unwrap();
    let t3 = store.create("three", None).unwrap();
    let t4 = store.create("four", None).unwrap();
    for task in [&t1, &t2, &t3, &t4] {
        store.start(&task.task_id).unwrap();
    }
    assert_eq!(store.get(&t3.task_id).unwrap().status, TaskStatus::Queued);
    assert_eq!(store.get(&t4.task_id).unwrap().status, TaskStatus::Queued);

    store.cancel(&t1.task_id).unwrap();
    store.cancel(&t2.task_id).unwrap();
    let first = store.promote_queued().unwrap().expect("first promoted");
    assert_eq!(first.task_id, t3.task_id);
    assert_eq!(first.status, TaskStatus::Running);
    let second = store.promote_queued().unwrap().expect("second promoted");
    assert_eq!(second.task_id, t4.task_id);
    assert_eq!(second.status, TaskStatus::Running);
    assert!(store.promote_queued().unwrap().is_none());
    assert_eq!(store.get(&t4.task_id).unwrap().status, TaskStatus::Running);
}

#[test]
fn two_worktrees_are_isolated_and_main_stays_clean() {
    let td = TestDir::new("isolation");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);

    let t1 = store.create("alpha", None).unwrap();
    let t2 = store.create("beta", None).unwrap();
    store.start(&t1.task_id).unwrap();
    store.start(&t2.task_id).unwrap();

    let wt1 = worktree_path(&t1);
    let wt2 = worktree_path(&t2);
    write_file(&wt1, "only-a.txt", "alpha\n");
    commit_all(&wt1, "alpha change");
    write_file(&wt2, "only-b.txt", "beta\n");
    commit_all(&wt2, "beta change");

    assert!(wt1.join("only-a.txt").is_file());
    assert!(!wt1.join("only-b.txt").exists());
    assert!(wt2.join("only-b.txt").is_file());
    assert!(!wt2.join("only-a.txt").exists());
    assert!(!repo.join("only-a.txt").exists());
    assert!(!repo.join("only-b.txt").exists());
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
}

// ---- changes / diff ----

#[test]
fn changes_parse_statuses_and_numstat() {
    let td = TestDir::new("changes");
    let repo = init_repo(&td.repo());
    write_file(&repo, "gone.txt", "gone\n");
    write_file(&repo, "old.txt", "rename me\n");
    std::fs::create_dir_all(repo.join("sub")).expect("create sub dir");
    write_file(&repo, "sub/old-name.txt", "nested rename\n");
    commit_all(&repo, "seed files");

    let mut store = td.store(&repo);
    let task = store.create("changes task", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);

    write_file(&wt, "README.md", "hello world\n");
    write_file(&wt, "added.txt", "added\n");
    git(&wt, &["add", "added.txt"]);
    std::fs::remove_file(wt.join("gone.txt")).expect("remove tracked file");
    git(&wt, &["mv", "old.txt", "renamed.txt"]);
    git(&wt, &["mv", "sub/old-name.txt", "sub/new-name.txt"]);
    write_file(&wt, "untracked.txt", "u1\nu2\nu3\n");

    let refreshed = store.refresh(&task.task_id).unwrap();
    let find = |path: &str, changes: &[lc_proto::FileChangeDto]| {
        changes
            .iter()
            .find(|change| change.path == path)
            .unwrap_or_else(|| panic!("missing change {path}: {changes:?}"))
            .clone()
    };

    let readme = find("README.md", &refreshed.changes);
    assert_eq!(readme.status, ChangeStatus::Modified);
    assert_eq!((readme.added, readme.removed), (1, 1));
    assert!(!readme.applied);

    let added = find("added.txt", &refreshed.changes);
    assert_eq!(added.status, ChangeStatus::Added);
    assert_eq!((added.added, added.removed), (1, 0));

    let deleted = find("gone.txt", &refreshed.changes);
    assert_eq!(deleted.status, ChangeStatus::Deleted);
    assert_eq!((deleted.added, deleted.removed), (0, 1));

    let renamed = find("renamed.txt", &refreshed.changes);
    assert_eq!(renamed.status, ChangeStatus::Renamed);
    assert_eq!((renamed.added, renamed.removed), (0, 0));

    // 共同前缀/后缀的重命名在 numstat 里是 `sub/{old => new}` 形式。
    let nested = find("sub/new-name.txt", &refreshed.changes);
    assert_eq!(nested.status, ChangeStatus::Renamed);
    assert_eq!((nested.added, nested.removed), (0, 0));

    let untracked = find("untracked.txt", &refreshed.changes);
    assert_eq!(untracked.status, ChangeStatus::Added);
    assert_eq!((untracked.added, untracked.removed), (3, 0));
}

#[test]
fn diff_contains_tracked_and_untracked_changes() {
    let td = TestDir::new("diff");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("diff task", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);

    write_file(&wt, "README.md", "hello changed\n");
    write_file(&wt, "new-file.txt", "brand new\ncontent\n");

    let diff = store.diff(&task.task_id).unwrap();
    assert!(diff.patch.contains("hello changed"));
    assert!(diff.patch.contains("-hello"));
    assert!(diff.patch.contains("new-file.txt"));
    assert!(diff.patch.contains("+brand new"));
    assert_eq!(diff.task_id, task.task_id);
    assert_eq!(diff.files.len(), 2);
    let readme = diff
        .files
        .iter()
        .find(|change| change.path == "README.md")
        .expect("README change");
    assert_eq!(readme.status, ChangeStatus::Modified);
    let new_file = diff
        .files
        .iter()
        .find(|change| change.path == "new-file.txt")
        .expect("untracked change");
    assert_eq!(new_file.status, ChangeStatus::Added);
    assert_eq!(new_file.added, 2);
    assert_eq!(store.get(&task.task_id).unwrap().changes.len(), 2);
}

// ---- apply ----

#[test]
fn apply_success_merges_and_cleans_up() {
    let td = TestDir::new("apply-ok");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("apply ok", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);
    let branch = task.branch.clone().expect("branch");

    write_file(&wt, "feature.txt", "feature\n");
    let refreshed = store.refresh(&task.task_id).unwrap();
    assert_eq!(refreshed.changes.len(), 1);
    commit_all(&wt, "feature");

    let outcome = store.apply(&task.task_id).unwrap();
    assert!(!outcome.commit.is_empty());
    assert_eq!(outcome.base_drift, 0);
    assert!(repo.join("feature.txt").is_file());
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");

    let applied = store.get(&task.task_id).unwrap();
    assert_eq!(applied.status, TaskStatus::Done);
    assert!(applied.worktree_path.is_none());
    assert!(applied.branch.is_none());
    assert!(applied.port.is_none());
    assert!(applied.changes.iter().all(|change| change.applied));
    assert!(!wt.exists());
    assert!(
        !git_quiet(&repo, &["rev-parse", "--verify", "--quiet", &branch])
            .status
            .success()
    );
    assert!(
        !worktree_listed(&repo, &wt),
        "removed worktree must disappear from git worktree list"
    );
}

#[test]
fn apply_auto_commits_dirty_worktree() {
    let td = TestDir::new("apply-autocommit");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("dirty apply", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);

    write_file(&wt, "dirty.txt", "uncommitted\n");
    let outcome = store.apply(&task.task_id).unwrap();
    assert!(repo.join("dirty.txt").is_file());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("uncommitted worktree changes"))
    );
    let after = store.get(&task.task_id).unwrap();
    assert_eq!(after.status, TaskStatus::Done);
    assert!(
        after
            .changes
            .iter()
            .any(|change| change.path == "dirty.txt" && change.applied),
        "apply should scan and mark changes as applied: {:?}",
        after.changes
    );
}

#[test]
fn apply_refuses_ignored_files_until_removed() {
    let td = TestDir::new("apply-ignored");
    let repo = init_repo(&td.repo());
    write_file(&repo, ".gitignore", ".env\n");
    commit_all(&repo, "ignore env");
    let mut store = td.store(&repo);
    let task = store.create("ignored", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);
    let branch = task.branch.clone().expect("branch");

    write_file(&wt, "feature.txt", "feature\n");
    commit_all(&wt, "feature");
    write_file(&wt, ".env", "SECRET=1\n");

    let error = store.apply(&task.task_id).unwrap_err();
    match &error {
        TaskError::IgnoredFiles { paths, .. } => {
            assert!(
                paths.iter().any(|path| path == ".env"),
                "paths should list the ignored file: {paths:?}"
            );
        }
        other => panic!("expected IgnoredFiles, got {other:?}"),
    }
    let body = error.to_error_body();
    assert_eq!(body.code, ErrorCode::InvalidArgument);
    let detail = body.detail.expect("ignored paths detail");
    assert!(detail.contains(".env"), "detail: {detail}");
    assert!(detail.contains("apply/discard"), "detail: {detail}");

    assert!(wt.join(".env").is_file(), "ignored file must be preserved");
    assert!(
        !repo.join("feature.txt").exists(),
        "apply must not merge while ignored files exist"
    );
    assert_eq!(
        store.get(&task.task_id).unwrap().status,
        TaskStatus::Running
    );
    git(&repo, &["rev-parse", "--verify", &branch]);

    std::fs::remove_file(wt.join(".env")).expect("remove ignored file");
    let outcome = store.apply(&task.task_id).unwrap();
    assert!(!outcome.commit.is_empty());
    assert!(repo.join("feature.txt").is_file());
    assert_eq!(store.get(&task.task_id).unwrap().status, TaskStatus::Done);
    assert!(!wt.exists());
}

#[test]
fn apply_conflict_keeps_worktree_and_marks_review() {
    let td = TestDir::new("apply-conflict");
    let repo = init_repo(&td.repo());
    let persist = td.data().join("tasks.json");
    let mut store = TaskStore::new(&persist, &repo);
    let task = store.create("conflict", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);
    let branch = task.branch.clone().expect("branch");

    write_file(&wt, "README.md", "task side\n");
    commit_all(&wt, "task side");
    write_file(&repo, "README.md", "main side\n");
    commit_all(&repo, "main side");

    let error = store.apply(&task.task_id).unwrap_err();
    assert!(matches!(error, TaskError::MergeConflict { .. }));

    let after = store.get(&task.task_id).unwrap();
    assert_eq!(after.status, TaskStatus::Review);
    assert_eq!(
        after.worktree_path.as_deref(),
        Some(wt.to_string_lossy().as_ref())
    );
    assert_eq!(after.branch.as_deref(), Some(branch.as_str()));
    assert!(after.port.is_none());
    assert!(wt.is_dir());
    assert!(git(&repo, &["status", "--porcelain"]).contains("UU"));
    git(&repo, &["rev-parse", "--verify", &branch]);

    // 冲突未解决时再次 apply：仍返回 MergeConflict，不自动选边，任务保持 Review。
    let again = store.apply(&task.task_id).unwrap_err();
    assert!(matches!(again, TaskError::MergeConflict { .. }));
    assert_eq!(store.get(&task.task_id).unwrap().status, TaskStatus::Review);
    assert!(wt.is_dir());

    // 失败不丢数据：worktree 仍可 diff，冲突后写入的改动可见。
    write_file(&wt, "after-conflict.txt", "still editable\n");
    let diff = store.diff(&task.task_id).unwrap();
    assert!(diff.patch.contains("after-conflict.txt"));

    drop(store);
    let reloaded = TaskStore::new(&persist, &repo);
    let after_reload = reloaded.get(&task.task_id).unwrap();
    assert_eq!(after_reload.status, TaskStatus::Review);
    assert_eq!(
        after_reload.worktree_path.as_deref(),
        Some(wt.to_string_lossy().as_ref())
    );
    assert!(after_reload.port.is_none());
}

#[test]
fn apply_reports_base_drift_and_still_merges() {
    let td = TestDir::new("apply-drift");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("drift", None).unwrap();
    assert!(task.base_sha.is_some());
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);

    write_file(&wt, "feature.txt", "feature\n");
    commit_all(&wt, "feature");
    write_file(&repo, "main.txt", "main\n");
    commit_all(&repo, "advance main");

    let outcome = store.apply(&task.task_id).unwrap();
    assert_eq!(outcome.base_drift, 1);
    assert!(repo.join("feature.txt").is_file());
    assert!(repo.join("main.txt").is_file());
    assert_eq!(store.get(&task.task_id).unwrap().status, TaskStatus::Done);
}

#[test]
fn apply_after_conflict_resolved_and_committed_finishes_task() {
    let td = TestDir::new("apply-conflict-committed");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("resolve conflict", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);
    let branch = task.branch.clone().expect("branch");

    write_file(&wt, "README.md", "task side\n");
    commit_all(&wt, "task side");
    write_file(&repo, "README.md", "main side\n");
    commit_all(&repo, "main side");

    assert!(matches!(
        store.apply(&task.task_id).unwrap_err(),
        TaskError::MergeConflict { .. }
    ));
    assert_eq!(store.get(&task.task_id).unwrap().status, TaskStatus::Review);

    // 模拟编辑器解决冲突：写最终内容、暂存并提交（MERGE_HEAD 存在时 git 生成 merge commit）。
    write_file(&repo, "README.md", "merged side\n");
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-q", "-m", "resolve conflict"]);
    assert!(
        !git_quiet(&repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .status
            .success(),
        "manual commit should conclude the merge"
    );

    let outcome = store.apply(&task.task_id).unwrap();
    assert!(!outcome.commit.is_empty());
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "merged side\n"
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");

    let done = store.get(&task.task_id).unwrap();
    assert_eq!(done.status, TaskStatus::Done);
    assert!(done.worktree_path.is_none());
    assert!(done.branch.is_none());
    assert!(done.port.is_none());
    assert!(!wt.exists());
    assert!(
        !git_quiet(&repo, &["rev-parse", "--verify", "--quiet", &branch])
            .status
            .success()
    );
}

#[test]
fn apply_commits_merge_resolved_but_only_staged() {
    let td = TestDir::new("apply-conflict-staged");
    let repo = init_repo(&td.repo());
    let persist = td.data().join("tasks.json");
    let mut store = TaskStore::new(&persist, &repo);
    let task = store.create("staged resolution", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);
    let branch = task.branch.clone().expect("branch");

    write_file(&wt, "README.md", "task side\n");
    commit_all(&wt, "task side");
    write_file(&repo, "README.md", "main side\n");
    commit_all(&repo, "main side");

    assert!(matches!(
        store.apply(&task.task_id).unwrap_err(),
        TaskError::MergeConflict { .. }
    ));

    // 只解决 + 暂存，不手动提交：apply 应识别 MERGE_HEAD 并代为提交 merge。
    write_file(&repo, "README.md", "merged side\n");
    git(&repo, &["add", "README.md"]);
    assert!(
        git_quiet(&repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .status
            .success(),
        "staged resolution should keep MERGE_HEAD"
    );

    // 模拟用户中途重启应用：Review 状态与主工作区 merge 状态都应保留。
    drop(store);
    let mut store = TaskStore::new(&persist, &repo);
    assert_eq!(store.get(&task.task_id).unwrap().status, TaskStatus::Review);

    let outcome = store.apply(&task.task_id).unwrap();
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("resolved merge committed")),
        "warnings should record the pending merge: {:?}",
        outcome.warnings
    );
    let head = git(&repo, &["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(
        head.split_whitespace().count(),
        3,
        "HEAD should be a merge commit: {head}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "merged side\n"
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");

    let done = store.get(&task.task_id).unwrap();
    assert_eq!(done.status, TaskStatus::Done);
    assert!(!wt.exists());
    assert!(
        !git_quiet(&repo, &["rev-parse", "--verify", "--quiet", &branch])
            .status
            .success()
    );
}

#[test]
fn parallel_tasks_merge_same_file_different_parts() {
    let td = TestDir::new("parallel-same-file");
    let repo = init_repo(&td.repo());
    let base: String = (1..=20).map(|line| format!("line {line}\n")).collect();
    write_file(&repo, "shared.txt", &base);
    commit_all(&repo, "shared base");

    let mut store = td.store(&repo);
    let top = store.create("top change", None).unwrap();
    let bottom = store.create("bottom change", None).unwrap();
    store.start(&top.task_id).unwrap();
    store.start(&bottom.task_id).unwrap();
    let wt_top = worktree_path(&top);
    let wt_bottom = worktree_path(&bottom);

    write_file(
        &wt_top,
        "shared.txt",
        &base.replace("line 2\n", "line 2 changed by top\n"),
    );
    commit_all(&wt_top, "top hunk");
    write_file(
        &wt_bottom,
        "shared.txt",
        &base.replace("line 19\n", "line 19 changed by bottom\n"),
    );
    commit_all(&wt_bottom, "bottom hunk");

    let first = store.apply(&top.task_id).unwrap();
    assert_eq!(first.base_drift, 0);
    let second = store.apply(&bottom.task_id).unwrap();
    assert!(
        second.base_drift >= 1,
        "top apply should advance main: {}",
        second.base_drift
    );

    let merged = std::fs::read_to_string(repo.join("shared.txt")).unwrap();
    assert!(merged.contains("line 2 changed by top"));
    assert!(merged.contains("line 19 changed by bottom"));
    assert!(merged.contains("line 11\n"));
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    assert_eq!(store.get(&top.task_id).unwrap().status, TaskStatus::Done);
    assert_eq!(store.get(&bottom.task_id).unwrap().status, TaskStatus::Done);
    assert!(!wt_top.exists());
    assert!(!wt_bottom.exists());
}

#[test]
fn apply_base_drift_counts_multiple_main_commits() {
    let td = TestDir::new("apply-drift-multi");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("drift multi", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);

    write_file(&wt, "feature.txt", "feature\n");
    commit_all(&wt, "feature");
    write_file(&repo, "main-a.txt", "a\n");
    commit_all(&repo, "main a");
    write_file(&repo, "main-b.txt", "b\n");
    commit_all(&repo, "main b");

    let outcome = store.apply(&task.task_id).unwrap();
    assert_eq!(outcome.base_drift, 2);
    assert!(repo.join("feature.txt").is_file());
    assert!(repo.join("main-a.txt").is_file());
    assert!(repo.join("main-b.txt").is_file());
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    assert_eq!(store.get(&task.task_id).unwrap().status, TaskStatus::Done);
}

#[test]
fn apply_rejects_dirty_main_worktree() {
    let td = TestDir::new("apply-dirty-main");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("dirty main", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);

    write_file(&wt, "feature.txt", "feature\n");
    commit_all(&wt, "feature");
    write_file(&repo, "dirty.txt", "dirty\n");

    let error = store.apply(&task.task_id).unwrap_err();
    assert!(matches!(error, TaskError::DirtyMainWorktree { .. }));
    assert_eq!(
        store.get(&task.task_id).unwrap().status,
        TaskStatus::Running
    );
    assert!(wt.is_dir());
    assert!(!repo.join("feature.txt").exists());
}

// ---- discard / 端口 / 状态机 ----

#[test]
fn discard_rejects_dirty_and_removes_clean_worktree() {
    let td = TestDir::new("discard");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("discard", None).unwrap();
    let started = store.start(&task.task_id).unwrap();
    assert!(started.port.is_some());
    let wt = worktree_path(&task);
    let branch = task.branch.clone().expect("branch");

    write_file(&wt, "wip.txt", "wip\n");
    let error = store.discard(&task.task_id).unwrap_err();
    assert!(matches!(error, TaskError::DirtyWorktree { .. }));
    assert!(wt.is_dir());
    assert_eq!(
        store.get(&task.task_id).unwrap().status,
        TaskStatus::Running
    );

    std::fs::remove_file(wt.join("wip.txt")).expect("remove wip");
    let discarded = store.discard(&task.task_id).unwrap();
    assert_eq!(discarded.status, TaskStatus::Failed);
    assert!(discarded.worktree_path.is_none());
    assert!(discarded.branch.is_none());
    assert!(discarded.port.is_none());
    assert!(!wt.exists());
    assert!(
        !git_quiet(&repo, &["rev-parse", "--verify", "--quiet", &branch])
            .status
            .success()
    );
}

#[test]
fn discard_refuses_ignored_files_until_removed() {
    let td = TestDir::new("discard-ignored");
    let repo = init_repo(&td.repo());
    write_file(&repo, ".gitignore", ".env\n");
    commit_all(&repo, "ignore env");
    let mut store = td.store(&repo);
    let task = store.create("discard ignored", None).unwrap();
    store.start(&task.task_id).unwrap();
    let wt = worktree_path(&task);
    let branch = task.branch.clone().expect("branch");

    write_file(&wt, ".env", "SECRET=1\n");
    let error = store.discard(&task.task_id).unwrap_err();
    assert!(matches!(error, TaskError::IgnoredFiles { .. }));
    let detail = error.to_error_body().detail.expect("detail");
    assert!(detail.contains(".env"), "detail: {detail}");

    assert!(wt.join(".env").is_file(), "ignored file must be preserved");
    assert_eq!(
        store.get(&task.task_id).unwrap().status,
        TaskStatus::Running
    );
    git(&repo, &["rev-parse", "--verify", &branch]);

    std::fs::remove_file(wt.join(".env")).expect("remove ignored file");
    let discarded = store.discard(&task.task_id).unwrap();
    assert_eq!(discarded.status, TaskStatus::Failed);
    assert!(discarded.worktree_path.is_none());
    assert!(!wt.exists());
    assert!(
        !git_quiet(&repo, &["rev-parse", "--verify", "--quiet", &branch])
            .status
            .success()
    );
}

#[test]
fn ports_are_allocated_in_range_and_reused() {
    let td = TestDir::new("ports");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);

    let t1 = store.create("one", None).unwrap();
    let t2 = store.create("two", None).unwrap();
    let p1 = store.start(&t1.task_id).unwrap().port.expect("port t1");
    let p2 = store.start(&t2.task_id).unwrap().port.expect("port t2");
    assert_port_in_range(p1);
    assert_port_in_range(p2);
    assert_ne!(p1, p2);

    store.cancel(&t1.task_id).unwrap();
    assert!(store.get(&t1.task_id).unwrap().port.is_none());

    let t3 = store.create("three", None).unwrap();
    let p3 = store.start(&t3.task_id).unwrap().port.expect("port t3");
    assert_eq!(p3, p1, "lowest released port should be reused");
    assert_eq!(store.get(&t2.task_id).unwrap().port, Some(p2));
}

#[test]
fn concurrency_limit_defaults_to_two_and_clamps_to_four() {
    let td = TestDir::new("limits");
    let plain = td.path().join("plain");
    std::fs::create_dir_all(&plain).expect("create plain dir");

    let mut store = TaskStore::new(td.data().join("tasks.json"), &plain);
    assert_eq!(store.max_running(), DEFAULT_MAX_RUNNING);
    assert_eq!(DEFAULT_MAX_RUNNING, 2);

    store.set_max_running(99);
    assert_eq!(store.max_running(), HARD_MAX_RUNNING);
    assert_eq!(HARD_MAX_RUNNING, 4);

    store.set_max_running(0);
    assert_eq!(store.max_running(), 1);

    let clamped = TaskStore::with_max_running(td.data().join("other.json"), &plain, 99);
    assert_eq!(clamped.max_running(), HARD_MAX_RUNNING);
}

#[test]
fn invalid_state_transitions_are_rejected() {
    let td = TestDir::new("transitions");
    let plain = td.path().join("plain");
    std::fs::create_dir_all(&plain).expect("create plain dir");
    let mut store = td.store(&plain);

    let task = store.create("transitions", None).unwrap();
    assert!(matches!(
        store.apply(&task.task_id).unwrap_err(),
        TaskError::InvalidState { .. }
    ));
    assert_eq!(
        store.start(&task.task_id).unwrap().status,
        TaskStatus::Running
    );
    assert!(matches!(
        store.start(&task.task_id).unwrap_err(),
        TaskError::InvalidState { .. }
    ));

    assert_eq!(store.apply(&task.task_id).unwrap().commit, "");
    assert_eq!(store.get(&task.task_id).unwrap().status, TaskStatus::Done);
    assert!(matches!(
        store.apply(&task.task_id).unwrap_err(),
        TaskError::InvalidState { .. }
    ));
    assert!(matches!(
        store.start(&task.task_id).unwrap_err(),
        TaskError::InvalidState { .. }
    ));
    assert!(matches!(
        store.cancel(&task.task_id).unwrap_err(),
        TaskError::InvalidState { .. }
    ));
    assert!(matches!(
        store.discard(&task.task_id).unwrap_err(),
        TaskError::InvalidState { .. }
    ));

    let canceled = store.create("cancel me", None).unwrap();
    store.start(&canceled.task_id).unwrap();
    assert_eq!(
        store.cancel(&canceled.task_id).unwrap().status,
        TaskStatus::Failed
    );
    assert!(matches!(
        store.start(&canceled.task_id).unwrap_err(),
        TaskError::InvalidState { .. }
    ));
}

#[test]
fn bind_session_persists_and_can_unbind() {
    let td = TestDir::new("bind");
    let plain = td.path().join("plain");
    std::fs::create_dir_all(&plain).expect("create plain dir");
    let persist = td.data().join("tasks.json");
    let mut store = TaskStore::new(&persist, &plain);

    let task = store.create("bind", None).unwrap();
    let bound = store
        .bind_session(&task.task_id, Some("session-42".into()))
        .unwrap();
    assert_eq!(bound.session_id.as_deref(), Some("session-42"));
    drop(store);

    let mut store = TaskStore::new(&persist, &plain);
    assert_eq!(
        store.get(&task.task_id).unwrap().session_id.as_deref(),
        Some("session-42")
    );
    let unbound = store.bind_session(&task.task_id, None).unwrap();
    assert!(unbound.session_id.is_none());
}

#[test]
fn task_for_session_finds_bound_task() {
    let td = TestDir::new("by-session");
    let plain = td.path().join("plain");
    std::fs::create_dir_all(&plain).expect("create plain dir");
    let mut store = td.store(&plain);

    let first = store.create("first", Some("session-1".into())).unwrap();
    let second = store.create("second", Some("session-2".into())).unwrap();

    assert_eq!(
        store.task_for_session("session-1").unwrap().task_id,
        first.task_id
    );
    assert_eq!(
        store.task_for_session("session-2").unwrap().task_id,
        second.task_id
    );

    // 解绑后不再命中；未绑定的第三个任务也不会串到旧会话。
    store.bind_session(&first.task_id, None).unwrap();
    assert!(store.task_for_session("session-1").is_none());
    let unbound = store.create("unbound", None).unwrap();
    assert!(unbound.session_id.is_none());
    assert_eq!(
        store.task_for_session("session-2").unwrap().task_id,
        second.task_id
    );
}

#[test]
fn task_for_session_missing_returns_none() {
    let td = TestDir::new("by-session-miss");
    let plain = td.path().join("plain");
    std::fs::create_dir_all(&plain).expect("create plain dir");
    let mut store = td.store(&plain);
    store.create("no session", None).unwrap();

    assert!(store.task_for_session("session-ghost").is_none());
}

#[test]
fn set_status_allows_explicit_runtime_transitions() {
    let td = TestDir::new("set-status-cycle");
    let repo = init_repo(&td.repo());
    let mut store = td.store(&repo);
    let task = store.create("cycle", None).unwrap();
    let started = store.start(&task.task_id).unwrap();
    let port = started.port.expect("port after start");
    let worktree = started.worktree_path.clone().expect("worktree");

    let blocked = store
        .set_status(&task.task_id, TaskStatus::Blocked)
        .unwrap();
    assert_eq!(blocked.status, TaskStatus::Blocked);
    // Blocked 不释放端口/worktree：审批结束后同一任务继续。
    assert_eq!(blocked.port, Some(port));
    assert_eq!(blocked.worktree_path.as_deref(), Some(worktree.as_str()));

    for expected in [
        TaskStatus::Running,
        TaskStatus::Review,
        TaskStatus::Failed,
        TaskStatus::Blocked,
        TaskStatus::Running,
    ] {
        let updated = store.set_status(&task.task_id, expected).unwrap();
        assert_eq!(updated.status, expected);
        assert_eq!(updated.port, Some(port));
        assert_eq!(updated.worktree_path.as_deref(), Some(worktree.as_str()));
    }

    // 幂等设置同一状态不报错，也不改变其它字段。
    let same = store
        .set_status(&task.task_id, TaskStatus::Running)
        .unwrap();
    assert_eq!(same.status, TaskStatus::Running);
    assert_eq!(store.get(&task.task_id).unwrap().port, Some(port));
}

#[test]
fn set_status_rejects_done_and_unstarted_tasks() {
    let td = TestDir::new("set-status-reject");
    let plain = td.path().join("plain");
    std::fs::create_dir_all(&plain).expect("create plain dir");
    let mut store = td.store(&plain);

    let planning = store.create("planning", None).unwrap();
    let error = store
        .set_status(&planning.task_id, TaskStatus::Blocked)
        .unwrap_err();
    assert!(matches!(
        error,
        TaskError::InvalidState {
            status: TaskStatus::Planning,
            ..
        }
    ));
    assert!(matches!(
        store
            .set_status(&planning.task_id, TaskStatus::Running)
            .unwrap_err(),
        TaskError::InvalidState { .. }
    ));

    store.start(&planning.task_id).unwrap();
    assert_eq!(store.apply(&planning.task_id).unwrap().commit, "");
    let after_done = store.get(&planning.task_id).unwrap();
    assert_eq!(after_done.status, TaskStatus::Done);

    for target in [
        TaskStatus::Running,
        TaskStatus::Blocked,
        TaskStatus::Review,
        TaskStatus::Failed,
    ] {
        assert!(
            matches!(
                store.set_status(&planning.task_id, target).unwrap_err(),
                TaskError::InvalidState {
                    status: TaskStatus::Done,
                    ..
                }
            ),
            "Done must not transition to {target:?}"
        );
    }
    assert_eq!(
        store.get(&planning.task_id).unwrap().status,
        TaskStatus::Done
    );

    let ghost = store
        .set_status("task-ghost", TaskStatus::Running)
        .unwrap_err();
    assert!(matches!(ghost, TaskError::NotFound { .. }));
}

#[test]
fn set_status_persists_across_reload() {
    let td = TestDir::new("set-status-persist");
    let repo = init_repo(&td.repo());
    let persist = td.data().join("tasks.json");
    let mut store = TaskStore::new(&persist, &repo);

    let task = store.create("persist status", Some("s-1".into())).unwrap();
    let started = store.start(&task.task_id).unwrap();
    let port = started.port.expect("port");
    let worktree = started.worktree_path.clone().expect("worktree");

    let blocked = store
        .set_status(&task.task_id, TaskStatus::Blocked)
        .unwrap();
    assert_eq!(blocked.status, TaskStatus::Blocked);
    drop(store);

    let reloaded = TaskStore::new(&persist, &repo);
    let after = reloaded.get(&task.task_id).unwrap();
    assert_eq!(after.status, TaskStatus::Blocked);
    assert_eq!(after.port, Some(port));
    assert_eq!(after.worktree_path.as_deref(), Some(worktree.as_str()));
    assert_eq!(after.session_id.as_deref(), Some("s-1"));
}

#[test]
fn cancel_keeps_worktree_and_releases_port() {
    let td = TestDir::new("cancel");
    let repo = init_repo(&td.repo());
    let persist = td.data().join("tasks.json");
    let mut store = TaskStore::new(&persist, &repo);
    let task = store.create("cancel", None).unwrap();
    let started = store.start(&task.task_id).unwrap();
    assert!(started.port.is_some());
    let wt = worktree_path(&task);
    let branch = task.branch.clone().expect("branch");

    let canceled = store.cancel(&task.task_id).unwrap();
    assert_eq!(canceled.status, TaskStatus::Failed);
    assert!(canceled.port.is_none());
    assert!(wt.is_dir());
    git(&repo, &["rev-parse", "--verify", &branch]);
    drop(store);

    let reloaded = TaskStore::new(&persist, &repo);
    let after = reloaded.get(&task.task_id).unwrap();
    assert_eq!(after.status, TaskStatus::Failed);
    assert_eq!(
        after.worktree_path.as_deref(),
        task.worktree_path.as_deref()
    );
    assert!(after.port.is_none());
}
