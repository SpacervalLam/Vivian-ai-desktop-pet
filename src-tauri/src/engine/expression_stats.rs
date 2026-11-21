//! 情境-表情映射学习：记录角色在特定情境下使用表情的频次，
//! 提供"该情境下高频表情"查询，让表情选择逐渐个性化。
//!
//! 机制：
//! - 按 (char_id, situation_hash) 记录表情使用次数
//! - 二次函数衰减：30 天衰减到 0.01，低于 0.01 自动清理
//! - 容量上限 300，超限时删除权重最小的

use std::collections::HashMap;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::utils::fnv1a_64;

/// 容量上限
const MAX_EXPRESSION_RECORDS: usize = 300;

/// 衰减阈值：低于此权重的记录自动清理
const DECAY_THRESHOLD: f64 = 0.01;

/// 衰减周期（天）：30 天衰减到 DECAY_THRESHOLD
const DECAY_PERIOD_DAYS: f64 = 30.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExpressionUseRecord {
    expression: String,
    count: u32,
    last_used_ts: f64,
}

impl ExpressionUseRecord {
    /// 计算衰减后的有效权重
    ///
    /// decay = DECAY_THRESHOLD / DECAY_PERIOD_DAYS² × days²
    /// 0 天 = count，30 天 = count × 0.01
    fn effective_weight(&self, now: f64) -> f64 {
        let days = ((now - self.last_used_ts).max(0.0)) / 86400.0;
        let decay = DECAY_THRESHOLD / (DECAY_PERIOD_DAYS * DECAY_PERIOD_DAYS) * days * days;
        (self.count as f64) * (1.0 - decay).max(0.0)
    }
}

/// 全局情境-表情映射表
/// key = (char_id, situation_hash)
static EXPRESSION_STATS: Lazy<RwLock<HashMap<(String, u64), ExpressionUseRecord>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// 对情境文本做哈希（FNV-1a 64bit），作为映射 key
fn situation_hash(situation: &str) -> u64 {
    fnv1a_64(situation)
}

/// 记录某角色在某情境下使用了某表情
///
/// `situation` 应为情境的简短描述（如"用户分享开心事""长时间沉默后问候"）。
/// 相同 (char_id, situation) + 相同 expression 时 count +1，否则覆盖。
pub fn record_expression_use(char_id: &str, situation: &str, expression: &str) {
    if expression.is_empty() || situation.is_empty() {
        return;
    }
    let key = (char_id.to_string(), situation_hash(situation));
    let now = chrono::Local::now().timestamp() as f64;
    let mut stats = EXPRESSION_STATS.write();

    // 容量控制：超限时删除权重最小的
    if stats.len() >= MAX_EXPRESSION_RECORDS && !stats.contains_key(&key) {
        if let Some(min_key) = stats
            .iter()
            .min_by(|a, b| {
                a.1.effective_weight(now)
                    .partial_cmp(&b.1.effective_weight(now))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(k, _)| k.clone())
        {
            stats.remove(&min_key);
        }
    }

    let record = stats.entry(key).or_insert(ExpressionUseRecord {
        expression: expression.to_string(),
        count: 0,
        last_used_ts: now,
    });
    record.count = record.count.saturating_add(1);
    record.last_used_ts = now;
    record.expression = expression.to_string();

    // 顺手清理已低于衰减阈值的记录
    let to_remove: Vec<_> = stats
        .iter()
        .filter(|(_, r)| r.effective_weight(now) < DECAY_THRESHOLD && r.count <= 1)
        .map(|(k, _)| k.clone())
        .collect();
    for k in to_remove {
        stats.remove(&k);
    }
}

/// 查询某角色在某情境下权重最高的表情
///
/// 返回 (expression, weight)。无记录返回 None。
pub fn get_top_expression(char_id: &str, situation: &str) -> Option<(String, f64)> {
    let key = (char_id.to_string(), situation_hash(situation));
    let now = chrono::Local::now().timestamp() as f64;
    let stats = EXPRESSION_STATS.read();
    stats.get(&key).map(|r| {
        let w = r.effective_weight(now);
        (r.expression.clone(), w)
    })
}

/// 查询某角色在指定情境关键词下所有相关表情（用于 prompt few_shots 参考）
///
/// `situation_keywords` 为多个情境片段，返回每个片段的 top 表情。
pub fn get_expression_hints(char_id: &str, situations: &[&str]) -> Vec<(String, String, f64)> {
    let now = chrono::Local::now().timestamp() as f64;
    let stats = EXPRESSION_STATS.read();
    let mut out = Vec::new();
    for sit in situations {
        let key = (char_id.to_string(), situation_hash(sit));
        if let Some(r) = stats.get(&key) {
            let w = r.effective_weight(now);
            if w >= 1.0 {
                out.push((sit.to_string(), r.expression.clone(), w));
            }
        }
    }
    out
}

/// 清空所有记录（用于测试或重置）
pub fn clear_expression_stats() {
    EXPRESSION_STATS.write().clear();
}

/// 当前记录总数
pub fn expression_stats_size() -> usize {
    EXPRESSION_STATS.read().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_query() {
        clear_expression_stats();
        record_expression_use("vivian", "用户分享开心事", "love_eyes");
        record_expression_use("vivian", "用户分享开心事", "love_eyes");
        record_expression_use("vivian", "用户分享开心事", "love_eyes");
        let top = get_top_expression("vivian", "用户分享开心事");
        assert!(top.is_some());
        assert_eq!(top.unwrap().0, "love_eyes");
    }

    #[test]
    fn different_situations_isolated() {
        clear_expression_stats();
        record_expression_use("vivian", "用户难过", "tears");
        record_expression_use("vivian", "用户开心", "love_eyes");
        let sad = get_top_expression("vivian", "用户难过");
        let happy = get_top_expression("vivian", "用户开心");
        assert_eq!(sad.unwrap().0, "tears");
        assert_eq!(happy.unwrap().0, "love_eyes");
    }

    #[test]
    fn empty_inputs_ignored() {
        clear_expression_stats();
        record_expression_use("vivian", "", "love_eyes");
        record_expression_use("vivian", "情境", "");
        assert_eq!(expression_stats_size(), 0);
    }
}
