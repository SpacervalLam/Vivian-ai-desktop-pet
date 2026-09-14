//! Needs 层 — 心理需求（5 项）。
//!
//! 需求是真正的行为驱动力。当需求未满足时（高于 set_point），会驱动 Behavior Drive。
//! 不是聊天欲这种游戏化数值，而是基于自我决定理论（Deci & Ryan）的真实心理需求。

use serde::{Deserialize, Serialize};

/// 心理需求状态（5 项，0.0-1.0）
///
/// 语义：值越高表示「越缺乏/越需要」。
/// - 0.0 = 完全满足
/// - 1.0 = 极度缺乏
///
/// 由 Homeostasis 自动调节：满足后缓慢回升，未满足时持续增长。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsState {
    /// 归属感 — 被接纳、被关心的需求
    pub belonging: f64,
    /// 自主性 — 自我决定、不受控制的需求
    pub autonomy: f64,
    /// 安全感 — 安全、稳定、可预测的需求
    pub security: f64,
    /// 新鲜感 — 探索、新奇、刺激的需求
    pub novelty: f64,
    /// 表达欲 — 自我表达、被理解的需求
    pub expression: f64,
}

impl Default for NeedsState {
    fn default() -> Self {
        Self {
            belonging: 0.40,
            autonomy: 0.35,
            security: 0.25,
            novelty: 0.45,
            expression: 0.35,
        }
    }
}

/// 需求增量（由 LLM 在 emotion_update 同期产出，或由事件驱动）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NeedDeltas {
    pub belonging: f64,
    pub autonomy: f64,
    pub security: f64,
    pub novelty: f64,
    pub expression: f64,
}

impl NeedsState {
    /// 应用需求增量（钳制到 0.0-1.0）
    ///
    /// 正增量表示「需求增加（更缺乏）」，负增量表示「需求被满足」。
    pub fn apply_delta(&mut self, delta: &NeedDeltas) {
        self.belonging = (self.belonging + delta.belonging).clamp(0.0, 1.0);
        self.autonomy = (self.autonomy + delta.autonomy).clamp(0.0, 1.0);
        self.security = (self.security + delta.security).clamp(0.0, 1.0);
        self.novelty = (self.novelty + delta.novelty).clamp(0.0, 1.0);
        self.expression = (self.expression + delta.expression).clamp(0.0, 1.0);
    }

    /// 返回最缺乏的需求（值最高者）及其标签
    pub fn most_deficient(&self) -> (&str, f64) {
        let items = [
            ("belonging", self.belonging),
            ("autonomy", self.autonomy),
            ("security", self.security),
            ("novelty", self.novelty),
            ("expression", self.expression),
        ];
        items
            .iter()
            .copied()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap()
    }

    /// 转为 prompt 友好的描述
    pub fn to_prompt_desc(&self) -> String {
        format!(
            "归属 {:.0}%  自主 {:.0}%  安全 {:.0}%  新鲜 {:.0}%  表达 {:.0}%",
            self.belonging * 100.0,
            self.autonomy * 100.0,
            self.security * 100.0,
            self.novelty * 100.0,
            self.expression * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_delta_clamps() {
        let mut needs = NeedsState::default();
        needs.apply_delta(&NeedDeltas {
            belonging: 0.8,
            ..Default::default()
        });
        assert!(needs.belonging <= 1.0);
    }

    #[test]
    fn test_most_deficient() {
        let needs = NeedsState {
            belonging: 0.2,
            autonomy: 0.8,
            security: 0.3,
            novelty: 0.4,
            expression: 0.5,
        };
        let (label, val) = needs.most_deficient();
        assert_eq!(label, "autonomy");
        assert!((val - 0.8).abs() < 0.001);
    }
}
