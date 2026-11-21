//! 压力检测
//!
//! 通过情绪记录感知用户压力水平，并动态调节主动交互频率。
//! 高压力 → 降低打扰频率，风格更温和。

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// 压力等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StressLevel {
    Low,
    Medium,
    High,
}

impl StressLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            StressLevel::Low => "low",
            StressLevel::Medium => "medium",
            StressLevel::High => "high",
        }
    }
}

/// 高压力情绪集合
const HIGH_STRESS_EMOTIONS: &[&str] = &[
    "anxious",
    "frustrated",
    "sad",
    "angry",
    "tired",
    "disappointed",
];

/// 各压力等级对应的调速参数
impl StressLevel {
    /// 冷却时间乘数（调用方应乘以冷却间隔）
    pub fn cooldown_multiplier(&self) -> f64 {
        match self {
            StressLevel::Low => 1.0,
            StressLevel::Medium => 1.5,
            StressLevel::High => 2.5,
        }
    }

    /// 推荐互动风格
    pub fn style_hint(&self) -> &'static str {
        match self {
            StressLevel::Low => "energetic",
            StressLevel::Medium => "gentle",
            StressLevel::High => "reassure",
        }
    }
}

/// 压力水平监测器
///
/// 维护最近 N 条情绪记录，按高压力情绪占比计算压力等级。
pub struct StressMonitor {
    /// 最近情绪记录（最多 10 条）
    recent: VecDeque<String>,
    capacity: usize,
}

impl StressMonitor {
    pub fn new() -> Self {
        Self {
            recent: VecDeque::with_capacity(10),
            capacity: 10,
        }
    }

    /// 记录一条情绪
    pub fn record_emotion(&mut self, emotion: &str) {
        if self.recent.len() >= self.capacity {
            self.recent.pop_front();
        }
        self.recent.push_back(emotion.to_lowercase());
    }

    /// 获取当前压力等级
    pub fn get_stress_level(&self) -> StressLevel {
        if self.recent.is_empty() {
            return StressLevel::Low;
        }
        let total = self.recent.len() as f64;
        let high = self
            .recent
            .iter()
            .filter(|e| HIGH_STRESS_EMOTIONS.contains(&e.as_str()))
            .count() as f64;
        let ratio = high / total;
        if ratio >= 0.5 {
            StressLevel::High
        } else if ratio >= 0.25 {
            StressLevel::Medium
        } else {
            StressLevel::Low
        }
    }

    /// 冷却时间乘数
    pub fn get_cooldown_multiplier(&self) -> f64 {
        self.get_stress_level().cooldown_multiplier()
    }

    /// 推荐互动风格
    pub fn get_style_hint(&self) -> &'static str {
        self.get_stress_level().style_hint()
    }

    /// 压力报告
    pub fn get_stress_report(&self) -> serde_json::Value {
        let level = self.get_stress_level();
        let total = self.recent.len();
        let high = self
            .recent
            .iter()
            .filter(|e| HIGH_STRESS_EMOTIONS.contains(&e.as_str()))
            .count();
        let ratio = if total > 0 {
            (high as f64) / (total as f64)
        } else {
            0.0
        };
        serde_json::json!({
            "level": level.as_str(),
            "cooldown_multiplier": level.cooldown_multiplier(),
            "style_hint": level.style_hint(),
            "analyzed_emotions": total,
            "high_stress_count": high,
            "high_stress_ratio": (ratio * 100.0).round() / 100.0,
        })
    }

    /// 基于工作时长 + 当前情绪综合判断
    ///
    /// `sustained_active_minutes`：用户持续活跃分钟数
    /// `current_emotion`：当前情绪标签
    pub fn assess_with_workload(
        &mut self,
        sustained_active_minutes: u32,
        current_emotion: &str,
    ) -> StressLevel {
        if !current_emotion.is_empty() {
            self.record_emotion(current_emotion);
        }
        let mut level = self.get_stress_level();
        // 长时间工作（>180 分钟）且当前情绪非积极 → 升级压力等级
        if sustained_active_minutes >= 180 {
            let positive = matches!(
                current_emotion,
                "happy" | "excited" | "content" | "playful"
            );
            if !positive {
                level = match level {
                    StressLevel::Low => StressLevel::Medium,
                    _ => StressLevel::High,
                };
            }
        }
        level
    }
}

impl Default for StressMonitor {
    fn default() -> Self {
        Self::new()
    }
}
