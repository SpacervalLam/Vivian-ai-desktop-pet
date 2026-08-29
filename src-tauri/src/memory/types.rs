use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Open Hook —— 记忆条目携带的"未闭环钩子"
///
/// 当一条记忆包含承诺、约定、待跟进事项等未完成的内容时，LLM 在写入时抽取 hook，
/// 后续对话中由 HookJudge 异步判定是否闭环。未闭环的 hook 在检索时获得 boost，
/// 闭环后记录闭环来源。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenHook {
    /// 钩子类型：promise（承诺）/ follow_up（待跟进）/ schedule（约定）/ question（待回答）
    #[serde(rename = "type")]
    pub hook_type: String,
    /// 闭环条件的自然语言描述（如"用户下次提到已还款"）
    pub condition: String,
    /// 创建时间戳（Unix 秒）
    pub created_at: f64,
    /// 闭环时间戳（None 表示未闭环）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<f64>,
    /// 闭环来源记忆 ID（闭环时记录是哪条新记忆/对话触发的）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<String>,
}

impl OpenHook {
    pub fn new(hook_type: impl Into<String>, condition: impl Into<String>) -> Self {
        Self {
            hook_type: hook_type.into(),
            condition: condition.into(),
            created_at: current_timestamp(),
            closed_at: None,
            closed_by: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }

    /// 标记闭环
    pub fn close(&mut self, closed_by: Option<String>) {
        self.closed_at = Some(current_timestamp());
        self.closed_by = closed_by;
    }
}

/// 记忆语义类型 —— 陪伴型智能体特有分类维度
///
/// 与 `MemoryType`（时长/来源维度）正交，描述记忆的**语义内容**。
/// 由 `MemoryEnricher` 在写入时通过 LLM 分类，持久化到 `metadata["semantic_type"]`。
/// 读路径直接读取，不调用 LLM（符合"LLM 分类只在写操作"硬约束）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticType {
    /// 用户身份/偏好/性格（如"喜欢晚上看书"）
    User,
    /// 用户对 Vivian 行为的反馈（如"不喜欢被叫宝贝"）
    Feedback,
    /// 关系事件（亲密度变化、共同回忆、约定）
    Relationship,
    /// 共同经历的对话/事件（如"上周一起讨论了 X"）
    SharedMemory,
    /// 用户当前的项目/任务（编程/工作上下文）
    Project,
    /// 外部信息指针（链接/书签/引用）
    Reference,
    /// 一般对话（无特殊语义价值）
    General,
}

impl SemanticType {
    pub fn all() -> Vec<SemanticType> {
        vec![
            SemanticType::User,
            SemanticType::Feedback,
            SemanticType::Relationship,
            SemanticType::SharedMemory,
            SemanticType::Project,
            SemanticType::Reference,
            SemanticType::General,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SemanticType::User => "user",
            SemanticType::Feedback => "feedback",
            SemanticType::Relationship => "relationship",
            SemanticType::SharedMemory => "shared_memory",
            SemanticType::Project => "project",
            SemanticType::Reference => "reference",
            SemanticType::General => "general",
        }
    }

    pub fn from_str(s: &str) -> Option<SemanticType> {
        match s {
            "user" => Some(SemanticType::User),
            "feedback" => Some(SemanticType::Feedback),
            "relationship" => Some(SemanticType::Relationship),
            "shared_memory" => Some(SemanticType::SharedMemory),
            "project" => Some(SemanticType::Project),
            "reference" => Some(SemanticType::Reference),
            "general" => Some(SemanticType::General),
            _ => None,
        }
    }

    /// 该类型记忆的默认重要性下限（用于 enricher 失败时的兜底）
    pub fn default_importance(&self) -> f64 {
        match self {
            SemanticType::User => 0.8,
            SemanticType::Feedback | SemanticType::Relationship => 0.7,
            SemanticType::SharedMemory | SemanticType::Project => 0.5,
            SemanticType::Reference => 0.4,
            SemanticType::General => 0.2,
        }
    }
}

/// 规范标签集合 —— 记忆 tags 字段只允许使用以下标签
///
/// LLM 生成的自由关键词不得进入 tags，应存入 metadata["keywords"] 供搜索使用。
/// tags 仅来源于结构化字段（MemoryType / SemanticType / 调用方传入的规范标签）。
pub fn canonical_tags() -> &'static [&'static str] {
    &[
        // 记忆类型标签（MemoryType.as_str）
        "short_term", "mid_term", "long_term",
        "user", "feedback", "project", "reference", "general",
        "preference", "identity", "important_event", "knowledge",
        "temporary_context", "casual_conversation",
        "session_summary", "insight", "inner_monologue",
        // 语义类型标签（SemanticType.as_str）
        "shared_memory", "relationship",
        // 来源/来源标签
        "llm_generated", "inner_os", "autonomous",
        // 主语归属标签
        "vivian",
        // 抽取器类型标签
        "user_profile", "project_context", "health",
        // 服务层标签
        "long_term_preference", "daily_diary", "diary",
        // 情绪标签
        "joy", "sadness", "anger", "fear", "closeness", "loneliness", "curiosity",
        "neutral", "happy", "shy", "curious", "excited", "bored",
        "caring", "sleepy", "playful", "proud", "grateful", "apologetic",
    ]
}

/// 判断一个标签是否属于规范标签集合
pub fn is_canonical_tag(tag: &str) -> bool {
    canonical_tags().iter().any(|&t| t == tag)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Turn,
    Session,
    Summary,
    Keyword,
}

impl Granularity {
    pub fn all() -> Vec<Granularity> {
        vec![
            Granularity::Turn,
            Granularity::Session,
            Granularity::Summary,
            Granularity::Keyword,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Granularity::Turn => "turn",
            Granularity::Session => "session",
            Granularity::Summary => "summary",
            Granularity::Keyword => "keyword",
        }
    }
}

impl FromStr for Granularity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "turn" => Ok(Granularity::Turn),
            "session" => Ok(Granularity::Session),
            "summary" => Ok(Granularity::Summary),
            "keyword" => Ok(Granularity::Keyword),
            other => Err(format!("未知粒度: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    // 时长类型
    ShortTerm,
    MidTerm,
    LongTerm,
    // 内容类型
    User,
    Feedback,
    Project,
    Reference,
    General,
    // 用户偏好与身份
    Preference,
    Identity,
    ImportantEvent,
    Knowledge,
    TemporaryContext,
    CasualConversation,
    /// 旁观记录：第三者视角观察到的他人对话，不含原文，仅作索引引用
    ObservationNote,
    // 巩固流水线产物
    /// 会话级摘要：Stage 1 把多轮 ShortTerm 摘要成 MidTerm SessionSummary。
    /// 与 `CasualConversation`（单轮原文）互补，代表"一段对话的主题级压缩"。
    SessionSummary,
    /// Stage 3 反思生成的高层洞察（insight），由多条 LongTerm 聚类抽象而来。
    Insight,
    /// 内心独白：Vivian 在用户不交互时自主思考的记录，写入记忆供将来检索
    InnerMonologue,
}

impl MemoryType {
    pub fn from_str(s: &str) -> Option<MemoryType> {
        match s {
            "short_term" => Some(MemoryType::ShortTerm),
            "mid_term" => Some(MemoryType::MidTerm),
            "long_term" => Some(MemoryType::LongTerm),
            "user" => Some(MemoryType::User),
            "feedback" => Some(MemoryType::Feedback),
            "project" => Some(MemoryType::Project),
            "reference" => Some(MemoryType::Reference),
            "general" => Some(MemoryType::General),
            "preference" => Some(MemoryType::Preference),
            "identity" => Some(MemoryType::Identity),
            "important_event" => Some(MemoryType::ImportantEvent),
            "knowledge" => Some(MemoryType::Knowledge),
            "temporary_context" => Some(MemoryType::TemporaryContext),
            "casual_conversation" => Some(MemoryType::CasualConversation),
            "observation_note" => Some(MemoryType::ObservationNote),
            "session_summary" => Some(MemoryType::SessionSummary),
            "insight" => Some(MemoryType::Insight),
            "inner_monologue" => Some(MemoryType::InnerMonologue),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::ShortTerm => "short_term",
            MemoryType::MidTerm => "mid_term",
            MemoryType::LongTerm => "long_term",
            MemoryType::User => "user",
            MemoryType::Feedback => "feedback",
            MemoryType::Project => "project",
            MemoryType::Reference => "reference",
            MemoryType::General => "general",
            MemoryType::Preference => "preference",
            MemoryType::Identity => "identity",
            MemoryType::ImportantEvent => "important_event",
            MemoryType::Knowledge => "knowledge",
            MemoryType::TemporaryContext => "temporary_context",
            MemoryType::CasualConversation => "casual_conversation",
            MemoryType::ObservationNote => "observation_note",
            MemoryType::SessionSummary => "session_summary",
            MemoryType::Insight => "insight",
            MemoryType::InnerMonologue => "inner_monologue",
        }
    }

    pub fn default_granularity(&self) -> Granularity {
        match self {
            MemoryType::ShortTerm | MemoryType::CasualConversation => Granularity::Turn,
            MemoryType::ObservationNote => Granularity::Summary,
            MemoryType::MidTerm | MemoryType::TemporaryContext => Granularity::Session,
            // SessionSummary 与 Insight 与 InnerMonologue 都按摘要粒度沉淀
            MemoryType::SessionSummary | MemoryType::Insight | MemoryType::InnerMonologue => Granularity::Summary,
            // 长期/偏好/身份/重要事件/知识 都按摘要粒度沉淀
            MemoryType::LongTerm
            | MemoryType::User
            | MemoryType::Feedback
            | MemoryType::Project
            | MemoryType::Reference
            | MemoryType::General
            | MemoryType::Preference
            | MemoryType::Identity
            | MemoryType::ImportantEvent
            | MemoryType::Knowledge => Granularity::Summary,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetrievalStrategy {
    Auto,
    Keyword,
    Vector,
    Hybrid,
    /// 仅走知识图谱路（relational recall）
    Graph,
}

impl RetrievalStrategy {
    pub fn from_str(s: &str) -> RetrievalStrategy {
        match s.to_lowercase().as_str() {
            "keyword" => RetrievalStrategy::Keyword,
            "vector" => RetrievalStrategy::Vector,
            "hybrid" => RetrievalStrategy::Hybrid,
            "graph" => RetrievalStrategy::Graph,
            _ => RetrievalStrategy::Auto,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: String,
    pub content: String,
    pub granularity: String,
    /// 原始记忆类型（casual_conversation/inner_monologue/long_term 等），前端分类展示依赖此字段
    #[serde(default)]
    pub memory_type: String,
    pub importance: f64,
    /// Unix 时间戳（秒）；序列化为 created_at（前端字段名），兼容旧字段名 timestamp
    #[serde(default, rename = "created_at", alias = "timestamp")]
    pub timestamp: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f64>>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub related_ids: Vec<String>,
    /// 一句话描述，用于检索时 manifest 与 LLM 选择
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 检索命中次数（热度机制）
    #[serde(default)]
    pub visit_count: u32,
    /// 最近一次被检索命中的时间戳（秒）
    #[serde(default)]
    pub last_visit_at: f64,
    /// 综合热度分数：α·visit_count + β·length + γ·recency
    #[serde(default)]
    pub heat_score: f64,
    /// 未闭环钩子列表（承诺/约定/待跟进事项）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_hooks: Vec<OpenHook>,
    // ── 证据驱动可信度系统 ──────────────────────────────────
    /// 正面证据累积量（用户陈述/确认强化）。半衰期 30 天。
    #[serde(default)]
    pub reinforcement: f64,
    /// 负面证据累积量（用户反驳/否定）。非负，半衰期 180 天。
    #[serde(default)]
    pub disputation: f64,
    /// 最近一次正面信号时间戳（Unix 秒）。两侧独立时钟，仅在被触动时重置。
    #[serde(default)]
    pub rein_last_signal_at: f64,
    /// 最近一次负面信号时间戳（Unix 秒）。
    #[serde(default)]
    pub disp_last_signal_at: f64,
    /// score < 0 的累计自然日数。达 EVIDENCE_ARCHIVE_DAYS 触发真正归档。
    #[serde(default)]
    pub sub_zero_days: u32,
    /// 上次递增 sub_zero_days 的日期（YYYY-MM-DD），防同日多次计数。
    #[serde(default)]
    pub sub_zero_last_increment_date: String,
    /// user_fact 正面信号的累计次数（combo 计数器，永不重置）。
    #[serde(default)]
    pub user_fact_reinforce_count: u32,
    /// 角色卡/重要事件来源标记。protected=true 时 evidence_score 返回 +∞，永不归档。
    #[serde(default)]
    pub protected: bool,
    /// Episode 归属：该记忆所属的经历封包 ID（如 "ep_20260716_2030_exam"）。
    /// None 表示尚未被任何 Episode 封包收录（旧记忆默认值）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    /// 反驳后重新确认宽限期（剩余次数）。
    /// 用户反驳后设为 3，每次正面信号 -1 且 delta 减半。
    /// 归零前 sub_zero_days 不会因 score 回正而重置。
    #[serde(default)]
    pub rebuttal_grace_remaining: u32,
    /// 已被整合归档标记。consolidated=true 的记忆不参与检索，
    /// 但保留在存储中作为审计追溯（取代原来的硬删除）。
    #[serde(default)]
    pub consolidated: bool,
}

impl MemoryItem {
    pub fn new(content: String, granularity: Granularity, importance: f64) -> Self {
        let id = format!("mem_{}", &uuid::Uuid::new_v4().to_string()[..12]);
        Self {
            id,
            content,
            granularity: granularity.as_str().to_string(),
            memory_type: String::new(),
            importance,
            timestamp: current_timestamp(),
            embedding: None,
            tags: Vec::new(),
            metadata: serde_json::json!({}),
            related_ids: Vec::new(),
            description: None,
            visit_count: 0,
            last_visit_at: 0.0,
            heat_score: 0.0,
            open_hooks: Vec::new(),
            reinforcement: 0.0,
            disputation: 0.0,
            rein_last_signal_at: 0.0,
            disp_last_signal_at: 0.0,
            sub_zero_days: 0,
            sub_zero_last_increment_date: String::new(),
            user_fact_reinforce_count: 0,
            protected: false,
            episode_id: None,
            rebuttal_grace_remaining: 0,
            consolidated: false,
        }
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        if self.content.to_lowercase().contains(&q) {
            return true;
        }
        for tag in &self.tags {
            if tag.to_lowercase().contains(&q) {
                return true;
            }
        }
        false
    }

    pub fn match_score(&self, query: &str) -> f64 {
        let q = query.to_lowercase();
        let mut score = self.importance;
        if self.content.to_lowercase().contains(&q) {
            score += 1.0;
        }
        for tag in &self.tags {
            if tag.to_lowercase().contains(&q) {
                score += 0.5;
            }
        }
        let age_hours = (current_timestamp() - self.timestamp).max(0.0) / 3600.0;
        let recency_factor = (-age_hours / 24.0).exp();
        score * (0.7 + 0.3 * recency_factor)
    }

    /// 从 metadata 中读取语义类型（由 MemoryEnricher 在写入时分类）。
    ///
    /// 符合"LLM 分类只在写操作"硬约束：读路径直接读取，不调用 LLM。
    pub fn semantic_type(&self) -> SemanticType {
        self.metadata
            .get("semantic_type")
            .and_then(|v| v.as_str())
            .and_then(SemanticType::from_str)
            .unwrap_or(SemanticType::General)
    }

    /// 从 metadata 读取关键词列表（由 MemoryEnricher 在写入时抽取）。
    pub fn keywords(&self) -> Vec<String> {
        self.metadata
            .get("keywords")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 从 metadata 读取主语范围（如 "vivian" / "user" / "project_x"）。
    /// 用于 retrieve_memory 工具的 subject_scopes 过滤。
    pub fn subject_scopes(&self) -> Vec<String> {
        self.metadata
            .get("subject_scopes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 从 metadata 读取分类标签列表。
    /// 与 tags 字段互补：tags 是规范标签，categories 是自由分类（如 "work" / "hobby"）。
    pub fn categories(&self) -> Vec<String> {
        self.metadata
            .get("categories")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 从 metadata 读取记忆产生时的时段标签（如 "morning" / "afternoon" / "evening" / "night"）。
    pub fn time_of_day(&self) -> Option<String> {
        self.metadata
            .get("time_of_day")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// 从 metadata 读取情绪余温标签列表（0-3 个，由 MemoryEnricher 在写入时抽取）。
    pub fn mood_tags(&self) -> Vec<String> {
        self.metadata
            .get("mood_tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 从 metadata 读取记忆产生时的日期标签（如 "2024-01-15" 或 "昨天"）。
    pub fn date_label(&self) -> Option<String> {
        self.metadata
            .get("date_label")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// 从 metadata 读取原始对话序号（用于上下文扩窗：按 seq_no ±1 取邻接记忆）。
    pub fn seq_no(&self) -> Option<u64> {
        self.metadata.get("seq_no").and_then(|v| v.as_u64())
    }

    /// 记忆来源层级：raw（原始对话）/ episodic（情节摘要）/ semantic（语义洞察）。
    /// 基于 granularity 字段推断，用于 retrieve_memory 的 source_layers 过滤。
    pub fn source_layer(&self) -> &'static str {
        match self.granularity.as_str() {
            "turn" => "raw",
            "session" => "episodic",
            _ => "semantic",
        }
    }

    /// 该记忆所属的 Episode ID（若已封包）。
    pub fn episode_id(&self) -> Option<&str> {
        self.episode_id.as_deref()
    }
}

pub fn current_timestamp() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStoreData {
    pub version: u32,
    pub entries: Vec<MemoryItem>,
}

impl Default for MemoryStoreData {
    fn default() -> Self {
        Self {
            version: 4,
            entries: Vec::new(),
        }
    }
}
