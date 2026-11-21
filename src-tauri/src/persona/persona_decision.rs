//! Persona Decision —— 人格对决策层的权重映射。
//!
//! ## 解决的问题
//!
//! "嘴硬/爱抬杠/阴阳"等性格倾向不应只写在 prompt 里，而应落在 Decision 层的权重上。
//! 同一情境下不同人格会做出不同决策——因为各自的 Decision 层被人格约束。
//!
//! 现在的 Vivian：`response_mode` 由 LLM 一次性输出，人格对它的影响只在 prompt 里。
//! 这意味着 Vivian 和 Nana 在同一情境下可能做出相同决策——因为 LLM 没有强约束。
//!
//! ## 设计
//!
//! 把 8 维 `CharacterExpression` 映射为三个决策权重：
//! - **Think propensity**：倾向内部思考的程度（好奇心高 + 内向 → 高）
//! - **Act propensity**：倾向主动行动的程度（元气高 + 黏人高 → 高）
//! - **Speak propensity**：倾向开口说话的程度（黏人高 + 元气高 → 高，傲娇高 → 中等）
//!
//! 这些权重不直接决定行为，而是调整 Cognitive Tick 各阶段的决策阈值：
//! - Think 阶段：think_propensity 高 → 放宽 Think 触发条件
//! - Act 阶段：act_propensity 高 → 放宽 Act 触发条件
//! - Speak 阶段：speak_propensity 高 → 放宽主动消息触发条件
//!
//! ## 与 prompt 层的区别
//!
//! prompt 层告诉 LLM "你是傲娇的"——LLM 知道但行为不一定一致。
//! Decision 层用傲娇权重调整阈值——规则强制行为偏向，LLM 无法绕过。
//!
//! 举例：Nana 的 healing=0.9 + clingy=0.4，Vivian 的 sass=0.65 + tsundere=0.3
//! - 同样看到用户 5 分钟没动静
//! - Nana（speak_propensity 高）：可能主动问"在忙吗？"
//! - Vivian（speak_propensity 中等）：可能选择 internal thinking 而非立即说话
//!
//! 这样即使两个角色用同一个 LLM、同一个 prompt 模板，决策层也会让人格生效。

use serde::{Deserialize, Serialize};

use crate::persona::CharacterExpression;

/// 人格决策权重（由 CharacterExpression 派生，0.0-1.0）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaDecisionWeights {
    /// 倾向内部思考的程度
    pub think_propensity: f64,
    /// 倾向主动行动的程度
    pub act_propensity: f64,
    /// 倾向开口说话的程度
    pub speak_propensity: f64,
    /// 决策风格描述（调试 + prompt 注入用）
    pub style_label: String,
}

impl Default for PersonaDecisionWeights {
    fn default() -> Self {
        Self {
            think_propensity: 0.5,
            act_propensity: 0.5,
            speak_propensity: 0.5,
            style_label: "balanced".to_string(),
        }
    }
}

impl PersonaDecisionWeights {
    /// 从 8 维 CharacterExpression 派生决策权重
    ///
    /// 映射规则（基于角色设计直觉，非严格心理学，可调）：
    ///
    /// **think_propensity** = curiosity × 0.4 + (1 - genki) × 0.3 + healing × 0.3
    /// - 好奇心高 → 喜欢思考
    /// - 元气低（内向）→ 更倾向内部思考而非直接行动
    /// - 治愈高 → 倾向反思和共情
    ///
    /// **act_propensity** = genki × 0.4 + clingy × 0.3 + curiosity × 0.3
    /// - 元气高 → 行动派
    /// - 黏人高 → 主动靠近
    /// - 好奇心高 → 主动探索
    ///
    /// **speak_propensity** = clingy × 0.35 + genki × 0.3 + (1 - tsundere × 0.5) × 0.2 + ritual × 0.15
    /// - 黏人高 → 想说话
    /// - 元气高 → 话多
    /// - 傲娇高 → 略微抑制直接说话（要先装一下）
    /// - 仪式感高 → 主动问候
    pub fn from_expression(expr: &CharacterExpression) -> Self {
        let think = (expr.curiosity * 0.4
            + (1.0 - expr.genki) * 0.3
            + expr.healing * 0.3)
            .clamp(0.0, 1.0);

        let act = (expr.genki * 0.4
            + expr.clingy * 0.3
            + expr.curiosity * 0.3)
            .clamp(0.0, 1.0);

        let speak = (expr.clingy * 0.35
            + expr.genki * 0.3
            + (1.0 - expr.tsundere * 0.5) * 0.2
            + expr.ritual * 0.15)
            .clamp(0.0, 1.0);

        let style_label = Self::derive_style_label(think, act, speak);

        Self {
            think_propensity: think,
            act_propensity: act,
            speak_propensity: speak,
            style_label,
        }
    }

    /// 根据三维权重派生风格标签（调试 + prompt 注入用）
    fn derive_style_label(think: f64, act: f64, speak: f64) -> String {
        // 找到最高维度
        let max = think.max(act).max(speak);
        if (think - max).abs() < 0.05 && think > 0.6 {
            "reflective".to_string() // 偏思考型
        } else if (act - max).abs() < 0.05 && act > 0.6 {
            "proactive".to_string() // 偏行动型
        } else if (speak - max).abs() < 0.05 && speak > 0.6 {
            "expressive".to_string() // 偏表达型
        } else if think < 0.4 && act < 0.4 && speak < 0.4 {
            "reserved".to_string() // 克制型
        } else {
            "balanced".to_string() // 平衡型
        }
    }

    /// Think 阶段阈值调整：propensity 越高，触发条件越宽松
    ///
    /// 基础阈值 0.6，propensity 0.5 时不变，0.8 时降到 0.45，0.2 时升到 0.7
    pub fn think_threshold(&self) -> f64 {
        Self::adjust_threshold(0.6, self.think_propensity)
    }

    /// Act 阶段阈值调整
    pub fn act_threshold(&self) -> f64 {
        Self::adjust_threshold(0.6, self.act_propensity)
    }

    /// Speak 阶段阈值调整：propensity 越高，主动消息触发条件越宽松
    pub fn speak_threshold(&self) -> f64 {
        Self::adjust_threshold(0.6, self.speak_propensity)
    }

    /// 通用阈值调整函数
    ///
    /// propensity=0.5 → 返回 base（中性）
    /// propensity=1.0 → 返回 base - 0.15（更易触发）
    /// propensity=0.0 → 返回 base + 0.15（更难触发）
    fn adjust_threshold(base: f64, propensity: f64) -> f64 {
        let delta = (propensity - 0.5) * -0.3; // -0.15 到 +0.15
        (base + delta).clamp(0.1, 0.9)
    }

    /// 序列化为 prompt 段落（注入 LLM，让 LLM 也感知决策风格）
    ///
    /// 注：这是辅助提示，真正的决策约束在规则层，不依赖 LLM 遵守。
    pub fn serialize_for_prompt(&self, lang: &str) -> Option<String> {
        if self.style_label == "balanced" {
            return None;
        }
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let (style_label, think_label, act_label, speak_label) = match lang_norm {
            "en" => ("Style", "think", "act", "speak"),
            "ja" => ("スタイル", "思考", "行動", "発言"),
            _ => ("风格", "思考", "行动", "发言"),
        };
        let header = crate::pipeline::prompt_modules::section_heading("decision_style", lang);
        Some(format!(
            "{}\n- {}: {} ({}={:.0}%, {}={:.0}%, {}={:.0}%)",
            header,
            style_label,
            self.style_label,
            think_label,
            self.think_propensity * 100.0,
            act_label,
            self.act_propensity * 100.0,
            speak_label,
            self.speak_propensity * 100.0
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vivian_expr() -> CharacterExpression {
        CharacterExpression {
            tsundere: 0.30,
            clingy: 0.50,
            genki: 0.75,
            sass: 0.65,
            healing: 0.65,
            curiosity: 0.75,
            ritual: 0.50,
            habit_awareness: 0.65,
        }
    }

    fn nana_expr() -> CharacterExpression {
        CharacterExpression {
            tsundere: 0.05,
            clingy: 0.40,
            genki: 0.30,
            sass: 0.10,
            healing: 0.90,
            curiosity: 0.65,
            ritual: 0.70,
            habit_awareness: 0.80,
        }
    }

    #[test]
    fn vivian_is_more_proactive_than_nana() {
        let vivian = PersonaDecisionWeights::from_expression(&vivian_expr());
        let nana = PersonaDecisionWeights::from_expression(&nana_expr());

        // Vivian 元气高 → act_propensity 应高于 Nana
        assert!(
            vivian.act_propensity > nana.act_propensity,
            "Vivian act ({}) should > Nana act ({})",
            vivian.act_propensity,
            nana.act_propensity
        );
    }

    #[test]
    fn nana_is_more_reflective_than_vivian() {
        let vivian = PersonaDecisionWeights::from_expression(&vivian_expr());
        let nana = PersonaDecisionWeights::from_expression(&nana_expr());

        // Nana 元气低 + 治愈高 → think_propensity 应高于 Vivian
        assert!(
            nana.think_propensity > vivian.think_propensity,
            "Nana think ({}) should > Vivian think ({})",
            nana.think_propensity,
            vivian.think_propensity
        );
    }

    #[test]
    fn think_threshold_decreases_with_high_propensity() {
        let weights = PersonaDecisionWeights {
            think_propensity: 0.9,
            ..Default::default()
        };
        let threshold = weights.think_threshold();
        // 高 propensity → 低阈值（更易触发）
        assert!(threshold < 0.6, "high propensity should lower threshold");
    }

    #[test]
    fn think_threshold_increases_with_low_propensity() {
        let weights = PersonaDecisionWeights {
            think_propensity: 0.1,
            ..Default::default()
        };
        let threshold = weights.think_threshold();
        // 低 propensity → 高阈值（更难触发）
        assert!(threshold > 0.6, "low propensity should raise threshold");
    }

    #[test]
    fn neutral_propensity_keeps_base_threshold() {
        let weights = PersonaDecisionWeights {
            think_propensity: 0.5,
            ..Default::default()
        };
        let threshold = weights.think_threshold();
        // propensity=0.5 → 阈值不变
        assert!((threshold - 0.6).abs() < 0.01);
    }

    #[test]
    fn style_label_reflective_for_high_think() {
        let weights = PersonaDecisionWeights {
            think_propensity: 0.8,
            act_propensity: 0.3,
            speak_propensity: 0.3,
            style_label: String::new(),
        };
        let label = PersonaDecisionWeights::derive_style_label(
            weights.think_propensity,
            weights.act_propensity,
            weights.speak_propensity,
        );
        assert_eq!(label, "reflective");
    }

    #[test]
    fn style_label_proactive_for_high_act() {
        let label = PersonaDecisionWeights::derive_style_label(0.3, 0.8, 0.3);
        assert_eq!(label, "proactive");
    }

    #[test]
    fn style_label_expressive_for_high_speak() {
        let label = PersonaDecisionWeights::derive_style_label(0.3, 0.3, 0.8);
        assert_eq!(label, "expressive");
    }

    #[test]
    fn style_label_balanced_for_neutral() {
        let label = PersonaDecisionWeights::derive_style_label(0.5, 0.5, 0.5);
        assert_eq!(label, "balanced");
    }

    #[test]
    fn serialize_for_prompt_returns_none_for_balanced() {
        let weights = PersonaDecisionWeights::default();
        assert_eq!(weights.serialize_for_prompt("zh"), None);
    }

    #[test]
    fn serialize_for_prompt_includes_label_for_non_balanced() {
        let weights = PersonaDecisionWeights {
            think_propensity: 0.8,
            act_propensity: 0.3,
            speak_propensity: 0.3,
            style_label: "reflective".to_string(),
        };
        let s = weights.serialize_for_prompt("zh").unwrap();
        assert!(s.contains("reflective"));
        assert!(s.contains("决策风格"));
    }
}
