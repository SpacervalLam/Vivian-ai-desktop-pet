//! 图谱路检索 — 作为 RRF 第三路
//!
//! 借鉴 GBrain 的 relational recall arm：
//! - parse_relational_query：检测查询是否为关系型（"谁在腾讯工作"/"马化腾创建了什么"）
//! - build_relational_arm：从图谱遍历获取相关实体，映射回记忆
//! - 作为 RRF 第三路与 BM25 + Vector 融合
//!
//! 与 GBrain 的差异：
//! - GBrain 用递归 CTE 遍历 Postgres links 表
//! - 本模块用 KnowledgeGraph::fanout 内存 BFS 遍历
//! - GBrain 返回 page slug，本模块返回 memory_id（通过实体的 memory_ids 映射）

use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::entity_extract::RelationType;
use super::graph_store::{FanoutResult, KnowledgeGraph};
use super::types::MemoryItem;

/// 关系型查询 archetype
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelationalKind {
    /// "谁投资了 X" / "谁创建了 X"（incoming 关系）
    WhoRel,
    /// "谁在 X 工作"（works_at incoming）
    WhoAt,
    /// "X 和 Y 有什么关系"（双 seed，both direction）
    Connects,
    /// "谁介绍我认识 X"（knows incoming）
    Intro,
    /// "谁喜欢 X" / "谁讨厌 X"（偏好/情感 incoming）
    WhoFeels,
    /// "X 的朋友是谁"（社交关系 outgoing）
    WhoSocial,
}

/// 关系型查询解析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedRelationalQuery {
    /// 查询类型
    pub kind: RelationalKind,
    /// 种子实体名（解析自查询文本）
    pub seeds: Vec<String>,
    /// 过滤的关系类型
    pub relation_types: Vec<RelationType>,
    /// 遍历方向
    pub direction: RelationDirection,
}

/// 遍历方向
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelationDirection {
    /// 入边（target → source）
    In,
    /// 出边（source → target）
    Out,
    /// 双向
    Both,
}

// ── regex 模式（借鉴 GBrain 的 relational-intent.ts）──

/// SEED 捕获：1-40 字符的有界 lazy 匹配（防 ReDoS）
const SEED_PATTERN: &str = r"(.{1,40}?)";

/// who_rel 模式：谁投资了/创建了/顾问了 SEED
static WHO_REL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?:谁|什么人|哪家公司|哪个机构).*?(?:投资了?|注资|领投|参投|创建了?|创办了?|成立了?|创立了?|担任.*顾问|给.*当顾问)\s*{SEED_PATTERN}"
    )).unwrap()
});

/// who_at 模式：谁在 SEED 工作
static WHO_AT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?:谁|什么人|哪些人).*?(?:在|就职于|任职于)\s*{SEED_PATTERN}\s*(?:工作|任职|就职|效力|做事)"
    )).unwrap()
});

/// connects 模式：SEED1 和 SEED2 的关系
static CONNECTS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"{SEED_PATTERN}\s*(?:和|与|跟)\s*{SEED_PATTERN}\s*(?:的关系|的关系是|有什么关系|怎么认识的|如何认识)"
    )).unwrap()
});

/// intro 模式：谁介绍我认识 SEED
static INTRO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?:谁|什么人).*?(?:介绍|引荐|带我认识|引见).*?(?:认识)?\s*{SEED_PATTERN}"
    )).unwrap()
});

/// who_feels 模式：谁喜欢/讨厌/信任/想念 SEED
static WHO_FEELS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?:谁|什么人|哪些人).*?(?:喜欢|喜爱|讨厌|不喜欢|反感|信任|想念|思念|关心|在意)\s*{SEED_PATTERN}"
    )).unwrap()
});

/// who_social 模式：SEED 的朋友/家人是谁
static WHO_SOCIAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"{SEED_PATTERN}\s*(?:的)?(?:朋友|好友|闺蜜|哥们|家人|哥哥|弟弟|姐姐|妹妹|爸爸|妈妈|父亲|母亲|儿子|女儿|丈夫|妻子|老公|老婆)(?:是谁|有哪些|都有谁)"
    )).unwrap()
});

/// 停用词种子（过滤代词和通用词）
const STOPWORD_SEEDS: &[&str] = &[
    "他", "她", "它", "他们", "她们", "它们",
    "这", "那", "这个", "那个", "这些", "那些",
    "什么", "怎么", "为什么", "哪里", "怎样",
    "我", "你", "我们", "你们",
    "人", "公司", "事情", "东西", "地方",
];

/// 解析关系型查询
///
/// 纯 regex，无 IO，无 LLM。
/// 按 specificity 降序匹配：connects > who_social > who_feels > intro > who_at > who_rel。
/// seed 收紧后过滤停用词。
pub fn parse_relational_query(query: &str) -> Option<ParsedRelationalQuery> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. connects（双 seed）
    if let Some(cap) = CONNECTS_RE.captures(trimmed) {
        let seed1 = cap.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let seed2 = cap.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if is_valid_seed(&seed1) && is_valid_seed(&seed2) {
            return Some(ParsedRelationalQuery {
                kind: RelationalKind::Connects,
                seeds: vec![seed1, seed2],
                relation_types: vec![],
                direction: RelationDirection::Both,
            });
        }
    }

    // 2. who_social（单 seed，社交关系 outgoing）
    if let Some(cap) = WHO_SOCIAL_RE.captures(trimmed) {
        let seed = cap.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if is_valid_seed(&seed) {
            let relation_types = infer_social_relation_types_from_query(trimmed);
            return Some(ParsedRelationalQuery {
                kind: RelationalKind::WhoSocial,
                seeds: vec![seed],
                relation_types,
                direction: RelationDirection::Out,
            });
        }
    }

    // 3. who_feels（单 seed，情感/偏好 incoming）
    if let Some(cap) = WHO_FEELS_RE.captures(trimmed) {
        let seed = cap.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if is_valid_seed(&seed) {
            let relation_types = infer_feeling_relation_types_from_query(trimmed);
            return Some(ParsedRelationalQuery {
                kind: RelationalKind::WhoFeels,
                seeds: vec![seed],
                relation_types,
                direction: RelationDirection::In,
            });
        }
    }

    // 4. intro（单 seed）
    if let Some(cap) = INTRO_RE.captures(trimmed) {
        let seed = cap.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if is_valid_seed(&seed) {
            return Some(ParsedRelationalQuery {
                kind: RelationalKind::Intro,
                seeds: vec![seed],
                relation_types: vec![RelationType::Knows],
                direction: RelationDirection::In,
            });
        }
    }

    // 5. who_at（单 seed，works_at incoming）
    if let Some(cap) = WHO_AT_RE.captures(trimmed) {
        let seed = cap.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if is_valid_seed(&seed) {
            return Some(ParsedRelationalQuery {
                kind: RelationalKind::WhoAt,
                seeds: vec![seed],
                relation_types: vec![RelationType::WorksAt],
                direction: RelationDirection::In,
            });
        }
    }

    // 6. who_rel（单 seed，动词表驱动）
    if let Some(cap) = WHO_REL_RE.captures(trimmed) {
        let seed = cap.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if is_valid_seed(&seed) {
            let relation_types = infer_relation_types_from_query(trimmed);
            return Some(ParsedRelationalQuery {
                kind: RelationalKind::WhoRel,
                seeds: vec![seed],
                relation_types,
                direction: RelationDirection::In,
            });
        }
    }

    None
}

/// 检查 seed 是否有效（非空、非停用词）
fn is_valid_seed(seed: &str) -> bool {
    if seed.is_empty() || seed.chars().count() < 2 {
        return false;
    }
    !STOPWORD_SEEDS.contains(&seed)
}

/// 根据查询文本推断关系类型
fn infer_relation_types_from_query(query: &str) -> Vec<RelationType> {
    let mut types = Vec::new();
    if query.contains("投资") || query.contains("注资") || query.contains("领投") {
        types.push(RelationType::InvestedIn);
    }
    if query.contains("创建") || query.contains("创办") || query.contains("成立") {
        types.push(RelationType::Founded);
    }
    if query.contains("顾问") {
        types.push(RelationType::Advises);
    }
    if query.contains("工作") || query.contains("就职") {
        types.push(RelationType::WorksAt);
    }
    if types.is_empty() {
        types.push(RelationType::Mentions);
    }
    types
}

/// 根据查询文本推断情感关系类型
fn infer_feeling_relation_types_from_query(query: &str) -> Vec<RelationType> {
    let mut types = Vec::new();
    if query.contains("喜欢") || query.contains("喜爱") {
        types.push(RelationType::Likes);
    }
    if query.contains("讨厌") || query.contains("不喜欢") || query.contains("反感") {
        types.push(RelationType::Dislikes);
    }
    if query.contains("信任") {
        types.push(RelationType::Trusts);
    }
    if query.contains("想念") || query.contains("思念") {
        types.push(RelationType::Misses);
    }
    if query.contains("关心") || query.contains("在意") {
        types.push(RelationType::CaresFor);
    }
    if types.is_empty() {
        types.push(RelationType::Likes);
    }
    types
}

/// 根据查询文本推断社交关系类型
fn infer_social_relation_types_from_query(query: &str) -> Vec<RelationType> {
    let mut types = Vec::new();
    let is_family = query.contains("家人")
        || query.contains("哥哥") || query.contains("弟弟")
        || query.contains("姐姐") || query.contains("妹妹")
        || query.contains("爸爸") || query.contains("妈妈")
        || query.contains("父亲") || query.contains("母亲")
        || query.contains("儿子") || query.contains("女儿")
        || query.contains("丈夫") || query.contains("妻子")
        || query.contains("老公") || query.contains("老婆");
    if is_family {
        types.push(RelationType::FamilyOf);
    }
    if query.contains("朋友") || query.contains("好友") || query.contains("闺蜜") || query.contains("哥们") {
        types.push(RelationType::FriendOf);
    }
    if types.is_empty() {
        types.push(RelationType::FriendOf);
    }
    types
}

/// 图谱检索结果（映射回记忆）
#[derive(Debug, Clone)]
pub struct RelationalHit {
    /// 记忆 ID
    pub memory_id: String,
    /// 图谱跳数
    pub hop: usize,
    /// 经由的关系类型
    pub via_relation: RelationType,
    /// 种子实体名
    pub seed: String,
    /// 边权重
    pub edge_weight: f64,
}

/// 构建图谱检索臂
///
/// 借鉴 GBrain 的 buildRelationalArm：
/// 1. parse_relational_query 解析查询
/// 2. 在图谱中 fanout 遍历
/// 3. 把图谱实体映射回记忆 ID
///
/// 返回的 RelationalHit 列表可作为 RRF 第三路与 BM25/Vector 融合。
pub fn build_relational_arm(
    graph: &Arc<KnowledgeGraph>,
    query: &str,
    all_entries: &[MemoryItem],
    limit: usize,
) -> Vec<RelationalHit> {
    let parsed = match parse_relational_query(query) {
        Some(p) => p,
        None => return Vec::new(),
    };

    if parsed.seeds.is_empty() {
        return Vec::new();
    }

    // 构建实体名 → 记忆 ID 映射
    let mut entity_to_memories: HashMap<String, Vec<String>> = HashMap::new();
    for entry in all_entries {
        let result = super::entity_extract::extract(&entry.content);
        for entity in &result.entities {
            entity_to_memories
                .entry(entity.name.clone())
                .or_default()
                .push(entry.id.clone());
        }
    }

    let mut hits = Vec::new();

    if parsed.kind == RelationalKind::Connects && parsed.seeds.len() >= 2 {
        // 双 seed：找两个实体的共同邻居
        let seed1 = parsed.seeds[0].as_str();
        let seed2 = parsed.seeds[1].as_str();

        let fanout1 = graph.fanout(&[seed1], &parsed.relation_types, 2, limit * 2);
        let fanout2 = graph.fanout(&[seed2], &parsed.relation_types, 2, limit * 2);

        // 找交集
        let entities1: HashMap<String, &FanoutResult> = fanout1
            .iter()
            .map(|r| (r.entity_name.clone(), r))
            .collect();
        let entities2: HashMap<String, &FanoutResult> = fanout2
            .iter()
            .map(|r| (r.entity_name.clone(), r))
            .collect();

        for (name, r1) in &entities1 {
            if let Some(r2) = entities2.get(name) {
                // 共同邻居
                if let Some(memory_ids) = entity_to_memories.get(name) {
                    for mid in memory_ids {
                        hits.push(RelationalHit {
                            memory_id: mid.clone(),
                            hop: r1.hop + r2.hop,
                            via_relation: r1.via_relation.clone(),
                            seed: format!("{}+{}", seed1, seed2),
                            edge_weight: (r1.edge_weight + r2.edge_weight) / 2.0,
                        });
                    }
                }
            }
        }
    } else {
        // 单 seed：直接 fanout
        let seed_strs: Vec<&str> = parsed.seeds.iter().map(|s| s.as_str()).collect();
        let fanout = graph.fanout(&seed_strs, &parsed.relation_types, 2, limit * 2);

        for result in &fanout {
            if let Some(memory_ids) = entity_to_memories.get(&result.entity_name) {
                for mid in memory_ids {
                    hits.push(RelationalHit {
                        memory_id: mid.clone(),
                        hop: result.hop,
                        via_relation: result.via_relation.clone(),
                        seed: parsed.seeds[0].clone(),
                        edge_weight: result.edge_weight,
                    });
                }
            }
        }
    }

    // 去重：相同 memory_id 只保留 hop 最小的
    hits.sort_by(|a, b| a.hop.cmp(&b.hop));
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    hits.retain(|h| seen.insert(h.memory_id.clone()));

    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_who_rel_invest() {
        let q = "谁投资了腾讯";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::WhoRel);
        assert!(p.relation_types.contains(&RelationType::InvestedIn));
    }

    #[test]
    fn test_parse_who_rel_found() {
        let q = "谁创建了腾讯";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::WhoRel);
        assert!(p.relation_types.contains(&RelationType::Founded));
    }

    #[test]
    fn test_parse_who_at() {
        let q = "谁在腾讯工作";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::WhoAt);
        assert!(p.relation_types.contains(&RelationType::WorksAt));
    }

    #[test]
    fn test_parse_connects() {
        let q = "马化腾和李彦宏的关系";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::Connects);
        assert_eq!(p.seeds.len(), 2);
        assert_eq!(p.direction, RelationDirection::Both);
    }

    #[test]
    fn test_parse_intro() {
        let q = "谁介绍我认识张三";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::Intro);
        assert!(p.relation_types.contains(&RelationType::Knows));
    }

    #[test]
    fn test_parse_non_relational() {
        assert!(parse_relational_query("今天天气怎么样").is_none());
        assert!(parse_relational_query("").is_none());
        assert!(parse_relational_query("你好").is_none());
    }

    #[test]
    fn test_stopword_filtered() {
        // "什么" 是停用词，应该被过滤
        let q = "谁投资了什么";
        let parsed = parse_relational_query(q);
        // "什么" 被过滤后 seed 无效，返回 None
        assert!(parsed.is_none());
    }

    #[test]
    fn test_is_valid_seed() {
        assert!(is_valid_seed("腾讯"));
        assert!(is_valid_seed("马化腾"));
        assert!(!is_valid_seed("他"));
        assert!(!is_valid_seed("什么"));
        assert!(!is_valid_seed(""));
        assert!(!is_valid_seed("A")); // 单字
    }

    #[test]
    fn test_infer_relation_types() {
        let types = infer_relation_types_from_query("谁投资了腾讯");
        assert!(types.contains(&RelationType::InvestedIn));

        let types = infer_relation_types_from_query("谁创建了腾讯");
        assert!(types.contains(&RelationType::Founded));

        let types = infer_relation_types_from_query("谁给腾讯当顾问");
        assert!(types.contains(&RelationType::Advises));

        // 无匹配关键词 → mentions
        let types = infer_relation_types_from_query("谁认识腾讯");
        assert!(types.contains(&RelationType::Mentions));
    }

    #[test]
    fn test_parse_who_feels_likes() {
        let q = "谁喜欢音乐";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::WhoFeels);
        assert!(p.relation_types.contains(&RelationType::Likes));
    }

    #[test]
    fn test_parse_who_feels_dislikes() {
        let q = "谁讨厌香菜";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::WhoFeels);
        assert!(p.relation_types.contains(&RelationType::Dislikes));
    }

    #[test]
    fn test_parse_who_feels_trusts() {
        let q = "谁信任小明";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::WhoFeels);
        assert!(p.relation_types.contains(&RelationType::Trusts));
    }

    #[test]
    fn test_parse_who_social_friend() {
        let q = "小明的朋友是谁";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::WhoSocial);
        assert!(p.relation_types.contains(&RelationType::FriendOf));
        assert_eq!(p.direction, RelationDirection::Out);
    }

    #[test]
    fn test_parse_who_social_family() {
        let q = "小红的家人有哪些";
        let parsed = parse_relational_query(q);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.kind, RelationalKind::WhoSocial);
        assert!(p.relation_types.contains(&RelationType::FamilyOf));
    }

    #[test]
    fn test_infer_feeling_relation_types() {
        let types = infer_feeling_relation_types_from_query("谁喜欢小明");
        assert!(types.contains(&RelationType::Likes));

        let types = infer_feeling_relation_types_from_query("谁讨厌小明");
        assert!(types.contains(&RelationType::Dislikes));

        let types = infer_feeling_relation_types_from_query("谁想念小明");
        assert!(types.contains(&RelationType::Misses));

        let types = infer_feeling_relation_types_from_query("谁关心小明");
        assert!(types.contains(&RelationType::CaresFor));

        // 无匹配 → 默认 likes
        let types = infer_feeling_relation_types_from_query("谁对小明的态度");
        assert!(types.contains(&RelationType::Likes));
    }

    #[test]
    fn test_infer_social_relation_types() {
        let types = infer_social_relation_types_from_query("小明的朋友是谁");
        assert!(types.contains(&RelationType::FriendOf));

        let types = infer_social_relation_types_from_query("小红的家人有哪些");
        assert!(types.contains(&RelationType::FamilyOf));

        let types = infer_social_relation_types_from_query("小红的哥哥是谁");
        assert!(types.contains(&RelationType::FamilyOf));

        // 无匹配 → 默认 friend_of
        let types = infer_social_relation_types_from_query("小明的关系网");
        assert!(types.contains(&RelationType::FriendOf));
    }
}
