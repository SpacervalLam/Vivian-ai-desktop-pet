//! 主动搜索流水线步骤 —— 基于认知知识需求评估（Epistemic Assessment）驱动搜索。
//!
//! 设计理念：
//! - Web Search 是认知能力，而不是用户显式调用的工具
//! - 认知知识需求评估由 FastSemantic 阶段同步完成（纯规则，不调用 LLM），
//!   产出多维 EpistemicAssessment（semantic_clarity / factual_dependence /
//!   temporal_sensitivity / interpretation_risk / knowledge_gap）和 KnowledgeDecision。
//! - 本步骤读取 EpistemicAssessment，根据 KnowledgeDecision 决定是否主动搜索：
//!   - SearchRequired → 必须搜索
//!   - SearchPreferred → 建议搜索（有帮助但非必需）
//!   - SearchOptional / NoSearch → 跳过
//! - 搜索结果作为上下文注入 prompt，让 LLM 基于实际资料回答
//! - 与 LLM function calling 路径互补：预搜索在生成前完成，LLM 生成时仍可自主调用 web_search

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::emotion::KnowledgeDecision;
use crate::error::VivianResult;
use crate::network::web::{WebSearchRequest, WebSearchService, WebSearchSource};
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::state::PipelineState;
use crate::providers::ModelRouter;

// ============================================================================
// 搜索结果格式化
// ============================================================================

/// 将搜索结果格式化为 prompt 可注入的文本
fn format_search_results(query: &str, results: &[WebSearchSource]) -> String {
    if results.is_empty() {
        return String::new();
    }

    let mut lines = Vec::with_capacity(results.len() + 2);
    lines.push(format!("搜索关键词: {}", query));
    lines.push("搜索结果:".to_string());

    for (i, r) in results.iter().take(5).enumerate() {
        lines.push(format!(
            "{}. {}\n   {}",
            i + 1,
            r.display_title(),
            r.snippet_text()
        ));
    }

    lines.join("\n")
}

// ============================================================================
// WebContextRunnable（主动搜索）
// ============================================================================

/// Web 上下文 Runnable —— 基于认知知识需求评估驱动主动搜索。
///
/// 与 FastSemantic 阶段同步计算的 EpistemicAssessment 协同工作：
/// - FastSemantic 产出多维评估（纯规则）
/// - 本步骤根据 KnowledgeDecision 决策是否搜索
/// - LLM 生成时仍可自主调用 web_search 工具做进一步搜索
///
/// 与 LLM function calling 路径互补：
/// - 本步骤：在生成前根据 KnowledgeDecision 执行预搜索，结果作为上下文注入 prompt
/// - LLM 工具调用：生成过程中 LLM 仍可自主调用 web_search 工具做进一步搜索
pub struct WebContextRunnable {
    pub router: Option<Arc<ModelRouter>>,
}

impl WebContextRunnable {
    pub fn new() -> Self {
        Self { router: None }
    }

    pub fn with_router(router: Arc<ModelRouter>) -> Self {
        Self { router: Some(router) }
    }
}

impl Default for WebContextRunnable {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for WebContextRunnable {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 命令或空输入跳过
        if state.is_command || state.user_input.trim().is_empty() {
            return Ok(state.to_json());
        }

        // 跨角色消息跳过（不影响角色间对话）
        if state.current_channel == "cross_character" {
            return Ok(state.to_json());
        }

        // 读取认知知识需求评估（由 FastSemantic 阶段同步计算）
        let assessment = match &state.epistemic_assessment {
            Some(a) => a,
            None => {
                // FastSemantic 未运行（极低概率），跳过
                return Ok(state.to_json());
            }
        };

        // 决策是否主动搜索
        let should_search = matches!(
            assessment.decision,
            KnowledgeDecision::SearchRequired | KnowledgeDecision::SearchPreferred
        );

        if !should_search {
            tracing::debug!(
                "[WebContext] 决策: {:?}, 跳过搜索。原因: {}",
                assessment.decision,
                assessment.reason
            );
            return Ok(state.to_json());
        }

        // 获取搜索关键词
        let query = match &assessment.search_query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => {
                // 决策要求搜索但无有效关键词，跳过
                tracing::debug!("[WebContext] 决策要求搜索但无有效关键词，跳过");
                return Ok(state.to_json());
            }
        };

        // 执行搜索（委托 network::web 服务缝隙）
        let (config, proxy_url) = crate::network::web::read_search_config();

        let config_ref = config.as_ref();
        let proxy_ref = proxy_url.as_deref();

        tracing::info!(
            "[WebContext] 主动搜索触发: decision={:?}, reason='{}', query={:?}, \
             clarity={:.2}, factual={:.2}, temporal={:.2}, risk={:.2}, gap={:.2}",
            assessment.decision,
            assessment.reason,
            query,
            assessment.semantic_clarity,
            assessment.factual_dependence,
            assessment.temporal_sensitivity,
            assessment.interpretation_risk,
            assessment.knowledge_gap,
        );

        let request = WebSearchRequest::new(&query).with_max_results(5);
        let result = WebSearchService::shared()
            .search(&request, config_ref, proxy_ref)
            .await;

        let sources = match result {
            Ok(r) if !r.sources.is_empty() => r.sources,
            // 搜索成功但无匹配：跳过注入
            Ok(_) => {
                tracing::info!("[WebContext] 主动搜索无结果，跳过注入");
                return Ok(state.to_json());
            }
            // 搜索失败：记日志跳过，不影响主对话链路
            Err(e) => {
                tracing::warn!("[WebContext] 主动搜索失败，跳过注入: {e}");
                return Ok(state.to_json());
            }
        };

        let search_text = format_search_results(&query, &sources);
        if !search_text.is_empty() {
            state.web_context = format!(
                "## 主动搜索结果\n\
                （系统检测到用户输入可能包含你不熟悉的内容，已预先搜索。\
                请基于以下搜索结果回答用户，不要假装你本来就知道这些内容。）\n\n\
                {}\n\n\
                注意：\n\
                - 区分搜索得到的事实和你的推断\n\
                - 如果搜索结果仍无法确认用户的意思，要明确告诉用户\n\
                - 不要为了保持对话自然而假装知道",
                search_text
            );

            tracing::info!(
                "[WebContext] 主动搜索完成: {} 条结果, 已注入 web_context",
                sources.len()
            );
        }

        Ok(state.to_json())
    }
}