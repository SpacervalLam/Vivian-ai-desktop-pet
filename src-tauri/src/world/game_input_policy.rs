//! Bounded input-rate estimates, never a keystroke history or a claim about game outcomes.
use serde::Serialize;

const BUCKET_MS: u64 = 100;
const WINDOW_MS: u64 = 2000;
const COOLDOWN_MS: u64 = 3000;
const BUCKETS: usize = (WINDOW_MS / BUCKET_MS) as usize;

#[derive(Clone, Copy, Default)]
struct Bucket { epoch: Option<u64>, keys: u32, clicks: u32, wheel: u32, motion: u64 }

#[derive(Debug, Clone, Default, Serialize)]
pub struct InputActivity {
    pub available: bool,
    pub intense: bool,
    pub key_presses: u32,
    pub mouse_clicks: u32,
    pub wheel_steps: u32,
    pub mouse_motion_units: u64,
    pub held_keys: u16,
    pub held_mouse_buttons: u16,
    pub since_input_ms: Option<u64>,
    pub cooldown_remaining_ms: u64,
}
impl InputActivity {
    pub fn allows_comment(&self) -> bool {
        self.available && !self.intense && self.since_input_ms.is_some_and(|idle| (1500..90_000).contains(&idle))
    }
}

pub struct InputIntensity {
    buckets: [Bucket; BUCKETS],
    // Only the currently held state is retained to filter OS auto-repeat; never serialized.
    keys: [bool; 256],
    held_keys: u16,
    held_buttons: u16,
    last_input: Option<u64>,
    last_intense: Option<u64>,
    last_evaluation: Option<u64>,
}
impl Default for InputIntensity {
    fn default() -> Self {
        Self { buckets: [Bucket::default(); BUCKETS], keys: [false; 256], held_keys: 0,
            held_buttons: 0, last_input: None, last_intense: None, last_evaluation: None }
    }
}
impl InputIntensity {
    pub fn reset(&mut self, now: u64) { *self = Self { last_input: Some(now), ..Default::default() }; }

    fn bucket(&mut self, now: u64) -> &mut Bucket {
        let epoch = now / BUCKET_MS;
        let bucket = &mut self.buckets[epoch as usize % BUCKETS];
        if bucket.epoch != Some(epoch) { *bucket = Bucket { epoch: Some(epoch), ..Default::default() }; }
        bucket
    }

    pub fn key(&mut self, now: u64, key: u16, down: bool) {
        if key >= 255 { return; }
        let held = &mut self.keys[key as usize];
        if *held == down { return; } // A held key is not hundreds of new key presses.
        *held = down;
        if down { self.held_keys += 1; self.bucket(now).keys += 1; }
        else { self.held_keys = self.held_keys.saturating_sub(1); }
        self.last_input = Some(now);
        self.evaluate(now, false);
    }

    pub fn mouse(&mut self, now: u64, dx: i32, dy: i32, flags: u16, wheel: u32) {
        let mut clicks = 0;
        for button in 0..5 {
            let down = 1u16 << (button * 2);
            let up = down << 1;
            let held = 1u16 << button;
            if flags & down != 0 && self.held_buttons & held == 0 { clicks += 1; self.held_buttons |= held; }
            if flags & up != 0 { self.held_buttons &= !held; }
        }
        let motion = u64::from(dx.unsigned_abs()) + u64::from(dy.unsigned_abs());
        if motion == 0 && flags == 0 && wheel == 0 { return; }
        let bucket = self.bucket(now);
        bucket.clicks = bucket.clicks.saturating_add(clicks);
        bucket.wheel = bucket.wheel.saturating_add(wheel);
        bucket.motion = bucket.motion.saturating_add(motion);
        self.last_input = Some(now);
        self.evaluate(now, false);
    }

    fn totals(&self, now: u64) -> (Bucket, usize) {
        let epoch = now / BUCKET_MS;
        let mut total = Bucket::default();
        let mut motion_bins = 0;
        for bucket in &self.buckets {
            if !bucket.epoch.is_some_and(|at| at <= epoch && epoch - at < BUCKETS as u64) { continue; }
            total.keys = total.keys.saturating_add(bucket.keys);
            total.clicks = total.clicks.saturating_add(bucket.clicks);
            total.wheel = total.wheel.saturating_add(bucket.wheel);
            total.motion = total.motion.saturating_add(bucket.motion);
            if bucket.motion >= 40 { motion_bins += 1; }
        }
        (total, motion_bins)
    }

    fn evaluate(&mut self, now: u64, force: bool) {
        if !force && self.last_evaluation.is_some_and(|at| now.saturating_sub(at) < BUCKET_MS) { return; }
        self.last_evaluation = Some(now);
        let (total, motion_bins) = self.totals(now);
        let busy = total.keys >= 10 || total.clicks >= 8 || (total.keys >= 4 && total.clicks >= 3)
            || (total.motion >= 2400 && motion_bins >= 4 && (self.held_keys > 0 || self.held_buttons != 0 || total.clicks >= 2))
            || (total.motion >= 6000 && motion_bins >= 8);
        // Polling a quiet source must never keep extending the cooldown.
        if busy { self.last_intense = self.last_input; }
    }

    pub fn snapshot(&mut self, now: u64, available: bool) -> InputActivity {
        if !available { return InputActivity::default(); }
        self.evaluate(now, true);
        let (total, _) = self.totals(now);
        let cooldown = self.last_intense.map(|at| COOLDOWN_MS.saturating_sub(now.saturating_sub(at))).unwrap_or(0);
        InputActivity { available, intense: cooldown > 0, key_presses: total.keys, mouse_clicks: total.clicks,
            wheel_steps: total.wheel, mouse_motion_units: total.motion, held_keys: self.held_keys,
            held_mouse_buttons: self.held_buttons.count_ones() as u16,
            since_input_ms: self.last_input.map(|at| now.saturating_sub(at)), cooldown_remaining_ms: cooldown }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rapid_keyboard_operations_enter_intense_and_brief_pauses_do_not_interrupt() {
        let mut input = InputIntensity::default(); input.reset(0);
        for i in 0..12 { input.key(i * 100, 65, true); input.key(i * 100 + 20, 65, false); }
        assert!(input.snapshot(1200, true).intense);
        assert!(!input.snapshot(2900, true).allows_comment(), "a 1.8s battle pause still belongs to the burst");
        assert!(input.snapshot(4500, true).allows_comment());
        assert!(!input.snapshot(100_000, true).allows_comment(), "long absence is not an invitation to speak");
    }
    #[test]
    fn auto_repeat_and_stationary_mouse_packets_do_not_fake_fast_actions() {
        let mut input = InputIntensity::default(); input.reset(0);
        for i in 0..1000 { input.key(i, 65, true); input.mouse(i, 0, 0, 0, 0); }
        let sample = input.snapshot(1100, true);
        assert_eq!(sample.key_presses, 1); assert_eq!(sample.held_keys, 1); assert!(!sample.intense);
    }
    #[test]
    fn relative_camera_motion_detects_play_even_when_the_os_cursor_is_locked() {
        let mut input = InputIntensity::default(); input.reset(0); input.key(0, 87, true);
        for i in 0..10 { input.mouse(i * 100, 300, -30, 0, 0); }
        assert!(input.snapshot(1000, true).intense);
        assert_eq!(input.snapshot(1000, true).mouse_motion_units, 3300);
    }
    #[test]
    fn frequent_mouse_clicks_are_counted_as_presses_not_releases() {
        let mut input = InputIntensity::default(); input.reset(0);
        for i in 0..9 { input.mouse(i * 100, 0, 0, 1, 0); input.mouse(i * 100 + 20, 0, 0, 2, 0); }
        let sample = input.snapshot(1000, true);
        assert!(sample.intense); assert_eq!(sample.mouse_clicks, 9); assert_eq!(sample.held_mouse_buttons, 0);
    }
    #[test]
    fn scope_changes_clear_input_state_and_unavailable_is_not_calm() {
        let mut input = InputIntensity::default(); input.key(0, 87, true);
        input.reset(1000);
        let sample = input.snapshot(3000, true);
        assert_eq!(sample.held_keys, 0); assert_eq!(sample.key_presses, 0);
        assert!(sample.allows_comment());
        assert!(!input.snapshot(3000, false).allows_comment());
        let encoded = serde_json::to_value(sample).unwrap();
        assert!(encoded.get("keys").is_none(), "only aggregates can leave the input observer");
    }
    #[test]
    fn a_reply_started_in_a_lull_is_suppressed_if_operations_resume_before_delivery() {
        let mut input = InputIntensity::default(); input.reset(0);
        assert!(input.snapshot(2000, true).allows_comment());
        for i in 0..12 { input.key(2010 + i * 25, 65, true); input.key(2020 + i * 25, 65, false); }
        let before_delivery = input.snapshot(2320, true);
        assert!(before_delivery.intense);
        assert!(!before_delivery.allows_comment());
    }
    #[test]
    fn high_polling_rate_is_bounded_and_large_motion_cannot_overflow() {
        let mut input = InputIntensity::default();
        for i in 0..80_000 { input.mouse(i / 8, 1, 0, 0, 0); }
        assert!(input.snapshot(10_000, true).mouse_motion_units <= 16_000);
        input.mouse(10_000, i32::MIN, i32::MAX, 0, 0);
        assert!(input.snapshot(10_000, true).mouse_motion_units > u32::MAX as u64);
        assert_eq!(input.buckets.len(), 20);
    }
}
