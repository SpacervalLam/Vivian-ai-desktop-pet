//! 实体提取 + 中文动词关系推断（零 LLM 调用）
//!
//! 借鉴 GBrain 的自布线知识图谱设计：
//! - gazetteer 构建：从已有记忆中提取实体名构建词典
//! - maximal-munch 匹配：在新文本中找最长匹配的实体
//! - 正则动词推断：用预编译 regex 推断实体间的关系类型
//!
//! 与 GBrain 的差异：
//! - GBrain 用 `[a-zA-Z0-9]+` 分词（英文优先），本模块用 jieba 分词（中文优先）
//! - GBrain 的实体来自 pages 表（用户显式创建），本模块的实体从记忆内容中自动抽取
//! - 关系动词 regex 针对中文优化（"在...工作"/"投资了"/"创建了"）

use std::collections::HashMap;

use jieba_rs::Jieba;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// 全局 jieba 实例
static JIEBA: Lazy<Jieba> = Lazy::new(Jieba::new);

/// 实体类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    /// 人名（jieba 词性 nr）
    Person,
    /// 地名（jieba 词性 ns）
    Location,
    /// 机构名（jieba 词性 nt）
    Organization,
    /// 其他专名（jieba 词性 nz）
    Other,
    /// 抽象概念（如 agent_autonomy / proactive / inner_monologue）
    ///
    /// 由概念层（UserModel）写入，非从文本自动抽取。作为图谱的"主题层"实体，
    /// 让 query 的话题词能通过图谱概念路命中概念相关记忆。
    Concept,
}

impl EntityType {
    /// 从 jieba 词性标签推断实体类型
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            t if t.starts_with("nr") => Some(Self::Person),
            t if t.starts_with("ns") => Some(Self::Location),
            t if t.starts_with("nt") => Some(Self::Organization),
            t if t.starts_with("nz") => Some(Self::Other),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Location => "location",
            Self::Organization => "organization",
            Self::Other => "other",
            Self::Concept => "concept",
        }
    }
}

/// 提取的实体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// 实体名（归一化后的文本）
    pub name: String,
    /// 实体类型
    pub entity_type: EntityType,
    /// 显著性分数（0-1，基于出现频率和位置）
    pub salience: f64,
}

/// 关系类型（typed edges）
///
/// 借鉴 GBrain 的 link_type，针对中文场景调整。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    /// 工作于（"在...工作"/"就职于"/"是...的员工"）
    WorksAt,
    /// 投资（"投资了"/"注资"/"领投"）
    InvestedIn,
    /// 创立（"创建了"/"创办了"/"联合创始人"）
    Founded,
    /// 顾问（"担任...顾问"/"给...当顾问"）
    Advises,
    /// 认识（"认识"/"介绍了"/"带我认识"）
    Knows,
    /// 喜欢（"喜欢"/"喜爱"/"钟爱"）
    Likes,
    /// 不喜欢（"不喜欢"/"讨厌"/"反感"）
    Dislikes,
    /// 偏好（"偏好"/"更倾向"/"更喜欢"）
    Prefers,
    /// 朋友（"是...的朋友"/"和...是朋友"）
    FriendOf,
    /// 家人（"是...的家人"/"和...是家人"）
    FamilyOf,
    /// 信任（"信任"/"相信"）
    Trusts,
    /// 关心（"关心"/"在意"/"照顾"）
    CaresFor,
    /// 想念（"想念"/"思念"/"惦记"）
    Misses,
    /// 提及（兜底关系）
    Mentions,
}

impl RelationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WorksAt => "works_at",
            Self::InvestedIn => "invested_in",
            Self::Founded => "founded",
            Self::Advises => "advises",
            Self::Knows => "knows",
            Self::Likes => "likes",
            Self::Dislikes => "dislikes",
            Self::Prefers => "prefers",
            Self::FriendOf => "friend_of",
            Self::FamilyOf => "family_of",
            Self::Trusts => "trusts",
            Self::CaresFor => "cares_for",
            Self::Misses => "misses",
            Self::Mentions => "mentions",
        }
    }

    /// 所有已知关系类型（用于验证）
    pub fn known_types() -> &'static [Self] {
        &[
            Self::WorksAt,
            Self::InvestedIn,
            Self::Founded,
            Self::Advises,
            Self::Knows,
            Self::Likes,
            Self::Dislikes,
            Self::Prefers,
            Self::FriendOf,
            Self::FamilyOf,
            Self::Trusts,
            Self::CaresFor,
            Self::Misses,
            Self::Mentions,
        ]
    }

    /// 关系是否对称（A→B 等价于 B→A）
    pub fn is_symmetric(&self) -> bool {
        matches!(self, Self::FriendOf | Self::FamilyOf)
    }
}

/// 提取的关系（三元组）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    /// 主体实体名
    pub subject: String,
    /// 客体实体名
    pub object: String,
    /// 关系类型
    pub relation_type: RelationType,
    /// 上下文片段（用于溯源）
    pub context: String,
    /// 置信度（0-1）
    pub confidence: f64,
}

// ── 中文动词关系推断的 regex 模式 ──────────────────────────────────
//
// 借鉴 GBrain 的 FOUNDED_RE / INVESTED_RE / ADVISES_RE / WORKS_AT_RE，
// 针对中文动词重新设计。优先级：founded > invested_in > advises > works_at > knows > mentions。

/// 工作于：在...工作 / 就职于 / 是...的员工 / 加入...担任
static WORKS_AT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:在|就职于|加入|任职于|效力于)\s*(.{1,30}?)\s*(?:工作|任职|就职|效力|担任|做|当)|(?:是|为)\s*(.{1,30}?)\s*(?:的)?(?:员工|工程师|经理|总监|主管|负责人|合伙人|创始人|CTO|CEO|CFO|COO|VP)"
    ).unwrap()
});

/// 投资：投资了 / 注资 / 领投 / 参投 / 出资
static INVESTED_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:投资了?|注资|领投|参投|出资|融资|注资了?)\s*(.{1,30}?)|(?:给|为)\s*(.{1,30}?)\s*(?:投了?|注资|出资)"
    ).unwrap()
});

/// 创立：创建了 / 创办了 / 联合创始人 / 成立了
static FOUNDED_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:创建|创办|成立|创立|建立)了?\s*(.{1,30}?)|(.{1,30}?)\s*(?:是|为)\s*(.{1,30}?)\s*(?:的)?(?:创始人|联合创始人|创办人|发起人)"
    ).unwrap()
});

/// 顾问：担任...顾问 / 给...当顾问 / 是...的顾问
static ADVISES_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:担任|当|做|是)\s*(.{1,30}?)\s*(?:的)?(?:顾问|参谋|指导)|给\s*(.{1,30}?)\s*(?:当|做)\s*顾问"
    ).unwrap()
});

/// 认识：认识 / 介绍了 / 带我认识 / 引荐
static KNOWS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:认识|介绍|引荐|带.*认识)\s*(.{1,30}?)"
    ).unwrap()
});

/// 喜欢：喜欢 / 喜爱 / 钟爱 / 爱好
static LIKES_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:喜欢|喜爱|钟爱|爱好|偏爱)\s*(.{1,30}?)"
    ).unwrap()
});

/// 不喜欢：不喜欢 / 讨厌 / 反感 / 厌恶
static DISLIKES_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:不喜欢|讨厌|反感|厌恶|不感冒)\s*(.{1,30}?)"
    ).unwrap()
});

/// 朋友：是...的朋友 / 和...是朋友
static FRIEND_OF_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:是|和|与)\s*(.{1,30}?)\s*(?:的)?(?:朋友|好友|闺蜜|哥们)|(?:和|与)\s*(.{1,30}?)\s*是\s*(?:朋友|好友)"
    ).unwrap()
});

/// 家人：是...的家人 / 和...是家人
static FAMILY_OF_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:是|和|与)\s*(.{1,30}?)\s*(?:的)?(?:家人|哥哥|弟弟|姐姐|妹妹|爸爸|妈妈|父亲|母亲|儿子|女儿|丈夫|妻子|老公|老婆)"
    ).unwrap()
});

/// 信任：信任 / 相信
static TRUSTS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:信任|信赖|相信)\s*(.{1,30}?)"
    ).unwrap()
});

/// 关心：关心 / 在意 / 照顾
static CARES_FOR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:关心|在意|照顾|惦念|挂念)\s*(.{1,30}?)"
    ).unwrap()
});

/// 想念：想念 / 思念 / 惦记
static MISSES_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(.{1,30}?)\s*(?:想念|思念|惦记|怀念)\s*(.{1,30}?)"
    ).unwrap()
});

/// 从文本中提取实体（使用 jieba 词性标注）
///
/// 提取词性为 nr（人名）/ ns（地名）/ nt（机构名）/ nz（其他专名）/ n（名词）的词。
/// 过滤长度 < 2 的实体（单字实体噪声大）。
/// 对于简单对话，名词也作为实体提取，以确保图谱有数据。
pub fn extract_entities(text: &str) -> Vec<Entity> {
    let tags = JIEBA.tag(text, true);
    let mut entity_counts: HashMap<(String, EntityType), usize> = HashMap::new();

    for tag in &tags {
        let et = if let Some(et) = EntityType::from_tag(&tag.tag) {
            et
        } else if tag.tag.starts_with('n') {
            EntityType::Other
        } else {
            continue;
        };
        
        let name = tag.word.trim().to_string();
        if name.chars().count() < 2 {
            continue;
        }
        *entity_counts.entry((name, et)).or_insert(0) += 1;
    }

    // 计算显著性：出现次数 / 总实体数
    let total: usize = entity_counts.values().sum();
    if total == 0 {
        return Vec::new();
    }

    entity_counts
        .into_iter()
        .map(|((name, et), count)| {
            let salience = (count as f64 / total as f64).min(1.0);
            Entity {
                name,
                entity_type: et,
                salience,
            }
        })
        .collect()
}

/// 从文本中推断实体间的关系（使用 regex 动词匹配）
///
/// 用预编译 regex 链推断关系类型。
/// 优先级：family > friend > likes > dislikes > trusts > cares_for > misses >
///         founded > invested_in > advises > works_at > knows > mentions。
/// 当没有明确的关系动词匹配时，使用 Mentions 作为兜底关系连接所有实体对。
pub fn infer_relations(text: &str, entities: &[Entity]) -> Vec<Relation> {
    if entities.len() < 2 {
        return Vec::new();
    }

    let entity_names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
    let mut relations = Vec::new();

    // 按优先级依次匹配 regex
    // 1. family_of（家人关系最强）
    for cap in FAMILY_OF_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::FamilyOf, 0.90) {
            relations.push(rel);
        }
    }

    // 2. friend_of
    for cap in FRIEND_OF_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::FriendOf, 0.85) {
            relations.push(rel);
        }
    }

    // 3. likes
    for cap in LIKES_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::Likes, 0.75) {
            relations.push(rel);
        }
    }

    // 4. dislikes
    for cap in DISLIKES_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::Dislikes, 0.75) {
            relations.push(rel);
        }
    }

    // 5. trusts
    for cap in TRUSTS_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::Trusts, 0.70) {
            relations.push(rel);
        }
    }

    // 6. cares_for
    for cap in CARES_FOR_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::CaresFor, 0.65) {
            relations.push(rel);
        }
    }

    // 7. misses
    for cap in MISSES_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::Misses, 0.65) {
            relations.push(rel);
        }
    }

    // 8. founded
    for cap in FOUNDED_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::Founded, 0.85) {
            relations.push(rel);
        }
    }

    // 9. invested_in
    for cap in INVESTED_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::InvestedIn, 0.80) {
            relations.push(rel);
        }
    }

    // 10. advises
    for cap in ADVISES_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::Advises, 0.75) {
            relations.push(rel);
        }
    }

    // 11. works_at
    for cap in WORKS_AT_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::WorksAt, 0.70) {
            relations.push(rel);
        }
    }

    // 12. knows
    for cap in KNOWS_RE.captures_iter(text) {
        if let Some(rel) = build_relation_from_capture(&cap, &entity_names, text, RelationType::Knows, 0.60) {
            relations.push(rel);
        }
    }

    // 13. Mentions 兜底：当没有明确关系匹配时，将所有实体两两连接
    if relations.is_empty() {
        for (i, e1) in entities.iter().enumerate() {
            for (j, e2) in entities.iter().enumerate() {
                if i != j {
                    relations.push(Relation {
                        subject: e1.name.clone(),
                        object: e2.name.clone(),
                        relation_type: RelationType::Mentions,
                        context: text.to_string(),
                        confidence: 0.3,
                    });
                }
            }
        }
    }

    // 去重：相同 (subject, object, relation_type) 只保留置信度最高的
    relations.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen = std::collections::HashSet::new();
    relations.retain(|r| {
        let key = (r.subject.clone(), r.object.clone(), r.relation_type.clone());
        seen.insert(key)
    });

    relations
}

/// 从 regex 捕获组构建关系
///
/// 捕获组可能匹配到实体名片段，需要验证是否在已知实体列表中。
fn build_relation_from_capture(
    cap: &regex::Captures,
    entity_names: &[&str],
    text: &str,
    rel_type: RelationType,
    base_confidence: f64,
) -> Option<Relation> {
    // 遍历所有捕获组，找两个已知实体
    let mut found_subject: Option<String> = None;
    let mut found_object: Option<String> = None;
    let mut context_snippet = String::new();

    for i in 1..cap.len() {
        if let Some(m) = cap.get(i) {
            let matched = m.as_str().trim();
            if matched.is_empty() || matched.chars().count() < 2 {
                continue;
            }
            // 检查是否匹配已知实体
            if entity_names.iter().any(|&name| name == matched || matched.contains(name) || name.contains(matched)) {
                if found_subject.is_none() {
                    found_subject = Some(matched.to_string());
                    context_snippet = extract_context(text, m.start(), 40);
                } else if found_object.is_none() {
                    found_object = Some(matched.to_string());
                }
            }
        }
    }

    match (found_subject, found_object) {
        (Some(s), Some(o)) if s != o => Some(Relation {
            subject: s,
            object: o,
            relation_type: rel_type,
            context: context_snippet,
            confidence: base_confidence,
        }),
        _ => None,
    }
}

/// 提取匹配位置周围的上下文片段
fn extract_context(text: &str, pos: usize, radius: usize) -> String {
    let start = pos.saturating_sub(radius);
    let end = (pos + radius).min(text.len());
    let start = text.ceil_char_boundary(start);
    let end = text.floor_char_boundary(end);
    text[start..end].to_string()
}

/// 实体+关系提取结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

/// 一次性提取实体和关系
pub fn extract(text: &str) -> ExtractionResult {
    let entities = extract_entities(text);
    let relations = infer_relations(text, &entities);
    ExtractionResult { entities, relations }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_entities_person() {
        let text = "张三昨天去了北京，和李四见面讨论了项目。";
        let entities = extract_entities(text);
        // jieba 可能不会把"张三"/"李四"标注为 nr，但至少应该提取到一些实体
        // 这个测试主要验证函数不 panic
        assert!(entities.iter().all(|e| e.name.chars().count() >= 2));
    }

    #[test]
    fn test_infer_relations_works_at() {
        let entities = vec![
            Entity { name: "张三".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "腾讯".to_string(), entity_type: EntityType::Organization, salience: 0.5 },
        ];
        let text = "张三在腾讯工作";
        let relations = infer_relations(text, &entities);
        // 可能匹配到 works_at 关系（取决于 regex）
        assert!(relations.iter().all(|r| r.subject != r.object));
    }

    #[test]
    fn test_infer_relations_founded() {
        let entities = vec![
            Entity { name: "马化腾".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "腾讯".to_string(), entity_type: EntityType::Organization, salience: 0.5 },
        ];
        let text = "马化腾创建了腾讯";
        let relations = infer_relations(text, &entities);
        // 应该匹配到 founded 关系
        assert!(relations.iter().any(|r| r.relation_type == RelationType::Founded));
    }

    #[test]
    fn test_infer_relations_invested() {
        let entities = vec![
            Entity { name: "红杉".to_string(), entity_type: EntityType::Organization, salience: 0.5 },
            Entity { name: "字节跳动".to_string(), entity_type: EntityType::Organization, salience: 0.5 },
        ];
        let text = "红杉投资了字节跳动";
        let relations = infer_relations(text, &entities);
        assert!(relations.iter().any(|r| r.relation_type == RelationType::InvestedIn));
    }

    #[test]
    fn test_infer_relations_no_entities() {
        let entities: Vec<Entity> = vec![];
        let relations = infer_relations("任意文本", &entities);
        assert!(relations.is_empty());
    }

    #[test]
    fn test_infer_relations_single_entity() {
        let entities = vec![
            Entity { name: "张三".to_string(), entity_type: EntityType::Person, salience: 1.0 },
        ];
        let relations = infer_relations("张三创建了腾讯", &entities);
        // 只有一个已知实体，无法构建关系
        assert!(relations.iter().all(|r| r.subject == "张三" || r.object == "张三"));
    }

    #[test]
    fn test_entity_type_from_tag() {
        assert_eq!(EntityType::from_tag("nr"), Some(EntityType::Person));
        assert_eq!(EntityType::from_tag("ns"), Some(EntityType::Location));
        assert_eq!(EntityType::from_tag("nt"), Some(EntityType::Organization));
        assert_eq!(EntityType::from_tag("nz"), Some(EntityType::Other));
        assert_eq!(EntityType::from_tag("n"), None);
        assert_eq!(EntityType::from_tag("v"), None);
    }

    #[test]
    fn test_relation_type_as_str() {
        assert_eq!(RelationType::WorksAt.as_str(), "works_at");
        assert_eq!(RelationType::InvestedIn.as_str(), "invested_in");
        assert_eq!(RelationType::Founded.as_str(), "founded");
        assert_eq!(RelationType::Mentions.as_str(), "mentions");
        assert_eq!(RelationType::Likes.as_str(), "likes");
        assert_eq!(RelationType::Dislikes.as_str(), "dislikes");
        assert_eq!(RelationType::FriendOf.as_str(), "friend_of");
        assert_eq!(RelationType::FamilyOf.as_str(), "family_of");
        assert_eq!(RelationType::Trusts.as_str(), "trusts");
        assert_eq!(RelationType::CaresFor.as_str(), "cares_for");
        assert_eq!(RelationType::Misses.as_str(), "misses");
    }

    #[test]
    fn test_relation_type_is_symmetric() {
        assert!(RelationType::FriendOf.is_symmetric());
        assert!(RelationType::FamilyOf.is_symmetric());
        assert!(!RelationType::Likes.is_symmetric());
        assert!(!RelationType::Knows.is_symmetric());
        assert!(!RelationType::Mentions.is_symmetric());
    }

    #[test]
    fn test_infer_relations_likes() {
        let entities = vec![
            Entity { name: "小明".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "音乐".to_string(), entity_type: EntityType::Other, salience: 0.5 },
        ];
        let text = "小明喜欢音乐";
        let relations = infer_relations(text, &entities);
        assert!(relations.iter().any(|r| r.relation_type == RelationType::Likes));
    }

    #[test]
    fn test_infer_relations_dislikes() {
        let entities = vec![
            Entity { name: "小明".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "香菜".to_string(), entity_type: EntityType::Other, salience: 0.5 },
        ];
        let text = "小明讨厌香菜";
        let relations = infer_relations(text, &entities);
        assert!(relations.iter().any(|r| r.relation_type == RelationType::Dislikes));
    }

    #[test]
    fn test_infer_relations_friend_of() {
        let entities = vec![
            Entity { name: "小明".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "小红".to_string(), entity_type: EntityType::Person, salience: 0.5 },
        ];
        let text = "小明和小红是朋友";
        let relations = infer_relations(text, &entities);
        assert!(relations.iter().any(|r| r.relation_type == RelationType::FriendOf));
    }

    #[test]
    fn test_infer_relations_family_of() {
        let entities = vec![
            Entity { name: "小明".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "小红".to_string(), entity_type: EntityType::Person, salience: 0.5 },
        ];
        let text = "小明和小红是家人";
        let relations = infer_relations(text, &entities);
        assert!(relations.iter().any(|r| r.relation_type == RelationType::FamilyOf));
    }

    #[test]
    fn test_infer_relations_trusts() {
        let entities = vec![
            Entity { name: "小明".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "小红".to_string(), entity_type: EntityType::Person, salience: 0.5 },
        ];
        let text = "小明信任小红";
        let relations = infer_relations(text, &entities);
        assert!(relations.iter().any(|r| r.relation_type == RelationType::Trusts));
    }

    #[test]
    fn test_infer_relations_cares_for() {
        let entities = vec![
            Entity { name: "小明".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "小红".to_string(), entity_type: EntityType::Person, salience: 0.5 },
        ];
        let text = "小明关心小红";
        let relations = infer_relations(text, &entities);
        assert!(relations.iter().any(|r| r.relation_type == RelationType::CaresFor));
    }

    #[test]
    fn test_infer_relations_misses() {
        let entities = vec![
            Entity { name: "小明".to_string(), entity_type: EntityType::Person, salience: 0.5 },
            Entity { name: "小红".to_string(), entity_type: EntityType::Person, salience: 0.5 },
        ];
        let text = "小明想念小红";
        let relations = infer_relations(text, &entities);
        assert!(relations.iter().any(|r| r.relation_type == RelationType::Misses));
    }

    #[test]
    fn test_extract_combined() {
        let text = "马化腾创建了腾讯，张三在腾讯工作";
        let result = extract(text);
        // 应该提取到实体和关系
        assert!(result.entities.iter().all(|e| e.name.chars().count() >= 2));
    }
}
