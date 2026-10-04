//! Test the application's actual portable modules without starting Tauri or accessing user data.
#![allow(dead_code)]
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
