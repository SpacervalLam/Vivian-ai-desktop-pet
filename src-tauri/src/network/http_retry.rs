//! HTTP 重试工具
//!
//! 由于 Rust 端使用 `reqwest`，无法直接在传输层注入重试逻辑，
//! 本模块提供等价的"重试包装"：
//! - `RETRYABLE_STATUS_CODES`（429/500/502/503/504）
//! - `is_retryable_status_code` / `is_retryable_reqwest_error`
//! - `RetryConfig` 配置（max_retries / base_delay / max_delay / timeout）
//! - `retry_request_async` 泛型异步重试包装器
//! - `build_client_with_retry` 构造带重试配置的 reqwest Client
//!
//! 与现有 `network::http_client::get_global_client` 共存：保留原 30s 超时全局客户端，
//! 新增可重试客户端用于 LLM API 等需要重试的场景。

use std::time::Duration;

use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::error::{VivianError, VivianResult};

/// 可重试 HTTP 状态码集合
pub const RETRYABLE_STATUS_CODES: &[u16] = &[429, 500, 502, 503, 504];

/// 默认连接建立超时（秒）
///
/// 与整体 `timeout` 分离：连接阶段（TCP + TLS 握手）通常应在 10s 内完成，
/// 超时多半是网络/DNS/代理问题；连接建立后到响应完成超时通常是服务端慢响应。
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// 判断状态码是否应触发重试
pub fn is_retryable_status_code(status: u16) -> bool {
    RETRYABLE_STATUS_CODES.contains(&status)
}

/// 判断 `reqwest::Error` 是否应触发重试
///
/// 可重试异常类型（httpx.ConnectError / ConnectTimeout / ReadTimeout 等）：
/// reqwest 的 `is_connect` / `is_timeout` / `is_request` 与上述类型语义等价。
pub fn is_retryable_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_timeout() || err.is_request()
}

/// 重试配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// 最大重试次数（不含首次请求）
    pub max_retries: u32,
    /// 指数退避基础延迟（毫秒）
    pub base_delay_ms: u64,
    /// 指数退避最大延迟（毫秒）
    pub max_delay_ms: u64,
    /// 请求超时（秒）
    pub timeout_secs: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,
            max_delay_ms: 10000,
            timeout_secs: 30,
        }
    }
}

impl RetryConfig {
    /// 计算第 `attempt` 次重试的延迟（指数退避，attempt 从 1 开始）
    ///
    /// 指数退避：delay = min(max_delay, base * 2^(attempt-1))
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1);
        let raw = (self.base_delay_ms as u128).checked_shl(exp).unwrap_or(self.max_delay_ms as u128);
        let capped = raw.min(self.max_delay_ms as u128);
        Duration::from_millis(capped as u64)
    }
}

/// HTTP 重试错误
#[derive(Debug)]
pub struct HttpRetryError {
    pub message: String,
    pub status_code: Option<u16>,
    pub attempts: u32,
    pub last_error: Option<String>,
}

impl std::fmt::Display for HttpRetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.status_code, &self.last_error) {
            (Some(code), Some(e)) => write!(
                f,
                "[HTTP Retry] {} (status={code}, attempts={}, last_error={e})",
                self.message, self.attempts
            ),
            (Some(code), None) => write!(
                f,
                "[HTTP Retry] {} (status={code}, attempts={})",
                self.message, self.attempts
            ),
            (None, Some(e)) => write!(
                f,
                "[HTTP Retry] {} (attempts={}, last_error={e})",
                self.message, self.attempts
            ),
            (None, None) => write!(
                f,
                "[HTTP Retry] {} (attempts={})",
                self.message, self.attempts
            ),
        }
    }
}

impl std::error::Error for HttpRetryError {}

impl From<HttpRetryError> for VivianError {
    fn from(e: HttpRetryError) -> Self {
        VivianError::Network(e.to_string())
    }
}

/// 异步重试包装器：执行 `request_fn` 直到成功或耗尽重试次数
///
/// - 状态码 ∈ RETRYABLE_STATUS_CODES → 重试
/// - reqwest 错误且 `is_retryable_reqwest_error` 为 true → 重试
/// - 其他错误立即返回
///
/// `request_fn` 接受当前 attempt（从 1 开始）作为参数，便于日志埋点。
///
/// # 示例
/// ```ignore
/// let client = build_client_with_retry(&RetryConfig::default())?;
/// let resp = retry_request_async(&RetryConfig::default(), |attempt| {
///     let c = client.clone();
///     async move { c.get("https://example.com").send().await }
/// }).await?;
/// ```
pub async fn retry_request_async<F, Fut>(
    config: &RetryConfig,
    mut request_fn: F,
) -> Result<Response, HttpRetryError>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<Response, reqwest::Error>>,
{
    let max_attempts = config.max_retries + 1;
    let mut last_error: Option<String> = None;
    let mut last_status: Option<u16> = None;

    for attempt in 1..=max_attempts {
        match request_fn(attempt).await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if !is_retryable_status_code(status) {
                    return Ok(resp);
                }
                // 可重试状态码
                tracing::warn!(
                    "[HTTP Retry] 请求返回状态码 {} (attempt {}/{})",
                    status,
                    attempt,
                    max_attempts
                );
                last_status = Some(status);
                last_error = None;
                if attempt >= max_attempts {
                    return Err(HttpRetryError {
                        message: format!("HTTP 请求返回可重试状态码: {status}"),
                        status_code: last_status,
                        attempts: attempt,
                        last_error: None,
                    });
                }
                let delay = config.delay_for_attempt(attempt);
                tracing::warn!(
                    "[HTTP Retry] {:.1}s 后重试 (attempt {}/{})",
                    delay.as_secs_f64(),
                    attempt + 1,
                    max_attempts
                );
                sleep(delay).await;
            }
            Err(e) => {
                let retryable = is_retryable_reqwest_error(&e);
                tracing::warn!(
                    "[HTTP Retry] 请求失败: {e} (attempt {}/{}, retryable={retryable})",
                    attempt,
                    max_attempts
                );
                last_error = Some(e.to_string());
                last_status = None;
                if !retryable || attempt >= max_attempts {
                    return Err(HttpRetryError {
                        message: format!("HTTP 请求失败: {e}"),
                        status_code: None,
                        attempts: attempt,
                        last_error: last_error,
                    });
                }
                let delay = config.delay_for_attempt(attempt);
                sleep(delay).await;
            }
        }
    }

    Err(HttpRetryError {
        message: "HTTP 请求重试全部失败".to_string(),
        status_code: last_status,
        attempts: max_attempts,
        last_error,
    })
}

/// 构造带重试配置的 `reqwest::Client`
///
/// - `timeout`：整体请求超时（含连接 + 读响应）
/// - `connect_timeout`：TCP/TLS 握手阶段独立超时，便于与读响应超时分离诊断
/// - 启用 HTTP/2
/// - 连接保持（pool_max_idle_per_host / tcp_keepalive）
///
/// 注意：reqwest 本身不支持 transport 层注入重试逻辑，重试需通过 `retry_request_async` 包装。
pub fn build_client_with_retry(config: &RetryConfig) -> VivianResult<Client> {
    let client = Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs))
        .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .map_err(|e| VivianError::Network(format!("构建 reqwest Client 失败: {e}")))?;
    Ok(client)
}

/// 便捷函数：使用默认 `RetryConfig` 构造客户端
pub fn default_retry_client() -> VivianResult<Client> {
    build_client_with_retry(&RetryConfig::default())
}

/// 同步检查响应状态码是否需要重试（用于流式响应或自定义逻辑）
pub fn check_response_retryable(response: &Response) -> bool {
    is_retryable_status_code(response.status().as_u16())
}

/// 将 `StatusCode` 转为 u16 便于日志输出
pub fn status_to_u16(status: StatusCode) -> u16 {
    status.as_u16()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_status_codes() {
        assert!(is_retryable_status_code(429));
        assert!(is_retryable_status_code(500));
        assert!(is_retryable_status_code(502));
        assert!(is_retryable_status_code(503));
        assert!(is_retryable_status_code(504));
        assert!(!is_retryable_status_code(200));
        assert!(!is_retryable_status_code(404));
        assert!(!is_retryable_status_code(401));
    }

    #[test]
    fn test_delay_for_attempt() {
        let cfg = RetryConfig::default();
        // base=1000, max=10000
        assert_eq!(cfg.delay_for_attempt(1), Duration::from_millis(1000));
        assert_eq!(cfg.delay_for_attempt(2), Duration::from_millis(2000));
        assert_eq!(cfg.delay_for_attempt(3), Duration::from_millis(4000));
        assert_eq!(cfg.delay_for_attempt(4), Duration::from_millis(8000));
        assert_eq!(cfg.delay_for_attempt(5), Duration::from_millis(10000)); // capped
        assert_eq!(cfg.delay_for_attempt(100), Duration::from_millis(10000)); // capped
    }

    #[tokio::test]
    async fn test_retry_attempts_on_connection_error() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let counter = std::sync::Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let cfg = RetryConfig {
            max_retries: 2,
            base_delay_ms: 1,
            max_delay_ms: 5,
            timeout_secs: 2,
        };

        // 构造一个总是连接失败的请求（指向一个保留端口，必然 connect error）
        let url = "http://127.0.0.1:1/";
        let client = build_client_with_retry(&cfg).unwrap();
        let result = retry_request_async(&cfg, |_| {
            let c = counter_clone.clone();
            let client_clone = client.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                client_clone.get(url).send().await
            }
        })
        .await;

        assert!(result.is_err());
        // 应该尝试 3 次（1 + 2 retries）
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_http_retry_error_display() {
        let e = HttpRetryError {
            message: "test".to_string(),
            status_code: Some(503),
            attempts: 4,
            last_error: None,
        };
        let s = format!("{e}");
        assert!(s.contains("503"));
        assert!(s.contains("attempts=4"));
    }

    #[test]
    fn test_build_client() {
        let cfg = RetryConfig::default();
        let client = build_client_with_retry(&cfg);
        assert!(client.is_ok());
    }
}
