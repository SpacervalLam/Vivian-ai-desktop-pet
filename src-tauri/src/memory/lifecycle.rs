//! 记忆生命周期统一评估：健康度评分 + 压缩预算规划。
//!
//! 现有系统已具备分散的生命周期机制（过热/证据/二次衰减/回收站/数量上限），
//! 本模块将它们统一为一个可决策的"健康度"评分，并据此做"压缩预算"规划，
//! 为周期性的压缩/归档/淘汰提供单一决策依据，避免各机制各自为政。
//!
//! ## 健康度评分（0..1）
//! 综合五个分量：
//! - evidence：`sigmoid(evidence_score)`，把证据强度（-∞..+∞）映射到 (0,1)
//! - importance：时间衰减后的有效重要性
//! - recency：基于创建时间的时效（`exp(-age_hours / τ)`）
//! - usage：最近被检索命中的时效（命中越久远越不健康）
//! - protected 恒为 1.0（永不降解）
//!
//! ## 压缩预算
//! 给定 token 预算，按健康度从低到高挑选"压缩候选"（低健康度且类型可压缩），
//! 供压缩/归档流水线消费，而不是直接删除（保留可恢复性）。

use super::age::compute_heat_score;
use super::evidence::evidence_score;
use super::retention::QuadraticDecay;
use super::time_stamped::estimate_tokens;
use super::types::MemoryItem;

/// 健康度分级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthGrade {
    /// 健康（≥ 0.7）：保持
    Healthy,
    /// 稳定（0.5..0.7）：正常
    Stable,
    /// 有风险（0.3..0.5）：考虑压缩
    AtRisk,
    /// 退化（< 0.3）：归档/回收候选
    Degraded,
}

impl HealthGrade {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Stable => "stable",
            Self::AtRisk => "at_risk",
            Self::Degraded => "degraded",
        }
    }

    /// 从分数分级
    pub fn from_score(score: f64) -> Self {
        if score >= 0.70 {
            Self::Healthy
        } else if score >= 0.50 {
            Self::Stable
        } else if score >= 0.30 {
            Self::AtRisk
        } else {
            Self::Degraded
        }
    }
}

/// 健康度评分常量
const EVIDENCE_WEIGHT: f64 = 0.40;
const IMPORTANCE_WEIGHT: f64 = 0.30;
const RECENCY_WEIGHT: f64 = 0.20;
const USAGE_WEIGHT: f64 = 0.10;
/// 创建时间时效半衰期（小时）：24h 后 recency ≈ 0.5
const RECENCY_HALFLIFE_HOURS: f64 = 24.0;
/// 使用时效半衰期（小时）：72h 未命中后 usage ≈ 0.5
const USAGE_HALFLIFE_HOURS: f64 = 72.0;

/// sigmoid：把 (-∞, +∞) 映射到 (0,1)，用于证据强度归一化
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// 计算单条记忆的健康度 [0, 1]。
///
/// - `protected` 恒返回 1.0
/// - 证据分量：`sigmoid(evidence_score)`，证据越强越健康
/// - 重要性分量：`decayed_importance`（时间衰减后的有效重要性）
/// - 时效分量：`exp(-age_hours / τ)`，基于创建时间
/// - 使用分量：`exp(-since_last_visit_hours / τ)`，命中越近越健康；从未命中取 0.5
pub fn health_score(memory: &MemoryItem, now: f64) -> f64 {
    if memory.protected {
        return 1.0;
    }

    // 1. 证据分量
    let evidence_term = sigmoid(evidence_score(memory, now));

    // 2. 重要性分量（时间衰减后的有效重要性）
    let importance_term = QuadraticDecay::decayed_importance(memory, now);

    // 3. 时效分量（基于创建时间）
    let age_hours = ((now - memory.timestamp).max(0.0)) / 3600.0;
    let recency_term = (-age_hours / RECENCY_HALFLIFE_HOURS).exp();

    // 4. 使用分量（基于最近命中时间）
    let usage_term = if memory.last_visit_at <= 0.0 {
        // 从未被命中：给中性值 0.5（不因未命中而瞬间降级，也不因命中过而虚高）
        0.5
    } else {
        let since_hours = ((now - memory.last_visit_at).max(0.0)) / 3600.0;
        (-since_hours / USAGE_HALFLIFE_HOURS).exp()
    };

    let score = EVIDENCE_WEIGHT * evidence_term
        + IMPORTANCE_WEIGHT * importance_term
        + RECENCY_WEIGHT * recency_term
        + USAGE_WEIGHT * usage_term;

    score.clamp(0.0, 1.0)
}

/// 计算"使用热度"的归一化分量（0..1），供诊断使用。
///
/// 复用 `compute_heat_score`，但把它映射到 (0,1)（heat 本身无上界）。
pub fn usage_term(memory: &MemoryItem, now: f64) -> f64 {
    let heat = compute_heat_score(
        memory.visit_count,
        memory.content.chars().count(),
        memory.last_visit_at,
        now,
    );
    if heat <= 0.0 {
        0.0
    } else {
        heat / (1.0 + heat)
    }
}

/// 是否允许某记忆参与压缩（作为压缩候选）。
///
/// - `protected` 永不压缩
/// - 核心身份/偏好/重要事件等"恒久记忆"类型不压缩
/// - 其余（闲聊/临时/一般记忆）在健康度不足时允许压缩
pub fn is_compressible(memory: &MemoryItem) -> bool {
    if memory.protected {
        return false;
    }
    let mem_type = memory.memory_type.as_str();
    !matches!(
        mem_type,
        "preference" | "identity" | "important_event" | "user" | "knowledge"
    )
}

/// 压缩预算规划结果
pub struct CompressionPlan<'a> {
    /// 应保留的高价值记忆（健康度靠前）
    pub keep: Vec<&'a MemoryItem>,
    /// 建议压缩/归档的候选（健康度靠后）
    pub compress_candidates: Vec<&'a MemoryItem>,
    /// 保留集合的估算 token 数
    pub keep_tokens: usize,
    /// 是否超出预算（预算不足以容纳全部保留项）
    pub over_budget: bool,
}

/// 按 token 预算规划压缩。
///
/// 策略：按健康度降序排列，优先保留高健康度记忆，直到估算 token 达到预算；
/// 剩余（健康度较低）的记忆标记为压缩候选。低健康度且不可压缩的类型
/// （如 preference/knowledge）仍会保留在 keep 中（不因预算被丢弃，仅提示超预算）。
///
/// - `token_budget`：允许进入 prompt/持久层的 token 预算上限
/// - `now`：当前时间戳
pub fn plan_compression<'a>(
    items: &'a [MemoryItem],
    token_budget: usize,
    now: f64,
) -> CompressionPlan<'a> {
    let mut scored: Vec<(&'a MemoryItem, f64)> = items
        .iter()
        .map(|m| (m, health_score(m, now)))
        .collect();
    // 健康度降序（高健康度优先保留）
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut keep: Vec<&MemoryItem> = Vec::new();
    let mut compress: Vec<&MemoryItem> = Vec::new();
    let mut keep_tokens = 0usize;
    let mut over_budget = false;

    for (item, score) in scored {
        let tokens = estimate_tokens(&item.content);
        // 恒久记忆（不可压缩类型）即使超预算也保留在 keep（标记超预算）
        if !is_compressible(item) {
            keep.push(item);
            keep_tokens += tokens;
            if keep_tokens > token_budget {
                over_budget = true;
            }
            continue;
        }
        // 可压缩：预算内且健康度未退化 → 保留；否则进压缩候选
        if keep_tokens + tokens <= token_budget && score >= 0.30 {
            keep.push(item);
            keep_tokens += tokens;
        } else if keep_tokens + tokens <= token_budget {
            // 预算内但健康度退化：仍保留（预算优先），但记录超预算风险由调用方决定
            keep.push(item);
            keep_tokens += tokens;
        } else {
            compress.push(item);
            over_budget = true;
        }
    }

    CompressionPlan {
        keep,
        compress_candidates: compress,
        keep_tokens,
        over_budget,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{current_timestamp, Granularity};

    fn make(content: &str, importance: f64, now: f64) -> MemoryItem {
        MemoryItem::new(content.to_string(), Granularity::Summary, importance)
    }

    #[test]
    fn protected_is_healthy() {
        let now = current_timestamp();
        let mut m = make("受保护", 0.0, now);
        m.protected = true;
        assert_eq!(health_score(&m, now), 1.0);
    }

    #[test]
    fn high_evidence_more_healthy_than_negated() {
        let now = current_timestamp();
        let mut reinforced = make("强化", 0.5, now);
        reinforced.reinforcement = 5.0;
        reinforced.rein_last_signal_at = now;
        let mut rebutted = make("反驳", 0.5, now);
        rebutted.disputation = 5.0;
        rebutted.disp_last_signal_at = now;
        assert!(health_score(&reinforced, now) > health_score(&rebutted, now));
    }

    #[test]
    fn recent_memory_more_healthy_than_old() {
        let now = current_timestamp();
        let recent = make("新", 0.5, now - 3600.0);
        let old = make("旧", 0.5, now - 30.0 * 86400.0);
        assert!(health_score(&recent, now) > health_score(&old, now));
    }

    #[test]
    fn grade_rounds() {
        assert_eq!(HealthGrade::from_score(0.8), HealthGrade::Healthy);
        assert_eq!(HealthGrade::from_score(0.6), HealthGrade::Stable);
        assert_eq!(HealthGrade::from_score(0.4), HealthGrade::AtRisk);
        assert_eq!(HealthGrade::from_score(0.2), HealthGrade::Degraded);
    }

    #[test]
    fn compressible_excludes_core_types() {
        let now = current_timestamp();
        let mut pref = make("偏好", 0.3, now);
        pref.memory_type = "preference".to_string();
        assert!(!is_compressible(&pref));

        let mut casual = make("闲聊", 0.3, now);
        casual.memory_type = "casual_conversation".to_string();
        assert!(is_compressible(&casual));
    }

    #[test]
    fn plan_respects_budget() {
        let now = current_timestamp();
        let items = vec![
            make("a".repeat(20), 0.9, now),
            make("b".repeat(20), 0.8, now),
            make("c".repeat(20), 0.2, now),
        ];
        let plan = plan_compression(&items, 40, now);
        // 低健康度的 c 应进入压缩候选
        assert!(plan.compress_candidates.iter().any(|m| m.content.starts_with('c')));
        // 高健康度 a/b 保留
        assert!(plan.keep.iter().any(|m| m.content.starts_with('a')));
        assert!(plan.keep.iter().any(|m| m.content.starts_with('b')));
    }

    #[test]
    fn plan_protects_core_even_when_degraded() {
        let now = current_timestamp();
        let mut core = make("核心".repeat(10), 0.1, now);
        core.memory_type = "preference".to_string();
        core.protected = true;
        let items = vec![core];
        let plan = plan_compression(&items, 10, now);
        // protected 恒保留，不进入压缩候选
        assert!(plan.keep.iter().any(|m| m.content.contains("核心")));
        assert!(plan.compress_candidates.is_empty());
    }
}