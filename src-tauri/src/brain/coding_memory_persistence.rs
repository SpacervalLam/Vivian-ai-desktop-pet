//! Durable background materials and optimistic commits for project memory.
use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Weak};

static WRITES: LazyLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn write_lock(path: &Path) -> Arc<Mutex<()>> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    // Resolve the existing ancestor even before .vivian has been created.
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    let mut key = loop {
        if let Ok(resolved) = ancestor.canonicalize() {
            break resolved;
        }
        match (ancestor.file_name(), ancestor.parent()) {
            (Some(name), Some(parent)) => {
                suffix.push(name.to_os_string());
                ancestor = parent;
            }
            _ => break absolute.clone(),
        }
    };
    for component in suffix.into_iter().rev() {
        key.push(component);
    }
    #[cfg(windows)]
    let key = PathBuf::from(key.to_string_lossy().to_lowercase());
    let mut locks = WRITES.lock();
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

pub fn transaction<T>(
    path: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock = write_lock(path);
    let _guard = lock.lock();
    action()
}

/// Acquire the commit lock before collecting state so an older snapshot cannot
/// be written after a newer one by another caller.
pub fn write_snapshot<T: Serialize>(
    path: &Path,
    collect: impl FnOnce() -> T,
) -> Result<(), String> {
    let lock = write_lock(path);
    let _guard = lock.lock();
    let text = serde_json::to_string_pretty(&collect()).map_err(|e| e.to_string())?;
    crate::utils::fs::write_atomic(path, &text).map_err(|e| e.to_string())
}

pub fn enqueue<T: Serialize>(dir: &Path, job: &T) -> Result<(), String> {
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let path = dir.join(format!("{created:032}-{}.json", uuid::Uuid::new_v4()));
    let text = serde_json::to_string(job).map_err(|e| e.to_string())?;
    crate::utils::fs::write_atomic(&path, &text).map_err(|e| e.to_string())
}

pub fn pending<T: DeserializeOwned>(dir: &Path) -> Vec<(PathBuf, T)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| crate::utils::fs::load_json_or_backup(&path).map(|job| (path, job)))
        .collect()
}

/// Check the model's snapshot while holding the same lock used by manual writes.
/// An external edit before commit is also detected. No filesystem lock crosses await.
pub fn update(
    path: &Path,
    expected: Option<Option<&str>>,
    edit: impl FnOnce(Option<&str>) -> String,
) -> Result<(), String> {
    let lock = write_lock(path);
    let _guard = lock.lock();
    let current = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let snapshot = current.as_deref();
    if expected.is_some_and(|expected| snapshot != expected) {
        return Err("项目记忆在提炼期间已变化，请重试".into());
    }
    crate::utils::fs::write_atomic(path, &edit(current.as_deref())).map_err(|e| e.to_string())
}

/// Entries and their replay receipt are one atomic commit, even on stale replay.
pub fn commit_once(
    path: &Path,
    expected: Option<&str>,
    id: &str,
    edit: impl FnOnce(Option<&str>) -> String,
) -> Result<bool, String> {
    if !super::coding_memory_records::valid_id(id) {
        return Err("记忆任务 ID 无效".into());
    }
    transaction(path, || {
        let current = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        if super::coding_memory_records::committed(current.as_deref().unwrap_or(""), id) {
            return Ok(false);
        }
        if current.as_deref() != expected {
            return Err("项目记忆在提炼期间已变化，请重试".into());
        }
        let body = format!(
            "{}\n{}\n",
            edit(current.as_deref()),
            super::coding_memory_records::job_marker(id)
        );
        crate::utils::fs::write_atomic(path, &body).map_err(|e| e.to_string())?;
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_after_commit_before_queue_removal_cannot_append_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        let id = "c".repeat(64);
        assert!(commit_once(&path, None, &id, |_| "- experience".into()).unwrap());
        assert!(!commit_once(&path, None, &id, |_| panic!(
            "replay must not render another append"
        ))
        .unwrap());
        assert_eq!(
            std::fs::read_to_string(&path)
                .unwrap()
                .matches("experience")
                .count(),
            1
        );
        assert!(commit_once(&path, None, &"d".repeat(64), |_| "stale".into()).is_err());
    }

    #[test]
    fn pending_materials_survive_reopening_until_success() {
        let dir = tempfile::tempdir().unwrap();
        enqueue(dir.path(), &vec!["verified command", "remaining work"]).unwrap();
        let jobs = pending::<Vec<String>>(dir.path());
        assert_eq!(jobs.len(), 1);
        assert_eq!(pending::<Vec<String>>(dir.path())[0].1, jobs[0].1);
        std::fs::remove_file(&jobs[0].0).unwrap();
        assert!(pending::<Vec<String>>(dir.path()).is_empty());
    }

    #[test]
    fn stale_model_result_cannot_overwrite_a_manual_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        update(&path, None, |_| "old".into()).unwrap();
        update(&path, None, |_| "user correction".into()).unwrap();
        assert!(update(&path, Some(Some("old")), |_| "stale model result".into()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user correction");
    }

    #[test]
    fn concurrent_appends_do_not_lose_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::thread::scope(|scope| {
            for i in 0..12 {
                let path = &path;
                scope.spawn(move || {
                    update(path, None, |old| {
                        format!("{}entry-{i}\n", old.unwrap_or(""))
                    })
                    .unwrap()
                });
            }
        });
        assert_eq!(std::fs::read_to_string(path).unwrap().lines().count(), 12);
    }

    #[test]
    fn version_check_detects_whitespace_and_empty_file_creation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        update(&path, None, |_| String::new()).unwrap();
        assert!(update(&path, Some(None), |_| "model".into()).is_err());
        update(&path, None, |_| "old\n".into()).unwrap();
        assert!(update(&path, Some(Some("old")), |_| "model".into()).is_err());
        update(&path, Some(Some("old\n")), |_| "new".into()).unwrap();
    }

    #[test]
    fn unrelated_projects_can_commit_while_one_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first/memory.md");
        let second = dir.path().join("second/memory.md");
        let lock = write_lock(&first);
        let guard = lock.lock();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            tx.send(update(&second, None, |_| "independent".into()))
                .unwrap();
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(2));
        drop(guard);
        thread.join().unwrap();
        result.unwrap().unwrap();
    }

    #[test]
    fn snapshot_is_collected_only_after_commit_lock_is_acquired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let lock = write_lock(&path);
        let guard = lock.lock();
        let state = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let thread_state = state.clone();
        let thread_path = path.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (collected_tx, collected_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            write_snapshot(&thread_path, || {
                let snapshot = thread_state.load(std::sync::atomic::Ordering::SeqCst);
                collected_tx.send(()).unwrap();
                snapshot
            })
        });
        started_rx.recv().unwrap();
        let premature = collected_rx.recv_timeout(std::time::Duration::from_millis(100));
        state.store(2, std::sync::atomic::Ordering::SeqCst);
        drop(guard);
        thread.join().unwrap().unwrap();
        assert!(matches!(
            premature,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "2");
    }

    #[test]
    fn lock_identity_is_stable_when_the_project_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".vivian/memory.md");
        let before = write_lock(&path);
        update(&path, None, |_| "created".into()).unwrap();
        assert!(Arc::ptr_eq(&before, &write_lock(&path)));
    }
}
