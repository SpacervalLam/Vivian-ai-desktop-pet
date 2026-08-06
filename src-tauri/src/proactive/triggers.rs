//! 触发器定义与阈值表

use serde::{Deserialize, Serialize};

/// 主动触发类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveTrigger {
    /// 整点问候
    HourlyGreeting,
    /// 空闲问候
    IdleGreeting,
    /// 调戏回应（拖拽触发）
    TeasingResponse,
    /// 破冰
    Icebreaker,
    /// 窗口切换
    WindowTrigger,
    /// 话题延展
    TopicExtension,
    /// 回忆式提问
    MemoryRecall,
    /// 健康提醒
    HealthReminder,
    /// 自发思考
    Spontaneous,
    /// 用户回归
    WelcomeBack,
    /// 心情驱动 —— Vivian 自身的需求/情绪积累到阈值时主动发声
    MoodDriven,
    /// 跨角色回应 —— 室友刚对用户说过话，本角色自然搭话/回应
    CrossCharacterReply,
    /// 旁观插话 —— 旁观用户与室友对话后主动插话
    BystanderInterjection,
}

impl ProactiveTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProactiveTrigger::HourlyGreeting => "hourly_greeting",
            ProactiveTrigger::IdleGreeting => "idle_greeting",
            ProactiveTrigger::TeasingResponse => "teasing_response",
            ProactiveTrigger::Icebreaker => "icebreaker",
            ProactiveTrigger::WindowTrigger => "window_trigger",
            ProactiveTrigger::TopicExtension => "topic_extension",
            ProactiveTrigger::MemoryRecall => "memory_recall",
            ProactiveTrigger::HealthReminder => "health_reminder",
            ProactiveTrigger::Spontaneous => "spontaneous",
            ProactiveTrigger::WelcomeBack => "welcome_back",
            ProactiveTrigger::MoodDriven => "mood_driven",
            ProactiveTrigger::CrossCharacterReply => "cross_character_reply",
            ProactiveTrigger::BystanderInterjection => "bystander_interjection",
        }
    }

    pub fn all() -> [ProactiveTrigger; 13] {
        [
            ProactiveTrigger::HourlyGreeting,
            ProactiveTrigger::IdleGreeting,
            ProactiveTrigger::TeasingResponse,
            ProactiveTrigger::Icebreaker,
            ProactiveTrigger::WindowTrigger,
            ProactiveTrigger::TopicExtension,
            ProactiveTrigger::MemoryRecall,
            ProactiveTrigger::HealthReminder,
            ProactiveTrigger::Spontaneous,
            ProactiveTrigger::WelcomeBack,
            ProactiveTrigger::MoodDriven,
            ProactiveTrigger::CrossCharacterReply,
            ProactiveTrigger::BystanderInterjection,
        ]
    }

    /// 优先级（数字越大越优先）
    pub fn priority(&self) -> u32 {
        match self {
            ProactiveTrigger::WelcomeBack => 100,
            ProactiveTrigger::TeasingResponse => 90,
            ProactiveTrigger::HealthReminder => 80,
            ProactiveTrigger::HourlyGreeting => 60,
            ProactiveTrigger::IdleGreeting => 50,
            ProactiveTrigger::Icebreaker => 40,
            ProactiveTrigger::WindowTrigger => 30,
            ProactiveTrigger::TopicExtension => 25,
            ProactiveTrigger::MemoryRecall => 20,
            ProactiveTrigger::MoodDriven => 15,
            ProactiveTrigger::BystanderInterjection => 13,
            ProactiveTrigger::CrossCharacterReply => 12,
            ProactiveTrigger::Spontaneous => 10,
        }
    }
}

impl std::fmt::Display for ProactiveTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 触发阈值配置
#[derive(Debug, Clone, Copy)]
pub struct TriggerThrottle {
    /// 触发阈值（0-1）
    pub threshold: f64,
    /// 冷却秒数
    pub cooldown_seconds: u64,
    /// 触发概率（0-1）
    pub probability: f64,
    /// 最小空闲秒数（仅部分触发器使用）
    pub min_idle_seconds: u64,
    /// 最小拖拽距离（仅 TeasingResponse 使用）
    pub min_drag_distance: f64,
    /// 最小离开秒数（仅 WelcomeBack 使用）
    pub min_away_seconds: u64,
}

impl TriggerThrottle {
    /// 获取触发器阈值配置
    ///
    /// 注意：`probability` / `threshold` 是否生效取决于 `check_trigger` 的门控分层
    /// - 全门控（冷却+时机+概率+冷却系数）：HourlyGreeting / IdleGreeting /
    ///   TeasingResponse / WindowTrigger / HealthReminder / Spontaneous / WelcomeBack
    /// - 仅冷却+时机：Icebreaker
    /// - 仅冷却：TopicExtension / MemoryRecall
    pub fn get(trigger: ProactiveTrigger) -> Self {
        match trigger {
            ProactiveTrigger::HourlyGreeting => Self {
                threshold: 0.5,
                cooldown_seconds: 600,
                probability: 0.12,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::IdleGreeting => Self {
                threshold: 0.6,
                cooldown_seconds: 1200,
                probability: 0.12,
                min_idle_seconds: 600,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::TeasingResponse => Self {
                threshold: 0.3,
                cooldown_seconds: 60,
                probability: 0.25,
                min_idle_seconds: 0,
                min_drag_distance: 200.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::Icebreaker => Self {
                threshold: 0.5,
                cooldown_seconds: 300,
                probability: 1.0,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::WindowTrigger => Self {
                threshold: 0.4,
                cooldown_seconds: 120,
                probability: 0.1,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::TopicExtension => Self {
                threshold: 0.5,
                cooldown_seconds: 600,
                probability: 0.08,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::MemoryRecall => Self {
                threshold: 0.6,
                cooldown_seconds: 900,
                probability: 0.06,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::HealthReminder => Self {
                threshold: 0.4,
                cooldown_seconds: 600,
                probability: 0.08,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::Spontaneous => Self {
                threshold: 0.5,
                cooldown_seconds: 1800,
                probability: 0.06,
                min_idle_seconds: 900,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::WelcomeBack => Self {
                threshold: 0.3,
                cooldown_seconds: 900,
                probability: 0.20,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 1800,
            },
            ProactiveTrigger::MoodDriven => Self {
                threshold: 0.6,
                cooldown_seconds: 2400,
                probability: 0.10,
                min_idle_seconds: 300,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::CrossCharacterReply => Self {
                threshold: 0.4,
                cooldown_seconds: 90,
                // 静态概率字段未被使用——CrossCharacterReply 的概率由
                // ProactiveOrchestrator::compute_cross_reply_probability 基于心情状态动态计算
                // （loneliness 越高概率越大）。这里保留 0.0 仅满足结构体完整性。
                probability: 0.0,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::BystanderInterjection => Self {
                threshold: 0.4,
                cooldown_seconds: 120,
                // 概率由 compute_bystander_interjection_probability 动态计算
                probability: 0.0,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
        }
    }

    /// 按概率决定是否触发
    pub fn roll_probability(&self) -> bool {
        // 使用系统时间作为简单随机源，避免引入 rand crate
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let r = (seed as f64 / u32::MAX as f64).min(1.0);
        r < self.probability
    }
}

/// 阈值表
pub static TRIGGER_THRESHOLDS: once_cell::sync::Lazy<
    std::collections::HashMap<&'static str, TriggerThrottle>,
> = once_cell::sync::Lazy::new(|| {
    use ProactiveTrigger::*;
    let triggers = [
        HourlyGreeting,
        IdleGreeting,
        TeasingResponse,
        Icebreaker,
        WindowTrigger,
        TopicExtension,
        MemoryRecall,
        HealthReminder,
        Spontaneous,
        WelcomeBack,
        MoodDriven,
        CrossCharacterReply,
        BystanderInterjection,
    ];
    triggers
        .iter()
        .map(|t| (t.as_str(), TriggerThrottle::get(*t)))
        .collect()
});
