// Production manager methods run with isolated SQLite and a no-op vector cache.
use super::*;
use crate::{entry_store::MemoryEntryStore, types::Granularity};
use serde_json::json;

fn item(id: &str) -> MemoryItem {
    let mut m = MemoryItem::new(format!("record {id}"),Granularity::Summary,0.8);
    m.id = id.into(); m.memory_type = "long_term".into();
    m.metadata = json!({"record_kind":"fact","subject":"user","known_by":["vivian"]});
    m
}
fn manager(resident: bool) -> (tempfile::TempDir, Manager, Vec<MemoryItem>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryEntryStore::open(dir.path().join("entries.db")).unwrap());
    let rows = vec![item("old"),item("new")];
    store.write_rows(rows.iter().map(|m|(m.id.clone(),m.clone())).collect(),&[]).unwrap();
    let mut inner = Inner {data:MemoryStoreData::default(), id_index:HashMap::new(),
        changes:Default::default(),dirty:false,revision:0,entry_store:store,vector_store:Vectors};
    if resident {inner.data.entries=rows.clone();inner.rebuild_index();}
    (dir, Manager{inner:RwLock::new(inner),char_id:"vivian".into(),search_cache:Mutex::new(HashMap::new())}, rows)
}

#[test]
fn replacement_and_replay_work_when_old_rows_are_not_resident() {
    let (_dir,m,rows)=manager(false);
    m.supersede_memory("old","new").unwrap();
    m.supersede_memory("old","new").unwrap();
    let groups=m.recall_relation_bundles(&[rows[1].clone()]).unwrap();
    assert_eq!(groups[0].items.len(),2);
    assert!(groups[0].items.iter().find(|m|m.id=="old").unwrap().consolidated);
    assert!(m.inner.read().data.entries.is_empty(),"history lookup must not repopulate the resident cache");
}

#[test]
fn archive_on_evicted_history_cannot_be_undone_by_replacement_replay() {
    let (_dir,m,rows)=manager(false);
    m.supersede_memory("old","new").unwrap();
    m.archive_memory("old").unwrap();
    assert!(m.supersede_memory("old","new").is_err());
    let groups=m.recall_relation_bundles(&[rows[1].clone()]).unwrap();
    assert_eq!(groups[0].items.len(),1);
    let stored=m.inner.read().entry_store.load_all().unwrap();
    assert_eq!(stored.iter().find(|m|m.id=="old").unwrap().metadata["recall_hidden"],true);
}

#[test]
fn hard_removal_marks_evicted_canonical_rows_for_deletion() {
    let (_dir,m,_)=manager(false);
    let mut inner=m.inner.write();
    inner.remove_by_id("old").unwrap();
    inner.flush_dirty().unwrap();
    assert_eq!(inner.entry_store.load_all().unwrap().iter().map(|m|m.id.as_str()).collect::<Vec<_>>(),["new"]);
}

#[test]
fn failed_replacement_does_not_mutate_either_resident_or_canonical_state() {
    let (dir,m,_)=manager(true);
    let conn=rusqlite::Connection::open(dir.path().join("entries.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_replacement BEFORE UPDATE ON entries WHEN NEW.id='new' BEGIN SELECT RAISE(ABORT,'test fault'); END;").unwrap();
    assert!(m.supersede_memory("old","new").is_err());
    assert!(m.inner.read().data.entries.iter().all(|m|!m.consolidated && m.metadata["supersedes"].is_null()));
    assert!(m.inner.read().entry_store.load_all().unwrap().iter().all(|m|!m.consolidated && m.metadata["supersedes"].is_null()));
}

#[test]
fn replacement_rejects_self_links_and_cross_scope_overwrites() {
    let (_dir,m,_)=manager(true);
    assert!(m.supersede_memory("old","old").is_err());
    {
        let mut inner=m.inner.write();
        let pos=inner.id_index["new"];
        inner.entry_mut(pos).unwrap().metadata["scope"]=json!({"level":"task","value":"different"});
    }
    assert!(m.supersede_memory("old","new").is_err());
    assert!(m.inner.read().entry_store.load_all().unwrap().iter().all(|m|!m.consolidated));
}
