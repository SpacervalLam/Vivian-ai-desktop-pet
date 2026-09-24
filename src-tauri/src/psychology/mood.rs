//! Mood 层 — 从心理状态派生的统一快照，供 UI 展示和轻量行为决策使用。
//!
//! Mood 不存储，由 PsychologyManager.compute_mood() 实时计算。
//! Emotion、Needs 和 Relationship 是状态真源；Mood 负责生成一致的展示/决策指标，
//! 不允许调用方各自用不同公式推导专注、精力、情感效价或关系指标。

use serde::{Deserialize, Serialize};

use super::emotion::{EmotionLabel, EmotionState};
use super::needs::NeedsState;
use super::relationship::RelationshipState;

/// 前端展示用的 Mood 快照（仅 UI，不持久化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoodSnapshot {
    /// 效价（-1.0 ~ 1.0）：正面 vs 负面
    pub valence: f64,
    /// 唤醒度（0.0 ~ 1.0）：平静 vs 激活
    pub arousal: f64,
    /// 主导情绪标签
    pub primary_emotion: EmotionLabel,
    /// 次要情绪标签
    pub secondary_emotion: EmotionLabel,
    /// 主导情绪强度
    pub primary_intensity: f64,
    /// 疲劳度（0-100，由 needs + 时间推导）
    pub fatigue: f64,
    /// 当前压力（0-100，由 emotion 推导）
    pub stress: f64,
    /// 关系综合分（0-100，供前端展示）
    pub relationship_score: f64,
    /// 关系信任（0-100）
    pub trust: f64,
    /// 关系亲密度（0-100）
    pub intimacy: f64,
    /// 正向情感强度（0-100），由快乐/亲近/好奇构成
    pub positive_affect: f64,
    /// 负向情感强度（0-100），由悲伤/愤怒/恐惧/孤独构成
    pub negative_affect: f64,
    /// 行为专注度（0-100），由好奇/正向投入与负面干扰共同推导
    pub focus: f64,
    /// 精力（0-100），疲劳的反向映射
    pub energy: f64,
}

impl Default for MoodSnapshot {
    fn default() -> Self {
        Self {
            valence: 0.3,
            arousal: 0.4,
            primary_emotion: EmotionLabel::Curiosity,
            secondary_emotion: EmotionLabel::Closeness,
            primary_intensity: 0.5,
            fatigue: 20.0,
            stress: 10.0,
            relationship_score: 20.0,
            trust: 20.0,
            intimacy: 20.0,
            positive_affect: 50.0,
            negative_affect: 10.0,
            focus: 50.0,
            energy: 80.0,
        }
    }
}

/// 实时计算 Mood 快照
///
/// Mood 是 Emotion + Needs + Relationship 的「投影」，不是独立状态。
/// 公式刻意简单：Mood 只做 UI 翻译，不做决策。
pub fn compute_mood(
    emotion: &EmotionState,
    needs: &NeedsState,
    relationship: &RelationshipState,
    last_interaction_secs: f64, // 距上次互动的秒数
) -> MoodSnapshot {
    let valence = emotion.valence();
    let arousal = emotion.arousal();

    // 找出主导和次要情绪
    let mut emotions = [
        (EmotionLabel::Joy, emotion.joy),
        (EmotionLabel::Sadness, emotion.sadness),
        (EmotionLabel::Anger, emotion.anger),
        (EmotionLabel::Fear, emotion.fear),
        (EmotionLabel::Closeness, emotion.closeness),
        (EmotionLabel::Loneliness, emotion.loneliness),
        (EmotionLabel::Curiosity, emotion.curiosity),
    ];
    emotions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let (primary_emotion, primary_intensity) = emotions[0];
    let secondary_emotion = emotions[1].0;

    // 疲劳：未满足需求构成基线负荷；长时间无互动增加有限的静息/低活动负荷。
    // 指数饱和避免线性公式在数小时后就把疲劳永久推满。
    let need_burden = (needs.belonging + needs.security + needs.expression) / 3.0;
    let idle_load = 45.0 * (1.0 - (-last_interaction_secs.max(0.0) / 14_400.0).exp());
    let fatigue = (need_burden * 35.0 + idle_load).clamp(0.0, 100.0);

    // 压力：负面情绪 + 安全需求
    let stress = ((emotion.fear + emotion.anger + emotion.sadness) / 3.0 * 60.0
        + needs.security * 40.0)
        .clamp(0.0, 100.0);

    // 关系综合分
    let relationship_score =
        (relationship.trust * 30.0 + relationship.intimacy * 30.0 + relationship.familiarity * 20.0
            + relationship.respect * 10.0
            + relationship.dependency * 10.0)
        .clamp(0.0, 100.0);

    let positive_affect =
        (emotion.joy * 0.45 + emotion.closeness * 0.30 + emotion.curiosity * 0.25) * 100.0;
    let negative_affect = (emotion.sadness * 0.30
        + emotion.anger * 0.30
        + emotion.fear * 0.20
        + emotion.loneliness * 0.20)
        * 100.0;
    let focus = (emotion.curiosity * 0.55
        + emotion.joy * 0.25
        + emotion.closeness * 0.20)
        * (1.0 - emotion.fear * 0.35 - emotion.anger * 0.35)
        * 100.0;
    let energy = 100.0 - fatigue;

    MoodSnapshot {
        valence,
        arousal,
        primary_emotion,
        secondary_emotion,
        primary_intensity,
        fatigue,
        stress,
        relationship_score,
        trust: relationship.trust * 100.0,
        intimacy: relationship.intimacy * 100.0,
        positive_affect: positive_affect.clamp(0.0, 100.0),
        negative_affect: negative_affect.clamp(0.0, 100.0),
        focus: focus.clamp(0.0, 100.0),
        energy: energy.clamp(0.0, 100.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mood_from_positive_emotion() {
        let emotion = EmotionState {
            joy: 0.8,
            closeness: 0.7,
            ..Default::default()
        };
        let needs = NeedsState::default();
        let rel = RelationshipState::default();
        let mood = compute_mood(&emotion, &needs, &rel, 0.0);
        assert!(mood.valence > 0.3);
        assert_eq!(mood.primary_emotion, EmotionLabel::Joy);
    }

    #[test]
    fn test_fatigue_increases_with_time() {
        let emotion = EmotionState::default();
        let needs = NeedsState::default();
        let rel = RelationshipState::default();
        let mood1 = compute_mood(&emotion, &needs, &rel, 60.0);
        let mood2 = compute_mood(&emotion, &needs, &rel, 600.0);
        assert!(mood2.fatigue > mood1.fatigue);
    }
}
