//! Appraisal 层 — 认知评估。
//!
//! 这是很多 Agent 都缺失的一层。真实人不会直接产生情绪，而是：
//! 事件 → 解释事件（Appraisal）→ 再产生情绪。
//!
//! Appraisal 由 LLM 在回复生成的同一次调用中产出（不单独成 LLM 调用），
//! 它解释「这件事对 Vivian 意味着什么」，然后由映射规则驱动 Emotion 变化。

use serde::{Deserialize, Serialize};

/// 认知评估结果（6 项，0.0-1.0）
///
/// 基于 Lazarus 认知评估理论，每次事件由 LLM 评估这 6 个维度。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Appraisal {
    /// 威胁感 — 事件是否构成对自我/关系的威胁
    pub threat: f64,
    /// 拒绝感 — 事件是否被解读为被拒绝/被排斥
    pub rejection: f64,
    /// 控制感 — Vivian 对局面有多大掌控力
    pub control: f64,
    /// 公平感 — 事件是否符合公平预期
    pub fairness: f64,
    /// 新奇度 — 事件有多出乎意料
    pub novelty: f64,
    /// 重要性 — 事件对 Vivian 的意义大小
    pub significance: f64,
}

impl Default for Appraisal {
    fn default() -> Self {
        Self {
            threat: 0.0,
            rejection: 0.0,
            control: 0.5,
            fairness: 0.5,
            novelty: 0.3,
            significance: 0.5,
        }
    }
}

/// Appraisal → Emotion 增量的映射
///
/// 这是由心理学理论驱动的固定映射，不是 LLM 决策：
/// - Threat↑ → Fear↑
/// - Rejection↑ → Sadness↑ + Loneliness↑
/// - Control↓ → Anxiety/Fear↑（但这里 Anxiety 已并入 Fear）
/// - Fairness↓ → Anger↑
/// - Novelty↑ → Curiosity↑
/// - Significance 放大所有情绪变化幅度
/// - Trust 门控：低信任放大负面情绪、抑制正面情绪
impl Appraisal {
    /// 根据 Appraisal 计算 Emotion 增量
    ///
    /// `sensitivity_mult` 来自 Persona（敏感度越高，情绪反应越大）
    /// `trust` 来自 RelationshipState（0.0-1.0），门控正负面情绪强度
    pub fn to_emotion_deltas(
        &self,
        sensitivity_mult: f64,
        trust: f64,
    ) -> super::emotion::EmotionDeltas {
        let sig = 0.5 + self.significance * 0.5;
        let m = sensitivity_mult * sig;

        let t = trust.clamp(0.0, 1.0);
        let fear_gate = (1.0 - t * 0.7).max(0.10);
        let anger_gate = (1.0 - t * 0.5).max(0.15);
        let sadness_gate = (1.0 - t * 0.3).max(0.20);
        let joy_gate = 0.5 + t * 0.5;

        super::emotion::EmotionDeltas {
            joy: self.fairness.max(0.0) * 0.15 * m * (1.0 - self.threat) * joy_gate,
            sadness: self.rejection * 0.20 * m * sadness_gate,
            anger: (1.0 - self.fairness) * 0.15 * m * anger_gate,
            fear: (self.threat * 0.20 * m + (1.0 - self.control) * 0.10 * m) * fear_gate,
            closeness: (1.0 - self.rejection) * self.fairness * 0.15 * m,
            loneliness: self.rejection * 0.12 * m,
            curiosity: self.novelty * 0.15 * m,
        }
    }

    /// 根据 Appraisal 计算 Need 增量
    ///
    /// 事件被评估为威胁/拒绝时，安全和归属需求会上升（更缺乏）。
    pub fn to_need_deltas(&self) -> super::needs::NeedDeltas {
        let sig = 0.5 + self.significance * 0.5;
        super::needs::NeedDeltas {
            security: self.threat * 0.10 * sig,
            belonging: self.rejection * 0.10 * sig,
            novelty: (1.0 - self.novelty) * 0.05 * sig, // 新奇度低 → 新鲜需求上升
            autonomy: (1.0 - self.control) * 0.05 * sig,
            expression: self.significance * 0.05 * sig,
        }
    }

    /// 转为 prompt 友好的描述（供 LLM 理解上一轮的评估结果）
    pub fn to_prompt_desc(&self) -> String {
        format!(
            "威胁 {:.0}%  拒绝 {:.0}%  控制 {:.0}%  公平 {:.0}%  新奇 {:.0}%  重要 {:.0}%",
            self.threat * 100.0,
            self.rejection * 100.0,
            self.control * 100.0,
            self.fairness * 100.0,
            self.novelty * 100.0,
            self.significance * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejection_drives_sadness() {
        let appraisal = Appraisal {
            rejection: 0.9,
            significance: 0.8,
            ..Default::default()
        };
        let deltas = appraisal.to_emotion_deltas(1.0, 0.5);
        assert!(deltas.sadness > 0.1);
        assert!(deltas.loneliness > 0.05);
    }

    #[test]
    fn test_threat_drives_fear() {
        let appraisal = Appraisal {
            threat: 0.8,
            significance: 0.7,
            ..Default::default()
        };
        let deltas = appraisal.to_emotion_deltas(1.0, 0.5);
        assert!(deltas.fear > 0.1);
    }

    #[test]
    fn test_unfair_drives_anger() {
        let appraisal = Appraisal {
            fairness: 0.1,
            significance: 0.7,
            ..Default::default()
        };
        let deltas = appraisal.to_emotion_deltas(1.0, 0.5);
        assert!(deltas.anger > 0.05);
    }

    #[test]
    fn test_low_trust_amplifies_negative() {
        let appraisal = Appraisal {
            threat: 0.7,
            rejection: 0.7,
            fairness: 0.2,
            significance: 0.8,
            ..Default::default()
        };
        let low_trust = appraisal.to_emotion_deltas(1.0, 0.1);
        let high_trust = appraisal.to_emotion_deltas(1.0, 0.9);
        assert!(low_trust.fear > high_trust.fear);
        assert!(low_trust.anger > high_trust.anger);
        assert!(low_trust.sadness > high_trust.sadness);
    }

    #[test]
    fn test_low_trust_suppresses_joy() {
        let appraisal = Appraisal {
            fairness: 0.8,
            significance: 0.7,
            ..Default::default()
        };
        let low_trust = appraisal.to_emotion_deltas(1.0, 0.1);
        let high_trust = appraisal.to_emotion_deltas(1.0, 0.9);
        assert!(low_trust.joy < high_trust.joy);
    }
}
