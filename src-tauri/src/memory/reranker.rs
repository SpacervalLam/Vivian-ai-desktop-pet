//! 独立精排（cross-encoder reranker）服务
//!
//! 在混合检索（BM25 + 向量 + RRF）召回之后，对接入的候选记忆做二次精排。
//! 与 bi-encoder 嵌入（离线存向量、余弦相似度）不同，cross-encoder 将 query 与
//! 每条文档同时送入模型，能捕捉 query-doc 间的细粒度交互，排序质量更高。
//!
//! 默认实现调用本地 Ollama 的 `/api/rerank`（bge-reranker 系列）；
//! 未配置或调用失败时静默回退到 NoopReranker（保持召回顺序），不阻塞检索主流程。

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use crate::config::manager::AppConfig;
use crate::error::{VivianError, VivianResult};

use super::types::MemoryItem;

/// 单次 rerank 调用发送的文档数。Ollama 侧对输入长度有限制，超过时按 top_k 截断。
const RERANK_MAX_DOCS_PER_CALL: usize = 32;

/// 远程 rerank 并发上限，避免耗尽 tokio worker 池
const RERANK_MAX_CONCURRENCY: usize = 2;

/// 精排返回的分数不在 [0,1] 时，按此线性归一（softmax 风格，避免负分破坏排序）
fn normalize_scores(mut scores: Vec<f64>) -> Vec<f64> {
    if scores.is_empty() {
        return scores;
    }
    let min = scores.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = (max - min).abs().max(1e-9);
    for s in &mut scores {
        *s = (*s - min) / range;
    }
    scores
}

/// 精排器 trait
///
/// 输出与 `docs` 长度一致的分数向量（越大越相关），调用方按分数降序重排。
/// 默认实现返回"保持原序"的分数（分配给递减数值），保证排序稳定。
#[async_trait]
pub trait Reranker: Send + Sync {
    /// 精排器名称（用于日志与 metadata 标注）
    fn name(&self) -> &'static str;

    /// 是否为真实精排器（true 时调用方会按其分数重排候选；
    /// false 表示回退实现，调用方应保留原有启发式排序）
    fn is_active(&self) -> bool {
        false
    }

    /// 对 `docs` 逐条打分，返回等长分数向量（越大越相关）
    async fn rerank(&self, query: &str, docs: &[MemoryItem]) -> Vec<f64>;
}

/// 回退精排器：不调用外部服务，按原顺序分配递减分数（排序稳定）
pub struct NoopReranker;

#[async_trait]
impl Reranker for NoopReranker {
    fn name(&self) -> &'static str {
        "noop"
    }

    async fn rerank(&self, _query: &str, docs: &[MemoryItem]) -> Vec<f64> {
        (0..docs.len())
            .map(|i| 1.0 - (i as f64) / (docs.len().max(1) as f64))
            .collect()
    }
}

/// Ollama rerank 客户端：调用 `POST {endpoint}/api/rerank`
///
/// Ollama 原生 rerank 接口（bge-reranker 系列，Ollama 0.5+）：
/// ```json
/// POST /api/rerank
/// { "model": "bge-reranker-v2-m3", "query": "...", "documents": ["...", "..."] }
/// → { "results": [ { "index": 0, "relevance_score": 0.9 }, ... ] }
/// ```
pub struct OllamaRerankClient {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    /// 并发上限信号量：限制同时进行的远程调用数
    concurrency: Arc<Semaphore>,
}

impl OllamaRerankClient {
    pub fn new(endpoint: String, model: String) -> Self {
        Self {
            // 本地 Ollama 服务禁止走系统代理，避免代理（Clash/V2Ray）拦截返回非 JSON 内容
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            model,
            concurrency: Arc::new(Semaphore::new(RERANK_MAX_CONCURRENCY)),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    async fn rerank_impl(&self, query: &str, docs: &[MemoryItem]) -> VivianResult<Vec<f64>> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| VivianError::Other("rerank 并发获取信号量失败".into()))?;

        let documents: Vec<String> = docs
            .iter()
            .take(RERANK_MAX_DOCS_PER_CALL)
            .map(|m| m.content.clone())
            .collect();

        let url = format!("{}/api/rerank", self.endpoint);
        let body = serde_json::json!({
            "model": self.model,
            "query": query,
            "documents": documents,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| {
                VivianError::Other(format!("调用 Ollama rerank 失败: {}", e))
            })?;

        let json: serde_json::Value = resp.json().await.map_err(|e| {
            VivianError::Other(format!("解析 Ollama rerank 响应失败: {}", e))
        })?;

        let results = json["results"].as_array().ok_or_else(|| {
            VivianError::Other("Ollama rerank 响应缺少 results 字段".into())
        })?;

        // 按 index 归位，缺失的补 0
        let mut scores: Vec<f64> = vec![0.0; documents.len()];
        for item in results {
            let idx = item["index"].as_u64().unwrap_or(u64::MAX) as usize;
            let score = item["relevance_score"].as_f64().unwrap_or(0.0);
            if idx < scores.len() {
                scores[idx] = score;
            }
        }
        Ok(scores)
    }
}

#[async_trait]
impl Reranker for OllamaRerankClient {
    fn name(&self) -> &'static str {
        "ollama-rerank"
    }

    fn is_active(&self) -> bool {
        true
    }

    async fn rerank(&self, query: &str, docs: &[MemoryItem]) -> Vec<f64> {
        match self.rerank_impl(query, docs).await {
            Ok(scores) => normalize_scores(scores),
            Err(e) => {
                tracing::warn!(
                    "[Reranker] Ollama rerank 调用失败，回退到召回顺序: {}",
                    e
                );
                NoopReranker.rerank(query, docs).await
            }
        }
    }
}

/// 精排器工厂：根据 `MemoryConfig.rerank` 选择 Ollama 或回退 Noop
///
/// 选择规则：
/// - `enabled == true` 且 `endpoint` 非空：使用 Ollama rerank（默认 model=bge-reranker-v2-m3）
/// - 否则回退到 NoopReranker（保持召回顺序）
pub fn build_reranker(config: &AppConfig) -> Arc<dyn Reranker> {
    let r = &config.memory.rerank;
    if !r.enabled || r.endpoint.trim().is_empty() {
        tracing::info!(
            "[Reranker] rerank 未启用（enabled={}, endpoint 空={}），使用 Noop 回退",
            r.enabled,
            r.endpoint.trim().is_empty()
        );
        return Arc::new(NoopReranker);
    }
    let model = if r.model.trim().is_empty() {
        "bge-reranker-v2-m3".to_string()
    } else {
        r.model.clone()
    };
    let client = OllamaRerankClient::new(r.endpoint.trim().to_string(), model);
    tracing::info!(
        "[Reranker] 启用 Ollama rerank: endpoint={}, model={}",
        client.endpoint,
        client.model
    );
    Arc::new(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(id: &str, content: &str) -> MemoryItem {
        MemoryItem::new(content.to_string(), super::super::types::Granularity::Summary, 0.5)
    }

    #[test]
    fn noop_preserves_order() {
        let docs = vec![
            make_item("a", "甲"),
            make_item("b", "乙"),
            make_item("c", "丙"),
        ];
        let scores = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(NoopReranker.rerank("q", &docs));
        assert_eq!(scores.len(), 3);
        // 降序：首条最高
        assert!(scores[0] > scores[1] && scores[1] > scores[2]);
    }

    #[test]
    fn normalize_scores_maps_to_unit_range() {
        let scores = normalize_scores(vec![0.2, 0.8, 0.5]);
        assert!((scores[0] - 0.0).abs() < 1e-9);
        assert!((scores[1] - 1.0).abs() < 1e-9);
        assert!((scores[2] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn normalize_scores_handles_single() {
        let scores = normalize_scores(vec![0.7]);
        assert!((scores[0] - 0.0).abs() < 1e-9);
    }
}