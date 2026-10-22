//! Local companion feedback policy. No model calls, input control or window operations.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionConfig {
    pub do_not_disturb: bool,
    pub hourly_chime: bool,
    pub focus_mode: bool,
    pub quiet_hours: bool,
    pub quiet_start: u32,
    pub quiet_end: u32,
    pub game_quiet: bool,
    pub game_companion: bool,
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
pub struct DesktopShortcut {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub target: String,
    /// 分组名；空串即未分组。旧配置没有这个字段，`default` 保证反序列化不报错。
    #[serde(default)]
    pub group: String,
}
impl Default for CompanionConfig {
    fn default() -> Self {
        Self { do_not_disturb: false, hourly_chime: false, focus_mode: false, quiet_hours: false,
            quiet_start: 23, quiet_end: 7, game_quiet: true, game_companion: false,
            game_processes: vec!["League of Legends.exe".into(), "GenshinImpact.exe".into()],
            sound: true, resource_feedback: true, network_feedback: false, clipboard_hint: true, weather_feedback: false,
            music_feedback: false, shortcuts: vec![
                DesktopShortcut { id: "github".into(), name: "GitHub".into(), kind: "website".into(), target: "https://github.com".into(), group: "开发".into() },
                DesktopShortcut { id: "chatgpt".into(), name: "ChatGPT".into(), kind: "website".into(), target: "https://chatgpt.com".into(), group: "AI".into() },
            ] }
    }
}
impl CompanionConfig {
    pub fn remember_game(&mut self, process: &str) -> bool {
        let process = crate::game_policy::normalize_process(process);
        if !self.game_companion || process.is_empty() || crate::game_policy::excluded(&process)
            || self.game_processes.iter().any(|p| crate::game_policy::same_process(p, &process)) { return false; }
        self.game_processes.push(process);
        true
    }

    pub fn quiet_reason(&self, hour: u32, process: &str) -> Option<&'static str> {
        if self.focus_mode { return Some("focus"); }
        let start = self.quiet_start.min(23);
        let end = self.quiet_end.min(23);
        let inside = if start < end { hour >= start && hour < end }
            else if start > end { hour >= start || hour < end } else { false };
        if self.quiet_hours && inside { return Some("quiet_hours"); }
        if self.game_quiet && !process.is_empty() && !crate::game_policy::shared_host(process) && self.game_processes.iter()
            .any(|p| !p.trim().is_empty() && crate::game_policy::same_process(p, process)) {
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
    fn learned_games_survive_restart_without_duplicates_or_launcher_entries() {
        let mut config = CompanionConfig::default();
        assert!(!config.remember_game("NewGame.exe"), "disabled mode cannot grow the table");
        config.game_companion = true;
        assert!(config.remember_game("NewGame.exe"));
        assert!(!config.remember_game("newgame"));
        assert!(!config.remember_game("STEAM.EXE"));
        assert!(config.remember_game("javaw.exe"));
        assert_eq!(config.quiet_reason(12, "javaw"), None, "a shared runtime name alone cannot silence unrelated apps");
        let restored: CompanionConfig = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert!(crate::game_policy::known_game("newgame.EXE", &restored.game_processes));
        assert_eq!(restored.game_processes.iter().filter(|p| crate::game_policy::same_process(p, "newgame")).count(), 1);
    }
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
    fn quiet_state_survives_serialization_and_old_configs_default_to_off() {
        let old: CompanionConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!old.do_not_disturb);
        assert!(!old.game_companion);
        for enabled in [true, false] {
            let config = CompanionConfig { do_not_disturb: enabled, ..Default::default() };
            let saved = serde_json::to_value(config).unwrap();
            let loaded: CompanionConfig = serde_json::from_value(saved).unwrap();
            assert_eq!(loaded.do_not_disturb, enabled);
        }
    }
    #[test]
    fn shortcuts_gained_groups_without_breaking_saved_configs() {
        // 旧配置里的快捷方式没有 group 字段：必须仍然反序列化成功，并落到空串（未分组）
        let legacy: CompanionConfig = serde_json::from_value(serde_json::json!({
            "shortcuts": [
                {"id": "old", "name": "旧书签", "kind": "website", "target": "https://example.com"}
            ]
        })).unwrap();
        assert_eq!(legacy.shortcuts.len(), 1);
        assert_eq!(legacy.shortcuts[0].group, "", "missing group must fall back to ungrouped");
        assert_eq!(legacy.shortcuts[0].name, "旧书签");

        // 分组要能存能取，且总会出现在序列化结果里（前端按字段名读取）
        let saved = serde_json::to_value(DesktopShortcut {
            id: "g".into(), name: "B站".into(), kind: "website".into(),
            target: "https://bilibili.com".into(), group: "娱乐".into(),
        }).unwrap();
        assert_eq!(saved["group"], "娱乐");
        assert_eq!(serde_json::from_value::<DesktopShortcut>(saved).unwrap().group, "娱乐");

        // 默认种子自带分组：新用户打开页面就能看到分组是怎么用的
        let seeds = CompanionConfig::default().shortcuts;
        assert_eq!(seeds.len(), 2);
        assert!(seeds.iter().all(|item| !item.group.is_empty()), "seed shortcuts ship grouped");
    }
    #[test]
    fn quiet_windows_and_only_the_actual_foreground_game() {
        let mut c = CompanionConfig { quiet_hours: true, ..Default::default() };
        assert_eq!(c.quiet_reason(23, ""), Some("quiet_hours"));
        assert_eq!(c.quiet_reason(6, ""), Some("quiet_hours"));
        assert_eq!(c.quiet_reason(7, ""), None);
        assert_eq!(c.quiet_reason(12, "genshinimpact.EXE"), Some("game"));
        assert_eq!(c.quiet_reason(12, "GenshinImpact"), Some("game"));
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
