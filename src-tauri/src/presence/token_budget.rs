//! Bounded evidence projections for background learning; originals stay in the evidence/corpus stores.
use crate::network::web::WebSearchSource;
use serde_json::json;
use std::collections::HashSet;

pub const LEARNING_INPUT_TOKENS: usize = 4096;
pub const KNOWLEDGE_OUTPUT_TOKENS: u32 = 1600;
pub const TOPIC_OUTPUT_TOKENS: u32 = 256;
pub const SHARE_OUTPUT_TOKENS: u32 = 512;
pub const MEME_OUTPUT_TOKENS: u32 = 1536;

/// Keep the start and end of long material, with an explicit omission marker.
pub fn excerpt(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.into();
    }
    const MARKER: &str = "\n[...excerpt omitted...]\n";
    if max_bytes < MARKER.len() {
        return String::new();
    }
    let available = max_bytes - MARKER.len();
    let mut head = available * 3 / 4;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - (available - head);
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{MARKER}{}", &text[..head], &text[tail..])
}

/// Keep source identities/dates and distinguish read bodies from search-only excerpts.
/// Budget is measured on the serialized payload, including JSON escaping and metadata.
pub fn evidence_context(sources: &[WebSearchSource], max_tokens: usize) -> String {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut clipped = sources.len() > 6;
    for source in sources
        .iter()
        .filter(|s| !s.url.is_empty() && seen.insert(s.url.as_str()))
        .take(6)
    {
        let body = source
            .raw_content
            .as_deref()
            .filter(|text| !text.trim().is_empty());
        let audience = source
            .snippet_text()
            .lines()
            .filter(|line| {
                line.starts_with("观众评论样本（不代表事实）")
                    || line.starts_with("弹幕反应（不代表事实）")
            })
            .collect::<Vec<_>>()
            .join("\n");
        clipped |= source.display_title().len() > 320 || audience.len() > 600;
        let mut item = json!({"url":source.url,"evidence":if body.is_some(){"read_body"}else{"search_excerpt_only"},
            "text":body.unwrap_or_else(||source.snippet_text())});
        if let Some(title) = source.title.as_deref().filter(|title| !title.is_empty()) {
            item["title"] = json!(excerpt(title, 320));
        }
        if let Some(date) = &source.published_at {
            item["published_at"] = json!(date);
        }
        if !source.source_tier.is_empty() {
            item["source_tier"] = json!(source.source_tier);
        }
        if !source.confidence.is_empty() {
            item["confidence"] = json!(source.confidence);
        }
        if !audience.is_empty() {
            item["audience_opinions"] = json!(excerpt(&audience, 600));
        }
        items.push(item);
    }
    let original_tokens = crate::utils::token_estimate::estimate_tokens(
        &serde_json::to_string(&items).unwrap_or_default(),
    );
    let mut payload = json!({"sources":items,"partial":clipped,"untrusted":true});
    let max_bytes = max_tokens.saturating_mul(4);
    // Reduce every body fairly, so a large first source cannot starve all other sources.
    let mut limit =
        (max_bytes / payload["sources"].as_array().map_or(1, |a| a.len().max(1))).max(32);
    loop {
        let serialized = payload.to_string();
        if serialized.len() <= max_bytes {
            tracing::debug!(
                original_tokens,
                projected_tokens = crate::utils::token_estimate::estimate_tokens(&serialized),
                "background learning evidence budget"
            );
            return serialized;
        }
        payload["partial"] = json!(true);
        let Some(items) = payload["sources"].as_array_mut() else {
            return "{}".into();
        };
        if items.is_empty() {
            return if max_bytes >= 2 {
                "{}".into()
            } else {
                String::new()
            };
        }
        if limit >= 32 {
            for item in items {
                for key in ["text", "audience_opinions"] {
                    let Some(text) = item[key].as_str() else {
                        continue;
                    };
                    item[key] = json!(excerpt(text, limit));
                }
            }
            limit = limit * 3 / 4;
        } else {
            // Never cut URLs in half or claim a dropped source was read.
            items.pop();
        }
    }
}

pub fn known_topics(refresh: &[String], hints: &[String], max: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    refresh
        .iter()
        .chain(hints)
        .map(|t| t.trim())
        .filter(|t| !t.is_empty() && seen.insert(t.to_lowercase()))
        .take(max)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn long_evidence_is_bounded_without_mutating_originals_or_cutting_source_urls() {
        let mut source = WebSearchSource::new("https://example.com/original");
        source.raw_content = Some(format!("开头事实{}结尾限制", "正文\n\"引文\"".repeat(4000)));
        let original = source.raw_content.clone();
        let text = evidence_context(&[source.clone(), source.clone()], 1200);
        assert!(crate::utils::token_estimate::estimate_tokens(&text) <= 1200);
        let data: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(data["sources"].as_array().unwrap().len(), 1);
        assert_eq!(data["sources"][0]["url"], source.url);
        assert!(data["sources"][0]["text"]
            .as_str()
            .unwrap()
            .contains("结尾限制"));
        assert_eq!(source.raw_content, original);
        assert_eq!(data["partial"], true);
    }
    #[test]
    fn short_evidence_stays_complete_and_snippets_never_turn_into_reading() {
        let mut source = WebSearchSource::new("https://example.com/short");
        source.snippet = Some("仅有搜索摘要".into());
        let data: Value = serde_json::from_str(&evidence_context(&[source], 4096)).unwrap();
        assert_eq!(data["partial"], false);
        assert_eq!(data["sources"][0]["evidence"], "search_excerpt_only");
        assert_eq!(data["sources"][0]["text"], "仅有搜索摘要");
    }
    #[test]
    fn known_topics_deduplicate_before_deciding_whether_another_model_call_is_needed() {
        assert_eq!(
            known_topics(
                &["Rust".into(), " Rust ".into(), "弹幕".into()],
                &["rust".into(), "小红书".into()],
                3
            ),
            vec!["Rust", "弹幕", "小红书"]
        );
    }
}
