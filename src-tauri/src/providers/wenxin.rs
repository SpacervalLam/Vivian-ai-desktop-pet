use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::error::{VivianError, VivianResult};
use crate::providers::base::{BaseProvider, ProviderBase};
use crate::types::response::ChatMessage;
use crate::utils::messages_cache_key;

const OAUTH_ENDPOINT: &str = "https://aip.baidubce.com/oauth/2.0/token";
const CHAT_BASE: &str = "https://aip.baidubce.com";
const TOKEN_TTL_MARGIN: Duration = Duration::from_secs(60);

/// 截断字符串用于日志/错误输出，避免泄露敏感信息或过长内容。
/// 按 UTF-8 字符边界安全截断到 max_bytes 字节以内。
fn truncate_for_log(s: &str, max_bytes: usize) -> String {
    if s.len() > max_bytes {
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

/// 百度文心一言（ERNIE Bot）原生 API Provider
///
/// 鉴权方式（与 OpenAI 兼容接口不同）：
/// - 使用 API Key + Secret Key 通过 OAuth 换取 access_token
/// - access_token 默认有效期 30 天，缓存复用
/// - 调用时通过 query 参数 access_token=xxx 传递（非 Bearer 头）
///
/// 端点：`https://aip.baidubce.com/wenxinworkshop/chat/{model_path}?access_token=xxx`
///
/// model 字段约定：用户在 UI 填模型名时填入完整路径，例如：
/// - `ernie-4.0-8k-latest` → 完整 URL /wenxinworkshop/chat/ernie-4.0-8k-latest
/// - `ernie-4.0-turbo-8k` → /wenxinworkshop/chat/ernie-4.0-turbo-8k
/// - `ernie-speed-128k` → /wenxinworkshop/chat/ernie-speed-128k
/// - `ernie-lite-8k` → /wenxinworkshop/chat/ernie-lite-8k
/// - `eb-instant` → /wenxinworkshop/chat/eb-instant
pub struct WenxinProvider {
    base: ProviderBase,
    /// API Secret（与 api_key 配对用于 OAuth）
    api_secret: String,
    /// 缓存的 access_token 及其过期时间
    token_cache: Mutex<Option<(String, Instant)>>,
}

impl WenxinProvider {
    pub fn new(
        config: &crate::config::manager::ProviderConfig,
        api_secret: &str,
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
            token_cache: Mutex::new(None),
        }
    }

    fn chat_endpoint(&self, access_token: &str) -> String {
        // access_token 经 URL 传递（百度 API 限制），错误处理需 mask
        let base = self.base.base_url.trim_end_matches('/');
        let url = if base.is_empty() || base == "https://aip.baidubce.com" {
            format!("{}/wenxinworkshop/chat/{}", CHAT_BASE, self.base.model)
        } else {
            format!("{}/wenxinworkshop/chat/{}", base, self.base.model)
        };
        format!("{}?access_token={}", url, access_token)
    }

    /// 获取或刷新 access_token
    async fn get_access_token(&self) -> VivianResult<String> {
        // 先查缓存
        {
            let cache = self.token_cache.lock();
            if let Some((token, expires_at)) = cache.as_ref() {
                if *expires_at > Instant::now() + TOKEN_TTL_MARGIN {
                    return Ok(token.clone());
                }
            }
        }

        // 缓存过期或不存在，发起 OAuth 请求
        let client = self.base.get_client();
        let response = client
            .post(OAUTH_ENDPOINT)
            .query(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.base.api_key.as_str()),
                ("client_secret", self.api_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|e| VivianError::Provider(format!("文心 access_token 请求失败: {}", e)))?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            // Mask 错误响应体，避免泄露完整 URL（含 access_token）或其他敏感信息到日志
            let masked = truncate_for_log(&text, 100);
            return Err(VivianError::Provider(format!(
                "文心 OAuth 失败: {}",
                masked
            )));
        }

        let json: Value = response
            .json()
            .await
            .map_err(|e| VivianError::Provider(format!("文心 OAuth 响应解析失败: {}", e)))?;

        let token = json
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                // Mask 响应体，避免 OAuth 响应可能包含的敏感信息泄露
                let masked = truncate_for_log(&json.to_string(), 100);
                VivianError::Provider(format!("文心 OAuth 响应缺少 access_token: {}", masked))
            })?
            .to_string();

        let expires_in = json
            .get("expires_in")
            .and_then(|e| e.as_u64())
            .unwrap_or(2592000);
        let expires_at = Instant::now() + Duration::from_secs(expires_in);

        // 写入缓存
        {
            let mut cache = self.token_cache.lock();
            *cache = Some((token.clone(), expires_at));
        }

        tracing::info!(
            "[Wenxin] 刷新 access_token 成功，有效期 {} 秒，model={}",
            expires_in,
            self.base.model
        );
        Ok(token)
    }

    fn build_messages(messages: &[ChatMessage]) -> Vec<Value> {
        // 文心 API：messages 中 role 仅支持 user / assistant / function
        // 第一条必须是 user；system 消息需通过单独的 system 字段传递
        messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                json!({"role": m.role, "content": m.content})
            })
            .collect()
    }

    fn extract_system(messages: &[ChatMessage]) -> Option<String> {
        let system_msgs: Vec<&str> = messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .collect();
        if system_msgs.is_empty() {
            None
        } else {
            Some(system_msgs.join("\n\n"))
        }
    }

    fn build_body(&self, messages: &[ChatMessage]) -> Value {
        let converted = Self::build_messages(messages);
        let mut body = json!({
            "messages": converted,
            "temperature": self.base.effective_temperature(),
        });
        // 工作智能体模式：省略 temperature（服务端默认）
        self.base.strip_temperature(&mut body);
        // max_tokens 在文心 API 中字段名为 max_output_tokens（部分模型）
        if self.base.max_tokens > 0 {
            body["max_output_tokens"] = json!(self.base.max_tokens);
        }
        if let Some(sys) = Self::extract_system(messages) {
            body["system"] = json!(sys);
        }
        body
    }

    async fn send_request(&self, body: &Value) -> VivianResult<Value> {
        let token = self.get_access_token().await?;
        let url = self.chat_endpoint(&token);
        let client = self.base.get_client();
        let response = client
            .post(&url)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| {
                // reqwest 错误 Display 含完整 URL（含 access_token），mask 后再写入错误消息
                let msg = e.to_string().replace(&token, "***");
                VivianError::Provider(format!("文心 API 请求失败: {}", msg))
            })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "文心 API 请求失败 ({}): {}",
                status, text
            )));
        }
        let json: Value = response.json().await?;
        Ok(json)
    }

    fn extract_content(json: &Value) -> VivianResult<String> {
        // 文心响应：{"id":"...","object":"chat.completion","result":"...","usage":{...}}
        // 错误响应：{"error_code":17,"error_msg":"..."}
        if let Some(err_code) = json.get("error_code").and_then(|c| c.as_i64()) {
            let err_msg = json
                .get("error_msg")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            return Err(VivianError::Provider(format!(
                "文心 API 错误 (code={}): {}",
                err_code, err_msg
            )));
        }
        json.get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| VivianError::Provider("文心响应缺少 result 字段".to_string()))
    }
}

#[async_trait]
impl BaseProvider for WenxinProvider {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
        let prompt_key = messages_cache_key(&messages);
        if let Some(cached) = self.base.get_cached_response(&prompt_key) {
            return Ok(cached);
        }
        let body = self.build_body(&messages);
        self.base.check_circuit()?;
        let json = self.send_request(&body).await?;
        self.base.record_success();
        let content = Self::extract_content(&json)?;
        self.base.cache_response(&prompt_key, &content);
        Ok(content)
    }

    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        _json_schema: Option<serde_json::Value>,
    ) -> VivianResult<String> {
        // 文心可通过 enable_search 参数启用百度搜索增强
        let prompt_key = messages_cache_key(&messages);
        if let Some(cached) = self.base.get_cached_response(&prompt_key) {
            return Ok(cached);
        }
        let mut body = self.build_body(&messages);
        if enable_search || self.base.is_enable_search() {
            body["enable_search"] = json!(true);
            tracing::info!("[Router] 文心百度搜索增强已启用: model={}", self.base.model);
        }
        self.base.check_circuit()?;
        let json = self.send_request(&body).await?;
        self.base.record_success();
        let content = Self::extract_content(&json)?;
        self.base.cache_response(&prompt_key, &content);
        Ok(content)
    }

    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        _json_schema: Option<serde_json::Value>,
    ) -> VivianResult<mpsc::Receiver<String>> {
        // 文心流式接口：URL 改为 /wenxinworkshop/chat/{model}?access_token=xxx&stream=true
        // body 加 stream: true；响应为 SSE，data: 行为 JSON
        self.base.check_circuit()?;
        let token = self.get_access_token().await?;
        let url = self.chat_endpoint(&token) + "&stream=true";
        let mut body = self.build_body(&messages);
        body["stream"] = json!(true);

        let client = self.base.get_client();
        let response = client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                // reqwest 错误 Display 含完整 URL（含 access_token），mask 后再写入错误消息
                let msg = e.to_string().replace(&token, "***");
                VivianError::Network(format!("文心流式 API 请求失败: {}", msg))
            })?;

        if !response.status().is_success() {
            self.base.record_failure();
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(VivianError::Provider(format!(
                "文心流式 API 请求失败 ({}): {}",
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
                        tracing::error!("文心流读取失败: {}", e);
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
                        if let Ok(json) = serde_json::from_str::<Value>(data) {
                            // 文心流式响应：{"result":"...","is_end":false,...}
                            if let Some(text) = json.get("result").and_then(|t| t.as_str()) {
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
