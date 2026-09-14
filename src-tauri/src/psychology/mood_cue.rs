//! 轻量级 Mood → Cue 快速通道
//!
//! 在完整心理管道（LLM appraisal → emotion → behavior drive）之外，
//! 提供一条纯规则驱动的快速路径：从当前 MoodSnapshot 直接映射到
//! 角色表情/动作提示，用于：
//! - 心情变化时的即时反馈
//! - 主动 tick 期间的背景微动画
//! - 低频心跳动画（避免静态站立）
//!
//! # 输出为什么是 tone + accent 两个字段
//!
//! Q 版渲染器是单通道互斥的：任意时刻只有一个动作在播，一次性帧序列播完会
//! 回落到"基准姿态"。所以"情绪"被拆成两层：
//!
//! - `tone`：可持续的图集格位（`idle` / `dizzy`），是长期挂着的底色。
//!   `accent` 播完后回落到它，而不是回到 idle。
//! - `accent`：一次性帧序列（`happy` / `angry` / `think` / `smug` / `blink`），
//!   闪一下就回落到 `tone`。
//!
//! 于是"持续 vs 爆发"成了新的语义维度：疲惫/悲伤是**持续挂着** dizzy，
//! 开心/生气是**闪一下**，两者在屏幕上真的不一样。
//!
//! 注意一个词表约束：`happy` 在图集里虽有格位，但词表规定同名帧序列优先，
//! 解析 `happy` 拿到的是一次性动画而非可持续格位。因此**正效价没有可持续的
//! 开心脸**，基调只能取 `idle` / `dizzy`，正向情绪一律靠 `accent` 表达。
//!
//! 设计原则：
//! - 纯函数，无状态，无 LLM 调用，无 IO
//! - 优先级低于 LLM 产出的 InteractionFeedback（仅作 fallback）
//!
//! 规则集按「真实心理表现的可观测优先级」分层：
//! 生理底线 > 强主导情绪 > 中度疲劳 > 效价-唤醒基调 > 关系背景 > 兜底。
//! 详见 [`default_rules`] 的文档。

use serde::{Deserialize, Serialize};

use super::emotion::EmotionLabel;
use super::mood::MoodSnapshot;

/// 角色提示：心情基调 + 一次性点缀（均可为空，空表示保持当前状态）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoodCue {
    /// 心情基调：可持续的图集格位（`idle` / `dizzy`），一次性动作播完回落到它
    pub tone: String,
    /// 一次性点缀：帧序列名（`happy` / `angry` / `think` / `smug` / `blink`）
    pub accent: String,
    /// 权重：多个 cue 来源冲突时取权重最高者
    pub weight: f32,
}

impl MoodCue {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.tone.is_empty() && self.accent.is_empty()
    }

    /// 基调是否合法：只有可持续的图集格位能当基调。
    ///
    /// 正效价没有可持续的开心脸（happy 被同名帧序列占了），所以合法基调
    /// 只有 `idle` / `dizzy`。写规则时传错会被这里挡下而不是静默变成一次性动画。
    pub fn tone_is_sustainable(&self) -> bool {
        self.tone.is_empty() || self.tone == "idle" || self.tone == "dizzy"
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
    cue: MoodCue,
}

impl MoodCueMapper {
    pub fn new() -> Self {
        Self {
            rules: default_rules(),
        }
    }

    /// 从 MoodSnapshot 映射到 MoodCue
    ///
    /// 按规则顺序评估，返回第一个匹配的 cue。
    /// 无匹配时返回空 cue（保持当前状态）。
    pub fn map(&self, mood: &MoodSnapshot) -> MoodCue {
        for rule in &self.rules {
            if (rule.condition)(mood) {
                tracing::debug!(
                    rule = rule.name,
                    tone = %rule.cue.tone,
                    accent = %rule.cue.accent,
                    "[MoodCue] 命中规则"
                );
                return rule.cue.clone();
            }
        }
        MoodCue::none()
    }

    /// 根据主导情绪标签直接映射（更快的捷径）
    ///
    /// 按强度分档：强档用更外放的 accent，弱档用温和版本。
    /// 基调只跟情绪正负走——负向/疲惫挂 dizzy，其余挂 idle。
    pub fn map_by_emotion(emotion: EmotionLabel, intensity: f64) -> MoodCue {
        let weight = (intensity as f32).clamp(0.0, 1.0);
        let (tone, accent) = match emotion {
            // 快乐：强 → 开心笑；弱 → 得意微笑（正效价没有可持续开心脸，只能闪）
            EmotionLabel::Joy if intensity > 0.6 => ("idle", "happy"),
            EmotionLabel::Joy => ("idle", "smug"),
            // 悲伤：无论强弱都是持续的低落脸，没有点缀
            EmotionLabel::Sadness => ("dizzy", ""),
            // 愤怒：强 → 生气；弱 → 也是生气（词表里没有更温和的怒）
            EmotionLabel::Anger => ("idle", "angry"),
            // 恐惧：持续的不安脸
            EmotionLabel::Fear => ("dizzy", ""),
            // 亲近：强 → 得意满足；弱 → 温柔眨眼
            EmotionLabel::Closeness if intensity > 0.6 => ("idle", "smug"),
            EmotionLabel::Closeness => ("idle", "blink"),
            // 孤独：持续的放空脸
            EmotionLabel::Loneliness => ("dizzy", ""),
            // 好奇：思考
            EmotionLabel::Curiosity => ("idle", "think"),
        };
        MoodCue {
            tone: tone.into(),
            accent: accent.into(),
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
/// 每条规则给出 `tone`（持续基调）与 `accent`（一次性点缀）：
/// 负向与疲惫类挂 `dizzy` 持续底色，正向/愤怒/思考类挂 `idle` 底色 + 一次性点缀。
///
/// 同层内的组间条件互斥（primary_emotion 唯一），组内强档先判。
fn default_rules() -> Vec<CueRule> {
    vec![
        // ═════════ 第一层：生理底线（压过一切情绪）═════════

        // 睡着：疲劳 > 90 → 持续挂着晕乎乎当睡脸，不点缀
        CueRule {
            name: "sleeping",
            condition: Box::new(|m| m.fatigue > 90.0),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.95,
            },
        },
        // 极度疲惫：疲劳 > 80 → 持续晕乎乎
        CueRule {
            name: "exhausted",
            condition: Box::new(|m| m.fatigue > 80.0),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.9,
            },
        },
        // 身心俱疲（burnout）：又累又有压力 → 持续挂着垮脸
        CueRule {
            name: "burnout",
            condition: Box::new(|m| m.fatigue > 60.0 && m.stress > 50.0),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.85,
            },
        },
        // 压力临界：压力 > 80 → 持续不安 + 偶尔思考状
        CueRule {
            name: "stressed_critical",
            condition: Box::new(|m| m.stress > 80.0),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: "think".into(),
                weight: 0.85,
            },
        },
        // 高压力：压力 > 70 → 压着火，一点就着
        CueRule {
            name: "stressed_irritable",
            condition: Box::new(|m| m.stress > 70.0),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "angry".into(),
                weight: 0.8,
            },
        },

        // ═════════ 第二层：高强度主导情绪（intensity > 0.55，压过中度疲劳）═════════

        // —— 快乐系 ——
        // 欣喜若狂：Joy 极强 + 高唤醒 → 得意到藏不住
        CueRule {
            name: "joy_ecstatic",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Joy
                    && m.primary_intensity > 0.75
                    && m.arousal > 0.55
            }),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "smug".into(),
                weight: 0.8,
            },
        },
        // 眉开眼笑：Joy 强 → 开心
        CueRule {
            name: "joy_delighted",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Joy && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "happy".into(),
                weight: 0.7,
            },
        },

        // —— 愤怒系 ——
        // 怒气冲冲：Anger 极强 → 生气
        CueRule {
            name: "anger_rage",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Anger && m.primary_intensity > 0.7
            }),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "angry".into(),
                weight: 0.8,
            },
        },
        // 生闷气：Anger 中强但唤醒不高 → 还是生气（词表无更温和档）
        CueRule {
            name: "anger_sulking",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Anger && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "angry".into(),
                weight: 0.7,
            },
        },

        // —— 悲伤系 ——
        // 泪如雨下：Sadness 极强 → 持续低落
        CueRule {
            name: "sadness_grieving",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Sadness && m.primary_intensity > 0.7
            }),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.75,
            },
        },
        // 闷闷不乐：Sadness 中强 → 持续低落
        CueRule {
            name: "sadness_down",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Sadness && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.65,
            },
        },

        // —— 恐惧系 ——
        // 惊慌失措：Fear 极强 + 高唤醒 → 持续晕
        CueRule {
            name: "fear_panicking",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Fear
                    && m.primary_intensity > 0.7
                    && m.arousal > 0.5
            }),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.75,
            },
        },
        // 忐忑不安：Fear 中强 → 持续不安
        CueRule {
            name: "fear_anxious",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Fear && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.65,
            },
        },

        // —— 亲近系 ——
        // 满心爱意：Closeness 强 → 得意满足
        CueRule {
            name: "closeness_lovestruck",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Closeness && m.primary_intensity > 0.65
            }),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "smug".into(),
                weight: 0.7,
            },
        },
        // 温柔害羞：Closeness 中强 → 温柔眨眼
        CueRule {
            name: "closeness_bashful",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Closeness && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "blink".into(),
                weight: 0.6,
            },
        },

        // —— 孤独系 ——
        // 失落出神：Loneliness 强 + 低唤醒 → 持续放空
        CueRule {
            name: "loneliness_withdrawn",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Loneliness && m.primary_intensity > 0.65
            }),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.65,
            },
        },
        // 怅然若失：Loneliness 中强 → 持续放空
        CueRule {
            name: "loneliness_wistful",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Loneliness && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.55,
            },
        },

        // —— 好奇系 ——
        // 满腹狐疑：Curiosity 强 + 高唤醒 → 思考
        CueRule {
            name: "curiosity_intrigued",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Curiosity
                    && m.primary_intensity > 0.6
                    && m.arousal > 0.5
            }),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "think".into(),
                weight: 0.6,
            },
        },

        // ═════════ 第三层：中度疲劳（无强情绪时，倦意才浮上表面）═════════

        // 昏昏欲睡：疲劳 > 55 且唤醒低 → 持续挂着想睡
        CueRule {
            name: "drowsy",
            condition: Box::new(|m| m.fatigue > 55.0 && m.arousal < 0.45),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.5,
            },
        },

        // ═════════ 第四层：效价-唤醒空间（中等情绪强度下的背景基调）═════════

        // 兴奋：高唤醒 + 正效价 → 得意
        CueRule {
            name: "excited",
            condition: Box::new(|m| m.arousal > 0.7 && m.valence > 0.4),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "smug".into(),
                weight: 0.7,
            },
        },
        // 满怀期待：中高唤醒 + 正效价 → 开心
        CueRule {
            name: "anticipating",
            condition: Box::new(|m| m.arousal > 0.5 && m.valence > 0.3),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "happy".into(),
                weight: 0.55,
            },
        },
        // 安心惬意：低唤醒 + 正效价 + 关系不疏远 → 温柔眨眼（安心陪伴感）
        CueRule {
            name: "cozy_companion",
            condition: Box::new(|m| {
                m.arousal < 0.35 && m.valence > 0.2 && m.relationship_score > 40.0
            }),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "blink".into(),
                weight: 0.6,
            },
        },
        // 温馨：低唤醒 + 正效价 → 温柔眨眼
        CueRule {
            name: "cozy",
            condition: Box::new(|m| m.arousal < 0.35 && m.valence > 0.2),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "blink".into(),
                weight: 0.6,
            },
        },
        // 焦虑不安：负效价 + 高唤醒 → 持续不安 + 思考状
        CueRule {
            name: "anxious",
            condition: Box::new(|m| m.valence < -0.3 && m.arousal > 0.5),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: "think".into(),
                weight: 0.7,
            },
        },
        // 不高兴：轻度负效价 + 中唤醒 → 生气（不到持续低落的程度）
        CueRule {
            name: "miffed",
            condition: Box::new(|m| m.valence < -0.15 && m.arousal > 0.35),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "angry".into(),
                weight: 0.5,
            },
        },
        // 情绪低落：负效价 → 持续低落
        CueRule {
            name: "sad",
            condition: Box::new(|m| m.valence < -0.3),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.6,
            },
        },
        // 委靡无力：轻度负效价 + 低唤醒 → 持续无力
        CueRule {
            name: "listless",
            condition: Box::new(|m| m.valence < -0.1 && m.arousal < 0.3),
            cue: MoodCue {
                tone: "dizzy".into(),
                accent: String::new(),
                weight: 0.45,
            },
        },
        // 好奇观察：中性效价 + 中唤醒 → 思考打量
        CueRule {
            name: "neutral_curious",
            condition: Box::new(|m| m.valence.abs() < 0.3 && m.arousal > 0.3),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "think".into(),
                weight: 0.4,
            },
        },

        // ═════════ 第五层：关系背景调制 ═════════

        // 老朋友默契：亲密度高且心情不差 → 得意满足
        CueRule {
            name: "warm_companion",
            condition: Box::new(|m| m.relationship_score > 75.0 && m.valence > 0.0),
            cue: MoodCue {
                tone: "idle".into(),
                accent: "smug".into(),
                weight: 0.5,
            },
        },
        // 疏离旁观：亲密度极低 → 持续平淡保持距离
        CueRule {
            name: "distant",
            condition: Box::new(|m| m.relationship_score < 15.0),
            cue: MoodCue {
                tone: "idle".into(),
                accent: String::new(),
                weight: 0.45,
            },
        },

        // ═════════ 兜底 ═════════

        // 平静待机：什么都不明显 → 普通 idle，不点缀（眨眼由环境动画自己来）
        CueRule {
            name: "calm_idle",
            condition: Box::new(|_| true),
            cue: MoodCue {
                tone: "idle".into(),
                accent: String::new(),
                weight: 0.2,
            },
        },
    ]
}

/// 全局单例 mapper（无状态，可安全共享）
static GLOBAL_MAPPER: once_cell::sync::Lazy<MoodCueMapper> =
    once_cell::sync::Lazy::new(MoodCueMapper::new);

/// 便捷接口：从 MoodSnapshot 映射 MoodCue
pub fn mood_to_cue(mood: &MoodSnapshot) -> MoodCue {
    GLOBAL_MAPPER.map(mood)
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
        assert_eq!(cue.tone, "dizzy");
        assert!(cue.accent.is_empty());
        assert!((cue.weight - 0.95).abs() < 0.01);
    }

    #[test]
    fn test_exhausted_rule() {
        let mood = make_mood(0.0, 0.3, 85.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "dizzy");
    }

    #[test]
    fn test_burnout_rule() {
        // 又累又有压力，但都未到单独阈值 → 持续挂着垮脸
        let mood = make_mood(-0.2, 0.4, 65.0, 60.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "dizzy");
    }

    #[test]
    fn test_stressed_rules() {
        // 压力临界 → 持续不安 + 思考状
        let mood = make_mood(-0.2, 0.6, 20.0, 85.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "dizzy");
        assert_eq!(cue.accent, "think");
        // 高压力 → 压着火
        let mood = make_mood(-0.2, 0.6, 20.0, 75.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "angry");
    }

    // ═══ 第二层：强主导情绪 ═══

    #[test]
    fn test_joy_strong_rules() {
        // 欣喜若狂：Joy 极强 + 高唤醒 → 得意
        let mood = make_mood_with_emotion(0.8, 0.7, 30.0, 10.0, EmotionLabel::Joy, 0.85);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "smug");
        // 眉开眼笑：Joy 强但唤醒不高 → 开心
        let mood = make_mood_with_emotion(0.6, 0.4, 30.0, 10.0, EmotionLabel::Joy, 0.65);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "happy");
    }

    #[test]
    fn test_anger_rules() {
        // 怒气冲冲
        let mood =
            make_mood_with_emotion(-0.7, 0.6, 20.0, 10.0, EmotionLabel::Anger, 0.8);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "angry");
        // 生闷气：中等强度的火 + 低唤醒 → 仍是生气（词表无更温和档）
        let mood =
            make_mood_with_emotion(-0.5, 0.3, 20.0, 10.0, EmotionLabel::Anger, 0.6);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "angry");
    }

    #[test]
    fn test_sadness_rules() {
        // 泪如雨下 → 持续低落，不点缀
        let mood =
            make_mood_with_emotion(-0.6, 0.3, 20.0, 10.0, EmotionLabel::Sadness, 0.8);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "dizzy");
        assert!(cue.accent.is_empty());
        // 闷闷不乐
        let mood =
            make_mood_with_emotion(-0.4, 0.3, 20.0, 10.0, EmotionLabel::Sadness, 0.6);
        assert_eq!(mood_to_cue(&mood).tone, "dizzy");
    }

    #[test]
    fn test_fear_rules() {
        // 惊慌失措
        let mood =
            make_mood_with_emotion(-0.6, 0.8, 20.0, 10.0, EmotionLabel::Fear, 0.8);
        assert_eq!(mood_to_cue(&mood).tone, "dizzy");
        // 忐忑不安
        let mood =
            make_mood_with_emotion(-0.4, 0.4, 20.0, 10.0, EmotionLabel::Fear, 0.6);
        assert_eq!(mood_to_cue(&mood).tone, "dizzy");
    }

    #[test]
    fn test_closeness_rules() {
        // 满心爱意
        let mood =
            make_mood_with_emotion(0.6, 0.4, 20.0, 10.0, EmotionLabel::Closeness, 0.7);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "smug");
        // 温柔害羞
        let mood =
            make_mood_with_emotion(0.4, 0.4, 20.0, 10.0, EmotionLabel::Closeness, 0.6);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "blink");
    }

    #[test]
    fn test_loneliness_rules() {
        // 失落出神 → 持续放空
        let mood =
            make_mood_with_emotion(-0.3, 0.25, 20.0, 10.0, EmotionLabel::Loneliness, 0.7);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "dizzy");
        assert!(cue.accent.is_empty());
        // 怅然若失
        let mood =
            make_mood_with_emotion(-0.2, 0.3, 20.0, 10.0, EmotionLabel::Loneliness, 0.6);
        assert_eq!(mood_to_cue(&mood).tone, "dizzy");
    }

    #[test]
    fn test_curiosity_strong_rule() {
        // 满腹狐疑：强好奇 + 高唤醒 → 思考
        let mood = make_mood_with_emotion(0.1, 0.6, 20.0, 10.0, EmotionLabel::Curiosity, 0.7);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "think");
    }

    #[test]
    fn test_strong_emotion_overrides_moderate_fatigue() {
        // 强情绪压过中度疲劳：有点累但很开心 → 还是看得出开心
        let mood = make_mood_with_emotion(0.6, 0.6, 50.0, 10.0, EmotionLabel::Joy, 0.7);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert!(!cue.accent.is_empty());
    }

    // ═══ 第三层：中度疲劳 ═══

    #[test]
    fn test_drowsy_rule() {
        // 无强情绪 + 中度疲劳 + 低唤醒 → 持续想睡
        let mood = make_mood(0.0, 0.3, 60.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "dizzy");
        assert!((cue.weight - 0.5).abs() < 0.01);
    }

    // ═══ 第四层：效价-唤醒空间 ═══

    #[test]
    fn test_excited_rule() {
        let mood = make_mood(0.6, 0.8, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "smug");
    }

    #[test]
    fn test_anticipating_rule() {
        // 中高唤醒 + 正效价（未到兴奋）→ 开心
        let mood = make_mood(0.35, 0.55, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "happy");
    }

    #[test]
    fn test_cozy_companion_rule() {
        // 低唤醒 + 正效价 + 关系分 > 40 → 温柔眨眼
        let mut mood = make_mood(0.3, 0.2, 20.0, 10.0);
        mood.relationship_score = 60.0;
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "blink");
    }

    #[test]
    fn test_anxious_rule() {
        let mood = make_mood(-0.4, 0.6, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "dizzy");
        assert_eq!(cue.accent, "think");
    }

    #[test]
    fn test_miffed_rule() {
        // 轻度负效价 + 中唤醒 → 生气（不到持续低落）
        let mood = make_mood(-0.2, 0.4, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "angry");
    }

    #[test]
    fn test_listless_rule() {
        // 轻度负效价 + 低唤醒 → 持续无力
        let mood = make_mood(-0.2, 0.2, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).tone, "dizzy");
    }

    #[test]
    fn test_sad_fallback_rule() {
        let mood = make_mood(-0.5, 0.2, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).tone, "dizzy");
    }

    #[test]
    fn test_neutral_curious_rule() {
        let mood = make_mood(0.1, 0.4, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "think");
    }

    // ═══ 第五层：关系背景 ═══

    #[test]
    fn test_warm_companion_rule() {
        // 亲密度高 + 心情平和 → 得意满足
        let mut mood = make_mood(0.1, 0.2, 20.0, 10.0);
        mood.relationship_score = 80.0;
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "smug");
    }

    #[test]
    fn test_distant_rule() {
        // 亲密度极低 → 持续平淡疏离
        let mut mood = make_mood(0.0, 0.2, 20.0, 10.0);
        mood.relationship_score = 10.0;
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert!(cue.accent.is_empty());
    }

    // ═══ 兜底 ═══

    #[test]
    fn test_calm_fallback() {
        let mood = make_mood(0.1, 0.2, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.tone, "idle");
        assert!(cue.accent.is_empty());
    }

    // ═══ 不变量：基调必须可持续 ═══

    /// 基调只能是可持续的图集格位。
    ///
    /// 这是踩过的坑：happy 在图集里虽有格位，但词表规定同名帧序列优先，
    /// 一旦有规则把 happy 写成 tone，桌宠就会把"一次性动画"当"持久底色"，
    /// 表现是开心脸闪一下就没了、底色再也回不去。
    #[test]
    fn test_tone_is_always_sustainable() {
        let samples = vec![
            make_mood(0.0, 0.3, 95.0, 10.0),
            make_mood(0.0, 0.3, 85.0, 10.0),
            make_mood(-0.2, 0.4, 65.0, 60.0),
            make_mood(-0.2, 0.6, 20.0, 85.0),
            make_mood(-0.2, 0.6, 20.0, 75.0),
            make_mood_with_emotion(0.8, 0.7, 30.0, 10.0, EmotionLabel::Joy, 0.85),
            make_mood_with_emotion(0.6, 0.4, 30.0, 10.0, EmotionLabel::Joy, 0.65),
            make_mood_with_emotion(-0.7, 0.6, 20.0, 10.0, EmotionLabel::Anger, 0.8),
            make_mood_with_emotion(-0.6, 0.3, 20.0, 10.0, EmotionLabel::Sadness, 0.8),
            make_mood_with_emotion(-0.6, 0.8, 20.0, 10.0, EmotionLabel::Fear, 0.8),
            make_mood_with_emotion(0.6, 0.4, 20.0, 10.0, EmotionLabel::Closeness, 0.7),
            make_mood_with_emotion(-0.3, 0.25, 20.0, 10.0, EmotionLabel::Loneliness, 0.7),
            make_mood_with_emotion(0.1, 0.6, 20.0, 10.0, EmotionLabel::Curiosity, 0.7),
            make_mood(0.0, 0.3, 60.0, 10.0),
            make_mood(0.6, 0.8, 20.0, 10.0),
            make_mood(0.35, 0.55, 20.0, 10.0),
            make_mood(0.3, 0.2, 20.0, 10.0),
            make_mood(-0.4, 0.6, 20.0, 10.0),
            make_mood(-0.2, 0.4, 20.0, 10.0),
            make_mood(-0.5, 0.2, 20.0, 10.0),
            make_mood(-0.2, 0.2, 20.0, 10.0),
            make_mood(0.1, 0.4, 20.0, 10.0),
            make_mood(0.1, 0.2, 20.0, 10.0),
        ];
        for mood in samples {
            let cue = mood_to_cue(&mood);
            assert!(
                cue.tone_is_sustainable(),
                "基调 {:?} 不是可持续格位（valence={} arousal={} fatigue={} stress={}）",
                cue.tone,
                mood.valence,
                mood.arousal,
                mood.fatigue,
                mood.stress,
            );
        }
    }

    /// 所有规则的 accent 都必须在 Q 版一次性动画词表内。
    #[test]
    fn test_accent_names_are_valid() {
        const VALID: &[&str] = &["", "happy", "angry", "think", "smug", "blink"];
        let mut seen = std::collections::HashSet::new();
        for rule in default_rules() {
            let accent = rule.cue.accent;
            assert!(
                VALID.contains(&accent.as_str()),
                "规则 {} 的 accent {:?} 不在一次性动画词表内",
                rule.name,
                accent,
            );
            seen.insert(accent);
        }
        // 确保映射真的用到了多种点缀，而不是全部退化成同一个
        assert!(seen.len() >= 4, "accent 种类过少：{:?}", seen);
    }

    // ═══ 情绪捷径：强度分档 ═══

    #[test]
    fn test_emotion_shortcut() {
        let cue = MoodCueMapper::map_by_emotion(EmotionLabel::Joy, 0.8);
        assert_eq!(cue.tone, "idle");
        assert_eq!(cue.accent, "happy");
        assert!((cue.weight - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_emotion_shortcut_intensity_tiers() {
        // 快乐：强 → 开心；弱 → 得意微笑
        assert_eq!(
            MoodCueMapper::map_by_emotion(EmotionLabel::Joy, 0.8).accent,
            "happy"
        );
        assert_eq!(
            MoodCueMapper::map_by_emotion(EmotionLabel::Joy, 0.4).accent,
            "smug"
        );
        // 悲伤 / 恐惧 / 孤独 → 持续低落底色，不点缀
        assert_eq!(
            MoodCueMapper::map_by_emotion(EmotionLabel::Sadness, 0.8).tone,
            "dizzy"
        );
        assert_eq!(
            MoodCueMapper::map_by_emotion(EmotionLabel::Fear, 0.8).tone,
            "dizzy"
        );
        assert_eq!(
            MoodCueMapper::map_by_emotion(EmotionLabel::Loneliness, 0.8).tone,
            "dizzy"
        );
        // 好奇 → 思考
        assert_eq!(
            MoodCueMapper::map_by_emotion(EmotionLabel::Curiosity, 0.8).accent,
            "think"
        );
    }

    /// 捷径产出的基调同样必须可持续。
    #[test]
    fn test_emotion_shortcut_tone_is_sustainable() {
        for emotion in [
            EmotionLabel::Joy,
            EmotionLabel::Sadness,
            EmotionLabel::Anger,
            EmotionLabel::Fear,
            EmotionLabel::Closeness,
            EmotionLabel::Loneliness,
            EmotionLabel::Curiosity,
        ] {
            for intensity in [0.2, 0.5, 0.9] {
                let cue = MoodCueMapper::map_by_emotion(emotion, intensity);
                assert!(
                    cue.tone_is_sustainable(),
                    "{:?}@{} 的基调 {:?} 不可持续",
                    emotion,
                    intensity,
                    cue.tone,
                );
            }
        }
    }
}
