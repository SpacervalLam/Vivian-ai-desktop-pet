//! 心理状态机 - 8 种心理状态

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PetMindState {
    Curious,
    Bored,
    Excited,
    Sleepy,
    Caring,
    Playful,
    Tired,
    Content,
}

impl PetMindState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PetMindState::Curious => "curious",
            PetMindState::Bored => "bored",
            PetMindState::Excited => "excited",
            PetMindState::Sleepy => "sleepy",
            PetMindState::Caring => "caring",
            PetMindState::Playful => "playful",
            PetMindState::Tired => "tired",
            PetMindState::Content => "content",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "bored" => PetMindState::Bored,
            "excited" => PetMindState::Excited,
            "sleepy" => PetMindState::Sleepy,
            "caring" => PetMindState::Caring,
            "playful" => PetMindState::Playful,
            "tired" => PetMindState::Tired,
            "content" => PetMindState::Content,
            _ => PetMindState::Curious,
        }
    }
}

impl Default for PetMindState {
    fn default() -> Self {
        PetMindState::Curious
    }
}

impl std::fmt::Display for PetMindState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
