//! 快速语义分析步骤 + 并行执行容器。
//!
//! ## FastSemanticStep
//! 在 LLM 主调用前对用户输入做多维度嵌入分类（情绪/意图/话题/记忆重要性/关系信号），
//! 驱动 prompt 动态组装。失败时仅记录日志并跳过，不阻塞主对话流。
//!
//! ## ParallelStep
//! 并行执行两个子 Runnable，合并结果到单一 PipelineState。
//! 用于让 FastSemanticStep 与 QueryRewriteStep 并行执行，缩短用户等待时间：
//! 耗时 = max(嵌入, LLM 改写) 而非 sum。

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::cross_character::parse_speaker_prefix;
use crate::emotion::FastSemanticAnalyzer;
use crate::error::VivianResult;
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::state::PipelineState;

/// 快速语义分析步骤。
pub struct FastSemanticStep {
    analyzer: Arc<FastSemanticAnalyzer>,
    char_id: String,
}

impl FastSemanticStep {
    pub fn new(analyzer: Arc<FastSemanticAnalyzer>, char_id: String) -> Self {
        Self { analyzer, char_id }
    }
}

#[async_trait]
impl Runnable for FastSemanticStep {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        if state.user_input.is_empty() || state.is_command {
            return Ok(state.to_json());
        }

        // 跳过跨角色前缀消息，避免对路由文本误判
        let analyze_text = parse_speaker_prefix(&state.user_input).0;
        match self.analyzer.analyze(&analyze_text) {
            Ok(perception) => {
                if !perception.guidance.is_empty() {
                    tracing::debug!(
                        "[FastSemanticStep:{}] emotion={} intent={} topics={} memory={} rel={} guidance=\"{}\"",
                        self.char_id,
                        perception.emotion.emotion,
                        perception.intent.label,
                        perception.topics.iter().map(|t| t.label.as_str()).collect::<Vec<_>>().join(","),
                        perception.memory_importance.label,
                        perception.relationship_signal.label,
                        perception.guidance
                    );
                }
                state.fast_perception = Some(perception.clone());
                // 同步填充认知知识需求评估（FastPerceptionResult 已包含 epistemic_assessment）
                state.epistemic_assessment = Some(perception.epistemic_assessment);
            }
            Err(e) => {
                tracing::warn!(
                    "[FastSemanticStep:{}] 快速语义感知失败，跳过：{}",
                    self.char_id,
                    e
                );
            }
        }

        Ok(state.to_json())
    }
}

/// 并行执行两个子 Runnable，合并结果。
///
/// 两个分支各自收到输入 PipelineState 的克隆，独立执行后合并：
/// - 分支 A 的结果作为 base，分支 B 的非默认字段合并进去
/// - metadata 执行 JSON 对象逐键覆盖
pub struct ParallelStep {
    a: Box<dyn Runnable>,
    b: Box<dyn Runnable>,
}

impl ParallelStep {
    pub fn new(a: Box<dyn Runnable>, b: Box<dyn Runnable>) -> Self {
        Self { a, b }
    }
}

#[async_trait]
impl Runnable for ParallelStep {
    async fn ainvoke(&self, input: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        let input_a = input.clone();
        let input_b = input;

        let (res_a, res_b) = tokio::join!(
            self.a.ainvoke(input_a, config.clone()),
            self.b.ainvoke(input_b, config),
        );

        let mut state_a = PipelineState::from_json(res_a?);
        let state_b = PipelineState::from_json(res_b?);
        state_a.merge_parallel_result(state_b);
        Ok(state_a.to_json())
    }
}

