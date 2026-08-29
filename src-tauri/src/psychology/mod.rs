//! Psychology 模块 — 五层心理架构。
//!
//! 架构因果链：
//! ```text
//! Persona（长期人格，含依恋特质）
//!     ↓ 调制 set_points / recovery_rates
//! Needs（5 项需求，带 set point）  ← Homeostasis 在此作用
//!     ↑
//! 事件 → LLM 单次调用产出：
//!     {appraisal(6项), emotion_update(7项), behavior_drive(8项), reply}
//!     ↓
//! Appraisal → Emotion（心理学固定映射）
//!     ↓
//! Emotion（7 项情绪）  ← Homeostasis 在此作用
//!     ↓
//! Behavior Drive（8 项行为驱动）— 混合模式（LLM + 规则）
//!     ↓
//! 行为决策模块（LLM/规则混合，带场景约束）
//!     ↓
//! 外显行为 + Mood + PetState（实时计算，仅 UI，不参与决策）
//! ```
//!
//! 核心原则：
//! - Mood / PetState 不参与决策，仅 UI 展示
//! - Appraisal 是 Emotion 的前置（事件不直接产生情绪）
//! - Homeostasis 让所有维度围绕 set point 自动调节
//! - Behavior Drive 混合模式：对话轮 LLM 决策，主动 tick 规则决策
//! - LLM 一次调用产出全部心理状态（不拆成多次调用）
//! - EmotionLabel 是系统唯一情绪枚举（7 项），旧 EmotionType 已废弃

pub mod appraisal;
pub mod behavior_drive;
pub mod emotion;
pub mod homeostasis;
pub mod manager;
pub mod mood;
pub mod mood_cue;
pub mod needs;
pub mod persona;
pub mod pet_state;
pub mod relationship;
pub mod relationship_facts;
pub mod relationship_log;
pub mod social_state;
pub mod snapshot;

// 便捷 re-export
pub use appraisal::Appraisal;
pub use behavior_drive::{BehaviorDrive, DriveSource, DriveLabel, RuleBasedDriveResolver};
pub use emotion::{EmotionDeltas, EmotionLabel, EmotionState, EmotionVector8D};
pub use homeostasis::{EmotionSetPoints, HomeostasisEngine, NeedSetPoints, RecoveryRateProfile};
pub use manager::{default_psychology_path, InteractionFeedback, PsychologyManager, PsychologyOutput};
pub use mood::{compute_mood, MoodSnapshot};
pub use needs::{NeedDeltas, NeedsState};
pub use persona::{AttachmentStyle, ExpressionHint, PersonaProfile, PersonaTraits};
pub use pet_state::{compute_pet_state, PetState, StateMeta};
pub use relationship::{
    permanent_strategy, temporary_strategy, MilestoneEntry, RelationshipDeltas, RelationshipEvent,
    RelationshipStage, RelationshipState, StageStrategy, TemporaryStage, EVENT_INTERACTION,
    EVENT_LONG_ABSENCE, EVENT_TIME_PASSAGE, EVENT_USER_RETURNED, EVENT_USER_SAD,
};
pub use relationship_log::{
    relationship_log, date_str_from_ts, today_date_str, yesterday_date_str,
    RelationshipDailySummary, RelationshipDirection, RelationshipLogEngine, RelationshipLogEntry,
};
pub use relationship_facts::{
    relationship_facts, FactCategory, RelationshipFact, RelationshipFactsEngine,
};
pub use social_state::{
    deltas_from_cross_character_sentiment, sentiment_from_signal_text, social_state,
    SocialStateEngine, SocialStateSnapshot,
};
pub use snapshot::{PsychEvent, PsychologySnapshot};
