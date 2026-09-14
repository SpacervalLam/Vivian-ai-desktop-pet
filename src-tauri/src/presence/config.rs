//! 在场状态系统配置

use serde::{Deserialize, Serialize};

/// 在场状态系统配置
///
/// 所有阈值可通过设置面板调整，运行时注入 PresenceManager。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceConfig {
    /// 最短状态持续秒数（防止频繁切换）
    ///
    /// 刚切换的状态至少保持这么多秒，才允许下一次切换检查生效。
    #[serde(default = "default_min_state_duration")]
    pub min_state_duration: f64,

    /// 疲劳度阈值（0-100），来自 MoodSnapshot.fatigue
    ///
    /// 疲劳度 ≥ 此值时 Online → Rest
    #[serde(default = "default_fatigue_threshold")]
    pub fatigue_threshold: f64,

    /// 孤独感阈值（0.0-1.0），来自 EmotionState.loneliness
    ///
    /// 孤独感 ≥ 此值且被忽略次数达标时 → Offline
    #[serde(default = "default_loneliness_threshold")]
    pub loneliness_threshold: f64,

    /// 被忽略次数阈值
    ///
    /// 连续被忽略 ≥ 此值时 → Offline
    #[serde(default = "default_ignored_threshold")]
    pub ignored_threshold: u32,

    /// 两角色协调阈值（秒）
    ///
    /// 两角色同时在场超过此秒数 → 其中一个 Rest
    #[serde(default = "default_coordination_threshold")]
    pub coordination_threshold: f64,

    /// 离线状态下主动上线的孤独感阈值（0.0-1.0）
    ///
    /// 离线后孤独感会持续累积，达到此值且离线时长 ≥ `offline_min_duration_before_recover`
    /// 时触发 Offline → Online，让智能体「想念用户」主动回归。
    /// 应高于 `loneliness_threshold`（下线阈值），避免抖动。
    #[serde(default = "default_offline_recover_loneliness_threshold")]
    pub offline_recover_loneliness_threshold: f64,

    /// 离线最短持续秒数（防止刚下线就立即上线）
    ///
    /// 离线满此时长且孤独感达标，才允许主动恢复 Online。
    #[serde(default = "default_offline_min_duration_before_recover")]
    pub offline_min_duration_before_recover: f64,

    /// 休息状态最短持续秒数（防止刚休息就立即醒来）
    ///
    /// Rest 持续满此时长后自动恢复 Online。
    /// 应略大于记忆沉淀后台任务的典型耗时，确保任务有充分时间完成。
    /// 唤醒仍可被用户交互立即触发（wake_on_user_interaction 不受此限制）。
    #[serde(default = "default_rest_min_duration_before_recover")]
    pub rest_min_duration_before_recover: f64,

    /// 在线空闲→Busy 空闲阈值（秒）
    ///
    /// 用户超过此秒数未与角色互动，且角色处于 Online 状态，
    /// 自动转入 Busy（去做知识采集等后台任务），比空等更有意义。
    #[serde(default = "default_online_idle_to_busy_threshold")]
    pub online_idle_to_busy_threshold: f64,
}

impl Default for PresenceConfig {
    fn default() -> Self {
        Self {
            min_state_duration: default_min_state_duration(),
            fatigue_threshold: default_fatigue_threshold(),
            loneliness_threshold: default_loneliness_threshold(),
            ignored_threshold: default_ignored_threshold(),
            coordination_threshold: default_coordination_threshold(),
            offline_recover_loneliness_threshold: default_offline_recover_loneliness_threshold(),
            offline_min_duration_before_recover: default_offline_min_duration_before_recover(),
            rest_min_duration_before_recover: default_rest_min_duration_before_recover(),
            online_idle_to_busy_threshold: default_online_idle_to_busy_threshold(),
        }
    }
}

fn default_min_state_duration() -> f64 {
    300.0 // 5 分钟
}

fn default_fatigue_threshold() -> f64 {
    70.0
}

fn default_loneliness_threshold() -> f64 {
    0.8
}

fn default_ignored_threshold() -> u32 {
    5
}

fn default_coordination_threshold() -> f64 {
    3600.0 // 1 小时
}

fn default_offline_recover_loneliness_threshold() -> f64 {
    0.95 // 高于下线阈值 0.8，离线后孤独感持续累积至此值才主动回归
}

fn default_offline_min_duration_before_recover() -> f64 {
    1800.0 // 30 分钟，防止刚下线就立即上线造成抖动
}

fn default_rest_min_duration_before_recover() -> f64 {
    28800.0 // 8 小时，角色需要充分休息
}

fn default_online_idle_to_busy_threshold() -> f64 {
    2700.0 // 45 分钟，用户长时间未互动则角色去做知识采集
}

impl PresenceConfig {}
