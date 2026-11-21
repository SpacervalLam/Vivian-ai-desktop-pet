use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioMode {
    Playback,
    Dictation,
    Call,
}

#[derive(Default)]
pub struct AudioSessions {
    next: AtomicU64,
    pending_capture: AtomicUsize,
    active: Mutex<HashMap<u64, (AudioMode, Arc<AtomicBool>)>>,
}

pub struct AudioLease {
    owner: Arc<AudioSessions>,
    pub id: u64,
    pub cancelled: Arc<AtomicBool>,
}
pub struct CaptureRequest(Arc<AudioSessions>);
impl Drop for CaptureRequest {
    fn drop(&mut self) {
        self.0.pending_capture.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AudioSessions {
    pub fn request_capture(self: &Arc<Self>) -> CaptureRequest {
        self.pending_capture.fetch_add(1, Ordering::SeqCst);
        self.cancel_playback();
        CaptureRequest(self.clone())
    }
    pub fn acquire(
        self: &Arc<Self>,
        mode: AudioMode,
        cancelled: Arc<AtomicBool>,
    ) -> Result<AudioLease, String> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if mode == AudioMode::Playback && self.pending_capture.load(Ordering::SeqCst) != 0 {
            return Err("录音会话正在获取音频设备".into());
        }
        if active
            .values()
            .any(|(existing, _)| *existing != AudioMode::Playback || mode != AudioMode::Playback)
        {
            return Err("音频设备正由另一个语音会话使用".into());
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        active.insert(id, (mode, cancelled.clone()));
        Ok(AudioLease {
            owner: self.clone(),
            id,
            cancelled,
        })
    }
    pub fn cancel_playback(&self) {
        for (mode, token) in self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            if *mode == AudioMode::Playback {
                token.store(true, Ordering::SeqCst);
            }
        }
    }
}

impl Drop for AudioLease {
    fn drop(&mut self) {
        self.owner
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_are_exclusive_and_cancelled_playback_is_not_released_early() {
        let sessions = Arc::new(AudioSessions::default());
        let playback = sessions
            .acquire(AudioMode::Playback, Arc::new(AtomicBool::new(false)))
            .unwrap();
        let request = sessions.request_capture();
        assert!(playback.cancelled.load(Ordering::SeqCst));
        assert!(sessions
            .acquire(AudioMode::Call, Arc::new(AtomicBool::new(false)))
            .is_err());
        drop(playback);
        assert!(sessions
            .acquire(AudioMode::Playback, Arc::new(AtomicBool::new(false)))
            .is_err());
        let call = sessions
            .acquire(AudioMode::Call, Arc::new(AtomicBool::new(false)))
            .unwrap();
        drop(request);
        assert!(sessions
            .acquire(AudioMode::Dictation, Arc::new(AtomicBool::new(false)))
            .is_err());
        drop(call);
        assert!(sessions
            .acquire(AudioMode::Dictation, Arc::new(AtomicBool::new(false)))
            .is_ok());
    }
}
