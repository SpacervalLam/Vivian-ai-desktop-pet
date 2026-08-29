//! OpenAI 官方 Responses API Provider
//!
//! 走 OpenAI 新一代 `/v1/responses` 端点（非传统 `/chat/completions`），
//! 面向 GPT-4o / o1 / o3 及之后版本的模型。核心能力：
//! - 顶层 `instructions` 参数传递框架规则（替代 system 消息回退）
//! - 扁平 tools schema（`name`/`description`/`parameters` 在顶层，非 `function` 嵌套）
//! - 原生支持 MCP (Model Context Protocol) 工具调用
//! - 流式 SSE 事件类型：`response.output_text.delta` / `response.function_call_arguments.delta`
//!   / `response.reasoning_summary_text.delta` / `response.completed`
//! - 多模态原生支持（图片/音频输入输出）
//! - web_search_options 原生联网搜索
//!
//! 与 `OpenAiCompatProvider`（Chat Completions）的关键差异：
//! - endpoint：`{base_url}/responses`（非 `/chat/completions`）
//! - 请求体：`input` 数组（非 `messages`），`max_output_tokens`（非 `max_tokens`），
//!   顶层 `instructions`
//! - tools schema：扁平格式（`type`/`name`/`description`/`parameters` 均在顶层）
//! - 响应：`output[]` 数组，按 `type` 区分 `message` / `reasoning` / `function_call`
//! - 流式：按 SSE 事件 `type` 字段路由（`response.output_text.delta` 等）
//!
//! 状态化多轮设计:Vivian 已有完整记忆架构(MemoryManager + TimeStampedMemory +
//! ConsolidationPipeline),Brain 每轮传完整 messages,不依赖服务端 Conversation State,
//! 避免双 Context 问题。因此本 provider 不使用 `previous_response_id` 字段,
//! Responses API 当 Stateless 接口用。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::config::manager::ProviderConfig;
use crate::error::{VivianError, VivianResult};
use crate::network::http_client::get_global_client;
use crate::providers::base::{
    BaseProvider, ChatResponse, ProviderBase, StreamEvent, StructuredToolCall, ToolDefinition,
};
use crate::providers::thinking_stripper::{strip_thinking_segments, ThinkingStreamStripper};
use crate::resilience::{classify_error, ErrorCategory};
use crate::types::response::ChatMessage;

const MAX_RETRIES: usize = 2;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const LARGE_PROMPT_BYTES: usize = 20_000;
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// OpenAI 官方 Responses API Provider
pub struct OpenAiResponsesProvider {
    base: ProviderBase,
    tools: Vec<ToolDefinition>,
    instructions: Option<String>,
    client: Option<reqwest::Client>,
}

impl OpenAiResponsesProvider {
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
                client: client.clone(),
                ..base
            },
            tools: Vec::new(),
            instructions: None,
            client,
        }
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

    fn get_client(&self) -> reqwest::Client {
        self.client.clone().unwrap_or_else(get_global_client)
    }

    /// 把 ChatMessage 数组转为 Responses API 的 `input` 数组
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
                            let args_str =
                                serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".into());
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
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        Some(Value::Array(arr))
    }

    fn inject_search_fields(&self, body: &mut Value) {
        if self.base.is_enable_search() {
            body["web_search_options"] = json!({"search_context_size": "high"});
        }
    }

    fn build_request_body(&self, input: Vec<Value>, json_schema: &Option<serde_json::Value>) -> Value {
        let mut body = json!({
            "model": self.base.model,
            "input": input,
            "temperature": self.base.effective_temperature(),
            "max_output_tokens": self.base.effective_max_tokens(),
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        if let Some(instructions) = &self.instructions {
            body["instructions"] = json!(instructions);
        }
        // Structured Outputs: 把 schema 注入 text.format
        // OpenAI Responses API 通过 text.format.type=json_schema 约束输出
        if let Some(schema) = json_schema {
            body["text"] = json!({
                "format": {
                    "type": "json_schema",
                    "name": "vivian_response",
                    "schema": schema,
                    "strict": true
                }
            });
        }
        self.inject_search_fields(&mut body);
        body
    }

    async fn send_request(&self, body: Value) -> VivianResult<Value> {
        let started = Instant::now();
        let client = self.get_client();
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

        response
            .json::<Value>()
            .await
            .map_err(|e| VivianError::Provider(format!("响应 JSON 解析失败: {}", e)))
    }

    async fn call_with_retry(&self, body: Value) -> VivianResult<Value> {
        let body_size = body.to_string().len();
        let bypass_circuit_failure = body_size > LARGE_PROMPT_BYTES;
        if bypass_circuit_failure {
            tracing::warn!(
                "[OpenAiResponses] {} 大 prompt 检测 ({} bytes)，失败时跳过熔断器记录",
                self.base.model,
                body_size
            );
        }

        let mut last_error: Option<VivianError> = None;
        let mut backoff = INITIAL_BACKOFF;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tracing::warn!("[OpenAiResponses] 第 {} 次重试: {}", attempt, self.base.model);
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;
            match self.send_request(body.clone()).await {
                Ok(json) => {
                    self.base.record_success();
                    return Ok(json);
                }
                Err(err) => {
                    if !bypass_circuit_failure {
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

        Err(last_error.unwrap_or_else(|| VivianError::Provider("重试次数耗尽".to_string())))
    }

    fn extract_responses_response(json: &Value) -> VivianResult<ChatResponse> {
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

        if !tool_calls.is_empty() {
            finish_reason = Some("tool_calls".to_string());
        } else if status == "completed" {
            finish_reason = Some("stop".to_string());
        }

        let content = strip_thinking_segments(&content).to_string();
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

    fn extract_text_response(json: &Value) -> VivianResult<String> {
        let resp = Self::extract_responses_response(json)?;
        Ok(resp.content)
    }
}

#[async_trait]
impl BaseProvider for OpenAiResponsesProvider {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        crate::persona::prompt_render::check_messages_for_leaks(
            &messages,
            &format!("call_chat model={}", self.base.model),
        );
        let input = Self::build_input_from_chat(&messages);
        let body = self.build_request_body(input, &None);
        let json = self.call_with_retry(body).await?;
        Self::extract_text_response(&json)
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<mpsc::Receiver<String>> {
        let input = Self::build_input_from_chat(&messages);
        let mut body = self.build_request_body(input, &json_schema);
        body["stream"] = json!(true);

        self.base.check_circuit()?;
        let client = self.get_client();
        let response = client
            .post(&self.endpoint())
            .bearer_auth(&self.base.api_key)
            .header("OpenAI-Beta", "responses=1")
            .json(&body)
            .send()
            .await
            .map_err(|e| VivianError::Provider(format!("流式请求失败: {}", e)))?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Responses API 流式请求失败 ({}): {}",
                status, text
            )));
        }
        self.base.record_success();

        let (tx, rx) = mpsc::channel::<String>(64);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut stripper = ThinkingStreamStripper::new();

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx
                            .send(format!("[stream error: {}]", e))
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
                            let residual = stripper.flush();
                            if !residual.is_empty() {
                                let _ = tx.send(residual).await;
                            }
                            return;
                        }
                        if let Ok(json_val) = serde_json::from_str::<Value>(data) {
                            let event_type = json_val["type"].as_str().unwrap_or("");
                            match event_type {
                                "response.output_text.delta" => {
                                    if let Some(delta) = json_val["delta"].as_str() {
                                        let out = stripper.feed(delta);
                                        if !out.is_empty() {
                                            if tx.send(out).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                                "response.completed" => {
                                    let residual = stripper.flush();
                                    if !residual.is_empty() {
                                        let _ = tx.send(residual).await;
                                    }
                                    return;
                                }
                                "response.failed" => {
                                    let msg = json_val["response"]["error"]["message"]
                                        .as_str()
                                        .unwrap_or("Responses API 流式失败");
                                    let _ = tx.send(format!("[stream failed: {}]", msg)).await;
                                    return;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            let residual = stripper.flush();
            if !residual.is_empty() {
                let _ = tx.send(residual).await;
            }
        });

        Ok(rx)
    }

    fn get_model(&self) -> &str {
        &self.base.model
    }

    fn get_circuit_breaker_stats(&self) -> Value {
        let stats = self.base.get_stats();
        serde_json::to_value(stats).unwrap_or(Value::Null)
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

    fn supports_native_function_calling(&self) -> bool {
        true
    }

    fn supports_structured_output(&self) -> bool {
        true
    }

    fn supports_json_mode(&self) -> bool {
        true
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> VivianResult<Box<dyn BaseProvider>> {
        Ok(Box::new(OpenAiResponsesProvider {
            base: ProviderBase {
                api_key: self.base.api_key.clone(),
                base_url: self.base.base_url.clone(),
                model: self.base.model.clone(),
                temperature: self.base.effective_temperature(),
                max_tokens: self.base.max_tokens,
                circuit_breaker: Arc::clone(&self.base.circuit_breaker),
                request_cache: parking_lot::Mutex::new(HashMap::new()),
                enable_search: std::sync::atomic::AtomicBool::new(self.base.is_enable_search()),
                proxy: self.base.proxy.clone(),
                client: self.base.client.clone(),
                max_tokens_override: std::sync::atomic::AtomicU32::new(0),
                temperature_override: std::sync::atomic::AtomicU64::new(0),
                omit_temperature: std::sync::atomic::AtomicBool::new(false),
                reasoning_pref: parking_lot::RwLock::new(*self.base.reasoning_pref.read()),
            },
            tools,
            instructions: self.instructions.clone(),
            client: self.client.clone(),
        }))
    }

    async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> {
        if self.tools.is_empty() {
            let content = self.call_chat(messages).await?;
            return Ok(ChatResponse::from_text(content));
        }

        let input = Self::build_input_from_chat(&messages);
        let mut body = self.build_request_body(input, &None);
        if let Some(tools_field) = self.build_tools_field() {
            body["tools"] = tools_field;
            body["tool_choice"] = json!("auto");
        }

        let json = self.call_with_retry(body).await?;
        let resp = Self::extract_responses_response(&json)?;
        Ok(resp)
    }

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
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect();
            Value::Array(arr)
        };

        let input = Self::build_input_from_chat(&messages);
        let mut body = self.build_request_body(input, &None);
        body["stream"] = json!(true);
        if !tools_field.as_array().map(|a| a.is_empty()).unwrap_or(true) {
            body["tools"] = tools_field;
            body["tool_choice"] = json!("auto");
        }

        let client = self.get_client();
        let response = client
            .post(&self.endpoint())
            .bearer_auth(&self.base.api_key)
            .header("OpenAI-Beta", "responses=1")
            .json(&body)
            .send()
            .await
            .map_err(|e| VivianError::Provider(format!("流式请求失败: {}", e)))?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "Responses API 流式请求失败 ({}): {}",
                status, text
            )));
        }
        self.base.record_success();

        let (tx, rx) = mpsc::channel::<StreamEvent>(64);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut finish_reason: Option<String> = None;
            let mut stripper = ThinkingStreamStripper::new();

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
                                        let out = stripper.feed(delta);
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
                                        let call_id = item["call_id"]
                                            .as_str()
                                            .map(String::from);
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
                                        finish_reason =
                                            if !tool_calls.is_empty() {
                                                Some("tool_calls".to_string())
                                            } else {
                                                Some("stop".to_string())
                                            };
                                    }
                                    let residual = stripper.flush();
                                    if !residual.is_empty() {
                                        let _ = tx
                                            .send(StreamEvent::Text { content: residual })
                                            .await;
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

            let residual = stripper.flush();
            if !residual.is_empty() {
                let _ = tx.send(StreamEvent::Text { content: residual }).await;
            }
            let _ = tx
                .send(StreamEvent::Done {
                    finish_reason: finish_reason.take(),
                })
                .await;
        });

        Ok(rx)
    }

    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<String> {
        let prev = self.base.is_enable_search();
        if enable_search != prev {
            self.base.set_enable_search(enable_search);
        }
        let input = Self::build_input_from_chat(&messages);
        let body = self.build_request_body(input, &json_schema);
        let result = self.call_with_retry(body).await.and_then(|json| Self::extract_text_response(&json));
        if enable_search != prev {
            self.base.set_enable_search(prev);
        }
        result
    }
}
