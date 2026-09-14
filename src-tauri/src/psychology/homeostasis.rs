//! Homeostasis — 心理稳态引擎。
//!
//! 这是整个系统最关键的机制：每个 Needs/Emotion 维度围绕各自的「目标区间（Set Point）」
//! 自动调节。长期孤独会驱使寻找陪伴；长期高压会促使休息；长期情绪高涨会逐渐回归平静。
//!
//! Persona 决定不同人的 set point 和恢复速度（外向的人归属需求目标更高，敏感的人恢复更慢）。

use serde::{Deserialize, Serialize};

// ============================================================================
// Set Points — 各维度的稳态目标区间
// ============================================================================

// ---- Needs 设定点默认值（基于心理学常模）----
/// 归属需求 set point：中等偏高，人本质是社会性动物
const DEFAULT_NEED_BELONGING: f64 = 0.55;
/// 自主需求 set point：中性，平衡自主与依赖
const DEFAULT_NEED_AUTONOMY: f64 = 0.50;
/// 安全需求 set point：偏低，假设环境基本安全
const DEFAULT_NEED_SECURITY: f64 = 0.40;
/// 新奇需求 set point：中等偏高，鼓励探索
const DEFAULT_NEED_NOVELTY: f64 = 0.55;
/// 表达需求 set point：中性
const DEFAULT_NEED_EXPRESSION: f64 = 0.50;

// ---- Emotion 设定点默认值（基于心理学常模）----
/// 喜悦 set point：温和的正面基线
const DEFAULT_EMO_JOY: f64 = 0.35;
/// 悲伤 set point：低基线，负面情绪自然衰减
const DEFAULT_EMO_SADNESS: f64 = 0.05;
/// 愤怒 set point：低基线
const DEFAULT_EMO_ANGER: f64 = 0.05;
/// 恐惧 set point：低基线，但略高于悲伤/愤怒（保持警觉）
const DEFAULT_EMO_FEAR: f64 = 0.10;
/// 亲密感 set point：温和的正面基线
const DEFAULT_EMO_CLOSENESS: f64 = 0.35;
/// 孤独感 set point：偏低，长期独处时缓慢上升
const DEFAULT_EMO_LONELINESS: f64 = 0.15;
/// 好奇心 set point：较高，驱使探索行为
const DEFAULT_EMO_CURIOSITY: f64 = 0.45;

/// Needs 维度的稳态目标（0.0-1.0）
///
/// 当某个 Need 低于其 set_point 时，表示「已满足」，会缓慢回归；
/// 当高于 set_point 时，表示「未满足」，会持续增长（驱动行为）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedSetPoints {
    pub belonging: f64,
    pub autonomy: f64,
    pub security: f64,
    pub novelty: f64,
    pub expression: f64,
}

impl Default for NeedSetPoints {
    fn default() -> Self {
        Self {
            belonging: DEFAULT_NEED_BELONGING,
            autonomy: DEFAULT_NEED_AUTONOMY,
            security: DEFAULT_NEED_SECURITY,
            novelty: DEFAULT_NEED_NOVELTY,
            expression: DEFAULT_NEED_EXPRESSION,
        }
    }
}

/// Emotion 维度的稳态目标（0.0-1.0）
///
/// 正面情绪 set point 偏高（人倾向于回到温和的正面状态），
/// 负面情绪 set point 偏低（负面情绪会自然衰减）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionSetPoints {
    pub joy: f64,
    pub sadness: f64,
    pub anger: f64,
    pub fear: f64,
    pub closeness: f64,
    pub loneliness: f64,
    pub curiosity: f64,
}

impl Default for EmotionSetPoints {
    fn default() -> Self {
        Self {
            joy: DEFAULT_EMO_JOY,
            sadness: DEFAULT_EMO_SADNESS,
            anger: DEFAULT_EMO_ANGER,
            fear: DEFAULT_EMO_FEAR,
            closeness: DEFAULT_EMO_CLOSENESS,
            loneliness: DEFAULT_EMO_LONELINESS,
            curiosity: DEFAULT_EMO_CURIOSITY,
        }
    }
}

/// 各维度的恢复速率（每秒回归系数，越大越快）
///
/// 由 Persona 的 resilience 调制。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryRateProfile {
    pub belonging: f64,
    pub autonomy: f64,
    pub security: f64,
    pub novelty: f64,
    pub expression: f64,
    pub joy: f64,
    pub sadness: f64,
    pub anger: f64,
    pub fear: f64,
    pub closeness: f64,
    pub loneliness: f64,
    pub curiosity: f64,
}

impl Default for RecoveryRateProfile {
    fn default() -> Self {
        Self {
            belonging: 0.05,
            autonomy: 0.05,
            security: 0.04,
            novelty: 0.06,
            expression: 0.05,
            joy: 0.04,
            sadness: 0.06,
            anger: 0.08,
            fear: 0.05,
            closeness: 0.04,
            loneliness: 0.05,
            curiosity: 0.05,
        }
    }
}

// ============================================================================
// Homeostasis 引擎
// ============================================================================

use rand::Rng;

/// 通用指数回归：value 向 set_point 以 rate 速率回归
///
/// 公式：value += (set_point - value) * (1 - exp(-rate * dt))
/// 当 value == set_point 时无变化；偏离越大，回归越快（但速率受 rate 限制）。
fn regress(value: f64, set_point: f64, rate: f64, dt: f64) -> f64 {
    let alpha = 1.0 - (-rate * dt).exp();
    value + (set_point - value) * alpha
}

/// 带微噪声的回归 — 让情绪持续"活着"，不静止
///
/// 三层机制：
/// 1. 正常回归到 set_point（稳态）
/// 2. 小幅随机噪声（模拟心理活动的自然波动，随 √dt 缩放保证时间不变性）
/// 3. 极值回避：接近 0 或 1 时施加更强的向中拉力，防止卡在极端值
fn fluctuate(value: f64, set_point: f64, rate: f64, dt: f64, noise_amp: f64) -> f64 {
    let regressed = regress(value, set_point, rate, dt);
    let mut rng = rand::rng();
    let noise = rng.random_range(-1.0..1.0) * noise_amp * dt.sqrt();
    let extreme_pull = if regressed > 0.85 {
        -(regressed - 0.85) * 0.20 * dt
    } else if regressed < 0.15 {
        (0.15 - regressed) * 0.20 * dt
    } else {
        0.0
    };
    (regressed + noise + extreme_pull).clamp(0.0, 1.0)
}

/// Needs 维度的非对称回归（带小幅噪声）
///
/// - 低于 set_point（已满足）：缓慢回归（速率 ×0.5），表示满足感会缓慢消退
/// - 高于 set_point（未满足）：正常回归，但 set_point 本身会缓慢上升（饥饿感）
///   这模拟「需求越久未满足，渴望越强」
///
/// `circadian_offset` 临时偏移回归目标（昼夜节律），不参与饥饿感累积基准。
fn need_decay(
    value: f64,
    set_point: &mut f64,
    circadian_offset: f64,
    rate: f64,
    dt: f64,
) -> f64 {
    let target = (*set_point + circadian_offset).clamp(0.0, 1.0);
    let diff = target - value;
    let mut rng = rand::rng();
    let noise = rng.random_range(-1.0..1.0) * 0.003 * dt.sqrt();
    if diff > 0.0 {
        // 已满足 → 缓慢回归到目标
        (regress(value, target, rate * 0.5, dt) + noise).clamp(0.0, 1.0)
    } else {
        // 未满足 → 持久化 set_point 缓慢上升（饥饿感增长，但不超过 0.85）
        let hunger_rate = 0.005 * dt;
        *set_point = (*set_point + hunger_rate).min(0.85);
        // value 也向目标回归，但更慢
        (regress(value, target, rate * 0.3, dt) + noise).clamp(0.0, 1.0)
    }
}

// ============================================================================
// 昼夜节律调制（Circadian Modulation）
// ============================================================================

/// 昼夜节律对心理稳态的临时调制因子
///
/// 基于昼夜节律心理学：皮质醇晨起峰值、褪黑素夜间峰值、深夜情绪调节下降、
/// 下午认知与情绪调节最佳。对 set_points / recovery / noise 做小幅度临时调制。
///
/// 仅作用于 homeostasis 背景，不污染持久化的 persona set_points ——
/// 每次 tick 实时计算，叠加到临时副本上。
#[derive(Debug, Clone, Copy)]
pub struct CircadianFactors {
    // Emotion set_point 增量
    pub joy_delta: f64,
    pub sadness_delta: f64,
    pub anger_delta: f64,
    pub fear_delta: f64,
    pub closeness_delta: f64,
    pub loneliness_delta: f64,
    pub curiosity_delta: f64,
    // Need set_point 增量
    pub belonging_delta: f64,
    pub autonomy_delta: f64,
    pub security_delta: f64,
    pub novelty_delta: f64,
    pub expression_delta: f64,
    // recovery_rates 乘数
    pub recovery_mult: f64,
    // 噪声乘数：正面情绪(joy/closeness/curiosity)、负面情绪(sadness/anger/fear/loneliness)
    pub positive_noise_mult: f64,
    pub negative_noise_mult: f64,
}

// 4 个时段中点处的基准值。时段划分：
//   早晨 6-11（中点 8.5）/ 下午 11-18（中点 14.5）/ 傍晚 18-23（中点 20.5）/ 深夜 23-6（中点 2.5）
// 任意时刻由相邻锚点线性插值得到，实现时段交界平滑过渡。
const CIRCADIAN_MORNING: CircadianFactors = CircadianFactors {
    joy_delta: -0.05,
    sadness_delta: 0.0,
    anger_delta: 0.0,
    fear_delta: 0.0,
    closeness_delta: -0.03,
    loneliness_delta: 0.0,
    curiosity_delta: 0.08,
    belonging_delta: 0.0,
    autonomy_delta: 0.0,
    security_delta: 0.0,
    novelty_delta: 0.05,
    expression_delta: 0.0,
    recovery_mult: 1.0,
    positive_noise_mult: 1.1,
    negative_noise_mult: 1.0,
};

const CIRCADIAN_AFTERNOON: CircadianFactors = CircadianFactors {
    joy_delta: 0.05,
    sadness_delta: -0.03,
    anger_delta: 0.0,
    fear_delta: 0.0,
    closeness_delta: 0.05,
    loneliness_delta: -0.05,
    curiosity_delta: 0.0,
    belonging_delta: -0.05,
    autonomy_delta: 0.0,
    security_delta: 0.0,
    novelty_delta: 0.0,
    expression_delta: 0.0,
    recovery_mult: 1.2,
    positive_noise_mult: 1.0,
    negative_noise_mult: 0.9,
};

const CIRCADIAN_EVENING: CircadianFactors = CircadianFactors {
    joy_delta: 0.0,
    sadness_delta: 0.0,
    anger_delta: 0.0,
    fear_delta: 0.0,
    closeness_delta: 0.08,
    loneliness_delta: 0.0,
    curiosity_delta: -0.05,
    belonging_delta: 0.05,
    autonomy_delta: 0.0,
    security_delta: 0.0,
    novelty_delta: 0.0,
    expression_delta: 0.05,
    recovery_mult: 0.9,
    positive_noise_mult: 0.9,
    negative_noise_mult: 1.0,
};

const CIRCADIAN_NIGHT: CircadianFactors = CircadianFactors {
    joy_delta: -0.08,
    sadness_delta: 0.05,
    anger_delta: 0.0,
    fear_delta: 0.03,
    closeness_delta: 0.05,
    loneliness_delta: 0.10,
    curiosity_delta: -0.08,
    belonging_delta: 0.0,
    autonomy_delta: 0.0,
    security_delta: 0.05,
    novelty_delta: -0.05,
    expression_delta: 0.0,
    recovery_mult: 0.8,
    positive_noise_mult: 0.7,
    negative_noise_mult: 1.3,
};

impl CircadianFactors {
    /// 锚点：(hour, factors)，按 hour 升序。深夜锚点在 2.5 和 26.5（环形展开）。
    const ANCHORS: [(f64, CircadianFactors); 5] = [
        (2.5, CIRCADIAN_NIGHT),
        (8.5, CIRCADIAN_MORNING),
        (14.5, CIRCADIAN_AFTERNOON),
        (20.5, CIRCADIAN_EVENING),
        (26.5, CIRCADIAN_NIGHT),
    ];

    fn lerp(a: CircadianFactors, b: CircadianFactors, t: f64) -> CircadianFactors {
        CircadianFactors {
            joy_delta: a.joy_delta + (b.joy_delta - a.joy_delta) * t,
            sadness_delta: a.sadness_delta + (b.sadness_delta - a.sadness_delta) * t,
            anger_delta: a.anger_delta + (b.anger_delta - a.anger_delta) * t,
            fear_delta: a.fear_delta + (b.fear_delta - a.fear_delta) * t,
            closeness_delta: a.closeness_delta + (b.closeness_delta - a.closeness_delta) * t,
            loneliness_delta: a.loneliness_delta + (b.loneliness_delta - a.loneliness_delta) * t,
            curiosity_delta: a.curiosity_delta + (b.curiosity_delta - a.curiosity_delta) * t,
            belonging_delta: a.belonging_delta + (b.belonging_delta - a.belonging_delta) * t,
            autonomy_delta: a.autonomy_delta + (b.autonomy_delta - a.autonomy_delta) * t,
            security_delta: a.security_delta + (b.security_delta - a.security_delta) * t,
            novelty_delta: a.novelty_delta + (b.novelty_delta - a.novelty_delta) * t,
            expression_delta: a.expression_delta + (b.expression_delta - a.expression_delta) * t,
            recovery_mult: a.recovery_mult + (b.recovery_mult - a.recovery_mult) * t,
            positive_noise_mult: a.positive_noise_mult
                + (b.positive_noise_mult - a.positive_noise_mult) * t,
            negative_noise_mult: a.negative_noise_mult
                + (b.negative_noise_mult - a.negative_noise_mult) * t,
        }
    }

    /// 根据本地时间的小数小时（0.0-23.999）返回该时刻的昼夜调制因子
    pub fn at_hour(hour: f64) -> CircadianFactors {
        let mut h = hour;
        // 把 [0, 2.5) 映射到 [24.5, 26.5)，使整个 [2.5, 26.5) 区间连续覆盖 24 小时
        if h < 2.5 {
            h += 24.0;
        }
        for i in 0..4 {
            let (h0, f0) = Self::ANCHORS[i];
            let (h1, f1) = Self::ANCHORS[i + 1];
            if h >= h0 && h <= h1 {
                let t = (h - h0) / (h1 - h0);
                return Self::lerp(f0, f1, t);
            }
        }
        CIRCADIAN_MORNING
    }
}

/// Homeostasis 引擎 — 对 Needs 和 Emotion 执行稳态调节
pub struct HomeostasisEngine;

impl HomeostasisEngine {
    /// 离线期间的情绪压缩 —— 各通道独立速率
    ///
    /// 不同情绪穿透睡眠的能力不同：愤怒消散最快，亲近感几乎不打折。
    /// `offline_minutes` 为离线总分钟数，各通道按独立压缩系数折算后回归到 set_point。
    pub fn apply_offline_compression(
        emotion: &mut super::emotion::EmotionState,
        set_points: &EmotionSetPoints,
        recovery_rates: &RecoveryRateProfile,
        offline_minutes: f64,
        circadian: CircadianFactors,
    ) {
        let compress = |factor: f64| -> f64 { offline_minutes * factor * 60.0 };

        emotion.anger = regress(
            emotion.anger,
            (set_points.anger + circadian.anger_delta).clamp(0.0, 1.0),
            recovery_rates.anger * circadian.recovery_mult,
            compress(0.08),
        );
        emotion.fear = regress(
            emotion.fear,
            (set_points.fear + circadian.fear_delta).clamp(0.0, 1.0),
            recovery_rates.fear * circadian.recovery_mult,
            compress(0.25),
        );
        emotion.sadness = regress(
            emotion.sadness,
            (set_points.sadness + circadian.sadness_delta).clamp(0.0, 1.0),
            recovery_rates.sadness * circadian.recovery_mult,
            compress(0.30),
        );
        emotion.joy = regress(
            emotion.joy,
            (set_points.joy + circadian.joy_delta).clamp(0.0, 1.0),
            recovery_rates.joy * circadian.recovery_mult,
            compress(0.20),
        );
        emotion.closeness = regress(
            emotion.closeness,
            (set_points.closeness + circadian.closeness_delta).clamp(0.0, 1.0),
            recovery_rates.closeness * circadian.recovery_mult,
            compress(0.85),
        );
        emotion.curiosity = regress(
            emotion.curiosity,
            (set_points.curiosity + circadian.curiosity_delta).clamp(0.0, 1.0),
            recovery_rates.curiosity * circadian.recovery_mult,
            compress(0.30),
        );

        let loneliness_target = (set_points.loneliness
            + circadian.loneliness_delta
            + (offline_minutes / 60.0).min(24.0) * 0.01)
        .clamp(0.0, 0.85);
        emotion.loneliness = regress(
            emotion.loneliness,
            loneliness_target,
            recovery_rates.loneliness * circadian.recovery_mult,
            offline_minutes * 60.0,
        );

        emotion.joy = emotion.joy.clamp(0.0, 1.0);
        emotion.sadness = emotion.sadness.clamp(0.0, 1.0);
        emotion.anger = emotion.anger.clamp(0.0, 1.0);
        emotion.fear = emotion.fear.clamp(0.0, 1.0);
        emotion.closeness = emotion.closeness.clamp(0.0, 1.0);
        emotion.loneliness = emotion.loneliness.clamp(0.0, 1.0);
        emotion.curiosity = emotion.curiosity.clamp(0.0, 1.0);
    }

    /// 对完整心理状态执行一次稳态 tick
    ///
    /// `dt` 是距离上次 tick 的秒数。会就地修改 needs 和 emotion，
    /// 并可能微调 need_set_points（饥饿感机制）。
    ///
    /// `circadian` 提供昼夜节律临时调制：偏移 set_points、缩放 recovery_rates 与
    /// noise 幅度。仅作用于本次 tick 的临时目标，不写回持久化的 persona set_points。
    pub fn tick(
        needs: &mut super::needs::NeedsState,
        emotion: &mut super::emotion::EmotionState,
        need_set_points: &mut NeedSetPoints,
        emotion_set_points: &EmotionSetPoints,
        recovery_rates: &RecoveryRateProfile,
        dt: f64,
        circadian: CircadianFactors,
    ) {
        // Needs 稳态（带饥饿感 + 昼夜节律临时偏移目标）
        needs.belonging = need_decay(needs.belonging, &mut need_set_points.belonging, circadian.belonging_delta, recovery_rates.belonging * circadian.recovery_mult, dt);
        needs.autonomy = need_decay(needs.autonomy, &mut need_set_points.autonomy, circadian.autonomy_delta, recovery_rates.autonomy * circadian.recovery_mult, dt);
        needs.security = need_decay(needs.security, &mut need_set_points.security, circadian.security_delta, recovery_rates.security * circadian.recovery_mult, dt);
        needs.novelty = need_decay(needs.novelty, &mut need_set_points.novelty, circadian.novelty_delta, recovery_rates.novelty * circadian.recovery_mult, dt);
        needs.expression = need_decay(needs.expression, &mut need_set_points.expression, circadian.expression_delta, recovery_rates.expression * circadian.recovery_mult, dt);

        // Emotion 稳态（带微噪声波动 + 极值回避 + 昼夜节律）
        // 噪声幅度：正面情绪 0.008，负面情绪 0.005（负面波动更小，避免无原因的大幅波动）
        // 正面/负面噪声分别受 positive_noise_mult / negative_noise_mult 调制
        emotion.joy = fluctuate(emotion.joy, (emotion_set_points.joy + circadian.joy_delta).clamp(0.0, 1.0), recovery_rates.joy * circadian.recovery_mult, dt, 0.008 * circadian.positive_noise_mult);
        emotion.sadness = fluctuate(emotion.sadness, (emotion_set_points.sadness + circadian.sadness_delta).clamp(0.0, 1.0), recovery_rates.sadness * circadian.recovery_mult, dt, 0.005 * circadian.negative_noise_mult);
        emotion.anger = fluctuate(emotion.anger, (emotion_set_points.anger + circadian.anger_delta).clamp(0.0, 1.0), recovery_rates.anger * circadian.recovery_mult, dt, 0.005 * circadian.negative_noise_mult);
        emotion.fear = fluctuate(emotion.fear, (emotion_set_points.fear + circadian.fear_delta).clamp(0.0, 1.0), recovery_rates.fear * circadian.recovery_mult, dt, 0.005 * circadian.negative_noise_mult);
        emotion.closeness = fluctuate(emotion.closeness, (emotion_set_points.closeness + circadian.closeness_delta).clamp(0.0, 1.0), recovery_rates.closeness * circadian.recovery_mult, dt, 0.007 * circadian.positive_noise_mult);
        emotion.loneliness = fluctuate(emotion.loneliness, (emotion_set_points.loneliness + circadian.loneliness_delta).clamp(0.0, 1.0), recovery_rates.loneliness * circadian.recovery_mult, dt, 0.006 * circadian.negative_noise_mult);
        emotion.curiosity = fluctuate(emotion.curiosity, (emotion_set_points.curiosity + circadian.curiosity_delta).clamp(0.0, 1.0), recovery_rates.curiosity * circadian.recovery_mult, dt, 0.008 * circadian.positive_noise_mult);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regress_towards_set_point() {
        // value=0.8, set_point=0.3, 应该向 0.3 回归
        let result = regress(0.8, 0.3, 0.1, 10.0);
        assert!(result < 0.8 && result > 0.3);
    }

    #[test]
    fn test_regress_at_set_point_no_change() {
        let result = regress(0.5, 0.5, 0.1, 10.0);
        assert!((result - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_need_decay_satisfied_slower() {
        // 已满足状态（value < set_point）回归更慢
        let mut sp = 0.5;
        let result = need_decay(0.2, &mut sp, 0.0, 0.1, 10.0);
        assert!(result > 0.2 && result < 0.5);
        // set_point 不应上升（已满足时不产生饥饿感）
        assert!((sp - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_need_decay_unsatisfied_hunger() {
        // 未满足状态（value > set_point）产生饥饿感，set_point 上升
        let mut sp = 0.5;
        let _result = need_decay(0.7, &mut sp, 0.0, 0.1, 10.0);
        assert!(sp > 0.5);
    }

    #[test]
    fn test_circadian_at_hour_anchors() {
        // 锚点处应返回对应时段的精确值
        let night = CircadianFactors::at_hour(2.5);
        assert!((night.loneliness_delta - 0.10).abs() < 1e-9);
        let morning = CircadianFactors::at_hour(8.5);
        assert!((morning.curiosity_delta - 0.08).abs() < 1e-9);
        let afternoon = CircadianFactors::at_hour(14.5);
        assert!((afternoon.recovery_mult - 1.2).abs() < 1e-9);
        let evening = CircadianFactors::at_hour(20.5);
        assert!((evening.closeness_delta - 0.08).abs() < 1e-9);
    }

    #[test]
    fn test_circadian_midpoint_interpolation() {
        // 早晨(8.5)与下午(14.5)的中点 11.5 应为两者平均值
        let mid = CircadianFactors::at_hour(11.5);
        let expected_joy = (CIRCADIAN_MORNING.joy_delta + CIRCADIAN_AFTERNOON.joy_delta) / 2.0;
        assert!((mid.joy_delta - expected_joy).abs() < 1e-9);
    }

    #[test]
    fn test_circadian_midnight_wraps() {
        // 0 点（午夜）应在深夜锚点(2.5)与傍晚锚点(20.5→26.5映射)之间
        let midnight = CircadianFactors::at_hour(0.0);
        // 0 点对应 h=24.0，在 [20.5, 26.5] 区间，t=(24-20.5)/6=0.583
        let t = (24.0 - 20.5) / (26.5 - 20.5);
        let expected_loneliness = CIRCADIAN_EVENING.loneliness_delta
            + (CIRCADIAN_NIGHT.loneliness_delta - CIRCADIAN_EVENING.loneliness_delta) * t;
        assert!((midnight.loneliness_delta - expected_loneliness).abs() < 1e-9);
    }

    #[test]
    fn test_offline_anger_decays_fast() {
        let mut emotion = super::super::emotion::EmotionState {
            anger: 0.8,
            ..Default::default()
        };
        let sp = EmotionSetPoints::default();
        let rr = RecoveryRateProfile::default();
        let circadian = CircadianFactors::at_hour(8.5);

        let anger_before = emotion.anger;
        HomeostasisEngine::apply_offline_compression(&mut emotion, &sp, &rr, 480.0, circadian);

        assert!(emotion.anger < anger_before);
        assert!(emotion.anger < 0.2);
    }

    #[test]
    fn test_offline_closeness_preserved() {
        let mut emotion = super::super::emotion::EmotionState {
            closeness: 0.7,
            ..Default::default()
        };
        let sp = EmotionSetPoints::default();
        let rr = RecoveryRateProfile::default();
        let circadian = CircadianFactors::at_hour(8.5);

        let closeness_before = emotion.closeness;
        HomeostasisEngine::apply_offline_compression(&mut emotion, &sp, &rr, 480.0, circadian);

        assert!((emotion.closeness - closeness_before).abs() < 0.1);
    }

    #[test]
    fn test_offline_loneliness_grows() {
        let mut emotion = super::super::emotion::EmotionState {
            loneliness: 0.15,
            ..Default::default()
        };
        let sp = EmotionSetPoints::default();
        let rr = RecoveryRateProfile::default();
        let circadian = CircadianFactors::at_hour(2.5);

        let loneliness_before = emotion.loneliness;
        HomeostasisEngine::apply_offline_compression(&mut emotion, &sp, &rr, 600.0, circadian);

        assert!(emotion.loneliness > loneliness_before);
    }
}
