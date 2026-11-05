//! P5 Git 面板命令层（docs/PLAN.md P4 周边）。
//!
//! 薄壳只做三件事：从 [`WorkspaceState`] 取主工作区根、把参数转交
//! `lc_core::git::GitService`、把 `GitError` 映射成契约 `ErrorBody`。
//! 每次命令构造一个 service，不引入全局 Git 状态；根不存在返回 `NotFound`。
//!
//! 所有 git 操作都在 `spawn_blocking` 中执行：status/diff/log 在大仓库或
//! 网络文件系统上可能明显慢于 UI 帧预算，不能占用 Tauri 主线程。
//! 本层不发射 `git:*` 事件（契约没有该事件），UI 命令返回后主动刷新。

use std::path::{Path, PathBuf};

use lc_core::git::GitService;
use lc_proto::{CommandResult, ErrorBody, ErrorCode};

use crate::WorkspaceState;

/// 工作区根：未设置或已被删除都按 `NotFound` 处理（与 fs/task 命令一致）。
fn workspace_root(workspace: &WorkspaceState) -> Result<PathBuf, ErrorBody> {
    let root = workspace.get().ok_or_else(crate::workspace_unavailable)?;
    if !root.is_dir() {
        return Err(crate::workspace_unavailable());
    }
    Ok(root)
}

/// 在阻塞线程池执行 git 操作；join 失败说明 worker panic，归入 `Internal`。
async fn run_blocking<T, F>(action: F) -> Result<T, ErrorBody>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ErrorBody> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(action)
        .await
        .map_err(|error| {
            ErrorBody::new(ErrorCode::Internal, format!("git worker failed: {error}"))
        })?
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn git_status(
    workspace: tauri::State<'_, WorkspaceState>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    run_blocking(move || status_at(&root)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn git_diff(
    path: Option<String>,
    staged: bool,
    workspace: tauri::State<'_, WorkspaceState>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    run_blocking(move || diff_at(&root, path, staged)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn git_stage(
    paths: Vec<String>,
    workspace: tauri::State<'_, WorkspaceState>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    run_blocking(move || stage_at(&root, paths)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn git_unstage(
    paths: Vec<String>,
    workspace: tauri::State<'_, WorkspaceState>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    run_blocking(move || unstage_at(&root, paths)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn git_commit(
    message: String,
    workspace: tauri::State<'_, WorkspaceState>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    run_blocking(move || commit_at(&root, message)).await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn git_log(
    limit: Option<u32>,
    workspace: tauri::State<'_, WorkspaceState>,
) -> Result<CommandResult, ErrorBody> {
    let root = workspace_root(&workspace)?;
    run_blocking(move || log_at(&root, limit)).await
}

// ==== 纯映射层（单测直接覆盖，不启动 Tauri 运行时） ====

fn status_at(root: &Path) -> Result<CommandResult, ErrorBody> {
    let status = GitService::new(root)
        .status()
        .map_err(|error| error.to_error_body())?;
    Ok(CommandResult::GitStatus { status })
}

fn diff_at(root: &Path, path: Option<String>, staged: bool) -> Result<CommandResult, ErrorBody> {
    let diff = GitService::new(root)
        .diff(path.as_deref(), staged)
        .map_err(|error| error.to_error_body())?;
    Ok(CommandResult::GitDiff {
        patch: diff.patch,
        files: diff.files,
    })
}

fn stage_at(root: &Path, paths: Vec<String>) -> Result<CommandResult, ErrorBody> {
    GitService::new(root)
        .stage(&paths)
        .map_err(|error| error.to_error_body())?;
    Ok(CommandResult::Ack)
}

fn unstage_at(root: &Path, paths: Vec<String>) -> Result<CommandResult, ErrorBody> {
    GitService::new(root)
        .unstage(&paths)
        .map_err(|error| error.to_error_body())?;
    Ok(CommandResult::Ack)
}

fn commit_at(root: &Path, message: String) -> Result<CommandResult, ErrorBody> {
    GitService::new(root)
        .commit(&message)
        .map_err(|error| error.to_error_body())?;
    Ok(CommandResult::Ack)
}

fn log_at(root: &Path, limit: Option<u32>) -> Result<CommandResult, ErrorBody> {
    let commits = GitService::new(root)
        .log(limit)
        .map_err(|error| error.to_error_body())?;
    Ok(CommandResult::GitLog { commits })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use lc_proto::{ChangeStatus, CommandResult, ErrorCode};

    use super::{commit_at, diff_at, log_at, stage_at, status_at, unstage_at, workspace_root};
    use crate::WorkspaceState;

    /// 进程内唯一临时目录（不引入 tempfile，与 lc-core 测试同策略）。
    fn temp_root(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let seq = N.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "lc-desktop-git-{}-{nanos}-{tag}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        root
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("LC_ALL", "C")
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(tag: &str) -> PathBuf {
        let root = temp_root(tag);
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "desktop-git@example.com"]);
        git(&root, &["config", "user.name", "Desktop Git"]);
        git(&root, &["config", "core.autocrlf", "false"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("README.md"), "hello\n").expect("write README");
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "init"]);
        root
    }

    fn kind(result: &CommandResult) -> String {
        serde_json::to_value(result).expect("serialize result")["kind"]
            .as_str()
            .expect("kind string")
            .to_string()
    }

    #[test]
    fn workspace_root_maps_missing_directory_to_not_found() {
        let root = temp_root("root-missing").join("ghost");
        let state = WorkspaceState::new();
        state.set(root);
        let error = workspace_root(&state).expect_err("missing root");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn status_command_maps_to_contract_result() {
        let root = init_repo("status");
        std::fs::write(root.join("README.md"), "changed\n").expect("write README");

        let result = status_at(&root).expect("status");
        assert_eq!(kind(&result), "git_status");
        let CommandResult::GitStatus { status } = result else {
            panic!("expected git_status");
        };
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(!status.clean);
        let file = status
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .expect("README status");
        assert_eq!(file.status, ChangeStatus::Modified);
    }

    #[test]
    fn diff_command_maps_patch_and_files() {
        let root = init_repo("diff");
        std::fs::write(root.join("README.md"), "changed\n").expect("write README");

        let result = diff_at(&root, None, false).expect("diff");
        assert_eq!(kind(&result), "git_diff");
        let CommandResult::GitDiff { patch, files } = result else {
            panic!("expected git_diff");
        };
        assert!(patch.contains("+changed"));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "README.md");
        assert!(!files[0].staged);
    }

    #[test]
    fn stage_unstage_and_commit_commands_map_to_ack() {
        let root = init_repo("stage");
        std::fs::write(root.join("README.md"), "committed\n").expect("write README");

        assert!(matches!(
            stage_at(&root, vec!["README.md".into()]).expect("stage"),
            CommandResult::Ack
        ));
        let staged = status_at(&root).expect("status");
        let CommandResult::GitStatus { status } = staged else {
            panic!("expected git_status");
        };
        assert!(status.files.iter().any(|file| file.staged));

        assert!(matches!(
            unstage_at(&root, vec!["README.md".into()]).expect("unstage"),
            CommandResult::Ack
        ));
        assert!(matches!(
            stage_at(&root, vec!["README.md".into()]).expect("stage again"),
            CommandResult::Ack
        ));
        assert!(matches!(
            commit_at(&root, "feat: desktop command".into()).expect("commit"),
            CommandResult::Ack
        ));

        let status = status_at(&root).expect("status after commit");
        let CommandResult::GitStatus { status } = status else {
            panic!("expected git_status");
        };
        assert!(status.clean);
    }

    #[test]
    fn log_command_maps_commits_and_clamps_limit() {
        let root = init_repo("log");
        let result = log_at(&root, Some(0)).expect("log");
        assert_eq!(kind(&result), "git_log");
        let CommandResult::GitLog { commits } = result else {
            panic!("expected git_log");
        };
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "init");
        assert_eq!(commits[0].author, "Desktop Git");
        assert!(!commits[0].short_sha.is_empty());
        assert!(commits[0].timestamp > 0);
    }

    #[test]
    fn git_errors_map_to_contract_codes() {
        let root = temp_root("not-repo");
        let error = status_at(&root).expect_err("non-repo");
        assert_eq!(error.code, ErrorCode::GitUnavailable);

        let repo = init_repo("reject");
        let error = stage_at(&repo, Vec::new()).expect_err("empty paths");
        assert_eq!(error.code, ErrorCode::GitRejected);
    }
}
