//! Web 上下文流水线步骤 —— 已弃用关键词决策，改为透传。
//!
//! 搜索决策完全交由 LLM 通过 `web_search` 工具自主判断，
//! 本步骤仅保留 Runnable 接口以兼容管线注册。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::VivianResult;
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::state::PipelineState;
use crate::providers::ModelRouter;

// ============================================================================
// WebContextRunnable（透传）
// ============================================================================

/// Web 上下文 Runnable —— 不再做关键词决策，仅透传状态。
///
/// 搜索决策已迁移至 LLM function calling 路径：LLM 通过 `web_search` 工具
/// 自主判断何时需要联网检索，不再依赖关键词匹配。
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
        let state = PipelineState::from_json(input);
        // 搜索决策已迁移至 LLM function calling，本步骤仅透传
        Ok(state.to_json())
    }
}
