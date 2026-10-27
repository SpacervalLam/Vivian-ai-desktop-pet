use super::*;

fn fixture() -> (tempfile::TempDir, MemoryManagerInner) {
    let dir = tempfile::tempdir().unwrap();
    let inner = MemoryManagerInner {
        data: MemoryStoreData::default(), id_index: HashMap::new(),
        capacities: [(Granularity::Turn, 2)].into_iter().collect(),
        entry_store: Arc::new(super::super::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap()),
        changes: Default::default(), revision: 0,
        vector_store: Arc::new(MemoryVectorStore::new(dir.path().join("vectors.db"), 256, "hashing").unwrap()),
        embedding: Arc::new(super::super::embedding::HashingMemoryEmbedding::default()),
        retrieval_weights: RetrievalWeights::default(), dirty: false, last_save_at: None,
    };
    (dir, inner)
}

#[test]
fn dialogue_retirement_and_changes_survive_reopen() {
    let (dir, mut inner) = fixture();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let mut item = MemoryItem::new("原话".into(), Granularity::Turn, 0.8);
        item.memory_type = "short_term".into();
        ids.push(item.id.clone());
        inner.add_entry(item).unwrap();
    }
    inner.entry_mut(1).unwrap().metadata["custom"] = serde_json::json!("updated");
    inner.save_throttled().unwrap();
    inner.flush_dirty().unwrap();
    let store = super::super::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
    let loaded = store.load_all().unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded.iter().find(|item| item.id == ids[0]).unwrap().metadata["index_active"], false);
    assert_eq!(loaded.iter().find(|item| item.id == ids[1]).unwrap().metadata["custom"], "updated");
    let removed = inner.take_by_id(&ids[1]).unwrap().unwrap();
    inner.save_to_disk().unwrap();
    assert_eq!(store.load_all().unwrap().len(), 2);
    inner.restore_entry(removed).unwrap();
    inner.flush_dirty().unwrap();
    assert_eq!(store.load_all().unwrap().len(), 3);
}

#[test]
fn archival_is_committed_before_resident_eviction() {
    let (_dir, mut inner) = fixture();
    for i in 0..MAX_RESIDENT_ENTRIES {
        let mut item = MemoryItem::new("保留证据".into(), Granularity::Summary, 0.8);
        item.id = format!("row-{i}"); item.memory_type = "user".into();
        inner.mark_changed(&item.id);
        inner.data.entries.push(item);
    }
    inner.rebuild_index();
    inner.save_to_disk().unwrap();
    inner.entry_mut(0).unwrap().consolidated = true;
    let mut last = MemoryItem::new("新增事实".into(), Granularity::Summary, 0.8);
    last.memory_type = "user".into();
    inner.add_entry(last).unwrap();
    assert!(!inner.id_index.contains_key("row-0"));
    let loaded = inner.entry_store.load_all().unwrap();
    assert_eq!(loaded.len(), MAX_RESIDENT_ENTRIES + 1);
    assert!(loaded.iter().find(|item| item.id == "row-0").unwrap().consolidated);
    inner.save_to_disk().unwrap();
    assert_eq!(inner.entry_store.load_all().unwrap().len(), MAX_RESIDENT_ENTRIES + 1);
}
