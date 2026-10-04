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
//!
//! 本模块只管「产出量」这一路信号；「是否构成实质进展」由调用方判定后传入
//! （工作侧见 `coding_agent::is_substantive_progress`）。判定口径若过窄，会把
//! 「先调研后交付」型任务在取证阶段误判为空转——这类任务直到定稿才第一次写文件。

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
    /// `made_progress` 为本轮是否取得实质进展（写/改/执行类工具成功、
    /// 取证类工具取到内容等，见 `coding_agent::is_substantive_progress`）。
    ///
    /// `0` 计入低产出：完全没有输出是最彻底的空转，不能反过来当成「产出正常」。
    /// `record_chars` 无此问题（`0 < LOW_OUTPUT_CHARS` 恒真），两条路径必须一致。
    pub fn record(&mut self, output_tokens: u64, made_progress: bool) -> BudgetVerdict {
        self.record_impl(output_tokens < LOW_OUTPUT_TOKENS, made_progress)
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
        // 实质进展清零计数：前两次低产出的累积作废
        assert_eq!(t.record(10, true), BudgetVerdict::Continue);
        assert_eq!(t.record(10, false), BudgetVerdict::Continue);
        assert_eq!(t.record(10, false), BudgetVerdict::Continue);
        // 清零后需重新累积满 DIMINISHING_ROUNDS 轮才停机
        assert_eq!(
            t.record(10, false),
            BudgetVerdict::StopDiminishing { low_rounds: DIMINISHING_ROUNDS }
        );
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

    /// 阈值边界：恰好等于 `LOW_OUTPUT_TOKENS` 不算低产出（判定是 `<` 而非 `<=`）。
    #[test]
    fn threshold_boundary_is_exclusive() {
        let mut t = OutputBudgetTracker::new();
        assert_eq!(t.record(LOW_OUTPUT_TOKENS - 1, false), BudgetVerdict::Continue);
        assert_eq!(t.record(LOW_OUTPUT_TOKENS - 1, false), BudgetVerdict::Continue);
        // 第三轮本该停机，但一次「刚好达标」的高产出轮次应清零计数
        assert_eq!(t.record(LOW_OUTPUT_TOKENS, false), BudgetVerdict::Continue);
        assert_eq!(t.record(LOW_OUTPUT_TOKENS - 1, false), BudgetVerdict::Continue);
        assert_eq!(t.record(LOW_OUTPUT_TOKENS - 1, false), BudgetVerdict::Continue);
        assert_eq!(
            t.record(LOW_OUTPUT_TOKENS - 1, false),
            BudgetVerdict::StopDiminishing { low_rounds: 3 }
        );
    }

    /// 两条记录路径对 0 的判定必须一致：0 token 与 0 字符都是最彻底的空转。
    /// 曾出现 `record` 用 `tokens > 0 && tokens < THRESHOLD` 而把 0 判成「产出正常」，
    /// 计数器被清零，空转反而永不触发。
    #[test]
    fn both_paths_agree_on_zero() {
        let mut a = OutputBudgetTracker::new();
        let mut b = OutputBudgetTracker::new();
        for round in 1..DIMINISHING_ROUNDS {
            let via_tokens = a.record(0, false);
            let via_chars = b.record_chars(0, false);
            assert_eq!(via_tokens, via_chars, "round {round}: 两条路径判定不一致");
            assert_eq!(via_tokens, BudgetVerdict::Continue);
        }
        let stop_a = a.record(0, false);
        let stop_b = b.record_chars(0, false);
        assert_eq!(stop_a, stop_b);
        assert_eq!(stop_a, BudgetVerdict::StopDiminishing { low_rounds: DIMINISHING_ROUNDS });
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
