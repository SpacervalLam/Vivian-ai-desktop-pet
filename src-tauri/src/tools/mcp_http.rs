//! Streamable HTTP: JSON/SSE POST responses and negotiated sessions.
use super::*;
use futures::StreamExt;

pub(super) struct HttpTransport {
    client: reqwest::Client,
    session: AsyncMutex<Option<String>>,
    version: Mutex<String>,
    expired: AtomicBool,
}

impl HttpTransport {
    pub fn new() -> VivianResult<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60)).build()
            .map_err(|e| VivianError::Tool(format!("MCP HTTP 客户端创建失败: {e}")))?;
        Ok(Self { client, session: AsyncMutex::new(None), version: Mutex::new("2025-11-25".into()), expired: AtomicBool::new(false) })
    }

    pub fn set_version(&self, version: &str) { *self.version.lock() = version.into(); }
    pub fn has_expired(&self) -> bool { self.expired.load(Ordering::Relaxed) }

    async fn reply(&self, config: &McpServerConfig, payload: Value) -> VivianResult<()> {
        let version = self.version.lock().clone();
        let session = self.session.lock().await.clone();
        let mut request = self.client.post(config.url.as_deref().unwrap_or_default())
            .headers(self.headers(config)?).header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", version).json(&payload);
        if let Some(session) = session { request = request.header("Mcp-Session-Id", session); }
        let response = request.send().await.map_err(|e|VivianError::Tool(format!("MCP HTTP 应答失败: {}", e.without_url())))?;
        if !response.status().is_success() { return Err(VivianError::Tool(format!("MCP HTTP 应答失败: {}",response.status()))); }
        Ok(())
    }

    fn headers(&self, config: &McpServerConfig) -> VivianResult<reqwest::header::HeaderMap> {
        let mut headers = reqwest::header::HeaderMap::new();
        for (key, value) in &config.headers {
            let name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| VivianError::Tool("MCP HTTP 请求头名称无效".into()))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| VivianError::Tool("MCP HTTP 请求头值无效".into()))?;
            headers.insert(name, value);
        }
        Ok(headers)
    }

    pub async fn send(&self, config: &McpServerConfig, payload: Value) -> VivianResult<Value> {
        let expected = payload.get("id").cloned();
        let initializing = payload["method"] == "initialize";
        let mut request = self.client.post(config.url.as_deref().unwrap_or_default())
            .headers(self.headers(config)?)
            .header("Accept", "application/json, text/event-stream")
            .json(&payload);
        if !initializing {
            request = request.header("MCP-Protocol-Version", self.version.lock().clone());
            if let Some(session) = self.session.lock().await.as_ref() {
                request = request.header("Mcp-Session-Id", session);
            }
        }
        let response = request.send().await
            .map_err(|e| VivianError::Tool(format!("MCP HTTP 连接失败: {}", e.without_url())))?;
        if !response.status().is_success() {
            if response.status() == reqwest::StatusCode::NOT_FOUND && !initializing {
                self.expired.store(true, Ordering::Relaxed);
            }
            // Do not echo upstream bodies or URLs, which may contain credentials.
            return Err(VivianError::Tool(format!("MCP HTTP {}{}", response.status(),
                if response.status() == reqwest::StatusCode::UNAUTHORIZED { "：请检查请求头中的认证凭据" }
                else if response.status() == reqwest::StatusCode::NOT_FOUND { "：会话失效或端点不存在，请重新连接" } else { "" })));
        }
        if initializing {
            if let Some(session) = response.headers().get("mcp-session-id").and_then(|s| s.to_str().ok()) {
                *self.session.lock().await = Some(session.to_string());
            }
        }
        if expected.is_none() { return Ok(Value::Null); }
        let is_sse = response.headers().get("content-type").and_then(|v|v.to_str().ok())
            .is_some_and(|v|v.starts_with("text/event-stream"));
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut total = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| VivianError::Tool(format!("MCP HTTP 响应读取失败: {}", e.without_url())))?;
            total += chunk.len();
            if total > 16 * 1024 * 1024 { return Err(VivianError::Tool("MCP 响应超过 16 MiB".into())); }
            buffer.extend_from_slice(&chunk);
            if is_sse {
                for message in protocol::take_sse_messages(&mut buffer).map_err(VivianError::Tool)? {
                    if message.get("method").is_some() {
                        if let Some(id) = message.get("id") {
                            let reply = if message["method"] == "ping" { json!({"jsonrpc":"2.0","id":id,"result":{}}) }
                                else { json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Client method not supported"}}) };
                            self.reply(config, reply).await?;
                        }
                        continue;
                    }
                    if message.get("id") == expected.as_ref() && (message.get("result").is_some() || message.get("error").is_some()) {
                        return Ok(message);
                    }
                }
            }
        }
        if is_sse { return Err(VivianError::Tool("MCP SSE 在收到调用回执前断开".into())); }
        let message: Value = serde_json::from_slice(&buffer)
            .map_err(|e|VivianError::Tool(format!("MCP HTTP 响应不是 JSON: {e}")))?;
        if message.get("id") != expected.as_ref() { return Err(VivianError::Tool("MCP HTTP 响应 ID 不匹配".into())); }
        Ok(message)
    }

    pub async fn shutdown(&self, config: &McpServerConfig) {
        let session = self.session.lock().await.take();
        if let Some(session) = session {
            if let Ok(headers) = self.headers(config) {
                let version = self.version.lock().clone();
                let _ = self.client.delete(config.url.as_deref().unwrap_or_default())
                    .headers(headers).header("Mcp-Session-Id", session)
                    .header("MCP-Protocol-Version", version)
                    .timeout(Duration::from_secs(5)).send().await;
            }
        }
    }
}
