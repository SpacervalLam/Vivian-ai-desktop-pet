use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::error::{VivianError, VivianResult};
use crate::providers::base::{BaseProvider, ProviderBase};
use crate::types::response::ChatMessage;
use crate::utils::messages_cache_key;

const MAX_RETRIES: usize = 2;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

type HmacSha256 = Hmac<Sha256>;

/// 讯飞星火认知大模型 WebSocket Provider
///
/// 鉴权方式（与 OpenAI 兼容接口完全不同）：
/// - 使用 APIKey + APISecret 通过 HMAC-SHA256 生成签名，构造授权 URL
/// - 通过 WebSocket 连接授权 URL，发送 JSON 帧，接收 JSON 帧
///
/// 端点（根据模型版本自动选择）：
/// - Ultra v4.0: wss://spark-api.xf-yun.com/v4.0/chat
/// - Max v3.5: wss://spark-api.xf-yun.com/v3.5/chat
/// - Pro v3.1: wss://spark-api.xf-yun.com/v3.1/chat
/// - Pro-128K v3.5: wss://spark-api.xf-yun.com/v3.5/chat
/// - Lite v1.1: wss://spark-api.xf-yun.com/v1.1/chat
///
/// model 字段约定：
/// - `4.0Ultra` → v4.0 端点
/// - `max-32k` / `generalv3.5` → v3.5 端点
/// - `generalv3` → v3.1 端点
/// - `general` → v1.1 端点
pub struct SparkProvider {
    base: ProviderBase,
    /// API Secret（与 api_key 配对用于 HMAC 签名）
    api_secret: String,
    /// 应用 ID（讯飞控制台获取，必填）
    app_id: String,
}

impl SparkProvider {
    pub fn new(
        config: &crate::config::manager::ProviderConfig,
        api_secret: &str,
        app_id: &str,
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
            api_secret: api_secret.to_string(),
            app_id: app_id.to_string(),
        }
    }

    /// 根据模型名解析 WebSocket 端点 host / path / domain
    fn resolve_endpoint(&self) -> (&'static str, &'static str, &'static str) {
        let model_lower = self.base.model.to_lowercase();
        if model_lower.contains("4.0ultra") || model_lower.contains("ultra") {
            ("spark-api.xf-yun.com", "/v4.0/chat", "4.0Ultra")
        } else if model_lower.contains("max-32k") {
            ("spark-api.xf-yun.com", "/v3.5/chat", "max-32k")
        } else if model_lower.contains("generalv3.5") || model_lower.contains("max") {
            ("spark-api.xf-yun.com", "/v3.5/chat", "generalv3.5")
        } else if model_lower.contains("generalv3") || model_lower.contains("pro") {
            ("spark-api.xf-yun.com", "/v3.1/chat", "generalv3")
        } else {
            ("spark-api.xf-yun.com", "/v1.1/chat", "general")
        }
    }

    /// 生成讯飞授权 URL
    ///
    /// 算法：
    /// 1. signature_origin = "host: {host}\ndate: {date}\nGET {path} HTTP/1.1"
    /// 2. signature = HMAC-SHA256(signature_origin, api_secret) → 十六进制
    /// 3. authorization_origin = "api_key=\"{api_key}\", algorithm=\"hmac-sha256\",
    ///    headers=\"host date request-line\", signature=\"{signature}\""
    /// 4. authorization = base64(authorization_origin)
    /// 5. url = wss://{host}{path}?authorization={authorization}&date={date}&host={host}
    fn build_auth_url(&self, host: &str, path: &str) -> VivianResult<String> {
        // RFC1123 格式 UTC 时间
        let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();

        let signature_origin = format!(
            "host: {}\ndate: {}\nGET {} HTTP/1.1",
            host, date, path
        );

        let mut mac = HmacSha256::new_from_slice(self.api_secret.as_bytes())
            .map_err(|e| VivianError::Provider(format!("HMAC 密钥初始化失败: {}", e)))?;
        mac.update(signature_origin.as_bytes());
        let signature = mac.finalize().into_bytes();
        let signature_hex = signature.iter().map(|b| format!("{:02x}", b)).collect::<String>();

        let authorization_origin = format!(
            "api_key=\"{}\", algorithm=\"hmac-sha256\", headers=\"host date request-line\", signature=\"{}\"",
            self.base.api_key, signature_hex
        );
        let authorization =
            base64::engine::general_purpose::STANDARD.encode(authorization_origin.as_bytes());

        let url = format!(
            "wss://{}{}?authorization={}&date={}&host={}",
            host,
            path,
            urlencode(&authorization),
            urlencode(&date),
            host
        );
        Ok(url)
    }

    fn build_frame(&self, messages: &[ChatMessage], domain: &str) -> Value {
        // 讯飞帧结构：{header, parameter, payload}
        // payload.message.text 中 role 为 user / assistant（无 system，system 通过 networking 拼接）
        let text: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                json!({"role": m.role, "content": m.content})
            })
            .collect();

        let mut body = json!({
            "header": {
                "app_id": self.app_id,
                "uid": "vivian"
            },
            "parameter": {
                "chat": {
                    "domain": domain,
                    "max_tokens": self.base.max_tokens,
                    "temperature": self.base.effective_temperature(),
                    "audience": "public"
                }
            },
            "payload": {
                "message": {
                    "text": text
                }
            }
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        body
    }

    fn extract_content(json: &Value) -> VivianResult<String> {
        // 成功响应：
        // {"header":{"code":0,"message":"Success","sid":"..."},
        //  "payload":{"choices":{"status":2,"seq":0,
        //    "text":[{"content":"...","role":"assistant","index":0}]}},
        //  "usage":{...}}
        // 失败响应：{"header":{"code":10014,"message":"..."}}
        let code = json
            .get("header")
            .and_then(|h| h.get("code"))
            .and_then(|c| c.as_i64())
            .unwrap_or(-1);
        if code != 0 {
            let msg = json
                .get("header")
                .and_then(|h| h.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            return Err(VivianError::Provider(format!(
                "讯飞星火 API 错误 (code={}): {}",
                code, msg
            )));
        }
        json.get("payload")
            .and_then(|p| p.get("choices"))
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| VivianError::Provider("讯飞响应缺少 payload.choices.text[0].content".to_string()))
    }

    /// 通过 WebSocket 发送请求并聚合响应（同步阻塞等待全部片段）
    async fn send_ws_request(&self, frame: &Value, auth_url: &str) -> VivianResult<Value> {
        // 使用 tokio_tungstenite 直接连 WebSocket（不支持自定义代理，但 reqwest 客户端已用于其他 provider）
        let (ws_stream, _response) = connect_async(auth_url)
            .await
            .map_err(|e| VivianError::Provider(format!("讯飞 WebSocket 连接失败: {}", e)))?;

        let (mut write, mut read) = ws_stream.split();

        // 发送请求帧
        let frame_str = serde_json::to_string(frame)
            .map_err(|e| VivianError::Provider(format!("讯飞帧序列化失败: {}", e)))?;
        write
            .send(Message::Text(frame_str))
            .await
            .map_err(|e| VivianError::Provider(format!("讯飞 WebSocket 发送失败: {}", e)))?;

        // 接收并聚合所有响应片段（status=2 表示最后一帧）
        let mut aggregated = String::new();
        let mut last_full: Option<Value> = None;
        while let Some(msg_result) = read.next().await {
            let msg = msg_result
                .map_err(|e| VivianError::Provider(format!("讯飞 WebSocket 接收失败: {}", e)))?;
            match msg {
                Message::Text(text) => {
                    let json: Value = serde_json::from_str(&text).map_err(|e| {
                        VivianError::Provider(format!("讯飞响应解析失败: {} | raw={}", e, text))
                    })?;

                    let status = json
                        .get("payload")
                        .and_then(|p| p.get("choices"))
                        .and_then(|c| c.get("status"))
                        .and_then(|s| s.as_i64())
                        .unwrap_or(0);
                    last_full = Some(json.clone());

                    if let Ok(content) = Self::extract_content(&json) {
                        aggregated.push_str(&content);
                    }

                    if status == 2 {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        // 把聚合内容塞回 last_full 的 payload.choices.text[0].content 中
        if let Some(mut full) = last_full {
            if let Some(text_arr) = full
                .get_mut("payload")
                .and_then(|p| p.get_mut("choices"))
                .and_then(|c| c.get_mut("text"))
                .and_then(|t| t.as_array_mut())
            {
                if let Some(first) = text_arr.first_mut() {
                    first["content"] = json!(aggregated);
                }
            }
            return Ok(full);
        }

        Err(VivianError::Provider(
            "讯飞 WebSocket 未收到任何响应".to_string(),
        ))
    }

    async fn call_with_retry(
        &self,
        frame: Value,
        cache_key_prompt: Option<&str>,
        host: &str,
        path: &str,
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
                tracing::warn!("第 {} 次重试讯飞请求: {}", attempt, self.base.model);
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }

            self.base.check_circuit()?;

            let auth_url = match self.build_auth_url(host, path) {
                Ok(u) => u,
                Err(e) => return Err(e),
            };

            match self.send_ws_request(&frame, &auth_url).await {
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
                    let category = crate::resilience::classify_error(&err);
                    match category {
                        crate::resilience::ErrorCategory::Permanent => return Err(err),
                        crate::resilience::ErrorCategory::Transient
                        | crate::resilience::ErrorCategory::RateLimit => {
                            last_error = Some(err);
                            continue;
                        }
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| VivianError::Provider("讯飞重试次数耗尽".to_string())))
    }
}

/// URL 编码（用于 query 参数）
fn urlencode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

#[async_trait]
impl BaseProvider for SparkProvider {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        let prompt_key = messages_cache_key(&messages);
        let (host, path, domain) = self.resolve_endpoint();
        let frame = self.build_frame(&messages, domain);
        self.call_with_retry(frame, Some(&prompt_key), host, path).await
    }

    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        _json_schema: Option<serde_json::Value>,
    ) -> VivianResult<String> {
        // 讯飞 Pro/Max/Ultra 支持联网搜索插件：通过 payload.message.text 拼接特定指令
        // 实际更优雅的方式是 network 参数（部分模型支持），这里通过 system 消息触发
        let mut augmented = messages;
        if enable_search || self.base.is_enable_search() {
            tracing::info!(
                "[Router] 讯飞星火联网搜索已启用（通过插件指令）: model={}",
                self.base.model
            );
            // 在消息前插入 system 提示，触发讯飞内置联网搜索
            let sys = ChatMessage::system("请使用联网搜索功能获取最新信息后再回答。");
            augmented.insert(0, sys);
        }
        self.call_chat(augmented).await
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        _json_schema: Option<serde_json::Value>,
    ) -> VivianResult<mpsc::Receiver<String>> {
        // 讯飞流式：通过 WebSocket 接收所有片段，每收到一帧就转发到 channel
        self.base.check_circuit()?;
        let (host, path, domain) = self.resolve_endpoint();
        let frame = self.build_frame(&messages, domain);
        let auth_url = self.build_auth_url(host, path)?;

        let (tx, rx) = mpsc::channel::<String>(32);
        let app_id = self.app_id.clone();

        tokio::spawn(async move {
            let (ws_stream, _response) = match connect_async(&auth_url).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("讯飞流式 WebSocket 连接失败: {}", e);
                    return;
                }
            };
            let (mut write, mut read) = ws_stream.split();

            let frame_str = match serde_json::to_string(&frame) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("讯飞流式帧序列化失败: {}", e);
                    return;
                }
            };
            if write.send(Message::Text(frame_str)).await.is_err() {
                return;
            }

            let _ = app_id; // 仅供日志使用

            while let Some(msg_result) = read.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        if let Ok(json) = serde_json::from_str::<Value>(&text) {
                            let status = json
                                .get("payload")
                                .and_then(|p| p.get("choices"))
                                .and_then(|c| c.get("status"))
                                .and_then(|s| s.as_i64())
                                .unwrap_or(0);

                            if let Ok(content) = SparkProvider::extract_content(&json) {
                                if !content.is_empty() {
                                    if tx.send(content).await.is_err() {
                                        return;
                                    }
                                }
                            }

                            if status == 2 {
                                return;
                            }
                        }
                    }
                    Ok(Message::Close(_)) => return,
                    Err(e) => {
                        tracing::error!("讯飞流式 WebSocket 接收失败: {}", e);
                        return;
                    }
                    _ => {}
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

    fn set_omit_temperature(&self, omit: bool) {
        self.base.set_omit_temperature(omit);
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
}
