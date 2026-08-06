//! 窗口级上下文压缩（多级压缩流水线）
//!
//! 在多轮工具调用循环中，对话消息会持续累积（assistant 工具调用 + tool 结果回喂），
//! 容易撑爆 LLM 上下文窗口。本模块提供三级压缩策略：
//!
//! ## 压缩层级
//!
//! **Level 1: Soft Trim**（每轮都执行）
//! - 扫描所有 tool_result 消息
//! - 内容超过 `SOFT_TRIM_HEAD + SOFT_TRIM_TAIL + 50` 字符时
//!   截断为 `"{head}\n…[截断 N 字符]…\n{tail}"` 格式
//!
//! **Level 2: Group Drop**（token 超阈值时触发）
//! - 将中段消息分组为 `MessageGroup`（单条消息 / 工具调用+结果捆绑）
//! - tool_call + tool_result 作为原子组保留或丢弃，永不拆分
//! - 从最旧组开始逐组丢弃，直到 token 达标
//!
//! **Level 3: Reminder Inject**（由 `compaction_reminder` 模块处理）

use crate::types::response::ChatMessage;
use crate::utils::token_estimate::{estimate_message_tokens, estimate_messages_tokens};

/// 单条消息截断后保留的字符数（字符级，用于单条消息预览截断）
const PREVIEW_CHARS: usize = 200;

/// Soft Trim 保留的头部字符数
const SOFT_TRIM_HEAD: usize = 1500;
/// Soft Trim 保留的尾部字符数
const SOFT_TRIM_TAIL: usize = 1500;
/// Soft Trim 触发阈值偏移量（head+tail+50 字符以上才触发）
const SOFT_TRIM_OVERHEAD: usize = 50;

/// 单条工具结果入队前的预截断阈值（字符级）
///
/// 在工具结果 push 到对话消息前先做一次预截断，
/// 避免巨大的工具输出（如 read_file 读取大文件）直接撑爆上下文窗口。
/// 预截断保留头尾各一半，中间用截断标记替代。
pub const MAX_TOOL_RESULT_CHARS: usize = 8000;

/// 上下文窗口预算（基于 128K context window）
///
/// 128K 总窗口 - 6K 系统提示 - 4K 输出预留 - 4K 安全余量 = 114K 可用
pub const CONTEXT_WINDOW_BUDGET: ContextWindowBudget = ContextWindowBudget {
    total_tokens: 128_000,
    system_prompt_reserve: 6_000,
    output_reserve: 4_000,
    safety_margin: 4_000,
};

/// 上下文窗口预算分配
///
/// 基于 128K context window 的精细预算设计：
/// - system_prompt_reserve：系统提示词预留（含 framework + character + style）
/// - output_reserve：LLM 输出预留
/// - safety_margin：安全余量（避免估算误差导致超限）
/// - available：可用 token 预算 = total - system - output - safety
pub struct ContextWindowBudget {
    pub total_tokens: usize,
    pub system_prompt_reserve: usize,
    pub output_reserve: usize,
    pub safety_margin: usize,
}

impl ContextWindowBudget {
    /// 可用于对话消息（含工具调用历史）的 token 预算
    pub const fn available_tokens(&self) -> usize {
        self.total_tokens - self.system_prompt_reserve - self.output_reserve - self.safety_margin
    }
}

/// 单条工具结果预截断（入队前调用）
///
/// 在工具结果 push 到 `current_messages` 之前先做一次预截断，
/// 避免巨大的工具输出（如 read_file 读取大文件、search_files 返回大量结果）
/// 直接撑爆上下文窗口。
///
/// 与 `soft_trim_tool_results` 的区别：
/// - `soft_trim_tool_results` 在压缩流水线内执行，作用于已有消息
/// - `truncate_tool_result` 在入队前执行，作用于原始字符串
///
/// 截断策略：保留头尾各 `MAX_TOOL_RESULT_CHARS / 2` 字符，中间用截断标记替代。
/// 返回截断后的字符串；若未超限则原样返回。
pub fn truncate_tool_result(content: &str) -> String {
    truncate_tool_result_with_limit(content, MAX_TOOL_RESULT_CHARS)
}

/// 带自定义阈值的单条工具结果预截断
///
/// `max_chars` 为最大字符数，超过则截断为头尾各半 + 中间标记。
pub fn truncate_tool_result_with_limit(content: &str, max_chars: usize) -> String {
    let char_count = content.chars().count();
    if char_count <= max_chars {
        return content.to_string();
    }

    let half = max_chars / 2;
    let head: String = content.chars().take(half).collect();
    let tail: String = content
        .chars()
        .rev()
        .take(half)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let skipped = char_count.saturating_sub(half * 2);

    format!(
        "{}\n…[截断 {} 字符]…\n{}",
        head, skipped, tail
    )
}

/// 消息分组：用于保证 tool_call + tool_result 原子性
#[derive(Debug)]
enum MessageGroup {
    /// 非工具消息，可独立截断或丢弃
    Single(ChatMessage),
    /// assistant tool_call + 所有对应 tool_result，原子保留/丢弃
    ToolBundle {
        assistant: ChatMessage,
        tool_results: Vec<ChatMessage>,
    },
}

impl MessageGroup {
    fn messages(&self) -> Vec<&ChatMessage> {
        match self {
            MessageGroup::Single(m) => vec![m],
            MessageGroup::ToolBundle {
                assistant,
                tool_results,
            } => {
                let mut v = vec![assistant];
                v.extend(tool_results.iter());
                v
            }
        }
    }

    fn token_count(&self) -> usize {
        match self {
            MessageGroup::Single(m) => estimate_message_tokens(m),
            MessageGroup::ToolBundle {
                assistant,
                tool_results,
            } => {
                let mut total = estimate_message_tokens(assistant);
                for r in tool_results {
                    total += estimate_message_tokens(r);
                }
                total
            }
        }
    }

    /// 工具名列表（仅 ToolBundle 有效，用于摘要生成）
    fn tool_names(&self) -> Vec<String> {
        match self {
            MessageGroup::Single(_) => vec![],
            MessageGroup::ToolBundle { assistant, .. } => assistant
                .tool_calls
                .as_ref()
                .map(|tcs| tcs.iter().map(|tc| tc.name.clone()).collect())
                .unwrap_or_default(),
        }
    }
}

/// 截断单条消息的 content 到指定字符数，并追加压缩标记
fn truncate_message(msg: &mut ChatMessage) {
    let chars: Vec<char> = msg.content.chars().collect();
    if chars.len() <= PREVIEW_CHARS {
        return;
    }
    let preview: String = chars.into_iter().take(PREVIEW_CHARS).collect();
    msg.content = format!("{}…[已压缩]", preview);
    // 压缩后清理 reasoning（推理链对历史回顾意义有限，舍弃可省 token）
    msg.reasoning = None;
}

/// Soft Trim：截断过大的 tool_result 内容（Level 1）
///
/// 扫描所有 role=tool 消息，若 content 长度超过
/// `SOFT_TRIM_HEAD + SOFT_TRIM_TAIL + SOFT_TRIM_OVERHEAD`，
/// 截断为 `"{head}\n…[截断 N 字符]…\n{tail}"` 格式。
///
/// 返回被 trim 的消息数量。
pub fn soft_trim_tool_results(messages: &mut [ChatMessage]) -> usize {
    let threshold = SOFT_TRIM_HEAD + SOFT_TRIM_TAIL + SOFT_TRIM_OVERHEAD;
    let mut trimmed = 0;

    for msg in messages.iter_mut() {
        if msg.role != "tool" {
            continue;
        }
        let char_count = msg.content.chars().count();
        if char_count <= threshold {
            continue;
        }

        let head: String = msg.content.chars().take(SOFT_TRIM_HEAD).collect();
        let tail: String = msg
            .content
            .chars()
            .rev()
            .take(SOFT_TRIM_TAIL)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let skipped = char_count - SOFT_TRIM_HEAD - SOFT_TRIM_TAIL;

        msg.content = format!(
            "{}\n…[截断 {} 字符]…\n{}",
            head, skipped, tail
        );
        trimmed += 1;
    }

    trimmed
}

/// 将消息列表的中段 [start..end) 分组为 MessageGroup
///
/// - assistant 消息（含 tool_calls）+ 后续所有对应 tool 消息 → ToolBundle
/// - 其他消息 → Single
///
/// `consumed` 集合用于标记已归入 ToolBundle 的 tool 消息索引，避免重复。
fn build_mid_groups(
    messages: &[ChatMessage],
    start: usize,
    end: usize,
) -> (Vec<MessageGroup>, std::collections::HashSet<usize>) {
    let mut groups: Vec<MessageGroup> = Vec::new();
    let mut consumed: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for i in start..end {
        if consumed.contains(&i) {
            continue;
        }

        let msg = &messages[i];
        if let Some(tool_calls) = &msg.tool_calls {
            // assistant 消息：收集后续对应的 tool_result
            let mut results = Vec::new();
            let call_ids: std::collections::HashSet<String> =
                tool_calls.iter().map(|tc| tc.id.clone()).collect();

            for j in (i + 1)..messages.len() {
                if let Some(ref tc_id) = messages[j].tool_call_id {
                    if call_ids.contains(tc_id) && !consumed.contains(&j) {
                        consumed.insert(j);
                        results.push(messages[j].clone());
                    }
                }
            }

            groups.push(MessageGroup::ToolBundle {
                assistant: msg.clone(),
                tool_results: results,
            });
        } else {
            groups.push(MessageGroup::Single(msg.clone()));
        }
    }

    (groups, consumed)
}

/// 压缩结果
#[derive(Debug, Clone, Copy, Default)]
pub struct CompressResult {
    /// 节省的 token 估算数
    pub saved_tokens: usize,
    /// 被压缩（丢弃原始内容、保留摘要）的消息组数量
    pub dropped_groups: usize,
}

/// 将单个消息组压缩为简短摘要文本
///
/// 保留角色/工具名 + 内容预览，确保信息不丢失
fn compact_group_to_summary(group: &MessageGroup) -> String {
    match group {
        MessageGroup::Single(m) => {
            let role = match m.role.as_str() {
                "user" => "用户",
                "assistant" => "AI",
                "system" => "系统",
                "tool" => "工具",
                _ => "其他",
            };
            let preview: String = m.content.chars().take(150).collect();
            let ellipsis = if m.content.chars().count() > 150 { "…" } else { "" };
            format!("[{}: {}{}]", role, preview, ellipsis)
        }
        MessageGroup::ToolBundle {
            assistant,
            tool_results,
        } => {
            let tool_names = group.tool_names().join(", ");
            let preview: String = assistant.content.chars().take(100).collect();
            let ellipsis = if assistant.content.chars().count() > 100 {
                "…"
            } else {
                ""
            };
            format!(
                "[工具调用({}) 结果{}条: {}{}]",
                tool_names,
                tool_results.len(),
                preview,
                ellipsis
            )
        }
    }
}

/// 聚合多个丢弃摘要为一条系统消息
///
/// 限制总长度，超长时按首尾保留、中间省略的策略截断
fn build_aggregate_summary(summaries: &[String]) -> ChatMessage {
    let total = summaries.len();
    const MAX_TOTAL_CHARS: usize = 2000;
    const PER_SUMMARY_LIMIT: usize = 200;

    let truncated: Vec<String> = summaries
        .iter()
        .map(|s| {
            if s.chars().count() > PER_SUMMARY_LIMIT {
                format!("{}…", s.chars().take(PER_SUMMARY_LIMIT).collect::<String>())
            } else {
                s.clone()
            }
        })
        .collect();

    let total_chars: usize = truncated.iter().map(|s| s.chars().count()).sum();
    if total_chars <= MAX_TOTAL_CHARS {
        return ChatMessage::system(format!(
            "[上下文已压缩 {} 段历史]\n{}",
            total,
            truncated.join("\n")
        ));
    }

    // 超限时：保留首尾各 N 条，中间省略
    let head_count = 3.min(truncated.len() / 2);
    let tail_count = head_count;
    let head: Vec<&String> = truncated.iter().take(head_count).collect();
    let tail: Vec<&String> = truncated.iter().rev().take(tail_count).rev().collect();
    let omitted = total - head_count - tail_count;
    ChatMessage::system(format!(
        "[上下文已压缩 {} 段历史]\n{}\n…(省略 {} 段)…\n{}",
        total,
        head.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"),
        omitted,
        tail.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"),
    ))
}

/// 窗口级压缩：在 FC 循环每轮调用之间治理对话消息总 token 数
///
/// - `messages`：当前对话消息列表（in-place 修改）
/// - `threshold_tokens`：总 token 估算值阈值，超过即触发压缩
/// - `keep_recent`：保留的最近消息条数（不截断）
///
/// 返回 [`CompressResult`]，包含节省的 token 数和被压缩的组数。
///
/// 压缩流水线：
/// 1. **Level 1 Soft Trim**：截断过大的 tool_result 内容（head 1500 + tail 1500 字符）
/// 2. 若总 token ≤ threshold，返回
/// 3. **Level 2 Group Drop**：
///    - 将中段消息分组（tool_call + tool_result 捆绑为原子组）
///    - 先对每组内消息做截断（PREVIEW_CHARS）
///    - 从最旧组开始逐组移除，**所有被移除的组都压缩为摘要**，不丢弃
///    - 聚合摘要作为一条系统消息插入头部
pub fn compress_conversation(
    messages: &mut Vec<ChatMessage>,
    threshold_tokens: usize,
    keep_recent: usize,
) -> CompressResult {
    // === Level 1: Soft Trim（每轮都执行） ===
    soft_trim_tool_results(messages);

    let before = estimate_messages_tokens(messages);
    if before <= threshold_tokens {
        return CompressResult::default();
    }

    let len = messages.len();
    let mid_end = len.saturating_sub(keep_recent);
    if mid_end <= 1 {
        return CompressResult::default();
    }

    // === Level 2: 分组中段消息 ===
    let (mut mid_groups, _) = build_mid_groups(messages, 1, mid_end);

    // 对组内消息做截断（减少单条消息 token）
    for group in &mut mid_groups {
        match group {
            MessageGroup::Single(m) => truncate_message(m),
            MessageGroup::ToolBundle {
                assistant,
                tool_results,
            } => {
                truncate_message(assistant);
                for r in tool_results.iter_mut() {
                    truncate_message(r);
                }
            }
        }
    }

    // 计算 head + tail 的 token
    let head_tail_tokens: usize = messages[..1]
        .iter()
        .chain(messages[mid_end..].iter())
        .map(|m| estimate_message_tokens(m))
        .sum();

    // 检查截断后是否达标
    let mid_tokens: usize = mid_groups.iter().map(|g| g.token_count()).sum();
    if head_tail_tokens + mid_tokens <= threshold_tokens {
        // 截断即可，单次重建
        let mut result = Vec::with_capacity(messages.len());
        result.push(messages[0].clone());
        for group in &mid_groups {
            for m in group.messages() {
                result.push(m.clone());
            }
        }
        result.extend(messages[mid_end..].iter().cloned());
        let after = estimate_messages_tokens(&result);
        *messages = result;
        return CompressResult {
            saved_tokens: before.saturating_sub(after),
            dropped_groups: 0,
        };
    }

    // 需要从最旧组开始逐组移除，所有移除的组压缩为摘要
    let mut kept_groups: Vec<MessageGroup> = mid_groups.into_iter().collect();
    let mut dropped_summaries: Vec<String> = Vec::new();

    while !kept_groups.is_empty() {
        let remaining_mid: usize = kept_groups.iter().map(|g| g.token_count()).sum();
        if head_tail_tokens + remaining_mid <= threshold_tokens {
            break;
        }
        let dropped = kept_groups.remove(0);
        dropped_summaries.push(compact_group_to_summary(&dropped));
    }

    let dropped_count = dropped_summaries.len();

    // 单次重建：head + 聚合摘要 + kept_groups + tail
    let mut result = Vec::with_capacity(2 + kept_groups.len() * 3 + keep_recent);
    result.push(messages[0].clone());
    if !dropped_summaries.is_empty() {
        result.push(build_aggregate_summary(&dropped_summaries));
    }
    for group in &kept_groups {
        for m in group.messages() {
            result.push(m.clone());
        }
    }
    result.extend(messages[mid_end..].iter().cloned());
    let after = estimate_messages_tokens(&result);
    *messages = result;

    CompressResult {
        saved_tokens: before.saturating_sub(after),
        dropped_groups: dropped_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::response::MessageToolCall;

    fn user(content: &str) -> ChatMessage {
        ChatMessage::user(content)
    }

    fn system(content: &str) -> ChatMessage {
        ChatMessage::system(content)
    }

    fn assistant_with_tools(content: &str, calls: Vec<(&str, &str)>) -> ChatMessage {
        let tool_calls = calls
            .into_iter()
            .map(|(id, name)| MessageToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: serde_json::json!({}),
            })
            .collect();
        ChatMessage::assistant_with_tool_calls(content, tool_calls)
    }

    fn tool_result(id: &str, content: &str) -> ChatMessage {
        ChatMessage::tool_result(content, id)
    }

    // === Basic tests ===

    #[test]
    fn no_compress_under_threshold() {
        let mut msgs = vec![user("hello"), user("world")];
        let result = compress_conversation(&mut msgs, 100000, 2);
        assert_eq!(result.saved_tokens, 0);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn truncate_middle_messages() {
        let long: String = "x".repeat(1000);
        let mut msgs = vec![
            system("system_prompt"),
            user(&long),
            user(&long),
            user("recent1"),
            user("recent2"),
        ];
        // 1000-char strings ≈ 250 tokens each; total ≈ 530 tokens
        let result = compress_conversation(&mut msgs, 300, 2);
        assert!(result.saved_tokens > 0);
        assert_eq!(msgs.first().unwrap().content, "system_prompt");
        assert_eq!(msgs.last().unwrap().content, "recent2");
        assert!(msgs[1].content.contains("[已压缩]"));
    }

    #[test]
    fn drop_oldest_when_still_over() {
        let long: String = "y".repeat(2000);
        let mut msgs = vec![
            system("sys"),
            user(&long),
            user(&long),
            user(&long),
            user("recent"),
        ];
        let before_len = msgs.len();
        let result = compress_conversation(&mut msgs, 200, 1);
        assert!(result.saved_tokens > 0);
        assert!(msgs.len() < before_len);
        assert_eq!(msgs.first().unwrap().content, "sys");
        assert_eq!(msgs.last().unwrap().content, "recent");
    }

    // === Tool call pair safety tests ===

    #[test]
    fn tool_call_result_not_split() {
        // assistant tool_call + tool_result 必须作为原子组保留或丢弃
        let long_result = "z".repeat(2000);
        let mut msgs = vec![
            system("sys"),
            assistant_with_tools("calling tool", vec![("c1", "read_file")]),
            tool_result("c1", &long_result),
            assistant_with_tools("calling tool 2", vec![("c2", "read_file")]),
            tool_result("c2", &long_result),
            user("recent"),
        ];

        let result = compress_conversation(&mut msgs, 300, 1);
        assert!(result.saved_tokens > 0);

        // 验证：不应存在孤立的 tool_call（无对应 tool_result）
        let call_ids: std::collections::HashSet<String> = msgs
            .iter()
            .filter_map(|m| {
                m.tool_calls
                    .as_ref()
                    .map(|tcs| tcs.iter().map(|tc| tc.id.clone()).collect::<Vec<_>>())
            })
            .flatten()
            .collect();
        let result_ids: std::collections::HashSet<String> = msgs
            .iter()
            .filter_map(|m| m.tool_call_id.clone())
            .collect();
        assert!(
            call_ids.is_subset(&result_ids),
            "tool_call/result 配对被拆分！orphaned calls: {:?}",
            call_ids.difference(&result_ids).collect::<Vec<_>>()
        );
    }

    #[test]
    fn tool_bundle_dropped_together() {
        // 整个 ToolBundle 一起丢弃
        let big = "a".repeat(3000);
        let mut msgs = vec![
            system("sys"),
            assistant_with_tools("call1", vec![("c1", "write_file")]),
            tool_result("c1", &big),
            user("mid conversation"),
            user("recent"),
        ];

        compress_conversation(&mut msgs, 150, 1);

        // 如果 c1 的 assistant 被丢弃，其 tool_result 也必须被丢弃
        let has_c1_call = msgs.iter().any(|m| {
            m.tool_calls
                .as_ref()
                .map(|tcs| tcs.iter().any(|tc| tc.id == "c1"))
                .unwrap_or(false)
        });
        let has_c1_result = msgs
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("c1"));
        assert_eq!(
            has_c1_call, has_c1_result,
            "tool_call 和 tool_result 必须同时存在或同时丢弃"
        );
    }

    #[test]
    fn stateful_tool_drop_adds_summary() {
        // 丢弃 ToolBundle 时，应生成包含工具名的压缩摘要
        let big = "b".repeat(3000);
        let mut msgs = vec![
            system("sys"),
            assistant_with_tools("writing", vec![("c1", "write_file")]),
            tool_result("c1", &big),
            user("next question"),
            user("recent"),
        ];

        let result = compress_conversation(&mut msgs, 100, 1);

        // 只有当 ToolBundle 确实被丢弃时才检查摘要
        let bundle_dropped = !msgs.iter().any(|m| {
            m.tool_calls
                .as_ref()
                .map(|tcs| tcs.iter().any(|tc| tc.id == "c1"))
                .unwrap_or(false)
        });
        if bundle_dropped {
            assert!(
                result.dropped_groups > 0,
                "应记录被压缩的组数"
            );
            let has_summary = msgs
                .iter()
                .any(|m| m.content.contains("write_file") && m.content.contains("上下文已压缩"));
            assert!(
                has_summary,
                "丢弃 ToolBundle 时应生成包含工具名的压缩摘要"
            );
        }
    }

    // === Soft Trim tests ===

    #[test]
    fn soft_trim_large_tool_result() {
        let huge = "x".repeat(5000); // > 1500 + 1500 + 50 = 3050
        let mut msgs = vec![tool_result("c1", &huge)];
        let trimmed = soft_trim_tool_results(&mut msgs);
        assert_eq!(trimmed, 1);
        assert!(msgs[0].content.contains("截断"));
        assert!(msgs[0].content.len() < huge.len());
    }

    #[test]
    fn soft_trim_skips_small_results() {
        let small = "ok result".to_string();
        let mut msgs = vec![tool_result("c1", &small)];
        let trimmed = soft_trim_tool_results(&mut msgs);
        assert_eq!(trimmed, 0);
        assert_eq!(msgs[0].content, "ok result");
    }

    #[test]
    fn soft_trim_ignores_non_tool_messages() {
        let long = "y".repeat(5000);
        let mut msgs = vec![user(&long), system(&long)];
        let trimmed = soft_trim_tool_results(&mut msgs);
        assert_eq!(trimmed, 0);
    }

    #[test]
    fn no_compress_no_mid_section() {
        let mut msgs = vec![system("sys"), user("recent")];
        let result = compress_conversation(&mut msgs, 10, 2);
        assert_eq!(result.saved_tokens, 0);
    }

    // === Pre-truncation tests ===

    #[test]
    fn truncate_tool_result_under_limit() {
        let content = "short result";
        let truncated = truncate_tool_result(content);
        assert_eq!(truncated, content);
    }

    #[test]
    fn truncate_tool_result_over_limit() {
        let huge = "x".repeat(20000); // > MAX_TOOL_RESULT_CHARS (12000)
        let truncated = truncate_tool_result(&huge);
        assert!(truncated.contains("截断"));
        assert!(truncated.len() < huge.len());
        // 头尾各 6000 字符 + 截断标记
        assert!(truncated.starts_with("xxxx"));
        assert!(truncated.ends_with("xxxx"));
    }

    #[test]
    fn truncate_tool_result_chinese() {
        let huge = "中".repeat(20000);
        let truncated = truncate_tool_result(&huge);
        assert!(truncated.contains("截断"));
        assert!(truncated.chars().count() < 20000);
    }

    #[test]
    fn truncate_tool_result_custom_limit() {
        let content = "abcdefghij".repeat(100); // 1000 chars
        let truncated = truncate_tool_result_with_limit(&content, 200);
        assert!(truncated.contains("截断"));
        // 头尾各 100 字符
        assert!(truncated.starts_with("abcdefghij"));
        assert!(truncated.ends_with("abcdefghij"));
    }

    #[test]
    fn context_window_budget_available() {
        // 128K - 6K - 4K - 4K = 114K
        assert_eq!(CONTEXT_WINDOW_BUDGET.available_tokens(), 114_000);
    }
}
