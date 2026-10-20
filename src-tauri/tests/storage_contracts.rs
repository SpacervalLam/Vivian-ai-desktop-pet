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

pub mod utils {
    pub mod path {
        pub fn get_user_data_dir() -> std::path::PathBuf {
            panic!("explicit isolated paths required")
        }
    }
    pub use crate::{assets, fs, sqlite};
}
