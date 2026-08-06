//! 主动性的 per-trigger 偏好学习
//!
//! 追踪每种主动触发类型的用户响应率（responded vs ignored），
//! 动态调整该触发器未来的触发概率。
//!
//! 学习目标：
//! - 用户经常响应的触发类型 → 降低阈值，更积极地触发
//! - 用户经常忽略的触发类型 → 提高阈值，减少打扰
//!
//! 算法：指数加权移动平均（EWMA）
//! - success_rate = α * recent_outcome + (1-α) * success_rate
//! - α = 0.3（近期权重 30%，历史权重 70%）
//! - probability_multiplier = clamp(0.3, 2.0, success_rate / baseline)
//!
//! 集成点：
//! - `push_message` 时调用 `record_trigger_fired`
//! - `on_user_interacted` 时调用 `record_response(true)`
//! - `on_ignored` 时调用 `record_response(false)`
//! - 触发判断时调用 `get_probability_multiplier` 调整原始概率

use std::collections::HashMap;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::triggers::ProactiveTrigger;
use crate::utils::path::get_user_data_dir;

/// 学习速率（EWMA 的 α 参数）
const LEARNING_RATE: f64 = 0.3;
/// 初始成功率假设（新触发器的先验）
const INITIAL_SUCCESS_RATE: f64 = 0.5;
/// 最小概率乘数（不会把概率降到 0，保留恢复机会）
const MIN_MULTIPLIER: f64 = 0.3;
/// 最大概率乘数（不会让概率无限增长）
const MAX_MULTIPLIER: f64 = 2.0;
/// 评估所需的最小样本数（低于此数不调整）
const MIN_SAMPLES: u32 = 3;

/// 单个触发器的学习统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerStats {
    /// EWMA 成功率（0.0-1.0）
    pub success_rate: f64,
    /// 总触发次数
    pub total_fired: u32,
    /// 总响应次数
    pub total_responded: u32,
    /// 总忽略次数
    pub total_ignored: u32,
}

impl Default for TriggerStats {
    fn default() -> Self {
        Self {
            success_rate: INITIAL_SUCCESS_RATE,
            total_fired: 0,
            total_responded: 0,
            total_ignored: 0,
        }
    }
}

/// 触发偏好学习器
pub struct TriggerPreferenceLearner {
    stats: RwLock<HashMap<String, TriggerStats>>,
    /// 上一个触发的触发器（用于归因用户响应/忽略）
    last_trigger: RwLock<Option<String>>,
    /// 上一个触发的时间戳
    last_trigger_at: RwLock<f64>,
    /// 持久化路径
    persistence_path: std::path::PathBuf,
}

impl TriggerPreferenceLearner {
    pub fn new() -> Self {
        let proactive_dir = get_user_data_dir().join("proactive");
        let _ = std::fs::create_dir_all(&proactive_dir);
        let persistence_path = proactive_dir.join("trigger_preferences.json");
        let learner = Self {
            stats: RwLock::new(HashMap::new()),
            last_trigger: RwLock::new(None),
            last_trigger_at: RwLock::new(0.0),
            persistence_path,
        };
        learner.load_from_disk();
        learner
    }

    /// 记录触发器已触发（在 push_message 时调用）
    pub fn record_trigger_fired(&self, trigger: ProactiveTrigger) {
        let key = trigger.as_str().to_string();
        let now = current_timestamp();

        {
            let mut stats = self.stats.write();
            let entry = stats.entry(key.clone()).or_default();
            entry.total_fired += 1;
        }
        *self.last_trigger.write() = Some(key);
        *self.last_trigger_at.write() = now;

        if let Err(e) = self.save_to_disk() {
            tracing::warn!("[PreferenceLearner] 持久化失败: {e}");
        }
    }

    /// 记录用户响应/忽略（在 on_user_interacted / on_ignored 时调用）
    ///
    /// `responded` = true 表示用户响应了上一个主动消息，
    /// false 表示用户忽略了上一个主动消息。
    /// 只归因最近 5 分钟内触发的消息（超时则不归因）。
    pub fn record_response(&self, responded: bool) {
        let now = current_timestamp();
        let last_at = *self.last_trigger_at.read();
        // 超时窗口：5 分钟内的响应才归因
        if now - last_at > 300.0 {
            *self.last_trigger.write() = None;
            return;
        }

        let key = match self.last_trigger.write().take() {
            Some(k) => k,
            None => return,
        };

        {
            let mut stats = self.stats.write();
            let entry = stats.entry(key.clone()).or_default();
            let outcome = if responded { 1.0 } else { 0.0 };
            // EWMA 更新
            entry.success_rate =
                LEARNING_RATE * outcome + (1.0 - LEARNING_RATE) * entry.success_rate;
            if responded {
                entry.total_responded += 1;
            } else {
                entry.total_ignored += 1;
            }
        }

        tracing::debug!(
            "[PreferenceLearner] {} {}",
            key,
            if responded { "responded" } else { "ignored" }
        );

        if let Err(e) = self.save_to_disk() {
            tracing::warn!("[PreferenceLearner] 持久化失败: {e}");
        }
    }

    /// 获取触发器的概率调整乘数
    ///
    /// 返回值乘以原始 probability 即可得到调整后的概率。
    /// 样本不足（< MIN_SAMPLES）时返回 1.0（不调整）。
    pub fn get_probability_multiplier(&self, trigger: ProactiveTrigger) -> f64 {
        let stats = self.stats.read();
        let key = trigger.as_str();
        let Some(entry) = stats.get(key) else {
            return 1.0;
        };
        if entry.total_fired < MIN_SAMPLES {
            return 1.0;
        }
        // success_rate 相对于 baseline(0.5) 的比值
        let ratio = entry.success_rate / INITIAL_SUCCESS_RATE;
        ratio.clamp(MIN_MULTIPLIER, MAX_MULTIPLIER)
    }

    /// 获取所有触发器的统计快照（供调试/状态面板使用）
    pub fn snapshot(&self) -> HashMap<String, TriggerStats> {
        self.stats.read().clone()
    }

    fn load_from_disk(&self) {
        if !self.persistence_path.exists() {
            return;
        }
        let Ok(content) = std::fs::read_to_string(&self.persistence_path) else {
            return;
        };
        if let Ok(loaded) = serde_json::from_str::<HashMap<String, TriggerStats>>(&content) {
            *self.stats.write() = loaded;
        }
    }

    fn save_to_disk(&self) -> Result<(), String> {
        let stats = self.stats.read();
        let json = serde_json::to_string_pretty(&*stats)
            .map_err(|e| format!("序列化失败: {e}"))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("写入失败: {e}"))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| format!("替换失败: {e}"))?;
        Ok(())
    }
}

impl Default for TriggerPreferenceLearner {
    fn default() -> Self {
        Self::new()
    }
}

fn current_timestamp() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_multiplier() {
        let learner = TriggerPreferenceLearner::new();
        // 新触发器，样本不足，应返回 1.0
        let m = learner.get_probability_multiplier(ProactiveTrigger::Icebreaker);
        assert!((m - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_learning_adjustment() {
        let learner = TriggerPreferenceLearner::new();
        // 触发 5 次都被忽略
        for _ in 0..5 {
            learner.record_trigger_fired(ProactiveTrigger::Spontaneous);
            learner.record_response(false);
        }
        let m = learner.get_probability_multiplier(ProactiveTrigger::Spontaneous);
        // 被忽略 5 次，success_rate 应很低，乘数应 < 1.0
        assert!(m < 1.0, "忽略后乘数应降低，实际: {}", m);
        assert!(m >= MIN_MULTIPLIER, "乘数不应低于最小值");
    }

    #[test]
    fn test_response_boost() {
        let learner = TriggerPreferenceLearner::new();
        // 触发 5 次都响应
        for _ in 0..5 {
            learner.record_trigger_fired(ProactiveTrigger::WelcomeBack);
            learner.record_response(true);
        }
        let m = learner.get_probability_multiplier(ProactiveTrigger::WelcomeBack);
        // 响应 5 次，success_rate 应很高，乘数应 > 1.0
        assert!(m > 1.0, "响应后乘数应升高，实际: {}", m);
        assert!(m <= MAX_MULTIPLIER, "乘数不应超过最大值");
    }
}
