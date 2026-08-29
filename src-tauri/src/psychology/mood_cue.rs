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
//!
//! 规则集按「真实心理表现的可观测优先级」分层：
//! 生理底线 > 强主导情绪 > 中度疲劳 > 效价-唤醒基调 > 关系背景 > 兜底。
//! 详见 [`default_rules`] 的文档。

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
    ///
    /// 按强度分档：强档用夸张表情，弱档用温和版本，
    /// 让同一情绪在不同强度下有层次感（开心有「欣喜若狂」和「微微一笑」之分）。
    pub fn map_by_emotion(emotion: EmotionLabel, intensity: f64) -> Live2DCue {
        let weight = (intensity as f32).clamp(0.0, 1.0);
        let (expression, idle_mouth) = match emotion {
            // 快乐：强 → 星星眼放光；弱 → 温和的爱意眼（微微一笑）
            EmotionLabel::Joy if intensity > 0.6 => ("star_eyes", 0.25),
            EmotionLabel::Joy => ("love_eyes", 0.15),
            // 悲伤：强 → 泪眼汪汪；弱 → 哭丧脸
            EmotionLabel::Sadness if intensity > 0.65 => ("tears", 0.0),
            EmotionLabel::Sadness => ("cry", 0.0),
            // 愤怒：强 → 生气符号；弱 → 嘟嘴（不高兴但没爆发）
            EmotionLabel::Anger if intensity > 0.65 => ("angry_symbol", 0.15),
            EmotionLabel::Anger => ("pout", 0.05),
            // 恐惧：强 → 晕头转向；弱 → 冒冷汗（不安）
            EmotionLabel::Fear if intensity > 0.65 => ("dizzy", 0.1),
            EmotionLabel::Fear => ("sweat", 0.05),
            // 亲近：强 → 爱心眼；弱 → 害羞脸红
            EmotionLabel::Closeness if intensity > 0.6 => ("love_eyes", 0.2),
            EmotionLabel::Closeness => ("shy", 0.15),
            // 孤独：强 → 发呆放空；弱 → 眼含泪光（怅然）
            EmotionLabel::Loneliness if intensity > 0.6 => ("blank_eyes", 0.0),
            EmotionLabel::Loneliness => ("tears", 0.0),
            // 好奇：强 → 大问号脸；弱 → 普通疑惑
            EmotionLabel::Curiosity if intensity > 0.6 => ("confused_intense", 0.15),
            EmotionLabel::Curiosity => ("confused", 0.1),
        };
        Live2DCue {
            expression: expression.into(),
            motion: "idle".into(),
            idle_mouth,
            weight,
        }
    }
}

impl Default for MoodCueMapper {
    fn default() -> Self {
        Self::new()
    }
}

/// 默认规则集（按优先级分层排序）
///
/// 规则模拟真实心理表现的可观测优先级，自上而下分五层：
///
/// ```text
/// 第一层 生理底线    睡着/极度疲惫/身心俱疲/压力临界
///        （慢变量，压过一切情绪：太困了笑不动）
/// 第二层 强主导情绪  intensity > 0.55 的 7 类情绪各分强/弱两档
///        （情绪足够强时藏不住：即使有点累也看得出开心）
/// 第三层 中度疲劳    无强情绪时，倦意才浮上表面
/// 第四层 效价-唤醒   中等情绪强度下的背景基调
///        （valence × arousal 平面细分象限）
/// 第五层 关系背景    高亲密度的心底暖意 / 低亲密度的疏离
/// 兜底   平静待机
/// ```
///
/// 同层内的组间条件互斥（primary_emotion 唯一），组内强档先判。
fn default_rules() -> Vec<CueRule> {
    vec![
        // ═════════ 第一层：生理底线（压过一切情绪）═════════

        // 睡着：疲劳 > 90 → 戴眼罩沉睡，嘴完全闭合
        CueRule {
            name: "sleeping",
            condition: Box::new(|m| m.fatigue > 90.0),
            cue: Live2DCue {
                expression: "blindfold".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.95,
            },
        },
        // 极度疲惫：疲劳 > 80 → 晕乎乎睁不开眼
        CueRule {
            name: "exhausted",
            condition: Box::new(|m| m.fatigue > 80.0),
            cue: Live2DCue {
                expression: "dizzy".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.9,
            },
        },
        // 身心俱疲（burnout）：又累又有压力 → 沉着脸硬撑
        CueRule {
            name: "burnout",
            condition: Box::new(|m| m.fatigue > 60.0 && m.stress > 50.0),
            cue: Live2DCue {
                expression: "dark_face".into(),
                motion: "idle".into(),
                idle_mouth: 0.05,
                weight: 0.85,
            },
        },
        // 压力临界：压力 > 80 → 手足无措冒冷汗
        CueRule {
            name: "stressed_critical",
            condition: Box::new(|m| m.stress > 80.0),
            cue: Live2DCue {
                expression: "sweat".into(),
                motion: "idle".into(),
                idle_mouth: 0.1,
                weight: 0.85,
            },
        },
        // 高压力：压力 > 70 → 压着火，一点就着
        CueRule {
            name: "stressed_irritable",
            condition: Box::new(|m| m.stress > 70.0),
            cue: Live2DCue {
                expression: "angry".into(),
                motion: "idle".into(),
                idle_mouth: 0.1,
                weight: 0.8,
            },
        },

        // ═════════ 第二层：高强度主导情绪（intensity > 0.55，压过中度疲劳）═════════

        // —— 快乐系 ——
        // 欣喜若狂：Joy 极强 + 高唤醒 → 星光环绕
        CueRule {
            name: "joy_ecstatic",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Joy
                    && m.primary_intensity > 0.75
                    && m.arousal > 0.55
            }),
            cue: Live2DCue {
                expression: "star_aura".into(),
                motion: "idle".into(),
                idle_mouth: 0.35,
                weight: 0.8,
            },
        },
        // 眉开眼笑：Joy 强 → 星星眼
        CueRule {
            name: "joy_delighted",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Joy && m.primary_intensity > 0.55
            }),
            cue: Live2DCue {
                expression: "star_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.25,
                weight: 0.7,
            },
        },

        // —— 愤怒系 ——
        // 怒气冲冲：Anger 极强 → 生气符号
        CueRule {
            name: "anger_rage",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Anger && m.primary_intensity > 0.7
            }),
            cue: Live2DCue {
                expression: "angry_symbol".into(),
                motion: "idle".into(),
                idle_mouth: 0.15,
                weight: 0.8,
            },
        },
        // 生闷气：Anger 中强但唤醒不高 → 鼓脸
        CueRule {
            name: "anger_sulking",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Anger && m.primary_intensity > 0.55
            }),
            cue: Live2DCue {
                expression: "puff_cheek".into(),
                motion: "idle".into(),
                idle_mouth: 0.05,
                weight: 0.7,
            },
        },

        // —— 悲伤系 ——
        // 泪如雨下：Sadness 极强 → 泪眼
        CueRule {
            name: "sadness_grieving",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Sadness && m.primary_intensity > 0.7
            }),
            cue: Live2DCue {
                expression: "tears".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.75,
            },
        },
        // 闷闷不乐：Sadness 中强 → 哭丧脸
        CueRule {
            name: "sadness_down",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Sadness && m.primary_intensity > 0.55
            }),
            cue: Live2DCue {
                expression: "cry".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.65,
            },
        },

        // —— 恐惧系 ——
        // 惊慌失措：Fear 极强 + 高唤醒 → 晕头转向
        CueRule {
            name: "fear_panicking",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Fear
                    && m.primary_intensity > 0.7
                    && m.arousal > 0.5
            }),
            cue: Live2DCue {
                expression: "dizzy".into(),
                motion: "idle".into(),
                idle_mouth: 0.1,
                weight: 0.75,
            },
        },
        // 忐忑不安：Fear 中强 → 冒冷汗
        CueRule {
            name: "fear_anxious",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Fear && m.primary_intensity > 0.55
            }),
            cue: Live2DCue {
                expression: "sweat".into(),
                motion: "idle".into(),
                idle_mouth: 0.05,
                weight: 0.65,
            },
        },

        // —— 亲近系 ——
        // 满心爱意：Closeness 强 → 爱心眼
        CueRule {
            name: "closeness_lovestruck",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Closeness && m.primary_intensity > 0.65
            }),
            cue: Live2DCue {
                expression: "love_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.2,
                weight: 0.7,
            },
        },
        // 温柔害羞：Closeness 中强 → 脸红
        CueRule {
            name: "closeness_bashful",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Closeness && m.primary_intensity > 0.55
            }),
            cue: Live2DCue {
                expression: "shy".into(),
                motion: "idle".into(),
                idle_mouth: 0.15,
                weight: 0.6,
            },
        },

        // —— 孤独系 ——
        // 失落出神：Loneliness 强 + 低唤醒 → 发呆放空
        CueRule {
            name: "loneliness_withdrawn",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Loneliness && m.primary_intensity > 0.65
            }),
            cue: Live2DCue {
                expression: "blank_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.65,
            },
        },
        // 怅然若失：Loneliness 中强 → 眼含泪光
        CueRule {
            name: "loneliness_wistful",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Loneliness && m.primary_intensity > 0.55
            }),
            cue: Live2DCue {
                expression: "tears".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.55,
            },
        },

        // —— 好奇系 ——
        // 满腹狐疑：Curiosity 强 + 高唤醒 → 大问号脸
        CueRule {
            name: "curiosity_intrigued",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Curiosity
                    && m.primary_intensity > 0.6
                    && m.arousal > 0.5
            }),
            cue: Live2DCue {
                expression: "confused_intense".into(),
                motion: "idle".into(),
                idle_mouth: 0.15,
                weight: 0.6,
            },
        },

        // ═════════ 第三层：中度疲劳（无强情绪时，倦意才浮上表面）═════════

        // 昏昏欲睡：疲劳 > 55 且唤醒低 → 想睡
        CueRule {
            name: "drowsy",
            condition: Box::new(|m| m.fatigue > 55.0 && m.arousal < 0.45),
            cue: Live2DCue {
                expression: "blindfold".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.5,
            },
        },

        // ═════════ 第四层：效价-唤醒空间（中等情绪强度下的背景基调）═════════

        // 兴奋：高唤醒 + 正效价 → 星光环绕
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
        // 满怀期待：中高唤醒 + 正效价 → 星星眼
        CueRule {
            name: "anticipating",
            condition: Box::new(|m| m.arousal > 0.5 && m.valence > 0.3),
            cue: Live2DCue {
                expression: "star_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.2,
                weight: 0.55,
            },
        },
        // 安心惬意：低唤醒 + 正效价 + 关系不疏远 → 爱意眼（安心陪伴感）
        CueRule {
            name: "cozy_companion",
            condition: Box::new(|m| {
                m.arousal < 0.35 && m.valence > 0.2 && m.relationship_score > 40.0
            }),
            cue: Live2DCue {
                expression: "love_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.15,
                weight: 0.6,
            },
        },
        // 温馨：低唤醒 + 正效价 → 害羞
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
        // 焦虑不安：负效价 + 高唤醒 → 冒冷汗
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
        // 不高兴嘟嘴：轻度负效价 + 中唤醒 → 嘟嘴
        CueRule {
            name: "miffed",
            condition: Box::new(|m| m.valence < -0.15 && m.arousal > 0.35),
            cue: Live2DCue {
                expression: "pout".into(),
                motion: "idle".into(),
                idle_mouth: 0.05,
                weight: 0.5,
            },
        },
        // 情绪低落：负效价 → 哭丧脸
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
        // 委靡无力：轻度负效价 + 低唤醒 → 无语脸
        CueRule {
            name: "listless",
            condition: Box::new(|m| m.valence < -0.1 && m.arousal < 0.3),
            cue: Live2DCue {
                expression: "speechless".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.45,
            },
        },
        // 好奇观察：中性效价 + 中唤醒 → 疑惑打量
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

        // ═════════ 第五层：关系背景调制 ═════════

        // 老朋友默契：亲密度高且心情不差 → 眼底带笑
        CueRule {
            name: "warm_companion",
            condition: Box::new(|m| m.relationship_score > 75.0 && m.valence > 0.0),
            cue: Live2DCue {
                expression: "love_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.15,
                weight: 0.5,
            },
        },
        // 疏离旁观：亲密度极低 → 发呆保持距离
        CueRule {
            name: "distant",
            condition: Box::new(|m| m.relationship_score < 15.0),
            cue: Live2DCue {
                expression: "blank_eyes".into(),
                motion: "idle".into(),
                idle_mouth: 0.0,
                weight: 0.45,
            },
        },

        // ═════════ 兜底 ═════════

        // 平静待机：什么都不明显 → 普通 idle
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

    fn make_mood_with_emotion(
        valence: f64,
        arousal: f64,
        fatigue: f64,
        stress: f64,
        primary: EmotionLabel,
        intensity: f64,
    ) -> MoodSnapshot {
        MoodSnapshot {
            valence,
            arousal,
            primary_emotion: primary,
            secondary_emotion: EmotionLabel::Curiosity,
            primary_intensity: intensity,
            fatigue,
            stress,
            relationship_score: 50.0,
        }
    }

    // ═══ 第一层：生理底线 ═══

    #[test]
    fn test_sleeping_rule() {
        let mood = make_mood(0.0, 0.3, 95.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.expression, "blindfold");
        assert!((cue.weight - 0.95).abs() < 0.01);
    }

    #[test]
    fn test_exhausted_rule() {
        let mood = make_mood(0.0, 0.3, 85.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.expression, "dizzy");
    }

    #[test]
    fn test_burnout_rule() {
        // 又累又有压力，但都未到单独阈值 → 黑脸硬撑
        let mood = make_mood(-0.2, 0.4, 65.0, 60.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.expression, "dark_face");
    }

    #[test]
    fn test_stressed_rules() {
        // 压力临界 → 冷汗
        let mood = make_mood(-0.2, 0.6, 20.0, 85.0);
        assert_eq!(mood_to_cue(&mood).expression, "sweat");
        // 高压力 → 压着火
        let mood = make_mood(-0.2, 0.6, 20.0, 75.0);
        assert_eq!(mood_to_cue(&mood).expression, "angry");
    }

    // ═══ 第二层：强主导情绪 ═══

    #[test]
    fn test_joy_strong_rules() {
        // 欣喜若狂：Joy 极强 + 高唤醒
        let mood = make_mood_with_emotion(0.8, 0.7, 30.0, 10.0, EmotionLabel::Joy, 0.85);
        assert_eq!(mood_to_cue(&mood).expression, "star_aura");
        // 眉开眼笑：Joy 强但唤醒不高
        let mood = make_mood_with_emotion(0.6, 0.4, 30.0, 10.0, EmotionLabel::Joy, 0.65);
        assert_eq!(mood_to_cue(&mood).expression, "star_eyes");
    }

    #[test]
    fn test_anger_rules() {
        // 怒气冲冲
        let mood =
            make_mood_with_emotion(-0.7, 0.6, 20.0, 10.0, EmotionLabel::Anger, 0.8);
        assert_eq!(mood_to_cue(&mood).expression, "angry_symbol");
        // 生闷气：中等强度的火 + 低唤醒 → 鼓脸
        let mood =
            make_mood_with_emotion(-0.5, 0.3, 20.0, 10.0, EmotionLabel::Anger, 0.6);
        assert_eq!(mood_to_cue(&mood).expression, "puff_cheek");
    }

    #[test]
    fn test_sadness_rules() {
        // 泪如雨下
        let mood =
            make_mood_with_emotion(-0.6, 0.3, 20.0, 10.0, EmotionLabel::Sadness, 0.8);
        assert_eq!(mood_to_cue(&mood).expression, "tears");
        // 闷闷不乐
        let mood =
            make_mood_with_emotion(-0.4, 0.3, 20.0, 10.0, EmotionLabel::Sadness, 0.6);
        assert_eq!(mood_to_cue(&mood).expression, "cry");
    }

    #[test]
    fn test_fear_rules() {
        // 惊慌失措
        let mood =
            make_mood_with_emotion(-0.6, 0.8, 20.0, 10.0, EmotionLabel::Fear, 0.8);
        assert_eq!(mood_to_cue(&mood).expression, "dizzy");
        // 忐忑不安
        let mood =
            make_mood_with_emotion(-0.4, 0.4, 20.0, 10.0, EmotionLabel::Fear, 0.6);
        assert_eq!(mood_to_cue(&mood).expression, "sweat");
    }

    #[test]
    fn test_closeness_rules() {
        // 满心爱意
        let mood =
            make_mood_with_emotion(0.6, 0.4, 20.0, 10.0, EmotionLabel::Closeness, 0.7);
        assert_eq!(mood_to_cue(&mood).expression, "love_eyes");
        // 温柔害羞
        let mood =
            make_mood_with_emotion(0.4, 0.4, 20.0, 10.0, EmotionLabel::Closeness, 0.6);
        assert_eq!(mood_to_cue(&mood).expression, "shy");
    }

    #[test]
    fn test_loneliness_rules() {
        // 失落出神
        let mood =
            make_mood_with_emotion(-0.3, 0.25, 20.0, 10.0, EmotionLabel::Loneliness, 0.7);
        assert_eq!(mood_to_cue(&mood).expression, "blank_eyes");
        // 怅然若失
        let mood =
            make_mood_with_emotion(-0.2, 0.3, 20.0, 10.0, EmotionLabel::Loneliness, 0.6);
        assert_eq!(mood_to_cue(&mood).expression, "tears");
    }

    #[test]
    fn test_curiosity_strong_rule() {
        // 满腹狐疑：强好奇 + 高唤醒
        let mood = make_mood_with_emotion(0.1, 0.6, 20.0, 10.0, EmotionLabel::Curiosity, 0.7);
        assert_eq!(mood_to_cue(&mood).expression, "confused_intense");
    }

    #[test]
    fn test_strong_emotion_overrides_moderate_fatigue() {
        // 强情绪压过中度疲劳：有点累但很开心 → 还是看得出开心
        let mood = make_mood_with_emotion(0.6, 0.6, 50.0, 10.0, EmotionLabel::Joy, 0.7);
        assert_eq!(mood_to_cue(&mood).expression, "star_eyes");
    }

    // ═══ 第三层：中度疲劳 ═══

    #[test]
    fn test_drowsy_rule() {
        // 无强情绪 + 中度疲劳 + 低唤醒 → 想睡
        let mood = make_mood(0.0, 0.3, 60.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.expression, "blindfold");
        assert!((cue.weight - 0.5).abs() < 0.01);
    }

    // ═══ 第四层：效价-唤醒空间 ═══

    #[test]
    fn test_excited_rule() {
        let mood = make_mood(0.6, 0.8, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.expression, "star_aura");
        assert_eq!(cue.motion, "idle");
    }

    #[test]
    fn test_anticipating_rule() {
        // 中高唤醒 + 正效价（未到兴奋）→ 星星眼
        let mood = make_mood(0.35, 0.55, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).expression, "star_eyes");
    }

    #[test]
    fn test_cozy_companion_rule() {
        // 低唤醒 + 正效价 + 关系分 > 40 → 爱意眼（安心陪伴）
        let mut mood = make_mood(0.3, 0.2, 20.0, 10.0);
        mood.relationship_score = 60.0;
        assert_eq!(mood_to_cue(&mood).expression, "love_eyes");
        // 关系疏远时 → 普通害羞
        mood.relationship_score = 30.0;
        assert_eq!(mood_to_cue(&mood).expression, "shy");
    }

    #[test]
    fn test_anxious_rule() {
        let mood = make_mood(-0.4, 0.6, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).expression, "sweat");
    }

    #[test]
    fn test_miffed_rule() {
        // 轻度负效价 + 中唤醒 → 嘟嘴（不到哭的程度）
        let mood = make_mood(-0.2, 0.4, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).expression, "pout");
    }

    #[test]
    fn test_listless_rule() {
        // 轻度负效价 + 低唤醒 → 无语脸
        let mood = make_mood(-0.2, 0.2, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).expression, "speechless");
    }

    #[test]
    fn test_sad_fallback_rule() {
        let mood = make_mood(-0.5, 0.2, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).expression, "cry");
    }

    #[test]
    fn test_neutral_curious_rule() {
        let mood = make_mood(0.1, 0.4, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).expression, "confused");
    }

    // ═══ 第五层：关系背景 ═══

    #[test]
    fn test_warm_companion_rule() {
        // 亲密度高 + 心情平和 → 眼底带笑
        let mut mood = make_mood(0.1, 0.2, 20.0, 10.0);
        mood.relationship_score = 80.0;
        assert_eq!(mood_to_cue(&mood).expression, "love_eyes");
    }

    #[test]
    fn test_distant_rule() {
        // 亲密度极低 → 发呆疏离
        let mut mood = make_mood(0.0, 0.2, 20.0, 10.0);
        mood.relationship_score = 10.0;
        assert_eq!(mood_to_cue(&mood).expression, "blank_eyes");
    }

    // ═══ 兜底 ═══

    #[test]
    fn test_calm_fallback() {
        let mood = make_mood(0.1, 0.2, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.motion, "idle");
        assert!(cue.expression.is_empty());
    }

    // ═══ 情绪捷径：强度分档 ═══

    #[test]
    fn test_emotion_shortcut() {
        let cue = emotion_to_cue(EmotionLabel::Joy, 0.8);
        assert_eq!(cue.expression, "star_eyes");
        assert!((cue.weight - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_emotion_shortcut_intensity_tiers() {
        // 快乐：强 → 星星眼；弱 → 温和爱意眼
        assert_eq!(emotion_to_cue(EmotionLabel::Joy, 0.8).expression, "star_eyes");
        assert_eq!(emotion_to_cue(EmotionLabel::Joy, 0.4).expression, "love_eyes");
        // 悲伤：强 → 泪眼；弱 → 哭丧脸
        assert_eq!(emotion_to_cue(EmotionLabel::Sadness, 0.8).expression, "tears");
        assert_eq!(emotion_to_cue(EmotionLabel::Sadness, 0.4).expression, "cry");
        // 愤怒：强 → 生气符号；弱 → 嘟嘴
        assert_eq!(
            emotion_to_cue(EmotionLabel::Anger, 0.8).expression,
            "angry_symbol"
        );
        assert_eq!(emotion_to_cue(EmotionLabel::Anger, 0.4).expression, "pout");
        // 孤独：强 → 发呆；弱 → 泪光
        assert_eq!(
            emotion_to_cue(EmotionLabel::Loneliness, 0.8).expression,
            "blank_eyes"
        );
        assert_eq!(
            emotion_to_cue(EmotionLabel::Loneliness, 0.4).expression,
            "tears"
        );
    }
}
