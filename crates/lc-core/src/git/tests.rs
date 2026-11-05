//! Git lane 集成测试：真实临时仓库 + 系统 git。
//!
//! 每个用例独立目录，不共享仓库状态；不引入 tempfile 等测试依赖。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lc_proto::{ChangeStatus, ErrorCode, GitFileStatus};

use super::{DEFAULT_LOG_LIMIT, GitService, MAX_LOG_LIMIT, normalize_limit};

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
            "lc-git-test-{}-{nanos}-{tag}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_quiet(dir, args);
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
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .output()
        .expect("spawn git")
}

fn git_raw(args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn configure_identity(dir: &Path) {
    git(dir, &["config", "user.email", "git-test@example.com"]);
    git(dir, &["config", "user.name", "Git Test"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn init_empty_repo(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).expect("create repo dir");
    git(root, &["init", "-q", "-b", "main"]);
    configure_identity(root);
    root.to_path_buf()
}

fn init_repo(root: &Path) -> PathBuf {
    init_empty_repo(root);
    write(root, "README.md", "hello\n");
    commit_all(root, "init");
    root.to_path_buf()
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn write(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).expect("write test file");
}

fn find_file<'a>(files: &'a [GitFileStatus], path: &str, staged: bool) -> &'a GitFileStatus {
    files
        .iter()
        .find(|file| file.path == path && file.staged == staged)
        .unwrap_or_else(|| panic!("missing {path:?} staged={staged}: {files:?}"))
}

#[test]
fn non_repository_maps_to_git_unavailable() {
    let dir = TestDir::new("non-repo");
    let service = GitService::new(dir.path());
    let error = service.status().expect_err("non-repo must fail");
    assert_eq!(error.code(), ErrorCode::GitUnavailable);
    assert!(error.to_string().contains("git unavailable"));
    assert_eq!(error.to_error_body().code, ErrorCode::GitUnavailable);
}

#[test]
fn empty_repository_status_has_branch_but_no_files() {
    let dir = TestDir::new("empty");
    let repo = init_empty_repo(&dir.path().join("repo"));
    let status = GitService::new(&repo).status().expect("status");

    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.upstream, None);
    assert_eq!((status.ahead, status.behind), (0, 0));
    assert!(status.files.is_empty());
    assert!(status.clean);
    assert!(!status.merge_in_progress);
    assert!(!status.rebase_in_progress);
}

#[test]
fn status_reports_modified_untracked_and_deleted() {
    let dir = TestDir::new("changes");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "keep.txt", "keep\n");
    commit_all(&repo, "add keep");

    write(&repo, "README.md", "changed\n");
    std::fs::remove_file(repo.join("keep.txt")).expect("delete keep");
    // 没有结尾换行：仍然算 3 行。
    write(&repo, "notes.txt", "a\nb\nc");

    let status = GitService::new(&repo).status().expect("status");
    assert!(!status.clean);

    let readme = find_file(&status.files, "README.md", false);
    assert_eq!(readme.status, ChangeStatus::Modified);
    assert_eq!((readme.added, readme.removed), (1, 1));

    let keep = find_file(&status.files, "keep.txt", false);
    assert_eq!(keep.status, ChangeStatus::Deleted);
    assert_eq!((keep.added, keep.removed), (0, 1));

    let notes = find_file(&status.files, "notes.txt", false);
    assert_eq!(notes.status, ChangeStatus::Added);
    assert_eq!((notes.added, notes.removed), (3, 0));
}

#[test]
fn status_splits_staged_and_unstaged_for_same_path() {
    let dir = TestDir::new("split");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "README.md", "one\nTWO\n");
    git(&repo, &["add", "README.md"]);
    write(&repo, "README.md", "one\nTWO\nthree\n");

    let status = GitService::new(&repo).status().expect("status");
    let readme: Vec<&GitFileStatus> = status
        .files
        .iter()
        .filter(|file| file.path == "README.md")
        .collect();
    assert_eq!(readme.len(), 2, "expected one record per staged side");

    let staged = find_file(&status.files, "README.md", true);
    assert_eq!(staged.status, ChangeStatus::Modified);
    // HEAD 的 "hello\n" → index 的 "one\nTWO\n"：新增 2 行、删除 1 行。
    assert_eq!((staged.added, staged.removed), (2, 1));

    let unstaged = find_file(&status.files, "README.md", false);
    assert_eq!(unstaged.status, ChangeStatus::Modified);
    assert_eq!((unstaged.added, unstaged.removed), (1, 0));
}

#[test]
fn status_reports_staged_rename_with_new_path() {
    let dir = TestDir::new("rename");
    let repo = init_repo(&dir.path().join("repo"));
    git(&repo, &["mv", "README.md", "GUIDE.md"]);

    let status = GitService::new(&repo).status().expect("status");
    let renamed = find_file(&status.files, "GUIDE.md", true);
    assert_eq!(renamed.status, ChangeStatus::Renamed);
    assert!(!status.files.iter().any(|file| file.path == "README.md"));
}

#[test]
fn status_tracks_ahead_and_behind_against_upstream() {
    let dir = TestDir::new("sync");
    let repo = init_repo(&dir.path().join("repo"));
    let remote = dir.path().join("remote.git");
    let clone = dir.path().join("clone");

    git_raw(&[
        "init",
        "-q",
        "--bare",
        "-b",
        "main",
        remote.to_str().expect("utf-8 temp path"),
    ]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            remote.to_str().expect("utf-8 temp path"),
        ],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);

    git_raw(&[
        "clone",
        "-q",
        "-b",
        "main",
        remote.to_str().expect("utf-8 temp path"),
        clone.to_str().expect("utf-8 temp path"),
    ]);
    configure_identity(&clone);
    write(&clone, "README.md", "remote\n");
    commit_all(&clone, "remote change");
    git(&clone, &["push", "-q"]);

    git(&repo, &["fetch", "-q", "origin"]);
    write(&repo, "README.md", "local\n");
    commit_all(&repo, "local change");

    let status = GitService::new(&repo).status().expect("status");
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!((status.ahead, status.behind), (1, 1));
}

#[test]
fn diff_shows_unstaged_and_staged_views() {
    let dir = TestDir::new("diff");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "README.md", "one\ntwo\n");
    let service = GitService::new(&repo);

    let unstaged = service.diff(None, false).expect("unstaged diff");
    assert!(unstaged.patch.contains("-hello"));
    assert!(unstaged.patch.contains("+two"));
    let file = find_file(&unstaged.files, "README.md", false);
    assert_eq!(file.status, ChangeStatus::Modified);
    assert_eq!((file.added, file.removed), (2, 1));

    let before_stage = service.diff(None, true).expect("staged diff");
    assert!(before_stage.patch.is_empty());
    assert!(before_stage.files.is_empty());

    service
        .stage(&["README.md".to_string()])
        .expect("stage README");
    let staged = service.diff(None, true).expect("staged diff");
    assert!(staged.patch.contains("+two"));
    assert!(find_file(&staged.files, "README.md", true).staged);

    let after_stage = service.diff(None, false).expect("worktree now clean");
    assert!(after_stage.patch.is_empty());
    assert!(after_stage.files.is_empty());
}

#[test]
fn diff_path_filter_limits_patch_and_files() {
    let dir = TestDir::new("diff-path");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "a.txt", "A\n");
    write(&repo, "b.txt", "B\n");
    commit_all(&repo, "add files");
    write(&repo, "a.txt", "A-CHANGE\n");
    write(&repo, "b.txt", "B-CHANGE\n");

    let diff = GitService::new(&repo)
        .diff(Some("a.txt"), false)
        .expect("scoped diff");
    assert!(diff.patch.contains("A-CHANGE"));
    assert!(!diff.patch.contains("B-CHANGE"));
    assert_eq!(diff.files.len(), 1);
    assert_eq!(diff.files[0].path, "a.txt");
}

#[test]
fn diff_includes_untracked_files_and_skips_other_paths() {
    let dir = TestDir::new("diff-untracked");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "new.txt", "x\ny\n");
    let service = GitService::new(&repo);

    let diff = service.diff(None, false).expect("diff");
    assert!(diff.patch.contains("+x"));
    assert!(diff.patch.contains("+y"));
    let file = find_file(&diff.files, "new.txt", false);
    assert_eq!(file.status, ChangeStatus::Added);
    assert_eq!((file.added, file.removed), (2, 0));

    let scoped = service.diff(Some("new.txt"), false).expect("scoped diff");
    assert!(scoped.patch.contains("+x"));
    assert_eq!(scoped.files.len(), 1);

    let missing = service
        .diff(Some("does-not-exist.txt"), false)
        .expect("missing path");
    assert!(missing.patch.is_empty());
    assert!(missing.files.is_empty());
}

#[test]
fn stage_and_unstage_move_changes_between_sides() {
    let dir = TestDir::new("stage");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "README.md", "staged\n");
    let service = GitService::new(&repo);

    service
        .stage(&["README.md".to_string()])
        .expect("stage README");
    let staged = service.status().expect("status");
    assert!(find_file(&staged.files, "README.md", true).staged);
    assert!(
        git_quiet(&repo, &["diff", "--cached", "--quiet"])
            .status
            .code()
            == Some(1)
    );

    service
        .unstage(&["README.md".to_string()])
        .expect("unstage README");
    let unstaged = service.status().expect("status");
    assert!(!find_file(&unstaged.files, "README.md", false).staged);
    assert!(
        git_quiet(&repo, &["diff", "--cached", "--quiet"])
            .status
            .success()
    );
}

#[test]
fn stage_and_unstage_reject_empty_paths() {
    let dir = TestDir::new("empty-paths");
    let repo = init_repo(&dir.path().join("repo"));
    let service = GitService::new(&repo);

    assert_eq!(
        service.stage(&[]).expect_err("empty stage").code(),
        ErrorCode::GitRejected
    );
    assert_eq!(
        service.unstage(&[]).expect_err("empty unstage").code(),
        ErrorCode::GitRejected
    );
    assert_eq!(
        service
            .stage(&[String::new()])
            .expect_err("empty path")
            .code(),
        ErrorCode::GitRejected
    );
}

#[test]
fn commit_rejects_empty_message_and_missing_staged_changes() {
    let dir = TestDir::new("commit-reject");
    let repo = init_repo(&dir.path().join("repo"));
    let service = GitService::new(&repo);

    let empty = service.commit("   ").expect_err("empty message");
    assert_eq!(empty.code(), ErrorCode::GitRejected);
    assert!(empty.to_string().contains("message"));

    let none = service
        .commit("valid message")
        .expect_err("no staged changes");
    assert_eq!(none.code(), ErrorCode::GitRejected);
    assert!(none.to_string().contains("no staged changes"));
}

#[test]
fn commit_cleans_worktree_and_log_matches_head() {
    let dir = TestDir::new("commit");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "README.md", "with change\n");
    let service = GitService::new(&repo);
    service
        .stage(&["README.md".to_string()])
        .expect("stage README");
    service.commit("feat: update readme").expect("commit");

    let status = service.status().expect("status");
    assert!(status.clean);
    assert!(status.files.is_empty());

    let head = git(&repo, &["rev-parse", "HEAD"]);
    let commits = service.log(Some(5)).expect("log");
    assert_eq!(commits.len(), 2);
    let latest = &commits[0];
    assert_eq!(latest.sha, head);
    assert!(head.starts_with(&latest.short_sha));
    assert_eq!(latest.author, "Git Test");
    assert_eq!(latest.message, "feat: update readme");
    assert!(latest.timestamp > 0);
}

#[test]
fn log_defaults_and_clamps_limit() {
    assert_eq!(normalize_limit(None), DEFAULT_LOG_LIMIT);
    assert_eq!(normalize_limit(Some(0)), 1);
    assert_eq!(normalize_limit(Some(1000)), MAX_LOG_LIMIT);

    let dir = TestDir::new("log-limit");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "one.txt", "1\n");
    commit_all(&repo, "one");
    write(&repo, "two.txt", "2\n");
    commit_all(&repo, "two");
    let service = GitService::new(&repo);

    assert_eq!(service.log(Some(0)).expect("limit 0").len(), 1);
    assert_eq!(service.log(Some(1000)).expect("limit 1000").len(), 3);
    assert_eq!(service.log(None).expect("default limit").len(), 3);
}

#[test]
fn log_on_empty_repository_is_empty() {
    let dir = TestDir::new("log-empty");
    let repo = init_empty_repo(&dir.path().join("repo"));
    assert!(GitService::new(&repo).log(None).expect("log").is_empty());
}

#[test]
fn merge_conflict_marks_merge_in_progress() {
    let dir = TestDir::new("merge");
    let repo = init_repo(&dir.path().join("repo"));

    git(&repo, &["checkout", "-q", "-b", "feature"]);
    write(&repo, "README.md", "feature\n");
    commit_all(&repo, "feature change");

    git(&repo, &["checkout", "-q", "main"]);
    write(&repo, "README.md", "main\n");
    commit_all(&repo, "main change");

    let merge = git_quiet(&repo, &["merge", "feature"]);
    assert!(!merge.status.success(), "merge should conflict");

    let status = GitService::new(&repo).status().expect("status");
    assert!(status.merge_in_progress);
    assert!(!status.rebase_in_progress);
    assert!(!status.clean);
    let conflicted = find_file(&status.files, "README.md", false);
    assert_eq!(conflicted.status, ChangeStatus::Modified);
}

#[test]
fn rebase_conflict_marks_rebase_in_progress() {
    let dir = TestDir::new("rebase");
    let repo = init_repo(&dir.path().join("repo"));

    git(&repo, &["checkout", "-q", "-b", "feature"]);
    write(&repo, "README.md", "feature\n");
    commit_all(&repo, "feature change");

    git(&repo, &["checkout", "-q", "main"]);
    write(&repo, "README.md", "main\n");
    commit_all(&repo, "main change");

    git(&repo, &["checkout", "-q", "feature"]);
    let rebase = git_quiet(&repo, &["rebase", "main"]);
    assert!(!rebase.status.success(), "rebase should conflict");

    let status = GitService::new(&repo).status().expect("status");
    assert!(status.rebase_in_progress);
    assert!(!status.merge_in_progress);
    assert!(!status.clean);
}

#[test]
fn commit_failure_maps_to_git_rejected() {
    let dir = TestDir::new("commit-failure");
    let repo = init_repo(&dir.path().join("repo"));
    write(&repo, "README.md", "signed\n");
    let service = GitService::new(&repo);
    service
        .stage(&["README.md".to_string()])
        .expect("stage README");

    // 强制签名但指向不存在的 gpg 程序：commit 必然失败，验证错误映射。
    git(&repo, &["config", "commit.gpgsign", "true"]);
    git(&repo, &["config", "gpg.program", "lc-test-missing-gpg"]);
    let error = service
        .commit("signed commit")
        .expect_err("commit must fail");
    assert_eq!(error.code(), ErrorCode::GitRejected);
}
