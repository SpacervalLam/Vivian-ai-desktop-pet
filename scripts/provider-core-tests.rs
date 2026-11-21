//! Low-memory source-based provider tests. Desktop bindings below are test doubles;
//! Desktop routing integration, plugin discovery, persona leak checks and URL fetching are NOT tested.
#![allow(dead_code)]
#![recursion_limit = "2048"]

#[path = "../src-tauri/src/utils/fs.rs"]
pub mod core_fs;
#[path = "../src-tauri/src/error.rs"]
mod error;
#[path = "../src-tauri/src/i18n/mod.rs"]
mod i18n;
#[path = "../src-tauri/src/messages.rs"]
mod messages;
#[path = "provider-core-modules.rs"]
mod providers;
#[path = "../src-tauri/src/resilience/mod.rs"]
mod resilience;
#[path = "../src-tauri/src/types/response.rs"]
pub mod response;
mod types {
    pub use crate::response;
}
#[path = "../src-tauri/src/brain/coding_event_log.rs"]
pub mod audit_log;
#[path = "../src-tauri/src/brain/coding_io.rs"]
pub mod coding_io;
pub use audit_log as coding_event_log;
#[path = "../src-tauri/src/brain/coding_event_capture.rs"]
pub mod audit_capture;
mod brain {
    pub use crate::audit_log as coding_event_log;
}

mod config {
    pub mod manager {
        pub struct ProviderConfig {
            pub base_url: String,
            pub api_key: String,
            pub model: String,
        }
    }
}
mod stickers {
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct StickerRef {
        pub id: String,
        pub character_id: String,
        pub version: String,
        pub label: String,
        pub meaning: String,
    }
}
mod plugins {
    pub struct Preset {
        pub reasoning_profiles: Vec<crate::providers::reasoning_profiles::ReasoningProfile>,
    }
    pub fn load_provider_presets() -> Vec<Preset> {
        vec![]
    }
}
mod persona {
    pub mod prompt_render {
        pub fn check_messages_for_leaks(_: &[crate::response::ChatMessage], _: &str) {}
    }
}
mod network {
    pub mod http_client {
        pub fn get_global_client() -> reqwest::Client {
            reqwest::Client::builder().no_proxy().build().unwrap()
        }
    }
    pub mod url_fetcher {
        pub fn validate_url(value: &str) -> crate::error::VivianResult<reqwest::Url> {
            let url = reqwest::Url::parse(value)
                .map_err(|error| crate::error::VivianError::Other(error.to_string()))?;
            if !matches!(url.scheme(), "https" | "http")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(crate::error::VivianError::Other("Invalid URL".into()));
            }
            Ok(url)
        }
    }
}
mod utils {
    pub use crate::core_fs as fs;
    pub mod path {
        pub fn get_user_data_dir() -> std::path::PathBuf {
            std::env::temp_dir().join(format!("vivian-provider-core-tests-{}", std::process::id()))
        }
    }
    // Cache-key integration remains covered by the full application's own tests.
    pub fn messages_cache_key(messages: &[crate::response::ChatMessage]) -> String {
        serde_json::to_string(messages).unwrap()
    }
}
