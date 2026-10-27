//! Query embedding is optional and runs outside memory locks and async workers.
use std::{sync::{Arc, LazyLock}, time::Duration};
use super::embedding::MemoryEmbeddingProvider;

static REMOTE_QUERIES: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(4)));

pub async fn prepare(provider: Arc<dyn MemoryEmbeddingProvider>, query: String) -> Option<Vec<f32>> {
    prepare_with_budget(provider, query, Duration::from_millis(300)).await
}

async fn prepare_with_budget(provider: Arc<dyn MemoryEmbeddingProvider>, query: String,
    budget: Duration) -> Option<Vec<f32>> {
    let remote = provider.is_remote();
    let work = async move {
        let permit = if remote { Some(REMOTE_QUERIES.clone().acquire_owned().await.ok()?) } else { None };
        // The worker owns its permit even if its caller times out. Repeated timeouts
        // cannot leave an unbounded number of synchronous requests running.
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            provider.embed(&query)
        }).await;
        match result {
            Ok(Ok(value)) => Some(value),
            result => { tracing::debug!(?result, "查询嵌入不可用，使用本地召回"); None }
        }
    };
    // Leave most of the pipeline's 900ms base budget for local recall.
    if remote {
        tokio::time::timeout(budget, work).await.unwrap_or_else(|_| {
            tracing::debug!("查询嵌入超时，使用本地召回"); None
        })
    } else { work.await }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Slow;
    impl MemoryEmbeddingProvider for Slow {
        fn dimension(&self) -> usize { 2 }
        fn is_remote(&self) -> bool { true }
        fn embed(&self, _: &str) -> crate::error::VivianResult<Vec<f32>> {
            std::thread::sleep(Duration::from_millis(200));
            Ok(vec![1.0, 0.0])
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_embedding_yields_and_returns_before_its_budget() {
        let start = std::time::Instant::now();
        let timer = tokio::spawn(async { tokio::time::sleep(Duration::from_millis(5)).await; });
        assert!(prepare_with_budget(Arc::new(Slow), "query".into(), Duration::from_millis(20)).await.is_none());
        timer.await.unwrap();
        assert!(start.elapsed() < Duration::from_millis(150));
    }
}
