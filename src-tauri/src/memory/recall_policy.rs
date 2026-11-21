//! Query-local relevance admission. Ranking, age and importance cannot create evidence.
use std::collections::HashSet;
use serde::{Deserialize, Serialize};
use super::types::MemoryItem;

/// Only an unresolved follow-up may carry a bounded previous topic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecallQuery {
    pub focus: String,
    pub context: String,
    #[serde(default)]
    pub continuation: bool,
}

impl RecallQuery {
    pub fn new(focus: &str, context: &str) -> Self {
        Self { focus: focus.trim().chars().take(2000).collect(),
            context: context.trim().chars().take(400).collect(), continuation:false }
    }

    pub fn effective_text(&self) -> &str {
        if (self.continuation && !self.context.is_empty()) || terms(&self.focus).is_empty() { &self.context } else { &self.focus }
    }

    pub fn cache_key(&self) -> String {
        // JSON preserves component boundaries, including user-supplied delimiters.
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Conservative vector-only floor. This is relevance evidence, never factual verification.
pub const SEMANTIC_ONLY_MIN: f64 = 0.78;

pub fn preference_query(text: &str) -> bool {
    ["用户喜好", "用户偏好", "我的喜好", "我的偏好", "我喜欢什么", "user preferences", "my preferences"]
        .contains(&text.trim().trim_end_matches(['?', '？']).to_lowercase().as_str())
}

pub fn query_terms(text: &str) -> Vec<String> {
    if preference_query(text) { return vec!["preference".into()]; }
    const STOP: &[&str] = &["我", "你", "他", "她", "它", "我们", "你们", "他们", "她们", "的", "了", "是", "在", "有", "和", "也", "就", "都", "说", "吗", "呢", "啊", "吧", "好", "嗯", "嗯嗯", "好的", "可以", "为什么", "怎么", "什么", "哪些", "如何", "这个", "那个", "这件事", "那件事", "刚才", "之前", "今天", "最近", "继续", "然后", "意思", "记得", "用户", "user", "assistant", "the", "a", "an", "is", "are", "was", "i", "you", "it", "this", "that", "and", "to", "of", "in", "what", "why", "how", "yes", "no", "ok", "okay", "continue", "それ", "その", "これ", "はい", "なぜ"];
    let lower = text.trim().to_lowercase();
    let single = lower.chars().count() == 1;
    let generic = ["知道", "告诉", "觉得", "时候", "事情", "问题", "有没有", "能不能", "请问", "一下", "了解", "想问", "想知道", "关于", "记忆", "回忆", "你好", "展开", "讲讲", "怎么说", "说说", "说说看", "怎么样", "do", "does", "can", "could", "tell", "me", "about", "know", "remember"];
    let mut seen = HashSet::new();
    super::tokenize::tokenize(&lower).into_iter().filter(|word|
        !STOP.contains(&word.as_str()) && !generic.contains(&word.as_str())
        && (single || word.chars().count() > 1 || ["猫", "狗", "饭", "药", "病", "钱", "书", "歌", "茶"].contains(&word.as_str()))
        && seen.insert(word.clone())
    ).collect()
}

fn terms(text: &str) -> HashSet<String> { query_terms(text).into_iter().collect() }

pub fn lexical_support(query: &str, item: &MemoryItem) -> usize {
    let mut body: HashSet<_> = super::tokenize::tokenize(&item.content.to_lowercase()).into_iter().collect();
    if let Some(description) = &item.description { body.extend(super::tokenize::tokenize(&description.to_lowercase())); }
    // Generic storage tags are not evidence that the user discussed a topic.
    for tag in &item.tags {
        if !["user", "identity", "preference", "project", "relationship", "short_term", "long_term", "dialogue_turn", "raw", "fact"].contains(&tag.as_str()) {
            body.extend(terms(tag));
        }
    }
    terms(query).intersection(&body).count()
}

#[derive(Debug, Clone, Serialize)]
pub struct Admission {
    pub kind: &'static str,
    pub lexical_terms: usize,
    pub vector_similarity: Option<f64>,
}

pub fn admit(query: &str, item: &MemoryItem, vector: Option<f64>, relational: bool) -> Option<Admission> {
    if query.trim().is_empty() || item.content.trim().is_empty() { return None; }
    let lexical_terms = lexical_support(query, item);
    let vector = vector.filter(|value| value.is_finite() && (-1.0..=1.0).contains(value));
    let category = preference_query(query) && (item.memory_type == "preference"
        || item.tags.iter().any(|tag|tag == "preference"));
    let kind = if lexical_terms > 0 { "lexical" }
        else if category { "category" }
        else if relational { "relational" }
        else if vector.is_some_and(|value| value >= SEMANTIC_ONLY_MIN) { "semantic" }
        else { return None; };
    Some(Admission { kind, lexical_terms, vector_similarity: vector })
}

pub fn stamp(item: &mut MemoryItem, query: &str, evidence: Admission) {
    if !item.metadata.is_object() { item.metadata = serde_json::json!({}); }
    item.metadata["recall_admission"] = serde_json::to_value(evidence).unwrap_or_default();
    item.metadata["recall_admission_query"] = serde_json::json!(query);
}

/// Expansion results must be relevant to this request, not just their expansion query.
pub fn filter_for_query(items: Vec<MemoryItem>, query: &str) -> Vec<MemoryItem> {
    items.into_iter().filter_map(|mut item| {
        let same_query = item.metadata["recall_admission_query"].as_str()
            .is_some_and(|stored| stored.trim().to_lowercase() == query.trim().to_lowercase());
        let vector = same_query.then(|| item.metadata["recall_admission"]["vector_similarity"].as_f64()).flatten();
        let relational = same_query && item.metadata["recall_admission"]["kind"] == "relational";
        let evidence = admit(query, &item, vector, relational)?;
        stamp(&mut item, query, evidence);
        Some(item)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::Granularity;
    fn item(text: &str) -> MemoryItem { MemoryItem::new(text.into(), Granularity::Turn, 1.0) }

    #[test]
    fn relevance_is_independent_of_rank_age_and_importance() {
        let mut old = item("用户对香菜过敏"); old.timestamp = 1.0;
        assert!(admit("香菜过敏", &old, None, false).is_some());
        let mut unrelated = item("用户喜欢养猫");
        unrelated.metadata = serde_json::json!({"fused_score":1.0,"combined_score":1.0});
        assert!(admit("量子纠缠", &unrelated, Some(0.65), false).is_none());
        assert!(admit("量子纠缠", &unrelated, Some(f64::NAN), false).is_none());
        assert!(admit("量子纠缠", &unrelated, Some(0.9), false).is_some());
    }
    #[test]
    fn stop_words_and_storage_tags_do_not_admit_unrelated_memories() {
        let mut memory = item("我今天养了一只猫"); memory.tags = vec!["user".into(),"project".into()];
        assert!(admit("我今天在研究量子纠缠", &memory, None, false).is_none());
        assert!(admit("project", &memory, None, false).is_none());
        assert!(admit("猫", &memory, None, false).is_some());
        assert!(admit("猫怎么样", &memory, None, false).is_some());
    }
    #[test]
    fn expansion_cannot_reuse_another_querys_semantic_evidence() {
        let mut memory = item("用户喜欢养猫");
        let evidence = admit("宠物",&memory,Some(0.9),false).unwrap();
        stamp(&mut memory,"宠物",evidence);
        assert!(filter_for_query(vec![memory.clone()],"量子纠缠").is_empty());
        assert_eq!(filter_for_query(vec![memory],"宠物").len(),1);
    }
    #[test]
    fn focus_and_context_are_distinct_cache_inputs() {
        let followup = RecallQuery::new("为什么", "香菜过敏");
        assert_eq!(followup.effective_text(), "香菜过敏");
        assert_eq!(RecallQuery::new("量子纠缠", "香菜过敏").effective_text(), "量子纠缠");
        assert_ne!(followup.cache_key(), RecallQuery::new("为什么", "考研").cache_key());
        let mut quantity = RecallQuery::new("30分钟", "最近失眠"); quantity.continuation = true;
        assert_eq!(quantity.effective_text(), "最近失眠");
    }
}
