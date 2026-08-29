//! 智谱 GLM Provider（Chat Completions 协议 + 智谱专属联网搜索）
//!
//! 智谱 GLM API 与 OpenAI Chat Completions 兼容，端点为
//! `https://open.bigmodel.cn/api/paas/v4/chat/completions`。
//!
//! 与 `ChatCompletionsProvider` 的差异：
//! - 联网搜索：智谱专属 `tools=[{"type":"web_search","web_search":{"enable":true,"search_result":true}}]`
//!   在无自定义工具绑定时注入，与 function calling 工具列表互斥。
//! - 认证：`Authorization: Bearer <API Key>`（直接用 API Key，无需 JWT）。
//! - 推理内容：GLM-4.6+ 通过 `reasoning_content` 字段返回思考过程（与 DeepSeek 风格一致）。
//!
//! 参考文档：
//! - 端点：POST https://open.bigmodel.cn/api/paas/v4/chat/completions
//! - 模型：glm-4.6 / glm-4-plus / glm-4-air / glm-4-flash / glm-4v 等
//! - 联网搜索：tools 字段中 type=web_search 的内置工具

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::config::manager::ProviderConfig;
use crate::error::{VivianError, VivianResult};
use crate::providers::base::{
    BaseProvider, ChatResponse, ProviderBase, StreamEvent, ToolDefinition,
};
use crate::providers::chat_completions::ChatCompletionsProvider;
use crate::providers::thinking_stripper::{
    leaks_thinking_in_content, ThinkingStreamStripper,
};
use crate::resilience::{classify_error, ErrorCategory};
use crate::types::response::ChatMessage;
use crate::utils::messages_cache_key;

const MAX_RETRIES: usize = 2;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const LARGE_PROMPT_BYTES: usize = 20_000;
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// 智谱 GLM API 限制 temperature 最多 2 位小数（错误码 1210）。
/// 对 effective_temperature() 结果做截断，避免 0.123 等值被拒绝。
fn zhipu_temperature(raw: f64) -> f64 {
    (raw * 100.0).round() / 100.0
}

/// 智谱 GLM 各模型 max_tokens 上限差异（错误码 1210）。
/// GLM-4V 系列 max_tokens 上限 1024，其余模型沿用传入值。
/// 对 effective_max_tokens() 结果做钳制，避免超出模型限制被拒绝。
fn zhipu_max_tokens(model: &str, raw: u32) -> u32 {
    let lower = model.to_lowercase();
    if lower.contains("4v") {
        raw.min(1024)
    } else {
        raw
    }
}

/// 智谱 GLM Provider
pub struct ZhipuProvider {
    base: ProviderBase,
    tools: Vec<ToolDefinition>,
    instructions: Option<String>,
}

impl ZhipuProvider {
    pub fn new(
        config: &ProviderConfig,
        temperature: f64,
        max_tokens: u32,
        proxy: Option<String>,
        client: Option<reqwest::Client>,
    ) -> Self {
        let base = ProviderBase::new(
            config.api_key.clone(),
            config.base_url.clone(),
            config.model.clone(),
            temperature,
            max_tokens,
        );
        Self {
            base: ProviderBase {
                proxy,
                client,
                ..base
            },
            tools: Vec::new(),
            instructions: None,
        }
    }

    pub fn with_instructions(mut self, instructions: Option<String>) -> Self {
        self.instructions = instructions;
        self
    }

    fn endpoint(&self) -> String {
        let url = self.base.base_url.trim_end_matches('/');
        if url.ends_with("/chat/completions") {
            url.to_string()
        } else {
            format!("{}/chat/completions", url)
        }
    }

    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder.bearer_auth(&self.base.api_key)
    }

    /// 构造 tools 字段（Chat Completions 嵌套格式）
    fn build_tools_field(&self) -> Option<Value> {
        if self.tools.is_empty() {
            return None;
        }
        let arr: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        Some(Value::Array(arr))
    }

    /// 注入智谱 GLM 专属联网搜索工具
    ///
    /// 智谱 GLM 的联网搜索通过 tools 字段注入 `{"type":"web_search",...}` 内置工具。
    /// 当已有自定义 function calling 工具绑定时跳过（两者互斥，避免工具列表冲突）。
    fn inject_web_search(&self, body: &mut Value, enable_search: bool) {
        if !enable_search {
            return;
        }
        if !self.tools.is_empty() {
            return;
        }
        body["tools"] = json!([{
            "type": "web_search",
            "web_search": {
                "enable": true,
                "search_result": true
            }
        }]);
    }

    /// 注入 response_format
    ///
    /// GLM `supports_structured_output()=false`，不支持 strict json_schema 模式
    /// （API 会静默忽略，导致 LLM 返回自由文本）。改用 `json_object` type，
    /// 由 API 保证返回合法 JSON 语法，结构由 prompt 文本约束。
    fn inject_response_format(body: &mut Value, json_schema: &Option<Value>) {
        if json_schema.is_some() {
            body["response_format"] = json!({"type": "json_object"});
        }
    }

    /// 按当前推理覆盖注入思考控制字段（GLM 思考型模型映射 thinking /
    /// reasoning_effort；强制思考模型的 Auto 档显式映射为较轻档位）。
    fn apply_reasoning_fields(&self, body: &mut Value, has_tools: bool) {
        let pref = self.base.effective_reasoning();
        let cap = crate::providers::reasoning::resolve_reasoning_capability(&self.base.model);
        crate::providers::reasoning::apply_reasoning_preference(body, pref, &cap, has_tools);
    }

    async fn send_request(&self, body: Value) -> VivianResult<Value> {
        let client = self.base.get_client();
        let started = Instant::now();
        let req = self
            .apply_auth(client.post(&self.endpoint()))
            .header("Content-Type", "application/json")
            .json(&body);
        let response = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                let elapsed = started.elapsed();
                let phase = if e.is_connect() {
                    "connect"
                } else if e.is_timeout() {
                    if elapsed.as_secs() < CONNECT_TIMEOUT_SECS {
                        "connect-timeout"
                    } else {
                        "read-timeout"
                    }
                } else if e.is_decode() {
                    "body-decode"
                } else {
                    "other"
                };
                tracing::warn!(
                    "[zhipu] {} 网络失败 phase={} elapsed={:.2}s err={}",
                    self.base.model,
                    phase,
                    elapsed.as_secs_f64(),
                    e
                );
                return Err(VivianError::Network(format!("网络请求失败: {e}")));
            }
        };

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "智谱 GLM 请求失败 ({}): {}",
                status, text
            )));
        }

        let json_val: Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "[zhipu] {} 响应解码失败 elapsed={:.2}s err={}",
                    self.base.model,
                    started.elapsed().as_secs_f64(),
                    e
                );
                return Err(VivianError::Network(format!("响应解码失败: {e}")));
            }
        };
        Ok(json_val)
    }

    async fn call_with_retry(
        &self,
        body: Value,
        cache_key_prompt: Option<&str>,
    ) -> VivianResult<String> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("[zhipu] 命中缓存: {}", self.base.model);
                return Ok(cached);
            }
        }

        let body_size = body.to_string().len();
        let bypass_circuit_failure = body_size > LARGE_PROMPT_BYTES;

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!("[zhipu] 第 {} 次重试: {}", attempt, self.base.model);
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(body.clone()).await {
                Ok(json_val) => {
                    let content = ChatCompletionsProvider::extract_content(&json_val)?;
                    self.base.record_success();
                    if let Some(prompt) = cache_key_prompt {
                        self.base.cache_response(prompt, &content);
                    }
                    return Ok(content);
                }
                Err(err) => {
                    if !bypass_circuit_failure {
                        self.base.record_failure();
                    }
                    match classify_error(&err) {
                        ErrorCategory::Permanent => return Err(err),
                        ErrorCategory::Transient | ErrorCategory::RateLimit => {
                            last_error = Some(err);
                            continue;
                        }
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| VivianError::Provider("重试次数耗尽".to_string())))
    }

    async fn invoke_with_retry(
        &self,
        body: Value,
        cache_key_prompt: Option<&str>,
    ) -> VivianResult<ChatResponse> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("[zhipu] 命中缓存(structured): {}", self.base.model);
                return Ok(ChatResponse::from_text(cached));
            }
        }

        let body_size = body.to_string().len();
        let bypass_circuit_failure = body_size > LARGE_PROMPT_BYTES;

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!(
                    "[zhipu] 第 {} 次重试(structured): {}",
                    attempt,
                    self.base.model
                );
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(body.clone()).await {
                Ok(json_val) => {
                    let resp = ChatCompletionsProvider::extract_chat_response(&json_val)?;
                    self.base.record_success();
                    if !resp.has_tool_calls() {
                        if let Some(prompt) = cache_key_prompt {
                            self.base.cache_response(prompt, &resp.content);
                        }
                    }
                    return Ok(resp);
                }
                Err(err) => {
                    if !bypass_circuit_failure {
                        self.base.record_failure();
                    }
                    match classify_error(&err) {
                        ErrorCategory::Permanent => return Err(err),
                        ErrorCategory::Transient | ErrorCategory::RateLimit => {
                            last_error = Some(err);
                            continue;
                        }
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| VivianError::Provider("重试次数耗尽".to_string())))
    }
}

#[async_trait]
impl BaseProvider for ZhipuProvider {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        crate::persona::prompt_render::check_messages_for_leaks(
            &messages,
            &format!("call_chat model={}", self.base.model),
        );
        let prompt_key = messages_cache_key(&messages);
        let mut body = json!({
            "model": self.base.model,
            "messages": ChatCompletionsProvider::build_messages(&messages, &self.instructions),
            "temperature": zhipu_temperature(self.base.effective_temperature()),
            "max_tokens": zhipu_max_tokens(&self.base.model, self.base.effective_max_tokens()),
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body, false);
        self.call_with_retry(body, Some(&prompt_key)).await
    }

    fn set_enable_search(&self, enable: bool) {
        self.base.set_enable_search(enable);
    }

    fn set_max_tokens_override(&self, tokens: u32) {
        self.base.set_max_tokens_override(tokens);
    }

    fn set_temperature_override(&self, temp: Option<f64>) {
        self.base.set_temperature_override(temp);
    }

    fn set_omit_temperature(&self, omit: bool) {
        self.base.set_omit_temperature(omit);
    }

    fn set_reasoning_pref(&self, pref: Option<crate::providers::reasoning::ReasoningPreference>) {
        self.base.set_reasoning_pref(pref);
    }

    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        json_schema: Option<Value>,
    ) -> VivianResult<String> {
        let prompt_key = messages_cache_key(&messages);
        let mut body = json!({
            "model": self.base.model,
            "messages": ChatCompletionsProvider::build_messages(&messages, &self.instructions),
            "temperature": zhipu_temperature(self.base.effective_temperature()),
            "max_tokens": zhipu_max_tokens(&self.base.model, self.base.effective_max_tokens()),
        });
        Self::inject_response_format(&mut body, &json_schema);
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body, false);
        self.inject_web_search(&mut body, enable_search);
        self.call_with_retry(body, Some(&prompt_key)).await
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        json_schema: Option<Value>,
    ) -> VivianResult<mpsc::Receiver<String>> {
        crate::persona::prompt_render::check_messages_for_leaks(
            &messages,
            &format!("call_stream_chat model={}", self.base.model),
        );
        self.base.check_circuit()?;

        let mut body = json!({
            "model": self.base.model,
            "messages": ChatCompletionsProvider::build_messages(&messages, &self.instructions),
            "temperature": zhipu_temperature(self.base.effective_temperature()),
            "max_tokens": zhipu_max_tokens(&self.base.model, self.base.effective_max_tokens()),
            "stream": true,
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body, false);
        Self::inject_response_format(&mut body, &json_schema);

        let client = self.base.get_client();
        let req = self
            .apply_auth(client.post(&self.endpoint()))
            .header("Content-Type", "application/json")
            .json(&body);
        let response = req.send().await?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "智谱 GLM 流式请求失败 ({}): {}",
                status, text
            )));
        }

        self.base.record_success();

        let (tx, rx) = mpsc::channel::<String>(32);
        let leaks = leaks_thinking_in_content(&self.base.model);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut stripper = if leaks {
                Some(ThinkingStreamStripper::new())
            } else {
                None
            };

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("[zhipu] 流读取失败: {}", e);
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim_end_matches('\r').to_string();
                    buffer = buffer[pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if data == "[DONE]" {
                            if let Some(s) = stripper.as_mut() {
                                let residual = s.flush();
                                if !residual.is_empty() {
                                    let _ = tx.send(residual).await;
                                }
                            }
                            return;
                        }
                        if let Ok(json_val) = serde_json::from_str::<Value>(data) {
                            if let Some(choices) = json_val["choices"].as_array() {
                                if let Some(first) = choices.first() {
                                    let delta = &first["delta"];
                                    if let Some(content) = delta["content"].as_str() {
                                        if !content.is_empty() {
                                            let out = if let Some(s) = stripper.as_mut() {
                                                s.feed(content)
                                            } else {
                                                content.to_string()
                                            };
                                            if !out.is_empty() {
                                                if tx.send(out).await.is_err() {
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(s) = stripper.as_mut() {
                let residual = s.flush();
                if !residual.is_empty() {
                    let _ = tx.send(residual).await;
                }
            }
        });

        Ok(rx)
    }

    fn get_model(&self) -> &str {
        &self.base.model
    }

    fn get_circuit_breaker_stats(&self) -> Value {
        let stats = self.base.get_stats();
        json!({
            "model": stats.model,
            "total_calls": stats.total_calls,
            "successful_calls": stats.successful_calls,
            "failed_calls": stats.failed_calls,
        })
    }

    fn supports_native_function_calling(&self) -> bool {
        true
    }

    fn supports_structured_output(&self) -> bool {
        false
    }

    fn supports_json_mode(&self) -> bool {
        true
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> VivianResult<Box<dyn BaseProvider>> {
        Ok(Box::new(ZhipuProvider {
            base: ProviderBase {
                api_key: self.base.api_key.clone(),
                base_url: self.base.base_url.clone(),
                model: self.base.model.clone(),
                temperature: zhipu_temperature(self.base.effective_temperature()),
                max_tokens: self.base.max_tokens,
                circuit_breaker: Arc::clone(&self.base.circuit_breaker),
                request_cache: Mutex::new(HashMap::new()),
                enable_search: AtomicBool::new(self.base.is_enable_search()),
                proxy: self.base.proxy.clone(),
                client: self.base.client.clone(),
                max_tokens_override: std::sync::atomic::AtomicU32::new(0),
                temperature_override: std::sync::atomic::AtomicU64::new(0),
                omit_temperature: std::sync::atomic::AtomicBool::new(false),
                reasoning_pref: parking_lot::RwLock::new(*self.base.reasoning_pref.read()),
            },
            tools,
            instructions: self.instructions.clone(),
        }))
    }

    async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> {
        if self.tools.is_empty() {
            let content = self.call_chat(messages).await?;
            return Ok(ChatResponse::from_text(content));
        }

        let prompt_key = messages_cache_key(&messages);
        let mut body = json!({
            "model": self.base.model,
            "messages": ChatCompletionsProvider::build_messages(&messages, &self.instructions),
            "temperature": zhipu_temperature(self.base.effective_temperature()),
            "max_tokens": zhipu_max_tokens(&self.base.model, self.base.effective_max_tokens()),
            "tools": self.build_tools_field().unwrap_or(Value::Array(vec![])),
            "tool_choice": "auto",
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body, true);

        self.invoke_with_retry(body, Some(&prompt_key)).await
    }

    /// 流式 + 原生 function calling（Chat Completions SSE 格式）
    async fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        crate::persona::prompt_render::check_messages_for_leaks(
            &messages,
            &format!("stream_with_tools model={}", self.base.model),
        );
        self.base.check_circuit()?;

        let tools_field: Value = if tools.is_empty() {
            Value::Array(vec![])
        } else {
            let arr: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            Value::Array(arr)
        };

        let mut body = json!({
            "model": self.base.model,
            "messages": ChatCompletionsProvider::build_messages(&messages, &self.instructions),
            "temperature": zhipu_temperature(self.base.effective_temperature()),
            "max_tokens": zhipu_max_tokens(&self.base.model, self.base.effective_max_tokens()),
            "stream": true,
            "tools": tools_field,
            "tool_choice": "auto",
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body, !tools.is_empty());

        let client = self.base.get_client();
        let req = self
            .apply_auth(client.post(&self.endpoint()))
            .header("Content-Type", "application/json")
            .json(&body);
        let response = req.send().await?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "智谱 GLM 流式请求失败 ({}): {}",
                status, text
            )));
        }

        self.base.record_success();

        let (tx, rx) = mpsc::channel::<StreamEvent>(64);
        let leaks = leaks_thinking_in_content(&self.base.model);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut finish_reason: Option<String> = None;
            let mut stripper = if leaks {
                Some(ThinkingStreamStripper::new())
            } else {
                None
            };
            let mut tool_calls: HashMap<usize, (Option<String>, Option<String>, String)> =
                HashMap::new();

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx
                            .send(StreamEvent::Error {
                                message: format!("流读取失败: {}", e),
                            })
                            .await;
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim_end_matches('\r').to_string();
                    buffer = buffer[pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if data == "[DONE]" {
                            if let Some(s) = stripper.as_mut() {
                                let residual = s.flush();
                                if !residual.is_empty() {
                                    if tx
                                        .send(StreamEvent::Text { content: residual })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            let _ = tx
                                .send(StreamEvent::Done {
                                    finish_reason: finish_reason.take(),
                                })
                                .await;
                            return;
                        }

                        if let Ok(json_val) = serde_json::from_str::<Value>(data) {
                            if let Some(choices) = json_val["choices"].as_array() {
                                if let Some(first) = choices.first() {
                                    if let Some(fr) = first["finish_reason"].as_str() {
                                        finish_reason = Some(fr.to_string());
                                    }

                                    let delta = &first["delta"];

                                    if let Some(content) = delta["content"].as_str() {
                                        if !content.is_empty() {
                                            let out = if let Some(s) = stripper.as_mut() {
                                                s.feed(content)
                                            } else {
                                                content.to_string()
                                            };
                                            if !out.is_empty() {
                                                if tx
                                                    .send(StreamEvent::Text { content: out })
                                                    .await
                                                    .is_err()
                                                {
                                                    return;
                                                }
                                            }
                                        }
                                    }

                                    if let Some(reasoning) =
                                        delta["reasoning_content"].as_str()
                                    {
                                        if !reasoning.is_empty() {
                                            if tx
                                                .send(StreamEvent::Thinking {
                                                    content: reasoning.to_string(),
                                                })
                                                .await
                                                .is_err()
                                            {
                                                return;
                                            }
                                        }
                                    }

                                    if let Some(tcs) = delta["tool_calls"].as_array() {
                                        for tc in tcs {
                                            let index =
                                                tc["index"].as_u64().unwrap_or(0) as usize;
                                            let entry = tool_calls
                                                .entry(index)
                                                .or_insert((None, None, String::new()));

                                            if let Some(id) = tc["id"].as_str() {
                                                entry.0 = Some(id.to_string());
                                            }
                                            if let Some(name) =
                                                tc["function"]["name"].as_str()
                                            {
                                                if !name.is_empty() {
                                                    entry.1 = Some(name.to_string());
                                                }
                                            }
                                            if let Some(args_delta) =
                                                tc["function"]["arguments"].as_str()
                                            {
                                                entry.2.push_str(args_delta);
                                            }

                                            if tx
                                                .send(StreamEvent::ToolCallDelta {
                                                    index,
                                                    id: entry.0.clone(),
                                                    name: entry.1.clone(),
                                                    arguments_delta: tc["function"]
                                                        ["arguments"]
                                                        .as_str()
                                                        .map(String::from),
                                                })
                                                .await
                                                .is_err()
                                            {
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(s) = stripper.as_mut() {
                let residual = s.flush();
                if !residual.is_empty() {
                    let _ = tx.send(StreamEvent::Text { content: residual }).await;
                }
            }
            let _ = tx
                .send(StreamEvent::Done {
                    finish_reason: finish_reason.take(),
                })
                .await;
        });

        Ok(rx)
    }
}
