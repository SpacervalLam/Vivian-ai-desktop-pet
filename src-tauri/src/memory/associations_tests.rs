use super::super::super::provenance;
use super::super::super::types::Granularity;
use super::super::MemoryEntryStore;
use super::*;
use serde_json::json;

fn item(id: &str, text: &str) -> MemoryItem {
    let mut item = MemoryItem::new(text.into(), Granularity::Summary, 0.7);
    item.id = id.into();
    item.memory_type = "long_term".into();
    item.metadata = json!({"record_kind":"fact","known_by":["vivian"]});
    item
}
fn store(items: &[MemoryItem]) -> (tempfile::TempDir, MemoryEntryStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryEntryStore::open(dir.path().join("memory.db")).unwrap();
    save(&store, items);
    (dir, store)
}
fn save(store: &MemoryEntryStore, items: &[MemoryItem]) {
    store
        .write_rows(
            items.iter().map(|m| (m.id.clone(), m.clone())).collect(),
            &[],
        )
        .unwrap();
}
fn recall(store: &MemoryEntryStore, hits: &[MemoryItem], limit: usize) -> Vec<MemoryItem> {
    store
        .expand_associations(
            hits,
            "极光",
            "vivian",
            &MemoryRetrievalFilter::default(),
            limit,
        )
        .unwrap()
}
fn event(id: &str, text: &str, phase: &str) -> MemoryItem {
    let mut item = item(id, text);
    item.memory_type = "session_summary".into();
    item.metadata = json!({"record_kind":"session_summary","known_by":["vivian"],
        "source":"session_consolidation","source_message_ids":[format!("msg:{id}")],
        "event_schema_version":1,"summary_parts":[{"events":[{
            "id":"event:trip", "source_message_id":format!("msg:{id}"),"source_quote":text,
            "phase":phase,"recorded_at":item.timestamp}]}]});
    item
}

#[test]
fn event_progress_recalls_a_memory_without_query_overlap_and_preserves_evidence() {
    let planned = event("plan", "我们约好去看极光", "planned");
    let completed = event("done", "已经平安回家，旅程结束了", "completed");
    let (_dir, store) = store(&[planned.clone(), completed]);
    let result = recall(&store, &[planned], 6);
    assert_eq!(
        result.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["plan", "done"]
    );
    let context = &result[1];
    assert_eq!(recall_policy::lexical_support("极光", context), 0);
    assert_eq!(
        context.metadata["recall_association"]["relation"],
        "same_event"
    );
    assert!(
        context.metadata["recall_association"]["evidence"][0]["target_evidence"]
            .as_str()
            .unwrap()
            .contains("completed")
    );
    assert!(provenance::attribution(&context.metadata).contains("关联不代表因果"));
    assert_eq!(
        recall_policy::filter_for_query(result.clone(), "极光").len(),
        2
    );
    assert!(recall_policy::filter_for_query(result, "量子纠缠").is_empty());
}

#[test]
fn unvalidated_titles_and_partial_uncovered_events_do_not_link() {
    let a = event("a", "极光旅行", "planned");
    let mut b = event("b", "另一次旅程", "completed");
    b.metadata["source_message_ids"] = json!([]);
    let mut c = event("c", "私人推测", "completed");
    c.metadata["event_schema_version"] = json!(0);
    let (_dir, store) = store(&[a.clone(), b, c]);
    assert_eq!(recall(&store, &[a], 6).len(), 1);
}

#[test]
fn common_original_evidence_and_merged_sources_are_traceable() {
    let mut a = item("a", "用户想看极光");
    a.metadata["source_message_ids"] = json!(["original"]);
    let mut b = item("b", "准备厚外套");
    b.metadata["evidence_sources"] = json!([{"message_ids":["original"],"source_quote":"原话"}]);
    let (_dir, store) = store(&[a.clone(), b]);
    let result = recall(&store, &[a], 6);
    assert_eq!(result.len(), 2);
    assert_eq!(
        result[1].metadata["recall_association"]["relation"],
        "shared_evidence"
    );
}

#[test]
fn two_grounded_rare_topics_can_connect_but_one_topic_or_an_unquoted_tag_cannot() {
    let mut a = item("a", "极光需要晴朗天空，北欧旅行要准备御寒");
    a.metadata["topics"] = json!(["晴朗天空", "北欧旅行", "无法考证的标签", "用户"]);
    let mut b = item("b", "北欧旅行时，晴朗天空很适合观星");
    b.metadata["topics"] = json!(["晴朗天空", "北欧旅行"]);
    let mut one = item("one", "北欧旅行时买了一杯咖啡");
    one.metadata["topics"] = json!(["北欧旅行", "无法考证的标签"]);
    let (_dir, store) = store(&[a.clone(), b, one]);
    let result = recall(&store, &[a], 6);
    assert_eq!(
        result.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(
        result[1].metadata["recall_association"]["relation"],
        "related_topics"
    );
}

#[test]
fn topic_hubs_do_not_flood_recall() {
    let items: Vec<_> = (0..20)
        .map(|i| {
            let mut m = item(&format!("m{i:02}"), "极光 晴朗天空 北欧旅行");
            m.metadata["topics"] = json!(["晴朗天空", "北欧旅行"]);
            m
        })
        .collect();
    let (_dir, store) = store(&items);
    assert_eq!(recall(&store, &items[..1], 12).len(), 1);
}

#[test]
fn reference_origin_of_private_learning_is_recalled_without_following_another_hop() {
    let a = item("a", "我们聊到极光");
    let mut b = item("b", "磁场中的带电粒子运动");
    b.tags = vec!["private_learning".into(), "learning_anchor:memory:a".into()];
    let mut c = item("c", "新一轮延展资料");
    c.related_ids = vec!["b".into()];
    let (_dir, store) = store(&[a.clone(), b, c]);
    let result = recall(&store, &[a], 12);
    assert_eq!(
        result.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(
        result[1].metadata["recall_association"]["relation"],
        "memory_reference"
    );
}

#[test]
fn visibility_type_filters_and_expiry_apply_to_the_whole_path() {
    let a = event("a", "极光旅行", "planned");
    let mut hidden = event("hidden", "只告诉了 Nana", "completed");
    hidden.metadata["known_by"] = json!(["nana"]);
    let mut expired = event("expired", "过时的阅读资料", "progressed");
    expired.metadata["expires_at"] = json!(1.0);
    let mut backstory = event("backstory", "角色预设往事", "completed");
    backstory.metadata["fictional_backstory"] = json!(true);
    let (_dir, store) = store(&[a.clone(), hidden, expired, backstory]);
    assert_eq!(recall(&store, &[a.clone()], 12).len(), 1);
    let mut revoked = a.clone();
    revoked.metadata["known_by"] = json!(["nana"]);
    save(&store, &[revoked]);
    assert!(recall(&store, &[a], 12).is_empty());
}

#[test]
fn deleted_reference_cannot_connect_two_remaining_memories() {
    let a = item("a", "极光");
    let mut b = item("b", "旅行准备");
    b.related_ids = vec!["a".into()];
    let mut c = item("c", "行程结束");
    c.related_ids = vec!["a".into()];
    let (_dir, store) = store(&[a, b.clone(), c]);
    assert_eq!(recall(&store, &[b.clone()], 12).len(), 3);
    store.write_rows(vec![], &["a".into()]).unwrap();
    assert_eq!(recall(&store, &[b], 12).len(), 1);
}

#[test]
fn correction_archive_delete_and_clear_remove_stale_memberships() {
    let a = event("a", "极光旅行", "planned");
    let mut b = event("b", "已经回家", "completed");
    let (_dir, store) = store(&[a.clone(), b.clone()]);
    assert_eq!(recall(&store, &[a.clone()], 6).len(), 2);
    b.metadata["source_message_ids"] = json!(["different"]);
    save(&store, &[b.clone()]);
    assert_eq!(recall(&store, &[a.clone()], 6).len(), 1);
    b = event("b", "已经回家", "completed");
    b.consolidated = true;
    save(&store, &[b]);
    assert_eq!(recall(&store, &[a.clone()], 6).len(), 1);
    store.write_rows(vec![], &["b".into()]).unwrap();
    let n: i64 = store
        .conn
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM memory_anchors WHERE memory_id='b'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
    store.clear_all().unwrap();
    let n: i64 = store
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM memory_anchors", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    assert!(recall(&store, &[a], 6).is_empty());
}

#[test]
fn graph_survives_restart_and_old_databases_backfill_in_bounded_batches() {
    let a = event("a", "极光旅行", "planned");
    let b = event("b", "已经回家", "completed");
    let (dir, store) = store(&[]);
    // Simulate a pre-upgrade database: canonical rows and lexical index existed,
    // while no association projection has ever been built.
    {
        let conn = store.conn.lock();
        for item in [&a, &b] {
            conn.execute(
                "INSERT INTO entries(id,json) VALUES(?1,?2)",
                params![item.id, serde_json::to_string(item).unwrap()],
            )
            .unwrap();
            search_index::upsert(&conn, item).unwrap();
        }
    }
    let report = store.organize_associations(1).unwrap();
    assert_eq!(report.indexed, 1);
    assert!(report.pending);
    let report = store.organize_associations(1).unwrap();
    assert_eq!(report.indexed, 1);
    assert!(!report.pending);
    assert_eq!(store.organize_associations(1).unwrap().indexed, 0);
    drop(store);
    let store = MemoryEntryStore::open(dir.path().join("memory.db")).unwrap();
    assert_eq!(recall(&store, &[a], 6).len(), 2);
}

#[test]
fn graph_and_canonical_edits_roll_back_together() {
    let a = event("a", "极光旅行", "planned");
    let b = event("b", "已经回家", "completed");
    let (_dir, store) = store(&[a.clone()]);
    store
        .conn
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_graph BEFORE INSERT ON memory_anchors
        WHEN NEW.memory_id='b' BEGIN SELECT RAISE(FAIL,'graph failure'); END;",
        )
        .unwrap();
    assert!(store.write_rows(vec![(b.id.clone(), b)], &[]).is_err());
    assert_eq!(store.load_all().unwrap().len(), 1);
    let n: i64 = store
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM memory_nodes WHERE id='b'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(recall(&store, &[a], 6).len(), 1);
}

#[test]
fn graph_ignores_heat_updates_and_limits_context_without_displacing_first_direct_hit() {
    let a = event("a", "极光旅行", "planned");
    let others: Vec<_> = (0..12)
        .map(|i| event(&format!("p{i:02}"), "旅途有了进展", "progressed"))
        .collect();
    let mut all = vec![a.clone()];
    all.extend(others);
    let (_dir, store) = store(&all);
    let mut hot = a.clone();
    hot.visit_count = 90;
    hot.heat_score = 0.9;
    let conn = store.conn.lock();
    let before = conn.total_changes();
    upsert(&conn, &hot).unwrap();
    assert_eq!(before, conn.total_changes());
    drop(conn);
    let result = recall(&store, &[a.clone()], 6);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].id, "a");
    let result = recall(&store, &all[..6], 6);
    assert_eq!(result.len(), 6);
    assert_eq!(result[0].id, "a");
    assert_eq!(
        result
            .iter()
            .filter(|m| !m.metadata["recall_association"].is_null())
            .count(),
        2
    );
    assert_eq!(recall(&store, &[a], 1).len(), 1);
}

#[test]
fn association_weight_uses_original_event_time_rather_than_summary_receipt() {
    let a = event("a", "极光旅行", "planned");
    let now = current_timestamp();
    let mut old = event("old", "早年的一次进展", "progressed");
    old.timestamp = now;
    old.metadata["started_at"] = json!(now - 365.0 * 86400.0);
    let mut recent = event("recent", "旅程刚刚结束", "completed");
    recent.timestamp = now - 3600.0;
    recent.metadata["started_at"] = json!(now - 86400.0);
    let (_dir, store) = store(&[a.clone(), old, recent]);
    let results = recall(&store, &[a], 6);
    assert_eq!(
        results.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "recent", "old"]
    );
}

#[test]
fn unrelated_seeds_cannot_pool_their_weak_topics_into_one_false_link() {
    let mut a = item("a", "极光和晴朗天空");
    a.metadata["topics"] = json!(["晴朗天空"]);
    let mut b = item("b", "极光与北欧旅行");
    b.metadata["topics"] = json!(["北欧旅行"]);
    let mut c = item("c", "晴朗天空，北欧旅行");
    c.metadata["topics"] = json!(["晴朗天空", "北欧旅行"]);
    let (_dir, store) = store(&[a.clone(), b.clone(), c]);
    assert_eq!(recall(&store, &[a, b], 6).len(), 2);
}

#[test]
fn changed_or_archived_bridge_and_filter_excluded_target_cannot_expand() {
    let a = event("a", "极光旅行", "planned");
    let mut b = event("b", "已经回来", "completed");
    let (_dir, store) = store(&[a.clone(), b.clone()]);
    b.memory_type = "knowledge".into();
    save(&store, &[b]);
    let filter = MemoryRetrievalFilter {
        memory_types: Some(vec!["session_summary".into()]),
        ..Default::default()
    };
    assert_eq!(
        store
            .expand_associations(&[a.clone()], "极光", "vivian", &filter, 6)
            .unwrap()
            .len(),
        1
    );
    let mut correction = a.clone();
    correction.content = "这是另一件事".into();
    save(&store, &[correction.clone()]);
    assert!(recall(&store, &[a.clone()], 6).is_empty());
    correction.consolidated = true;
    save(&store, &[correction]);
    assert!(recall(&store, &[a], 6).is_empty());
}

#[test]
fn explicit_episode_memberships_are_distinct_from_unrelated_events() {
    let mut a = item("a", "极光旅行");
    a.episode_id = Some("episode:trip".into());
    let mut b = item("b", "当天收到了朋友的明信片");
    b.episode_id = a.episode_id.clone();
    let mut c = item("c", "另一次旅程");
    c.episode_id = Some("episode:another".into());
    let (_dir, store) = store(&[a.clone(), b, c]);
    let result = recall(&store, &[a], 6);
    assert_eq!(result.len(), 2);
    assert_eq!(
        result[1].metadata["recall_association"]["relation"],
        "same_episode"
    );
}
