use super::*;
use super::super::{entry_store::MemoryEntryStore, types::Granularity};
use serde_json::json;

fn item(id: &str, content: &str, metadata: serde_json::Value) -> MemoryItem {
    let mut item = MemoryItem::new(content.into(),Granularity::Summary,0.8);
    item.id=id.into(); item.memory_type="user".into(); item.metadata=metadata;
    kinds::initialize_record(&mut item); item
}
fn save(store: &MemoryEntryStore, items: &[MemoryItem]) {
    store.write_rows(items.iter().map(|item|(item.id.clone(),item.clone())).collect(),&[]).unwrap();
}
fn search(store: &MemoryEntryStore, query: &str, owner: &str, limit: usize) -> Vec<String> {
    store.search_candidates(query,owner,&MemoryRetrievalFilter::default(),limit).unwrap().into_iter().map(|(id,_)|id).collect()
}

#[test]
fn chinese_recall_and_visibility_are_checked_before_limit() {
    let dir=tempfile::tempdir().unwrap();
    let store=MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
    let mut items: Vec<_>=(0..300).map(|i|item(&format!("private-{i}"),"Rust Rust",json!({"known_by":["nana"]}))).collect();
    items.push(item("public","Rust 与 SQLite 的项目记录",json!({"known_by":["vivian"]})));
    items.push(item("location","用户现在住在上海浦东",json!({})));
    items.push(item("food","用户明确说不吃香菜",json!({})));
    items.push(item("pet","用户的小猫名字叫团子",json!({})));
    save(&store,&items);
    assert_eq!(search(&store,"Rust","vivian",1),vec!["public"]);
    for (query,expected) in [("上海浦东住址","location"),("香菜饮食忌口","food"),("小猫团子的名字","pet")] {
        assert!(search(&store,query,"vivian",3).contains(&expected.to_owned()));
    }
    assert!(search(&store,"毫不相关的唯一词汇","vivian",3).is_empty());
    assert!(search(&store,"\" OR NEAR() - *","vivian",3).is_empty());
    let long_query = format!("{} Rust",(0..100).map(|i|format!("padding{i}")).collect::<Vec<_>>().join(" "));
    assert_eq!(search(&store,&long_query,"vivian",1),vec!["public"]);
}

#[test]
fn projected_audience_matches_canonical_rules_and_changes_immediately() {
    let dir=tempfile::tempdir().unwrap();let store=MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
    let metadata=[json!({}),json!({"memory_owner":"nana"}),json!({"speaker":"user","listener":"nana","memory_owner":"vivian"}),
        json!({"known_by":[],"listener":"all","knowledge_source":"broadcast"}),json!({"known_by":["nana"]}),
        json!({"speaker":"nana","observer_id":"vivian"}),json!({"listener":"all","knowledge_source":"broadcast"})];
    let items:Vec<_>=metadata.into_iter().enumerate().map(|(i,meta)|item(&format!("audience-{i}"),"needle",meta)).collect();save(&store,&items);
    for owner in ["nana","vivian","third"] {
        let actual:HashSet<_>=search(&store,"needle",owner,100).into_iter().collect();
        let expected:HashSet<_>=items.iter().filter(|item|provenance::visible_to(&item.metadata,owner)).map(|item|item.id.clone()).collect();
        assert_eq!(actual,expected);
    }
    let mut changed=items[0].clone();changed.metadata["known_by"]=json!(["nana"]);save(&store,&[changed.clone()]);
    assert!(!search(&store,"needle","vivian",100).contains(&changed.id));
    changed.consolidated=true;save(&store,&[changed.clone()]);
    assert!(!search(&store,"needle","nana",100).contains(&changed.id));
    changed.consolidated=false;changed.content="differentkeyword".into();save(&store,&[changed.clone()]);
    assert_eq!(search(&store,"differentkeyword","nana",1),vec![changed.id.clone()]);
    store.write_rows(vec![],&[changed.id]).unwrap();
    assert!(search(&store,"differentkeyword","nana",1).is_empty());
}

#[test]
fn legacy_backfill_repair_and_missing_fts_preserve_original_records() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("entries.db");
    let original=item("legacy","原始上海记录",json!({}));let raw=serde_json::to_string(&original).unwrap();
    let conn=Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE entries(id TEXT PRIMARY KEY,json TEXT NOT NULL); CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);").unwrap();
    conn.execute("INSERT INTO entries VALUES('legacy',?1)",[&raw]).unwrap();drop(conn);
    let store=MemoryEntryStore::open(path.clone()).unwrap();assert_eq!(search(&store,"上海","vivian",3),vec!["legacy"]);drop(store);
    let conn=Connection::open(&path).unwrap();
    assert_eq!(conn.query_row("SELECT json FROM entries WHERE id='legacy'",[],|r|r.get::<_,String>(0)).unwrap(),raw);
    let mut changed=original.clone();changed.content="修改为北京记录".into();
    conn.execute("UPDATE entries SET json=?1 WHERE id='legacy'",[serde_json::to_string(&changed).unwrap()]).unwrap();drop(conn);
    let store=MemoryEntryStore::open(path.clone()).unwrap();assert_eq!(search(&store,"北京","vivian",3),vec!["legacy"]);assert!(search(&store,"上海","vivian",3).is_empty());drop(store);
    let conn=Connection::open(&path).unwrap();conn.execute_batch("DROP TABLE memory_fts").unwrap();drop(conn);
    let store=MemoryEntryStore::open(path).unwrap();assert_eq!(search(&store,"北京","vivian",3),vec!["legacy"]);assert_eq!(store.load_all().unwrap().len(),1);
}

#[test]
fn index_failure_rolls_back_canonical_write_and_metadata_does_not_retokenize() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("entries.db");let store=MemoryEntryStore::open(path.clone()).unwrap();
    let conn=Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_index BEFORE INSERT ON memory_search BEGIN SELECT RAISE(FAIL,'index failed'); END;").unwrap();
    let mut row=item("atomic","needle",json!({}));
    assert!(store.write_rows(vec![(row.id.clone(),row.clone())],&[]).is_err());assert!(store.load_all().unwrap().is_empty());
    conn.execute_batch("DROP TRIGGER reject_index").unwrap();save(&store,&[row.clone()]);
    conn.execute_batch("CREATE TRIGGER reject_retokenize BEFORE UPDATE OF tokens ON memory_search BEGIN SELECT RAISE(FAIL,'unexpected token update'); END;").unwrap();
    row.visit_count+=1;row.metadata["known_by"]=json!(["nana"]);save(&store,&[row.clone()]);
    assert!(search(&store,"needle","vivian",1).is_empty());assert_eq!(search(&store,"needle","nana",1),vec!["atomic"]);
}

#[test]
fn diary_scope_filter_finds_diary_entries() {
    let dir=tempfile::tempdir().unwrap();
    let store=MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
    let mut diary=MemoryItem::new("今天和用户去了海边，风很大但很开心".into(),Granularity::Summary,0.8);
    diary.id="diary-1".into();
    diary.tags=vec!["diary".into(),"2026-10-29".into(),"开心".into()];
    diary.metadata=json!({"kind":"diary","record_kind":"diary","diary_id":"d1","date":"2026-10-29","mood_tag":"开心"});
    kinds::initialize_record(&mut diary);
    save(&store,&[diary.clone()]);
    let filter=MemoryRetrievalFilter{tags_any:Some(vec!["diary".into()]),..Default::default()};
    let hits=store.search_candidates("海边 风", "vivian", &filter, 5).unwrap();
    assert_eq!(hits.iter().map(|(id,_)|id.as_str()).collect::<Vec<_>>(),vec!["diary-1"]);
    assert!(kinds::recallable(&diary));
}

#[test]
fn filters_and_context_loading_keep_order_and_session_boundaries() {
    let dir=tempfile::tempdir().unwrap();let store=MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
    let mut items=Vec::new();
    for session in ["a","b"] {for seq in 0..3 {
        let mut row=item(&format!("{session}-{seq}"),"needle",json!({"session_id":session,"seq_no":seq}));
        row.granularity="turn".into();
        row.timestamp=seq as f64;row.tags=vec![session.into()];items.push(row);
    }}save(&store,&items);
    let filter=MemoryRetrievalFilter{tags_any:Some(vec!["a".into()]),time_after:Some(1.0),time_before:Some(2.0),..Default::default()};
    assert_eq!(store.search_candidates("needle","vivian",&filter,1).unwrap()[0].0,"a-1");
    let loaded=store.load_by_ids(&["b-2".into(),"a-0".into(),"b-2".into()],Some("vivian"),&Default::default()).unwrap();
    assert_eq!(loaded.iter().map(|r|r.id.as_str()).collect::<Vec<_>>(),vec!["b-2","a-0"]);
    let neighbors=store.context_neighbors(&[items[1].clone()],"vivian",2).unwrap();
    assert_eq!(neighbors.iter().map(|r|r.id.as_str()).collect::<Vec<_>>(),vec!["a-2","a-0"]);
    assert!(neighbors.iter().all(|r|r.metadata["expansion"]==true));
}
