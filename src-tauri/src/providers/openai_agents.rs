//! Agents API adapter for Vivian's locally owned conversation and tool loop.
//!
//! A managed session lasts through one turn (including local function calls).
//! Tool IDs carry the session/turn/call reference across provider rebinding. The
//! next user turn starts from Vivian's current context; completed sessions are
//! deleted. No hosted shell is enabled, so tools still use Vivian's permissions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::StreamExt;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::config::manager::ProviderConfig;
use crate::error::{VivianError, VivianResult};
use crate::providers::base::{
    parse_stream_usage, scope_provider_call, BaseProvider, ChatResponse, ProviderBase,
    ProviderCallOptions, StreamEvent, StructuredToolCall, ToolDefinition,
};
use crate::providers::reasoning::ReasoningPreference;
use crate::types::response::ChatMessage;

const CALL_PREFIX: &str = "vivian_agents_";

#[derive(Clone)]
pub struct OpenAiAgentsProvider {
    base: Arc<ProviderBase>,
    tools: Vec<ToolDefinition>,
    instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallReference {
    session: String,
    turn: String,
    call: String,
}

impl CallReference {
    fn encode(&self) -> String {
        format!(
            "{CALL_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap())
        )
    }

    fn decode(id: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(id.strip_prefix(CALL_PREFIX)?).ok()?;
        let reference: Self = serde_json::from_slice(&bytes).ok()?;
        // These IDs become URL path segments. Never accept arbitrary paths from history.
        [&reference.session, &reference.turn, &reference.call]
            .iter()
            .all(|id| {
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            })
            .then_some(reference)
    }
}

impl OpenAiAgentsProvider {
    pub fn new(
        config: &ProviderConfig,
        temperature: f64,
        max_tokens: u32,
        proxy: Option<String>,
        client: Option<reqwest::Client>,
    ) -> Self {
        Self {
            base: Arc::new(ProviderBase {
                proxy,
                client,
                ..ProviderBase::new(
                    config.api_key.clone(),
                    config.base_url.clone(),
                    config.model.clone(),
                    temperature,
                    max_tokens,
                )
            }),
            tools: Vec::new(),
            instructions: None,
        }
    }

    pub fn with_instructions(mut self, instructions: Option<String>) -> Self {
        self.instructions = instructions;
        self
    }

    fn sessions_url(&self) -> String {
        let base = self.base.base_url.trim_end_matches('/');
        if base.ends_with("/agents/sessions") {
            base.to_string()
        } else {
            format!("{base}/agents/sessions")
        }
    }

    fn request(&self, method: Method, suffix: &str) -> reqwest::RequestBuilder {
        self.base
            .get_client()
            .request(method, format!("{}{suffix}", self.sessions_url()))
            .timeout(Duration::from_secs(600))
            .bearer_auth(&self.base.api_key)
            .header("OpenAI-Beta", "agents=v1")
    }

    async fn checked(response: reqwest::Response) -> VivianResult<reqwest::Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        Err(crate::providers::transport::http_error(response).await)
    }

    async fn get_json(&self, suffix: &str) -> VivianResult<Value> {
        let response = self
            .request(Method::GET, suffix)
            .send()
            .await
            .map_err(|e| VivianError::Network(e.to_string()))?;
        Self::checked(response)
            .await?
            .json()
            .await
            .map_err(|e| VivianError::Network(e.to_string()))
    }

    fn create_body(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        schema: Option<Value>,
    ) -> Value {
        let mut instructions =
            crate::providers::base::effective_instructions(&self.instructions).unwrap_or_default();
        let mut history = Vec::new();
        let mut content = Vec::new();
        for message in messages {
            if message.role == "system" || message.role == "developer" {
                instructions.push('\n');
                instructions.push_str(&message.content);
            } else {
                // Agents input only accepts user messages. Preserve role and tool
                // provenance in a transcript rather than inventing unsupported roles.
                history.push(json!({"role": message.role, "content": message.content,
                    "tool_calls": message.tool_calls, "tool_call_id": message.tool_call_id}));
                for image in message.images.iter().flatten() {
                    let url = if !image.data.is_empty() {
                        format!("data:{};base64,{}", image.media_type, image.data)
                    } else {
                        image.url.clone().unwrap_or_default()
                    };
                    if !url.is_empty() {
                        content.push(json!({"type": "input_image", "image_url": url,
                            "detail": image.detail.as_deref().unwrap_or("auto")}));
                    }
                }
            }
        }
        content.insert(0, json!({"type": "input_text", "text": format!(
            "Continue the following conversation. Answer the latest user request; earlier tool results are historical context.\n{}",
            serde_json::to_string(&history).unwrap()
        )}));
        let mut agent_tools: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function", "name": tool.name, "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();
        if self.base.is_enable_search() {
            agent_tools.push(json!({"type": "web_search"}));
        }
        let mut agent = json!({"model": self.base.model, "instructions": instructions,
            "tools": agent_tools, "multi_agent": {"enabled": false}});
        // Reasoning fields are applied once by the final request adapter.
        if let Some(schema) = schema {
            agent["text"] = json!({"format": {"type": "json_schema", "name": "response", "strict": true, "schema": schema}});
        }
        // Agents does not accept Chat/Responses sampling or max_output_tokens fields.
        self.base.finalize_body(json!({"agent": agent, "environment": {"type": "none"},
            "input": [{"role": "user", "content": content}], "stream": true}))
    }

    fn continuation(messages: &[ChatMessage]) -> VivianResult<Option<(CallReference, Vec<Value>)>> {
        // Only the latest assistant tool batch can resume a pending managed turn.
        // Older encoded IDs in a completed conversation must never resurrect it.
        let Some(start) = messages.iter().rposition(|m| m.role == "assistant") else {
            return Ok(None);
        };
        let calls = messages[start].tool_calls.as_deref().unwrap_or_default();
        let Some(reference) = calls
            .first()
            .and_then(|call| CallReference::decode(&call.id))
        else {
            return Ok(None);
        };
        let mut events = Vec::new();
        for call in calls {
            let current = CallReference::decode(&call.id)
                .ok_or_else(|| VivianError::Provider("Agents API 工具调用引用无效".into()))?;
            if current.session != reference.session || current.turn != reference.turn {
                return Err(VivianError::Provider(
                    "Agents API 工具结果属于不同会话".into(),
                ));
            }
            let Some(result) = messages[start + 1..]
                .iter()
                .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some(&call.id))
            else {
                return Err(VivianError::Provider(format!(
                    "Agents API 缺少工具结果：{}",
                    call.name
                )));
            };
            events.push(
                json!({"type": "agent.session.input.tool_result", "turn_id": current.turn,
                "call_id": current.call, "success": true, "output": result.content}),
            );
        }
        for message in &messages[start + 1..] {
            if message.role == "user" {
                events.push(
                    json!({"type": "agent.session.input.message", "input": [{"role": "user",
                    "content": [{"type": "input_text", "text": message.content}]}]}),
                );
            }
        }
        Ok(Some((reference, events)))
    }

    async fn cleanup(&self, session: &str, cancel: bool) {
        if cancel {
            let _ = self
                .request(Method::POST, &format!("/{session}/events"))
                .timeout(Duration::from_secs(5))
                .json(&json!({"events": [{"type": "agent.session.input.cancel"}]}))
                .send()
                .await;
        }
        match self
            .request(Method::DELETE, &format!("/{session}"))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {
            }
            _ => tracing::warn!("[Agents API] 无法清理会话 {session}"),
        }
    }

    async fn send_actions(
        &self,
        session: &str,
        actions: &Value,
        tools: &[ToolDefinition],
        tx: &mpsc::Sender<StreamEvent>,
    ) -> VivianResult<()> {
        let actions = actions
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| {
                VivianError::Provider("Agents API 等待输入，但没有可执行的函数调用".into())
            })?;
        for (index, action) in actions.iter().enumerate() {
            let name = action["name"].as_str().unwrap_or_default();
            if action["type"] != "function_call" || !tools.iter().any(|tool| tool.name == name) {
                return Err(VivianError::Provider(format!(
                    "Agents API 请求了未注册的工具：{name}"
                )));
            }
            let reference = CallReference {
                session: session.to_string(),
                turn: action["turn_id"].as_str().unwrap_or_default().to_string(),
                call: action["call_id"].as_str().unwrap_or_default().to_string(),
            };
            if CallReference::decode(&reference.encode()).is_none() {
                return Err(VivianError::Provider(
                    "Agents API 返回了无效的调用 ID".into(),
                ));
            }
            let arguments = if let Some(raw) = action["arguments"].as_str() {
                serde_json::from_str::<Value>(raw)?
            } else {
                action["arguments"].clone()
            };
            tx.send(StreamEvent::ToolCallDelta {
                index,
                id: Some(reference.encode()),
                name: Some(name.to_string()),
                arguments_delta: Some(serde_json::to_string(&arguments)?),
            })
            .await
            .map_err(|_| VivianError::Provider("Agents API 请求已取消".into()))?;
        }
        Ok(())
    }

    async fn saved_text(&self, session: &str, turn: &str) -> VivianResult<Vec<(String, String)>> {
        let mut suffix = format!("/{session}/items?order=asc&limit=100");
        let mut result = Vec::new();
        loop {
            let page = self.get_json(&suffix).await?;
            let items = page["data"]
                .as_array()
                .ok_or_else(|| VivianError::Provider("Agents API items 响应无效".into()))?;
            for item in items {
                if item["type"] != "message"
                    || item["role"] != "assistant"
                    || item["turn_id"] != turn
                {
                    continue;
                }
                for (index, part) in item["content"].as_array().into_iter().flatten().enumerate() {
                    if let Some(value) = part["text"]
                        .as_str()
                        .filter(|_| part["type"] == "output_text")
                    {
                        result.push((
                            format!("{}:{index}", item["id"].as_str().unwrap_or_default()),
                            crate::providers::web_citations::attach(
                                value,
                                &crate::providers::web_citations::sources(part),
                            ),
                        ));
                    }
                }
            }
            if page["has_more"] != true {
                return Ok(result);
            }
            let cursor = page["last_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| VivianError::Provider("Agents API items 缺少分页游标".into()))?;
            suffix = format!(
                "/{session}/items?order=asc&limit=100&after={}",
                urlencoding::encode(cursor)
            );
        }
    }

    async fn saved_output(
        &self,
        session: &str,
        turn: &str,
        text: &mut HashMap<String, String>,
        tx: &mpsc::Sender<StreamEvent>,
    ) -> VivianResult<()> {
        for (key, value) in self.saved_text(session, turn).await? {
            emit_text(text, key, &value, false, tx).await?;
        }
        Ok(())
    }

    async fn run(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
        schema: Option<Value>,
        tx: &mpsc::Sender<StreamEvent>,
        session: &mut Option<String>,
    ) -> VivianResult<bool> {
        let continuation = Self::continuation(&messages)?;
        let mut turn = String::new();
        let mut text = HashMap::new();
        let response = if let Some((reference, mut events)) = continuation {
            *session = Some(reference.session.clone());
            turn = reference.turn;
            let state = self.get_json(&format!("/{}", reference.session)).await?;
            if state["status"] == "requires_action" {
                // Suppress commentary already returned with the preceding tool batch.
                text.extend(self.saved_text(&reference.session, &turn).await?);
            }
            // Reconnect before submitting results. Never blindly resubmit completed actions.
            let response = self
                .request(
                    Method::GET,
                    &format!("/{}/events?stream=true", reference.session),
                )
                .header("Accept", "text/event-stream")
                .send()
                .await
                .map_err(|e| VivianError::Network(e.to_string()))?;
            let response = Self::checked(response).await?;
            if let Some(pending) = state["required_actions"].as_array() {
                events.retain(|event| {
                    event["type"] != "agent.session.input.tool_result"
                        || pending.iter().any(|a| {
                            a["turn_id"] == event["turn_id"] && a["call_id"] == event["call_id"]
                        })
                });
            } else {
                events.retain(|event| event["type"] != "agent.session.input.tool_result");
            }
            if !events.is_empty() {
                let result = self
                    .request(Method::POST, &format!("/{}/events", reference.session))
                    .json(&json!({"events": events}))
                    .send()
                    .await
                    .map_err(|e| VivianError::Network(e.to_string()))?;
                Self::checked(result).await?;
            }
            // A previous delivery may already have completed the turn.
            if state["status"] == "idle" {
                let saved = self
                    .get_json(&format!("/{}/turns/{turn}", reference.session))
                    .await?;
                if saved["status"] == "completed" {
                    self.saved_output(&reference.session, &turn, &mut HashMap::new(), tx)
                        .await?;
                    if let Some(usage) = parse_stream_usage(&saved["usage"]) {
                        let _ = tx.send(usage).await;
                    }
                    return Ok(false);
                }
            }
            response
        } else {
            let response = self
                .request(Method::POST, "")
                .header("Accept", "text/event-stream")
                .json(&self.create_body(&messages, &tools, schema))
                .send()
                .await
                .map_err(|e| VivianError::Network(e.to_string()))?;
            Self::checked(response).await?
        };
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut frame = String::new();
        loop {
            let chunk = tokio::select! {
                _ = tx.closed() => return Err(VivianError::Provider("Agents API 请求已取消".into())),
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = chunk else {
                break;
            };
            buffer.extend_from_slice(&chunk.map_err(|e| VivianError::Network(e.to_string()))?);
            while let Some(end) = buffer.iter().position(|c| *c == b'\n') {
                let line: Vec<u8> = buffer.drain(..=end).collect();
                let line = std::str::from_utf8(&line)
                    .map_err(|e| VivianError::Provider(e.to_string()))?
                    .trim_end_matches(['\r', '\n']);
                if let Some(data) = line.strip_prefix("data:") {
                    if !frame.is_empty() {
                        frame.push('\n');
                    }
                    frame.push_str(data.trim_start());
                } else if line.is_empty() && !frame.is_empty() {
                    let event: Value = serde_json::from_str(&std::mem::take(&mut frame))?;
                    let sources = crate::providers::web_citations::sources(&event);
                    if !sources.is_empty() {
                        let _ = tx.send(StreamEvent::WebSources { sources }).await;
                    }
                    if let Some(id) = event["session"]["id"]
                        .as_str()
                        .or_else(|| event["session_id"].as_str())
                    {
                        // Validate before using an ID in request URLs.
                        let reference = CallReference {
                            session: id.into(),
                            turn: "check".into(),
                            call: "check".into(),
                        };
                        if CallReference::decode(&reference.encode()).is_none() {
                            return Err(VivianError::Provider("Agents API session ID 无效".into()));
                        }
                        *session = Some(id.to_string());
                    }
                    if !event["subagent_id"].is_null() || !event["turn"]["subagent_id"].is_null() {
                        continue;
                    }
                    if let Some(id) = event["turn"]["id"]
                        .as_str()
                        .or_else(|| event["turn_id"].as_str())
                    {
                        turn = id.to_string();
                    }
                    let kind = event["type"].as_str().unwrap_or_default();
                    match kind {
                        "agent.session.turn.output_text.delta"
                        | "agent.session.turn.output_text.done" => {
                            let delta = kind.ends_with(".delta");
                            let value = event[if delta { "delta" } else { "text" }]
                                .as_str()
                                .unwrap_or_default();
                            let key = format!(
                                "{}:{}",
                                event["item_id"].as_str().unwrap_or_default(),
                                event["content_index"].as_u64().unwrap_or(0)
                            );
                            emit_text(&mut text, key, value, delta, tx).await?;
                        }
                        "agent.session.requires_action" => {
                            let id = session.as_deref().ok_or_else(|| {
                                VivianError::Provider("Agents API 缺少 session ID".into())
                            })?;
                            let state = self.get_json(&format!("/{id}")).await?;
                            self.send_actions(id, &state["required_actions"], &tools, tx)
                                .await?;
                            return Ok(true);
                        }
                        "agent.session.turn.completed" => {
                            let id = session.as_deref().ok_or_else(|| {
                                VivianError::Provider("Agents API 缺少 session ID".into())
                            })?;
                            self.saved_output(id, &turn, &mut text, tx).await?;
                            if let Some(usage) = parse_stream_usage(&event["turn"]["usage"]) {
                                let _ = tx.send(usage).await;
                            }
                            return Ok(false);
                        }
                        "error"
                        | "agent.session.failed"
                        | "agent.session.environment.failed"
                        | "agent.session.turn.failed"
                        | "agent.session.turn.cancelled" => {
                            let detail = event["turn"]["error"]["message"]
                                .as_str()
                                .or_else(|| event["error"]["message"].as_str())
                                .or_else(|| event["session"]["error"]["message"].as_str())
                                .unwrap_or(kind);
                            return Err(VivianError::Provider(format!("Agents API: {detail}")));
                        }
                        _ => {}
                    }
                }
            }
        }
        // A disconnected stream is not a successful completion. Recover saved state
        // without starting another task or rerunning tools that may have side effects.
        let id = session
            .as_deref()
            .ok_or_else(|| VivianError::Provider("Agents API 流中断，未收到 session ID".into()))?;
        let state = self.get_json(&format!("/{id}")).await?;
        if state["status"] == "requires_action" {
            self.send_actions(id, &state["required_actions"], &tools, tx)
                .await?;
            return Ok(true);
        }
        if !turn.is_empty() {
            let saved = self.get_json(&format!("/{id}/turns/{turn}")).await?;
            if saved["status"] == "completed" {
                self.saved_output(id, &turn, &mut text, tx).await?;
                if let Some(usage) = parse_stream_usage(&saved["usage"]) {
                    let _ = tx.send(usage).await;
                }
                return Ok(false);
            }
        }
        Err(VivianError::Provider(
            "Agents API 流中断，任务尚未完成".into(),
        ))
    }

    fn start_stream(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
        schema: Option<Value>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        let guard = crate::providers::transport::RequestGuard::begin(&self.base)?;
        let provider = self.clone();
        let options = ProviderCallOptions::current();
        let (tx, rx) = mpsc::channel(128);
        tokio::spawn(scope_provider_call(options, async move {
            let mut session = None;
            let outcome = tokio::select! {
                result = tokio::time::timeout(Duration::from_secs(600), provider.run(messages, tools, schema, &tx, &mut session)) => result,
                _ = tx.closed() => {
                    drop(guard);
                    if let Some(id) = session.as_deref() { provider.cleanup(id, true).await; }
                    return;
                },
            };
            match outcome {
                Ok(Ok(pending)) => {
                    guard.success();
                    if !pending {
                        if let Some(id) = session.as_deref() {
                            provider.cleanup(id, false).await;
                        }
                    }
                    let _ = tx
                        .send(StreamEvent::Done {
                            finish_reason: Some(if pending { "tool_calls" } else { "stop" }.into()),
                        })
                        .await;
                }
                error => {
                    let error = match error {
                        Ok(Err(error)) => error,
                        _ => VivianError::Timeout("Agents API 请求超时".into()),
                    };
                    guard.failure(&error);
                    if let Some(id) = session.as_deref() {
                        provider.cleanup(id, true).await;
                    }
                    let message = error.to_string();
                    let _ = tx.send(StreamEvent::Error { message }).await;
                }
            }
        }));
        Ok(rx)
    }
}

async fn emit_text(
    text: &mut HashMap<String, String>,
    key: String,
    value: &str,
    delta: bool,
    tx: &mpsc::Sender<StreamEvent>,
) -> VivianResult<()> {
    let seen = text.entry(key).or_default();
    let addition = if delta {
        value
    } else {
        value
            .strip_prefix(seen.as_str())
            .ok_or_else(|| VivianError::Provider("Agents API 已保存文本与流式文本不一致".into()))?
    };
    if !addition.is_empty() {
        tx.send(StreamEvent::Text {
            content: addition.to_string(),
        })
        .await
        .map_err(|_| VivianError::Provider("Agents API 请求已取消".into()))?;
        seen.push_str(addition);
    }
    Ok(())
}

#[async_trait]
impl BaseProvider for OpenAiAgentsProvider {
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

    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        Ok(self.invoke(messages).await?.content)
    }

    async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> {
        let mut stream = self.start_stream(
            messages,
            self.tools.clone(),
            ProviderCallOptions::current_json_schema(),
        )?;
        let mut response = ChatResponse::from_text(String::new());
        let mut usage = crate::providers::usage_store::StreamUsageAccumulator::default();
        let mut web_sources = Vec::new();
        while let Some(event) = stream.recv().await {
            match event {
                StreamEvent::Text { content } => response.content.push_str(&content),
                StreamEvent::WebSources { sources } => web_sources.extend(sources),
                StreamEvent::ToolCallDelta {
                    id,
                    name,
                    arguments_delta,
                    ..
                } => response.tool_calls.push(StructuredToolCall {
                    id: id.unwrap_or_default(),
                    name: name.unwrap_or_default(),
                    arguments: serde_json::from_str(arguments_delta.as_deref().unwrap_or("{}"))?,
                }),
                StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                } => {
                    usage.observe(
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                    );
                }
                StreamEvent::Done { finish_reason } => {
                    usage.record_current(&self.base.model);
                    response.content =
                        crate::providers::web_citations::attach(&response.content, &web_sources);
                    response.finish_reason = finish_reason;
                    return Ok(response);
                }
                StreamEvent::Error { message } => {
                    usage.record_current(&self.base.model);
                    return Err(VivianError::Provider(message));
                }
                _ => {}
            }
        }
        usage.record_current(&self.base.model);
        Err(VivianError::Provider("Agents API 响应未完成".into()))
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        json_schema: Option<Value>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        self.start_stream(
            messages,
            self.tools.clone(),
            json_schema.or_else(ProviderCallOptions::current_json_schema),
        )
    }

    async fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        self.start_stream(messages, tools, ProviderCallOptions::current_json_schema())
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> VivianResult<Box<dyn BaseProvider>> {
        Ok(Box::new(Self {
            tools,
            ..self.clone()
        }))
    }

    fn get_model(&self) -> &str {
        &self.base.model
    }

    fn get_endpoint(&self) -> &str {
        &self.base.base_url
    }
    fn provider_identity(&self) -> String {
        format!("openai_agents:{}@{}", self.base.model, self.base.base_url)
    }
    fn get_circuit_breaker_stats(&self) -> Value {
        serde_json::to_value(self.base.get_stats()).unwrap_or(Value::Null)
    }
    fn supports_native_function_calling(&self) -> bool {
        true
    }
    fn supports_structured_output(&self) -> bool {
        true
    }
    fn set_enable_search(&self, enable: bool) {
        self.base.set_enable_search(enable);
    }
    fn set_temperature_override(&self, temp: Option<f64>) {
        self.base.set_temperature_override(temp);
    }
    fn set_omit_temperature(&self, omit: bool) {
        self.base.set_omit_temperature(omit);
    }
    fn set_reasoning_pref(&self, pref: Option<ReasoningPreference>) {
        self.base.set_reasoning_pref(pref);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::response::MessageToolCall;
    use axum::{
        body::Body,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::Response,
        routing::{get, post},
        Json, Router,
    };
    use parking_lot::Mutex;

    #[derive(Default)]
    struct MockState {
        requests: Mutex<Vec<Value>>,
        deletes: Mutex<usize>,
        completed: Mutex<bool>,
    }

    fn function() -> ToolDefinition {
        ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        }
    }

    fn actions() -> Value {
        json!([{"type": "function_call", "turn_id": "turn_1", "call_id": "call_1",
            "name": "read_file", "arguments": {"path": "README.md"}}])
    }

    fn sse(events: Vec<Value>) -> Response {
        let bytes = events
            .into_iter()
            .map(|event| format!("data: {event}\r\n\r\n"))
            .collect::<String>()
            .into_bytes();
        // Single-byte chunks deliberately split UTF-8 codepoints and SSE frames.
        let chunks = bytes
            .into_iter()
            .map(|byte| Ok::<_, std::io::Error>(vec![byte]));
        Response::builder()
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(futures::stream::iter(chunks)))
            .unwrap()
    }

    async fn create(
        State(state): State<Arc<MockState>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        assert_eq!(headers["openai-beta"], "agents=v1");
        assert_eq!(body["environment"]["type"], "none");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["agent"]["tools"][0]["name"], "read_file");
        assert!(body["agent"].get("max_output_tokens").is_none());
        state.requests.lock().push(body);
        sse(vec![
            json!({"type": "agent.session.created", "session": {"id": "session_1"}}),
            json!({"type": "agent.session.turn.output_text.delta", "turn_id": "turn_1", "item_id": "msg_1", "content_index": 0, "delta": "正在读取"}),
            json!({"type": "agent.session.turn.output_text.done", "item_id": "msg_1", "content_index": 0, "text": "正在读取"}),
            json!({"type": "agent.session.requires_action", "session_id": "session_1"}),
        ])
    }

    async fn state(State(state): State<Arc<MockState>>) -> Json<Value> {
        Json(
            json!({"id": "session_1", "status": if *state.completed.lock() { "idle" } else { "requires_action" }, "required_actions": actions()}),
        )
    }

    async fn submit(State(state): State<Arc<MockState>>, Json(body): Json<Value>) -> StatusCode {
        assert_eq!(body["events"][0]["type"], "agent.session.input.tool_result");
        assert_eq!(body["events"][0]["turn_id"], "turn_1");
        assert_eq!(body["events"][0]["call_id"], "call_1");
        assert_eq!(body["events"][0]["output"], "file contents");
        state.requests.lock().push(body);
        *state.completed.lock() = true;
        StatusCode::ACCEPTED
    }

    async fn events() -> Response {
        sse(vec![
            json!({"type": "agent.session.idle"}),
            // Subagent completions cannot terminate the root stream.
            json!({"type": "agent.session.turn.completed", "turn": {"id": "child_1", "subagent_id": "agent_1"}}),
            json!({"type": "agent.session.turn.output_text.done", "turn_id": "turn_1", "item_id": "msg_2", "content_index": 0, "text": "读取完成✅"}),
            json!({"type": "agent.session.turn.completed", "turn": {"id": "turn_1", "subagent_id": null,
                "usage": {"input_tokens": 12, "output_tokens": 8}}}),
        ])
    }

    async fn items(State(state): State<Arc<MockState>>) -> Json<Value> {
        let mut items = vec![
            json!({"type": "message", "role": "assistant", "id": "msg_1", "turn_id": "turn_1",
            "content": [{"type": "output_text", "text": "正在读取"}]}),
        ];
        if *state.completed.lock() {
            items.push(
                json!({"type": "message", "role": "assistant", "id": "msg_2", "turn_id": "turn_1",
                "content": [{"type": "output_text", "text": "读取完成✅"}]}),
            );
        }
        Json(json!({"data": items, "has_more": false}))
    }

    async fn delete(State(state): State<Arc<MockState>>) -> Json<Value> {
        *state.deletes.lock() += 1;
        Json(json!({"id": "session_1", "deleted": true}))
    }

    fn provider(url: &str) -> OpenAiAgentsProvider {
        OpenAiAgentsProvider::new(
            &ProviderConfig {
                base_url: url.into(),
                api_key: "test-key".into(),
                model: "gpt-5.5".into(),
            },
            0.7,
            100,
            None,
            Some(reqwest::Client::builder().no_proxy().build().unwrap()),
        )
    }

    #[tokio::test]
    async fn managed_function_round_trip_preserves_utf8_and_does_not_repeat_commentary() {
        let state = Arc::new(MockState::default());
        let app = Router::new()
            .route("/v1/agents/sessions", post(create))
            .route("/v1/agents/sessions/:id", get(self::state).delete(delete))
            .route("/v1/agents/sessions/:id/events", get(events).post(submit))
            .route("/v1/agents/sessions/:id/items", get(items))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = provider(&url);
        let first = provider
            .bind_tools(vec![function()])
            .unwrap()
            .invoke(vec![ChatMessage::user("Read the README")])
            .await
            .unwrap();
        assert_eq!(first.content, "正在读取");
        assert_eq!(first.tool_calls.len(), 1);
        assert_eq!(*state.deletes.lock(), 0);
        let call = &first.tool_calls[0];
        let mut assistant = ChatMessage::assistant(&first.content);
        assistant.tool_calls = Some(vec![MessageToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        }]);
        let result = ChatMessage::tool_result("file contents", &call.id);
        // Rebinding must retain the pending cloud session even with a new provider.
        let second = self::provider(&url)
            .bind_tools(vec![function()])
            .unwrap()
            .invoke(vec![
                ChatMessage::user("Read the README"),
                assistant,
                result,
            ])
            .await
            .unwrap();
        assert_eq!(second.content, "读取完成✅");
        assert_eq!(second.finish_reason.as_deref(), Some("stop"));
        assert_eq!(state.requests.lock().len(), 2);
        assert_eq!(*state.deletes.lock(), 1);
        server.abort();
    }

    #[test]
    fn agents_protocol_is_native_and_does_not_fall_back_to_chat() {
        use crate::providers::factory::ProviderKind;
        for name in ["openai_agents", "openai-agents", "agents_api", "agents-api"] {
            assert_eq!(
                ProviderKind::try_from_str(name),
                Some(ProviderKind::OpenAiAgents)
            );
        }
        assert_eq!(ProviderKind::OpenAiAgents.as_str(), "openai_agents");
    }

    #[test]
    fn new_turn_carries_local_context_schema_and_images_without_unsupported_roles() {
        let provider = provider("https://api.openai.com/v1");
        let mut user = ChatMessage::user("new request");
        user.images = Some(vec![crate::types::response::MessageImage {
            media_type: "image/png".into(),
            data: "aGVsbG8=".into(),
            ..Default::default()
        }]);
        let body = provider.create_body(
            &[
                ChatMessage::system("personality"),
                ChatMessage::assistant("prior reply"),
                user,
            ],
            &[],
            Some(json!({"type": "object"})),
        );
        assert!(body["agent"]["instructions"]
            .as_str()
            .unwrap()
            .contains("personality"));
        assert!(body["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("prior reply"));
        assert_eq!(body["agent"]["text"]["format"]["type"], "json_schema");
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
        assert_eq!(
            body["input"][0]["content"][1]["image_url"],
            "data:image/png;base64,aGVsbG8="
        );
        assert_eq!(
            provider.sessions_url(),
            "https://api.openai.com/v1/agents/sessions"
        );
    }

    #[test]
    fn unsafe_call_paths_and_missing_tool_results_are_rejected() {
        let reference = CallReference {
            session: "../other".into(),
            turn: "turn_1".into(),
            call: "call_1".into(),
        };
        assert!(CallReference::decode(&reference.encode()).is_none());
        let reference = CallReference {
            session: "session_1".into(),
            ..reference
        };
        let mut assistant = ChatMessage::assistant("");
        assistant.tool_calls = Some(vec![MessageToolCall {
            id: reference.encode(),
            name: "read_file".into(),
            arguments: json!({}),
        }]);
        assert!(OpenAiAgentsProvider::continuation(&[assistant.clone()]).is_err());
        // A final assistant answer cuts off old pending references.
        assert!(OpenAiAgentsProvider::continuation(&[
            assistant,
            ChatMessage::assistant("done"),
            ChatMessage::user("next")
        ])
        .unwrap()
        .is_none());
    }
}
