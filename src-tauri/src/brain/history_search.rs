//! 会话原文检索的纯逻辑：匹配、上下文内外标记、分页与摘录。
//!
//! 与存储和工具边界解耦，便于单独验证；工具层只负责取消息与渲染。

/// 一条可检索消息的最小投影。工具结果额外带上工具名与参数，
/// 这样「当时跑的是什么命令」也能被搜到，而不只是输出正文。
pub struct MsgText<'a> {
    pub content: &'a str,
    pub tool: Option<&'a str>,
    pub arguments: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryHit {
    pub index: usize,
    /// 该消息已压缩出当前上下文（索引在 `context_start` 之前）。
    pub compacted: bool,
    pub tool: Option<String>,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPage {
    pub total: usize,
    pub hits: Vec<HistoryHit>,
    pub next_offset: Option<usize>,
}

/// 截取命中位置附近的一段文本。
pub fn snippet(text: &str, needle_lower: &str, radius: usize) -> String {
    let lower = text.to_lowercase();
    let Some(pos) = lower.find(needle_lower) else {
        return text.chars().take(radius * 2).collect();
    };
    let mut start = pos.saturating_sub(radius).min(text.len());
    let mut end = (pos + needle_lower.len() + radius).min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&text[start..end]);
    if end < text.len() {
        out.push('…');
    }
    out.replace('\n', " ")
}

/// 关键词检索：正文与工具参数都参与匹配，正文优先作为摘录来源。
pub fn search(
    messages: &[MsgText<'_>],
    context_start: usize,
    query: &str,
    offset: usize,
    limit: usize,
) -> HistoryPage {
    let needle = query.trim().to_lowercase();
    let limit = limit.max(1);
    let mut hits = Vec::new();
    let mut total = 0usize;
    if needle.is_empty() {
        return HistoryPage { total: 0, hits, next_offset: None };
    }
    for (index, message) in messages.iter().enumerate() {
        let in_content = message.content.to_lowercase().contains(&needle);
        if !in_content && !message.arguments.to_lowercase().contains(&needle) {
            continue;
        }
        total += 1;
        if total <= offset || hits.len() >= limit {
            continue;
        }
        let source = if in_content { message.content } else { message.arguments };
        hits.push(HistoryHit {
            index,
            compacted: index < context_start,
            tool: message.tool.map(str::to_string),
            excerpt: snippet(source, &needle, 140),
        });
    }
    let next_offset = (offset + hits.len() < total).then_some(offset + hits.len());
    HistoryPage { total, hits, next_offset }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages() -> Vec<MsgText<'static>> {
        vec![
            MsgText { content: "决定使用 SQLite 作为会话存储", tool: None, arguments: "" },
            MsgText { content: "已读取配置", tool: Some("read_file"), arguments: "{\"path\":\"config.toml\"}" },
            MsgText { content: "测试通过", tool: Some("run_command"), arguments: "{\"command\":\"cargo test\"}" },
            MsgText { content: "没有相关内容的回复", tool: None, arguments: "" },
        ]
    }

    #[test]
    fn marks_messages_outside_the_current_context() {
        let page = search(&messages(), 2, "sqlite", 0, 10);
        assert_eq!(page.total, 1);
        assert!(page.hits[0].compacted, "#0 在 context_start=2 之前");
        assert_eq!(page.hits[0].index, 0);
        assert!(page.hits[0].excerpt.contains("SQLite"));
    }

    #[test]
    fn matches_tool_arguments_and_reports_the_tool_name() {
        let page = search(&messages(), 0, "cargo test", 0, 10);
        assert_eq!(page.total, 1);
        assert_eq!(page.hits[0].index, 2);
        assert_eq!(page.hits[0].tool.as_deref(), Some("run_command"));
        assert!(page.hits[0].excerpt.contains("cargo test"));
        assert!(!page.hits[0].compacted);
    }

    #[test]
    fn paging_reports_total_and_next_offset() {
        let list: Vec<MsgText<'static>> = (0..5)
            .map(|_| MsgText { content: "同一条命中文本", tool: None, arguments: "" })
            .collect();
        let first = search(&list, 0, "命中", 0, 2);
        assert_eq!(first.total, 5);
        assert_eq!(first.hits.len(), 2);
        assert_eq!(first.next_offset, Some(2));
        let last = search(&list, 0, "命中", 4, 2);
        assert_eq!(last.hits.len(), 1);
        assert_eq!(last.next_offset, None);
        assert_eq!(search(&list, 0, "", 0, 2).total, 0, "空关键词不匹配任何消息");
    }

    #[test]
    fn offset_skips_earlier_hits_without_losing_the_total() {
        let list: Vec<MsgText<'static>> = (0..3)
            .map(|_| MsgText { content: "命中", tool: None, arguments: "" })
            .collect();
        let page = search(&list, 0, "命中", 1, 1);
        assert_eq!(page.total, 3);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].index, 1);
        assert_eq!(page.next_offset, Some(2));
    }

    #[test]
    fn snippet_never_splits_a_multibyte_character() {
        let text = "前缀".repeat(80) + "关键词" + &"后缀".repeat(80);
        let excerpt = snippet(&text, "关键词", 5);
        assert!(excerpt.contains("关键词"));
        assert!(excerpt.starts_with('…') && excerpt.ends_with('…'));
    }
}
