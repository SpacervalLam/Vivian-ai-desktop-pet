//! A single native desktop worker owns COM, accessibility snapshots and input.
//! Requests expire before input; timing out never schedules a retry.
use crate::desktop_contract::DesktopConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowTarget {
    pub id: i64,
    pub pid: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Action {
    pub action: String,
    pub window: Option<WindowTarget>,
    pub screenshot_id: Option<String>,
    pub element_index: Option<usize>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub to_x: Option<i32>,
    pub to_y: Option<i32>,
    pub button: Option<String>,
    pub keys: Option<String>,
    pub text: Option<String>,
    pub steps: Option<i32>,
    pub horizontal: bool,
    pub secondary_action: Option<String>,
    #[serde(skip)]
    pub absolute_coordinates: bool,
}
impl Action {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(
            self.action.as_str(),
            "click"
                | "key"
                | "type"
                | "move"
                | "drag"
                | "scroll"
                | "set_value"
                | "secondary"
                | "focus"
        ) {
            return Err("Unknown desktop action".into());
        }
        if self.element_index.is_some() && self.screenshot_id.as_deref().is_none_or(str::is_empty) {
            return Err("element_index requires the screenshot_id from computer_state".into());
        }
        if self.element_index.is_none()
            && matches!(self.action.as_str(), "click" | "move" | "drag" | "scroll")
            && (self.x.is_none() || self.y.is_none())
        {
            return Err("This action requires x/y or an element_index".into());
        }
        if self.action == "drag" && (self.to_x.is_none() || self.to_y.is_none()) {
            return Err("Drag requires to_x/to_y".into());
        }
        if self.action == "scroll" && !self.steps.is_some_and(|n| (-30..=30).contains(&n)) {
            return Err("steps must be from -30 to 30".into());
        }
        if self
            .button
            .as_deref()
            .is_some_and(|b| !matches!(b, "left" | "right" | "middle" | "double"))
        {
            return Err("Invalid mouse button".into());
        }
        if matches!(self.action.as_str(), "type" | "set_value")
            && !self.text.as_ref().is_some_and(|s| {
                s.chars().count() <= 4000 && (self.action == "set_value" || !s.is_empty())
            })
        {
            return Err("text is required (maximum 4000 characters)".into());
        }
        if matches!(self.action.as_str(), "set_value" | "secondary") && self.element_index.is_none()
        {
            return Err("This action requires element_index and screenshot_id".into());
        }
        if self.action == "key" {
            parse_keys(self.keys.as_deref().unwrap_or(""))?;
        }
        if self.action == "secondary"
            && !matches!(
                self.secondary_action.as_deref(),
                Some("invoke" | "toggle" | "expand" | "collapse" | "select")
            )
        {
            return Err("secondary_action must be invoke/toggle/expand/collapse/select".into());
        }
        Ok(())
    }
}

pub fn parse_keys(chord: &str) -> Result<Vec<u16>, String> {
    if chord.len() > 200 {
        return Err("Key chord is too long".into());
    }
    chord
        .split('+')
        .map(|s| {
            let key = s.trim().to_ascii_lowercase();
            let vk = match key.as_str() {
                "enter" | "return" => 13,
                "esc" | "escape" => 27,
                "backspace" => 8,
                "tab" => 9,
                "space" => 32,
                "ctrl" | "control" | "control_l" | "control_r" => 17,
                "alt" | "alt_l" | "alt_r" => 18,
                "shift" | "shift_l" | "shift_r" => 16,
                "win" | "meta" | "super_l" => 91,
                "left" => 37,
                "up" => 38,
                "right" => 39,
                "down" => 40,
                "home" => 36,
                "end" => 35,
                "pageup" | "page_up" => 33,
                "pagedown" | "page_down" => 34,
                "delete" | "del" => 46,
                "insert" => 45,
                "period" | "." => 190,
                "comma" | "," => 188,
                "minus" | "-" => 189,
                "equal" | "=" => 187,
                "slash" | "/" => 191,
                "semicolon" | ";" => 186,
                "apostrophe" | "'" => 222,
                "bracketleft" | "[" => 219,
                "bracketright" | "]" => 221,
                "backslash" | "\\" => 220,
                "grave" | "`" => 192,
                _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => {
                    key.as_bytes()[0].to_ascii_uppercase() as u16
                }
                _ if key.starts_with('f') => key[1..]
                    .parse::<u16>()
                    .ok()
                    .filter(|n| (1..=24).contains(n))
                    .map(|n| 111 + n)
                    .ok_or_else(|| format!("Unsupported key: {s}"))?,
                _ => return Err(format!("Unsupported key: {s}")),
            };
            Ok(vk)
        })
        .collect()
}

#[derive(Debug)]
pub struct Pixels {
    pub png: Vec<u8>,
    pub name: String,
}
#[derive(Debug)]
pub struct Reply {
    pub data: Value,
    pub images: Vec<Pixels>,
}
pub enum Request {
    Windows,
    Observe {
        window: WindowTarget,
        include_text: bool,
        include_screenshot: bool,
    },
    Perform(Action),
    PerformAndObserve {
        action: Action,
        observe_after: bool,
        include_text: bool,
        include_screenshot: bool,
    },
}
pub struct Deadline {
    until: Instant,
    cancelled: Arc<AtomicBool>,
}
impl Deadline {
    pub fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) || Instant::now() >= self.until {
            Err("Desktop request expired; observe before retrying input".into())
        } else {
            Ok(())
        }
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[cfg(windows)]
mod capture;
#[cfg(all(windows, test))]
mod smoke;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
struct Envelope {
    owner: String,
    request: Request,
    config: DesktopConfig,
    deadline: Deadline,
    tx: tokio::sync::oneshot::Sender<Result<Reply, String>>,
}
#[cfg(windows)]
static WORKER: std::sync::LazyLock<std::sync::mpsc::SyncSender<Envelope>> =
    std::sync::LazyLock::new(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Envelope>(1);
        std::thread::Builder::new()
            .name("desktop-native".into())
            .spawn(move || {
                let mut engine = windows::Engine::new();
                while let Ok(job) = rx.recv() {
                    if job.tx.is_closed() {
                        continue;
                    }
                    let reply = job.deadline.check().and_then(|_| {
                        engine.as_mut().map_err(|e| e.clone())?.run(
                            &job.owner,
                            job.request,
                            &job.config,
                            &job.deadline,
                        )
                    });
                    let _ = job.tx.send(reply);
                }
            })
            .expect("native desktop worker");
        tx
    });

pub async fn request(
    owner: String,
    request: Request,
    config: DesktopConfig,
) -> Result<Reply, String> {
    #[cfg(windows)]
    {
        let timeout = Duration::from_millis(config.bounded().operation_timeout_ms);
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel = CancelOnDrop(cancelled.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        WORKER
            .try_send(Envelope {
                owner,
                request,
                config: config.bounded(),
                deadline: Deadline {
                    until: Instant::now() + timeout,
                    cancelled,
                },
                tx,
            })
            .map_err(|_| {
                "Desktop worker is busy or unavailable; no input was queued".to_string()
            })?;
        // Give the worker a small delivery margin to preserve a completed-input receipt
        // when its observation encounters the deadline.
        tokio::time::timeout(timeout + Duration::from_millis(250), rx)
            .await
            .map_err(|_| {
                "Desktop operation timed out; outcome may be unknown. Observe before another input"
                    .to_string()
            })?
            .map_err(|_| "Desktop worker stopped".to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (owner, request, config);
        Err("Native desktop control requires Windows; use browser tools on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_input_never_reaches_native_worker() {
        for input in [
            serde_json::json!({"action":"click","x":1}),
            serde_json::json!({"action":"click","element_index":0}),
            serde_json::json!({"action":"scroll","x":0,"y":0,"steps":31}),
            serde_json::json!({"action":"key","keys":"ctrl++a"}),
        ] {
            assert!(serde_json::from_value::<Action>(input)
                .unwrap()
                .validate()
                .is_err());
        }
        assert_eq!(
            parse_keys("Control_L+Shift_L+period").unwrap(),
            vec![17, 16, 190]
        );
        assert!(parse_keys("f25").is_err());
    }
}
