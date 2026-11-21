use std::sync::{atomic::AtomicBool, Arc, LazyLock};
pub use vivian_speech_runtime::session::{AudioLease, AudioMode};
pub static AUDIO_SESSIONS: LazyLock<Arc<vivian_speech_runtime::session::AudioSessions>> =
    LazyLock::new(|| Arc::new(Default::default()));

pub async fn cancelled(token: Arc<AtomicBool>) {
    while !token.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

pub async fn capture(mode: AudioMode) -> Result<AudioLease, String> {
    let _request = AUDIO_SESSIONS.request_capture();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match AUDIO_SESSIONS.acquire(mode, Arc::new(AtomicBool::new(false))) {
            Ok(lease) => return Ok(lease),
            Err(error) if tokio::time::Instant::now() >= deadline => return Err(error),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
        }
    }
}
