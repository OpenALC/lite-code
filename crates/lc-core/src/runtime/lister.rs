//! [`DirLister`] 的真实实现与通配遍历。

use std::fs;
use std::path::{Component, Path, PathBuf};

use super::DirLister;

/// 单段通配遍历：模式中任意一段可含 `*`（不跨段），如
/// `%APPDATA%\nvm\v*\node.exe`、`%SCOOP%\apps\nodejs*\current\node.exe`。
pub fn glob_walk(pattern: &Path, lister: &dyn DirLister) -> Vec<PathBuf> {
    let parts: Vec<Component> = pattern.components().collect();
    let mut out = Vec::new();
    walk(&parts, Path::new(""), lister, &mut out);
    out
}

fn walk(parts: &[Component], current: &Path, lister: &dyn DirLister, out: &mut Vec<PathBuf>) {
    let Some((first, rest)) = parts.split_first() else {
        // 模式走完：只接受“存在且不是目录”的终点（node.exe 本体）
        if lister.exists(current) && !lister.is_dir(current) {
            out.push(current.to_path_buf());
        }
        return;
    };
    match first {
        Component::Prefix(_) | Component::RootDir => {
            walk(rest, &current.join(first), lister, out);
        }
        Component::CurDir | Component::ParentDir => {
            walk(rest, &current.join(first), lister, out);
        }
        Component::Normal(seg) => {
            let seg = seg.to_string_lossy();
            if seg.contains('*') {
                for name in lister.list_dir(current) {
                    if wild_match(&seg, &name) {
                        walk(rest, &current.join(&name), lister, out);
                    }
                }
            } else {
                walk(rest, &current.join(&*seg), lister, out);
            }
        }
    }
}

/// 经典单字符通配匹配（`*` 匹配任意子串），大小写不敏感。
pub fn wild_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    wild_match_inner(&p, &t)
}

fn wild_match_inner(p: &[char], t: &[char]) -> bool {
    match (p.first(), t.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some('*'), _) => {
            // `*` 匹配空串或吞一个字符
            wild_match_inner(&p[1..], t) || (!t.is_empty() && wild_match_inner(p, &t[1..]))
        }
        (Some(a), Some(b)) if a == b => wild_match_inner(&p[1..], &t[1..]),
        _ => false,
    }
}

/// 真实文件系统实现。
pub struct RealDirLister;

impl DirLister for RealDirLister {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        // std::fs::canonicalize 对 reparse point 有效（shim → 真实路径）
        fs::canonicalize(path)
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(path) {
            for entry in rd.flatten() {
                out.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        out
    }

    fn glob_files(&self, pattern: &Path) -> Vec<PathBuf> {
        glob_walk(pattern, self)
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn wildcard_matches_case_insensitive() {
        assert!(wild_match("nodejs*", "NodeJS-current"));
        assert!(wild_match("v*", "v22.4.0"));
        assert!(!wild_match("v*", "installation"));
        assert!(!wild_match("*x*", "abc"));
        assert!(wild_match("*x*", "abcx"));
    }
}
