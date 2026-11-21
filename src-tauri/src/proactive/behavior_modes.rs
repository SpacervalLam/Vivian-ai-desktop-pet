//! 行为模式管理器
//!
//! 三种陪伴模式：
//! - Shadow（影随）：用户专注工作时安静跟随
//! - Guardian（守护）：深夜温柔提醒休息
//! - Companion（陪伴）：默认正常活跃

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 桌宠行为模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PetBehaviorMode {
    /// 影随模式：暗中跟随，不打扰
    Shadow,
    /// 守护模式：深夜守护，温柔提醒
    Guardian,
    /// 陪伴模式：正常活跃（默认）
    Companion,
}

impl PetBehaviorMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PetBehaviorMode::Shadow => "shadow",
            PetBehaviorMode::Guardian => "guardian",
            PetBehaviorMode::Companion => "companion",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "shadow" => PetBehaviorMode::Shadow,
            "guardian" => PetBehaviorMode::Guardian,
            _ => PetBehaviorMode::Companion,
        }
    }

    /// 模式描述
    pub fn description(&self) -> &'static str {
        match self {
            PetBehaviorMode::Shadow => "影随模式 — 在用户专注工作时安静跟随",
            PetBehaviorMode::Guardian => "守护模式 — 深夜守护，温柔提醒",
            PetBehaviorMode::Companion => "陪伴模式 — 正常活跃",
        }
    }

    /// 跟随策略
    pub fn follow_strategy(&self) -> &'static str {
        match self {
            PetBehaviorMode::Shadow => "edge",
            PetBehaviorMode::Guardian => "near",
            PetBehaviorMode::Companion => "normal",
        }
    }

    /// 互动频率
    pub fn interaction_frequency(&self) -> &'static str {
        match self {
            PetBehaviorMode::Shadow => "low",
            PetBehaviorMode::Guardian => "moderate",
            PetBehaviorMode::Companion => "normal",
        }
    }

    /// 是否显示气泡
    pub fn show_bubble(&self) -> bool {
        !matches!(self, PetBehaviorMode::Shadow)
    }

    /// 动画风格
    pub fn animation(&self) -> &'static str {
        match self {
            PetBehaviorMode::Shadow => "subtle",
            PetBehaviorMode::Guardian => "gentle",
            PetBehaviorMode::Companion => "lively",
        }
    }

    /// 模式冷却系数（影随/守护时降低主动频率）
    pub fn cooling_multiplier(&self) -> f64 {
        match self {
            PetBehaviorMode::Shadow => 0.3,
            PetBehaviorMode::Guardian => 0.7,
            PetBehaviorMode::Companion => 1.0,
        }
    }
}

/// 行为模式内部状态
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BehaviorModeState {
    pub current_mode: String,
    pub previous_mode: String,
    pub mode_changed_at: f64,
    pub last_mode_switch_time: f64,
    pub current_app_category: String,
    pub app_category_start_time: f64,
    pub last_user_activity: f64,
    pub is_meeting: bool,
}

/// 行为模式管理器
pub struct BehaviorModeManager {
    state: RwLock<BehaviorModeState>,
}

impl BehaviorModeManager {
    const SHADOW_TRIGGER_IDLE_MINUTES: f64 = 10.0;
    const SHADOW_TRIGGER_MEETING_MINUTES: f64 = 2.0;
    const GUARDIAN_START_HOUR: u32 = 22;
    const GUARDIAN_END_HOUR: u32 = 6;
    const MODE_COOLDOWN_SECONDS: f64 = 300.0;

    pub fn new(now: f64) -> Self {
        let state = BehaviorModeState {
            current_mode: PetBehaviorMode::Companion.as_str().to_string(),
            previous_mode: PetBehaviorMode::Companion.as_str().to_string(),
            mode_changed_at: now,
            last_mode_switch_time: 0.0,
            last_user_activity: now,
            ..Default::default()
        };
        Self {
            state: RwLock::new(state),
        }
    }

    /// 每 tick 调用，评估并切换模式
    pub fn update(&self, app_category: &str, user_active: bool, now: f64, hour: u32) {
        let mut state = self.state.write();
        // 冷却检查
        if now - state.last_mode_switch_time < Self::MODE_COOLDOWN_SECONDS {
            // 仍更新追踪状态
            if user_active {
                state.last_user_activity = now;
            }
            Self::update_app_tracking(&mut state, app_category, now);
            return;
        }
        if user_active {
            state.last_user_activity = now;
        }
        Self::update_app_tracking(&mut state, app_category, now);

        let new_mode = Self::evaluate_mode(&state, now, hour);
        let current = PetBehaviorMode::from_str(&state.current_mode);
        if new_mode != current {
            state.previous_mode = state.current_mode.clone();
            state.current_mode = new_mode.as_str().to_string();
            state.mode_changed_at = now;
            state.last_mode_switch_time = now;
            tracing::info!(
                "[BehaviorMode] 模式切换: {} → {}",
                state.previous_mode,
                new_mode.as_str()
            );
        }
    }

    fn update_app_tracking(state: &mut BehaviorModeState, app_category: &str, now: f64) {
        if app_category != state.current_app_category {
            state.current_app_category = app_category.to_string();
            state.app_category_start_time = now;
        }
        // 判断是否在开会
        let meeting_apps = ["meeting", "presentation", "zoom", "teams", "slack"];
        state.is_meeting = meeting_apps
            .iter()
            .any(|m| state.current_app_category.contains(m));
    }

    fn evaluate_mode(state: &BehaviorModeState, now: f64, hour: u32) -> PetBehaviorMode {
        // 守护模式优先级最高
        if Self::should_be_guardian(state, now, hour) {
            return PetBehaviorMode::Guardian;
        }
        if Self::should_be_shadow(state, now) {
            return PetBehaviorMode::Shadow;
        }
        PetBehaviorMode::Companion
    }

    fn should_be_guardian(state: &BehaviorModeState, now: f64, hour: u32) -> bool {
        let is_night = hour >= Self::GUARDIAN_START_HOUR || hour < Self::GUARDIAN_END_HOUR;
        if !is_night {
            return false;
        }
        // 用户 10 分钟无活动 → 可能已离开
        if now - state.last_user_activity > 600.0 {
            return false;
        }
        true
    }

    fn should_be_shadow(state: &BehaviorModeState, now: f64) -> bool {
        // 会议中 → 持续 2 分钟进入影随
        if state.is_meeting {
            let duration = now - state.app_category_start_time;
            return duration >= Self::SHADOW_TRIGGER_MEETING_MINUTES * 60.0;
        }
        // 工作应用持续 10 分钟 → 影随
        // 与 SmartAppClassifier 9 分类对齐：coding/office（不含 reading，归入 other）
        let work_categories = ["coding", "office"];
        if work_categories.iter().any(|c| state.current_app_category == *c) {
            let duration = now - state.app_category_start_time;
            return duration >= Self::SHADOW_TRIGGER_IDLE_MINUTES * 60.0;
        }
        false
    }

    /// 获取当前模式
    pub fn get_current_mode(&self) -> PetBehaviorMode {
        PetBehaviorMode::from_str(&self.state.read().current_mode)
    }

    /// 获取模式配置
    pub fn get_mode_config(&self) -> serde_json::Value {
        let mode = self.get_current_mode();
        serde_json::json!({
            "follow_strategy": mode.follow_strategy(),
            "interaction_frequency": mode.interaction_frequency(),
            "show_bubble": mode.show_bubble(),
            "animation": mode.animation(),
        })
    }

    /// 强制设置模式
    pub fn force_mode(&self, mode: PetBehaviorMode, now: f64) {
        let mut state = self.state.write();
        let current = PetBehaviorMode::from_str(&state.current_mode);
        if mode != current {
            state.previous_mode = state.current_mode.clone();
            state.current_mode = mode.as_str().to_string();
            state.mode_changed_at = now;
            state.last_mode_switch_time = now;
        }
    }

    /// 获取状态
    pub fn get_status(&self) -> serde_json::Value {
        let state = self.state.read();
        let mode = PetBehaviorMode::from_str(&state.current_mode);
        serde_json::json!({
            "current_mode": state.current_mode,
            "previous_mode": state.previous_mode,
            "mode_changed_at": state.mode_changed_at,
            "description": mode.description(),
            "config": self.get_mode_config(),
            "app_category": state.current_app_category,
            "is_meeting": state.is_meeting,
        })
    }
}
