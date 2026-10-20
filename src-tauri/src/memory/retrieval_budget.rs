//! One deadline across recall arms; a slow optional arm never erases completed recall.
use serde::Serialize;
use std::{
    future::Future,
    time::{Duration, Instant},
};

#[derive(Serialize)]
pub struct Stage {
    pub name: String,
    pub elapsed_ms: u128,
    pub status: &'static str,
}
pub struct Budget {
    start: Instant,
    deadline: Instant,
    stages: Vec<Stage>,
    dependency_wait: Duration,
}
impl Budget {
    pub fn new(total: Duration) -> Self {
        let start = Instant::now();
        Self {
            start,
            deadline: start + total,
            stages: Vec::new(),
            dependency_wait: Duration::ZERO,
        }
    }
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
    /// Awaiting shared semantic inputs is not time spent doing recall work.
    pub fn exclude_dependency_wait(&mut self, ready_at: Instant) {
        let waited = ready_at.elapsed();
        self.deadline += waited;
        self.dependency_wait += waited;
    }
    pub async fn run<T, F: Future<Output = T>>(
        &mut self,
        name: &str,
        cap: Duration,
        make: impl FnOnce() -> F,
    ) -> Option<T> {
        let allowance = self.remaining().min(cap);
        if allowance.is_zero() {
            self.stages.push(Stage {
                name: name.into(),
                elapsed_ms: 0,
                status: "skipped_deadline",
            });
            return None;
        }
        let start = Instant::now();
        let result = tokio::time::timeout(allowance, make()).await;
        self.stages.push(Stage {
            name: name.into(),
            elapsed_ms: start.elapsed().as_millis(),
            status: if result.is_ok() {
                "completed"
            } else {
                "timed_out"
            },
        });
        result.ok()
    }
    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({"elapsed_ms":self.start.elapsed().saturating_sub(self.dependency_wait).as_millis(),
            "dependency_wait_ms":self.dependency_wait.as_millis(),
            "wall_elapsed_ms":self.start.elapsed().as_millis(),
            "deadline_exhausted":self.remaining().is_zero(),"stages":self.stages})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn waiting_for_shared_semantics_does_not_consume_recall_budget() {
        let mut budget = Budget::new(Duration::from_millis(25));
        let ready_at = Instant::now();
        tokio::time::sleep(Duration::from_millis(35)).await;
        budget.exclude_dependency_wait(ready_at);
        assert_eq!(
            budget
                .run("enrichment", Duration::from_millis(10), || async { 7 })
                .await,
            Some(7)
        );
        assert!(budget.report()["dependency_wait_ms"].as_u64().unwrap() >= 30);
    }
    #[tokio::test]
    async fn slow_optional_stage_retains_completed_results_and_stops_at_deadline() {
        let mut budget = Budget::new(Duration::from_millis(25));
        let base = budget
            .run("base", Duration::from_millis(10), || async {
                vec!["qualified memory"]
            })
            .await
            .unwrap();
        assert!(budget
            .run("slow", Duration::from_secs(1), || async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                false
            })
            .await
            .is_none());
        let mut called = false;
        assert!(budget
            .run("later", Duration::from_secs(1), || {
                called = true;
                async { 42 }
            })
            .await
            .is_none());
        assert!(!called);
        assert_eq!(base, vec!["qualified memory"]);
        assert_eq!(budget.report()["stages"][1]["status"], "timed_out");
    }
}
