//! 音频播放边界感知门控

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tokio::sync::broadcast;

const STALE_TIMEOUT_SECS: u64 = 45;

#[derive(Clone)]
pub struct PlaybackGate {
    inner: Arc<Inner>,
}

struct Inner {
    playing: AtomicBool,
    speakers: Mutex<std::collections::HashSet<String>>,
    last_start: Mutex<Option<Instant>>,
    last_end: Mutex<Option<Instant>>,
    tx: broadcast::Sender<PlaybackEvent>,
}

#[derive(Debug, Clone, Copy)]
pub enum PlaybackEvent {
    Started,
    Finished,
}

impl PlaybackGate {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(32);
        Self {
            inner: Arc::new(Inner {
                playing: AtomicBool::new(false),
                speakers: Mutex::new(std::collections::HashSet::new()),
                last_start: Mutex::new(None),
                last_end: Mutex::new(None),
                tx,
            }),
        }
    }

    pub fn clear(&self) { self.inner.speakers.lock().clear(); self.mark_finished(); }

    pub fn mark_started_for(&self, speaker: &str) {
        self.inner.speakers.lock().insert(speaker.to_owned());
        self.mark_started();
    }

    pub fn mark_finished_for(&self, speaker: &str) {
        let mut speakers = self.inner.speakers.lock();
        speakers.remove(speaker);
        if speakers.is_empty() { self.mark_finished(); }
    }

    pub fn mark_started(&self) {
        self.inner.playing.store(true, Ordering::SeqCst);
        *self.inner.last_start.lock() = Some(Instant::now());
        let _ = self.inner.tx.send(PlaybackEvent::Started);
    }

    pub fn mark_finished(&self) {
        self.inner.playing.store(false, Ordering::SeqCst);
        *self.inner.last_end.lock() = Some(Instant::now());
        let _ = self.inner.tx.send(PlaybackEvent::Finished);
    }

    pub fn is_playing(&self) -> bool {
        if !self.inner.speakers.lock().is_empty() { return true; }
        if self.inner.playing.load(Ordering::SeqCst) {
            let stale = self
                .inner
                .last_start
                .lock()
                .map(|t| t.elapsed().as_secs() >= STALE_TIMEOUT_SECS)
                .unwrap_or(false);
            if stale {
                self.inner.playing.store(false, Ordering::SeqCst);
                tracing::warn!(
                    "播放状态超过 {}s 未收到结束信号，自动清除",
                    STALE_TIMEOUT_SECS
                );
                return false;
            }
            true
        } else {
            false
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PlaybackEvent> {
        self.inner.tx.subscribe()
    }

    pub async fn wait_until_idle(&self, timeout_ms: u64) -> bool {
        if !self.is_playing() {
            return true;
        }
        let mut rx = self.subscribe();
        let deadline = tokio::time::sleep(tokio::time::Duration::from_millis(timeout_ms));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return !self.is_playing(),
                ev = rx.recv() => {
                    match ev {
                        Ok(PlaybackEvent::Finished) if !self.is_playing() => return true,
                        Ok(PlaybackEvent::Finished) => continue,
                        Ok(PlaybackEvent::Started) => continue,
                        Err(_) => return !self.is_playing(),
                    }
                }
            }
        }
    }
}

impl Default for PlaybackGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod speech_gate_tests {
    use super::*;
    #[test]
    fn one_speaker_finishing_cannot_release_another() {
        let gate = PlaybackGate::new();
        gate.mark_started_for("a"); gate.mark_started_for("b");
        gate.mark_finished_for("a"); assert!(gate.is_playing());
        gate.mark_finished_for("b"); assert!(!gate.is_playing());
        gate.mark_started_for("a"); gate.clear(); assert!(!gate.is_playing());
    }
}
