use std::collections::HashMap;

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::VivianResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnableConfig {
    pub run_id: String,
    pub max_retries: u32,
    pub tags: Vec<String>,
    pub metadata: Value,
    /// 并行执行时单个序列内同时运行的最大任务数（None 表示无限）
    #[serde(default)]
    pub max_concurrency: Option<u32>,
    /// 递归调用上限（默认 25，防无限递归）
    #[serde(default = "default_recursion_limit")]
    pub recursion_limit: u32,
}

fn default_recursion_limit() -> u32 {
    25
}

impl Default for RunnableConfig {
    fn default() -> Self {
        Self {
            run_id: Uuid::new_v4().to_string(),
            max_retries: 2,
            tags: Vec::new(),
            metadata: Value::Object(serde_json::Map::new()),
            max_concurrency: None,
            recursion_limit: default_recursion_limit(),
        }
    }
}

impl RunnableConfig {
    /// 判断是否启用了流式（与现有步骤约定一致：tags 含 "stream"）
    pub fn is_streaming(&self) -> bool {
        self.tags.iter().any(|t| t == "stream")
    }

    /// 读取 metadata 中的 task_type，默认 "chat"
    pub fn task_type(&self) -> String {
        self.metadata
            .get("task_type")
            .and_then(|v| v.as_str())
            .unwrap_or("chat")
            .to_string()
    }

    /// 检查递归是否超限（每层调用应先检查并递减）
    pub fn check_recursion(&self) -> VivianResult<()> {
        if self.recursion_limit == 0 {
            Err(crate::error::VivianError::Engine(
                "Runnable 递归超限（recursion_limit=0）".into(),
            ))
        } else {
            Ok(())
        }
    }

    /// 返回一个递归层级减 1 的子 config（用于子 Runnable 调用前）
    pub fn child(&self) -> Self {
        self.clone().with_recursion_limit(self.recursion_limit.saturating_sub(1))
    }

    /// 链式设置 max_concurrency
    pub fn with_max_concurrency(mut self, n: u32) -> Self {
        self.max_concurrency = Some(n);
        self
    }

    /// 链式设置 recursion_limit
    pub fn with_recursion_limit(mut self, n: u32) -> Self {
        self.recursion_limit = n;
        self
    }
}

#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub event: String,
    pub data: String,
    pub metadata: Value,
}

impl StreamEvent {
    pub fn new(event: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            event: event.into(),
            data: data.into(),
            metadata: Value::Null,
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Runnable trait —— 异步可执行的流水线单元。
///
/// - `ainvoke`：异步执行，输入/输出均为 `serde_json::Value`
/// - `atransform`：流式 transform，将产出增量推送到 `Sender<Value>`（默认实现：执行 `ainvoke` 后推送单个结果）
/// - `astream_events`：流式事件（默认实现：执行 `ainvoke` 后发出单个 `text_done` 事件）
#[async_trait]
pub trait Runnable: Send + Sync {
    async fn ainvoke(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Value>;

    /// 流式 transform：执行 Runnable 并将产出增量推送到 `output` sender。
    ///
    /// 默认实现：执行 `ainvoke` 后推送单个完整结果，等价于非流式调用。
    /// 支持真正流式产出的实现（如 LLM 生成、JSON 流式解析）应覆盖此方法，
    /// 在产出过程中多次调用 `output.send(chunk).await`。
    ///
    /// 调用方负责创建 channel 并消费 receiver，例如：
    /// ```ignore
    /// let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    /// let handle = tokio::spawn(async move { runnable.atransform(input, tx, config).await });
    /// while let Some(chunk) = rx.recv().await { /* 处理 chunk */ }
    /// handle.await??;
    /// ```
    async fn atransform(
        &self,
        input: Value,
        output: tokio::sync::mpsc::Sender<Value>,
        config: Option<RunnableConfig>,
    ) -> VivianResult<()> {
        let result = self.ainvoke(input, config).await?;
        let _ = output.send(result).await;
        Ok(())
    }

    /// 流式事件收集（默认实现：执行一次 ainvoke 并发出完成事件）。
    ///
    /// 返回 `Vec<StreamEvent>` 而非异步迭代器，规避 async trait 的流类型限制；
    /// 真正的增量流式由具体实现覆盖（如 [`crate::pipeline::parsers::StreamingOutputParser`]）。
    async fn astream_events(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Vec<StreamEvent>> {
        let run_id = config
            .as_ref()
            .map(|c| c.run_id.clone())
            .unwrap_or_default();
        let result = self.ainvoke(input, config).await?;
        let metadata = if run_id.is_empty() {
            Value::Null
        } else {
            serde_json::json!({ "run_id": run_id })
        };
        Ok(vec![StreamEvent::new("text_done", result.to_string()).with_metadata(metadata)])
    }
}

pub struct RunnableLambda {
    pub func: Box<dyn Fn(Value) -> Value + Send + Sync>,
    pub afunc: Option<Box<dyn Fn(Value) -> BoxFuture<'static, Value> + Send + Sync>>,
}

impl RunnableLambda {
    pub fn new<F>(func: F) -> Self
    where
        F: Fn(Value) -> Value + Send + Sync + 'static,
    {
        Self {
            func: Box::new(func),
            afunc: None,
        }
    }

    pub fn with_async<F, Fut>(mut self, afunc: F) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Value> + Send + 'static,
    {
        self.afunc = Some(Box::new(
            move |input: Value| -> BoxFuture<'static, Value> { Box::pin(afunc(input)) },
        ));
        self
    }
}

#[async_trait]
impl Runnable for RunnableLambda {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        if let Some(afunc) = &self.afunc {
            Ok(afunc(input).await)
        } else {
            Ok((self.func)(input))
        }
    }
}

pub struct RunnableSequence {
    steps: Vec<Box<dyn Runnable>>,
}

impl RunnableSequence {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn add_step(&mut self, step: Box<dyn Runnable>) -> &mut Self {
        self.steps.push(step);
        self
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

impl Default for RunnableSequence {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for RunnableSequence {
    async fn ainvoke(&self, input: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut current = input;
        for step in &self.steps {
            current = step.ainvoke(current, config.clone()).await?;
        }
        Ok(current)
    }

    /// 序列 transform：前 N-1 步走 `ainvoke`（非流式），最后一步走 `atransform`
    /// 将增量产出推送到 `output`。
    ///
    /// 这是"末端流式"模式：大多数步骤（预处理、记忆检索、prompt 组装）不产生流式输出，
    /// 只有末端步骤（LLM 生成 + 解析）需要流式推送。
    /// 需要中间步骤也流式的场景应直接组合具体 Runnable 的 `atransform`。
    async fn atransform(
        &self,
        input: Value,
        output: tokio::sync::mpsc::Sender<Value>,
        config: Option<RunnableConfig>,
    ) -> VivianResult<()> {
        if self.steps.is_empty() {
            let _ = output.send(input).await;
            return Ok(());
        }
        let last_idx = self.steps.len() - 1;
        // 前 N-1 步走 ainvoke（非流式）
        let mut current = input;
        for step in &self.steps[..last_idx] {
            current = step.ainvoke(current, config.clone()).await?;
        }
        // 最后一步走 atransform（流式推送）
        self.steps[last_idx]
            .atransform(current, output, config)
            .await?;
        Ok(())
    }

    /// 序列流事件 —— 逐 step 发出 `status_change` 事件，最终发出 `text_done`。
    async fn astream_events(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Vec<StreamEvent>> {
        let run_id = config
            .as_ref()
            .map(|c| c.run_id.clone())
            .unwrap_or_default();
        let mk_meta = || {
            if run_id.is_empty() {
                Value::Null
            } else {
                serde_json::json!({ "run_id": run_id })
            }
        };

        let mut events: Vec<StreamEvent> = Vec::new();
        let mut current = input;
        for (i, step) in self.steps.iter().enumerate() {
            let step_name = format!("step_{}", i);
            events.push(
                StreamEvent::new("status_change", format!("{}_start", step_name))
                    .with_metadata(mk_meta()),
            );
            current = step.ainvoke(current, config.clone()).await?;
            events.push(
                StreamEvent::new("status_change", format!("{}_complete", step_name))
                    .with_metadata(mk_meta()),
            );
        }
        events.push(StreamEvent::new("text_done", current.to_string()).with_metadata(mk_meta()));
        Ok(events)
    }
}

/// 并行执行的 Runnable。
///
/// 接收一个 `HashMap<String, Box<dyn Runnable>>`，`ainvoke` 时：
/// - 输入需为 JSON 对象，键与 steps 的键对应；
/// - 每个 step 接收对应键的值，并行执行；
/// - 输出为 JSON 对象，键与 steps 的键对应。
///
/// 任一 step 失败时返回首个错误（与 `tokio::try_join` 语义一致）。
pub struct RunnableParallel {
    steps: HashMap<String, Box<dyn Runnable>>,
    /// 保持键的插入顺序（HashMap 本身无序）
    order: Vec<String>,
}

impl RunnableParallel {
    pub fn new() -> Self {
        Self {
            steps: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn add(&mut self, key: impl Into<String>, step: Box<dyn Runnable>) -> &mut Self {
        let key = key.into();
        if !self.steps.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.steps.insert(key, step);
        self
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn keys(&self) -> &[String] {
        &self.order
    }
}

impl Default for RunnableParallel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for RunnableParallel {
    async fn ainvoke(&self, input: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        // 输入需为 JSON 对象，键与 steps 的键对应
        let input_map = input.as_object().ok_or_else(|| {
            crate::error::VivianError::Engine(format!(
                "RunnableParallel 期望 JSON 对象输入，实际为: {}",
                input
            ))
        })?;

        // 为每个 step 取出对应的输入，构造 (key, future) 对。
        let futs: Vec<_> = self
            .order
            .iter()
            .filter_map(|key| {
                let step = self.steps.get(key)?;
                let step_input = input_map.get(key).cloned().unwrap_or(Value::Null);
                let step_cfg = config.clone();
                Some(async move {
                    step.ainvoke(step_input, step_cfg)
                        .await
                        .map(|v| (key.clone(), v))
                })
            })
            .collect();

        // 并发执行：若配置了 max_concurrency 则用 buffer_unordered 限流
        let pairs = if let Some(limit) = config.as_ref().and_then(|c| c.max_concurrency) {
            let limit = limit.max(1) as usize;
            use futures::stream::{StreamExt, TryStreamExt};
            futures::stream::iter(futs)
                .buffer_unordered(limit)
                .try_collect::<Vec<_>>()
                .await?
        } else {
            futures::future::try_join_all(futs).await?
        };

        let mut results: serde_json::Map<String, Value> = serde_json::Map::new();
        for (k, v) in pairs {
            results.insert(k, v);
        }
        Ok(Value::Object(results))
    }
}

pub struct TimingMiddleware {
    name: String,
    inner: Box<dyn Runnable>,
}

impl TimingMiddleware {
    pub fn new(name: impl Into<String>, inner: Box<dyn Runnable>) -> Self {
        Self {
            name: name.into(),
            inner,
        }
    }

    /// 获取被包裹的内部 Runnable 名称（用于流事件）
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl Runnable for TimingMiddleware {
    async fn ainvoke(&self, input: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        let start = std::time::Instant::now();
        let result = self.inner.ainvoke(input, config).await;
        let elapsed = start.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;
        match &result {
            Ok(_) => tracing::info!(
                stage = %self.name,
                elapsed_ms = elapsed_ms,
                "stage completed"
            ),
            Err(e) => tracing::warn!(
                stage = %self.name,
                elapsed_ms = elapsed_ms,
                error = %e,
                "stage failed"
            ),
        }
        // 将 timing 写入 state.metadata["timings"]，供 ReasoningTrace 组装使用。
        // 仅在成功路径注入（失败路径直接返回 Err）。
        match result {
            Ok(mut value) => {
                if let Some(obj) = value.as_object_mut() {
                    let metadata = obj
                        .entry("metadata".to_string())
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(meta_obj) = metadata.as_object_mut() {
                        let timings = meta_obj
                            .entry("timings".to_string())
                            .or_insert_with(|| serde_json::json!([]));
                        if let Some(arr) = timings.as_array_mut() {
                            arr.push(serde_json::json!({
                                "stage": self.name,
                                "elapsed_ms": elapsed_ms,
                                "success": true,
                            }));
                        }
                    }
                }
                Ok(value)
            }
            Err(e) => Err(e),
        }
    }

    async fn astream_events(
        &self,
        input: Value,
        config: Option<RunnableConfig>,
    ) -> VivianResult<Vec<StreamEvent>> {
        let run_id = config
            .as_ref()
            .map(|c| c.run_id.clone())
            .unwrap_or_default();
        let mk_meta = || {
            if run_id.is_empty() {
                Value::Null
            } else {
                serde_json::json!({ "run_id": run_id })
            }
        };

        let mut events: Vec<StreamEvent> = Vec::new();
        events.push(
            StreamEvent::new("status_change", format!("{}_start", self.name))
                .with_metadata(mk_meta()),
        );

        let start = std::time::Instant::now();
        let result = self.inner.astream_events(input, config).await;
        let elapsed = start.elapsed();

        match &result {
            Ok(inner_events) => {
                events.extend(inner_events.clone());
                tracing::info!(
                    stage = %self.name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    "stage completed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    stage = %self.name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "stage failed"
                );
            }
        }

        events.push(
            StreamEvent::new("status_change", format!("{}_complete", self.name))
                .with_metadata(mk_meta()),
        );
        result.map(|inner| {
            events.extend(inner);
            events
        })
    }
}
