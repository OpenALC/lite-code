//! git worktree 生命周期（docs/TASKS.md §3、§5）。
//!
//! 约定：
//! - worktree 固定放 `<data>/worktrees/<project-key>/<task-id>`，并在计算后
//!   再次校验不落在项目目录内（数据目录被配置到项目里时退到系统临时目录）；
//! - 分支名 `lc/<task-id>-<slug>`；
//! - 主工作区 dirty 时不合并、不自动 stash；冲突时不 abort，保留 merge 状态
//!   供用户在编辑器里解决；
//! - 删除 worktree 前由调用方确认干净（`discard` 的 DirtyWorktree 拒绝）。

use std::path::{Path, PathBuf};

use super::{
    GIT_TIMEOUT, MERGE_TIMEOUT, TaskError, git_failure, git_ok, git_toplevel, path_inside,
    project_key, run_git, slugify, tail,
};

/// 新任务 worktree 的创建结果。失败不是致命错误：`path/branch` 为 `None`
/// 时任务降级为串行（主工作区执行），`notes` 记录原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeCreate {
    pub path: Option<PathBuf>,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub notes: Vec<String>,
}

/// 成功合并后的返回信息。`base_drift` 是 `base_sha` 之后主分支前进的提交数
/// （不阻止合并，供 UI 提示“这个任务基于 abc123，主分支已前进 N 个提交”）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub commit: String,
    pub base_drift: u32,
    pub warnings: Vec<String>,
}

/// 创建 worktree；任何一步失败都返回带 notes 的降级结果，不向调用方抛错。
pub(crate) fn create(
    repo_root: &Path,
    data_root: &Path,
    task_id: &str,
    title: &str,
) -> WorktreeCreate {
    let mut notes = Vec::new();
    let Some(git_root) = git_toplevel(repo_root) else {
        notes.push(
            "not a git repository (or git unavailable): task runs serially in the main workspace"
                .into(),
        );
        return WorktreeCreate {
            path: None,
            branch: None,
            base_sha: None,
            notes,
        };
    };

    let base_sha = match run_git(&git_root, &["rev-parse", "HEAD"], GIT_TIMEOUT) {
        Ok(out) if out.success() => {
            let sha = out.stdout.trim().to_string();
            if sha.is_empty() {
                notes.push("git rev-parse HEAD returned an empty commit".into());
                return WorktreeCreate {
                    path: None,
                    branch: None,
                    base_sha: None,
                    notes,
                };
            }
            sha
        }
        Ok(out) => {
            notes.push(format!(
                "repository has no usable HEAD: {}",
                tail(&out.stderr, 500)
            ));
            return WorktreeCreate {
                path: None,
                branch: None,
                base_sha: None,
                notes,
            };
        }
        Err(error) => {
            notes.push(format!("cannot read HEAD: {error}"));
            return WorktreeCreate {
                path: None,
                branch: None,
                base_sha: None,
                notes,
            };
        }
    };

    let branch = format!("lc/{task_id}-{}", slugify(title));
    let key = project_key(&git_root);
    let mut worktree = data_root.join("worktrees").join(&key).join(task_id);

    // 硬规则：worktree 不能落在项目目录内（否则 file watcher / git status 会一直
    // 看到它）。正常数据目录本来就在项目外，这里是防御性回退。
    if path_inside(&worktree, &git_root) || path_inside(&worktree, repo_root) {
        notes.push(
            "data directory is inside the project; worktree relocated to the system temp dir"
                .into(),
        );
        worktree = std::env::temp_dir()
            .join("lite-code-worktrees")
            .join(&key)
            .join(task_id);
        if path_inside(&worktree, &git_root) {
            notes.push("cannot place worktree outside the project; running serially".into());
            return WorktreeCreate {
                path: None,
                branch: None,
                base_sha: Some(base_sha),
                notes,
            };
        }
    }

    if let Some(parent) = worktree.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        notes.push(format!(
            "cannot create worktree directory {}: {error}",
            parent.display()
        ));
        return WorktreeCreate {
            path: None,
            branch: None,
            base_sha: Some(base_sha),
            notes,
        };
    }

    let path_arg = worktree.to_string_lossy().into_owned();
    match run_git(
        &git_root,
        &["worktree", "add", "-b", &branch, &path_arg, "HEAD"],
        GIT_TIMEOUT,
    ) {
        Ok(out) if out.success() => WorktreeCreate {
            path: Some(worktree),
            branch: Some(branch),
            base_sha: Some(base_sha),
            notes,
        },
        Ok(out) => {
            notes.push(format!(
                "git worktree add failed; running serially: {}",
                tail(&format!("{}\n{}", out.stdout, out.stderr), 800)
            ));
            WorktreeCreate {
                path: None,
                branch: None,
                base_sha: Some(base_sha),
                notes,
            }
        }
        Err(error) => {
            notes.push(format!("git worktree add failed: {error}"));
            WorktreeCreate {
                path: None,
                branch: None,
                base_sha: Some(base_sha),
                notes,
            }
        }
    }
}

/// 主工作区必须 clean（含未跟踪文件）才允许合并（docs/TASKS.md §5 第 1 步）。
pub(crate) fn ensure_main_clean(repo_root: &Path) -> Result<(), TaskError> {
    if git_toplevel(repo_root).is_none() {
        return Err(TaskError::NotGit {
            root: repo_root.display().to_string(),
        });
    }
    let out = git_ok(repo_root, &["status", "--porcelain"], GIT_TIMEOUT)?;
    let detail = out.stdout.trim();
    if detail.is_empty() {
        Ok(())
    } else {
        Err(TaskError::DirtyMainWorktree {
            detail: tail(detail, 1_500),
        })
    }
}

/// task.base_sha 之后主分支前进的提交数。
pub(crate) fn base_drift(repo_root: &Path, base_sha: &str) -> Result<u32, TaskError> {
    let range = format!("{base_sha}..HEAD");
    let out = git_ok(repo_root, &["rev-list", "--count", &range], GIT_TIMEOUT)?;
    out.stdout
        .trim()
        .parse::<u32>()
        .map_err(|error| TaskError::Io {
            path: repo_root.display().to_string(),
            message: format!("cannot parse rev-list --count output: {error}"),
        })
}

/// worktree 是否有未提交改动（含未跟踪文件）。
pub(crate) fn worktree_is_dirty(worktree: &Path) -> Result<bool, TaskError> {
    let out = git_ok(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
        GIT_TIMEOUT,
    )?;
    Ok(!out.stdout.trim().is_empty())
}

/// 把 worktree 中尚未提交的改动提交到任务分支，保证 merge 不会丢改动。
/// 返回是否真的产生了提交。
///
/// 机器人身份作为兜底，避免用户没配置 `user.name/email` 时 apply 半途失败；
/// 用户已配置的身份优先（这里用 `-c` 覆盖，注释说明这是 apply 的自动提交）。
pub(crate) fn auto_commit_worktree(worktree: &Path, task_id: &str) -> Result<bool, TaskError> {
    if !worktree_is_dirty(worktree)? {
        return Ok(false);
    }
    git_ok(worktree, &["add", "-A"], GIT_TIMEOUT)?;
    let message = format!("lc: apply {task_id}");
    let args = [
        "-c",
        "user.name=Lite Code",
        "-c",
        "user.email=lc@lite-code.local",
        "commit",
        "--no-verify",
        "-m",
        message.as_str(),
    ];
    let out = run_git(worktree, &args, GIT_TIMEOUT)?;
    if out.success() {
        return Ok(true);
    }
    let combined = format!("{}\n{}", out.stdout, out.stderr);
    if combined.contains("nothing to commit") || combined.contains("no changes added") {
        Ok(false)
    } else {
        Err(git_failure(&args[4..], &out))
    }
}

/// `git merge --no-ff`。冲突时**不** abort/解决，保留 `MERGE_HEAD` 与冲突
/// 标记，返回 [`TaskError::MergeConflict`]（调用方负责把任务置为 Review）。
pub(crate) fn merge_branch(repo_root: &Path, branch: &str, task_id: &str) -> Result<(), TaskError> {
    let out = run_git(
        repo_root,
        &["merge", "--no-ff", "--no-edit", branch],
        MERGE_TIMEOUT,
    )?;
    if out.success() {
        return Ok(());
    }

    let merge_head = run_git(
        repo_root,
        &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
        GIT_TIMEOUT,
    );
    let in_merge = merge_head.map(|out| out.success()).unwrap_or(false);
    if in_merge {
        let detail = format!("{}\n{}", out.stdout.trim(), out.stderr.trim());
        Err(TaskError::MergeConflict {
            task_id: task_id.to_string(),
            detail: tail(&detail, 1_800),
        })
    } else {
        Err(git_failure(&["merge", "--no-ff", branch], &out))
    }
}

/// 合并后的 HEAD（即合并提交）。
pub(crate) fn head_sha(repo_root: &Path) -> Result<String, TaskError> {
    let out = git_ok(repo_root, &["rev-parse", "HEAD"], GIT_TIMEOUT)?;
    let sha = out.stdout.trim();
    if sha.is_empty() {
        Err(TaskError::Io {
            path: repo_root.display().to_string(),
            message: "git rev-parse HEAD returned an empty commit".into(),
        })
    } else {
        Ok(sha.to_string())
    }
}

/// 尽力删除 worktree 与分支；失败不致命，返回 warnings（docs/TASKS.md §3）。
pub(crate) fn remove(
    repo_root: &Path,
    worktree: Option<&Path>,
    branch: Option<&str>,
) -> Vec<String> {
    let mut warnings = Vec::new();

    if let Some(worktree) = worktree {
        let path = worktree.to_string_lossy().into_owned();
        match run_git(
            repo_root,
            &["worktree", "remove", "--force", &path],
            GIT_TIMEOUT,
        ) {
            Ok(out) if out.success() => {}
            Ok(out) => warnings.push(format!(
                "git worktree remove failed for {path}: {}",
                tail(&format!("{}\n{}", out.stdout, out.stderr), 500)
            )),
            Err(error) => warnings.push(format!("git worktree remove failed for {path}: {error}")),
        }
    }

    if let Some(branch) = branch {
        match run_git(repo_root, &["branch", "-D", branch], GIT_TIMEOUT) {
            Ok(out) if out.success() => {}
            Ok(out) => warnings.push(format!(
                "git branch -D {branch} failed: {}",
                tail(&format!("{}\n{}", out.stdout, out.stderr), 500)
            )),
            Err(error) => warnings.push(format!("git branch -D {branch} failed: {error}")),
        }
    }

    warnings
}
