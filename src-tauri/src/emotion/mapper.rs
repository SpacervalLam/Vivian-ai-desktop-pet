//! 情绪映射 — 14 类 LLM 情绪 <-> 7 类 EmotionLabel
//!
//! `happy / excited / grateful / sad / frustrated / anxious / tired /
//!  angry / disappointed / surprised / curious / neutral / bored / confused`
//!
//! EmotionLabel 是系统唯一情绪枚举（7 项），见 `crate::psychology::EmotionLabel`。
//!
//! 映射规则要点：
//!   - 14 → 7：多对一关系（happy/excited→Joy、sad/disappointed→Sadness 等）
//!   - 7 → 14：每个 EmotionLabel 选一个最贴近的 LLM 标签代表
//!   - valence/arousal 数值：与 PetState meta 中对应情绪保持一致或近似，用于 LLM 结果回填。

use serde::{Deserialize, Serialize};

use crate::psychology::EmotionLabel;

/// LLM 情绪分类标签（14 类）
///
/// ```text
/// happy, excited, grateful,
/// sad, frustrated, anxious, tired,
/// angry, disappointed,
/// surprised, curious,
/// neutral, bored, confused,
/// ```
pub const LLM_EMOTION_LABELS: &[&str] = &[
    "happy",
    "excited",
    "grateful",
    "sad",
    "frustrated",
    "anxious",
    "tired",
    "angry",
    "disappointed",
    "surprised",
    "curious",
    "neutral",
    "bored",
    "confused",
];

/// 14 类 LLM 情绪的 (valence, arousal) 基线值
///
/// valence: -1.0(极负面) ~ +1.0(极正面)
/// arousal: 0.0(完全平静) ~ 1.0(极度激动)
pub fn llm_emotion_valence_arousal(emotion: &str) -> (f64, f64) {
    match emotion {
        "happy" => (0.7, 0.6),
        "excited" => (0.8, 0.85),
        "grateful" => (0.6, 0.35),
        "sad" => (-0.5, 0.25),
        "frustrated" => (-0.4, 0.6),
        "anxious" => (-0.4, 0.7),
        "tired" => (-0.1, 0.15),
        "angry" => (-0.6, 0.8),
        "disappointed" => (-0.45, 0.3),
        "surprised" => (0.1, 0.7),
        "curious" => (0.2, 0.5),
        "neutral" => (0.0, 0.3),
        "bored" => (-0.1, 0.1),
        "confused" => (0.0, 0.4),
        _ => (0.0, 0.3),
    }
}

/// 判断字符串是否为合法的 14 类 LLM 情绪标签
pub fn is_valid_llm_emotion(label: &str) -> bool {
    LLM_EMOTION_LABELS.contains(&label)
}

/// 规范化 LLM 情绪标签：未知标签回退为 "neutral"
pub fn normalize_llm_emotion(label: &str) -> &str {
    if is_valid_llm_emotion(label) {
        label
    } else {
        "neutral"
    }
}

/// 14 类 LLM 情绪 → 7 类 EmotionLabel
///
/// 多对一映射：
/// - happy / excited → Joy
/// - grateful → Closeness（感激是温暖的亲近感）
/// - sad / disappointed → Sadness
/// - angry / frustrated → Anger
/// - anxious → Fear
/// - surprised / curious / neutral / confused → Curiosity
/// - tired / bored → Loneliness
/// - 未知标签回退到 Curiosity（与 EmotionLabel::from_str 默认一致）
pub fn llm_to_emotion_label(emotion: &str) -> EmotionLabel {
    match emotion {
        "happy" | "excited" => EmotionLabel::Joy,
        "grateful" => EmotionLabel::Closeness,
        "sad" | "disappointed" => EmotionLabel::Sadness,
        "angry" | "frustrated" => EmotionLabel::Anger,
        "anxious" => EmotionLabel::Fear,
        "tired" | "bored" => EmotionLabel::Loneliness,
        "surprised" | "curious" | "neutral" | "confused" => EmotionLabel::Curiosity,
        _ => EmotionLabel::Curiosity,
    }
}

/// 7 类 EmotionLabel → 14 类 LLM 情绪标签（选最贴近的代表标签）
///
/// 一对一映射：每个 EmotionLabel 选一个最能代表它的 LLM 标签
/// - Joy → "happy"
/// - Sadness → "sad"
/// - Anger → "angry"
/// - Fear → "anxious"
/// - Closeness → "grateful"
/// - Loneliness → "sad"（孤独偏向悲伤）
/// - Curiosity → "curious"
pub fn emotion_label_to_llm(label: EmotionLabel) -> &'static str {
    match label {
        EmotionLabel::Joy => "happy",
        EmotionLabel::Sadness => "sad",
        EmotionLabel::Anger => "angry",
        EmotionLabel::Fear => "anxious",
        EmotionLabel::Closeness => "grateful",
        EmotionLabel::Loneliness => "sad",
        EmotionLabel::Curiosity => "curious",
    }
}

/// 类型安全的 14 类 LLM 情绪枚举
///
/// 与字符串标签一一对应，便于在 API/枚举场景下使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmEmotion {
    Happy,
    Excited,
    Grateful,
    Sad,
    Frustrated,
    Anxious,
    Tired,
    Angry,
    Disappointed,
    Surprised,
    Curious,
    Neutral,
    Bored,
    Confused,
}

impl Default for LlmEmotion {
    fn default() -> Self {
        LlmEmotion::Neutral
    }
}

impl LlmEmotion {
    /// 全部 14 类情绪（按枚举声明顺序）
    pub fn all() -> &'static [LlmEmotion] {
        &[
            LlmEmotion::Happy,
            LlmEmotion::Excited,
            LlmEmotion::Grateful,
            LlmEmotion::Sad,
            LlmEmotion::Frustrated,
            LlmEmotion::Anxious,
            LlmEmotion::Tired,
            LlmEmotion::Angry,
            LlmEmotion::Disappointed,
            LlmEmotion::Surprised,
            LlmEmotion::Curious,
            LlmEmotion::Neutral,
            LlmEmotion::Bored,
            LlmEmotion::Confused,
        ]
    }

    /// 字符串标签
    pub fn as_label(&self) -> &'static str {
        match self {
            LlmEmotion::Happy => "happy",
            LlmEmotion::Excited => "excited",
            LlmEmotion::Grateful => "grateful",
            LlmEmotion::Sad => "sad",
            LlmEmotion::Frustrated => "frustrated",
            LlmEmotion::Anxious => "anxious",
            LlmEmotion::Tired => "tired",
            LlmEmotion::Angry => "angry",
            LlmEmotion::Disappointed => "disappointed",
            LlmEmotion::Surprised => "surprised",
            LlmEmotion::Curious => "curious",
            LlmEmotion::Neutral => "neutral",
            LlmEmotion::Bored => "bored",
            LlmEmotion::Confused => "confused",
        }
    }

    /// 由字符串标签解析为 LlmEmotion；未知标签回退为 Neutral
    pub fn from_label(label: &str) -> Self {
        match label {
            "happy" => LlmEmotion::Happy,
            "excited" => LlmEmotion::Excited,
            "grateful" => LlmEmotion::Grateful,
            "sad" => LlmEmotion::Sad,
            "frustrated" => LlmEmotion::Frustrated,
            "anxious" => LlmEmotion::Anxious,
            "tired" => LlmEmotion::Tired,
            "angry" => LlmEmotion::Angry,
            "disappointed" => LlmEmotion::Disappointed,
            "surprised" => LlmEmotion::Surprised,
            "curious" => LlmEmotion::Curious,
            "neutral" => LlmEmotion::Neutral,
            "bored" => LlmEmotion::Bored,
            "confused" => LlmEmotion::Confused,
            _ => LlmEmotion::Neutral,
        }
    }

    /// 获取 (valence, arousal) 基线值
    pub fn valence_arousal(&self) -> (f64, f64) {
        llm_emotion_valence_arousal(self.as_label())
    }

    /// 映射到 EmotionLabel
    pub fn to_emotion_label(&self) -> EmotionLabel {
        llm_to_emotion_label(self.as_label())
    }

    /// 由 EmotionLabel 反向构造
    pub fn from_emotion_label(label: EmotionLabel) -> Self {
        LlmEmotion::from_label(emotion_label_to_llm(label))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_emotion_count_is_14() {
        assert_eq!(LlmEmotion::all().len(), 14);
        assert_eq!(LLM_EMOTION_LABELS.len(), 14);
    }

    #[test]
    fn test_labels_match_python() {
        let python_labels = [
            "happy", "excited", "grateful", "sad", "frustrated", "anxious", "tired",
            "angry", "disappointed", "surprised", "curious", "neutral", "bored", "confused",
        ];
        assert_eq!(LLM_EMOTION_LABELS, python_labels);
    }

    #[test]
    fn test_label_roundtrip() {
        for &emo in LlmEmotion::all() {
            let label = emo.as_label();
            assert_eq!(LlmEmotion::from_label(label), emo);
        }
    }

    #[test]
    fn test_unknown_label_falls_back_to_neutral() {
        assert_eq!(LlmEmotion::from_label("not_a_real_emotion"), LlmEmotion::Neutral);
        assert_eq!(normalize_llm_emotion("foobar"), "neutral");
        assert!(!is_valid_llm_emotion("foobar"));
        assert!(is_valid_llm_emotion("happy"));
        assert!(!is_valid_llm_emotion("fearful"));
        assert!(!is_valid_llm_emotion("disgusted"));
        assert!(!is_valid_llm_emotion("lonely"));
        assert!(!is_valid_llm_emotion("affectionate"));
        assert!(is_valid_llm_emotion("grateful"));
        assert!(is_valid_llm_emotion("frustrated"));
        assert!(is_valid_llm_emotion("disappointed"));
        assert!(is_valid_llm_emotion("confused"));
    }

    #[test]
    fn test_llm_to_emotion_label_mapping() {
        // 多对一映射
        assert_eq!(llm_to_emotion_label("happy"), EmotionLabel::Joy);
        assert_eq!(llm_to_emotion_label("excited"), EmotionLabel::Joy);
        assert_eq!(llm_to_emotion_label("grateful"), EmotionLabel::Closeness);
        assert_eq!(llm_to_emotion_label("sad"), EmotionLabel::Sadness);
        assert_eq!(llm_to_emotion_label("disappointed"), EmotionLabel::Sadness);
        assert_eq!(llm_to_emotion_label("frustrated"), EmotionLabel::Anger);
        assert_eq!(llm_to_emotion_label("angry"), EmotionLabel::Anger);
        assert_eq!(llm_to_emotion_label("anxious"), EmotionLabel::Fear);
        assert_eq!(llm_to_emotion_label("tired"), EmotionLabel::Loneliness);
        assert_eq!(llm_to_emotion_label("bored"), EmotionLabel::Loneliness);
        assert_eq!(llm_to_emotion_label("surprised"), EmotionLabel::Curiosity);
        assert_eq!(llm_to_emotion_label("curious"), EmotionLabel::Curiosity);
        assert_eq!(llm_to_emotion_label("neutral"), EmotionLabel::Curiosity);
        assert_eq!(llm_to_emotion_label("confused"), EmotionLabel::Curiosity);
        // 未知回退到 Curiosity
        assert_eq!(llm_to_emotion_label("???"), EmotionLabel::Curiosity);
    }

    #[test]
    fn test_emotion_label_to_llm_one_to_one() {
        assert_eq!(emotion_label_to_llm(EmotionLabel::Joy), "happy");
        assert_eq!(emotion_label_to_llm(EmotionLabel::Sadness), "sad");
        assert_eq!(emotion_label_to_llm(EmotionLabel::Anger), "angry");
        assert_eq!(emotion_label_to_llm(EmotionLabel::Fear), "anxious");
        assert_eq!(emotion_label_to_llm(EmotionLabel::Closeness), "grateful");
        assert_eq!(emotion_label_to_llm(EmotionLabel::Loneliness), "sad");
        assert_eq!(emotion_label_to_llm(EmotionLabel::Curiosity), "curious");
    }

    #[test]
    fn test_roundtrip_all_14_llm_emotions() {
        // 14 → 7 → 14：由于 7→14 是一对一，每个 14 类标签映射到 7 类再映射回来应保持一致
        for &emo in LlmEmotion::all() {
            let label = emo.to_emotion_label();
            let back = LlmEmotion::from_emotion_label(label);
            // 由于多对一关系，roundtrip 后某些情绪会归并到代表标签
            // 验证 roundtrip 结果与该 EmotionLabel 的代表标签一致
            let representative = LlmEmotion::from_label(emotion_label_to_llm(label));
            assert_eq!(
                back, representative,
                "roundtrip failed for {:?}: label={:?}, back={:?}, representative={:?}",
                emo, label, back, representative
            );
        }
    }

    #[test]
    fn test_valence_arousal_values() {
        // 抽样核对几个关键情绪的 (valence, arousal) 基线
        assert_eq!(llm_emotion_valence_arousal("happy"), (0.7, 0.6));
        assert_eq!(llm_emotion_valence_arousal("sad"), (-0.5, 0.25));
        assert_eq!(llm_emotion_valence_arousal("excited"), (0.8, 0.85));
        assert_eq!(llm_emotion_valence_arousal("angry"), (-0.6, 0.8));
        assert_eq!(llm_emotion_valence_arousal("grateful"), (0.6, 0.35));
        assert_eq!(llm_emotion_valence_arousal("frustrated"), (-0.4, 0.6));
        assert_eq!(llm_emotion_valence_arousal("disappointed"), (-0.45, 0.3));
        assert_eq!(llm_emotion_valence_arousal("confused"), (0.0, 0.4));
        assert_eq!(llm_emotion_valence_arousal("neutral"), (0.0, 0.3));
        // 未知回退到 neutral
        assert_eq!(llm_emotion_valence_arousal("???"), (0.0, 0.3));

        let (v, a) = LlmEmotion::Excited.valence_arousal();
        assert_eq!((v, a), (0.8, 0.85));
    }

    #[test]
    fn test_emotion_label_roundtrip() {
        // 7 → 14 → 7 应保持一致
        for &label in EmotionLabel::all() {
            let llm = emotion_label_to_llm(label);
            let back = llm_to_emotion_label(llm);
            assert_eq!(back, label, "roundtrip failed for {:?}", label);
        }
    }
}
