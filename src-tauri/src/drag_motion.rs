//! Release velocity from recent physical cursor samples, including the release point.
use std::collections::VecDeque;
use std::time::Instant;

pub const FLING_SAMPLE_WINDOW_MS: u64 = 120;
pub const FLING_MIN_VELOCITY: f64 = 0.5;
pub const FLING_MAX_VELOCITY: f64 = 4.0;
const MIN_SPAN_MS: f64 = 40.0;
const MAX_SAMPLE_AGE_MS: u128 = 100;

/// Physical px/ms, only when an upward throw crosses the top from inside the bounds.
pub fn ceiling_impact(from: f64, next: f64, vy: f64, min: f64, max: f64) -> f64 {
    if from > min && from < max && next < min && vy < 0.0 && vy.is_finite() { -vy } else { 0.0 }
}

/// 甩飞撞边触发晕眩的最小法向撞击速度（逻辑像素/ms），轻贴边缘不触发
const FLING_BOUNCE_MIN_IMPACT: f64 = 0.6;
/// 甩飞撞边晕眩的基准时长（ms）
const FLING_BOUNCE_DIZZY_BASE_MS: u64 = 1400;
/// 甩飞撞边晕眩的时长上限（ms）
const FLING_BOUNCE_DIZZY_MAX_MS: u64 = 2400;

/// 撞边晕眩时长：撞击越重晕得越久；轻贴边缘（法向速度低于阈值）返回 `None` 不触发。
///
/// 注意：本函数只做「给定时长」的换算，**是否构成撞击**由调用方判定。
/// 调用方只把「甩飞速度把窗口从界内推出界」的撞击记进 `impact`，
/// 智能避让等外部移动把桌宠摆到墙边不算撞击（`impact` 保持 0，这里自然返回 `None`）。
pub fn bounce_dizzy_ms(impact: f64) -> Option<u64> {
    // 用 `!(x >= min)` 而非 `x < min`，顺带把 NaN 归入「不触发」
    if !(impact >= FLING_BOUNCE_MIN_IMPACT) {
        return None;
    }
    let extra = ((impact - FLING_BOUNCE_MIN_IMPACT) * 1200.0).min(
        (FLING_BOUNCE_DIZZY_MAX_MS - FLING_BOUNCE_DIZZY_BASE_MS) as f64,
    );
    Some(FLING_BOUNCE_DIZZY_BASE_MS + extra as u64)
}

pub fn release_velocity(
    samples: &VecDeque<(Instant, f64, f64)>,
    release: (Instant, f64, f64),
    dpi_scale: f64,
) -> Option<(f64, f64)> {
    let last = samples.back()?;
    // A stalled tracking thread must not throw with an old velocity when it resumes.
    if release.0.checked_duration_since(last.0)?.as_millis() > MAX_SAMPLE_AGE_MS {
        return None;
    }
    let first = samples.iter().find(|sample| {
        release.0.checked_duration_since(sample.0)
            .is_some_and(|age| age.as_millis() <= FLING_SAMPLE_WINDOW_MS as u128)
    })?;
    let span_ms = release.0.checked_duration_since(first.0)?.as_secs_f64() * 1000.0;
    if span_ms < MIN_SPAN_MS || !dpi_scale.is_finite() || dpi_scale <= 0.0 {
        return None;
    }
    let vx = (release.1 - first.1) / span_ms;
    let vy = (release.2 - first.2) / span_ms;
    let speed = vx.hypot(vy);
    let scale = dpi_scale.max(0.75);
    if !speed.is_finite() || speed < FLING_MIN_VELOCITY * scale {
        return None;
    }
    let transfer = (FLING_MAX_VELOCITY * scale / speed).min(1.0);
    Some((vx * transfer, vy * transfer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_upward_top_crossings_count_as_ceiling_impacts() {
        assert_eq!(ceiling_impact(100.0, -10.0, -1.2, 0.0, 600.0), 1.2);
        for (from, next, speed) in [(500.0, 610.0, 2.0), (100.0, 90.0, -2.0),
            (0.0, -10.0, -2.0), (-20.0, -10.0, -2.0), (100.0, -10.0, 2.0),
            (100.0, -10.0, f64::NAN)] {
            assert_eq!(ceiling_impact(from, next, speed, 0.0, 600.0), 0.0);
        }
        assert_eq!(ceiling_impact(-500.0, -610.0, -1.2, -600.0, 0.0), 1.2);
    }

    fn at(base: Instant, ms: u64, x: f64, y: f64) -> (Instant, f64, f64) {
        (base + Duration::from_millis(ms), x, y)
    }

    #[test]
    fn release_includes_the_final_flick_and_preserves_direction() {
        let base = Instant::now();
        let samples = VecDeque::from([at(base, 0, 0.0, 0.0), at(base, 60, 0.0, 0.0)]);
        assert_eq!(release_velocity(&samples, at(base, 120, -120.0, 60.0), 1.0), Some((-1.0, 0.5)));
    }

    #[test]
    fn holding_before_release_does_not_reuse_earlier_speed() {
        let base = Instant::now();
        let samples = VecDeque::from([at(base, 0, 0.0, 0.0), at(base, 60, 100.0, 0.0), at(base, 120, 100.0, 0.0)]);
        assert_eq!(release_velocity(&samples, at(base, 180, 100.0, 0.0), 1.0), None);
        assert_eq!(release_velocity(&samples, at(base, 300, 300.0, 0.0), 1.0), None, "stale tracking sample");
    }

    #[test]
    fn equivalent_logical_gestures_have_equivalent_threshold_and_cap() {
        let base = Instant::now();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let samples = VecDeque::from([at(base, 0, -800.0, 50.0)]);
            assert_eq!(release_velocity(&samples, at(base, 60, -800.0 + 24.0 * scale, 50.0), scale), None);
            let (vx, vy) = release_velocity(&samples, at(base, 60, -800.0 + 600.0 * scale, 50.0 + 800.0 * scale), scale).unwrap();
            assert!((vx.hypot(vy) - FLING_MAX_VELOCITY * scale).abs() < 1e-9);
            assert!((vx / vy - 0.75).abs() < 1e-9);
        }
    }

    #[test]
    fn invalid_or_insufficient_samples_do_not_throw() {
        let base = Instant::now();
        let samples = VecDeque::from([at(base, 60, 0.0, 0.0)]);
        assert_eq!(release_velocity(&VecDeque::new(), at(base, 120, 100.0, 0.0), 1.0), None);
        assert_eq!(release_velocity(&samples, at(base, 59, 100.0, 0.0), 1.0), None);
        assert_eq!(release_velocity(&samples, at(base, 80, 100.0, 0.0), 1.0), None);
        assert_eq!(release_velocity(&samples, at(base, 120, f64::NAN, 0.0), 1.0), None);
        assert_eq!(release_velocity(&samples, at(base, 120, 100.0, 0.0), 0.0), None);
    }
    #[test]
    fn bounce_dizzy_skips_gentle_touch() {
        assert_eq!(bounce_dizzy_ms(0.0), None);
        assert_eq!(bounce_dizzy_ms(0.24), None);
        assert_eq!(bounce_dizzy_ms(-1.0), None);
        assert_eq!(bounce_dizzy_ms(f64::NAN), None);
    }
    #[test]
    fn bounce_dizzy_grows_with_impact_and_is_capped() {
        let base = bounce_dizzy_ms(FLING_BOUNCE_MIN_IMPACT).unwrap();
        assert_eq!(base, FLING_BOUNCE_DIZZY_BASE_MS);

        let medium = bounce_dizzy_ms(0.75).unwrap();
        assert!(medium > base, "撞击更重应当晕更久: {medium} <= {base}");

        // 极端撞击（甩飞初速度上限 4.0 px/ms）封顶
        let hardest = bounce_dizzy_ms(FLING_MAX_VELOCITY).unwrap();
        assert_eq!(hardest, FLING_BOUNCE_DIZZY_MAX_MS);
    }
}
