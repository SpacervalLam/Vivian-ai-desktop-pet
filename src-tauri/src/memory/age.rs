//! 记忆热度评分。
//!
//! 热度公式：H = α·visit_count + β·length + γ·R_recency
//! - α：访问次数权重
//! - β：内容长度权重（每字符）
//! - γ：近期性权重
//! - R_recency = exp(-delta_hours / 24)，取值 (0, 1]
//!
//! 时间的**展示**不在这里：一旦某条记忆要进 prompt，统一走
//! `utils::prompt_time::format_relative_age_at`（本地日界 + 三语）。
//! 本模块只回答"这条记忆该排多前"，不回答"它看起来是多久以前"。

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
}
