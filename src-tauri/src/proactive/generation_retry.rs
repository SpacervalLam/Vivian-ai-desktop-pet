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
}
