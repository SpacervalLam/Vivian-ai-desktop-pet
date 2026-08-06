//! 轻量级 Mood → Cue 快速通道
//!
//! 在完整心理管道（LLM appraisal → emotion → behavior drive）之外，
//! 提供一条纯规则驱动的快速路径：从当前 MoodSnapshot 直接映射到
//! Live2D 表情/动作提示，用于：
//! - 主动 tick 期间的背景微动画
//! - 用户输入到达但 LLM 尚未响应时的即时反馈
//! - 低频心跳动画（避免静态站立）
//!
//! 设计原则：
//! - 纯函数，无状态，无 LLM 调用，无 IO
//! - 映射表可配置（运行时可替换）
//! - 优先级低于 LLM 产出的 InteractionFeedback（仅作 fallback）

use serde::{Deserialize, Serialize};

use super::emotion::EmotionLabel;
use super::mood::MoodSnapshot;

/// Live2D 提示：表情名 + 动作名（均可为空，空表示保持当前状态）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Live2DCue {
    /// 表情名（如 "star_eyes" / "cry" / "confused"）
    pub expression: String,
    /// 动作名（如 "idle"）
    pub motion: String,
    /// 嘴形基础开合（0.0-1.0，仅 idle 时参考）
    pub idle_mouth: f32,
    /// 权重：多个 cue 来源冲突时取权重最高者
    pub weight: f32,
}

impl Live2DCue {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.expression.is_empty() && self.motion.is_empty()
    }
}

/// Mood → Cue 映射规则
///
/// 规则按优先级评估，返回第一个匹配的 cue。
/// 匹配条件基于 valence / arousal / 主导情绪 / 疲劳度。
pub struct MoodCueMapper {
    rules: Vec<CueRule>,
}

struct CueRule {
    /// 规则名（用于调试日志标识）
    name: &'static str,
    condition: Box<dyn Fn(&MoodSnapshot) -> bool + Send + Sync>,
    cue: Live2DCue,
}

impl MoodCueMapper {
    pub fn new() -> Self {
        Self {
            rules: default_rules(),
        }
    }

    /// 从 MoodSnapshot 映射到 Live2DCue
    ///
    /// 按规则顺序评估，返回第一个匹配的 cue。
    /// 无匹配时返回空 cue（保持当前状态）。
    pub fn map(&self, mood: &MoodSnapshot) -> Live2DCue {
        for rule in &self.rules {
            if (rule.condition)(mood) {
                tracing::debug!(
                    rule = rule.name,
                    "[MoodCue] 命中规则"
                );
                return rule.cue.clone();
            }
        }
        Live2DCue::none()
    }

    /// 根据主导情绪标签直接映射（更快的捷径）
    pub fn map_by_emotion(emotion: EmotionLabel, intensity: f64) -> Live2DCue {
        let weight = (intensity as f32).clamp(0.0, 1.0);
        match emotion {
            EmotionLabel::Joy => Live2DCue {
                expression: "star_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.2,
                weight,
            },
            EmotionLabel::Sadness => Live2DCue {
                expression: "cry".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight,
            },
            EmotionLabel::Anger => Live2DCue {
                expression: "angry".into(),
                motion: "idle".into(),
                idle_mouth: 0.1,
                weight,
            },
            EmotionLabel::Fear => Live2DCue {
                expression: "sweat".into(),
                motion: "idle".into(),
                idle_mouth: 0.05,
                weight,
            },
            EmotionLabel::Closeness => Live2DCue {
                expression: "shy".into(),
                motion: "idle".into(),
                idle_mouth: 0.15,
                weight,
            },
            EmotionLabel::Loneliness => Live2DCue {
                expression: "blank_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight,
            },
            EmotionLabel::Curiosity => Live2DCue {
                expression: "confused".into(),
                motion: "idle".into(),
                idle_mouth: 0.1,
                weight,
            },
        }
    }
}

impl Default for MoodCueMapper {
    fn default() -> Self {
        Self::new()
    }
}

/// 默认规则集（按优先级排序）
fn default_rules() -> Vec<CueRule> {
    vec![
        // 1. 极度疲劳 → 打瞌睡
        CueRule {
            name: "exhausted",
            condition: Box::new(|m| m.fatigue > 80.0),
            cue: Live2DCue {
                expression: "blank_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.9,
            },
        },
        // 2. 高压力 → 烦躁
        CueRule {
            name: "stressed",
            condition: Box::new(|m| m.stress > 70.0),
            cue: Live2DCue {
                expression: "angry".into(),
                motion: "idle".into(),
                idle_mouth: 0.1,
                weight: 0.8,
            },
        },
        // 3. 高唤醒 + 正效价 → 兴奋
        CueRule {
            name: "excited",
            condition: Box::new(|m| m.arousal > 0.7 && m.valence > 0.4),
            cue: Live2DCue {
                expression: "star_aura".into(),
                motion: "idle".into(),
                idle_mouth: 0.3,
                weight: 0.7,
            },
        },
        // 4. 低唤醒 + 正效价 → 温馨
        CueRule {
            name: "cozy",
            condition: Box::new(|m| m.arousal < 0.35 && m.valence > 0.2),
            cue: Live2DCue {
                expression: "shy".into(),
                motion: "idle".into(),
                idle_mouth: 0.15,
                weight: 0.6,
            },
        },
        // 5. 负效价 + 高唤醒 → 焦虑
        CueRule {
            name: "anxious",
            condition: Box::new(|m| m.valence < -0.3 && m.arousal > 0.5),
            cue: Live2DCue {
                expression: "sweat".into(),
                motion: "idle".into(),
                idle_mouth: 0.05,
                weight: 0.7,
            },
        },
        // 6. 负效价 + 低唤醒 → 悲伤
        CueRule {
            name: "sad",
            condition: Box::new(|m| m.valence < -0.3),
            cue: Live2DCue {
                expression: "cry".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.6,
            },
        },
        // 7. 中性 + 中等唤醒 → 好奇 idle
        CueRule {
            name: "neutral_curious",
            condition: Box::new(|m| m.valence.abs() < 0.3 && m.arousal > 0.3),
            cue: Live2DCue {
                expression: "confused".into(),
                motion: "idle".into(),
                idle_mouth: 0.1,
                weight: 0.4,
            },
        },
        // 8. 兜底：平静 idle
        CueRule {
            name: "calm_idle",
            condition: Box::new(|_| true),
            cue: Live2DCue {
                expression: String::new(),
                motion: "idle".into(),
                idle_mouth: 0.1,
                weight: 0.2,
            },
        },
    ]
}

/// 全局单例 mapper（无状态，可安全共享）
static GLOBAL_MAPPER: once_cell::sync::Lazy<MoodCueMapper> =
    once_cell::sync::Lazy::new(MoodCueMapper::new);

/// 便捷接口：从 MoodSnapshot 映射 Live2DCue
pub fn mood_to_cue(mood: &MoodSnapshot) -> Live2DCue {
    GLOBAL_MAPPER.map(mood)
}

/// 便捷接口：从情绪标签快速映射（用于即时反馈）
pub fn emotion_to_cue(emotion: EmotionLabel, intensity: f64) -> Live2DCue {
    MoodCueMapper::map_by_emotion(emotion, intensity)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mood(valence: f64, arousal: f64, fatigue: f64, stress: f64) -> MoodSnapshot {
        MoodSnapshot {
            valence,
            arousal,
            primary_emotion: EmotionLabel::Curiosity,
            secondary_emotion: EmotionLabel::Closeness,
            primary_intensity: 0.5,
            fatigue,
            stress,
            relationship_score: 50.0,
        }
    }

    #[test]
    fn test_exhausted_rule() {
        let mood = make_mood(0.0, 0.3, 85.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.expression, "blank_eyes");
    }

    #[test]
    fn test_excited_rule() {
        let mood = make_mood(0.6, 0.8, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.expression, "star_aura");
        assert_eq!(cue.motion, "idle");
    }

    #[test]
    fn test_calm_fallback() {
        let mood = make_mood(0.1, 0.2, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.motion, "idle");
    }

    #[test]
    fn test_emotion_shortcut() {
        let cue = emotion_to_cue(EmotionLabel::Joy, 0.8);
        assert_eq!(cue.expression, "star_eyes");
        assert!((cue.weight - 0.8).abs() < 0.01);
    }
}
