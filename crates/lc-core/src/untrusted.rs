//! 不可信内容边界标记的转义。
//!
//! 注入 prompt 或工具结果的内容可能自带边界字面量；若不处理，内容可以
//! 提前闭合 `[UNTRUSTED …]` 包装，让后续文本看起来像边界外的可信指令。
//! 所有包装点都必须先用 [`escape_markers`] 处理正文。

const OPEN: &str = "[UNTRUSTED";
const OPEN_ESCAPED: &str = "[UNTRUSTED_ESCAPED";
const CLOSE: &str = "[/UNTRUSTED";
const CLOSE_ESCAPED: &str = "[/UNTRUSTED_ESCAPED";

/// 把正文里大小写不敏感的 `[UNTRUSTED` / `[/UNTRUSTED` 前缀替换成
/// `_ESCAPED` 变体。已转义的标记原样保留，因此重复调用是幂等的。
pub(crate) fn escape_markers(content: &str) -> String {
    let mut escaped = String::with_capacity(content.len());
    let mut rest = content;
    while !rest.is_empty() {
        if starts_with_ignore_ascii_case(rest, CLOSE_ESCAPED) {
            escaped.push_str(&rest[..CLOSE_ESCAPED.len()]);
            rest = &rest[CLOSE_ESCAPED.len()..];
        } else if starts_with_ignore_ascii_case(rest, CLOSE) {
            escaped.push_str(CLOSE_ESCAPED);
            rest = &rest[CLOSE.len()..];
        } else if starts_with_ignore_ascii_case(rest, OPEN_ESCAPED) {
            escaped.push_str(&rest[..OPEN_ESCAPED.len()]);
            rest = &rest[OPEN_ESCAPED.len()..];
        } else if starts_with_ignore_ascii_case(rest, OPEN) {
            escaped.push_str(OPEN_ESCAPED);
            rest = &rest[OPEN.len()..];
        } else {
            let ch = rest.chars().next().expect("rest is non-empty");
            escaped.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    escaped
}

fn starts_with_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack
        .get(..needle.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_open_and_close_markers() {
        let escaped = escape_markers("before [/UNTRUSTED] [UNTRUSTED x] after");
        assert_eq!(
            escaped,
            "before [/UNTRUSTED_ESCAPED] [UNTRUSTED_ESCAPED x] after"
        );
        assert!(!escaped.contains("[/UNTRUSTED]"));
        assert!(!escaped.contains("[UNTRUSTED "));
    }

    #[test]
    fn escapes_case_insensitively_and_is_idempotent() {
        let once = escape_markers("前缀 [/untrusted] [Untrusted");
        assert_eq!(once, "前缀 [/UNTRUSTED_ESCAPED] [UNTRUSTED_ESCAPED");
        assert_eq!(escape_markers(&once), once);
    }
}
