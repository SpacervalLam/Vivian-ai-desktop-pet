//! Advisor 拦截器链 —— 围绕 Runnable 的横切关注点解耦。
//!
//! 设计：每个 Advisor 是一个 around 钩子，按 `order()` 升序执行，可改写输入、跳过下游、
//! 改写输出或记录指标。`AdvisorChain` 把多个 Advisor 串成链，最终调用被包裹的 Runnable。
//!
//! 当前内置 Advisor：
//! - [`LoggingAdvisor`]：统一请求/响应/耗时日志（替代散落的 tracing 调用）
//! - [`RateLimitAdvisor`]：基于 [`crate::brain::rate_limiter::RateLimiterRegistry`] 的软限流
//! - [`Re2Advisor`]：Re2 重读增强（仅对 reasoning 任务启用）
//! - [`LoopDetectionAdvisor`]：检测重复输出并注入"换策略"提示
//!
//! 注意：违禁词/敏感词过滤已按"自用桌宠"场景剔除。

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value;

use crate::brain::rate_limiter::RateLimiterRegistry;
use crate::error::{VivianError, VivianResult};
use crate::pipeline::base::{Runnable, RunnableConfig};

/// Advisor around 钩子。
///
/// `req` 是当前 PipelineState 的 JSON 表示；`next` 是链中下一个 Advisor 或最终 Runnable。
/// Advisor 可以：
/// - 改写 `req` 后透传给 `next`
/// - 不调 `next` 直接返回（短路，如限流拒绝）
/// - 修改 `next` 返回的结果
#[async_trait]
pub trait Advisor: Send + Sync {
    /// 标识符，用于日志/监控。
    fn name(&self) -> &str;

    /// 执行顺序，升序（负数先执行，最外层）。
    fn order(&self) -> i32;

    /// around 调用。
    async fn around_invoke(
        &self,
        req: Value,
        config: Option<RunnableConfig>,
        next: &dyn AdvisorNext,
    ) -> VivianResult<Value>;
}

/// 链中"下一跳"抽象，让 Advisor 可以递归调用剩余链路或最终 Runnable。
#[async_trait]
pub trait AdvisorNext: Send + Sync {
    async fn invoke(&self, req: Value, config: Option<RunnableConfig>) -> VivianResult<Value>;
}

/// 把剩余 Advisor 链包成下一跳。
struct ChainNext {
    advisors: Vec<Arc<dyn Advisor>>,
    /// 当前 Advisor 索引。
    idx: usize,
    /// 链末端 Runnable。
    terminal: Arc<dyn AdvisorNext>,
}

#[async_trait]
impl AdvisorNext for ChainNext {
    async fn invoke(&self, req: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        if self.idx >= self.advisors.len() {
            return self.terminal.invoke(req, config).await;
        }
        let cur = self.advisors[self.idx].clone();
        let next = ChainNext {
            advisors: self.advisors.clone(),
            idx: self.idx + 1,
            terminal: self.terminal.clone(),
        };
        cur.around_invoke(req, config, &next).await
    }
}

/// Advisor 链：按 `order()` 升序执行所有 Advisor，最终调用被包裹的 Runnable。
pub struct AdvisorChain {
    advisors: Vec<Arc<dyn Advisor>>,
    inner: Arc<dyn Runnable>,
}

impl AdvisorChain {
    pub fn new(inner: Arc<dyn Runnable>) -> Self {
        Self {
            advisors: Vec::new(),
            inner,
        }
    }

    /// 添加 Advisor（不会立即排序，构造完成后调用 [`Self::build`]）。
    pub fn with_advisor(mut self, advisor: Arc<dyn Advisor>) -> Self {
        self.advisors.push(advisor);
        self
    }

    /// 排序并冻结链。未调用则首次 ainvoke 时按当前顺序执行（不排序）。
    pub fn build(mut self) -> Self {
        self.advisors.sort_by_key(|a| a.order());
        self
    }
}

#[async_trait]
impl Runnable for AdvisorChain {
    async fn ainvoke(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        if self.advisors.is_empty() {
            return self.inner.ainvoke(input, config).await;
        }
        let terminal = Arc::new(AdvisorChainBridge {
            inner: self.inner.clone(),
        });
        let next = ChainNext {
            advisors: self.advisors.clone(),
            idx: 0,
            terminal,
        };
        next.invoke(input, config).await
    }
}

/// 把 `Arc<dyn Runnable>` 桥接成 `AdvisorNext` 的内部适配器。
struct AdvisorChainBridge {
    inner: Arc<dyn Runnable>,
}

#[async_trait]
impl AdvisorNext for AdvisorChainBridge {
    async fn invoke(&self, req: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        self.inner.ainvoke(req, config).await
    }
}

// ============================================================================
// LoggingAdvisor：统一请求/响应/耗时日志
// ============================================================================

/// 统一日志 Advisor —— 记录请求摘要、响应摘要、总耗时。
///
/// order = -100，作为最外层 Advisor，包住所有其他 Advisor 与 Runnable。
pub struct LoggingAdvisor {
    name: String,
}

impl LoggingAdvisor {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Advisor for LoggingAdvisor {
    fn name(&self) -> &str {
        &self.name
    }

    fn order(&self) -> i32 {
        -100
    }

    async fn around_invoke(
        &self,
        req: Value,
        config: Option<RunnableConfig>,
        next: &dyn AdvisorNext,
    ) -> VivianResult<Value> {
        let start = std::time::Instant::now();
        let user_input = req
            .get("user_input")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(80)
            .collect::<String>();
        let task_type = config
            .as_ref()
            .map(|c| c.task_type())
            .unwrap_or_else(|| "chat".to_string());

        tracing::info!(
            advisor = %self.name,
            task_type = %task_type,
            input_preview = %user_input,
            "advisor_chain: invoke start"
        );

        let result = next.invoke(req, config).await;
        let elapsed = start.elapsed();

        match &result {
            Ok(resp) => {
                let text = resp
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>();
                tracing::info!(
                    advisor = %self.name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    output_preview = %text,
                    "advisor_chain: invoke complete"
                );
            }
            Err(e) => {
                tracing::warn!(
                    advisor = %self.name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "advisor_chain: invoke failed"
                );
            }
        }
        result
    }
}

// ============================================================================
// RateLimitAdvisor：软限流
// ============================================================================

/// 软限流 Advisor —— 基于令牌桶，限流时返回错误而非降级。
///
/// order = -50，在日志 Advisor 内层、业务 Advisor 外层。
pub struct RateLimitAdvisor {
    name: String,
    registry: Arc<RateLimiterRegistry>,
    /// 任务类型到 provider 名的映射（同一任务用同一桶）。
    task_provider: RwLock<std::collections::HashMap<String, String>>,
    default_provider: String,
}

impl RateLimitAdvisor {
    pub fn new(registry: Arc<RateLimiterRegistry>, default_provider: impl Into<String>) -> Self {
        Self {
            name: "rate_limit".to_string(),
            registry,
            task_provider: RwLock::new(std::collections::HashMap::new()),
            default_provider: default_provider.into(),
        }
    }

    /// 为指定任务类型指定 provider 名（不同任务可共享或独立桶）。
    pub fn with_task_provider(self, task: impl Into<String>, provider: impl Into<String>) -> Self {
        self.task_provider
            .write()
            .insert(task.into(), provider.into());
        self
    }

    fn provider_for(&self, task: &str) -> String {
        self.task_provider
            .read()
            .get(task)
            .cloned()
            .unwrap_or_else(|| self.default_provider.clone())
    }
}

#[async_trait]
impl Advisor for RateLimitAdvisor {
    fn name(&self) -> &str {
        &self.name
    }

    fn order(&self) -> i32 {
        -50
    }

    async fn around_invoke(
        &self,
        req: Value,
        config: Option<RunnableConfig>,
        next: &dyn AdvisorNext,
    ) -> VivianResult<Value> {
        let task = config
            .as_ref()
            .map(|c| c.task_type())
            .unwrap_or_else(|| "chat".to_string());
        let provider = self.provider_for(&task);
        // 1 token = 1 次调用；非阻塞 acquire，限流时直接返回错误
        if !self.registry.acquire(&provider, 1) {
            tracing::warn!(
                advisor = %self.name,
                task_type = %task,
                provider = %provider,
                "rate limited"
            );
            return Err(VivianError::Engine(format!(
                "请求被限流（task={}, provider={}），请稍后再试",
                task, provider
            )));
        }
        next.invoke(req, config).await
    }
}

// ============================================================================
// Re2Advisor：Re2 重读增强
// ============================================================================

/// Re2 重读增强 Advisor —— 在用户输入末尾追加"Read the question again: {q}"。
///
/// 仅对 reasoning 类任务启用（通过 `RunnableConfig::task_type()` 判断）。
/// 依据：重读提问能改善多步推理的作答质量。
///
/// order = 0，在限流之后、循环检测之前。
pub struct Re2Advisor {
    name: String,
    /// 启用 Re2 的任务类型列表（默认 ["reasoning"]）。
    enabled_tasks: RwLock<Vec<String>>,
}

impl Re2Advisor {
    pub fn new() -> Self {
        Self {
            name: "re2".to_string(),
            enabled_tasks: RwLock::new(vec!["reasoning".to_string()]),
        }
    }

    pub fn with_enabled_tasks(self, tasks: Vec<String>) -> Self {
        *self.enabled_tasks.write() = tasks;
        self
    }
}

impl Default for Re2Advisor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Advisor for Re2Advisor {
    fn name(&self) -> &str {
        &self.name
    }

    fn order(&self) -> i32 {
        0
    }

    async fn around_invoke(
        &self,
        mut req: Value,
        config: Option<RunnableConfig>,
        next: &dyn AdvisorNext,
    ) -> VivianResult<Value> {
        let task = config
            .as_ref()
            .map(|c| c.task_type())
            .unwrap_or_else(|| "chat".to_string());
        let enabled = self.enabled_tasks.read().iter().any(|t| t == &task);
        if !enabled {
            return next.invoke(req, config).await;
        }

        // 取出 user_input，追加 Re2 后写回
        if let Some(obj) = req.as_object_mut() {
            if let Some(user_input) = obj
                .get("user_input")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            {
                if !user_input.trim().is_empty() {
                    let augmented = format!(
                        "{}\nRead the question again: {}",
                        user_input, user_input
                    );
                    obj.insert(
                        "user_input".to_string(),
                        Value::String(augmented),
                    );
                }
            }
        }
        next.invoke(req, config).await
    }
}

// ============================================================================
// LoopDetectionAdvisor：循环检测 + 策略注入
// ============================================================================

/// 循环检测 Advisor —— 维护最近 N 条 assistant 输出，命中重复时注入"换策略"提示。
///
/// 工作机制：
/// 1. 调用下游拿到响应，提取 `text` 字段
/// 2. 与最近 `window` 条 assistant 输出对比，若与任一完全相同（或相似度 ≥ threshold）则命中
/// 3. 命中时把"换策略"提示写入 `system_prompt_extension`，再次调用下游重试
/// 4. 单次请求最多重试 `max_retries` 次
///
/// order = 100，作为最内层 Advisor，紧贴生成 Runnable。
pub struct LoopDetectionAdvisor {
    name: String,
    /// 历史输出窗口（最近 N 条 assistant text）
    history: Arc<RwLock<Vec<String>>>,
    /// 窗口大小
    window: usize,
    /// 完全相同即命中的阈值（归一化等价比较）
    /// 重复命中后单次请求的最大重试次数
    max_retries: usize,
    /// 注入到 system_prompt_extension 的策略提示
    strategy_prompt: String,
}

impl LoopDetectionAdvisor {
    pub fn new(window: usize, max_retries: usize) -> Self {
        Self {
            name: "loop_detection".to_string(),
            history: Arc::new(RwLock::new(Vec::with_capacity(window))),
            window,
            max_retries,
            strategy_prompt: "观察到刚才的回复可能与之前重复。请尝试新的角度或表达方式，避免与近期回复雷同。"
                .to_string(),
        }
    }

    /// 自定义策略提示。
    pub fn with_strategy_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.strategy_prompt = prompt.into();
        self
    }

    /// 归一化比较：trim + 转小写 + 折叠空白。
    fn normalize(s: &str) -> String {
        s.trim()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// 检查候选输出是否与历史窗口中任一条重复。
    fn is_duplicate(&self, candidate: &str) -> bool {
        let normalized = Self::normalize(candidate);
        if normalized.is_empty() {
            return false;
        }
        let history = self.history.read();
        history.iter().any(|h| Self::normalize(h) == normalized)
    }

    /// 把一条 assistant 输出推入历史窗口。
    fn push_history(&self, text: &str) {
        let mut history = self.history.write();
        if history.len() >= self.window {
            history.remove(0);
        }
        history.push(text.to_string());
    }
}

#[async_trait]
impl Advisor for LoopDetectionAdvisor {
    fn name(&self) -> &str {
        &self.name
    }

    fn order(&self) -> i32 {
        100
    }

    async fn around_invoke(
        &self,
        req: Value,
        config: Option<RunnableConfig>,
        next: &dyn AdvisorNext,
    ) -> VivianResult<Value> {
        // 命令类请求不做循环检测
        let is_command = req
            .get("is_command")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || req
                .get("metadata")
                .and_then(|m| m.get("is_command"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        if is_command {
            return next.invoke(req, config).await;
        }

        let mut attempts = 0;
        let mut current_req = req.clone();
        loop {
            let resp = next.invoke(current_req.clone(), config.clone()).await?;
            let text = resp
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if text.trim().is_empty() || !self.is_duplicate(&text) {
                // 非重复：写入历史并返回
                self.push_history(&text);
                return Ok(resp);
            }

            // 命中重复
            attempts += 1;
            tracing::warn!(
                advisor = %self.name,
                attempt = attempts,
                max_retries = self.max_retries,
                preview = %text.chars().take(80).collect::<String>(),
                "loop detected, injecting strategy prompt"
            );

            if attempts > self.max_retries {
                // 超出重试上限：直接返回当前结果（避免无限循环），但仍记录历史
                self.push_history(&text);
                return Ok(resp);
            }

            // 注入策略提示到 system_prompt_extension，重试
            if let Some(obj) = current_req.as_object_mut() {
                let ext = obj
                    .get("system_prompt_extension")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let new_ext = if ext.is_empty() {
                    self.strategy_prompt.clone()
                } else {
                    format!("{}\n{}", ext, self.strategy_prompt)
                };
                obj.insert(
                    "system_prompt_extension".to_string(),
                    Value::String(new_ext),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::base::RunnableLambda;

    struct StubNext {
        response: Value,
    }

    #[async_trait]
    impl AdvisorNext for StubNext {
        async fn invoke(&self, _req: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn logging_advisor_passes_through() {
        let advisor = LoggingAdvisor::new("log");
        let stub = StubNext {
            response: serde_json::json!({ "text": "hello" }),
        };
        let req = serde_json::json!({ "user_input": "hi" });
        let resp = advisor.around_invoke(req, None, &stub).await.unwrap();
        assert_eq!(resp.get("text").unwrap().as_str().unwrap(), "hello");
    }

    #[tokio::test]
    async fn re2_advisor_augments_input_for_reasoning() {
        let advisor = Re2Advisor::new();
        let captured = Arc::new(RwLock::new(Value::Null));
        let captured_clone = captured.clone();
        let next = RepeatingNext {
            captured: captured_clone,
            response: serde_json::json!({ "text": "ok" }),
        };
        let mut config = RunnableConfig::default();
        config.metadata = serde_json::json!({ "task_type": "reasoning" });
        let req = serde_json::json!({ "user_input": "什么是质数" });
        advisor.around_invoke(req, Some(config), &next).await.unwrap();
        let passed = captured.read().clone();
        let input = passed.get("user_input").unwrap().as_str().unwrap();
        assert!(input.contains("Read the question again"));
    }

    #[tokio::test]
    async fn re2_advisor_skips_chat_task() {
        let advisor = Re2Advisor::new();
        let captured = Arc::new(RwLock::new(Value::Null));
        let captured_clone = captured.clone();
        let next = RepeatingNext {
            captured: captured_clone,
            response: serde_json::json!({ "text": "ok" }),
        };
        let mut config = RunnableConfig::default();
        config.metadata = serde_json::json!({ "task_type": "chat" });
        let req = serde_json::json!({ "user_input": "你好" });
        advisor.around_invoke(req, Some(config), &next).await.unwrap();
        let passed = captured.read().clone();
        let input = passed.get("user_input").unwrap().as_str().unwrap();
        assert_eq!(input, "你好");
    }

    struct RepeatingNext {
        captured: Arc<RwLock<Value>>,
        response: Value,
    }

    #[async_trait]
    impl AdvisorNext for RepeatingNext {
        async fn invoke(&self, req: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
            *self.captured.write() = req;
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn loop_detection_retries_on_duplicate() {
        let advisor = LoopDetectionAdvisor::new(5, 2);
        // 预置历史
        advisor.push_history("你好");

        // 第一次返回重复，第二次返回新内容
        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count_clone = call_count.clone();
        let next = LoopTestNext {
            call_count: call_count_clone,
            first: "你好".to_string(),
            second: "你好，很高兴见到你".to_string(),
        };
        let req = serde_json::json!({});
        let resp = advisor.around_invoke(req, None, &next).await.unwrap();
        // 应该调用了 2 次
        assert_eq!(call_count.load(std::sync::atomic::Ordering::Relaxed), 2);
        let text = resp.get("text").unwrap().as_str().unwrap();
        assert_eq!(text, "你好，很高兴见到你");
    }

    struct LoopTestNext {
        call_count: Arc<std::sync::atomic::AtomicU32>,
        first: String,
        second: String,
    }

    #[async_trait]
    impl AdvisorNext for LoopTestNext {
        async fn invoke(&self, _req: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
            let n = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let text = if n == 0 { &self.first } else { &self.second };
            Ok(serde_json::json!({ "text": text }))
        }
    }

    #[tokio::test]
    async fn advisor_chain_orders_by_order_ascending() {
        // order: logging(-100) -> re2(0) -> loop(100)
        let logging = Arc::new(LoggingAdvisor::new("log"));
        let re2 = Arc::new(Re2Advisor::new());
        let loop_d = Arc::new(LoopDetectionAdvisor::new(5, 1));
        let order_log = logging.order();
        let order_re2 = re2.order();
        let order_loop = loop_d.order();
        assert!(order_log < order_re2);
        assert!(order_re2 < order_loop);
    }

    #[tokio::test]
    async fn advisor_chain_empty_invokes_inner_directly() {
        let inner: Arc<dyn Runnable> = Arc::new(RunnableLambda::new(|_v| {
            serde_json::json!({ "text": "from_inner" })
        }));
        let chain = AdvisorChain::new(inner);
        let resp = chain.ainvoke(serde_json::json!({}), None).await.unwrap();
        assert_eq!(
            resp.get("text").unwrap().as_str().unwrap(),
            "from_inner"
        );
    }
}
