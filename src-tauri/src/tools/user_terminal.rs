//! Read-only screen snapshots of user-operated terminals, scoped to a work chat.
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Default)]
struct Screen {
    owner: String,
    cwd: String,
    text: String,
    alive: bool,
    truncated: bool,
}

fn screens() -> &'static Mutex<HashMap<String, Screen>> {
    static SCREENS: OnceLock<Mutex<HashMap<String, Screen>>> = OnceLock::new();
    SCREENS.get_or_init(Default::default)
}

pub(crate) fn register(id: &str, owner: &str, cwd: &str) {
    screens().lock().insert(id.into(), Screen {
        owner: owner.into(), cwd: cwd.into(), alive: true, ..Default::default()
    });
}

pub(crate) fn update(id: &str, text: &str) {
    if let Some(screen) = screens().lock().get_mut(id) {
        // Keep a bounded tail on Unicode boundaries, independently of agent reads.
        let start = text.char_indices().rev().nth(40_000 - 1).map_or(0, |(i, _)| i);
        screen.text = text[start..].into();
        screen.truncated = start > 0;
    }
}

/// A terminal opened before the first chat message can join that new chat once.
pub(crate) fn bind(id: &str, owner: &str) {
    if owner.is_empty() { return; }
    if let Some(screen) = screens().lock().get_mut(id) {
        if screen.owner.is_empty() { screen.owner = owner.into(); }
    }
}

pub(crate) fn exited(id: &str) {
    if let Some(screen) = screens().lock().get_mut(id) { screen.alive = false; }
}

pub(crate) fn remove(id: &str) { screens().lock().remove(id); }

pub(crate) fn list(owner: &str) -> Vec<Value> {
    if owner.is_empty() { return vec![]; }
    screens().lock().iter().filter(|(_, s)| s.owner == owner).map(|(id, s)| {
        json!({"terminal_id": id, "cwd": s.cwd, "alive": s.alive,
            "source": "user", "read_only": true})
    }).collect()
}

pub(crate) fn read(id: &str, owner: &str, max_chars: usize) -> Option<Value> {
    let guard = screens().lock();
    let screen = guard.get(id)?;
    if owner.is_empty() || screen.owner != owner { return None; }
    let start = screen.text.char_indices().rev().nth(max_chars.saturating_sub(1))
        .map_or(0, |(i, _)| i);
    Some(json!({"terminal_id": id, "cwd": screen.cwd, "output": &screen.text[start..],
        "process_alive": screen.alive, "source": "user", "read_only": true,
        "snapshot": true, "truncated": screen.truncated || start > 0}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_screens_are_scoped_repeatable_and_unicode_bounded() {
        let id = "user-screen-scope-test";
        register(id, "chat-a", "workspace");
        bind(id, "chat-b");
        update(id, "甲🙂乙abc");
        assert!(read(id, "chat-b", 3).is_none());
        assert!(read(id, "", 3).is_none());
        assert!(list("chat-b").iter().all(|s| s["terminal_id"] != id));
        assert_eq!(read(id, "chat-a", 4).unwrap()["output"], "乙abc");
        assert_eq!(read(id, "chat-a", 4).unwrap()["output"], "乙abc");
        update(id, &"🙂".repeat(50_000));
        assert_eq!(read(id, "chat-a", 50_000).unwrap()["output"].as_str().unwrap().chars().count(), 40_000);
        exited(id);
        assert_eq!(read(id, "chat-a", 2).unwrap()["process_alive"], false);
        remove(id);
        assert!(read(id, "chat-a", 4).is_none());
    }
    #[test]
    fn terminals_created_before_a_chat_can_only_be_bound_once() {
        let id = "user-screen-unbound-test";
        register(id, "", "workspace");
        update(id, "prior output");
        assert!(read(id, "chat-new", 100).is_none());
        bind(id, "chat-new");
        bind(id, "another-chat");
        assert_eq!(read(id, "chat-new", 100).unwrap()["output"], "prior output");
        assert!(read(id, "another-chat", 100).is_none());
        remove(id);
    }
}
