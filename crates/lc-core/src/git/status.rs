//! `git status --porcelain=v1 -z`、`--numstat -z`、`--name-status -z` 的解析。
//!
//! 所有输入都来自 `-z` 输出，路径可能包含空格、制表符与非 ASCII 字符，
//! 因此按 NUL 切分、按固定列读取状态位，不对路径做任何转义/反转义。

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use lc_proto::ChangeStatus;

/// porcelain v1 的一条记录。`index`/`worktree` 只保留非空列；
/// 未跟踪与冲突单独标记，避免调用方重复推断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PorcelainEntry {
    pub path: String,
    pub index: Option<char>,
    pub worktree: Option<char>,
    pub untracked: bool,
    pub unmerged: bool,
}

/// 解析 `git status --porcelain=v1 -z --untracked-files=all`。
///
/// `-z` 下 rename/copy 记录为 `XY <new>\0<old>\0`；这里消费旧路径，
/// 只保留新路径（DTO 没有 old_path 字段，且后续 stage/diff 都按新路径操作）。
pub(crate) fn parse_porcelain(raw: &str) -> Vec<PorcelainEntry> {
    let mut tokens = raw.split('\0');
    let mut entries = Vec::new();

    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        let bytes = token.as_bytes();
        if bytes.len() < 3 {
            continue;
        }
        // 前两列固定是 ASCII 状态位，第三列是分隔空格，路径从字节 3 开始
        // （UTF-8 边界安全：前 3 字节都是 ASCII）。
        let index = bytes[0] as char;
        let worktree = bytes[1] as char;
        let path = token[3..].to_string();

        if index == '!' && worktree == '!' {
            continue;
        }
        if index == '?' || worktree == '?' {
            entries.push(PorcelainEntry {
                path,
                index: None,
                worktree: None,
                untracked: true,
                unmerged: false,
            });
            continue;
        }
        // rename/copy 的源路径紧跟在 NUL 之后，必须消费掉。
        if index == 'R' || index == 'C' {
            let _old_path = tokens.next();
        }
        let unmerged = index == 'U'
            || worktree == 'U'
            || (index == 'A' && worktree == 'A')
            || (index == 'D' && worktree == 'D');
        entries.push(PorcelainEntry {
            path,
            index: (index != ' ').then_some(index),
            worktree: (worktree != ' ').then_some(worktree),
            untracked: false,
            unmerged,
        });
    }

    entries
}

/// 解析 `git diff --numstat -z`：`<added>\t<removed>\t<path>\0`；
/// rename 记录为 `<added>\t<removed>\t\0<old>\0<new>\0`，按新路径入表；
/// 二进制文件的 `-` 计为 0。
pub(crate) fn parse_numstat(raw: &str) -> Vec<(String, u32, u32)> {
    let mut tokens = raw.split('\0');
    let mut entries = Vec::new();

    while let Some(head) = tokens.next() {
        if head.is_empty() {
            continue;
        }
        let mut parts = head.splitn(3, '\t');
        let added = parse_count(parts.next().unwrap_or_default());
        let removed = parse_count(parts.next().unwrap_or_default());
        let path_field = parts.next().unwrap_or_default();

        let path = if path_field.is_empty() {
            let _old_path = tokens.next();
            tokens.next().unwrap_or_default().to_string()
        } else {
            path_field.to_string()
        };
        if path.is_empty() {
            continue;
        }
        entries.push((path, added, removed));
    }

    entries
}

/// 解析 `git diff --name-status -z`：`<status>\0<path>\0`；
/// rename/copy 为 `<status>\0<old>\0<new>\0`，按新路径返回。
pub(crate) fn parse_name_status(raw: &str) -> Vec<(String, char)> {
    let mut tokens = raw.split('\0');
    let mut entries = Vec::new();

    while let Some(status) = tokens.next() {
        if status.is_empty() {
            continue;
        }
        let code = status.chars().next().unwrap_or('M');
        let Some(first) = tokens.next() else {
            continue;
        };
        if first.is_empty() {
            continue;
        }
        let path = if code == 'R' || code == 'C' {
            match tokens.next() {
                Some(new_path) if !new_path.is_empty() => new_path.to_string(),
                _ => first.to_string(),
            }
        } else {
            first.to_string()
        };
        entries.push((path, code));
    }

    entries
}

/// `ChangeStatus` 只有四类：copy/typechange/conflict 都归入最接近的语义。
pub(crate) fn map_status(code: char) -> ChangeStatus {
    match code {
        'A' => ChangeStatus::Added,
        'D' => ChangeStatus::Deleted,
        'R' => ChangeStatus::Renamed,
        // C（copy）没有独立 DTO：新路径以 Added 呈现。
        'C' => ChangeStatus::Added,
        // M、T（typechange）、U（冲突）在 UI 上都是“已修改”。
        _ => ChangeStatus::Modified,
    }
}

/// 未跟踪文件按行数统计 added；二进制（含 NUL）按 0 处理，读取失败也按 0。
pub(crate) fn untracked_line_count(path: &Path) -> u32 {
    let Ok(file) = File::open(path) else {
        return 0;
    };
    let mut reader = BufReader::new(file);
    let mut buf = [0u8; 8192];
    let mut lines = 0u64;
    let mut any = false;
    let mut last_byte_is_newline = true;

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                if chunk.contains(&0) {
                    return 0;
                }
                any = true;
                lines += chunk.iter().filter(|&&byte| byte == b'\n').count() as u64;
                last_byte_is_newline = chunk[n - 1] == b'\n';
            }
            Err(_) => return 0,
        }
    }

    if !any {
        return 0;
    }
    let count = if last_byte_is_newline {
        lines
    } else {
        lines + 1
    };
    count.min(u32::MAX as u64) as u32
}

fn parse_count(value: &str) -> u32 {
    value.trim().parse::<u32>().unwrap_or(0)
}
