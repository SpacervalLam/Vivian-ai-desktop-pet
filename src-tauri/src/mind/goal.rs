//! Goal —— 角色当前目标。
//!
//! Goal 不是 prompt 写死的 todo list，而是可演化的运行时状态。
//! 同时活跃的 Goal 数量稀少（≤ 5），由 Reflection / 用户请求 / 日程产生。
//!
//! Goal 影响 Attention 分配（高优先级 Goal 自动提升相关实体注意力）。

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 目标来源
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GoalOrigin {
    /// 反思得出："应该减少打扰"
    Reflection,
    /// 用户明确要求："提醒我喝水"
    UserRequest,
    /// 主动行为系统发起
    Proactive,
    /// 作息/日程触发
    Schedule,
}

impl Default for GoalOrigin {
    fn default() -> Self {
        GoalOrigin::Reflection
    }
}

/// 单条目标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    /// "陪伴主人" / "逗主人开心" / "提醒喝水"
    pub description: String,
    #[serde(default)]
    pub origin: GoalOrigin,
    /// 优先级 0.0-1.0，影响 Attention 分配
    #[serde(default = "default_priority")]
    pub priority: f64,
    /// 是否仍然活跃（已完成/放弃的目标标记 false 但保留用于反思）
    #[serde(default = "default_active")]
    pub active: bool,
    pub created_at: i64,

    /// 超时时间戳（Unix 秒，超过则 active=false）
    #[serde(default)]
    pub deadline: Option<i64>,
    /// 优先级衰减率（每 30s 衰减到此倍数，默认 0.97）
    #[serde(default = "default_decay_rate")]
    pub decay_rate: f64,
}

fn default_priority() -> f64 {
    0.5
}

fn default_active() -> bool {
    true
}

fn default_decay_rate() -> f64 {
    0.97
}

impl Goal {
    pub fn new(id: impl Into<String>, description: impl Into<String>, origin: GoalOrigin, priority: f64, now: i64) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            origin,
            priority,
            active: true,
            created_at: now,
            deadline: None,
            decay_rate: 0.97,
        }
    }

    /// 是否已超时
    pub fn is_expired(&self, now: i64) -> bool {
        self.active && matches!(self.deadline, Some(d) if now > d)
    }
}

/// 目标集合 —— 单角色的全部 Goal
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GoalStore {
    pub goals: Vec<Goal>,
}

impl GoalStore {
    pub fn new() -> Self {
        Self { goals: Vec::new() }
    }

    /// 取当前活跃目标，按优先级降序
    pub fn active_sorted(&self) -> Vec<&Goal> {
        let mut v: Vec<&Goal> = self.goals.iter().filter(|g| g.active).collect();
        v.sort_by(|a, b| b.priority.partial_cmp(&a.priority).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// 取活跃目标 Top-N（prompt 注入用）
    pub fn active_top_n(&self, n: usize) -> Vec<&Goal> {
        let mut v = self.active_sorted();
        v.truncate(n);
        v
    }

    /// 添加新目标
    pub fn add(&mut self, goal: Goal) {
        self.goals.push(goal);
    }

    /// 标记目标完成/放弃
    pub fn deactivate(&mut self, id: &str) {
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            g.active = false;
        }
    }

    /// 持久化到 JSON
    pub fn save(&self, path: &std::path::PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }

    /// 从 JSON 加载
    pub fn load(path: &std::path::PathBuf) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::new(),
        }
    }
}

pub type SharedGoalStore = std::sync::Arc<RwLock<GoalStore>>;
