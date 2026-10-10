//! Portable policy for temporary companion silence and source-specific chat dismissal.
use std::time::{Duration, Instant};

pub const REENTER_COOLDOWN: Duration = Duration::from_secs(60);
pub const MENU_SLIDE_DURATION: Duration = Duration::from_millis(220);

/// Physical coordinates work identically on scaled displays and monitors left of zero.
pub fn menu_slide_x(from: i32, to: i32, elapsed: Duration) -> i32 {
    let progress = (elapsed.as_secs_f64() / MENU_SLIDE_DURATION.as_secs_f64()).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - progress).powi(3);
    (f64::from(from) + (f64::from(to) - f64::from(from)) * eased).round() as i32
}

#[derive(Default)]
pub struct QuietTransitions {
    pub active: bool,
    pub revision: u64,
    last_entered: Option<Instant>,
}

impl QuietTransitions {
    pub fn set(&mut self, active: bool, now: Instant) -> Result<bool, Duration> {
        if self.active == active { return Ok(false); }
        if active {
            if let Some(last) = self.last_entered {
                let elapsed = now.saturating_duration_since(last);
                if elapsed < REENTER_COOLDOWN { return Err(REENTER_COOLDOWN - elapsed); }
            }
            self.last_entered = Some(now);
        }
        self.active = active;
        self.revision += 1;
        Ok(true)
    }
}

pub fn dismiss_on_outside_click(origin: &str) -> bool { origin == "menu_tool" }

pub fn menu_panel(action: &str) -> Option<&str> {
    match action { "notes" | "shortcuts" => Some(action), _ => None }
}

/// The last enabled movement mode wins; disabling one never enables the other.
pub fn enforce_movement_exclusion(smart: &mut bool, gravity: &mut bool, changed_key: &str) {
    if *smart && *gravity {
        if changed_key == "window.smart_positioning_enabled" { *gravity = false; }
        else { *smart = false; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn movement_switches_are_exclusive_and_can_both_be_off() {
        for initial in [(false, false), (true, false), (false, true)] {
            for (key, enabled) in [("window.smart_positioning_enabled", true), ("window.smart_positioning_enabled", false),
                ("window.desktop_physics_enabled", true), ("window.desktop_physics_enabled", false)] {
                let (mut smart, mut gravity) = initial;
                if key == "window.smart_positioning_enabled" { smart = enabled; } else { gravity = enabled; }
                enforce_movement_exclusion(&mut smart, &mut gravity, key);
                assert!(!(smart && gravity));
                if key == "window.smart_positioning_enabled" { assert_eq!(smart, enabled); }
                else { assert_eq!(gravity, enabled); }
                if !enabled {
                    if key == "window.smart_positioning_enabled" { assert_eq!(gravity, initial.1); }
                    else { assert_eq!(smart, initial.0); }
                }
            }
        }
        let (mut smart, mut gravity) = (true, true);
        enforce_movement_exclusion(&mut smart, &mut gravity, "window");
        assert_eq!((smart, gravity), (false, true), "legacy conflicts keep gravity");
    }
    #[test]
    fn menu_slides_from_outside_the_right_edge_without_overshooting() {
        for (from, to) in [(1920, 1860), (2560, 2500), (0, -60), (-1920, -1980)] {
            assert_eq!(menu_slide_x(from, to, Duration::ZERO), from);
            let mut previous = from;
            for millis in 1..=220 {
                let x = menu_slide_x(from, to, Duration::from_millis(millis));
                assert!(x <= previous && x >= to);
                previous = x;
            }
            assert_eq!(previous, to);
            assert_eq!(menu_slide_x(from, to, Duration::from_secs(10)), to);
            assert!(menu_slide_x(from, to, Duration::from_millis(110)) < from - (from - to) / 2);
        }
    }
    #[test]
    fn menu_slides_back_out_to_the_right_edge() {
        // 退场是入场的镜像：从屏幕内滑回右边缘外，单调不减、不越过终点。
        for (from, to) in [(1860, 1920), (2500, 2560), (-60, 0)] {
            assert_eq!(menu_slide_x(from, to, Duration::ZERO), from);
            let mut previous = from;
            for millis in 1..=220 {
                let x = menu_slide_x(from, to, Duration::from_millis(millis));
                assert!(x >= previous && x <= to);
                previous = x;
            }
            assert_eq!(previous, to);
            assert_eq!(menu_slide_x(from, to, Duration::from_secs(10)), to);
        }
    }
    #[test]
    fn rapid_toggles_cannot_repeat_model_calls_but_exit_is_always_available() {
        let start = Instant::now();
        let mut state = QuietTransitions::default();
        assert_eq!(state.set(true, start), Ok(true));
        for _ in 0..100 { assert_eq!(state.set(true, start), Ok(false)); }
        assert_eq!(state.set(false, start), Ok(true));
        for n in 1..60 {
            assert!(state.set(true, start + Duration::from_secs(n)).is_err());
            assert!(!state.active);
        }
        assert_eq!(state.set(true, start + REENTER_COOLDOWN), Ok(true));
        assert_eq!(state.set(false, start + REENTER_COOLDOWN), Ok(true));
        assert_eq!(state.revision, 4);
    }
    #[test]
    fn only_menu_tools_dismiss_on_outside_click() {
        assert!(dismiss_on_outside_click("menu_tool"));
        for origin in ["chat", "header", "shortcut", "tray", "", "menu"] {
            assert!(!dismiss_on_outside_click(origin));
        }
        for action in ["notes", "shortcuts"] { assert_eq!(menu_panel(action), Some(action)); }
        assert_eq!(menu_panel("clipboard"), None);
        for action in ["chat", "office", "dnd", "dialogue", "screen"] { assert_eq!(menu_panel(action), None); }
    }
}
