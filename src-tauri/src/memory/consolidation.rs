//! 记忆巩固 —— 夜间/空闲时整理记忆，模拟"睡眠巩固"
//!
//! 在深夜（2-5 点）或用户长时间离场时触发：
//! 1. 调用现有 ConsolidationPipeline 跑完整三阶段（ShortTerm→MidTerm→LongTerm→Insight）
//! 2. 强化近期重要记忆（提升 importance）
//! 3. 衰减无关的临时记忆
//! 4. Belief/Goal 生成（Stage 4）：从 Insight + LongTerm 提炼信念写入 Mind
//!
//! 设计：复用现有 MemoryManager 与 ConsolidationPipeline，不重复造轮子。

use std::sync::Arc;

use parking_lot::Mutex;

use crate::memory::manager::MemoryManager;
use crate::memory::pipeline::ConsolidationPipeline;
use crate::mind::{BeliefGenerator, Mind};

/// 记忆巩固器
pub struct MemoryConsolidator {
    memory: Arc<MemoryManager>,
    pipeline: Arc<ConsolidationPipeline>,
    /// Belief/Goal 生成器（可选 —— 未注入 Mind 时不执行 Stage 4）
    belief_generator: Option<Arc<BeliefGenerator>>,
    /// 关联的 Mind（可选 —— 未注入时跳过 Belief 生成）
    mind: Option<Arc<Mind>>,
    /// 上次巩固时间戳
    last_consolidation: Mutex<f64>,
}

impl MemoryConsolidator {
    pub fn new(memory: Arc<MemoryManager>, pipeline: Arc<ConsolidationPipeline>) -> Self {
        Self {
            memory,
            pipeline,
            belief_generator: None,
            mind: None,
            last_consolidation: Mutex::new(0.0),
        }
    }

    /// 注入 Mind 与 BeliefGenerator，启用 Stage 4（Belief/Goal 生成）
    ///
    /// 由 Brain 在初始化 Mind 后调用。注入后每次巩固末尾会额外生成 Belief/Goal。
    pub fn with_mind(mut self, mind: Arc<Mind>) -> Self {
        let router = self.pipeline.router();
        self.belief_generator = Some(Arc::new(BeliefGenerator::new(router)));
        self.mind = Some(mind);
        self
    }

    /// 是否冷却已过（距上次巩固 ≥ 6 小时）
    ///
    /// 仅检查冷却，窗口判断由调用方负责（与配置的睡眠窗口保持一致）。
    pub fn should_run(&self) -> bool {
        let now = chrono::Utc::now().timestamp() as f64;
        let last = *self.last_consolidation.lock();
        now - last >= 6.0 * 3600.0
    }

    /// 执行一次记忆巩固
    ///
    /// 返回是否实际执行了巩固（若距上次不足 6 小时则跳过）。
    pub async fn consolidate(&self) -> bool {
        let now = chrono::Utc::now().timestamp() as f64;
        {
            let last = *self.last_consolidation.lock();
            if now - last < 6.0 * 3600.0 {
                // 距上次巩固不足 6 小时，跳过
                return false;
            }
        }

        tracing::info!("开始夜间记忆巩固...");

        // 跑完整巩固流水线（Stage 1/2/3：ShortTerm→MidTerm→LongTerm→Insight）
        // ConsolidationPipeline::run 会处理 ShortTerm 摘要、画像抽取、Insight 生成
        match self.pipeline.run(&self.memory).await {
            Ok(report) => {
                tracing::info!("记忆巩固完成: {:?}", report);
            }
            Err(e) => {
                tracing::warn!("记忆巩固流水线失败: {}", e);
            }
        }

        // Stage 4: Belief/Goal 生成（仅在注入 Mind 时执行）
        if let (Some(gen), Some(mind)) = (&self.belief_generator, &self.mind) {
            match gen.generate(&self.memory, mind).await {
                Ok(report) => {
                    tracing::info!("Belief 生成完成: {:?}", report);
                }
                Err(e) => {
                    tracing::warn!("Belief 生成失败: {}", e);
                }
            }
        }

        *self.last_consolidation.lock() = now;
        true
    }
}
