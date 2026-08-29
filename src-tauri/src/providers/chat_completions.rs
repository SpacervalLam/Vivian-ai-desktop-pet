//! 标准 OpenAI Chat Completions Provider（`/v1/chat/completions` 端点）
//!
//! 面向仅支持 Chat Completions 协议的服务商：
//! OpenRouter / Groq / Mistral / Together / Ollama / vLLM / LM Studio 等。
//!
//! 与 `openai_compat`（Responses API `/responses`）的关键差异：
//! - 请求体：`messages` 数组（非 `input`），`max_tokens`（非 `max_output_tokens`）
//! - system prompt：`role: "system"` 消息（非顶层 `instructions`）
//! - tools schema：嵌套格式 `{"type":"function","function":{name,description,parameters}}`
//! - 响应：`choices[0].message.content`（非 `output[].content[].text`）
//! - 流式：`choices[0].delta.content`（非 `response.output_text.delta` 事件）
//! - 流式工具调用：`choices[0].delta.tool_calls[]` 按 index 累积

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
    parse_stream_usage, BaseProvider, ChatResponse, ProviderBase, StreamEvent, StructuredToolCall,
    ToolDefinition,
};
use crate::providers::thinking_stripper::{
    leaks_thinking_in_content, strip_thinking_segments, ThinkingStreamStripper,
};
use crate::resilience::{classify_error, ErrorCategory};
use crate::types::response::ChatMessage;
use crate::utils::messages_cache_key;

const MAX_RETRIES: usize = 2;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const LARGE_PROMPT_BYTES: usize = 20_000;
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// 标准 Chat Completions 协议 Provider
pub struct ChatCompletionsProvider {
    base: ProviderBase,
    tools: Vec<ToolDefinition>,
    instructions: Option<String>,
}

impl ChatCompletionsProvider {
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

    /// 构造请求头：api_key 为空时不发送 Authorization（支持 Ollama 等无鉴权本地服务）
    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.base.api_key.is_empty() {
            builder
        } else {
            builder.bearer_auth(&self.base.api_key)
        }
    }

    /// 把内部 ChatMessage 转为 Chat Completions 的 messages 数组
    ///
    /// - system：`{"role":"system","content":"..."}`
    /// - user + images：content 转数组 `[{"type":"text",...},{"type":"image_url",...}]`
    /// - assistant + tool_calls：追加 `tool_calls` 字段
    /// - tool：`{"role":"tool","tool_call_id":"...","content":"..."}`
    pub(crate) fn build_messages(messages: &[ChatMessage], instructions: &Option<String>) -> Vec<Value> {
        let mut result: Vec<Value> = Vec::new();

        // instructions 作为首条 system 消息注入
        if let Some(instr) = instructions {
            if !instr.is_empty() {
                result.push(json!({"role": "system", "content": instr}));
            }
        }

        for m in messages {
            match m.role.as_str() {
                "system" => {
                    result.push(json!({"role": "system", "content": m.content}));
                }
                "assistant" => {
                    let mut msg = json!({"role": "assistant", "content": m.content});
                    if let Some(tcs) = &m.tool_calls {
                        let tc_arr: Vec<Value> = tcs
                            .iter()
                            .map(|tc| {
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": serde_json::to_string(&tc.arguments)
                                            .unwrap_or_else(|_| "{}".to_string()),
                                    }
                                })
                            })
                            .collect();
                        msg["tool_calls"] = Value::Array(tc_arr);
                    }
                    // 回传 reasoning_content（DeepSeek / Qwen 风格）
                    if let Some(reasoning) = &m.reasoning {
                        if !reasoning.is_empty() {
                            msg["reasoning_content"] = json!(reasoning);
                        }
                    }
                    result.push(msg);
                }
                "tool" => {
                    result.push(json!({
                        "role": "tool",
                        "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                        "content": m.content,
                    }));
                }
                _ => {
                    // user 或其他角色
                    if let Some(imgs) = &m.images {
                        if imgs.is_empty() {
                            result.push(json!({"role": m.role, "content": m.content}));
                        } else {
                            let mut content_arr: Vec<Value> =
                                vec![json!({"type": "text", "text": m.content})];
                            for img in imgs {
                                let image_url = if !img.data.is_empty() {
                                    format!("data:{};base64,{}", img.media_type, img.data)
                                } else if let Some(u) = &img.url {
                                    u.clone()
                                } else {
                                    continue;
                                };
                                let mut part = json!({
                                    "type": "image_url",
                                    "image_url": {"url": image_url},
                                });
                                if let Some(detail) = &img.detail {
                                    part["image_url"]["detail"] = json!(detail);
                                }
                                content_arr.push(part);
                            }
                            result.push(json!({"role": m.role, "content": content_arr}));
                        }
                    } else {
                        result.push(json!({"role": m.role, "content": m.content}));
                    }
                }
            }
        }
        result
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

    /// 按当前推理覆盖注入思考控制字段（思考型模型按能力映射 thinking /
    /// reasoning_effort / enable_thinking；非思考模型不注入）。
    fn apply_reasoning_fields(&self, body: &mut Value, has_tools: bool) {
        let pref = self.base.effective_reasoning();
        let cap = crate::providers::reasoning::resolve_reasoning_capability(&self.base.model);
        crate::providers::reasoning::apply_reasoning_preference(body, pref, &cap, has_tools);
    }

    /// 注入 response_format（按能力分级：json_schema / json_object / 不注入）
    fn inject_response_format(&self, body: &mut Value, json_schema: &Option<Value>) {
        if json_schema.is_none() {
            return;
        }
        if self.supports_structured_output() {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "vivian_response",
                    "schema": json_schema,
                    "strict": true
                }
            });
        } else if self.supports_json_mode() {
            body["response_format"] = json!({"type": "json_object"});
        }
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
                    "[chat_completions] {} 网络失败 phase={} elapsed={:.2}s err={}",
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
                "Chat Completions 请求失败 ({}): {}",
                status, text
            )));
        }

        let json_val: Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "[chat_completions] {} 响应解码失败 elapsed={:.2}s err={}",
                    self.base.model,
                    started.elapsed().as_secs_f64(),
                    e
                );
                return Err(VivianError::Network(format!("响应解码失败: {e}")));
            }
        };
        Ok(json_val)
    }

    /// 从 Chat Completions 响应中提取文本内容
    pub(crate) fn extract_content(json_val: &Value) -> VivianResult<String> {
        if let Some(choices) = json_val["choices"].as_array() {
            if let Some(first) = choices.first() {
                let content = first["message"]["content"].as_str().unwrap_or("");
                return Ok(strip_thinking_segments(content));
            }
        }
        Err(VivianError::Provider(
            "响应中缺少 choices[0].message.content".to_string(),
        ))
    }

    /// 从 Chat Completions 响应中提取完整结构化结果（含 tool_calls）
    pub(crate) fn extract_chat_response(json_val: &Value) -> VivianResult<ChatResponse> {
        let choices = json_val["choices"]
            .as_array()
            .ok_or_else(|| VivianError::Provider("响应缺少 choices 数组".to_string()))?;

        let first = choices
            .first()
            .ok_or_else(|| VivianError::Provider("choices 数组为空".to_string()))?;

        let message = &first["message"];
        let content = strip_thinking_segments(message["content"].as_str().unwrap_or(""));
        let finish_reason = first["finish_reason"].as_str().map(String::from);

        // 提取 reasoning_content（DeepSeek / Qwen / GLM 风格）
        let reasoning = message["reasoning_content"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from);

        // 提取 tool_calls
        let mut tool_calls: Vec<StructuredToolCall> = Vec::new();
        if let Some(tcs) = message["tool_calls"].as_array() {
            for tc in tcs {
                let id = tc["id"].as_str().unwrap_or("").to_string();
                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                let arguments: Value =
                    serde_json::from_str(args_str).unwrap_or(Value::Object(serde_json::Map::new()));
                if !name.is_empty() {
                    tool_calls.push(StructuredToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
            }
        }

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning,
            raw: json_val.clone(),
        })
    }

    async fn call_with_retry(&self, body: Value, cache_key_prompt: Option<&str>) -> VivianResult<String> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("[chat_completions] 命中缓存: {}", self.base.model);
                return Ok(cached);
            }
        }

        let body_size = body.to_string().len();
        let bypass_circuit_failure = body_size > LARGE_PROMPT_BYTES;

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!(
                    "[chat_completions] 第 {} 次重试: {}",
                    attempt,
                    self.base.model
                );
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(body.clone()).await {
                Ok(json_val) => {
                    let content = Self::extract_content(&json_val)?;
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

    async fn invoke_with_retry(&self, body: Value, cache_key_prompt: Option<&str>) -> VivianResult<ChatResponse> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("[chat_completions] 命中缓存(structured): {}", self.base.model);
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
                    "[chat_completions] 第 {} 次重试(structured): {}",
                    attempt,
                    self.base.model
                );
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(body.clone()).await {
                Ok(json_val) => {
                    let resp = Self::extract_chat_response(&json_val)?;
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
impl BaseProvider for ChatCompletionsProvider {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        crate::persona::prompt_render::check_messages_for_leaks(
            &messages,
            &format!("call_chat model={}", self.base.model),
        );
        let prompt_key = messages_cache_key(&messages);
        let mut body = json!({
            "model": self.base.model,
            "messages": Self::build_messages(&messages, &self.instructions),
            "temperature": self.base.effective_temperature(),
            "max_tokens": self.base.effective_max_tokens(),
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
        _enable_search: bool,
        json_schema: Option<Value>,
    ) -> VivianResult<String> {
        // Chat Completions 协议无标准联网搜索字段，忽略 enable_search
        let prompt_key = messages_cache_key(&messages);
        let mut body = json!({
            "model": self.base.model,
            "messages": Self::build_messages(&messages, &self.instructions),
            "temperature": self.base.effective_temperature(),
            "max_tokens": self.base.effective_max_tokens(),
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body, false);
        self.inject_response_format(&mut body, &json_schema);
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
            "messages": Self::build_messages(&messages, &self.instructions),
            "temperature": self.base.effective_temperature(),
            "max_tokens": self.base.effective_max_tokens(),
            "stream": true,
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body, false);
        self.inject_response_format(&mut body, &json_schema);

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
                "Chat Completions 流式请求失败 ({}): {}",
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
                        tracing::error!("[chat_completions] 流读取失败: {}", e);
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

    fn bind_tools(
        &self,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<Box<dyn BaseProvider>> {
        Ok(Box::new(ChatCompletionsProvider {
            base: ProviderBase {
                api_key: self.base.api_key.clone(),
                base_url: self.base.base_url.clone(),
                model: self.base.model.clone(),
                temperature: self.base.effective_temperature(),
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
            "messages": Self::build_messages(&messages, &self.instructions),
            "temperature": self.base.effective_temperature(),
            "max_tokens": self.base.effective_max_tokens(),
            "tools": self.build_tools_field().unwrap_or(Value::Array(vec![])),
            "tool_choice": "auto",
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body, true);

        self.invoke_with_retry(body, Some(&prompt_key)).await
    }

    /// 流式 + 原生 function calling（Chat Completions SSE 格式）
    ///
    /// 事件格式：
    /// - `choices[0].delta.content` —— 文本增量
    /// - `choices[0].delta.reasoning_content` —— 推理增量（DeepSeek 风格）
    /// - `choices[0].delta.tool_calls[]` —— 工具调用增量，按 index 累积
    /// - `choices[0].finish_reason` —— 流结束标记
    /// - `data: [DONE]` —— SSE 终止符
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
            "messages": Self::build_messages(&messages, &self.instructions),
            "temperature": self.base.effective_temperature(),
            "max_tokens": self.base.effective_max_tokens(),
            "stream": true,
            "stream_options": { "include_usage": true },
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
                "Chat Completions 流式请求失败 ({}): {}",
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
            // 按 index 累积工具调用：(id, name, arguments)
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
                            // usage chunk：stream_options.include_usage 下末尾 chunk 携带（choices 为空数组）
                            if let Some(ev) = parse_stream_usage(&json_val["usage"]) {
                                let _ = tx.send(ev).await;
                            }
                            if let Some(choices) = json_val["choices"].as_array() {
                                if let Some(first) = choices.first() {
                                    // finish_reason
                                    if let Some(fr) = first["finish_reason"].as_str() {
                                        finish_reason = Some(fr.to_string());
                                    }

                                    let delta = &first["delta"];

                                    // 文本增量
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

                                    // 推理增量（DeepSeek / Qwen 风格）
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

                                    // 工具调用增量
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

            // 流自然结束（未收到 [DONE]）
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
