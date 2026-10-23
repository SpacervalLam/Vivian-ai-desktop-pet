//! Lightweight catalog with stable references to histories hydrated on demand.
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

struct Slot<T> {
    head: T,
    value: OnceLock<Option<T>>,
}
pub struct Catalog<T> {
    slots: BTreeMap<String, Slot<T>>,
    removed: std::collections::BTreeSet<String>,
    dirty: BTreeMap<String, u64>,
    revision: u64,
    load: Arc<dyn Fn(&str) -> Option<T> + Send + Sync>,
    summarize: Arc<dyn Fn(&T) -> T + Send + Sync>,
}

impl<T: Clone> Catalog<T> {
    pub fn new(
        heads: Vec<(String, T)>,
        load: impl Fn(&str) -> Option<T> + Send + Sync + 'static,
        summarize: impl Fn(&T) -> T + Send + Sync + 'static,
    ) -> Self {
        Self {
            slots: heads
                .into_iter()
                .map(|(id, head)| {
                    (
                        id,
                        Slot {
                            head,
                            value: OnceLock::new(),
                        },
                    )
                })
                .collect(),
            removed: Default::default(),
            dirty: Default::default(),
            revision: 0,
            load: Arc::new(load),
            summarize: Arc::new(summarize),
        }
    }
    pub fn insert(&mut self, id: String, value: T) -> Option<T> {
        self.removed.remove(&id);
        self.revision += 1;
        self.dirty.insert(id.clone(), self.revision);
        let head = (self.summarize)(&value);
        let cell = OnceLock::new();
        cell.set(Some(value)).ok();
        self.slots
            .insert(id, Slot { head, value: cell })
            .map(|old| old.value.into_inner().flatten().unwrap_or(old.head))
    }
    pub fn get(&self, id: &str) -> Option<&T> {
        self.slots
            .get(id)?
            .value
            .get_or_init(|| (self.load)(id))
            .as_ref()
    }
    pub fn retry_failed(&mut self, id: &str) {
        if let Some(slot) = self.slots.get_mut(id) {
            if slot.value.get().is_some_and(Option::is_none) {
                slot.value.take();
            }
        }
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut T> {
        self.retry_failed(id);
        let slot = self.slots.get_mut(id)?;
        slot.value.get_or_init(|| (self.load)(id));
        let value = slot.value.get_mut()?.as_mut()?;
        self.revision += 1;
        self.dirty.insert(id.into(), self.revision);
        Some(value)
    }
    pub fn remove(&mut self, id: &str) -> Option<T> {
        if self.slots.contains_key(id) {
            self.removed.insert(id.to_string());
            self.revision += 1;
            self.dirty.insert(id.into(), self.revision);
        }
        self.slots
            .remove(id)
            .map(|slot| slot.value.into_inner().flatten().unwrap_or(slot.head))
    }
    pub fn contains_key(&self, id: &str) -> bool {
        self.slots.contains_key(id)
    }
    pub fn removed(&self) -> Vec<String> {
        self.removed.iter().cloned().collect()
    }
    /// Iterating never hydrates old histories.
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.slots.values().map(|slot| {
            slot.value
                .get()
                .and_then(Option::as_ref)
                .unwrap_or(&slot.head)
        })
    }
    pub fn heads(&self) -> Vec<(String, T)> {
        self.slots
            .iter()
            .map(|(id, slot)| {
                (
                    id.clone(),
                    slot.value
                        .get()
                        .and_then(Option::as_ref)
                        .map(|value| (self.summarize)(value))
                        .unwrap_or_else(|| slot.head.clone()),
                )
            })
            .collect()
    }
    pub fn loaded(&self) -> Vec<(String, T)> {
        self.slots
            .iter()
            .filter_map(|(id, slot)| {
                slot.value
                    .get()
                    .and_then(Option::as_ref)
                    .map(|value| (id.clone(), value.clone()))
            })
            .collect()
    }

    /// Copy only changed sessions. Acknowledgement cannot erase a later edit.
    pub fn pending(&self) -> Pending<T> {
        let mut pending = Pending { loaded: Vec::new(), heads: Vec::new(), deleted: Vec::new(), revisions: Vec::new() };
        for (id, revision) in &self.dirty {
            pending.revisions.push((id.clone(), *revision));
            if let Some(slot) = self.slots.get(id) {
                if let Some(value) = slot.value.get().and_then(Option::as_ref) {
                    pending.loaded.push((id.clone(), value.clone()));
                    pending.heads.push((id.clone(), (self.summarize)(value)));
                }
            } else if self.removed.contains(id) { pending.deleted.push(id.clone()); }
        }
        pending
    }
    pub fn has_pending(&self) -> bool { !self.dirty.is_empty() }
    pub fn acknowledge(&mut self, revisions: &[(String, u64)]) {
        for (id, revision) in revisions {
            if self.dirty.get(id) == Some(revision) {
                self.dirty.remove(id);
                if !self.slots.contains_key(id) { self.removed.remove(id); }
            }
        }
    }
}

pub struct Pending<T> {
    pub loaded: Vec<(String, T)>,
    pub heads: Vec<(String, T)>,
    pub deleted: Vec<String>,
    pub revisions: Vec<(String, u64)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn saving_one_session_does_not_clone_unrelated_histories_or_clear_new_edits() {
        let mut catalog = Catalog::new(vec![("old".into(), 0)], |_| Some(1), |_| 0);
        catalog.get("old");
        assert!(catalog.pending().loaded.is_empty());
        catalog.insert("new".into(), 7);
        let first = catalog.pending();
        assert_eq!(first.loaded, [("new".into(), 7)]);
        *catalog.get_mut("new").unwrap() = 8;
        catalog.acknowledge(&first.revisions);
        assert_eq!(catalog.pending().loaded, [("new".into(), 8)]);
        let latest = catalog.pending(); catalog.acknowledge(&latest.revisions);
        assert!(catalog.pending().revisions.is_empty());
        catalog.remove("new"); let deleted = catalog.pending();
        assert_eq!(deleted.deleted, ["new"]);
        catalog.insert("new".into(), 9); catalog.acknowledge(&deleted.revisions);
        assert_eq!(catalog.pending().loaded, [("new".into(), 9)]);
    }
    #[test]
    fn recent_window_and_page_iteration_do_not_read_old_histories() {
        let reads = Arc::new(AtomicUsize::new(0));
        let count = reads.clone();
        let catalog = Catalog::new(
            (0..75)
                .map(|i| (format!("{i:02}"), String::new()))
                .collect(),
            move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Some("x".repeat(16_384))
            },
            |_| String::new(),
        );
        for i in 45..75 {
            catalog.get(&format!("{i:02}"));
        }
        assert_eq!(reads.load(Ordering::SeqCst), 30);
        assert_eq!(catalog.values().count(), 75);
        assert!(catalog.heads().iter().all(|(_, head)| head.is_empty()));
        assert_eq!(reads.load(Ordering::SeqCst), 30);
        assert_eq!(
            catalog
                .loaded()
                .iter()
                .map(|(_, value)| value.len())
                .sum::<usize>(),
            30 * 16_384
        );
        assert_eq!(catalog.get("00").unwrap().len(), 16_384);
        catalog.get("00");
        assert_eq!(reads.load(Ordering::SeqCst), 31);
        eprintln!("75 histories: eager=75 reads/1228800 payload bytes; lazy startup=30 reads/491520 payload bytes; page=0 additional reads");
    }
    #[test]
    fn concurrent_opens_hydrate_once_and_deletion_never_hydrates() {
        let reads = Arc::new(AtomicUsize::new(0));
        let count = reads.clone();
        let mut catalog = Catalog::new(
            vec![("old".into(), 0), ("delete".into(), 0)],
            move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Some(42)
            },
            |_| 0,
        );
        std::thread::scope(|scope| {
            for _ in 0..12 {
                let catalog = &catalog;
                scope.spawn(move || assert_eq!(catalog.get("old"), Some(&42)));
            }
        });
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert!(catalog.remove("delete").is_some());
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        *catalog.get_mut("old").unwrap() = 99;
        assert_eq!(catalog.loaded()[0].1, 99);
    }
}
