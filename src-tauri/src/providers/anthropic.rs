use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::error::{VivianError, VivianResult};
use crate::providers::base::{
    BaseProvider, ChatResponse, ProviderBase, StreamEvent, StructuredToolCall, ToolDefinition,
};
use crate::providers::openai_compat::CacheStrategy;
use crate::resilience::{classify_error, ErrorCategory};
use crate::types::response::ChatMessage;
use crate::utils::messages_cache_key;

const ANTHROPIC_API_VERSION: &str = "2023-06-01";
const MAX_RETRIES: usize = 2;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// Anthropic Claude 原生 Messages API Provider
///
/// 与 OpenAI 兼容接口的差异：
/// - 鉴权头：`x-api-key`（非 Bearer）
/// - 必须带 `anthropic-version` 头（如 2023-06-01）
/// - 端点：`{base_url}/v1/messages`（base_url 默认 https://api.anthropic.com）
/// - 请求体：`system` 单独字段；`messages` 中 user/assistant 交替；`max_tokens` 必填
/// - 响应：`content` 为数组，每个元素 `{type, text}`；`stop_reason` 而非 `finish_reason`
pub struct AnthropicProvider {
    base: ProviderBase,
    /// 已绑定的工具列表（原生 function calling 路径使用）
    ///
    /// `bind_tools` 返回的新实例会填充此字段；后续 `invoke` 调用会把
    /// 它注入请求体的 `tools` 字段（Anthropic 格式：`input_schema`），
    /// 并解析响应 content 数组中的 `tool_use` 块。
    tools: Vec<ToolDefinition>,
    /// 提示缓存策略
    cache_strategy: CacheStrategy,
    /// 模型级别预设（通过 system 字段传递）
    ///
    /// 适用于 Claude 的 `system` 参数，将框架规则一次性设置，
    /// 不在每次请求中重复传输，减少 token 开销。
    instructions: Option<String>,
}

impl AnthropicProvider {
    pub fn new(
        config: &crate::config::manager::ProviderConfig,
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
            cache_strategy: CacheStrategy::Auto,
            instructions: None,
        }
    }

    pub fn with_cache_strategy(mut self, strategy: CacheStrategy) -> Self {
        self.cache_strategy = strategy;
        self
    }

    pub fn with_instructions(mut self, instructions: Option<String>) -> Self {
        self.instructions = instructions;
        self
    }

    fn endpoint(&self) -> String {
        let url = self.base.base_url.trim_end_matches('/');
        // 用户填的 base_url 可能已含 /v1，也可能不含；统一规范化为 {base}/v1/messages
        if url.ends_with("/v1") {
            format!("{}/messages", url)
        } else if url.ends_with("/messages") {
            url.to_string()
        } else {
            format!("{}/v1/messages", url)
        }
    }

    /// 把 OpenAI 风格 messages 转换为 Anthropic 风格
    ///
    /// Anthropic 要求：
    /// - system 消息单独字段，不放在 messages 数组里
    /// - messages 中 role 必须是 user / assistant 交替
    /// - 数组首条必须是 user
    ///
    /// 原生 function calling 路径的额外处理：
    /// - `assistant` + `tool_calls`：content 转为数组，含 `text` 块（若 content 非空）
    ///   与若干 `tool_use` 块（`id` / `name` / `input`）
    /// - `tool` 角色：转换为 `role="user"`，content 数组含一个 `tool_result` 块
    ///   （`tool_use_id` 关联前序 assistant 的 tool_use.id，`content` 为工具返回文本）
    /// 把内部 ChatMessage 转换为 Anthropic 风格
    ///
    /// 多模态：user 消息携带 `images` 时，content 转为数组形式，
    /// 在文本块之后追加 `image` 块（base64 source 或 url source）。
    ///
    /// 推理回传：assistant 消息携带 `reasoning` 时，作为 `thinking` 块原样回传，
    /// 保证多轮工具调用上下文中 extended thinking 连续。
    fn convert_messages(messages: &[ChatMessage]) -> (Option<Value>, Vec<Value>) {
        let mut system_text: Option<String> = None;
        let mut converted: Vec<Value> = Vec::with_capacity(messages.len());

        for m in messages {
            match m.role.as_str() {
                "system" => {
                    // 多条 system 拼接（Anthropic 只接受单一 system 字段）
                    system_text = Some(match system_text {
                        Some(s) => format!("{}\n\n{}", s, m.content),
                        None => m.content.clone(),
                    });
                }
                "assistant" => {
                    if let Some(tc) = &m.tool_calls {
                        // assistant + tool_calls → content 数组（text 块 + tool_use 块）
                        let mut content_arr: Vec<Value> = Vec::new();
                        if let Some(r) = &m.reasoning {
                            if !r.is_empty() {
                                content_arr.push(json!({
                                    "type": "thinking",
                                    "thinking": r,
                                }));
                            }
                        }
                        if !m.content.is_empty() {
                            content_arr.push(json!({"type": "text", "text": m.content}));
                        }
                        for c in tc {
                            content_arr.push(json!({
                                "type": "tool_use",
                                "id": c.id,
                                "name": c.name,
                                "input": c.arguments,
                            }));
                        }
                        converted.push(json!({"role": "assistant", "content": content_arr}));
                    } else {
                        let mut content_arr: Vec<Value> = Vec::new();
                        if let Some(r) = &m.reasoning {
                            if !r.is_empty() {
                                content_arr.push(json!({
                                    "type": "thinking",
                                    "thinking": r,
                                }));
                            }
                        }
                        if content_arr.is_empty() {
                            converted.push(json!({"role": "assistant", "content": m.content}));
                        } else {
                            if !m.content.is_empty() {
                                content_arr.push(json!({"type": "text", "text": m.content}));
                            }
                            converted.push(json!({"role": "assistant", "content": content_arr}));
                        }
                    }
                }
                "tool" => {
                    // tool 结果 → role="user"，content 数组含 tool_result 块
                    // Anthropic 不支持 role="tool"，工具结果必须放在 user 消息下
                    let tool_use_id = m.tool_call_id.clone().unwrap_or_default();
                    converted.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": m.content,
                        }]
                    }));
                }
                "user" => {
                    if let Some(imgs) = &m.images {
                        if !imgs.is_empty() {
                            let mut content_arr: Vec<Value> =
                                vec![json!({"type": "text", "text": m.content})];
                            for img in imgs {
                                if !img.data.is_empty() {
                                    content_arr.push(json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": img.media_type,
                                            "data": img.data,
                                        }
                                    }));
                                } else if let Some(u) = &img.url {
                                    content_arr.push(json!({
                                        "type": "image",
                                        "source": {"type": "url", "url": u}
                                    }));
                                }
                            }
                            converted.push(json!({"role": "user", "content": content_arr}));
                            continue;
                        }
                    }
                    converted.push(json!({"role": "user", "content": m.content}));
                }
                // function 等统一作为 user
                _ => converted.push(json!({"role": "user", "content": m.content})),
            }
        }

        let system_value = system_text.map(|s| {
            // cache_control 策略时给 system 块打 ephemeral 标记
            // system 作为数组形式以携带 cache_control 字段
            json!([{
                "type": "text",
                "text": s,
                "cache_control": {"type": "ephemeral"},
            }])
        });
        (system_value, converted)
    }

    fn build_body(&self, messages: &[ChatMessage], stream: bool) -> Value {
        let (system, converted) = Self::convert_messages(messages);
        let mut body = json!({
            "model": self.base.model,
            "max_tokens": self.base.max_tokens,
            "temperature": self.base.effective_temperature(),
            "messages": converted,
            "stream": stream,
        });

        let mut system_parts: Vec<String> = Vec::new();
        
        // 模型级别预设：instructions（框架规则一次性设置）
        if let Some(instructions) = &self.instructions {
            system_parts.push(instructions.clone());
        }
        
        // messages 中的 system 内容
        if let Some(s) = system {
            if let Some(arr) = s.as_array() {
                if let Some(first) = arr.first() {
                    if let Some(text) = first.get("text").and_then(|t| t.as_str()) {
                        system_parts.push(text.to_string());
                    }
                }
            }
        }
        
        if !system_parts.is_empty() {
            let system_text = system_parts.join("\n\n");
            // cache_control 策略：Auto / CacheControl 时给 system 块打 ephemeral 标记
            let use_cache_control = matches!(
                self.cache_strategy,
                CacheStrategy::Auto | CacheStrategy::CacheControl
            );
            if use_cache_control {
                body["system"] = json!([{
                    "type": "text",
                    "text": system_text,
                    "cache_control": {"type": "ephemeral"},
                }]);
            } else {
                body["system"] = json!(system_text);
            }
        }
        body
    }

    async fn send_request(&self, body: &Value) -> VivianResult<Value> {
        let client = self.base.get_client();
        let response = client
            .post(&self.endpoint())
            .header("x-api-key", &self.base.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Anthropic API 请求失败 ({}): {}",
                status, text
            )));
        }
        let json: Value = response.json().await?;
        Ok(json)
    }

    fn extract_content(json: &Value) -> VivianResult<String> {
        // content 是数组：[{"type": "text", "text": "..."}, ...]
        let content = json
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| VivianError::Provider("Anthropic 响应缺少 content 数组".to_string()))?;

        let mut text = String::new();
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(s) = block.get("text").and_then(|t| t.as_str()) {
                    text.push_str(s);
                }
            }
        }
        if text.is_empty() {
            return Err(VivianError::Provider(
                "Anthropic 响应 content 中无文本块".to_string(),
            ));
        }
        Ok(text)
    }

    /// 构造 Anthropic function calling 的 tools 字段
    ///
    /// Anthropic 格式与 OpenAI 不同：使用 `input_schema`（而非 `parameters`），
    /// 且没有 `type: "function"` 包装层。输出形如：
    /// ```json
    /// [{"name": "...", "description": "...", "input_schema": {...}}]
    /// ```
    fn build_tools_field(&self, json_schema: &Option<serde_json::Value>) -> Option<Value> {
        // Structured Outputs: schema 被传入时追加 emit_response 伪工具
        // Claude 没有 response_format 通道，结构化字段必须通过 tool_use 返回
        let has_schema = json_schema.is_some();

        if self.tools.is_empty() && !has_schema {
            return None;
        }

        let mut arr: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect();

        if has_schema {
            arr.push(crate::providers::schema::emit_response_tool_definition());
        }

        Some(Value::Array(arr))
    }

    /// 从 Anthropic 响应中提取完整结构化结果（content 文本 + tool_calls + stop_reason）
    ///
    /// Anthropic 响应的 `content` 是数组，可混合以下块类型：
    /// - `{"type": "text", "text": "..."}`：拼接为最终文本
    /// - `{"type": "tool_use", "id": "toolu_xxx", "name": "...", "input": {...}}`：
    ///   转为 `StructuredToolCall`（id 取自 tool_use.id）
    ///
    /// 与 `extract_content` 不同：此处允许文本为空（仅 tool_use 的响应合法）。
    /// 结束原因字段为 `stop_reason`（Anthropic 命名，区别于 OpenAI 的 finish_reason），
    /// 工具调用场景下值为 `"tool_calls"`。
    fn extract_chat_response(&self, json: &Value) -> VivianResult<ChatResponse> {
        let content_arr = json
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| VivianError::Provider("Anthropic 响应缺少 content 数组".to_string()))?;

        let mut text = String::new();
        let mut tool_calls: Vec<StructuredToolCall> = Vec::new();
        let mut reasoning: Option<String> = None;

        for block in content_arr {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(s) = block.get("text").and_then(|t| t.as_str()) {
                        text.push_str(s);
                    }
                }
                Some("thinking") => {
                    if let Some(s) = block.get("thinking").and_then(|t| t.as_str()) {
                        reasoning = Some(match reasoning {
                            Some(r) => format!("{}{}", r, s),
                            None => s.to_string(),
                        });
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                    if !name.is_empty() {
                        // emit_response 伪工具的 input 是结构化字段 JSON
                        // 把它序列化后塞到 content 顶部，让 JsonProcessor 像解析
                        // 普通 LLM 输出那样解析（保持下游处理路径统一）
                        if crate::providers::schema::is_emit_response_call(&name) {
                            let json_str = serde_json::to_string(&input)
                                .unwrap_or_else(|_| "{}".to_string());
                            // emit_response 内容优先级低于 text 块，仅在 text 为空时填充
                            if text.is_empty() {
                                text = json_str;
                            }
                        } else {
                            tool_calls.push(StructuredToolCall {
                                id,
                                name,
                                arguments: input,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        let finish_reason = json
            .get("stop_reason")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());

        Ok(ChatResponse {
            content: text,
            tool_calls,
            finish_reason,
            reasoning,
            raw: json.clone(),

        })
    }

    /// 结构化调用（带工具）的请求执行 —— 与 `call_with_retry` 类似，但返回 `ChatResponse`
    ///
    /// 仅缓存无工具调用的响应（带工具调用的响应需重新触发执行，不应缓存）。
    async fn invoke_with_retry(
        &self,
        body: Value,
        cache_key_prompt: Option<&str>,
    ) -> VivianResult<ChatResponse> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("命中缓存(structured): {}", self.base.model);
                return Ok(ChatResponse::from_text(cached));
            }
        }

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!("第 {} 次重试 Anthropic 请求: {}", attempt, self.base.model);
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(&body).await {
                Ok(json) => {
                    let resp = self.extract_chat_response(&json)?;
                    self.base.record_success();
                    if !resp.has_tool_calls() {
                        if let Some(prompt) = cache_key_prompt {
                            self.base.cache_response(prompt, &resp.content);
                        }
                    }
                    return Ok(resp);
                }
                Err(err) => {
                    self.base.record_failure();
                    let category = classify_error(&err);
                    match category {
                        ErrorCategory::Permanent => return Err(err),
                        ErrorCategory::Transient | ErrorCategory::RateLimit => {
                            last_error = Some(err);
                            continue;
                        }
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| VivianError::Provider("Anthropic 重试次数耗尽".to_string())))
    }

    async fn call_with_retry(
        &self,
        body: Value,
        cache_key_prompt: Option<&str>,
    ) -> VivianResult<String> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("命中缓存: {}", self.base.model);
                return Ok(cached);
            }
        }

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!("第 {} 次重试 Anthropic 请求: {}", attempt, self.base.model);
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(&body).await {
                Ok(json) => {
                    let content = Self::extract_content(&json)?;
                    self.base.record_success();
                    if let Some(prompt) = cache_key_prompt {
                        self.base.cache_response(prompt, &content);
                    }
                    return Ok(content);
                }
                Err(err) => {
                    self.base.record_failure();
                    let category = classify_error(&err);
                    match category {
                        ErrorCategory::Permanent => return Err(err),
                        ErrorCategory::Transient | ErrorCategory::RateLimit => {
                            last_error = Some(err);
                            continue;
                        }
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| VivianError::Provider("Anthropic 重试次数耗尽".to_string())))
    }
}

#[async_trait]
impl BaseProvider for AnthropicProvider {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        let prompt_key = messages_cache_key(&messages);
        let body = self.build_body(&messages, false);
        self.call_with_retry(body, Some(&prompt_key)).await
    }

    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<String> {
        // Anthropic 自 2024-12 起支持 server-side web_search_tool。
        // 当 enable_search=true 时注入到 tools 字段，让 Claude 自主决定是否搜索。
        // 注意：若已通过 bind_tools 绑定 function calling 工具，两者可共存（Anthropic
        // 允许 tools 数组混合 web_search 与 function 类型），但需确保 max_tokens 足够。
        let need_search = enable_search || self.base.is_enable_search();
        if need_search || json_schema.is_some() {
            let mut body = self.build_body(&messages, false);
            // Structured Outputs: 注入 emit_response 伪工具
            if let Some(tools_field) = self.build_tools_field(&json_schema) {
                body["tools"] = tools_field;
            }
            // 追加 web_search 工具到 tools 数组
            if need_search {
                let web_search_tool = json!({
                    "type": "web_search_20250305",
                    "name": "web_search",
                    "max_uses": 3
                });
                if let Some(arr) = body.get_mut("tools").and_then(|t| t.as_array_mut()) {
                    arr.push(web_search_tool);
                } else {
                    body["tools"] = json!([web_search_tool]);
                }
                tracing::info!(
                    "[Router] Anthropic 启用 web_search_tool: model={}",
                    self.base.model
                );
            }
            let json = self.send_request(&body).await?;
            return Self::extract_content(&json);
        }
        self.call_chat(messages).await
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<mpsc::Receiver<String>> {
        self.base.check_circuit()?;

        let mut body = self.build_body(&messages, true);
        // Structured Outputs: 注入 emit_response 伪工具
        if let Some(tools_field) = self.build_tools_field(&json_schema) {
            body["tools"] = tools_field;
        }
        let client = self.base.get_client();
        let response = client
            .post(&self.endpoint())
            .header("x-api-key", &self.base.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Anthropic 流式 API 请求失败 ({}): {}",
                status, text
            )));
        }

        self.base.record_success();

        let (tx, rx) = mpsc::channel::<String>(32);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();

            // Anthropic SSE 事件类型：
            //   event: message_start / content_block_start / content_block_delta /
            //          content_block_stop / message_delta / message_stop
            // 我们只关心 content_block_delta 中的 text_delta
            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Anthropic 流读取失败: {}", e);
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // 按双换行分割 SSE 事件块
                while let Some(pos) = buffer.find("\n\n") {
                    let event_block = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    // 提取 data: 行
                    for line in event_block.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(json) = serde_json::from_str::<Value>(data) {
                                // content_block_delta: {"type":"content_block_delta","delta":{"type":"text_delta","text":"..."}}
                                if json.get("type").and_then(|t| t.as_str())
                                    == Some("content_block_delta")
                                {
                                    if let Some(text) = json
                                        .get("delta")
                                        .and_then(|d| d.get("text"))
                                        .and_then(|t| t.as_str())
                                    {
                                        if !text.is_empty() {
                                            if tx.send(text.to_string()).await.is_err() {
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
        });

        Ok(rx)
    }

    fn get_model(&self) -> &str {
        &self.base.model
    }

    /// 设置 temperature 运行时覆盖
    fn set_temperature_override(&self, temp: Option<f64>) {
        self.base.set_temperature_override(temp);
    }

    fn get_circuit_breaker_stats(&self) -> serde_json::Value {
        let stats = self.base.get_stats();
        serde_json::json!({
            "model": stats.model,
            "total_calls": stats.total_calls,
            "successful_calls": stats.successful_calls,
            "failed_calls": stats.failed_calls,
        })
    }

    /// Anthropic Claude Messages API 原生支持 function calling（tools + tool_use），
    /// 返回 true。配置层可通过 `enable_native_function_calling=false` 全局禁用。
    fn supports_native_function_calling(&self) -> bool {
        true
    }

    /// Anthropic 不直接支持 response_format / JSON Schema，
    /// 但可通过 `emit_response` 伪工具的 tool_use 通道实现等效结构化输出。
    /// 返回 true 表示调用方可以传入请求级 json_schema，provider 会自动包装成工具。
    fn supports_structured_output(&self) -> bool {
        true
    }

    /// Anthropic 不支持 JSON Mode (无 response_format 字段)
    fn supports_json_mode(&self) -> bool {
        false
    }

    /// 绑定工具列表，返回携带工具的新 provider 实例
    ///
    /// 通过克隆基础配置（api_key / base_url / model / circuit_breaker / proxy / client 等）
    /// 构造新实例，并在新实例的 `tools` 字段填充工具列表。
    /// 后续 `invoke` 调用会把 tools 注入请求体（Anthropic `input_schema` 格式），
    /// 并解析响应 content 数组中的 `tool_use` 块。
    /// 注意：`request_cache` 不共享（新实例独立缓存），`circuit_breaker` 共享（Arc）。
    fn bind_tools(
        &self,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<Box<dyn BaseProvider>> {
        Ok(Box::new(AnthropicProvider {
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
            },
            tools,
            cache_strategy: self.cache_strategy,
            instructions: self.instructions.clone(),
        }))
    }

    /// 结构化对话调用
    ///
    /// - 若未绑定 tools：回退到普通 `call_chat`
    /// - 否则：构建请求体并注入 Anthropic 格式的 `tools` 字段，
    ///   通过 `invoke_with_retry` 发送请求
    async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> {
        // 无工具 → 走普通文本路径
        if self.tools.is_empty() {
            let content = self.call_chat(messages).await?;
            return Ok(ChatResponse::from_text(content));
        }

        let prompt_key = messages_cache_key(&messages);

        let mut body = self.build_body(&messages, false);
        if let Some(tools_field) = self.build_tools_field(&None) {
            body["tools"] = tools_field;
        }

        self.invoke_with_retry(body, Some(&prompt_key)).await
    }

    /// 流式 + 原生 function calling
    ///
    /// Anthropic Claude Messages API 的 SSE 事件映射：
    /// - `content_block_start` (type=="text") → 开始追踪文本块（不发送事件）
    /// - `content_block_delta` (delta.type=="text_delta") → `StreamEvent::Text`
    /// - `content_block_start` (type=="tool_use") → `StreamEvent::ToolCallDelta`（携带 id 和 name）
    /// - `content_block_delta` (delta.type=="input_json_delta") → `StreamEvent::ToolCallDelta`（携带 arguments_delta）
    /// - `message_delta` 的 `stop_reason` → 记录为 `finish_reason`
    /// - `message_stop` → `StreamEvent::Done`
    async fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        self.base.check_circuit()?;

        let mut body = self.build_body(&messages, true);
        // tools 非空时注入 tools 字段
        if !tools.is_empty() {
            if let Some(tools_field) = self.build_tools_field(&None) {
                body["tools"] = tools_field;
            }
        }

        let client = self.base.get_client();
        let response = client
            .post(&self.endpoint())
            .header("x-api-key", &self.base.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Anthropic 流式 API 请求失败 ({}): {}",
                status, text
            )));
        }

        self.base.record_success();

        let (tx, rx) = mpsc::channel::<StreamEvent>(64);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut finish_reason: Option<String> = None;
            // 跟踪 emit_response 伪工具的 content_block index
            // 这些 index 的 input_json_delta 会被转成 StreamEvent::Text
            // 让下游消费者拿到结构化 JSON 文本（与非流式路径行为一致）
            let mut emit_response_indices: std::collections::HashSet<usize> =
                std::collections::HashSet::new();

            // Anthropic SSE 事件类型：
            //   message_start / content_block_start / content_block_delta /
            //   content_block_stop / message_delta / message_stop
            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx
                            .send(StreamEvent::Error {
                                message: format!("Anthropic 流读取失败: {}", e),
                            })
                            .await;
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // 按双换行分割 SSE 事件块
                while let Some(pos) = buffer.find("\n\n") {
                    let event_block = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    // 提取 data: 行
                    for line in event_block.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            let data = data.trim();
                            if let Ok(json_val) = serde_json::from_str::<Value>(data) {
                                let event_type = json_val
                                    .get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("");

                                match event_type {
                                    "content_block_start" => {
                                        let index = json_val
                                            .get("index")
                                            .and_then(|i| i.as_u64())
                                            .unwrap_or(0) as usize;
                                        let block = &json_val["content_block"];
                                        let block_type = block
                                            .get("type")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("");
                                        // type=="text" → 开始追踪文本块（不发送事件）
                                        // type=="thinking" → 开始追踪思维链块（不发送事件，等 delta）
                                        // type=="tool_use" → 发送携带 id 和 name 的 ToolCallDelta
                                        if block_type == "tool_use" {
                                            let id = block
                                                .get("id")
                                                .and_then(|i| i.as_str())
                                                .map(String::from);
                                            let name = block
                                                .get("name")
                                                .and_then(|n| n.as_str())
                                                .map(String::from);
                                            // emit_response 伪工具: 记录 index, 不发 ToolCallDelta
                                            // 它的 input_json_delta 会被转成 Text 事件
                                            if let Some(n) = &name {
                                                if crate::providers::schema::is_emit_response_call(n) {
                                                    emit_response_indices.insert(index);
                                                } else if tx
                                                    .send(StreamEvent::ToolCallDelta {
                                                        index,
                                                        id,
                                                        name,
                                                        arguments_delta: None,
                                                    })
                                                    .await
                                                    .is_err()
                                                {
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    "content_block_delta" => {
                                        let index = json_val
                                            .get("index")
                                            .and_then(|i| i.as_u64())
                                            .unwrap_or(0) as usize;
                                        let delta = &json_val["delta"];
                                        let delta_type = delta
                                            .get("type")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("");
                                        match delta_type {
                                            "text_delta" => {
                                                if let Some(text) =
                                                    delta.get("text").and_then(|t| t.as_str())
                                                {
                                                    if !text.is_empty()
                                                        && tx.send(StreamEvent::Text {
                                                            content: text.to_string(),
                                                        })
                                                        .await
                                                        .is_err()
                                                    {
                                                        return;
                                                    }
                                                }
                                            }
                                            "thinking_delta" => {
                                                if let Some(thinking) =
                                                    delta.get("thinking").and_then(|t| t.as_str())
                                                {
                                                    if !thinking.is_empty()
                                                        && tx
                                                            .send(StreamEvent::Thinking {
                                                                content: thinking.to_string(),
                                                            })
                                                            .await
                                                            .is_err()
                                                    {
                                                        return;
                                                    }
                                                }
                                            }
                                            "input_json_delta" => {
                                                if let Some(partial) = delta
                                                    .get("partial_json")
                                                    .and_then(|p| p.as_str())
                                                {
                                                    // emit_response 的 input_json_delta 转成 Text
                                                    // 让下游消费者拿到结构化 JSON 文本
                                                    if emit_response_indices.contains(&index) {
                                                        if !partial.is_empty()
                                                            && tx.send(StreamEvent::Text {
                                                                content: partial.to_string(),
                                                            })
                                                            .await
                                                            .is_err()
                                                        {
                                                            return;
                                                        }
                                                    } else if tx
                                                        .send(StreamEvent::ToolCallDelta {
                                                            index,
                                                            id: None,
                                                            name: None,
                                                            arguments_delta: Some(
                                                                partial.to_string(),
                                                            ),
                                                        })
                                                        .await
                                                        .is_err()
                                                    {
                                                        return;
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    "message_delta" => {
                                        // delta.stop_reason → finish_reason
                                        if let Some(stop) = json_val
                                            .get("delta")
                                            .and_then(|d| d.get("stop_reason"))
                                            .and_then(|s| s.as_str())
                                        {
                                            if !stop.is_empty() {
                                                finish_reason = Some(stop.to_string());
                                            }
                                        }
                                    }
                                    "message_stop" => {
                                        let _ = tx
                                            .send(StreamEvent::Done {
                                                finish_reason: finish_reason.take(),
                                    
                                            })
                                            .await;
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }

            // 流自然结束（未收到 message_stop）
            let _ = tx
                .send(StreamEvent::Done {
                    finish_reason: finish_reason.take(),
        
                })
                .await;
        });

        Ok(rx)
    }
}
