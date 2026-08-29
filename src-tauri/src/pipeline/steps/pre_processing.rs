//! 预处理流水线步骤：trim + 输入长度检测 + `/` 命令前缀识别。
//!
//! - [`PreProcessingStep`]：基础预处理

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{VivianError, VivianResult};
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::injection_guard::guard_user_content;
use crate::pipeline::state::PipelineState;

// ============================================================================
// PreProcessingStep：基础 trim + 注入检测 + 输入检测
// ============================================================================

pub struct PreProcessingStep;

impl PreProcessingStep {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PreProcessingStep {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for PreProcessingStep {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        let trimmed = state.user_input.trim().to_string();
        if trimmed.is_empty() {
            return Err(VivianError::Engine("用户输入为空".to_string()));
        }

        let is_command = trimmed.starts_with('/');
        if is_command {
            let command = trimmed
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            state.metadata["is_command"] = json!(true);
            state.metadata["command"] = json!(command);
        } else {
            state.metadata["is_command"] = json!(false);
        }

        // 注入检测：非命令场景下扫描用户消息，命中时追加安全标注并记录标签
        if !is_command {
            let (guarded, labels) = guard_user_content(&trimmed);
            state.metadata["injection_detected"] = json!(labels.is_injected());
            if labels.is_injected() {
                state.metadata["injection_rules"] =
                    json!(labels.hit_rules);
                state.user_input = guarded;
            } else {
                state.user_input = trimmed;
            }
        } else {
            state.metadata["injection_detected"] = json!(false);
            state.user_input = trimmed;
        }

        state.metadata["input_length"] = json!(state.user_input.chars().count());
        state.metadata["input_bytes"] = json!(state.user_input.len());

        Ok(state.to_json())
    }
}
