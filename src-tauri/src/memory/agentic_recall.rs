//! Agentic recall —— 由 LLM 自己给关键词、并行跨渠道检索记忆。
//!
//! 首轮提示词里的召回依赖「用户消息 + 历史对话」的 embedding，容易漏掉
//! 字面不相似但实际有用的旧信息（日记、笔记、会话摘要、事实等）。
//! 这里的检索词完全由模型在工具调用里给出：它觉得自己的证据不够、或遇到
//! 模糊指代（"上次那个""你说的那件事"）时，才调用 `search_memory` 深入回忆；
//! 不是每轮的固定流程。
//!
//! 设计约束：
//! - 只读、无副作用；调用方（工具层）负责决定何时检索。
//! - 检索走 `RetrievalStrategy::Hybrid`（关键词 + 时效），不生成 query embedding，
//!   与「LLM 提供关键词」的语义一致。
//! - 多查询结果按 id / 内容去重，并过滤不可见与种子人设内容。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::memory::{
    kinds, provenance, MemoryItem, MemoryManager, MemoryRetrievalFilter, RetrievalStrategy,
};
use crate::utils::truncate_chars;

/// 单次工具调用最多并行发起的检索数。再多会明显增加延迟，且收益递减。
pub const MAX_RECALL_QUERIES: usize = 3;

/// 单个检索词的最大字符数，防止模型把整段对话塞进 one query。
const MAX_KEYWORDS_CHARS: usize = 80;

/// 检索渠道。模型只负责选择渠道，具体 filter 由本地映射，避免提示词泄漏 SQL 语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecallScope {
    /// 全渠道
    All,
    /// 日记（`record_kind="diary"` + diary 标签）
    Diary,
    /// 笔记 / 采集资料（knowledge / reference）
    Note,
    /// 会话摘要 / 洞察
    Summary,
    /// 事实与长期偏好
    Fact,
    /// 对话原文
    Dialogue,
}

impl Default for RecallScope {
    fn default() -> Self {
        Self::All
    }
}

impl RecallScope {
    /// 渠道 → 检索过滤器。
    pub fn filter(self) -> MemoryRetrievalFilter {
        match self {
            Self::All => MemoryRetrievalFilter::default(),
            Self::Diary => MemoryRetrievalFilter {
                tags_any: Some(vec!["diary".to_string(), "daily_diary".to_string()]),
                ..Default::default()
            },
            Self::Note => MemoryRetrievalFilter {
                memory_types: Some(vec!["knowledge".to_string(), "reference".to_string()]),
                ..Default::default()
            },
            Self::Summary => MemoryRetrievalFilter {
                memory_types: Some(vec![
                    "session_summary".to_string(),
                    "mid_term".to_string(),
                    "insight".to_string(),
                    "observation_note".to_string(),
                ]),
                ..Default::default()
            },
            Self::Fact => MemoryRetrievalFilter {
                memory_types: Some(vec![
                    "user".to_string(),
                    "preference".to_string(),
                    "identity".to_string(),
                    "important_event".to_string(),
                    "project".to_string(),
                    "feedback".to_string(),
                    "long_term".to_string(),
                    "general".to_string(),
                ]),
                ..Default::default()
            },
            Self::Dialogue => MemoryRetrievalFilter {
                memory_types: Some(vec![
                    "casual_conversation".to_string(),
                    "short_term".to_string(),
                    "temporary_context".to_string(),
                ]),
                ..Default::default()
            },
        }
    }

    /// 解析工具参数里的渠道名；未知值按全渠道处理，避免模型拼写差异让整次检索失败。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "" | "all" => Some(Self::All),
            "diary" | "daily_diary" => Some(Self::Diary),
            "note" | "notes" | "knowledge" => Some(Self::Note),
            "summary" | "summaries" => Some(Self::Summary),
            "fact" | "facts" | "preference" | "preferences" => Some(Self::Fact),
            "dialogue" | "conversation" => Some(Self::Dialogue),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Diary => "diary",
            Self::Note => "note",
            Self::Summary => "summary",
            Self::Fact => "fact",
            Self::Dialogue => "dialogue",
        }
    }
}

/// 一条检索：关键词 + 渠道，均由模型在工具调用中给出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallQuery {
    pub keywords: String,
    pub scope: RecallScope,
}

/// 规范化模型给出的检索：去空白、截断、按「渠道 + 关键词」去重，最多 [`MAX_RECALL_QUERIES`] 条。
pub fn sanitize_queries(queries: Vec<RecallQuery>) -> Vec<RecallQuery> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for query in queries {
        let keywords = query.keywords.trim();
        if keywords.is_empty() {
            continue;
        }
        let keywords = truncate_chars(keywords, MAX_KEYWORDS_CHARS);
        let key = format!("{}|{}", query.scope.as_str(), keywords.to_lowercase());
        if !seen.insert(key) {
            continue;
        }
        out.push(RecallQuery {
            keywords,
            scope: query.scope,
        });
        if out.len() >= MAX_RECALL_QUERIES {
            break;
        }
    }
    out
}

/// 并行执行模型给出的检索，并合并去重。
///
/// 任一查询失败只记录日志，不影响其他渠道；全部失败时返回空列表。
pub async fn recall_in_parallel(
    memory: &MemoryManager,
    queries: &[RecallQuery],
    char_id: &str,
    per_query_limit: usize,
) -> Vec<MemoryItem> {
    if queries.is_empty() || per_query_limit == 0 {
        return Vec::new();
    }
    let futures: Vec<_> = queries
        .iter()
        .map(|query| {
            let memory = memory.clone();
            let keywords = query.keywords.clone();
            let filter = query.scope.filter();
            async move {
                memory
                    .search_memories_filtered(
                        &keywords,
                        RetrievalStrategy::Hybrid,
                        per_query_limit,
                        &filter,
                    )
                    .await
                    .map_err(|error| {
                        tracing::debug!(
                            "[AgenticRecall] scope={} keywords={} failed: {}",
                            query.scope.as_str(),
                            keywords,
                            error
                        );
                        error
                    })
                    .unwrap_or_default()
            }
        })
        .collect();
    let batches = futures::future::join_all(futures).await;

    merge_and_filter(batches, char_id)
}

/// 合并多批检索结果：按 id / 内容去重，过滤不可见、种子人设与不可召回记录。
fn merge_and_filter(batches: Vec<Vec<MemoryItem>>, char_id: &str) -> Vec<MemoryItem> {
    let mut seen_ids = HashSet::new();
    let mut seen_contents = HashSet::new();
    let mut merged = Vec::new();

    for batch in batches {
        for item in batch {
            if !seen_ids.insert(item.id.clone()) {
                continue;
            }
            let content = item.content.trim();
            if content.is_empty() {
                continue;
            }
            let content_key = content.to_lowercase();
            if !seen_contents.insert(content_key) {
                continue;
            }
            if !kinds::recallable(&item) || !provenance::visible_to(&item.metadata, char_id) {
                continue;
            }
            if matches!(
                item.metadata.get("source").and_then(|v| v.as_str()),
                Some("system_seed" | "environment_preset")
            ) || item.id.starts_with("seed_")
            {
                continue;
            }
            merged.push(item);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::Granularity;

    #[test]
    fn scopes_map_to_expected_channels() {
        assert!(RecallScope::All.filter().memory_types.is_none());
        assert_eq!(
            RecallScope::Diary.filter().tags_any,
            Some(vec!["diary".to_string(), "daily_diary".to_string()])
        );
        assert_eq!(
            RecallScope::Note.filter().memory_types,
            Some(vec!["knowledge".to_string(), "reference".to_string()])
        );
        assert!(RecallScope::Summary
            .filter()
            .memory_types
            .unwrap()
            .contains(&"session_summary".to_string()));
        assert!(RecallScope::Dialogue
            .filter()
            .memory_types
            .unwrap()
            .contains(&"casual_conversation".to_string()));
    }

    #[test]
    fn scope_parse_is_lenient_and_defaults_to_all() {
        assert_eq!(RecallScope::parse(""), Some(RecallScope::All));
        assert_eq!(RecallScope::parse(" Diary "), Some(RecallScope::Diary));
        assert_eq!(RecallScope::parse("notes"), Some(RecallScope::Note));
        assert_eq!(RecallScope::parse("unknown-scope"), None);
    }

    #[test]
    fn sanitize_dedupes_trims_and_limits_queries() {
        let wire = vec![
            RecallQuery { keywords: " 小麦过敏 ".into(), scope: RecallScope::Fact },
            RecallQuery { keywords: "小麦过敏".into(), scope: RecallScope::Fact },
            RecallQuery { keywords: "".into(), scope: RecallScope::All },
            RecallQuery { keywords: "上周的面试".into(), scope: RecallScope::Diary },
            RecallQuery { keywords: "咖啡店".into(), scope: RecallScope::Note },
            RecallQuery { keywords: "第四个".into(), scope: RecallScope::All },
        ];
        let queries = sanitize_queries(wire);
        assert_eq!(queries.len(), MAX_RECALL_QUERIES);
        assert_eq!(queries[0].keywords, "小麦过敏");
        assert_eq!(queries[0].scope, RecallScope::Fact);
        assert_eq!(queries[1].scope, RecallScope::Diary);
        let long = sanitize_queries(vec![RecallQuery {
            keywords: "长".repeat(200),
            scope: RecallScope::All,
        }]);
        assert!(long[0].keywords.chars().count() <= MAX_KEYWORDS_CHARS);
    }

    #[test]
    fn merge_filters_invisible_and_authored_entries() {
        let mut visible = MemoryItem::new("用户对小麦过敏".into(), Granularity::Turn, 0.8);
        visible.metadata = serde_json::json!({"speaker":"user","listener":"nana","record_kind":"fact"});
        let mut hidden = MemoryItem::new("只有 vivian 知道的事".into(), Granularity::Turn, 0.9);
        hidden.metadata = serde_json::json!({"known_by":["vivian"],"listener":"all","record_kind":"fact"});
        let mut seed = MemoryItem::new("种子人设".into(), Granularity::Turn, 1.0);
        seed.metadata = serde_json::json!({"source":"system_seed","record_kind":"fact"});
        let mut duplicate = visible.clone();
        duplicate.id = "duplicate".into();

        let merged = merge_and_filter(vec![vec![visible.clone()], vec![hidden, seed, duplicate]], "nana");
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, visible.id);
    }
}
