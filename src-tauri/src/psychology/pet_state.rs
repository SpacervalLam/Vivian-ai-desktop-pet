//! PetState — 桌宠衍生状态（18 种，仅 UI 展示）。
//!
//! PetState 不参与决策，由 EmotionState + Needs + Relationship 投影推导。
//! 真正驱动行为的是 Behavior Drive，PetState 仅用于前端展示标签。
//!
//! 保留 18 种细粒度标签是为了让前端 UI 有丰富的情感展示
//! （如「调皮」「依恋」「困倦」等），但这些标签不会反向影响决策。

use serde::{Deserialize, Serialize};

use super::emotion::EmotionState;
use super::needs::NeedsState;
use super::relationship::RelationshipState;

/// 桌宠衍生状态（18 种）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PetState {
    // 积极高唤醒
    Joyful,
    Excited,
    Playful,
    // 积极低唤醒
    Calm,
    Content,
    Affectionate,
    // 消极高唤醒
    Anxious,
    Angry,
    Frustrated,
    Worried,
    // 消极低唤醒
    Sad,
    Tired,
    Sleepy,
    Bored,
    Lonely,
    // 中性/其他
    Curious,
    Shy,
    Neutral,
}

impl Default for PetState {
    fn default() -> Self {
        PetState::Neutral
    }
}

/// 衍生状态元数据（图标 / 中文标签 / 颜色）
#[derive(Debug, Clone, Copy)]
pub struct StateMeta {
    pub icon: &'static str,
    pub label: &'static str,
    pub color: &'static str,
}

impl PetState {
    pub fn as_label(&self) -> &'static str {
        match self {
            PetState::Joyful => "joyful",
            PetState::Excited => "excited",
            PetState::Playful => "playful",
            PetState::Calm => "calm",
            PetState::Content => "content",
            PetState::Affectionate => "affectionate",
            PetState::Anxious => "anxious",
            PetState::Angry => "angry",
            PetState::Frustrated => "frustrated",
            PetState::Worried => "worried",
            PetState::Sad => "sad",
            PetState::Tired => "tired",
            PetState::Sleepy => "sleepy",
            PetState::Bored => "bored",
            PetState::Lonely => "lonely",
            PetState::Curious => "curious",
            PetState::Shy => "shy",
            PetState::Neutral => "neutral",
        }
    }

    pub fn from_label(label: &str) -> Self {
        match label {
            "joyful" => PetState::Joyful,
            "excited" => PetState::Excited,
            "playful" => PetState::Playful,
            "calm" => PetState::Calm,
            "content" => PetState::Content,
            "affectionate" => PetState::Affectionate,
            "anxious" => PetState::Anxious,
            "angry" => PetState::Angry,
            "frustrated" => PetState::Frustrated,
            "worried" => PetState::Worried,
            "sad" => PetState::Sad,
            "tired" => PetState::Tired,
            "sleepy" => PetState::Sleepy,
            "bored" => PetState::Bored,
            "lonely" => PetState::Lonely,
            "curious" => PetState::Curious,
            "shy" => PetState::Shy,
            _ => PetState::Neutral,
        }
    }

    /// 获取状态元数据（icon / label / color）
    pub fn meta(&self) -> StateMeta {
        match self {
            PetState::Joyful => StateMeta { icon: "😊", label: "喜悦", color: "#FEE440" },
            PetState::Excited => StateMeta { icon: "🥰", label: "兴奋", color: "#FF007F" },
            PetState::Playful => StateMeta { icon: "😜", label: "调皮", color: "#FF6B6B" },
            PetState::Calm => StateMeta { icon: "😌", label: "平静", color: "#00F5D4" },
            PetState::Content => StateMeta { icon: "☺️", label: "满足", color: "#90BE6D" },
            PetState::Affectionate => StateMeta { icon: "🥺", label: "依恋", color: "#F8B4D9" },
            PetState::Anxious => StateMeta { icon: "😰", label: "焦虑", color: "#F9C74F" },
            PetState::Angry => StateMeta { icon: "😠", label: "生气", color: "#F94144" },
            PetState::Frustrated => StateMeta { icon: "😤", label: "沮丧", color: "#F3722C" },
            PetState::Worried => StateMeta { icon: "😟", label: "担心", color: "#F8961E" },
            PetState::Sad => StateMeta { icon: "😢", label: "难过", color: "#577590" },
            PetState::Tired => StateMeta { icon: "😫", label: "疲惫", color: "#577590" },
            PetState::Sleepy => StateMeta { icon: "😴", label: "困倦", color: "#277DA1" },
            PetState::Bored => StateMeta { icon: "😒", label: "无聊", color: "#6C757D" },
            PetState::Lonely => StateMeta { icon: "🥺", label: "孤独", color: "#577590" },
            PetState::Curious => StateMeta { icon: "🤔", label: "好奇", color: "#4D908E" },
            PetState::Shy => StateMeta { icon: "😊", label: "害羞", color: "#F8B4D9" },
            PetState::Neutral => StateMeta { icon: "😐", label: "默认", color: "#ADB5BD" },
        }
    }

    /// 映射到 Live2D 表情名
    ///
    /// 返回空字符串表示不触发表情变化。
    pub fn to_live2d_expression(&self) -> &'static str {
        match self {
            PetState::Joyful | PetState::Excited | PetState::Playful | PetState::Affectionate
            | PetState::Shy => "shy",
            PetState::Angry | PetState::Frustrated | PetState::Anxious => "angry",
            PetState::Curious => "confused",
            PetState::Calm | PetState::Content | PetState::Worried | PetState::Sad
            | PetState::Tired | PetState::Sleepy | PetState::Bored | PetState::Lonely
            | PetState::Neutral => "",
        }
    }
}

/// 由 EmotionState + Needs + Relationship 推导 PetState
///
/// 优先级：负面情绪 > 疲劳 > 正面情绪 > 中性
pub fn compute_pet_state(
    emotion: &EmotionState,
    needs: &NeedsState,
    relationship: &RelationshipState,
    last_interaction_secs: f64,
) -> PetState {
    let valence = emotion.valence();
    let arousal = emotion.arousal();

    // 疲劳度（与 mood.rs 中公式一致）
    let need_burden = (needs.belonging + needs.security + needs.expression) / 3.0;
    let fatigue = (last_interaction_secs / 60.0 * 0.5 + need_burden * 40.0).clamp(0.0, 100.0);

    let current_hour = chrono::Local::now().format("%H").to_string().parse::<i32>().unwrap_or(12);
    let is_night = current_hour >= 22 || current_hour < 8;

    // 消极高唤醒
    if emotion.anger > 0.6 && arousal > 0.6 {
        return PetState::Angry;
    }
    if emotion.fear > 0.6 && arousal > 0.5 {
        return PetState::Anxious;
    }
    if valence < -0.3 && arousal > 0.5 {
        return PetState::Frustrated;
    }
    if emotion.fear > 0.4 || (valence < -0.2 && arousal > 0.4) {
        return PetState::Worried;
    }
    // 消极低唤醒
    if fatigue > 80.0 && is_night {
        return PetState::Sleepy;
    }
    if fatigue > 70.0 {
        return PetState::Tired;
    }
    if emotion.loneliness > 0.6 && relationship.intimacy < 0.4 {
        return PetState::Lonely;
    }
    if arousal < 0.25 && valence < 0.0 {
        return PetState::Bored;
    }
    if emotion.sadness > 0.5 {
        return PetState::Sad;
    }
    // 积极高唤醒
    if emotion.joy > 0.6 && arousal > 0.6 && fatigue < 40.0 {
        return PetState::Excited;
    }
    if emotion.joy > 0.5 && arousal > 0.5 {
        return PetState::Playful;
    }
    if emotion.joy > 0.5 {
        return PetState::Joyful;
    }
    // 积极低唤醒
    if emotion.closeness > 0.6 && relationship.intimacy > 0.4 {
        return PetState::Affectionate;
    }
    if valence > 0.2 && arousal < 0.4 {
        return PetState::Content;
    }
    if valence > 0.0 && arousal < 0.35 {
        return PetState::Calm;
    }
    // 中性
    if emotion.curiosity > 0.5 && arousal > 0.4 {
        return PetState::Curious;
    }
    if relationship.intimacy > 0.3 && relationship.trust < 0.5 {
        return PetState::Shy;
    }
    PetState::Neutral
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_neutral() {
        assert_eq!(PetState::default(), PetState::Neutral);
    }

    #[test]
    fn test_label_roundtrip() {
        for state in [
            PetState::Joyful, PetState::Excited, PetState::Playful, PetState::Calm,
            PetState::Content, PetState::Affectionate, PetState::Anxious, PetState::Angry,
            PetState::Frustrated, PetState::Worried, PetState::Sad, PetState::Tired,
            PetState::Sleepy, PetState::Bored, PetState::Lonely, PetState::Curious,
            PetState::Shy, PetState::Neutral,
        ] {
            let label = state.as_label();
            assert_eq!(PetState::from_label(label), state);
        }
    }

    #[test]
    fn test_compute_pet_state_joyful() {
        let emotion = EmotionState {
            joy: 0.8,
            ..Default::default()
        };
        let needs = NeedsState::default();
        let rel = RelationshipState::default();
        let state = compute_pet_state(&emotion, &needs, &rel, 0.0);
        // 高 joy 应该映射到 Joyful/Excited/Playful 之一
        assert!(matches!(state, PetState::Joyful | PetState::Excited | PetState::Playful));
    }

    #[test]
    fn test_compute_pet_state_angry() {
        let emotion = EmotionState {
            anger: 0.8,
            ..Default::default()
        };
        let needs = NeedsState::default();
        let rel = RelationshipState::default();
        let state = compute_pet_state(&emotion, &needs, &rel, 0.0);
        assert_eq!(state, PetState::Angry);
    }
}
