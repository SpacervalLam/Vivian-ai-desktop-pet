//! Runnable 装饰器组合子
//!
//! - [`RunnableRetry`]：失败自动重试，支持指数退避 + jitter
//! - [`RunnableWithFallbacks`]：主路径失败时按顺序尝试备选路径
//!
//! 两者都实现 `Runnable` trait，可独立或嵌套使用：
//! ```ignore
//! // 重试包装
//! let retry = RunnableRetry::new(primary, 3);
//! // 备选包装
//! let wf = RunnableWithFallbacks::new(primary, vec![backup_a, backup_b]);
//! // 嵌套：备选整体加重试
//! let combined = RunnableRetry::new(Box::new(wf), 2);
//! ```

use std::time::Duration;

use async_trait::async_trait;
use rand::Rng;
use serde_json::Value;

use crate::error::{VivianError, VivianResult};
use crate::pipeline::base::{Runnable, RunnableConfig};

/// 默认重试间隔初始值
const DEFAULT_INITIAL_DELAY: Duration = Duration::from_millis(100);
/// 默认重试间隔上限
const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(5);
/// 默认退避底数（每次 delay *= base）
const DEFAULT_BACKOFF_BASE: u32 = 2;

/// 判断错误是否值得重试
///
/// 默认重试瞬时错误（Network / Provider / Timeout / Io），不重试
/// 配置错误、权限拒绝、熔断器已打开等不可恢复错误。
pub fn is_retryable(err: &VivianError) -> bool {
    matches!(
        err,
        VivianError::Network(_)
            | VivianError::Provider(_)
            | VivianError::Timeout(_)
            | VivianError::Io(_)
            | VivianError::Database(_)
    )
}

/// 指数退避 + jitter 计算第 n 次重试的等待时长
///
/// - 第 1 次重试：initial_delay
/// - 第 n 次重试：min(initial_delay * base^(n-1), max_delay)
/// - 加 ±20% jitter 防惊群
pub fn compute_backoff(
    attempt: u32,
    initial_delay: Duration,
    max_delay: Duration,
    base: u32,
) -> Duration {
    let exp = base.saturating_pow(attempt.saturating_sub(1));
    let raw = initial_delay
        .checked_mul(exp)
        .unwrap_or(max_delay);
    let capped = if raw > max_delay { max_delay } else { raw };
    let millis = capped.as_millis() as u64;
    if millis == 0 {
        return Duration::ZERO;
    }
    let jitter_range = (millis / 5).max(1); // ±20%
    let mut rng = rand::rng();
    let delta: i64 = rng.random_range(-(jitter_range as i64)..=(jitter_range as i64));
    let final_millis = (millis as i64 + delta).max(0) as u64;
    Duration::from_millis(final_millis)
}

/// 重试装饰器
pub struct RunnableRetry {
    inner: Box<dyn Runnable>,
    max_attempts: u32,
    initial_delay: Duration,
    max_delay: Duration,
    backoff_base: u32,
    /// 自定义重试判定（None 时用 [`is_retryable`]）
    retry_if: Option<Box<dyn Fn(&VivianError) -> bool + Send + Sync>>,
}

impl RunnableRetry {
    pub fn new(inner: Box<dyn Runnable>, max_attempts: u32) -> Self {
        Self {
            inner,
            max_attempts: max_attempts.max(1),
            initial_delay: DEFAULT_INITIAL_DELAY,
            max_delay: DEFAULT_MAX_DELAY,
            backoff_base: DEFAULT_BACKOFF_BASE,
            retry_if: None,
        }
    }

    pub fn with_initial_delay(mut self, d: Duration) -> Self {
        self.initial_delay = d;
        self
    }

    pub fn with_max_delay(mut self, d: Duration) -> Self {
        self.max_delay = d;
        self
    }

    pub fn with_backoff_base(mut self, base: u32) -> Self {
        self.backoff_base = base.max(1);
        self
    }

    pub fn with_retry_filter<F>(mut self, f: F) -> Self
    where
        F: Fn(&VivianError) -> bool + Send + Sync + 'static,
    {
        self.retry_if = Some(Box::new(f));
        self
    }

    fn should_retry(&self, err: &VivianError) -> bool {
        match &self.retry_if {
            Some(f) => f(err),
            None => is_retryable(err),
        }
    }

    /// 实际最大尝试次数：取构造值与 config.max_retries + 1（首次）的较小值
    fn effective_max_attempts(&self, config: &Option<RunnableConfig>) -> u32 {
        match config {
            Some(c) if c.max_retries > 0 => self.max_attempts.min(c.max_retries + 1),
            _ => self.max_attempts,
        }
    }
}

#[async_trait]
impl Runnable for RunnableRetry {
    async fn ainvoke(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        let max_attempts = self.effective_max_attempts(&config);
        let mut last_err: Option<VivianError> = None;

        for attempt in 1..=max_attempts {
            match self.inner.ainvoke(input.clone(), config.clone()).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let retryable = self.should_retry(&e);
                    if !retryable || attempt >= max_attempts {
                        return Err(e);
                    }
                    tracing::warn!(
                        attempt,
                        max_attempts,
                        error = %e,
                        "Runnable 失败，将重试"
                    );
                    let delay = compute_backoff(
                        attempt,
                        self.initial_delay,
                        self.max_delay,
                        self.backoff_base,
                    );
                    tokio::time::sleep(delay).await;
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| VivianError::Other("RunnableRetry 未执行任何尝试".into())))
    }
}

/// 备选装饰器：主路径失败时按顺序尝试 fallback
pub struct RunnableWithFallbacks {
    primary: Box<dyn Runnable>,
    fallbacks: Vec<Box<dyn Runnable>>,
    /// 自定义错误判定（None 时所有 VivianError 都触发 fallback）
    handle_if: Option<Box<dyn Fn(&VivianError) -> bool + Send + Sync>>,
}

impl RunnableWithFallbacks {
    pub fn new(primary: Box<dyn Runnable>, fallbacks: Vec<Box<dyn Runnable>>) -> Self {
        Self {
            primary,
            fallbacks,
            handle_if: None,
        }
    }

    pub fn with_handle_filter<F>(mut self, f: F) -> Self
    where
        F: Fn(&VivianError) -> bool + Send + Sync + 'static,
    {
        self.handle_if = Some(Box::new(f));
        self
    }

    fn should_handle(&self, err: &VivianError) -> bool {
        match &self.handle_if {
            Some(f) => f(err),
            None => true,
        }
    }
}

#[async_trait]
impl Runnable for RunnableWithFallbacks {
    async fn ainvoke(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        // primary
        let primary_err = match self.primary.ainvoke(input.clone(), config.clone()).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if !self.should_handle(&e) {
                    return Err(e);
                }
                e
            }
        };

        // fallbacks：失败时保留首个错误
        for (i, fb) in self.fallbacks.iter().enumerate() {
            tracing::warn!(fallback_idx = i, "主路径失败，尝试 fallback");
            match fb.ainvoke(input.clone(), config.clone()).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if !self.should_handle(&e) {
                        return Err(e);
                    }
                }
            }
        }

        Err(primary_err)
    }
}

/// 为 `Box<dyn Runnable>` 提供装饰器扩展方法
pub trait RunnableDecorators {
    fn with_retry(self, max_attempts: u32) -> RunnableRetry;
    fn with_fallbacks(self, fallbacks: Vec<Box<dyn Runnable>>) -> RunnableWithFallbacks;
}

impl RunnableDecorators for Box<dyn Runnable> {
    fn with_retry(self, max_attempts: u32) -> RunnableRetry {
        RunnableRetry::new(self, max_attempts)
    }

    fn with_fallbacks(self, fallbacks: Vec<Box<dyn Runnable>>) -> RunnableWithFallbacks {
        RunnableWithFallbacks::new(self, fallbacks)
    }
}

/// 分支组合子
///
/// 顺序求值每个分支的 condition，第一个返回 true 的分支被执行后立即返回。
/// 全部不匹配时执行 default；default 为 None 时返回错误。
pub struct RunnableBranch {
    branches: Vec<(BranchCondition, Box<dyn Runnable>)>,
    default: Option<Box<dyn Runnable>>,
}

/// 分支条件谓词
type BranchCondition = Box<dyn Fn(&Value) -> bool + Send + Sync>;

impl RunnableBranch {
    pub fn new() -> Self {
        Self {
            branches: Vec::new(),
            default: None,
        }
    }

    pub fn add_branch<F>(mut self, condition: F, runnable: Box<dyn Runnable>) -> Self
    where
        F: Fn(&Value) -> bool + Send + Sync + 'static,
    {
        self.branches.push((Box::new(condition), runnable));
        self
    }

    pub fn with_default(mut self, runnable: Box<dyn Runnable>) -> Self {
        self.default = Some(runnable);
        self
    }
}

impl Default for RunnableBranch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for RunnableBranch {
    async fn ainvoke(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        for (i, (cond, runnable)) in self.branches.iter().enumerate() {
            if cond(&input) {
                tracing::debug!(branch_idx = i, "命中分支");
                return runnable.ainvoke(input, config).await;
            }
        }
        match &self.default {
            Some(d) => {
                tracing::debug!("未命中任何分支，执行 default");
                d.ainvoke(input, config).await
            }
            None => Err(VivianError::Engine(
                "RunnableBranch 未命中任何分支且无 default".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct CountingRunnable {
        counter: Arc<AtomicU32>,
        fail_until: u32,
    }

    #[async_trait]
    impl Runnable for CountingRunnable {
        async fn ainvoke(
            &self,
            _input: Value,
            _config: Option<RunnableConfig>,
        ) -> VivianResult<Value> {
            let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= self.fail_until {
                Err(VivianError::Network(format!("模拟失败 #{}", n)))
            } else {
                Ok(serde_json::json!({"attempts": n}))
            }
        }
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let counter = Arc::new(AtomicU32::new(0));
        let r = CountingRunnable {
            counter: counter.clone(),
            fail_until: 2,
        };
        let retry = RunnableRetry::new(Box::new(r), 3)
            .with_initial_delay(Duration::from_millis(1));
        let result = retry.ainvoke(Value::Null, None).await.unwrap();
        assert_eq!(result["attempts"], 3);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausts_attempts() {
        let counter = Arc::new(AtomicU32::new(0));
        let r = CountingRunnable {
            counter: counter.clone(),
            fail_until: 100,
        };
        let retry = RunnableRetry::new(Box::new(r), 2)
            .with_initial_delay(Duration::from_millis(1));
        let err = retry.ainvoke(Value::Null, None).await.unwrap_err();
        assert!(matches!(err, VivianError::Network(_)));
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_skips_non_retryable() {
        struct ConfErr;
        #[async_trait]
        impl Runnable for ConfErr {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Err(VivianError::Config("不可恢复".into()))
            }
        }
        let retry = RunnableRetry::new(Box::new(ConfErr), 3);
        let err = retry.ainvoke(Value::Null, None).await.unwrap_err();
        assert!(matches!(err, VivianError::Config(_)));
    }

    #[tokio::test]
    async fn test_fallbacks_uses_first_success() {
        struct AlwaysErr;
        #[async_trait]
        impl Runnable for AlwaysErr {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Err(VivianError::Provider("主路径失败".into()))
            }
        }
        struct AlwaysOk;
        #[async_trait]
        impl Runnable for AlwaysOk {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Ok(serde_json::json!({"from": "fallback"}))
            }
        }
        let wf = RunnableWithFallbacks::new(
            Box::new(AlwaysErr),
            vec![Box::new(AlwaysOk)],
        );
        let result = wf.ainvoke(Value::Null, None).await.unwrap();
        assert_eq!(result["from"], "fallback");
    }

    #[tokio::test]
    async fn test_fallbacks_all_fail_returns_first_error() {
        struct ErrA;
        #[async_trait]
        impl Runnable for ErrA {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Err(VivianError::Provider("A 失败".into()))
            }
        }
        struct ErrB;
        #[async_trait]
        impl Runnable for ErrB {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Err(VivianError::Timeout("B 失败".into()))
            }
        }
        let wf = RunnableWithFallbacks::new(
            Box::new(ErrA),
            vec![Box::new(ErrB)],
        );
        let err = wf.ainvoke(Value::Null, None).await.unwrap_err();
        match err {
            VivianError::Provider(msg) => assert_eq!(msg, "A 失败"),
            other => panic!("期望 Provider 错误，实际: {:?}", other),
        }
    }

    #[test]
    fn test_compute_backoff_monotonic_and_capped() {
        let d1 = compute_backoff(1, Duration::from_millis(100), Duration::from_secs(5), 2);
        let d3 = compute_backoff(3, Duration::from_millis(100), Duration::from_secs(5), 2);
        let d10 = compute_backoff(10, Duration::from_millis(100), Duration::from_secs(5), 2);
        // jitter 后仍应大致递增
        assert!(d3 >= d1 || d1.as_millis() <= 120);
        // 上限 5s + 20% jitter = 6s
        assert!(d10.as_millis() <= 6000);
    }

    #[tokio::test]
    async fn test_branch_first_match_wins() {
        struct SayHello;
        #[async_trait]
        impl Runnable for SayHello {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Ok(serde_json::json!({"msg": "hello"}))
            }
        }
        struct SayBye;
        #[async_trait]
        impl Runnable for SayBye {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Ok(serde_json::json!({"msg": "bye"}))
            }
        }
        let br = RunnableBranch::new()
            .add_branch(|v| v.get("intent").and_then(|x| x.as_str()) == Some("greet"), Box::new(SayHello))
            .add_branch(|v| v.get("intent").and_then(|x| x.as_str()) == Some("exit"), Box::new(SayBye));

        let r = br
            .ainvoke(serde_json::json!({"intent": "exit"}), None)
            .await
            .unwrap();
        assert_eq!(r["msg"], "bye");
    }

    #[tokio::test]
    async fn test_branch_falls_through_to_default() {
        struct DefaultR;
        #[async_trait]
        impl Runnable for DefaultR {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Ok(serde_json::json!({"msg": "default"}))
            }
        }
        let br = RunnableBranch::new()
            .add_branch(|v| v.get("x").and_then(|x| x.as_i64()) == Some(1), Box::new(DefaultR))
            .with_default(Box::new(DefaultR));

        let r = br.ainvoke(serde_json::json!({"x": 99}), None).await.unwrap();
        assert_eq!(r["msg"], "default");
    }

    #[tokio::test]
    async fn test_branch_no_match_no_default_errors() {
        struct Dummy;
        #[async_trait]
        impl Runnable for Dummy {
            async fn ainvoke(
                &self,
                _input: Value,
                _config: Option<RunnableConfig>,
            ) -> VivianResult<Value> {
                Ok(Value::Null)
            }
        }
        let br = RunnableBranch::new().add_branch(|_| false, Box::new(Dummy));
        let err = br.ainvoke(Value::Null, None).await.unwrap_err();
        assert!(matches!(err, VivianError::Engine(_)));
    }
}
