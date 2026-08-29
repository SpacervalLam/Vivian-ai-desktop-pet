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
/// 大 prompt 阈值（字节）：超过此体积的请求失败时不计入熔断器，
/// 避免一次性大 prompt 触发熔断后拖垮全局可用性。
const LARGE_PROMPT_BYTES: usize = 20_000;
/// 连接阶段超时上限（秒），与 `connect_timeout` 保持一致，
/// 用于在错误日志中区分 connect 阶段与 read 阶段失败。
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// 提示缓存策略
///
/// `auto` 由 provider 按模型启发式选择；`none` 显式关闭。
/// `prompt_cache_key` 注入顶层字段（Kimi / Moonshot），命中后端缓存降低延迟与费用。
/// OpenAI 兼容协议下 `cache_control` 无标准字段，此处仅作为标记位供上层路由判断。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheStrategy {
    #[default]
    Auto,
    PromptCacheKey,
    CacheControl,
    None,
}

impl CacheStrategy {
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "prompt_cache_key" => Self::PromptCacheKey,
            "cache_control" => Self::CacheControl,
            "none" => Self::None,
            _ => Self::Auto,
        }
    }
}

/// OpenAI Responses API 兼容提供商（`/responses` 端点）
///
/// 走 OpenAI Responses API，面向已实现该协议的厂商：DeepSeek / Qwen / Moonshot /
/// GLM / Doubao / SiliconFlow / Grok 等。核心能力：
/// - 顶层 `instructions` 参数传递框架规则（Responses API 原生支持）
/// - 扁平 tools schema（`name`/`description`/`parameters` 在顶层，非 `function` 嵌套）
/// - 流式 SSE 事件类型：`response.output_text.delta` /
///   `response.function_call_arguments.delta` / `response.reasoning_summary_text.delta` /
///   `response.completed`
/// - 多模态原生支持（`input_text` / `input_image` 内容块）
/// - 模型级别联网搜索字段注入（DeepSeek / GPT-4o / Qwen / GLM / Kimi / Doubao）
/// - 提示缓存策略（Kimi / Moonshot `prompt_cache_key`）
///
/// 标准 Chat Completions（`/chat/completions`）请使用 `chat_completions` 模块。
pub struct OpenAiCompatProvider {
    base: ProviderBase,
    /// 已绑定的工具列表（原生 function calling 路径使用）
    ///
    /// `bind_tools` 返回的新实例会填充此字段；后续 `invoke` 调用会把
    /// 它注入请求体的 `tools` 字段，并解析响应中的 `tool_calls`。
    tools: Vec<ToolDefinition>,
    /// 提示缓存策略
    cache_strategy: CacheStrategy,
    /// 模型级别预设（instructions 参数）
    ///
    /// 适用于 OpenAI Responses API 的 `instructions` 参数，将框架规则一次性设置，
    /// 不在每次请求中重复传输，减少 token 开销。
    instructions: Option<String>,
}

impl OpenAiCompatProvider {
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
        if url.ends_with("/responses") {
            url.to_string()
        } else {
            format!("{}/responses", url)
        }
    }

    /// 注入 Responses API 的结构化输出约束（`text.format`）
    ///
    /// 按能力分级注入：
    /// - `supports_structured_output()=true` → `text.format.type=json_schema` (strict)
    ///   通过 API 层强制 LLM 返回合法 JSON 并约束字段
    /// - `supports_json_mode()=true`（但不支持 strict）→ `text.format.type=json_object`
    ///   仅保证返回合法 JSON 语法，字段约束由 prompt 文本的 output_format 段提供
    /// - 都不支持 → 不注入，纯 prompt 文本约束
    ///
    /// 适用场景：纯文本对话路径（call_chat / call_chat_with_search / call_stream_chat）。
    /// **不适用场景**：function calling 路径（`invoke` / `stream_with_tools`），因 OpenAI 协议
    /// 禁止 `tools` 与 `text.format` 同时使用（会返回 400 错误），调用方需确保不传入 schema。
    fn inject_json_schema(&self, body: &mut Value, json_schema: &Option<serde_json::Value>) {
        if json_schema.is_none() {
            return;
        }
        if self.supports_structured_output() {
            body["text"] = json!({
                "format": {
                    "type": "json_schema",
                    "name": "vivian_response",
                    "schema": json_schema,
                    "strict": true
                }
            });
        } else if self.supports_json_mode() {
            // OpenAI / Ark 等 Responses API 兼容平台要求：使用 json_object 模式时，
            // input 中必须出现 "json" 一词，否则返回 400 InvalidParameter。
            // 检查 input 数组的序列化文本是否包含 "json"（不区分大小写），
            // 不满足时向 input 末尾追加一条轻量提示，确保关键词存在，保证结构化输出路径可用。
            let input_contains_json = body
                .get("input")
                .map(|v| v.to_string().to_lowercase().contains("json"))
                .unwrap_or(false);
            if !input_contains_json {
                if let Some(input) = body.get_mut("input").and_then(|v| v.as_array_mut()) {
                    input.push(json!({
                        "role": "user",
                        "content": "(Please respond in JSON format.)"
                    }));
                    tracing::debug!(
                        "[inject_json_schema] input 不含 'json' 关键词，已追加提示以保证 json_object 模式可用"
                    );
                }
            }
            body["text"] = json!({
                "format": {
                    "type": "json_object"
                }
            });
        }
    }

    /// 把内部 ChatMessage 转为 Responses API 的 `input` 数组
    ///
    /// 处理三种角色：
    /// - `system`：`{"role":"system","content":"..."}`（Responses API 接受此格式）
    /// - `assistant` + `tool_calls`：拆分为 `{"role":"assistant","content":"..."}`
    ///   + `{"type":"function_call","call_id":"...","name":"...","arguments":"..."}`
    /// - `tool`：`{"type":"function_call_output","call_id":"...","output":"..."}`
    ///
    /// 多模态：user 消息携带 `images` 时，content 转为数组形式，
    /// 在文本块（`input_text`）之后追加 `input_image` 块（base64 data URI 或 URL）。
    fn build_input_from_chat(messages: &[ChatMessage]) -> Vec<Value> {
        let mut input: Vec<Value> = Vec::new();
        for m in messages {
            match m.role.as_str() {
                "system" => {
                    input.push(json!({"role": "system", "content": m.content}));
                }
                "assistant" => {
                    if !m.content.is_empty() {
                        input.push(json!({"role": "assistant", "content": m.content}));
                    }
                    if let Some(tcs) = &m.tool_calls {
                        for tc in tcs {
                            let args_str = serde_json::to_string(&tc.arguments)
                                .unwrap_or_else(|_| "{}".to_string());
                            input.push(json!({
                                "type": "function_call",
                                "call_id": tc.id,
                                "name": tc.name,
                                "arguments": args_str,
                            }));
                        }
                    }
                }
                "tool" => {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": m.tool_call_id.clone().unwrap_or_default(),
                        "output": m.content,
                    }));
                }
                _ => {
                    if let Some(imgs) = &m.images {
                        if imgs.is_empty() {
                            input.push(json!({"role": m.role, "content": m.content}));
                        } else {
                            let mut content_arr: Vec<Value> =
                                vec![json!({"type": "input_text", "text": m.content})];
                            for img in imgs {
                                let image_url = if !img.data.is_empty() {
                                    format!("data:{};base64,{}", img.media_type, img.data)
                                } else if let Some(u) = &img.url {
                                    u.clone()
                                } else {
                                    continue;
                                };
                                content_arr.push(json!({
                                    "type": "input_image",
                                    "image_url": image_url,
                                }));
                            }
                            input.push(json!({"role": m.role, "content": content_arr}));
                        }
                    } else {
                        input.push(json!({"role": m.role, "content": m.content}));
                    }
                }
            }
        }
        input
    }

    /// 根据模型名注入联网搜索字段
    ///
    /// 各服务商联网搜索字段对照（基于 2026-07 官方文档）：
    /// - DeepSeek（model 含 `deepseek`）：顶层 `enable_search=true`
    /// - GPT-4o 系列（model 含 `gpt-4o`）：`web_search_options={"search_context_size": "high"}`（OpenAI 官方字段）
    /// - 通义千问 Qwen（model 含 `qwen`，DashScope OpenAI 兼容模式）：顶层 `enable_search=true`
    /// - 智谱 GLM（model 含 `glm`）：`tools=[{"type": "web_search", "web_search": {"enable": true}}]`
    /// - Moonshot Kimi（model 含 `moonshot` 或 `kimi`）：
    ///   `tools=[{"type": "builtin_function", "function": {"name": "$web_search"}}]`
    /// - 豆包 Doubao（model 含 `doubao`，火山方舟）：顶层 `enable_search=true`
    ///   （Seed 系列模型要求 tools 数组每项必须含 function 字段，旧格式 {"type":"web_search"} 会返回 400 错误）
    /// - 其他：通用顶层 `enable_search=true`（兼容 DeepSeek/Qwen 风格及多数 OpenAI 代理）
    ///
    /// 注意：若已绑定 function calling 工具（`self.tools` 非空），跳过 GLM / Moonshot
    /// 通过 `tools` 字段注入的联网搜索，避免与 function calling 的 `tools` 冲突；
    /// DeepSeek / GPT-4o / Qwen / Doubao / 通用顶层参数不冲突，仍可叠加。
    fn inject_search_fields(&self, body: &mut serde_json::Value) {
        let model_lower = self.base.model.to_lowercase();
        let function_calling_active = !self.tools.is_empty();

        if model_lower.contains("deepseek") {
            body["enable_search"] = json!(true);
            tracing::info!("[Router] DeepSeek 联网搜索已启用: model={}", self.base.model);
        } else if model_lower.contains("gpt-4o") {
            body["web_search_options"] = json!({"search_context_size": "high"});
            tracing::info!("[Router] GPT-4o 联网搜索已启用: model={}", self.base.model);
        } else if model_lower.contains("qwen") {
            body["enable_search"] = json!(true);
            tracing::info!("[Router] Qwen (DashScope) 联网搜索已启用: model={}", self.base.model);
        } else if model_lower.contains("glm") {
            if !function_calling_active {
                body["tools"] = json!([{
                    "type": "web_search",
                    "web_search": {"enable": true, "search_result": true}
                }]);
                tracing::info!("[Router] GLM (智谱) 联网搜索已启用: model={}", self.base.model);
            } else {
                tracing::info!("[Router] GLM 联网搜索已禁用（function calling 占用 tools 字段）: model={}", self.base.model);
            }
        } else if model_lower.contains("moonshot") || model_lower.contains("kimi") {
            if !function_calling_active {
                body["tools"] = json!([{
                    "type": "builtin_function",
                    "function": {"name": "$web_search"}
                }]);
                tracing::info!(
                    "[Router] Moonshot Kimi 联网搜索已启用: model={}",
                    self.base.model
                );
            } else {
                tracing::info!("[Router] Moonshot Kimi 联网搜索已禁用（function calling 占用 tools 字段）: model={}", self.base.model);
            }
        } else if model_lower.contains("doubao") {
            body["enable_search"] = json!(true);
            tracing::info!(
                "[Router] Doubao (火山方舟) 联网搜索已启用: model={}",
                self.base.model
            );
        } else {
            body["enable_search"] = json!(true);
            tracing::info!(
                "[Router] 通用联网搜索已启用: model={}, provider=openai_compat",
                self.base.model
            );
        }
    }

    async fn send_request(&self, body: serde_json::Value) -> VivianResult<serde_json::Value> {
        let client = self.base.get_client();
        let started = Instant::now();
        let response = client
            .post(&self.endpoint())
            .bearer_auth(&self.base.api_key)
            .header("OpenAI-Beta", "responses=1")
            .json(&body)
            .send()
            .await;
        let response = match response {
            Ok(r) => r,
            Err(e) => {
                let elapsed = started.elapsed();
                // 区分连接阶段与响应阶段失败：
                // - is_connect + elapsed < CONNECT_TIMEOUT_SECS → connect 阶段失败（TLS/DNS/TCP）
                // - is_timeout + elapsed < CONNECT_TIMEOUT_SECS → connect 阶段超时
                // - is_timeout + elapsed >= CONNECT_TIMEOUT_SECS → read 阶段超时（服务端慢响应）
                // - is_decode → 响应体解码失败（中断/格式错误）
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
                } else if e.is_body() {
                    "body-error"
                } else {
                    "other"
                };
                tracing::warn!(
                    "[send_request] {} 网络失败 phase={} elapsed={:.2}s err={}",
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
                "Responses API 请求失败 ({}): {}",
                status, text
            )));
        }

        let json: serde_json::Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                let elapsed = started.elapsed();
                tracing::warn!(
                    "[send_request] {} 响应解码失败 elapsed={:.2}s err={}",
                    self.base.model,
                    elapsed.as_secs_f64(),
                    e
                );
                return Err(VivianError::Network(format!("响应解码失败: {e}")));
            }
        };
        Ok(json)
    }

    fn extract_content(json: &serde_json::Value) -> VivianResult<String> {
        // 优先解析 Responses API 标准便捷字段 output_text（顶层）
        // 某些 provider 在模型只输出 reasoning 时仍会把空文本放到 output_text，
        // 或把正文直接放到顶层而不展开为 output[].content[].text 结构。
        if let Some(text) = json["output_text"].as_str() {
            if !text.trim().is_empty() {
                return Ok(strip_thinking_segments(text));
            }
        }

        // 解析 Responses API 的 output[] 数组，提取 message 项中的 output_text 文本
        if let Some(output) = json["output"].as_array() {
            for item in output {
                if item["type"].as_str() == Some("message") {
                    if let Some(content_arr) = item["content"].as_array() {
                        for c in content_arr {
                            if c["type"].as_str() == Some("output_text") {
                                if let Some(text) = c["text"].as_str() {
                                    return Ok(strip_thinking_segments(text));
                                }
                            }
                        }
                    }
                }
            }
        }
        // 检查是否存在 function_call 项（合法的工具调用响应，无文本输出）
        let has_function_calls = json["output"]
            .as_array()
            .map(|arr| arr.iter().any(|item| item["type"].as_str() == Some("function_call")))
            .unwrap_or(false);
        // 检查是否存在 reasoning 项（模型只输出思考内容，无最终 message 文本）
        let has_reasoning = json["output"]
            .as_array()
            .map(|arr| arr.iter().any(|item| item["type"].as_str() == Some("reasoning")))
            .unwrap_or(false);
        if has_function_calls || has_reasoning {
            Ok(String::new())
        } else {
            Err(VivianError::Provider(
                "响应中缺少 output[].content[].text".to_string(),
            ))
        }
    }

    /// 从 Responses API 响应 JSON 中提取完整结构化结果
    ///
    /// 解析 `output[]` 数组，按 `type` 区分：
    /// - `message`：提取 `content[]` 中 `type=="output_text"` 项的 `text`
    /// - `function_call`：提取 `call_id` / `name` / `arguments`（字符串，需 JSON 解析）
    /// - `reasoning`：提取 `summary[]` 中 `type=="summary_text"` 项的 `text`
    ///
    /// `finish_reason` 推断：
    /// - 存在 function_call 项 → "tool_calls"
    /// - status=="completed" → "stop"
    fn extract_chat_response(json: &serde_json::Value) -> VivianResult<ChatResponse> {
        let status = json["status"].as_str().unwrap_or("");
        if status == "failed" || status == "incomplete" {
            let err_msg = json["error"]["message"]
                .as_str()
                .unwrap_or("Responses API 返回失败状态");
            return Err(VivianError::Provider(format!(
                "Responses API 状态={}: {}",
                status, err_msg
            )));
        }

        let mut content = String::new();
        let mut reasoning_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<StructuredToolCall> = Vec::new();
        let mut finish_reason: Option<String> = None;

        if let Some(output) = json["output"].as_array() {
            for item in output {
                let item_type = item["type"].as_str().unwrap_or("");
                match item_type {
                    "message" => {
                        if let Some(content_arr) = item["content"].as_array() {
                            for c in content_arr {
                                if c["type"].as_str() == Some("output_text") {
                                    if let Some(text) = c["text"].as_str() {
                                        content.push_str(text);
                                    }
                                }
                            }
                        }
                    }
                    "reasoning" => {
                        if let Some(summary) = item["summary"].as_array() {
                            for s in summary {
                                if s["type"].as_str() == Some("summary_text") {
                                    if let Some(text) = s["text"].as_str() {
                                        reasoning_parts.push(text.to_string());
                                    }
                                }
                            }
                        }
                    }
                    "function_call" => {
                        let call_id = item["call_id"].as_str().unwrap_or("").to_string();
                        let name = item["name"].as_str().unwrap_or("").to_string();
                        let args_str = item["arguments"].as_str().unwrap_or("{}");
                        let arguments = serde_json::from_str(args_str)
                            .unwrap_or(Value::Object(serde_json::Map::new()));
                        if !name.is_empty() {
                            tool_calls.push(StructuredToolCall {
                                id: call_id,
                                name,
                                arguments,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // 备选：若 output[] 未提取到正文，尝试 Responses API 顶层便捷字段 output_text
        if content.is_empty() {
            if let Some(text) = json["output_text"].as_str() {
                content.push_str(text);
            }
        }

        if !tool_calls.is_empty() {
            finish_reason = Some("tool_calls".to_string());
        } else if status == "completed" {
            finish_reason = Some("stop".to_string());
        }

        let content = strip_thinking_segments(&content);
        let reasoning = if reasoning_parts.is_empty() {
            None
        } else {
            Some(reasoning_parts.join(""))
        };

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning,
            raw: json.clone(),
        })
    }

    /// 构造 function calling tools 字段（Responses API 扁平格式）
    ///
    /// 输出形如：
    /// ```json
    /// [{"type":"function","name":"...","description":"...","parameters":{...}}]
    /// ```
    fn build_tools_field(&self) -> Option<serde_json::Value> {
        if self.tools.is_empty() {
            return None;
        }
        let arr: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        Some(Value::Array(arr))
    }

    /// 结构化调用（带工具）的请求执行 —— 与 `call_with_retry` 类似，但返回 `ChatResponse`
    async fn invoke_with_retry(
        &self,
        body: serde_json::Value,
        cache_key_prompt: Option<&str>,
    ) -> VivianResult<ChatResponse> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("命中缓存(structured): {}", self.base.model);
                return Ok(ChatResponse::from_text(cached));
            }
        }

        let body_size = body.to_string().len();
        // 大 prompt 失败不计入熔断器：避免一次性大请求拖垮全局可用性
        let bypass_circuit_failure = body_size > LARGE_PROMPT_BYTES;
        if bypass_circuit_failure {
            tracing::warn!(
                "[invoke_with_retry] {} 大 prompt 检测 ({} bytes)，失败时跳过熔断器记录",
                self.base.model,
                body_size
            );
        }

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!("第 {} 次重试请求: {}", attempt, self.base.model);
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            match self.send_request(body.clone()).await {
                Ok(json) => {
                    let resp = Self::extract_chat_response(&json)?;
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
                    if bypass_circuit_failure {
                        tracing::debug!(
                            "[invoke_with_retry] 大 prompt 失败已跳过熔断器记录: {}",
                            err
                        );
                    } else {
                        self.base.record_failure();
                    }
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

    async fn call_with_retry(
        &self,
        body: serde_json::Value,
        cache_key_prompt: Option<&str>,
    ) -> VivianResult<String> {
        if let Some(prompt) = cache_key_prompt {
            if let Some(cached) = self.base.get_cached_response(prompt) {
                tracing::debug!("命中缓存: {}", self.base.model);
                return Ok(cached);
            }
        }

        let body_size = body.to_string().len();
        // 大 prompt 失败不计入熔断器：避免一次性大请求拖垮全局可用性
        let bypass_circuit_failure = body_size > LARGE_PROMPT_BYTES;
        if bypass_circuit_failure {
            tracing::warn!(
                "[call_with_retry] {} 大 prompt 检测 ({} bytes)，失败时跳过熔断器记录",
                self.base.model,
                body_size
            );
        }

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!("第 {} 次重试请求: {}", attempt, self.base.model);
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
                    if bypass_circuit_failure {
                        tracing::debug!(
                            "[call_with_retry] 大 prompt 失败已跳过熔断器记录: {}",
                            err
                        );
                    } else {
                        self.base.record_failure();
                    }
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

    /// 应用提示缓存策略
    ///
    /// - `prompt_cache_key`：注入顶层 `prompt_cache_key` 字段（Kimi / Moonshot），
    ///   后端按此 key 命中缓存，降低重复 prompt 的延迟与费用
    /// - `auto`：按模型名启发式，Kimi/Moonshot 模型自动注入 prompt_cache_key
    /// - `cache_control` / `none`：OpenAI 兼容协议无标准字段，不注入
    fn apply_cache_hints(&self, body: &mut Value) {
        let model_lower = self.base.model.to_lowercase();
        let is_kimi = model_lower.contains("kimi") || model_lower.contains("moonshot");
        let strategy = match self.cache_strategy {
            CacheStrategy::Auto => {
                if is_kimi {
                    CacheStrategy::PromptCacheKey
                } else {
                    CacheStrategy::None
                }
            }
            s => s,
        };
        if let CacheStrategy::PromptCacheKey = strategy {
            let key = format!("vivian:{}", &self.base.model);
            body["prompt_cache_key"] = json!(key);
        }
    }

    /// 按当前推理覆盖注入思考档位（Responses API 的 `reasoning.effort` 字段；
    /// Off / Auto 不注入，交由服务端默认）。
    fn apply_reasoning_fields(&self, body: &mut Value) {
        let pref = self.base.effective_reasoning();
        let cap = crate::providers::reasoning::resolve_reasoning_capability(&self.base.model);
        crate::providers::reasoning::apply_responses_reasoning(body, pref, &cap);
    }
}

#[async_trait]
impl BaseProvider for OpenAiCompatProvider {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        // 提示词占位符泄露检测（生产 warn，测试 panic）
        crate::persona::prompt_render::check_messages_for_leaks(
            &messages,
            &format!("call_chat model={}", self.base.model),
        );
        let prompt_key = messages_cache_key(&messages);
        let mut body = json!({
            "model": self.base.model,
            "input": Self::build_input_from_chat(&messages),
            "temperature": self.base.effective_temperature(),
            "max_output_tokens": self.base.effective_max_tokens(),
        });
        if let Some(instructions) = &self.instructions {
            body["instructions"] = json!(instructions);
        }
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body);
        self.inject_json_schema(&mut body, &None);
        self.call_with_retry(body, Some(&prompt_key)).await
    }

    /// 设置联网搜索开关 —— 覆盖 trait 默认实现，写入 ProviderBase 的 AtomicBool
    fn set_enable_search(&self, enable: bool) {
        self.base.set_enable_search(enable);
    }

    /// 设置 max_tokens 运行时覆盖 —— 凝神模式激活时由生成层调用。
    fn set_max_tokens_override(&self, tokens: u32) {
        self.base.set_max_tokens_override(tokens);
    }

    /// 设置 temperature 运行时覆盖 —— emotion→temperature 映射在每轮对话前调用。
    fn set_temperature_override(&self, temp: Option<f64>) {
        self.base.set_temperature_override(temp);
    }

    fn set_omit_temperature(&self, omit: bool) {
        self.base.set_omit_temperature(omit);
    }

    /// 设置推理偏好运行时覆盖 —— ModelRouter 按请求设置 / 恢复。
    fn set_reasoning_pref(&self, pref: Option<crate::providers::reasoning::ReasoningPreference>) {
        self.base.set_reasoning_pref(pref);
    }

    /// 带联网搜索的对话查询
    ///
    /// 当 `enable_search=true`（或 provider 自身开关开启）时，按模型名注入：
    /// - DeepSeek/Qwen/通用：顶层 `enable_search=true`
    /// - GPT-4o: `web_search_options={"search_context_size": "high"}`
    /// - GLM/Kimi/Doubao：通过 tools 字段注入对应搜索工具
    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<String> {
        let prompt_key = messages_cache_key(&messages);

        let mut body = json!({
            "model": self.base.model,
            "input": Self::build_input_from_chat(&messages),
            "temperature": self.base.effective_temperature(),
            "max_output_tokens": self.base.effective_max_tokens(),
        });

        if let Some(instructions) = &self.instructions {
            body["instructions"] = json!(instructions);
        }

        // 联网搜索：参数优先，叠加 provider 自身持久配置
        if enable_search || self.base.is_enable_search() {
            self.inject_search_fields(&mut body);
        }

        // Structured Outputs：在联网搜索注入之后（不与 tools 字段冲突）
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body);
        self.inject_json_schema(&mut body, &json_schema);

        self.call_with_retry(body, Some(&prompt_key)).await
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<mpsc::Receiver<String>> {
        // 提示词占位符泄露检测（生产 warn，测试 panic）
        crate::persona::prompt_render::check_messages_for_leaks(
            &messages,
            &format!("call_stream_chat model={}", self.base.model),
        );
        self.base.check_circuit()?;

        let mut body = json!({
            "model": self.base.model,
            "input": Self::build_input_from_chat(&messages),
            "temperature": self.base.effective_temperature(),
            "max_output_tokens": self.base.effective_max_tokens(),
            "stream": true,
        });

        if let Some(instructions) = &self.instructions {
            body["instructions"] = json!(instructions);
        }

        // 流式联网搜索：读取 provider 自身的 enable_search 字段（由 set_enable_search 同步）
        if self.base.is_enable_search() {
            self.inject_search_fields(&mut body);
        }

        // Structured Outputs：在联网搜索注入之后（不与 tools 字段冲突）
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body);
        self.inject_json_schema(&mut body, &json_schema);

        let client = self.base.get_client();
        let response = client
            .post(&self.endpoint())
            .bearer_auth(&self.base.api_key)
            .header("OpenAI-Beta", "responses=1")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Responses API 请求失败 ({}): {}",
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
                        tracing::error!("流读取失败: {}", e);
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
                            // 流结束：排空 stripper 残留（无闭合标签时原样返回）
                            if let Some(s) = stripper.as_mut() {
                                let residual = s.flush();
                                if !residual.is_empty() {
                                    let _ = tx.send(residual).await;
                                }
                            }
                            return;
                        }
                        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(data) {
                            let event_type = json_val["type"].as_str().unwrap_or("");
                            match event_type {
                                "response.output_text.delta" => {
                                    if let Some(delta) = json_val["delta"].as_str() {
                                        if !delta.is_empty() {
                                            let out = if let Some(s) = stripper.as_mut() {
                                                s.feed(delta)
                                            } else {
                                                delta.to_string()
                                            };
                                            if !out.is_empty() {
                                                if tx.send(out).await.is_err() {
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                }
                                "response.completed" => {
                                    // 流结束：排空 stripper 残留
                                    if let Some(s) = stripper.as_mut() {
                                        let residual = s.flush();
                                        if !residual.is_empty() {
                                            let _ = tx.send(residual).await;
                                        }
                                    }
                                    return;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            // 流自然结束（未收到 response.completed）：排空 stripper 残留
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

    fn get_circuit_breaker_stats(&self) -> serde_json::Value {
        let stats = self.base.get_stats();
        serde_json::json!({
            "model": stats.model,
            "total_calls": stats.total_calls,
            "successful_calls": stats.successful_calls,
            "failed_calls": stats.failed_calls,
        })
    }

    /// OpenAI 兼容接口（OpenAI / DeepSeek / Qwen / Moonshot / GLM / Doubao / SiliconFlow 等）
    /// 普遍支持原生 function calling，返回 true。
    /// 个别极简实现可能不支持，但主流服务商均支持，此处乐观返回 true。
    fn supports_native_function_calling(&self) -> bool {
        true
    }

    /// 第三方兼容平台（Ark/SiliconFlow 等）的 Responses API 对 strict json_schema 支持有限，
    /// 返回 false。此时 `inject_json_schema` 会降级为 `text.format.type=json_object`，
    /// 由 API 保证返回合法 JSON 语法，字段约束由 prompt 文本的 output_format 段提供。
    fn supports_structured_output(&self) -> bool {
        false
    }

    /// Responses API 兼容 JSON Mode (text.format.json_object 等价语义)
    fn supports_json_mode(&self) -> bool {
        true
    }

    /// 绑定工具列表，返回携带工具的新 provider 实例
    ///
    /// 通过克隆基础配置（api_key / base_url / model / circuit_breaker / cache 等）
    /// 构造新实例，并在新实例的 `tools` 字段填充工具列表。
    /// 后续 `invoke` 调用会把 tools 注入请求体并解析响应中的 tool_calls。
    fn bind_tools(
        &self,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<Box<dyn BaseProvider>> {
        Ok(Box::new(OpenAiCompatProvider {
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
            cache_strategy: self.cache_strategy,
            instructions: self.instructions.clone(),
        }))
    }

    /// 结构化对话调用
    ///
    /// - 若绑定了 tools：注入 `tools` + `tool_choice="auto"`，解析 `output[]` 中的 function_call
    /// - 否则回退到普通 `call_chat`，返回无工具调用的 `ChatResponse`
    async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> {
        // 无工具绑定 → 走普通文本路径，包装为 ChatResponse
        if self.tools.is_empty() {
            let content = self.call_chat(messages).await?;
            return Ok(ChatResponse::from_text(content));
        }

        let prompt_key = messages_cache_key(&messages);

        let mut body = json!({
            "model": self.base.model,
            "input": Self::build_input_from_chat(&messages),
            "temperature": self.base.effective_temperature(),
            "max_output_tokens": self.base.effective_max_tokens(),
            "tools": self.build_tools_field().unwrap_or(Value::Array(vec![])),
            "tool_choice": "auto",
        });

        // 模型级别预设：Responses API 原生支持 instructions 顶层参数
        if let Some(instructions) = &self.instructions {
            body["instructions"] = json!(instructions);
        }

        // 联网搜索：仅在工具未占用 tools 字段时叠加（DeepSeek/GPT-4o/Qwen 等不冲突的可叠加）
        if self.base.is_enable_search() {
            self.inject_search_fields(&mut body);
        }

        self.apply_cache_hints(&mut body);
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body);

        self.invoke_with_retry(body, Some(&prompt_key)).await
    }

    /// 流式 + 原生 function calling（Responses API 事件格式）
    ///
    /// Responses API 的流式工具调用事件：
    /// - `response.output_text.delta` —— 文本增量（field `delta`）
    /// - `response.reasoning_summary_text.delta` —— 推理增量（field `delta`）
    /// - `response.output_item.added` (item.type=="function_call") —— 工具调用项创建，
    ///   提取 `call_id` 和 `name`
    /// - `response.function_call_arguments.delta` —— 参数字符串增量（field `delta`，
    ///   `output_index` 区分多个并发工具调用）
    /// - `response.completed` —— 流结束，按是否有 tool_calls 推断 finish_reason
    ///
    /// DeepSeek / Qwen / GLM / Moonshot / Doubao 等均兼容此格式。
    async fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        // 提示词占位符泄露检测（生产 warn，测试 panic）
        crate::persona::prompt_render::check_messages_for_leaks(
            &messages,
            &format!("stream_with_tools model={}", self.base.model),
        );
        self.base.check_circuit()?;

        // 直接使用传入的 tools 构造 schema（扁平格式，无需 bind_tools 步骤）
        let tools_field: Value = if tools.is_empty() {
            Value::Array(vec![])
        } else {
            let arr: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect();
            Value::Array(arr)
        };

        let mut body = json!({
            "model": self.base.model,
            "input": Self::build_input_from_chat(&messages),
            "temperature": self.base.effective_temperature(),
            "max_output_tokens": self.base.effective_max_tokens(),
            "stream": true,
            "tools": tools_field,
            "tool_choice": "auto",
        });

        if let Some(instructions) = &self.instructions {
            body["instructions"] = json!(instructions);
        }

        if self.base.is_enable_search() {
            self.inject_search_fields(&mut body);
        }

        self.apply_cache_hints(&mut body);
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        self.apply_reasoning_fields(&mut body);

        let client = self.base.get_client();
        let response = client
            .post(&self.endpoint())
            .bearer_auth(&self.base.api_key)
            .header("OpenAI-Beta", "responses=1")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Responses API 请求失败 ({}): {}",
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
            // 按 output_index 累积工具调用的 (call_id, name, arguments)
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
                            // 流结束：排空 stripper 残留
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
                            let event_type = json_val["type"].as_str().unwrap_or("");
                            match event_type {
                                "response.output_text.delta" => {
                                    if let Some(delta) = json_val["delta"].as_str() {
                                        if !delta.is_empty() {
                                            let out = if let Some(s) = stripper.as_mut() {
                                                s.feed(delta)
                                            } else {
                                                delta.to_string()
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
                                }
                                "response.reasoning_summary_text.delta" => {
                                    if let Some(delta) = json_val["delta"].as_str() {
                                        if !delta.is_empty()
                                            && tx
                                                .send(StreamEvent::Thinking {
                                                    content: delta.to_string(),
                                                })
                                                .await
                                                .is_err()
                                        {
                                            return;
                                        }
                                    }
                                }
                                "response.output_item.added" => {
                                    let item = &json_val["item"];
                                    if item["type"].as_str() == Some("function_call") {
                                        let output_index = json_val["output_index"]
                                            .as_u64()
                                            .unwrap_or(0) as usize;
                                        let call_id =
                                            item["call_id"].as_str().map(String::from);
                                        let name = item["name"].as_str().map(String::from);
                                        let entry = tool_calls
                                            .entry(output_index)
                                            .or_insert((None, None, String::new()));
                                        if call_id.is_some() {
                                            entry.0 = call_id;
                                        }
                                        if name.is_some() {
                                            entry.1 = name;
                                        }
                                        if tx
                                            .send(StreamEvent::ToolCallDelta {
                                                index: output_index,
                                                id: entry.0.clone(),
                                                name: entry.1.clone(),
                                                arguments_delta: None,
                                            })
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                }
                                "response.function_call_arguments.delta" => {
                                    let output_index = json_val["output_index"]
                                        .as_u64()
                                        .unwrap_or(0) as usize;
                                    if let Some(delta) = json_val["delta"].as_str() {
                                        let entry = tool_calls
                                            .entry(output_index)
                                            .or_insert((None, None, String::new()));
                                        entry.2.push_str(delta);
                                        if tx
                                            .send(StreamEvent::ToolCallDelta {
                                                index: output_index,
                                                id: None,
                                                name: None,
                                                arguments_delta: Some(delta.to_string()),
                                            })
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                }
                                "response.completed" => {
                                    let status = json_val["response"]["status"]
                                        .as_str()
                                        .unwrap_or("");
                                    if status == "completed" {
                                        finish_reason = if !tool_calls.is_empty() {
                                            Some("tool_calls".to_string())
                                        } else {
                                            Some("stop".to_string())
                                        };
                                    }
                                    // usage：Responses API 在 response.completed 携带，兼容各家字段名
                                    if let Some(ev) = parse_stream_usage(&json_val["response"]["usage"]) {
                                        let _ = tx.send(ev).await;
                                    }
                                    if let Some(s) = stripper.as_mut() {
                                        let residual = s.flush();
                                        if !residual.is_empty() {
                                            let _ = tx
                                                .send(StreamEvent::Text { content: residual })
                                                .await;
                                        }
                                    }
                                    let _ = tx
                                        .send(StreamEvent::Done {
                                            finish_reason: finish_reason.take(),
                                        })
                                        .await;
                                    return;
                                }
                                "response.failed" => {
                                    let msg = json_val["response"]["error"]["message"]
                                        .as_str()
                                        .unwrap_or("Responses API 流式失败");
                                    let _ = tx
                                        .send(StreamEvent::Error {
                                            message: msg.to_string(),
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

            // 流自然结束（未收到 response.completed）：排空 stripper 残留
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
