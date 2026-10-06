//! 回复格式验证 Runnable
//!
//! 在 ResponseParsing 之后、ExpressionMotion 之前执行，对 AI 回复做验证：
//! - 空文本检测：should_respond=true 但 text 为空时记录 warning
//! - 长度上限截断：超过 MAX_RESPONSE_CHARS 时在句边界截断
//! - 基础清理：去除首尾空白、折叠连续空行
//! - 事实核对（可选）：注入 router 后，对非空主对话回复执行，
//!   逐条核对当前用户、工具回执和历史证据；记录矛盾/未知，不将未知视为通过。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::VivianResult;
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::state::PipelineState;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

fn evidence_json(evidence: &super::faithfulness::Evidence) -> String {
    serde_json::to_string(evidence).expect("evidence only contains JSON serializable strings")
}

/// 回复文本最大字符数（超过后在句边界截断）
const MAX_RESPONSE_CHARS: usize = 500;

/// 截断时保留的最小字符数（避免截断到只剩几个字）
const MIN_KEEP_CHARS: usize = 50;

/// 幻觉检测超时时间
const HALLUCINATION_CHECK_TIMEOUT: Duration = Duration::from_secs(8);

pub struct ValidationRunnable {
    /// 可选的 LLM 路由器：注入后启用轻量幻觉检测
    router: Option<Arc<ModelRouter>>,
}

impl ValidationRunnable {
    pub fn new() -> Self {
        Self { router: None }
    }

    /// 注入 LLM 路由器，启用轻量幻觉检测
    pub fn with_router(router: Arc<ModelRouter>) -> Self {
        Self { router: Some(router) }
    }

    /// 在句边界截断文本
    ///
    /// 从句末标点（。！？.!?）处断开，尽量不截断到半句话。
    /// 如果找不到合适的句边界，在 MIN_KEEP_CHARS 之后的空格/换行处截断。
    fn truncate_at_boundary(text: &str, max_chars: usize) -> String {
        let char_count = text.chars().count();
        if char_count <= max_chars {
            return text.to_string();
        }

        // 收集字符索引，用于按字符数而非字节数定位
        let chars: Vec<char> = text.chars().collect();

        // 从句末标点向前搜索
        let sentence_ends: &[char] = &['。', '！', '？', '.', '!', '?', '\n'];
        let mut best_cut = None;
        for i in (MIN_KEEP_CHARS..max_chars).rev() {
            if sentence_ends.contains(&chars[i]) {
                best_cut = Some(i + 1); // 保留句末标点
                break;
            }
        }

        // 找不到句边界时，在空格处截断
        if best_cut.is_none() {
            for i in (MIN_KEEP_CHARS..max_chars).rev() {
                if chars[i] == ' ' || chars[i] == '\n' {
                    best_cut = Some(i);
                    break;
                }
            }
        }

        match best_cut {
            Some(cut) => {
                let truncated: String = chars[..cut].iter().collect();
                format!("{}…", truncated.trim_end())
            }
            None => {
                // 极端情况：直接硬截断
                let truncated: String = chars[..max_chars].iter().collect();
                format!("{}…", truncated.trim_end())
            }
        }
    }

    /// 折叠连续空行为单个换行，去除首尾空白
    fn normalize_whitespace(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut prev_blank = false;
        for line in text.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                if !prev_blank && !result.is_empty() {
                    result.push('\n');
                }
                prev_blank = true;
            } else {
                result.push_str(trimmed);
                result.push('\n');
                prev_blank = false;
            }
        }
        result.trim().to_string()
    }

    /// 单次有截止时间的事实核对；失败和证据不足都不能当成通过。
    async fn check_faithfulness(router: &ModelRouter, state: &PipelineState) -> Result<Value, String> {
        use super::faithfulness::{Evidence, CHECK_PROMPT, schema, assess};
        use crate::providers::reasoning::{ReasoningPreference, ReasoningMode, ReasoningEffort};
        let evidence = Evidence::from_state(state);
        let request = LLMRequest::new("memory", vec![
            ChatMessage::system(CHECK_PROMPT),
            ChatMessage::user(evidence_json(&evidence)),
        ])
            .with_json_schema(schema())
            .with_max_tokens(1800)
            .with_temperature(0.0)
            .with_reasoning_pref(ReasoningPreference { mode: ReasoningMode::Off,
                effort: Some(ReasoningEffort::Minimal), budget_tokens: None })
            .without_framework_instructions();
        let raw = tokio::time::timeout(HALLUCINATION_CHECK_TIMEOUT, router.generate(request)).await
            .map_err(|_| "事实核对超时，未得出结论".to_string())?
            .map_err(|e| format!("事实核对调用失败: {e}"))?;
        assess(&raw, &evidence)
    }

}

#[async_trait]
impl Runnable for ValidationRunnable {
    async fn ainvoke(
        &self,
        input: Value,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 跳过不需要回复的场景
        if state.is_command || !state.should_respond || state.graceful_exit {
            return Ok(state.to_json());
        }

        state.text = crate::utils::protocol_text::strip_protocol_text(&state.text);
        if let Some(response) = state.ai_response.as_mut() {
            response.text = crate::utils::protocol_text::strip_protocol_text(&response.text);
        }
        // 1. 空文本检测
        if state.text.trim().is_empty() && state.sticker.is_none() {
            tracing::warn!(
                "[Validation] AI 回复文本为空（should_respond=true, response_mode={}）",
                state.response_mode
            );
            // 不修改 state，让下游处理空文本
            return Ok(state.to_json());
        }

        // 2. 空白清理
        let cleaned = Self::normalize_whitespace(&state.text);
        if cleaned != state.text {
            tracing::debug!("[Validation] 回复空白清理：{} → {} 字符", state.text.chars().count(), cleaned.chars().count());
            state.text = cleaned;
        }

        // 3. 长度截断
        let char_count = state.text.chars().count();
        if char_count > MAX_RESPONSE_CHARS {
            let truncated = Self::truncate_at_boundary(&state.text, MAX_RESPONSE_CHARS);
            tracing::debug!(
                "[Validation] 回复超长截断：{} → {} 字符（上限 {}）",
                char_count,
                truncated.chars().count(),
                MAX_RESPONSE_CHARS
            );
            state.text = truncated;
        }

        // 保证核对的是最终返回的正文；格式清理不能只更新 PipelineState.text。
        if let Some(response) = state.ai_response.as_mut() { response.text = state.text.clone(); }

        // 所有非空主对话回复均可核对，包括短回复、无记忆但有工具回执的回复。
        // 跨角色工具路径预算有限，显式记录未核对，而不是给出“通过”。
        state.metadata["hallucination_check"] = if state.current_channel == "cross_character" {
            json!({"status":"skipped", "reason":"cross_character_latency_budget"})
        } else if let Some(router) = &self.router {
            match Self::check_faithfulness(router, &state).await {
                Ok(report) => {
                    if report["status"] == "flagged" {
                        tracing::warn!("[Validation] 模型报告潜在证据矛盾: {}", report);
                    } else if report["status"] == "uncertain" {
                        tracing::debug!("[Validation] 部分事实暂无法验证: {}", report);
                    }
                    report
                }
                Err(error) => {
                    tracing::debug!("[Validation] 事实核对未完成: {}", error);
                    json!({"status":"unknown", "reason":error})
                }
            }
        } else { json!({"status":"skipped", "reason":"router_not_configured"}) };

        Ok(state.to_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn short_reply_without_memory_uses_current_tool_evidence_and_invalid_report_is_unknown() {
        use axum::{Router, Json, extract::State, routing::post};
        type Captured = Arc<parking_lot::Mutex<Vec<Value>>>;
        async fn handle(State(captured): State<Captured>, Json(body): Json<Value>) -> Json<Value> {
            let index = { let mut requests = captured.lock(); let index = requests.len(); requests.push(body); index };
            let content = if index == 0 {
                json!({"non_factual":false,"claims":[{"claim":"换好了","verdict":"contradicted",
                    "citations":[{"source_id":"current_tool:0","quote":"\"success\":false"}],"reason":"工具实际拒绝执行"}]}).to_string()
            } else { "OK".to_string() };
            Json(json!({"id":"audit-test","object":"chat.completion","choices":[{"index":0,
                "message":{"role":"assistant","content":content},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":1,"completion_tokens":1}}))
        }
        let captured: Captured = Arc::new(parking_lot::Mutex::new(vec![]));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = crate::config::manager::AppConfig::default();
        config.enable_routing_matrix = false;
        config.ai.provider = "chat_completions".into();
        config.ai.endpoint = Some(format!("http://{}/v1", listener.local_addr().unwrap()));
        config.ai.api_key = Some("local-test-key".into());
        config.ai.model = format!("test-faithfulness-{}", uuid::Uuid::new_v4());
        config.network.proxy_mode = "direct".into();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let app = Router::new().route("/v1/chat/completions", post(handle)).with_state(captured.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).with_graceful_shutdown(async { let _ = stop_rx.await; }).await.unwrap(); });
        let runnable = ValidationRunnable::with_router(Arc::new(ModelRouter::new(&config).unwrap()));
        let mut state = PipelineState::default();
        state.user_input = "帮我设置无尽の梦，我今年29岁。".into();
        state.text = "换好了。".into();
        state.ai_response = Some(crate::types::response::AiResponse::new(state.text.clone()));
        state.metadata["verified_tool_receipts"] = json!([{"tool_name":"wallpaper_set","success":false,"result":null,"error":"拒绝执行"}]);
        let result = tokio::time::timeout(Duration::from_secs(15), runnable.ainvoke(state.to_json(), None)).await.unwrap().unwrap();
        let result = PipelineState::from_json(result);
        assert_eq!(result.metadata["hallucination_check"]["status"], "flagged");
        state.text = "你好呀。".into();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let result = PipelineState::from_json(result);
        assert_eq!(result.metadata["hallucination_check"]["status"], "unknown");
        assert_eq!(result.ai_response.unwrap().text, "你好呀。");
        let _ = stop_tx.send(()); server.await.unwrap();
        let requests = captured.lock();
        assert_eq!(requests.len(), 2, "单次核对不得叠加 NOUL 与 fallback 超时");
        let messages = requests[0]["messages"].to_string();
        assert!(messages.contains("我今年29岁"));
        assert!(messages.contains("current_tool:0"));
        assert!(!messages.contains("PERSONA_LOAD"));
    }

    #[test]
    fn truncate_short_text_unchanged() {
        let text = "你好呀～";
        assert_eq!(ValidationRunnable::truncate_at_boundary(text, 500), text);
    }

    #[test]
    fn truncate_long_text_at_sentence() {
        let text = "这是第一句话。这是第二句话。这是第三句话。这是第四句话。这是第五句话。";
        let result = ValidationRunnable::truncate_at_boundary(text, 20);
        assert!(result.chars().count() <= 22); // 20 + 句末标点 + …
        assert!(result.ends_with('…'));
    }

    #[test]
    fn normalize_whitespace_collapses_blanks() {
        let text = "你好\n\n\n\n世界\n\n\n你好";
        let result = ValidationRunnable::normalize_whitespace(text);
        assert_eq!(result, "你好\n\n世界\n\n你好");
    }

    #[test]
    fn normalize_whitespace_trims() {
        let text = "  \n  你好  \n  ";
        let result = ValidationRunnable::normalize_whitespace(text);
        assert_eq!(result, "你好");
    }
}
