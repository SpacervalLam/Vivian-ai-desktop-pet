//! Generation attempts are separate from delivered speech and user silence.
use parking_lot::Mutex;
use std::{collections::HashMap, sync::Arc};

#[derive(Default)]
pub struct RetryGate {
    entries: HashMap<String, Entry>,
}
#[derive(Default)]
struct Entry {
    busy: bool,
    failures: u32,
    next_attempt: f64,
}
pub fn scope_key(trigger: &str, hint: Option<&str>) -> String {
    if trigger == "spontaneous" {
        if let Some(proposal) = hint.and_then(|hint| serde_json::from_str::<serde_json::Value>(hint).ok()) {
            return format!("{trigger}:{}", serde_json::json!({"meaning_key":proposal["meaning_key"],
                "evidence":proposal["evidence"],"event_id":proposal["event_id"]}));
        }
    }
    trigger.to_owned()
}
impl RetryGate {
    pub fn retry_pending(&self, key: &str) -> bool { self.entries.contains_key(key) }
}
pub struct Attempt {
    gate: Arc<Mutex<RetryGate>>,
    key: String,
    now: f64,
    started: std::time::Instant,
    succeeded: bool,
}
impl Attempt {
    pub fn reserve(gate: &Arc<Mutex<RetryGate>>, key: &str, now: f64) -> Option<Self> {
        let mut state = gate.lock();
        state.entries.retain(|_, entry| entry.busy || entry.next_attempt >= now - 3600.0);
        let entry = state.entries.entry(key.into()).or_default();
        if entry.busy || now < entry.next_attempt {
            return None;
        }
        entry.busy = true;
        Some(Self {
            gate: gate.clone(),
            key: key.into(),
            now,
            started: std::time::Instant::now(),
            succeeded: false,
        })
    }
    pub fn succeeded(&mut self) {
        self.succeeded = true;
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        if self.succeeded {
            state.entries.remove(&self.key);
            return;
        }
        let entry = state.entries.entry(self.key.clone()).or_default();
        entry.busy = false;
        entry.failures = entry.failures.saturating_add(1);
        let delay = (30.0 * 2f64.powi(entry.failures.saturating_sub(1).min(4) as i32)).min(300.0);
        entry.next_attempt = self.now + self.started.elapsed().as_secs_f64() + delay;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failures_back_off_and_success_resets_without_recording_speech() {
        let gate = Arc::new(Mutex::new(RetryGate::default()));
        let attempt = Attempt::reserve(&gate, "idle", 100.0).unwrap();
        assert!(Attempt::reserve(&gate, "idle", 101.0).is_none());
        drop(attempt);
        assert!(Attempt::reserve(&gate, "idle", 129.0).is_none());
        drop(Attempt::reserve(&gate, "idle", 131.0).unwrap());
        assert!(Attempt::reserve(&gate, "idle", 189.0).is_none());
        let mut successful = Attempt::reserve(&gate, "idle", 192.0).unwrap();
        successful.succeeded();
        drop(successful);
        assert!(Attempt::reserve(&gate, "idle", 193.0).is_some());
        assert!(Attempt::reserve(&gate, "welcome_back", 191.0).is_some());
    }
    #[test]
    fn failure_of_one_meaning_does_not_delay_a_different_meaning() {
        let gate = Arc::new(Mutex::new(RetryGate::default()));
        drop(Attempt::reserve(&gate, "spontaneous:point_a", 100.0).unwrap());
        assert!(Attempt::reserve(&gate, "spontaneous:point_a", 101.0).is_none());
        assert!(Attempt::reserve(&gate, "spontaneous:point_b", 101.0).is_some());
    }
    #[test]
    fn semantic_retry_identity_ignores_elapsed_time_and_reworded_topic() {
        let first=r#"{"meaning_key":"exam_card","evidence":"明天考试","event_id":"","seconds_since_proposal":0,"topic":"准考证"}"#;
        let later=r#"{"meaning_key":"exam_card","evidence":"明天考试","event_id":"","seconds_since_proposal":60,"topic":"带好准考证"}"#;
        assert_eq!(scope_key("spontaneous",Some(first)),scope_key("spontaneous",Some(later)));
        assert_ne!(scope_key("spontaneous",Some(first)),scope_key("spontaneous",Some(&later.replace("exam_card","exam_route"))));
    }
}
