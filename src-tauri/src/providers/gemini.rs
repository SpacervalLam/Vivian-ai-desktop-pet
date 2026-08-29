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
use crate::resilience::{classify_error, ErrorCategory};
use crate::types::response::ChatMessage;
use crate::utils::messages_cache_key;

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const MAX_RETRIES: usize = 2;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

pub struct GeminiProvider {
    api_key: String,
    model: String,
    base: ProviderBase,
    /// 已绑定的工具列表（原生 function calling 路径使用）
    ///
    /// `bind_tools` 返回的新实例会填充此字段；后续 `invoke` 调用会把
    /// 它注入请求体的 `tools` 字段，并解析响应中的 `functionCall`。
    tools: Vec<ToolDefinition>,
}

impl GeminiProvider {
    pub fn new(
        api_key: &str,
        model: &str,
        temperature: f64,
        max_tokens: u32,
        proxy: Option<String>,
        client: Option<reqwest::Client>,
    ) -> Self {
        let base = ProviderBase::new(
            api_key.to_string(),
            GEMINI_BASE_URL.to_string(),
            model.to_string(),
            temperature,
            max_tokens,
        );
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            base: ProviderBase {
                proxy,
                client,
                ..base
            },
            tools: Vec::new(),
        }
    }

    fn generate_endpoint(&self) -> String {
        format!(
            "{}/models/{}:generateContent?key={}",
            GEMINI_BASE_URL, self.model, self.api_key
        )
    }

    fn stream_endpoint(&self) -> String {
        format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            GEMINI_BASE_URL, self.model, self.api_key
        )
    }

    fn build_contents_from_chat(messages: &[ChatMessage]) -> serde_json::Value {
        // 收集 assistant 消息中 tool_call_id → tool name 的映射，
        // 用于把 tool 角色消息回填 Gemini 所需的 functionResponse.name。
        let mut tool_call_names: HashMap<String, String> = HashMap::new();
        for m in messages {
            if m.role == "assistant" {
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        tool_call_names.insert(tc.id.clone(), tc.name.clone());
                    }
                }
            }
        }

        let contents: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                if m.role == "assistant" {
                    // assistant + tool_calls → role="model"，parts 包含 functionCall
                    if let Some(tcs) = &m.tool_calls {
                        let mut parts: Vec<Value> = Vec::new();
                        if !m.content.is_empty() {
                            parts.push(json!({"text": m.content}));
                        }
                        for tc in tcs {
                            parts.push(json!({
                                "functionCall": {
                                    "name": tc.name,
                                    "args": tc.arguments,
                                }
                            }));
                        }
                        json!({"role": "model", "parts": parts})
                    } else {
                        json!({"role": "model", "parts": [{"text": m.content}]})
                    }
                } else if m.role == "tool" {
                    // tool → role="user"（Gemini 要求 functionResponse 在 user 消息中）
                    let name = m
                        .tool_call_id
                        .as_ref()
                        .and_then(|id| tool_call_names.get(id))
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    // 尝试把 content 解析为 JSON 对象；失败则包装为 {"result": content}
                    let response = serde_json::from_str::<Value>(&m.content)
                        .unwrap_or_else(|_| json!({"result": m.content}));
                    json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": name,
                                "response": response,
                            }
                        }]
                    })
                } else {
                    // user / system / 其他：默认 role + text parts
                    let role = &m.role;
                    // 多模态：user 消息携带 images 时，parts 中追加 inline_data（base64）
                    // 或 file_data（url）块
                    if let Some(imgs) = &m.images {
                        if !imgs.is_empty() {
                            let mut parts: Vec<Value> = Vec::new();
                            if !m.content.is_empty() {
                                parts.push(json!({"text": m.content}));
                            }
                            for img in imgs {
                                if !img.data.is_empty() {
                                    parts.push(json!({
                                        "inline_data": {
                                            "mime_type": img.media_type,
                                            "data": img.data,
                                        }
                                    }));
                                } else if let Some(u) = &img.url {
                                    parts.push(json!({
                                        "file_data": {
                                            "mime_type": img.media_type,
                                            "file_uri": u,
                                        }
                                    }));
                                }
                            }
                            json!({"role": role, "parts": parts})
                        } else {
                            json!({"role": role, "parts": [{"text": m.content}]})
                        }
                    } else {
                        json!({"role": role, "parts": [{"text": m.content}]})
                    }
                }
            })
            .collect();
        json!(contents)
    }

    fn build_body(&self, contents: serde_json::Value, json_schema: &Option<serde_json::Value>) -> serde_json::Value {
        let mut generation_config = json!({
            "temperature": self.base.effective_temperature(),
            "maxOutputTokens": self.base.max_tokens,
            // JSON Mode：强制 Gemini 返回合法 JSON，与 prompt 中的 OUTPUT_FORMAT 约束协同
            // 注意：function calling 路径（stream_with_tools）也会走 build_body，
            // Gemini 允许 responseMimeType 与 tools 同时使用（与 OpenAI 不同），故统一注入
            "responseMimeType": "application/json",
        });
        // Structured Outputs: Gemini 通过 generationConfig.responseSchema 约束输出
        // 与 responseMimeType=application/json 配合使用
        // 注意：Gemini 不支持 JSON Schema 的 $ref / $defs，需内联解析后再注入
        if let Some(schema) = json_schema {
            let sanitized = Self::resolve_schema_refs(schema);
            generation_config["responseSchema"] = sanitized;
        }
        let mut body = json!({
            "contents": contents,
            "generationConfig": generation_config,
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        body
    }

    /// 构建带 Google Search grounding 的请求体
    ///
    /// 注：Rust 生态暂无成熟的 `google-genai` SDK，使用 REST API 直接调用，
    /// 通过 `tools=[{"google_search": {}}]` 实现等效的 Google Search grounding。
    /// 后续可替换为原生 SDK 调用以获得更完整的 grounding 元数据。
    fn build_body_with_search(&self, contents: serde_json::Value, enable_search: bool, json_schema: &Option<serde_json::Value>) -> serde_json::Value {
        let mut body = self.build_body(contents, json_schema);
        if enable_search {
            body["tools"] = json!([{"google_search": {}}]);
            tracing::info!(
                "[Router] Gemini Google Search Grounding 已启用(REST): model={}",
                self.model
            );
        }
        body
    }

    /// 解析 JSON Schema 中的 `$ref` 引用，将定义内联到引用位置。
    ///
    /// Gemini 的 `responseSchema` 不支持 `$ref` / `$defs`，
    /// 需要在发送前将所有引用展开为内联定义。
    fn resolve_schema_refs(schema: &serde_json::Value) -> serde_json::Value {
        let defs = schema.get("$defs")
            .or_else(|| schema.get("definitions"))
            .cloned()
            .unwrap_or(json!({}));

        let mut resolved = schema.clone();
        if let Some(obj) = resolved.as_object_mut() {
            obj.remove("$defs");
            obj.remove("definitions");
        }
        Self::inline_refs(&resolved, &defs, 0)
    }

    /// 递归地将 `$ref` 替换为内联定义
    fn inline_refs(value: &serde_json::Value, defs: &serde_json::Value, depth: u32) -> serde_json::Value {
        if depth > 10 {
            return value.clone();
        }

        match value {
            serde_json::Value::Object(obj) => {
                if let Some(ref_path) = obj.get("$ref").and_then(|v| v.as_str()) {
                    let name = ref_path.rsplit('/').next().unwrap_or("");
                    if let Some(def) = defs.get(name) {
                        return Self::inline_refs(def, defs, depth + 1);
                    }
                    return json!({});
                }
                let mut new_obj = serde_json::Map::new();
                for (k, v) in obj.iter() {
                    new_obj.insert(k.clone(), Self::inline_refs(v, defs, depth + 1));
                }
                serde_json::Value::Object(new_obj)
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(
                    arr.iter().map(|v| Self::inline_refs(v, defs, depth + 1)).collect(),
                )
            }
            _ => value.clone(),
        }
    }

    async fn send_request(&self, body: serde_json::Value) -> VivianResult<serde_json::Value> {
        let client = self.base.get_client();
        let response = client
            .post(&self.generate_endpoint())
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Gemini API 请求失败 ({}): {}",
                status, text
            )));
        }

        let json: serde_json::Value = response.json().await?;
        Ok(json)
    }

    /// 构造 Gemini function calling 的 tools 字段
    ///
    /// 输出形如：
    /// ```json
    /// [{"function_declarations": [{"name": "...", "description": "...", "parameters": {...}}]}]
    /// ```
    /// 无工具时返回 None。
    fn build_tools_field(&self) -> Option<Value> {
        if self.tools.is_empty() {
            return None;
        }
        let arr: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        Some(json!([{"function_declarations": arr}]))
    }

    /// 从 Gemini 响应中提取 content + tool_calls，构造结构化 ChatResponse
    ///
    /// 解析 `candidates[0].content.parts` 数组：
    /// - `{"text": "..."}` → 拼接到 content
    /// - `{"functionCall": {"name": "...", "args": {...}}}` → 转为 StructuredToolCall
    ///   （Gemini 不返回 tool_call_id，使用 `call_{index}` 合成）
    fn extract_chat_response(&self, json: &Value) -> VivianResult<ChatResponse> {
        let candidate = &json["candidates"][0];
        let parts = &candidate["content"]["parts"];

        let mut content = String::new();
        let mut tool_calls: Vec<StructuredToolCall> = Vec::new();

        if let Some(arr) = parts.as_array() {
            for part in arr.iter() {
                if let Some(text) = part["text"].as_str() {
                    content.push_str(text);
                }
                if let Some(fc) = part.get("functionCall") {
                    let name = fc["name"].as_str().unwrap_or("").to_string();
                    let args = fc["args"].clone();
                    if !name.is_empty() {
                        tool_calls.push(StructuredToolCall {
                            id: format!("call_{}", tool_calls.len()),
                            name,
                            arguments: args,
                        });
                    }
                }
            }
        }

        let finish_reason = candidate["finishReason"]
            .as_str()
            .map(|s| s.to_string());

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning: None,
            raw: json.clone(),

        })
    }

    /// 结构化调用（带工具）的请求执行 —— 与 `call_with_retry` 类似，但返回 `ChatResponse`
    async fn invoke_with_retry(
        &self,
        body: serde_json::Value,
        cache_key_prompt: Option<&str>,
    ) -> VivianResult<ChatResponse> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("命中缓存(structured): {}", self.model);
                return Ok(ChatResponse::from_text(cached));
            }
        }

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!("第 {} 次重试请求: {}", attempt, self.model);
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(body.clone()).await {
                Ok(json) => {
                    let resp = self.extract_chat_response(&json)?;
                    self.base.record_success();
                    // 仅缓存无工具调用的响应（带工具调用的响应需重新触发执行）
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
            .unwrap_or_else(|| VivianError::Provider("重试次数耗尽".to_string())))
    }

    fn extract_content(json: &serde_json::Value) -> VivianResult<String> {
        json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                VivianError::Provider(
                    "响应中缺少 candidates[0].content.parts[0].text".to_string(),
                )
            })
    }

    async fn call_with_retry(
        &self,
        body: serde_json::Value,
        cache_key_prompt: Option<&str>,
    ) -> VivianResult<String> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("命中缓存: {}", self.model);
                return Ok(cached);
            }
        }

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!("第 {} 次重试请求: {}", attempt, self.model);
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(body.clone()).await {
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
            .unwrap_or_else(|| VivianError::Provider("重试次数耗尽".to_string())))
    }
}

#[async_trait]
impl BaseProvider for GeminiProvider {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        let prompt_key = messages_cache_key(&messages);
        let contents = Self::build_contents_from_chat(&messages);
        let body = self.build_body(contents, &None);
        self.call_with_retry(body, Some(&prompt_key)).await
    }

    /// 设置联网搜索开关 —— 覆盖 trait 默认实现，写入 ProviderBase 的 AtomicBool
    fn set_enable_search(&self, enable: bool) {
        self.base.set_enable_search(enable);
    }

    /// 设置 temperature 运行时覆盖
    fn set_temperature_override(&self, temp: Option<f64>) {
        self.base.set_temperature_override(temp);
    }

    fn set_omit_temperature(&self, omit: bool) {
        self.base.set_omit_temperature(omit);
    }

    /// 带联网搜索的对话查询
    ///
    /// 注：Rust 生态暂无成熟的 `google-genai` SDK，使用 REST API 直接调用，
    /// 通过 `tools=[{"google_search": {}}]` 实现等效的 Google Search grounding。
    /// 后续可替换为原生 SDK 调用。
    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<String> {
        let prompt_key = messages_cache_key(&messages);
        let contents = Self::build_contents_from_chat(&messages);
        // 联网搜索：参数优先，叠加 provider 自身持久配置
        let flag = enable_search || self.base.is_enable_search();
        let body = self.build_body_with_search(contents, flag, &json_schema);
        self.call_with_retry(body, Some(&prompt_key)).await
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<mpsc::Receiver<String>> {
        self.base.check_circuit()?;

        let contents = Self::build_contents_from_chat(&messages);
        // 流式联网搜索：读取 provider 自身的 enable_search 字段（由 set_enable_search 同步）
        let body = self.build_body_with_search(contents, self.base.is_enable_search(), &json_schema);

        let client = self.base.get_client();
        let response = client
            .post(&self.stream_endpoint())
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Gemini API 请求失败 ({}): {}",
                status, text
            )));
        }

        self.base.record_success();

        let (tx, rx) = mpsc::channel::<String>(32);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Gemini 流读取失败: {}", e);
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
                            return;
                        }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(content) =
                                json["candidates"][0]["content"]["parts"][0]["text"].as_str()
                            {
                                if !content.is_empty() {
                                    if tx.send(content.to_string()).await.is_err() {
                                        return;
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
        &self.model
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

    /// Gemini 支持原生 function calling（REST API 注入 `tools[].functionDeclarations[]`）
    fn supports_native_function_calling(&self) -> bool {
        true
    }

    /// Gemini 支持 Structured Outputs（generationConfig.responseSchema）
    fn supports_structured_output(&self) -> bool {
        true
    }

    /// Gemini 支持 JSON Mode（generationConfig.responseMimeType=application/json）
    fn supports_json_mode(&self) -> bool {
        true
    }

    /// 绑定工具列表，返回携带工具的新 provider 实例
    ///
    /// 通过克隆基础配置（api_key / base_url / model / circuit_breaker / cache 等）
    /// 构造新实例，并在新实例的 `tools` 字段填充工具列表。
    /// 后续 `invoke` 调用会把 tools 注入请求体并解析响应中的 `functionCall`。
    fn bind_tools(
        &self,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<Box<dyn BaseProvider>> {
        Ok(Box::new(GeminiProvider {
            api_key: self.api_key.clone(),
            model: self.model.clone(),
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
        }))
    }

    /// 结构化对话调用
    ///
    /// - 若绑定了 tools：构建请求体并注入 Gemini `function_declarations`，解析 `functionCall`
    /// - 否则回退到普通 `call_chat`，返回无工具调用的 `ChatResponse`
    async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> {
        // 无工具绑定 → 走普通文本路径，包装为 ChatResponse
        if self.tools.is_empty() {
            let content = self.call_chat(messages).await?;
            return Ok(ChatResponse::from_text(content));
        }

        let prompt_key = messages_cache_key(&messages);

        let contents = Self::build_contents_from_chat(&messages);
        let mut body = self.build_body(contents, &None);

        // 注入 Gemini function calling 工具
        if let Some(tools_field) = self.build_tools_field() {
            body["tools"] = tools_field;
        }

        self.invoke_with_retry(body, Some(&prompt_key)).await
    }

    /// 流式 + 原生 function calling
    ///
    /// Gemini `streamGenerateContent` 端点（`?alt=sse` 返回 SSE 格式），每个 `data:`
    /// 行是一个完整的 `GenerateContentResponse`。
    ///
    /// 与 OpenAI/Anthropic 的关键差异：
    /// - `functionCall` 在流式中**完整出现**（非增量），首个包含 functionCall 的
    ///   chunk 即给出完整 `name` + `args`
    /// - 多个 parts 可能同时出现（text + functionCall）
    /// - 无 `tool_call_id` 概念，`StreamEvent::ToolCallDelta.id` 始终为 `None`
    /// - `finishReason` 值：STOP / SAFETY / RECITATION / MAX_TOKENS 等
    ///
    /// 映射规则：
    /// - `parts[].text` → `StreamEvent::Text`
    /// - `parts[].functionCall` → `StreamEvent::ToolCallDelta`
    ///   （`args` 序列化为 JSON 字符串作为 `arguments_delta`，`index` 按出现顺序递增）
    /// - `candidates[0].finishReason` → 记录，流结束时通过 `Done` 事件返回
    async fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        self.base.check_circuit()?;

        let contents = Self::build_contents_from_chat(&messages);
        let mut body = self.build_body(contents, &None);

        // 注入 Gemini function calling 工具声明（复用 build_tools_field）
        if !tools.is_empty() {
            if let Some(tools_field) = self.build_tools_field() {
                body["tools"] = tools_field;
            }
        }

        // 流式端点（不在 URL 中携带 key，改用 x-goog-api-key header）
        let endpoint = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            GEMINI_BASE_URL, self.model
        );

        let client = self.base.get_client();
        let response = client
            .post(&endpoint)
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Gemini API 请求失败 ({}): {}",
                status, text
            )));
        }

        self.base.record_success();

        let (tx, rx) = mpsc::channel::<StreamEvent>(64);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut finish_reason: Option<String> = None;
            // 工具调用索引：按 functionCall 在流中的出现顺序递增
            let mut tool_call_index: usize = 0;

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx
                            .send(StreamEvent::Error {
                                message: format!("Gemini 流读取失败: {}", e),
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
                            let _ = tx
                                .send(StreamEvent::Done {
                                    finish_reason: finish_reason.take(),
                        
                                })
                                .await;
                            return;
                        }
                        if let Ok(json_val) = serde_json::from_str::<Value>(data) {
                            let candidate = &json_val["candidates"][0];

                            // 记录结束原因
                            if let Some(fr) = candidate["finishReason"].as_str() {
                                if !fr.is_empty() {
                                    finish_reason = Some(fr.to_string());
                                }
                            }

                            // 解析 parts：text 与 functionCall 可能同时出现
                            let parts = &candidate["content"]["parts"];
                            if let Some(arr) = parts.as_array() {
                                for part in arr.iter() {
                                    // 文本增量
                                    if let Some(text) = part["text"].as_str() {
                                        if !text.is_empty() {
                                            if tx
                                                .send(StreamEvent::Text {
                                                    content: text.to_string(),
                                                })
                                                .await
                                                .is_err()
                                            {
                                                return;
                                            }
                                        }
                                    }

                                    // functionCall（Gemini 流式中完整出现）
                                    if let Some(fc) = part.get("functionCall") {
                                        let name = fc["name"].as_str().map(String::from);
                                        // args 是完整 JSON 对象，序列化为字符串作为 arguments_delta
                                        let arguments_delta = serde_json::to_string(&fc["args"])
                                            .ok();
                                        let idx = tool_call_index;
                                        tool_call_index += 1;

                                        if tx
                                            .send(StreamEvent::ToolCallDelta {
                                                index: idx,
                                                id: None,
                                                name,
                                                arguments_delta,
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

            // 流自然结束（未收到 [DONE]）
            let _ = tx
                .send(StreamEvent::Done {
                    finish_reason: finish_reason.take(),
        
                })
                .await;
        });

        Ok(rx)
    }
}
