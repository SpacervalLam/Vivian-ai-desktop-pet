//! 放松阶梯公共模块：5 阶逐级放宽过滤条件，保证结果集单调递增。
//!
//! strict → drop_importance_min → drop_categories → drop_subjects → no_filters
//! 每阶条件累积 OR，避免 strict 过滤导致空结果。
//!
//! 调用方提供阶段谓词 `predicate(stage, &MemoryItem) -> bool`，
//! 阶梯迭代逻辑与 STAGE_NAMES 由本模块统一维护，消除 filter.rs 与
//! precision_filter.rs 的重复实现。

use super::types::MemoryItem;

/// 5 阶放松阶梯的阶段名（固定顺序，索引即阶段编号）
pub const STAGE_NAMES: [&str; 5] = [
    "strict",
    "drop_importance_min",
    "drop_categories",
    "drop_subjects",
    "no_filters",
];

/// 放松阶梯执行器：按阶段顺序逐级放宽，首个结果数 ≥ `min_results` 的阶段胜出。
///
/// 全部阶段都不足时回退到最后阶段（no_filters）的全部候选，保证非空。
pub struct RelaxationLadder {
    pub min_results: usize,
}

impl RelaxationLadder {
    pub fn new(min_results: usize) -> Self {
        Self {
            min_results: min_results.max(1),
        }
    }

    /// 按阶段顺序逐级过滤，返回首个满足 `min_results` 的阶段结果。
    ///
    /// `predicate(stage, &m)` 由调用方实现：stage 是阶段索引（0..STAGE_NAMES.len()），
    /// 返回 true 表示该记忆在此阶段通过过滤。
    pub fn run<P>(&self, memories: &[MemoryItem], predicate: P) -> Vec<MemoryItem>
    where
        P: Fn(usize, &MemoryItem) -> bool,
    {
        for stage in 0..STAGE_NAMES.len() {
            let allowed: Vec<&MemoryItem> = memories
                .iter()
                .filter(|m| predicate(stage, m))
                .collect();

            if allowed.len() >= self.min_results || stage == STAGE_NAMES.len() - 1 {
                tracing::debug!(
                    "[RelaxationLadder] 命中 stage={} ({}): {} 条记忆",
                    stage,
                    STAGE_NAMES[stage],
                    allowed.len()
                );
                return allowed.into_iter().cloned().collect();
            }
        }
        Vec::new()
    }
}

impl Default for RelaxationLadder {
    fn default() -> Self {
        Self::new(3)
    }
}
