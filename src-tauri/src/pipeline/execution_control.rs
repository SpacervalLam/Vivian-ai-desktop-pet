//! A character's new input invalidates old decisions without cancelling in-flight effects.
use std::future::Future;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct ExecutionControl(Arc<tokio::sync::watch::Sender<u64>>);

impl Default for ExecutionControl {
    fn default() -> Self { Self(Arc::new(tokio::sync::watch::channel(0).0)) }
}

#[derive(Clone)]
pub(crate) struct ExecutionLease {
    revision: u64,
    current: Arc<tokio::sync::watch::Sender<u64>>,
    outcome: Arc<parking_lot::Mutex<Option<serde_json::Value>>>,
}

tokio::task_local! { static CURRENT: ExecutionLease; }

impl ExecutionControl {
    pub(crate) fn interrupt(&self) { self.0.send_modify(|revision| *revision = revision.wrapping_add(1)); }
    pub(crate) fn lease(&self) -> ExecutionLease {
        ExecutionLease { revision: *self.0.borrow(), current: self.0.clone(), outcome: Default::default() }
    }
}

impl ExecutionLease {
    pub(crate) async fn scope<F: Future>(&self, future: F) -> F::Output {
        CURRENT.scope(self.clone(), future).await
    }
}

pub(crate) fn interrupted() -> bool {
    CURRENT.try_with(|lease| *lease.current.borrow() != lease.revision).unwrap_or(false)
}

/// Only race this against side-effect-free waits, never an executing tool.
pub(crate) async fn changed() {
    let Ok(lease) = CURRENT.try_with(Clone::clone) else { return std::future::pending().await; };
    let mut receiver = lease.current.subscribe();
    let _ = receiver.wait_for(|revision| *revision != lease.revision).await;
}

pub(crate) fn publish(outcome: serde_json::Value) {
    let _ = CURRENT.try_with(|lease| *lease.outcome.lock() = Some(outcome));
}

pub(crate) fn outcome() -> Option<serde_json::Value> {
    CURRENT.try_with(|lease| lease.outcome.lock().clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_input_invalidates_only_its_character_and_never_revives_an_old_lease() {
        let first = ExecutionControl::default();
        let other = ExecutionControl::default();
        let old = first.lease();
        old.scope(async {
            assert!(!interrupted());
            first.interrupt();
            assert!(interrupted());
            other.lease().scope(async { assert!(!interrupted()); }).await;
            first.lease().scope(async { assert!(!interrupted()); }).await;
            assert!(interrupted());
        }).await;
        assert!(!interrupted(), "work/background tasks without a lease are unaffected");
    }

    #[tokio::test]
    async fn interruption_wakes_model_waits_even_when_it_precedes_subscription() {
        let control = ExecutionControl::default();
        let lease = control.lease();
        control.interrupt();
        lease.scope(async { tokio::time::timeout(std::time::Duration::from_secs(1), changed()).await.unwrap(); }).await;
        let latest = control.lease();
        let interrupt = control.clone();
        latest.scope(async {
            let worker = tokio::spawn(async move { tokio::task::yield_now().await; interrupt.interrupt(); });
            tokio::time::timeout(std::time::Duration::from_secs(1), changed()).await.unwrap();
            worker.await.unwrap();
            assert!(interrupted());
        }).await;
    }
}
