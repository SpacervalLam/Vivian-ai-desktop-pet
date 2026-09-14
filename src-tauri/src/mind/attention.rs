//! Attention —— 运行时注意力聚焦权重。
//!
//! Attention 决定"现在关注什么"，是检索预过滤器的核心维度：
//! 高注意力实体的记忆优先保留，低注意力实体的记忆降权。
//!
//! Attention 纯运行时，不持久化。每次启动从最近事件重建，对话过程中由
//! 事件驱动更新（相关实体提升、其他衰减）。
//!
//! Attention 与 Graph 的区别：Graph 决定"知道什么"（知识广度），
//! Attention 决定"现在关注什么"（认知焦点）。两者正交。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 单个注意力焦点条目
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AttentionFocus {
    /// 权重 0.0-1.0
    pub weight: f32,
    /// 最近一次被激活的时间戳（Unix 秒），用于衰减
    pub last_activated: i64,
}

impl Default for AttentionFocus {
    fn default() -> Self {
        Self { weight: 0.0, last_activated: 0 }
    }
}

/// 注意力状态 —— 实体 → 聚焦权重
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Attention {
    /// key: "user" / "self" / 角色ID / 具体实体名（如 "考试"）
    pub focus: HashMap<String, AttentionFocus>,
    /// 最近一次更新时间，用于衰减计算
    pub last_updated: i64,
}

impl Attention {
    pub fn new() -> Self {
        Self::default()
    }

    /// 提升某实体的注意力
    ///
    /// `boost` 为本次激活的强度（0.0-1.0），与既有权重取 max 后再叠加衰减因子。
    pub fn boost(&mut self, entity: &str, boost: f32, now: i64) {
        let entry = self.focus.entry(entity.to_string()).or_insert(AttentionFocus::default());
        let new_weight = entry.weight.max(boost);
        entry.weight = new_weight.min(1.0);
        entry.last_activated = now;
        self.last_updated = now;
    }

    /// 对所有实体执行指数衰减
    ///
    /// `factor` 为单次衰减系数（如 0.95），权重低于 `floor`（如 0.05）的条目移除。
    pub fn decay(&mut self, factor: f32, floor: f32) {
        self.focus.retain(|_, f| {
            f.weight *= factor;
            if f.weight < floor {
                false
            } else {
                true
            }
        });
    }

    /// 取权重 Top-N 实体（prompt 注入用，按权重降序）
    pub fn top_n(&self, n: usize) -> Vec<(&String, f32)> {
        let mut v: Vec<(&String, f32)> = self
            .focus
            .iter()
            .map(|(k, f)| (k, f.weight))
            .collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(n);
        v
    }

    /// 查询某实体的注意力权重（未命中返回 0.0）
    pub fn weight_of(&self, entity: &str) -> f32 {
        self.focus.get(entity).map(|f| f.weight).unwrap_or(0.0)
    }

    /// 确保核心实体拥有基线注意力（角色"活着"的最低意识）
    ///
    /// 仅在当前权重低于基线时提升，不覆盖交互产生的高权重。
    pub fn seed_baseline(&mut self, now: i64) {
        const BASELINES: &[(&str, f32)] = &[
            ("self", 0.30),
            ("desktop", 0.20),
            ("user", 0.15),
        ];
        for &(entity, floor) in BASELINES {
            let entry = self.focus.entry(entity.to_string()).or_insert(AttentionFocus::default());
            if entry.weight < floor {
                entry.weight = floor;
                entry.last_activated = now;
            }
        }
        self.last_updated = now;
    }

    /// 是否为高注意力实体（权重 ≥ threshold，默认 0.3）
    pub fn is_focused(&self, entity: &str, threshold: f32) -> bool {
        self.weight_of(entity) >= threshold
    }
}
