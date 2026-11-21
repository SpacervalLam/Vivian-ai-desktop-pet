//! 相对时间锚点：检测记忆原文中的相对时间词并追加"以记忆产生时为准"标注。
//!
//! 记忆原文中"今天/昨天/明天/上周"等词在检索时会产生时间漂移——
//! 用户问"我昨天说了什么"时，检索到的记忆里"昨天"指的是记忆产生时的昨天，
//! 而非当前时刻的昨天。本模块在记忆返回前自动追加锚点标注。

use chrono::{Local, TimeZone};

use super::types::MemoryItem;

/// 相对时间词列表（中文 + 英文常见表达）。
const RELATIVE_TIME_WORDS: &[&str] = &[
    "今天", "昨天", "明天", "后天", "前天",
    "上周", "本周", "下周", "上个月", "下个月",
    "去年", "今年", "明年",
    "刚才", "之前", "之后",
    "今天早上", "今天下午", "今天晚上",
    "昨天早上", "昨天下午", "昨天晚上",
    "tomorrow", "yesterday", "today",
    "last week", "next week", "last month", "next month",
];

/// 检测文本中是否包含相对时间词。
pub fn has_relative_time_word(text: &str) -> bool {
    let lower = text.to_lowercase();
    RELATIVE_TIME_WORDS.iter().any(|w| lower.contains(&w.to_lowercase()))
}

/// 为记忆内容追加相对时间锚点标注。
///
/// 如果记忆原文包含相对时间词，且记忆有 timestamp，则追加：
/// ` （以记忆产生时为准：YYYY-MM-DD）`
///
/// 已包含锚点标注的记忆不会重复追加。
pub fn anchor_memory_content(m: &MemoryItem) -> String {
    if !has_relative_time_word(&m.content) {
        return m.content.clone();
    }

    // 已有锚点标注则不重复追加
    if m.content.contains("以记忆产生时为准") {
        return m.content.clone();
    }

    let dt = Local
        .timestamp_opt(m.timestamp as i64, 0)
        .single()
        .unwrap_or_else(Local::now);
    let date_str = dt.format("%Y-%m-%d").to_string();
    format!("{} （以记忆产生时为准：{}）", m.content, date_str)
}

/// 批量追加时间锚点，返回新的内容列表（与输入记忆一一对应）。
pub fn anchor_memories(memories: &[MemoryItem]) -> Vec<String> {
    memories.iter().map(anchor_memory_content).collect()
}

/// 为记忆列表追加时间锚点，返回修改后的记忆列表（content 已更新）。
pub fn apply_time_anchor(mut memories: Vec<MemoryItem>) -> Vec<MemoryItem> {
    for m in &mut memories {
        let anchored = anchor_memory_content(m);
        if anchored != m.content {
            m.content = anchored;
        }
    }
    memories
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{current_timestamp, Granularity, MemoryItem};

    fn make_item(content: &str) -> MemoryItem {
        MemoryItem::new(content.to_string(), Granularity::Turn, 0.5)
    }

    #[test]
    fn test_detect_relative_time() {
        assert!(has_relative_time_word("我昨天去看了电影"));
        assert!(has_relative_time_word("上周我们讨论过这个"));
        assert!(!has_relative_time_word("我去了电影院"));
    }

    #[test]
    fn test_anchor_appended() {
        let m = make_item("我昨天说想吃火锅");
        let anchored = anchor_memory_content(&m);
        assert!(anchored.contains("以记忆产生时为准"));
        assert!(anchored.contains("我昨天说想吃火锅"));
    }

    #[test]
    fn test_no_anchor_for_absolute_time() {
        let m = make_item("我2024年1月去了北京");
        let anchored = anchor_memory_content(&m);
        assert!(!anchored.contains("以记忆产生时为准"));
    }

    #[test]
    fn test_no_duplicate_anchor() {
        let mut m = make_item("我昨天说想吃火锅 （以记忆产生时为准：2024-01-01）");
        m.timestamp = current_timestamp();
        let anchored = anchor_memory_content(&m);
        // 已有锚点不应重复追加
        assert!(!anchored.contains("以记忆产生时为准：2024-01-01） （以记忆产生时为准"));
    }

    #[test]
    fn test_apply_to_batch() {
        let memories = vec![
            make_item("今天天气不错"),
            make_item("我去了公园"),
        ];
        let result = apply_time_anchor(memories);
        assert!(result[0].content.contains("以记忆产生时为准"));
        assert!(!result[1].content.contains("以记忆产生时为准"));
    }
}
