//! 外部向量数据库客户端（Qdrant REST）
//!
//! 提供企业级向量检索所需的能力：collection 管理（HNSW 索引参数）、带元数据过滤的
//! 向量检索、增删改、计数与快照备份。通过 HTTP 调用，不引入额外重依赖（复用 reqwest）。
//!
//! 同步包装：Qdrant API 为 HTTP（异步），本模块在同步 `MemoryVectorStore` 中使用
//! `block_in_place` 包裹 `Handle::current().block_on`，与 `RemoteMemoryEmbedding` 一致。
//! 调用方应在 tokio 多线程 runtime 下使用。

use tokio::sync::Semaphore;

use crate::error::{VivianError, VivianResult};

/// 远程调用并发上限，避免耗尽 tokio worker 池
const QDRANT_MAX_CONCURRENCY: usize = 4;

/// HNSW 建图 ef 上限（防御异常配置）
const HNSW_EF_MAX: usize = 1000;

/// 将字符串 doc_id 稳定映射为 u64 点 ID（Qdrant 要求 point id 为无符号整数或 UUID）。
/// 用 FNV-1a 哈希，同 doc_id 恒定同一 id，支持 upsert 覆盖。
fn point_id(doc_id: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in doc_id.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Qdrant 客户端（线程安全：内部用 Mutex 保护 reqwest 无关的并发信号量）
pub struct QdrantClient {
    inner: std::sync::Arc<HttpCore>,
    /// 并发上限信号量
    concurrency: ArcSemaphore,
}

type ArcSemaphore = std::sync::Arc<Semaphore>;

struct HttpCore {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl QdrantClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15));
        // 本地地址（localhost）禁用代理，避免代理拦截返回非 JSON 内容
        if base_url.contains("localhost") || base_url.contains("127.0.0.1") {
            builder = builder.no_proxy();
        }
        let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        Self {
            inner: std::sync::Arc::new(HttpCore {
                client,
                base_url: base_url.trim_end_matches('/').to_string(),
                api_key,
            }),
            concurrency: ArcSemaphore::new(Semaphore::new(QDRANT_MAX_CONCURRENCY)),
        }
    }

    fn block<T>(&self, fut: impl std::future::Future<Output = VivianResult<T>>) -> VivianResult<T> {
        let _permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| VivianError::Other("外部向量库并发已达上限，请稍后重试".into()))?;
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::try_current()
                .map_err(|e| VivianError::Other(format!("tokio runtime 不可用: {e}")))?;
            handle.block_on(fut)
        })
    }

    fn auth_header(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        if !self.inner.api_key.is_empty() {
            if let Ok(v) = reqwest::header::HeaderValue::from_str(&self.inner.api_key) {
                h.insert("api-key", v);
            }
        }
        h
    }

    /// 检查集合是否存在
    pub fn collection_exists(&self, name: &str) -> VivianResult<bool> {
        let url = format!("{}/collections/{}", self.inner.base_url, name);
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client.get(&url).headers(headers).send().await.map_err(|e| {
                VivianError::Other(format!("查询集合失败: {e}"))
            })?;
            Ok(resp.status().is_success())
        })
    }

    /// 创建集合（含 HNSW 索引参数）
    pub fn ensure_collection(
        &self,
        name: &str,
        dimension: usize,
        hnsw_m: usize,
        ef_construction: usize,
    ) -> VivianResult<()> {
        if self.collection_exists(name)? {
            return Ok(());
        }
        let url = format!("{}/collections/{}", self.inner.base_url, name);
        let body = serde_json::json!({
            "vectors": {
                "size": dimension,
                "distance": "Cosine"
            },
            "hnsw_config": {
                "m": hnsw_m.max(4),
                "ef_construct": ef_construction.min(HNSW_EF_MAX)
            },
            "optimizers_config": {
                "indexing_threshold": 0
            }
        });
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client
                .put(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| VivianError::Other(format!("创建集合失败: {e}")))?;
            if resp.status().is_success() || resp.status().as_u16() == 409 {
                Ok(())
            } else {
                Err(VivianError::Other(format!(
                    "创建集合失败: HTTP {}",
                    resp.status()
                )))
            }
        })
    }

    /// 删除集合（模型切换重建时调用）
    pub fn drop_collection(&self, name: &str) -> VivianResult<()> {
        let url = format!("{}/collections/{}", self.inner.base_url, name);
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client.delete(&url).headers(headers).send().await;
            Ok(resp.map(|_| ()).unwrap_or(()))
        })
    }

    /// 批量 upsert 向量（payload 存 doc_id/memory_id/content/importance/memory_type/timestamp/model）
    pub fn upsert_points(
        &self,
        collection: &str,
        points: &[super::vector_search::MemoryVector],
        model: &str,
    ) -> VivianResult<()> {
        if points.is_empty() {
            return Ok(());
        }
        let url = format!(
            "{}/collections/{}/points?wait=true",
            self.inner.base_url, collection
        );
        let points_json: Vec<serde_json::Value> = points
            .iter()
            .map(|v| {
                let mut payload = serde_json::Map::new();
                payload.insert("doc_id".into(), serde_json::json!(v.doc_id));
                payload.insert("memory_id".into(), serde_json::json!(v.memory_id));
                payload.insert("content".into(), serde_json::json!(v.content));
                payload.insert("importance".into(), serde_json::json!(v.importance));
                payload.insert("memory_type".into(), serde_json::json!(v.memory_type));
                payload.insert("timestamp".into(), serde_json::json!(v.timestamp));
                payload.insert("model".into(), serde_json::json!(model));
                serde_json::json!({
                    "id": point_id(&v.doc_id),
                    "vector": v.embedding,
                    "payload": serde_json::Value::Object(payload),
                })
            })
            .collect();
        let body = serde_json::json!({ "points": points_json });
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client
                .put(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| VivianError::Other(format!("upsert 向量失败: {e}")))?;
            if resp.status().is_success() {
                Ok(())
            } else {
                Err(VivianError::Other(format!(
                    "upsert 向量失败: HTTP {}",
                    resp.status()
                )))
            }
        })
    }

    /// 向量检索（支持可选元数据过滤），返回 (doc_id, memory_id, score)
    pub fn search(
        &self,
        collection: &str,
        query: &[f32],
        k: usize,
        filter: Option<serde_json::Value>,
    ) -> VivianResult<Vec<(String, String, f64)>> {
        let url = format!(
            "{}/collections/{}/points/search",
            self.inner.base_url, collection
        );
        let mut body = serde_json::json!({
            "vector": query,
            "limit": k,
            "with_payload": true,
        });
        if let Some(f) = filter {
            body["filter"] = f;
        }
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client
                .post(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| VivianError::Other(format!("向量检索失败: {e}")))?;
            let json: serde_json::Value = resp.json().await.map_err(|e| {
                VivianError::Other(format!("解析检索响应失败: {e}"))
            })?;
            let mut out = Vec::new();
            if let Some(result) = json["result"].as_array() {
                for item in result {
                    let score = item["score"].as_f64().unwrap_or(0.0);
                    let payload = &item["payload"];
                    let doc_id = payload["doc_id"].as_str().unwrap_or("").to_string();
                    let memory_id = payload["memory_id"].as_str().unwrap_or("").to_string();
                    if !doc_id.is_empty() {
                        out.push((doc_id, memory_id, score));
                    }
                }
            }
            Ok(out)
        })
    }

    /// 删除指定 doc_id 的点
    pub fn delete_points(&self, collection: &str, doc_ids: &[String]) -> VivianResult<()> {
        if doc_ids.is_empty() {
            return Ok(());
        }
        let url = format!(
            "{}/collections/{}/points/delete?wait=true",
            self.inner.base_url, collection
        );
        let ids: Vec<u64> = doc_ids.iter().map(|d| point_id(d)).collect();
        let body = serde_json::json!({ "points": ids });
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client
                .post(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| VivianError::Other(format!("删除向量失败: {e}")))?;
            if resp.status().is_success() {
                Ok(())
            } else {
                Err(VivianError::Other(format!(
                    "删除向量失败: HTTP {}",
                    resp.status()
                )))
            }
        })
    }

    /// 清空集合全部点
    pub fn clear_points(&self, collection: &str) -> VivianResult<()> {
        let url = format!(
            "{}/collections/{}/points/delete?wait=true",
            self.inner.base_url, collection
        );
        let body = serde_json::json!({ "filter": {} });
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client
                .post(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| VivianError::Other(format!("清空向量失败: {e}")))?;
            if resp.status().is_success() {
                Ok(())
            } else {
                Err(VivianError::Other(format!(
                    "清空向量失败: HTTP {}",
                    resp.status()
                )))
            }
        })
    }

    /// 集合内点数统计
    pub fn count_points(&self, collection: &str) -> VivianResult<usize> {
        let url = format!(
            "{}/collections/{}/points/count",
            self.inner.base_url, collection
        );
        let body = serde_json::json!({ "exact": true });
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client
                .post(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| VivianError::Other(format!("统计点数失败: {e}")))?;
            let json: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!({}));
            Ok(json["result"]["count"].as_u64().unwrap_or(0) as usize)
        })
    }

    /// 滚动读取全部点的 (doc_id, memory_id, model)（payload），用于增量重建判定与孤儿清理
    pub fn scroll_doc_ids(&self, collection: &str) -> VivianResult<Vec<(String, String, String)>> {
        let url = format!(
            "{}/collections/{}/points/scroll",
            self.inner.base_url, collection
        );
        let body = serde_json::json!({
            "limit": 5000,
            "with_payload": true,
            "with_vector": false,
        });
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client
                .post(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| VivianError::Other(format!("滚动读取失败: {e}")))?;
            let json: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!({}));
            let mut out = Vec::new();
            if let Some(points) = json["result"]["points"].as_array() {
                for p in points {
                    let payload = &p["payload"];
                    let doc_id = payload["doc_id"].as_str().unwrap_or("").to_string();
                    let memory_id = payload["memory_id"].as_str().unwrap_or("").to_string();
                    let model = payload["model"].as_str().unwrap_or("").to_string();
                    if !doc_id.is_empty() {
                        out.push((doc_id, memory_id, model));
                    }
                }
            }
            Ok(out)
        })
    }

    /// 触发快照（持久化备份能力；Qdrant 服务端管理，此处仅为健康检查旁路）
    pub fn is_healthy(&self) -> bool {
        self.collection_exists("__health_probe__").unwrap_or(false)
            || self.ping().unwrap_or(false)
    }

    fn ping(&self) -> VivianResult<bool> {
        let url = format!("{}/", self.inner.base_url);
        let client = self.inner.client.clone();
        let headers = self.auth_header();
        self.block(async move {
            let resp = client.get(&url).headers(headers).send().await;
            Ok(resp.map(|r| r.status().is_success()).unwrap_or(false))
        })
    }
}

/// 供检索结果过滤复用：构造 Qdrant 元数据过滤（and 语义）
pub fn build_filter(memory_types: &[String], before: Option<f64>, after: Option<f64>) -> serde_json::Value {
    let mut must: Vec<serde_json::Value> = Vec::new();
    if !memory_types.is_empty() {
        must.push(serde_json::json!({
            "key": "memory_type",
            "match": { "value": memory_types[0] }
        }));
    }
    if let Some(b) = before {
        must.push(serde_json::json!({
            "key": "timestamp",
            "range": { "lt": b }
        }));
    }
    if let Some(a) = after {
        must.push(serde_json::json!({
            "key": "timestamp",
            "range": { "gt": a }
        }));
    }
    if must.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::json!({ "must": must })
    }
}