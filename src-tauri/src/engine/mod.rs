//! Live2D 引擎模块 - 管理动画、表情与状态机
//!

pub mod animation;
pub mod auto_trigger;
pub mod expression;
pub mod expression_stats;
pub mod feedback;
pub mod manifest;
pub mod motion_player;
pub mod pre_parsed;
pub mod presentation;
pub mod resource_loader;
pub mod state_machine;

// 顶层便捷重导出
pub use animation::{
    AnimationManager, AnimationStatistics, MotionCallback, MotionEventArgs,
    MotionPriority, MotionState,
};
pub use auto_trigger::{AutoExpressionTrigger, IdleStage, AUTO_TRIGGER, record_user_interaction, trigger_event, auto_trigger_tick, update_mood_state};
pub use expression::{
    ExpressionChangeCallback, ExpressionManager, ExpressionStatistics, RevertCallback,
    DEFAULT_EXPRESSION,
};
pub use motion_player::{MotionCurve, MotionFile, MotionPlayer};
pub use resource_loader::{
    ExpressionInfo, MotionInfo, PresetInfo, ResourceLoader, Resources, TextureInfo,
    CANVAS_EXTENSIONS, CDI_EXTENSIONS, EXPRESSION_EXTENSIONS, MODEL_EXTENSIONS,
    MOTION_EXTENSIONS, PHYSICS_EXTENSIONS, TEXTURE_EXTENSIONS,
    VTUBE_EXTENSIONS,
};
pub use state_machine::{
    PetState, StateCallback, StateMachine, StateMachineStatistics, StateTransition,
    TransitionCondition, DEFAULT_IDLE_INTERVAL_MAX, DEFAULT_IDLE_INTERVAL_MIN,
};
pub use manifest::{ResourceManifest, DEFAULT_MOTION as MANIFEST_DEFAULT_MOTION};
pub use presentation::{PresentationPack, PresentationSource};
pub use feedback::{FeedbackChannel, PassiveFeedbackEvent};

use serde::{Deserialize, Serialize};

/// 表情配置（前端可能依赖，保留原有结构）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionConfig {
    pub name: String,
    pub duration_ms: u32,
}

impl ExpressionConfig {
    pub fn new(name: impl Into<String>, duration_ms: u32) -> Self {
        Self {
            name: name.into(),
            duration_ms,
        }
    }
}

impl Default for ExpressionConfig {
    fn default() -> Self {
        Self::new("default", 3000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expression_config_default() {
        let config = ExpressionConfig::default();
        assert_eq!(config.name, "default");
        assert_eq!(config.duration_ms, 3000);
    }

    #[test]
    fn test_expression_config_new() {
        let config = ExpressionConfig::new("happy", 5000);
        assert_eq!(config.name, "happy");
        assert_eq!(config.duration_ms, 5000);
    }

    #[test]
    fn test_expression_config_serialize() {
        let config = ExpressionConfig::new("shy", 2000);
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"name\":\"shy\""));
        assert!(json.contains("\"duration_ms\":2000"));
    }

    #[test]
    fn test_pet_state_serialization() {
        let state = PetState::Idle;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"Idle\"");
        let deserialized: PetState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, PetState::Idle);
    }

    #[test]
    fn test_motion_priority_values() {
        assert_eq!(MotionPriority::Idle.value(), 0);
        assert_eq!(MotionPriority::Low.value(), 10);
        assert_eq!(MotionPriority::Normal.value(), 50);
        assert_eq!(MotionPriority::High.value(), 100);
        assert_eq!(MotionPriority::Critical.value(), 200);
    }
}
