//! Request-scoped gather: dependent work starts as soon as its own inputs are ready.
use futures::FutureExt;
use std::future::Future;

/// Semantic consumers share one result; base recall does not wait for semantic analysis.
/// All branches belong to this future, so cancellation drops them together.
pub async fn gather<S, B, P, M, SF, BF, PF, MF>(
    semantic: SF,
    base_recall: BF,
    prepare: impl FnOnce(S) -> PF,
    finish_recall: impl FnOnce(S, B) -> MF,
) -> (P, M)
where
    S: Clone,
    SF: Future<Output = S>,
    BF: Future<Output = B>,
    PF: Future<Output = P>,
    MF: Future<Output = M>,
{
    let semantic = semantic.shared();
    let for_prompt = semantic.clone();
    let prompt = async move { prepare(for_prompt.await).await };
    let recall = async move {
        let (semantic, base) = tokio::join!(semantic, base_recall);
        finish_recall(semantic, base).await
    };
    tokio::join!(prompt, recall)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    use tokio::sync::{oneshot, Semaphore};

    #[tokio::test(flavor = "current_thread")]
    async fn base_recall_starts_before_semantics_and_candidates_do_not_wait_for_recall() {
        let (base_started, semantic_may_finish) = oneshot::channel();
        let (candidates_started, base_may_finish) = oneshot::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let work = gather(
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                semantic_may_finish.await.unwrap();
                "current semantic labels"
            },
            async {
                base_started.send(()).unwrap();
                base_may_finish.await.unwrap();
                "rewritten-query recall"
            },
            |semantic| async move {
                assert_eq!(semantic, "current semantic labels");
                candidates_started.send(()).unwrap();
                "tool/example/web candidates"
            },
            |semantic, base| async move { (semantic, base) },
        );
        let (prompt, recall) = tokio::time::timeout(Duration::from_secs(1), work)
            .await
            .expect("a false dependency serialized or deadlocked gather");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(prompt, "tool/example/web candidates");
        assert_eq!(
            recall,
            ("current semantic labels", "rewritten-query recall")
        );
    }

    #[tokio::test]
    async fn failed_semantics_are_shared_as_data_and_all_completed_inputs_are_retained() {
        let expected: Result<&str, &str> = Err("embedding unavailable");
        let (prompt, recall) = gather(
            async { expected },
            async { "actual memory" },
            |result| async move { (result, "core tools remain available") },
            |result, memory| async move { (result, memory) },
        )
        .await;
        assert_eq!(prompt, (expected, "core tools remain available"));
        assert_eq!(recall, (expected, "actual memory"));
    }

    #[tokio::test]
    async fn cancelling_gather_drops_all_async_branches_and_their_permits() {
        let resources = Arc::new(Semaphore::new(2));
        let memory_resource = resources.clone();
        let prompt_resource = resources.clone();
        let (base_started, base_ready) = oneshot::channel();
        let (prompt_started, prompt_ready) = oneshot::channel();
        let task = tokio::spawn(gather(
            async {},
            async move {
                let _permit = memory_resource.acquire_owned().await.unwrap();
                base_started.send(()).unwrap();
                std::future::pending::<()>().await;
            },
            |_| async move {
                let _permit = prompt_resource.acquire_owned().await.unwrap();
                prompt_started.send(()).unwrap();
                std::future::pending::<()>().await;
            },
            |_, _| async {},
        ));
        base_ready.await.unwrap();
        prompt_ready.await.unwrap();
        assert_eq!(resources.available_permits(), 0);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(resources.available_permits(), 2);
    }

    async fn delay(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "controlled pre-API scheduling measurement; no configured services are used"]
    async fn benchmark_sequential_vs_dependency_gather() {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for round in 0..7 {
            for parallel in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let start = Instant::now();
                let context = async {
                    if parallel {
                        gather(
                            delay(40),
                            async {
                                delay(10).await;
                                delay(55).await
                            },
                            |_| async {
                                tokio::join!(delay(45), delay(35), delay(25));
                            },
                            |_, _| delay(15),
                        )
                        .await;
                    } else {
                        // Former order: rewrite || semantics -> memory -> web -> tools -> examples.
                        tokio::join!(delay(40), delay(10));
                        delay(55).await;
                        delay(15).await;
                        delay(45).await;
                        delay(35).await;
                        delay(25).await;
                    }
                };
                tokio::join!(context, delay(30)); // Stable prompt candidates already overlapped.
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if parallel {
                    new.push(elapsed);
                } else {
                    old.push(elapsed);
                }
            }
        }
        old.sort_by(f64::total_cmp);
        new.sort_by(f64::total_cmp);
        eprintln!("controlled pre-API gather (7 alternating runs): sequential median={:.2}ms range={:.2}..{:.2}; dependency gather median={:.2}ms range={:.2}..{:.2}; no network/model latency measured", old[3], old[0], old[6], new[3], new[0], new[6]);
    }
}
