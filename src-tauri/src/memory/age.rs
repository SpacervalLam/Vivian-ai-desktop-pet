//! 记忆热度评分与陈旧度提示。
//!
//! 热度公式：H = α·visit_count + β·length + γ·R_recency
//! - α：访问次数权重
//! - β：内容长度权重（每字符）
//! - γ：近期性权重
//! - R_recency = exp(-delta_hours / 24)，取值 (0, 1]
//!
//! 陈旧度提示：超过 1 天的记忆返回提示文本，由 pipeline 注入到记忆上下文，
//! 让 LLM 自行判断是否仍然有效，而非自动过期。

use super::types::{current_timestamp, MemoryItem};

/// 访问次数权重
const ALPHA: f64 = 0.5;
/// 内容长度权重（每字符）
const BETA: f64 = 0.01;
/// 近期性权重
const GAMMA: f64 = 0.5;
/// 半衰期常数：24 小时后 R_recency ≈ 1/e ≈ 0.368
const RECENCY_HALFLIFE_HOURS: f64 = 24.0;

/// 计算记忆热度分数。
///
/// - `visit_count`：累计被检索命中次数
/// - `length`：内容字符数
/// - `last_visit_at`：最近一次命中时间戳（秒）；0 表示从未被命中
/// - `now`：当前时间戳（秒）
///
/// 从未被命中的记忆（`last_visit_at <= 0`）按"刚创建"处理，`R_recency = 1.0`，
/// 随时间自然衰减。原逻辑返回 0.0 导致新创建的 SessionSummary 热度极低、
/// 永远无法达到 Stage 2 升级阈值。
///
/// 内容长度采用对数阻尼 `ln(1 + length)`，避免长记忆（500 字）线性碾压
/// 短记忆（10 字），让长度因子快速饱和、访问次数和近期性主导排序。
pub fn compute_heat_score(
    visit_count: u32,
    length: usize,
    last_visit_at: f64,
    now: f64,
) -> f64 {
    let n = visit_count as f64;
    // 对数阻尼：ln(1 + length)，使长度贡献快速饱和（10 字≈2.4, 100 字≈4.6, 500 字≈6.2）
    let l = (1.0 + length as f64).ln();

    let r_recency = if last_visit_at <= 0.0 {
        // 从未被命中：视为新鲜（R=1.0），随时间衰减
        1.0
    } else {
        let delta_hours = ((now - last_visit_at).max(0.0)) / 3600.0;
        (-delta_hours / RECENCY_HALFLIFE_HOURS).exp()
    };

    ALPHA * n + BETA * l + GAMMA * r_recency
}

/// 返回记忆的时间感知标签（用于 prompt 注入，让 LLM 直观感知记忆的新旧）。
///
/// - 1 小时内：`刚刚`
/// - 1 天内：`N小时前`
/// - 超过 1 天：`N天前`
///
/// `timestamp` 为记忆创建时间戳（秒），`now` 为当前时间戳（秒）。
pub fn staleness_text(timestamp: f64, now: f64) -> Option<String> {
    if timestamp <= 0.0 || now <= 0.0 {
        return None;
    }
    let age_secs = (now - timestamp).max(0.0);
    let hours = (age_secs / 3600.0) as i64;
    let days = day_diff(timestamp, now);

    if hours < 1 {
        Some("刚刚".to_string())
    } else if days < 1 {
        Some(format!("{}小时前", hours))
    } else {
        Some(format!("{}天前", days))
    }
}

/// 计算两个时间戳之间的"自然天"差值（按 UTC 日期边界）。
fn day_diff(timestamp: f64, now: f64) -> i64 {
    use chrono::{DateTime, Utc};

    let ts_secs = timestamp.max(0.0) as i64;
    let now_secs = now.max(0.0) as i64;

    let ts_date = DateTime::<Utc>::from_timestamp(ts_secs, 0)
        .map(|dt| dt.date_naive())
        .or_else(|| DateTime::<Utc>::from_timestamp(0, 0).map(|dt| dt.date_naive()));
    let now_date = DateTime::<Utc>::from_timestamp(now_secs, 0)
        .map(|dt| dt.date_naive())
        .or_else(|| DateTime::<Utc>::from_timestamp(0, 0).map(|dt| dt.date_naive()));

    match (ts_date, now_date) {
        (Some(a), Some(b)) => (b - a).num_days().max(0),
        _ => 0,
    }
}

/// 检索命中后更新记忆热度：增加访问次数、刷新最近命中时间、重算热度分数。
///
/// 返回更新后的热度分数。调用方需负责持久化（如 `MemoryManager::bump_visits`）。
pub fn bump_visit(item: &mut MemoryItem, now: f64) -> f64 {
    item.visit_count = item.visit_count.saturating_add(1);
    item.last_visit_at = now;
    let length = item.content.chars().count();
    let heat = compute_heat_score(item.visit_count, length, item.last_visit_at, now);
    item.heat_score = heat;
    heat
}

/// 初始化记忆热度（写入时调用）。
///
/// 未被检索过的记忆 visit_count=0、last_visit_at=0，
/// 仅基于内容长度给出一个基础热度，便于排序时区分长短记忆。
pub fn init_heat(item: &mut MemoryItem) {
    let length = item.content.chars().count();
    let now = current_timestamp();
    item.heat_score = compute_heat_score(item.visit_count, length, item.last_visit_at, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> f64 {
        current_timestamp()
    }

    #[test]
    fn heat_score_zero_visit_is_low() {
        let now = now_secs();
        // 从未被命中：R_recency=1.0（视为新鲜），热度 = α·0 + β·ln(1+10) + γ·1
        let h = compute_heat_score(0, 10, 0.0, now);
        let expected = BETA * (1.0 + 10.0_f64).ln() + GAMMA * 1.0;
        assert!((h - expected).abs() < 1e-9, "未命中记忆应含长度+新鲜度: {h}");
    }

    #[test]
    fn heat_score_increases_with_visit() {
        let now = now_secs();
        let h0 = compute_heat_score(0, 10, now, now);
        let h1 = compute_heat_score(1, 10, now, now);
        let h3 = compute_heat_score(3, 10, now, now);
        assert!(h1 > h0, "visit_count=1 应高于 0");
        assert!(h3 > h1, "visit_count=3 应高于 1");
    }

    #[test]
    fn heat_score_recency_decays() {
        let now = now_secs();
        let recent = compute_heat_score(1, 10, now, now);
        let old = compute_heat_score(1, 10, now - 72.0 * 3600.0, now);
        assert!(recent > old, "近期命中应高于旧命中");
    }

    #[test]
    fn staleness_recent_returns_none() {
        let now = now_secs();
        assert_eq!(staleness_text(now, now), None);
        assert_eq!(staleness_text(now - 3600.0, now), None);
    }

    #[test]
    fn staleness_old_returns_hint() {
        let now = now_secs();
        let ts = now - 3.0 * 24.0 * 3600.0;
        let hint = staleness_text(ts, now).expect("3 天前应有提示");
        assert!(hint.contains("3 天前"), "提示文本应含天数: {hint}");
    }

    #[test]
    fn staleness_one_day_boundary() {
        let now = now_secs();
        let ts = now - 25.0 * 3600.0;
        let hint = staleness_text(ts, now).expect("25 小时前应触发提示");
        assert!(hint.contains("1 天前"), "跨天边界应记为 1 天: {hint}");
    }

    #[test]
    fn bump_visit_increments_and_updates_heat() {
        let now = now_secs();
        let mut item = MemoryItem::new("测试内容".to_string(), super::super::types::Granularity::Turn, 0.5);
        item.heat_score = 0.0;
        item.last_visit_at = 0.0;

        let h1 = bump_visit(&mut item, now);
        assert_eq!(item.visit_count, 1);
        assert!((item.last_visit_at - now).abs() < 1e-3);
        assert!(h1 > 0.0, "命中后热度应大于 0");

        let h2 = bump_visit(&mut item, now);
        assert_eq!(item.visit_count, 2);
        assert!(h2 > h1, "二次命中应进一步提升热度");
    }

    #[test]
    fn day_diff_handles_zero_timestamp() {
        let now = now_secs();
        // 时间戳为 0（1970-01-01）应返回一个很大的正数，不 panic
        let d = day_diff(0.0, now);
        assert!(d > 0);
    }
}
