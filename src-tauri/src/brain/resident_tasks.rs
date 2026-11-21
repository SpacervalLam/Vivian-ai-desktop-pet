//! Character-scoped ownership of resident tasks, independent of any webview.
use std::collections::HashMap;

pub struct TaskEntry<S> {
    pub task: tokio::task::JoinHandle<()>,
    pub state: S,
}
pub struct ResidentTasks<S> {
    entries: HashMap<String, TaskEntry<S>>,
}
impl<S> Default for ResidentTasks<S> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}
impl<S: Clone> ResidentTasks<S> {
    pub fn get(&self, id: &str) -> Option<&TaskEntry<S>> {
        self.entries.get(id)
    }
    pub fn retain(&mut self, ids: &[String]) {
        self.entries.retain(|id, entry| {
            if ids.contains(id) {
                true
            } else {
                entry.task.abort();
                false
            }
        });
    }
    pub fn ensure(
        &mut self,
        id: &str,
        init: impl FnOnce(Option<S>) -> S,
        spawn: impl FnOnce(S) -> tokio::task::JoinHandle<()>,
    ) -> bool {
        if self
            .entries
            .get(id)
            .is_some_and(|entry| !entry.task.is_finished())
        {
            return false;
        }
        let previous = self.entries.remove(id).map(|entry| entry.state);
        let state = init(previous);
        let task = spawn(state.clone());
        self.entries.insert(id.into(), TaskEntry { task, state });
        true
    }
}
impl<S> Drop for ResidentTasks<S> {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            entry.task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;
    #[tokio::test]
    async fn one_owner_per_character_and_a_waiting_brain_does_not_block_its_peer() {
        let mut tasks = ResidentTasks::default();
        let waiting = Arc::new(Notify::new());
        let count = Arc::new(AtomicUsize::new(0));
        assert!(tasks.ensure(
            "vivian",
            |_| count.clone(),
            |_| {
                let waiting = waiting.clone();
                tokio::spawn(async move {
                    waiting.notified().await;
                })
            }
        ));
        assert!(!tasks.ensure(
            "vivian",
            |_| panic!("must not initialize twice"),
            |_| panic!("must not spawn twice")
        ));
        let (sent, received) = tokio::sync::oneshot::channel();
        assert!(tasks.ensure(
            "nana",
            |_| count.clone(),
            |count| tokio::spawn(async move {
                count.fetch_add(1, Ordering::SeqCst);
                let _ = sent.send(());
            })
        ));
        received.await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!tasks.get("vivian").unwrap().task.is_finished());
        tasks.retain(&["vivian".into()]);
        assert!(tasks.get("nana").is_none());
    }
    #[tokio::test]
    async fn finished_tasks_resume_with_their_own_state_and_live_tasks_are_retained() {
        let mut tasks = ResidentTasks::default();
        tasks.ensure("vivian", |_| 7, |_| tokio::spawn(async {}));
        while !tasks.get("vivian").unwrap().task.is_finished() {
            tokio::task::yield_now().await;
        }
        assert!(tasks.ensure(
            "vivian",
            |previous| previous.unwrap() + 1,
            |_| tokio::spawn(std::future::pending::<()>())
        ));
        assert_eq!(tasks.get("vivian").unwrap().state, 8);
        assert!(!tasks.ensure("vivian", |_| 0, |_| panic!("live actor must not restart")));
    }
}
