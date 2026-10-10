//! Release velocity from recent physical cursor samples, including the release point.
use std::collections::VecDeque;
use std::time::Instant;

pub const FLING_SAMPLE_WINDOW_MS: u64 = 120;
pub const FLING_MIN_VELOCITY: f64 = 0.5;
pub const FLING_MAX_VELOCITY: f64 = 4.0;
const MIN_SPAN_MS: f64 = 40.0;
const MAX_SAMPLE_AGE_MS: u128 = 100;

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
}
