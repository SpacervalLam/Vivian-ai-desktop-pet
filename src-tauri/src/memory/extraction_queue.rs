//! Durable FIFO batches: even fewer than three turns survive restart and expire into work.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceTurn {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: f64,
    pub session_id: Option<String>,
    pub metadata: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch {
    pub id: String,
    pub sources: Vec<SourceTurn>,
    pub attempts: u32,
    pub retry_at: f64,
    #[serde(default)]
    pub analysis: Option<Value>,
    #[serde(default)]
    pub completed: BTreeSet<usize>,
    #[serde(default)]
    pub targets: std::collections::BTreeMap<usize, Vec<String>>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    pending: VecDeque<Batch>,
    partial: Vec<SourceTurn>,
    first_at: Option<f64>,
}
pub struct Queue {
    path: PathBuf,
    state: parking_lot::Mutex<State>,
    pub execution: std::sync::Arc<tokio::sync::Mutex<()>>,
}
impl Queue {
    pub fn shared(path: impl AsRef<Path>) -> std::sync::Arc<Self> {
        static REGISTRY: once_cell::sync::Lazy<
            parking_lot::Mutex<std::collections::HashMap<PathBuf, std::sync::Weak<Queue>>>,
        > = once_cell::sync::Lazy::new(Default::default);
        let path = path.as_ref().to_path_buf();
        let mut registry = REGISTRY.lock();
        if let Some(queue) = registry.get(&path).and_then(std::sync::Weak::upgrade) {
            return queue;
        }
        let queue = std::sync::Arc::new(Self::new(&path));
        registry.insert(path, std::sync::Arc::downgrade(&queue));
        queue
    }
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let state = crate::utils::fs::load_json_or_backup(&path)
            .or_else(|| {
                crate::utils::fs::load_json_or_backup(&path.with_extension("previous.json"))
            })
            .unwrap_or_default();
        Self {
            path,
            state: parking_lot::Mutex::new(state),
            execution: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }
    fn change<R>(&self, mutate: impl FnOnce(&mut State) -> R) -> Result<R, String> {
        let mut state = self.state.lock();
        let mut next = state.clone();
        let result = mutate(&mut next);
        let text = serde_json::to_string(&next).map_err(|e| e.to_string())?;
        let previous = serde_json::to_string(&*state).map_err(|e| e.to_string())?;
        crate::utils::fs::write_atomic(&self.path.with_extension("previous.json"), &previous)
            .map_err(|e| e.to_string())?;
        crate::utils::fs::write_atomic(&self.path, &text).map_err(|e| e.to_string())?;
        *state = next;
        Ok(result)
    }
    pub fn sources(&self) -> Vec<SourceTurn> {
        let state = self.state.lock();
        state
            .pending
            .iter()
            .flat_map(|b| b.sources.iter())
            .chain(state.partial.iter())
            .cloned()
            .collect()
    }
    pub fn clear(&self) -> Result<(), String> {
        let mut state = self.state.lock();
        let next = State::default();
        let text = serde_json::to_string(&next).map_err(|e| e.to_string())?;
        // Clear both checkpoints before reset can proceed; the worker holds execution.
        crate::utils::fs::write_atomic(&self.path.with_extension("previous.json"), &text)
            .map_err(|e| e.to_string())?;
        crate::utils::fs::write_atomic(&self.path, &text).map_err(|e| e.to_string())?;
        *state = next;
        Ok(())
    }
    pub fn enqueue(&self, sources: Vec<SourceTurn>, now: f64) -> Result<(), String> {
        self.change(|state| {
            for source in sources {
                if state.partial.iter().any(|s| s.id == source.id)
                    || state
                        .pending
                        .iter()
                        .any(|b| b.sources.iter().any(|s| s.id == source.id))
                {
                    continue;
                }
                state.first_at.get_or_insert(now);
                state.partial.push(source);
            }
            Self::flush(state, now, false);
        })
    }
    fn flush(state: &mut State, now: f64, aged: bool) {
        if state.partial.is_empty() {
            return;
        }
        if state.partial.len() < 6 && !(aged && state.first_at.is_some_and(|t| now - t >= 60.0)) {
            return;
        }
        let sources = std::mem::take(&mut state.partial);
        state.first_at = None;
        // Identity comes from immutable host message IDs, not generated memory text.
        let id = sources
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>()
            .join("/");
        state.pending.push_back(Batch {
            id,
            sources,
            attempts: 0,
            retry_at: now,
            analysis: None,
            completed: BTreeSet::new(),
            targets: Default::default(),
        });
    }
    pub fn ready(&self, now: f64) -> Result<Option<Batch>, String> {
        if self.state.lock().first_at.is_some_and(|t| now - t >= 60.0) {
            self.change(|s| Self::flush(s, now, true))?;
        }
        Ok(self
            .state
            .lock()
            .pending
            .front()
            .filter(|b| b.retry_at <= now)
            .cloned())
    }
    pub fn analysis(&self, id: &str, value: Value) -> Result<(), String> {
        self.change(|s| {
            if let Some(b) = s.pending.front_mut().filter(|b| b.id == id) {
                b.analysis = Some(value);
            }
        })
    }
    pub fn targets(&self, id: &str, index: usize, targets: Vec<String>) -> Result<(), String> {
        self.change(|s| {
            if let Some(b) = s.pending.front_mut().filter(|b| b.id == id) {
                b.targets.entry(index).or_insert(targets);
            }
        })
    }
    pub fn complete_operation(&self, id: &str, index: usize) -> Result<(), String> {
        self.change(|s| {
            if let Some(b) = s.pending.front_mut().filter(|b| b.id == id) {
                b.completed.insert(index);
            }
        })
    }
    pub fn finish(&self, id: &str) -> Result<(), String> {
        self.change(|s| {
            if s.pending.front().is_some_and(|b| b.id == id) {
                s.pending.pop_front();
            }
        })
    }
    pub fn retry(&self, id: &str, now: f64) -> Result<(), String> {
        self.change(|s| {
            if let Some(b) = s.pending.front_mut().filter(|b| b.id == id) {
                b.attempts = b.attempts.saturating_add(1);
                b.retry_at = now + (2f64.powi(b.attempts.min(8) as i32)).min(300.0);
            }
        })
    }
}

/// A candidate can refer only to the current batch's actual speaker and original quote.
pub fn source_for_quote<'a>(
    sources: &'a [SourceTurn],
    subject: &str,
    quote: &str,
    char_id: &str,
) -> Option<&'a SourceTurn> {
    if quote.trim().is_empty() {
        return None;
    }
    sources.iter().rev().find(|s| {
        let speaker = s.metadata["speaker"]
            .as_str()
            .unwrap_or(if s.role == "user" { "user" } else { char_id });
        let permitted = match subject {
            "user" => speaker == "user" && s.role == "user",
            "self" => speaker == char_id && s.role == "assistant",
            _ => false,
        };
        permitted && s.content.contains(quote)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(id: &str) -> SourceTurn {
        SourceTurn {
            id: id.into(),
            role: "user".into(),
            content: "相同的偏好".into(),
            timestamp: 1.0,
            session_id: Some("session".into()),
            metadata: serde_json::json!({"speaker":"user","channel":"direct"}),
        }
    }
    #[test]
    fn partial_turns_restart_retry_and_receipts_are_durable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let queue = Queue::new(&path);
        queue.enqueue(vec![source("first")], 0.0).unwrap();
        assert!(queue.ready(59.0).unwrap().is_none());
        drop(queue);
        let queue = Queue::new(&path);
        let job = queue.ready(60.0).unwrap().unwrap();
        queue
            .analysis(&job.id, serde_json::json!({"operations":[]}))
            .unwrap();
        queue
            .targets(&job.id, 0, vec!["original target".into()])
            .unwrap();
        queue.complete_operation(&job.id, 0).unwrap();
        queue.retry(&job.id, 60.0).unwrap();
        assert!(queue.ready(61.0).unwrap().is_none());
        drop(queue);
        let queue = Queue::new(&path);
        let job = queue.ready(62.0).unwrap().unwrap();
        assert_eq!(job.attempts, 1);
        assert!(job.completed.contains(&0));
        assert!(job.analysis.is_some());
        assert_eq!(job.targets[&0], vec!["original target"]);
        queue.finish("stale").unwrap();
        assert!(queue.ready(62.0).unwrap().is_some());
        queue.finish(&job.id).unwrap();
        assert!(queue.ready(100.0).unwrap().is_none());
    }
    #[test]
    fn corrupted_queue_uses_backup_and_clear_cannot_resurrect_old_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let queue = Queue::new(&path);
        queue.enqueue(vec![source("first")], 0.0).unwrap();
        let job = queue.ready(60.0).unwrap().unwrap();
        queue.retry(&job.id, 60.0).unwrap();
        drop(queue);
        std::fs::write(&path, "broken").unwrap();
        let queue = Queue::new(&path);
        assert!(queue.ready(100.0).unwrap().is_some());
        queue.clear().unwrap();
        drop(queue);
        std::fs::write(&path, "broken again").unwrap();
        let queue = Queue::new(&path);
        assert!(queue.sources().is_empty());
    }
    #[test]
    fn quotation_is_scoped_to_batch_and_actual_speaker() {
        let mut a = source("older");
        let mut b = source("actual");
        b.timestamp = 2.0;
        let sources = vec![a.clone(), b];
        assert_eq!(
            source_for_quote(&sources, "user", "偏好", "vivian")
                .unwrap()
                .id,
            "actual"
        );
        a.metadata["speaker"] = Value::String("nana".into());
        assert!(source_for_quote(&[a], "user", "偏好", "vivian").is_none());
        assert!(source_for_quote(&sources, "self", "偏好", "vivian").is_none());
    }
    #[test]
    fn failed_persistence_does_not_acknowledge_or_clear_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let queue = Queue::new(&path);
        queue.enqueue(vec![source("first")], 0.0).unwrap();
        let batch = queue.ready(60.0).unwrap().unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(queue.finish(&batch.id).is_err());
        assert!(queue.ready(60.0).unwrap().is_some());
    }
}
