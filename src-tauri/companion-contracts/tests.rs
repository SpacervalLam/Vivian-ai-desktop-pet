//! Runs the actual scene policy, throttles and interjection scoring without desktop bindings.
#![allow(dead_code)]
#[path = "../src/utils/background_priority.rs"]
mod background_priority;
#[path = "../src/pipeline/gather.rs"]
mod gather;
#[path = "../src/memory/retrieval_budget.rs"]
mod retrieval_budget;
mod gather_flow;
#[path = "../src/pipeline/steps/context_gather.rs"]
mod context_gather;
mod error { pub use crate::gather_flow::{VivianError, VivianResult}; }
mod pipeline {
    pub use crate::gather_flow::{base, state};
    pub mod gather { pub use crate::gather::gather; }
}
mod prompt { pub use crate::gather_flow::PromptBuildingStep; }
#[derive(Default)]
pub struct SessionStore(parking_lot::Mutex<Option<String>>);
impl SessionStore {
    pub fn get_session_id(&self) -> Option<String> { self.0.lock().clone() }
    pub fn set_session_id(&self, value: Option<String>) { *self.0.lock() = value; }
}
mod memory {
    pub type MemoryManager = crate::SessionStore;
    pub use crate::gather_flow::MemoryRetrievalStep;
}
mod dialogue { pub type DialogueManager = crate::SessionStore; }
#[path = "../src/utils/session_coordinator.rs"]
mod session_coordinator;
#[path = "../src/world/environment_input.rs"]
mod environment_input;
#[path = "../src/world/environment_policy.rs"]
mod environment_policy;
#[path = "../src/world/interaction_guidance.rs"]
mod interaction_guidance;
#[path = "../src/proactive/interjection.rs"]
mod interjection;
#[path = "../src/proactive/thought_lifecycle.rs"]
mod thought_lifecycle;
#[path = "../src/proactive/triggers.rs"]
mod triggers;

#[test]
fn default_idle_triggers_fit_before_the_absence_boundary() {
    use triggers::{ProactiveTrigger, TriggerThrottle};
    for trigger in [
        ProactiveTrigger::IdleGreeting,
        ProactiveTrigger::Spontaneous,
    ] {
        let throttle = TriggerThrottle::get(trigger);
        let threshold = environment_policy::idle_threshold(throttle.min_idle_seconds.max(300), 600);
        assert!(threshold >= 120.0 && threshold < 600.0);
    }
}

#[test]
fn cancelling_an_older_optional_turn_cannot_clear_the_users_new_turn_or_session() {
    use std::sync::Arc;
    use session_coordinator::{SessionCoordinator, TurnKind};
    let coordinator = SessionCoordinator::new();
    let memory = Arc::new(SessionStore::default());
    let dialogue = Arc::new(SessionStore::default());
    let older = coordinator.try_enter_proactive_turn("vivian", &memory, &dialogue).unwrap();
    coordinator.signal_user_input("vivian");
    let user = coordinator.enter_user_turn("vivian", "user-session", &memory, &dialogue);
    drop(older);
    assert_eq!(coordinator.current_turn_kind("vivian"), Some(TurnKind::UserChat));
    assert_eq!(memory.get_session_id().as_deref(), Some("user-session"));
    assert_eq!(dialogue.get_session_id().as_deref(), Some("user-session"));
    drop(user);
    assert!(coordinator.current_turn_kind("vivian").is_none());
    assert!(memory.get_session_id().is_none());
}
