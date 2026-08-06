//! 用户行为日志（Event 层）—— 记录用户持续状态的起止与时长。
//!
//! 当 LLM 在反思阶段判断用户进入了一个持续状态（睡觉/写代码/玩游戏），
//! 该状态被记录为"进行中"。当状态结束（用户回归 / 切换到新状态 / 系统清除），
//! 程序自动封存（seal）一条带时长的行为事件到本日志。
//!
//! 日志是认知引擎（UserCognitionEngine）的证据来源：Rest 时 LLM 汇总近期
//! 行为日志 → 提炼为用户习惯 Belief（如"用户通常睡 7 小时"）。
//!
//! 与 UnifiedEventLedger 的区别：后者是全局事件账本（会被 LLM 压缩），
//! 本日志是结构化的行为时序记录（带 duration，不被压缩），专供认知整理。

use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 行为事件的结束原因
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorEndReason {
    /// 用户从 Away 回来（mark_present），上一个状态结束
    UserReturn,
    /// LLM 输出了新的 world_update，旧状态被新状态覆盖
    StateChange,
    /// 系统显式清除（如重置）
    SystemClear,
    /// 状态被同名活动刷新（LLM 重新确认同一状态，视为延续而非结束）
    Override,
}

/// 单条用户行为事件（已封存）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserBehaviorEntry {
    /// 唯一 ID
    pub id: String,
    /// 活动标签（LLM 产出的简短中文词语，如"睡觉""写代码"）
    pub activity_label: String,
    /// 开始时间（Unix 秒）
    pub started_at: f64,
    /// 结束时间（Unix 秒）
    pub ended_at: f64,
    /// 持续秒数（ended_at - started_at）
    pub duration_secs: f64,
    /// 来源：llm_observation（LLM 反思产出）/ return_detected（用户回归时推断）
    #[serde(default = "default_source")]
    pub source: String,
    /// 结束原因
    pub ended_by: BehaviorEndReason,
    /// LLM 给出的置信度（0.0-1.0，来自 world_update）
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

fn default_source() -> String {
    "llm_observation".to_string()
}

fn default_confidence() -> f64 {
    0.8
}

impl UserBehaviorEntry {
    /// 持续时长（小时）
    pub fn duration_hours(&self) -> f64 {
        self.duration_secs / 3600.0
    }

    /// 持续时长（分钟）
    pub fn duration_minutes(&self) -> f64 {
        self.duration_secs / 60.0
    }
}

/// 用户行为日志存储
#[derive(Debug, Serialize, Deserialize)]
pub struct UserBehaviorLog {
    /// 已封存的行为事件（按时间升序）
    pub entries: Vec<UserBehaviorEntry>,
    /// 最大保留条数（FIFO 淘汰）
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
    /// 持久化路径（None 时不落盘）
    #[serde(skip)]
    pub persistence_path: Option<PathBuf>,
}

fn default_max_entries() -> usize {
    300
}

impl Default for UserBehaviorLog {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 300,
            persistence_path: None,
        }
    }
}

impl UserBehaviorLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_persistence(mut self, path: PathBuf) -> Self {
        self.persistence_path = Some(path);
        self
    }

    /// 从磁盘加载（若文件不存在则返回空日志）
    pub fn load(path: &PathBuf) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                let mut log: Self = serde_json::from_str(&s).unwrap_or_default();
                log.persistence_path = Some(path.clone());
                log
            }
            Err(_) => Self {
                persistence_path: Some(path.clone()),
                ..Self::default()
            },
        }
    }

    /// 落盘（write-through：每次 seal 后立即保存）
    pub fn save(&self) {
        if let Some(path) = &self.persistence_path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(path, json);
            }
        }
    }

    /// 封存一条行为事件
    ///
    /// 推入日志并立即落盘。超过 max_entries 时按 FIFO 淘汰最旧条目。
    pub fn seal(&mut self, entry: UserBehaviorEntry) {
        self.entries.push(entry);
        while self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
        self.save();
    }

    /// 最近 N 条（按时间降序）
    pub fn recent(&self, n: usize) -> Vec<&UserBehaviorEntry> {
        let len = self.entries.len();
        let start = len.saturating_sub(n);
        self.entries[start..].iter().rev().collect()
    }

    /// 按活动标签过滤
    pub fn by_label(&self, label: &str) -> Vec<&UserBehaviorEntry> {
        self.entries
            .iter()
            .filter(|e| e.activity_label == label)
            .collect()
    }

    /// 全部条目（按时间升序）
    pub fn all(&self) -> &[UserBehaviorEntry] {
        &self.entries
    }

    /// 序列化为 prompt 段落（认知整理用）
    pub fn serialize_for_consolidation(&self, n: usize) -> String {
        let recent = self.recent(n);
        if recent.is_empty() {
            return "（暂无行为记录）".to_string();
        }
        recent
            .iter()
            .map(|e| {
                let start = chrono::DateTime::from_timestamp(e.started_at as i64, 0)
                    .map(|dt| {
                        dt.with_timezone(&chrono::Local)
                            .format("%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                let dur = if e.duration_hours() >= 1.0 {
                    format!("{:.1}h", e.duration_hours())
                } else {
                    format!("{:.0}min", e.duration_minutes())
                };
                format!("- {}：{} 起，持续 {}", e.activity_label, start, dur)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 线程安全句柄
pub type SharedUserBehaviorLog = std::sync::Arc<RwLock<UserBehaviorLog>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seal_and_recent() {
        let mut log = UserBehaviorLog::new();
        log.seal(UserBehaviorEntry {
            id: "1".into(),
            activity_label: "睡觉".into(),
            started_at: 1000.0,
            ended_at: 1000.0 + 7.5 * 3600.0,
            duration_secs: 7.5 * 3600.0,
            source: "llm_observation".into(),
            ended_by: BehaviorEndReason::UserReturn,
            confidence: 0.9,
        });
        log.seal(UserBehaviorEntry {
            id: "2".into(),
            activity_label: "写代码".into(),
            started_at: 2000.0,
            ended_at: 2000.0 + 2.0 * 3600.0,
            duration_secs: 2.0 * 3600.0,
            source: "llm_observation".into(),
            ended_by: BehaviorEndReason::StateChange,
            confidence: 0.85,
        });
        assert_eq!(log.entries.len(), 2);
        let recent = log.recent(1);
        assert_eq!(recent[0].activity_label, "写代码");
        let sleep = log.by_label("睡觉");
        assert_eq!(sleep.len(), 1);
        assert!((sleep[0].duration_hours() - 7.5).abs() < 0.01);
    }
}
