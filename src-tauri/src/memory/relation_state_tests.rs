use super::*;
use super::super::super::types::Granularity;
use super::super::MemoryEntryStore;

fn item(id: &str, text: &str) -> MemoryItem {
    let mut m = MemoryItem::new(text.into(), Granularity::Summary, 0.8);
    m.id = id.into();
    m.memory_type = "long_term".into();
    m.metadata = json!({"record_kind":"fact","subject":"user","known_by":["vivian"]});
    m
}

fn save(store: &MemoryEntryStore, items: &[MemoryItem]) {
    store.write_rows(items.iter().map(|m|(m.id.clone(),m.clone())).collect(), &[]).unwrap();
}

fn store(items: &[MemoryItem]) -> (tempfile::TempDir, MemoryEntryStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
    save(&store, items);
    (dir, store)
}

#[test]
fn replacement_chain_survives_restart_and_keeps_historical_roles() {
    let mut old = item("old", "以前在上海工作");
    let mut middle = item("middle", "后来搬到杭州");
    let mut latest = item("latest", "现在住北京");
    mark_superseded(&mut old, "middle");
    mark_superseded(&mut middle, "latest");
    middle.metadata["supersedes"] = json!("old");
    latest.metadata["supersedes"] = json!("middle");
    let (dir, s) = store(&[latest.clone(), middle, old]);
    drop(s);
    let s = MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
    let groups = s.relation_bundles(&[latest], "vivian").unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].items.iter().map(|m|m.id.as_str()).collect::<Vec<_>>(), ["latest", "middle", "old"]);
    assert_eq!(groups[0].relations.len(), 2);
    assert!(!groups[0].incomplete);
    assert!(groups[0].usage_hint("old").contains("历史"));
    assert!(groups[0].usage_hint("middle").contains("历史"));
    assert!(groups[0].usage_hint("latest").contains("替代旧版本"));
    assert!(s.load_by_ids(&["old".into()], Some("vivian"), &Default::default()).unwrap().is_empty());
}

#[test]
fn incoming_conflict_is_recalled_from_either_endpoint_and_deduplicated() {
    let a = item("a", "用户自述是星期一");
    let mut b = item("b", "用户又说是星期二，尚待核实");
    mark_conflict(&mut b, "a");
    let (_dir, s) = store(&[a.clone(), b.clone()]);
    for seeds in [vec![a.clone()], vec![b.clone()], vec![a.clone(), b.clone()]] {
        let groups = s.relation_bundles(&seeds, "vivian").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].relations.len(), 1);
        assert!(groups[0].usage_hint("a").contains("不可将任一方"));
    }
    clear_conflict(&mut b, "a");
    save(&s, &[b]);
    assert_eq!(s.relation_bundles(&[a], "vivian").unwrap()[0].items.len(), 1);
}

#[test]
fn private_or_revoked_endpoint_never_leaks_content_or_identifiers() {
    let a = item("public", "可见资料");
    let mut b = item("private", "不应出现在结果的私有资料");
    mark_conflict(&mut b, "public");
    let (_dir, s) = store(&[a.clone(), b.clone()]);
    b.metadata["known_by"] = json!(["nana"]);
    save(&s, &[b]);
    let groups = s.relation_bundles(&[a.clone()], "vivian").unwrap();
    assert_eq!(groups[0].items.len(), 1);
    assert!(groups[0].relations.is_empty());
    assert!(groups[0].incomplete);
    let json = serde_json::to_string(&groups).unwrap();
    assert!(!json.contains("private"));
    assert!(!json.contains("私有"));
    let mut revoked = a.clone(); revoked.metadata["known_by"] = json!(["nana"]);
    save(&s, &[revoked]);
    assert!(s.relation_bundles(&[a], "vivian").unwrap().is_empty());
}

#[test]
fn generic_archive_and_user_delete_are_not_revived_as_history() {
    let mut old = item("old", "旧记录");
    let mut new = item("new", "新记录");
    new.metadata["supersedes"] = json!("old");
    old.consolidated = true; // Legacy archive without proof of supersession.
    let (_dir, s) = store(&[old.clone(), new.clone()]);
    assert_eq!(s.relation_bundles(&[new.clone()], "vivian").unwrap()[0].items.len(), 1);
    mark_superseded(&mut old, "new");
    old.metadata["recall_hidden"] = json!(true); // Explicit delete beats any link.
    save(&s, &[old]);
    assert_eq!(s.relation_bundles(&[new.clone()], "vivian").unwrap()[0].items.len(), 1);
    s.write_rows(Vec::new(), &["old".into()]).unwrap();
    assert_eq!(s.relation_bundles(&[new], "vivian").unwrap()[0].items.len(), 1);
}

#[test]
fn explicit_scope_and_subject_mismatches_cannot_form_replacements() {
    let a = item("a", "工作时别主动聊天");
    let mut b = item("b", "休息时欢迎聊天");
    b.metadata["supersedes"] = json!("a");
    b.metadata["scope"] = json!({"level":"situation","value":"休息"});
    let (_dir, s) = store(&[a.clone(), b.clone()]);
    assert_eq!(s.relation_bundles(&[b.clone()], "vivian").unwrap()[0].items.len(), 1);
    b.metadata["scope"] = Value::Null;
    b.metadata["subject"] = json!("self");
    save(&s, &[b.clone()]);
    assert_eq!(s.relation_bundles(&[b], "vivian").unwrap()[0].items.len(), 1);
    assert!(!same_slot(&json!({"scope":{"level":"task","value":"A"}}), &json!({"scope":{"level":"task","value":"B"}})));
    assert!(same_slot(&json!({}), &json!({})));
}

#[test]
fn stale_seed_and_removed_relation_do_not_survive_canonical_reads() {
    let a = item("a", "旧文本");
    let mut b = item("b", "另一条记录");
    mark_conflict(&mut b, "a");
    let (_dir, s) = store(&[a.clone(), b.clone()]);
    let mut changed = a.clone(); changed.content = "纠正后的文本".into();
    save(&s, &[changed.clone()]);
    assert!(s.relation_bundles(&[a], "vivian").unwrap().is_empty());
    b.metadata["conflicts_with"] = json!([]);
    save(&s, &[b]);
    assert!(s.relation_bundles(&[changed], "vivian").unwrap()[0].relations.is_empty());
}

#[test]
fn unrelated_records_and_derived_summaries_cannot_create_relations() {
    let a = item("a", "同一个主题");
    let mut b = item("b", "同一个主题");
    let (_dir, s) = store(&[a.clone(), b.clone()]);
    assert_eq!(s.relation_bundles(&[a.clone()], "vivian").unwrap()[0].items.len(), 1);
    b.metadata["record_kind"] = json!("session_summary");
    b.metadata["supersedes"] = json!("a");
    save(&s, &[b]);
    assert_eq!(s.relation_bundles(&[a], "vivian").unwrap()[0].items.len(), 1);
}

#[test]
fn relation_fanout_is_bounded_and_reports_incompleteness() {
    let root = item("root", "根记录");
    let mut rows = vec![root.clone()];
    for i in 0..30 { let mut m = item(&format!("m{i:02}"), "冲突记录"); mark_conflict(&mut m, "root"); rows.push(m); }
    let (_dir, s) = store(&rows);
    let groups = s.relation_bundles(&[root], "vivian").unwrap();
    assert!(groups[0].items.len() <= MAX_GROUP);
    assert!(groups[0].incomplete);
    assert!(groups[0].relations.iter().all(|r|groups[0].items.iter().any(|m|m.id==r.source) && groups[0].items.iter().any(|m|m.id==r.target)));
}

#[test]
fn token_budget_never_retains_only_half_a_relation() {
    assert_eq!(select_groups(&[vec![600,600], vec![100]], 1000), [1]);
    assert_eq!(select_groups(&[vec![300,300], vec![400]], 1000), [0,1]);
    assert!(select_groups(&[vec![1,usize::MAX]], 100).is_empty());
    assert!(select_groups(&[vec![1]], 0).is_empty());
}

#[test]
fn relation_projection_and_records_rollback_together() {
    let a = item("a", "原始内容");
    let b = item("b", "另一原始内容");
    let (_dir, s) = store(&[a.clone(), b.clone()]);
    s.conn.lock().execute_batch("CREATE TRIGGER reject_relation BEFORE INSERT ON memory_state_relations BEGIN SELECT RAISE(ABORT,'test rejection'); END;").unwrap();
    let mut changed = b; mark_conflict(&mut changed, "a"); changed.content = "尚未提交".into();
    assert!(s.write_rows(vec![(changed.id.clone(),changed)], &[]).is_err());
    let groups = s.relation_bundles(&[a], "vivian").unwrap();
    assert_eq!(groups[0].items.len(), 1);
    assert!(s.load_all().unwrap().iter().all(|m|m.content != "尚未提交"));
}

#[test]
fn old_database_relations_are_backfilled_in_bounded_batches() {
    let a = item("a", "旧库记录");
    let mut b = item("b", "旧库冲突"); mark_conflict(&mut b,"a");
    let (_dir, s) = store(&[a.clone(), b]);
    s.conn.lock().execute_batch("DELETE FROM memory_state_relations; UPDATE memory_nodes SET version=1; DELETE FROM meta WHERE key='memory_associations_ready';").unwrap();
    assert!(s.organize_associations(1).unwrap().pending);
    assert!(!s.organize_associations(1).unwrap().pending);
    assert_eq!(s.relation_bundles(&[a], "vivian").unwrap()[0].items.len(),2);
    assert_eq!(s.organize_associations(1).unwrap().indexed,0);
}
