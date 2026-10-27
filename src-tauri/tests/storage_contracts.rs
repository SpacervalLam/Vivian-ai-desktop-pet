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
#[path = "../src/memory/provenance.rs"]
pub mod provenance;
#[path = "../src/memory/tokenize.rs"]
pub mod tokenize;
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
    pub mod path {
        pub fn get_user_data_dir() -> std::path::PathBuf {
            panic!("explicit isolated paths required")
        }
        pub fn get_character_data_dir(_: &str) -> std::path::PathBuf {
            panic!("explicit isolated paths required")
        }
    }
    pub use crate::{assets, fs, sqlite};
}
