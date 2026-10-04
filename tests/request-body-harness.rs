//! Lightweight test entry point for request customization, without linking the desktop app.
#![allow(dead_code)]
#[path = "../src-tauri/src/providers/capabilities.rs"]
pub mod capabilities;
#[path = "../src-tauri/src/providers/reasoning.rs"]
pub mod reasoning;
#[path = "../src-tauri/src/providers/reasoning_profiles.rs"]
pub mod reasoning_profiles;
#[path = "../src-tauri/src/providers/request_body.rs"]
pub mod request_body;
pub mod providers { pub use crate::{capabilities, reasoning}; }
pub mod plugins {
    #[derive(serde::Deserialize)]
    pub struct Preset {
        #[serde(default, rename = "reasoningProfiles")]
        pub reasoning_profiles: Vec<crate::reasoning_profiles::ReasoningProfile>,
    }
    pub fn load_provider_presets() -> Vec<Preset> {
        serde_json::from_str(include_str!("../src-tauri/plugins/llm-providers/providers.json")).unwrap()
    }
}
