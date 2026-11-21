//! PsychologySnapshot — 心理状态完整快照（持久化）。
//!
//! 这是整个心理系统的持久化载体，保存到 psychology.json。
//! Persona 是长期稳定的，Needs/Emotion/Relationship 是动态的，
//! Appraisal/BehaviorDrive 是上一次交互的缓存（供 prompt 参考上下文）。

use serde::{Deserialize, Serialize};

use super::appraisal::Appraisal;
use super::behavior_drive::BehaviorDrive;
use super::emotion::EmotionState;
use super::needs::NeedsState;
use super::persona::PersonaProfile;
use super::relationship::RelationshipState;

/// 情绪采样记录（用于情绪弧线叙事，仅保留数值序列）
///
/// 仅记录事件后的情绪快照，供日记"今天 Vivian 从 X 变成 Y"等弧线叙事使用。
/// 语义性事件摘要改由 LLM 产出 event_summary 并写入记忆系统 ImportantEvent。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsychEvent {
    pub timestamp: f64, // Unix 时间戳
    pub emotion_after: EmotionState,
}

/// 心理状态完整快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsychologySnapshot {
    /// 长期人格（数月变化）
    pub persona: PersonaProfile,
    /// 心理需求（动态，受 Homeostasis 调节）
    pub needs: NeedsState,
    /// 情绪状态（动态，受 Homeostasis 调节）
    pub emotion: EmotionState,
    /// 关系状态（动态，每次互动更新）
    pub relationship: RelationshipState,
    /// 上一次 Appraisal（供 prompt 参考上下文）
    pub last_appraisal: Option<Appraisal>,
    /// 上一次 Behavior Drive（供 prompt 参考上下文）
    pub last_drive: Option<BehaviorDrive>,
    /// 上次互动时间（Unix 时间戳）
    pub last_interaction_time: f64,
    /// 上次 Homeostasis tick 时间
    pub last_tick_time: f64,
    /// 近期心理事件（最多 30 条）
    pub events: Vec<PsychEvent>,
}

impl Default for PsychologySnapshot {
    fn default() -> Self {
        let now = chrono::Utc::now().timestamp() as f64;
        Self {
            persona: PersonaProfile::default(),
            needs: NeedsState::default(),
            emotion: EmotionState::default(),
            relationship: RelationshipState::default(),
            last_appraisal: None,
            last_drive: None,
            last_interaction_time: now,
            last_tick_time: now,
            events: Vec::new(),
        }
    }
}

impl PsychologySnapshot {
    /// 距上次互动的秒数
    pub fn secs_since_last_interaction(&self) -> f64 {
        let now = chrono::Utc::now().timestamp() as f64;
        (now - self.last_interaction_time).max(0.0)
    }

    /// 添加情绪采样（保留最近 30 条）
    pub fn add_event(&mut self, emotion_after: EmotionState) {
        let timestamp = chrono::Utc::now().timestamp() as f64;
        self.events.push(PsychEvent {
            timestamp,
            emotion_after,
        });
        if self.events.len() > 30 {
            self.events.remove(0);
        }
    }
}
