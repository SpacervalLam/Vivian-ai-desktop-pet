//! Environment-agent-owned foreground history, independent of OS hooks and character planning.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ForegroundWindowSnapshot {
    pub title: String,
    pub process: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExternalAppObservation {
    pub window: ForegroundWindowSnapshot,
    pub observed_at_unix_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ForegroundContext {
    pub current: Option<ExternalAppObservation>,
    pub last_external: Option<ExternalAppObservation>,
}

impl ForegroundContext {
    /// A delayed hook event can fill history but cannot replace a newer foreground sample.
    pub fn observe(&mut self, window: ForegroundWindowSnapshot, companion_pid: u32, at: i64) -> bool {
        let sample = ExternalAppObservation { window, observed_at_unix_ms: at };
        if sample.window.pid != 0 && sample.window.pid != companion_pid
            && self.last_external.as_ref().is_none_or(|old| at >= old.observed_at_unix_ms) {
            self.last_external = Some(sample.clone());
        }
        if self.current.as_ref().is_some_and(|old| at < old.observed_at_unix_ms) { return false; }
        let changed = self.current.as_ref().is_none_or(|old| old.window != sample.window);
        self.current = Some(sample);
        changed
    }

    pub fn current_window(&self, now: i64) -> Option<ForegroundWindowSnapshot> {
        self.current.as_ref().filter(|sample| sample.window.pid != 0
            && (0..=30_000).contains(&now.saturating_sub(sample.observed_at_unix_ms)))
            .map(|sample| sample.window.clone())
    }

    /// Keep an existing game context briefly while the companion's own chat has focus.
    /// Missing focus, a different external app or expired history never starts a game session.
    pub fn game_window(&self, companion_pid: u32, existing_game_pid: u32, now: i64) -> Option<ForegroundWindowSnapshot> {
        let current = self.current_window(now)?;
        if current.pid != companion_pid { return Some(current); }
        self.last_external.as_ref().filter(|sample| existing_game_pid != 0
            && sample.window.pid == existing_game_pid
            && (0..120_000).contains(&now.saturating_sub(sample.observed_at_unix_ms)))
            .map(|sample| sample.window.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn window(pid: u32, title: &str) -> ForegroundWindowSnapshot {
        ForegroundWindowSnapshot { pid, process: format!("app-{pid}"), title: title.into() }
    }
    #[test]
    fn chat_focus_preserves_history_without_reporting_it_as_foreground() {
        let mut context = ForegroundContext::default();
        context.observe(window(7, "Game"), 42, 1000);
        context.observe(window(42, "Chat"), 42, 2000);
        assert_eq!(context.current_window(2100).unwrap().pid, 42);
        assert_eq!(context.last_external.as_ref().unwrap().observed_at_unix_ms, 1000);
        assert_eq!(context.game_window(42, 7, 2100).unwrap().pid, 7);
        assert!(context.game_window(42, 0, 2100).is_none());
        context.observe(window(42, "Chat"), 42, 120_000);
        assert!(context.game_window(42, 7, 121_000).is_none());
    }
    #[test]
    fn missing_or_stale_focus_does_not_leave_a_game_active() {
        let mut context = ForegroundContext::default();
        context.observe(window(7, "Game"), 42, 1000);
        assert!(context.current_window(31_001).is_none());
        context.observe(ForegroundWindowSnapshot::default(), 42, 2000);
        assert!(context.game_window(42, 7, 2100).is_none());
        assert_eq!(context.last_external.as_ref().unwrap().window.pid, 7);
    }
    #[test]
    fn rapid_hook_events_preserve_external_history_but_cannot_rewind_focus() {
        let mut context = ForegroundContext::default();
        context.observe(window(42, "Chat"), 42, 2000);
        assert!(!context.observe(window(7, "Game"), 42, 1000));
        assert_eq!(context.current_window(2100).unwrap().pid, 42);
        assert_eq!(context.last_external.as_ref().unwrap().window.pid, 7);
        context.observe(window(8, "Editor"), 42, 3000);
        context.observe(window(7, "Game"), 42, 1500);
        assert_eq!(context.game_window(42, 7, 3100).unwrap().pid, 8);
        assert_eq!(context.last_external.as_ref().unwrap().window.pid, 8);
    }
    #[test]
    fn same_title_in_different_apps_still_counts_as_a_change() {
        let mut context = ForegroundContext::default();
        assert!(context.observe(window(7, "Untitled"), 42, 1000));
        assert!(context.observe(window(8, "Untitled"), 42, 2000));
        assert!(!context.observe(window(8, "Untitled"), 42, 3000));
    }
}
