//! 打扰控制器 (InterruptionController)。
//!
//! 1. 监测用户活动状态（键盘、鼠标、窗口）
//! 2. 根据用户状态动态调整打扰阈值
//! 3. 决定是否允许桌宠主动打扰用户
//! 4. 维护打扰频率和冷却机制
//! 5. 作息模型学习 + 破冰策略
//!

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// 用户活动级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserActivityLevel {
    VeryActive,
    Active,
    Normal,
    Idle,
    VeryIdle,
}

impl Default for UserActivityLevel {
    fn default() -> Self {
        Self::Normal
    }
}

impl UserActivityLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::VeryActive => "very_active",
            Self::Active => "active",
            Self::Normal => "normal",
            Self::Idle => "idle",
            Self::VeryIdle => "very_idle",
        }
    }
}

/// 用户打扰容忍度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserInterruptionTolerance {
    High,
    Medium,
    Low,
}

impl Default for UserInterruptionTolerance {
    fn default() -> Self {
        Self::Medium
    }
}

impl UserInterruptionTolerance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// 破冰强度等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IceBreakerLevel {
    None,
    Gentle,
    Warm,
    Reengage,
}

impl IceBreakerLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gentle => "gentle",
            Self::Warm => "warm",
            Self::Reengage => "reengage",
        }
    }
}

/// 打扰优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptPriority {
    Normal,
    High,
    Urgent,
}

impl InterruptPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
}

/// 打扰判定结果。
#[derive(Debug, Clone, Serialize)]
pub struct InterruptionDecision {
    pub allowed: bool,
    pub reason: String,
}

/// 打扰就绪状态快照。
#[derive(Debug, Clone, Serialize)]
pub struct InterruptionReadiness {
    pub can_interrupt: bool,
    pub reason: String,
    pub activity_level: String,
    pub tolerance: String,
    pub idle_seconds: u64,
    pub interruptions_this_hour: u32,
    pub consecutive_ignores: u32,
    pub consecutive_responses: u32,
    pub cooldown_remaining: u64,
    pub icebreaker_level: String,
    pub user_expected_active: bool,
    pub schedule_confidence: f64,
}

/// 打扰控制器状态（受 Mutex 保护）。
struct ControllerState {
    /// 上次用户活动时间（单调时钟，秒）
    last_activity_monotonic: Instant,
    /// 上次用户活动时间戳（Unix 秒）
    last_activity_ts: f64,
    /// 活动分数 [0, 1]
    activity_score: f64,
    /// 当前活动级别
    activity_level: UserActivityLevel,
    /// 连续无视次数
    consecutive_ignores: u32,
    /// 连续回应次数
    consecutive_responses: u32,
    /// 当前小时窗口内打扰次数
    interruption_count: u32,
    /// 当前小时窗口起始时间戳
    interruption_window_start: f64,
    /// 当前容忍度
    current_tolerance: UserInterruptionTolerance,
    /// 操作模式
    operating_mode: String,
    /// 强制静默截止时间戳
    force_silent_until: f64,
    /// 打扰历史 (timestamp, succeeded)
    interruption_history: VecDeque<(f64, bool)>,
    /// 上次打扰时间戳
    last_interruption_time: f64,
    /// 作息槽位计数（slot_idx → count）
    schedule_slots: HashMap<u32, u32>,
    /// 作息置信度 [0, 1]
    schedule_confidence: f64,
    /// 总作息样本数
    total_schedule_samples: u32,
    /// 最近 50 次互动结果（true=回应，false=无视）
    interaction_outcomes: VecDeque<bool>,
    /// 上次用户互动时间戳（用于破冰判定）
    last_user_interaction_time: f64,
}

impl ControllerState {
    fn new() -> Self {
        let now = now_ts();
        Self {
            last_activity_monotonic: Instant::now(),
            last_activity_ts: now,
            activity_score: 0.0,
            activity_level: UserActivityLevel::Normal,
            consecutive_ignores: 0,
            consecutive_responses: 0,
            interruption_count: 0,
            interruption_window_start: now,
            current_tolerance: UserInterruptionTolerance::Medium,
            operating_mode: "normal".to_string(),
            force_silent_until: 0.0,
            interruption_history: VecDeque::with_capacity(20),
            last_interruption_time: 0.0,
            schedule_slots: HashMap::new(),
            schedule_confidence: 0.0,
            total_schedule_samples: 0,
            interaction_outcomes: VecDeque::with_capacity(50),
            last_user_interaction_time: now,
        }
    }
}

/// 打扰控制器。
pub struct InterruptionController {
    inner: Arc<Mutex<ControllerState>>,
}

impl InterruptionController {
    // 配置常量
    pub const COOLDOWN_BASE: u64 = 60;
    pub const COOLDOWN_AFTER_IGNORE: u64 = 120;
    pub const COOLDOWN_AFTER_SUCCESS: u64 = 30;
    pub const IDLE_THRESHOLD: u64 = 120;
    pub const VERY_IDLE_THRESHOLD: u64 = 600;
    pub const MAX_INTERRUPTIONS_PER_HOUR: u32 = 6;
    pub const SCHEDULE_SLOT_MINUTES: u32 = 30;
    pub const GENTLE_THRESHOLD_MINUTES: f64 = 30.0;
    pub const WARM_THRESHOLD_MINUTES: f64 = 180.0;
    pub const REENGAGE_THRESHOLD_HOURS: f64 = 24.0;

    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ControllerState::new())),
        }
    }

    /// 记录用户活动（键盘/鼠标/触摸）。
    pub fn record_user_activity(&self) {
        let now = now_ts();
        let mut state = self.inner.lock();
        state.last_activity_monotonic = Instant::now();
        state.last_activity_ts = now;
        state.activity_score = (state.activity_score + 0.3).min(1.0);
        state.last_user_interaction_time = now;

        // 学习作息槽位
        let slot = Self::current_schedule_slot();
        *state.schedule_slots.entry(slot).or_insert(0) += 1;
        state.total_schedule_samples += 1;
        state.schedule_confidence = (state.total_schedule_samples as f64 / 50.0).min(1.0);
    }

    /// 记录键盘活动。
    pub fn record_keyboard(&self) {
        self.record_user_activity();
    }

    /// 记录鼠标活动。
    pub fn record_mouse(&self) {
        self.record_user_activity();
    }

    /// 记录用户正面回应。
    pub fn record_user_response(&self) {
        let mut state = self.inner.lock();
        state.consecutive_responses += 1;
        state.consecutive_ignores = 0;
        if state.interaction_outcomes.len() >= 50 {
            state.interaction_outcomes.pop_front();
        }
        state.interaction_outcomes.push_back(true);
    }

    /// 记录用户无视。
    pub fn record_user_ignored(&self) {
        let mut state = self.inner.lock();
        state.consecutive_ignores += 1;
        state.consecutive_responses = 0;
        if state.interaction_outcomes.len() >= 50 {
            state.interaction_outcomes.pop_front();
        }
        state.interaction_outcomes.push_back(false);
    }

    /// 判断当前是否适合打扰。
    pub fn should_interrupt(&self, priority: InterruptPriority) -> InterruptionDecision {
        let now = now_ts();
        let mut state = self.inner.lock();

        // 强制静默期（urgent 也遵守）
        if now < state.force_silent_until {
            return InterruptionDecision {
                allowed: false,
                reason: format!(
                    "force_silent: {:.0}s remaining",
                    state.force_silent_until - now
                ),
            };
        }

        // 紧急打扰机制
        if priority == InterruptPriority::Urgent {
            let idle_seconds = state.last_activity_monotonic.elapsed().as_secs();
            if idle_seconds > 900 {
                return InterruptionDecision {
                    allowed: true,
                    reason: "urgent_idle_break".to_string(),
                };
            }
            if now - state.last_interruption_time >= Self::COOLDOWN_BASE as f64 {
                return InterruptionDecision {
                    allowed: true,
                    reason: "urgent_priority_ok".to_string(),
                };
            }
            let remaining = Self::COOLDOWN_BASE as f64 - (now - state.last_interruption_time);
            return InterruptionDecision {
                allowed: false,
                reason: format!("urgent_cooldown:{:.0}s", remaining),
            };
        }

        // 活动级别检查（高优先级跳过）
        if priority != InterruptPriority::High {
            if matches!(
                state.activity_level,
                UserActivityLevel::VeryActive | UserActivityLevel::Active
            ) {
                return InterruptionDecision {
                    allowed: false,
                    reason: format!("user_active:{}", state.activity_level.as_str()),
                };
            }
        }

        // 冷却检查
        if now - state.last_interruption_time < Self::COOLDOWN_BASE as f64 {
            let remaining = Self::COOLDOWN_BASE as f64 - (now - state.last_interruption_time);
            return InterruptionDecision {
                allowed: false,
                reason: format!("cooldown:{:.0}s", remaining),
            };
        }

        // 打扰频率检查
        if now - state.interruption_window_start > 3600.0 {
            state.interruption_count = 0;
            state.interruption_window_start = now;
        }
        if state.interruption_count >= Self::MAX_INTERRUPTIONS_PER_HOUR {
            return InterruptionDecision {
                allowed: false,
                reason: "max_interruptions_reached".to_string(),
            };
        }

        // 作息模型检查
        if state.schedule_confidence > 0.3 {
            let slot = Self::current_schedule_slot();
            if state.schedule_slots.get(&slot).copied().unwrap_or(0) == 0 {
                return InterruptionDecision {
                    allowed: false,
                    reason: "user_typically_inactive_now".to_string(),
                };
            }
        }

        InterruptionDecision {
            allowed: true,
            reason: "ok".to_string(),
        }
    }

    /// 记录一次打扰。
    pub fn record_interruption(&self) {
        let now = now_ts();
        let mut state = self.inner.lock();
        state.last_interruption_time = now;
        state.interruption_count += 1;
        state.interruption_history.push_back((now, false));
        if state.interruption_history.len() > 20 {
            state.interruption_history.pop_front();
        }
    }

    /// 获取打扰就绪状态快照。
    pub fn get_interruption_readiness(&self) -> InterruptionReadiness {
        let decision = self.should_interrupt(InterruptPriority::Normal);
        let now = now_ts();
        let state = self.inner.lock();

        InterruptionReadiness {
            can_interrupt: decision.allowed,
            reason: decision.reason,
            activity_level: state.activity_level.as_str().to_string(),
            tolerance: state.current_tolerance.as_str().to_string(),
            idle_seconds: state.last_activity_monotonic.elapsed().as_secs(),
            interruptions_this_hour: state.interruption_count,
            consecutive_ignores: state.consecutive_ignores,
            consecutive_responses: state.consecutive_responses,
            cooldown_remaining: ((Self::COOLDOWN_BASE as f64
                - (now - state.last_interruption_time))
                .max(0.0)) as u64,
            icebreaker_level: Self::compute_icebreaker_level_locked(&state).as_str().to_string(),
            user_expected_active: Self::is_user_expected_active_locked(&state),
            schedule_confidence: state.schedule_confidence,
        }
    }

    /// 获取最活跃时段（按活跃度排序，最多 6 个）。
    pub fn get_active_hours(&self) -> Vec<u32> {
        let state = self.inner.lock();
        let mut hourly: HashMap<u32, u32> = HashMap::new();
        for (&slot_idx, &count) in state.schedule_slots.iter() {
            let hour = slot_idx * Self::SCHEDULE_SLOT_MINUTES / 60;
            *hourly.entry(hour).or_insert(0) += count;
        }
        let mut sorted: Vec<(u32, u32)> = hourly.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.into_iter().take(6).map(|(h, _)| h).collect()
    }

    /// 根据作息模型预测用户当前是否可能活跃。
    pub fn is_user_expected_active(&self) -> bool {
        let state = self.inner.lock();
        Self::is_user_expected_active_locked(&state)
    }

    fn is_user_expected_active_locked(state: &ControllerState) -> bool {
        if state.schedule_confidence < 0.3 {
            return true;
        }
        let slot = Self::current_schedule_slot();
        state.schedule_slots.get(&slot).copied().unwrap_or(0) > 0
    }

    /// 获取最不活跃的安静时段。
    pub fn get_silent_hours(&self) -> Vec<u32> {
        let state = self.inner.lock();
        if state.schedule_slots.is_empty() || state.schedule_confidence < 0.2 {
            return (0..6).collect();
        }
        let mut hourly: HashMap<u32, u32> = HashMap::new();
        for (&slot_idx, &count) in state.schedule_slots.iter() {
            let hour = slot_idx * Self::SCHEDULE_SLOT_MINUTES / 60;
            *hourly.entry(hour).or_insert(0) += count;
        }
        let mut sorted: Vec<(u32, u32)> = hourly.into_iter().collect();
        sorted.sort_by(|a, b| a.1.cmp(&b.1));
        sorted.into_iter().take(4).map(|(h, _)| h).collect()
    }

    /// 计算当前需要的破冰强度。
    pub fn get_icebreaker_level(&self) -> IceBreakerLevel {
        let state = self.inner.lock();
        Self::compute_icebreaker_level_locked(&state)
    }

    fn compute_icebreaker_level_locked(state: &ControllerState) -> IceBreakerLevel {
        let elapsed_min = (now_ts() - state.last_user_interaction_time) / 60.0;

        if elapsed_min < Self::GENTLE_THRESHOLD_MINUTES {
            return IceBreakerLevel::None;
        }

        let positive = state.interaction_outcomes.iter().filter(|&&x| x).count();
        let total = state.interaction_outcomes.len();
        let response_rate = if total > 0 {
            positive as f64 / total as f64
        } else {
            0.5
        };

        if elapsed_min >= Self::REENGAGE_THRESHOLD_HOURS * 60.0 {
            return if response_rate >= 0.3 {
                IceBreakerLevel::Reengage
            } else {
                IceBreakerLevel::None
            };
        }
        if elapsed_min >= Self::WARM_THRESHOLD_MINUTES {
            return if response_rate >= 0.3 {
                IceBreakerLevel::Warm
            } else {
                IceBreakerLevel::Gentle
            };
        }
        if elapsed_min >= Self::GENTLE_THRESHOLD_MINUTES {
            return if response_rate >= 0.3 {
                IceBreakerLevel::Gentle
            } else {
                IceBreakerLevel::None
            };
        }

        IceBreakerLevel::None
    }

    /// 生成破冰策略提示词。
    pub fn get_icebreaker_prompt(&self) -> String {
        let level = self.get_icebreaker_level();
        match level {
            IceBreakerLevel::None => String::new(),
            IceBreakerLevel::Gentle => {
                "Tips: The user hasn't interacted for a short while. Be natural and light."
                    .to_string()
            }
            IceBreakerLevel::Warm => {
                "Tips: It's been a few hours. Show warmth and casual curiosity.".to_string()
            }
            IceBreakerLevel::Reengage => {
                "Tips: The user has been gone long. Express gentle happiness to see them back."
                    .to_string()
            }
        }
    }

    /// 设置静默模式（持续 duration_seconds 秒）。
    pub fn set_silent_mode(&self, duration_seconds: u64) {
        let mut state = self.inner.lock();
        state.force_silent_until = now_ts() + duration_seconds as f64;
        tracing::debug!(duration = duration_seconds, "Silent mode set");
    }

    /// 设置操作模式。
    pub fn set_operating_mode(&self, mode: impl Into<String>) {
        let mut state = self.inner.lock();
        state.operating_mode = mode.into();
    }

    /// 获取当前状态的可读字符串。
    pub fn get_current_status(&self) -> String {
        let decision = self.should_interrupt(InterruptPriority::Normal);
        let level = self.get_icebreaker_level();
        let state = self.inner.lock();
        let now = now_ts();
        format!(
            "activity={} | tolerance={} | can_interrupt={} | cooldown={:.0}s | icebreaker={} | schedule_conf={:.0}%",
            state.activity_level.as_str(),
            state.current_tolerance.as_str(),
            decision.allowed,
            (Self::COOLDOWN_BASE as f64 - (now - state.last_interruption_time)).max(0.0),
            level.as_str(),
            state.schedule_confidence * 100.0
        )
    }

    /// 获取当前活动级别。
    pub fn get_activity_level(&self) -> UserActivityLevel {
        self.inner.lock().activity_level
    }

    /// 计算当前作息槽位索引。
    fn current_schedule_slot() -> u32 {
        use chrono::Timelike;
        let now = chrono::Local::now();
        (now.hour() * 60 + now.minute()) as u32 / Self::SCHEDULE_SLOT_MINUTES
    }
}

impl Default for InterruptionController {
    fn default() -> Self {
        Self::new()
    }
}

// ── 全局单例 ──

use tokio::sync::OnceCell;

static GLOBAL_CONTROLLER: OnceCell<Arc<InterruptionController>> = OnceCell::const_new();

/// 获取全局 InterruptionController 单例。
pub async fn get_interruption_controller() -> Arc<InterruptionController> {
    GLOBAL_CONTROLLER
        .get_or_init(|| async { Arc::new(InterruptionController::new()) })
        .await
        .clone()
}

/// 同步获取全局单例（若已初始化）。
pub fn try_get_interruption_controller() -> Option<Arc<InterruptionController>> {
    GLOBAL_CONTROLLER.get().cloned()
}

// ── 工具函数 ──

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_activity() {
        let ctrl = InterruptionController::new();
        ctrl.record_user_activity();
        let state = ctrl.inner.lock();
        assert!(state.activity_score > 0.0);
        assert!(state.total_schedule_samples > 0);
    }

    #[test]
    fn test_silent_mode_blocks_all() {
        let ctrl = InterruptionController::new();
        ctrl.set_silent_mode(60);
        let decision = ctrl.should_interrupt(InterruptPriority::Urgent);
        assert!(!decision.allowed);
        assert!(decision.reason.starts_with("force_silent"));
    }

    #[test]
    fn test_cooldown_after_interruption() {
        let ctrl = InterruptionController::new();
        ctrl.record_interruption();
        let decision = ctrl.should_interrupt(InterruptPriority::Normal);
        assert!(!decision.allowed);
        assert!(decision.reason.starts_with("cooldown"));
    }

    #[test]
    fn test_response_tracking() {
        let ctrl = InterruptionController::new();
        ctrl.record_user_response();
        ctrl.record_user_response();
        let state = ctrl.inner.lock();
        assert_eq!(state.consecutive_responses, 2);
        assert_eq!(state.consecutive_ignores, 0);
    }

    #[test]
    fn test_icebreaker_level_none_when_recent() {
        let ctrl = InterruptionController::new();
        ctrl.record_user_activity(); // 重置 last_user_interaction_time
        let level = ctrl.get_icebreaker_level();
        assert_eq!(level, IceBreakerLevel::None);
    }
}
