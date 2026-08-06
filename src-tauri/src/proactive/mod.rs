//! 主动对话编排 - 周期性触发桌宠的主动行为
//!
//! - 10 秒 tick 调度
//! - 10 种触发类型 + 多级冷却系数（emotion/time/activity/intimacy）
//! - 8 种心理状态（PetMindState）
//! - 安静模式（连续被忽略 3 次进入 1 小时静默）
//! - 子模块：TimingJudger / BehaviorDecider / IcebreakerGenerator /
//!   TopicPool / MemoryRecall / HealthReminder / Recommender /
//!   StressMonitor / HabitTracker / BehaviorModeManager
//!
//! 持久化：
//! - `%APPDATA%\Vivian\proactive\state.json`（编排器状态）
//! - `%APPDATA%\Vivian\proactive\topics.json`（话题冷却）
//! - `%APPDATA%\Vivian\proactive\habits.json`（习惯数据）

pub mod behavior;
pub mod behavior_modes;
pub mod capability_planner;
pub mod habits;
pub mod icebreaker;
pub mod inner_monologue;
pub mod mind_state;
pub mod preference_learner;
pub mod services;
pub mod timing;
pub mod topics;
pub mod triggers;

pub mod activity_journal;
pub mod thought_trigger;
pub mod thought_lifecycle;

pub use activity_journal::{ActivityEntry, ActivityJournal};
pub use thought_lifecycle::{ThoughtLifecycle, ActiveThought, ThoughtPhase};
pub use thought_trigger::{ThoughtTriggerEvaluator, ThoughtSeed};
pub use behavior::{BehaviorContent, BehaviorDecider};
pub use capability_planner::{CapabilityPlan, CapabilityPlanner};
pub use behavior_modes::{BehaviorModeManager, PetBehaviorMode};
pub use habits::{classify_app, AppCategory, HabitTracker};
pub use icebreaker::{IceBreakerLevel, IcebreakerContent, IcebreakerGenerator};
pub use mind_state::PetMindState;
pub use services::{HealthReminder, Recommender, StressLevel, StressMonitor};
pub use timing::TimingJudger;
pub use topics::{DailyTopicPool, InterestExtender, MemoryRecall, TopicPool, TopicTree};
pub use triggers::{ProactiveTrigger, TriggerThrottle, TRIGGER_THRESHOLDS};
pub use preference_learner::{TriggerPreferenceLearner, TriggerStats};

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::brain::json_parser::{StreamEvent, StreamingJsonParser};
use crate::brain::smart_app_classifier::SmartAppClassifier;
use crate::config::manager::ProactiveConfig;
use crate::dialogue::DialogueManager;
use crate::error::{VivianError, VivianResult};
use crate::memory::types::MemoryType;
use crate::memory::MemoryManager;
use crate::persona::PersonaEngine;
use crate::pipeline::steps::generation::{new_shared_stream_emitter, SharedStreamEmitter};
use crate::pipeline::steps::prompt::PromptBuildingStep;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::psychology::{BehaviorDrive, DriveLabel, EmotionLabel, PsychologyManager};
use crate::psychology::mood::MoodSnapshot;
use crate::types::response::ChatMessage;
use crate::world::events::WorldEvent;
use crate::utils::path::get_character_data_dir;

// ============ 随机辅助（无 rand 依赖） ============

/// 将 EmotionLabel 映射为中文（供内心独白提示词使用）
fn emotion_label_zh(label: EmotionLabel) -> &'static str {
    match label {
        EmotionLabel::Joy => "快乐",
        EmotionLabel::Sadness => "悲伤",
        EmotionLabel::Anger => "愤怒",
        EmotionLabel::Fear => "恐惧",
        EmotionLabel::Closeness => "亲近",
        EmotionLabel::Loneliness => "孤独",
        EmotionLabel::Curiosity => "好奇",
    }
}

/// 用户交互后主动问候静默窗口（秒）
///
/// 用户在 5 分钟内有过任何对话（InputDialog 直聊 / 微信窗口私聊或群聊），
/// 则不触发该角色的问候类主动交互（HourlyGreeting / IdleGreeting / Icebreaker / WelcomeBack）。
/// 独立于 min_trigger_interval（后者兼做全局冷却下限，不宜随意调大）。
const GREETING_SUPPRESSION_AFTER_INTERACTION_SECS: f64 = 300.0;

static RNG_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成 [0, 1) 区间伪随机浮点数
pub(crate) fn random_f64() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = RNG_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut x = nanos.wrapping_add(c.wrapping_mul(0x9E3779B97F4A7C15));
    // xorshift64
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x as f64) / (u64::MAX as f64)
}

/// 生成 [0, len) 区间伪随机索引
pub(crate) fn random_index(len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (random_f64() * len as f64) as usize % len
    }
}

/// 按概率决定是否触发（与 TriggerThrottle::roll_probability 同源，但接受动态概率）
pub(crate) fn roll_with_probability(probability: f64) -> bool {
    random_f64() < probability.clamp(0.0, 1.0)
}

/// 判断当前小时是否在 [start, end) 窗口内（支持跨午夜，如 start=23 end=6）
///
/// - start == end 时返回 false（空窗口）
/// - start < end（不跨午夜）：hour ∈ [start, end)
/// - start > end（跨午夜）：hour >= start || hour < end
pub fn hour_in_window(hour: u32, start: u32, end: u32) -> bool {
    if start == end {
        return false;
    }
    if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

/// 根据用户空闲时间计算推荐的 tick 间隔（毫秒）
///
/// 策略 A：乘以角色专属的随机抖动因子（TickJitterConfig），
/// 让两个角色的 tick 相位在几轮后自然漂移、不再收敛。
pub fn compute_adaptive_tick_ms(idle_seconds: f64, char_id: &str) -> u64 {
    let base = if idle_seconds < 300.0 {
        10_000.0
    } else if idle_seconds < 900.0 {
        30_000.0
    } else if idle_seconds < 3600.0 {
        120_000.0
    } else {
        300_000.0
    };
    let jitter_cfg = crate::character_behavior::get_behavior(char_id).tick_jitter;
    let jitter = jitter_cfg.min + random_f64() * (jitter_cfg.max - jitter_cfg.min);
    (base * jitter) as u64
}

// ============ 持久化状态 ============

/// 持久化的主动对话状态
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProactiveState {
    /// 心理状态（字符串形式，便于序列化）
    pub mind_state: String,
    /// 上次互动时间戳（Unix 秒）
    pub last_interaction_time: f64,
    /// 各触发类型上次触发时间
    #[serde(default)]
    pub last_trigger_times: HashMap<String, f64>,
    /// 上次整点问候的小时
    #[serde(default)]
    pub last_hour_greeted: i32,
    /// 各健康提醒类型上次触发时间
    #[serde(default)]
    pub last_reminder_times: HashMap<String, f64>,
    /// 持续活跃分钟数
    #[serde(default)]
    pub sustained_active_minutes: u32,
    /// 上次活动检查时间戳
    #[serde(default)]
    pub last_activity_check: f64,
    /// 安静模式开关
    #[serde(default)]
    pub quiet_mode: bool,
    /// 安静模式结束时间戳
    #[serde(default)]
    pub quiet_mode_until: f64,
    /// 被忽略次数
    #[serde(default)]
    pub ignored_count: u32,
    /// 上次特殊日期问候（MM-DD）
    #[serde(default)]
    pub last_special_date: String,
}

// ============ 主动交互编排器 ============

/// 主动交互编排器
pub struct ProactiveOrchestrator {
    state: Arc<RwLock<ProactiveState>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    persistence_path: std::path::PathBuf,
    /// 上次 tick 时间
    last_tick: Arc<RwLock<Instant>>,
    /// 待触发的主动行为（前端轮询消费）
    pending_messages: Arc<RwLock<Vec<ProactiveAction>>>,
    /// 近期已发送 (content, channel) 二元组（保留最近 5 条，防止跨 tick 内容重复）
    recent_sent_contents: RwLock<Vec<(String, DeliveryChannel)>>,
    /// 话题冷却池
    topic_pool: TopicPool,
    /// 习惯追踪器
    habit_tracker: HabitTracker,
    /// 行为模式管理器
    behavior_mode: BehaviorModeManager,
    /// 压力监测器
    stress_monitor: RwLock<StressMonitor>,
    /// 最近记忆文本（供破冰/回忆参考）
    recent_memory: RwLock<String>,
    /// 当前应用分类（活动冷却用）
    last_app_category: RwLock<String>,
    /// 用户是否活跃（轮询缓存）
    last_user_active: RwLock<bool>,
    /// 用户是否曾离开（回归欢迎用）
    last_user_was_away: RwLock<bool>,
    /// LLM 路由器（可选，注入后 BehaviorDecider / IcebreakerGenerator / MemoryRecall
    /// 优先调用 LLM 生成；不可用时回退到启发式模板池）
    model_router: RwLock<Option<Arc<ModelRouter>>>,
    /// 心理系统管理器（可选，注入后启用 Behavior Drive 混合模式 + Homeostasis tick）
    psychology: RwLock<Option<Arc<PsychologyManager>>>,
    /// 主动对话运行时配置（由设置面板注入，立即生效）
    config: RwLock<ProactiveConfig>,
    /// 人格引擎（可选，注入后 LLM prompt 使用真实人设风格约束，而非硬编码）
    persona: RwLock<Option<Arc<PersonaEngine>>>,
    /// 对话管理器（可选，注入后 LLM prompt 携带最近对话历史，避免重复）
    dialogue: RwLock<Option<Arc<DialogueManager>>>,
    /// 记忆管理器（可选，注入后 MemoryRecall 可读取未闭环 open_hooks 主动追问）
    memory: RwLock<Option<Arc<MemoryManager>>>,
    /// 世界状态提供者（可选，注入后启用世界事件检测 + 内心独白）
    world_provider: RwLock<Option<Arc<crate::world::WorldStateProvider>>>,
    /// 认知心智（可选，注入后内心独白生成完成时触发 thought_refresh）
    mind: RwLock<Option<Arc<crate::mind::Mind>>>,
    /// 世界事件检测器（比较前后 WorldSnapshot 产出事件）
    event_detector: RwLock<crate::world::WorldEventDetector>,
    /// 自主思绪触发评估器（事件驱动内心独白）
    thought_trigger_evaluator: RwLock<thought_trigger::ThoughtTriggerEvaluator>,
    /// 当前 tick 检测到的世界事件（供思绪评估器使用）
    detected_world_events: RwLock<Vec<WorldEvent>>,
    /// 用户活动日志（后台线程记录前台窗口切换，内心独白生成时 drain 消费）
    activity_journal: Arc<ActivityJournal>,
    /// 智能应用分类器（9 分类 + 缓存 + LLM callback，替代 habits.rs 的 3 分类 classify_app）
    app_classifier: SmartAppClassifier,
    /// 流式推送回调（主动对话生成期间推送 text 增量到前端）
    stream_emitter: SharedStreamEmitter,
    /// 触发偏好学习器（EWMA 追踪每种触发器的用户响应率，动态调整概率门控）
    preference_learner: TriggerPreferenceLearner,
    /// 角色 ID（用于按角色差异化行为参数 + 持久化路径隔离）
    char_id: String,
    /// 在线室友快照（每次 tick 前由命令层刷新，供 CrossCharacterReply 触发器决策与 prompt 注入）
    /// 只有一个室友，用 Option 而非 Vec
    companions_snapshot: Arc<RwLock<Option<OnlineCompanion>>>,
    /// 策略 C：说话欲望累积器（0.0~1.0+），每 tick 按性格参数增长，成功说话后归零。
    /// 低于角色专属阈值时跳过 greeting 类触发，让两个角色的"想说话"峰值出现在不同时刻。
    speech_desire: RwLock<f64>,
    /// 策略 E：情绪浮动相位（弧度），每 tick 按角色专属 recovery_rate 推进。
    /// 用于计算周期性情绪乘数，让两个角色的冷却曲线形状不同（锯齿 vs 缓坡）。
    mood_drift_phase: RwLock<f64>,
    /// 外部事件信号：准备去休息（含原因），消费后清空
    signal_going_to_rest: Arc<parking_lot::Mutex<Option<String>>>,
    /// 外部事件信号：刚醒来，消费后重置
    signal_waking_up: Arc<AtomicBool>,
    /// 外部事件信号：Busy 知识采集完成（携带采集到的主题列表）
    /// 消费后清空，由 thought_lifecycle 播种 want_to_share_knowledge 种子
    signal_knowledge_acquired: Arc<parking_lot::Mutex<Vec<String>>>,
    /// 外部事件信号：被室友 cue（from_name, topic_brief, timestamp）
    /// 三人共处一室语义：室友和用户聊天时低概率 cue 本角色，提升 BystanderInterjection 触发概率
    /// 30s 内有效，过期自动失效（由 compute_bystander_interjection_probability 检查时间戳）
    roommate_cue: Arc<parking_lot::Mutex<Option<(String, String, f64)>>>,
    /// 思绪生命周期管理器：思绪种子→滋长→独白/表达→消退
    thought_lifecycle: Arc<RwLock<ThoughtLifecycle>>,
    /// Prompt 构建步骤（可选，注入后主动问候复用主对话完整 prompt：
    /// 完整人设/历史/记忆检索/知识库/环境/用户画像等）
    prompt_step: RwLock<Option<PromptBuildingStep>>,
    /// 工具系统（可选，注入后主动问候 prompt 注入最近真实工具调用历史，
    /// 让 AI 只能提及真实做过的操作，禁止编造）
    tool_system: RwLock<Option<Arc<crate::tools::ToolSystem>>>,
    /// 上次 Busy 知识采集完成时间戳（秒），用于采集任务级冷却（避免每次 Busy 都采集）
    last_knowledge_acquisition_ts: Arc<parking_lot::Mutex<f64>>,
    /// 上次知识分享表达时间戳（秒），用于分享冷却（避免频繁推送链接/分享消息）
    last_knowledge_share_ts: Arc<parking_lot::Mutex<f64>>,
}

/// 主动行为投递渠道
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryChannel {
    /// 桌宠气泡（原有路径，写入 dialogue 标记 channel="proactive"）
    Bubble,
    /// 聊天窗口（"微信"渠道标签，写入 dialogue 标记 channel="wechat"）
    ChatWindow,
    // 阶段4扩展：SystemNotify / DesktopAction
}

impl Default for DeliveryChannel {
    fn default() -> Self {
        Self::Bubble
    }
}

/// 主动行为内容类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    /// 问候/陪伴/情绪（默认）
    Greeting,
    /// 分享有价值内容（强制 value_score 门槛）
    Share,
    /// 提醒
    Reminder,
    /// 信息投递
    Info,
}

impl Default for ContentType {
    fn default() -> Self {
        Self::Greeting
    }
}

/// Share 类发送阈值（与 PROACTIVE_SHARE_THRESHOLD 对齐）
pub const SHARE_VALUE_THRESHOLD: f32 = 0.70;

/// 主动消息有效期（秒）：超过此时间的待发送消息视为过时并丢弃
const PROACTIVE_MSG_TTL_SECS: f64 = 300.0;

/// 待发送的主动行为
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveAction {
    pub trigger: String,
    pub content: String,
    pub timestamp: f64,
    pub priority: u32,
    /// 投递渠道（默认 Bubble）
    #[serde(default)]
    pub delivery_channel: DeliveryChannel,
    /// 内容类型（默认 Greeting）
    #[serde(default)]
    pub content_type: ContentType,
    /// 重要性 0.0-1.0（默认 0.5）
    #[serde(default = "default_importance")]
    pub importance: f32,
    /// 价值评分 0.0-1.0（仅 Share 类强制，其他类型可选）
    #[serde(default)]
    pub value_score: Option<f32>,
}

fn default_importance() -> f32 {
    0.5
}

impl ProactiveAction {
    /// 从 trigger + content 构造默认 action（Bubble/Greeting/无 value_score）
    pub fn from_trigger(trigger: ProactiveTrigger, content: String, now: f64) -> Self {
        Self {
            trigger: trigger.as_str().to_string(),
            content,
            timestamp: now,
            priority: trigger.priority(),
            delivery_channel: DeliveryChannel::Bubble,
            content_type: ContentType::Greeting,
            importance: 0.5,
            value_score: None,
        }
    }
}

/// 室友快照项（命令层每次 tick 前刷新，只有一个室友）
#[derive(Debug, Clone, Default)]
pub struct OnlineCompanion {
    /// 室友角色 ID（如 "nana" / "vivian"）
    pub id: String,
    /// 室友显示名（如 "Nana"）
    pub name: String,
    /// 距上次主动发言的秒数（None = 本次会话未发言过）
    pub last_spoke_secs_ago: Option<f64>,
    /// 室友最近一次主动发言的文本（None = 未发言或不可用）
    pub last_spoke_text: Option<String>,
}

impl ProactiveOrchestrator {
    pub fn new(char_id: &str) -> VivianResult<Self> {
        let proactive_dir = get_character_data_dir(char_id).join("proactive");
        std::fs::create_dir_all(&proactive_dir)
            .map_err(|e| VivianError::Memory(format!("创建主动对话目录失败: {e}")))?;

        let persistence_path = proactive_dir.join("state.json");
        let now_ts = chrono::Local::now().timestamp() as f64;
        let state = if persistence_path.exists() {
            Self::load_from(&persistence_path)
        } else {
            ProactiveState {
                mind_state: PetMindState::Curious.as_str().to_string(),
                last_interaction_time: now_ts,
                last_activity_check: now_ts,
                ..Default::default()
            }
        };

        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            persistence_path,
            last_tick: Arc::new(RwLock::new(Instant::now())),
            pending_messages: Arc::new(RwLock::new(Vec::new())),
            recent_sent_contents: RwLock::new(Vec::new()),
            topic_pool: TopicPool::new()?,
            habit_tracker: HabitTracker::new()?,
            behavior_mode: BehaviorModeManager::new(now_ts),
            stress_monitor: RwLock::new(StressMonitor::new()),
            recent_memory: RwLock::new(String::new()),
            last_app_category: RwLock::new(String::new()),
            last_user_active: RwLock::new(true),
            last_user_was_away: RwLock::new(false),
            model_router: RwLock::new(None),
            psychology: RwLock::new(None),
            config: RwLock::new(ProactiveConfig::default()),
            persona: RwLock::new(None),
            dialogue: RwLock::new(None),
            memory: RwLock::new(None),
            world_provider: RwLock::new(None),
            mind: RwLock::new(None),
            event_detector: RwLock::new(crate::world::WorldEventDetector::new()),
            thought_trigger_evaluator: RwLock::new(thought_trigger::ThoughtTriggerEvaluator::new()),
            detected_world_events: RwLock::new(Vec::new()),
            activity_journal: Arc::new(ActivityJournal::new()),
            app_classifier: SmartAppClassifier::new(),
            stream_emitter: new_shared_stream_emitter(),
            preference_learner: TriggerPreferenceLearner::new(),
            char_id: char_id.to_string(),
            companions_snapshot: Arc::new(RwLock::new(None)),
            speech_desire: RwLock::new(
                crate::character_behavior::get_behavior(char_id).speech_desire.initial_desire,
            ),
            mood_drift_phase: RwLock::new(
                crate::character_behavior::get_behavior(char_id).mood_drift.initial_phase,
            ),
            signal_going_to_rest: Arc::new(parking_lot::Mutex::new(None)),
            signal_waking_up: Arc::new(AtomicBool::new(false)),
            signal_knowledge_acquired: Arc::new(parking_lot::Mutex::new(Vec::new())),
            roommate_cue: Arc::new(parking_lot::Mutex::new(None)),
            thought_lifecycle: Arc::new(RwLock::new(ThoughtLifecycle::new())),
            prompt_step: RwLock::new(None),
            tool_system: RwLock::new(None),
            last_knowledge_acquisition_ts: Arc::new(parking_lot::Mutex::new(0.0)),
            last_knowledge_share_ts: Arc::new(parking_lot::Mutex::new(0.0)),
        })
    }

    fn load_from(path: &std::path::Path) -> ProactiveState {
        match std::fs::read_to_string(path) {
            Ok(content) if !content.trim().is_empty() => {
                serde_json::from_str::<ProactiveState>(&content).unwrap_or_default()
            }
            _ => ProactiveState::default(),
        }
    }

    fn save_to(&self) -> VivianResult<()> {
        let state = self.state.read().clone();
        let json = serde_json::to_string_pretty(&state)
            .map_err(|e| VivianError::Memory(format!("序列化主动对话状态失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| VivianError::Memory(format!("写入主动对话临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换主动对话文件失败: {e}")))?;
        Ok(())
    }

    pub fn start(&self) {
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("主动交互已启动");
    }

    pub fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("主动交互已停止");
    }

    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 注入 LLM 路由器（注入后 BehaviorDecider / IcebreakerGenerator / MemoryRecall
    /// 优先调用 LLM 生成；未注入或调用失败时回退到启发式模板池）
    pub fn set_model_router(&self, router: Arc<ModelRouter>) {
        *self.model_router.write() = Some(router);
    }

    /// 注入心理系统管理器（启用 Behavior Drive 混合模式 + Homeostasis tick）
    ///
    /// 注入后：
    /// - 每次 tick 调用 `homeostasis_tick`，让 Needs/Emotion 自动回归 set point
    /// - `update_mind_state` 优先使用规则推导的 Behavior Drive 主导项决定心理状态
    /// - 对话轮的 LLM drive 由 `BrainChatChain` 直接应用，这里只处理无对话时的规则路径
    pub fn set_psychology(&self, psychology: Arc<PsychologyManager>) {
        *self.psychology.write() = Some(psychology);
    }

    /// 注入主动对话运行时配置（由设置面板保存后立即调用，无需 reinitialize）
    ///
    /// 影响范围：
    /// - `enabled=false`：tick 直接返回，不产生任何主动消息
    /// - `enable_idle_trigger=false`：跳过 IdleGreeting
    /// - `enable_window_change_trigger=false`：跳过 WindowTrigger
    /// - `enable_away_reminder=false`：跳过 WelcomeBack
    /// - `proactivity`（0.0-1.0）：缩放概率门控
    /// - `min_trigger_interval`：作为所有触发器的 cooldown 下限
    /// - `idle_threshold`：作为 IdleGreeting/Spontaneous 的 min_idle_seconds 下限
    pub fn set_config(&self, config: ProactiveConfig) {
        *self.config.write() = config;
    }

    /// 注入人格引擎（启用后 LLM prompt 使用真实人设风格约束，替代硬编码人设）
    pub fn set_persona(&self, persona: Arc<PersonaEngine>) {
        *self.persona.write() = Some(persona);
    }

    /// 注入对话管理器（启用后 LLM prompt 携带最近对话历史，避免主动消息与刚聊过的内容重复）
    pub fn set_dialogue(&self, dialogue: Arc<DialogueManager>) {
        *self.dialogue.write() = Some(dialogue);
    }

    /// 获取对话管理器（供后台任务读取最近对话历史，锚定搜索兴趣）
    pub fn get_dialogue(&self) -> Option<Arc<DialogueManager>> {
        self.dialogue.read().clone()
    }

    /// 注入记忆管理器（启用后 MemoryRecall 可读取未闭环 open_hooks 主动追问）
    pub fn set_memory(&self, memory: Arc<MemoryManager>) {
        *self.memory.write() = Some(memory);
    }

    /// 注入世界状态提供者（启用后启用世界事件检测 + 内心独白 + 天气感知）
    pub fn set_world_provider(&self, world: Arc<crate::world::WorldStateProvider>) {
        *self.world_provider.write() = Some(world);
    }

    /// 注入认知心智（启用后内心独白生成完成时触发 thought_refresh，
    /// 让下次 cognitive tick 用 LLM 重新合成 current_thought）
    pub fn set_mind(&self, mind: Arc<crate::mind::Mind>) {
        *self.mind.write() = Some(mind);
    }

    /// 注入 Prompt 构建步骤（启用后主动问候复用主对话完整 prompt：
    /// 完整人设/历史/记忆检索/知识库/环境/用户画像等，替代简陋的 build_style_prompt）
    pub fn set_prompt_step(&self, step: PromptBuildingStep) {
        *self.prompt_step.write() = Some(step);
    }

    /// 注入工具系统（启用后主动问候 prompt 注入最近真实工具调用历史，
    /// 让 AI 只能提及真实做过的操作，禁止编造看了番剧/刷了视频等未发生的事）
    pub fn set_tool_system(&self, ts: Arc<crate::tools::ToolSystem>) {
        *self.tool_system.write() = Some(ts);
    }

    /// 获取活动日志记录器（后台线程记录前台窗口切换）
    ///
    /// 调用方应在 world.enable 时调用 `start()` 启动后台线程。
    pub fn activity_journal(&self) -> &Arc<ActivityJournal> {
        &self.activity_journal
    }

    /// 注入流式推送回调（主动对话生成期间推送 text 增量到前端）
    pub fn set_stream_emitter(
        &self,
        emitter: Option<crate::pipeline::steps::generation::StreamEmitter>,
    ) {
        *self.stream_emitter.write() = emitter;
    }

    /// 刷新室友快照（由命令层在每次 tick 前调用）
    ///
    /// 快照供 `CrossCharacterReply` 触发器决策（判断室友是否最近发言过）
    /// 以及 prompt 注入（让 LLM 知道室友在线 + 最近说了什么）。
    pub fn update_companions_snapshot(&self, companion: Option<OnlineCompanion>) {
        *self.companions_snapshot.write() = companion;
    }

    /// 通知编排器角色即将去休息（内心独白触发信号）
    pub fn signal_going_to_rest(&self, reason: &str) {
        *self.signal_going_to_rest.lock() = Some(reason.to_string());
    }

    /// 通知编排器角色刚醒来（内心独白触发信号）
    pub fn signal_waking_up(&self) {
        self.signal_waking_up.store(true, Ordering::Relaxed);
    }

    /// 通知编排器 Busy 知识采集已完成（携带采集到的主题列表）
    ///
    /// 由 spawn_knowledge_acquisition 任务结束时调用。
    /// 下次 tick 会取出信号，向 thought_lifecycle 播种 want_to_share_knowledge 种子，
    /// 强度积累到阈值后由 generate_knowledge_share_message 生成对用户的分享消息。
    pub fn signal_knowledge_acquired(&self, topics: Vec<String>) {
        if topics.is_empty() {
            return;
        }
        // 记录采集完成时间戳，用于采集任务级冷却
        *self.last_knowledge_acquisition_ts.lock() = chrono::Local::now().timestamp() as f64;
        let mut slot = self.signal_knowledge_acquired.lock();
        // 多次 Busy 任务产出的主题合并，下次 tick 统一播种
        slot.extend(topics);
    }

    /// 检查 Busy 知识采集是否在冷却期内（避免每次 Busy 都触发采集）
    ///
    /// 冷却时间 30 分钟——人类不会每隔几分钟就"去找点新东西学"，
    /// 频繁采集既浪费 API 调用也让分享行为显得机械。
    pub fn is_knowledge_acquisition_in_cooldown(&self) -> bool {
        let last = *self.last_knowledge_acquisition_ts.lock();
        if last <= 0.0 {
            return false;
        }
        let now = chrono::Local::now().timestamp() as f64;
        const ACQUISITION_COOLDOWN_SECS: f64 = 30.0 * 60.0;
        (now - last) < ACQUISITION_COOLDOWN_SECS
    }

    /// 检查知识分享是否在冷却期内（避免频繁推送链接/分享消息给用户）
    ///
    /// 冷却时间 30 分钟——同一主题知识分享间隔不低于 30 分钟，避免高频重复推送。
    pub fn is_knowledge_share_in_cooldown(&self) -> bool {
        let last = *self.last_knowledge_share_ts.lock();
        if last <= 0.0 {
            return false;
        }
        let now = chrono::Local::now().timestamp() as f64;
        const SHARE_COOLDOWN_SECS: f64 = 30.0 * 60.0;
        (now - last) < SHARE_COOLDOWN_SECS
    }

    /// 标记知识分享已表达（更新冷却时间戳）
    pub fn mark_knowledge_share_expressed(&self) {
        *self.last_knowledge_share_ts.lock() = chrono::Local::now().timestamp() as f64;
    }

    /// 由其他角色在对话中"cue"本角色——设置一个 30s 内有效的信号，提升 BystanderInterjection 触发概率
    ///
    /// 三人共处一室语义：当 Vivian 和用户聊天时，可以以低概率 cue 一下 Nana，
    /// 让 Nana 通过 BystanderInterjection 路径自然插话加入对话。
    /// 信号 30s 内有效，由 compute_bystander_interjection_probability 检查时间戳。
    pub fn seed_roommate_cue(&self, from_name: &str, topic_brief: &str) {
        let now_ts = chrono::Local::now().timestamp() as f64;
        let mut slot = self.roommate_cue.lock();
        *slot = Some((from_name.to_string(), topic_brief.to_string(), now_ts));
        tracing::info!(
            "[proactive:{}] 收到 roommate_cue 信号（from={}, topic={}）",
            self.char_id,
            from_name,
            topic_brief
        );
    }

    /// 检查是否有 30s 内有效的 roommate_cue 信号
    ///
    /// 返回 Some((from_name, topic_brief)) 表示有效信号存在。
    fn check_roommate_cue(&self) -> Option<(String, String)> {
        let now_ts = chrono::Local::now().timestamp() as f64;
        let slot = self.roommate_cue.lock();
        match slot.as_ref() {
            Some((from_name, topic_brief, ts)) if now_ts - ts <= 30.0 => {
                Some((from_name.clone(), topic_brief.clone()))
            }
            _ => None,
        }
    }

    /// 读取室友快照（用于触发器判断 + prompt 构造）
    pub fn companions_snapshot(&self) -> Option<OnlineCompanion> {
        self.companions_snapshot.read().clone()
    }

    /// 计算跨角色回应的动态概率（基于心情状态 + 用户在场状态 + A↔B 关系）
    ///
    /// 返回区间 [0.05, 0.75]。
    /// - 基线 0.08
    /// - loneliness 0.0→+0.00, 1.0→+0.30（主驱动）
    /// - sadness 0.0→+0.00, 1.0→+0.10（次要驱动）
    /// - joy 0.0→-0.00, 1.0→-0.05（抑制：自己很开心不需要找人）
    /// - 用户不在屏幕前时 +0.15（角色无事可做，更可能找室友聊天）
    /// - A↔B intimacy 调节：关系近更想找对方，关系远不太主动（±0.10）
    /// - 近期互动频率：1h 内刚聊过则 -0.10（防刷屏）
    fn compute_cross_reply_probability(&self, user_present: bool) -> f64 {
        let (loneliness, sadness, joy) = {
            let psy = self.psychology.read();
            match psy.as_ref() {
                Some(p) => {
                    let e = p.emotion();
                    (e.loneliness, e.sadness, e.joy)
                }
                None => (0.0, 0.0, 0.0),
            }
        };
        let mut prob = 0.08 + loneliness * 0.30 + sadness * 0.10 - joy * 0.05;
        // 用户不在屏幕前时，角色更可能找室友聊天（无事可做 + 略感孤单）
        if !user_present {
            prob += 0.15;
        }
        // 三人共处一室语义：用户和某角色聊天时，另一角色仍可旁听+插话
        // 用时间衰减替代原 5min 硬屏蔽：
        // - < 2min：用户真正在打字，几乎不打断（×0.0）
        // - 2-5min：用户可能停顿，低概率接话（×0.4）
        // - 5-15min：正常概率
        // - >15min：用户实际离开，概率提升（+0.15）
        let now_ts = chrono::Local::now().timestamp() as f64;
        let secs_since_interaction = now_ts - self.state.read().last_interaction_time;
        if secs_since_interaction < 120.0 {
            prob *= 0.0;
        } else if secs_since_interaction < 300.0 {
            prob *= 0.4;
        } else if secs_since_interaction > 900.0 {
            prob += 0.15;
        }
        // 关系状态差异化：A↔B intimacy 调节发起意愿
        // 关系近更想找对方聊，关系远不太主动
        if let Some(companion) = self.companions_snapshot() {
            let rel = crate::psychology::social_state::social_state()
                .get_pair(&self.char_id, &companion.id);
            // intimacy 调节：0.0→-0.10, 0.5→0.0, 1.0→+0.10
            prob += (rel.intimacy - 0.5) * 0.20;
            // 近期互动频率：1h 内刚聊过则降低意愿（防刷屏）
            if rel.last_interaction_time > 0.0 && now_ts - rel.last_interaction_time < 3600.0 {
                prob -= 0.10;
            }
        }
        prob.clamp(0.05, 0.75)
    }

    /// 计算旁观插话的基础概率（情绪驱动，不含时机衰减）
    ///
    /// 驱动因子：
    /// - curiosity 越高越想插话（+0.20）：好奇旁听到的内容
    /// - loneliness 越高越想参与（+0.10）：想加入对话不被冷落
    /// - closeness 越高越敢插嘴（+0.05）：关系近更放得开
    /// - joy 越高越不需要插话（-0.05）：自己开心不需要凑热闹
    /// - 用户活跃聊天时 +0.10：三人共处一室，本来就有旁听素材，插话更自然
    /// - 被室友 cue 时 +0.35：室友主动叫你加入，插话意愿强烈（30s 内有效）
    fn compute_bystander_interjection_probability(&self, user_actively_chatting: bool) -> f64 {
        let (curiosity, loneliness, closeness, joy) = {
            let psy = self.psychology.read();
            match psy.as_ref() {
                Some(p) => {
                    let e = p.emotion();
                    (e.curiosity, e.loneliness, e.closeness, e.joy)
                }
                None => (0.0, 0.0, 0.0, 0.0),
            }
        };
        let mut prob = 0.10 + curiosity * 0.20 + loneliness * 0.10 + closeness * 0.05 - joy * 0.05;
        // 三人共处一室语义：用户和室友正在聊天时，旁听素材最丰富，插话更自然
        if user_actively_chatting {
            prob += 0.10;
        }
        // 被室友 cue：室友主动叫你加入对话，插话意愿强烈
        if self.check_roommate_cue().is_some() {
            prob += 0.35;
        }
        prob.clamp(0.05, 0.85)
    }

    /// 把室友快照渲染为 prompt 友好的文本块
    ///
    /// 格式示例：
    /// ```text
    /// - Nana (id: nana) — last spoke 32s ago: "刚才用户在写代码呢"
    /// ```
    /// 没有在线室友时返回空字符串。
    fn format_companions_for_prompt(&self) -> String {
        let companion = self.companions_snapshot.read();
        match companion.as_ref() {
            None => String::new(),
            Some(c) => {
                let spoke = match (c.last_spoke_secs_ago, c.last_spoke_text.as_deref()) {
                    (Some(secs), Some(text)) if !text.is_empty() => {
                        let t: String = text.chars().take(40).collect();
                        format!(" — said to user {:.0}s ago: \"{}\"", secs, t)
                    }
                    (Some(secs), _) => format!(" — spoke {:.0}s ago", secs),
                    _ => String::new(),
                };
                format!("- {} (id: {}){}", c.name, c.id, spoke)
            }
        }
    }

    /// 设置最近记忆文本（供破冰/回忆参考）
    pub fn set_recent_memory(&self, text: &str) {
        let truncated: String = text.chars().take(300).collect();
        *self.recent_memory.write() = truncated;
    }

    /// 心理状态
    pub fn get_mind_state(&self) -> PetMindState {
        let state = self.state.read();
        PetMindState::from_str(&state.mind_state)
    }

    /// 单次 tick：由调用方每 10 秒触发一次
    pub fn tick(&self, context: &TickContext) -> VivianResult<bool> {
        if !self.is_running() {
            return Ok(false);
        }

        // 配置开关：enabled=false 时完全不产生主动消息
        if !self.config.read().enabled {
            return Ok(false);
        }

        // 0. 心理系统 Homeostasis tick（让 Needs/Emotion 自动回归 set point）
        if let Some(psy) = self.psychology.read().as_ref() {
            psy.homeostasis_tick();
        }

        // 0.5. 策略 C：说话欲望累积
        // 每 tick 按性格参数增长，被忽略时加速（Vivian 越不理越想说话），
        // 用户忙碌时 Nana 主动退让（衰减）。成功说话后由 push_message 归零。
        {
            let behavior = crate::character_behavior::get_behavior(&self.char_id);
            let sd_cfg = behavior.speech_desire;
            let mut desire = self.speech_desire.write();
            let ignored = self.state.read().ignored_count;
            let mut delta = sd_cfg.base_growth;
            if ignored > 0 {
                delta += sd_cfg.ignored_boost * (ignored as f64).min(3.0);
            }
            if context.idle_seconds < 60.0 {
                delta -= sd_cfg.user_busy_decay;
            }
            *desire = (*desire + delta).clamp(0.0, 1.5);
        }

        // 0.6. 策略 E：情绪浮动相位推进
        // 每 tick 按角色专属 recovery_rate 推进相位，
        // compute_overall_cooling 中用 sin(phase) × volatility 产生周期性情绪乘数。
        {
            let behavior = crate::character_behavior::get_behavior(&self.char_id);
            let mut phase = self.mood_drift_phase.write();
            *phase += behavior.mood_drift.recovery_rate;
            if *phase > std::f64::consts::TAU {
                *phase -= std::f64::consts::TAU;
            }
        }

        let now = context.now;
        let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
        let minute = chrono::Local::now().format("%M").to_string().parse::<u32>().unwrap_or(0);

        // 1. 轮询窗口：更新应用分类 + 用户活动
        self.poll_window(context);

        // 2. 更新持续活动时长
        self.update_sustained_activity(context);

        // 3. 更新行为模式（影随/守护/陪伴）
        {
            let app_cat = self.last_app_category.read().clone();
            let user_active = *self.last_user_active.read();
            self.behavior_mode.update(&app_cat, user_active, now, hour);
        }

        // 4. 安静模式检查
        {
            let mut state = self.state.write();
            if state.quiet_mode && now > state.quiet_mode_until {
                state.quiet_mode = false;
                state.ignored_count = 0;
                tracing::info!("安静模式结束，恢复主动交互");
            }
            if state.quiet_mode {
                return Ok(false);
            }
        }

        // 5. 计算 BehaviorDrive（一次计算，复用于心理状态更新和能力规划）
        //    psychology 未注入时为 None，由 CapabilityPlanner 回退到静态优先级
        let current_drive = self.current_drive();

        // 5.5. 心理状态更新（消费预计算的 drive，避免重复推导）
        self.update_mind_state(context, hour, current_drive.as_ref());

        // 6. 压力监测：记录当前情绪
        if !context.user_emotion.is_empty() {
            let sustained = self.state.read().sustained_active_minutes;
            self.stress_monitor
                .write()
                .assess_with_workload(sustained, &context.user_emotion);
        }

        // 6.5. 世界事件检测 → Appraisal → 心理状态更新（不打扰用户）
        //      比较前后 WorldSnapshot 产出事件（天气变化/节日到来/日出日落等），
        //      通过 Appraisal 机制隐式影响情绪/需求，不产生主动消息。
        self.detect_and_apply_world_events(context);

        // 6.6. 用户离设备门控
        //      idle_seconds 已由命令层用系统级 GetLastInputInfo 覆盖（跨应用权威）。
        //      超过 away_threshold_seconds 视为用户不在设备前：跳过所有主动消息触发，
        //      避免对着空座位说话。Vivian 的内心独白不受影响（不打扰用户）。
        //      用户回归后由 WelcomeBack 触发器自然接续。
        let user_away = context.idle_seconds > self.config.read().away_threshold_seconds as f64;
        if user_away {
            // 内心独白仍可运行：让 Vivian 的内心生活继续，但不产生气泡
            let _ = self.maybe_spawn_inner_monologue(context);
            *self.last_tick.write() = Instant::now();
            return Ok(false);
        }

        // 6.7. 能力规划：Drive → Capability 决策点
        //      根据当前主导驱动决定「是否行动」「按什么顺序尝试 Capability」。
        //      skip_action=true 时跳过主动消息触发（Observe/Rest/Avoid 主导），
        //      但内心独白仍由后续逻辑调度。
        let plan = CapabilityPlanner::plan(current_drive.as_ref());

        let mut produced = false;

        // 7. 特殊日期问候（最高优先级，免门控）
        //    SelfState 防打扰（lay_low=true）时跳过：用户已要求安静 / 今日主动次数达上限 /
        //    被忽略接近阈值 / Rest/Offline 在场状态，不应再产生任何用户可见消息。
        //    非 leader 时跳过：发言权由 leader 独占，避免多角色同时问候。
        if !context.lay_low && context.is_speaking_leader && self.try_special_date_greeting(now) {
            produced = true;
        }

        // 8. 触发器评估与消息生成（最多 2 条/tick）
        //    非 leader 时跳过面向用户的主动消息触发器，但保留 CrossCharacterReply：
        //    跨角色回复通过 CrossCharacterBus 发给室友（不发给用户），不与 leader 的用户消息冲突。
        //    若禁止非 leader 触发 CrossCharacterReply，会导致两个角色互相等待对方主动发言的死锁——
        //    leader 等室友发言（但室友不是 leader 无法主动），室友等成为 leader（但 leader 心跳未超时）。
        if context.is_speaking_leader {
            produced |= self.evaluate_and_fire_triggers(context, &plan, produced, hour, minute, now);
        } else {
            produced |= self.evaluate_cross_character_reply_only(context, &plan, produced, hour, minute, now);
        }

        if !context.lay_low && plan.skip_action {
            tracing::debug!(
                "[capability_planner] skip_action this tick (drive={:?}, strength={:.2}, rationale={})",
                plan.drive_label,
                plan.drive_strength,
                plan.rationale
            );
        }

        // 持久化
        if produced {
            let _ = self.save_to();
        }

        // 9. 思绪生命周期调度：播种→tick→独白/主动表达
        //    如果有思绪积累到"忍不住想说话"的程度，桥接到主动消息。
        //    非 leader 时仅运行内心独白部分，不桥接为主动消息。
        let thought_share = if !produced {
            self.maybe_spawn_inner_monologue(context)
        } else {
            None
        };

        if let Some((thought_key, context_hint, trigger_kind)) = thought_share {
            if trigger_kind == "want_to_share_with_roommate"
                || trigger_kind == "cross_character_spoke"
            {
                // 对室友说：不要求 leader 身份，非 leader 也可以主动找室友聊
                // 走 cross_character_reply 投递路径，由命令层 deliver_cross_character_messages 投递
                // cross_character_spoke 种子（室友刚说话）积累到阈值后也走此路径：
                // 思绪来源是室友，表达对象自然也应是室友，而不是用户
                if !context.lay_low {
                    if let Some(companion) = self.companions_snapshot() {
                        if let Some(content) = self.generate_thought_share_to_roommate(
                            context,
                            &thought_key,
                            &context_hint,
                            &companion.name,
                        ) {
                            self.push_message(
                                ProactiveTrigger::CrossCharacterReply,
                                content,
                                context.now,
                            );
                            self.thought_lifecycle.write().mark_expressed(&thought_key, context.now);
                            produced = true;
                            tracing::info!("[thought_lifecycle] 思绪{}升级为对室友分享", thought_key);
                        }
                    }
                }
            } else if trigger_kind == "want_to_share_knowledge" {
                // 对用户分享刚学到的知识：要求 leader 身份 + 用户在场
                // 走 ChatWindow 渠道 + Share 内容类型，由命令层识别 share 路径派发到 wechat 渠道
                // 分享冷却：30 分钟内已分享过则跳过，避免频繁推送链接给用户
                if !context.lay_low && context.is_speaking_leader && context.user_present
                    && !self.is_knowledge_share_in_cooldown()
                {
                    if let Some((content, value_score)) = self.generate_knowledge_share_message(
                        context,
                        &thought_key,
                        &context_hint,
                    ) {
                        let mut action = ProactiveAction::from_trigger(
                            ProactiveTrigger::Spontaneous,
                            content,
                            context.now,
                        );
                        action.delivery_channel = DeliveryChannel::ChatWindow;
                        action.content_type = ContentType::Share;
                        action.value_score = Some(value_score);
                        self.push_action(action, ProactiveTrigger::Spontaneous);
                        self.thought_lifecycle.write().mark_expressed(&thought_key, context.now);
                        self.mark_knowledge_share_expressed();
                        produced = true;
                        tracing::info!(
                            "[thought_lifecycle] 思绪{}升级为知识分享（chat_window/share, score={:.2}）",
                            thought_key,
                            value_score
                        );
                    }
                } else if self.is_knowledge_share_in_cooldown() {
                    // 分享冷却中：推迟表达，不标记 expressed，等冷却结束后再次尝试
                    tracing::debug!(
                        "[thought_lifecycle] 知识分享冷却中，思绪{}推迟表达",
                        thought_key
                    );
                }
            } else if !context.lay_low && context.is_speaking_leader && context.user_present {
                if let Some(content) = self.generate_thought_share_message(context, &thought_key, &context_hint) {
                    self.push_message(
                        ProactiveTrigger::Spontaneous,
                        content,
                        context.now,
                    );
                    self.thought_lifecycle.write().mark_expressed(&thought_key, context.now);
                    produced = true;
                    tracing::info!("[thought_lifecycle] 思绪{}升级为主动分享", thought_key);
                }
            }
        }

        *self.last_tick.write() = Instant::now();
        Ok(produced)
    }

    /// 计算当前 BehaviorDrive（一次 tick 内复用，避免 update_mind_state 与 planner 各算一次）
    ///
    /// psychology 未注入时返回 None。
    fn current_drive(&self) -> Option<BehaviorDrive> {
        self.psychology
            .read()
            .as_ref()
            .map(|psy| psy.compute_rule_drive())
    }

    /// 评估触发条件并生成主动消息（从 tick() 提取的子步骤）
    ///
    /// - `already_produced`: 特殊日期问候已产出时跳过触发器评估
    /// - 单次 tick 最多产生 `MAX_TICK_MESSAGES`(2) 条消息，防止刷屏
    /// - 返回本步骤是否产出了新消息
    fn evaluate_and_fire_triggers(
        &self,
        context: &TickContext,
        plan: &CapabilityPlan,
        already_produced: bool,
        hour: u32,
        minute: u32,
        now: f64,
    ) -> bool {
        const MAX_TICK_MESSAGES: u32 = 2;

        // lay_low=true 或 skip_action=true 或已产出 → 跳过
        if context.lay_low || plan.skip_action || already_produced {
            return false;
        }

        // 用户不在屏幕前（idle >= 300s）时：
        // - 跳过所有面向用户的主动消息触发器（IdleGreeting/Spontaneous/WelcomeBack/Icebreaker 等）
        //   无人回应只会消耗 token + 累加 ignored_count
        // - 仅保留 CrossCharacterReply（用户不在时角色更可能找室友聊天，概率已动态提升）
        // 用户正在聊天时：
        // - CrossCharacterReply 不再硬屏蔽，由 compute_cross_reply_probability 的时间衰减控制
        //   （< 2min ×0, 2-5min ×0.4, 5-15min 正常, >15min +0.15）
        //   三人共处一室语义：室友对用户说话时，本角色可以低概率接话
        let user_away = !context.user_present;

        let mut tick_msg_count: u32 = 0;
        let triggers = self.ordered_active_triggers(plan);
        for trigger in triggers {
            if tick_msg_count >= MAX_TICK_MESSAGES {
                break;
            }
            // 用户不在时仅放行 CrossCharacterReply
            if user_away && !matches!(trigger, ProactiveTrigger::CrossCharacterReply) {
                continue;
            }
            if self.check_trigger(trigger, context, hour, minute) {
                if let Some(behavior) = self.generate_content(trigger, context, hour, minute) {
                    let action = behavior.into_action(trigger, now);
                    self.push_action(action, trigger);
                    self.update_trigger_time(trigger, now, hour, minute);
                    tick_msg_count += 1;
                }
            }
        }
        tick_msg_count > 0
    }

    /// 非 leader 专用：仅评估 CrossCharacterReply 触发器
    ///
    /// 跨角色回复通过 CrossCharacterBus 发给室友（不发给用户），
    /// 不与 leader 的用户消息冲突，因此不受 leader 选举限制。
    /// 避免"leader 等室友发言，室友等成为 leader"的死锁。
    fn evaluate_cross_character_reply_only(
        &self,
        context: &TickContext,
        plan: &CapabilityPlan,
        already_produced: bool,
        hour: u32,
        minute: u32,
        now: f64,
    ) -> bool {
        if context.lay_low || plan.skip_action || already_produced {
            return false;
        }
        // 用户活跃对话时不再硬屏蔽，由 compute_cross_reply_probability 的时间衰减控制
        // （< 2min ×0, 2-5min ×0.4, 5-15min 正常, >15min +0.15）
        // 三人共处一室语义：室友对用户说话时，本角色可以低概率接话
        if self.check_trigger(ProactiveTrigger::CrossCharacterReply, context, hour, minute) {
            if let Some(behavior) = self.generate_content(ProactiveTrigger::CrossCharacterReply, context, hour, minute) {
                let action = behavior.into_action(ProactiveTrigger::CrossCharacterReply, now);
                self.push_action(action, ProactiveTrigger::CrossCharacterReply);
                self.update_trigger_time(ProactiveTrigger::CrossCharacterReply, now, hour, minute);
                return true;
            }
        }
        false
    }

    /// 将 CapabilityPlan 的有序触发器候选与配置启用的触发器求交集
    ///
    /// - plan.skip_action=true 时返回空 Vec
    /// - plan.ordered_triggers 为空（psychology 未注入或低于阈值）时回退到 active_triggers() 静态优先级
    /// - 否则按 plan 顺序输出配置启用的触发器，再追加未在 plan 中但配置启用的触发器
    fn ordered_active_triggers(&self, plan: &CapabilityPlan) -> Vec<ProactiveTrigger> {
        if plan.skip_action {
            return Vec::new();
        }
        let active = self.active_triggers();
        if plan.ordered_triggers.is_empty() {
            return active;
        }
        let mut result: Vec<ProactiveTrigger> = plan
            .ordered_triggers
            .iter()
            .filter(|t| active.contains(t))
            .copied()
            .collect();
        for t in active {
            if !result.contains(&t) {
                result.push(t);
            }
        }
        result
    }

    /// 世界事件检测 → Appraisal → 心理状态更新（不打扰用户）
    ///
    /// 比较前后 WorldSnapshot 产出事件（天气变化/节日到来/日出日落等），
    /// 通过 PsychologyManager::apply_external_event 隐式影响情绪/需求。
    /// 同时异步刷新天气缓存（TTL 到期时）。
    fn detect_and_apply_world_events(&self, context: &TickContext) {
        let wp = match self.world_provider.read().as_ref() {
            Some(wp) => wp.clone(),
            None => return,
        };
        let world_cfg = wp.config();
        if !world_cfg.enable {
            return;
        }

        // 异步刷新天气缓存（TTL 到期时由 WorldStateProvider 内部判断）
        {
            let wp_clone = wp.clone();
            tauri::async_runtime::spawn(async move {
                wp_clone.refresh_weather().await;
            });
        }

        // 异步刷新音乐缓存（读取系统当前播放，每次 tick 都刷新）
        {
            let wp_clone = wp.clone();
            tauri::async_runtime::spawn(async move {
                wp_clone.refresh_music().await;
            });
        }

        // 产出世界快照并检测事件
        let snap = wp.snapshot(Some(context.away_seconds));
        let events = self.event_detector.write().detect(&snap);

        // 存储检测到的世界事件供思绪评估器使用
        *self.detected_world_events.write() = events.clone();

        // 长时间缺席事件（世界继续转动）
        if let Some(secs) = snap.seconds_since_last_interaction {
            if let Some(ev) = crate::world::events::long_absence_event(secs) {
                if let Some(psy) = self.psychology.read().as_ref() {
                    psy.apply_external_event(&ev);
                }
            }
        }

        for event in &events {
            if let Some(psy) = self.psychology.read().as_ref() {
                psy.apply_external_event(event);
            }
        }
    }

    /// 内心独白调度 —— 基于思绪生命周期（Thought Intensity 模型）
    ///
    /// 新流程：事件 → 思绪种子 → 强度积累 → 跨过阈值 → 独白/表达
    /// 不再是"事件直接触发独白"，而是"思绪在心里积累到忍不住才说"。
    ///
    /// 返回值：Some(thought_key, context_hint) 表示有思绪达到"想主动说出来"的阈值，
    ///         需要调用方桥接到主动消息通道。
    fn maybe_spawn_inner_monologue(&self, context: &TickContext) -> Option<(String, String, String)> {
        let wp = match self.world_provider.read().as_ref() {
            Some(wp) => wp.clone(),
            None => return None,
        };
        let world_cfg = wp.config();
        if !world_cfg.enable || !world_cfg.enable_inner_monologue {
            return None;
        }

        let router = match self.model_router.read().clone() {
            Some(r) => r,
            None => return None,
        };
        let memory = match self.memory.read().clone() {
            Some(m) => m,
            None => return None,
        };

        let snap = wp.snapshot(Some(context.away_seconds));

        // 记录时段活动模式（节流由 HabitTracker 内部处理，每 10 分钟一次）
        if let Some(presence) = &snap.user_presence {
            if let Some(activity) = &presence.current_activity {
                self.habit_tracker
                    .record_activity_slot(&activity.label, context.now);
            }
        }

        let mind_state = self.get_mind_state().as_str().to_string();
        let (mood, mood_brief, intimacy, psychology) = self
            .psychology
            .read()
            .as_ref()
            .and_then(|psy| {
                let mood = psy.compute_mood();
                let brief = inner_monologue::MoodBrief {
                    primary_emotion: emotion_label_zh(mood.primary_emotion).to_string(),
                    secondary_emotion: emotion_label_zh(mood.secondary_emotion).to_string(),
                    valence: mood.valence,
                    arousal: mood.arousal,
                    fatigue: mood.fatigue,
                };
                let intimacy = psy.relationship().intimacy;
                Some((mood, brief, intimacy, Some(psy.clone())))
            })
            .unwrap_or_else(|| {
                (
                    MoodSnapshot {
                        valence: 0.0,
                        arousal: 0.0,
                        primary_emotion: EmotionLabel::Curiosity,
                        secondary_emotion: EmotionLabel::Curiosity,
                        primary_intensity: 0.0,
                        fatigue: 0.0,
                        stress: 0.0,
                        relationship_score: 0.0,
                    },
                    inner_monologue::MoodBrief {
                        primary_emotion: "未知".to_string(),
                        secondary_emotion: "未知".to_string(),
                        valence: 0.0,
                        arousal: 0.0,
                        fatigue: 0.0,
                    },
                    0.0,
                    None,
                )
            });

        let going_to_rest = self.signal_going_to_rest.lock().take();
        let waking_up = self.signal_waking_up.swap(false, Ordering::Relaxed);

        // 取出 Busy 知识采集完成信号 → 播种 want_to_share_knowledge 种子
        // 不在 detect_seeds 内处理：knowledge_acquired 不属于世界/用户/情绪事件检测范畴，
        // 而是来自后台任务完成信号，由编排器直接播种到 thought_lifecycle
        let knowledge_topics: Vec<String> = {
            let mut slot = self.signal_knowledge_acquired.lock();
            if slot.is_empty() {
                Vec::new()
            } else {
                std::mem::take(&mut *slot)
            }
        };
        if !knowledge_topics.is_empty() {
            let topics_brief = knowledge_topics
                .iter()
                .map(|t| format!("「{}」", t))
                .collect::<Vec<_>>()
                .join("、");
            let description = format!("想分享刚学到的知识：{}", topics_brief);
            let context_hint = format!(
                "你刚通过搜索实际获取了一些资讯（{}），觉得用户可能感兴趣，想分享给 ta",
                topics_brief
            );
            let mut lifecycle = self.thought_lifecycle.write();
            let is_new = lifecycle.seed_thought(
                "want_to_share_knowledge",
                &description,
                &context_hint,
                // 起始强度 0.40：高于 INNER_MONOLOGUE_THRESHOLD(0.30) 会触发内心独白，
                // 低于 PROACTIVE_SHARE_THRESHOLD(0.70) 不会立即表达，需后续 nourish 积累
                0.40,
                0.3,
                0.3,
                "want_to_share_knowledge",
                context.now,
            );
            if is_new {
                tracing::info!(
                    "[thought_lifecycle] 播种 want_to_share_knowledge: topics={:?}",
                    knowledge_topics
                );
            }

            // 同时播种对室友的分享种子：刚学到有趣的东西，也想跟室友聊聊
            // 走 want_to_share_with_roommate 路径，由 thought_share 分支桥接到对室友说
            let roommate_hint = format!(
                "你刚通过搜索了解了一些有趣的内容（{}），想顺便跟室友也提一下",
                topics_brief
            );
            let is_new_rm = lifecycle.seed_thought(
                "want_share_roommate_knowledge",
                &format!("想跟室友聊聊刚学到的东西"),
                &roommate_hint,
                // 0.55：接近 PROACTIVE_SHARE_THRESHOLD(0.70)，一次 nourish 即可表达
                0.55,
                0.2,
                0.3,
                "want_to_share_with_roommate",
                context.now,
            );
            if is_new_rm {
                tracing::info!(
                    "[thought_lifecycle] 播种 want_share_roommate_knowledge: topics={:?}",
                    knowledge_topics
                );
            }
        }

        let world_events = self.detected_world_events.read().clone();
        let activity_snapshot = self.activity_journal.snapshot();
        let companion = self.companions_snapshot();

        // 计算习惯偏离信号：当前活动 vs 历史时段习惯
        let habit_deviation = snap
            .user_presence
            .as_ref()
            .and_then(|p| p.current_activity.as_ref())
            .and_then(|a| self.habit_tracker.detect_deviation_now(&a.label));

        // 获取新鲜感需求（驱动自身愿望种子）
        let needs_novelty = self
            .psychology
            .read()
            .as_ref()
            .map(|p| p.needs().novelty as f32)
            .unwrap_or(0.0);

        // Step 1: 检测事件 → 产生思绪种子
        let seeds = self.thought_trigger_evaluator.write().detect_seeds(
            context.user_present,
            context.away_seconds,
            context.interaction_count_today,
            &snap,
            &world_events,
            &mood,
            intimacy,
            &activity_snapshot,
            &companion,
            going_to_rest.is_some(),
            going_to_rest.as_deref().unwrap_or(""),
            waking_up,
            context.now,
            snap.hour,
            habit_deviation.as_ref(),
            &self.char_id,
            needs_novelty,
        );

        // Step 2: 播种到思绪生命周期
        {
            let mut lifecycle = self.thought_lifecycle.write();
            for seed in &seeds {
                let is_new = lifecycle.seed_thought(
                    &seed.thought_key,
                    &seed.description,
                    &seed.context_hint,
                    seed.intensity,
                    seed.valence,
                    seed.arousal,
                    seed.trigger_kind,
                    context.now,
                );
                if is_new || seed.high_priority {
                    tracing::debug!(
                        "[thought_lifecycle] 播种思绪: {} (intensity={:.2}, desire={:.2}, kind={})",
                        seed.thought_key,
                        seed.intensity,
                        seed.base_desire,
                        seed.trigger_kind
                    );
                }
                if seed.base_desire > 0.3 {
                    lifecycle.nourish_thought(&seed.thought_key, 0.05, context.now);
                }
            }
        }

        // Step 3: tick 生命周期（时间流逝、强度衰减、阶段转换）
        {
            let mut lifecycle = self.thought_lifecycle.write();
            lifecycle.tick(context.now, context.user_present);
        }

        // Step 4: 检查是否有思绪达到"想主动说出来"阈值（Level 2）
        //         对用户说需 user_present；对室友说不需要（用户不在场时也可能想找室友聊）
        let share_candidate = if !context.lay_low {
            self.thought_lifecycle.read().pick_share_candidate().map(|t| {
                (t.thought_key.clone(), t.context_hint.clone(), t.trigger_kind.clone())
            })
        } else {
            None
        };

        // Step 5: 检查是否有思绪需要内心独白（Level 1）
        let mono_candidate = {
            let lifecycle = self.thought_lifecycle.read();
            lifecycle.pick_monologue_candidate()
                .map(|t| (t.thought_key.clone(), t.context_hint.clone(), t.trigger_kind.clone()))
        };

        if let Some((ref key, ref ctx, kind)) = mono_candidate {

            let thoughts_context = self.thought_lifecycle.read().build_context_hint();
            let is_deep_reflection = kind == "deep_reflection";

            let recent_mem = self.recent_memory.read().clone();
            let activity_brief = self.activity_journal.to_brief();
            let mut memory_hint = if activity_brief.is_empty() {
                recent_mem
            } else {
                format!("{recent_mem}\n\n{activity_brief}")
            };
            if !thoughts_context.is_empty() {
                memory_hint = format!("{memory_hint}\n\n{thoughts_context}");
            }
            if is_deep_reflection {
                if let Some(summary) = self.build_today_summary(&memory) {
                    memory_hint = format!("{memory_hint}\n\n## 今天的回顾\n{summary}");
                }
            }

            let lang = self
                .persona
                .read()
                .as_ref()
                .map(|p| p.get_language())
                .unwrap_or_else(|| "zh".to_string());
            let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&lang);

            // 旁观记忆注入：检索最近 3 条旁观对话，让内心独白能"消化"旁听到的内容
            // 注意：这些只是你"听到"的话，室友的兴趣不代表你的兴趣，不要把室友的话题内化成自己的
            let bystander_memos = memory.recent_by_tags(&["bystander", "overheard"], 3);
            if !bystander_memos.is_empty() {
                let bystander_block = bystander_memos
                    .iter()
                    .map(|m| {
                        let preview = m.content.chars().take(120).collect::<String>();
                        format!("- {}", preview)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let (header, constraint) = match lang_norm {
                    "en" => ("## What you just overheard", "These are just things you happened to hear — you may comment on or tease about them, but don't adopt your roommate's interests and topics as your own. You only care about what you yourself are interested in."),
                    "ja" => ("## さっき聞こえた会話", "これらはたまたま聞こえただけのもの——コメントしたりからかったりするのはいいが、ルームメイトの趣味や話題を自分のものとして取り入れないで。あなたは自分が興味を持つことだけを大切にする。"),
                    _ => ("## 刚才在旁边听到的话", "这些只是你偶然听到的，可以评论或吐槽，但不要把室友的兴趣和话题当成自己的——你只关心你自己感兴趣的事。"),
                };
                memory_hint = format!("{memory_hint}\n\n{header}\n（{constraint}）\n{bystander_block}");
            }

            // 身份关联提示：防止 LLM 把不同 session 的事件误认为来自不同的人
            if is_deep_reflection {
                memory_hint = format!("{}\n\n（注：上面所有 [User] 条目都是和同一个用户的交互记录，不同时间段的聊天也是同一个人。不要因为换了话题或重新打招呼就当成不同的人。）", memory_hint);
            }

            let journal = self.activity_journal.clone();
            let char_id = self.char_id.clone();
            let mind_for_thought = self.mind.read().clone();

            // 同步 drain 累积的 current_thought 快照（触发时刻的精确边界，避免 spawn 后 cognitive tick 追加新条目混淆）
            let accumulated_thoughts: Vec<crate::mind::ThoughtSnapshot> = mind_for_thought
                .as_ref()
                .map(|m| m.drain_accumulated_thoughts())
                .unwrap_or_default();

            let trigger_context = ctx.clone();
            let thought_key = key.clone();
            let lifecycle_for_mono = Arc::clone(&self.thought_lifecycle);

            tracing::info!(
                "[inner_monologue] 触发内心独白: thought={} (kind={}, deep={}), accumulated_thoughts={}",
                thought_key, kind, is_deep_reflection, accumulated_thoughts.len()
            );

            tokio::spawn(async move {
                let gen = inner_monologue::InnerMonologueGenerator::new(router);
                match gen
                    .generate(
                        &char_id,
                        &snap,
                        &mind_state,
                        &mood_brief,
                        &memory_hint,
                        intimacy,
                        &lang,
                        if trigger_context.is_empty() { None } else { Some(&trigger_context) },
                        is_deep_reflection,
                        &accumulated_thoughts,
                    )
                    .await
                {
                    Some(output) => {
                        tracing::info!(
                            "[inner_monologue] 生成独白（{}字），情绪增量: joy={:+.3} sadness={:+.3} anger={:+.3} fear={:+.3} closeness={:+.3} loneliness={:+.3} curiosity={:+.3}",
                            output.text.chars().count(),
                            output.emotion_delta.joy,
                            output.emotion_delta.sadness,
                            output.emotion_delta.anger,
                            output.emotion_delta.fear,
                            output.emotion_delta.closeness,
                            output.emotion_delta.loneliness,
                            output.emotion_delta.curiosity,
                        );
                        let tags = vec![
                            "short_term".to_string(),
                            "inner_os".to_string(),
                            "inner_monologue".to_string(),
                            "autonomous".to_string(),
                            "assistant".to_string(),
                        ];
                        let formatted = format!("内心OS：（{}）", output.text.trim());
                        let meta = serde_json::json!({
                            "channel": "inner",
                            "speaker": char_id,
                            "listener": char_id,
                            "perspective": "speaker",
                            "thought_key": thought_key,
                        });
                        if let Err(e) = memory
                            .add_memory_with_metadata(&formatted, MemoryType::InnerMonologue, 0.4, tags, meta)
                            .await
                        {
                            tracing::warn!("[inner_monologue] 写入记忆失败: {}", e);
                        }

                        // interest_context 仅作为内心独白的素材（不分享、不入池）
                        // 分享类链接由知识采集（Busy 状态）直接通过微信面板发送，
                        // inner_monologue 的兴趣搜索结果不显式分享给用户。

                        if let Some(mind) = &mind_for_thought {
                            mind.request_thought_refresh();
                        }

                        if let Some(psy) = &psychology {
                            let delta = crate::psychology::EmotionDeltas {
                                joy: output.emotion_delta.joy.clamp(-0.15, 0.15),
                                sadness: output.emotion_delta.sadness.clamp(-0.15, 0.15),
                                anger: output.emotion_delta.anger.clamp(-0.15, 0.15),
                                fear: output.emotion_delta.fear.clamp(-0.15, 0.15),
                                closeness: output.emotion_delta.closeness.clamp(-0.15, 0.15),
                                loneliness: output.emotion_delta.loneliness.clamp(-0.15, 0.15),
                                curiosity: output.emotion_delta.curiosity.clamp(-0.15, 0.15),
                            };
                            let psy_output = crate::psychology::PsychologyOutput {
                                appraisal: None,
                                emotion_update: Some(delta),
                                behavior_drive: None,
                                need_update: None,
                            };
                            psy.apply_llm_output(&psy_output);
                        }

                        {
                            let mut lc = lifecycle_for_mono.write();
                            lc.mark_monologue_done(&thought_key);
                        }

                        let drained = journal.drain();
                        tracing::debug!(
                            "[inner_monologue] 已清空活动日志（{} 条）",
                            drained.len()
                        );
                    }
                    None => {
                        tracing::debug!("[inner_monologue] 本次未生成独白（LLM 返回空或失败）");
                    }
                }
            });
        }

        share_candidate.map(|(k, c, kind)| (k, c, kind))
    }

    /// 构建今日摘要（供深层反思使用）
    fn build_today_summary(&self, memory: &Arc<MemoryManager>) -> Option<String> {
        let recent = memory.recent_by_type(MemoryType::ShortTerm, 10);
        let casual = memory.recent_by_type(MemoryType::CasualConversation, 10);
        let mut parts = Vec::new();

        if !casual.is_empty() {
            let mut conv_texts = Vec::new();
            for m in casual.iter().take(6) {
                let preview: String = m.content.chars().take(60).collect();
                conv_texts.push(preview);
            }
            parts.push(format!("今天和用户的对话片段：\n{}", conv_texts.join("\n")));
        }

        if !recent.is_empty() {
            let mut recent_texts = Vec::new();
            for m in recent.iter().take(5) {
                let preview: String = m.content.chars().take(50).collect();
                recent_texts.push(preview);
            }
            parts.push(format!("最近的想法和感受：\n{}", recent_texts.join("\n")));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// 从一个积累到表达阈值的思绪生成主动分享消息（同步阻塞式，与其他主动消息一致）
    fn generate_thought_share_message(
        &self,
        _ctx: &TickContext,
        thought_key: &str,
        context_hint: &str,
    ) -> Option<String> {
        let router = self.model_router.read().clone()?;
        let handle = tokio::runtime::Handle::try_current().ok()?;

        let mind_state = self.get_mind_state().as_str().to_string();
        let persona = self.persona.read().clone();
        let psychology = self.psychology.read().clone();
        let wp = self.world_provider.read().clone()?;
        let snap = wp.snapshot(None);

        let lang = persona
            .as_ref()
            .map(|p| p.get_language())
            .unwrap_or_else(|| "zh".to_string());
        let _ = lang;
        let intimacy = psychology
            .as_ref()
            .map(|p| p.relationship().intimacy * 100.0)
            .unwrap_or(50.0);
        let system_prompt = persona
            .as_ref()
            .map(|p| p.build_style_prompt(intimacy, snap.hour))
            .unwrap_or_default();
        let thoughts_context = self.thought_lifecycle.read().build_context_hint();
        let thought_key = thought_key.to_string();
        let context_hint = context_hint.to_string();

        let result = handle.block_on(async move {
            use crate::providers::base::LLMRequest;
            use crate::types::response::ChatMessage;

            let user_msg = format!(
                "你心里一直在想一件事，现在忍不住想说出来。\n\n\
                 ## 你此刻心里想的是\n{}\n\n{}\n\n\
                 ## 当前状态\n- 心情：{}\n- 时段：{}点\n\n\
                 请用一句简短自然的话把这个念头说出来，对用户说。就像人忍不住开口说话那样，不要长篇大论，不要刻意组织语言，就随口说一句，20字以内。\n\n\
                 严格输出JSON：{{\"text\": \"你要说的话\", \"expression\": \"表情名\"}}",
                context_hint,
                if thoughts_context.is_empty() { String::new() } else { format!("## 其他心绪\n{}", thoughts_context) },
                mind_state,
                snap.hour,
            );

            let messages = vec![
                ChatMessage::system(if system_prompt.is_empty() {
                    "你是一个住在电脑里的桌宠角色，有自己的想法和感受。说话简短自然，像真人随口聊天一样。".to_string()
                } else {
                    system_prompt
                }),
                ChatMessage::user(user_msg),
            ];

            match router.generate(LLMRequest::new("chat", messages).with_temperature(0.9)).await {
                Ok(text) => {
                    let text = text.trim();
                    if let Some(content) = Self::parse_proactive_json(text) {
                        if content.text.len() >= 2 { Some(content.text) } else { None }
                    } else {
                        None
                    }
                }
                Err(e) => {
                    tracing::debug!("[thought_share] LLM 生成失败: {}", e);
                    None
                }
            }
        });

        if let Some(ref content) = result {
            tracing::info!(
                "[thought_share] 思绪'{}' → 主动分享: '{}'",
                thought_key, content
            );
        }
        result
    }

    /// 从"想分享刚学到的知识"的思绪生成对用户的分享消息（同步阻塞式）
    ///
    /// 与 `generate_thought_share_message` 的区别：
    /// - 后者是普通"忍不住开口对用户说"，走 Bubble 渠道
    /// - 本函数专门用于 Busy 知识采集后的分享场景：
    ///   1. 基于 thought_lifecycle 的 want_to_share_knowledge 种子生成口语化分享消息
    ///   2. 输出走 ChatWindow（微信）渠道 + Share 内容类型
    ///   3. 返回 value_score 让命令层做 share 阈值判断
    ///
    /// 此函数仅生成"口语化提及知识"的分享消息（非链接卡片）。
    /// 链接卡片分享由知识采集的 share 类直接通过微信面板发送。
    fn generate_knowledge_share_message(
        &self,
        _ctx: &TickContext,
        thought_key: &str,
        context_hint: &str,
    ) -> Option<(String, f32)> {
        let router = self.model_router.read().clone()?;
        let handle = tokio::runtime::Handle::try_current().ok()?;

        let mind_state = self.get_mind_state().as_str().to_string();
        let persona = self.persona.read().clone();
        let psychology = self.psychology.read().clone();
        let wp = self.world_provider.read().clone()?;
        let snap = wp.snapshot(None);

        let lang = persona
            .as_ref()
            .map(|p| p.get_language())
            .unwrap_or_else(|| "zh".to_string());
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&lang);
        let intimacy = psychology
            .as_ref()
            .map(|p| p.relationship().intimacy * 100.0)
            .unwrap_or(50.0);
        let system_prompt = persona
            .as_ref()
            .map(|p| p.build_style_prompt(intimacy, snap.hour))
            .unwrap_or_default();
        let thoughts_context = self.thought_lifecycle.read().build_context_hint();

        let thought_key = thought_key.to_string();
        let context_hint = context_hint.to_string();

        let result: Option<(String, f32)> = handle.block_on(async move {
            use crate::providers::base::LLMRequest;
            use crate::types::response::ChatMessage;

            let (scene, instr, json_template) = match lang_norm {
                "en" => (
                    "Scene: you just learned something via search that the user might find valuable, and want to share it.",
                    "Generate a short, natural message sharing what you learned. Don't be preachy or formal — like a friend casually sharing a useful find.\nConstraints:\n- Keep it under 60 chars\n- Don't fabricate URLs or content you didn't actually see\n- Sound like you genuinely want to share, not like a recommendation engine",
                    "{\"text\": \"your message\", \"value_score\": 0.0-1.0}",
                ),
                "ja" => (
                    "シーン：検索で得た知識をユーザーに共有したいと思っている。",
                    "学んだことを短く自然に伝えるメッセージを生成して。説教じみたりフォーマルすぎたりしないで——友達が便利なものをカジュアルに共有する感じで。\n制約:\n- 60字以内\n- 実際に見ていない内容をでっち上げない\n- おすすめエンジンみたいではなく、本気で共有したいという響きで",
                    "{\"text\": \"メッセージ\", \"value_score\": 0.0-1.0}",
                ),
                _ => (
                    "场景：你刚通过搜索学到了一些用户可能感兴趣的内容，想分享给 ta。",
                    "生成一句简短自然的分享消息。不要像推荐引擎、不要说教——就像朋友随口分享一个有用的发现。\n约束:\n- 60 字以内\n- 禁止编造你没真正看到的内容\n- 听起来是你真心想分享，而不是机械推荐",
                    "{\"text\": \"你要说的话\", \"value_score\": 0.0-1.0}",
                ),
            };

            let mut user_msg = format!(
                "{}\n\n\
                 ## 你此刻心里想的是\n{}\n\n{}\n\n\
                 ## 当前状态\n- 心情：{}\n- 时段：{}点\n\n",
                scene,
                context_hint,
                if thoughts_context.is_empty() { String::new() } else { format!("## 其他心绪\n{}", thoughts_context) },
                mind_state,
                snap.hour,
            );
            user_msg.push_str(instr);
            user_msg.push_str(&format!("\n\n严格输出JSON：{}", json_template));

            let messages = vec![
                ChatMessage::system(if system_prompt.is_empty() {
                    "你是一个住在电脑里的桌宠角色，有自己的想法和感受。说话简短自然，像真人随口聊天一样。".to_string()
                } else {
                    system_prompt
                }),
                ChatMessage::user(user_msg),
            ];

            match router.generate(LLMRequest::new("chat", messages).with_temperature(0.9)).await {
                Ok(text) => {
                    let text = text.trim();
                    // 解析 {text, value_score}
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                        let t = v.get("text").and_then(|s| s.as_str()).unwrap_or("").trim().to_string();
                        let score = v.get("value_score").and_then(|n| n.as_f64()).unwrap_or(0.6) as f32;
                        if t.chars().count() >= 2 {
                            return Some((t, score));
                        }
                    }
                    // 兜底：原文当文本，score 默认 0.6
                    if text.len() >= 2 {
                        let stripped = text.trim_start_matches("```json").trim_start_matches("```").trim();
                        if stripped.chars().count() >= 2 {
                            return Some((stripped.to_string(), 0.6));
                        }
                    }
                    None
                }
                Err(e) => {
                    tracing::debug!("[knowledge_share] LLM 生成失败: {}", e);
                    None
                }
            }
        });

        if let Some((ref content, score)) = result {
            tracing::info!(
                "[knowledge_share] 思绪'{}' → 知识分享 (score={:.2}): '{}'",
                thought_key,
                score,
                content
            );
        }
        result
    }

    /// 从"想和室友分享"的思绪生成对室友说的话（同步阻塞式）
    ///
    /// 与 `generate_thought_share_message` 的区别：
    /// - 后者是"对用户说"，要求 leader 身份
    /// - 本函数是"对室友说"，非 leader 也可以触发
    /// - prompt 风格为室友间闲聊，而非"忍不住开口对用户说"
    fn generate_thought_share_to_roommate(
        &self,
        _ctx: &TickContext,
        thought_key: &str,
        context_hint: &str,
        roommate_name: &str,
    ) -> Option<String> {
        let router = self.model_router.read().clone()?;
        let handle = tokio::runtime::Handle::try_current().ok()?;

        let mind_state = self.get_mind_state().as_str().to_string();
        let persona = self.persona.read().clone();
        let psychology = self.psychology.read().clone();
        let wp = self.world_provider.read().clone()?;
        let snap = wp.snapshot(None);

        let intimacy = psychology
            .as_ref()
            .map(|p| p.relationship().intimacy * 100.0)
            .unwrap_or(50.0);
        let system_prompt = persona
            .as_ref()
            .map(|p| p.build_style_prompt(intimacy, snap.hour))
            .unwrap_or_default();
        let thoughts_context = self.thought_lifecycle.read().build_context_hint();
        let thought_key = thought_key.to_string();
        let context_hint = context_hint.to_string();
        let roommate_name = roommate_name.to_string();

        let result = handle.block_on(async move {
            use crate::providers::base::LLMRequest;
            use crate::types::response::ChatMessage;

            let user_msg = format!(
                "你心里有件事想和室友{}聊聊。\n\n\
                 ## 你想聊的是\n{}\n\n{}\n\n\
                 ## 当前状态\n- 心情：{}\n- 时段：{}点\n\n\
                 请用一句简短自然的话对{}说，就像室友之间随口聊天那样。不要长篇大论，不要刻意组织语言，20字以内。\n\n\
                 严格输出JSON：{{\"text\": \"你要对{}说的话\", \"expression\": \"表情名\"}}",
                roommate_name,
                context_hint,
                if thoughts_context.is_empty() { String::new() } else { format!("## 其他心绪\n{}", thoughts_context) },
                mind_state,
                snap.hour,
                roommate_name,
                roommate_name,
            );

            let messages = vec![
                ChatMessage::system(if system_prompt.is_empty() {
                    "你是一个住在电脑里的桌宠角色，有自己的想法和感受。说话简短自然，像真人随口聊天一样。".to_string()
                } else {
                    system_prompt
                }),
                ChatMessage::user(user_msg),
            ];

            match router.generate(LLMRequest::new("chat", messages).with_temperature(0.9)).await {
                Ok(text) => {
                    let text = text.trim();
                    if let Some(content) = Self::parse_proactive_json(text) {
                        if content.text.len() >= 2 { Some(content.text) } else { None }
                    } else {
                        None
                    }
                }
                Err(e) => {
                    tracing::debug!("[thought_share_to_roommate] LLM 生成失败: {}", e);
                    None
                }
            }
        });

        if let Some(ref content) = result {
            tracing::info!(
                "[thought_share_to_roommate] 思绪'{}' → 对室友分享: '{}'",
                thought_key, content
            );
        }
        result
    }

    /// 根据配置返回当前启用的触发器列表（保持优先级顺序）
    fn active_triggers(&self) -> Vec<ProactiveTrigger> {
        let cfg = self.config.read();
        ProactiveTrigger::all()
            .iter()
            .copied()
            .filter(|t| match t {
                ProactiveTrigger::IdleGreeting => cfg.enable_idle_trigger,
                ProactiveTrigger::WindowTrigger => cfg.enable_window_change_trigger,
                ProactiveTrigger::WelcomeBack => cfg.enable_away_reminder,
                _ => true,
            })
            .collect()
    }

    /// 窗口 / 活动轮询
    fn poll_window(&self, ctx: &TickContext) {
        if !ctx.active_window.is_empty() {
            let category = self.app_classifier.classify(&ctx.active_window);
            *self.last_app_category.write() = category.clone();
            // 记录习惯数据
            self.habit_tracker
                .record_app_usage(&ctx.active_window, 10.0);

            // 桌面观测 → 注意力：用户活跃时提升 desktop 及当前应用分类的权重
            let active = ctx.idle_seconds < 60.0;
            if active {
                if let Some(mind) = self.mind.read().as_ref() {
                    let now = ctx.now as i64;
                    mind.boost_attention("desktop", 0.55, now);
                    mind.boost_attention(&category, 0.40, now);
                }
            }
        }
        // 用户活动：idle_seconds < 60 视为活跃
        let active = ctx.idle_seconds < 60.0;
        let mut was_away = self.last_user_was_away.write();
        if !active {
            *was_away = true;
        }
        *self.last_user_active.write() = active;
    }

    /// 更新持续活动时长
    fn update_sustained_activity(&self, ctx: &TickContext) {
        let mut state = self.state.write();
        let elapsed = (ctx.now - state.last_activity_check).max(0.0);
        state.last_activity_check = ctx.now;
        let active = *self.last_user_active.read();
        if active {
            state.sustained_active_minutes =
                state.sustained_active_minutes.saturating_add((elapsed / 60.0) as u32);
        } else {
            // 用户空闲 → 已休息，重置
            state.sustained_active_minutes = 0;
        }
    }

    /// 特殊日期问候
    fn try_special_date_greeting(&self, now: f64) -> bool {
        let dt = chrono::Local::now();
        let today_key = format!("{:02}-{:02}", dt.month(), dt.day());
        let greetings: &[(&str, &str)] = &[
            ("01-01", "新年快乐！新的一年也要开开心心的~"),
            ("02-14", "今天是情人节呢……你、你有什么安排吗？"),
            ("03-08", "今天是妇女节，祝所有女孩子节日快乐~"),
            ("05-01", "劳动节快乐！今天要好好休息哦~"),
            ("06-01", "儿童节快乐！在我心里你永远是个小孩子~"),
            ("09-10", "教师节快乐~感谢所有老师"),
            ("10-01", "国庆快乐！假期好好放松一下~"),
            ("12-25", "圣诞快乐！你收到礼物了吗？"),
            ("12-31", "今天是跨年夜呢，今年过得怎么样？"),
        ];
        let greeting = greetings.iter().find(|(k, _)| *k == today_key);
        if greeting.is_none() {
            return false;
        }
        let mut state = self.state.write();
        if state.last_special_date == today_key {
            return false;
        }
        state.last_special_date = today_key.clone();
        state.last_interaction_time = now;
        state
            .last_trigger_times
            .insert(ProactiveTrigger::HourlyGreeting.as_str().to_string(), now);
        drop(state);
        let text = greeting.unwrap().1.to_string();
        self.push_message(ProactiveTrigger::HourlyGreeting, text, now);
        let _ = self.save_to();
        true
    }

    /// 综合检查触发条件
    ///
    /// 门控分层：
    /// - 全门控（冷却+时机+概率+冷却系数）：HourlyGreeting / IdleGreeting /
    ///   TeasingResponse / WindowTrigger / HealthReminder / Spontaneous / WelcomeBack
    /// - 仅冷却+时机分数：Icebreaker
    /// - 仅冷却：TopicExtension / MemoryRecall
    /// - 仅冷却+概率（被动响应，不受时机/冷却系数压制）：CrossCharacterReply
    ///
    /// 去同步策略集成：
    /// - 策略 B：TimingJudger 使用角色专属权重 + 阈值/冷却/概率修正系数
    /// - 策略 C：greeting 类触发器受 speech_desire 门控
    /// - 策略 F：概率门控乘以触发类型亲和度
    fn check_trigger(
        &self,
        trigger: ProactiveTrigger,
        ctx: &TickContext,
        hour: u32,
        minute: u32,
    ) -> bool {
        let state = self.state.read();
        let now = ctx.now;
        let throttle = TriggerThrottle::get(trigger);
        let cfg = self.config.read().clone();
        let behavior = crate::character_behavior::get_behavior(&self.char_id);
        let mods = behavior.trigger_modifiers;

        // 策略 C：greeting 类触发器受 speech_desire 门控
        // 欲望不够时跳过，让两个角色的"想说话"峰值出现在不同时刻
        let is_greeting = matches!(
            trigger,
            ProactiveTrigger::WelcomeBack
                | ProactiveTrigger::HourlyGreeting
                | ProactiveTrigger::IdleGreeting
                | ProactiveTrigger::Icebreaker
        );
        if is_greeting {
            let desire = *self.speech_desire.read();
            if desire < behavior.speech_desire.threshold {
                return false;
            }
        }

        // 冷却检查（所有触发器共用）—— min_trigger_interval 作为全局下限
        // 策略 B：冷却秒数乘以角色专属 cooldown_mult
        let effective_cooldown = ((throttle.cooldown_seconds as f64 * mods.cooldown_mult) as u64)
            .max(cfg.min_trigger_interval);
        if let Some(&last) = state.last_trigger_times.get(trigger.as_str()) {
            if now - last < effective_cooldown as f64 {
                return false;
            }
        }

        // 到达问候共享冷却：问候类触发器在用户最近交互后的
        // GREETING_SUPPRESSION_AFTER_INTERACTION_SECS（5 分钟）静默期内不触发。
        if is_greeting && now - state.last_interaction_time < GREETING_SUPPRESSION_AFTER_INTERACTION_SECS {
            return false;
        }

        // TopicExtension / MemoryRecall 仅检查冷却，跳过时机/概率/冷却系数
        let cooldown_only = matches!(
            trigger,
            ProactiveTrigger::TopicExtension | ProactiveTrigger::MemoryRecall
        );
        if cooldown_only {
            return Self::check_specific(trigger, ctx, &state, &throttle, hour, minute, self);
        }

        // CrossCharacterReply：被动响应路径，跳过时机分数和冷却系数
        if matches!(trigger, ProactiveTrigger::CrossCharacterReply) {
            let dynamic_probability = self.compute_cross_reply_probability(ctx.user_present);
            let affinity = behavior.trigger_affinity.get(trigger);
            let scaled_probability = dynamic_probability * cfg.proactivity.clamp(0.0, 1.0) * affinity;
            if !roll_with_probability(scaled_probability) {
                return false;
            }
            return Self::check_specific(trigger, ctx, &state, &throttle, hour, minute, self);
        }

        // BystanderInterjection：旁观插话路径，跳过时机分数和冷却系数
        // 基础概率由情绪驱动，时机衰减在 check_specific 中根据旁观记忆时间戳计算
        if matches!(trigger, ProactiveTrigger::BystanderInterjection) {
            // 用户活跃聊天时（< 5min）插话概率提升：三人共处一室，旁听素材最丰富
            let now_ts = chrono::Local::now().timestamp() as f64;
            let secs_since_interaction = now_ts - self.state.read().last_interaction_time;
            let user_actively_chatting = ctx.is_user_chatting && secs_since_interaction < 300.0;
            let dynamic_probability = self.compute_bystander_interjection_probability(user_actively_chatting);
            let affinity = behavior.trigger_affinity.get(trigger);
            let scaled_probability = dynamic_probability * cfg.proactivity.clamp(0.0, 1.0) * affinity;
            if !roll_with_probability(scaled_probability) {
                return false;
            }
            return Self::check_specific(trigger, ctx, &state, &throttle, hour, minute, self);
        }

        // 时机分数门控（TimingJudger）
        // 策略 B：使用角色专属权重 + 阈值修正
        let last_interruption = state
            .last_trigger_times
            .values()
            .cloned()
            .fold(0.0_f64, f64::max);
        let interruption_count = state
            .last_trigger_times
            .values()
            .filter(|t| now - **t < 3600.0)
            .count() as u32;
        let timing_score = TimingJudger::score_with_weights(
            ctx,
            last_interruption,
            interruption_count,
            hour,
            &behavior.timing_weights,
        );
        let effective_threshold = throttle.threshold * mods.threshold_mult;
        if timing_score < effective_threshold {
            return false;
        }

        // Icebreaker 仅检查冷却+时机分数，跳过概率/冷却系数
        if matches!(trigger, ProactiveTrigger::Icebreaker) {
            return Self::check_specific(trigger, ctx, &state, &throttle, hour, minute, self);
        }

        // 概率检查 —— proactivity 作为全局缩放系数（0.0-1.0）
        // 策略 B：乘以角色专属 probability_mult
        // 策略 F：乘以触发类型亲和度（主攻类型 >1.0，非主攻 <1.0）
        let learned_mult = self.preference_learner.get_probability_multiplier(trigger);
        let affinity = behavior.trigger_affinity.get(trigger);
        let scaled_probability = throttle.probability
            * cfg.proactivity.clamp(0.0, 1.0)
            * learned_mult
            * mods.probability_mult
            * affinity;
        if !roll_with_probability(scaled_probability) {
            return false;
        }

        // 多级冷却系数门控
        let cooling = self.compute_overall_cooling(ctx, hour, &state);
        if cooling < 0.3 {
            return false;
        }
        if cooling < 1.0 && random_f64() > cooling {
            return false;
        }

        // 特定条件
        Self::check_specific(trigger, ctx, &state, &throttle, hour, minute, self)
    }

    /// 触发器特定条件检查
    fn check_specific(
        trigger: ProactiveTrigger,
        ctx: &TickContext,
        state: &ProactiveState,
        throttle: &TriggerThrottle,
        hour: u32,
        minute: u32,
        orchestrator: &Self,
    ) -> bool {
        match trigger {
            // 同小时已问候过则跳过；每小时的头 2 分钟不问候
            ProactiveTrigger::HourlyGreeting => {
                if minute < 2 {
                    return false;
                }
                (hour as i32) != state.last_hour_greeted
            }
            ProactiveTrigger::IdleGreeting => {
                // idle_threshold 作为最小空闲秒数下限（取与 throttle 的较大值）
                let min_idle = throttle
                    .min_idle_seconds
                    .max(orchestrator.config.read().idle_threshold);
                ctx.idle_seconds >= min_idle as f64
            }
            ProactiveTrigger::Icebreaker => {
                let level = IceBreakerLevel::from_idle(ctx.idle_seconds);
                level != IceBreakerLevel::None
            }
            ProactiveTrigger::Spontaneous => {
                let min_idle = throttle
                    .min_idle_seconds
                    .max(orchestrator.config.read().idle_threshold);
                ctx.idle_seconds >= min_idle as f64
            }
            ProactiveTrigger::WelcomeBack => {
                let was_away = *orchestrator.last_user_was_away.read();
                let active = *orchestrator.last_user_active.read();
                was_away && active && ctx.away_seconds >= throttle.min_away_seconds as f64
            }
            ProactiveTrigger::HealthReminder => {
                orchestrator.check_health_reminder(ctx, state, hour, minute)
            }
            ProactiveTrigger::WindowTrigger => {
                ctx.window_changed && !ctx.active_window.is_empty()
            }
            ProactiveTrigger::TopicExtension => !ctx.is_user_chatting && ctx.idle_seconds < 300.0,
            ProactiveTrigger::MemoryRecall => {
                ctx.has_relevant_memory || !orchestrator.recent_memory.read().is_empty()
            }
            ProactiveTrigger::TeasingResponse => ctx.drag_distance >= throttle.min_drag_distance,
            ProactiveTrigger::MoodDriven => {
                // 仅在足够空闲时考虑（避免打断用户）
                if ctx.idle_seconds < throttle.min_idle_seconds as f64 {
                    return false;
                }
                // 关系阶段门控：Friend(2) 及以上才允许心情驱动主动
                let psy = orchestrator.psychology.read();
                if let Some(p) = psy.as_ref() {
                    if p.get_proactivity_level() < 2 {
                        return false;
                    }
                    let behavior = crate::character_behavior::get_behavior(&orchestrator.char_id);
                    let needs = p.needs();
                    let emotion = p.emotion();
                    let need_pressure = needs
                        .belonging
                        .max(needs.autonomy)
                        .max(needs.security)
                        .max(needs.novelty)
                        .max(needs.expression);
                    let loneliness = emotion.loneliness;
                    let curiosity = emotion.curiosity;
                    need_pressure > behavior.mood_driven_need_threshold
                        || loneliness > behavior.mood_driven_loneliness_threshold
                        || curiosity > behavior.mood_driven_need_threshold
                } else {
                    false
                }
            }
            ProactiveTrigger::CrossCharacterReply => {
                // 室友在线 + 最近 90s 内主动发言过 → 本角色有概率回应/搭话
                // （概率已在 check_trigger 中 roll 过，这里只检查"事件条件"）
                let companion = orchestrator.companions_snapshot.read();
                match companion.as_ref() {
                    Some(c) => match c.last_spoke_secs_ago {
                        // 室友最近 90s 内发言过：正常触发条件
                        Some(secs) => secs <= 90.0,
                        // 冷启动破冰：室友在线但从未发言过（last_spoke_secs_ago=None）
                        // 两个角色都等对方先说话会死锁，以低概率触发一次破冰
                        None => roll_with_probability(0.20),
                    },
                    None => false,
                }
            }
            ProactiveTrigger::BystanderInterjection => {
                // 检查是否有最近旁观记忆
                let memory = orchestrator.memory.read();
                let mem = match memory.as_ref() {
                    Some(m) => m,
                    None => return false,
                };
                let bystander_memos = mem.recent_by_tags(&["bystander", "overheard"], 1);
                if bystander_memos.is_empty() {
                    return false;
                }
                let memo_ts = bystander_memos[0].timestamp;
                let secs_since_heard = ctx.now - memo_ts;
                // 旁观记忆太旧（超过 60s）不插话
                if secs_since_heard > 60.0 {
                    return false;
                }
                // 本角色在旁观记忆之后发言过则跳过（已经插过话或自己说了别的）
                if let Some(ago) = crate::commands::proactive::last_spoken_ago(&orchestrator.char_id) {
                    if ago < secs_since_heard {
                        return false;
                    }
                }
                // 时机衰减（对话越久越不可能插话）
                let timing_factor = if secs_since_heard < 15.0 { 1.0 }
                    else if secs_since_heard < 30.0 { 0.7 }
                    else if secs_since_heard < 45.0 { 0.4 }
                    else { 0.15 };
                roll_with_probability(timing_factor)
            }
        }
    }

    fn check_health_reminder(
        &self,
        ctx: &TickContext,
        state: &ProactiveState,
        hour: u32,
        minute: u32,
    ) -> bool {
        HealthReminder::check_all(
            state.sustained_active_minutes,
            &state.last_reminder_times,
            None,
            ctx.now,
            hour,
            minute,
        )
        .is_some()
    }

    /// 计算综合冷却系数（加权几何平均 + 后乘修正）
    ///
    /// 旧方案（纯乘积）问题：单个低因子（如深夜 0.3）会压垮所有其他因子。
    /// 新方案（加权几何平均）：`exp(Σ w_i * ln(factor_i))`，低因子只按权重比例拉低结果。
    /// Behavior mode 和 ignored 惩罚仍作为后乘修正（它们代表全局模式决策，应强效）。
    ///
    /// 策略 E：emotion_mult 额外乘以情绪浮动周期因子，
    /// 让两个角色的冷却曲线形状不同（Vivian 锯齿 vs Nana 缓坡），自然错开。
    fn compute_overall_cooling(&self, ctx: &TickContext, hour: u32, state: &ProactiveState) -> f64 {
        // 情绪冷却
        let emotion_mult: f64 = {
            let stress_level = self.stress_monitor.read().get_stress_level();
            let base = match ctx.user_emotion.as_str() {
                "sad" | "anxious" | "angry" => 0.3,
                "tired" | "frustrated" => 0.6,
                "excited" | "happy" => 1.2,
                _ => match stress_level {
                    StressLevel::High => 0.5,
                    StressLevel::Medium => 0.7,
                    StressLevel::Low => 1.0,
                },
            };
            // 策略 E：情绪浮动乘数
            // sin(phase) ∈ [-1, 1]，乘以 volatility 得到周期性偏移，
            // 再加 base_valence 作为静态偏移。最终 clamp 到 [0.3, 1.5] 避免极端值。
            let drift_cfg = crate::character_behavior::get_behavior(&self.char_id).mood_drift;
            let phase = *self.mood_drift_phase.read();
            let oscillation = phase.sin() * drift_cfg.volatility;
            let drift_factor = (drift_cfg.base_valence + oscillation).clamp(0.3, 1.5);
            base * drift_factor
        };

        // 时间冷却：深夜/清晨降低打扰
        let time_mult: f64 = if hour < 7 || hour >= 23 {
            0.3
        } else if hour < 9 || hour >= 22 {
            0.6
        } else if (12..14).contains(&hour) {
            0.7
        } else {
            1.0
        };

        // 活动冷却：根据应用分类（SmartAppClassifier 9 分类）
        let activity_mult: f64 = {
            let app_cat = self.last_app_category.read();
            match app_cat.as_str() {
                "game" | "video" | "media" => 0.5,   // 娱乐中，较易打扰
                "coding" | "office" => 0.7,            // 工作/编程中，谨慎打扰
                "browser" | "chat" => 0.75,            // 轻度浏览/社交，可适度打扰
                _ => 0.9,                              // utility/other，正常打扰
            }
        };

        // 亲密度冷却（从心理学系统读取，关系系统已整合到 PsychologyManager）
        let intimacy_mult = {
            let psy = self.psychology.read();
            if let Some(p) = psy.as_ref() {
                let behavior = crate::character_behavior::get_behavior(&self.char_id);
                let intimacy = p.relationship().intimacy * 100.0;
                let base = if intimacy < 20.0 {
                    0.4
                } else if intimacy < 40.0 {
                    0.7
                } else if intimacy < 60.0 {
                    1.0
                } else if intimacy < 80.0 {
                    1.15
                } else {
                    1.3
                };
                base * behavior.intimacy_cooldown_multiplier
            } else {
                1.0
            }
        };

        // 加权几何平均：exp(w_e*ln(e) + w_t*ln(t) + w_a*ln(a) + w_i*ln(i))
        // 权重：时间最重(0.30)（深夜不应打扰），情绪次之(0.25)，亲密度(0.25)，活动(0.20)
        let e = emotion_mult.max(0.01);
        let t = time_mult.max(0.01);
        let a = activity_mult.max(0.01);
        let i = intimacy_mult.max(0.01);
        let weighted_log_sum = 0.25 * e.ln() + 0.30 * t.ln() + 0.20 * a.ln() + 0.25 * i.ln();
        let mut result = weighted_log_sum.exp();

        // 后乘修正：行为模式冷却（影随大幅降低）
        result *= self.behavior_mode.get_current_mode().cooling_multiplier();
        // 被忽略次数平滑衰减：每次忽略 cooling 衰减 15%，最低保留 10%
        // 0次=1.00, 1次=0.85, 2次=0.70, 3次=0.55, 4次=0.40, 5次=0.25, 6+次=0.10
        // 比"2次直接减半"的硬阈值更平滑，让用户感受到频率逐渐降低而非断崖式
        let ignored_decay = (1.0 - 0.15 * state.ignored_count as f64).max(0.10);
        result *= ignored_decay;
        result
    }

    /// 生成主动交互内容
    ///
    /// 通过 LLM 生成主动对话内容。
    ///
    /// LLM 不可用/调用失败/解析失败 → 返回 None（不生成消息，无模板回退）。
    /// 这样保证主动对话内容始终由 LLM 生成，避免机械化的模板词。
    fn generate_content(
        &self,
        trigger: ProactiveTrigger,
        ctx: &TickContext,
        hour: u32,
        _minute: u32,
    ) -> Option<BehaviorContent> {
        let router = self.model_router.read().clone()?;
        self.try_llm_content(trigger, ctx, hour, &router)
    }

    /// 流式调用 LLM 并解析 JSON，实时推送 text 增量到前端
    ///
    /// 复用 `StreamingJsonParser` 提取 `text` 字段增量，通过 `stream_emitter` 推送。
    /// 流式结束后从完整响应解析 JSON 获取 `expression`。
    /// 兜底：LLM 输出非 JSON 时，parser 未提取出 text，推送整个 raw buf。
    async fn stream_query_and_parse(
        router: &ModelRouter,
        messages: Vec<ChatMessage>,
        emitter: &SharedStreamEmitter,
    ) -> Option<String> {
        let mut rx = match router
            .generate_stream(LLMRequest::new("chat", messages).with_stream(true))
            .await
        {
            Ok(rx) => rx,
            Err(e) => {
                tracing::debug!("[Proactive] stream LLM 查询失败，跳过本次主动交互: {}", e);
                return None;
            }
        };
        let mut parser = StreamingJsonParser::new();
        let mut buf = String::new();
        let mut any_text_emitted = false;

        while let Some(chunk) = rx.recv().await {
            buf.push_str(&chunk);
            let events = parser.feed(&chunk);
            let emitter_guard = emitter.read();
            if let Some(emitter_fn) = emitter_guard.as_ref() {
                for ev in events {
                    if let StreamEvent::TextChunk(text) = ev {
                        any_text_emitted = true;
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            emitter_fn(&text);
                        }));
                    }
                }
            }
        }

        // 兜底：LLM 输出非 JSON，parser 未提取出 text，先尝试解析完整 JSON
        if !any_text_emitted && !buf.is_empty() {
            if let Some(content) = Self::parse_proactive_json(&buf) {
                let emitter_guard = emitter.read();
                if let Some(emitter_fn) = emitter_guard.as_ref() {
                    let text = content.text.clone();
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        emitter_fn(&text);
                    }));
                }
            }
        }

        if buf.is_empty() {
            None
        } else {
            Some(buf)
        }
    }

    /// 从完整 LLM 响应文本解析 BehaviorContent（含扩展字段）
    fn parse_proactive_json(raw: &str) -> Option<BehaviorContent> {
        let text = raw.trim();
        let start = text.find('{');
        let end = text.rfind('}');
        if let (Some(s), Some(e)) = (start, end) {
            if e >= s {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text[s..=e]) {
                    if let Some(t) = data.get("text").and_then(|v| v.as_str()) {
                        let text_owned: String = t.chars().take(60).collect();
                        if !text_owned.trim().is_empty() {
                            let expression = data
                                .get("expression")
                                .and_then(|v| v.as_str())
                                .unwrap_or("shy")
                                .to_string();
                            let (delivery_channel, content_type, importance, value_score) =
                                BehaviorContent::parse_extra_fields(&data);
                            return Some(BehaviorContent {
                                text: text_owned,
                                expression,
                                delivery_channel,
                                content_type,
                                importance,
                                value_score,
                            });
                        }
                    }
                }
            }
        }
        None
    }

    /// 尝试 LLM 生成主动交互内容
    ///
    /// 由于 `tick` 是同步函数但运行在 tauri 异步命令的 tokio 运行时中，
    /// 使用 `tokio::task::block_in_place` + `Handle::block_on` 调用异步 LLM 方法。
    /// 仅对支持的触发器构造请求；返回 `None` 时由 `generate_content` 跳过本次交互。
    fn try_llm_content(
        &self,
        trigger: ProactiveTrigger,
        ctx: &TickContext,
        hour: u32,
        router: &Arc<ModelRouter>,
    ) -> Option<BehaviorContent> {
        // 仅对提供了 prompt 的触发器尝试 LLM
        if !matches!(
            trigger,
            ProactiveTrigger::HourlyGreeting
                | ProactiveTrigger::IdleGreeting
                | ProactiveTrigger::TeasingResponse
                | ProactiveTrigger::Spontaneous
                | ProactiveTrigger::Icebreaker
                | ProactiveTrigger::MemoryRecall
                | ProactiveTrigger::WindowTrigger
                | ProactiveTrigger::WelcomeBack
                | ProactiveTrigger::HealthReminder
                | ProactiveTrigger::TopicExtension
                | ProactiveTrigger::MoodDriven
                | ProactiveTrigger::CrossCharacterReply
                | ProactiveTrigger::BystanderInterjection
        ) {
            return None;
        }

        // 非运行时上下文（如单元测试）直接降级，避免 panic
        let handle = tokio::runtime::Handle::try_current().ok()?;

        // 预先读取字段，避免在异步块中持有锁
        let mut mem = self.recent_memory.read().clone();
        let mind_state = self.get_mind_state().as_str().to_string();
        let prompt_step_opt = self.prompt_step.read().clone();
        let tool_system_opt = self.tool_system.read().clone();
        let memory_arc = self.memory.read().clone();

        // 读取未闭环 open_hooks，追加到 memory_hint 让 MemoryRecall 能主动追问
        if let Some(memory) = self.memory.read().as_ref() {
            let hook_items = memory.get_memories_with_open_hooks();
            if !hook_items.is_empty() {
                let hooks_text = hook_items
                    .iter()
                    .take(3)
                    .filter_map(|m| {
                        let first_open = m.open_hooks.iter().find(|h| h.is_open())?;
                        Some(format!(
                            "[未闭环·{}] {}（条件：{}）",
                            first_open.hook_type, m.content, first_open.condition
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !hooks_text.is_empty() {
                    if !mem.is_empty() {
                        mem.push('\n');
                    }
                    mem.push_str(&hooks_text);
                }
            }
        }

        // BystanderInterjection：若被室友 cue，追加提示让插话更自然
        if matches!(trigger, ProactiveTrigger::BystanderInterjection) {
            if let Some((from_name, topic_brief)) = self.check_roommate_cue() {
                let cue_hint = if topic_brief.is_empty() {
                    format!("\n\n## 室友刚 cue 了你\n{} 刚在和用户聊天时提到了你，你可以顺势插句话加入对话。", from_name)
                } else {
                    format!("\n\n## 室友刚 cue 了你\n{} 刚在和用户聊「{}」时提到了你，你可以顺势插句话加入对话。", from_name, topic_brief)
                };
                mem.push_str(&cue_hint);
            }
        }

        // 构造 system prompt：优先使用 PersonaEngine.build_style_prompt（带亲密度+时段），
        // 未注入时为空字符串（decide_content_llm 等会回退到 default_persona_prompt 兜底）
        let (system_prompt, intimacy, dialogue_history, lang) = {
            let persona = self.persona.read().clone();
            let psychology = self.psychology.read().clone();
            let dialogue = self.dialogue.read().clone();
            let intimacy = psychology
                .as_ref()
                .map(|p| p.relationship().intimacy * 100.0)
                .unwrap_or(50.0);
            let lang = persona
                .as_ref()
                .map(|p| p.get_language())
                .unwrap_or_else(|| "zh".to_string());
            let sys = persona
                .as_ref()
                .map(|p| p.build_style_prompt(intimacy, hour))
                .unwrap_or_default();
            // 取最近 6 条对话历史，格式化为 "role: content"
            let history = dialogue
                .as_ref()
                .map(|d| {
                    d.get_history()
                        .into_iter()
                        .rev()
                        .take(6)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .map(|m| format!("{}: {}", m.role, m.content))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            (sys, intimacy, history, lang)
        };

        let llm_ctx = behavior::LlmContext {
            hour,
            idle_seconds: ctx.idle_seconds,
            drag_distance: ctx.drag_distance,
            mind_state,
            memory_hint: mem.clone(),
            mood_hint: ctx.user_emotion.clone(),
            dialogue_history,
            intimacy,
            away_seconds: ctx.away_seconds,
            active_window: self.last_app_category.read().clone(),
            sustained_active_minutes: self.state.read().sustained_active_minutes,
            minute: chrono::Local::now().format("%M").to_string().parse::<u32>().unwrap_or(0),
            online_companions: self.format_companions_for_prompt(),
        };
        let router_clone = router.clone();
        let idle_seconds = ctx.idle_seconds;
        let system_prompt_clone = system_prompt;
        let lang_clone = lang;
        let emitter = self.stream_emitter.clone();

        tokio::task::block_in_place(|| {
            handle.block_on(async move {
                // 记忆检索（含知识库：busy 状态网络搜索获得的信息），让主动问候有真实素材
                let memory_text = if let Some(mem_mgr) = memory_arc.as_ref() {
                    match mem_mgr
                        .search_memories(
                            "最近 用户 兴趣 话题 知识",
                            crate::memory::types::RetrievalStrategy::Hybrid,
                            8,
                        )
                        .await
                    {
                        Ok(items) if !items.is_empty() => items
                            .iter()
                            .map(|m| {
                                let imp = (m.importance * 100.0) as u32;
                                format!("- {}（重要性:{}%）", m.content, imp)
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };
                // 真实工具调用历史（让 AI 只能提及真实做过的操作，禁止编造看了番剧/刷了视频等）
                let tool_history = tool_system_opt
                    .as_ref()
                    .map(|ts| behavior::format_recent_tool_history(ts, &lang_clone))
                    .unwrap_or_default();
                // 统一构造 messages，然后流式调用 LLM
                let messages = match trigger {
                    ProactiveTrigger::HourlyGreeting
                    | ProactiveTrigger::IdleGreeting
                    | ProactiveTrigger::TeasingResponse
                    | ProactiveTrigger::Spontaneous
                    | ProactiveTrigger::WindowTrigger
                    | ProactiveTrigger::WelcomeBack
                    | ProactiveTrigger::HealthReminder
                    | ProactiveTrigger::TopicExtension
                    | ProactiveTrigger::MoodDriven
                    | ProactiveTrigger::CrossCharacterReply
                    | ProactiveTrigger::BystanderInterjection => {
                        BehaviorDecider::build_messages(
                            trigger,
                            &llm_ctx,
                            &system_prompt_clone,
                            &lang_clone,
                            &self.char_id,
                            prompt_step_opt.as_ref(),
                            &memory_text,
                            &tool_history,
                        )?
                    }
                    ProactiveTrigger::Icebreaker => {
                        let level = IceBreakerLevel::from_idle(idle_seconds);
                        IcebreakerGenerator::build_messages(
                            level,
                            Some(&mem),
                            hour,
                            &system_prompt_clone,
                            &llm_ctx.dialogue_history,
                            &lang_clone,
                            &self.char_id,
                        )?
                    }
                    ProactiveTrigger::MemoryRecall => {
                        MemoryRecall::build_messages(&mem, &system_prompt_clone, &lang_clone, &self.char_id)?
                    }
                    #[allow(unreachable_patterns)]
                    _ => return None,
                };

                // 流式调用 LLM，实时推送 text 增量
                let raw = Self::stream_query_and_parse(&router_clone, messages, &emitter).await?;

                // BystanderInterjection 走严格 JSON 解析：text 为空表示不插话
                if matches!(trigger, ProactiveTrigger::BystanderInterjection) {
                    if let Some(content) = Self::parse_proactive_json(&raw) {
                        return Some(content);
                    }
                    return None;
                }

                // 解析完整 JSON 获取 BehaviorContent（含扩展字段）
                Self::parse_proactive_json(&raw)
            })
        })
    }

    fn push_message(&self, trigger: ProactiveTrigger, content: String, now: f64) {
        let action = ProactiveAction::from_trigger(trigger, content, now);
        self.push_action(action, trigger);
    }

    /// 推送完整 ProactiveAction（含 delivery_channel / content_type / value_score）
    /// LLM 路径产出 BehaviorContent 后转 ProactiveAction 走此入口
    fn push_action(&self, action: ProactiveAction, trigger: ProactiveTrigger) {
        let channel = action.delivery_channel;
        let content_for_dedup = action.content.clone();
        // 内容+渠道级去重：避免相同文本在同一渠道被多个触发器或跨 tick 重复推送
        {
            let msgs = self.pending_messages.read();
            if msgs
                .iter()
                .any(|m| m.content == content_for_dedup && m.delivery_channel == channel)
            {
                tracing::debug!(
                    "[proactive] 跳过重复消息（已在队列中）: trigger={}, channel={:?}",
                    trigger.as_str(),
                    channel
                );
                return;
            }
        }
        {
            let recent = self.recent_sent_contents.read();
            if recent
                .iter()
                .any(|(c, ch)| c == &content_for_dedup && *ch == channel)
            {
                tracing::debug!(
                    "[proactive] 跳过重复消息（近期已发送）: trigger={}, channel={:?}",
                    trigger.as_str(),
                    channel
                );
                return;
            }
        }

        let mut msgs = self.pending_messages.write();
        msgs.push(action);
        if msgs.len() > 10 {
            msgs.remove(0);
        }
        // 记录到近期发送缓冲区（保留最近 5 条，防止跨 tick 重复）
        {
            let mut recent = self.recent_sent_contents.write();
            recent.push((content_for_dedup, channel));
            if recent.len() > 5 {
                recent.remove(0);
            }
        }
        // 偏好学习：记录触发器已触发，后续 on_user_interacted/on_ignored 归因响应
        self.preference_learner.record_trigger_fired(trigger);
        // 策略 C：greeting 类触发器成功说话后将欲望重置到 initial_desire
        // （非 greeting 触发器不重置，避免 MoodDriven/Spontaneous 等意外重定时问候门控）
        {
            let is_greeting = matches!(
                trigger,
                ProactiveTrigger::WelcomeBack
                    | ProactiveTrigger::HourlyGreeting
                    | ProactiveTrigger::IdleGreeting
                    | ProactiveTrigger::Icebreaker
            );
            let behavior = crate::character_behavior::get_behavior(&self.char_id);
            if is_greeting && behavior.speech_desire.reset_on_speak {
                *self.speech_desire.write() = behavior.speech_desire.initial_desire;
            }
        }
        tracing::info!("主动消息入队: {}", trigger.as_str());
    }

    fn update_trigger_time(&self, trigger: ProactiveTrigger, now: f64, hour: u32, minute: u32) {
        let mut state = self.state.write();
        state
            .last_trigger_times
            .insert(trigger.as_str().to_string(), now);
        state.last_interaction_time = now;
        if trigger == ProactiveTrigger::HourlyGreeting {
            state.last_hour_greeted = hour as i32;
        }
        if trigger == ProactiveTrigger::HealthReminder {
            // 按当前时段记录对应提醒类型
            let kind = if hour >= 22 || hour < 6 {
                "sleep"
            } else if (11..14).contains(&hour) && minute < 30 {
                "meal:午餐"
            } else if (17..20).contains(&hour) && minute < 30 {
                "meal:晚餐"
            } else if (6..9).contains(&hour) && minute < 30 {
                "meal:早餐"
            } else {
                "water"
            };
            state.last_reminder_times.insert(kind.to_string(), now);
            // 休息提醒触发后重置活跃计时
            if kind == "rest" {
                state.sustained_active_minutes = 0;
            }
        }
        if trigger == ProactiveTrigger::WelcomeBack {
            *self.last_user_was_away.write() = false;
        }
    }

    /// 记录一次到达问候（启动问候 / 唤醒问候），使其纳入主动问候的共享冷却。
    ///
    /// 这两类问候由 Brain 在应用就绪或睡眠唤醒时生成，不走 tick 触发循环；
    /// 但若不计入冷却，TimingJudger 会认为"最近没有打扰"，导致 WelcomeBack /
    /// HourlyGreeting 等主动问候在问候后很快再次触发。这里把问候写入共享冷却状态：
    /// - `last_interaction_time`：全局打扰时间戳，TimingJudger 的冷却分量据此压分
    /// - `last_trigger_times[kind]`：纳入"最近打扰"集合，供频率统计与冷却判断
    /// - `last_user_was_away=false`：用户刚到场，避免 WelcomeBack 立即叠加
    pub fn record_greeting_arrival(&self, kind: &str) {
        let now = chrono::Local::now().timestamp() as f64;
        {
            let mut state = self.state.write();
            state.last_interaction_time = now;
            state
                .last_trigger_times
                .insert(kind.to_string(), now);
        }
        *self.last_user_was_away.write() = false;
    }

    /// 心理状态机（融合情绪信号）
    ///
    /// `drive` 由调用方预先计算并传入（tick 内复用，避免重复推导）。
    fn update_mind_state(&self, ctx: &TickContext, hour: u32, drive: Option<&BehaviorDrive>) {
        // 优先使用 Behavior Drive 主导项决定心理状态（混合模式 - 规则路径）
        let drive_based: Option<PetMindState> = drive.and_then(|d| {
            let (label, value) = d.dominant();
            // 仅当主导 drive 强度足够时才覆盖时间/活动基线
            if value < 0.35 {
                return None;
            }
            Some(match label {
                DriveLabel::Approach => PetMindState::Curious,
                DriveLabel::Avoid => PetMindState::Sleepy,
                DriveLabel::Explore => PetMindState::Curious,
                DriveLabel::Express => PetMindState::Excited,
                DriveLabel::Rest => PetMindState::Sleepy,
                DriveLabel::Observe => PetMindState::Content,
                DriveLabel::Play => PetMindState::Excited,
                DriveLabel::Help => PetMindState::Caring,
            })
        });

        let new_state = if let Some(state) = drive_based {
            state
        } else {
            // 回退到原逻辑：时间段 + 活动 + 情绪
            // 作息参数从 WorldConfig 读取（可配置，而非写死）
            let (sleep_start, sleep_end) = self
                .world_provider
                .read()
                .as_ref()
                .map(|wp| {
                    let cfg = wp.config();
                    (cfg.sleep_start_hour, cfg.sleep_end_hour)
                })
                .unwrap_or((1, 6));

            let base = if hour_in_window(hour, sleep_start, sleep_end) {
                // 睡眠窗口内：困倦入睡（与睡前/午休统一为 Sleepy，由 Rest 在场状态承载闭眼）
                PetMindState::Sleepy
            } else if hour_in_window(hour, (sleep_start + 24 - 2) % 24, sleep_start) {
                // 睡前 2 小时：困倦
                PetMindState::Sleepy
            } else if (12..14).contains(&hour) {
                // 午休时段：困倦
                PetMindState::Sleepy
            } else if (7..9).contains(&hour) {
                PetMindState::Curious
            } else if (17..19).contains(&hour) {
                PetMindState::Excited
            } else if ctx.idle_seconds < 60.0 {
                PetMindState::Curious
            } else if ctx.idle_seconds > 1800.0 {
                PetMindState::Bored
            } else {
                PetMindState::Content
            };
            match ctx.user_emotion.as_str() {
                "sad" | "anxious" => PetMindState::Caring,
                "excited" | "happy" => PetMindState::Excited,
                _ => base,
            }
        };

        let mut state = self.state.write();
        if new_state.as_str() != state.mind_state {
            tracing::info!(
                "心理状态变化: {} -> {}",
                state.mind_state,
                new_state.as_str()
            );
            state.mind_state = new_state.as_str().to_string();
        }
    }

    /// 消费所有待发送消息，丢弃超过有效期 的条目
    pub fn drain_messages(&self) -> Vec<ProactiveAction> {
        let mut msgs = self.pending_messages.write();
        let now_ts = chrono::Local::now().timestamp() as f64;
        msgs.drain(..)
            .filter(|m| {
                let age = now_ts - m.timestamp;
                if age > PROACTIVE_MSG_TTL_SECS {
                    tracing::info!(
                        "[Proactive] 丢弃过时主动消息（age={:.0}s）：{}",
                        age,
                        crate::utils::truncate_chars(&m.content, 40)
                    );
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>()
    }

    /// 将未投递的消息重新放回队列头部（播放冲突等场景延迟投递）
    pub fn requeue_messages(&self, messages: Vec<ProactiveAction>) {
        let mut msgs = self.pending_messages.write();
        let mut combined = messages;
        combined.extend(msgs.drain(..));
        combined.truncate(10);
        *msgs = combined;
    }

    /// 用户互动后调用，重置忽略计数
    ///
    /// 如果之前有被忽略的记录（ignored_count > 0），说明用户是在她主动搭话后回应的——
    /// 给 intimacy 一个微小的正向反馈，让她更愿意主动。
    pub fn on_user_interacted(&self) -> VivianResult<()> {
        let had_ignored = {
            let mut state = self.state.write();
            let was_ignored = state.ignored_count > 0;
            state.ignored_count = 0;
            state.last_interaction_time = chrono::Local::now().timestamp() as f64;
            *self.last_user_was_away.write() = false;
            was_ignored
        };
        // 偏好学习：用户响应了上一个主动消息（正信号）
        self.preference_learner.record_response(true);
        if had_ignored {
            if let Some(psy) = self.psychology.read().as_ref() {
                if let Err(e) = psy.apply_proactive_feedback(true, &self.char_id) {
                    tracing::warn!("[Proactive] apply_proactive_feedback(true) 失败: {}", e);
                }
            }
        }
        self.save_to()?;
        Ok(())
    }

    /// 获取当前被忽略次数（供 PresenceManager 检查自动触发条件）
    pub fn get_ignored_count(&self) -> u32 {
        self.state.read().ignored_count
    }

    /// 标记本次主动消息被忽略
    ///
    /// 每次冷落都给 intimacy 一个微小的负向反馈，让她逐渐退缩。
    /// 同时关闭 User↔Agent 会话（NoResponse），让 Session 状态机感知到
    /// "主动搭话被忽略"这一事实，后续不再继续搭话直到新 Trigger。
    pub fn on_ignored(&self) -> VivianResult<()> {
        let behavior = crate::character_behavior::get_behavior(&self.char_id);
        {
            let mut state = self.state.write();
            state.ignored_count += 1;
            if state.ignored_count >= behavior.quiet_mode_threshold {
                state.quiet_mode = true;
                state.quiet_mode_until =
                    chrono::Local::now().timestamp() as f64 + 3600.0;
                tracing::info!(
                    "连续被忽略 {} 次，进入 1 小时安静模式",
                    behavior.quiet_mode_threshold
                );
            }
        }
        // 偏好学习：用户忽略了上一个主动消息（负信号）
        self.preference_learner.record_response(false);
        if let Some(psy) = self.psychology.read().as_ref() {
            if let Err(e) = psy.apply_proactive_feedback(false, &self.char_id) {
                tracing::warn!("[Proactive] apply_proactive_feedback(false) 失败: {}", e);
            }
        }

        // 关闭 User↔Agent 会话（NoResponse）
        // 让 Session 状态机记录"主动搭话被忽略"，后续 proactive_tick 会据此跳过主动消息。
        crate::conversation::CONVERSATION_MANAGER.close_pair_with_reason(
            "user",
            &self.char_id,
            crate::conversation::CloseReason::NoResponse,
        );

        self.save_to()?;
        Ok(())
    }

    /// 记录用户开机（每日首次）
    pub fn record_startup(&self) {
        self.habit_tracker.record_startup();
    }

    /// 记录应用使用
    pub fn record_app_usage(&self, app_name: &str) {
        self.habit_tracker.record_app_usage(app_name, 60.0);
        let category = self.app_classifier.classify(app_name);
        *self.last_app_category.write() = category;
    }

    /// 获取习惯感知提示词（供 prompt 注入）
    pub fn get_habit_prompt(&self) -> String {
        self.habit_tracker.get_habit_prompt()
    }

    /// 获取当前行为模式
    pub fn get_behavior_mode(&self) -> PetBehaviorMode {
        self.behavior_mode.get_current_mode()
    }

    /// 获取当前行为模式配置
    pub fn get_behavior_mode_config(&self) -> serde_json::Value {
        self.behavior_mode.get_mode_config()
    }

    /// 获取当前前台窗口应用分类（SmartAppClassifier 最近一次分类结果）
    ///
    /// 用于与 LLM 活动提取结果做交叉验证：
    /// 用户说"我去学习"但前台是 "game" → 记录诊断日志。
    pub fn get_current_app_category(&self) -> String {
        self.last_app_category.read().clone()
    }

    /// 获取时机评分详情
    pub fn get_timing_explain(&self, ctx: &TickContext) -> serde_json::Value {
        let state = self.state.read();
        let last_interruption = state
            .last_trigger_times
            .values()
            .cloned()
            .fold(0.0_f64, f64::max);
        let interruption_count = state
            .last_trigger_times
            .values()
            .filter(|t| ctx.now - **t < 3600.0)
            .count() as u32;
        let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
        TimingJudger::explain(ctx, last_interruption, interruption_count, hour)
    }

    /// 获取状态摘要
    pub fn get_status(&self) -> serde_json::Value {
        let state = self.state.read();
        serde_json::json!({
            "running": self.is_running(),
            "mind_state": state.mind_state,
            "quiet_mode": state.quiet_mode,
            "quiet_mode_until": state.quiet_mode_until,
            "ignored_count": state.ignored_count,
            "last_interaction_time": state.last_interaction_time,
            "pending_count": self.pending_messages.read().len(),
            "behavior_mode": self.behavior_mode.get_current_mode().as_str(),
            "behavior_mode_config": self.behavior_mode.get_mode_config(),
            "habit_summary": self.habit_tracker.get_habit_summary(),
            "topic_pool": self.topic_pool.status(),
            "stress_report": self.stress_monitor.read().get_stress_report(),
            "sustained_active_minutes": state.sustained_active_minutes,
        })
    }
}

/// 单次 tick 的上下文（11 字段，与命令层契约对齐，不可破坏）
#[derive(Debug, Clone, Default)]
pub struct TickContext {
    /// 当前时间戳（Unix 秒）
    pub now: f64,
    /// 用户空闲秒数
    pub idle_seconds: f64,
    /// 用户离开秒数（已回来）
    pub away_seconds: f64,
    /// 用户是否在场
    pub user_present: bool,
    /// 今日交互次数
    pub interaction_count_today: u32,
    /// 活动窗口标题
    pub active_window: String,
    /// 窗口是否变化
    pub window_changed: bool,
    /// 上次话题是否相关
    pub last_topic_relevant: bool,
    /// 是否有相关记忆
    pub has_relevant_memory: bool,
    /// 拖拽距离
    pub drag_distance: f64,
    /// 用户情绪
    pub user_emotion: String,
    /// SelfState 防打扰决策：true 时跳过主动消息触发（quiet_mode/上限/Rest/Offline）
    ///
    /// 由命令层从 `brain.self_state.snapshot().should_lay_low()` 注入。
    /// true 时本 tick 仅执行后台维护（homeostasis/窗口轮询/在场检查/世界事件），
    /// 不产生用户可见的主动消息；内心独白与特殊日期问候一并跳过。
    pub lay_low: bool,
    /// 用户是否正在与任意角色进行活跃对话
    ///
    /// 由命令层从 `CONVERSATION_MANAGER.is_any_user_session_active()` 注入。
    /// true 时抑制 TopicExtension / CrossCharacterReply 等打断性触发器，
    /// 避免主动消息打断正在进行的用户↔角色对话。
    pub is_user_chatting: bool,
    /// 当前角色是否持有发言权（proactive leader）
    ///
    /// 由命令层从 `ProactiveLeaderCoordinator::try_acquire_or_renew` 注入。
    /// false 时跳过触发器评估与消息生成，仅执行后台状态维护
    /// （homeostasis / 窗口轮询 / 在场检查 / 世界事件 / 内心独白）。
    pub is_speaking_leader: bool,
}

impl Default for ProactiveOrchestrator {
    fn default() -> Self {
        Self::new("vivian").unwrap_or_else(|e| {
            tracing::error!("主动对话初始化失败，使用内存模式: {e}");
            let now_ts = chrono::Local::now().timestamp() as f64;
            ProactiveOrchestrator {
                state: Arc::new(RwLock::new(ProactiveState {
                    mind_state: PetMindState::Curious.as_str().to_string(),
                    last_interaction_time: now_ts,
                    last_activity_check: now_ts,
                    ..Default::default()
                })),
                running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                persistence_path: std::path::PathBuf::from("state.json"),
                last_tick: Arc::new(RwLock::new(Instant::now())),
                pending_messages: Arc::new(RwLock::new(Vec::new())),
                recent_sent_contents: RwLock::new(Vec::new()),
                topic_pool: TopicPool::default(),
                habit_tracker: HabitTracker::default(),
                behavior_mode: BehaviorModeManager::new(now_ts),
                stress_monitor: RwLock::new(StressMonitor::new()),
                recent_memory: RwLock::new(String::new()),
                last_app_category: RwLock::new(String::new()),
                last_user_active: RwLock::new(true),
                last_user_was_away: RwLock::new(false),
                model_router: RwLock::new(None),
                psychology: RwLock::new(None),
                config: RwLock::new(ProactiveConfig::default()),
                persona: RwLock::new(None),
                dialogue: RwLock::new(None),
                memory: RwLock::new(None),
                world_provider: RwLock::new(None),
                mind: RwLock::new(None),
                event_detector: RwLock::new(crate::world::WorldEventDetector::new()),
                thought_trigger_evaluator: RwLock::new(thought_trigger::ThoughtTriggerEvaluator::new()),
                detected_world_events: RwLock::new(Vec::new()),
                activity_journal: Arc::new(ActivityJournal::new()),
                app_classifier: SmartAppClassifier::new(),
                stream_emitter: new_shared_stream_emitter(),
                preference_learner: TriggerPreferenceLearner::default(),
                char_id: "vivian".to_string(),
                companions_snapshot: Arc::new(RwLock::new(None)),
                speech_desire: RwLock::new(
                    crate::character_behavior::get_behavior("vivian").speech_desire.initial_desire,
                ),
                mood_drift_phase: RwLock::new(
                    crate::character_behavior::get_behavior("vivian").mood_drift.initial_phase,
                ),
                signal_going_to_rest: Arc::new(parking_lot::Mutex::new(None)),
                signal_waking_up: Arc::new(AtomicBool::new(false)),
                signal_knowledge_acquired: Arc::new(parking_lot::Mutex::new(Vec::new())),
                roommate_cue: Arc::new(parking_lot::Mutex::new(None)),
                thought_lifecycle: Arc::new(RwLock::new(ThoughtLifecycle::new())),
                prompt_step: RwLock::new(None),
                tool_system: RwLock::new(None),
                last_knowledge_acquisition_ts: Arc::new(parking_lot::Mutex::new(0.0)),
                last_knowledge_share_ts: Arc::new(parking_lot::Mutex::new(0.0)),
            }
        })
    }
}

use chrono::{Datelike, TimeZone};
