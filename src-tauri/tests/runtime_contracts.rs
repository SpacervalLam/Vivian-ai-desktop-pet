//! Test the application's actual portable modules without starting Tauri or accessing user data.
#![allow(dead_code)]
#[path = "../src/brain/coding_compaction.rs"]
pub mod coding_compaction;
#[path = "../src/brain/coding_memory_persistence.rs"]
pub mod coding_memory_persistence;
#[path = "../src/brain/coding_memory_navigation.rs"]
pub mod coding_memory_navigation;
#[path = "../src/brain/coding_session_store.rs"]
pub mod coding_session_store;
#[path = "../src/brain/coding_session_cache.rs"]
pub mod coding_session_cache;
#[path = "../src/brain/coding_memory_records.rs"]
pub mod coding_memory_records;
#[path = "../src/brain/coding_memory_queue.rs"]
pub mod coding_memory_queue;
#[path = "../src/desktop_contract.rs"]
pub mod desktop_contract;
#[path = "../src/plugin_contract.rs"]
pub mod plugin_contract;
#[path = "../src/pipeline/inline_tag_scanner.rs"]
pub mod inline_tag_scanner;
#[path = "../src/feature_flags.rs"]
pub mod feature_flags;
#[path = "../src/memory/tokenize.rs"]
pub mod tokenize;
#[path = "../src/memory/entity_extract.rs"]
pub mod entity_extract;
#[path = "../src/memory/graph_store.rs"]
pub mod graph_store;
#[path = "../src/utils/fs.rs"]
pub mod fs;
#[path = "../src/brain/scheduler.rs"]
pub mod scheduler;
#[path = "../src/brain/reminder_delivery.rs"]
pub mod reminder_delivery;
#[path = "../src/brain/interruption_controller.rs"]
pub mod interruption_controller;

pub mod brain {
    pub use crate::{reminder_delivery, interruption_controller};
}
pub mod utils {
    pub use crate::fs;
    pub mod path {
        pub fn get_user_data_dir() -> std::path::PathBuf {
            panic!("Runtime contract tests must supply isolated persistence paths")
        }
    }
    pub mod watchdog {
        pub fn register(_: &str, _: f64, _: Option<()>) {}
        pub fn beat(_: &str) {}
        pub fn unregister(_: &str) {}
    }
}

#[path = "../src/companion_policy.rs"]
pub mod companion_policy;

#[path = "../src/voice_diagnostics.rs"]
pub mod voice_diagnostics;

#[path = "../src/shortcut.rs"]
pub mod shortcut;

#[path = "../src/screen_capture.rs"]
pub mod screen_capture;

#[path = "../src/visual_evidence.rs"]
pub mod visual_evidence;
#[path = "../src/desktop_menu_policy.rs"]
pub mod desktop_menu_policy;

#[path = "../src/memory/provenance.rs"]
pub mod memory_provenance;

#[path = "../src/memory/types.rs"]
pub mod types;
#[path = "../src/memory/kinds.rs"]
pub mod memory_kinds;

#[path = "../src/dialogue/jsonl_index.rs"]
pub mod jsonl_index;
#[path = "../src/memory/extraction_queue.rs"]
pub mod extraction_queue;

#[path = "../src/memory/retrieval_budget.rs"]
pub mod retrieval_budget;

#[path = "../src/memory/summary_commit.rs"]
pub mod summary_commit;
