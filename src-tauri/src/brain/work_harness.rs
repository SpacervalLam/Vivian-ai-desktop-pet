//! Small policies shared by work execution; none require another model call.

pub const CORE_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "run_command",
    "grep_search",
    "glob",
    "list_dir",
    "read_spilled_result",
    "web_search",
    "tool_list",
    "tool_search",
    "work_ask_user",
    "work_todo_write",
    "goal",
];
pub const MAX_PARALLEL_READS: usize = 4;
pub const STEP_TIMEOUT_SECS: u64 = 300;
pub const MAX_PROVIDER_ATTEMPTS: usize = 6;

/// Only independent observations may share a group. Discovery and task control
/// can change runtime state even when their tool metadata says read-only.
pub fn parallel_read(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "grep_search"
            | "glob"
            | "list_dir"
            | "web_search"
            | "read_content"
            | "read_spilled_result"
            | "read_image"
    )
}

pub fn contiguous_reads<'a>(calls: impl Iterator<Item = (&'a str, bool)>) -> usize {
    calls
        .take_while(|(name, read_only)| *read_only && parallel_read(name))
        .count()
}

/// Poll a bounded rolling pool and publish each completion immediately. The
/// caller commits results in authored order after this read-only group settles.
pub async fn run_reads<C, R, Run, Fut, Done>(
    calls: Vec<C>,
    concurrency: usize,
    run: Run,
    mut done: Done,
) -> Vec<(R, u64)>
where
    Run: Fn(C) -> Fut,
    Fut: std::future::Future<Output = R>,
    Done: FnMut(&R, u64),
{
    use futures::StreamExt;
    let mut running = futures::stream::iter(calls.into_iter().map(|call| {
        let future = run(call);
        async move {
            let started = std::time::Instant::now();
            let result = future.await;
            (result, started.elapsed().as_millis() as u64)
        }
    }))
    .buffer_unordered(concurrency.max(1));
    let mut results = Vec::new();
    while let Some((result, duration)) = running.next().await {
        done(&result, duration);
        results.push((result, duration));
    }
    results
}

#[derive(Default)]
pub struct CompactionGate {
    failures: usize,
    window: u64,
}
impl CompactionGate {
    pub fn permits(&mut self, window: u64) -> bool {
        if self.window != window {
            self.window = window;
            self.failures = 0;
        }
        self.failures < 3
    }
    pub fn settled(&mut self, committed: bool) {
        if committed {
            self.failures = 0;
        } else {
            self.failures += 1;
        }
    }
}

/// Keep the most actionable intervention instead of overwriting it later.
#[derive(Default)]
pub struct Hint {
    pending: Option<(u8, String)>,
}
impl Hint {
    pub fn offer(&mut self, priority: u8, text: String) {
        if self
            .pending
            .as_ref()
            .map_or(true, |(old, _)| priority > *old)
        {
            self.pending = Some((priority, text));
        }
    }
    pub fn take(&mut self) -> Option<String> {
        self.pending.take().map(|(_, text)| text)
    }
    pub fn is_none(&self) -> bool {
        self.pending.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_changes_are_barriers_between_reads() {
        let calls = [
            "read_file",
            "grep_search",
            "edit_file",
            "read_file",
            "tool_search",
            "read_file",
        ];
        let groups: Vec<_> = calls
            .split(|name| !parallel_read(name))
            .map(|g| g.len())
            .collect();
        assert_eq!(groups, [2, 1, 1]);
        assert!(!parallel_read("work_ask_user"));
        assert!(!parallel_read("run_command"));
        assert_eq!(
            contiguous_reads(
                [
                    ("edit_file", false),
                    ("read_file", true),
                    ("read_file", true)
                ]
                .into_iter()
            ),
            0
        );
        assert_eq!(
            contiguous_reads(
                [
                    ("read_file", true),
                    ("grep_search", true),
                    ("edit_file", false),
                    ("read_file", true)
                ]
                .into_iter()
            ),
            2
        );
        assert_eq!(
            contiguous_reads([("tool_search", true), ("read_file", true)].into_iter()),
            0
        );
    }
    #[test]
    fn compaction_failures_stop_until_a_new_run_or_window() {
        let mut gate = CompactionGate::default();
        for _ in 0..3 {
            assert!(gate.permits(128000));
            gate.settled(false);
        }
        assert!(!gate.permits(128000));
        assert!(gate.permits(256000));
        gate.settled(true);
        assert!(gate.permits(256000));
    }
    #[test]
    fn later_advice_cannot_erase_a_blocker() {
        let mut hints = Hint::default();
        hints.offer(3, "failure".into());
        hints.offer(1, "finish soon".into());
        assert_eq!(hints.take().as_deref(), Some("failure"));
        assert!(hints.is_none());
    }

    #[tokio::test]
    async fn fast_results_are_published_before_the_batch_finishes() {
        use std::sync::Arc;
        let release = Arc::new(tokio::sync::Notify::new());
        let mut order = Vec::new();
        let run = |slow| {
            let release = release.clone();
            async move {
                if slow {
                    release.notified().await;
                }
                slow
            }
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run_reads(vec![true, false], 2, run, |slow, _| {
                order.push(*slow);
                if !slow {
                    release.notify_one();
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(order, [false, true]);
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn read_pool_never_exceeds_the_configured_concurrency() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let result = run_reads(
            (0..20).collect(),
            4,
            |n| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    n
                }
            },
            |_, _| {},
        )
        .await;
        assert_eq!(result.len(), 20);
        assert!(peak.load(Ordering::SeqCst) <= 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}
