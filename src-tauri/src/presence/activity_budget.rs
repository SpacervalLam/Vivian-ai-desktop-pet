//! Background learning and consolidation share one slot per character.
use std::collections::HashSet;
use once_cell::sync::Lazy;
use parking_lot::Mutex;

static ACTIVE: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

pub struct ActivityPermit(String);
pub fn try_acquire(char_id: &str) -> Option<ActivityPermit> {
    ACTIVE.lock().insert(char_id.into()).then(|| ActivityPermit(char_id.into()))
}
impl Drop for ActivityPermit {
    fn drop(&mut self) { ACTIVE.lock().remove(&self.0); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_work_is_exclusive_per_character_and_released_on_drop() {
        let a = try_acquire("activity-a").unwrap();
        assert!(try_acquire("activity-a").is_none());
        let b = try_acquire("activity-b").unwrap();
        drop(a);
        assert!(try_acquire("activity-a").is_some());
        drop(b);
    }
    #[tokio::test]
    async fn aborting_a_task_releases_the_slot() {
        let permit = try_acquire("aborted-activity").unwrap();
        let task = tokio::spawn(async move { let _permit = permit; std::future::pending::<()>().await; });
        task.abort();
        let _ = task.await;
        assert!(try_acquire("aborted-activity").is_some());
    }
}
