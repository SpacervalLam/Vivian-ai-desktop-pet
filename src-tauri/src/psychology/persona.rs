//! Persona 层 — 长期人格特质 + 依恋模式 + 稳态目标区间。
//!
//! Persona 是整个心理架构的最顶层，决定「同一件事，不同人格为什么反应不同」。
//! 它通过 set_points 和 recovery_rates 调制 Homeostasis，并通过 traits 调制 Appraisal。
//! 数值变化极慢（数月才变化一点），持久化到 persona.json。

use serde::{Deserialize, Serialize};

use super::homeostasis::{EmotionSetPoints, NeedSetPoints, RecoveryRateProfile};

/// 依恋模式 — 连续三维度（非互斥类型），基于 Bowlby 依恋理论
///
/// 三个值相互独立（0.0-1.0），一个人可以同时具有较高的安全型和一定的焦虑型倾向。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentStyle {
    /// 安全型 — 信任他人、自在亲密
    pub secure: f64,
    /// 焦虑型 — 害怕被抛弃、过度寻求确认
    pub anxious: f64,
    /// 回避型 — 回避亲密、偏好独立
    pub avoidant: f64,
}

impl Default for AttachmentStyle {
    fn default() -> Self {
        Self {
            secure: 0.6,
            anxious: 0.3,
            avoidant: 0.2,
        }
    }
}

/// 长期人格特质（8 项，0.0-1.0）
///
/// 这些特质相对稳定，数月才变化一点。它们决定 Appraisal 的调制系数和 Homeostasis 的 set point。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaTraits {
    /// 温暖度 — 关心他人的基础倾向
    pub warmth: f64,
    /// 顽皮度 — 玩耍、幽默的倾向
    pub playfulness: f64,
    /// 敏感度 — 对外部刺激的反应强度（越高，情绪波动越大）
    pub sensitivity: f64,
    /// 心理韧性 — 从负面情绪恢复的速度（越高，恢复越快）
    pub resilience: f64,
    /// 好奇心 — 探索新事物的内在驱动
    pub curiosity: f64,
    /// 社交性 — 主动寻求互动的倾向
    pub sociability: f64,
    /// 表达欲 — 主动分享、表达的倾向
    pub expressiveness: f64,
    /// 独立性 — 自主行动、不依赖他人的倾向
    pub independence: f64,
}

impl Default for PersonaTraits {
    fn default() -> Self {
        Self {
            warmth: 0.75,
            playfulness: 0.60,
            sensitivity: 0.55,
            resilience: 0.60,
            curiosity: 0.70,
            sociability: 0.55,
            expressiveness: 0.60,
            independence: 0.50,
        }
    }
}

impl PersonaTraits {
    /// 敏感度调制系数 — sensitivity 越高，事件引起的情绪变化幅度越大
    pub fn sensitivity_multiplier(&self) -> f64 {
        0.7 + self.sensitivity * 0.6 // 0.7 ~ 1.3
    }

    /// 韧性调制系数 — resilience 越高，情绪恢复速率越快
    pub fn resilience_multiplier(&self) -> f64 {
        0.6 + self.resilience * 0.9 // 0.6 ~ 1.5
    }
}

/// Persona 完整画像 — 特质 + 依恋 + 稳态目标区间

/// 逆向映射提示：从 PersonaTraits 推导出的 CharacterExpression 近似值。
///
/// 仅作为同步参考，调用方应与当前 CharacterExpression 做低权重混合。
#[derive(Debug, Clone, Default)]
pub struct ExpressionHint {
    pub tsundere: f64,
    pub clingy: f64,
    pub genki: f64,
    pub sass: f64,
    pub healing: f64,
    pub curiosity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaProfile {
    pub traits: PersonaTraits,
    pub attachment: AttachmentStyle,
    /// 各维度的稳态目标区间（由 Persona 调制 Homeostasis）
    pub need_set_points: NeedSetPoints,
    pub emotion_set_points: EmotionSetPoints,
    /// 各维度的恢复速率（由 Persona 调制）
    pub recovery_rates: RecoveryRateProfile,
}

impl Default for PersonaProfile {
    fn default() -> Self {
        Self {
            traits: PersonaTraits::default(),
            attachment: AttachmentStyle::default(),
            need_set_points: NeedSetPoints::default(),
            emotion_set_points: EmotionSetPoints::default(),
            recovery_rates: RecoveryRateProfile::default(),
        }
    }
}

impl PersonaProfile {
    /// 从人设表达维度（CharacterExpression）推导 PersonaProfile
    ///
    /// 将 6 维表演参数映射为心理特质 + 依恋 + 稳态目标。
    /// Persona 为只读初始参数，运行时不被 Appraisal/Emotion 修改；
    /// 仅在启动时由配置推导，并作为 set_points / recovery_rates 的调制基准。
    pub fn from_expression(
        tsundere: f64,
        clingy: f64,
        genki: f64,
        sass: f64,
        healing: f64,
        curiosity: f64,
    ) -> Self {
        let mut profile = Self::default();

        // 特质推导
        profile.traits.warmth = 0.4 + healing * 0.5;
        profile.traits.playfulness = 0.3 + genki * 0.5 + sass * 0.2;
        profile.traits.sensitivity = 0.3 + healing * 0.4 + clingy * 0.3;
        profile.traits.resilience = 0.4 + genki * 0.3 + (1.0 - tsundere) * 0.2;
        profile.traits.curiosity = 0.4 + curiosity * 0.5;
        profile.traits.sociability = 0.3 + clingy * 0.4 + genki * 0.3;
        profile.traits.expressiveness = 0.3 + genki * 0.4 + sass * 0.2;
        profile.traits.independence = 0.3 + (1.0 - clingy) * 0.4;

        // 依恋推导：clingy 高 → 焦虑型倾向；clingy 低 + healing 低 → 回避型倾向
        profile.attachment.secure = 0.4 + healing * 0.4 + (1.0 - tsundere) * 0.2;
        profile.attachment.anxious = 0.2 + clingy * 0.5;
        profile.attachment.avoidant = 0.1 + (1.0 - clingy) * 0.3 + (1.0 - healing) * 0.2;

        // 稳态目标由特质调制
        profile.apply_trait_modulation();

        profile
    }

    /// 启发式逆向映射：从当前 PersonaTraits 推导近似的 CharacterExpression 值。
    ///
    /// 由于正向映射 `from_expression()` 是多对一（6 输入 → 8+3 输出），
    /// 代数逆不唯一。此方法使用启发式逆估计 + 钳位，
    /// 用于心理学系统长期演化后反向同步到 PersonaEngine 的表达维度。
    ///
    /// 返回值仅作为"提示"（hint），调用方应与当前 CharacterExpression
    /// 做低权重混合（如 blend=0.1），而非直接覆盖。
    pub fn to_expression_hint(&self) -> ExpressionHint {
        let t = &self.traits;

        // healing ≈ (warmth - 0.4) / 0.5
        let healing = ((t.warmth - 0.4) / 0.5).clamp(0.0, 1.0);

        // curiosity ≈ (curiosity_trait - 0.4) / 0.5
        let curiosity_dim = ((t.curiosity - 0.4) / 0.5).clamp(0.0, 1.0);

        // clingy ≈ 1 - (independence - 0.3) / 0.4
        let clingy = (1.0 - (t.independence - 0.3) / 0.4).clamp(0.0, 1.0);

        // genki ≈ (playfulness - 0.3) / 0.7（假设 sass 贡献约 0.2*0.5 = 0.1）
        let genki = ((t.playfulness - 0.3) / 0.7).clamp(0.0, 1.0);

        // sass ≈ (expressiveness - 0.3 - genki*0.4) / 0.2
        let sass = ((t.expressiveness - 0.3 - genki * 0.4) / 0.2).clamp(0.0, 1.0);

        // tsundere ≈ 1 - (resilience - 0.4 - genki*0.3) / 0.2
        let tsundere = (1.0 - (t.resilience - 0.4 - genki * 0.3) / 0.2).clamp(0.0, 1.0);

        ExpressionHint {
            tsundere,
            clingy,
            genki,
            sass,
            healing,
            curiosity: curiosity_dim,
        }
    }

    /// 根据当前特质调制 set_points 和 recovery_rates
    ///
    /// 这在初始化后调用，也在 Persona 微调后调用以保持一致性。
    pub fn apply_trait_modulation(&mut self) {
        // Needs set points
        // 社交性高 → 归属需求目标高
        self.need_set_points.belonging = 0.40 + self.traits.sociability * 0.30;
        // 独立性高 → 自主需求目标高
        self.need_set_points.autonomy = 0.35 + self.traits.independence * 0.30;
        // 安全型依恋高 → 安全需求目标低（已满足基线低）；焦虑型高 → 安全需求目标高
        self.need_set_points.security =
            0.30 + self.attachment.anxious * 0.30 - self.attachment.secure * 0.10;
        // 好奇心高 → 新鲜需求目标高
        self.need_set_points.novelty = 0.35 + self.traits.curiosity * 0.30;
        // 表达欲高 → 表达需求目标高
        self.need_set_points.expression = 0.35 + self.traits.expressiveness * 0.30;

        // Emotion set points（默认偏中性，由特质微调）
        self.emotion_set_points.joy = 0.30 + self.traits.warmth * 0.15;
        self.emotion_set_points.closeness =
            0.25 + self.traits.warmth * 0.15 + self.attachment.secure * 0.10;
        self.emotion_set_points.loneliness = 0.10 + self.attachment.anxious * 0.15;
        self.emotion_set_points.curiosity = 0.35 + self.traits.curiosity * 0.15;
        // 负面情绪默认 set point 低
        self.emotion_set_points.sadness = 0.05;
        self.emotion_set_points.anger = 0.05;
        self.emotion_set_points.fear =
            0.08 + self.traits.sensitivity * 0.10 + self.attachment.anxious * 0.10;

        // Recovery rates 由韧性调制
        let resilience_mult = self.traits.resilience_multiplier();
        self.recovery_rates.belonging = 0.05 * resilience_mult;
        self.recovery_rates.autonomy = 0.05 * resilience_mult;
        self.recovery_rates.security = 0.04 * resilience_mult;
        self.recovery_rates.novelty = 0.06 * resilience_mult;
        self.recovery_rates.expression = 0.05 * resilience_mult;

        self.recovery_rates.joy = 0.04 * resilience_mult;
        self.recovery_rates.sadness = 0.06 * resilience_mult;
        self.recovery_rates.anger = 0.08 * resilience_mult;
        self.recovery_rates.fear = 0.05 * resilience_mult;
        self.recovery_rates.closeness = 0.04 * resilience_mult;
        self.recovery_rates.loneliness = 0.05 * resilience_mult;
        self.recovery_rates.curiosity = 0.05 * resilience_mult;
    }
}
