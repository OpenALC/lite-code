//! 语义化版本（SemVer）解析与比较：应用更新器与插件市场共用的单一语义来源
//! （docs/plans/plugin-marketplace.md §3.5）。实现从 `apps/desktop/src-tauri/src/updater.rs`
//! 原样迁入，双方共用同一套测试矩阵，避免「更新器 vs 市场」的比较语义漂移。
//!
//! 只实现比较所需的子集，不引新 crate：三段核心版本号 + prerelease
//! （数字段 < 字母段，release > prerelease；`0.6.1-preview.2 < 0.6.1-preview.10`
//! 按数值比较）+ build metadata（不参与比较但必须语法合法）。

/// SemVer 版本。字段私有，构造唯一入口是 [`Version::parse`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<PreId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreId {
    Numeric(u64),
    Alpha(String),
}

impl Version {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        // build metadata 不参与比较，但必须语法合法。
        let (rest, build) = match raw.split_once('+') {
            Some((rest, build)) => (rest, Some(build)),
            None => (raw, None),
        };
        if let Some(build) = build
            && (build.is_empty()
                || !build
                    .split('.')
                    .all(|part| !part.is_empty() && is_identifier(part)))
        {
            return None;
        }
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };
        let mut parts = core.split('.');
        let major = parse_core(parts.next()?)?;
        let minor = parse_core(parts.next()?)?;
        let patch = parse_core(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some(pre) => {
                let mut ids = Vec::new();
                for part in pre.split('.') {
                    if part.is_empty() || !is_identifier(part) {
                        return None;
                    }
                    if part.bytes().all(|b| b.is_ascii_digit()) {
                        if part.len() > 1 && part.starts_with('0') {
                            return None;
                        }
                        ids.push(PreId::Numeric(part.parse().ok()?));
                    } else {
                        ids.push(PreId::Alpha(part.to_string()));
                    }
                }
                ids
            }
            None => Vec::new(),
        };
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

fn parse_core(raw: &str) -> Option<u64> {
    if raw.is_empty()
        || !raw.bytes().all(|b| b.is_ascii_digit())
        || (raw.len() > 1 && raw.starts_with('0'))
    {
        return None;
    }
    raw.parse().ok()
}

fn is_identifier(raw: &str) -> bool {
    raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                (false, false) => cmp_prerelease(&self.pre, &other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn cmp_prerelease(left: &[PreId], right: &[PreId]) -> std::cmp::Ordering {
    for (a, b) in left.iter().zip(right) {
        let ordering = match (a, b) {
            (PreId::Numeric(a), PreId::Numeric(b)) => a.cmp(b),
            (PreId::Numeric(_), PreId::Alpha(_)) => std::cmp::Ordering::Less,
            (PreId::Alpha(_), PreId::Numeric(_)) => std::cmp::Ordering::Greater,
            (PreId::Alpha(a), PreId::Alpha(b)) => a.cmp(b),
        };
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

/// 极简 engines 约束求值（docs/plans/plugin-marketplace.md §3.1 安装前预检）。
///
/// 支持 `>=` / `<=` / `>` / `<` / `=` / `==` + 版本号，多个子句以空格或逗号
/// 分隔（须全部满足）；裸版本号视为精确匹配。约束无法解析时 fail-closed
/// （视为不满足），由调用方决定是报错还是 `force` 跳过。
pub fn satisfies_range(current: &str, constraint: &str) -> bool {
    let Some(current) = Version::parse(current) else {
        return false;
    };
    let constraint = constraint.trim();
    if constraint.is_empty() {
        return true;
    }
    constraint
        .split([',', ' '])
        .filter(|clause| !clause.is_empty())
        .all(|clause| satisfies_clause(&current, clause))
}

fn satisfies_clause(current: &Version, clause: &str) -> bool {
    let (op, inclusive, raw_version) = if let Some(rest) = clause.strip_prefix(">=") {
        (std::cmp::Ordering::Greater, true, rest)
    } else if let Some(rest) = clause.strip_prefix("<=") {
        (std::cmp::Ordering::Less, true, rest)
    } else if let Some(rest) = clause.strip_prefix("==") {
        (std::cmp::Ordering::Equal, true, rest)
    } else if let Some(rest) = clause.strip_prefix('>') {
        (std::cmp::Ordering::Greater, false, rest)
    } else if let Some(rest) = clause.strip_prefix('<') {
        (std::cmp::Ordering::Less, false, rest)
    } else if let Some(rest) = clause.strip_prefix('=') {
        (std::cmp::Ordering::Equal, true, rest)
    } else {
        (std::cmp::Ordering::Equal, true, clause)
    };
    let Some(version) = Version::parse(raw_version) else {
        return false;
    };
    match current.cmp(&version) {
        std::cmp::Ordering::Equal => inclusive,
        ordering => ordering == op,
    }
}

#[cfg(test)]
mod tests {
    use super::{Version, satisfies_range};

    /// 更新器随迁的 semver 测试矩阵（语义单一来源，双方共用）。
    #[test]
    fn semver_ordering_handles_prerelease_and_build_metadata() {
        let parse = |v: &str| Version::parse(v).unwrap_or_else(|| panic!("parse {v}"));
        assert!(parse("0.6.1-preview.1") > parse("0.5.1-preview.1"));
        assert!(parse("0.6.1-preview.2") > parse("0.6.1-preview.1"));
        assert!(parse("0.6.1") > parse("0.6.1-preview.9"));
        assert!(parse("0.6.1-preview.10") > parse("0.6.1-preview.9"));
        assert_eq!(parse("0.6.1-preview.1+build.7"), parse("0.6.1-preview.1"));
        assert!(parse("1.0.0-alpha") < parse("1.0.0-alpha.1"));
        assert!(Version::parse("0.6").is_none());
        assert!(Version::parse("v0.6.1").is_none());
        assert!(Version::parse("0.6.1-preview.01").is_none());
    }

    /// 市场新增用例：prerelease 数字段按数值比较（preview.2 < preview.10），
    /// release 恒大于 prerelease（docs/plans/plugin-marketplace.md §3.5/§4）。
    #[test]
    fn semver_prerelease_compares_numerically() {
        let parse = |v: &str| Version::parse(v).unwrap_or_else(|| panic!("parse {v}"));
        assert!(parse("0.6.1-preview.2") < parse("0.6.1-preview.10"));
        assert!(parse("1.0.0-alpha") < parse("1.0.0"));
        assert!(parse("1.0.0-alpha") < parse("1.0.0-alpha.1"));
        assert!(parse("1.0.0-alpha.1") < parse("1.0.0-beta"));
        assert!(parse("2.0.0") > parse("2.0.0-rc.1"));
    }

    #[test]
    fn satisfies_range_supports_operators_and_clauses() {
        assert!(satisfies_range("0.6.1", ">=0.6.1"));
        assert!(satisfies_range("0.6.1", "0.6.1"));
        assert!(!satisfies_range("0.6.0", ">=0.6.1"));
        assert!(!satisfies_range("0.6.1-preview.1", ">=0.6.1"));
        assert!(satisfies_range("1.0.0", ">=0.6.1 <2.0.0"));
        assert!(!satisfies_range("2.0.0", ">=0.6.1 <2.0.0"));
        assert!(!satisfies_range("0.6.2", "0.6.1"));
        assert!(!satisfies_range("0.6.1", "not-a-range"));
        assert!(!satisfies_range("not-a-version", ">=0.6.1"));
        assert!(satisfies_range("0.6.1", ""));
        assert!(satisfies_range("0.6.1+build.1", ">=0.6.1"));
    }
}
