//! 轮次产出预算与收益递减检测（Diminishing Returns Detection）
//!
//! agent 循环（编程智能体 / 自治任务）可能陷入「空转」：每轮都正常返回但
//! 产出极小、无实质进展，一直磨到轮数预算耗尽——白白消耗 LLM 配额。
//! 本模块按「每轮产出量 + 实质进展标志」双信号判定收益递减：
//! 连续 N 轮产出低于阈值且期间无实质进展 → 判定空转，建议提前停机收尾。
//!
//! 与 doom_loop（同签名重复检测）互补：
//! - DoomLoopTracker 抓「完全相同的重复调用」
//! - OutputBudgetTracker 抓「调用各不相同但都毫无产出」（如反复读小文件、
//!   反复列目录、短回复后继续轮转）

/// 单轮低产出判定阈值（LLM 输出 token 数）
const LOW_OUTPUT_TOKENS: u64 = 500;
/// 单步低产出判定阈值（工具结果摘要字符数，用于无 usage 上报的任务循环）
const LOW_OUTPUT_CHARS: usize = 120;
/// 连续低产出轮数阈值：达到即判定收益递减
const DIMINISHING_ROUNDS: u32 = 3;

/// 收益递减判定结果
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetVerdict {
    /// 继续循环
    Continue,
    /// 收益递减：连续 `low_rounds` 轮低产出且无实质进展
    StopDiminishing { low_rounds: u32 },
}

/// 轮次产出跟踪器
///
/// 每轮循环结束后调用 [`record`](Self::record)（或无 usage 场景的
/// [`record_chars`](Self::record_chars)），传入本轮产出量与是否取得实质进展。
/// 实质进展会立即清零低产出计数；连续低产出达到阈值返回停机判定。
pub struct OutputBudgetTracker {
    low_rounds: u32,
    started_at: std::time::Instant,
}

impl Default for OutputBudgetTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputBudgetTracker {
    pub fn new() -> Self {
        Self {
            low_rounds: 0,
            started_at: std::time::Instant::now(),
        }
    }

    /// 记录一轮（LLM usage 可用场景）：按输出 token 数判定产出量。
    ///
    /// `output_tokens` 为本轮 LLM 输出 token（含工具调用 JSON）；
    /// `made_progress` 为本轮是否取得实质进展（写/改/执行类工具成功等）。
    pub fn record(&mut self, output_tokens: u64, made_progress: bool) -> BudgetVerdict {
        self.record_impl(output_tokens > 0 && output_tokens < LOW_OUTPUT_TOKENS, made_progress)
    }

    /// 记录一轮（无 usage 上报场景）：用产出文本长度近似产出量。
    pub fn record_chars(&mut self, summary_chars: usize, made_progress: bool) -> BudgetVerdict {
        self.record_impl(summary_chars < LOW_OUTPUT_CHARS, made_progress)
    }

    fn record_impl(&mut self, is_low: bool, made_progress: bool) -> BudgetVerdict {
        if made_progress {
            self.low_rounds = 0;
            return BudgetVerdict::Continue;
        }
        if is_low {
            self.low_rounds += 1;
            if self.low_rounds >= DIMINISHING_ROUNDS {
                return BudgetVerdict::StopDiminishing {
                    low_rounds: self.low_rounds,
                };
            }
        } else {
            self.low_rounds = 0;
        }
        BudgetVerdict::Continue
    }

    /// 跟踪器启动至今的耗时
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_resets_counter() {
        let mut t = OutputBudgetTracker::new();
        assert_eq!(t.record(10, false), BudgetVerdict::Continue);
        assert_eq!(t.record(10, false), BudgetVerdict::Continue);
        // 实质进展清零计数
        assert_eq!(t.record(10, true), BudgetVerdict::Continue);
        assert_eq!(t.record(10, false), BudgetVerdict::Continue);
        assert_eq!(t.record(10, false), BudgetVerdict::Continue);
        assert_eq!(t.record(10, false), BudgetVerdict::Continue);
    }

    #[test]
    fn diminishing_after_consecutive_low_rounds() {
        let mut t = OutputBudgetTracker::new();
        assert_eq!(t.record(100, false), BudgetVerdict::Continue);
        assert_eq!(t.record(100, false), BudgetVerdict::Continue);
        assert_eq!(
            t.record(100, false),
            BudgetVerdict::StopDiminishing { low_rounds: 3 }
        );
    }

    #[test]
    fn high_output_does_not_trigger() {
        let mut t = OutputBudgetTracker::new();
        for _ in 0..10 {
            assert_eq!(t.record(2000, false), BudgetVerdict::Continue);
        }
    }

    #[test]
    fn zero_output_counts_as_low() {
        let mut t = OutputBudgetTracker::new();
        assert_eq!(t.record(0, false), BudgetVerdict::Continue);
        assert_eq!(t.record(0, false), BudgetVerdict::Continue);
        assert_eq!(t.record(0, false), BudgetVerdict::StopDiminishing { low_rounds: 3 });
    }

    #[test]
    fn chars_variant() {
        let mut t = OutputBudgetTracker::new();
        assert_eq!(t.record_chars(50, false), BudgetVerdict::Continue);
        assert_eq!(t.record_chars(50, false), BudgetVerdict::Continue);
        assert_eq!(t.record_chars(50, false), BudgetVerdict::StopDiminishing { low_rounds: 3 });
        // 长结果不触发
        let mut t2 = OutputBudgetTracker::new();
        for _ in 0..10 {
            assert_eq!(t2.record_chars(500, false), BudgetVerdict::Continue);
        }
    }
}
