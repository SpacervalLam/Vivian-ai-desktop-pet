//! 声明式 Provider —— 按 `ProtocolSpec` 通用解释器
//!
//! 同一份代码驱动所有来自插件 `protocols/*.json` 的协议，无需为每个厂商改 Rust：
//! - `request.body` 提供静态骨架 + `{占位符}`，运行时用消息/参数/工具/搜索等替换。
//! - `message_format` 选择原生序列化器（角色映射 / 工具重建 / 多模态）。
//! - `response` / `streaming` 用 `PathExpr` 从 JSON 提取结果与流式增量。
//!
//! 复用 `ProviderBase`（熔断 / 缓存 / 温度&max_tokens 覆盖 / 代理 / 专属客户端），
//! 从而与原生 adapter 的鲁棒性底座保持一致。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::config::manager::ProviderConfig;
use crate::error::{VivianError, VivianResult};
use crate::providers::base::{
    parse_stream_usage, scope_provider_call, BaseProvider, ChatResponse, ProviderBase,
    ProviderCallOptions, StreamEvent, StructuredToolCall, ToolDefinition,
};
use crate::providers::message_format;
use crate::providers::spec::{
    CacheMode, CompiledSpec, JsonSchemaMode, MessageFormat, ReasoningStyle, SearchMode, ToolsFormat,
};
use crate::providers::thinking_stripper::strip_thinking_segments;
use crate::providers::usage_store;
use crate::types::response::ChatMessage;

/// 声明式（通用解释器）Provider
pub struct DeclarativeProvider {
    base: ProviderBase,
    spec: Arc<CompiledSpec>,
    instructions: Option<String>,
    api_secret: String,
    tools: Vec<ToolDefinition>,
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use axum::{body::Body, response::Response, routing::post, Router};

    fn spec() -> Arc<CompiledSpec> {
        Arc::new(CompiledSpec::compile(serde_json::from_value(json!({
            "id":"transport-test", "provider_type":"test-protocol",
            "transport":{"path":"/v1/stream"},
            "request":{"message_format":"chat_completions"},
            "capabilities":{"supports_streaming":true, "supports_tools":true},
            "streaming":{"event_type_field":"type", "completed_event":"complete", "failed_event":"failed",
                "chunk":{"text":"delta", "usage":"usage", "finish_reason":"finish_reason",
                    "tool_index":"tool.index", "tool_id":"tool.id", "tool_name":"tool.name", "tool_args_delta":"tool.arguments"}}
        })).unwrap()).unwrap())
    }

    async fn serve(bytes: Vec<u8>) -> (DeclarativeProvider, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/v1/stream",
            post(move || {
                let bytes = bytes.clone();
                async move {
                    // Deliberately split UTF-8 code points and framing delimiters.
                    let chunks = bytes
                        .into_iter()
                        .map(|byte| Ok::<_, std::io::Error>(vec![byte]));
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(futures::stream::iter(chunks)))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1?region=test", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            DeclarativeProvider::new(
                &ProviderConfig {
                    base_url: endpoint,
                    api_key: "".into(),
                    model: "test".into(),
                },
                0.7,
                100,
                None,
                Some(reqwest::Client::builder().no_proxy().build().unwrap()),
                spec(),
                "".into(),
            ),
            server,
        )
    }

    #[tokio::test]
    async fn actual_http_stream_preserves_unicode_tool_deltas_and_completed_usage() {
        let bytes = concat!(
            "data:{\"delta\":\"你好😀\"}\r\n\r\n",
            "data:{\"tool\":{\"index\":2,\"id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"{}\"}}\r\n\r\n",
            "data:{\"type\":\"complete\",\"finish_reason\":\"stop\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}\r\n\r\n",
            "data:{\"delta\":\"must not leak after completion\"}\r\n\r\n"
        ).as_bytes().to_vec();
        let (provider, server) = serve(bytes).await;
        let mut rx = provider.stream_body(json!({})).await.unwrap();
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Text { content }) if content == "你好😀")
        );
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::ToolCallDelta { index:2, name:Some(name), .. }) if name == "read_file")
        );
        assert!(matches!(
            rx.recv().await,
            Some(StreamEvent::Usage {
                input_tokens: 3,
                output_tokens: 2,
                ..
            })
        ));
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Done { finish_reason:Some(reason) }) if reason == "stop")
        );
        assert!(rx.recv().await.is_none());
        assert_eq!(provider.base.circuit_breaker.read().success_count, 1);
        server.abort();
    }

    #[tokio::test]
    async fn http_200_error_event_is_terminal_and_not_followed_by_done() {
        let (provider, server) =
            serve(b"event: failed\ndata:{\"message\":\"overloaded\"}\n\n".to_vec()).await;
        let mut rx = provider.stream_body(json!({})).await.unwrap();
        assert!(matches!(rx.recv().await, Some(StreamEvent::Error { .. })));
        assert!(rx.recv().await.is_none());
        assert_eq!(provider.base.circuit_breaker.read().success_count, 0);
        assert_eq!(provider.base.circuit_breaker.read().failure_count, 1);
        server.abort();
    }
}

impl DeclarativeProvider {
    pub fn new(
        config: &ProviderConfig,
        temperature: f64,
        max_tokens: u32,
        proxy: Option<String>,
        client: Option<reqwest::Client>,
        spec: Arc<CompiledSpec>,
        api_secret: String,
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
            spec,
            instructions: None,
            api_secret,
            tools: Vec::new(),
        }
    }

    pub fn with_instructions(mut self, instructions: Option<String>) -> Self {
        self.instructions = instructions;
        self
    }

    fn spec(&self) -> &CompiledSpec {
        &self.spec
    }

    fn endpoint(&self) -> String {
        let base = self.base.base_url.trim_end_matches('/');
        let transport = &self.spec.spec.transport;
        let path = transport
            .as_ref()
            .and_then(|t| t.path.clone())
            .or_else(|| {
                self.spec.spec.transport.as_ref().and_then(|t| {
                    t.path_template
                        .as_ref()
                        .map(|p| p.replace("{model}", &self.base.model))
                })
            })
            .unwrap_or_else(|| format!("/{}", self.base.model));
        crate::providers::transport::api_endpoint(base, &path, None)
    }

    fn value_from_auth(&self, from: &crate::providers::spec::AuthValueFrom) -> String {
        match from {
            crate::providers::spec::AuthValueFrom::ApiKey => self.base.api_key.clone(),
            crate::providers::spec::AuthValueFrom::ApiSecret => self.api_secret.clone(),
        }
    }

    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let Some(auth) = &self.spec.spec.auth else {
            // 无鉴权声明（本地/无 key）：api_key 非空时才加 Bearer（对齐原生行为）
            if self.base.api_key.is_empty() {
                return builder;
            }
            return builder.bearer_auth(&self.base.api_key);
        };
        match auth {
            crate::providers::spec::ProtocolAuth::Header {
                header_name,
                scheme,
                value_from,
            } => {
                let value = self.value_from_auth(value_from);
                if value.is_empty() {
                    return builder;
                }
                let header = header_name
                    .clone()
                    .filter(|h| !h.is_empty())
                    .unwrap_or_else(|| "Authorization".to_string());
                let full = match scheme {
                    Some(s) if !s.is_empty() => format!("{} {}", s.trim(), value),
                    _ => value,
                };
                builder.header(header, full)
            }
            crate::providers::spec::ProtocolAuth::Query {
                param_name,
                value_from,
            } => {
                let value = self.value_from_auth(value_from);
                if value.is_empty() {
                    return builder;
                }
                builder.query(&[(
                    param_name.clone().unwrap_or_else(|| "key".to_string()),
                    value,
                )])
            }
        }
    }

    /// 工具 schema（按 tools_format 转成 wire 格式；无工具返回 None）。
    fn build_tools_field(&self) -> Option<Value> {
        if self.tools.is_empty() {
            return None;
        }
        let tools = match self.spec.spec.request.tools_format {
            ToolsFormat::ChatCompletions => self
                .tools
                .iter()
                .map(|t| {
                    json!({"type":"function","function":{
                        "name":t.name,"description":t.description,"parameters":t.parameters
                    }})
                })
                .collect(),
            ToolsFormat::Responses => self
                .tools
                .iter()
                .map(|t| {
                    json!({"type":"function","name":t.name,
                    "description":t.description,"parameters":t.parameters})
                })
                .collect(),
            ToolsFormat::Anthropic => self
                .tools
                .iter()
                .map(|t| {
                    json!({"name":t.name,"description":t.description,
                    "input_schema":t.parameters})
                })
                .collect(),
            ToolsFormat::GeminiDeclarations => {
                let declarations: Vec<Value> = self
                    .tools
                    .iter()
                    .map(|t| {
                        json!({"name":t.name,"description":t.description,
                        "parameters":t.parameters})
                    })
                    .collect();
                return Some(json!([{"function_declarations": declarations}]));
            }
        };
        Some(Value::Array(tools))
    }

    /// 递归替换 `{占位符}` 叶子。
    fn substitute(node: &mut Value, f: &dyn Fn(&str) -> Option<Value>) {
        match node {
            Value::Object(map) => {
                for v in map.values_mut() {
                    Self::substitute(v, f);
                }
            }
            Value::Array(arr) => {
                for v in arr.iter_mut() {
                    Self::substitute(v, f);
                }
            }
            Value::String(s) => {
                if let Some(replacement) = f(s) {
                    *node = replacement;
                }
            }
            _ => {}
        }
    }

    fn set_dot_path(root: &mut Value, path: &str, value: Value) {
        let mut parts = path.split('.').filter(|part| !part.is_empty()).peekable();
        let mut current = root;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                current[part] = value;
                return;
            }
            if !current[part].is_object() {
                current[part] = json!({});
            }
            current = &mut current[part];
        }
    }

    /// 构造请求体：骨架替换占位符 + 按 spec 注入搜索 / JSON schema / 推理字段。
    fn build_body(
        &self,
        messages: &[ChatMessage],
        json_schema: &Option<Value>,
        stream: bool,
    ) -> Value {
        let mut body = self
            .spec
            .spec
            .request
            .body
            .clone()
            .unwrap_or_else(|| json!({}));
        let format = self.spec.spec.request.message_format;
        let instructions = if self.spec.spec.request.instructions_field.is_some() {
            crate::providers::base::effective_instructions(&self.instructions)
        } else {
            None
        };
        let messages_arr = message_format::serialize(format, messages, &instructions);

        let set_field = |body: &mut Value, name: &str, value: Value| {
            if !name.is_empty() {
                body[name] = value;
            }
        };
        let messages_field = self
            .spec
            .spec
            .request
            .messages_field
            .clone()
            .unwrap_or_else(|| match format {
                MessageFormat::ChatCompletions => "messages".to_string(),
                MessageFormat::ResponsesInput => "input".to_string(),
                MessageFormat::AnthropicMessages => "messages".to_string(),
                MessageFormat::GeminiContents => "contents".to_string(),
            });
        let body_decl = self.spec.spec.request.body.as_ref();
        let messages_in_body = body_decl
            .map(|b| b.get(&messages_field).is_some() || b.get("messages").is_some())
            .unwrap_or(false);
        if !messages_in_body {
            set_field(&mut body, &messages_field, messages_arr.clone());
        }

        let temperature = self.base.effective_temperature();
        let max_tokens = self.base.effective_max_tokens();
        let enable_search = self.base.is_enable_search();
        let tools = self.build_tools_field();

        Self::substitute(&mut body, &|tok| match tok {
            "{model}" => Some(Value::String(self.base.model.clone())),
            "{messages}" => Some(messages_arr.clone()),
            "{stream}" => Some(Value::Bool(stream)),
            "{temperature}" => Some(json!(temperature)),
            "{max_tokens}" => Some(json!(max_tokens)),
            "{tools}" => Some(tools.clone().unwrap_or_else(|| json!([]))),
            "{enable_search}" => Some(Value::Bool(enable_search)),
            "{instructions}" => instructions
                .clone()
                .map(Value::String)
                .or(Some(Value::Null)),
            "{stream_field}" => Some(Value::String(
                self.spec
                    .spec
                    .request
                    .stream_field
                    .clone()
                    .unwrap_or_else(|| "stream".to_string()),
            )),
            _ => None,
        });

        // 流式开关：按 stream_field 写入（默认 stream）
        let stream_field = self
            .spec
            .spec
            .request
            .stream_field
            .clone()
            .unwrap_or_else(|| "stream".to_string());
        if body_decl
            .map(|b| b.get(&stream_field).is_none())
            .unwrap_or(true)
        {
            set_field(&mut body, &stream_field, Value::Bool(stream));
        }

        let max_tokens_field = self
            .spec
            .spec
            .request
            .max_tokens_field
            .as_deref()
            .unwrap_or("max_tokens");
        if body.get(max_tokens_field).is_none() {
            Self::set_dot_path(&mut body, max_tokens_field, json!(max_tokens));
        }
        if let Some(field) = self.spec.spec.request.instructions_field.as_deref() {
            if let Some(value) = instructions.clone() {
                if body.get(field).is_none() {
                    Self::set_dot_path(&mut body, field, Value::String(value));
                }
            }
        }
        if let Some(tools) = tools.clone() {
            body["tools"] = tools;
        }

        if enable_search {
            let field = self
                .spec
                .spec
                .request
                .search
                .field
                .as_deref()
                .unwrap_or("enable_search");
            match self.spec.spec.request.search.mode {
                SearchMode::None => {}
                SearchMode::BooleanTrue => Self::set_dot_path(&mut body, field, Value::Bool(true)),
                SearchMode::WebSearchOptions => {
                    Self::set_dot_path(&mut body, field, json!({"search_context_size":"high"}))
                }
                SearchMode::ToolsWebSearch => body["tools"] = json!([{"type":"web_search"}]),
                SearchMode::KimiWebSearch => {
                    body["tools"] = json!([{
                        "type":"builtin_function","function":{"name":"$web_search"}
                    }])
                }
                SearchMode::GeminiGoogleSearch => {
                    let tools = body["tools"].as_array_mut();
                    if let Some(tools) = tools {
                        tools.push(json!({"google_search":{}}));
                    } else {
                        body["tools"] = json!([{"google_search":{}}]);
                    }
                }
            }
        }

        // JSON schema 注入（结构化或 JSON Mode）
        self.inject_json_schema(&mut body, json_schema);

        // Legacy dynamic specs declare the protocol shape; model matching stays centralized.
        let customization = self.base.request_customization.read();
        if customization.profile.is_none()
            && !matches!(
                customization.provider_type.as_str(),
                "chat_completions"
                    | "openai"
                    | "openai_responses"
                    | "openai_agents"
                    | "anthropic"
                    | "gemini"
                    | "spark"
                    | "doubao"
                    | "zhipu"
            )
        {
            let protocol = match self.spec.spec.request.reasoning_style {
                ReasoningStyle::ChatCompletions => "chat_completions",
                ReasoningStyle::Responses => "openai_responses",
                ReasoningStyle::None => "",
            };
            crate::providers::reasoning_profiles::apply_legacy_adapter(
                &mut body,
                self.base.effective_reasoning(),
                protocol,
                &self.base.model,
            );
        }
        drop(customization);
        // Dynamic protocols use their declared adapter profile at finalization.

        match self.spec.spec.request.cache.mode {
            CacheMode::None => {}
            CacheMode::PromptCacheKey => {
                let field = self
                    .spec
                    .spec
                    .request
                    .cache
                    .field
                    .as_deref()
                    .unwrap_or("prompt_cache_key");
                let source = crate::utils::messages_cache_key(messages);
                Self::set_dot_path(
                    &mut body,
                    field,
                    Value::String(self.base.get_cache_key(&source)),
                );
            }
            CacheMode::CacheControl => {
                if let Some(system) = body.get_mut("system") {
                    if let Some(blocks) = system.as_array_mut() {
                        if let Some(last) = blocks.last_mut() {
                            last["cache_control"] = json!({"type":"ephemeral"});
                        }
                    } else if let Some(text) = system.as_str().map(str::to_owned) {
                        *system = json!([{"type":"text","text":text,
                            "cache_control":{"type":"ephemeral"}}]);
                    }
                }
            }
        }

        // 省略 temperature 时按 spec 通知后端（默认移除顶层 temperature）
        self.base.strip_temperature(&mut body);

        body
    }

    fn inject_json_schema(&self, body: &mut Value, json_schema: &Option<Value>) {
        let Some(schema) = json_schema else {
            return;
        };
        let req = &self.spec.spec.request;
        match req.json_schema.mode {
            JsonSchemaMode::JsonSchema => {
                let field = req
                    .json_schema
                    .field
                    .as_deref()
                    .unwrap_or("response_format");
                Self::set_dot_path(
                    body,
                    field,
                    json!({
                        "type": "json_schema",
                        "json_schema": { "name": "vivian_response", "schema": schema, "strict": true }
                    }),
                );
            }
            JsonSchemaMode::JsonObject => {
                let field = req
                    .json_schema
                    .field
                    .as_deref()
                    .unwrap_or("response_format");
                Self::set_dot_path(body, field, json!({"type": "json_object"}));
            }
            JsonSchemaMode::ResponsesTextFormat => {
                let field = req.json_schema.field.as_deref().unwrap_or("text.format");
                Self::set_dot_path(
                    body,
                    field,
                    json!({"type":"json_schema",
                    "name":"vivian_response","schema":schema,"strict":true}),
                );
            }
            JsonSchemaMode::GeminiResponseSchema => {
                let field = req
                    .json_schema
                    .field
                    .as_deref()
                    .unwrap_or("generationConfig.responseSchema");
                Self::set_dot_path(
                    body,
                    "generationConfig.responseMimeType",
                    json!("application/json"),
                );
                Self::set_dot_path(body, field, schema.clone());
            }
            JsonSchemaMode::None => {}
        }
    }

    fn request_builder(&self, body: &Value) -> VivianResult<reqwest::RequestBuilder> {
        let transport = self.spec.spec.transport.as_ref();
        let method = transport
            .and_then(|t| t.method.as_deref())
            .unwrap_or("POST")
            .parse::<reqwest::Method>()
            .map_err(|e| VivianError::Provider(format!("声明式协议 HTTP method 无效: {e}")))?;
        let client = self.base.get_client();
        let mut builder = client.request(method, self.endpoint());
        if let Some(transport) = transport {
            let params: Vec<(&str, &str)> = transport
                .query_params
                .iter()
                .map(|p| (p.key.as_str(), p.value.as_str()))
                .collect();
            if !params.is_empty() {
                builder = builder.query(&params);
            }
        }
        Ok(self
            .apply_auth(builder)
            .header("Content-Type", "application/json")
            .json(&self.base.finalize_body(body.clone())))
    }

    async fn send_json(&self, body: Value) -> VivianResult<(u16, Value)> {
        crate::providers::transport::with_retry(&self.base, || async {
            let response = self.request_builder(&body)?.send().await?;
            let status = response.status().as_u16();
            if !response.status().is_success() {
                return Err(crate::providers::transport::http_error(response).await);
            }
            let value = response.json::<Value>().await?;
            crate::providers::transport::validate_json(&value)?;
            Ok((status, value))
        })
        .await
    }

    fn extract_tool_calls(v: &Value) -> Vec<StructuredToolCall> {
        let mut calls = Vec::new();
        let mut push_call = |id: Option<&str>, name: Option<&str>, args: Option<&Value>| {
            let Some(name) = name.filter(|name| !name.is_empty()) else {
                return;
            };
            let arguments = match args {
                Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or_else(|_| json!({})),
                Some(value) => value.clone(),
                None => json!({}),
            };
            calls.push(StructuredToolCall {
                id: id.unwrap_or(name).to_string(),
                name: name.to_string(),
                arguments,
            });
        };
        if let Some(items) = v
            .pointer("/choices/0/message/tool_calls")
            .and_then(Value::as_array)
        {
            for item in items {
                push_call(
                    item["id"].as_str(),
                    item["function"]["name"].as_str(),
                    item["function"].get("arguments"),
                );
            }
        }
        if let Some(items) = v.get("output").and_then(Value::as_array) {
            for item in items.iter().filter(|item| item["type"] == "function_call") {
                push_call(
                    item["call_id"].as_str().or_else(|| item["id"].as_str()),
                    item["name"].as_str(),
                    item.get("arguments"),
                );
            }
        }
        if let Some(items) = v.get("content").and_then(Value::as_array) {
            for item in items.iter().filter(|item| item["type"] == "tool_use") {
                push_call(
                    item["id"].as_str(),
                    item["name"].as_str(),
                    item.get("input"),
                );
            }
        }
        if let Some(parts) = v
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array)
        {
            for part in parts {
                let call = &part["functionCall"];
                push_call(None, call["name"].as_str(), call.get("args"));
            }
        }
        calls
    }

    /// 非流式提取响应：content / reasoning / finish_reason。
    fn extract_response(&self, v: &Value) -> VivianResult<ChatResponse> {
        let content = self
            .spec()
            .resp_content
            .as_ref()
            .and_then(|p| p.first_str(v))
            .unwrap_or_default();
        let content = if self.spec.spec.response.strip_thinking {
            strip_thinking_segments(&content)
        } else {
            content
        };
        let reasoning = self
            .spec()
            .resp_reasoning
            .as_ref()
            .and_then(|p| p.first_str(v))
            .filter(|s| !s.is_empty());
        let finish_reason = self
            .spec()
            .resp_finish
            .as_ref()
            .and_then(|p| p.first_str(v));
        Ok(ChatResponse {
            content,
            tool_calls: Self::extract_tool_calls(v),
            finish_reason,
            reasoning,
            raw: v.clone(),
        })
    }

    fn record_usage(&self, value: &Value) {
        let usage = self
            .spec
            .resp_usage
            .as_ref()
            .and_then(|path| path.first_value(value))
            .or_else(|| value.get("usage").cloned());
        if let Some(StreamEvent::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        }) = usage.as_ref().and_then(parse_stream_usage)
        {
            usage_store::record_usage(
                &self.base.model,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
            );
        }
    }

    fn clone_with_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        Self {
            base: self.base.fork(),
            spec: self.spec.clone(),
            instructions: self.instructions.clone(),
            api_secret: self.api_secret.clone(),
            tools,
        }
    }

    fn decode_stream_events(
        spec: &CompiledSpec,
        event: &crate::providers::sse::SseEvent,
    ) -> VivianResult<Vec<StreamEvent>> {
        let Some(value) = event.json.as_ref() else {
            return Ok(Vec::new());
        };
        let cfg = spec.spec.streaming.as_ref().expect("streaming checked");
        if cfg
            .event_type_field
            .as_deref()
            .and_then(|field| value.get(field))
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .zip(cfg.failed_event.as_deref())
            .is_some_and(|(actual, failed)| actual == failed)
        {
            return Err(VivianError::Provider(value.to_string()));
        }
        let mut events = Vec::new();
        if let Some(content) = spec
            .chunk
            .text
            .as_ref()
            .and_then(|path| path.first_str(value))
            .filter(|s| !s.is_empty())
        {
            events.push(StreamEvent::Text { content });
        }
        if let Some(content) = spec
            .chunk
            .reasoning
            .as_ref()
            .and_then(|path| path.first_str(value))
            .filter(|s| !s.is_empty())
        {
            events.push(StreamEvent::Thinking { content });
        }
        let name = spec
            .chunk
            .tool_name
            .as_ref()
            .and_then(|path| path.first_str(value));
        let id = spec
            .chunk
            .tool_id
            .as_ref()
            .and_then(|path| path.first_str(value));
        let arguments_delta = spec
            .chunk
            .tool_args_delta
            .as_ref()
            .and_then(|path| path.first_str(value));
        if name.is_some() || id.is_some() || arguments_delta.is_some() {
            let index = spec
                .chunk
                .tool_index
                .as_ref()
                .and_then(|path| path.first_value(value))
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(0) as usize;
            events.push(StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            });
        }
        if let Some(usage) = spec
            .chunk
            .usage
            .as_ref()
            .and_then(|path| path.first_value(value))
            .as_ref()
            .and_then(parse_stream_usage)
        {
            events.push(usage);
        }
        Ok(events)
    }

    async fn stream_body(&self, body: Value) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        use crate::providers::transport::RequestGuard;
        let guard = RequestGuard::begin(&self.base)?;
        let (response, guard) = guard
            .response(self.request_builder(&body)?.send().await)
            .await?;
        let spec = self.spec.clone();
        let cfg = spec.spec.streaming.clone().expect("streaming checked");
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = crate::providers::sse::event_stream(
                response.bytes_stream(),
                cfg.event_path.clone(),
                Default::default(),
            );
            let mut finish_reason = None;
            let mut useful = false;
            loop {
                let event = tokio::select! {
                    _ = tx.closed() => return,
                    event = stream.next() => event,
                };
                let mut completed = false;
                let events = match event {
                    Some(Ok(event))
                        if cfg
                            .done_sentinel
                            .as_deref()
                            .is_some_and(|sentinel| event.data.trim() == sentinel) =>
                    {
                        break
                    }
                    Some(Ok(event)) => {
                        if let Some(value) = event.json.as_ref() {
                            finish_reason = spec
                                .chunk
                                .finish_reason
                                .as_ref()
                                .and_then(|path| path.first_str(value))
                                .or(finish_reason);
                            completed = cfg
                                .event_type_field
                                .as_deref()
                                .and_then(|field| value.get(field))
                                .and_then(Value::as_str)
                                .or(event.event.as_deref())
                                .zip(cfg.completed_event.as_deref())
                                .is_some_and(|(actual, expected)| actual == expected);
                        }
                        Self::decode_stream_events(&spec, &event)
                    }
                    Some(Err(message)) => Err(VivianError::Provider(message)),
                    None => break,
                };
                match events {
                    Ok(events) => {
                        for event in events {
                            useful |= matches!(
                                &event,
                                StreamEvent::Text { .. }
                                    | StreamEvent::Thinking { .. }
                                    | StreamEvent::ToolCallDelta { .. }
                            );
                            if tx.send(event).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        guard.failure(&error);
                        let _ = tx.send(StreamEvent::Error { message }).await;
                        return;
                    }
                }
                if completed {
                    break;
                }
            }
            if !useful {
                let error = VivianError::Provider("Provider returned an empty stream".into());
                let message = error.to_string();
                guard.failure(&error);
                let _ = tx.send(StreamEvent::Error { message }).await;
                return;
            }
            guard.success();
            let _ = tx.send(StreamEvent::Done { finish_reason }).await;
        });
        Ok(rx)
    }

    async fn stream_bound_tools(
        &self,
        messages: Vec<ChatMessage>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        let schema = ProviderCallOptions::current_json_schema();
        self.stream_body(self.build_body(&messages, &schema, true))
            .await
    }
}

#[async_trait]
impl BaseProvider for DeclarativeProvider {
    fn set_request_customization(
        &self,
        customization: crate::providers::reasoning_profiles::RequestCustomization,
    ) {
        *self.base.request_customization.write() = customization;
    }

    fn set_request_parameters(&self, temperature: bool, max_tokens: bool) {
        self.base
            .send_temperature
            .store(temperature, std::sync::atomic::Ordering::Relaxed);
        self.base
            .send_max_tokens
            .store(max_tokens, std::sync::atomic::Ordering::Relaxed);
    }

    fn get_model(&self) -> &str {
        &self.base.model
    }

    fn get_endpoint(&self) -> &str {
        &self.base.base_url
    }

    fn get_circuit_breaker_stats(&self) -> Value {
        let s = self.base.get_stats();
        json!({"model": s.model, "total_calls": s.total_calls,
               "successful_calls": s.successful_calls, "failed_calls": s.failed_calls})
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

    fn provider_identity(&self) -> String {
        format!(
            "{}@{}#{}",
            self.base.model, self.base.base_url, self.spec.spec.id
        )
    }

    fn supports_native_function_calling(&self) -> bool {
        self.spec.spec.capabilities.supports_tools
    }
    fn supports_structured_output(&self) -> bool {
        self.spec.spec.capabilities.supports_structured_output
    }
    fn supports_json_mode(&self) -> bool {
        self.spec.spec.capabilities.supports_json_mode
    }

    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        let schema = ProviderCallOptions::current_json_schema();
        let body = self.build_body(&messages, &schema, false);
        let cache_key = body.to_string();
        if let Some(cached) = self.base.get_cached_response(&cache_key) {
            return Ok(cached);
        }
        let (_, v) = self.send_json(body).await?;
        let resp = self.extract_response(&v)?;
        if resp.content.is_empty() && resp.tool_calls.is_empty() {
            return Err(VivianError::Provider(
                "声明式协议响应为空（内容路径未命中）".to_string(),
            ));
        }
        self.record_usage(&v);
        self.base.cache_response(&cache_key, &resp.content);
        Ok(resp.content)
    }

    async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> {
        let schema = ProviderCallOptions::current_json_schema();
        let body = self.build_body(&messages, &schema, false);
        let (_, v) = self.send_json(body).await?;
        self.extract_response(&v)
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> VivianResult<Box<dyn BaseProvider>> {
        if !self.spec.spec.capabilities.supports_tools {
            return Err(VivianError::NotImplemented(format!(
                "声明式协议 {} 未声明工具能力",
                self.spec.spec.id
            )));
        }
        Ok(Box::new(self.clone_with_tools(tools)))
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        json_schema: Option<Value>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        if !self.spec.spec.capabilities.supports_streaming || self.spec.spec.streaming.is_none() {
            return Err(VivianError::NotImplemented(format!(
                "声明式协议 {} 未声明流式配置",
                self.spec.spec.id
            )));
        }
        self.stream_body(self.build_body(&messages, &json_schema, true))
            .await
    }

    async fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        if !self.spec.spec.capabilities.supports_tools
            || !self.spec.spec.capabilities.supports_streaming
            || self.spec.spec.streaming.is_none()
        {
            return Err(VivianError::NotImplemented(format!(
                "声明式协议 {} 不支持流式工具调用",
                self.spec.spec.id
            )));
        }
        self.clone_with_tools(tools)
            .stream_bound_tools(messages)
            .await
    }

    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        json_schema: Option<Value>,
    ) -> VivianResult<String> {
        scope_provider_call(
            ProviderCallOptions {
                enable_search: Some(enable_search),
                ..ProviderCallOptions::default()
            },
            async {
                let body = self.build_body(&messages, &json_schema, false);
                let cache_key = body.to_string();
                if let Some(cached) = self.base.get_cached_response(&cache_key) {
                    return Ok(cached);
                }
                let (_, v) = self.send_json(body).await?;
                let resp = self.extract_response(&v)?;
                self.record_usage(&v);
                self.base.cache_response(&cache_key, &resp.content);
                Ok(resp.content)
            },
        )
        .await
    }
}
