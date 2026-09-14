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
    /// 日出 —— 检测到天亮时刻，主动提醒用户并推荐浅色主题
    Sunrise,
    /// 日落 —— 检测到天黑时刻，主动提醒用户并推荐深色主题
    Sunset,
    /// 系统资源压力 —— 内存占用越过阈值（normal→high 转换瞬间）时提醒用户
    SystemPressure,
    /// 主动截屏观察 —— 窗口切换引发好奇，经用户同意后截屏理解屏幕内容并搭话
    ScreenPeek,
    /// 应用持续使用超时 —— 同一类应用（如 IDE/游戏）连续使用超过时长阈值时，
    /// 按应用语义生成个性化关心/调侃
    AppDuration,
    /// 深夜未眠 —— 凌晨时段检测到用户仍活跃时，温柔地关心睡眠
    LateNight,
    /// 音乐切换 —— 用户开始播放/切换新曲目时，基于 SMTC 曲目信息自然搭话
    MusicChanged,
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
            ProactiveTrigger::Sunrise => "sunrise",
            ProactiveTrigger::Sunset => "sunset",
            ProactiveTrigger::SystemPressure => "system_pressure",
            ProactiveTrigger::ScreenPeek => "screen_peek",
            ProactiveTrigger::AppDuration => "app_duration",
            ProactiveTrigger::LateNight => "late_night",
            ProactiveTrigger::MusicChanged => "music_changed",
        }
    }

    pub fn all() -> [ProactiveTrigger; 20] {
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
            ProactiveTrigger::Sunrise,
            ProactiveTrigger::Sunset,
            ProactiveTrigger::SystemPressure,
            ProactiveTrigger::ScreenPeek,
            ProactiveTrigger::AppDuration,
            ProactiveTrigger::LateNight,
            ProactiveTrigger::MusicChanged,
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
            // 日出日落由世界事件驱动，仅在事件瞬间评估；优先级取低值避免抢占常规触发器
            ProactiveTrigger::Sunrise => 9,
            ProactiveTrigger::Sunset => 9,
            // 系统资源压力：设备健康类提醒，仅次于健康提醒
            ProactiveTrigger::SystemPressure => 75,
            // 主动截屏观察：好奇心驱动的轻量搭话
            ProactiveTrigger::ScreenPeek => 35,
            // 深夜未眠：健康关怀类，优先级高于普通问候
            ProactiveTrigger::LateNight => 76,
            // 应用持续使用超时：健康关怀类（按应用语义关心/调侃）
            ProactiveTrigger::AppDuration => 74,
            // 音乐切换：好奇心驱动的轻量搭话
            ProactiveTrigger::MusicChanged => 16,
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
                // 180s：与 companion_spoke 种子冷却（300s）拉开梯度，
                // 触发器路径（被动响应）频率高于思绪路径（主动分享），
                // 但不过频以免淹没自然积累的对室友分享。
                cooldown_seconds: 180,
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
            // 日出日落：事件驱动（世界事件转换瞬间触发），无需概率门控；
            // 冷却 1 小时兜底防重复（事件检测器正常只会转换一次）
            ProactiveTrigger::Sunrise => Self {
                threshold: 0.0,
                cooldown_seconds: 3600,
                probability: 1.0,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            ProactiveTrigger::Sunset => Self {
                threshold: 0.0,
                cooldown_seconds: 3600,
                probability: 1.0,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            // 系统资源压力：阈值驱动（内存 normal→high 转换），无需概率门控；
            // 冷却 30 分钟兜底，防止内存长期高位时反复提醒
            ProactiveTrigger::SystemPressure => Self {
                threshold: 0.0,
                cooldown_seconds: 1800,
                probability: 1.0,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            // 主动截屏观察：窗口切换事件驱动 + 概率 roll（0.12 × proactivity），
            // 冷却 1 小时；用户拒绝后另有 2 小时请求冷却（见 proactive::mod.rs）
            ProactiveTrigger::ScreenPeek => Self {
                threshold: 0.0,
                cooldown_seconds: 3600,
                probability: 0.12,
                min_idle_seconds: 30,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            // 应用持续使用超时：时长阈值由会话跟踪判断，无需概率门控；
            // 冷却 2 小时，防止同一会话内反复提醒（会话重置后重新计时）
            ProactiveTrigger::AppDuration => Self {
                threshold: 0.0,
                cooldown_seconds: 7200,
                probability: 1.0,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            // 深夜未眠：凌晨时段事件驱动，无需概率门控；
            // 冷却兜底 10 小时，且每晚只提醒一次（按日期去重，见 proactive::mod.rs）
            ProactiveTrigger::LateNight => Self {
                threshold: 0.0,
                cooldown_seconds: 36000,
                probability: 1.0,
                min_idle_seconds: 0,
                min_drag_distance: 0.0,
                min_away_seconds: 0,
            },
            // 音乐切换：曲目切换是相对高频事件，低概率抽样（0.3 × proactivity）；
            // 冷却 45 分钟，避免用户连续切歌时频繁搭话
            ProactiveTrigger::MusicChanged => Self {
                threshold: 0.0,
                cooldown_seconds: 2700,
                probability: 0.3,
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
    ProactiveTrigger::all()
        .iter()
        .map(|t| (t.as_str(), TriggerThrottle::get(*t)))
        .collect()
});
