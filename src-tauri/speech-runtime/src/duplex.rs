//! Timestamped render reference and microphone processing off the device callbacks.
use crate::aec::EchoCanceller;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

enum Packet {
    Render(u64, Vec<i16>),
    Capture(u64, Vec<i16>),
}
pub struct Duplex {
    tx: mpsc::SyncSender<Packet>,
    origin: Instant,
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    worker: Option<thread::JoinHandle<()>>,
}

struct Reference {
    start: u64,
    samples: VecDeque<i16>,
}
impl Reference {
    fn append(&mut self, at: u64, samples: Vec<i16>) {
        let end = self.start + self.samples.len() as u64;
        if self.samples.is_empty() || at.saturating_sub(end) > 16000 {
            self.start = at;
            self.samples.clear();
        }
        let end = self.start + self.samples.len() as u64;
        if at > end {
            self.samples
                .extend(std::iter::repeat(0).take((at - end) as usize));
        }
        let skip = end.saturating_sub(at) as usize;
        self.samples.extend(samples.into_iter().skip(skip));
        let remove = self.samples.len().saturating_sub(32000);
        self.samples.drain(..remove);
        self.start += remove as u64;
    }
    fn frame(&self, at: u64) -> [i16; 320] {
        std::array::from_fn(|i| {
            (at + i as u64)
                .checked_sub(self.start)
                .and_then(|offset| self.samples.get(offset as usize))
                .copied()
                .unwrap_or(0)
        })
    }
}

impl Duplex {
    pub fn start(
        enabled: bool,
        tail_ms: u32,
        consume: impl Fn(&[i16]) + Send + 'static,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel(128);
        let (ready, status) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let dropped = Arc::new(AtomicU64::new(0));
        let worker = thread::Builder::new()
            .name("speech-aec".into())
            .spawn(move || {
                let mut aec = if enabled {
                    match EchoCanceller::new(16000, 320, tail_ms) {
                        Ok(aec) => Some(aec),
                        Err(error) => {
                            let _ = ready.send(Err(error));
                            return;
                        }
                    }
                } else {
                    None
                };
                let _ = ready.send(Ok(()));
                let mut reference = Reference {
                    start: 0,
                    samples: VecDeque::new(),
                };
                while !worker_stop.load(Ordering::SeqCst) {
                    match rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(Packet::Render(at, samples)) => reference.append(at, samples),
                        Ok(Packet::Capture(at, samples)) => {
                            let mut output = [0; 320];
                            if samples.len() != 320 {
                                continue;
                            }
                            if let Some(aec) = &mut aec {
                                if aec
                                    .process(&samples, &reference.frame(at), &mut output)
                                    .is_err()
                                {
                                    continue;
                                }
                                consume(&output);
                            } else {
                                consume(&samples);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        status
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "AEC worker startup timed out")??;
        Ok(Self {
            tx,
            origin: Instant::now(),
            stop,
            dropped,
            worker: Some(worker),
        })
    }
    fn at(&self, instant: Instant) -> u64 {
        instant
            .saturating_duration_since(self.origin)
            .as_nanos()
            .saturating_mul(16000)
            .checked_div(1_000_000_000)
            .unwrap_or(0) as u64
    }
    pub fn render(&self, samples: Vec<i16>, playback: Instant) {
        if self
            .tx
            .try_send(Packet::Render(self.at(playback), samples))
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn capture(&self, samples: Vec<i16>, captured: Instant) {
        if self
            .tx
            .try_send(Packet::Capture(self.at(captured), samples))
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn dropped_packets(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for Duplex {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_reference_is_zero_and_history_is_bounded() {
        let mut reference = Reference {
            start: 0,
            samples: VecDeque::new(),
        };
        reference.append(100, vec![42; 640]);
        assert_eq!(reference.frame(100), [42; 320]);
        assert_eq!(reference.frame(1000), [0; 320]);
        reference.append(740, vec![1; 40000]);
        assert!(reference.samples.len() <= 32000);
    }
}
