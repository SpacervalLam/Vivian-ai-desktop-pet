//! Memory 嵌入服务 - 文本向量化
//!
//! 提供两种实现：
//! - [`HashingMemoryEmbedding`]：零依赖确定性哈希嵌入（默认，离线可用）
//! - [`RemoteMemoryEmbedding`]：包装 [`OpenAIEmbedding`]，调用远程 OpenAI 兼容接口
//!
//! 通过 [`build_embedding`] 根据 `MemoryConfig` 选择实现。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tokio::sync::Semaphore;

use crate::config::manager::AppConfig;
use crate::error::{VivianError, VivianResult};
use crate::utils::fnv1a_64;

use super::tokenize::tokenize;

/// 远程嵌入并发上限，避免耗尽 tokio worker 池
const REMOTE_EMBEDDING_MAX_CONCURRENCY: usize = 4;

/// 全局嵌入缓存上限：超过此条数时清空一半最旧条目
///
/// 512 条 × 1024 维 × 4B ≈ 2MB，对桌面端单次会话的重复查询去重已足够；
/// 原值 2000 会额外常驻 8MB，性价比很低。
const EMBEDDING_CACHE_CAP: usize = 512;

/// 单请求 `input` 数组条数上限的**兜底值**
///
/// 各服务商差异极大（OpenAI 2048、智谱 64、百炼 text-embedding-v4 仅 10），
/// 调用方按自己的语料规模设 chunk（情绪语料 168、工具描述 100），无法兼顾所有服务商。
/// 优先用插件预设里按模型声明的 `maxBatch`，无预设时按域名回退，最后才用这个兜底值。
const MAX_BATCH_INPUTS: usize = 64;

/// 缓存键：(文本哈希, 文本字节长度, 模型名, 维度)
///
/// 存哈希而不是完整文本：记忆正文动辄数百到数千字节，原实现把整段文本
/// 当作 HashMap 的 key，光 key 就能吃掉与向量本身同量级的内存。
/// 附记字节长度是为了把 64 位哈希的碰撞概率再压低若干个数量级。
type EmbeddingCacheKey = (u64, usize, String, String, usize);

/// 全局嵌入缓存：避免对同一段文本重复发起远程嵌入调用
#[derive(Default)]
struct EmbeddingCache {
    values: HashMap<EmbeddingCacheKey, Vec<f32>>,
    recency: VecDeque<EmbeddingCacheKey>,
}

static EMBEDDING_CACHE: Lazy<Mutex<EmbeddingCache>> =
    Lazy::new(|| Mutex::new(EmbeddingCache::default()));

fn embedding_cache_key(text: &str, provider: &str, model: &str, dim: usize) -> EmbeddingCacheKey {
    (fnv1a_64(text), text.len(), provider.to_string(), model.to_string(), dim)
}

/// 查询全局嵌入缓存
pub fn embedding_cache_get(text: &str, provider: &str, model: &str, dim: usize) -> Option<Vec<f32>> {
    let key = embedding_cache_key(text, provider, model, dim);
    let mut cache = EMBEDDING_CACHE.lock();
    let value = cache.values.get(&key).cloned();
    if value.is_some() {
        cache.recency.retain(|existing| existing != &key);
        cache.recency.push_back(key);
    }
    value
}

/// 写入全局嵌入缓存，超限时清空一半
pub fn embedding_cache_put(text: &str, provider: &str, model: &str, dim: usize, vec: Vec<f32>) {
    let key = embedding_cache_key(text, provider, model, dim);
    let mut cache = EMBEDDING_CACHE.lock();
    cache.recency.retain(|existing| existing != &key);
    if cache.values.len() >= EMBEDDING_CACHE_CAP && !cache.values.contains_key(&key) {
        if let Some(oldest) = cache.recency.pop_front() {
            cache.values.remove(&oldest);
        }
    }
    cache.recency.push_back(key.clone());
    cache.values.insert(key, vec);
}

/// 当前缓存条目数（用于诊断与测试）
pub fn embedding_cache_size() -> usize {
    EMBEDDING_CACHE.lock().values.len()
}

/// 同步嵌入服务 trait（Memory 路径专用）
pub trait MemoryEmbeddingProvider: Send + Sync {
    /// 向量维度
    fn dimension(&self) -> usize;

    /// 嵌入单个文本
    fn embed(&self, text: &str) -> VivianResult<Vec<f32>>;

    /// 批量嵌入（默认逐个调用 `embed`）
    fn embed_batch(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(t)?);
        }
        Ok(out)
    }

    /// 分块批量嵌入，每 `chunk_size` 条嵌入后回调进度
    ///
    /// 默认实现：按 chunk_size 切分后逐块调用 `embed_batch`。
    /// 远程提供商可覆盖此方法以优化 HTTP 请求粒度。
    fn embed_batch_chunked(
        &self,
        texts: &[String],
        chunk_size: usize,
        on_progress: &(dyn Fn(usize, usize) + Send + Sync),
    ) -> VivianResult<Vec<Vec<f32>>> {
        let total = texts.len();
        let mut all = Vec::with_capacity(total);
        for chunk in texts.chunks(chunk_size) {
            let batch = self.embed_batch(chunk)?;
            all.extend(batch);
            on_progress(all.len(), total);
        }
        Ok(all)
    }

    /// 是否为远程嵌入（用于日志区分）
    fn is_remote(&self) -> bool {
        false
    }

    /// 模型标识（用于向量索引变更检测：模型切换时需重建索引）
    fn model_id(&self) -> &str {
        "hashing"
    }
}

/// 异步嵌入服务 trait（远程调用专用）
#[async_trait]
pub trait EmbeddingService: Send + Sync {
    fn dimension(&self) -> usize;

    async fn embed(&self, text: &str) -> VivianResult<Vec<f32>>;

    async fn embed_batch(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}

/// 哈希嵌入 - 基于特征哈希（hashing trick）的确定性零依赖嵌入
///
/// 对每个 token 使用 FNV-1a 哈希映射到固定维度向量，符号位累加后 L2 归一化。
/// 共享 token 越多的文档余弦相似度越高。仅相同 token 共享，无真实语义理解。
pub struct HashingMemoryEmbedding {
    dim: usize,
}

impl HashingMemoryEmbedding {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(1) }
    }
}

impl Default for HashingMemoryEmbedding {
    fn default() -> Self {
        Self::new(256)
    }
}

impl MemoryEmbeddingProvider for HashingMemoryEmbedding {
    fn dimension(&self) -> usize {
        self.dim
    }

    fn embed(&self, text: &str) -> VivianResult<Vec<f32>> {
        let tokens = tokenize(text);
        let mut vec = vec![0.0f32; self.dim];
        let mut add_feature = |feature: &str, weight: f32| {
            let h = fnv1a_64(feature);
            let idx = (h % self.dim as u64) as usize;
            let sign = if (h >> 63) & 1 == 0 { 1.0f32 } else { -1.0f32 };
            vec[idx] += sign * weight;
        };
        for token in &tokens {
            add_feature(token, 1.0);
        }
        // Character n-grams improve CJK and typo-tolerant lexical recall when a
        // semantic embedding service is intentionally disabled. They do not pretend
        // to provide semantic understanding, but avoid all-or-nothing token overlap.
        let chars: Vec<char> = text
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || is_cjk_char(*c))
            .collect();
        for n in [2usize, 3usize] {
            for window in chars.windows(n) {
                let gram: String = window.iter().collect();
                add_feature(&format!("char{n}:{gram}"), 0.35);
            }
        }
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        }
        Ok(vec)
    }

    fn model_id(&self) -> &str {
        "hashing"
    }

    fn embed_batch_chunked(
        &self,
        texts: &[String],
        _chunk_size: usize,
        on_progress: &(dyn Fn(usize, usize) + Send + Sync),
    ) -> VivianResult<Vec<Vec<f32>>> {
        let all = self.embed_batch(texts)?;
        on_progress(all.len(), texts.len());
        Ok(all)
    }
}

fn is_cjk_char(c: char) -> bool {
    ('\u{3400}'..='\u{4DBF}').contains(&c)
        || ('\u{4E00}'..='\u{9FFF}').contains(&c)
        || ('\u{F900}'..='\u{FAFF}').contains(&c)
}

/// 把一次批量嵌入拆成若干不超过 `cap` 条的下标区间
fn batch_chunk_ranges(len: usize, cap: usize) -> Vec<std::ops::Range<usize>> {
    let cap = cap.max(1);
    (0..len)
        .step_by(cap)
        .map(|start| start..(start + cap).min(len))
        .collect()
}

/// OpenAI 兼容嵌入服务 - 通过 reqwest 调用 `/v1/embeddings`
pub struct OpenAIEmbedding {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    dimension: usize,
    /// 插件预设显式声明的维度参数名（优先于域名/模型启发式）
    dimension_param_override: Option<String>,
    /// 插件预设显式声明的单请求条数上限（0 = 未声明，走启发式）
    max_batch_override: usize,
}

impl OpenAIEmbedding {
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        let base_url = base_url
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
            .trim_end_matches('/')
            .to_string();
        // 本地端点（Ollama 等）禁用系统代理：reqwest 默认读取系统代理设置，
        // Clash 等代理不转发 localhost 会导致嵌入请求连接被拒
        let is_local = base_url.contains("localhost") || base_url.contains("127.0.0.1");
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(60));
        if is_local {
            builder = builder.no_proxy();
        }
        let client = builder.build().unwrap_or_default();
        Self {
            client,
            api_key,
            base_url,
            model: model.unwrap_or_else(|| "text-embedding-3-small".to_string()),
            dimension: 1024,
            dimension_param_override: None,
            max_batch_override: 0,
        }
    }

    pub fn with_dimension(mut self, dim: usize) -> Self {
        self.dimension = dim;
        self
    }

    /// 应用插件预设声明的请求能力（维度参数名、单请求条数上限）
    pub fn with_capabilities(
        mut self,
        dimension_param: Option<String>,
        max_batch: usize,
    ) -> Self {
        self.dimension_param_override = dimension_param;
        self.max_batch_override = max_batch;
        self
    }

    fn cache_provider_id(&self) -> &str {
        &self.base_url
    }

    fn is_ollama(&self) -> bool {
        self.base_url.contains("localhost:11434") || self.base_url.contains("127.0.0.1:11434")
    }

    fn embeddings_url(&self) -> String {
        if self.is_ollama() {
            format!("{}/api/embed", self.base_url.trim_end_matches("/v1"))
        } else if self.base_url.ends_with("/embeddings") {
            self.base_url.clone()
        } else {
            format!("{}/embeddings", self.base_url)
        }
    }

    /// 请求体里下发向量维度的参数名；不支持的服务商/模型返回 `None`（不下发）
    ///
    /// 维度参数是**模型级**能力，不是厂商级能力：同一厂商的老模型往往固定维度，
    /// 硬发 `dimensions` 会被服务端判为非法参数直接 400。所以这里同时看 host 和 model，
    /// 只在明确支持时下发；未知/自建端点保持"不下发"的旧行为。
    /// - 插件预设显式声明的 `dimensionParam` 优先（Cohere 兼容层、Voyage 等以预设为准）
    /// - Ollama `/api/embed` 没有维度参数
    fn dimension_param(&self) -> Option<&str> {
        if let Some(param) = self.dimension_param_override.as_deref() {
            return Some(param);
        }
        if self.is_ollama() {
            return None;
        }
        let host = self.base_url.to_lowercase();
        let model = self.model.to_lowercase();
        // OpenAI：仅 text-embedding-3-* 可调维度，ada-002 固定 1536
        if host.contains("api.openai.com") {
            return model.starts_with("text-embedding-3").then_some("dimensions");
        }
        // 智谱 GLM：embedding-3 支持 256/512/1024/2048，embedding-2 固定 1024
        if host.contains("bigmodel.cn") {
            return model.starts_with("embedding-3").then_some("dimensions");
        }
        // 阿里百炼：text-embedding-v3/v4 可调维度，v1/v2 固定
        if host.contains("dashscope.aliyuncs.com") {
            return (model.contains("v3") || model.contains("v4")).then_some("dimensions");
        }
        None
    }

    /// 单请求 `input` 数组条数上限
    ///
    /// 预设显式声明优先；无预设时按域名保守回退——百炼各模型的批次上限最小
    /// （text-embedding-v4 仅 10 条），估大了会被服务端直接 400 拒绝整批。
    fn max_batch_inputs(&self) -> usize {
        if self.max_batch_override > 0 {
            return self.max_batch_override;
        }
        let host = self.base_url.to_lowercase();
        if host.contains("dashscope.aliyuncs.com") {
            return 10;
        }
        if self.is_ollama() {
            return 2048;
        }
        MAX_BATCH_INPUTS
    }

    /// 组装请求体
    ///
    /// 基准形态是 `{"model": <model>, "input": <string|string[]>}`。额外两处：
    /// - 服务商支持时显式下发维度。**不下发会踩坑**：服务端默认维度常与配置不一致
    ///   （智谱 `embedding-3` 默认 2048，而配置多为 1024），响应回来会在
    ///   [`Self::parse_embedding`] 的维度校验处整批失败。
    /// - Ollama 追加 `keep_alive`，避免模型被卸载后每次都要重新加载。
    fn build_body(&self, inputs: serde_json::Value) -> serde_json::Value {
        let mut body = serde_json::json!({ "model": self.model, "input": inputs });
        if self.is_ollama() {
            body["keep_alive"] = serde_json::Value::String("30m".to_string());
        } else if let Some(param) = self.dimension_param() {
            body[param] = serde_json::json!(self.dimension);
        }
        body
    }

    async fn post_embeddings(&self, inputs: serde_json::Value) -> VivianResult<serde_json::Value> {
        let body = self.build_body(inputs);
        let url = self.embeddings_url();
        for attempt in 1..=3 {
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&body)
                .send()
                .await;
            let resp = match response {
                Ok(resp) => resp,
                Err(error) if attempt < 3 && (error.is_connect() || error.is_timeout()) => {
                    tokio::time::sleep(Duration::from_millis(250 * attempt as u64)).await;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let status = resp.status();
            let response_body = resp.text().await?;
            if status.is_success() {
                return serde_json::from_str(&response_body).map_err(Into::into);
            }
            let retryable = status.as_u16() == 429 || status.is_server_error();
            if retryable && attempt < 3 {
                tracing::warn!(
                    "[MemoryEmbedding] HTTP {}，准备第 {} 次重试",
                    status.as_u16(),
                    attempt + 1
                );
                tokio::time::sleep(Duration::from_millis(250 * attempt as u64)).await;
                continue;
            }
            let detail = serde_json::from_str::<serde_json::Value>(&response_body)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .or_else(|| v.get("error"))
                        .map(|m| m.to_string())
                })
                .unwrap_or_else(|| response_body.chars().take(500).collect());
            return Err(VivianError::Provider(format!(
                "embedding HTTP {}: {}",
                status.as_u16(),
                detail
            )));
        }
        Err(VivianError::Network("embedding 请求重试耗尽".into()))
    }

    fn parse_embedding(&self, value: &serde_json::Value) -> VivianResult<Vec<f32>> {
        let array = value
            .as_array()
            .ok_or_else(|| VivianError::Provider("embedding 向量不是数组".into()))?;
        let vector: Vec<f32> = array
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|n| n as f32)
                    .ok_or_else(|| VivianError::Provider("embedding 向量包含非数值元素".into()))
            })
            .collect::<VivianResult<_>>()?;
        if vector.len() != self.dimension {
            return Err(VivianError::Provider(format!(
                "embedding 维度不匹配: 期望 {}, 实际 {}",
                self.dimension,
                vector.len()
            )));
        }
        Ok(vector)
    }

    /// 单次 HTTP 批量嵌入（入参条数须 ≤ [`MAX_BATCH_INPUTS`]），返回顺序与入参一致
    async fn post_batch_chunk(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
        let resp = self.post_embeddings(serde_json::json!(texts)).await?;
        let data = if self.is_ollama() {
            resp["embeddings"].as_array()
        } else {
            resp["data"].as_array()
        }
        .ok_or_else(|| VivianError::Provider("embedding 响应缺少向量数组".into()))?;
        let mut out: Vec<Option<Vec<f32>>> = (0..texts.len()).map(|_| None).collect();
        for (position, item) in data.iter().enumerate() {
            let response_index = if self.is_ollama() {
                position
            } else {
                item.get("index")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(position)
            };
            let raw = if self.is_ollama() { item } else { &item["embedding"] };
            let emb = self.parse_embedding(raw)?;
            let slot = out.get_mut(response_index).ok_or_else(|| {
                VivianError::Other("embedding 响应条数与请求不匹配".into())
            })?;
            *slot = Some(emb);
        }
        if out.iter().any(|o| o.is_none()) {
            return Err(VivianError::Other("embedding 部分结果缺失".into()));
        }
        Ok(out.into_iter().flatten().collect())
    }
}

#[async_trait]
impl EmbeddingService for OpenAIEmbedding {
    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, text: &str) -> VivianResult<Vec<f32>> {
        if let Some(cached) = embedding_cache_get(
            text,
            self.cache_provider_id(),
            &self.model,
            self.dimension,
        ) {
            return Ok(cached);
        }
        let resp = self
            .post_embeddings(serde_json::Value::String(text.to_string()))
            .await?;
        let raw = if self.is_ollama() {
            &resp["embeddings"][0]
        } else {
            &resp["data"][0]["embedding"]
        };
        let emb = self.parse_embedding(raw)?;
        embedding_cache_put(
            text,
            self.cache_provider_id(),
            &self.model,
            self.dimension,
            emb.clone(),
        );
        Ok(emb)
    }

    async fn embed_batch(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
        let mut results: Vec<Option<Vec<f32>>> = (0..texts.len()).map(|_| None).collect();
        let mut miss_indices: Vec<usize> = Vec::new();
        let mut miss_texts: Vec<String> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            if let Some(cached) = embedding_cache_get(
                t,
                self.cache_provider_id(),
                &self.model,
                self.dimension,
            ) {
                results[i] = Some(cached);
            } else {
                miss_indices.push(i);
                miss_texts.push(t.clone());
            }
        }
        if miss_texts.is_empty() {
            return Ok(results.into_iter().flatten().collect());
        }
        // 适配器级分块：调用方按自己的语料规模设 chunk（情绪语料 168、工具描述 100），
        // 可能远超服务商上限（智谱 64、百炼 10），这里按服务商能力统一兜底。
        let cap = self.max_batch_inputs();
        for range in batch_chunk_ranges(miss_texts.len(), cap) {
            let embeddings = self.post_batch_chunk(&miss_texts[range.clone()]).await?;
            for (offset, emb) in embeddings.into_iter().enumerate() {
                let miss_idx = miss_indices[range.start + offset];
                embedding_cache_put(
                    &miss_texts[range.start + offset],
                    self.cache_provider_id(),
                    &self.model,
                    self.dimension,
                    emb.clone(),
                );
                results[miss_idx] = Some(emb);
            }
        }
        if results.iter().any(|o| o.is_none()) {
            return Err(VivianError::Other("embedding 部分结果缺失".into()));
        }
        Ok(results.into_iter().flatten().collect())
    }
}

/// 兜底 Tokio Runtime：调用线程不在任何 Tokio runtime 上下文内时（如同步 Tauri
/// command 线程、非 tokio 后台线程），使用它执行异步嵌入调用。
static FALLBACK_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> =
    std::sync::OnceLock::new();

/// 在任意线程上下文同步阻塞执行异步 future（嵌入调用专用）
///
/// - 调用线程在 Tokio multi-thread runtime 内：`block_in_place` + 当前 handle `block_on`，
///   不额外创建线程，维持原有并发语义。
/// - 调用线程不在 runtime 内（如同步 `#[tauri::command]` 线程）：改用懒启动的专用
///   Runtime，避免 `Handle::try_current()` 报 "no reactor running" 导致嵌入失败。
pub(crate) fn run_blocking_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => FALLBACK_RUNTIME
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("初始化兜底 tokio runtime 失败")
            })
            .block_on(fut),
    }
}

/// 远程嵌入包装器：在同步接口内调用异步 `OpenAIEmbedding`
///
/// 实现策略：优先用 `tokio::task::block_in_place` 包裹 `Handle::current().block_on`；
/// 调用线程不在 tokio runtime 时回退到懒初始化的专用 Runtime（见 [`run_blocking_on`]），
/// 保证同步上下文（如即时情绪分析的 tauri command 线程）也能正常嵌入。
///
/// ## 并发限制
///
/// 通过 `concurrency` 信号量限制同时进入 `block_in_place` 的调用数，避免耗尽 tokio
/// worker 池。超出限额时 `try_acquire` fail-fast 返回错误，调用方已有降级处理。
pub struct RemoteMemoryEmbedding {
    inner: Arc<OpenAIEmbedding>,
    api_key: String,
    base_url: String,
    model: String,
    /// 插件预设声明的维度参数名（透传给内层适配器）
    dimension_param: Option<String>,
    /// 插件预设声明的单请求条数上限（0 = 未声明）
    max_batch: usize,
    /// 向量维度（重建内层适配器时需要）
    dimension: usize,
    /// 并发上限信号量：限制 `block_in_place` 同时阻塞的 worker 线程数
    concurrency: Arc<Semaphore>,
}

impl RemoteMemoryEmbedding {
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        let base_url = base_url
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let model = model
            .unwrap_or_else(|| "text-embedding-3-small".to_string());
        let emb = OpenAIEmbedding::new(
            api_key.clone(),
            Some(base_url.clone()),
            Some(model.clone()),
        );
        Self {
            inner: Arc::new(emb),
            api_key,
            base_url,
            model,
            dimension_param: None,
            max_batch: 0,
            dimension: 1024,
            concurrency: Arc::new(Semaphore::new(REMOTE_EMBEDDING_MAX_CONCURRENCY)),
        }
    }

    /// 应用插件预设声明的请求能力（维度参数名、单请求条数上限）
    pub fn with_capabilities(
        mut self,
        dimension_param: Option<String>,
        max_batch: usize,
    ) -> Self {
        self.dimension_param = dimension_param;
        self.max_batch = max_batch;
        self.inner = Arc::new(self.rebuild_inner());
        self
    }

    pub fn with_dimension(mut self, dim: usize) -> Self {
        self.dimension = dim;
        self.inner = Arc::new(self.rebuild_inner());
        self
    }

    /// 按当前字段重建内层适配器（维度/能力变更后调用）
    fn rebuild_inner(&self) -> OpenAIEmbedding {
        OpenAIEmbedding::new(
            self.api_key.clone(),
            Some(self.base_url.clone()),
            Some(self.model.clone()),
        )
        .with_dimension(self.dimension)
        .with_capabilities(self.dimension_param.clone(), self.max_batch)
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub fn dim(&self) -> usize {
        self.inner.dimension()
    }

    fn block_embed(&self, text: &str) -> VivianResult<Vec<f32>> {
        run_blocking_on(async {
            let _permit = tokio::time::timeout(Duration::from_secs(5), self.concurrency.acquire())
                .await
                .map_err(|_| VivianError::Timeout("等待嵌入并发许可超时".into()))?
                .map_err(|_| VivianError::Other("嵌入服务已关闭".into()))?;
            self.inner.embed(text).await
        })
    }

    fn block_embed_batch(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
        run_blocking_on(async {
            let _permit = tokio::time::timeout(Duration::from_secs(5), self.concurrency.acquire())
                .await
                .map_err(|_| VivianError::Timeout("等待嵌入并发许可超时".into()))?
                .map_err(|_| VivianError::Other("嵌入服务已关闭".into()))?;
            self.inner.embed_batch(texts).await
        })
    }
}

impl MemoryEmbeddingProvider for RemoteMemoryEmbedding {
    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn embed(&self, text: &str) -> VivianResult<Vec<f32>> {
        self.block_embed(text)
    }

    fn embed_batch(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
        self.block_embed_batch(texts)
    }

    fn embed_batch_chunked(
        &self,
        texts: &[String],
        chunk_size: usize,
        on_progress: &(dyn Fn(usize, usize) + Send + Sync),
    ) -> VivianResult<Vec<Vec<f32>>> {
        let total = texts.len();
        let mut all = Vec::with_capacity(total);
        for chunk in texts.chunks(chunk_size) {
            let batch = self.block_embed_batch(chunk)?;
            all.extend(batch);
            on_progress(all.len(), total);
        }
        Ok(all)
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

/// 默认嵌入服务（哈希嵌入，256 维）
pub fn default_embedding() -> Arc<dyn MemoryEmbeddingProvider> {
    Arc::new(HashingMemoryEmbedding::default())
}

/// 由维度构造默认哈希嵌入服务
pub fn default_embedding_with_dim(dim: usize) -> Arc<dyn MemoryEmbeddingProvider> {
    Arc::new(HashingMemoryEmbedding::new(dim))
}

/// 同步探测本地 Ollama（127.0.0.1:11434）是否在运行且装有嵌入模型
///
/// 纯 socket 实现（不走 reqwest blocking，可在任意同步上下文调用）：
/// 发送 HTTP GET /v1/models 并解析模型列表，优先返回 bge-m3，
/// 其次任意 bge*/embed*/nomic-embed* 模型。探测失败返回 None（调用方回退哈希嵌入）。
fn probe_ollama_embedding_model() -> Option<String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let addr: std::net::SocketAddr = "127.0.0.1:11434".parse().ok()?;
    let mut stream =
        TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(400)).ok()?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(1500)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(1500)));
    let request = "GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    let body = String::from_utf8_lossy(&buf);
    // 跳过 HTTP 响应头，定位 JSON 体
    let json_start = body.find('{')?;
    let value: serde_json::Value = serde_json::from_str(&body[json_start..]).ok()?;
    let models: Vec<String> = value
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    if models.is_empty() {
        return None;
    }
    if models.iter().any(|m| m == "bge-m3" || m.starts_with("bge-m3:")) {
        return Some("bge-m3".to_string());
    }
    models.into_iter().find(|m| {
        let l = m.to_lowercase();
        l.contains("bge") || l.contains("embed") || l.contains("nomic-embed")
    })
}

/// 从插件预设里取出某模型的请求能力（维度参数名、单请求条数上限）
///
/// 预设是能力的**主数据源**：端点、模型可调维度、数组条数上限都随厂商迭代变化，
/// 放在可热更新的插件数据里比硬编码在代码里更不容易过期。预设缺失时返回
/// `(None, 0)`，由适配器的域名启发式兜底。
fn preset_capabilities(model: &str) -> (Option<String>, usize) {
    let Some(preset) = crate::plugins::find_embedding_preset_by_model(model) else {
        return (None, 0);
    };
    let max_batch = preset
        .models
        .iter()
        .find(|m| m.model.trim() == model.trim())
        .and_then(|m| m.max_batch)
        .unwrap_or(0);
    (preset.dimension_param, max_batch)
}

/// 嵌入服务工厂：根据 `MemoryConfig.embedding` 选择远程或哈希
///
/// 选择规则（统一优先使用用户配置的嵌入模型，未配置才回退到哈希）：
/// - `source == "local"`：使用本地 Ollama（http://localhost:11434/v1）
/// - `api_key` 与 `endpoint` 非空：使用远程嵌入
/// - 否则回退到 256 维哈希嵌入
pub fn build_embedding(config: &AppConfig) -> Arc<dyn MemoryEmbeddingProvider> {
    let emb = &config.memory.embedding;

    // Respect the explicit privacy/offline switch. Disabled means no remote or
    // local-service probing; hashing remains entirely in-process.
    if !emb.enabled {
        tracing::info!("[MemoryEmbedding] 嵌入服务已禁用，使用本地哈希嵌入");
        return Arc::new(HashingMemoryEmbedding::default());
    }

    // 本地 Ollama 模式
    if emb.source == "local" {
        let model = if emb.ollama_model.trim().is_empty() {
            "bge-m3".to_string()
        } else {
            emb.ollama_model.clone()
        };
        // 用注册表自动校正维度，避免维度填错导致向量索引反复重建
        let dim = super::embedding_registry::normalize_dimension(&model, emb.dimension);
        let provider = RemoteMemoryEmbedding::new(
            "ollama".to_string(),
            Some("http://localhost:11434/v1".to_string()),
            Some(model),
        )
        .with_dimension(dim);
        tracing::info!(
            "[MemoryEmbedding] 启用本地 Ollama 嵌入: model={}, dim={}",
            provider.model_name(),
            provider.dim()
        );
        return Arc::new(provider);
    }

    // 云端模式
    let api_key = emb.api_key.trim();
    let endpoint = emb.endpoint.trim();
    if !api_key.is_empty() && !endpoint.is_empty() {
        let dim = super::embedding_registry::normalize_dimension(&emb.model, emb.dimension);
        let (dimension_param, max_batch) = preset_capabilities(&emb.model);
        let provider = RemoteMemoryEmbedding::new(
            api_key.to_string(),
            Some(endpoint.to_string()),
            Some(emb.model.clone()),
        )
        .with_dimension(dim)
        .with_capabilities(dimension_param.clone(), max_batch);
        tracing::info!(
            "[MemoryEmbedding] 启用远程嵌入: model={}, dim={}, dimensionParam={}, maxBatch={}",
            provider.model_name(),
            provider.dim(),
            dimension_param.as_deref().unwrap_or("none"),
            if max_batch == 0 {
                "auto".to_string()
            } else {
                max_batch.to_string()
            }
        );
        return Arc::new(provider);
    }

    // 未配置任何嵌入模型：探测运行中的本地 Ollama，可用则自动升级为真实语义嵌入
    // （探测为纯 socket 快速检查，不启动任何服务；Ollama 未运行则回退哈希嵌入）
    if let Some(model) = probe_ollama_embedding_model() {
        let dim = super::embedding_registry::normalize_dimension(&model, 0);
        // 维度未知的模型跳过自动升级（向量索引无法建维度为 0 的表）
        if dim > 0 {
            let provider = RemoteMemoryEmbedding::new(
                "ollama".to_string(),
                Some("http://localhost:11434/v1".to_string()),
                Some(model.clone()),
            )
            .with_dimension(dim);
            tracing::info!(
                "[MemoryEmbedding] 检测到本地 Ollama 已运行，自动升级嵌入: model={}, dim={}",
                provider.model_name(),
                provider.dim()
            );
            return Arc::new(provider);
        }
        tracing::warn!(
            "[MemoryEmbedding] Ollama 嵌入模型 {} 维度未知，跳过自动升级",
            model
        );
    }
    tracing::warn!(
        "[MemoryEmbedding] 未配置嵌入模型（source 非 local 且无 api_key/endpoint），回退到哈希嵌入"
    );
    default_embedding()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hashing_embedding_deterministic() {
        let emb = HashingMemoryEmbedding::default();
        let v1 = emb.embed("你好世界").unwrap();
        let v2 = emb.embed("你好世界").unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_hashing_embedding_dimension() {
        let emb = HashingMemoryEmbedding::default();
        assert_eq!(emb.dimension(), 256);
        let v = emb.embed("hello world").unwrap();
        assert_eq!(v.len(), 256);
    }

    #[test]
    fn test_hashing_embedding_normalized() {
        let emb = HashingMemoryEmbedding::default();
        let v = emb.embed("hello world test").unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "向量未归一化: norm = {norm}");
    }

    #[test]
    fn test_hashing_embedding_similarity() {
        let emb = HashingMemoryEmbedding::new(256);
        let v1 = emb.embed("我喜欢吃苹果").unwrap();
        let v2 = emb.embed("我喜欢吃苹果").unwrap();
        let v3 = emb.embed("完全不同的内容xyz").unwrap();
        let sim_same = cosine_sim(&v1, &v2);
        let sim_diff = cosine_sim(&v1, &v3);
        assert!(sim_same > 0.99, "相同文本相似度应接近1: {sim_same}");
        assert!(sim_diff < sim_same, "不同文本相似度应低于相同文本");
    }

    #[test]
    fn test_hashing_embedding_cjk_partial_overlap() {
        let emb = HashingMemoryEmbedding::new(512);
        let base = emb.embed("用户现在住在上海浦东").unwrap();
        let related = emb.embed("上海浦东天气怎么样").unwrap();
        let unrelated = emb.embed("量子计算机芯片架构").unwrap();
        assert!(cosine_sim(&base, &related) > cosine_sim(&base, &unrelated));
    }

    #[test]
    fn test_hashing_embedding_batch() {
        let emb = HashingMemoryEmbedding::new(64);
        let texts = vec!["你好".to_string(), "世界".to_string()];
        let results = emb.embed_batch(&texts).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 64);
    }

    #[test]
    fn embedding_url_normalizes_cloud_and_ollama_endpoints() {
        let cloud = OpenAIEmbedding::new(
            "key".into(),
            Some("https://api.example.com/v1/".into()),
            Some("model".into()),
        );
        assert_eq!(cloud.embeddings_url(), "https://api.example.com/v1/embeddings");

        let full = OpenAIEmbedding::new(
            "key".into(),
            Some("https://api.example.com/v1/embeddings".into()),
            Some("model".into()),
        );
        assert_eq!(full.embeddings_url(), "https://api.example.com/v1/embeddings");

        let ollama = OpenAIEmbedding::new(
            "ollama".into(),
            Some("http://127.0.0.1:11434/v1".into()),
            Some("bge-m3".into()),
        );
        assert_eq!(ollama.embeddings_url(), "http://127.0.0.1:11434/api/embed");
    }

    #[test]
    fn embedding_body_sends_dimension_only_for_supporting_hosts() {
        let zhipu = OpenAIEmbedding::new(
            "key".into(),
            Some("https://open.bigmodel.cn/api/paas/v4/embeddings".into()),
            Some("embedding-3".into()),
        )
        .with_dimension(1024);
        let body = zhipu.build_body(serde_json::json!("你好"));
        assert_eq!(body["model"].as_str(), Some("embedding-3"));
        assert_eq!(body["input"].as_str(), Some("你好"));
        assert_eq!(
            body["dimensions"].as_u64(),
            Some(1024),
            "智谱支持 dimensions，不下发就会拿到默认的 2048 维"
        );

        let openai = OpenAIEmbedding::new("key".into(), None, Some("text-embedding-3-small".into()))
            .with_dimension(1536);
        assert_eq!(openai.build_body(serde_json::json!("hi"))["dimensions"].as_u64(), Some(1536));

        // 未知/自建端点：不下发，避免把不认识的字段塞给别人的接口
        let custom = OpenAIEmbedding::new(
            "key".into(),
            Some("https://api.example.com/v1".into()),
            Some("my-model".into()),
        )
        .with_dimension(1024);
        assert!(custom.build_body(serde_json::json!("hi")).get("dimensions").is_none());

        // 维度参数是模型级能力：同厂商的老模型固定维度，发了会被判非法参数
        let ada = OpenAIEmbedding::new(
            "key".into(),
            Some("https://api.openai.com/v1".into()),
            Some("text-embedding-ada-002".into()),
        )
        .with_dimension(1536);
        assert!(ada.build_body(serde_json::json!("hi")).get("dimensions").is_none());

        let glm2 = OpenAIEmbedding::new(
            "key".into(),
            Some("https://open.bigmodel.cn/api/paas/v4".into()),
            Some("embedding-2".into()),
        )
        .with_dimension(1024);
        assert!(glm2.build_body(serde_json::json!("hi")).get("dimensions").is_none());

        // Voyage 的字段名是 output_dimension，发 dimensions 是错的
        let voyage = OpenAIEmbedding::new(
            "key".into(),
            Some("https://api.voyageai.com/v1".into()),
            Some("voyage-4".into()),
        )
        .with_dimension(1024);
        assert!(voyage.build_body(serde_json::json!("hi")).get("dimensions").is_none());
    }

    #[test]
    fn embedding_body_keeps_ollama_keep_alive_without_dimension() {
        let ollama = OpenAIEmbedding::new(
            "ollama".into(),
            Some("http://127.0.0.1:11434/v1".into()),
            Some("bge-m3".into()),
        )
        .with_dimension(1024);
        let body = ollama.build_body(serde_json::json!(["a", "b"]));
        assert_eq!(body["keep_alive"].as_str(), Some("30m"));
        assert!(
            body.get("dimensions").is_none(),
            "Ollama /api/embed 没有维度参数"
        );
    }

    #[test]
    fn batch_chunk_ranges_caps_every_request_at_64() {
        assert!(batch_chunk_ranges(0, MAX_BATCH_INPUTS).is_empty());
        assert_eq!(batch_chunk_ranges(1, MAX_BATCH_INPUTS), vec![0..1]);
        assert_eq!(
            batch_chunk_ranges(MAX_BATCH_INPUTS, MAX_BATCH_INPUTS),
            vec![0..MAX_BATCH_INPUTS]
        );
        // 夹具必须真的越线，否则这条用例什么也没测
        assert!(168 > MAX_BATCH_INPUTS, "夹具本身必须超过上限");
        // 情绪语料 168 条 → 3 次请求；工具描述 100 条 → 2 次请求
        assert_eq!(
            batch_chunk_ranges(168, MAX_BATCH_INPUTS),
            vec![0..64, 64..128, 128..168]
        );
        assert_eq!(batch_chunk_ranges(100, MAX_BATCH_INPUTS), vec![0..64, 64..100]);
    }

    /// 百炼 text-embedding-v4 的批次上限只有 10 条：按 64 条切会整批 400
    #[test]
    fn batch_chunk_ranges_honours_tight_provider_cap() {
        let ranges = batch_chunk_ranges(25, 10);
        assert_eq!(ranges, vec![0..10, 10..20, 20..25]);
        assert!(ranges.iter().all(|r| r.len() <= 10));
        // 上限为 0 时不能 panic、也不能退化成"整批发一条"
        assert_eq!(batch_chunk_ranges(3, 0), vec![0..1, 1..2, 2..3]);
    }

    /// 预设声明的能力优先于域名启发式，未声明时才走启发式
    #[test]
    fn provider_capabilities_override_heuristics() {
        let zhipu = OpenAIEmbedding::new(
            "key".into(),
            Some("https://open.bigmodel.cn/api/paas/v4".into()),
            Some("embedding-3".into()),
        );
        assert_eq!(zhipu.dimension_param(), Some("dimensions"));
        assert_eq!(zhipu.max_batch_inputs(), MAX_BATCH_INPUTS);

        // 预设显式声明：Voyage 的字段名是 output_dimension，且批次上限 128
        let voyage = OpenAIEmbedding::new(
            "key".into(),
            Some("https://api.voyageai.com/v1".into()),
            Some("voyage-4".into()),
        )
        .with_capabilities(Some("output_dimension".into()), 128);
        assert_eq!(voyage.dimension_param(), Some("output_dimension"));
        assert_eq!(voyage.max_batch_inputs(), 128);
        assert_eq!(
            voyage.build_body(serde_json::json!("hi"))["output_dimension"].as_u64(),
            Some(1024)
        );

        // 无预设时按域名保守回退：百炼批次上限最小
        let dashscope = OpenAIEmbedding::new(
            "key".into(),
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1".into()),
            Some("text-embedding-v4".into()),
        );
        assert_eq!(dashscope.max_batch_inputs(), 10);
    }

    #[test]
    fn embedding_parser_rejects_wrong_dimension_and_non_numbers() {
        let embedding = OpenAIEmbedding::new("key".into(), None, Some("model".into()))
            .with_dimension(2);
        assert_eq!(embedding.parse_embedding(&serde_json::json!([1.0, 2.0])).unwrap(), vec![1.0, 2.0]);
        assert!(embedding.parse_embedding(&serde_json::json!([1.0])).is_err());
        assert!(embedding.parse_embedding(&serde_json::json!([1.0, "bad"])).is_err());
    }

    #[test]
    fn embedding_cache_is_isolated_by_provider() {
        let text = "provider-isolation-test";
        embedding_cache_put(text, "provider-a", "same-model", 2, vec![1.0, 0.0]);
        assert_eq!(
            embedding_cache_get(text, "provider-a", "same-model", 2),
            Some(vec![1.0, 0.0])
        );
        assert_eq!(embedding_cache_get(text, "provider-b", "same-model", 2), None);
    }

    fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
    }
}
