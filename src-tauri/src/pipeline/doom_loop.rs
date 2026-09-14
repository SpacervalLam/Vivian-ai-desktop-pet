//! 工具调用死循环检测（Doom Loop Detection）
//!
//! 在原生 function calling 循环中，LLM 可能反复调用相同工具并使用相同参数，
//! 陷入无进展的死循环直到 `max_rounds` 耗尽。本模块追踪每轮的 (tool_name, args)
//! 签名，当同一签名连续出现 ≥ 阈值次时判定为死循环，并生成注入消息打断循环。
//!
//! 与现有 `LoopDetectionAdvisor` 的关系：
//! - `LoopDetectionAdvisor` 检测**文本输出**重复（order=100 Advisor）
//! - `DoomLoopTracker` 检测**工具调用**重复（嵌入 FC 循环内部）
//! 两者互补，不重叠。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use serde_json::Value;

/// 工具调用签名：(tool_name, canonical_args_json)
///
/// `canonical_args` 通过 BTreeMap 排序确保相同参数产生相同签名，
/// 不受 JSON 键序影响。
#[derive(Debug, Clone)]
struct ToolCallSignature {
    tool_name: String,
    canonical_args: String,
}

impl ToolCallSignature {
    fn new(tool_name: &str, arguments: &Value) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            canonical_args: canonical_json(arguments),
        }
    }

    fn hash_key(&self) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        self.tool_name.hash(&mut hasher);
        self.canonical_args.hash(&mut hasher);
        hasher.finish()
    }
}

/// 将 JSON Value 序列化为规范形式（BTreeMap 排序），确保相同内容产生相同字符串
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            // BTreeMap 自动按键排序
            let sorted: std::collections::BTreeMap<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::from_str(&canonical_json(v)).unwrap_or(v.clone())))
                .collect();
            serde_json::to_string(&sorted).unwrap_or_default()
        }
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// 死循环检测状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopStatus {
    /// 正常：未达到阈值
    Normal,
    /// 死循环：同一签名连续出现 ≥ 阈值次
    Doomed {
        /// 重复调用的工具名
        tool: String,
        /// 已调用次数
        count: u32,
    },
}

/// 工具调用死循环追踪器
///
/// 在原生 FC 循环的每一轮中，记录所有工具调用的签名。
/// 当同一签名累计达到阈值时返回 `Doomed`。
///
/// 每个 FC 循环开始时调用 `reset()`，跨循环不累计。
pub struct DoomLoopTracker {
    /// 签名哈希 → 出现次数
    signatures: HashMap<u64, (String, u32)>, // hash → (tool_name, count)
    /// 触发阈值（默认 3）
    threshold: u32,
}

impl DoomLoopTracker {
    /// 创建追踪器，`threshold` 为触发死循环的最小重复次数
    pub fn new(threshold: u32) -> Self {
        Self {
            signatures: HashMap::new(),
            threshold: threshold.max(2), // 至少需要 2 次才能判定重复
        }
    }

    /// 记录一次工具调用，返回当前状态
    ///
    /// - `tool_name`：工具名称
    /// - `arguments`：工具参数（JSON）
    ///
    /// 返回 `LoopStatus::Doomed` 表示检测到死循环
    pub fn record(&mut self, tool_name: &str, arguments: &Value) -> LoopStatus {
        if self.threshold == 0 {
            return LoopStatus::Normal;
        }

        let sig = ToolCallSignature::new(tool_name, arguments);
        let key = sig.hash_key();

        let entry = self.signatures.entry(key).or_insert_with(|| (tool_name.to_string(), 0));
        entry.1 += 1;

        if entry.1 >= self.threshold {
            LoopStatus::Doomed {
                tool: entry.0.clone(),
                count: entry.1,
            }
        } else {
            LoopStatus::Normal
        }
    }

    /// 批量记录一轮中的所有工具调用，返回首个 Doomed 状态（如果有）
    pub fn record_round(&mut self, calls: &[(String, Value)]) -> LoopStatus {
        for (name, args) in calls {
            let status = self.record(name, args);
            if let LoopStatus::Doomed { .. } = &status {
                return status;
            }
        }
        LoopStatus::Normal
    }

    /// 重置追踪器（新 FC 循环开始时调用）
    pub fn reset(&mut self) {
        self.signatures.clear();
    }

    /// 生成打断注入消息
    ///
    /// 当检测到 `Doomed` 时，生成一条 system 风格的用户消息注入到对话中，
    /// 引导 LLM 换策略。
    pub fn build_intervention_message(status: &LoopStatus) -> Option<String> {
        match status {
            LoopStatus::Normal => None,
            LoopStatus::Doomed { tool, count } => Some(format!(
                "[System] 你已连续 {count} 次调用 `{tool}` 并使用相同参数，\
                 这没有取得进展。请尝试不同的方法、调整参数，\
                 或告诉用户当前遇到了什么障碍。"
            )),
        }
    }
}

impl Default for DoomLoopTracker {
    fn default() -> Self {
        Self::new(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normal_under_threshold() {
        let mut tracker = DoomLoopTracker::new(3);
        let args = json!({"file": "test.txt"});
        assert_eq!(tracker.record("read_file", &args), LoopStatus::Normal);
        assert_eq!(tracker.record("read_file", &args), LoopStatus::Normal);
    }

    #[test]
    fn doomed_at_threshold() {
        let mut tracker = DoomLoopTracker::new(3);
        let args = json!({"file": "test.txt"});
        tracker.record("read_file", &args);
        tracker.record("read_file", &args);
        let status = tracker.record("read_file", &args);
        assert_eq!(
            status,
            LoopStatus::Doomed {
                tool: "read_file".to_string(),
                count: 3
            }
        );
    }

    #[test]
    fn different_args_separate_tracking() {
        let mut tracker = DoomLoopTracker::new(3);
        let args1 = json!({"file": "a.txt"});
        let args2 = json!({"file": "b.txt"});
        tracker.record("read_file", &args1);
        tracker.record("read_file", &args2);
        let status = tracker.record("read_file", &args1);
        // args1 只出现 2 次，未达阈值
        assert_eq!(status, LoopStatus::Normal);
    }

    #[test]
    fn different_tools_separate_tracking() {
        let mut tracker = DoomLoopTracker::new(3);
        let args = json!({"file": "test.txt"});
        tracker.record("read_file", &args);
        tracker.record("write_file", &args);
        tracker.record("read_file", &args);
        let status = tracker.record("read_file", &args);
        // read_file 出现 3 次
        assert!(matches!(status, LoopStatus::Doomed { .. }));
    }

    #[test]
    fn reset_clears_state() {
        let mut tracker = DoomLoopTracker::new(3);
        let args = json!({"file": "test.txt"});
        tracker.record("read_file", &args);
        tracker.record("read_file", &args);
        tracker.reset();
        let status = tracker.record("read_file", &args);
        assert_eq!(status, LoopStatus::Normal);
    }

    #[test]
    fn canonical_json_key_order_independent() {
        let a = json!({"b": 1, "a": 2});
        let b = json!({"a": 2, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }

    #[test]
    fn intervention_message_format() {
        let status = LoopStatus::Doomed {
            tool: "grep".to_string(),
            count: 4,
        };
        let msg = DoomLoopTracker::build_intervention_message(&status).unwrap();
        assert!(msg.contains("grep"));
        assert!(msg.contains("4"));
    }

    #[test]
    fn no_intervention_for_normal() {
        assert!(DoomLoopTracker::build_intervention_message(&LoopStatus::Normal).is_none());
    }

    #[test]
    fn record_round_returns_first_doomed() {
        let mut tracker = DoomLoopTracker::new(2);
        // 先记录一次，让 count=1
        tracker.record("read_file", &json!({"f": "x"}));

        // 本轮有两个调用：第一个就会触发 doomed
        let calls = vec![
            ("read_file".to_string(), json!({"f": "x"})),
            ("write_file".to_string(), json!({"f": "y"})),
        ];
        let status = tracker.record_round(&calls);
        assert!(matches!(status, LoopStatus::Doomed { .. }));
    }
}
