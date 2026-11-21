//! One shared value with bounded freshness and a single in-flight refresh.
use std::{future::Future, time::Duration};
use tokio::{sync::Mutex, time::Instant};

pub struct AsyncTtlCache<T> {
    entry: Mutex<Option<(Instant, T)>>,
}

impl<T: Clone> AsyncTtlCache<T> {
    pub const fn new() -> Self {
        Self { entry: Mutex::const_new(None) }
    }

    /// The initializer selects the TTL, including a short TTL for fallbacks.
    /// Cancelling a refresh leaves the next caller free to retry.
    pub async fn get_or_init<F, Fut>(&self, initialize: F) -> T
    where F: FnOnce() -> Fut, Fut: Future<Output = (T, Duration)> {
        let mut entry = self.entry.lock().await;
        if let Some((expires, value)) = entry.as_ref() {
            if Instant::now() < *expires { return value.clone(); }
        }
        let (value, ttl) = initialize().await;
        *entry = Some((Instant::now() + ttl, value.clone()));
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn concurrent_callers_share_one_refresh_and_cached_value() {
        let baseline_calls = AtomicUsize::new(0);
        let baseline_values = futures::future::join_all((0..8).map(|_| async {
            baseline_calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            vec!["voice"]
        })).await;
        let calls = AtomicUsize::new(0);
        let cache = AsyncTtlCache::new();
        let values = futures::future::join_all((0..8).map(|_| cache.get_or_init(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            (vec!["voice"], Duration::from_secs(3600))
        }))).await;
        assert!(values.iter().all(|value| value == &vec!["voice"]));
        assert_eq!(values, baseline_values);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(baseline_calls.load(Ordering::SeqCst), 8);
        println!("Voice-list fetches for 8 concurrent callers: uncached=8, shared_cache=1");
    }

    #[tokio::test]
    async fn fallback_cools_down_then_refreshes_successfully() {
        let calls = AtomicUsize::new(0);
        let cache = AsyncTtlCache::new();
        for _ in 0..8 {
            assert_eq!(cache.get_or_init(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                ("fallback", Duration::from_millis(100))
            }).await, "fallback");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(150)).await;
        for _ in 0..8 {
            assert_eq!(cache.get_or_init(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                ("online", Duration::from_secs(3600))
            }).await, "online");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancelled_refresh_does_not_poison_the_cache() {
        let cache = AsyncTtlCache::<usize>::new();
        assert!(tokio::time::timeout(Duration::from_millis(10), cache.get_or_init(|| async {
            std::future::pending::<(usize, Duration)>().await
        })).await.is_err());
        assert_eq!(cache.get_or_init(|| async { (42, Duration::from_secs(1)) }).await, 42);
    }
}
