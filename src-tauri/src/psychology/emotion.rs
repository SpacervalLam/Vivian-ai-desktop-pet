//! Emotion 层 — 真正的情绪（7 项）。
//!
//! 7 项核心情绪：Joy / Sadness / Anger / Fear / Closeness / Loneliness / Curiosity。
//! 不包含 Trust（属于 Relationship 层，参见 relationship.rs）。
//! 不包含 Confidence（属于认知）、不包含 Anxiety（并入 Fear）。
//!
//! Emotion 由 Appraisal 驱动（不是事件直接驱动），并由 Homeostasis 自动回归到 set point。

use serde::{Deserialize, Serialize};

/// 情绪标签 — 7 项核心情绪
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmotionLabel {
    Joy,
    Sadness,
    Anger,
    Fear,
    Closeness,
    Loneliness,
    Curiosity,
}

impl EmotionLabel {
    pub fn as_str(&self) -> &'static str {
        match self {
            EmotionLabel::Joy => "joy",
            EmotionLabel::Sadness => "sadness",
            EmotionLabel::Anger => "anger",
            EmotionLabel::Fear => "fear",
            EmotionLabel::Closeness => "closeness",
            EmotionLabel::Loneliness => "loneliness",
            EmotionLabel::Curiosity => "curiosity",
        }
    }

    pub fn display_zh(&self) -> &'static str {
        match self {
            EmotionLabel::Joy => "快乐",
            EmotionLabel::Sadness => "悲伤",
            EmotionLabel::Anger => "愤怒",
            EmotionLabel::Fear => "恐惧",
            EmotionLabel::Closeness => "亲近",
            EmotionLabel::Loneliness => "孤独",
            EmotionLabel::Curiosity => "好奇",
        }
    }

    /// 全部 7 类（按枚举声明顺序）
    pub fn all() -> &'static [EmotionLabel] {
        &[
            EmotionLabel::Joy,
            EmotionLabel::Sadness,
            EmotionLabel::Anger,
            EmotionLabel::Fear,
            EmotionLabel::Closeness,
            EmotionLabel::Loneliness,
            EmotionLabel::Curiosity,
        ]
    }

    /// 由字符串标签解析；未知回退为 Curiosity（默认好奇）
    pub fn from_str(label: &str) -> Self {
        match label {
            "joy" => EmotionLabel::Joy,
            "sadness" => EmotionLabel::Sadness,
            "anger" => EmotionLabel::Anger,
            "fear" => EmotionLabel::Fear,
            "closeness" => EmotionLabel::Closeness,
            "loneliness" => EmotionLabel::Loneliness,
            "curiosity" => EmotionLabel::Curiosity,
            _ => EmotionLabel::Curiosity,
        }
    }

    /// 由 valence/arousal 二维推断主导情绪标签
    ///
    /// 用于把旧版基于 VA 的情绪推断逻辑收敛到统一模型。
    pub fn from_valence_arousal(valence: f64, arousal: f64) -> Self {
        if valence > 0.4 && arousal > 0.6 {
            EmotionLabel::Joy
        } else if valence > 0.2 && arousal > 0.5 {
            EmotionLabel::Joy
        } else if valence < -0.3 && arousal > 0.6 {
            EmotionLabel::Anger
        } else if valence < -0.2 && arousal > 0.5 {
            EmotionLabel::Fear
        } else if valence < -0.3 {
            EmotionLabel::Sadness
        } else if arousal < 0.25 && valence < 0.0 {
            EmotionLabel::Loneliness
        } else if arousal > 0.5 && valence >= -0.1 && valence <= 0.3 {
            EmotionLabel::Curiosity
        } else if valence > 0.1 {
            EmotionLabel::Closeness
        } else {
            EmotionLabel::Curiosity
        }
    }

    /// 映射到 Live2D 表情名
    ///
    /// Vivian 模型支持的表情：shy / cry / smile / eye_roll / default / angry / surprised 等。
    /// 返回空字符串表示不触发表情变化。
    pub fn to_live2d_expression(&self) -> &'static str {
        match self {
            EmotionLabel::Joy => "shy",
            EmotionLabel::Sadness => "",
            EmotionLabel::Anger => "cry",
            EmotionLabel::Fear => "cry",
            EmotionLabel::Closeness => "shy",
            EmotionLabel::Loneliness => "",
            EmotionLabel::Curiosity => "",
        }
    }
}

/// 情绪状态（7 项，0.0-1.0）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionState {
    pub joy: f64,
    pub sadness: f64,
    pub anger: f64,
    pub fear: f64,
    pub closeness: f64,
    pub loneliness: f64,
    pub curiosity: f64,
}

impl Default for EmotionState {
    fn default() -> Self {
        Self {
            joy: 0.35,
            sadness: 0.05,
            anger: 0.05,
            fear: 0.10,
            closeness: 0.35,
            loneliness: 0.15,
            curiosity: 0.45,
        }
    }
}

/// 情绪增量（-0.3 ~ +0.3），由 LLM 在 emotion_update 中产出
#[derive(Debug, Clone, Serialize, Deserialize, Default, schemars::JsonSchema)]
pub struct EmotionDeltas {
    pub joy: f64,
    pub sadness: f64,
    pub anger: f64,
    pub fear: f64,
    pub closeness: f64,
    pub loneliness: f64,
    pub curiosity: f64,
}

impl EmotionState {
    /// 应用情绪增量（钳制到 0.0-1.0）
    ///
    /// `sensitivity_mult` 来自 Persona，放大情绪变化幅度。
    pub fn apply_delta(&mut self, delta: &EmotionDeltas, sensitivity_mult: f64) {
        let m = sensitivity_mult;
        self.joy = (self.joy + delta.joy * m).clamp(0.0, 1.0);
        self.sadness = (self.sadness + delta.sadness * m).clamp(0.0, 1.0);
        self.anger = (self.anger + delta.anger * m).clamp(0.0, 1.0);
        self.fear = (self.fear + delta.fear * m).clamp(0.0, 1.0);
        self.closeness = (self.closeness + delta.closeness * m).clamp(0.0, 1.0);
        self.loneliness = (self.loneliness + delta.loneliness * m).clamp(0.0, 1.0);
        self.curiosity = (self.curiosity + delta.curiosity * m).clamp(0.0, 1.0);
    }

    /// 情绪间互相调制 —— 各通道不抵消，而是放大或抑制
    ///
    /// 返回触发的混合标签（供日志记录）。
    pub fn apply_interactions(&mut self) -> Vec<&'static str> {
        let mut blends = Vec::new();
        let sat = |x: f64| (1.0 - x).max(0.0);

        if self.sadness > 0.15 && self.anger > 0.15 {
            self.anger = (self.anger + self.sadness * 0.30 * sat(self.anger)).min(1.0);
        }
        if self.anger > 0.15 && self.closeness > 0.15 {
            self.closeness = (self.closeness - self.anger * 0.10).max(0.0);
        }
        if self.closeness > 0.15 && self.fear > 0.15 {
            self.fear = (self.fear - self.closeness * 0.15).max(0.0);
        }
        if self.fear > 0.15 && self.anger > 0.15 {
            self.anger = (self.anger + self.fear * 0.20 * sat(self.anger)).min(1.0);
        }
        if self.loneliness > 0.15 && self.sadness > 0.15 {
            self.sadness = (self.sadness + self.loneliness * 0.20 * sat(self.sadness)).min(1.0);
        }
        if self.loneliness > 0.20 && self.joy > 0.20 {
            self.joy = (self.joy - self.loneliness * 0.15).max(0.0);
        }
        if self.curiosity > 0.20 && self.fear > 0.20 {
            self.fear = (self.fear + self.curiosity * 0.30 * sat(self.fear)).min(1.0);
        }
        if self.curiosity > 0.20 && self.joy > 0.20 {
            self.joy = (self.joy + self.curiosity * 0.20 * sat(self.joy)).min(1.0);
        }
        if self.closeness > 0.20 && self.joy > 0.20 {
            self.joy = (self.joy + self.closeness * 0.15 * sat(self.joy)).min(1.0);
        }

        if self.joy > 0.20 && self.sadness > 0.20 {
            blends.push("bittersweet");
        }
        if self.joy > 0.20 && self.fear > 0.20 {
            blends.push("fear_of_loss");
        }
        if self.loneliness > 0.30 && self.closeness > 0.30 {
            blends.push("yearning");
        }

        blends
    }

    /// 返回主导情绪（值最高者）及其标签
    pub fn dominant(&self) -> (EmotionLabel, f64) {
        let items = [
            (EmotionLabel::Joy, self.joy),
            (EmotionLabel::Sadness, self.sadness),
            (EmotionLabel::Anger, self.anger),
            (EmotionLabel::Fear, self.fear),
            (EmotionLabel::Closeness, self.closeness),
            (EmotionLabel::Loneliness, self.loneliness),
            (EmotionLabel::Curiosity, self.curiosity),
        ];
        items
            .iter()
            .copied()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap()
    }

    /// 计算效价（valence, -1.0 ~ 1.0）— 正面情绪减负面情绪
    pub fn valence(&self) -> f64 {
        let positive = self.joy * 0.5 + self.closeness * 0.3 + self.curiosity * 0.2;
        let negative = self.sadness * 0.3 + self.anger * 0.3 + self.fear * 0.2 + self.loneliness * 0.2;
        (positive - negative).clamp(-1.0, 1.0)
    }

    /// 计算唤醒度（arousal, 0.0 ~ 1.0）— 情绪激活强度
    pub fn arousal(&self) -> f64 {
        (self.anger * 0.3
            + self.fear * 0.3
            + self.joy * 0.2
            + self.curiosity * 0.15
            + self.sadness * 0.05)
            .clamp(0.0, 1.0)
    }

    /// 转为 prompt 友好的描述
    pub fn to_prompt_desc(&self) -> String {
        format!(
            "快乐 {:.0}%  悲伤 {:.0}%  愤怒 {:.0}%  恐惧 {:.0}%  亲近 {:.0}%  孤独 {:.0}%  好奇 {:.0}%",
            self.joy * 100.0,
            self.sadness * 100.0,
            self.anger * 100.0,
            self.fear * 100.0,
            self.closeness * 100.0,
            self.loneliness * 100.0,
            self.curiosity * 100.0
        )
    }

    /// 8 维情绪向量：7 项 EmotionState + trust（来自 RelationshipState）。
    ///
    /// 用于 emotion→temperature 映射，让 LLM 输出温度随情绪变化：
    /// 越开心/信任 → 越开放（高温度）；越悲伤/孤独 → 越克制（低温度）。
    pub fn to_8d_vector(&self, trust: f64) -> EmotionVector8D {
        EmotionVector8D {
            joy: self.joy,
            sadness: self.sadness,
            anger: self.anger,
            fear: self.fear,
            closeness: self.closeness,
            loneliness: self.loneliness,
            curiosity: self.curiosity,
            trust: trust.clamp(0.0, 1.0),
        }
    }
}

/// 8 维情绪向量（7 项 EmotionState + trust）
#[derive(Debug, Clone, Copy)]
pub struct EmotionVector8D {
    pub joy: f64,
    pub sadness: f64,
    pub anger: f64,
    pub fear: f64,
    pub closeness: f64,
    pub loneliness: f64,
    pub curiosity: f64,
    pub trust: f64,
}

impl EmotionVector8D {
    /// 默认映射参数
    pub const DEFAULT_BASE_TEMP: f64 = 0.8;
    pub const DEFAULT_SCALE: f64 = 0.4;
    pub const DEFAULT_MIN_TEMP: f64 = 0.3;
    pub const DEFAULT_MAX_TEMP: f64 = 1.2;

    /// 情绪向量 → LLM temperature 映射。
    ///
    /// 正面情绪（joy/closeness/curiosity/trust）提升温度 → 输出更开放多样；
    /// 负面情绪（sadness/anger/fear/loneliness）降低温度 → 输出更克制稳定。
    ///
    /// - `base_temp`：中性情绪时的基准温度（默认 0.8）
    /// - `scale`：情绪对温度的影响幅度（默认 0.4，即 ±0.4 范围）
    /// - 结果钳制到 `[min_temp, max_temp]`（默认 [0.3, 1.2]）
    pub fn to_temperature(
        &self,
        base_temp: f64,
        scale: f64,
        min_temp: f64,
        max_temp: f64,
    ) -> f64 {
        let positive = self.joy * 0.30
            + self.closeness * 0.20
            + self.curiosity * 0.15
            + self.trust * 0.35;
        let negative = self.sadness * 0.30
            + self.anger * 0.25
            + self.fear * 0.20
            + self.loneliness * 0.25;
        let delta = (positive - negative) * scale;
        (base_temp + delta).clamp(min_temp, max_temp)
    }

    /// 使用默认参数的便捷映射
    pub fn to_temperature_default(&self) -> f64 {
        self.to_temperature(
            Self::DEFAULT_BASE_TEMP,
            Self::DEFAULT_SCALE,
            Self::DEFAULT_MIN_TEMP,
            Self::DEFAULT_MAX_TEMP,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_delta_clamps() {
        let mut emotion = EmotionState::default();
        emotion.apply_delta(
            &EmotionDeltas {
                joy: 0.5,
                ..Default::default()
            },
            1.0,
        );
        assert!(emotion.joy <= 1.0);
    }

    #[test]
    fn test_sensitivity_amplifies() {
        let mut e1 = EmotionState::default();
        let mut e2 = EmotionState::default();
        let delta = EmotionDeltas {
            joy: 0.2,
            ..Default::default()
        };
        e1.apply_delta(&delta, 1.0);
        e2.apply_delta(&delta, 1.5);
        assert!(e2.joy > e1.joy);
    }

    #[test]
    fn test_dominant() {
        let emotion = EmotionState {
            joy: 0.8,
            sadness: 0.1,
            ..Default::default()
        };
        let (label, val) = emotion.dominant();
        assert_eq!(label, EmotionLabel::Joy);
        assert!((val - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_valence() {
        let positive = EmotionState {
            joy: 0.8,
            closeness: 0.6,
            ..Default::default()
        };
        assert!(positive.valence() > 0.3);

        let negative = EmotionState {
            sadness: 0.8,
            anger: 0.7,
            ..Default::default()
        };
        assert!(negative.valence() < -0.3);
    }

    #[test]
    fn test_no_trust_field() {
        // Trust 已从 EmotionState 中移除，仅保留在 RelationshipState
        let state = EmotionState::default();
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("trust"));
    }

    #[test]
    fn test_interactions_sadness_amplifies_anger() {
        let mut emotion = EmotionState {
            sadness: 0.5,
            anger: 0.3,
            ..Default::default()
        };
        let anger_before = emotion.anger;
        emotion.apply_interactions();
        assert!(emotion.anger > anger_before);
    }

    #[test]
    fn test_interactions_closeness_suppresses_fear() {
        let mut emotion = EmotionState {
            closeness: 0.6,
            fear: 0.5,
            ..Default::default()
        };
        let fear_before = emotion.fear;
        emotion.apply_interactions();
        assert!(emotion.fear < fear_before);
    }

    #[test]
    fn test_interactions_bittersweet_blend() {
        let mut emotion = EmotionState {
            joy: 0.5,
            sadness: 0.5,
            ..Default::default()
        };
        let blends = emotion.apply_interactions();
        assert!(blends.contains(&"bittersweet"));
    }

    #[test]
    fn test_interactions_loneliness_amplifies_sadness() {
        let mut emotion = EmotionState {
            loneliness: 0.6,
            sadness: 0.3,
            ..Default::default()
        };
        let sadness_before = emotion.sadness;
        emotion.apply_interactions();
        assert!(emotion.sadness > sadness_before);
    }
}
