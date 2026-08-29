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
pub mod recap;
pub mod services;
pub mod timing;
pub mod topics;
pub mod triggers;
pub mod wakeup;

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
use crate::world::events::{WorldEvent, WorldEventKind};
use crate::world::SystemMetrics;
use crate::tools::builtin::system_ops::{capture_screen_png_bytes, describe_screen_bytes};
use crate::tools::confirmation::{ConfirmationResponse, ConfirmationRisk};
use crate::tools::executor::{is_session_allowed_tool, session_allow_tool};
use crate::utils::path::get_character_data_dir;

// ============ AppHandle 注入（日出/日落 toast 推送用） ============

static APP_HANDLE: once_cell::sync::Lazy<RwLock<Option<tauri::AppHandle>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次），供日出/日落提醒弹主题切换 toast
pub fn set_app_handle(handle: tauri::AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 前端上报的当前生效主题（"light"/"dark"；设置"跟随系统"时已按系统偏好解析）
static EFFECTIVE_THEME: once_cell::sync::Lazy<RwLock<Option<String>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

/// 前端上报当前生效主题（App 主窗口在启动/主题变化/系统深浅偏好变化时调用）
pub fn set_effective_theme(theme: &str) {
    let normalized = match theme {
        "light" | "dark" => theme.to_string(),
        _ => return,
    };
    *EFFECTIVE_THEME.write() = Some(normalized);
}

/// 当前生效主题：优先前端上报值（"跟随系统"已解析为实际深浅），
/// 未上报时回退 base.theme 的显式 light/dark 设置，仍未知返回 None。
/// 日出/日落提醒在建议切换主题前用它核对，避免"本来就用浅色还让用户改成浅色"。
pub fn current_effective_theme() -> Option<String> {
    if let Some(t) = EFFECTIVE_THEME.read().clone() {
        return Some(t);
    }
    let base = read_base_config("base.theme", "system");
    match base.as_str() {
        "light" | "dark" => Some(base),
        _ => None,
    }
}

/// 读取 base 段配置（base.theme / base.language 等），读取失败回退默认值
fn read_base_config(key: &str, default: &str) -> String {
    APP_HANDLE
        .read()
        .clone()
        .and_then(|handle| {
            use tauri::Manager;
            Some(
                handle
                    .state::<Arc<crate::state::AppState>>()
                    .config
                    .read()
                    .get(key)
                    .as_str()
                    .unwrap_or(default)
                    .to_string(),
            )
        })
        .unwrap_or_else(|| default.to_string())
}

/// 主题推荐 toast 文案（按界面语言返回）
fn theme_toast_message(is_sunrise: bool) -> &'static str {
    let lang = read_base_config("base.language", "zh-CN");
    let en = lang.starts_with("en");
    let ja = lang.starts_with("ja");
    if is_sunrise {
        if en {
            "The sun's up — consider switching to Light theme for easier eyes"
        } else if ja {
            "日の出です〜目に優しいライトテーマに切り替えてみては？"
        } else {
            "天亮了～建议切换到浅色主题，对眼睛更友好哦"
        }
    } else if en {
        "The sun's down — consider switching to Dark theme for a cozier night"
    } else if ja {
        "日没です〜夜はダークテーマのほうが目に優しいですよ"
    } else {
        "天黑了～建议切换到深色主题，晚上用更护眼"
    }
}

/// 内存压力提醒的触发阈值（内存占用百分比）
const MEMORY_PRESSURE_THRESHOLD_PCT: f32 = 85.0;

/// 用户拒绝主动截屏后的请求冷却（秒）：拒绝后 2 小时内不再发起请求
const SCREEN_PEEK_DENY_COOLDOWN_SECS: f64 = 7200.0;

/// 主动截屏请求的"先发言"气泡文案（按界面语言返回）
///
/// 未获得截屏权限时，角色先说这句话表达好奇，同时前端弹出确认 toast。
fn screen_peek_ask_message() -> &'static str {
    let lang = read_base_config("base.language", "zh-CN");
    let en = lang.starts_with("en");
    let ja = lang.starts_with("ja");
    if en {
        "Can I take a quick peek at your screen? I'm curious what you're up to~"
    } else if ja {
        "画面をちょっと覗いてもいい？今何してるか気になったの〜"
    } else {
        "可以让我看一眼你的屏幕吗？有点好奇你现在在忙什么～"
    }
}

/// 主动截屏确认 toast 的请求原因文案（按界面语言返回）
fn screen_peek_ask_reason(char_id: &str) -> String {
    let lang = read_base_config("base.language", "zh-CN");
    let en = lang.starts_with("en");
    let ja = lang.starts_with("ja");
    let char_name = crate::cross_character::display_name(char_id);
    if en {
        format!("{char_name} wants to capture your screen to see what you're busy with (a vision model will interpret it)")
    } else if ja {
        format!("{char_name}が画面をキャプチャして今の様子を見たがっています（視覚モデルで解析します）")
    } else {
        format!("{char_name} 想截个屏看看你在忙什么（会调用视觉模型理解屏幕内容）")
    }
}

/// 读取视觉功能开关（ai.enable_vision）
fn vision_enabled() -> bool {
    APP_HANDLE
        .read()
        .clone()
        .and_then(|handle| {
            use tauri::Manager;
            Some(
                handle
                    .state::<Arc<crate::state::AppState>>()
                    .config
                    .read()
                    .get_typed::<bool>("ai.enable_vision", false),
            )
        })
        .unwrap_or(false)
}

/// 读取视觉图片细节配置（ai.image_detail，默认 "auto"）
fn vision_image_detail() -> String {
    APP_HANDLE
        .read()
        .clone()
        .and_then(|handle| {
            use tauri::Manager;
            Some(
                handle
                    .state::<Arc<crate::state::AppState>>()
                    .config
                    .read()
                    .get_typed::<String>("ai.image_detail", "auto".to_string()),
            )
        })
        .unwrap_or_else(|| "auto".to_string())
}

/// 格式化系统指标摘要（注入 SystemPressure 触发器 prompt）
fn build_system_hint(m: &SystemMetrics) -> String {
    format!(
        "内存占用 {:.0}%（已用 {:.1}GB / 总量 {:.1}GB），CPU 占用 {:.0}%",
        m.memory_usage_pct,
        m.memory_used as f64 / 1024.0 / 1024.0 / 1024.0,
        m.memory_total as f64 / 1024.0 / 1024.0 / 1024.0,
        m.cpu_usage
    )
}

/// 应用类别 → 中文描述（AppDuration 触发器 prompt 注入用）
fn app_category_label_zh(cat: &str) -> &'static str {
    match cat {
        "coding" => "写代码/开发",
        "game" => "打游戏",
        "video" => "看视频",
        "browser" => "浏览网页",
        "chat" => "聊天",
        "office" => "办公",
        "media" => "处理媒体/听音乐",
        "utility" => "使用系统工具",
        _ => "使用某个应用",
    }
}

/// 应用类别 → 连续使用提醒阈值（秒）
fn app_duration_threshold_secs(cat: &str) -> f64 {
    match cat {
        // 高专注类别（写代码/办公）更早提醒，符合"久坐/长时间专注"关怀场景
        "coding" | "office" => 50.0 * 60.0,
        // 游戏/看视频画 75 分钟节点
        "game" | "video" => 75.0 * 60.0,
        // 浏览器/聊天/媒体多为混合使用，阈值放宽
        "browser" | "chat" | "media" => 90.0 * 60.0,
        _ => 90.0 * 60.0,
    }
}

/// 秒数 → 人类可读时长（"52 分钟" / "1 小时 12 分"）
fn format_duration_cn(secs: f64) -> String {
    let total_min = (secs / 60.0).round() as u32;
    if total_min < 60 {
        format!("{} 分钟", total_min)
    } else {
        format!("{} 小时 {} 分", total_min / 60, total_min % 60)
    }
}

/// 深夜未眠提醒的时间窗口：凌晨 1 点 至 4 点（含端点）
fn is_late_night_hour(hour: u32) -> bool {
    (1..=4).contains(&hour)
}

/// 主题推荐 toast 的确认按钮文案（按界面语言返回）
fn theme_toast_action_label(is_sunrise: bool) -> &'static str {
    let lang = read_base_config("base.language", "zh-CN");
    let en = lang.starts_with("en");
    let ja = lang.starts_with("ja");
    if is_sunrise {
        if en {
            "Switch to Light"
        } else if ja {
            "ライトに切替"
        } else {
            "换成浅色"
        }
    } else if en {
        "Switch to Dark"
    } else if ja {
        "ダークに切替"
    } else {
        "换成深色"
    }
}

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

/// 将秒数格式化为多语言时长字符串（如 "1小时23分钟" / "1h23min" / "1時間23分"）
///
/// 用于主动问候提示词中向 LLM 表达"距上次对话已过多久"，
/// 避免把旧事说成刚发生。`lang_norm` 取值："en" / "ja" / 其他（按中文处理）。
pub(crate) fn format_elapsed_lang(secs: f64, lang_norm: &str) -> String {
    let s = secs.max(0.0) as u64;
    if s < 60 {
        return match lang_norm {
            "en" => format!("{}s", s),
            "ja" => format!("{}秒", s),
            _ => format!("{}秒", s),
        };
    }
    let m = s / 60;
    if m < 60 {
        return match lang_norm {
            "en" => format!("{}min", m),
            "ja" => format!("{}分", m),
            _ => format!("{}分钟", m),
        };
    }
    let h = m / 60;
    let remain_m = m % 60;
    if h < 24 {
        return match lang_norm {
            "en" => {
                if remain_m == 0 { format!("{}h", h) } else { format!("{}h{}min", h, remain_m) }
            }
            "ja" => {
                if remain_m == 0 { format!("{}時間", h) } else { format!("{}時間{}分", h, remain_m) }
            }
            _ => {
                if remain_m == 0 { format!("{}小时", h) } else { format!("{}小时{}分钟", h, remain_m) }
            }
        };
    }
    let d = h / 24;
    let remain_h = h % 24;
    match lang_norm {
        "en" => {
            if remain_h == 0 { format!("{}d", d) } else { format!("{}d{}h", d, remain_h) }
        }
        "ja" => {
            if remain_h == 0 { format!("{}日", d) } else { format!("{}日{}時間", d, remain_h) }
        }
        _ => {
            if remain_h == 0 { format!("{}天", d) } else { format!("{}天{}小时", d, remain_h) }
        }
    }
}

/// 将 Unix 秒时间戳格式化为多语言相对时间（如 "3小时前" / "3h ago" / "3時間前"）
///
/// 用于记忆/对话历史注入提示词时让 LLM 感知"这条信息是多久前的"。
pub(crate) fn format_relative_time_lang(unix_secs: f64, lang_norm: &str) -> String {
    let now = chrono::Local::now().timestamp() as f64;
    let elapsed = (now - unix_secs).max(0.0);
    let dur = format_elapsed_lang(elapsed, lang_norm);
    match lang_norm {
        "en" => format!("{} ago", dur),
        "ja" => format!("{}前", dur),
        _ => format!("{}前", dur),
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
    /// 连续主动打扰计数（阶梯退避指数）：每次产出主动消息 +1，
    /// 用户交互归零，距上次打扰超过 backoff_grace_secs 减半回落
    #[serde(default)]
    pub consecutive_interruptions: u32,
    /// 最近一次主动打扰时间戳（退避自然衰减依据）
    #[serde(default)]
    pub last_interruption_at: f64,
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
    /// 用户开始离开的时间戳（WelcomeBack 判定离开持续时长的可靠依据，
    /// 替代前端传入的 away_seconds）
    away_since: RwLock<Option<f64>>,
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
    /// 内存压力状态（是否处于高位），用于 normal→high 转换检测
    memory_pressure_active: RwLock<bool>,
    /// 上次主动截屏请求被用户拒绝的时间戳（秒），拒绝后 2 小时内不再请求
    last_screen_peek_denied: Arc<RwLock<Option<f64>>>,
    /// 当前连续使用应用类别（"coding"/"game"/...，空串=未在应用中），
    /// 由 poll_window 维护，类别变化时重置会话计时
    app_session_category: RwLock<String>,
    /// 当前应用会话开始时间戳（Unix 秒），用于计算持续使用时长
    app_session_start: RwLock<f64>,
    /// 上次 tick 的音乐快照（供 music_changed 检测播放/切歌变化）
    last_music: RwLock<Option<crate::world::MusicSnapshot>>,
    /// 上次深夜未眠提醒日期（"YYYY-MM-DD"），每晚只提醒一次
    last_late_night_date: RwLock<String>,
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
            away_since: RwLock::new(None),
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
            memory_pressure_active: RwLock::new(false),
            last_screen_peek_denied: Arc::new(RwLock::new(None)),
            app_session_category: RwLock::new(String::new()),
            app_session_start: RwLock::new(now_ts),
            last_music: RwLock::new(None),
            last_late_night_date: RwLock::new(String::new()),
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
        // 用户离场时概率大幅提升，支持持续跨角色交流
        if !user_present {
            prob += 0.35;
        }
        // 三人共处一室语义：用户和某角色聊天时，另一角色仍可旁听+插话
        // 用时间衰减替代原 5min 硬屏蔽：
        // - < 2min：用户真正在打字，几乎不打断（×0.0）
        // - 2-5min：用户可能停顿，低概率接话（×0.4）
        // - 5-15min：正常概率
        // - >15min：用户实际离开，概率提升（+0.20）
        let now_ts = chrono::Local::now().timestamp() as f64;
        let secs_since_interaction = now_ts - self.state.read().last_interaction_time;
        if secs_since_interaction < 120.0 {
            prob *= 0.0;
        } else if secs_since_interaction < 300.0 {
            prob *= 0.4;
        } else if secs_since_interaction > 900.0 {
            prob += 0.20;
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
        // 用户离场时上限提升至 0.90，支持持续跨角色交流
        let max_prob = if !user_present { 0.90 } else { 0.75 };
        prob.clamp(0.05, max_prob)
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
    pub fn tick(self: Arc<Self>, context: &TickContext) -> VivianResult<bool> {
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

        // 6.8. 自主定点唤醒消费：角色先前通过 schedule_wakeup 工具给自己安排的
        //      "稍后再来"日程到期时，兑现为一条主动消息（带 purpose 上下文）。
        //      门控与特殊日期问候一致：lay_low / 非 leader 时跳过（下轮 tick 重试，
        //      任务未消费不会丢失——drain 只在成功生成后调用）。
        if !context.lay_low && context.is_speaking_leader {
            produced |= self.consume_due_wakeups(context, now);
        }

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

        // 8.5. 日出/日落提醒：检测到天亮/天黑转换瞬间时，
        //     用完整提示词生成一句自然提醒，并弹出推荐切换浅色/深色主题的 toast。
        //     仅在用户在场、非防打扰、且本角色持有发言权时触发（世界事件由 leader 统一表达）。
        if !context.lay_low && context.is_speaking_leader && context.user_present {
            produced |= self.maybe_sunrise_sunset_reminder(context, now);
        }

        // 8.6. 系统资源压力提醒：内存占用越过阈值（normal→high 转换瞬间）时，
        //     生成一句关心的提醒（如"内存有点满了，要不要关几个小程序"）。
        //     与日出/日落同为事件驱动路径，不经 check_trigger 通用门控。
        if !context.lay_low && context.is_speaking_leader && context.user_present {
            produced |= self.maybe_system_pressure_reminder(context, now);
        }

        // 8.7. 主动截屏观察：窗口切换引发"用户在干嘛"的好奇，经用户同意后
        //     截屏 + 视觉理解，再基于屏幕内容生成角色口吻的搭话。
        //     未授权时先发言请求同意（气泡消息）并弹出确认 toast；
        //     被拒绝后 2 小时内不再请求。整个截屏流程异步执行，不阻塞 tick。
        if !context.lay_low
            && context.is_speaking_leader
            && context.user_present
            && !context.is_user_chatting
        {
            produced |= Self::maybe_screen_peek(&self, context, now);
        }

        // 8.8. 应用持续使用超时 & 深夜未眠：健康关怀类提醒。
        //     两者共用一次发言机会（互斥短路），避免一次 tick 串行多次 LLM——
        //     熬夜写代码时两个条件同时成立，只发一条。
        if !context.lay_low && context.is_speaking_leader && context.user_present {
            if !produced {
                produced |= self.maybe_late_night(context, now);
            }
            if !produced {
                produced |= self.maybe_app_duration_reminder(context, now);
            }
        }

        // 8.9. 音乐切换搭话：检测到播放/切歌变化时基于 SMTC 曲目信息自然搭话。
        //     内部每次 tick 都更新播放状态跟踪（无 LLM 成本），仅变化瞬间可能触发。
        if !context.lay_low
            && context.is_speaking_leader
            && context.user_present
            && !context.is_user_chatting
        {
            produced |= self.maybe_music_changed(context, now);
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
                            &companion.id,
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
                        } else {
                            // LLM 失败也标记已表达，避免每 tick 重试同一思绪形成死循环
                            self.thought_lifecycle.write().mark_expressed(&thought_key, context.now);
                            tracing::warn!("[thought_lifecycle] 思绪{}对室友分享生成失败，标记已表达避免重试", thought_key);
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
                    } else {
                        // LLM 失败也标记已表达，避免每 tick 重试同一思绪形成死循环
                        self.thought_lifecycle.write().mark_expressed(&thought_key, context.now);
                        tracing::warn!("[thought_lifecycle] 思绪{}知识分享生成失败，标记已表达避免重试", thought_key);
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
                } else {
                    // LLM 失败也标记已表达，避免每 tick 重试同一思绪形成死循环
                    self.thought_lifecycle.write().mark_expressed(&thought_key, context.now);
                    tracing::warn!("[thought_lifecycle] 思绪{}主动分享生成失败，标记已表达避免重试", thought_key);
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

    /// 阶梯退避惰性衰减：距上次主动打扰超过 backoff_grace_secs → 计数减半，
    /// 持续无打扰时逐渐回落直到归零（打扰收敛的自然恢复）
    fn decay_backoff(&self, now: f64) {
        let grace = self.config.read().backoff_grace_secs as f64;
        let mut state = self.state.write();
        if state.consecutive_interruptions == 0 || grace <= 0.0 {
            return;
        }
        if now - state.last_interruption_at > grace {
            state.consecutive_interruptions /= 2;
            if state.consecutive_interruptions == 0 {
                state.last_interruption_at = 0.0;
            }
        }
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

        // 阶梯退避惰性衰减：很久没打扰则计数减半回落（在触发器检查前统一执行）
        self.decay_backoff(now);

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
        // 阶梯退避惰性衰减（与主触发路径保持一致）
        self.decay_backoff(now);
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

    /// 日出/日落提醒：检测到天亮/天黑转换瞬间时，
    /// 用完整提示词（主对话流程）生成一句自然提醒，并弹出可一键切换主题的确认 toast。
    ///
    /// 触发条件：
    /// - 本次 tick 检测到 Sunrise/Sunset 世界事件（is_daytime 转换瞬间只检测到一次）
    /// - 1 小时冷却兜底，防止事件检测器异常时重复提醒
    ///
    /// 主题建议门控：当前生效主题（含"跟随系统"按系统偏好解析）已是推荐主题时，
    /// 提示词禁止建议切换主题，且不再弹确认 toast（避免"本来就用浅色还让用户改浅色"）。
    fn maybe_sunrise_sunset_reminder(&self, context: &TickContext, now: f64) -> bool {
        // 读取本次 tick 检测到的世界事件
        let events = self.detected_world_events.read().clone();
        let event = events
            .iter()
            .find(|e| matches!(e.kind, WorldEventKind::Sunrise | WorldEventKind::Sunset));
        let (trigger, is_sunrise) = match event {
            Some(e) => match e.kind {
                WorldEventKind::Sunrise => (ProactiveTrigger::Sunrise, true),
                WorldEventKind::Sunset => (ProactiveTrigger::Sunset, false),
                _ => return false,
            },
            None => return false,
        };

        // 冷却兜底：同一事件 1 小时内不重复提醒
        {
            let state = self.state.read();
            if let Some(&last) = state.last_trigger_times.get(trigger.as_str()) {
                if now - last < 3600.0 {
                    return false;
                }
            }
        }

        // 用完整提示词（主对话流程）生成提醒内容；失败直接跳过，不降级到模板回退
        let router = match self.model_router.read().clone() {
            Some(r) => r,
            None => return false,
        };
        let hour = chrono::Local::now()
            .format("%H")
            .to_string()
            .parse::<u32>()
            .unwrap_or(12);
        let content = match self.try_llm_content(trigger, context, hour, &router, None) {
            Some(c) if !c.text.trim().is_empty() => c.text,
            _ => return false,
        };

        // 推送提醒消息（气泡渠道）
        self.push_message(trigger, content, now);
        // 记录触发时间，纳入共享冷却
        self.update_trigger_time(trigger, now, hour, 0);

        // 弹出推荐切换浅色/深色主题的 toast
        self.emit_theme_recommendation_toast(is_sunrise);

        tracing::info!(
            "[proactive:{}] 日出/日落提醒已推送: {}",
            self.char_id,
            trigger.as_str()
        );
        true
    }

    /// 弹出推荐切换浅色/深色主题的确认 toast（附一键切换按钮），
    /// 当前生效主题（含"跟随系统"按系统偏好解析）已与推荐一致时跳过
    fn emit_theme_recommendation_toast(&self, is_sunrise: bool) {
        let handle = match APP_HANDLE.read().clone() {
            Some(h) => h,
            None => return,
        };
        let recommended = if is_sunrise { "light" } else { "dark" };
        // 当前生效主题已是推荐主题则不再提示
        if current_effective_theme().as_deref() == Some(recommended) {
            return;
        }
        use tauri::Emitter;
        let _ = handle.emit(
            "toast:show",
            serde_json::json!({
                "message": theme_toast_message(is_sunrise),
                "type": "info",
                "duration": 8000,
                "key": chrono::Utc::now().timestamp_millis(),
                "character_id": self.char_id,
                // 确认按钮：前端点击后直接写入 base.theme 并广播主题变更
                "action": {
                    "kind": "switch_theme",
                    "theme": recommended,
                    "label": theme_toast_action_label(is_sunrise),
                },
            }),
        );
    }

    /// 系统资源压力提醒：内存占用越过阈值（normal→high 转换瞬间）时提醒用户
    ///
    /// 触发条件：
    /// - 配置启用（proactive.enable_system_pressure_trigger）
    /// - 内存占用百分比 ≥ MEMORY_PRESSURE_THRESHOLD_PCT（85%），且上一 tick 不在高位
    ///   （持续高位只提醒一次，降回正常后再次升高才会重新提醒）
    /// - 冷却兜底（默认 30 分钟），防止指标在阈值附近抖动导致反复提醒
    fn maybe_system_pressure_reminder(&self, context: &TickContext, now: f64) -> bool {
        // 无论开关如何都跟踪转换状态，避免配置中途开启时误把存量高位当转换
        let metrics = self
            .world_provider
            .read()
            .as_ref()
            .and_then(|wp| wp.system_metrics());
        let Some(m) = metrics else {
            return false;
        };
        let high = m.memory_usage_pct >= MEMORY_PRESSURE_THRESHOLD_PCT;
        let was_active = std::mem::replace(&mut *self.memory_pressure_active.write(), high);
        if !high || was_active {
            return false;
        }
        if !self.config.read().enable_system_pressure_trigger {
            return false;
        }

        let trigger = ProactiveTrigger::SystemPressure;
        // 冷却兜底
        {
            let throttle = TriggerThrottle::get(trigger);
            let state = self.state.read();
            if let Some(&last) = state.last_trigger_times.get(trigger.as_str()) {
                if now - last < throttle.cooldown_seconds as f64 {
                    return false;
                }
            }
        }

        // 用完整提示词生成提醒内容；失败直接跳过，不降级到模板回退
        let router = match self.model_router.read().clone() {
            Some(r) => r,
            None => return false,
        };
        let hour = chrono::Local::now()
            .format("%H")
            .to_string()
            .parse::<u32>()
            .unwrap_or(12);
        let content = match self.try_llm_content(trigger, context, hour, &router, None) {
            Some(c) if !c.text.trim().is_empty() => c.text,
            _ => return false,
        };

        self.push_message(trigger, content, now);
        self.update_trigger_time(trigger, now, hour, 0);

        tracing::info!(
            "[proactive:{}] 系统压力提醒已推送: 内存 {:.0}%",
            self.char_id,
            m.memory_usage_pct
        );
        true
    }

    /// 主动截屏观察：窗口切换引发好奇，经用户同意后截屏理解屏幕内容并搭话
    ///
    /// 触发条件：
    /// - 配置启用（proactive.enable_screen_peek_trigger）+ 视觉功能开启（ai.enable_vision）
    /// - 本 tick 检测到窗口切换，且用户空闲 ≥ 30s（不打断正在操作的用户）
    /// - 触发器冷却（默认 1 小时）+ 概率 roll（0.12 × proactivity）
    /// - 用户拒绝后的请求冷却（2 小时）未到期时不发起
    ///
    /// 权限流程：
    /// - screenshot_analyze 已在会话放行列表 → 直接异步截屏
    /// - 未放行 → 先推送一条"想看看你在干嘛"的气泡消息（本次 tick 即送达），
    ///   同时异步弹确认 toast；用户同意后截屏，拒绝则记录时间戳冷却 2 小时
    ///
    /// 返回 true 表示本次 tick 已产出（请求消息已入队或异步任务已发起）
    fn maybe_screen_peek(orch: &Arc<Self>, context: &TickContext, now: f64) -> bool {
        let this: &Self = orch;
        if !this.config.read().enable_screen_peek_trigger {
            return false;
        }
        // 窗口切换事件驱动：无切换不触发
        if !context.window_changed || context.active_window.is_empty() {
            return false;
        }
        // 用户刚操作过不打扰（正在打字/切窗口的瞬间不适合被"观察"）
        let throttle = TriggerThrottle::get(ProactiveTrigger::ScreenPeek);
        if context.idle_seconds < throttle.min_idle_seconds as f64 {
            return false;
        }
        // 视觉功能关闭时无从理解屏幕，跳过
        if !vision_enabled() {
            return false;
        }
        // 拒绝冷却：用户明确拒绝后 2 小时内不再请求
        if let Some(denied_at) = *this.last_screen_peek_denied.read() {
            if now - denied_at < SCREEN_PEEK_DENY_COOLDOWN_SECS {
                return false;
            }
        }
        // 触发器冷却（角色专属 cooldown_mult）
        {
            let behavior = crate::character_behavior::get_behavior(&this.char_id);
            let effective_cooldown =
                ((throttle.cooldown_seconds as f64 * behavior.trigger_modifiers.cooldown_mult)
                    as u64)
                    .max(1);
            let state = this.state.read();
            if let Some(&last) = state.last_trigger_times.get(ProactiveTrigger::ScreenPeek.as_str()) {
                if now - last < effective_cooldown as f64 {
                    return false;
                }
            }
        }
        // 概率 roll：窗口切换是高频事件，低概率抽样避免每次切换都想看
        let cfg = this.config.read().clone();
        let scaled_probability = throttle.probability * cfg.proactivity.clamp(0.0, 1.0);
        if !roll_with_probability(scaled_probability) {
            return false;
        }

        // 会话级授权：用户此前选过"始终允许"则直接截屏，否则先发言请求同意
        let session_allowed = is_session_allowed_tool("screenshot_analyze");
        if !session_allowed {
            // 先发言（气泡渠道）：表达好奇并请求同意，本条消息随本次 tick 一起送达
            this.push_message(
                ProactiveTrigger::ScreenPeek,
                screen_peek_ask_message().to_string(),
                now,
            );
        }

        // 记录触发时间（在异步流程启动前写入，防止异步期间重复触发）
        let hour = chrono::Local::now()
            .format("%H")
            .to_string()
            .parse::<u32>()
            .unwrap_or(12);
        this.update_trigger_time(ProactiveTrigger::ScreenPeek, now, hour, 0);

        // 异步执行：权限确认（如需）→ 截屏 → 视觉理解 → 生成搭话 → 入队
        // 消息在下一次 tick（约 10s 后）被 drain 送达前端
        Self::spawn_screen_peek_task(Arc::clone(orch), context.clone(), hour, !session_allowed);

        tracing::info!(
            "[proactive:{}] 主动截屏观察已发起（session_allowed={}）",
            this.char_id,
            session_allowed
        );
        true
    }

    /// 主动截屏观察的异步执行体：
    /// 请求权限（如需）→ 截屏 → 视觉理解 → 角色口吻搭话 → push_message
    fn spawn_screen_peek_task(
        orchestrator: Arc<Self>,
        context: TickContext,
        hour: u32,
        need_permission: bool,
    ) {
        let char_id = orchestrator.char_id.clone();
        tauri::async_runtime::spawn(async move {
            // 1. 权限：未授权时弹确认 toast 等待用户三态选择
            if need_permission {
                let tool_system = orchestrator.tool_system.read().clone();
                let Some(tool_system) = tool_system else {
                    tracing::debug!("[proactive:{}] ToolSystem 未注入，跳过主动截屏", char_id);
                    return;
                };
                let response = tool_system
                    .request_confirmation(
                        "screenshot_analyze",
                        &serde_json::json!({}),
                        screen_peek_ask_reason(&char_id),
                        ConfirmationRisk::Medium,
                        &char_id,
                        "session",
                    )
                    .await;
                match response {
                    Some(ConfirmationResponse::AllowOnce) => {}
                    Some(ConfirmationResponse::AllowAlways) => {
                        // 会话级记忆：本会话内后续主动截屏不再询问
                        session_allow_tool("screenshot_analyze");
                    }
                    _ => {
                        // 拒绝/超时/未响应：记录时间戳，2 小时内不再请求
                        // （用拒绝时刻的真实时间，而非 tick 时间，保证冷却完整覆盖）
                        *orchestrator.last_screen_peek_denied.write() =
                            Some(chrono::Local::now().timestamp() as f64);
                        tracing::info!(
                            "[proactive:{}] 用户拒绝了主动截屏请求，{}s 内不再请求",
                            char_id,
                            SCREEN_PEEK_DENY_COOLDOWN_SECS
                        );
                        return;
                    }
                }
            }

            // 2. 截屏（临时文件读出后立即删除，不进剪贴板）
            let png_bytes = match capture_screen_png_bytes().await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("[proactive:{}] 主动截屏失败: {}", char_id, e);
                    return;
                }
            };

            // 3. 视觉理解：拿到屏幕内容的客观描述
            let router = match orchestrator.model_router.read().clone() {
                Some(r) => r,
                None => return,
            };
            let image_detail = vision_image_detail();
            let (description, _reply) = match describe_screen_bytes(
                &router,
                image_detail,
                png_bytes,
                "",
            )
            .await
            {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("[proactive:{}] 屏幕视觉理解失败: {}", char_id, e);
                    return;
                }
            };
            if description.trim().is_empty() {
                tracing::debug!("[proactive:{}] 屏幕描述为空，跳过搭话", char_id);
                return;
            }

            // 4. 基于屏幕描述生成角色口吻的搭话（完整提示词路径）
            let content = orchestrator.try_llm_content(
                ProactiveTrigger::ScreenPeek,
                &context,
                hour,
                &router,
                Some(&description),
            );
            let Some(content) = content else {
                return;
            };
            if content.text.trim().is_empty() {
                return;
            }

            // 5. 入队（下一次 tick drain 送达前端气泡）
            //    用户确认 + 视觉理解可能耗时较久，用新鲜时间戳入队，
            //    避免消息年龄超过 PROACTIVE_MSG_TTL_SECS 被 drain 丢弃
            let fresh_now = chrono::Local::now().timestamp() as f64;
            orchestrator.push_message(ProactiveTrigger::ScreenPeek, content.text, fresh_now);
            tracing::info!("[proactive:{}] 主动截屏观察完成，已生成搭话", char_id);
        });
    }

    /// 深夜未眠关心：凌晨 1-4 点用户仍活跃时，温柔地提醒注意休息
    ///
    /// 触发条件：
    /// - 配置启用（proactive.enable_late_night_trigger）+ 当前为凌晨 1-4 点
    /// - 用户活跃（空闲 < 300s，不对着空电脑说话）
    /// - 当晚（按本地日期）未提醒过 + 冷却兜底
    fn maybe_late_night(&self, context: &TickContext, now: f64) -> bool {
        use chrono::Timelike;
        if !self.config.read().enable_late_night_trigger {
            return false;
        }
        let local_now = chrono::Local::now();
        let hour = local_now.hour();
        if !is_late_night_hour(hour) {
            return false;
        }
        // 用户不活跃时跳过（看起来没在用电脑）
        if context.idle_seconds >= 300.0 {
            return false;
        }
        let today = local_now.format("%Y-%m-%d").to_string();
        {
            let last_date = self.last_late_night_date.read();
            if *last_date == today {
                return false;
            }
        }
        // 冷却兜底（默认 10 小时，与"每晚一次"的意图对齐）
        let trigger = ProactiveTrigger::LateNight;
        {
            let throttle = TriggerThrottle::get(trigger);
            let state = self.state.read();
            if let Some(&last_ts) = state.last_trigger_times.get(trigger.as_str()) {
                if now - last_ts < throttle.cooldown_seconds as f64 {
                    return false;
                }
            }
        }

        let router = match self.model_router.read().clone() {
            Some(r) => r,
            None => return false,
        };
        let content = match self.try_llm_content(trigger, context, hour, &router, None) {
            Some(c) if !c.text.trim().is_empty() => c.text,
            _ => return false,
        };

        self.push_message(trigger, content, now);
        self.update_trigger_time(trigger, now, hour, 0);
        *self.last_late_night_date.write() = today;

        tracing::info!("[proactive:{}] 深夜未眠提醒已推送（{}:00 仍在用电脑）", self.char_id, hour);
        true
    }

    /// 应用持续使用超时提醒：同一类应用连续使用达到阈值时，
    /// 按应用语义（写代码/打游戏/看视频）生成个性化关心或轻调侃
    ///
    /// 会话跟踪由 poll_window 维护（类别变化重置计时）。
    /// 触发后重置会话计时 + 冷却兜底，同一会话内 2 小时内不重复提醒。
    fn maybe_app_duration_reminder(&self, context: &TickContext, now: f64) -> bool {
        if !self.config.read().enable_app_duration_trigger {
            return false;
        }
        let category = self.app_session_category.read().clone();
        if category.is_empty() || matches!(category.as_str(), "other" | "utility") {
            return false;
        }
        let session_start = *self.app_session_start.read();
        let duration_secs = (now - session_start).max(0.0);
        let threshold = app_duration_threshold_secs(&category);
        if duration_secs < threshold {
            return false;
        }
        // 用户空闲太久不算"持续使用"（可能走开没关窗口）
        if context.idle_seconds >= 300.0 {
            return false;
        }

        let trigger = ProactiveTrigger::AppDuration;
        // 冷却兜底（2 小时），防止阈值边界抖动反复提醒
        {
            let throttle = TriggerThrottle::get(trigger);
            let state = self.state.read();
            if let Some(&last_ts) = state.last_trigger_times.get(trigger.as_str()) {
                if now - last_ts < throttle.cooldown_seconds as f64 {
                    return false;
                }
            }
        }

        let router = match self.model_router.read().clone() {
            Some(r) => r,
            None => return false,
        };
        let hour = chrono::Local::now()
            .format("%H")
            .to_string()
            .parse::<u32>()
            .unwrap_or(12);
        let content = match self.try_llm_content(trigger, context, hour, &router, None) {
            Some(c) if !c.text.trim().is_empty() => c.text,
            _ => return false,
        };

        self.push_message(trigger, content, now);
        self.update_trigger_time(trigger, now, hour, 0);
        // 重置会话计时：本次提醒后重新累计（配合冷却避免反复打扰）
        *self.app_session_start.write() = now;

        tracing::info!(
            "[proactive:{}] 应用时长提醒已推送: {} 连续 {:.0} 分钟",
            self.char_id,
            category,
            duration_secs / 60.0
        );
        true
    }

    /// 音乐切换搭话：用户开始播放/切换新曲目时，基于 SMTC 曲目信息自然搭话
    ///
    /// 触发条件：
    /// - 配置启用（proactive.enable_music_trigger）+ world 音乐感知开启
    /// - 检测到播放状态变化：无→播放，或暂停→播放，或播放中切歌
    /// - 过滤视频播放源（source_app 含 video/player 等关键词，避免"看剧"被当"听歌"）
    /// - 触发器冷却（45 分钟）+ 概率 roll（0.3 × proactivity）
    ///
    /// 播放状态跟踪每次 tick 都更新（无论是否触发），保证变化检测不丢失。
    fn maybe_music_changed(&self, context: &TickContext, now: f64) -> bool {
        let wp = match self.world_provider.read().as_ref() {
            Some(wp) => wp.clone(),
            None => return false,
        };
        let world_cfg = wp.config();
        if !world_cfg.enable {
            return false;
        }

        // 读取当前音乐快照（缓存由 tick 前 spawn 的 refresh_music 更新）
        let current = wp.snapshot(Some(context.away_seconds)).music;

        // 检测播放/切歌变化（对比上次 tick 的状态）
        let prev = self.last_music.read().clone();
        let is_media_changed = match (&current, &prev) {
            (Some(cur), None) => {
                cur.status == crate::world::PlaybackStatus::Playing && !cur.title.is_empty()
            }
            (Some(cur), Some(prev_m)) => {
                if cur.status != crate::world::PlaybackStatus::Playing {
                    false
                } else if prev_m.status != crate::world::PlaybackStatus::Playing {
                    // 暂停/停止 → 恢复播放
                    true
                } else {
                    // 播放中切换曲目
                    cur.title != prev_m.title
                }
            }
            (None, _) => false,
        };
        // 无论是否触发都记录最新状态（含停止播放 None）
        *self.last_music.write() = current.clone();

        // 过滤视频播放源：播放器/视频站点的 SMTC 是"看剧"而非"听歌"
        if let Some(m) = &current {
            let src = m.source_app.to_lowercase();
            let video_kw = [
                "video", "player", "mpc", "vlc", "potplayer", "bilibili", "youtube",
                "netflix", "iqiyi", "youku", "tencent",
            ];
            if video_kw.iter().any(|k| src.contains(k)) {
                return false;
            }
        }

        if !is_media_changed {
            return false;
        }
        if !self.config.read().enable_music_trigger {
            return false;
        }

        let trigger = ProactiveTrigger::MusicChanged;
        let throttle = TriggerThrottle::get(trigger);
        // 冷却检查（角色专属 cooldown_mult）
        {
            let behavior = crate::character_behavior::get_behavior(&self.char_id);
            let effective_cooldown =
                ((throttle.cooldown_seconds as f64 * behavior.trigger_modifiers.cooldown_mult)
                    as u64)
                    .max(1);
            let state = self.state.read();
            if let Some(&last_ts) = state.last_trigger_times.get(trigger.as_str()) {
                if now - last_ts < effective_cooldown as f64 {
                    return false;
                }
            }
        }
        // 概率 roll：曲目切换相对高频，低概率抽样避免每次切歌都搭话
        let cfg = self.config.read().clone();
        let scaled_probability = throttle.probability * cfg.proactivity.clamp(0.0, 1.0);
        if !roll_with_probability(scaled_probability) {
            return false;
        }

        let router = match self.model_router.read().clone() {
            Some(r) => r,
            None => return false,
        };
        let hour = chrono::Local::now()
            .format("%H")
            .to_string()
            .parse::<u32>()
            .unwrap_or(12);
        let content = match self.try_llm_content(trigger, context, hour, &router, None) {
            Some(c) if !c.text.trim().is_empty() => c.text,
            _ => return false,
        };

        self.push_message(trigger, content, now);
        self.update_trigger_time(trigger, now, hour, 0);

        tracing::info!("[proactive:{}] 音乐切换搭话已推送", self.char_id);
        true
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
                false,
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
                false,
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
                    seed.high_priority,
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
                        // 即使 LLM 失败也标记独白完成，避免每 tick 重复选同一条思绪形成死循环
                        let mut lc = lifecycle_for_mono.write();
                        lc.mark_monologue_done(&thought_key);
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

    /// 消费到期的自主唤醒任务：逐条生成主动消息并入队
    ///
    /// 只处理最早到期的一条（单 tick 最多兑现一个承诺，保持自然），
    /// 其余顺延到后续 tick。LLM 生成失败的任务也标记消费（防死循环重试）。
    fn consume_due_wakeups(&self, ctx: &TickContext, now: f64) -> bool {
        let scheduler = crate::proactive::wakeup::get_scheduler(&self.char_id);
        let due = scheduler.drain_due(now);
        if due.is_empty() {
            return false;
        }
        let mut produced = false;
        // 单 tick 只兑现最早的一条，其余重新入队顺延
        let (first, rest) = due.split_first().expect("due 非空");
        for w in rest {
            let _ = scheduler.schedule(w.due_at - now, &w.purpose);
        }
        match self.generate_wakeup_message(ctx, &first.purpose) {
            Some(content) => {
                self.push_message(ProactiveTrigger::Spontaneous, content, now);
                produced = true;
                tracing::info!(
                    "[wakeup] 自主唤醒兑现: purpose=\"{}\"",
                    first.purpose
                );
            }
            None => {
                tracing::warn!(
                    "[wakeup] 唤醒消息生成失败，任务丢弃: purpose=\"{}\"",
                    first.purpose
                );
            }
        }
        produced
    }

    /// 依据唤醒目的生成一句兑现承诺的主动消息
    fn generate_wakeup_message(&self, ctx: &TickContext, purpose: &str) -> Option<String> {
        let router = self.model_router.read().clone()?;
        let handle = tokio::runtime::Handle::try_current().ok()?;

        let persona = self.persona.read().clone();
        let psychology = self.psychology.read().clone();
        let lang = persona
            .as_ref()
            .map(|p| p.get_language())
            .unwrap_or_else(|| "zh".to_string());

        let system_prompt = persona
            .as_ref()
            .map(|p| {
                let intimacy = psychology
                    .as_ref()
                    .map(|psy| psy.relationship().intimacy * 100.0)
                    .unwrap_or(50.0);
                p.build_style_prompt(intimacy, ctx.now as u32 % 24)
            })
            .unwrap_or_default();

        let purpose = purpose.to_string();
        let result = handle.block_on(async move {
            use crate::providers::base::LLMRequest;
            use crate::types::response::ChatMessage;

            let user_msg = match lang.as_str() {
                "en" => format!(
                    "You told the user you would come back later, and now the scheduled moment has arrived.\n\n## What you planned to do\n{purpose}\n\n\
                     Say one short natural line to the user, as if keeping a promise you made earlier. Keep it under 30 words.\n\n\
                     Strict JSON output: {{\"text\": \"...\", \"expression\": \"...\"}}",
                ),
                "ja" => format!(
                    "あなたはユーザーに「また後で来る」と伝えていました。その約束の時刻が来ました。\n\n## 予定していたこと\n{purpose}\n\n\
                     以前自分が言ったことを守るように、自然な一言をユーザーに話しかけてください。30文字以内。\n\n\
                     厳密なJSON出力: {{\"text\": \"...\", \"expression\": \"...\"}}",
                ),
                _ => format!(
                    "你之前对用户说过稍后要做什么，现在约定的时刻到了。\n\n## 你当时的计划\n{purpose}\n\n\
                     请用一句简短自然的话对用户说，就像兑现自己之前说过的话那样。30字以内。\n\n\
                     严格输出JSON：{{\"text\": \"你要说的话\", \"expression\": \"表情名\"}}",
                ),
            };

            let messages = vec![
                ChatMessage::system(if system_prompt.is_empty() {
                    "你是用户的桌面伙伴，说话简短自然。".to_string()
                } else {
                    system_prompt
                }),
                ChatMessage::user(&user_msg),
            ];
            router
                .generate(LLMRequest::new("proactive", messages))
                .await
        });

        match result {
            Ok(text) => {
                let text = text.trim();
                Self::parse_proactive_json(text)
                    .filter(|c| c.text.len() >= 2)
                    .map(|c| c.text)
            }
            Err(e) => {
                tracing::warn!("[wakeup] LLM 生成失败: {e}");
                None
            }
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
        let intimacy = psychology
            .as_ref()
            .map(|p| p.relationship().intimacy * 100.0)
            .unwrap_or(50.0);
        let system_prompt = persona
            .as_ref()
            .map(|p| p.build_style_prompt(intimacy, snap.hour))
            .unwrap_or_default();
        let thoughts_context = self.thought_lifecycle.read().build_context_hint();

        // 最近对话历史（让 LLM 感知上下文，避免突兀或重复）
        let dialogue_history = {
            let dialogue = self.dialogue.read().clone();
            dialogue
                .as_ref()
                .map(|d| {
                    d.get_history()
                        .into_iter()
                        .rev()
                        .take(4)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .map(|m| format!("{}: {}", m.role, m.content))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        };

        // 真实工具调用历史（禁止编造未发生的操作）
        let tool_history = {
            let ts_opt = self.tool_system.read().clone();
            ts_opt
                .as_ref()
                .map(|ts| behavior::format_recent_tool_history(ts, &lang))
                .unwrap_or_default()
        };

        let thought_key = thought_key.to_string();
        let context_hint = context_hint.to_string();

        let result = handle.block_on(async move {
            use crate::providers::base::LLMRequest;
            use crate::types::response::ChatMessage;

            let dialogue_section = if dialogue_history.is_empty() {
                String::new()
            } else {
                format!("## 最近对话\n{}\n\n", dialogue_history)
            };
            let tool_section = if tool_history.is_empty() {
                String::new()
            } else {
                format!("## 最近实际执行的操作\n{}\n（只能提及上述真实操作，不要编造未发生的人类生活行为）\n\n", tool_history)
            };

            let user_msg = format!(
                "你心里一直在想一件事，现在忍不住想说出来。\n\n\
                 ## 你此刻心里想的是\n{hint}\n\n\
                 {other_thoughts}\
                 {dialogue}\
                 {tools}\
                 ## 当前状态\n- 心情：{mood}\n- 时段：{hour}点\n\n\
                 请用一句简短自然的话把这个念头说出来，对用户说。就像人忍不住开口说话那样，不要长篇大论，不要刻意组织语言，就随口说一句，25字以内。\n\n\
                 严格输出JSON：{{\"text\": \"你要说的话\", \"expression\": \"表情名\"}}",
                hint = context_hint,
                other_thoughts = if thoughts_context.is_empty() { String::new() } else { format!("## 其他心绪\n{}\n\n", thoughts_context) },
                dialogue = dialogue_section,
                tools = tool_section,
                mood = mind_state,
                hour = snap.hour,
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
        roommate_id: &str,
        roommate_name: &str,
    ) -> Option<String> {
        let router = self.model_router.read().clone()?;
        let handle = tokio::runtime::Handle::try_current().ok()?;

        let mind_state = self.get_mind_state().as_str().to_string();
        let persona = self.persona.read().clone();
        let psychology = self.psychology.read().clone();
        let wp = self.world_provider.read().clone()?;
        let snap = wp.snapshot(None);

        let user_intimacy = psychology
            .as_ref()
            .map(|p| p.relationship().intimacy * 100.0)
            .unwrap_or(50.0);
        let system_prompt = persona
            .as_ref()
            .map(|p| p.build_style_prompt(user_intimacy, snap.hour))
            .unwrap_or_default();
        let thoughts_context = self.thought_lifecycle.read().build_context_hint();

        // A↔B 亲密度（角色间关系）
        let pair_intimacy = crate::psychology::social_state::social_state()
            .get_pair(&self.char_id, roommate_id)
            .intimacy;

        // 最近跨角色对话历史（从统一事件账本检索）
        let recent_dialogue = {
            let ledger = crate::memory::unified_event_ledger::unified_event_ledger();
            let events = ledger.events_between(&self.char_id, roommate_id, 3);
            if events.is_empty() {
                String::new()
            } else {
                let lines: Vec<String> = events
                    .iter()
                    .map(|e| {
                        let preview: String = e.content_preview.chars().take(60).collect();
                        if e.sender == self.char_id {
                            format!("- 我对{}说：{}", roommate_name, preview)
                        } else {
                            format!("- {}对我说：{}", roommate_name, preview)
                        }
                    })
                    .collect();
                format!("## 最近你们聊过的话题\n{}\n", lines.join("\n"))
            }
        };

        let thought_key = thought_key.to_string();
        let context_hint = context_hint.to_string();
        let roommate_name = roommate_name.to_string();

        let result = handle.block_on(async move {
            use crate::providers::base::LLMRequest;
            use crate::types::response::ChatMessage;

            // 关系远近描述
            let closeness = if pair_intimacy >= 0.75 {
                "你们关系很好，可以随口搭话"
            } else if pair_intimacy >= 0.5 {
                "你们还算熟，自然地搭话就行"
            } else if pair_intimacy >= 0.25 {
                "你们关系一般，稍微客气一点"
            } else {
                "你们还不太熟，找个自然的话题开口"
            };

            let user_msg = format!(
                "你心里有件事想和室友{roommate}聊聊。这是你主动想找ta说话，不是ta先找你的。\n\n\
                 ## 你想聊的是\n{hint}\n\n\
                 {other_thoughts}\
                 {recent}\
                 ## 当前状态\n- 心情：{mood}\n- 时段：{hour}点\n- 和{roommate}的关系：{closeness}\n\n\
                 请用一句简短自然的话对{roommate}说，就像室友之间随口聊天那样。不要长篇大论，不要刻意组织语言，25字以内。\n\
                 注意：只能提及上面上下文中真实出现的内容，不要编造你没做过的事或没发生过的对话。\n\n\
                 严格输出JSON：{{\"text\": \"你要对{roommate}说的话\", \"expression\": \"表情名\"}}",
                roommate = roommate_name,
                hint = context_hint,
                other_thoughts = if thoughts_context.is_empty() { String::new() } else { format!("## 其他心绪\n{}\n", thoughts_context) },
                recent = recent_dialogue,
                mood = mind_state,
                hour = snap.hour,
                closeness = closeness,
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
                // 事件驱动触发器（日出/日落/系统压力/主动截屏/应用时长/深夜未眠/音乐切换）
                // 由 tick 中专门路径处理，不进入常规触发循环（check_specific 也返回 false 双重保险）
                ProactiveTrigger::Sunrise
                | ProactiveTrigger::Sunset
                | ProactiveTrigger::SystemPressure
                | ProactiveTrigger::ScreenPeek
                | ProactiveTrigger::AppDuration
                | ProactiveTrigger::LateNight
                | ProactiveTrigger::MusicChanged => false,
                _ => true,
            })
            .collect()
    }

    /// 窗口 / 活动轮询
    fn poll_window(&self, ctx: &TickContext) {
        if !ctx.active_window.is_empty() {
            let category = self.app_classifier.classify(&ctx.active_window);
            *self.last_app_category.write() = category.clone();
            // 应用会话跟踪：类别变化（或应用切换导致分类不同）时重置连续使用计时。
            // 相同类别持续使用则累计时长，供 AppDuration 触发器按语义生成关心/调侃。
            {
                let mut sess_cat = self.app_session_category.write();
                if *sess_cat != category {
                    *sess_cat = category.clone();
                    *self.app_session_start.write() = ctx.now;
                }
            }
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
        } else {
            // 回到桌面/无焦点窗口：中断应用会话计时（避免回到桌面后仍按应用时长提醒）
            let mut sess_cat = self.app_session_category.write();
            if !sess_cat.is_empty() {
                *sess_cat = String::new();
            }
        }
        // 用户活动：idle_seconds < 60 视为活跃
        let active = ctx.idle_seconds < 60.0;
        let mut was_away = self.last_user_was_away.write();
        if !active {
            // 首次检测到离开时记录时间戳，用于 WelcomeBack 判定离开持续时长
            if !*was_away {
                *self.away_since.write() = Some(ctx.now);
            }
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
        // 节日映射：(日期, 节日名, 兜底文案)
        let festivals: &[(&str, &str, &str)] = &[
            ("01-01", "元旦/新年", "新年快乐！新的一年也要开开心心的~"),
            ("02-14", "情人节", "今天是情人节呢……你、你有什么安排吗？"),
            ("03-08", "妇女节", "今天是妇女节，祝所有女孩子节日快乐~"),
            ("05-01", "劳动节", "劳动节快乐！今天要好好休息哦~"),
            ("06-01", "儿童节", "儿童节快乐！在我心里你永远是个小孩子~"),
            ("09-10", "教师节", "教师节快乐~感谢所有老师"),
            ("10-01", "国庆节", "国庆快乐！假期好好放松一下~"),
            ("12-25", "圣诞节", "圣诞快乐！你收到礼物了吗？"),
            ("12-31", "跨年夜", "今天是跨年夜呢，今年过得怎么样？"),
        ];
        let festival = festivals.iter().find(|(k, _, _)| *k == today_key);
        if festival.is_none() {
            return false;
        }
        let mut state = self.state.write();
        if state.last_special_date == today_key {
            return false;
        }
        state.last_special_date = today_key.clone();
        // 注意：不更新 last_interaction_time，理由同 update_trigger_time
        state
            .last_trigger_times
            .insert(ProactiveTrigger::HourlyGreeting.as_str().to_string(), now);
        drop(state);

        let (_, festival_name, fallback) = festival.unwrap();
        let text = self.generate_festival_greeting(festival_name)
            .unwrap_or_else(|| fallback.to_string());
        self.push_message(ProactiveTrigger::HourlyGreeting, text, now);
        let _ = self.save_to();
        true
    }

    /// 通过 LLM 生成节日问候（失败时返回 None，由调用方回退到兜底文案）
    fn generate_festival_greeting(&self, festival_name: &str) -> Option<String> {
        let router = self.model_router.read().clone()?;
        let handle = tokio::runtime::Handle::try_current().ok()?;
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
        let char_id = self.char_id.clone();
        let festival_name = festival_name.to_string();

        let result = handle.block_on(async move {
            use crate::providers::base::LLMRequest;
            use crate::types::response::ChatMessage;

            let user_msg = format!(
                "今天是{}。请用你的风格对用户说一句节日问候，简短自然，像随口说的，25字以内。\n\
                 不要客套话，不要百科式介绍，用你自己的语气。只能提及节日本身，不要编造你没做过的事。\n\n\
                 严格输出JSON：{{\"text\": \"你要说的话\", \"expression\": \"表情名\"}}",
                festival_name
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
                    tracing::debug!("[festival_greeting] {} LLM 生成失败: {}", char_id, e);
                    None
                }
            }
        });

        if let Some(ref content) = result {
            tracing::info!("[festival_greeting] {} 节日问候: '{}'", self.char_id, content);
        }
        result
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

        // 策略 G：social_urge 提前触发说明
        // current_thought 每 60s 调 LLM 顺便产出 social_urge（0-1），
        // urge 很高时（>= 0.8）在 check_specific 中放宽问候的"特定条件"（整点/空闲阈值等），
        // 让角色在"真的想说话"时提前问候。urge 中/低时不拦截规则触发（保底不漏）。
        // 具体实现见 check_specific 开头。可通过 proactive.enable_social_urge_gating 关闭。

        // 冷却检查（所有触发器共用）—— min_trigger_interval 作为全局下限
        // 策略 B：冷却秒数乘以角色专属 cooldown_mult
        // 阶梯退避：连续打扰计数越大，冷却按 backoff_multiplier 指数拉长
        // （base × mult^count，count 封顶 backoff_max_level），
        // 让"刚打扰过多"的角色自然安静，用户交互后计数归零立即恢复活跃
        let backoff_count = state
            .consecutive_interruptions
            .min(cfg.backoff_max_level);
        let backoff_factor = cfg.backoff_multiplier.powf(backoff_count as f64);
        let effective_cooldown = ((throttle.cooldown_seconds as f64 * mods.cooldown_mult * backoff_factor) as u64)
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
        // 策略 G：social_urge 双向门控
        // current_thought 每 60s 调 LLM 顺便产出 social_urge（0-1），
        // 表示角色"现在想主动搭话"的冲动强度。
        //   urge >= 0.8 → 提前触发（跳过整点/空闲阈值等特定条件）
        //   urge < 0.3  → 拦截规则（时间到但角色不想说话，推迟到下次）
        //   中间值      → 正常规则触发（保底）
        // 规则本身没有语义，时间到不一定适合问候；但 urge 持续低时不问候也合理。
        // 通用门控（冷却/时机分数/概率）已在 check_trigger 前置流程中检查，这里不重复。
        // WelcomeBack 不受影响：它有自己的"用户刚回来"语义，不应被 urge 绕过。
        // 可通过 proactive.enable_social_urge_gating 开关关闭。
        if orchestrator.config.read().enable_social_urge_gating
            && matches!(
                trigger,
                ProactiveTrigger::HourlyGreeting
                    | ProactiveTrigger::IdleGreeting
                    | ProactiveTrigger::Icebreaker
            )
        {
            const URGE_HIGH: f32 = 0.8;
            const URGE_LOW: f32 = 0.3;
            let urge = orchestrator
                .mind
                .read()
                .as_ref()
                .map(|m| m.social_urge_snapshot())
                .unwrap_or(0.5);
            if urge >= URGE_HIGH {
                tracing::debug!(
                    "[check_specific] {} 提前触发：social_urge={:.2} >= {:.2}",
                    trigger.as_str(),
                    urge,
                    URGE_HIGH
                );
                return true;
            }
            if urge < URGE_LOW {
                tracing::debug!(
                    "[check_specific] {} 推迟：social_urge={:.2} < {:.2}",
                    trigger.as_str(),
                    urge,
                    URGE_LOW
                );
                return false;
            }
        }

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
                // 用 away_since 时间戳计算离开时长，替代前端传入的 away_seconds（后者基于
                // lastUserMessageRef，会把"发完消息后空闲"误判为"离开"，导致 30 分钟后回来
                // 触发 WelcomeBack 但用户其实一直在屏幕前）
                let away_duration = orchestrator
                    .away_since
                    .read()
                    .map(|ts| (ctx.now - ts).max(0.0))
                    .unwrap_or(0.0);
                was_away && active && away_duration >= throttle.min_away_seconds as f64
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
                // 室友在线 + 最近 180s 内主动发言过 → 本角色有概率回应/搭话
                // 事件窗口与触发器冷却(180s)对齐，避免窗口短于冷却导致漏掉发言
                let companion = orchestrator.companions_snapshot.read();
                match companion.as_ref() {
                    Some(c) => match c.last_spoke_secs_ago {
                        // 室友最近 180s 内发言过：正常触发条件
                        Some(secs) => secs <= 180.0,
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
            // 日出日落由世界事件驱动（maybe_sunrise_sunset_reminder 在 tick 中专门处理），
            // 不经 check_trigger 通用门控（时机/概率/冷却系数），
            // 此处返回 false 防止常规触发循环在冷却到期后"凭空"生成日出日落问候
            ProactiveTrigger::Sunrise | ProactiveTrigger::Sunset => false,
            // 系统压力 / 主动截屏 / 应用时长 / 深夜未眠 / 音乐切换同为事件驱动，
            // 由 tick 中专门路径处理（maybe_system_pressure_reminder / maybe_screen_peek /
            // maybe_app_duration_reminder / maybe_late_night / maybe_music_changed），
            // 不进常规触发循环
            ProactiveTrigger::SystemPressure
            | ProactiveTrigger::ScreenPeek
            | ProactiveTrigger::AppDuration
            | ProactiveTrigger::LateNight
            | ProactiveTrigger::MusicChanged => false,
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
        self.try_llm_content(trigger, ctx, hour, &router, None)
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
    ///
    /// `extra_hint`：触发器专属的附加上下文（如 ScreenPeek 的屏幕视觉描述），
    /// 无附加上下文时传 `None`。
    fn try_llm_content(
        &self,
        trigger: ProactiveTrigger,
        ctx: &TickContext,
        hour: u32,
        router: &Arc<ModelRouter>,
        extra_hint: Option<&str>,
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
                | ProactiveTrigger::Sunrise
                | ProactiveTrigger::Sunset
                | ProactiveTrigger::SystemPressure
                | ProactiveTrigger::ScreenPeek
                | ProactiveTrigger::AppDuration
                | ProactiveTrigger::LateNight
                | ProactiveTrigger::MusicChanged
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
            // 取最近 6 条对话历史，格式化为 "[相对时间] role: content"
            // 带相对时间标注，让 LLM 感知每条对话距现在多久，避免把旧事说成刚发生
            let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&lang);
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
                        .map(|m| {
                            let ts_prefix = m
                                .timestamp
                                .map(|t| {
                                    let unix = t.timestamp() as f64;
                                    format!(
                                        "[{}] ",
                                        format_relative_time_lang(unix, lang_norm)
                                    )
                                })
                                .unwrap_or_default();
                            format!("{}{}: {}", ts_prefix, m.role, m.content)
                        })
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
            // SystemPressure：注入实时系统指标摘要
            system_hint: if matches!(trigger, ProactiveTrigger::SystemPressure) {
                self.world_provider
                    .read()
                    .as_ref()
                    .and_then(|wp| wp.system_metrics())
                    .map(|m| build_system_hint(&m))
                    .unwrap_or_default()
            } else {
                String::new()
            },
            // ScreenPeek：注入屏幕视觉描述（extra_hint）
            screen_hint: match extra_hint {
                Some(h) if matches!(trigger, ProactiveTrigger::ScreenPeek) => h.to_string(),
                _ => String::new(),
            },
            // AppDuration：注入应用会话摘要（类别 + 连续时长）
            app_duration_hint: if matches!(trigger, ProactiveTrigger::AppDuration) {
                let cat = self.app_session_category.read().clone();
                if cat.is_empty() || cat == "other" {
                    String::new()
                } else {
                    let dur = (ctx.now - *self.app_session_start.read()).max(0.0);
                    format!(
                        "（{}；已连续约 {}）",
                        app_category_label_zh(&cat),
                        format_duration_cn(dur)
                    )
                }
            } else {
                String::new()
            },
            // MusicChanged：注入当前曲目信息（来自刚更新的 SMTC 快照）
            music_hint: if matches!(trigger, ProactiveTrigger::MusicChanged) {
                self.last_music
                    .read()
                    .as_ref()
                    .map(|m| {
                        let source = if m.source_app.is_empty() {
                            String::new()
                        } else {
                            format!("（{}）", m.source_app)
                        };
                        format!("《{}》 - {}{}", m.title, m.artist, source)
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            },
            current_theme: current_effective_theme(),
        };
        let router_clone = router.clone();
        let idle_seconds = ctx.idle_seconds;
        let system_prompt_clone = system_prompt;
        let lang_clone = lang;
        let emitter = self.stream_emitter.clone();

        tokio::task::block_in_place(|| {
            handle.block_on(async move {
                // 记忆检索（含知识库：busy 状态网络搜索获得的信息），让主动问候有真实素材
                // 每条记忆带相对时间标注（如"3小时前"），让 LLM 区分刚发生的事与旧记忆
                let memory_text = if let Some(mem_mgr) = memory_arc.as_ref() {
                    match mem_mgr
                        .search_memories(
                            "最近 用户 兴趣 话题 知识",
                            crate::memory::types::RetrievalStrategy::Hybrid,
                            8,
                        )
                        .await
                    {
                        Ok(items) if !items.is_empty() => {
                            let lang_norm_mem =
                                crate::pipeline::prompt_modules::normalize_lang(&lang_clone);
                            items
                                .iter()
                                .map(|m| {
                                    let imp = (m.importance * 100.0) as u32;
                                    let rel_time =
                                        format_relative_time_lang(m.timestamp, lang_norm_mem);
                                    format!("- {}（{}，重要性:{}%）", m.content, rel_time, imp)
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
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
                    | ProactiveTrigger::BystanderInterjection
                    | ProactiveTrigger::Sunrise
                    | ProactiveTrigger::Sunset
                    | ProactiveTrigger::SystemPressure
                    | ProactiveTrigger::ScreenPeek
                    | ProactiveTrigger::AppDuration
                    | ProactiveTrigger::LateNight
                    | ProactiveTrigger::MusicChanged => {
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
                            idle_seconds,
                        )?
                    }
                    ProactiveTrigger::MemoryRecall => {
                        MemoryRecall::build_messages(&mem, &system_prompt_clone, &lang_clone, &self.char_id, idle_seconds)?
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

    /// 主动旁观插话评估（轻量判断）
    ///
    /// 用户与说话角色 A 对话时，立即用轻量 LLM 判断本角色（旁观者 B）是否有动机插话。
    /// 绕过概率 roll 和 proactive_tick 周期，每次用户普通消息都进行 LLM 判断。
    /// 提示词包含 B 的人设、当前情绪、亲密度、旁观记忆和刚听到的对话，让 LLM 判断 B 是否有动机插话。
    /// 只判断是否插话，不生成插话内容——插话内容由主对话流程生成。
    /// 返回 Some(String) 表示要插话（String 为插话指令，传给 brain.think），None 表示不插话或冷却中。
    pub async fn evaluate_active_bystander_interjection(
        &self,
        user_msg: &str,
        agent_reply: &str,
        speaker_name: &str,
        mood_hint: &str,
        dialogue_history: &str,
        system_prompt: &str,
        intimacy: f64,
        lang: &str,
    ) -> Option<String> {
        let now = chrono::Local::now().timestamp() as f64;
        let trigger = ProactiveTrigger::BystanderInterjection;

        // 冷却检查（BystanderInterjection 120s 冷却 × 角色 cooldown_mult）
        let throttle = TriggerThrottle::get(trigger);
        let cfg = self.config.read().clone();
        let behavior_cfg = crate::character_behavior::get_behavior(&self.char_id);
        let mods = behavior_cfg.trigger_modifiers;
        let effective_cooldown = ((throttle.cooldown_seconds as f64 * mods.cooldown_mult) as u64)
            .max(cfg.min_trigger_interval);
        {
            let state = self.state.read();
            if let Some(&last) = state.last_trigger_times.get(trigger.as_str()) {
                if now - last < effective_cooldown as f64 {
                    tracing::debug!(
                        "[Proactive:{}] 主动旁观插话冷却中，跳过",
                        self.char_id
                    );
                    return None;
                }
            }
        }

        let router = match self.model_router.read().clone() {
            Some(r) => r,
            None => {
                tracing::debug!(
                    "[Proactive:{}] model_router 未设置，跳过主动旁观插话",
                    self.char_id
                );
                return None;
            }
        };

        // 构造轻量判断提示词（只判断是否插话，不生成内容）
        let (scene_text, overheard_label, mood_label, intimacy_label, recent_label, judge_instr) = match crate::pipeline::prompt_modules::normalize_lang(lang) {
            "en" => (
                "Scene: you just overheard a conversation between the user and your roommate. You were NOT part of it — you just happened to be in the same room and heard them. Now decide whether you have a motive to chime in TO THE USER.",
                "What you overheard:",
                "Your current state:",
                "Intimacy with user:",
                "Recent overheard conversations (for reference):",
                "Decide whether you have a motive to chime in right now. Interjection should be occasional — only chime in when:\n- The topic genuinely interests you (your own interests, not your roommate's)\n- You have a unique take or tease\n- The situation naturally invites it\nDo NOT chime in just because you can. Most of the time you should stay silent. If your fatigue is high, or your mood doesn't fit, or the topic is unrelated to you, stay silent.\nReturn JSON: {\"should_interject\": true or false}",
            ),
            "ja" => (
                "シーン：ユーザーとルームメイトの会話を聞いてしまった。あなたは参加していない——たまたま同じ部屋にいて聞こえただけ。今、ユーザーに向けて口を挟む動機があるか判断して。",
                "聞こえた会話：",
                "あなたの現在の状態：",
                "ユーザーとの親密度：",
                "最近聞いた会話（参考）：",
                "今すぐ口を挟む動機があるか判断して。插話は偶発的であるべき——以下の場合のみ挟む:\n- 話題が本当に自分の興味を引いた（ルームメイトの趣味ではなく自分の）\n- 独自の見解やツッコミがある\n- 状況が自然にそれを誘う\n「挟めるから」という理由で挟まない。大抵は黙っているべき。疲労度が高い、または気分が合わない、または話題が自分に関係ない場合は黙っている。\nJSON出力: {\"should_interject\": true または false}",
            ),
            _ => (
                "场景：你刚听到用户和室友的对话。你没有参与——只是碰巧在同一个房间听到了。现在判断你是否有动机对用户插话。",
                "你听到的对话：",
                "你的当前状态：",
                "与用户的亲密度：",
                "最近旁观记忆：",
                "判断你此刻是否有动机插话。插话应该是偶发的——只在以下情况插话:\n- 话题确实引起了你的兴趣（你自己的兴趣，不是室友的）\n- 你有独特的看法或吐槽\n- 情境自然适合插话\n不要因为「能插话就插话」。大多数时候应该保持沉默。如果你当前疲劳度高，或者情绪不适合，或者话题与你无关，就不要插话。\n返回 JSON: {\"should_interject\": true 或 false}",
            ),
        };

        let overheard = format!(
            "[User says to {}] {}\n[{} says to User] {}",
            speaker_name, user_msg, speaker_name, agent_reply
        );

        let mut user_parts: Vec<String> = vec![scene_text.to_string()];
        user_parts.push(format!("{}\n{}", overheard_label, overheard));
        user_parts.push(format!("{}\n{}", mood_label, mood_hint));
        user_parts.push(format!("{} {:.2}", intimacy_label, intimacy));
        if !dialogue_history.is_empty() {
            user_parts.push(format!("{}\n{}", recent_label, dialogue_history));
        }
        user_parts.push(judge_instr.to_string());

        let messages = vec![
            crate::types::response::ChatMessage::system(system_prompt),
            crate::types::response::ChatMessage::user(user_parts.join("\n\n")),
        ];

        let response = match router
            .generate(LLMRequest::new("bystander_judge", messages))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(
                    "[Proactive:{}] 主动旁观插话 LLM 调用失败: {}",
                    self.char_id,
                    e
                );
                return None;
            }
        };

        // 更新冷却时间（无论是否插话，都记录本次评估，避免每条消息都调 LLM）
        {
            let mut state = self.state.write();
            state
                .last_trigger_times
                .insert(trigger.as_str().to_string(), now);
        }

        // 解析判断结果：只提取 should_interject 字段
        let should_interject = Self::parse_interjection_judgment(&response);

        if should_interject {
            tracing::info!(
                "[Proactive:{}] 主动旁观插话评估：决定插话",
                self.char_id
            );
            // 构造插话指令（传给 brain.think 作为 user_input，出现在完整 prompt 末尾）
            let (directive_scene, directive_instr) = match crate::pipeline::prompt_modules::normalize_lang(lang) {
                "en" => (
                    format!("You just overheard a conversation between the user and {}:\n{}\n\nNow you want to chime in.", speaker_name, overheard),
                    "Address the USER, not your roommate. This is you butting into THEIR conversation. You may comment on or tease about the topic you heard, but don't pretend to share your roommate's interests — your own interests are your own.",
                ),
                "ja" => (
                    format!("ユーザーと{}の会話を聞いてしまった:\n{}\n\n今、口を挟みたい。", speaker_name, overheard),
                    "ユーザーに向けて。ルームメイトではなく。これは彼らの会話に割り込むあなた。聞いた話題についてコメントしたりからかったりするのはいいが、ルームメイトの趣味を自分のもののように装わないで——あなたの趣味はあなた自身のもの。",
                ),
                _ => (
                    format!("你刚听到用户和{}的对话:\n{}\n\n现在你想插话。", speaker_name, overheard),
                    "对用户说，不是对室友。这是你插进他们的对话。你可以评论或吐槽听到的话题，但不要假装和室友有同样的兴趣——你的兴趣是你自己的。",
                ),
            };
            Some(format!("{}\n\n{}", directive_scene, directive_instr))
        } else {
            tracing::debug!(
                "[Proactive:{}] 主动旁观插话评估：决定不插话",
                self.char_id
            );
            None
        }
    }

    /// 解析插话判断 LLM 响应
    ///
    /// 提取 should_interject 布尔字段。容错多种格式（true/false/1/0/yes/no）。
    fn parse_interjection_judgment(response: &str) -> bool {
        // 提取首个 { 到末个 } 的子串
        let start = match response.find('{') {
            Some(s) => s,
            None => return false,
        };
        let end = match response.rfind('}') {
            Some(e) => e + 1,
            None => return false,
        };
        if end <= start {
            return false;
        }
        let json_str = &response[start..end];
        let val: serde_json::Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => return false,
        };
        if let Some(b) = val.get("should_interject").and_then(|v| v.as_bool()) {
            return b;
        }
        // 容错：字符串形式
        if let Some(s) = val.get("should_interject").and_then(|v| v.as_str()) {
            let lower = s.to_lowercase();
            return matches!(lower.as_str(), "true" | "1" | "yes" | "y");
        }
        false
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
        // 阶梯退避：成功推送一次主动消息 → 连续打扰计数 +1（用户交互时归零）
        // 后续 check_trigger 冷却按指数拉长，实现"打扰越多间隔越长"的渐进收敛
        {
            let max_level = self.config.read().backoff_max_level;
            let mut state = self.state.write();
            state.consecutive_interruptions =
                (state.consecutive_interruptions + 1).min(max_level + 2);
            state.last_interruption_at = chrono::Local::now().timestamp() as f64;
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
        // 注意：不更新 last_interaction_time —— 该字段应只由真实用户交互更新（on_user_interacted）。
        // 主动消息发出后覆盖它会导致 GREETING_SUPPRESSION_AFTER_INTERACTION_SECS 误判，
        // 把"刚发了主动消息"当成"用户刚交互"，从而抑制后续 5 分钟内的合理问候。
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
            *self.away_since.write() = None;
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
            // 用户真实交互：退避计数归零，让角色获得"重新活跃"的资格
            state.consecutive_interruptions = 0;
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
            // 被忽略 = 一次无效打扰：同步递增退避计数，让下次打扰间隔拉长，
            // 与 speech_desire 的热度提升互补（越不理越热情，但越少尝试）
            let max_level = self.config.read().backoff_max_level;
            state.consecutive_interruptions =
                (state.consecutive_interruptions + 1).min(max_level + 2);
            state.last_interruption_at = chrono::Local::now().timestamp() as f64;
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
                away_since: RwLock::new(None),
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
                memory_pressure_active: RwLock::new(false),
                last_screen_peek_denied: Arc::new(RwLock::new(None)),
                app_session_category: RwLock::new(String::new()),
                app_session_start: RwLock::new(now_ts),
                last_music: RwLock::new(None),
                last_late_night_date: RwLock::new(String::new()),
            }
        })
    }
}

use chrono::Datelike;
