//! Exercise production persistence modules without building or launching Tauri.
//! Every test passes isolated paths; no default user directory is available here.
#![allow(dead_code)]

#[path = "../src/utils/assets.rs"]
pub mod assets;
#[path = "../src/brain/coding_memory_persistence.rs"]
pub mod coding_memory_persistence;
#[path = "../src/brain/coding_memory_records.rs"]
pub mod coding_memory_records;
#[path = "../src/brain/coding_session_store.rs"]
pub mod coding_session_store;
#[path = "../src/memory/entry_store.rs"]
pub mod entry_store;
#[path = "../src/memory/search_index.rs"]
pub mod search_index;
#[path = "../src/memory/kinds.rs"]
pub mod kinds;
#[path = "../src/memory/ongoing_tasks.rs"]
pub mod ongoing_tasks;
#[path = "../src/memory/provenance.rs"]
pub mod provenance;
#[path = "../src/memory/tokenize.rs"]
pub mod tokenize;
#[path = "../src/memory/recall_policy.rs"]
pub mod recall_policy;
#[path = "../src/persona/shared_language.rs"]
pub mod shared_language;
#[path = "../src/error.rs"]
pub mod error;
#[path = "../src/utils/fs.rs"]
pub mod fs;
#[path = "../src/utils/sqlite.rs"]
pub mod sqlite;
#[path = "../src/storage.rs"]
pub mod storage;
#[path = "../src/memory/types.rs"]
pub mod types;
#[path = "../src/memory/user_model.rs"]
pub mod user_model;
#[path = "../src/memory/memory_md.rs"]
pub mod memory_md;
#[path = "../src/memory/git_history.rs"]
pub mod git_history;
pub mod brain { pub use crate::coding_memory_persistence; }
pub mod memory { pub use crate::types; }
pub mod pipeline { pub mod prompt_modules {
    pub fn normalize_lang(lang: &str) -> &'static str {
        match lang { "en" => "en", "ja" => "ja", _ => "zh" }
    }
} }
#[path = "../src/persona/example_library.rs"]
pub mod example_library;
pub mod persona { pub use crate::example_library; }
// Storage accounting only needs the notebook identifier convention, not its UI.
pub mod notebook { pub mod collected {
    pub const COLLECTED_PREFIX: &str = "note_collected_";
    #[test]
    fn collected_prefix_fixture_matches_production() {
        assert!(include_str!("../src/notebook/collected.rs").contains(&format!("const COLLECTED_PREFIX: &str = \"{COLLECTED_PREFIX}\"")));
    }
} }

pub mod utils {
    pub mod job_object {
        pub fn assign_process(_: isize) -> bool { false }
    }
    pub mod path {
        std::thread_local! {
            // Tests of APIs that resolve character paths still use disposable
            // storage; the real application profile is never reachable.
            static CHARACTER_ROOT: tempfile::TempDir = tempfile::tempdir().unwrap();
        }
        pub fn get_user_data_dir() -> std::path::PathBuf {
            panic!("explicit isolated paths required")
        }
        pub fn get_character_data_dir(character: &str) -> std::path::PathBuf {
            CHARACTER_ROOT.with(|root| root.path().join("characters").join(character))
        }
        pub fn get_companion_data_dir(_: &str) -> std::path::PathBuf {
            panic!("explicit isolated paths required")
        }
        pub fn ensure_dir(path: &std::path::Path) -> std::io::Result<()> {
            std::fs::create_dir_all(path)
        }
    }
    pub use crate::{assets, fs, sqlite};
}

#[test]
fn ongoing_collection_survives_storage_and_indexes_older_entries_with_character_privacy() {
    use ongoing_tasks::{apply, Source, TaskPatch};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.db");
    let quotes: Vec<_> = (0..16).map(|i|format!("已看完了作品{i}号")).collect();
    let text = format!("以后帮我持续记录看番进度。{}", quotes.join("，"));
    let entries: Vec<_> = quotes.iter().enumerate().map(|(i,q)|serde_json::json!({
        "key":format!("show-{i}"),"label":format!("作品{i}号"),"state":"completed",
        "source_id":"original","source_quote":q})).collect();
    let patch: TaskPatch = serde_json::from_value(serde_json::json!({"action":"activate","key":"anime-list",
        "title":"番剧清单","goal":"维护看番记录","trigger":"用户提到追番进度","mode":"continuous",
        "source_id":"original","source_quote":"以后帮我持续记录看番进度",
        "request_quote":"以后帮我持续记录看番进度","entries":entries})).unwrap();
    let item = apply(None,&patch,&[Source{id:"original".into(),content:text,timestamp:1.0}],"vivian","first").unwrap();
    let store = entry_store::MemoryEntryStore::open(path.clone()).unwrap();
    store.write_rows(vec![(item.id.clone(),item.clone())],&[]).unwrap();
    drop(store);
    let store = entry_store::MemoryEntryStore::open(path).unwrap();
    let restored = store.load_by_ids(&[item.id.clone()],Some("vivian"),&Default::default()).unwrap().pop().unwrap();
    assert_eq!(ongoing_tasks::task(&restored).unwrap().entries.len(),16);
    assert!(!restored.content.contains("作品0号"));
    assert!(store.search_candidates("作品0号","vivian",&Default::default(),5).unwrap().iter().any(|(id,_)|id==&item.id));
    assert!(store.search_candidates("番剧清单","nana",&Default::default(),5).unwrap().is_empty());
}
