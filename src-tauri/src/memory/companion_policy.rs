//! 陪伴对话的记忆边界。存储类型可以保持兼容，但进入对话提示词的内容必须有明确用途。
use std::collections::HashSet;

use super::types::MemoryItem;

/// 内心活动和机器生成的索引不是用户经历的证据。旧数据库中的这些条目也在召回时隔离。
pub fn is_dialogue_evidence(item: &MemoryItem) -> bool {
    if item.consolidated || item.content.trim().is_empty() {
        return false;
    }
    if matches!(item.memory_type.as_str(), "inner_monologue" | "observation_note") {
        return false;
    }
    !item.tags.iter().any(|tag| {
        matches!(tag.as_str(), "inner_monologue" | "inner_os" | "topic_signal" | "return_recap" | "tool_call")
    }) && !item.metadata.get("topic_signal").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 长期事实抽取的已知事实只来自长期层，避免拿原话、摘要、想法充当已确认事实。
pub fn is_durable_fact(item: &MemoryItem) -> bool {
    is_dialogue_evidence(item)
        && matches!(
            item.memory_type.as_str(),
            "long_term" | "user" | "feedback" | "project" | "preference" | "identity" | "important_event"
        )
}

/// 近端对话由 DialogueManager 提供；旧原话只在近期内允许补充，防止它挤掉长期事实。
pub fn is_recallable(item: &MemoryItem, now: f64) -> bool {
    if !is_dialogue_evidence(item) {
        return false;
    }
    match item.memory_type.as_str() {
        "short_term" | "casual_conversation" | "temporary_context" =>
            now - item.timestamp <= 12.0 * 3600.0,
        _ => true,
    }
}

/// 检索融合后的最后一道去重：不同流水线可能为同一句话保存多个副本。
pub fn dedup_recall(items: Vec<MemoryItem>, now: f64, limit: usize) -> Vec<MemoryItem> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|item| {
        if !is_recallable(item, now) {
            return false;
        }
        let plain = crate::cross_character::parse_any_speaker_prefix(&item.content).0;
        let key: String = plain.chars().filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
            .flat_map(char::to_lowercase).collect();
        !key.is_empty() && seen.insert(key)
    }).take(limit).collect()
}

/// 长对话取首尾而非仅取开头，避免把句末的决定或期限裁掉。
pub fn compact_excerpt(content: &str, max_chars: usize) -> String {
    let chars: Vec<char> = content.chars().collect();
    if chars.len() <= max_chars || max_chars < 16 {
        return chars.into_iter().take(max_chars).collect();
    }
    let head = max_chars * 2 / 3;
    let tail = max_chars - head - 1;
    format!("{}…{}", chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::Granularity;

    #[test]
    fn private_thoughts_and_duplicate_turns_stay_out_of_recall() {
        let now = 1_000_000.0;
        let mut thought = MemoryItem::new("我有点无聊".into(), Granularity::Summary, 0.4);
        thought.memory_type = "inner_monologue".into();
        let mut a = MemoryItem::new("喜欢喝茶".into(), Granularity::Summary, 0.8);
        a.memory_type = "preference".into();
        let mut b = a.clone();
        b.id = "another".into();
        let mut old_turn = MemoryItem::new("昨天聊天".into(), Granularity::Turn, 0.3);
        old_turn.memory_type = "short_term".into();
        old_turn.timestamp = now - 13.0 * 3600.0;
        let result = dedup_recall(vec![thought, a, b, old_turn], now, 5);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "喜欢喝茶");
    }

    #[test]
    fn excerpt_keeps_a_decision_at_the_end() {
        let text = format!("{}明天面试", "前情".repeat(30));
        let excerpt = compact_excerpt(&text, 30);
        assert!(excerpt.chars().count() <= 30);
        assert!(excerpt.ends_with("明天面试"));
    }
}
