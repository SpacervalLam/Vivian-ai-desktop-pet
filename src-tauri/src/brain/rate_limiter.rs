//! 线程安全的令牌桶限流器。
//!
//! - 经典令牌桶：每 1/rate 秒生成一个 token，最多 capacity 个
//! - [`TokenBucketRateLimiter::acquire`] 非阻塞，限流时直接返回 `false`（快失败）
//! - [`TokenBucketRateLimiter::acquire_async`] 异步等待，直到拿到 token 或超时
//! - [`RateLimiterRegistry`] 支持多 provider 独立限流
//!
//! 软限流：仅减少 LLM 调用频次，不保留降级路径。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// 限流配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// 每秒允许的请求数（token 生成速率）
    pub rate_per_second: f64,
    /// 桶容量（最大突发）
    pub capacity: u32,
    /// 起始填充数（None = 满桶）
    pub initial_tokens: Option<u32>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            rate_per_second: 5.0,
            capacity: 10,
            initial_tokens: None,
        }
    }
}

impl RateLimitConfig {
    /// 每分钟 N 次请求的便捷构造。
    pub fn per_minute(per_minute: u32) -> Self {
        Self {
            rate_per_second: per_minute as f64 / 60.0,
            capacity: per_minute.max(1),
            initial_tokens: None,
        }
    }
}

/// 线程安全的令牌桶限流器。
///
/// 非阻塞 `acquire` + 可选异步等待。
pub struct TokenBucketRateLimiter {
    config: RateLimitConfig,
    name: String,
    inner: Mutex<BucketState>,
}

struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucketRateLimiter {
    pub fn new(config: RateLimitConfig, name: impl Into<String>) -> Self {
        let initial = config
            .initial_tokens
            .map(|n| n as f64)
            .unwrap_or(config.capacity as f64);
        Self {
            config,
            name: name.into(),
            inner: Mutex::new(BucketState {
                tokens: initial,
                last_refill: Instant::now(),
            }),
        }
    }

    /// 名称（用于日志/监控）。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 配置快照。
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    /// 内部：补充 token（需持锁）。
    fn refill_locked(state: &mut BucketState, config: &RateLimitConfig) {
        let now = Instant::now();
        let elapsed = now.duration_since(state.last_refill);
        if elapsed.as_nanos() == 0 {
            return;
        }
        let add = elapsed.as_secs_f64() * config.rate_per_second;
        state.tokens = (state.tokens + add).min(config.capacity as f64);
        state.last_refill = now;
    }

    /// 尝试获取 token。返回 `true` = 放行，`false` = 被限流。
    pub fn acquire(&self, tokens: u32) -> bool {
        let mut state = self.inner.lock();
        Self::refill_locked(&mut state, &self.config);
        if state.tokens >= tokens as f64 {
            state.tokens -= tokens as f64;
            return true;
        }
        false
    }

    /// 异步等待直到获取到 token，或超时返回 `false`。
    ///
    /// 对齐任务描述中的"异步等待（tokio::time::sleep）"。
    pub async fn acquire_async(&self, tokens: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.acquire(tokens) {
                return true;
            }
            // 计算下一次有 token 可用需要等待的时间
            let wait_ms = {
                let state = self.inner.lock();
                let deficit = tokens as f64 - state.tokens;
                if deficit <= 0.0 || self.config.rate_per_second <= 0.0 {
                    50
                } else {
                    ((deficit / self.config.rate_per_second) * 1000.0) as u64 + 10
                }
            };
            let now = Instant::now();
            if now + Duration::from_millis(wait_ms) > deadline {
                // 最后一搏
                if self.acquire(tokens) {
                    return true;
                }
                return false;
            }
            tokio::time::sleep(Duration::from_millis(wait_ms.min(200))).await;
        }
    }

    /// 当前可用 token 数（用于测试与监控）。
    pub fn available_tokens(&self) -> f64 {
        let mut state = self.inner.lock();
        Self::refill_locked(&mut state, &self.config);
        state.tokens
    }

    /// 重置桶（用于测试）。
    pub fn reset(&self) {
        let mut state = self.inner.lock();
        state.tokens = self.config.capacity as f64;
        state.last_refill = Instant::now();
    }
}

/// 多 provider 独立限流注册表。
///
/// 对齐任务描述"多 provider 独立限流"：每个 provider 名独立持有一个
/// `TokenBucketRateLimiter`，互不影响。
pub struct RateLimiterRegistry {
    limiters: Mutex<HashMap<String, TokenBucketRateLimiter>>,
    default_config: RateLimitConfig,
}

impl RateLimiterRegistry {
    pub fn new(default_config: RateLimitConfig) -> Self {
        Self {
            limiters: Mutex::new(HashMap::new()),
            default_config,
        }
    }

    /// 注册或更新某个 provider 的限流配置。
    pub fn register(&self, provider: impl Into<String>, config: RateLimitConfig) {
        let name = provider.into();
        let limiter = TokenBucketRateLimiter::new(config, name.clone());
        self.limiters.lock().insert(name, limiter);
    }

    /// 尝试获取 provider 的 token（非阻塞）。
    pub fn acquire(&self, provider: &str, tokens: u32) -> bool {
        let mut limiters = self.limiters.lock();
        let limiter = limiters
            .entry(provider.to_string())
            .or_insert_with(|| TokenBucketRateLimiter::new(self.default_config.clone(), provider));
        limiter.acquire(tokens)
    }

    /// 异步等待获取 provider 的 token。
    pub async fn acquire_async(&self, provider: &str, tokens: u32, timeout: Duration) -> bool {
        // 不能在持锁状态下 await，故先克隆配置，再创建临时限流器。
        // 但这会丢失状态 —— 我们改用循环 acquire + sleep 的方式，
        // 避免在 await 期间持锁。
        let deadline = Instant::now() + timeout;
        loop {
            {
                let mut limiters = self.limiters.lock();
                let limiter = limiters.entry(provider.to_string()).or_insert_with(|| {
                    TokenBucketRateLimiter::new(self.default_config.clone(), provider)
                });
                if limiter.acquire(tokens) {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// 列出所有已注册 provider 的可用 token 数（监控用）。
    pub fn snapshot(&self) -> HashMap<String, f64> {
        let limiters = self.limiters.lock();
        limiters.iter().map(|(k, v)| (k.clone(), v.available_tokens())).collect()
    }
}

impl Default for RateLimiterRegistry {
    fn default() -> Self {
        Self::new(RateLimitConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acquire_basic() {
        let limiter = TokenBucketRateLimiter::new(
            RateLimitConfig {
                rate_per_second: 10.0,
                capacity: 2,
                initial_tokens: Some(2),
            },
            "test",
        );
        assert!(limiter.acquire(1));
        assert!(limiter.acquire(1));
        // 第三次应当被限流
        assert!(!limiter.acquire(1));
    }

    #[test]
    fn test_refill_over_time() {
        let limiter = TokenBucketRateLimiter::new(
            RateLimitConfig {
                rate_per_second: 100.0,
                capacity: 1,
                initial_tokens: Some(0),
            },
            "test",
        );
        // 初始为 0，应当被限流
        assert!(!limiter.acquire(1));
        // 等待一段时间后应当能拿到
        std::thread::sleep(Duration::from_millis(30));
        assert!(limiter.acquire(1));
    }

    #[tokio::test]
    async fn test_acquire_async_waits() {
        let limiter = TokenBucketRateLimiter::new(
            RateLimitConfig {
                rate_per_second: 50.0,
                capacity: 1,
                initial_tokens: Some(0),
            },
            "test",
        );
        // 应当在 ~20ms 后拿到 token
        let got = limiter.acquire_async(1, Duration::from_millis(500)).await;
        assert!(got);
    }

    #[test]
    fn test_registry_multi_provider() {
        let registry = RateLimiterRegistry::new(RateLimitConfig {
            rate_per_second: 1.0,
            capacity: 1,
            initial_tokens: Some(1),
        });
        // 两个 provider 各自独立
        assert!(registry.acquire("openai", 1));
        assert!(!registry.acquire("openai", 1)); // openai 已耗尽
        assert!(registry.acquire("gemini", 1)); // gemini 仍有
    }

    #[test]
    fn test_per_minute_helper() {
        let cfg = RateLimitConfig::per_minute(60);
        assert!((cfg.rate_per_second - 1.0).abs() < 1e-6);
        assert_eq!(cfg.capacity, 60);
    }
}
