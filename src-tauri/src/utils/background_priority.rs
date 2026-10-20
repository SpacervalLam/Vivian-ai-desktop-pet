//! Foreground input preempts optional work. Dropping background futures releases their permits.
use std::future::Future;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::watch;

pub struct BackgroundPriority {
    foreground: AtomicUsize,
    changed: watch::Sender<u64>,
}

impl Default for BackgroundPriority {
    fn default() -> Self {
        Self {
            foreground: AtomicUsize::new(0),
            changed: watch::channel(0).0,
        }
    }
}

pub struct ForegroundGuard(Arc<BackgroundPriority>);

impl BackgroundPriority {
    pub fn enter(self: &Arc<Self>) -> ForegroundGuard {
        self.foreground.fetch_add(1, Ordering::AcqRel);
        self.interrupt();
        ForegroundGuard(self.clone())
    }

    pub fn interrupt(&self) {
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    pub fn is_idle(&self) -> bool {
        self.foreground.load(Ordering::Acquire) == 0
    }
    pub fn revision(&self) -> u64 {
        *self.changed.borrow()
    }

    pub async fn run<F: Future>(&self, future: F) -> Option<F::Output> {
        let mut changed = self.changed.subscribe();
        let revision = *changed.borrow_and_update();
        if !self.is_idle() {
            return None;
        }
        tokio::select! {
            biased;
            _ = changed.changed() => None,
            output = future => {
                (self.is_idle() && self.revision() == revision).then_some(output)
            }
        }
    }
}

impl Drop for ForegroundGuard {
    fn drop(&mut self) {
        self.0.foreground.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::sync::{oneshot, Semaphore};

    #[tokio::test]
    async fn new_input_cancels_slow_background_and_releases_its_resource() {
        let priority = Arc::new(BackgroundPriority::default());
        let resource = Arc::new(Semaphore::new(1));
        let (started, ready) = oneshot::channel();
        let background_priority = priority.clone();
        let background_resource = resource.clone();
        let background = tokio::spawn(async move {
            background_priority
                .run(async move {
                    let _permit = background_resource.acquire_owned().await.unwrap();
                    let _ = started.send(());
                    std::future::pending::<()>().await;
                })
                .await
        });
        ready.await.unwrap();
        let before = Instant::now();
        let _user = priority.enter();
        let _foreground_resource =
            tokio::time::timeout(Duration::from_millis(250), resource.acquire())
                .await
                .unwrap()
                .unwrap();
        assert!(background.await.unwrap().is_none());
        eprintln!(
            "Foreground preemption released stalled background resource in {:?}",
            before.elapsed()
        );
    }

    #[tokio::test]
    async fn active_or_overlapping_user_requests_never_start_optional_work() {
        let priority = Arc::new(BackgroundPriority::default());
        let first = priority.enter();
        let second = priority.enter();
        assert!(priority
            .run(async { panic!("optional work ran during foreground input") })
            .await
            .is_none());
        drop(first);
        assert!(!priority.is_idle());
        drop(second);
        assert_eq!(priority.run(async { 7 }).await, Some(7));
    }

    #[tokio::test]
    async fn input_wins_when_a_background_result_and_preemption_are_both_ready() {
        let priority = Arc::new(BackgroundPriority::default());
        let p = priority.clone();
        let result = priority
            .run(async move {
                let user = p.enter();
                drop(user);
                "stale decision"
            })
            .await;
        assert!(result.is_none());
    }
}
