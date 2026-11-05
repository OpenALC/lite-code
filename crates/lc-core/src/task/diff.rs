//! changes / unified diff：全部以 worktree 为根读取，不触碰主工作区。
//!
//! - 状态解析用 `git status --porcelain=v1 -z`：NUL 分隔避免引号/转义，
//!   重命名条目固定为 `<XY> <new>\0<old>\0`（docs/TASKS.md §2）；
//! - 行数统计用 `git diff --numstat HEAD`，未跟踪文件单独统计行数；
//! - diff 用 `git diff --no-color HEAD`，未跟踪文件用
//!   `git diff --no-color --no-index -- /dev/null <file>` 追加（Git for Windows
//!   同样识别 `/dev/null`；失败时跳过并在 patch 末尾注明）。

use std::collections::HashMap;
use std::path::Path;

use lc_proto::{ChangeStatus, FileChangeDto};

use super::{GIT_TIMEOUT, TaskError, git_ok, run_git, tail};

/// 一次扫描的完整结果：对外的 changes 与仅供 diff 追加的未跟踪路径。
pub(crate) struct ScanResult {
    pub changes: Vec<FileChangeDto>,
    pub untracked: Vec<String>,
}

/// 任务 diff 的返回体（与 `CommandResult::TaskDiff` 字段一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDiff {
    pub task_id: String,
    pub patch: String,
    pub files: Vec<FileChangeDto>,
}

/// 扫描 worktree 的变更清单：状态 + numstat + 未跟踪行数。
pub(crate) fn scan_changes(worktree: &Path) -> Result<ScanResult, TaskError> {
    let status = git_ok(
        worktree,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        GIT_TIMEOUT,
    )?;

    let mut changes = Vec::new();
    let mut untracked = Vec::new();
    let mut entries = status.stdout.split('\0');
    while let Some(entry) = entries.next() {
        if entry.is_empty() {
            continue;
        }
        let bytes = entry.as_bytes();
        // `XY<space><path>`：前 3 字节固定为 ASCII，按字节切安全。
        if bytes.len() < 4 {
            continue;
        }
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        if x == '!' && y == '!' {
            continue; // ignored；默认不会被列出，防御性跳过
        }
        let path = entry[3..].to_string();
        // 重命名/复制在 -z 模式下把旧路径放在下一个 NUL 段。
        if x == 'R' || y == 'R' || x == 'C' || y == 'C' {
            let _old_path = entries.next();
        }
        if x == '?' {
            untracked.push(path.clone());
        }
        changes.push(FileChangeDto {
            path,
            status: map_status(x, y),
            applied: false,
            added: 0,
            removed: 0,
        });
    }

    let numstat = git_ok(
        worktree,
        &["-c", "core.quotePath=false", "diff", "--numstat", "HEAD"],
        GIT_TIMEOUT,
    )?;

    let mut stats: HashMap<String, (u32, u32)> = HashMap::new();
    for line in numstat.stdout.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // 二进制文件是 "-"，按 0/0 展示；解析失败同样退化，不阻塞清单。
        let added = added.parse::<u32>().unwrap_or(0);
        let removed = removed.parse::<u32>().unwrap_or(0);
        stats.insert(numstat_path(path), (added, removed));
    }

    for path in &untracked {
        stats.insert(path.clone(), count_lines(&worktree.join(path)));
    }

    for change in &mut changes {
        if let Some((added, removed)) = stats.get(&change.path) {
            change.added = *added;
            change.removed = *removed;
        }
    }

    Ok(ScanResult { changes, untracked })
}

/// 生成 unified diff：tracked 变更 + 逐未跟踪文件。
pub(crate) fn build_diff(worktree: &Path, untracked: &[String]) -> Result<String, TaskError> {
    let out = git_ok(
        worktree,
        &["-c", "core.quotePath=false", "diff", "--no-color", "HEAD"],
        GIT_TIMEOUT,
    )?;
    let mut patch = out.stdout;
    let mut notes = Vec::new();

    for path in untracked {
        let args = [
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-color",
            "--no-index",
            "--",
            "/dev/null",
            path.as_str(),
        ];
        match run_git(worktree, &args, GIT_TIMEOUT) {
            // --no-index 有差异时退出码是 1，0 表示内容相同（空文件）。
            Ok(out) if out.code == Some(0) || out.code == Some(1) => {
                if !out.stdout.is_empty() {
                    ensure_newline(&mut patch);
                    patch.push_str(&out.stdout);
                }
            }
            Ok(out) => notes.push(format!(
                "# lc: untracked diff skipped for {path}: {}",
                tail(&format!("{}\n{}", out.stdout, out.stderr), 400)
            )),
            Err(error) => notes.push(format!("# lc: untracked diff skipped for {path}: {error}")),
        }
    }

    for note in notes {
        ensure_newline(&mut patch);
        patch.push_str(&note);
        patch.push('\n');
    }

    Ok(patch)
}

fn ensure_newline(text: &mut String) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
}

/// `XY` → 契约枚举。`AD`/`RD` 这类“净效果已删除”的状态按 Deleted 归类；
/// `C`（复制）没有对应枚举，按 Added 处理。
fn map_status(x: char, y: char) -> ChangeStatus {
    if x == '?' {
        ChangeStatus::Added
    } else if x == 'D' || y == 'D' {
        ChangeStatus::Deleted
    } else if x == 'R' || y == 'R' {
        ChangeStatus::Renamed
    } else if x == 'A' || y == 'A' || x == 'C' || y == 'C' {
        ChangeStatus::Added
    } else {
        ChangeStatus::Modified
    }
}

/// numstat 的重命名路径还原成“新路径”：
/// `old => new`、`dir/{old => new}/file` 都归一为最终路径。
fn numstat_path(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(open) = raw.find('{')
        && let Some(close_rel) = raw[open..].find('}')
    {
        let close = open + close_rel;
        let prefix = &raw[..open];
        let inner = &raw[open + 1..close];
        let suffix = &raw[close + 1..];
        let new_part = inner
            .rsplit_once(" => ")
            .map(|(_, new)| new)
            .unwrap_or(inner);
        return format!("{prefix}{new_part}{suffix}");
    }
    raw.rsplit_once(" => ")
        .map(|(_, new)| new.to_string())
        .unwrap_or_else(|| raw.to_string())
}

/// 未跟踪文件行数：二进制（前 8 KiB 含 NUL）按 0/0。
fn count_lines(path: &Path) -> (u32, u32) {
    let Ok(bytes) = std::fs::read(path) else {
        return (0, 0);
    };
    if bytes.iter().take(8 * 1024).any(|&byte| byte == 0) {
        return (0, 0);
    }
    let mut lines = bytes.iter().filter(|&&byte| byte == b'\n').count() as u32;
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        lines = lines.saturating_add(1);
    }
    (lines, 0)
}
