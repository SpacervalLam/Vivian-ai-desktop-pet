//! Cache prompt projections by exact file metadata; explicit editing still reads
//! the source afresh and never uses this cache for optimistic write checks.
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::SystemTime,
};

pub struct Snapshot {
    pub pinned: String,
    pub navigation: Option<String>,
}
type Stamp = (SystemTime, u64);
static CACHE: LazyLock<Mutex<HashMap<PathBuf, (Stamp, Arc<Snapshot>)>>> =
    LazyLock::new(Default::default);

pub fn read(path: &Path) -> Option<Arc<Snapshot>> {
    let metadata = std::fs::metadata(path).ok()?;
    let stamp = (metadata.modified().ok()?, metadata.len());
    let mut cache = CACHE.lock();
    if let Some((previous, value)) = cache.get(path) {
        if *previous == stamp {
            return Some(value.clone());
        }
    }
    let raw = std::fs::read_to_string(path).ok()?;
    let value = Arc::new(Snapshot {
        pinned: super::coding_memory_records::pinned_rules(&raw),
        navigation: super::coding_memory_navigation::navigation(&raw),
    });
    if cache.len() >= 32 {
        cache.clear();
    }
    cache.insert(path.into(), (stamp, value.clone()));
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_memory_reuses_projection_and_edits_are_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "## 常驻约定\n- use Rust\n").unwrap();
        let a = read(&path).unwrap();
        let b = read(&path).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(a.pinned.contains("Rust"));
        std::fs::write(&path, "## 常驻约定\n- use TypeScript instead\n").unwrap();
        let c = read(&path).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert!(c.pinned.contains("TypeScript"));
        std::fs::remove_file(&path).unwrap();
        assert!(read(&path).is_none());
    }
}
