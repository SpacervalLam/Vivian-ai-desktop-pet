//! Per-run cancellation remains valid until the executing future has been dropped.
use std::{collections::HashMap, sync::{Arc, Weak}};
use parking_lot::Mutex;
use tokio::sync::Notify;

#[derive(Default)]
pub struct Runs(Mutex<HashMap<String, Weak<Lease>>>);
struct Lease { owner: Arc<Runs>, id: String, signal: Notify }
pub struct Run(Arc<Lease>);
impl Runs {
    pub fn begin(self: &Arc<Self>, id: &str) -> Option<Run> {
        let mut runs = self.0.lock();
        if runs.get(id).is_some_and(|run| run.strong_count() > 0) { return None; }
        let lease = Arc::new(Lease { owner: self.clone(), id: id.into(), signal: Notify::new() });
        runs.insert(id.into(), Arc::downgrade(&lease));
        Some(Run(lease))
    }
    /// Keep the old run reserved until cancellation cleanup has completed.
    pub fn hold(&self, id: &str) -> Option<Run> { self.0.lock().get(id).and_then(Weak::upgrade).map(Run) }
    pub fn cancel(&self, id: &str) { if let Some(run) = self.hold(id) { run.cancel(); } }
}
impl Run {
    pub fn cancel(&self) { self.0.signal.notify_one(); }
    pub async fn cancelled(&self) { self.0.signal.notified().await; }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut runs = self.owner.0.lock();
        // A concurrent begin may have replaced an expired weak entry already.
        if runs.get(&self.id).is_some_and(|entry| entry.as_ptr() == self as *const Lease) { runs.remove(&self.id); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn cancellation_drops_pending_work_and_allows_a_new_turn() {
        let runs = Arc::new(Runs::default());
        let run = runs.begin("task").unwrap();
        assert!(runs.begin("task").is_none());
        // Cancellation before the future is polled must not get lost.
        runs.cancel("task");
        tokio::time::timeout(Duration::from_millis(100), async {
            tokio::select! { biased; _ = run.cancelled() => {}, _ = std::future::pending::<()>() => unreachable!() }
        }).await.unwrap();
        assert!(runs.begin("task").is_none(), "do not allow a restart before the old run exits");
        let cleanup = runs.hold("task").unwrap();
        drop(run);
        assert!(runs.begin("task").is_none(), "reserve the old run until cancellation cleanup ends");
        drop(cleanup);
        let next = runs.begin("task").unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(10), next.cancelled()).await.is_err(), "old cancellation must not leak into a new turn");
    }
    #[tokio::test]
    async fn cancellation_during_a_pending_request_drops_it() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Probe(Arc<AtomicBool>);
        impl Drop for Probe { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
        let runs = Arc::new(Runs::default());
        let run = runs.begin("stream").unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = run.cancelled() => {},
                _ = async { let _probe = Probe(flag); let _ = started.send(()); std::future::pending::<()>().await; } => unreachable!(),
            }
        });
        ready.await.unwrap();
        runs.cancel("stream");
        tokio::time::timeout(Duration::from_millis(100), task).await.unwrap().unwrap();
        assert!(dropped.load(Ordering::SeqCst), "drop the HTTP/tool future instead of merely marking UI stopped");
    }
    #[tokio::test]
    async fn stopping_one_session_does_not_stop_another() {
        let runs = Arc::new(Runs::default());
        let first = runs.begin("first").unwrap();
        let second = runs.begin("second").unwrap();
        runs.cancel("first");
        first.cancelled().await;
        assert!(tokio::time::timeout(Duration::from_millis(10), second.cancelled()).await.is_err());
    }
}
