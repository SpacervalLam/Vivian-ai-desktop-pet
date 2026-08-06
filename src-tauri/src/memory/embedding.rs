//! Memory 嵌入服务 - 文本向量化
//!
//! 提供两种实现：
//! - [`HashingMemoryEmbedding`]：零依赖确定性哈希嵌入（默认，离线可用）
//! - [`RemoteMemoryEmbedding`]：包装 [`OpenAIEmbedding`]，调用远程 OpenAI 兼容接口
//!
//! 通过 [`build_embedding`] 根据 `MemoryConfig` 选择实现。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use tokio::sync::Semaphore;

use crate::config::manager::AppConfig;
use crate::error::{VivianError, VivianResult};
use crate::utils::fnv1a_64;

use super::tokenize::tokenize;

/// 远程嵌入并发上限，避免耗尽 tokio worker 池
const REMOTE_EMBEDDING_MAX_CONCURRENCY: usize = 4;

/// 全局嵌入缓存上限：超过此条数时清空一半最旧条目
const EMBEDDING_CACHE_CAP: usize = 2000;

/// 全局嵌入缓存：按 (text, model, dimension) 缓存向量，避免重复远程调用
static EMBEDDING_CACHE: Lazy<RwLock<std::collections::HashMap<(String, String, usize), Vec<f32>>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 查询全局嵌入缓存
pub fn embedding_cache_get(text: &str, model: &str, dim: usize) -> Option<Vec<f32>> {
    EMBEDDING_CACHE
        .read()
        .get(&(text.to_string(), model.to_string(), dim))
        .cloned()
}

/// 写入全局嵌入缓存，超限时清空一半
pub fn embedding_cache_put(text: &str, model: &str, dim: usize, vec: Vec<f32>) {
    let mut cache = EMBEDDING_CACHE.write();
    if cache.len() >= EMBEDDING_CACHE_CAP {
        let drop_count = cache.len() / 2;
        let keys: Vec<_> = cache.keys().take(drop_count).cloned().collect();
        for k in keys {
            cache.remove(&k);
        }
    }
    cache.insert((text.to_string(), model.to_string(), dim), vec);
}

/// 当前缓存条目数（用于诊断与测试）
pub fn embedding_cache_size() -> usize {
    EMBEDDING_CACHE.read().len()
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
        for token in &tokens {
            let h = fnv1a_64(token);
            let idx = (h % self.dim as u64) as usize;
            let sign = if (h >> 63) & 1 == 0 { 1.0f32 } else { -1.0f32 };
            vec[idx] += sign;
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

/// OpenAI 兼容嵌入服务 - 通过 reqwest 调用 `/v1/embeddings`
pub struct OpenAIEmbedding {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    dimension: usize,
}

impl OpenAIEmbedding {
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: base_url
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            model: model.unwrap_or_else(|| "BAAI/bge-m3".to_string()),
            dimension: 1024,
        }
    }

    pub fn with_dimension(mut self, dim: usize) -> Self {
        self.dimension = dim;
        self
    }
}

#[async_trait]
impl EmbeddingService for OpenAIEmbedding {
    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, text: &str) -> VivianResult<Vec<f32>> {
        if let Some(cached) = embedding_cache_get(text, &self.model, self.dimension) {
            return Ok(cached);
        }
        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
        });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let emb: Vec<f32> = resp["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| VivianError::Other("OpenAI embedding 响应格式错误".into()))?
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();
        embedding_cache_put(text, &self.model, self.dimension, emb.clone());
        Ok(emb)
    }

    async fn embed_batch(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
        let mut results: Vec<Option<Vec<f32>>> = (0..texts.len()).map(|_| None).collect();
        let mut miss_indices: Vec<usize> = Vec::new();
        let mut miss_texts: Vec<String> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            if let Some(cached) = embedding_cache_get(t, &self.model, self.dimension) {
                results[i] = Some(cached);
            } else {
                miss_indices.push(i);
                miss_texts.push(t.clone());
            }
        }
        if miss_texts.is_empty() {
            return Ok(results.into_iter().map(|o| o.unwrap()).collect());
        }
        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "input": miss_texts,
        });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let data = resp["data"]
            .as_array()
            .ok_or_else(|| VivianError::Other("OpenAI embedding 响应格式错误".into()))?;
        for (i, item) in data.iter().enumerate() {
            let emb: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or_else(|| VivianError::Other("embedding 格式错误".into()))?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            let miss_idx = *miss_indices.get(i).ok_or_else(|| {
                VivianError::Other("embedding 响应条数与请求不匹配".into())
            })?;
            let miss_text = &miss_texts[i];
            embedding_cache_put(miss_text, &self.model, self.dimension, emb.clone());
            results[miss_idx] = Some(emb);
        }
        if results.iter().any(|o| o.is_none()) {
            return Err(VivianError::Other("embedding 部分结果缺失".into()));
        }
        Ok(results.into_iter().map(|o| o.unwrap()).collect())
    }
}

/// 远程嵌入包装器：在同步接口内调用异步 `OpenAIEmbedding`
///
/// 实现策略：使用 `tokio::task::block_in_place` 包裹 `Handle::current().block_on`，
/// 允许在同步上下文中执行异步嵌入调用，避免 `async` 污染 `MemoryEmbeddingProvider` trait。
/// 调用方应在 tokio 多线程 runtime 下使用。
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
            concurrency: Arc::new(Semaphore::new(REMOTE_EMBEDDING_MAX_CONCURRENCY)),
        }
    }

    pub fn with_dimension(self, dim: usize) -> Self {
        let emb = OpenAIEmbedding::new(
            self.api_key.clone(),
            Some(self.base_url.clone()),
            Some(self.model.clone()),
        )
        .with_dimension(dim);
        Self {
            inner: Arc::new(emb),
            api_key: self.api_key,
            base_url: self.base_url,
            model: self.model,
            // 复用原实例的信号量，保持全局限流语义
            concurrency: self.concurrency,
        }
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub fn dim(&self) -> usize {
        self.inner.dimension()
    }

    fn block_embed(&self, text: &str) -> VivianResult<Vec<f32>> {
        let _permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| VivianError::Other("嵌入并发数已达上限，请稍后重试".into()))?;
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::try_current()
                .map_err(|e| VivianError::Other(format!("tokio runtime 不可用: {e}")))?;
            handle.block_on(self.inner.embed(text))
        })
    }

    fn block_embed_batch(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
        let _permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| VivianError::Other("嵌入并发数已达上限，请稍后重试".into()))?;
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::try_current()
                .map_err(|e| VivianError::Other(format!("tokio runtime 不可用: {e}")))?;
            handle.block_on(self.inner.embed_batch(texts))
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

/// 嵌入服务工厂：根据 `MemoryConfig.embedding` 选择远程或哈希
///
/// 选择规则：
/// - `source == "local"`：使用本地 Ollama（http://localhost:11434/v1）
/// - `source == "cloud"` 且 `api_key` 与 `endpoint` 非空：使用远程嵌入
/// - 否则使用 256 维哈希嵌入
pub fn build_embedding(config: &AppConfig) -> Arc<dyn MemoryEmbeddingProvider> {
    if config.memory.embedding.enabled {
        // 本地 Ollama 模式
        if config.memory.embedding.source == "local" {
            let model = if config.memory.embedding.ollama_model.trim().is_empty() {
                "bge-m3".to_string()
            } else {
                config.memory.embedding.ollama_model.clone()
            };
            let provider = RemoteMemoryEmbedding::new(
                "ollama".to_string(),
                Some("http://localhost:11434/v1".to_string()),
                Some(model),
            )
            .with_dimension(config.memory.embedding.dimension);
            tracing::info!(
                "[MemoryEmbedding] 启用本地 Ollama 嵌入: model={}, dim={}",
                provider.model_name(),
                provider.dim()
            );
            return Arc::new(provider);
        }

        // 云端模式
        let api_key = config.memory.embedding.api_key.trim();
        let endpoint = config.memory.embedding.endpoint.trim();
        if !api_key.is_empty() && !endpoint.is_empty() {
            let provider = RemoteMemoryEmbedding::new(
                api_key.to_string(),
                Some(endpoint.to_string()),
                Some(config.memory.embedding.model.clone()),
            )
            .with_dimension(config.memory.embedding.dimension);
            tracing::info!(
                "[MemoryEmbedding] 启用远程嵌入: model={}, dim={}",
                provider.model_name(),
                provider.dim()
            );
            return Arc::new(provider);
        }
        tracing::warn!("[MemoryEmbedding] embedding.enabled=true 但 api_key/endpoint 为空，回退到哈希嵌入");
    }

    tracing::info!("[MemoryEmbedding] 启用哈希嵌入 (dim=256)");
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
    fn test_hashing_embedding_batch() {
        let emb = HashingMemoryEmbedding::new(64);
        let texts = vec!["你好".to_string(), "世界".to_string()];
        let results = emb.embed_batch(&texts).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 64);
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
