//! 记忆冲突检测流水线
//!
//! 参考 Cyrene-Agent 的三阶段架构，适配 Vivian 的 MemoryItem 数据模型：
//! - Stage 1: 本地规则矛盾检测（词法矛盾对 + 主题共享预筛）
//! - Stage 2: 6 维评分（纠正意图 / 语义相似 / 近期注入 / 证据 / 本地矛盾 / 影响范围）
//! - Stage 3: 动作决策（KeepBoth / ReplaceOld / MergeSupersede / QueueLlm）
//!
//! 集成点：`MemoryManager::add_memory` 在 `add_entry` 之前调用。
//! 仅对"持久型"记忆（LongTerm / Preference / Identity / ImportantEvent / Knowledge / User / Feedback）
//! 执行冲突检测，ShortTerm / CasualConversation 等缓冲型记忆跳过。
//!
//! LLM 仲裁不在写入热路径上同步执行，而是标记为 QueueLlm 后由后台任务消费。

use crate::memory::types::{MemoryItem, MemoryType};

/// 矛盾词对：(正面/肯定词, [否定/反义词...])
///
/// 当新记忆和旧记忆分别命中正反词时，判定为潜在词法矛盾。
const CONTRADICTION_PAIRS: &[(&str, &[&str])] = &[
    ("喜欢", &["不喜欢", "讨厌", "反感", "厌恶", "不再喜欢"]),
    ("爱", &["不爱", "讨厌", "恨"]),
    ("想", &["不想", "别想", "不愿"]),
    ("要", &["不要", "别要"]),
    ("是", &["不是", "并非"]),
    ("可以", &["不可以", "不行", "不能"]),
    ("会", &["不会"]),
    ("有", &["没有", "没了", "无"]),
    ("忙", &["不忙", "闲"]),
    ("能", &["不能", "无法"]),
    ("住", &["不住", "搬走"]),
    ("工作", &["辞职", "离职", "退休"]),
];

/// 纠正意图信号词：新记忆中出现这些词暗示用户在纠正旧信息
const CORRECTION_SIGNALS: &[&str] = &[
    "不是", "其实", "应该是", "纠正一下", "说错了", "更正", "准确来说",
    "不对", "搞错了", "实际上", "确切地说",
];

/// 停用词表（主题提取时过滤）
const STOP_TERMS: &[&str] = &[
    "用户", "一个", "因为", "所以", "但是", "虽然", "不过", "然后",
    "现在", "已经", "可能", "应该", "觉得", "认为", "知道", "什么",
    "怎么", "为什么", "怎样", "这样", "那样", "的话", "的话",
];

/// 冲突评分输入
#[derive(Debug, Clone)]
pub struct ConflictScoreInput {
    /// 候选来源（本地扫描 / 向量检索 / 近期注入）
    pub candidate_source: CandidateSource,
    /// 向量相似度分数（0.0-1.0），None 表示未做向量检索
    pub rag_score: Option<f64>,
    /// 新记忆是否含纠正意图信号词
    pub correction_intent: bool,
    /// 旧记忆是否在近 N 轮内被注入过对话（visit_count > 0 且 last_visit_at 近期）
    pub recent_injection: bool,
    /// 本地词法矛盾检测结果
    pub local_contradiction: bool,
    /// 证据可用性：是否有 metadata 上下文
    pub evidence: EvidenceLevel,
    /// 旧记忆是否仍活跃（importance >= 0.3）
    pub active_target: bool,
    /// 影响范围
    pub impact_scope: ImpactScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSource {
    Local,
    Rag,
    RecentInjection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceLevel {
    None,
    OneSide,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpactScope {
    Low,
    Medium,
    High,
}

/// 冲突评分结果
#[derive(Debug, Clone)]
pub struct ConflictScoreResult {
    /// 冲突分数 0-100
    pub conflict_score: f64,
    /// Resolver 优先级
    pub resolver_priority: ResolverPriority,
    /// 评分信号快照（审计用）
    pub scoring_signals: ScoringSignals,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverPriority {
    High,
    Normal,
    Idle,
    None,
}

#[derive(Debug, Clone, Default)]
pub struct ScoringSignals {
    pub correction_intent: bool,
    pub rag_candidate: bool,
    pub recent_injection: bool,
    pub evidence_available: bool,
    pub local_contradiction: bool,
    pub impact_scope: ImpactScope,
    pub penalties: Vec<&'static str>,
}

impl Default for ImpactScope {
    fn default() -> Self {
        ImpactScope::Low
    }
}

/// 冲突动作决策
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    /// 保留两者（无冲突或低优先级）
    KeepBoth,
    /// 用新记忆替换旧记忆（直接矛盾 + 高权威性）
    ReplaceOld,
    /// 合并：删除旧记忆，写入合并后内容（由调用方执行合并）
    MergeSupersede,
    /// 排队等待 LLM 仲裁（非阻塞，后台消费）
    QueueLlm,
}

/// 判断记忆类型是否需要冲突检测
pub fn should_check_conflict(memory_type: MemoryType) -> bool {
    matches!(
        memory_type,
        MemoryType::LongTerm
            | MemoryType::Preference
            | MemoryType::Identity
            | MemoryType::ImportantEvent
            | MemoryType::Knowledge
            | MemoryType::User
            | MemoryType::Feedback
    )
}

/// Stage 1: 本地规则矛盾检测
///
/// 返回 Some(confidence) 表示检测到潜在矛盾，None 表示无矛盾信号。
/// 置信度固定 0.35（与 Cyrene 一致），最终是否冲突由评分阶段决定。
pub fn detect_local_contradiction(new_content: &str, existing_content: &str) -> Option<f64> {
    let new_lower = new_content.to_lowercase();
    let old_lower = existing_content.to_lowercase();

    // 主题共享预筛：若无共享主题词，直接跳过
    if !has_shared_topic(&new_lower, &old_lower) {
        return None;
    }

    // 词法矛盾对检测
    for (positive, negatives) in CONTRADICTION_PAIRS {
        let new_has_pos = new_lower.contains(positive);
        let old_has_pos = old_lower.contains(positive);
        let new_has_neg = negatives.iter().any(|n| new_lower.contains(n));
        let old_has_neg = negatives.iter().any(|n| old_lower.contains(n));

        // 正反词互斥：(新正+旧反) 或 (旧正+新反)
        if (new_has_pos && old_has_neg) || (old_has_pos && new_has_neg) {
            return Some(0.35);
        }
    }

    None
}

/// 主题共享检测：提取主题词，判断是否有交集
fn has_shared_topic(a: &str, b: &str) -> bool {
    let terms_a = extract_topic_terms(a);
    if terms_a.is_empty() {
        return false;
    }
    let terms_b = extract_topic_terms(b);
    if terms_b.is_empty() {
        return false;
    }
    terms_a.iter().any(|t| terms_b.contains(t))
}

/// 提取主题词：中文 >= 2 字，拉丁字母数字 >= 3 字，过滤停用词
fn extract_topic_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else {
            if current.len() >= 3 {
                let t = current.to_lowercase();
                if !STOP_TERMS.contains(&t.as_str()) {
                    terms.push(t);
                }
            }
            current.clear();
            // 中文词：>= 2 字
            if '\u{4e00}' <= ch && ch <= '\u{9fff}' {
                current.push(ch);
            }
        }
    }
    if current.len() >= 3 {
        let t = current.to_lowercase();
        if !STOP_TERMS.contains(&t.as_str()) {
            terms.push(t);
        }
    }

    // 中文连续片段切分（>= 2 字）
    let mut cn_buf = String::new();
    for ch in text.chars() {
        if '\u{4e00}' <= ch && ch <= '\u{9fff}' {
            cn_buf.push(ch);
        } else {
            if cn_buf.chars().count() >= 2 {
                let t = cn_buf.clone();
                if !STOP_TERMS.contains(&t.as_str()) {
                    terms.push(t);
                }
                // 长度 > 2 时额外切 2-gram
                let chars: Vec<char> = cn_buf.chars().collect();
                for i in 0..chars.len().saturating_sub(1) {
                    let gram: String = chars[i..i + 2].iter().collect();
                    if !STOP_TERMS.contains(&gram.as_str()) {
                        terms.push(gram);
                    }
                }
            }
            cn_buf.clear();
        }
    }
    // 处理尾部中文
    if cn_buf.chars().count() >= 2 {
        let t = cn_buf.clone();
        if !STOP_TERMS.contains(&t.as_str()) {
            terms.push(t);
        }
        let chars: Vec<char> = cn_buf.chars().collect();
        for i in 0..chars.len().saturating_sub(1) {
            let gram: String = chars[i..i + 2].iter().collect();
            if !STOP_TERMS.contains(&gram.as_str()) {
                terms.push(gram);
            }
        }
    }

    terms
}

/// 检测纠正意图信号词
pub fn detect_correction_intent(content: &str) -> bool {
    CORRECTION_SIGNALS.iter().any(|s| content.contains(s))
}

/// Stage 2: 6 维冲突评分
pub fn score_conflict(input: &ConflictScoreInput) -> ConflictScoreResult {
    let mut score: f64 = 0.0;
    let mut signals = ScoringSignals::default();

    // 1. 纠正意图 (+20)
    if input.correction_intent {
        score += 20.0;
        signals.correction_intent = true;
    }

    // 2. RAG 候选 / 语义相似 (+10/+18/+25)
    if input.candidate_source == CandidateSource::Rag || input.rag_score.is_some() {
        signals.rag_candidate = true;
        let pts = rag_points(input.rag_score);
        score += pts;
    }

    // 3. 近期注入 (+20)
    if input.recent_injection || input.candidate_source == CandidateSource::RecentInjection {
        score += 20.0;
        signals.recent_injection = true;
    }

    // 4. 证据可用性 (+0/+8/+15)
    let ev_pts = evidence_points(input.evidence);
    score += ev_pts;
    signals.evidence_available = input.evidence != EvidenceLevel::None;

    // 5. 本地词法矛盾 (+10)
    if input.local_contradiction {
        score += 10.0;
        signals.local_contradiction = true;
    }

    // 6. 影响范围 (+3/+6/+10)
    let imp_pts = impact_points(input.impact_scope);
    score += imp_pts;
    signals.impact_scope = input.impact_scope;

    // 扣分
    if !input.active_target {
        score -= 25.0;
        signals.penalties.push("archived_only_target");
    }
    if input.evidence == EvidenceLevel::None {
        score -= 20.0;
        signals.penalties.push("missing_evidence");
    }

    // 归一化 0-100
    let score = score.clamp(0.0, 100.0);

    // 优先级映射
    let mut priority = if score >= 75.0 {
        ResolverPriority::High
    } else if score >= 55.0 {
        ResolverPriority::Normal
    } else if score >= 35.0 {
        ResolverPriority::Idle
    } else {
        ResolverPriority::None
    };

    // 强制降级：本地候选 / 非活跃目标 / 无证据 → None
    if input.candidate_source == CandidateSource::Local
        || !input.active_target
        || input.evidence == EvidenceLevel::None
    {
        priority = ResolverPriority::None;
    }

    ConflictScoreResult {
        conflict_score: score,
        resolver_priority: priority,
        scoring_signals: signals,
    }
}

fn rag_points(rag_score: Option<f64>) -> f64 {
    match rag_score {
        Some(s) if s >= 0.75 => 25.0,
        Some(s) if s >= 0.45 => 18.0,
        Some(_) => 10.0,
        None => 0.0,
    }
}

fn evidence_points(evidence: EvidenceLevel) -> f64 {
    match evidence {
        EvidenceLevel::Both => 15.0,
        EvidenceLevel::OneSide => 8.0,
        EvidenceLevel::None => 0.0,
    }
}

fn impact_points(scope: ImpactScope) -> f64 {
    match scope {
        ImpactScope::High => 10.0,
        ImpactScope::Medium => 6.0,
        ImpactScope::Low => 3.0,
    }
}

/// Stage 3: 根据评分结果决定动作
///
/// 决策规则：
/// - priority == None → KeepBoth（无冲突或证据不足）
/// - priority == Idle 且有本地矛盾 → ReplaceOld（直接矛盾，新信息权威）
/// - priority == Normal/High 且有本地矛盾 + 纠正意图 → ReplaceOld
/// - priority == Normal/High 且有语义相似但无直接矛盾 → MergeSupersede
/// - priority == High 且无本地矛盾 → QueueLlm（需 LLM 判断）
pub fn resolve_action(score: &ConflictScoreResult) -> ConflictAction {
    match score.resolver_priority {
        ResolverPriority::None => ConflictAction::KeepBoth,

        ResolverPriority::Idle => {
            if score.scoring_signals.local_contradiction {
                ConflictAction::ReplaceOld
            } else {
                ConflictAction::KeepBoth
            }
        }

        ResolverPriority::Normal => {
            if score.scoring_signals.local_contradiction {
                if score.scoring_signals.correction_intent {
                    ConflictAction::ReplaceOld
                } else {
                    ConflictAction::MergeSupersede
                }
            } else {
                ConflictAction::QueueLlm
            }
        }

        ResolverPriority::High => {
            if score.scoring_signals.local_contradiction
                && score.scoring_signals.correction_intent
            {
                ConflictAction::ReplaceOld
            } else if score.scoring_signals.rag_candidate {
                ConflictAction::MergeSupersede
            } else {
                ConflictAction::QueueLlm
            }
        }
    }
}

/// 构建冲突评分输入（从 MemoryItem 对推导）
///
/// `new_item`: 待写入的新记忆
/// `old_item`: 已有的相似记忆
/// `similarity`: 向量相似度（0.0-1.0），None 表示未做向量检索
/// `recently_injected`: 旧记忆是否在近期对话中被注入过
pub fn build_score_input(
    new_item: &MemoryItem,
    old_item: &MemoryItem,
    similarity: Option<f64>,
    recently_injected: bool,
) -> ConflictScoreInput {
    let local_contradiction =
        detect_local_contradiction(&new_item.content, &old_item.content).is_some();
    let correction_intent = detect_correction_intent(&new_item.content);

    // 证据可用性：metadata 是否非空
    let new_has_meta = new_item.metadata.is_object()
        && new_item.metadata.as_object().map(|o| !o.is_empty()).unwrap_or(false);
    let old_has_meta = old_item.metadata.is_object()
        && old_item.metadata.as_object().map(|o| !o.is_empty()).unwrap_or(false);
    let evidence = match (new_has_meta, old_has_meta) {
        (true, true) => EvidenceLevel::Both,
        (true, false) | (false, true) => EvidenceLevel::OneSide,
        (false, false) => EvidenceLevel::None,
    };

    // 影响范围：基于重要性差值
    let imp_delta = (new_item.importance - old_item.importance).abs();
    let impact_scope = if new_item.importance >= 0.8 || old_item.importance >= 0.8 {
        ImpactScope::High
    } else if imp_delta >= 0.3 {
        ImpactScope::Medium
    } else {
        ImpactScope::Low
    };

    // 活跃目标：旧记忆 importance >= 0.3
    let active_target = old_item.importance >= 0.3;

    let candidate_source = match similarity {
        Some(s) if s >= 0.45 => CandidateSource::Rag,
        None => CandidateSource::Local,
        _ => CandidateSource::Local,
    };

    ConflictScoreInput {
        candidate_source,
        rag_score: similarity,
        correction_intent,
        recent_injection: recently_injected,
        local_contradiction,
        evidence,
        active_target,
        impact_scope,
    }
}

/// 简单合并两条记忆内容（非 LLM 路径，用于 MergeSupersede）
///
/// 策略：以新记忆为主，保留旧记忆中不重复的关键信息。
pub fn simple_merge(old: &str, new: &str) -> String {
    // 若旧记忆是新记忆的子串，直接用新记忆
    if new.contains(old) {
        return new.to_string();
    }
    // 若新记忆是旧记忆的子串，保留旧记忆（新信息已包含）
    if old.contains(new) {
        return old.to_string();
    }
    // 否则拼接：新记忆 + 旧记忆补充
    format!("{}（{}）", new.trim(), old.trim())
}

// ============================================================================
// LLM 仲裁：QueueLlm 冲突的后台消费者
// ============================================================================

/// 待 LLM 仲裁的冲突条目（持久化到 pending_conflicts.json）
///
/// 当写入时检测到冲突但本地规则无法决策（ConflictAction::QueueLlm）时，
/// 推入 MemoryManager::pending_conflicts 队列。后台 tick 调用
/// `process_pending_conflicts` 消费，调 LLM 仲裁后执行相应动作。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingConflict {
    /// 唯一 ID
    pub id: String,
    /// 新记忆 ID
    pub new_memory_id: String,
    /// 新记忆内容
    pub new_content: String,
    /// 旧记忆 ID
    pub old_memory_id: String,
    /// 旧记忆内容
    pub old_content: String,
    /// 双方向量相似度
    pub similarity: f64,
    /// 入队时间戳（Unix 秒）
    pub created_at: f64,
    /// LLM 仲裁失败重试次数（≥3 次丢弃）
    pub retry_count: u32,
}

/// LLM 仲裁结果
#[derive(Debug, Clone)]
pub enum ArbitrationOutcome {
    /// 保留两者（语义不冲突，是互补信息）
    KeepBoth,
    /// 用新记忆替换旧记忆（旧记忆已过时/错误）
    ReplaceOld,
    /// 合并取代：用合并后的内容更新新记忆，删除旧记忆
    MergeSupersede(String),
}

/// LLM 仲裁器 trait（供 mock 测试和真实 LLM 实现解耦）
#[async_trait::async_trait]
pub trait ConflictLlmArbiter: Send + Sync {
    /// 仲裁两条记忆是否冲突，返回处理方式
    ///
    /// `new_content`/`old_content`：两条记忆的文本内容
    /// `similarity`：向量相似度（0.0-1.0），LLM 可参考判断语义重叠程度
    async fn arbitrate(
        &self,
        new_content: &str,
        old_content: &str,
        similarity: f64,
    ) -> Result<ArbitrationOutcome, String>;
}

/// 默认 LLM 仲裁器：基于 ModelRouter 实现
pub struct DefaultConflictArbiter {
    router: std::sync::Arc<crate::providers::ModelRouter>,
}

impl DefaultConflictArbiter {
    pub fn new(router: std::sync::Arc<crate::providers::ModelRouter>) -> Self {
        Self { router }
    }
}

#[async_trait::async_trait]
impl ConflictLlmArbiter for DefaultConflictArbiter {
    async fn arbitrate(
        &self,
        new_content: &str,
        old_content: &str,
        similarity: f64,
    ) -> Result<ArbitrationOutcome, String> {
        use crate::providers::base::LLMRequest;
        use crate::types::response::ChatMessage;

        let system = "你是记忆冲突仲裁器。给定两条记忆内容和它们的向量相似度，判断它们是否冲突，并决定如何处理。\n\n\
            ## 判断标准\n\
            - **ReplaceOld**：旧记忆是错误/过时信息，新记忆正确（如用户纠正了之前的信息）\n\
            - **MergeSupersede**：两条记忆都有效但可以合并为更完整的表述（返回合并后的内容）\n\
            - **KeepBoth**：两条记忆语义不冲突，是互补信息（如不同时间点的不同事实）\n\n\
            ## 输出格式（严格 JSON）\n\
            ```json\n\
            {\"action\": \"replace_old\" | \"merge_supersede\" | \"keep_both\", \"merged\": \"合并后内容（仅 merge_supersede 时需要）\"}\n\
            ```\n\
            不要输出 JSON 之外的任何内容。";

        let user = format!(
            "向量相似度: {:.2}\n\n旧记忆:\n{}\n\n新记忆:\n{}\n\n请判断如何处理。",
            similarity, old_content, new_content
        );

        let messages = vec![
            ChatMessage::system(system),
            ChatMessage::user(&user),
        ];

        let resp = self
            .router
            .generate(LLMRequest::new("memory_conflict_arbitration", messages))
            .await
            .map_err(|e| e.to_string())?;

        let trimmed = resp.trim();
        // 尝试从可能含 markdown 代码块的响应中提取 JSON
        let json_str = extract_json_block(trimmed);
        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| format!("LLM 仲裁响应非 JSON: {} (response: {})", e, trimmed))?;

        let action = parsed
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or("missing action field")?;

        match action {
            "replace_old" => Ok(ArbitrationOutcome::ReplaceOld),
            "merge_supersede" => {
                let merged = parsed
                    .get("merged")
                    .and_then(|v| v.as_str())
                    .unwrap_or(new_content)
                    .to_string();
                Ok(ArbitrationOutcome::MergeSupersede(merged))
            }
            _ => Ok(ArbitrationOutcome::KeepBoth),
        }
    }
}

/// 从可能含 markdown 代码块的字符串中提取 JSON
fn extract_json_block(s: &str) -> &str {
    let s = s.trim();
    if let Some(start) = s.find("```json") {
        if let Some(end) = s.rfind("```") {
            let content_start = start + 7;
            if content_start < end {
                return s[content_start..end].trim();
            }
        }
    }
    if let Some(start) = s.find("```") {
        if let Some(end) = s.rfind("```") {
            let content_start = start + 3;
            if content_start < end {
                return s[content_start..end].trim();
            }
        }
    }
    // 尝试找第一个 { 到最后一个 }
    if let Some(start) = s.find('{') {
        if let Some(end) = s.rfind('}') {
            if start <= end {
                return &s[start..=end];
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contradiction_detection_like_dislike() {
        let result = detect_local_contradiction("我喜欢猫", "我不喜欢猫");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 0.35);
    }

    #[test]
    fn test_no_contradiction_different_topics() {
        let result = detect_local_contradiction("我喜欢猫", "今天天气很好");
        assert!(result.is_none());
    }

    #[test]
    fn test_no_contradiction_same_direction() {
        let result = detect_local_contradiction("我喜欢猫", "我也喜欢狗");
        assert!(result.is_none());
    }

    #[test]
    fn test_correction_intent_detection() {
        assert!(detect_correction_intent("其实我不是在北京工作"));
        assert!(detect_correction_intent("说错了，应该是上海"));
        assert!(!detect_correction_intent("我在北京工作"));
    }

    #[test]
    fn test_score_conflict_high_priority() {
        let input = ConflictScoreInput {
            candidate_source: CandidateSource::Rag,
            rag_score: Some(0.85),
            correction_intent: true,
            recent_injection: true,
            local_contradiction: true,
            evidence: EvidenceLevel::Both,
            active_target: true,
            impact_scope: ImpactScope::High,
        };
        let result = score_conflict(&input);
        assert!(result.conflict_score >= 75.0);
        assert_eq!(result.resolver_priority, ResolverPriority::High);
    }

    #[test]
    fn test_score_conflict_no_priority_no_evidence() {
        let input = ConflictScoreInput {
            candidate_source: CandidateSource::Rag,
            rag_score: Some(0.5),
            correction_intent: false,
            recent_injection: false,
            local_contradiction: false,
            evidence: EvidenceLevel::None,
            active_target: true,
            impact_scope: ImpactScope::Low,
        };
        let result = score_conflict(&input);
        // 无证据强制降级为 None
        assert_eq!(result.resolver_priority, ResolverPriority::None);
    }

    #[test]
    fn test_resolve_action_replace_on_correction() {
        let result = ConflictScoreResult {
            conflict_score: 80.0,
            resolver_priority: ResolverPriority::High,
            scoring_signals: ScoringSignals {
                local_contradiction: true,
                correction_intent: true,
                ..Default::default()
            },
        };
        assert_eq!(resolve_action(&result), ConflictAction::ReplaceOld);
    }

    #[test]
    fn test_resolve_action_keep_both_no_conflict() {
        let result = ConflictScoreResult {
            conflict_score: 20.0,
            resolver_priority: ResolverPriority::None,
            scoring_signals: ScoringSignals::default(),
        };
        assert_eq!(resolve_action(&result), ConflictAction::KeepBoth);
    }

    #[test]
    fn test_simple_merge_substring() {
        assert_eq!(simple_merge("猫", "我喜欢猫"), "我喜欢猫");
        assert_eq!(simple_merge("我喜欢猫", "猫"), "我喜欢猫");
    }
}
