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
}
impl Budget {
    pub fn new(total: Duration) -> Self {
        let start = Instant::now();
        Self {
            start,
            deadline: start + total,
            stages: Vec::new(),
        }
    }
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
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
        serde_json::json!({"elapsed_ms":self.start.elapsed().as_millis(),"deadline_exhausted":self.remaining().is_zero(),"stages":self.stages})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
