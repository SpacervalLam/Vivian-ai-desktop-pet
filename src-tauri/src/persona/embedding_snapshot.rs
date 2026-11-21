//! Fixed corpora are built once off the reply path, then published atomically.
use std::{sync::{Arc, OnceLock, mpsc}, time::{Duration, Instant}};
use parking_lot::Mutex;
use crate::{emotion::corpus_cache, memory::embedding::MemoryEmbeddingProvider};

#[derive(Clone)]
pub(super) struct EmbeddingSnapshot(Arc<Inner>);
struct Inner {
    provider: Arc<dyn MemoryEmbeddingProvider>,
    texts: Vec<String>,
    name: String,
    state: Mutex<State>,
}
#[derive(Default)]
struct State { vectors: Option<Arc<Vec<Vec<f32>>>>, building: bool, retry_after: Option<Instant> }

fn builders() -> &'static mpsc::SyncSender<EmbeddingSnapshot> {
    static QUEUE: OnceLock<mpsc::SyncSender<EmbeddingSnapshot>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<EmbeddingSnapshot>(8);
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..2 {
            let receiver = receiver.clone();
            let _ = std::thread::Builder::new().name("persona-embedding".into()).spawn(move || loop {
                let job = receiver.lock().recv();
                let Ok(job) = job else { break; };
                job.build();
            });
        }
        sender
    })
}

impl EmbeddingSnapshot {
    pub fn new(provider: Arc<dyn MemoryEmbeddingProvider>, name: String, texts: Vec<String>) -> Self {
        let state = if !provider.is_remote() { State { vectors: Some(Arc::new(Vec::new())), ..Default::default() } } else { State::default() };
        Self(Arc::new(Inner { provider, name, texts, state: Mutex::new(state) }))
    }
    pub fn get(&self) -> Option<Arc<Vec<Vec<f32>>>> { self.0.state.lock().vectors.clone() }
    fn claim(&self) -> bool {
        let mut state = self.0.state.lock();
        if state.vectors.is_some() || state.building || state.retry_after.is_some_and(|at| Instant::now() < at) { return false; }
        state.building = true;
        true
    }
    pub fn request(&self) {
        if !self.claim() { return; }
        if builders().try_send(self.clone()).is_err() {
            self.fail();
        }
    }
    #[cfg(test)]
    pub fn preload(&self) { if self.claim() { self.build(); } }
    fn fail(&self) {
        let mut state = self.0.state.lock();
        state.building = false;
        state.retry_after = Some(Instant::now() + Duration::from_secs(30));
    }
    fn build(&self) {
        // No state/reader lock is held during disk or provider I/O.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !self.0.provider.is_remote() || self.0.texts.is_empty() { return Some(Vec::new()); }
            let texts: Vec<_> = self.0.texts.iter().map(String::as_str).collect();
            let dim = self.0.provider.dimension();
            let key = corpus_cache::corpus_key(&self.0.provider.index_identity(), dim, &texts);
            let valid = |v: &Vec<Vec<f32>>| v.len() == texts.len() && v.iter().all(|row|
                row.len() == dim && row.iter().all(|x| x.is_finite()) && row.iter().any(|x| *x != 0.0));
            if let Some(vectors) = corpus_cache::load(&self.0.name, key, texts.len(), dim).filter(valid) { return Some(vectors); }
            let vectors = self.0.provider.embed_batch(&self.0.texts).ok().filter(valid)?;
            corpus_cache::save(&self.0.name, key, &vectors, dim);
            Some(vectors)
        })).ok().flatten();
        if let Some(vectors) = result {
            let mut state = self.0.state.lock();
            state.vectors = Some(Arc::new(vectors));
            state.building = false;
            state.retry_after = None;
        } else { self.fail(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::{AtomicUsize, Ordering}, Barrier};
    struct Slow { calls: AtomicUsize, start: Arc<Barrier>, release: Arc<Barrier> }
    impl MemoryEmbeddingProvider for Slow {
        fn dimension(&self) -> usize { 2 }
        fn is_remote(&self) -> bool { true }
        fn model_id(&self) -> &str { "snapshot-test" }
        fn embed(&self, _: &str) -> crate::error::VivianResult<Vec<f32>> { unreachable!("must batch") }
        fn embed_batch(&self, texts: &[String]) -> crate::error::VivianResult<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::SeqCst); self.start.wait(); self.release.wait();
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }
    #[test]
    fn slow_cold_build_never_holds_readers_and_reload_uses_disk() {
        let start = Arc::new(Barrier::new(2)); let release = Arc::new(Barrier::new(2));
        let provider = Arc::new(Slow { calls: AtomicUsize::new(0), start: start.clone(), release: release.clone() });
        let name = format!("snapshot-test-{}", uuid::Uuid::new_v4());
        let snapshot = EmbeddingSnapshot::new(provider.clone(), name.clone(), vec!["a".into(), "b".into()]);
        snapshot.request(); start.wait();
        let before = Instant::now();
        for _ in 0..100 { snapshot.request(); assert!(snapshot.get().is_none()); }
        assert!(before.elapsed() < Duration::from_millis(100));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        release.wait();
        let deadline = Instant::now() + Duration::from_secs(2);
        while snapshot.get().is_none() { assert!(Instant::now() < deadline); std::thread::yield_now(); }
        let reloaded = EmbeddingSnapshot::new(provider.clone(), name.clone(), vec!["a".into(), "b".into()]);
        reloaded.preload();
        assert_eq!(snapshot.get(), reloaded.get());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(crate::utils::path::get_user_data_dir().join("cache/corpus_embeddings").join(format!("{name}.bin")));
    }
}
