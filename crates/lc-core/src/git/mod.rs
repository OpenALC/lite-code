//! Git 面板服务（docs/ARCHITECTURE.md §5.3、docs/PLAN.md P4 周边）。
//!
//! 只操作**主工作区**：`GitService { root }` 的所有命令都是
//! `git -C <root> ...`，不经过 shell、不拼接参数字符串，普通操作 10s 超时，
//! commit 30s（允许钩子运行）。任务 worktree 的 diff 由 `lc_core::task` 提供，
//! 本 lane 不依赖它，也不共享其 git 执行器。
//!
//! # status 的 staged 语义
//!
//! [`lc_proto::GitFileStatus`] 没有 old_path，也只有一个 `staged` 布尔，因此
//! [`GitService::status`] 采用**每个（路径，暂存侧）一条记录**：
//!
//! - 同一路径同时有 staged 与 unstaged 改动时返回两条，`staged` 分别为
//!   `true`/`false`，`added`/`removed` 是对应侧（index↔HEAD 或 worktree↔index）
//!   的 `--numstat` 统计；
//! - 未跟踪文件返回一条 `staged = false`、`status = Added`，行数按文件内容统计；
//! - rename/copy 只保留新路径（`status = Renamed` / `Added`），因为 DTO 没有
//!   old_path，且 stage/diff 都按新路径操作；
//! - 冲突（`UU`/`AA`/`DD` 等）合并为一条 `staged = false`、`status = Modified`，
//!   由 `merge_in_progress` 标记提示用户解决。
//!
//! `clean` 表示“没有文件改动且没有进行中的 merge/rebase”。

mod cli;
mod status;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lc_proto::{ChangeStatus, ErrorBody, ErrorCode, GitCommit, GitFileStatus, GitStatus};

use cli::{COMMIT_TIMEOUT, GIT_TIMEOUT, GitOutput, output_detail, rejected, run_git, tail};
use status::{map_status, parse_name_status, parse_numstat, parse_porcelain, untracked_line_count};

/// `git_log` 未给 limit 时的默认条数。
pub const DEFAULT_LOG_LIMIT: u32 = 20;
/// `git_log` 的硬上限，避免 UI 一次拉取过多提交。
pub const MAX_LOG_LIMIT: u32 = 100;

/// `diff` 的结果：补丁与文件统计。Tauri 层直接映射到
/// `CommandResult::GitDiff { patch, files }`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOutput {
    pub patch: String,
    pub files: Vec<GitFileStatus>,
}

/// git lane 的错误集合。`code()` 映射到契约错误码：
/// 非仓库/未安装 → `GitUnavailable`；空 message、无 staged 改动、冲突与钩子
/// 失败 → `GitRejected`；超时 → `Timeout`。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("git unavailable: {message}")]
    Unavailable { message: String },
    #[error("git {operation} rejected: {detail}")]
    Rejected { operation: String, detail: String },
    #[error("git {args} timed out after {timeout_secs}s")]
    Timeout { args: String, timeout_secs: u64 },
    #[error("git io error at {path}: {message}")]
    Io { path: String, message: String },
}

impl GitError {
    pub fn code(&self) -> ErrorCode {
        match self {
            GitError::Unavailable { .. } => ErrorCode::GitUnavailable,
            GitError::Rejected { .. } => ErrorCode::GitRejected,
            GitError::Timeout { .. } => ErrorCode::Timeout,
            GitError::Io { .. } => ErrorCode::Io,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        ErrorBody::new(self.code(), self.to_string())
    }
}

/// 主工作区 Git 服务。无全局状态：Tauri 命令每次按 `WorkspaceState` 的根构造。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitService {
    root: PathBuf,
}

impl GitService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 工作区状态：分支/upstream、ahead/behind、merge/rebase 标记与文件列表。
    /// staged 语义见模块文档。
    pub fn status(&self) -> Result<GitStatus, GitError> {
        self.ensure_repo()?;

        let out = run_git(
            &self.root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            GIT_TIMEOUT,
        )?;
        if !out.success() {
            return Err(rejected("status", &out));
        }
        let entries = parse_porcelain(&out.stdout);

        let staged_stats = self.numstat(true)?;
        let unstaged_stats = self.numstat(false)?;
        let (branch, upstream, ahead, behind) = self.branch_info();
        let (merge_in_progress, rebase_in_progress) = self.operations_in_progress()?;

        let mut files = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.untracked {
                files.push(GitFileStatus {
                    path: entry.path.clone(),
                    status: ChangeStatus::Added,
                    staged: false,
                    added: untracked_line_count(&self.root.join(&entry.path)),
                    removed: 0,
                });
                continue;
            }
            if entry.unmerged {
                // 冲突不虚构 added/removed：index 里是三个 stage，不是普通 diff。
                files.push(GitFileStatus {
                    path: entry.path,
                    status: ChangeStatus::Modified,
                    staged: false,
                    added: 0,
                    removed: 0,
                });
                continue;
            }
            if let Some(code) = entry.index {
                let (added, removed) = staged_stats.get(&entry.path).copied().unwrap_or((0, 0));
                files.push(GitFileStatus {
                    path: entry.path.clone(),
                    status: map_status(code),
                    staged: true,
                    added,
                    removed,
                });
            }
            if let Some(code) = entry.worktree {
                let (added, removed) = unstaged_stats.get(&entry.path).copied().unwrap_or((0, 0));
                files.push(GitFileStatus {
                    path: entry.path,
                    status: map_status(code),
                    staged: false,
                    added,
                    removed,
                });
            }
        }

        let clean = files.is_empty() && !merge_in_progress && !rebase_in_progress;
        Ok(GitStatus {
            branch,
            upstream,
            ahead,
            behind,
            clean,
            merge_in_progress,
            rebase_in_progress,
            files,
        })
    }

    /// `git diff [--cached] [-- <path>]`；未跟踪文件用 `--no-index` 补充，
    /// 单个文件失败时跳过并在 patch 末尾以 `# lc:` 注释说明。
    pub fn diff(&self, path: Option<&str>, staged: bool) -> Result<DiffOutput, GitError> {
        self.ensure_repo()?;
        let path = path.filter(|value| !value.is_empty());

        let mut args = vec!["diff", "--no-color"];
        if staged {
            args.push("--cached");
        }
        if let Some(path) = path {
            args.push("--");
            args.push(path);
        }
        let out = run_git(&self.root, &args, GIT_TIMEOUT)?;
        if !out.success() {
            return Err(rejected("diff", &out));
        }

        let untracked = if staged {
            Vec::new()
        } else {
            self.untracked_paths(path)?
        };

        let mut patch = out.stdout;
        for path in &untracked {
            match self.untracked_diff(path) {
                Ok(chunk) if !chunk.is_empty() => {
                    if !patch.is_empty() && !patch.ends_with('\n') {
                        patch.push('\n');
                    }
                    patch.push_str(&chunk);
                    if !patch.ends_with('\n') {
                        patch.push('\n');
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    if !patch.is_empty() && !patch.ends_with('\n') {
                        patch.push('\n');
                    }
                    patch.push_str(&format!("# lc: skipped untracked {path}: {error}\n"));
                }
            }
        }

        let mut files = self.tracked_diff_files(path, staged)?;
        for path in &untracked {
            files.push(GitFileStatus {
                path: path.clone(),
                status: ChangeStatus::Added,
                staged: false,
                added: untracked_line_count(&self.root.join(path)),
                removed: 0,
            });
        }

        Ok(DiffOutput { patch, files })
    }

    /// `git add -- <paths>`。空列表/空路径直接拒绝，不启动 git。
    pub fn stage(&self, paths: &[String]) -> Result<(), GitError> {
        let paths = validated_paths(paths, "stage")?;
        self.ensure_repo()?;
        let mut args = vec!["add", "--"];
        args.extend(paths);
        let out = run_git(&self.root, &args, GIT_TIMEOUT)?;
        if !out.success() {
            return Err(rejected("stage", &out));
        }
        Ok(())
    }

    /// `git restore --staged -- <paths>`。空列表/空路径直接拒绝。
    pub fn unstage(&self, paths: &[String]) -> Result<(), GitError> {
        let paths = validated_paths(paths, "unstage")?;
        self.ensure_repo()?;
        let mut args = vec!["restore", "--staged", "--"];
        args.extend(paths);
        let out = run_git(&self.root, &args, GIT_TIMEOUT)?;
        if !out.success() {
            return Err(rejected("unstage", &out));
        }
        Ok(())
    }

    /// 提交主工作区已暂存的改动。message 去掉首尾空白后必须非空；
    /// 没有 staged 改动时返回 `GitRejected`，不会创建空提交。
    pub fn commit(&self, message: &str) -> Result<(), GitError> {
        let message = message.trim();
        if message.is_empty() {
            return Err(GitError::Rejected {
                operation: "commit".into(),
                detail: "commit message must not be empty".into(),
            });
        }
        self.ensure_repo()?;

        let staged = run_git(&self.root, &["diff", "--cached", "--quiet"], GIT_TIMEOUT)?;
        match staged.code {
            Some(0) => {
                return Err(GitError::Rejected {
                    operation: "commit".into(),
                    detail: "no staged changes".into(),
                });
            }
            Some(1) => {}
            _ => return Err(rejected("commit", &staged)),
        }

        let out = run_git(&self.root, &["commit", "-m", message], COMMIT_TIMEOUT)?;
        if !out.success() {
            // 冲突、钩子失败、签名失败等都归入 GitRejected，stderr 尾部进 detail。
            return Err(rejected("commit", &out));
        }
        Ok(())
    }

    /// 最近提交；默认 20 条，clamp 到 1..=100。无 HEAD 的空仓库返回空列表。
    pub fn log(&self, limit: Option<u32>) -> Result<Vec<GitCommit>, GitError> {
        self.ensure_repo()?;
        let head = run_git(
            &self.root,
            &["rev-parse", "--verify", "-q", "HEAD"],
            GIT_TIMEOUT,
        )?;
        if !head.success() {
            return Ok(Vec::new());
        }

        let limit = normalize_limit(limit).to_string();
        let out = run_git(
            &self.root,
            &["log", "-n", &limit, "--format=%H%x00%h%x00%an%x00%s%x00%ct"],
            GIT_TIMEOUT,
        )?;
        if !out.success() {
            return Err(rejected("log", &out));
        }
        Ok(parse_log(&out.stdout))
    }

    fn ensure_repo(&self) -> Result<(), GitError> {
        let out = run_git(
            &self.root,
            &["rev-parse", "--is-inside-work-tree"],
            GIT_TIMEOUT,
        )?;
        if !out.success() {
            return Err(GitError::Unavailable {
                message: format!(
                    "not a git repository at {}: {}",
                    self.root.display(),
                    output_detail(&out)
                ),
            });
        }
        Ok(())
    }

    /// 分支、upstream、ahead/behind；这些是可选信息，单个探测失败不影响 status。
    fn branch_info(&self) -> (Option<String>, Option<String>, u32, u32) {
        let mut branch = None;
        if let Ok(out) = run_git(
            &self.root,
            &["rev-parse", "--abbrev-ref", "HEAD"],
            GIT_TIMEOUT,
        ) && out.success()
        {
            let name = out.stdout.trim();
            // 空仓库/游离 HEAD 都输出 HEAD；空仓库再回退到 symbolic-ref。
            if !name.is_empty() && name != "HEAD" {
                branch = Some(name.to_string());
            }
        }
        if branch.is_none()
            && let Ok(out) = run_git(
                &self.root,
                &["symbolic-ref", "--short", "HEAD"],
                GIT_TIMEOUT,
            )
            && out.success()
        {
            let name = out.stdout.trim();
            if !name.is_empty() {
                branch = Some(name.to_string());
            }
        }

        let upstream = run_git(
            &self.root,
            &["rev-parse", "--abbrev-ref", "@{u}"],
            GIT_TIMEOUT,
        )
        .ok()
        .filter(GitOutput::success)
        .map(|out| out.stdout.trim().to_string())
        .filter(|name| !name.is_empty());

        let (mut ahead, mut behind) = (0, 0);
        if upstream.is_some()
            && let Ok(out) = run_git(
                &self.root,
                &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
                GIT_TIMEOUT,
            )
            && out.success()
        {
            let mut counts = out.stdout.split_whitespace();
            ahead = counts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            behind = counts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        }

        (branch, upstream, ahead, behind)
    }

    /// `.git/MERGE_HEAD` 与 rebase 目录；`--git-dir` 兼容 subdirectory 与 worktree。
    fn operations_in_progress(&self) -> Result<(bool, bool), GitError> {
        let git_dir = self.git_dir()?;
        let merge = git_dir.join("MERGE_HEAD").is_file();
        let rebase = git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir();
        Ok((merge, rebase))
    }

    fn git_dir(&self) -> Result<PathBuf, GitError> {
        let out = run_git(&self.root, &["rev-parse", "--git-dir"], GIT_TIMEOUT)?;
        if !out.success() {
            return Err(GitError::Unavailable {
                message: format!(
                    "cannot locate git directory for {}: {}",
                    self.root.display(),
                    output_detail(&out)
                ),
            });
        }
        let raw = out.stdout.trim();
        if raw.is_empty() {
            return Err(GitError::Unavailable {
                message: format!("git dir is empty for {}", self.root.display()),
            });
        }
        let path = PathBuf::from(raw);
        Ok(if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        })
    }

    /// staged 或 unstaged 的 numstat，按路径聚合（rename 以新路径为 key）。
    fn numstat(&self, staged: bool) -> Result<HashMap<String, (u32, u32)>, GitError> {
        let mut args = vec!["diff", "--numstat", "-z"];
        if staged {
            args.push("--cached");
        }
        let out = run_git(&self.root, &args, GIT_TIMEOUT)?;
        if !out.success() {
            return Err(rejected(
                if staged { "diff --cached" } else { "diff" },
                &out,
            ));
        }
        Ok(parse_numstat(&out.stdout)
            .into_iter()
            .map(|(path, added, removed)| (path, (added, removed)))
            .collect())
    }

    /// 未跟踪文件（`--exclude-standard` 遵守 .gitignore），受可选 path 限定。
    fn untracked_paths(&self, path: Option<&str>) -> Result<Vec<String>, GitError> {
        let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z"];
        if let Some(path) = path {
            args.push("--");
            args.push(path);
        }
        let out = run_git(&self.root, &args, GIT_TIMEOUT)?;
        if !out.success() {
            return Err(rejected("diff", &out));
        }
        Ok(out
            .stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// name-status 决定状态/顺序，numstat 补充行数。
    fn tracked_diff_files(
        &self,
        path: Option<&str>,
        staged: bool,
    ) -> Result<Vec<GitFileStatus>, GitError> {
        let mut name_args = vec!["diff", "--name-status", "-z"];
        if staged {
            name_args.push("--cached");
        }
        if let Some(path) = path {
            name_args.push("--");
            name_args.push(path);
        }
        let names = run_git(&self.root, &name_args, GIT_TIMEOUT)?;
        if !names.success() {
            return Err(rejected("diff", &names));
        }

        let mut stat_args = vec!["diff", "--numstat", "-z"];
        if staged {
            stat_args.push("--cached");
        }
        if let Some(path) = path {
            stat_args.push("--");
            stat_args.push(path);
        }
        let stats = run_git(&self.root, &stat_args, GIT_TIMEOUT)?;
        if !stats.success() {
            return Err(rejected("diff", &stats));
        }
        let stats: HashMap<String, (u32, u32)> = parse_numstat(&stats.stdout)
            .into_iter()
            .map(|(path, added, removed)| (path, (added, removed)))
            .collect();

        Ok(parse_name_status(&names.stdout)
            .into_iter()
            .map(|(path, code)| {
                let (added, removed) = stats.get(&path).copied().unwrap_or((0, 0));
                GitFileStatus {
                    path,
                    status: map_status(code),
                    staged,
                    added,
                    removed,
                }
            })
            .collect())
    }

    /// 未跟踪文件的 `--no-index` 补丁。exit 1 是“有差异”的正常路径。
    fn untracked_diff(&self, path: &str) -> Result<String, String> {
        let out = run_git(
            &self.root,
            &["diff", "--no-index", "--no-color", "--", "/dev/null", path],
            GIT_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;
        if !out.success() && out.code != Some(1) {
            return Err(tail(&output_detail(&out), 500));
        }
        Ok(out.stdout)
    }
}

/// limit 缺省 20，硬 clamp 到 1..=100（0 也抬到 1）。
fn normalize_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LOG_LIMIT).clamp(1, MAX_LOG_LIMIT)
}

/// `%H%x00%h%x00%an%x00%s%x00%ct`，每条提交一行。
fn parse_log(raw: &str) -> Vec<GitCommit> {
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\0');
            let sha = fields.next()?.trim();
            if sha.is_empty() {
                return None;
            }
            let short_sha = fields.next().unwrap_or_default().to_string();
            let author = fields.next().unwrap_or_default().to_string();
            let message = fields.next().unwrap_or_default().to_string();
            let timestamp = fields
                .next()
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or(0);
            Some(GitCommit {
                sha: sha.to_string(),
                short_sha,
                author,
                message,
                timestamp,
            })
        })
        .collect()
}

/// 空列表与空字符串路径都在启动 git 前拒绝。
fn validated_paths<'a>(paths: &'a [String], operation: &str) -> Result<Vec<&'a str>, GitError> {
    if paths.is_empty() {
        return Err(GitError::Rejected {
            operation: operation.to_string(),
            detail: "no paths given".into(),
        });
    }
    if paths.iter().any(|path| path.is_empty()) {
        return Err(GitError::Rejected {
            operation: operation.to_string(),
            detail: "path must not be empty".into(),
        });
    }
    Ok(paths.iter().map(String::as_str).collect())
}
