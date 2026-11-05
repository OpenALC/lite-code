//! 上下文预算与压缩（docs/AGENT.md §6）：先瘦身旧工具输出，再摘要早期对话，
//! 仍超窗口时做一次激进压缩。
//!
//! 压缩只做"可记录的替换"：被裁掉的内容要么缩成带标记的短文本，要么汇入
//! 自描述的摘要。调用方通过 [`CompactionRecord`] 把每次压缩暴露给 UI。

use serde_json::Value;

use crate::llm::LlmInput;

/// 压缩时始终保留的最近消息条数。
pub const KEEP_RECENT_MESSAGES: usize = 4;

const SLIM_THRESHOLD_CHARS: usize = 800;
const SLIM_KEEP_CHARS: usize = 600;
const SLIM_SUFFIX: &str = "\n…[tool output compacted]";
const MAX_USER_SNIPPET_CHARS: usize = 200;
const MAX_TOOL_NAMES: usize = 8;
const AGGRESSIVE_KEEP_MESSAGES: usize = 2;

/// 一次压缩的审计记录。`removed_messages` 只统计被删除/替换的消息条数，
/// 仅瘦身工具输出时为 0。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionRecord {
    pub id: u64,
    pub before_tokens: u32,
    pub after_tokens: u32,
    pub removed_messages: u32,
    pub summary: String,
}

/// 粗估上下文 token：每个部分按字符数 / 4 向上取整，总和饱和到 `u32`。
///
/// 计入 instructions、工具 JSON 以及每条消息的文本 / 调用参数 / 工具输出。
pub fn estimate_tokens(instructions: Option<&str>, tools: &[Value], messages: &[LlmInput]) -> u32 {
    let mut tokens = 0u32;
    if let Some(instructions) = instructions {
        tokens = tokens.saturating_add(tokens_for_chars(instructions.chars().count()));
    }
    for tool in tools {
        tokens = tokens.saturating_add(tokens_for_chars(tool.to_string().chars().count()));
    }
    for message in messages {
        let chars = match message {
            LlmInput::Message { content, .. } => content.chars().count(),
            LlmInput::FunctionCall { arguments, .. } => arguments.chars().count(),
            LlmInput::FunctionCallOutput { output, .. } => output.chars().count(),
        };
        tokens = tokens.saturating_add(tokens_for_chars(chars));
    }
    tokens
}

/// 按需压缩消息历史。
///
/// 触发条件：估算 token 超过 `compress_at_k * 1000` 且消息数大于
/// [`KEEP_RECENT_MESSAGES`]。发生任何替换（含仅瘦身工具输出）时返回记录并
/// 递增 `next_id`；否则消息保持不变并返回 `None`。
pub fn compact(
    messages: &mut Vec<LlmInput>,
    instructions: Option<&str>,
    tools: &[Value],
    context_k: u32,
    compress_at_k: u32,
    next_id: &mut u64,
) -> Option<CompactionRecord> {
    let before_tokens = estimate_tokens(instructions, tools, messages);
    if u64::from(before_tokens) <= token_limit(compress_at_k)
        || messages.len() <= KEEP_RECENT_MESSAGES
    {
        return None;
    }

    let first_user = first_user_text(messages);
    let tool_names = tool_names(messages);

    // 1. 旧工具输出瘦身：最近 KEEP_RECENT_MESSAGES 条之前的输出只留前 600 字符。
    let keep_from = messages.len() - KEEP_RECENT_MESSAGES;
    let mut slimmed = 0u32;
    for message in messages.iter_mut().take(keep_from) {
        if let LlmInput::FunctionCallOutput { output, .. } = message
            && output.chars().count() > SLIM_THRESHOLD_CHARS
        {
            *output = slim_tool_output(output);
            slimmed = slimmed.saturating_add(1);
        }
    }

    let mut tokens = estimate_tokens(instructions, tools, messages);
    let mut removed_messages = 0u32;
    let mut summary = slim_summary(slimmed);

    // 2. 早期对话摘要：前缀替换为一条自描述 summary，最近 4 条原样保留。
    if u64::from(tokens) > token_limit(compress_at_k) {
        let prefix = messages.len() - KEEP_RECENT_MESSAGES;
        let removed = u32::try_from(prefix).unwrap_or(u32::MAX);
        summary = conversation_summary(
            removed,
            &first_user,
            &tool_names,
            KEEP_RECENT_MESSAGES,
            slimmed,
            false,
        );
        messages.splice(0..prefix, [LlmInput::new("user", summary.clone())]);
        removed_messages = removed_messages.saturating_add(removed);
        tokens = estimate_tokens(instructions, tools, messages);
    }

    // 3. 仍超 context 上限：保留最后 2 条 + 一条摘要；没有可删内容则保持原样。
    if u64::from(tokens) > token_limit(context_k) && messages.len() > AGGRESSIVE_KEEP_MESSAGES {
        let prefix = messages.len() - AGGRESSIVE_KEEP_MESSAGES;
        let removed = u32::try_from(prefix).unwrap_or(u32::MAX);
        removed_messages = removed_messages.saturating_add(removed);
        summary = conversation_summary(
            removed_messages,
            &first_user,
            &tool_names,
            AGGRESSIVE_KEEP_MESSAGES,
            slimmed,
            true,
        );
        messages.splice(0..prefix, [LlmInput::new("user", summary.clone())]);
    }

    if slimmed == 0 && removed_messages == 0 {
        return None;
    }

    let after_tokens = estimate_tokens(instructions, tools, messages);
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    Some(CompactionRecord {
        id,
        before_tokens,
        after_tokens,
        removed_messages,
        summary,
    })
}

fn token_limit(k: u32) -> u64 {
    u64::from(k) * 1000
}

fn tokens_for_chars(chars: usize) -> u32 {
    let chars = u64::try_from(chars).unwrap_or(u64::MAX);
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

/// 按 char 边界截断，避免切坏 UTF-8。
fn slim_tool_output(output: &str) -> String {
    let mut trimmed: String = output.chars().take(SLIM_KEEP_CHARS).collect();
    trimmed.push_str(SLIM_SUFFIX);
    trimmed
}

fn slim_summary(slimmed: u32) -> String {
    if slimmed == 0 {
        return String::new();
    }
    format!(
        "[上下文压缩] 瘦身了 {slimmed} 条工具输出；最近 {KEEP_RECENT_MESSAGES} 条消息保持原样。"
    )
}

/// 首个真实用户消息的前 200 字符；跳过之前生成的压缩摘要，避免摘要套摘要。
fn first_user_text(messages: &[LlmInput]) -> String {
    let snippet = messages.iter().find_map(|message| match message {
        LlmInput::Message { role, content }
            if role == "user" && !content.is_empty() && !content.starts_with("[上下文压缩]") =>
        {
            Some(content.chars().take(MAX_USER_SNIPPET_CHARS).collect())
        }
        _ => None,
    });
    snippet.unwrap_or_else(|| "无".to_string())
}

fn tool_names(messages: &[LlmInput]) -> Vec<String> {
    let mut names = Vec::new();
    for message in messages {
        if let LlmInput::FunctionCall { name, .. } = message
            && !name.is_empty()
            && !names.iter().any(|seen| seen == name)
        {
            names.push(name.clone());
            if names.len() == MAX_TOOL_NAMES {
                break;
            }
        }
    }
    names
}

fn conversation_summary(
    removed: u32,
    first_user: &str,
    tool_names: &[String],
    keep_recent: usize,
    slimmed: u32,
    aggressive: bool,
) -> String {
    let kind = if aggressive { "激进压缩：" } else { "" };
    let tools = if tool_names.is_empty() {
        "无".to_string()
    } else {
        tool_names.join("、")
    };
    let mut summary = format!(
        "[上下文压缩] {kind}已压缩 {removed} 条早期消息；早期用户请求：{first_user}；\
         调用过的工具：{tools}；最近 {keep_recent} 条消息保持原样"
    );
    if slimmed > 0 {
        summary.push_str(&format!("；瘦身了 {slimmed} 条工具输出"));
    }
    summary.push('。');
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn big_text(ch: char, chars: usize) -> String {
        std::iter::repeat_n(ch, chars).collect()
    }

    fn output_of(message: &LlmInput) -> &str {
        match message {
            LlmInput::FunctionCallOutput { output, .. } => output,
            other => panic!("expected function_call_output, got {other:?}"),
        }
    }

    #[test]
    fn estimates_instructions_tools_and_messages() {
        let instructions = "12345678";
        let tools = vec![json!({"type": "function", "name": "read_file"})];
        let messages = vec![
            LlmInput::new("user", "12345"),
            LlmInput::function_call("c1", "read_file", "{}"),
            LlmInput::function_call_output("c1", "abcdefgh"),
        ];
        let tool_chars = tools[0].to_string().chars().count();
        let expected = 2 + tokens_for_chars(tool_chars) + 2 + 1 + 2;
        assert_eq!(
            estimate_tokens(Some(instructions), &tools, &messages),
            expected
        );
        assert!(expected > 5, "每一部分都必须计入");
        assert_eq!(estimate_tokens(None, &[], &[]), 0);
    }

    #[test]
    fn extreme_budgets_saturate_without_panicking() {
        assert_eq!(tokens_for_chars(0), 0);
        assert_eq!(tokens_for_chars(4), 1);
        assert_eq!(tokens_for_chars(5), 2);
        assert_eq!(tokens_for_chars(usize::MAX), u32::MAX);
        assert_eq!(token_limit(u32::MAX), u64::from(u32::MAX) * 1000);

        let mut messages = vec![LlmInput::new("user", big_text('x', 10_000))];
        let mut next_id = 0;
        assert!(compact(&mut messages, None, &[], 0, u32::MAX, &mut next_id).is_none());
        assert_eq!(messages.len(), 1);
        assert_eq!(
            estimate_tokens(
                None,
                &[],
                &[LlmInput::new("user", big_text('x', 1_000_000))]
            ),
            250_000
        );
    }

    #[test]
    fn below_threshold_leaves_messages_unchanged() {
        let original: Vec<LlmInput> = (0..6)
            .map(|i| LlmInput::new("user", format!("message {i}")))
            .collect();
        let mut messages = original.clone();
        let mut next_id = 0;
        assert!(compact(&mut messages, None, &[], 400, 300, &mut next_id).is_none());
        assert_eq!(messages, original);
        assert_eq!(next_id, 0);
    }

    #[test]
    fn does_not_compact_when_only_recent_messages_exist() {
        let original = vec![
            LlmInput::new("user", big_text('x', 4000)),
            LlmInput::new("assistant", big_text('y', 4000)),
            LlmInput::new("user", big_text('z', 4000)),
            LlmInput::new("assistant", "done"),
        ];
        let mut messages = original.clone();
        let mut next_id = 0;
        assert!(compact(&mut messages, None, &[], 400, 1, &mut next_id).is_none());
        assert_eq!(messages, original);
    }

    #[test]
    fn slims_only_old_tool_outputs() {
        let old_output = big_text('测', 4000);
        let recent_output = big_text('近', 1000);
        let mut messages = vec![
            LlmInput::new("user", "start"),
            LlmInput::function_call("c1", "read_file", "{}"),
            LlmInput::function_call_output("c1", old_output),
            LlmInput::function_call_output("c2", recent_output.clone()),
            LlmInput::new("user", "next"),
            LlmInput::new("assistant", "ok"),
            LlmInput::new("user", "last"),
        ];
        let mut next_id = 0;
        let record = compact(&mut messages, None, &[], 400, 1, &mut next_id).expect("compaction");

        assert_eq!(record.id, 0);
        assert_eq!(next_id, 1);
        assert_eq!(record.removed_messages, 0);
        assert!(
            record.summary.contains("瘦身了 1 条工具输出"),
            "{}",
            record.summary
        );
        assert!(record.after_tokens < record.before_tokens);

        let slimmed = output_of(&messages[2]);
        assert!(slimmed.starts_with(&big_text('测', 600)));
        assert!(slimmed.ends_with("[tool output compacted]"));
        assert_eq!(slimmed.chars().count(), 600 + SLIM_SUFFIX.chars().count());
        assert!(slimmed.is_char_boundary(slimmed.len()));

        // 最近 4 条（含大输出）不参与瘦身。
        assert_eq!(output_of(&messages[3]), recent_output);
        assert_eq!(messages.len(), 7);
    }

    #[test]
    fn summarizes_early_messages_and_keeps_recent() {
        let mut messages = vec![
            LlmInput::new("user", "refactor the parser"),
            LlmInput::function_call("c1", "read_file", "{}"),
            LlmInput::function_call_output("c1", big_text('x', 4000)),
            LlmInput::function_call("c2", "shell", "{}"),
            LlmInput::new("assistant", "working"),
            LlmInput::new("user", big_text('y', 8000)),
            LlmInput::new("assistant", "done"),
            LlmInput::new("user", "thanks"),
            LlmInput::new("assistant", "ok"),
            LlmInput::new("user", "next"),
        ];
        let recent = messages[6..].to_vec();
        let mut next_id = 0;
        let record = compact(&mut messages, None, &[], 400, 1, &mut next_id).expect("compaction");

        assert_eq!(record.removed_messages, 6);
        assert_eq!(next_id, 1);
        assert!(record.after_tokens < record.before_tokens);
        assert_eq!(messages.len(), 1 + KEEP_RECENT_MESSAGES);
        assert_eq!(&messages[1..], recent.as_slice());

        let LlmInput::Message { role, content } = &messages[0] else {
            panic!("expected summary message");
        };
        assert_eq!(role, "user");
        assert!(content.starts_with("[上下文压缩]"), "{content}");
        assert!(content.contains("已压缩 6 条早期消息"), "{content}");
        assert!(content.contains("refactor the parser"), "{content}");
        assert!(content.contains("read_file、shell"), "{content}");
        assert!(content.contains("最近 4 条消息保持原样"), "{content}");
        assert!(content.chars().count() < 500, "摘要必须保持短小：{content}");
    }

    #[test]
    fn summary_uses_none_placeholders() {
        let mut messages = vec![
            LlmInput::new("assistant", big_text('a', 4000)),
            LlmInput::new("assistant", "thinking"),
            LlmInput::new("assistant", "more"),
            LlmInput::new("assistant", "ok"),
            LlmInput::new("assistant", "done"),
            LlmInput::new("assistant", "end"),
        ];
        let mut next_id = 0;
        let record = compact(&mut messages, None, &[], 400, 1, &mut next_id).expect("compaction");
        assert_eq!(record.removed_messages, 2);
        assert!(
            record.summary.contains("早期用户请求：无"),
            "{}",
            record.summary
        );
        assert!(
            record.summary.contains("调用过的工具：无"),
            "{}",
            record.summary
        );
    }

    #[test]
    fn compaction_ids_increment() {
        let make = || {
            vec![
                LlmInput::new("user", big_text('x', 8000)),
                LlmInput::function_call("c1", "read_file", "{}"),
                LlmInput::new("assistant", "one"),
                LlmInput::new("assistant", "two"),
                LlmInput::new("assistant", "three"),
                LlmInput::new("assistant", "four"),
            ]
        };
        let mut first = make();
        let mut second = make();
        let mut next_id = 7;
        let r1 = compact(&mut first, None, &[], 400, 1, &mut next_id).expect("first");
        let r2 = compact(&mut second, None, &[], 400, 1, &mut next_id).expect("second");
        assert_eq!((r1.id, r2.id), (7, 8));
        assert_eq!(next_id, 9);
    }

    #[test]
    fn aggressive_pass_runs_when_still_over_context() {
        let mut messages: Vec<LlmInput> = (0..4)
            .map(|i| LlmInput::new("assistant", format!("small {i}")))
            .collect();
        for _ in 0..4 {
            messages.push(LlmInput::new("user", big_text('z', 200_000)));
        }
        let before = estimate_tokens(None, &[], &messages);
        let mut next_id = 0;
        let record = compact(&mut messages, None, &[], 100, 50, &mut next_id).expect("compaction");

        assert_eq!(messages.len(), 1 + AGGRESSIVE_KEEP_MESSAGES);
        assert_eq!(record.removed_messages, 7);
        assert!(record.summary.contains("激进压缩"), "{}", record.summary);
        assert!(
            record.summary.contains("最近 2 条消息保持原样"),
            "{}",
            record.summary
        );
        assert_eq!(record.before_tokens, before);
        assert!(record.after_tokens < record.before_tokens);
    }

    #[test]
    fn instructions_and_tools_alone_can_trigger_context_pressure() {
        let instructions = big_text('i', 8000);
        let mut messages: Vec<LlmInput> = vec![LlmInput::new("user", "hi")];
        let mut next_id = 0;
        // 消息数不足，不压缩；调用方应据此判断超窗。
        assert!(
            compact(
                &mut messages,
                Some(&instructions),
                &[],
                400,
                1,
                &mut next_id
            )
            .is_none()
        );
        assert_eq!(messages.len(), 1);
        assert!(estimate_tokens(Some(&instructions), &[], &messages) > 2000);
    }
}
