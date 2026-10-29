//! Test the application's actual portable modules without starting Tauri or accessing user data.
#![allow(dead_code)]
#[path = "../src/utils/async_ttl_cache.rs"]
pub mod async_ttl_cache;
#[path = "../src/world/desktop_state.rs"]
pub mod desktop_state;
#[path = "../src/world/topic_window.rs"]
pub mod topic_window;
#[path = "../src/pipeline/presentation_grounding.rs"]
pub mod presentation_grounding;
#[path = "../src/brain/work_contract.rs"]
pub mod work_contract;
#[path = "../src/brain/work_context.rs"]
pub mod work_context;
#[path = "../src/brain/work_question.rs"]
pub mod work_question;
#[path = "../src/brain/reference_research.rs"]
pub mod reference_research;
#[path = "../src/brain/coding_event_log.rs"]
pub mod coding_event_log;
#[path = "../src/drag_motion.rs"]
pub mod drag_motion;
#[path = "../src/brain/coding_compaction.rs"]
pub mod coding_compaction;
#[path = "../src/brain/work_harness.rs"]
pub mod work_harness;
#[path = "../src/brain/work_memory_cache.rs"]
pub mod work_memory_cache;
#[path = "../src/brain/coding_io.rs"]
pub mod coding_io;
#[path = "../src/brain/coding_workflow.rs"]
pub mod coding_workflow;
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
#[path = "../src/desktop_native/mod.rs"]
pub mod desktop_native;
#[path = "../src/plugin_contract.rs"]
pub mod plugin_contract;
#[path = "../src/provider_preset_updates.rs"]
pub mod provider_preset_updates;
#[path = "../src/plugin_advisor_contract.rs"]
pub mod plugin_advisor_contract;
#[path = "../src/pipeline/inline_tag_scanner.rs"]
pub mod inline_tag_scanner;
#[path = "../src/utils/action_text.rs"]
pub mod action_text;

#[cfg(test)]
mod action_text_output_tests {
    use super::{action_text::filter_parentheses, inline_tag_scanner::InlineTagScanner};

    #[test]
    fn inline_scanner_tail_is_filtered_before_stream_completion() {
        for (chunks, expected) in [
            (vec!["好呀（猫耳", "抖了抖<e"], "好呀"),
            (vec!["（点头）嗯(抬手", "<m name="], "嗯"),
            (vec!["你好<", "e"], "你好<e"),
            (vec!["（点头）", "<e name=\"happy\"/>", "好。"], "好。"),
        ] {
            let mut scanner = InlineTagScanner::new(Box::new(|_| {}));
            let mut deferred = String::new();
            let mut output = String::new();
            for chunk in chunks {
                let (visible, pending) = filter_parentheses(&scanner.feed(chunk), &deferred);
                output.push_str(&visible);
                deferred = pending;
            }
            let (visible, _) = filter_parentheses(&scanner.flush(), &deferred);
            output.push_str(&visible);
            assert_eq!(output, expected);
        }
    }
}
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
#[path = "../src/utils/sqlite.rs"]
pub mod sqlite;
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
    pub use crate::{fs, sqlite};
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
#[path = "../src/game_policy.rs"]
pub mod game_policy;
#[path = "../src/world/foreground_context.rs"]
pub mod foreground_context;
#[path = "../src/world/game_input_policy.rs"]
pub mod game_input_policy;
#[path = "../src/world/game_input.rs"]
mod game_input;

#[path = "../src/voice_diagnostics.rs"]
pub mod voice_diagnostics;

#[path = "../src/shortcut.rs"]
pub mod shortcut;

#[path = "../src/screen_capture.rs"]
pub mod screen_capture;
#[path = "../src/screen_freeze_geometry.rs"]
pub mod screen_freeze_geometry;

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

#[path = "../src/brain/coding_cancel.rs"]
pub mod coding_cancel;

#[path = "../src/brain/work_memory.rs"]
pub mod work_memory;
#[path = "../src/brain/work_instructions.rs"]
pub mod work_instructions;

#[path = "../src/speech/pcm.rs"]
mod speech_pcm;

#[path = "../src/utils/playback_gate.rs"]
mod speech_playback_gate;
