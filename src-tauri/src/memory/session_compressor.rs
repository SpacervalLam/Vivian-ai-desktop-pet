//! 会话记忆压缩层
//!
//! DialogueManager 维护一个固定窗口，超出部分被静默丢弃。
//! TimeStampedMemory 已有 LLM 摘要能力，但摘要仅注入工具上下文，不进入主对话窗口。
//!
//! 本模块桥接两者：从 TimeStampedMemory 提取摘要，注入为会话回顾消息，
//! 让 LLM 在近期消息之外也能感知"之前聊过什么"，并明确这段时间并非刚刚发生。

use chrono::{DateTime, Local};
use crate::memory::time_stamped::TimeStampedMemory;
use crate::pipeline::prompt_modules::normalize_lang;
use crate::types::response::ChatMessage;
use crate::utils::prompt_time::{format_age_seconds, format_local_time};

/// 从 TimeStampedMemory 提取摘要并构造会话回顾消息（使用当前时间与全局语言）。
pub fn build_conversation_recap(tsm: &TimeStampedMemory) -> Option<ChatMessage> {
    build_conversation_recap_at(tsm, Local::now(), &crate::i18n::get_language())
}

/// 显式时间/语言版本，供测试和主动路径复用。
pub fn build_conversation_recap_at(
    tsm: &TimeStampedMemory,
    now: DateTime<Local>,
    lang: &str,
) -> Option<ChatMessage> {
    let summary = tsm.get_summaries().last()?;
    let body = summary.content.trim();
    if body.is_empty() {
        return None;
    }
    let lang = normalize_lang(lang);
    let start = summary.start_time;
    let end = summary.end_time;
    let age = format_age_seconds((now - end).num_seconds().max(0) as f64, lang);
    let range = match lang {
        "en" => format!("{} ~ {} ({age}, before the recent window)", format_local_time(start), format_local_time(end)),
        "ja" => format!("{} ~ {}（{age}、直近ウィンドウより前）", format_local_time(start), format_local_time(end)),
        _ => format!("{} ~ {}（{age}，位于近期窗口之前）", format_local_time(start), format_local_time(end)),
    };
    let text = match lang {
        "en" => format!(
            "[CONVERSATION RECAP — DATA]\nThis recap covers an earlier conversation segment, not the current turn: {range}.\n{body}\n[END RECAP]\nThis is your own fallible recap. Continue a still-relevant thread naturally, but do not treat old details as something that just happened or recite the recap line by line."
        ),
        "ja" => format!(
            "[CONVERSATION RECAP — DATA]\nこの振り返りは現在のターンではなく、以前の会話区間を対象とする：{range}。\n{body}\n[END RECAP]\nこれはあなた自身の誤りうる振り返り。今も関係する話題だけ自然に続け、古い内容を「たった今」の出来事として扱わず、逐条暗唱しないこと。"
        ),
        _ => format!(
            "[CONVERSATION RECAP — DATA]\n这段回顾覆盖的是更早的对话区段，不是当前这一轮：{range}。\n{body}\n[END RECAP]\n这是你自己可能记错的回顾。只在话题仍然相关时自然延续，不要把旧内容当成刚刚发生的事，也不要逐条复诵。"
        ),
    };
    Some(ChatMessage::system(text))
}

/// 将会话回顾消息注入到消息列表头部（使用当前时间与全局语言）。
pub fn inject_recap_if_available(
    messages: &mut Vec<ChatMessage>,
    tsm: &TimeStampedMemory,
) {
    inject_recap_if_available_at(messages, tsm, Local::now(), &crate::i18n::get_language());
}

/// 显式时间/语言版本。
pub fn inject_recap_if_available_at(
    messages: &mut Vec<ChatMessage>,
    tsm: &TimeStampedMemory,
    now: DateTime<Local>,
    lang: &str,
) {
    if let Some(recap) = build_conversation_recap_at(tsm, now, lang) {
        messages.insert(0, recap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recap_header(start: DateTime<Local>, end: DateTime<Local>, now: DateTime<Local>, lang: &str) -> String {
        let age = format_age_seconds((now - end).num_seconds().max(0) as f64, normalize_lang(lang));
        format!("{} ~ {}（{}", format_local_time(start), format_local_time(end), age)
    }

    #[test]
    fn recap_range_keeps_old_segment_separate_from_current_turn() {
        let now = Local::now();
        let start = now - chrono::Duration::hours(4);
        let end = now - chrono::Duration::hours(3);
        for lang in ["zh", "en", "ja"] {
            let text = build_conversation_recap_at(
                &TimeStampedMemory::with_test_summary("old topic", start, end),
                now,
                lang,
            ).unwrap().content;
            assert!(text.contains("之前") || text.contains("earlier") || text.contains("以前") || text.contains("以前の"), "{lang}: {text}");
            assert!(recap_header(start, end, now, lang).contains("3"), "{lang}");
            assert!(!text.contains("刚刚发生的事。"), "{lang}: {text}");
        }
    }
}
