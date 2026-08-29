use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use parking_lot::Mutex as PlMutex;
use serde::{Deserialize, Serialize};

use super::types::{current_timestamp, MemoryItem};

const RETENTION_SECONDS: f64 = 7.0 * 24.0 * 3600.0;
const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecycleEntry {
    pub item: MemoryItem,
    pub deleted_at: f64,
    pub reason: String,
    pub original_index_hint: Option<usize>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RecycleBinData {
    pub entries: Vec<RecycleEntry>,
}

#[derive(Clone)]
pub struct RecycleBin {
    inner: Arc<PlMutex<RecycleBinData>>,
    store_path: PathBuf,
    dirty: Arc<Mutex<bool>>,
}

impl RecycleBin {
    pub fn new(store_path: PathBuf) -> Self {
        let mut data = Self::load(&store_path).unwrap_or_default();
        data.entries.sort_by(|a, b| {
            b.deleted_at
                .partial_cmp(&a.deleted_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Self {
            inner: Arc::new(PlMutex::new(data)),
            store_path,
            dirty: Arc::new(Mutex::new(false)),
        }
    }

    fn load(path: &PathBuf) -> Option<RecycleBinData> {
        crate::utils::fs::load_json_or_backup(path)
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut dirty = self.dirty.lock().unwrap();
        if !*dirty {
            return Ok(());
        }
        let data = self.inner.lock();
        let json = serde_json::to_string_pretty(&*data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let tmp = self.store_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.store_path)?;
        *dirty = false;
        Ok(())
    }

    pub fn push(&self, item: MemoryItem, reason: impl Into<String>) {
        let mut data = self.inner.lock();
        data.entries.push(RecycleEntry {
            item,
            deleted_at: current_timestamp(),
            reason: reason.into(),
            original_index_hint: None,
        });
        if data.entries.len() > MAX_ENTRIES {
            let overflow = data.entries.len() - MAX_ENTRIES;
            data.entries.drain(0..overflow);
        }
        drop(data);
        *self.dirty.lock().unwrap() = true;
    }

    pub fn restore(&self, id: &str) -> Option<MemoryItem> {
        let mut data = self.inner.lock();
        let pos = data.entries.iter().position(|e| e.item.id == id)?;
        let entry = data.entries.remove(pos);
        drop(data);
        *self.dirty.lock().unwrap() = true;
        Some(entry.item)
    }

    pub fn purge(&self, id: &str) -> bool {
        let mut data = self.inner.lock();
        if let Some(pos) = data.entries.iter().position(|e| e.item.id == id) {
            data.entries.remove(pos);
            drop(data);
            *self.dirty.lock().unwrap() = true;
            return true;
        }
        false
    }

    pub fn purge_expired(&self) -> usize {
        let now = current_timestamp();
        let mut data = self.inner.lock();
        let before = data.entries.len();
        data.entries.retain(|e| now - e.deleted_at < RETENTION_SECONDS);
        let removed = before - data.entries.len();
        drop(data);
        if removed > 0 {
            *self.dirty.lock().unwrap() = true;
        }
        removed
    }

    pub fn purge_all(&self) -> usize {
        let mut data = self.inner.lock();
        let count = data.entries.len();
        data.entries.clear();
        drop(data);
        *self.dirty.lock().unwrap() = true;
        count
    }

    pub fn list(&self) -> Vec<RecycleEntry> {
        self.inner.lock().entries.clone()
    }

    pub fn get(&self, id: &str) -> Option<RecycleEntry> {
        self.inner
            .lock()
            .entries
            .iter()
            .find(|e| e.item.id == id)
            .cloned()
    }

    pub fn count(&self) -> usize {
        self.inner.lock().entries.len()
    }

    pub fn exists(&self, id: &str) -> bool {
        self.inner.lock().entries.iter().any(|e| e.item.id == id)
    }

    pub fn dedupe_by_content(&self) -> usize {
        let mut data = self.inner.lock();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut remove_indices: Vec<usize> = Vec::new();
        for (i, entry) in data.entries.iter().enumerate() {
            let key = entry.item.content.clone();
            if let Some(&_i) = seen.get(&key) {
                remove_indices.push(i);
            } else {
                seen.insert(key, i);
            }
        }
        for &i in remove_indices.iter().rev() {
            data.entries.remove(i);
        }
        let removed = remove_indices.len();
        drop(data);
        if removed > 0 {
            *self.dirty.lock().unwrap() = true;
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(id: &str, content: &str) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            content: content.to_string(),
            granularity: "turn".to_string(),
            memory_type: String::new(),
            importance: 0.5,
            timestamp: current_timestamp(),
            embedding: None,
            tags: Vec::new(),
            metadata: serde_json::json!({}),
            related_ids: Vec::new(),
            description: None,
            visit_count: 0,
            last_visit_at: 0.0,
            heat_score: 0.0,
            open_hooks: Vec::new(),
            reinforcement: 0.0,
            disputation: 0.0,
            rein_last_signal_at: 0.0,
            disp_last_signal_at: 0.0,
            sub_zero_days: 0,
            sub_zero_last_increment_date: String::new(),
            user_fact_reinforce_count: 0,
            protected: false,
            episode_id: None,
            consolidated: false,
            rebuttal_grace_remaining: 0,
        }
    }

    #[test]
    fn push_and_restore_roundtrip() {
        let bin = RecycleBin::new(PathBuf::from(":memory:"));
        let item = make_item("m1", "hello");
        bin.push(item.clone(), "test");
        assert_eq!(bin.count(), 1);
        let restored = bin.restore("m1").expect("should restore");
        assert_eq!(restored.id, "m1");
        assert_eq!(bin.count(), 0);
    }

    #[test]
    fn purge_expired_removes_old_entries() {
        let bin = RecycleBin::new(PathBuf::from(":memory:"));
        let mut item = make_item("m_old", "old");
        item.timestamp = current_timestamp() - (RETENTION_SECONDS + 100.0);
        let entry = RecycleEntry {
            item,
            deleted_at: current_timestamp() - (RETENTION_SECONDS + 100.0),
            reason: "expired".to_string(),
            original_index_hint: None,
        };
        bin.inner.lock().entries.push(entry.clone());
        let recent = make_item("m_new", "new");
        bin.push(recent, "fresh");
        let removed = bin.purge_expired();
        assert_eq!(removed, 1);
        assert_eq!(bin.count(), 1);
        assert!(bin.exists("m_new"));
        assert!(!bin.exists("m_old"));
    }
}
