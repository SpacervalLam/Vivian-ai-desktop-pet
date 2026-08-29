//! 角色行为参数 —— 按 char_id 索引的本地非 LLM 控制参数。
//!
//! 这些参数控制主动发话反馈幅度、MoodDriven 触发阈值、亲密度冷却系数、
//! 安静模式触发阈值、表情触发冷却等本地行为，让不同角色表现出不同的节奏感。
//!
//! 多角色去同步策略参数（A~F）也在此定义，确保两个角色的主动问候时机自然错开。

use once_cell::sync::Lazy;
use std::collections::HashMap;

use crate::proactive::triggers::ProactiveTrigger;

// ============ 策略 A：Tick 相位抖动 ============

/// Tick 间隔抖动范围（乘数）
///
/// 每次 tick 返回的 recommended_next_interval_ms 会乘以 [min, max] 内的随机因子，
/// 让两个角色的 tick 相位在几轮后自然漂移、不再收敛。
#[derive(Debug, Clone, Copy)]
pub struct TickJitterConfig {
    pub min: f64,
    pub max: f64,
}

// ============ 策略 B：性格驱动 TimingJudger 权重 ============

/// 角色专属的 TimingJudger 维度权重（总和应为 1.0）
///
/// Vivian 偏重 idle（忍不住在用户空闲时开口），Nana 偏重 time-of-day（按作息规律关心人）。
#[derive(Debug, Clone, Copy)]
pub struct TimingWeights {
    pub idle: f64,
    pub schedule: f64,
    pub time: f64,
    pub cooldown: f64,
    pub frequency: f64,
}

/// 角色专属的触发阈值/冷却/概率修正系数
///
/// 乘在 TriggerThrottle 的基准值上，让同一种触发器在不同角色身上表现不同。
#[derive(Debug, Clone, Copy)]
pub struct TriggerModifiers {
    /// 阈值乘数（>1.0 更难触发）
    pub threshold_mult: f64,
    /// 冷却乘数（>1.0 冷却更久）
    pub cooldown_mult: f64,
    /// 概率乘数（<1.0 概率更低）
    pub probability_mult: f64,
}

// ============ 策略 C：说话欲望累积器 ============

/// 说话欲望（speech_desire）参数
///
/// 每个 tick 按性格参数自动增减，成功说话后归零。
/// 当 speech_desire < threshold 时跳过所有 greeting 类触发。
/// 两个角色增长曲线不同 → 峰值出现在不同时刻。
#[derive(Debug, Clone, Copy)]
pub struct SpeechDesireConfig {
    /// 每 tick 基础增长率
    pub base_growth: f64,
    /// 被忽略/用户忙碌时的额外增长（Vivian 越不理越想说话）
    pub ignored_boost: f64,
    /// 用户忙碌时 Nana 的主动退让衰减（负增长）
    pub user_busy_decay: f64,
    /// 触发 greeting 的最低欲望阈值
    pub threshold: f64,
    /// 说话成功后是否归零
    pub reset_on_speak: bool,
    /// 初始/重置后的欲望值（错开两角色到达阈值的时刻）
    pub initial_desire: f64,
}

// ============ 策略 D：发言仲裁 ============

/// 跨角色发言仲裁参数
///
/// priority 越小越优先（同时想说话时 priority 小的先说）。
/// reluctance 是"被对方发言阻塞后"的冷却乘数（×CROSS_ROLE_COOLDOWN_SECS）。
#[derive(Debug, Clone, Copy)]
pub struct ArbitrationConfig {
    /// 优先级（1=最高）
    pub priority: u8,
    /// 被阻塞时的冷却乘数（Vivian 不甘心等得短，Nana 体贴等得长）
    pub reluctance: f64,
    /// 同时产生意图时，低优先级角色的额外延迟秒数
    pub yield_delay_secs: f64,
}

// ============ 策略 E：情绪自动浮动 ============

/// 情绪自动浮动参数（Mood Drift）
///
/// 影响 compute_overall_cooling 中的 emotion_mult，
/// 让两个角色的"冷却曲线"形状不同：
/// Vivian 锯齿形（快速升高→快速冷却→又快速升高），
/// Nana 缓坡形（慢慢暖→慢慢凉），自然错开。
#[derive(Debug, Clone, Copy)]
pub struct MoodDriftConfig {
    /// 静息情绪基线（Vivian 偏中性 0.4, Nana 偏暖 0.6）
    pub base_valence: f64,
    /// 波动幅度（Vivian 大 0.3, Nana 小 0.1）
    pub volatility: f64,
    /// 回正速度（Vivian 慢, Nana 快）
    pub recovery_rate: f64,
    /// 被忽略时烦躁增速（Vivian 高, Nana 低）
    pub irritation_growth: f64,
    /// 开心衰减速度（Vivian 快—傲娇不让自己一直开心, Nana 慢）
    pub joy_decay: f64,
    /// 初始相位偏移（弧度，0~TAU）。错开两角色的情绪周期起点。
    pub initial_phase: f64,
}

// ============ 策略 F：触发类型领地划分 ============

/// 触发类型亲和度乘数
///
/// 乘在 check_trigger 的概率门控上。
/// 主攻类型 >1.0（更容易触发），非主攻类型 <1.0（概率衰减）。
/// Vivian 主攻情绪化/忍不住型，Nana 主攻关怀/规律型。
#[derive(Debug, Clone, Copy)]
pub struct TriggerAffinity {
    pub mood_driven: f64,
    pub icebreaker: f64,
    pub welcome_back: f64,
    pub spontaneous: f64,
    pub hourly_greeting: f64,
    pub idle_greeting: f64,
    pub health_reminder: f64,
    pub window_trigger: f64,
    pub topic_extension: f64,
    pub memory_recall: f64,
    pub teasing_response: f64,
    pub cross_character_reply: f64,
    pub bystander_interjection: f64,
}

impl TriggerAffinity {
    pub fn get(&self, trigger: ProactiveTrigger) -> f64 {
        match trigger {
            ProactiveTrigger::MoodDriven => self.mood_driven,
            ProactiveTrigger::Icebreaker => self.icebreaker,
            ProactiveTrigger::WelcomeBack => self.welcome_back,
            ProactiveTrigger::Spontaneous => self.spontaneous,
            ProactiveTrigger::HourlyGreeting => self.hourly_greeting,
            ProactiveTrigger::IdleGreeting => self.idle_greeting,
            ProactiveTrigger::HealthReminder => self.health_reminder,
            ProactiveTrigger::WindowTrigger => self.window_trigger,
            ProactiveTrigger::TopicExtension => self.topic_extension,
            ProactiveTrigger::MemoryRecall => self.memory_recall,
            ProactiveTrigger::TeasingResponse => self.teasing_response,
            ProactiveTrigger::CrossCharacterReply => self.cross_character_reply,
            ProactiveTrigger::BystanderInterjection => self.bystander_interjection,
            // 日出日落由世界事件直接驱动，不走概率门控，取中性亲和度
            ProactiveTrigger::Sunrise | ProactiveTrigger::Sunset => 1.0,
            // 系统压力 / 主动截屏 / 应用时长 / 深夜未眠 / 音乐切换为事件驱动（tick 专门路径），取中性亲和度
            ProactiveTrigger::SystemPressure
            | ProactiveTrigger::ScreenPeek
            | ProactiveTrigger::AppDuration
            | ProactiveTrigger::LateNight
            | ProactiveTrigger::MusicChanged => 1.0,
        }
    }
}

// ============ 主结构体 ============

/// 角色行为参数
#[derive(Debug, Clone, Copy)]
pub struct CharacterBehavior {
    /// 主动发话正向反馈：用户回应 → intimacy 增幅
    pub proactive_feedback_positive: f64,
    /// 主动发话负向反馈：冷落 → intimacy 降幅
    pub proactive_feedback_negative: f64,
    /// MoodDriven 触发：需求压力阈值（越高越矜持）
    pub mood_driven_need_threshold: f64,
    /// MoodDriven 触发：孤独感阈值（越高越不轻易示弱）
    pub mood_driven_loneliness_threshold: f64,
    /// 亲密度冷却系数乘数（<1.0 更冷淡，>1.0 更热情）
    pub intimacy_cooldown_multiplier: f64,
    /// 安静模式触发：连续被忽略次数（越大越倔强）
    pub quiet_mode_threshold: u32,
    /// 表情触发冷却秒数（越大越克制）
    pub mood_expression_cooldown_secs: i64,

    // ── 去同步策略参数 ──
    /// 策略 A：tick 相位抖动
    pub tick_jitter: TickJitterConfig,
    /// 策略 B：TimingJudger 权重
    pub timing_weights: TimingWeights,
    /// 策略 B：触发阈值/冷却/概率修正
    pub trigger_modifiers: TriggerModifiers,
    /// 策略 C：说话欲望累积器
    pub speech_desire: SpeechDesireConfig,
    /// 策略 D：发言仲裁
    pub arbitration: ArbitrationConfig,
    /// 策略 E：情绪自动浮动
    pub mood_drift: MoodDriftConfig,
    /// 策略 F：触发类型亲和度
    pub trigger_affinity: TriggerAffinity,
}

impl CharacterBehavior {
    /// Vivian：傲娇网瘾少女 —— 慢热、记仇、矜持、冷淡、倔强、表情克制
    ///
    /// 去同步特征：
    /// - tick 抖动窄（0.8~1.2）→ 节奏快且稳定，容易先忍不住开口
    /// - idle 权重高 → 用户一空闲就想说话
    /// - 阈值高 + 概率低 → 不轻易承认想说话
    /// - 冷却长 → 说完一次要憋更久
    /// - speech_desire 增长快 → 内心戏多，峰值来得早
    /// - 情绪锯齿 → 快速升高→快速冷却→又快速升高
    /// - 主攻情绪化触发器（MoodDriven/Icebreaker/WelcomeBack）
    const VIVIAN: Self = Self {
        proactive_feedback_positive: 0.002,
        proactive_feedback_negative: 0.003,
        mood_driven_need_threshold: 0.85,
        mood_driven_loneliness_threshold: 0.75,
        intimacy_cooldown_multiplier: 0.8,
        quiet_mode_threshold: 5,
        mood_expression_cooldown_secs: 30,

        tick_jitter: TickJitterConfig { min: 0.8, max: 1.2 },
        timing_weights: TimingWeights {
            idle: 0.35,
            schedule: 0.15,
            time: 0.15,
            cooldown: 0.20,
            frequency: 0.15,
        },
        trigger_modifiers: TriggerModifiers {
            threshold_mult: 1.2,
            cooldown_mult: 1.5,
            probability_mult: 0.8,
        },
        speech_desire: SpeechDesireConfig {
            base_growth: 0.08,
            ignored_boost: 0.12,
            user_busy_decay: 0.02,
            threshold: 0.6,
            reset_on_speak: true,
            initial_desire: 0.35,
        },
        arbitration: ArbitrationConfig {
            priority: 1,
            reluctance: 2.0,
            yield_delay_secs: 0.0,
        },
        mood_drift: MoodDriftConfig {
            base_valence: 0.4,
            volatility: 0.3,
            recovery_rate: 0.02,
            irritation_growth: 0.06,
            joy_decay: 0.05,
            initial_phase: 0.0,
        },
        trigger_affinity: TriggerAffinity {
            mood_driven: 1.3,
            icebreaker: 1.2,
            welcome_back: 1.3,
            spontaneous: 1.2,
            hourly_greeting: 0.4,
            idle_greeting: 0.5,
            health_reminder: 0.3,
            window_trigger: 0.8,
            topic_extension: 1.2,
            memory_recall: 0.6,
            teasing_response: 1.1,
            cross_character_reply: 1.0,
            bystander_interjection: 1.1,
        },
    };

    /// Nana：温柔从容的姐姐 —— 容易亲近、宽容、主动关心、热情、敏感、表情丰富
    ///
    /// 去同步特征：
    /// - tick 抖动宽（0.9~1.4）→ 节奏慢且不规则，自然错开
    /// - time-of-day 权重高 → 按作息规律关心人
    /// - 阈值低 + 概率高 → 更容易触发关怀型问候
    /// - 冷却短 → 可以更频繁地轻声关心
    /// - speech_desire 增长慢 → 安静陪伴型，峰值来得晚
    /// - 情绪缓坡 → 慢慢暖→慢慢凉
    /// - 主攻关怀/规律型触发器（Hourly/Idle/HealthReminder）
    const NANA: Self = Self {
        proactive_feedback_positive: 0.005,
        proactive_feedback_negative: 0.001,
        mood_driven_need_threshold: 0.65,
        mood_driven_loneliness_threshold: 0.55,
        intimacy_cooldown_multiplier: 1.2,
        quiet_mode_threshold: 2,
        mood_expression_cooldown_secs: 15,

        tick_jitter: TickJitterConfig { min: 0.9, max: 1.4 },
        timing_weights: TimingWeights {
            idle: 0.15,
            schedule: 0.20,
            time: 0.35,
            cooldown: 0.15,
            frequency: 0.15,
        },
        trigger_modifiers: TriggerModifiers {
            threshold_mult: 0.8,
            cooldown_mult: 0.7,
            probability_mult: 1.3,
        },
        speech_desire: SpeechDesireConfig {
            base_growth: 0.04,
            ignored_boost: 0.02,
            user_busy_decay: 0.06,
            threshold: 0.4,
            reset_on_speak: true,
            initial_desire: 0.05,
        },
        arbitration: ArbitrationConfig {
            priority: 2,
            reluctance: 4.0,
            yield_delay_secs: 90.0,
        },
        mood_drift: MoodDriftConfig {
            base_valence: 0.6,
            volatility: 0.1,
            recovery_rate: 0.05,
            irritation_growth: 0.02,
            joy_decay: 0.01,
            initial_phase: std::f64::consts::PI,
        },
        trigger_affinity: TriggerAffinity {
            mood_driven: 0.5,
            icebreaker: 0.6,
            welcome_back: 0.8,
            spontaneous: 0.5,
            hourly_greeting: 1.3,
            idle_greeting: 1.2,
            health_reminder: 1.4,
            window_trigger: 0.9,
            topic_extension: 0.6,
            memory_recall: 1.2,
            teasing_response: 0.8,
            cross_character_reply: 1.0,
            bystander_interjection: 0.8,
        },
    };

    /// 默认参数（未知角色时使用，与 Vivian 一致）
    const DEFAULT: Self = Self::VIVIAN;
}

static BEHAVIOR_REGISTRY: Lazy<HashMap<&'static str, CharacterBehavior>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert("vivian", CharacterBehavior::VIVIAN);
    map.insert("nana", CharacterBehavior::NANA);
    map
});

/// 按 char_id 获取角色行为参数，未知角色返回默认值
pub fn get_behavior(char_id: &str) -> CharacterBehavior {
    BEHAVIOR_REGISTRY
        .get(char_id)
        .copied()
        .unwrap_or(CharacterBehavior::DEFAULT)
}
