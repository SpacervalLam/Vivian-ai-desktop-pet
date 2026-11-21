//! 打扰时机评分器
//!
//! 聚合多维度时间信号 → 适合打扰分数 0.0~1.0。
//! 维度：空闲时长 / 作息模型 / 时间段 / 冷却 / 频率。

use super::TickContext;

/// 权重系数
pub const WEIGHT_IDLE: f64 = 0.25;
pub const WEIGHT_SCHEDULE: f64 = 0.25;
pub const WEIGHT_TIME: f64 = 0.20;
pub const WEIGHT_COOLDOWN: f64 = 0.15;
pub const WEIGHT_FREQUENCY: f64 = 0.15;

/// 时间段分数（24h 每小时）
const TIME_SCORE_MAP: [f64; 24] = [
    0.1, 0.1, 0.1, 0.1, 0.2, 0.3, // 0~5: 深夜(低)
    0.4, 0.5, 0.6, 0.7, 0.8, 0.9, // 6~11: 上午(渐升)
    0.6, 0.4, 0.5, 0.6, 0.7, 0.8, // 12~17: 下午(午休降)
    0.9, 1.0, 1.0, 0.9, 0.8, 0.6, // 18~23: 晚(最高)
];

/// 冷却基准（秒）
const COOLDOWN_BASE: f64 = 300.0;
/// 每小时最大打扰次数
const MAX_INTERRUPTIONS_PER_HOUR: u32 = 6;

/// 打扰时机评分器
pub struct TimingJudger;

impl TimingJudger {
    /// 综合打扰分数（使用默认硬编码权重）
    ///
    /// `last_interruption_time`：上次打扰时间戳
    /// `interruption_count_hour`：本小时已打扰次数
    pub fn score(
        ctx: &TickContext,
        last_interruption_time: f64,
        interruption_count_hour: u32,
        hour: u32,
    ) -> f64 {
        let s = WEIGHT_IDLE * Self::idle_ratio(ctx)
            + WEIGHT_SCHEDULE * Self::schedule_score()
            + WEIGHT_TIME * Self::time_score(hour)
            + WEIGHT_COOLDOWN * Self::cooldown_score(ctx.now, last_interruption_time)
            + WEIGHT_FREQUENCY * Self::frequency_score(interruption_count_hour);
        s.clamp(0.0, 1.0)
    }

    /// 综合打扰分数（使用角色专属权重，策略 B）
    ///
    /// 不同角色对同一信号的敏感度不同：
    /// Vivian 偏重 idle（用户一空闲就忍不住），Nana 偏重 time-of-day（按作息规律关心人）。
    pub fn score_with_weights(
        ctx: &TickContext,
        last_interruption_time: f64,
        interruption_count_hour: u32,
        hour: u32,
        weights: &crate::character_behavior::TimingWeights,
    ) -> f64 {
        let s = weights.idle * Self::idle_ratio(ctx)
            + weights.schedule * Self::schedule_score()
            + weights.time * Self::time_score(hour)
            + weights.cooldown * Self::cooldown_score(ctx.now, last_interruption_time)
            + weights.frequency * Self::frequency_score(interruption_count_hour);
        s.clamp(0.0, 1.0)
    }

    /// 是否适合打扰（基于阈值）
    pub fn should_interrupt(
        ctx: &TickContext,
        last_interruption_time: f64,
        interruption_count_hour: u32,
        hour: u32,
        threshold: f64,
    ) -> bool {
        Self::score(ctx, last_interruption_time, interruption_count_hour, hour) >= threshold
    }

    /// 详细分解分数构成
    pub fn explain(
        ctx: &TickContext,
        last_interruption_time: f64,
        interruption_count_hour: u32,
        hour: u32,
    ) -> serde_json::Value {
        serde_json::json!({
            "total": Self::score(ctx, last_interruption_time, interruption_count_hour, hour),
            "idle_ratio": Self::idle_ratio(ctx),
            "schedule_score": Self::schedule_score(),
            "time_score": Self::time_score(hour),
            "cooldown_score": Self::cooldown_score(ctx.now, last_interruption_time),
            "frequency_score": Self::frequency_score(interruption_count_hour),
        })
    }

    /// 空闲时长评分：太活跃→低分，有适当空闲→高分
    fn idle_ratio(ctx: &TickContext) -> f64 {
        // 用 idle_seconds 推断活动等级
        let secs = ctx.idle_seconds;
        if secs < 60.0 {
            // very_active
            0.1
        } else if secs < 300.0 {
            // active
            0.3
        } else if secs < 600.0 {
            // normal
            0.7
        } else if secs < 1800.0 {
            // idle
            0.9
        } else {
            // very_idle
            0.6
        }
    }

    /// 作息模型评分：Rust 版无作息模型，返回中性 0.7（不盲目压分）
    fn schedule_score() -> f64 {
        0.7
    }

    /// 时间段评分
    fn time_score(hour: u32) -> f64 {
        TIME_SCORE_MAP[(hour as usize) % 24]
    }

    /// 冷却评分：刚打扰过→低分
    fn cooldown_score(now: f64, last_interruption_time: f64) -> f64 {
        let elapsed = (now - last_interruption_time).max(0.0);
        if elapsed >= COOLDOWN_BASE {
            1.0
        } else {
            elapsed / COOLDOWN_BASE
        }
    }

    /// 频率评分：近 1 小时打扰太多→压分
    fn frequency_score(interruption_count_hour: u32) -> f64 {
        if interruption_count_hour >= MAX_INTERRUPTIONS_PER_HOUR {
            0.0
        } else {
            1.0 - (interruption_count_hour as f64 / MAX_INTERRUPTIONS_PER_HOUR as f64) * 0.8
        }
    }
}
