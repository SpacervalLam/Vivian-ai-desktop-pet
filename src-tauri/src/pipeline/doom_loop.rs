//! 工具调用死循环检测（Doom Loop Detection）
//!
//! 在原生 function calling 循环中，LLM 可能反复调用相同工具并使用相同参数，
//! 陷入无进展的死循环直到 `max_rounds` 耗尽。本模块比较近期调用的参数与实际结果，
//! 只有连续重复的调用序列才触发干预；读取新状态、修复后重跑测试不应被误判。
//!
//! 与现有 `LoopDetectionAdvisor` 的关系：
//! - `LoopDetectionAdvisor` 检测**文本输出**重复（order=100 Advisor）
//! - `DoomLoopTracker` 检测**工具调用**重复（嵌入 FC 循环内部）
//! 两者互补，不重叠。

use std::collections::VecDeque;
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

    fn hash_key(&self, outcome: Option<(bool, &str)>) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        self.tool_name.hash(&mut hasher);
        self.canonical_args.hash(&mut hasher);
        outcome.hash(&mut hasher);
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
                .map(|(k, v)| {
                    (
                        k.clone(),
                        serde_json::from_str(&canonical_json(v)).unwrap_or(v.clone()),
                    )
                })
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
    /// 死循环：同一调用序列（含结果）连续出现 ≥ 阈值次
    Doomed {
        /// 重复调用的工具名
        tool: String,
        /// 模式重复次数
        count: u32,
    },
}

/// 工具调用死循环追踪器
///
/// 检测 A-A-A 以及 A-B-A-B-A-B 等长度不超过 8 的重复模式。
/// 只保存最近 64 个指纹，长期运行也不会累计无限签名或把很早的调用算进来。
///
/// 每个 FC 循环开始时调用 `reset()`，跨循环不累计。
pub struct DoomLoopTracker {
    history: VecDeque<u64>,
    /// 触发阈值（默认 3）
    threshold: u32,
}

impl DoomLoopTracker {
    /// 创建追踪器，`threshold` 为触发死循环的最小重复次数
    pub fn new(threshold: u32) -> Self {
        Self {
            history: VecDeque::new(),
            threshold: if threshold == 0 {
                0
            } else {
                threshold.clamp(2, 64)
            },
        }
    }

    /// 记录一次工具调用，返回当前状态
    ///
    /// - `tool_name`：工具名称
    /// - `arguments`：工具参数（JSON）
    ///
    /// 返回 `LoopStatus::Doomed` 表示检测到死循环
    pub fn record(&mut self, tool_name: &str, arguments: &Value) -> LoopStatus {
        self.record_observation(tool_name, arguments, None)
    }

    /// 完成执行后记录实际结果；成功与失败、结果内容变化均构成不同观察。
    pub fn record_result(
        &mut self,
        tool_name: &str,
        arguments: &Value,
        success: bool,
        output: &str,
    ) -> LoopStatus {
        self.record_observation(tool_name, arguments, Some((success, output)))
    }

    fn record_observation(
        &mut self,
        tool_name: &str,
        arguments: &Value,
        outcome: Option<(bool, &str)>,
    ) -> LoopStatus {
        if self.threshold == 0 {
            return LoopStatus::Normal;
        }

        let sig = ToolCallSignature::new(tool_name, arguments);
        self.history.push_back(sig.hash_key(outcome));
        if self.history.len() > 64 {
            self.history.pop_front();
        }
        let len = self.history.len();
        for period in 1..=8.min(len / self.threshold as usize) {
            let mut repeats = 1;
            while (repeats + 1) * period <= len
                && (0..period).all(|offset| {
                    self.history[len - 1 - offset]
                        == self.history[len - 1 - repeats * period - offset]
                })
            {
                repeats += 1;
            }
            if repeats >= self.threshold as usize {
                return LoopStatus::Doomed {
                    tool: tool_name.to_string(),
                    count: repeats as u32,
                };
            }
        }
        LoopStatus::Normal
    }

    /// 批量记录整轮，按最后的观察判定；后续新动作可以打断前面的重复。
    pub fn record_round(&mut self, calls: &[(String, Value)]) -> LoopStatus {
        let mut status = LoopStatus::Normal;
        for (name, args) in calls {
            status = self.record(name, args);
        }
        status
    }

    /// 重置追踪器（新 FC 循环开始时调用）
    pub fn reset(&mut self) {
        self.history.clear();
    }

    /// 生成打断注入消息
    ///
    /// 当检测到 `Doomed` 时，生成一条 system 风格的用户消息注入到对话中，
    /// 引导 LLM 换策略。
    pub fn build_intervention_message(status: &LoopStatus) -> Option<String> {
        match status {
            LoopStatus::Normal => None,
            LoopStatus::Doomed { tool, count } => Some(format!(
                "[System] 最近包含 `{tool}` 的工具调用序列已重复 {count} 次，\
                 参数和观察结果没有变化。请尝试不同的方法、调整参数，\
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
    fn intervening_different_call_breaks_single_call_streak() {
        let mut tracker = DoomLoopTracker::new(3);
        let args = json!({"file": "test.txt"});
        tracker.record("read_file", &args);
        tracker.record("write_file", &args);
        tracker.record("read_file", &args);
        let status = tracker.record("read_file", &args);
        // 累计 3 次不等于连续 3 次：中间的写入可能改变了读取状态。
        assert_eq!(status, LoopStatus::Normal);
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
    fn later_progress_in_same_round_prevents_premature_stop() {
        let mut tracker = DoomLoopTracker::new(2);
        // 先记录一次，让 count=1
        tracker.record("read_file", &json!({"f": "x"}));

        // 本轮有两个调用：第一个就会触发 doomed
        let calls = vec![
            ("read_file".to_string(), json!({"f": "x"})),
            ("write_file".to_string(), json!({"f": "y"})),
        ];
        let status = tracker.record_round(&calls);
        assert_eq!(status, LoopStatus::Normal);
        // 即使首个调用命中，也要记录剩余调用，免得下一轮忽略已有进展。
        assert_eq!(
            tracker.record("read_file", &json!({"f": "x"})),
            LoopStatus::Normal
        );
    }

    #[test]
    fn changing_results_do_not_trigger_doom_loop() {
        let mut tracker = DoomLoopTracker::default();
        for output in ["pending", "running", "done", "new result"] {
            assert_eq!(
                tracker.record_result("work_job", &json!({"id": "job"}), true, output),
                LoopStatus::Normal
            );
        }
    }

    #[test]
    fn unchanged_result_cycle_is_detected() {
        let mut tracker = DoomLoopTracker::default();
        for _ in 0..2 {
            assert_eq!(
                tracker.record_result("read_file", &json!({}), true, "same file"),
                LoopStatus::Normal
            );
            assert_eq!(
                tracker.record_result("run_command", &json!({}), false, "same error"),
                LoopStatus::Normal
            );
        }
        tracker.record_result("read_file", &json!({}), true, "same file");
        assert!(matches!(
            tracker.record_result("run_command", &json!({}), false, "same error"),
            LoopStatus::Doomed { count: 3, .. }
        ));
    }

    #[test]
    fn success_after_failures_is_a_new_observation() {
        let mut tracker = DoomLoopTracker::default();
        tracker.record_result("run_command", &json!({}), false, "output");
        tracker.record_result("run_command", &json!({}), false, "output");
        assert_eq!(
            tracker.record_result("run_command", &json!({}), true, "output"),
            LoopStatus::Normal
        );
    }

    #[test]
    fn history_is_bounded_and_zero_disables_detection() {
        let mut tracker = DoomLoopTracker::default();
        let mut disabled = DoomLoopTracker::new(0);
        for i in 0..1000 {
            assert_eq!(
                tracker.record_result("read_file", &json!({"page": i}), true, "data"),
                LoopStatus::Normal
            );
            assert_eq!(disabled.record("read_file", &json!({})), LoopStatus::Normal);
        }
        assert_eq!(tracker.history.len(), 64);
        assert!(disabled.history.is_empty());
    }
}
