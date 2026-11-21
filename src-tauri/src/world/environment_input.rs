//! Bounded, non-waiting handoff. The reply path never locks environment observations.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::{sync_channel, Receiver, SyncSender},
    Mutex,
};

pub struct UserExchange {
    pub revision: u64,
    pub character_id: String,
    pub user_text: String,
    pub assistant_text: String,
    pub at: f64,
}

pub struct EnvironmentInput {
    sender: SyncSender<UserExchange>,
    receiver: Mutex<Receiver<UserExchange>>,
    revision: AtomicU64,
}

impl Default for EnvironmentInput {
    fn default() -> Self {
        let (sender, receiver) = sync_channel(16);
        Self {
            sender,
            receiver: Mutex::new(receiver),
            revision: AtomicU64::new(0),
        }
    }
}

impl EnvironmentInput {
    pub fn publish(
        &self,
        character_id: &str,
        user_text: &str,
        assistant_text: &str,
        at: f64,
    ) -> bool {
        if user_text.trim().is_empty() {
            return false;
        }
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        self.sender
            .try_send(UserExchange {
                revision,
                character_id: character_id.chars().take(64).collect(),
                user_text: user_text.chars().take(768).collect(),
                assistant_text: assistant_text.chars().take(384).collect(),
                at,
            })
            .is_ok()
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn drain(&self) -> Vec<UserExchange> {
        let Ok(receiver) = self.receiver.try_lock() else {
            return Vec::new();
        };
        receiver.try_iter().take(16).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    #[test]
    fn reply_handoff_never_waits_for_a_busy_background_reader() {
        let input = Arc::new(EnvironmentInput::default());
        let reader = input.receiver.lock().unwrap();
        let producer = input.clone();
        let (done, completion) = sync_channel(1);
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            assert!(producer.publish("vivian", "hello", "hi", 1000.0));
            done.send(start.elapsed()).unwrap();
        });
        let elapsed = completion.recv_timeout(Duration::from_millis(250));
        drop(reader);
        thread.join().unwrap();
        eprintln!(
            "Reply handoff while background reader locked: {:?}",
            elapsed.as_ref().unwrap()
        );
        assert!(elapsed.is_ok(), "reply waited for background work");
        assert_eq!(input.drain().len(), 1);
    }

    #[test]
    fn overloaded_optional_input_drops_without_waiting_and_invalidates_older_context() {
        let input = EnvironmentInput::default();
        for _ in 0..16 {
            assert!(input.publish("vivian", "hello", "reply", 1000.0));
        }
        let before = Instant::now();
        assert!(!input.publish("vivian", "newer input", "reply", 1001.0));
        let inputs = input.drain();
        assert_ne!(inputs.last().unwrap().revision, input.revision());
        assert!(before.elapsed() < Duration::from_millis(250));
        assert!(input.publish("vivian", "fresh", "reply", 1002.0));
        assert_eq!(input.drain()[0].revision, input.revision());
    }

    #[test]
    #[ignore = "synthetic contention benchmark; run explicitly with --ignored --nocapture"]
    fn contention_measurement() {
        let mut legacy = Vec::new();
        let mut handoff = Vec::new();
        for _ in 0..5 {
            let shared = Arc::new(Mutex::new(()));
            let worker_lock = shared.clone();
            let (ready, started) = sync_channel(0);
            let worker = std::thread::spawn(move || {
                let _background = worker_lock.lock().unwrap();
                ready.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(100));
            });
            started.recv().unwrap();
            let start = Instant::now();
            drop(shared.lock().unwrap());
            legacy.push(start.elapsed());
            worker.join().unwrap();

            let input = EnvironmentInput::default();
            let _busy_reader = input.receiver.lock().unwrap();
            let start = Instant::now();
            assert!(input.publish("vivian", "hello", "reply", 1000.0));
            handoff.push(start.elapsed());
        }
        eprintln!("100ms synthetic background contention; shared-lock reference={legacy:?}; production input handoff={handoff:?}");
    }
}
