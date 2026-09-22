//! 轻量级 Mood → Cue 快速通道
//!
//! 在完整心理管道（LLM appraisal → emotion → behavior drive）之外，
//! 提供一条纯规则驱动的快速路径：从当前 MoodSnapshot 直接映射到
//! 角色表情/动作提示，用于：
//! - 心情变化时的即时反馈
//! - 主动 tick 期间的背景微动画
//! - 低频心跳动画（避免静态站立）
//!
//! # 输出为什么只有一个 accent
//!
//! Q 版渲染器是单通道互斥的：任意时刻只有一个动作在播，动作播完回落 `idle`。
//! 所以情绪只剩一种表达方式——**限时闪一下**（`accent` + 调用方给的时长）。
//!
//! 这里曾经分出第二层 `tone`（可持续的图集格位，`idle` / `dizzy`），让"疲惫/悲伤"
//! 长期挂着当底色。撤掉它的原因有两个，都不是审美问题：
//!
//! - 挂上就不动：底色只在规则切换时才重发，于是角色可以几十分钟停在同一张脸上，
//!   而"疲惫"本身是会变的，屏幕上的脸却在骗人。
//! - 连带冻住漫步：自主漫步的启用门槛是「姿态是 `idle`」，基调一旦不是 idle
//!   （如 `dizzy`），桌宠**彻底不走动**，而这个冻结跟情绪毫无关系。
//!
//! `dizzy` 于是降级成普通点缀：图集格位不能自己计时，由调用方传 `duration_ms`
//! 限时，到点回落 `idle`。代价是那张脸会偶发闪现而不是一直在，换来的是状态永远
//! 不黏住、漫步不受情绪牵连。
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

/// 角色提示：一次性点缀（空表示这次不出表情）
///
/// 曾经这里还有个 `tone` 字段（可持续的图集格位），用来把"疲惫/悲伤"长期挂在
/// 屏幕上当底色。那个设计被撤掉了：挂了底色之后桌宠会长时间停在一张脸上不动，
/// 而且因为基调不是 `idle`，自主漫步的门槛（要求姿态是 `idle`）会把它一起冻住。
/// 现在**没有任何持续状态**，一切情绪表达都是限时闪一下。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoodCue {
    /// 一次性点缀：动作名。帧序列（`happy` / `angry` / `think` / `smug` / `blink`）
    /// 按自己的节奏播完；图集格位（`dizzy`）靠调用方给的 `duration_ms` 限时，
    /// 到点回落 `idle`。
    pub accent: String,
    /// 权重：多个 cue 来源冲突时取权重最高者
    pub weight: f32,
}

impl MoodCue {
    pub fn none() -> Self {
        Self::default()
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
                    accent = %rule.cue.accent,
                    "[MoodCue] 命中规则"
                );
                return rule.cue.clone();
            }
        }
        MoodCue::none()
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
/// 第四层 效价-唤醒   中等情绪强度下的背景情绪
///        （valence × arousal 平面细分象限）
/// 第五层 关系背景    高亲密度的心底暖意 / 低亲密度的疏离
/// 兜底   平静待机
/// ```
///
/// 每条规则给出 `accent`（一次性点缀，可以是帧序列也可以是图集格位）：
/// 负向与疲惫类闪一张 `dizzy`，正向/愤怒/思考类闪对应的表情。空串表示这个状态
/// 不值得给表情（保持 `idle`）。
///
/// 同层内的组间条件互斥（primary_emotion 唯一），组内强档先判。
fn default_rules() -> Vec<CueRule> {
    vec![
        // ═════════ 第一层：生理底线（压过一切情绪）═════════

        // 睡着：疲劳 > 90 → 闪一张晕乎乎，随后回 idle
        CueRule {
            name: "sleeping",
            condition: Box::new(|m| m.fatigue > 90.0),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.95,
            },
        },
        // 极度疲惫：疲劳 > 80 → 闪一张晕乎乎
        CueRule {
            name: "exhausted",
            condition: Box::new(|m| m.fatigue > 80.0),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.9,
            },
        },
        // 身心俱疲（burnout）：又累又有压力 → 闪一张垮脸
        // 阈值抬高：只有真正「累到精神恍惚」才配 dizzy，日常有点累+有点压力不算。
        CueRule {
            name: "burnout",
            condition: Box::new(|m| m.fatigue > 72.0 && m.stress > 55.0),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.85,
            },
        },
        // 压力临界：压力 > 80 → 限时挂一次不安脸
        // 原本是「持续 dizzy 底色 + think 点缀」。去掉持续基调后就只剩一个通道，
        // 取这个状态的主读法（不安），不再兼顾那点"偶尔思考状"。
        CueRule {
            name: "stressed_critical",
            condition: Box::new(|m| m.stress > 80.0),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.85,
            },
        },
        // 高压力：压力 > 70 → 压着火，一点就着
        CueRule {
            name: "stressed_irritable",
            condition: Box::new(|m| m.stress > 70.0),
            cue: MoodCue {
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
                accent: "angry".into(),
                weight: 0.7,
            },
        },

        // —— 悲伤系 ——
        // 泪如雨下：Sadness 极强 → 闪一张低落脸
        CueRule {
            name: "sadness_grieving",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Sadness && m.primary_intensity > 0.7
            }),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.75,
            },
        },
        // 闷闷不乐：Sadness 中强 → 闪一张低落脸
        CueRule {
            name: "sadness_down",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Sadness && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.65,
            },
        },

        // —— 恐惧系 ——
        // 惊慌失措：Fear 极强 + 高唤醒 → 闪一张晕脸
        CueRule {
            name: "fear_panicking",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Fear
                    && m.primary_intensity > 0.7
                    && m.arousal > 0.5
            }),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.75,
            },
        },
        // 忐忑不安：Fear 中强 → 闪一张不安脸
        CueRule {
            name: "fear_anxious",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Fear && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                accent: "dizzy".into(),
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
                accent: "blink".into(),
                weight: 0.6,
            },
        },

        // —— 孤独系 ——
        // 失落出神：Loneliness 强 + 低唤醒 → 闪一张放空脸
        CueRule {
            name: "loneliness_withdrawn",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Loneliness && m.primary_intensity > 0.65
            }),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.65,
            },
        },
        // 怅然若失：Loneliness 中强 → 闪一张放空脸
        CueRule {
            name: "loneliness_wistful",
            condition: Box::new(|m| {
                m.primary_emotion == EmotionLabel::Loneliness && m.primary_intensity > 0.55
            }),
            cue: MoodCue {
                accent: "dizzy".into(),
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
                accent: "think".into(),
                weight: 0.6,
            },
        },

        // ═════════ 第三层：中度疲劳（无强情绪时，倦意才浮上表面）═════════

        // 昏昏欲睡：疲劳 > 72 且唤醒很低 → 闪一张想睡脸
        // 阈值抬高：fatigue 55~72 只是「有点累」，不该闪晕脸（日常太常见）。
        CueRule {
            name: "drowsy",
            condition: Box::new(|m| m.fatigue > 72.0 && m.arousal < 0.4),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.5,
            },
        },

        // ═════════ 第四层：效价-唤醒空间（中等情绪强度下的背景情绪）═════════

        // 兴奋：高唤醒 + 正效价 → 得意
        CueRule {
            name: "excited",
            condition: Box::new(|m| m.arousal > 0.7 && m.valence > 0.4),
            cue: MoodCue {
                accent: "smug".into(),
                weight: 0.7,
            },
        },
        // 满怀期待：中高唤醒 + 正效价 → 开心
        CueRule {
            name: "anticipating",
            condition: Box::new(|m| m.arousal > 0.5 && m.valence > 0.3),
            cue: MoodCue {
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
                accent: "blink".into(),
                weight: 0.6,
            },
        },
        // 温馨：低唤醒 + 正效价 → 温柔眨眼
        CueRule {
            name: "cozy",
            condition: Box::new(|m| m.arousal < 0.35 && m.valence > 0.2),
            cue: MoodCue {
                accent: "blink".into(),
                weight: 0.6,
            },
        },
        // 焦虑不安：负效价 + 高唤醒 → 思考状（坐立不安），不闪晕脸
        // 焦虑是「烦躁/坐立难安」，不是眩晕；dizzy 留给真正的头晕/虚脱。
        CueRule {
            name: "anxious",
            condition: Box::new(|m| m.valence < -0.3 && m.arousal > 0.5),
            cue: MoodCue {
                accent: "think".into(),
                weight: 0.7,
            },
        },
        // 不高兴：轻度负效价 + 中唤醒 → 生气（不到低落脸那档）
        CueRule {
            name: "miffed",
            condition: Box::new(|m| m.valence < -0.15 && m.arousal > 0.35),
            cue: MoodCue {
                accent: "angry".into(),
                weight: 0.5,
            },
        },
        // 情绪低落：负效价 → 持续低落
        CueRule {
            name: "sad",
            condition: Box::new(|m| m.valence < -0.3),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.6,
            },
        },
        // 委靡无力：明显低落 + 低唤醒 → 持续无力（仅真正「闷到极点」才晕）
        // 阈值抬高：valence 仅略低于 0（日常「有点闷/有点丧」）不应闪晕脸，
        // 回落到 idle 更自然；只有 valence < -0.45 这种明确低落才进 dizzy（与 sad 兜底呼应）。
        CueRule {
            name: "listless",
            condition: Box::new(|m| m.valence < -0.45 && m.arousal < 0.25),
            cue: MoodCue {
                accent: "dizzy".into(),
                weight: 0.45,
            },
        },
        // 好奇观察：中性效价 + 中唤醒 → 思考打量
        CueRule {
            name: "neutral_curious",
            condition: Box::new(|m| m.valence.abs() < 0.3 && m.arousal > 0.3),
            cue: MoodCue {
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
                accent: "smug".into(),
                weight: 0.5,
            },
        },
        // 疏离旁观：亲密度极低 → 持续平淡保持距离
        CueRule {
            name: "distant",
            condition: Box::new(|m| m.relationship_score < 15.0),
            cue: MoodCue {
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
        assert_eq!(cue.accent, "dizzy");
        assert!((cue.weight - 0.95).abs() < 0.01);
    }

    #[test]
    fn test_exhausted_rule() {
        let mood = make_mood(0.0, 0.3, 85.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "dizzy");
    }

    #[test]
    fn test_burnout_rule() {
        // 又累又有压力，且都到高位 → 闪一张垮脸
        let mood = make_mood(-0.2, 0.4, 75.0, 60.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "dizzy");
    }

    #[test]
    fn test_burnout_rule_not_triggered_for_mild() {
        // 轻度累 + 轻度压力（fatigue 65 / stress 60）不该闪晕脸
        let mood = make_mood(-0.2, 0.4, 65.0, 60.0);
        assert_ne!(mood_to_cue(&mood).accent, "dizzy");
    }

    #[test]
    fn test_stressed_rules() {
        // 压力临界 → 闪一张不安脸
        let mood = make_mood(-0.2, 0.6, 20.0, 85.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "dizzy");
        // 高压力 → 压着火
        let mood = make_mood(-0.2, 0.6, 20.0, 75.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "angry");
    }

    // ═══ 第二层：强主导情绪 ═══

    #[test]
    fn test_joy_strong_rules() {
        // 欣喜若狂：Joy 极强 + 高唤醒 → 得意
        let mood = make_mood_with_emotion(0.8, 0.7, 30.0, 10.0, EmotionLabel::Joy, 0.85);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "smug");
        // 眉开眼笑：Joy 强但唤醒不高 → 开心
        let mood = make_mood_with_emotion(0.6, 0.4, 30.0, 10.0, EmotionLabel::Joy, 0.65);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "happy");
    }

    #[test]
    fn test_anger_rules() {
        // 怒气冲冲
        let mood =
            make_mood_with_emotion(-0.7, 0.6, 20.0, 10.0, EmotionLabel::Anger, 0.8);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "angry");
        // 生闷气：中等强度的火 + 低唤醒 → 仍是生气（词表无更温和档）
        let mood =
            make_mood_with_emotion(-0.5, 0.3, 20.0, 10.0, EmotionLabel::Anger, 0.6);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "angry");
    }

    #[test]
    fn test_sadness_rules() {
        // 泪如雨下 → 闪一张低落脸
        let mood =
            make_mood_with_emotion(-0.6, 0.3, 20.0, 10.0, EmotionLabel::Sadness, 0.8);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "dizzy");
        // 闷闷不乐
        let mood =
            make_mood_with_emotion(-0.4, 0.3, 20.0, 10.0, EmotionLabel::Sadness, 0.6);
        assert_eq!(mood_to_cue(&mood).accent, "dizzy");
    }

    #[test]
    fn test_fear_rules() {
        // 惊慌失措
        let mood =
            make_mood_with_emotion(-0.6, 0.8, 20.0, 10.0, EmotionLabel::Fear, 0.8);
        assert_eq!(mood_to_cue(&mood).accent, "dizzy");
        // 忐忑不安
        let mood =
            make_mood_with_emotion(-0.4, 0.4, 20.0, 10.0, EmotionLabel::Fear, 0.6);
        assert_eq!(mood_to_cue(&mood).accent, "dizzy");
    }

    #[test]
    fn test_closeness_rules() {
        // 满心爱意
        let mood =
            make_mood_with_emotion(0.6, 0.4, 20.0, 10.0, EmotionLabel::Closeness, 0.7);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "smug");
        // 温柔害羞
        let mood =
            make_mood_with_emotion(0.4, 0.4, 20.0, 10.0, EmotionLabel::Closeness, 0.6);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "blink");
    }

    #[test]
    fn test_loneliness_rules() {
        // 失落出神 → 闪一张放空脸
        let mood =
            make_mood_with_emotion(-0.3, 0.25, 20.0, 10.0, EmotionLabel::Loneliness, 0.7);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "dizzy");
        // 怅然若失
        let mood =
            make_mood_with_emotion(-0.2, 0.3, 20.0, 10.0, EmotionLabel::Loneliness, 0.6);
        assert_eq!(mood_to_cue(&mood).accent, "dizzy");
    }

    #[test]
    fn test_curiosity_strong_rule() {
        // 满腹狐疑：强好奇 + 高唤醒 → 思考
        let mood = make_mood_with_emotion(0.1, 0.6, 20.0, 10.0, EmotionLabel::Curiosity, 0.7);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "think");
    }

    #[test]
    fn test_strong_emotion_overrides_moderate_fatigue() {
        // 强情绪压过中度疲劳：有点累但很开心 → 还是看得出开心
        let mood = make_mood_with_emotion(0.6, 0.6, 50.0, 10.0, EmotionLabel::Joy, 0.7);
        let cue = mood_to_cue(&mood);
        assert!(!cue.accent.is_empty());
    }

    // ═══ 第三层：中度疲劳 ═══

    #[test]
    fn test_drowsy_rule() {
        // 真正很累 + 低唤醒 → 闪一张想睡脸（阈值已抬高，fatigue 55~72 不再触发）
        let mood = make_mood(0.0, 0.35, 75.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "dizzy");
        assert!((cue.weight - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_drowsy_rule_not_triggered_for_mild_fatigue() {
        // 只是「有点累」（fatigue 60）不该闪晕脸，回落到平静 idle
        let mood = make_mood(0.0, 0.3, 60.0, 10.0);
        assert!(mood_to_cue(&mood).accent.is_empty());
    }

    // ═══ 第四层：效价-唤醒空间 ═══

    #[test]
    fn test_excited_rule() {
        let mood = make_mood(0.6, 0.8, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "smug");
    }

    #[test]
    fn test_anticipating_rule() {
        // 中高唤醒 + 正效价（未到兴奋）→ 开心
        let mood = make_mood(0.35, 0.55, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "happy");
    }

    #[test]
    fn test_cozy_companion_rule() {
        // 低唤醒 + 正效价 + 关系分 > 40 → 温柔眨眼
        let mut mood = make_mood(0.3, 0.2, 20.0, 10.0);
        mood.relationship_score = 60.0;
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "blink");
    }

    #[test]
    fn test_anxious_rule() {
        // 焦虑 = 烦躁/坐立难安，只给思考状点缀，不闪晕脸
        let mood = make_mood(-0.4, 0.6, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "think");
    }

    #[test]
    fn test_miffed_rule() {
        // 轻度负效价 + 中唤醒 → 生气（不到低落脸那档）
        let mood = make_mood(-0.2, 0.4, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "angry");
    }

    #[test]
    fn test_listless_rule() {
        // 明显低落 + 低唤醒（valence < -0.45）→ 闪一张无力脸
        let mood = make_mood(-0.5, 0.2, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).accent, "dizzy");
    }

    #[test]
    fn test_mild_negative_does_not_flash_dizzy() {
        // 日常「有点闷/有点丧」（valence 仅略低于 0）不该闪晕脸，回落到平静 idle。
        // 这是修复「频繁错误触发 dizzy」的关键不变量：只有明确低落才进 dizzy。
        let mild = make_mood(-0.2, 0.2, 20.0, 10.0);
        assert!(mood_to_cue(&mild).accent.is_empty());
        let slightly = make_mood(-0.3, 0.25, 20.0, 10.0);
        assert!(mood_to_cue(&slightly).accent.is_empty());
    }

    #[test]
    fn test_sad_fallback_rule() {
        let mood = make_mood(-0.5, 0.2, 20.0, 10.0);
        assert_eq!(mood_to_cue(&mood).accent, "dizzy");
    }

    #[test]
    fn test_neutral_curious_rule() {
        let mood = make_mood(0.1, 0.4, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "think");
    }

    // ═══ 第五层：关系背景 ═══

    #[test]
    fn test_warm_companion_rule() {
        // 亲密度高 + 心情平和 → 得意满足
        let mut mood = make_mood(0.1, 0.2, 20.0, 10.0);
        mood.relationship_score = 80.0;
        let cue = mood_to_cue(&mood);
        assert_eq!(cue.accent, "smug");
    }

    #[test]
    fn test_distant_rule() {
        // 亲密度极低 → 平淡疏离，不给表情
        let mut mood = make_mood(0.0, 0.2, 20.0, 10.0);
        mood.relationship_score = 10.0;
        let cue = mood_to_cue(&mood);
        assert!(cue.accent.is_empty());
    }

    // ═══ 兜底 ═══

    #[test]
    fn test_calm_fallback() {
        let mood = make_mood(0.1, 0.2, 20.0, 10.0);
        let cue = mood_to_cue(&mood);
        assert!(cue.accent.is_empty());
    }

    // ═══ 不变量：accent 必须是渲染器真的能播的名字 ═══

    /// 所有规则的 accent 都必须是渲染器真的能播的名字。
    ///
    /// 名单里同时有两类：帧序列（`happy` / `angry` / `think` / `smug` / `blink`）
    /// 自带节奏，播完即落；图集格位（`dizzy`）自己不会计时，**必须由调用方传
    /// `duration_ms`**，否则那张脸会一直挂着——`auto_trigger` 那侧有对应用例守着。
    #[test]
    fn test_accent_names_are_valid() {
        const VALID: &[&str] = &["", "happy", "angry", "think", "smug", "blink", "dizzy"];
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
}
