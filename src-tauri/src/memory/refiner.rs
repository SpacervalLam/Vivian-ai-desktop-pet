//! 记忆候选 LLM 精选层。
//!
//! 在 hybrid_search 召回大量候选后，先用关键词粗筛减少数量，
//! 再用 LLM 从中挑选最相关的 N 条，降低注入 prompt 的噪声。
//!
//! 两阶段设计：
//! 1. keyword_prefilter：基于 query 词项命中数粗筛，保留 top_k
//! 2. llm_refine：LLM 从粗筛结果中按相关性挑选 target_n 条

use std::sync::Arc;

use serde::Deserialize;

use crate::error::VivianResult;
use crate::providers::base::LLMRequest;
use crate::providers::router::ModelRouter;
use crate::types::response::ChatMessage;

use super::tokenize::tokenize;
use super::types::MemoryItem;

/// 关键词粗筛：按 query token 在 content/tags 中的命中数排序，取 top_k
pub fn keyword_prefilter(candidates: &[MemoryItem], query: &str, top_k: usize) -> Vec<MemoryItem> {
    if candidates.len() <= top_k {
        return candidates.to_vec();
    }
    let query_tokens: Vec<String> = tokenize(query)
        .into_iter()
        .filter(|t| t.len() > 1)
        .collect();
    if query_tokens.is_empty() {
        return candidates.iter().take(top_k).cloned().collect();
    }

    let mut scored: Vec<(f64, MemoryItem)> = candidates
        .iter()
        .map(|m| {
            let content_lower = m.content.to_lowercase();
            let tags_lower: Vec<String> = m.tags.iter().map(|t| t.to_lowercase()).collect();
            let hits = query_tokens
                .iter()
                .filter(|t| {
                    let tl = t.to_lowercase();
                    content_lower.contains(&tl) || tags_lower.iter().any(|tag| tag.contains(&tl))
                })
                .count();
            let score = hits as f64 + m.importance * 0.1;
            (score, m.clone())
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(top_k).map(|(_, m)| m).collect()
}

#[derive(Debug, Deserialize)]
struct LlmRefineResult {
    /// 选中的记忆 ID 列表
    selected: Vec<String>,
}

/// LLM 精选：从候选中挑选 target_n 条最相关的
///
/// 返回选中的 MemoryItem 列表（保持候选顺序）。
pub async fn llm_refine(
    candidates: &[MemoryItem],
    query: &str,
    router: &ModelRouter,
    char_id: &str,
    target_n: usize,
) -> VivianResult<Vec<MemoryItem>> {
    if candidates.len() <= target_n {
        return Ok(candidates.to_vec());
    }

    let system = ChatMessage::system(
        "你是记忆筛选助手。从候选记忆中挑选与用户查询最相关的若干条。只输出 JSON。",
    );
    let mut candidate_lines = Vec::with_capacity(candidates.len());
    for (idx, m) in candidates.iter().enumerate() {
        let preview = crate::utils::truncate_chars(&m.content, 80);
        candidate_lines.push(format!("[{}] id={} | {}", idx, m.id, preview));
    }
    let user = ChatMessage::user(format!(
        "角色：{}\n用户查询：{}\n候选记忆：\n{}\n\n请挑选与查询最相关的 {} 条，输出 JSON：{{\"selected\": [\"id1\", \"id2\"]}}",
        char_id,
        query,
        candidate_lines.join("\n"),
        target_n
    ));
    let messages = vec![system, user];
    let resp = router.generate(LLMRequest::new("consolidation", messages)).await?;
    let selected_ids = parse_selected_ids(&resp, candidates.len());

    let mut refined: Vec<MemoryItem> = Vec::with_capacity(target_n);
    for id in selected_ids.iter().take(target_n) {
        if let Some(m) = candidates.iter().find(|c| &c.id == id) {
            refined.push(m.clone());
        }
    }
    if refined.is_empty() {
        return Ok(candidates.iter().take(target_n).cloned().collect());
    }
    Ok(refined)
}

/// 解析 LLM 返回的 selected ID 列表
fn parse_selected_ids(text: &str, candidate_count: usize) -> Vec<String> {
    // 优先尝试 JSON 解析
    if let Ok(result) = serde_json::from_str::<LlmRefineResult>(text.trim()) {
        return result.selected;
    }
    // 提取 JSON 对象
    let start = text.find('{');
    let end = text.rfind('}');
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            let json_str = &text[s..=e];
            if let Ok(result) = serde_json::from_str::<LlmRefineResult>(json_str) {
                return result.selected;
            }
        }
    }
    // 降级：返回前 candidate_count 个 ID（实际上限由调用方控制）
    let _ = candidate_count;
    Vec::new()
}

/// 组合入口：先关键词粗筛，再 LLM 精选
///
/// `prefilter_k`：粗筛保留数量（建议 12-20）
/// `target_n`：最终精选数量（建议 3-5）
pub async fn refine_candidates(
    candidates: Vec<MemoryItem>,
    query: &str,
    router: Option<&Arc<ModelRouter>>,
    char_id: &str,
    prefilter_k: usize,
    target_n: usize,
) -> Vec<MemoryItem> {
    if candidates.is_empty() || target_n == 0 {
        return Vec::new();
    }
    if candidates.len() <= target_n {
        return candidates;
    }
    // 阶段 1：关键词粗筛
    let prefiltered = keyword_prefilter(&candidates, query, prefilter_k);
    if prefiltered.len() <= target_n {
        return prefiltered;
    }
    // 阶段 2：LLM 精选
    let Some(router) = router else {
        return prefiltered.into_iter().take(target_n).collect();
    };
    match llm_refine(&prefiltered, query, router, char_id, target_n).await {
        Ok(refined) => refined,
        Err(_) => prefiltered.into_iter().take(target_n).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::Granularity;

    fn make_item(id: &str, content: &str, importance: f64) -> MemoryItem {
        let mut item = MemoryItem::new(content.to_string(), Granularity::Turn, importance);
        item.id = id.to_string();
        item.timestamp = 1000.0;
        item
    }

    #[test]
    fn prefilter_returns_all_when_below_k() {
        let items = vec![make_item("1", "a", 0.5), make_item("2", "b", 0.5)];
        let out = keyword_prefilter(&items, "query", 5);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn prefilter_ranks_by_hits() {
        let items = vec![
            make_item("1", "今天天气真好", 0.5),
            make_item("2", "天气", 0.5),
            make_item("3", "无关内容", 0.5),
        ];
        let out = keyword_prefilter(&items, "天气", 2);
        assert_eq!(out.len(), 2);
        assert!(out[0].content.contains("天气"));
    }

    #[test]
    fn parse_selected_ids_handles_json() {
        let text = r#"{"selected": ["id1", "id2"]}"#;
        let ids = parse_selected_ids(text, 5);
        assert_eq!(ids, vec!["id1", "id2"]);
    }

    #[test]
    fn parse_selected_ids_handles_wrapped_json() {
        let text = "结果：{\"selected\": [\"a\"]} 完成";
        let ids = parse_selected_ids(text, 5);
        assert_eq!(ids, vec!["a"]);
    }

    #[test]
    fn parse_selected_ids_returns_empty_on_invalid() {
        let ids = parse_selected_ids("invalid", 5);
        assert!(ids.is_empty());
    }
}
