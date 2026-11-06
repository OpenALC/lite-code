//! 工作区全文搜索（VS Code 式搜索侧栏的后端）。
//!
//! 约束（docs/UI.md §6）：搜索在主线程之外由 Rust 完成，前端只渲染第一页结果；
//! 遍历复用 `ignore` crate，因此 `.gitignore` / `.ignore` 语义与文件树、ripgrep
//! 一致。默认跳过隐藏文件、二进制与超过 [`MAX_FILE_BYTES`] 的文件。
//!
//! 匹配以"行"为单位：同一行只返回首个非空匹配，避免 `^` / `a*` 之类正则产生
//! 零长度结果刷屏。返回的路径是展示用的绝对路径（与 `fs_read_dir` 一致），
//! 前端直接回传给 `fs_read_file` 即可打开。

use std::fs;
use std::path::Path;

use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};

use super::FsError;
use super::paths::{PathResolver, display_path, ensure_within_root};

/// 总结果上限（行数），防止超大工作区把 IPC 撑爆。
pub const DEFAULT_MAX_RESULTS: usize = 500;
/// 单文件结果上限。
pub const PER_FILE_LIMIT: usize = 100;
/// 参与搜索的单文件大小上限；超过则跳过（不读入内存）。
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// 结果行预览的最大字符数（不含首尾省略号）。
const PREVIEW_CHARS: usize = 240;
/// 命中位置左侧保留的上下文字符数。
const PREVIEW_LEAD: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    pub query: String,
    pub case_sensitive: bool,
    pub regex: bool,
    pub max_results: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            case_sensitive: false,
            regex: false,
            max_results: DEFAULT_MAX_RESULTS,
        }
    }
}

/// 单行命中。`line` / `column` 从 1 开始，`column` 与 `match_start` 按**字符**
/// 计数（前端 JS 字符串索引也是 UTF-16，BMP 内一致；emoji 等补充平面字符的
/// 高亮位置可能偏 1，属于可接受误差）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub line: u32,
    pub column: u32,
    pub length: u32,
    pub preview: String,
    pub match_start: u32,
}

/// 按文件分组的结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileResult {
    pub path: String,
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub files: Vec<SearchFileResult>,
    pub total_matches: usize,
    pub truncated: bool,
    pub scanned_files: usize,
}

/// 在工作区内做全文搜索。路径校验、忽略规则与文件读取都在 Rust 侧完成。
pub fn search_workspace(
    root: &Path,
    resolver: &dyn PathResolver,
    options: &SearchOptions,
) -> Result<SearchResult, FsError> {
    if options.query.is_empty() {
        return Ok(SearchResult {
            files: Vec::new(),
            total_matches: 0,
            truncated: false,
            scanned_files: 0,
        });
    }

    let pattern = if options.regex {
        options.query.clone()
    } else {
        regex::escape(&options.query)
    };
    let matcher = RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
        .map_err(|e| FsError::InvalidArgument {
            message: format!("无效的搜索正则：{e}"),
        })?;

    let canonical = ensure_within_root(root, root, resolver)?;
    if !canonical.is_dir() {
        return Err(FsError::Io {
            path: display_path(&canonical),
            message: "not a directory".into(),
        });
    }

    let limit = options.max_results.max(1);
    // 多探一条用于区分“恰好 N 条”与“被截断”，避免满额时误报。
    let probe = limit.saturating_add(1);
    let mut files = Vec::new();
    let mut total_matches = 0usize;
    let mut truncated = false;
    let mut scanned_files = 0usize;

    for entry in WalkBuilder::new(&canonical).build() {
        if total_matches >= probe {
            break;
        }
        let Ok(de) = entry else { continue };
        let Some(ft) = de.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let path = de.path();
        let Ok(meta) = de.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(path) else { continue };
        // 前 8 KiB 出现 NUL 视为二进制：与常见搜索工具一致，避免把压缩包/图片
        // 当文本扫描。
        if bytes.iter().take(8192).any(|b| *b == 0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        scanned_files += 1;

        let remaining = probe - total_matches;
        let want = remaining.min(PER_FILE_LIMIT);
        let mut matches = collect_file_matches(&matcher, &text, want + 1);
        if matches.len() > want {
            matches.truncate(want);
            truncated = true;
        }
        if matches.is_empty() {
            continue;
        }
        total_matches += matches.len();
        files.push(SearchFileResult {
            path: display_path(path),
            matches,
        });
    }

    if total_matches > limit {
        truncated = true;
        let overflow = total_matches - limit;
        if let Some(last) = files.last_mut() {
            let keep = last.matches.len().saturating_sub(overflow);
            last.matches.truncate(keep);
        }
        files.retain(|file| !file.matches.is_empty());
        total_matches = limit;
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(SearchResult {
        files,
        total_matches,
        truncated,
        scanned_files,
    })
}

fn collect_file_matches(matcher: &Regex, text: &str, limit: usize) -> Vec<SearchMatch> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if out.len() >= limit {
            break;
        }
        let Some(found) = matcher.find_iter(line).find(|m| m.start() != m.end()) else {
            continue;
        };
        let (preview, match_start) = preview_for(line, found.start());
        out.push(SearchMatch {
            line: (index + 1) as u32,
            column: (line[..found.start()].chars().count() + 1) as u32,
            length: line[found.start()..found.end()].chars().count() as u32,
            preview,
            match_start,
        });
    }
    out
}

/// 截取命中行的一段预览：命中点左侧保留 [`PREVIEW_LEAD`] 个字符，总长不超过
/// [`PREVIEW_CHARS`]；被裁剪的一侧补省略号，并同步命中在预览里的起始位置。
fn preview_for(line: &str, byte_start: usize) -> (String, u32) {
    let chars: Vec<char> = line.chars().collect();
    let match_start_char = line[..byte_start].chars().count();
    let window_start = match_start_char.saturating_sub(PREVIEW_LEAD);
    let window_end = (window_start + PREVIEW_CHARS).min(chars.len());

    let mut preview: String = chars[window_start..window_end].iter().collect();
    let mut match_start = match_start_char - window_start;
    if window_start > 0 {
        preview.insert(0, '…');
        match_start += 1;
    }
    if window_end < chars.len() {
        preview.push('…');
    }
    (preview, match_start as u32)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::fs::RealPathResolver;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_root(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("lc-search-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn opts(query: &str) -> SearchOptions {
        SearchOptions {
            query: query.to_string(),
            ..SearchOptions::default()
        }
    }

    fn run(root: &Path, options: &SearchOptions) -> SearchResult {
        search_workspace(root, &RealPathResolver, options).expect("search")
    }

    #[test]
    fn literal_search_groups_by_file_and_reports_positions() {
        let root = temp_root("literal");
        fs::write(root.join("a.txt"), "alpha\nBeta alpha\n").unwrap();
        fs::write(root.join("b.txt"), "alpha").unwrap();

        let result = run(&root, &opts("alpha"));
        assert_eq!(result.total_matches, 3, "{result:?}");
        assert_eq!(result.files.len(), 2);
        assert!(result.files.iter().any(|f| f.path.ends_with("a.txt")
            && f.matches.len() == 2
            && f.matches[0].line == 1
            && f.matches[0].column == 1
            && f.matches[1].line == 2
            && f.matches[1].column == 6));
        assert!(!result.truncated);
    }

    #[test]
    fn case_sensitivity_is_opt_in() {
        let root = temp_root("case");
        fs::write(root.join("a.txt"), "Alpha\nalpha\n").unwrap();

        let insensitive = run(&root, &opts("Alpha"));
        assert_eq!(insensitive.total_matches, 2);

        let sensitive = run(
            &root,
            &SearchOptions {
                query: "Alpha".into(),
                case_sensitive: true,
                ..SearchOptions::default()
            },
        );
        assert_eq!(sensitive.total_matches, 1);
        assert_eq!(sensitive.files[0].matches[0].line, 1);
    }

    #[test]
    fn regex_mode_and_invalid_pattern() {
        let root = temp_root("regex");
        fs::write(root.join("a.txt"), "item 12\nitem x\n").unwrap();

        let result = run(
            &root,
            &SearchOptions {
                query: r"item \d+".into(),
                regex: true,
                ..SearchOptions::default()
            },
        );
        assert_eq!(result.total_matches, 1);

        let err = search_workspace(
            &root,
            &RealPathResolver,
            &SearchOptions {
                query: "(".into(),
                regex: true,
                ..SearchOptions::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, FsError::InvalidArgument { .. }));
    }

    #[test]
    fn zero_length_regex_matches_are_ignored() {
        let root = temp_root("zero");
        fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        let result = run(
            &root,
            &SearchOptions {
                query: "^".into(),
                regex: true,
                ..SearchOptions::default()
            },
        );
        assert_eq!(result.total_matches, 0);
    }

    #[test]
    fn ignore_file_is_respected() {
        let root = temp_root("ignore");
        fs::write(root.join(".ignore"), "skip.txt\n").unwrap();
        fs::write(root.join("keep.txt"), "needle\n").unwrap();
        fs::write(root.join("skip.txt"), "needle\n").unwrap();

        let result = run(&root, &opts("needle"));
        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].path.ends_with("keep.txt"));
    }

    #[test]
    fn binary_and_oversized_files_are_skipped() {
        let root = temp_root("binary");
        fs::write(root.join("bin.dat"), b"needle\0more").unwrap();
        fs::write(root.join("big.txt"), "needle\n".repeat(400 * 1024)).unwrap();
        fs::write(root.join("ok.txt"), "needle\n").unwrap();

        let result = run(&root, &opts("needle"));
        assert!(result.files.iter().any(|f| f.path.ends_with("ok.txt")));
        assert!(!result.files.iter().any(|f| f.path.ends_with("bin.dat")));
        assert!(!result.files.iter().any(|f| f.path.ends_with("big.txt")));
    }

    #[test]
    fn max_results_truncates_and_reports() {
        let root = temp_root("limit");
        fs::write(root.join("a.txt"), "hit\n".repeat(200)).unwrap();

        let result = run(
            &root,
            &SearchOptions {
                query: "hit".into(),
                max_results: 7,
                ..SearchOptions::default()
            },
        );
        assert_eq!(result.total_matches, 7);
        assert!(result.truncated);
    }

    #[test]
    fn exact_limit_is_not_reported_as_truncated() {
        let root = temp_root("exact");
        fs::write(root.join("a.txt"), "hit\n".repeat(7)).unwrap();
        let result = run(
            &root,
            &SearchOptions {
                query: "hit".into(),
                max_results: 7,
                ..SearchOptions::default()
            },
        );
        assert_eq!(result.total_matches, 7);
        assert!(!result.truncated, "{result:?}");
    }

    #[test]
    fn per_file_cap_marks_truncated() {
        let root = temp_root("per-file");
        fs::write(root.join("a.txt"), "hit\n".repeat(PER_FILE_LIMIT + 50)).unwrap();
        let result = run(
            &root,
            &SearchOptions {
                query: "hit".into(),
                max_results: 500,
                ..SearchOptions::default()
            },
        );
        assert_eq!(result.total_matches, PER_FILE_LIMIT);
        assert!(result.truncated);
    }

    #[test]
    fn empty_query_returns_empty_without_scanning() {
        let root = temp_root("empty");
        fs::write(root.join("a.txt"), "anything\n").unwrap();
        let result = run(&root, &opts(""));
        assert_eq!(result.total_matches, 0);
        assert_eq!(result.scanned_files, 0);
    }

    #[test]
    fn long_lines_are_windowed_around_match() {
        let root = temp_root("preview");
        let line = format!("{}needle{}", "x".repeat(400), "y".repeat(400));
        fs::write(root.join("a.txt"), format!("{line}\n")).unwrap();

        let result = run(&root, &opts("needle"));
        let hit = &result.files[0].matches[0];
        assert!(hit.preview.starts_with('…'), "{}", hit.preview);
        assert!(hit.preview.ends_with('…'), "{}", hit.preview);
        assert!(hit.preview.chars().count() <= PREVIEW_CHARS + 2);
        let chars: Vec<char> = hit.preview.chars().collect();
        let start = hit.match_start as usize;
        assert_eq!(chars[start..start + 6].iter().collect::<String>(), "needle");
        assert_eq!(hit.column, 401);
    }
}
