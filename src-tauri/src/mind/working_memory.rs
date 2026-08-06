//! Working Memory —— 30 秒级"正在想什么"缓冲区。
//!
//! 与长期 Memory（MemoryManager）和 Mind 的 Belief/Goal/Attention 不同，
//! Working Memory 是角色"此刻意识中的活跃信息"——类似人类工作记忆，
//! 容量有限（Miller's 7±2），随时间衰减，会话结束后清空。
//!
//! 设计原则：
//! - **纯运行时**：不持久化，每次启动从空开始，由对话过程重建
//! - **固定容量**：最多 7 条，FIFO + 衰减淘汰
//! - **内容蒸馏**：不存原文，存短摘要（≤80 字），避免与 DialogueManager 重复
//! - **mind_tick 衰减**：30s 级指数衰减，低于 0.05 的条目移除
//!
//! 与现有机制的区别：
//! - DialogueManager.messages：存原始对话原文（截断窗口），按时间序
//! - Attention.focus：存"实体 → 权重"，不存内容
//! - UnifiedEventLedger：存全局事件指针 + 80 字预览，跨角色共享
//! - Working Memory：存"角色此刻脑中的活跃想法"，个人视角，会话级

use serde::{Deserialize, Serialize};

/// 工作记忆条目来源
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkingMemorySource {
    /// 用户消息摘要
    UserMessage,
    /// AI 回复摘要
    AiReply,
    /// 内心独白结论
    InnerMonologue,
    /// 世界事件感知（用户离开/回归等）
    WorldEvent,
}

/// 单条工作记忆
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingMemoryEntry {
    /// 蒸馏后的短摘要（≤80 字）
    pub content: String,
    /// 权重 0.0-1.0，随时间衰减
    pub weight: f32,
    /// 产生时间（Unix 秒）
    pub created_at: i64,
    /// 来源
    pub source: WorkingMemorySource,
}

/// 工作记忆缓冲区
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WorkingMemory {
    /// 条目列表，最多 7 条
    pub entries: Vec<WorkingMemoryEntry>,
}

const MAX_ENTRIES: usize = 7;
const DECAY_FLOOR: f32 = 0.05;

impl WorkingMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一条工作记忆
    ///
    /// 超过容量时移除权重最低的条目（而非最旧的），让重要信息存活更久。
    pub fn push(&mut self, content: String, source: WorkingMemorySource, now: i64) {
        // 蒸馏：截断到 80 字
        let content: String = content.chars().take(80).collect();
        if content.trim().is_empty() {
            return;
        }
        let entry = WorkingMemoryEntry {
            content,
            weight: 1.0,
            created_at: now,
            source,
        };
        if self.entries.len() >= MAX_ENTRIES {
            // 移除权重最低的条目
            if let Some(idx) = self.entries.iter().enumerate().min_by(|a, b| {
                a.1.weight.partial_cmp(&b.1.weight).unwrap_or(std::cmp::Ordering::Equal)
            }).map(|(i, _)| i) {
                self.entries.remove(idx);
            }
        }
        self.entries.push(entry);
    }

    /// mind_tick 驱动的指数衰减
    ///
    /// 衰减率：每分钟衰减到 ~0.8（即 0.8^(dt/60)），低于 DECAY_FLOOR 的条目移除。
    pub fn decay(&mut self, dt_secs: f64) {
        let decay_factor = 0.8f32.powf((dt_secs / 60.0).max(0.0) as f32);
        for e in &mut self.entries {
            e.weight *= decay_factor;
        }
        self.entries.retain(|e| e.weight >= DECAY_FLOOR);
    }

    /// 取 Top-N（按权重降序），prompt 注入用
    pub fn top_n(&self, n: usize) -> Vec<&WorkingMemoryEntry> {
        let mut v: Vec<&WorkingMemoryEntry> = self.entries.iter().collect();
        v.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(n);
        v
    }

    /// 清空（会话关闭时调用）
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 序列化为 prompt 段落
    pub fn serialize_for_prompt(&self, lang: &str) -> Option<String> {
        let top = self.top_n(5);
        if top.is_empty() {
            return None;
        }
        let lines: Vec<String> = top.iter()
            .map(|e| {
                let tag = match e.source {
                    WorkingMemorySource::UserMessage => "user",
                    WorkingMemorySource::AiReply => "self",
                    WorkingMemorySource::InnerMonologue => "thought",
                    WorkingMemorySource::WorldEvent => "world",
                };
                format!("- [{}] {}", tag, e.content)
            })
            .collect();
        let header = crate::pipeline::prompt_modules::section_heading("current_thoughts", lang);
        Some(format!("{}\n{}", header, lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_capacity() {
        let mut wm = WorkingMemory::new();
        for i in 0..10 {
            wm.push(format!("msg {}", i), WorkingMemorySource::UserMessage, 0);
        }
        assert_eq!(wm.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn test_decay_removes_low_weight() {
        let mut wm = WorkingMemory::new();
        wm.push("test".to_string(), WorkingMemorySource::UserMessage, 0);
        // 10 分钟衰减
        wm.decay(600.0);
        // 0.8^10 ≈ 0.107，应保留
        assert_eq!(wm.entries.len(), 1);
        // 30 分钟衰减
        wm.decay(1800.0);
        // 0.8^30 ≈ 0.0012，应被移除
        assert_eq!(wm.entries.len(), 0);
    }

    #[test]
    fn test_clear() {
        let mut wm = WorkingMemory::new();
        wm.push("a".to_string(), WorkingMemorySource::UserMessage, 0);
        wm.push("b".to_string(), WorkingMemorySource::AiReply, 0);
        wm.clear();
        assert!(wm.entries.is_empty());
    }

    #[test]
    fn test_serialize_for_prompt() {
        let mut wm = WorkingMemory::new();
        assert_eq!(wm.serialize_for_prompt("zh"), None);
        wm.push("用户问了天气".to_string(), WorkingMemorySource::UserMessage, 0);
        let s = wm.serialize_for_prompt("zh").unwrap();
        assert!(s.contains("用户问了天气"));
        assert!(s.contains("[user]"));
    }
}
