//! Local companion feedback policy. No model calls, input control or window operations.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionConfig {
    pub hourly_chime: bool,
    pub focus_mode: bool,
    pub quiet_hours: bool,
    pub quiet_start: u32,
    pub quiet_end: u32,
    pub game_quiet: bool,
    pub game_processes: Vec<String>,
    pub sound: bool,
    pub resource_feedback: bool,
    pub network_feedback: bool,
    pub clipboard_hint: bool,
    pub weather_feedback: bool,
    pub music_feedback: bool,
    pub shortcuts: Vec<DesktopShortcut>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopShortcut { pub id: String, pub name: String, pub kind: String, pub target: String }
impl Default for CompanionConfig {
    fn default() -> Self {
        Self { hourly_chime: false, focus_mode: false, quiet_hours: false,
            quiet_start: 23, quiet_end: 7, game_quiet: true,
            game_processes: vec!["League of Legends.exe".into(), "GenshinImpact.exe".into()],
            sound: true, resource_feedback: true, network_feedback: false, clipboard_hint: true, weather_feedback: false,
            music_feedback: false, shortcuts: vec![
                DesktopShortcut { id: "github".into(), name: "GitHub".into(), kind: "website".into(), target: "https://github.com".into() },
                DesktopShortcut { id: "chatgpt".into(), name: "ChatGPT".into(), kind: "website".into(), target: "https://chatgpt.com".into() },
            ] }
    }
}
impl CompanionConfig {
    pub fn quiet_reason(&self, hour: u32, process: &str) -> Option<&'static str> {
        if self.focus_mode { return Some("focus"); }
        let start = self.quiet_start.min(23);
        let end = self.quiet_end.min(23);
        let inside = if start < end { hour >= start && hour < end }
            else if start > end { hour >= start || hour < end } else { false };
        if self.quiet_hours && inside { return Some("quiet_hours"); }
        if self.game_quiet && !process.is_empty() && self.game_processes.iter()
            .any(|p| !p.trim().is_empty() && p.trim().eq_ignore_ascii_case(process)) {
            return Some("game");
        }
        None
    }
}

/// Track only an OS revision, never the clipboard contents. Zero means unavailable.
#[derive(Default)]
pub struct ClipboardChanges { last_sequence: Option<u32> }
impl ClipboardChanges {
    pub fn observe(&mut self, sequence: u32) -> bool {
        if sequence == 0 { return false; }
        self.last_sequence.replace(sequence).is_some_and(|old| old != sequence)
    }
}

/// Hysteresis plus sustained load; gaps in sampling never count as sustained pressure.
#[derive(Default)]
pub struct PressureState { since: Option<i64>, last_sample: Option<i64>, pub active: bool }
impl PressureState {
    pub fn sample(&mut self, now: i64, value: Option<f32>, enter: f32, exit: f32) -> bool {
        if self.last_sample.is_some_and(|last| now < last || now - last > 10) { self.since = None; }
        self.last_sample = Some(now);
        let Some(value) = value.filter(|v| v.is_finite()) else { self.since = None; return false; };
        if value < exit { self.active = false; self.since = None; }
        if value >= enter && !self.active {
            let since = *self.since.get_or_insert(now);
            if now - since >= 20 { self.active = true; return true; }
        } else if !self.active { self.since = None; }
        false
    }
}

#[derive(Default)]
pub struct HourlyClock { last_hour: Option<i64> }
impl HourlyClock {
    /// First observation establishes a baseline. Never replay missed or backwards hours.
    pub fn observe(&mut self, unix_seconds: i64, local_minute: u32, local_second: u32) -> bool {
        let hour = unix_seconds - i64::from(local_minute) * 60 - i64::from(local_second);
        let previous = self.last_hour.replace(self.last_hour.map_or(hour, |old| old.max(hour)));
        previous.is_some_and(|old| hour > old) && local_minute == 0 && local_second < 5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_hints_require_a_change_and_ignore_unavailable_samples() {
        assert!(CompanionConfig::default().clipboard_hint);
        let mut changes = ClipboardChanges::default();
        assert!(!changes.observe(0));
        assert!(!changes.observe(10)); // Startup establishes a baseline, without prompting.
        assert!(!changes.observe(10));
        assert!(!changes.observe(0));
        assert!(!changes.observe(10));
        assert!(changes.observe(11));
        assert!(!changes.observe(11));
        assert!(changes.observe(1)); // Sequence wrap is still a change.
    }
    #[test]
    fn quiet_windows_and_only_the_actual_foreground_game() {
        let mut c = CompanionConfig { quiet_hours: true, ..Default::default() };
        assert_eq!(c.quiet_reason(23, ""), Some("quiet_hours"));
        assert_eq!(c.quiet_reason(6, ""), Some("quiet_hours"));
        assert_eq!(c.quiet_reason(7, ""), None);
        assert_eq!(c.quiet_reason(12, "genshinimpact.EXE"), Some("game"));
        assert_eq!(c.quiet_reason(12, "GenshinImpact.exe.bak"), None);
        c.focus_mode = true;
        assert_eq!(c.quiet_reason(12, ""), Some("focus"));
        c.focus_mode = false;
        c.quiet_start = 9; c.quiet_end = 17;
        assert_eq!(c.quiet_reason(9, ""), Some("quiet_hours"));
        assert_eq!(c.quiet_reason(17, ""), None);
        c.quiet_start = 17;
        assert_eq!(c.quiet_reason(17, ""), None);
    }
    #[test]
    fn hourly_dedup_suspend_and_clock_changes() {
        let mut c = HourlyClock::default();
        assert!(!c.observe(3599, 59, 59));
        assert!(c.observe(3600, 0, 0));
        assert!(!c.observe(3601, 0, 1));
        assert!(!c.observe(10860, 1, 0));
        assert!(!c.observe(3600, 0, 0));
        assert!(c.observe(14400, 0, 0));
        let mut offset = HourlyClock::default();
        assert!(!offset.observe(1799, 59, 59));
        assert!(offset.observe(1800, 0, 0)); // Half-hour time zone boundary.
    }
    #[test]
    fn pressure_is_sustained_and_recovers_without_flapping() {
        let mut p = PressureState::default();
        for now in 0..20 { assert!(!p.sample(now, Some(90.0), 85.0, 70.0)); }
        assert!(p.sample(20, Some(90.0), 85.0, 70.0));
        assert!(!p.sample(21, Some(80.0), 85.0, 70.0));
        assert!(p.active);
        p.sample(22, Some(60.0), 85.0, 70.0);
        assert!(!p.active);
        p.sample(23, Some(90.0), 85.0, 70.0);
        assert!(!p.sample(100, Some(90.0), 85.0, 70.0));
        assert!(!p.sample(101, Some(f32::NAN), 85.0, 70.0));
    }
}
