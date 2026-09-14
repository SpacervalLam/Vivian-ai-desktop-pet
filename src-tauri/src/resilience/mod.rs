use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    pub failure_threshold: u32,
    pub failure_rate_threshold: f64,
    pub reset_timeout: Duration,
    pub last_failure_time: Option<Instant>,
    /// 滑动窗口：最近一次调用是否成功（true=成功，false=失败）
    pub recent_results: VecDeque<bool>,
    /// 滑动窗口大小（默认 20）
    pub window_size: u32,
    /// 触发失败率判定的最小样本数（默认 5）
    pub min_samples: u32,
}

impl CircuitBreaker {
    pub fn new(name: impl Into<String>, threshold: u32, rate: f64, timeout: Duration) -> Self {
        Self {
            name: name.into(),
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            failure_threshold: threshold,
            failure_rate_threshold: rate,
            reset_timeout: timeout,
            last_failure_time: None,
            recent_results: VecDeque::new(),
            window_size: 20,
            min_samples: 5,
        }
    }

    pub fn allow_request(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                if let Some(last) = self.last_failure_time {
                    if last.elapsed() >= self.reset_timeout {
                        self.state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }

    pub fn record_success(&mut self) {
        self.failure_count = 0;
        self.success_count += 1;
        self.push_result(true);
        self.state = CircuitState::Closed;
    }

    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure_time = Some(Instant::now());
        self.push_result(false);

        // 半开状态失败 → 立即熔断
        if self.state == CircuitState::HalfOpen {
            self.state = CircuitState::Open;
            return;
        }

        if self.state == CircuitState::Closed {
            let samples = self.recent_results.len() as u32;
            let rate = self.recent_failure_rate();
            // 失败次数达阈值 或 滑动窗口失败率达阈值（需满足最小样本数）→ 熔断
            if self.failure_count >= self.failure_threshold
                || (samples >= self.min_samples && rate >= self.failure_rate_threshold)
            {
                self.state = CircuitState::Open;
            }
        }
    }

    /// 记录一次调用结果到滑动窗口，并维护窗口大小
    fn push_result(&mut self, success: bool) {
        self.recent_results.push_back(success);
        while (self.recent_results.len() as u32) > self.window_size {
            self.recent_results.pop_front();
        }
    }

    /// 计算滑动窗口内的失败率（false 比例）
    fn recent_failure_rate(&self) -> f64 {
        if self.recent_results.is_empty() {
            return 0.0;
        }
        let failures = self.recent_results.iter().filter(|&&r| !r).count();
        failures as f64 / self.recent_results.len() as f64
    }

    pub fn reset(&mut self) {
        self.state = CircuitState::Closed;
        self.failure_count = 0;
        self.success_count = 0;
        self.last_failure_time = None;
        self.recent_results.clear();
    }

    pub fn get_stats(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "state": self.state,
            "failure_count": self.failure_count,
            "success_count": self.success_count,
            "failure_threshold": self.failure_threshold,
            "failure_rate_threshold": self.failure_rate_threshold,
            "reset_timeout_ms": self.reset_timeout.as_millis() as u64,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorCategory {
    Permanent,
    Transient,
    RateLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmErrorKind {
    InvalidApiKey,
    InsufficientBalance,
    QuotaExceeded,
    RateLimited,
    ModelNotFound,
    ContextLengthExceeded,
    ContentPolicy,
    ServerError,
    Overloaded,
    Timeout,
    NetworkError,
    BadRequest,
    RegionNotSupported,
    PermissionDenied,
    CircuitBreakerOpen,
    Unknown,
}

pub fn classify_error(error: &dyn std::error::Error) -> ErrorCategory {
    classify_error_from_str(&error.to_string())
}

pub fn classify_error_from_str(msg: &str) -> ErrorCategory {
    let msg = msg.to_lowercase();
    if msg.contains("401")
        || msg.contains("invalid_api_key")
        || msg.contains("invalid authentication")
        || msg.contains("incorrect api key")
    {
        ErrorCategory::Permanent
    } else if msg.contains("400")
        || msg.contains("bad request")
        || msg.contains("invalid_argument")
    {
        // 400 Bad Request：请求格式错误，重试无意义
        ErrorCategory::Permanent
    } else if msg.contains("insufficient")
        || msg.contains("balance")
        || msg.contains("quota")
        || (msg.contains("403") && !msg.contains("country") && !msg.contains("region") && !msg.contains("territory"))
    {
        ErrorCategory::Permanent
    } else if msg.contains("404")
        || msg.contains("model_not_found")
        || msg.contains("does not exist")
    {
        ErrorCategory::Permanent
    } else if msg.contains("429") || msg.contains("rate_limit") || msg.contains("rate limit") {
        ErrorCategory::RateLimit
    } else if msg.contains("context_length")
        || msg.contains("content_policy")
        || msg.contains("content_filter")
        || msg.contains("moderation")
    {
        ErrorCategory::Permanent
    } else if msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("internal server error")
        || msg.contains("overloaded")
    {
        ErrorCategory::Transient
    } else if msg.contains("timeout")
        || msg.contains("timed out")
        || msg.contains("deadline")
    {
        ErrorCategory::Transient
    } else if msg.contains("dns")
        || msg.contains("unreachable")
        || msg.contains("refused")
        || msg.contains("broken pipe")
        || msg.contains("certificate")
        || msg.contains("ssl")
        || msg.contains("hyper error")
    {
        ErrorCategory::Transient
    } else if msg.contains("country")
        || msg.contains("region")
        || msg.contains("territory")
        || msg.contains("geolocation")
        || msg.contains("ip not authorized")
        || msg.contains("ip allowlist")
        || msg.contains("ip whitelist")
    {
        ErrorCategory::Permanent
    } else if msg.contains("permission")
        || msg.contains("access denied")
    {
        ErrorCategory::Permanent
    } else if msg.contains("circuit_breaker")
        || msg.contains("熔断器")
    {
        ErrorCategory::Transient
    } else {
        ErrorCategory::Transient
    }
}

pub fn classify_llm_error(error: &dyn std::error::Error) -> LlmErrorKind {
    classify_llm_error_from_str(&error.to_string())
}

pub fn classify_llm_error_from_str(msg: &str) -> LlmErrorKind {
    let lower = msg.to_lowercase();

    if lower.contains("circuit_breaker") || lower.contains("熔断器") {
        return LlmErrorKind::CircuitBreakerOpen;
    }

    if lower.contains("invalid_api_key")
        || lower.contains("invalid authentication")
        || lower.contains("incorrect api key")
        || (lower.contains("api key") && (lower.contains("invalid") || lower.contains("expired") || lower.contains("revoked") || lower.contains("incorrect")))
        || (lower.contains("401") && !lower.contains("ip"))
    {
        return LlmErrorKind::InvalidApiKey;
    }

    if (lower.contains("403") && (lower.contains("insufficient") || lower.contains("balance")))
        || lower.contains("account balance is insufficient")
        || lower.contains("insufficient balance")
        || lower.contains("insufficient_quota")
        || lower.contains("余额不足")
    {
        return LlmErrorKind::InsufficientBalance;
    }

    if lower.contains("exceeded your current quota")
        || lower.contains("quota exceeded")
        || lower.contains("billing details")
        || lower.contains("run out of credits")
        || lower.contains("you exceeded")
    {
        return LlmErrorKind::QuotaExceeded;
    }

    if lower.contains("429")
        || lower.contains("rate_limit")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("slow down")
    {
        return LlmErrorKind::RateLimited;
    }

    if lower.contains("model_not_found")
        || lower.contains("does not exist")
        || (lower.contains("model") && lower.contains("not found"))
        || (lower.contains("model") && lower.contains("unavailable"))
        || (lower.contains("404") && lower.contains("model"))
    {
        return LlmErrorKind::ModelNotFound;
    }

    if lower.contains("context_length")
        || lower.contains("context length")
        || lower.contains("maximum context")
        || lower.contains("token limit")
        || (lower.contains("max_tokens") && lower.contains("exceed"))
    {
        return LlmErrorKind::ContextLengthExceeded;
    }

    if lower.contains("content_policy")
        || lower.contains("content_filter")
        || lower.contains("content was filtered")
        || lower.contains("moderation")
        || lower.contains("rejected")
        || lower.contains("敏感")
    {
        return LlmErrorKind::ContentPolicy;
    }

    if lower.contains("500") || lower.contains("internal server error") {
        return LlmErrorKind::ServerError;
    }

    if lower.contains("overloaded")
        || lower.contains("engine is currently overloaded")
        || lower.contains("capacity")
        || (lower.contains("503") && !lower.contains("model"))
    {
        return LlmErrorKind::Overloaded;
    }

    if lower.contains("502") || lower.contains("504") {
        return LlmErrorKind::ServerError;
    }

    if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("deadline exceeded")
        || lower.contains("request timeout")
    {
        return LlmErrorKind::Timeout;
    }

    if lower.contains("dns")
        || lower.contains("unreachable")
        || lower.contains("refused")
        || lower.contains("broken pipe")
        || lower.contains("certificate")
        || lower.contains("ssl")
        || lower.contains("hyper error")
        || (lower.contains("connect") && (lower.contains("error") || lower.contains("fail") || lower.contains("reset")))
    {
        return LlmErrorKind::NetworkError;
    }

    if lower.contains("country")
        || lower.contains("region")
        || lower.contains("territory")
        || lower.contains("geolocation")
        || lower.contains("ip not authorized")
        || lower.contains("ip allowlist")
        || lower.contains("ip whitelist")
    {
        return LlmErrorKind::RegionNotSupported;
    }

    if lower.contains("permission")
        || lower.contains("forbidden")
        || lower.contains("access denied")
        || lower.contains("not authorized")
    {
        return LlmErrorKind::PermissionDenied;
    }

    if lower.contains("bad request")
        || lower.contains("400")
    {
        return LlmErrorKind::BadRequest;
    }

    LlmErrorKind::Unknown
}

pub fn error_kind_to_message_key(kind: &LlmErrorKind) -> &'static str {
    match kind {
        LlmErrorKind::InvalidApiKey => "toast.llm_error_invalid_api_key",
        LlmErrorKind::InsufficientBalance => "toast.llm_error_insufficient_balance",
        LlmErrorKind::QuotaExceeded => "toast.llm_error_quota_exceeded",
        LlmErrorKind::RateLimited => "toast.llm_error_rate_limited",
        LlmErrorKind::ModelNotFound => "toast.llm_error_model_not_found",
        LlmErrorKind::ContextLengthExceeded => "toast.llm_error_context_length",
        LlmErrorKind::ContentPolicy => "toast.llm_error_content_policy",
        LlmErrorKind::ServerError => "toast.llm_error_server_error",
        LlmErrorKind::Overloaded => "toast.llm_error_overloaded",
        LlmErrorKind::Timeout => "toast.llm_error_timeout",
        LlmErrorKind::NetworkError => "toast.llm_error_network",
        LlmErrorKind::BadRequest => "toast.llm_error_bad_request",
        LlmErrorKind::RegionNotSupported => "toast.llm_error_region",
        LlmErrorKind::PermissionDenied => "toast.llm_error_permission",
        LlmErrorKind::CircuitBreakerOpen => "toast.llm_error_circuit_breaker",
        LlmErrorKind::Unknown => "toast.llm_error_unknown",
    }
}

#[derive(Debug)]
pub struct CircuitBreakerError(pub String);

impl std::fmt::Display for CircuitBreakerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CircuitBreaker error: {}", self.0)
    }
}

impl std::error::Error for CircuitBreakerError {}

// 全局熔断器注册表：所有调用方通过 Arc 共享同一熔断器状态，避免 clone 导致状态不同步
pub static GLOBAL_BREAKERS: Lazy<RwLock<HashMap<String, Arc<RwLock<CircuitBreaker>>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

// 注册熔断器：返回 Arc 共享句柄，调用方与注册表持有同一 Arc，状态实时同步
pub fn register_circuit_breaker(
    name: String,
    threshold: u32,
    rate: f64,
    timeout: Duration,
) -> Arc<RwLock<CircuitBreaker>> {
    let breaker = Arc::new(RwLock::new(CircuitBreaker::new(
        name.clone(),
        threshold,
        rate,
        timeout,
    )));
    let mut breakers = GLOBAL_BREAKERS.write();
    breakers.insert(name, Arc::clone(&breaker));
    breaker
}

// 获取熔断器共享句柄：返回与注册表相同的 Arc，状态实时同步
pub fn get_circuit_breaker(name: &str) -> Option<Arc<RwLock<CircuitBreaker>>> {
    let breakers = GLOBAL_BREAKERS.read();
    breakers.get(name).map(|arc| Arc::clone(arc))
}

// ===================== 重试机制 =====================

/// 重试配置
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// 最大尝试次数（含首次调用，默认 3）
    pub max_attempts: u32,
    /// 基础退避时长（默认 500ms）
    pub base_delay: Duration,
    /// 最大退避时长（默认 10s）
    pub max_delay: Duration,
    /// 是否启用 jitter 抖动（默认 true）
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            jitter: true,
        }
    }
}

impl RetryConfig {
    /// 计算第 `attempt` 次失败后的重试延迟（指数退避 + 可选 jitter）
    ///
    /// - 指数退避：`delay = min(max_delay, base_delay * 2^(attempt-1))`
    /// - RateLimit 错误使用更长退避（×2）
    /// - 启用 jitter 时叠加 ±20% 随机抖动
    pub fn get_delay(&self, attempt: u32, category: &ErrorCategory) -> Duration {
        // 指数退避：base_delay * 2^(attempt-1)，限制指数防止溢出
        let exp = attempt.saturating_sub(1).min(31);
        let multiplier = 1u64 << exp;
        let base_ms = self.base_delay.as_millis() as u64;
        let mut delay_ms = base_ms.saturating_mul(multiplier);

        // RateLimit 错误用更长退避（×2）
        if matches!(category, ErrorCategory::RateLimit) {
            delay_ms = delay_ms.saturating_mul(2);
        }

        // 不超过最大延迟
        let max_ms = self.max_delay.as_millis() as u64;
        delay_ms = delay_ms.min(max_ms);

        if self.jitter {
            // ±20% 随机抖动
            let factor = pseudo_random_factor();
            delay_ms = ((delay_ms as f64) * factor) as u64;
        }

        Duration::from_millis(delay_ms)
    }
}

/// 基于时间的伪随机因子（无 rand 依赖时使用），返回 [0.8, 1.2)
fn pseudo_random_factor() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let r = (nanos as f64) / (u32::MAX as f64);
    0.8 + r * 0.4
}

/// 异步重试：按 `RetryConfig` 对 `operation` 进行指数退避重试
///
/// - Transient/RateLimit 错误会重试，Permanent 错误立即返回
/// - 达到 `max_attempts` 仍失败则返回最后一次错误
pub async fn async_retry<F, Fut, T, E>(config: &RetryConfig, operation: F) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::error::Error,
{
    let mut attempt = 1u32;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let category = classify_error(&err);
                // Permanent 错误不重试
                if matches!(category, ErrorCategory::Permanent) {
                    return Err(err);
                }
                // 达到最大尝试次数，返回错误
                if attempt >= config.max_attempts {
                    return Err(err);
                }
                let delay = config.get_delay(attempt, &category);
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

/// 组合重试 + 熔断器：每次调用前检查熔断器，失败时记录并按配置重试
///
/// - 熔断器打开时直接返回错误
/// - 调用成功记录 success，失败记录 failure
/// - Permanent 错误不重试，其余按 `RetryConfig` 重试
pub async fn with_retry_and_breaker<F, Fut, T, E>(
    breaker: &Arc<RwLock<CircuitBreaker>>,
    config: &RetryConfig,
    operation: F,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut attempt = 1u32;
    loop {
        // 熔断器检查
        {
            let mut b = breaker.write();
            if !b.allow_request() {
                return Err(anyhow::anyhow!("熔断器已打开，拒绝请求: {}", b.name));
            }
        }

        match operation().await {
            Ok(value) => {
                breaker.write().record_success();
                return Ok(value);
            }
            Err(err) => {
                let category = classify_error(&err);
                breaker.write().record_failure();

                if matches!(category, ErrorCategory::Permanent) {
                    return Err(err.into());
                }
                if attempt >= config.max_attempts {
                    return Err(err.into());
                }
                let delay = config.get_delay(attempt, &category);
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}
