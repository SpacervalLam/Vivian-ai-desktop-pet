//! 自动表情/动作触发引擎
//!
//! 提供非LLM触发路径：
//! - 空闲检测：用户长时间不交互时，逐步触发无聊/打哈欠/困倦/睡眠表情
//! - 用户回来：用户从长时空闲回来时，触发惊喜/挥手
//! - 心情持续：当前主导情绪持续一段时间后，触发心情对应表情
//! - 程序事件：时间（早/午/晚/夜）、窗口聚焦/失焦、对话开始/结束、音乐播放等
//!
//! 设计原则：
//! 1. 纯规则驱动，不调用LLM，响应即时
//! 2. 概率触发 + 冷却时间，避免机械重复
//! 3. 尊重 ResourceManifest 映射，不同角色可自定义
//! 4. 通过 PetActionRequest 队列与前端通信，统一动作投递

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use tracing::debug;

use crate::engine::manifest::ResourceManifest;

/// 触发器结果：(expression, motion, action, duration_ms, probability)
type TriggerResult = (String, String, String, Option<u64>, f64);

/// 空闲阶段定义
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdleStage {
    /// 活跃（刚刚交互过）
    Active = 0,
    /// 短时空闲（30秒-2分钟）
    Short = 1,
    /// 中等空闲（2-5分钟）
    Medium = 2,
    /// 长时空闲（5-15分钟）
    Long = 3,
    /// 睡眠状态（15分钟以上）
    Asleep = 4,
}

impl IdleStage {
    fn from_seconds(secs: u64) -> Self {
        match secs {
            0..=30 => IdleStage::Active,
            31..=120 => IdleStage::Short,
            121..=300 => IdleStage::Medium,
            301..=900 => IdleStage::Long,
            _ => IdleStage::Asleep,
        }
    }
}

impl Default for IdleStage {
    fn default() -> Self {
        IdleStage::Active
    }
}

/// 单个角色的自动触发状态
#[derive(Debug)]
struct TriggerState {
    /// 上次用户交互时间
    last_interaction: Instant,
    /// 当前空闲阶段
    last_idle_stage: IdleStage,
    /// 已触发过的空闲阶段（避免同一阶段重复触发）
    triggered_idle_stages: HashSet<IdleStage>,
    /// 当前主导情绪标签
    current_mood_label: String,
    /// 上次心情表情触发时间
    last_mood_idle_time: Instant,
    /// 事件触发冷却表
    event_cooldowns: HashMap<String, Instant>,
}

impl Default for TriggerState {
    fn default() -> Self {
        Self {
            last_interaction: Instant::now(),
            last_idle_stage: IdleStage::Active,
            triggered_idle_stages: HashSet::new(),
            current_mood_label: "neutral".to_string(),
            last_mood_idle_time: Instant::now(),
            event_cooldowns: HashMap::new(),
        }
    }
}

/// 全局自动表情触发引擎
pub struct AutoExpressionTrigger {
    states: RwLock<HashMap<String, TriggerState>>,
}

impl AutoExpressionTrigger {
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
        }
    }

    /// 获取或初始化角色状态并执行闭包
    fn with_state<F, R>(&self, char_id: &str, f: F) -> R
    where
        F: FnOnce(&mut TriggerState) -> R,
    {
        let mut states = self.states.write();
        if !states.contains_key(char_id) {
            states.insert(char_id.to_string(), TriggerState::default());
        }
        let state = states.get_mut(char_id).unwrap();
        f(state)
    }

    /// 记录用户交互（重置空闲计时），返回之前是否处于长时空闲
    pub fn record_interaction(&self, char_id: &str) -> bool {
        self.with_state(char_id, |state| {
            let was_long_idle = state.last_idle_stage >= IdleStage::Long;
            state.last_interaction = Instant::now();
            state.triggered_idle_stages.clear();
            state.last_mood_idle_time = Instant::now();
            state.last_idle_stage = IdleStage::Active;
            was_long_idle
        })
    }

    /// 更新当前心情状态，返回是否需要触发情绪变化表情
    pub fn update_mood(&self, char_id: &str, manifest: &ResourceManifest, mood_label: &str, emotion_intensity: f64) -> Option<TriggerResult> {
        let old_label = self.with_state(char_id, |state| {
            let old = state.current_mood_label.clone();
            state.current_mood_label = mood_label.to_string();
            old
        });

        if old_label != mood_label && emotion_intensity > 0.4 {
            return manifest.get_event_trigger(&format!("mood_change_{}", mood_label));
        }
        None
    }

    /// 检查事件冷却，返回是否可以触发
    fn check_event_cooldown(&self, char_id: &str, event_key: &str, cooldown: Duration) -> bool {
        self.with_state(char_id, |state| {
            let now = Instant::now();
            if let Some(next_time) = state.event_cooldowns.get(event_key) {
                if now < *next_time {
                    return false;
                }
            }
            state.event_cooldowns.insert(event_key.to_string(), now + cooldown);
            true
        })
    }

    /// tick 检查：空闲检测 + 心情持续表情
    pub fn tick(&self, char_id: &str, manifest: &ResourceManifest) -> Vec<TriggerResult> {
        let mut results = Vec::new();
        let now = Instant::now();

        self.with_state(char_id, |state| {
            let idle_secs = now.saturating_duration_since(state.last_interaction).as_secs();
            let current_stage = IdleStage::from_seconds(idle_secs);

            // 阶段变化检测
            if current_stage != state.last_idle_stage {
                let old_stage = state.last_idle_stage;
                state.last_idle_stage = current_stage;

                // 前进到新阶段（只在阶段升级时触发）
                if current_stage > old_stage {
                    let trigger_key = match current_stage {
                        IdleStage::Short => "idle_short",
                        IdleStage::Medium => "idle_medium",
                        IdleStage::Long => "idle_long",
                        IdleStage::Asleep => "idle_asleep",
                        _ => "",
                    };

                    if !trigger_key.is_empty() && !state.triggered_idle_stages.contains(&current_stage) {
                        let base_prob = match current_stage {
                            IdleStage::Short => 0.4,
                            IdleStage::Medium => 0.6,
                            IdleStage::Long => 0.8,
                            IdleStage::Asleep => 0.95,
                            _ => 0.0,
                        };

                        let should_trigger = rand::random::<f64>() < base_prob;
                        if should_trigger {
                            if let Some(trigger) = manifest.get_idle_trigger(trigger_key) {
                                results.push(trigger);
                                state.triggered_idle_stages.insert(current_stage);
                            }
                        }
                    }
                }
            }

            // 心情持续表情：主导情绪持续超过45秒后，随机触发心情表情（有冷却）
            let mood_cooldown = Duration::from_secs(45);
            if now.saturating_duration_since(state.last_mood_idle_time) > mood_cooldown {
                if current_stage >= IdleStage::Short {
                    let mood_label = &state.current_mood_label;
                    // get_mood_idle_expression返回(expr, priority)，包装成TriggerResult
                    if let Some((expr_name, _priority)) = manifest.get_mood_idle_expression(mood_label) {
                        if rand::random::<f64>() < 0.25 {
                            results.push((expr_name, String::new(), String::new(), Some(3000), 0.25));
                            state.last_mood_idle_time = now;
                        }
                    }
                }
            }
        });

        results
    }

    /// 触发程序事件
    pub fn trigger_event_with_manifest(&self, char_id: &str, event_key: &str, manifest: &ResourceManifest) -> Option<TriggerResult> {
        let cooldown = match event_key {
            "morning" | "afternoon" | "evening" | "night" => Duration::from_secs(3600),
            "window_focus" | "window_blur" => Duration::from_secs(10),
            "chat_start" | "chat_end" => Duration::from_secs(5),
            "music_start" | "music_stop" => Duration::from_secs(30),
            "battery_low" => Duration::from_secs(300),
            "user_return" => Duration::from_secs(10),
            _ => Duration::from_secs(30),
        };

        if !self.check_event_cooldown(char_id, event_key, cooldown) {
            return None;
        }

        debug!("[AutoTrigger] 触发事件: char={}, event={}", char_id, event_key);
        manifest.get_event_trigger(event_key)
    }

    /// 获取当前空闲时间（秒）
    pub fn idle_seconds(&self, char_id: &str) -> u64 {
        self.with_state(char_id, |state| {
            Instant::now().saturating_duration_since(state.last_interaction).as_secs()
        })
    }

    /// 获取当前空闲阶段
    pub fn idle_stage(&self, char_id: &str) -> IdleStage {
        self.with_state(char_id, |state| state.last_idle_stage)
    }
}

/// 全局单例
pub static AUTO_TRIGGER: std::sync::LazyLock<AutoExpressionTrigger> =
    std::sync::LazyLock::new(|| AutoExpressionTrigger::new());

/// 向 PetActionRequest 队列投递动作
fn push_action(char_id: &str, kind: &str, target: &str, params: serde_json::Value) {
    use crate::tools::builtin::pet_tools::{push_action as push_pet_action, PetActionRequest};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    push_pet_action(PetActionRequest {
        kind: kind.to_string(),
        target: target.to_string(),
        params,
        timestamp: now,
        character_id: char_id.to_string(),
    });
}

/// 应用触发器结果到动作队列
fn apply_trigger_result(char_id: &str, result: &TriggerResult) {
    let (expr, motion, _action, duration_opt, _prob) = result;
    let duration = duration_opt.unwrap_or(3000) as u32;

    // 触发表情
    if !expr.is_empty() && expr != "none" {
        push_action(
            char_id,
            "expression",
            expr,
            serde_json::json!({ "duration_ms": duration }),
        );
    }

    // 触发motion文件
    if !motion.is_empty() && motion != "none" {
        push_action(char_id, "motion", motion, serde_json::json!({}));
    }
}

/// 便捷函数：记录用户交互，返回是否处于长时空闲（调用方需用manifest触发user_return）
pub fn record_user_interaction(char_id: &str) -> bool {
    AUTO_TRIGGER.record_interaction(char_id)
}

/// 便捷函数：更新心情状态
pub fn update_mood_state(char_id: &str, manifest: &ResourceManifest, mood_label: &str, intensity: f64) {
    if let Some(result) = AUTO_TRIGGER.update_mood(char_id, manifest, mood_label, intensity) {
        apply_trigger_result(char_id, &result);
    }
}

/// 便捷函数：触发事件
pub fn trigger_event(char_id: &str, event_key: &str, manifest: &ResourceManifest) {
    if let Some(result) = AUTO_TRIGGER.trigger_event_with_manifest(char_id, event_key, manifest) {
        apply_trigger_result(char_id, &result);
    }
}

/// 便捷函数：执行tick，自动应用触发结果
pub fn auto_trigger_tick(char_id: &str, manifest: &ResourceManifest) {
    let triggers = AUTO_TRIGGER.tick(char_id, manifest);
    for result in &triggers {
        apply_trigger_result(char_id, result);
    }
}
