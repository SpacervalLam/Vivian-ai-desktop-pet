//! 精度过滤器：参数化的 5 阶放松阶梯。
//!
//! 与 `filter.rs` 中的 `relaxed_strict_filter` 互补：
//! - `filter.rs` 的阶梯用于「新会话注入」场景，条件固定（长期类 + importance 阈值）。
//! - 本模块的阶梯用于 `retrieve_memory` 工具调用，条件由 LLM 传入的参数决定。
//!
//! strict → drop_importance_min → drop_categories → drop_subjects → no_filters
//! 每阶条件累积 OR，保证结果集单调递增，避免 strict 过滤导致空结果。
//!
//! 阶梯迭代逻辑复用 `super::relaxation::RelaxationLadder`，本模块只保留阶段谓词。

use super::filter::MemoryFilter;
use super::relaxation::RelaxationLadder;
use super::types::MemoryItem;
use crate::mind::Attention;

/// 放松阶梯最小结果数：strict 路径返回少于此值时逐级放松。
pub const RELAXATION_MIN_RESULTS_DEFAULT: usize = 3;

/// 检索精度过滤条件（由 retrieve_memory 工具参数构造）。
///
/// 所有字段为空/None 时等价于无过滤（直接返回全部候选）。
#[derive(Debug, Clone, Default)]
pub struct PrecisionFilterCriteria {
    /// 关键词过滤：记忆的 keywords / content / tags 中需至少命中一个。
    pub keywords: Vec<String>,
    /// 主语范围过滤：记忆的 subject_scopes 需至少命中一个。
    pub subject_scopes: Vec<String>,
    /// 分类过滤：记忆的 categories / tags 需至少命中一个。
    pub categories: Vec<String>,
    /// 重要性下限。
    pub importance_min: Option<f64>,
    /// 时间提示过滤（如 "昨天" / "上周" / "2024-01"）：匹配 date_label 或 time_of_day。
    pub time_hint: Option<String>,
    /// 来源层级过滤：raw / episodic / semantic。
    pub source_layers: Vec<String>,
    /// 实体-实体检索范围：按记忆元数据中的 speaker/listener 过滤。
    ///
    /// - `EntityScope::WithUser`：只检索 `knowledge_source == "direct"` 的记忆
    /// - `EntityScope::WithAgent(char_id)`：只检索 speaker 或 listener 等于 char_id 的记忆
    /// - `EntityScope::All`：不额外过滤（默认）
    ///
    /// 用于避免"把用户说过的话记成是室友说的"这类认知混乱。
    pub entity_scope: Option<EntityScope>,
    /// 注意力权重（运行时聚焦）：检索时不做硬过滤，仅在 rank_and_truncate
    /// 排序阶段对命中高注意力实体的记忆加权。None 时退化为不加权。
    ///
    /// Attention 决定"现在关注什么"，与 entity_scope（硬过滤）正交：
    /// entity_scope 控制范围，attention 调整排序。
    pub attention_weights: Option<Attention>,
}

/// 实体-实体检索范围
#[derive(Debug, Clone, PartialEq)]
pub enum EntityScope {
    /// 只检索与用户相关的记忆（knowledge_source == "direct"）
    WithUser,
    /// 只检索与指定角色相关的记忆（speaker 或 listener == char_id）
    WithAgent(String),
    /// 不额外过滤（默认行为）
    All,
}

impl PrecisionFilterCriteria {
    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty()
            && self.subject_scopes.is_empty()
            && self.categories.is_empty()
            && self.importance_min.is_none()
            && self.time_hint.is_none()
            && self.source_layers.is_empty()
            && self.entity_scope.as_ref().map_or(true, |s| matches!(s, EntityScope::All))
    }
}

/// 对候选记忆应用精度过滤，按 5 阶放松阶梯逐级放宽直到结果数 ≥ min_results。
///
/// 返回过滤后的记忆列表（保持原始顺序）。
///
/// 阶梯定义：
/// - 0 (strict)：全部条件 AND
/// - 1 (drop_importance_min)：取消 importance 下限
/// - 2 (drop_categories)：取消 categories 过滤
/// - 3 (drop_subjects)：取消 subject_scopes 过滤
/// - 4 (no_filters)：全部通过
pub fn apply_precision_filter(
    memories: &[MemoryItem],
    criteria: &PrecisionFilterCriteria,
    min_results: usize,
) -> Vec<MemoryItem> {
    if criteria.is_empty() {
        return memories.to_vec();
    }

    let ladder = RelaxationLadder::new(min_results);
    ladder.run(memories, |stage, m| stage_allows(stage, m, criteria))
}

/// 判断记忆是否通过指定阶段的过滤条件。
fn stage_allows(stage: usize, m: &MemoryItem, criteria: &PrecisionFilterCriteria) -> bool {
    // stage 0+ : entity_scope 过滤始终生效（实体-实体检索，防止认知混乱）
    if let Some(scope) = &criteria.entity_scope {
        if !matches_entity_scope(m, scope) {
            return false;
        }
    }

    // stage 0+ : keywords 过滤始终生效（核心检索条件）
    if !criteria.keywords.is_empty() && !matches_keywords(m, &criteria.keywords) {
        return false;
    }

    // stage 0+ : source_layers 过滤始终生效
    if !criteria.source_layers.is_empty() && !criteria.source_layers.iter().any(|l| l == m.source_layer()) {
        return false;
    }

    // stage 0+ : time_hint 过滤始终生效
    if let Some(hint) = &criteria.time_hint {
        if !matches_time_hint(m, hint) {
            return false;
        }
    }

    // stage 0 : importance_min 生效
    if stage <= 0 {
        if let Some(min) = criteria.importance_min {
            if m.importance < min {
                return false;
            }
        }
    }

    // stage 0-1 : categories 过滤生效
    if stage <= 1 && !criteria.categories.is_empty() {
        if !matches_categories(m, &criteria.categories) {
            return false;
        }
    }

    // stage 0-2 : subject_scopes 过滤生效
    if stage <= 2 && !criteria.subject_scopes.is_empty() {
        if !matches_subject_scopes(m, &criteria.subject_scopes) {
            return false;
        }
    }

    true
}

/// 判断记忆是否匹配实体-实体检索范围
///
/// - `WithUser`：记忆元数据中 `knowledge_source == "direct"`
/// - `WithAgent(char_id)`：记忆元数据中 `speaker == char_id` 或 `listener == char_id`
/// - `All`：始终匹配
fn matches_entity_scope(m: &MemoryItem, scope: &EntityScope) -> bool {
    match scope {
        EntityScope::All => true,
        EntityScope::WithUser => {
            m.metadata
                .get("knowledge_source")
                .and_then(|v| v.as_str())
                .map(|s| s == "direct")
                .unwrap_or(false)
        }
        EntityScope::WithAgent(char_id) => {
            let speaker = m.metadata.get("speaker").and_then(|v| v.as_str()).unwrap_or("");
            let listener = m.metadata.get("listener").and_then(|v| v.as_str()).unwrap_or("");
            speaker == char_id || listener == char_id
        }
    }
}

/// 判断记忆是否与当前角色相关（用于主对话检索路径的实体相关性过滤）。
///
/// 相关性定义（满足任一即相关）：
/// - 无 metadata 的记忆（普通 ShortTerm/LongTerm）：视为相关
/// - `knowledge_source == "direct"`：用户直接对话，相关
/// - `speaker == char_id || listener == char_id`：自己参与的跨角色对话，相关
/// - `observer_id == char_id`：自己旁观到的，相关
/// - 其他情况（如另一对角色之间的对话）：不相关，过滤掉
///
/// 修复 M6：主对话路径未应用 EntityScope 过滤，可能检索到其他角色之间的对话记忆，
/// 导致 LLM 认知混乱（把"室友对用户说的话"误记为"用户对我说的话"）。
pub fn is_relevant_to_entity(m: &MemoryItem, char_id: &str) -> bool {
    // 无 metadata 或非对象：视为普通记忆，保留
    let obj = match m.metadata.as_object() {
        Some(o) => o,
        None => return true,
    };

    // 无 speaker/listener/observer_id 字段：视为普通记忆，保留
    let has_entity_fields = obj.contains_key("speaker")
        || obj.contains_key("listener")
        || obj.contains_key("observer_id")
        || obj.contains_key("knowledge_source");
    if !has_entity_fields {
        return true;
    }

    // knowledge_source == "direct"：用户直接对话，相关
    if obj
        .get("knowledge_source")
        .and_then(|v| v.as_str())
        .map(|s| s == "direct")
        .unwrap_or(false)
    {
        return true;
    }

    // speaker 或 listener == char_id：自己参与的对话，相关
    let speaker = obj.get("speaker").and_then(|v| v.as_str()).unwrap_or("");
    let listener = obj.get("listener").and_then(|v| v.as_str()).unwrap_or("");
    if speaker == char_id || listener == char_id {
        return true;
    }

    // observer_id == char_id：自己旁观的，相关
    if obj
        .get("observer_id")
        .and_then(|v| v.as_str())
        .map(|s| s == char_id)
        .unwrap_or(false)
    {
        return true;
    }

    // 其他情况：不相关（如另一对角色之间的对话）
    false
}

fn matches_keywords(m: &MemoryItem, keywords: &[String]) -> bool {
    let mem_keywords: Vec<String> = m.keywords();
    let content_lower = m.content.to_lowercase();
    let tags_lower: Vec<String> = m.tags.iter().map(|t| t.to_lowercase()).collect();

    keywords.iter().any(|kw| {
        let kw_lower = kw.to_lowercase();
        mem_keywords.iter().any(|k| k.to_lowercase() == kw_lower)
            || content_lower.contains(&kw_lower)
            || tags_lower.iter().any(|t| t.contains(&kw_lower))
    })
}

/// 记忆是否命中分类列表（检查 categories / tags）。
fn matches_categories(m: &MemoryItem, categories: &[String]) -> bool {
    let mem_cats = m.categories();
    categories.iter().any(|cat| {
        let cat_lower = cat.to_lowercase();
        mem_cats.iter().any(|c| c.to_lowercase() == cat_lower)
            || m.tags.iter().any(|t| t.to_lowercase() == cat_lower)
    })
}

/// 记忆是否命中主语范围列表。
fn matches_subject_scopes(m: &MemoryItem, scopes: &[String]) -> bool {
    let mem_scopes = m.subject_scopes();
    if mem_scopes.is_empty() {
        // 无 subject_scopes 时回退到 tags 匹配
        return scopes.iter().any(|s| m.tags.iter().any(|t| t == s));
    }
    scopes.iter().any(|s| mem_scopes.iter().any(|ms| ms == s))
}

/// 记忆是否匹配时间提示（检查 date_label / time_of_day / content）。
fn matches_time_hint(m: &MemoryItem, hint: &str) -> bool {
    let hint_lower = hint.to_lowercase();

    // 匹配 time_of_day
    if let Some(tod) = m.time_of_day() {
        if tod.to_lowercase().contains(&hint_lower) {
            return true;
        }
    }

    // 匹配 date_label
    if let Some(dl) = m.date_label() {
        if dl.to_lowercase().contains(&hint_lower) {
            return true;
        }
    }

    // 匹配 content 中的时间词
    if m.content.to_lowercase().contains(&hint_lower) {
        return true;
    }

    false
}

/// 排除可见上下文：从候选记忆中移除已在 prompt 中的记忆（按 id 去重）。
///
/// 不应在后检索阶段重复返回，避免 prompt 中出现重复信息。
pub fn exclude_visible_context(
    candidates: Vec<MemoryItem>,
    visible_ids: &[String],
) -> Vec<MemoryItem> {
    if visible_ids.is_empty() {
        return candidates;
    }
    let visible_set: std::collections::HashSet<&str> =
        visible_ids.iter().map(|s| s.as_str()).collect();
    candidates
        .into_iter()
        .filter(|m| !visible_set.contains(m.id.as_str()))
        .collect()
}

/// 对记忆列表按综合权重排序并截断。
///
/// 权重计算复用 `MemoryFilter::calculate_memory_weight`，保持与现有系统一致。
pub fn rank_and_truncate(memories: Vec<MemoryItem>, limit: usize) -> Vec<MemoryItem> {
    rank_and_truncate_with_attention(memories, limit, None)
}

/// 带 Attention 加权的排序截断。
///
/// 在 `calculate_memory_weight` 基础权重之上叠加注意力奖励：
/// 记忆涉及的实体（speaker/listener/keywords/tags）若命中高注意力焦点，
/// 按其注意力权重提升得分（最高 ×1.5），让"现在关注的事"更容易被召回。
///
/// Attention 不做硬过滤——低注意力记忆只是降权，不被剔除，保证召回广度。
pub fn rank_and_truncate_with_attention(
    memories: Vec<MemoryItem>,
    limit: usize,
    attention: Option<&Attention>,
) -> Vec<MemoryItem> {
    if memories.len() <= limit {
        return memories;
    }
    let mut scored: Vec<(MemoryItem, f64)> = memories
        .into_iter()
        .map(|m| {
            let base = MemoryFilter::calculate_memory_weight(&m);
            let bonus = attention
                .map(|a| attention_bonus(&m, a))
                .unwrap_or(0.0);
            (m, base + bonus)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(m, _)| m).collect()
}

/// 计算记忆的注意力奖励分。
///
/// 匹配维度：speaker / listener / keywords / tags。命中任一高注意力实体（权重 ≥ 0.3）
/// 即按其权重累加奖励，上限 0.5（即最高把基础权重提升 50%）。
fn attention_bonus(m: &MemoryItem, attention: &Attention) -> f64 {
    let mut bonus = 0.0f64;
    const THRESHOLD: f32 = 0.3;
    const MAX_BONUS: f64 = 0.5;

    // speaker / listener
    if let Some(obj) = m.metadata.as_object() {
        if let Some(speaker) = obj.get("speaker").and_then(|v| v.as_str()) {
            let w = attention.weight_of(speaker);
            if w >= THRESHOLD {
                bonus += w as f64 * 0.15;
            }
        }
        if let Some(listener) = obj.get("listener").and_then(|v| v.as_str()) {
            let w = attention.weight_of(listener);
            if w >= THRESHOLD {
                bonus += w as f64 * 0.15;
            }
        }
    }

    // keywords
    for kw in m.keywords() {
        let w = attention.weight_of(&kw);
        if w >= THRESHOLD {
            bonus += w as f64 * 0.1;
        }
    }

    // tags
    for tag in &m.tags {
        let w = attention.weight_of(tag);
        if w >= THRESHOLD {
            bonus += w as f64 * 0.1;
        }
    }

    bonus.min(MAX_BONUS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{Granularity, MemoryItem};

    fn make_item(id: &str, content: &str, importance: f64) -> MemoryItem {
        let mut m = MemoryItem::new(content.to_string(), Granularity::Turn, importance);
        m.id = id.to_string();
        m
    }

    #[test]
    fn test_empty_criteria_returns_all() {
        let memories = vec![
            make_item("m1", "hello", 0.5),
            make_item("m2", "world", 0.3),
        ];
        let criteria = PrecisionFilterCriteria::default();
        let result = apply_precision_filter(&memories, &criteria, 3);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_keyword_filter() {
        let mut m1 = make_item("m1", "我喜欢咖啡", 0.8);
        m1.metadata = serde_json::json!({"keywords": ["咖啡", "偏好"]});
        let m2 = make_item("m2", "今天天气不错", 0.3);
        let memories = vec![m1, m2];

        let criteria = PrecisionFilterCriteria {
            keywords: vec!["咖啡".to_string()],
            ..Default::default()
        };
        let result = apply_precision_filter(&memories, &criteria, 1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "m1");
    }

    #[test]
    fn test_relaxation_drops_importance() {
        // strict 要求 importance >= 0.8，但只有 1 条；放松到 stage 1 后 2 条都通过
        let m1 = make_item("m1", "咖啡偏好", 0.8);
        let m2 = make_item("m2", "咖啡事件", 0.3);
        let memories = vec![m1, m2];

        let criteria = PrecisionFilterCriteria {
            keywords: vec!["咖啡".to_string()],
            importance_min: Some(0.8),
            ..Default::default()
        };
        // min_results=2 触发放松
        let result = apply_precision_filter(&memories, &criteria, 2);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_source_layer_filter() {
        let m1 = make_item("m1", "raw 对话", 0.5);
        let mut m2 = MemoryItem::new("semantic 摘要".to_string(), Granularity::Summary, 0.7);
        m2.id = "m2".to_string();
        let memories = vec![m1, m2];

        let criteria = PrecisionFilterCriteria {
            source_layers: vec!["raw".to_string()],
            ..Default::default()
        };
        let result = apply_precision_filter(&memories, &criteria, 1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "m1");
    }

    #[test]
    fn test_exclude_visible_context() {
        let m1 = make_item("m1", "a", 0.5);
        let m2 = make_item("m2", "b", 0.5);
        let m3 = make_item("m3", "c", 0.5);
        let visible = vec!["m1".to_string()];
        let result = exclude_visible_context(vec![m1, m2, m3], &visible);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|m| m.id != "m1"));
    }
}
