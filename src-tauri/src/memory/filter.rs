//! 记忆过滤器：区分临时话题与长期偏好，避免跨会话强行关联无关话题。
//!
//! 核心流程：检测新会话 → 过滤临时话题 → 仅保留长期偏好，并按时间衰减计权。
//! 文本判定基于 jieba 分词 + 词性标注。

use chrono::{DateTime, Local};
use jieba_rs::{Jieba, Tag};
use once_cell::sync::Lazy;
use uuid::Uuid;

use super::relaxation::RelaxationLadder;
use super::types::{current_timestamp, MemoryItem};

/// 全局 jieba 实例（首次使用时初始化）
static JIEBA: Lazy<Jieba> = Lazy::new(Jieba::new);

/// 长期偏好判定的最小内容长度
const LONG_TERM_MIN_LEN: usize = 8;

/// 临时话题判定的短陈述阈值
const TEMPORARY_SHORT_THRESHOLD: usize = 20;

/// 权重过低的截断阈值
const MIN_MEMORY_WEIGHT: f64 = 0.01;

/// 放松阶梯最小结果数：strict 路径返回少于此值时逐级放松约束。
///
const RELAXATION_MIN_RESULTS: usize = 3;

/// 记忆过滤器：跨会话过滤临时话题、保留长期偏好的核心组件。
pub struct MemoryFilter {
    /// 上次交互时间；None 表示首次交互
    last_interaction_time: Option<DateTime<Local>>,
    /// 当前会话 ID，用于区分语义上断开的对话片段
    pub last_session_id: Option<String>,
    /// 最近一次会话的关键词，用于判断主题连续性
    last_topic_keywords: Vec<String>,
}

/// jieba 分词 + 词性标注。返回 `Tag` 列表，每个元素含 `word` 和 `tag`。
/// 词性首字母：r=代词、t=时间词、y=语气词、v=动词、n=名词、a=形容词、d=副词。
fn tag_tokens(text: &str) -> Vec<Tag<'_>> {
    JIEBA.tag(text, true)
}

/// 是否为疑问句：含语气词"吗/呢/吧"（词性 `y`）或疑问代词（词性 `r`）。
fn is_question(tokens: &[Tag<'_>]) -> bool {
    tokens.iter().any(|t| {
        matches!(t.word, "吗" | "呢" | "吧") && t.tag.starts_with('y')
    }) || tokens.iter().any(|t| {
        matches!(t.word, "怎么" | "什么" | "为什么" | "哪" | "哪里" | "怎样")
            && t.tag.starts_with('r')
    })
}

/// 提取实词关键词（用于话题交集判定）。保留名词/动词/形容词/副词，过滤虚词与单字。
fn content_keywords(text: &str) -> Vec<String> {
    let tokens = tag_tokens(text);
    tokens
        .into_iter()
        .filter(|t| {
            matches!(t.tag.chars().next(), Some('n') | Some('v') | Some('a') | Some('d'))
                && t.word.chars().count() >= 2
        })
        .map(|t| t.word.to_string())
        .collect()
}

impl MemoryFilter {
    pub fn new() -> Self {
        Self {
            last_interaction_time: None,
            last_session_id: None,
            last_topic_keywords: Vec::new(),
        }
    }

    fn update_session_state(&mut self, current_keywords: Vec<String>) {
        self.last_topic_keywords = current_keywords;
        self.last_interaction_time = Some(Local::now());
        if self.last_session_id.is_none() {
            self.last_session_id = Some(Uuid::new_v4().to_string());
        }
    }

    fn start_new_session(&mut self, current_keywords: Vec<String>) {
        self.last_session_id = Some(Uuid::new_v4().to_string());
        self.last_topic_keywords = current_keywords;
        self.last_interaction_time = Some(Local::now());
    }

    /// 是否为新会话：以 ConversationManager 状态机为单一真相源
    ///
    /// 判定规则：
    /// - 会话不存在（首次交互）→ 新会话
    /// - 会话刚创建（rounds ≤ 1 或 state == Created）→ 新会话
    /// - 会话已关闭（Closed）→ 新会话
    /// - 会话 Active 且 rounds > 1 → 老会话
    ///
    /// 旧的启发式逻辑（1 小时阈值/问候语/短输入/话题断裂）已移除，
    /// 由 ConversationManager 的状态机 + 关键词检测统一负责。
    pub fn is_new_session(&self, _user_input: &str, char_id: &str) -> bool {
        let mgr = &crate::conversation::CONVERSATION_MANAGER;
        match mgr.get("user", char_id) {
            None => true,
            Some(conv) => {
                conv.state == crate::conversation::ConversationState::Created
                    || conv.state == crate::conversation::ConversationState::Closed
                    || conv.rounds <= 1
            }
        }
    }

    /// 是否为长期偏好：含第一人称代词"我"（词性 `r`）+ 长度 ≥8 + 非疑问句。
    pub fn is_long_term_preference(content: &str) -> bool {
        let tokens = tag_tokens(content);
        let has_first_person = tokens
            .iter()
            .any(|t| t.word == "我" && t.tag.starts_with('r'));
        let has_reasonable_length = content.chars().count() >= LONG_TERM_MIN_LEN;
        let is_not_question = !is_question(&tokens);
        has_first_person && has_reasonable_length && is_not_question
    }

    /// 是否为临时话题：含时间词（词性 `t`）或长度 <20。
    pub fn is_temporary_topic(content: &str) -> bool {
        let tokens = tag_tokens(content);
        let has_time_word = tokens.iter().any(|t| t.tag.starts_with('t'));
        let is_short_statement = content.chars().count() < TEMPORARY_SHORT_THRESHOLD;
        has_time_word || is_short_statement
    }

    /// 获取记忆年龄（小时）。
    pub fn memory_age_hours(memory: &MemoryItem) -> f64 {
        let now = current_timestamp();
        ((now - memory.timestamp).max(0.0)) / 3600.0
    }

    /// 是否属于长期类记忆：tags 命中长期类标签，或 memory_type 本身即长期类。
    ///
    /// 种子记忆把 important_event / long_term 写在 memory_type 字段（tags 是
    /// world_canon/backstory 等叙事标签），仅查 tags 会把角色前史种子全部漏判，
    /// 导致新会话严格过滤（RelaxationLadder）把"创造者"这类核心设定记忆淘汰。
    pub fn is_tagged_long_term(memory: &MemoryItem) -> bool {
        let tag_hit = memory.tags.iter().any(|tag| {
            matches!(tag.to_lowercase().as_str(),
                "long_term" | "summary" | "identity" | "preference" | "important_event" | "knowledge" | "user_preferences" | "user_identity"
            )
        });
        tag_hit || matches!(
            memory.memory_type.to_lowercase().as_str(),
            "long_term" | "important_event" | "preference" | "identity" | "knowledge" | "session_summary"
        )
    }

    pub fn is_tagged_temporary(memory: &MemoryItem) -> bool {
        memory.tags.iter().any(|tag| {
            matches!(tag.to_lowercase().as_str(),
                "temporary" | "turn" | "session" | "keyword" | "short" | "recent"
            )
        })
    }

    /// 计算记忆权重：长期偏好基础 1.0 慢衰减，临时话题基础 0.3 快衰减；
    /// 超过 1 小时的临时话题额外减半。
    pub fn calculate_memory_weight(memory: &MemoryItem) -> f64 {
        let hours = Self::memory_age_hours(memory);

        let (base_weight, decay_rate) = if Self::is_tagged_long_term(memory)
            || Self::is_long_term_preference(&memory.content)
        {
            (1.0, 0.05 / 24.0)
        } else {
            (0.3, 0.25 / 2.0)
        };

        let time_decay = (1.0 - hours * decay_rate).max(0.0);
        let mut final_weight = base_weight * time_decay;

        if hours > 1.0 && (Self::is_tagged_temporary(memory) || Self::is_temporary_topic(&memory.content)) {
            final_weight *= 0.5;
        }

        final_weight.clamp(0.0, 1.0)
    }

    /// 新会话时仅保留偏好/身份类且重要性 ≥ 0.7 的记忆。
    pub fn filter_for_session(
        &mut self,
        memories: Vec<MemoryItem>,
        is_new_session: bool,
    ) -> Vec<MemoryItem> {
        if is_new_session {
            memories
                .into_iter()
                .filter(|m| {
                    let is_preference_or_identity = Self::is_tagged_long_term(m)
                        || m.tags.iter().any(|t| {
                            let lower = t.to_lowercase();
                            lower == "preference"
                                || lower == "identity"
                                || lower == "user_preferences"
                                || lower == "user_identity"
                        });
                    is_preference_or_identity && m.importance >= 0.7
                })
                .collect()
        } else {
            memories
        }
    }

    /// 获取过滤后的记忆列表（带权重）。新会话且未提及旧话题时仅返回长期偏好；
    /// 否则返回按权重排序的记忆，截断到 k 条。
    pub fn get_filtered_memories(
        &mut self,
        user_input: &str,
        memories: Vec<MemoryItem>,
        k: usize,
        char_id: &str,
    ) -> Vec<(MemoryItem, f64)> {
        let current_keywords: Vec<String> = content_keywords(user_input);
        let is_new_session = self.is_new_session(user_input, char_id);

        if self.last_interaction_time.is_none() || is_new_session {
            self.start_new_session(current_keywords.clone());
        } else {
            self.update_session_state(current_keywords.clone());
        }

        if memories.is_empty() {
            return Vec::new();
        }

        let user_mentioned_topic = memories.iter().any(|m| {
            let memory_keywords = content_keywords(&m.content);
            memory_keywords
                .iter()
                .any(|kw| current_keywords.iter().any(|ik| ik == kw))
        });

        if is_new_session && !user_mentioned_topic {
            return Self::relaxed_strict_filter(&memories, k);
        }

        let mut scored: Vec<(MemoryItem, f64)> = memories
            .into_iter()
            .map(|m| {
                let w = Self::calculate_memory_weight(&m);
                (m, w)
            })
            .filter(|(_, w)| *w > MIN_MEMORY_WEIGHT)
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    /// 放松阶梯各阶谓词（累积：stage N 允许 stage N-1 的全部 + 本阶新放宽条件）。
    ///
    /// 阶梯定义：
    /// - 0 (strict)：长期类 AND importance ≥ 0.7
    /// - 1 (drop_importance_min)：长期类（取消 importance 下限）
    /// - 2 (drop_categories)：长期类 OR importance ≥ 0.5（放宽类别）
    /// - 3 (drop_subjects)：长期类 OR importance ≥ 0.5 OR weight > MIN（放宽 importance）
    /// - 4 (no_filters)：全部
    fn relaxation_allows(stage: usize, m: &MemoryItem) -> bool {
        let long_term = Self::is_tagged_long_term(m) || Self::is_long_term_preference(&m.content);
        match stage {
            0 => long_term && m.importance >= 0.7,
            1 => long_term,
            2 => long_term || m.importance >= 0.5,
            3 => long_term || m.importance >= 0.5 || Self::calculate_memory_weight(m) > MIN_MEMORY_WEIGHT,
            _ => true,
        }
    }

    /// strict 路径的 5 阶放松阶梯：逐级放宽约束直到结果数 ≥ `RELAXATION_MIN_RESULTS`。
    ///
    /// 仅在「新会话且未提及旧话题」时调用。逐阶尝试，首个结果充足的阶梯胜出；
    /// 全部不足时回退到 no_filters（保证有上下文可注入，避免空检索）。
    fn relaxed_strict_filter(memories: &[MemoryItem], k: usize) -> Vec<(MemoryItem, f64)> {
        let ladder = RelaxationLadder::new(RELAXATION_MIN_RESULTS);
        let allowed = ladder.run(memories, |stage, m| Self::relaxation_allows(stage, m));

        let mut scored: Vec<(MemoryItem, f64)> = allowed
            .into_iter()
            .map(|m| {
                let w = Self::calculate_memory_weight(&m);
                (m, w)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    /// 获取过滤后的记忆文本（用于构建提示词）。
    pub fn get_filtered_memory_text(
        &mut self,
        user_input: &str,
        memories: Vec<MemoryItem>,
        k: usize,
        char_id: &str,
    ) -> String {
        let filtered = self.get_filtered_memories(user_input, memories, k, char_id);
        filtered
            .into_iter()
            .map(|(m, _)| format!("- {}", m.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 重置过滤器状态（用于测试或手动重置会话）。
    pub fn reset(&mut self) {
        self.last_interaction_time = None;
    }
}

impl Default for MemoryFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_item(memory_type: &str, tags: Vec<&str>, importance: f64) -> MemoryItem {
        MemoryItem {
            id: "seed_test".to_string(),
            content: "AlenTinn 创造了我。\n我第一次醒来的时候他就在那里，什么都不说，就看着我。".to_string(),
            granularity: "summary".to_string(),
            memory_type: memory_type.to_string(),
            importance,
            timestamp: current_timestamp(),
            embedding: None,
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            metadata: serde_json::json!({"source": "system_seed"}),
            related_ids: Vec::new(),
            description: Some("AlenTinn 创造了我".to_string()),
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
            protected: true,
            episode_id: None,
            consolidated: false,
            rebuttal_grace_remaining: 0,
        }
    }

    /// 种子记忆的 important_event 在 memory_type 而非 tags，须按 type 识别为长期类，
    /// 否则新会话严格过滤会把"创造者"这类核心设定记忆淘汰。
    #[test]
    fn seed_important_event_type_is_long_term() {
        let seed = seed_item("important_event", vec!["world_canon", "backstory", "vivian"], 0.9);
        assert!(MemoryFilter::is_tagged_long_term(&seed));
        // 内容含"什么"会被疑问句启发式拒绝，type 判定是唯一通过路径
        assert!(!MemoryFilter::is_long_term_preference(&seed.content));
        // 权重走长期类基线（≥0.9），而非临时话题基线（0.3）
        assert!(MemoryFilter::calculate_memory_weight(&seed) >= 0.9);
        // 放松阶梯 stage 0（长期类 AND importance ≥ 0.7）直接通过
        assert!(MemoryFilter::relaxation_allows(0, &seed));
    }

    /// 普通对话记忆（short_term）不受 type 判定影响，仍走启发式路径。
    #[test]
    fn short_term_dialogue_not_promoted_by_type() {
        let dialogue = seed_item("short_term", vec!["short_term", "user", "dialogue_turn"], 0.3);
        assert!(!MemoryFilter::is_tagged_long_term(&dialogue));
    }
}
