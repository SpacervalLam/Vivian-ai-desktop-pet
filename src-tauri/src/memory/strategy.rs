//! 检索策略抽象
//!
//! 将 MemoryManager.search_memories 中的 4 种 RetrievalStrategy 分发逻辑
//! 拆分为独立 struct，每个 struct 实现一个策略。Auto 策略作为组合策略，
//! Auto 以本地多路召回为基础，向量不可用时保留词法和图谱结果。

use std::collections::HashMap;
use std::sync::Arc;

use crate::memory::age::staleness_text;
use crate::memory::embedding::MemoryEmbeddingProvider;
use crate::memory::graph_store::KnowledgeGraph;
use crate::memory::retriever::{attach_items_with_weights, hybrid_search_with_arms, RecallArms, RetrievalWeights};
use crate::memory::types::MemoryItem;
use crate::memory::vector_search::MemoryVectorStore;

pub enum QueryEmbedding<'a> {
    Generate,
    /// None means embedding already failed or exceeded its budget; never retry it here.
    Prepared(Option<&'a [f32]>),
}

/// 检索上下文：策略执行所需的只读访问
pub struct RetrievalContext<'a> {
    pub entries: &'a [MemoryItem],
    pub vector_store: &'a MemoryVectorStore,
    pub embedding: &'a Arc<dyn MemoryEmbeddingProvider>,
    /// 知识图谱引用（None 时跳过图谱路检索）
    pub graph: Option<&'a Arc<KnowledgeGraph>>,
    /// 五因子加权配置；None 时使用默认权重
    pub weights: Option<RetrievalWeights>,
    pub query_embedding: QueryEmbedding<'a>,
    pub prepared_arms: Option<&'a RecallArms>,
}

impl<'a> RetrievalContext<'a> {
    /// 当前生效的权重（None 退化为默认）
    pub fn effective_weights(&self) -> RetrievalWeights {
        self.weights.unwrap_or_default()
    }

    fn embed_query(&self, query: &str) -> Option<Vec<f32>> {
        match self.query_embedding {
            QueryEmbedding::Prepared(value) => value.map(<[f32]>::to_vec),
            QueryEmbedding::Generate => self.embedding.embed(query).map_err(|error| {
                tracing::warn!(%error, "查询嵌入失败，保留本地召回");
            }).ok(),
        }
    }
}

/// 检索策略 trait
pub trait MemoryRetrievalStrategy: Send + Sync {
    /// 策略名称（用于日志和 metadata）
    fn name(&self) -> &'static str;

    /// 执行检索，返回按相关度降序排列的 MemoryItem 列表
    fn search(&self, ctx: &RetrievalContext<'_>, query: &str, limit: usize) -> Vec<MemoryItem>;
}

// ============================================================================
// KeywordStrategy：子串匹配 + match_score 排序
// ============================================================================

pub struct KeywordStrategy;

impl MemoryRetrievalStrategy for KeywordStrategy {
    fn name(&self) -> &'static str {
        "keyword"
    }

    fn search(&self, ctx: &RetrievalContext<'_>, query: &str, limit: usize) -> Vec<MemoryItem> {
        search_keyword_impl(ctx.entries, query, limit)
    }
}

fn search_keyword_impl(entries: &[MemoryItem], query: &str, limit: usize) -> Vec<MemoryItem> {
    let lower_query = query.to_lowercase();
    let query_terms = super::tokenize::tokenize(&lower_query);

    let mut scored: Vec<(MemoryItem, f64)> = entries
        .iter()
        .filter_map(|m| {
            let content_lower = m.content.to_lowercase();
            let tags_lower: Vec<String> = m.tags.iter().map(|t| t.to_lowercase()).collect();

            let mut score = 0.0;
            if !query_terms.is_empty() {
                for term in &query_terms {
                    if content_lower.contains(term.as_str()) {
                        score += 1.0;
                    }
                    if tags_lower.iter().any(|t| t.contains(term.as_str())) {
                        score += 0.5;
                    }
                }
            } else if content_lower.contains(&lower_query) {
                score = 1.0;
            }

            if score > 0.0 && super::recall_policy::admit(query, m, None, false).is_some() {
                score += m.importance * 0.1;
                Some((m.clone(), score))
            } else {
                None
            }
        })
        .collect();

    if scored.is_empty() {
        // A miss must remain a no-match; arbitrary fallback entries pollute prompts.
        return Vec::new();
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(mut m, _)| {
        if let Some(evidence) = super::recall_policy::admit(query, &m, None, false) {
            super::recall_policy::stamp(&mut m, query, evidence);
        }
        m
    }).collect()
}

// ============================================================================
// VectorStrategy：嵌入 + 余弦相似度 + match_score 融合
// ============================================================================

pub struct VectorStrategy;

impl MemoryRetrievalStrategy for VectorStrategy {
    fn name(&self) -> &'static str {
        "vector"
    }

    fn search(&self, ctx: &RetrievalContext<'_>, query: &str, limit: usize) -> Vec<MemoryItem> {
        search_vector_impl(ctx, query, limit)
    }
}

fn search_vector_impl(
    ctx: &RetrievalContext<'_>,
    query: &str,
    limit: usize,
) -> Vec<MemoryItem> {
    let hits = if let Some(arms) = ctx.prepared_arms { arms.vector.clone() } else {
        let Some(qemb) = ctx.embed_query(query) else {return search_keyword_impl(ctx.entries,query,limit);};
        super::retriever::vector_search(ctx.vector_store, &qemb, ctx.entries, limit)
    };
    if hits.is_empty() {
        return search_keyword_impl(ctx.entries, query, limit);
    }

    // id → 索引映射
    let mut id_index: HashMap<&str, usize> = HashMap::new();
    for (i, e) in ctx.entries.iter().enumerate() {
        id_index.insert(&e.id, i);
    }

    let mut results: Vec<MemoryItem> = Vec::new();
    for (memory_id, sim) in hits {
        let Some(&idx) = id_index.get(memory_id.as_str()) else {
            continue;
        };
        let mut item = ctx.entries[idx].clone();
        let semantic = (ctx.prepared_arms.is_some() || ctx.embedding.model_id() != "hashing").then_some(sim);
        let Some(evidence) = super::recall_policy::admit(query, &item, semantic, false) else { continue; };
        super::recall_policy::stamp(&mut item, query, evidence);
        let ms = compute_match_score(&item, query);
        let combined = 0.75 * sim + 0.25 * (ms / (1.0 + ms));
        if let Some(obj) = item.metadata.as_object_mut() {
            obj.insert("vector_similarity".into(), serde_json::json!(sim));
            obj.insert("match_score".into(), serde_json::json!(ms));
            obj.insert("combined_score".into(), serde_json::json!(combined));
        }
        results.push(item);
    }

    results.sort_by(|a, b| {
        let ca = a.metadata.get("combined_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cb = b.metadata.get("combined_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    // 对 Knowledge 类型施加时间衰减和过期惩罚
    apply_temporal_decay(&mut results);
    results
}

fn compute_match_score(item: &MemoryItem, query: &str) -> f64 {
    let lower_query = query.to_lowercase();
    let content_lower = item.content.to_lowercase();
    let terms: Vec<&str> = lower_query.split_whitespace().collect();
    let mut score = 0.0;
    for term in &terms {
        if content_lower.contains(term) {
            score += 1.0;
        }
    }
    if terms.is_empty() && content_lower.contains(&lower_query) {
        score = 1.0;
    }
    score + item.importance * 0.1
}

// ============================================================================
// HybridStrategy：match_score + 关键词 boost（简化版，非 BM25）
// ============================================================================

pub struct HybridStrategy;

impl MemoryRetrievalStrategy for HybridStrategy {
    fn name(&self) -> &'static str {
        "hybrid"
    }

    fn search(&self, ctx: &RetrievalContext<'_>, query: &str, limit: usize) -> Vec<MemoryItem> {
        let lower_query = query.to_lowercase();
        let mut scored: Vec<(MemoryItem, f64)> = ctx
            .entries
            .iter()
            .filter(|m| super::recall_policy::admit(query, m, None, false).is_some())
            .map(|m| {
                let ms = compute_match_score(m, query);
                let boost = if m.content.to_lowercase().contains(&lower_query) {
                    0.3
                } else {
                    0.0
                };
                (m.clone(), ms + boost)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        let mut results: Vec<MemoryItem> = scored.into_iter().map(|(m, _)| m).collect();
        // 注入 metadata
        for item in &mut results {
            let ms = compute_match_score(item, query);
            if let Some(obj) = item.metadata.as_object_mut() {
                obj.insert("combined_score".into(), serde_json::json!(ms));
            }
        }
        // 对 Knowledge 类型施加时间衰减和过期惩罚
        apply_temporal_decay(&mut results);
        results
    }
}

// ============================================================================
// AutoStrategy：向量可选的多路融合
// ============================================================================

pub struct AutoStrategy;

impl MemoryRetrievalStrategy for AutoStrategy {
    fn name(&self) -> &'static str {
        "auto"
    }

    fn search(&self, ctx: &RetrievalContext<'_>, query: &str, limit: usize) -> Vec<MemoryItem> {
        if limit == 0 || ctx.entries.is_empty() { return Vec::new(); }
        // 向量是可选的一路：失败或超时仍执行 BM25、实体与图谱，不再重复嵌入。
        if ctx.prepared_arms.is_some() || ctx.entries.len() >= 3 {
            let qemb = ctx.embed_query(query);
            let candidate_k = limit.saturating_mul(4).max(20);
            let hits = hybrid_search_with_arms(
                ctx.entries, query, qemb.as_deref(), Some(ctx.vector_store), ctx.graph,
                candidate_k, limit, ctx.prepared_arms,
            );
            if !hits.is_empty() {
                let attached = attach_items_with_weights(hits, ctx.entries, ctx.effective_weights(), query);
                let mut items: Vec<MemoryItem> = attached.into_iter().map(|h| h.item).collect();
                inject_staleness(&mut items);
                apply_temporal_decay(&mut items);
                return items;
            }
            return search_keyword_impl(ctx.entries, query, limit);
        }

        search_keyword_impl(ctx.entries, query, limit)
    }
}

fn inject_staleness(items: &mut [MemoryItem]) {
    let now = chrono::Utc::now().timestamp() as f64;
    for item in items {
        if let Some(hint) = staleness_text(item.timestamp, now) {
            if let Some(obj) = item.metadata.as_object_mut() {
                obj.insert("staleness".into(), serde_json::json!(hint));
            }
        }
    }
}

/// 时间衰减半衰期（天）：30 天后 recency_factor ≈ 1/e ≈ 0.368
const TEMPORAL_HALFLIFE_DAYS: f64 = 30.0;
/// 已过 TTL 的知识的惩罚系数：分数乘以 0.3，降权但不硬删
const EXPIRED_PENALTY: f64 = 0.3;

/// 对检索结果施加时间衰减和过期惩罚。
///
/// - 对 Knowledge 类型记忆：根据创建时间计算 recency_factor，乘以 combined_score
/// - 对已过 expires_at 的知识：额外乘以 EXPIRED_PENALTY
/// - 重新按调整后的分数排序
///
/// 其他类型记忆不受影响（对话/事件等有自己的热度机制）。
fn apply_temporal_decay(items: &mut [MemoryItem]) {
    let now = chrono::Utc::now().timestamp() as f64;

    for item in items.iter_mut() {
        // 仅对 Knowledge 类型施加时间衰减
        if item.memory_type != "knowledge" {
            continue;
        }

        let combined = item
            .metadata
            .get("fused_score")
            .or_else(|| item.metadata.get("combined_score"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        // 计算时间衰减因子：exp(-age_days / halflife)
        let age_days = ((now - item.timestamp).max(0.0)) / 86400.0;
        let recency_factor = (-age_days / TEMPORAL_HALFLIFE_DAYS).exp();

        // 检查是否已过 TTL
        let is_expired = item
            .metadata
            .get("expires_at")
            .and_then(|v| v.as_f64())
            .map(|expires_at| now >= expires_at)
            .unwrap_or(false);

        let penalty = if is_expired { EXPIRED_PENALTY } else { 1.0 };
        let adjusted = combined * recency_factor * penalty;

        if let Some(obj) = item.metadata.as_object_mut() {
            obj.insert("recency_factor".into(), serde_json::json!(recency_factor));
            obj.insert("temporal_adjusted_score".into(), serde_json::json!(adjusted));
            if is_expired {
                obj.insert("expired".into(), serde_json::json!(true));
            }
        }
    }

    // 重新排序：有 temporal_adjusted_score 的按其排序，没有的按原 combined_score
    items.sort_by(|a, b| {
        let sa = a
            .metadata
            .get("temporal_adjusted_score")
            .and_then(|v| v.as_f64())
            .or_else(|| a.metadata.get("combined_score").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        let sb = b
            .metadata
            .get("temporal_adjusted_score")
            .and_then(|v| v.as_f64())
            .or_else(|| b.metadata.get("combined_score").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
}

// ============================================================================
// GraphStrategy：仅走知识图谱路（relational recall）
// ============================================================================

pub struct GraphStrategy;

impl MemoryRetrievalStrategy for GraphStrategy {
    fn name(&self) -> &'static str {
        "graph"
    }

    fn search(&self, ctx: &RetrievalContext<'_>, query: &str, limit: usize) -> Vec<MemoryItem> {
        let Some(graph) = ctx.graph else {
            // 无图谱时回退到关键词检索
            return search_keyword_impl(ctx.entries, query, limit);
        };

        let hits = crate::memory::relational_recall::build_relational_arm(
            graph,
            query,
            ctx.entries,
            limit,
        );

        if hits.is_empty() {
            return Vec::new();
        }

        // 映射回 MemoryItem
        let id_index: HashMap<&str, usize> = ctx.entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.as_str(), i))
            .collect();

        let mut results: Vec<MemoryItem> = hits
            .into_iter()
            .filter_map(|h| {
                let &idx = id_index.get(h.memory_id.as_str())?;
                let mut cloned = ctx.entries[idx].clone();
                let evidence = super::recall_policy::admit(query, &cloned, None, true)?;
                super::recall_policy::stamp(&mut cloned, query, evidence);
                if let Some(obj) = cloned.metadata.as_object_mut() {
                    obj.insert("graph_hop".into(), serde_json::json!(h.hop));
                    obj.insert("graph_relation".into(), serde_json::json!(h.via_relation.as_str()));
                    obj.insert("graph_seed".into(), serde_json::json!(h.seed));
                    obj.insert("graph_weight".into(), serde_json::json!(h.edge_weight));
                }
                Some(cloned)
            })
            .collect();

        inject_staleness(&mut results);
        results
    }
}

/// 工厂函数：根据 RetrievalStrategy enum 创建对应策略实例
pub fn create_strategy(
    strategy: &crate::memory::types::RetrievalStrategy,
) -> Box<dyn MemoryRetrievalStrategy> {
    use crate::memory::types::RetrievalStrategy;
    match strategy {
        RetrievalStrategy::Auto => Box::new(AutoStrategy),
        RetrievalStrategy::Keyword => Box::new(KeywordStrategy),
        RetrievalStrategy::Vector => Box::new(VectorStrategy),
        RetrievalStrategy::Hybrid => Box::new(HybridStrategy),
        RetrievalStrategy::Graph => Box::new(GraphStrategy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding::HashingMemoryEmbedding;
    use crate::memory::types::{Granularity, MemoryItem, RetrievalStrategy};
    use crate::memory::vector_search::MemoryVectorStore;
    use std::path::PathBuf;

    #[test]
    fn every_strategy_rejects_unrelated_high_importance_and_weak_vector_candidates() {
        let mut unrelated = MemoryItem::new("用户养了一只猫".into(),Granularity::Summary,1.0);
        unrelated.id = "unrelated".into();
        let items = vec![unrelated]; let dir = tempfile::tempdir().unwrap();
        let store = MemoryVectorStore::new(dir.path().join("vectors.db"),2,"test").unwrap();
        let embedding: Arc<dyn MemoryEmbeddingProvider> = Arc::new(HashingMemoryEmbedding::default());
        let arms = RecallArms { lexical:vec![],vector:vec![("unrelated".into(),0.65)] };
        let ctx = RetrievalContext { entries:&items,vector_store:&store,embedding:&embedding,
            graph:None,weights:None,query_embedding:QueryEmbedding::Prepared(None),prepared_arms:Some(&arms) };
        for strategy in [RetrievalStrategy::Auto,RetrievalStrategy::Keyword,RetrievalStrategy::Vector,RetrievalStrategy::Hybrid,RetrievalStrategy::Graph] {
            assert!(create_strategy(&strategy).search(&ctx,"量子纠缠",5).is_empty(),"{strategy:?}");
        }
    }

    #[test]
    fn old_literal_matches_survive_rank_cutoffs_and_large_batches() {
        let mut items: Vec<_> = (0..8).map(|i| {
            let mut item = MemoryItem::new(format!("项目里第{i}条明确的香菜过敏记录"),Granularity::Summary,0.1);
            item.id = format!("old-{i}"); item.timestamp = 1.0; item
        }).collect();
        items[0].importance = 1.0;
        let dir=tempfile::tempdir().unwrap();
        let store=MemoryVectorStore::new(dir.path().join("vectors.db"),2,"test").unwrap();
        let embedding:Arc<dyn MemoryEmbeddingProvider>=Arc::new(HashingMemoryEmbedding::default());
        let ctx=RetrievalContext{entries:&items,vector_store:&store,embedding:&embedding,graph:None,
            weights:Some(RetrievalWeights{min_score:1.0,..Default::default()}),
            query_embedding:QueryEmbedding::Prepared(None),prepared_arms:None};
        assert_eq!(AutoStrategy.search(&ctx,"香菜过敏",8).len(),8);
    }

    fn make_items() -> Vec<MemoryItem> {
        vec![
            {
                let mut m = MemoryItem::new("用户喜欢喝咖啡".to_string(), Granularity::Summary, 0.9);
                m.tags = vec!["user".to_string(), "preference".to_string()];
                m
            },
            {
                let mut m = MemoryItem::new("今天天气不错".to_string(), Granularity::Turn, 0.3);
                m.tags = vec!["user".to_string()];
                m
            },
            {
                let mut m = MemoryItem::new("用户叫小明".to_string(), Granularity::Summary, 0.8);
                m.tags = vec!["user".to_string(), "identity".to_string()];
                m
            },
        ]
    }

    fn make_ctx(items: &[MemoryItem]) -> (MemoryVectorStore, Arc<dyn MemoryEmbeddingProvider>) {
        let emb: Arc<dyn MemoryEmbeddingProvider> = Arc::new(HashingMemoryEmbedding::default());
        let mut vs = MemoryVectorStore::new(
            PathBuf::from("test_vectors.json"),
            emb.dimension(),
            emb.model_id(),
        )
        .expect("创建测试向量库失败");
        for m in items {
            if let Ok(e) = emb.embed(&m.content) {
                vs.add(crate::memory::vector_search::MemoryVector {
                    doc_id: m.id.clone(),
                    memory_id: m.id.clone(),
                    content: m.content.clone(),
                    embedding: e,
                    importance: m.importance,
                    memory_type: "user".to_string(),
                    timestamp: m.timestamp,
                })
                .expect("测试向量插入失败");
            }
        }
        (vs, emb)
    }

    #[test]
    fn test_keyword_strategy_finds_match() {
        let items = make_items();
        let (vs, emb) = make_ctx(&items);
        let ctx = RetrievalContext {
            entries: &items,
            vector_store: &vs,
            embedding: &emb,
            graph: None,
            weights: None,
            query_embedding: QueryEmbedding::Generate,
            prepared_arms: None,
        };
        let result = KeywordStrategy.search(&ctx, "咖啡", 5);
        assert!(result.iter().any(|m| m.content.contains("咖啡")));
    }

    #[test]
    fn test_vector_strategy_returns_results() {
        let items = make_items();
        let (vs, emb) = make_ctx(&items);
        let ctx = RetrievalContext {
            entries: &items,
            vector_store: &vs,
            embedding: &emb,
            graph: None,
            weights: None,
            query_embedding: QueryEmbedding::Generate,
            prepared_arms: None,
        };
        let result = VectorStrategy.search(&ctx, "用户喜好", 3);
        // 向量检索应返回非空结果
        assert!(!result.is_empty());
    }

    #[test]
    fn test_auto_strategy_falls_back_when_vector_empty() {
        let items = make_items();
        let emb: Arc<dyn MemoryEmbeddingProvider> = Arc::new(HashingMemoryEmbedding::default());
        let vs = MemoryVectorStore::new(
            PathBuf::from("test_vectors.json"),
            emb.dimension(),
            emb.model_id(),
        )
        .expect("创建测试向量库失败"); // 空向量库
        let ctx = RetrievalContext {
            entries: &items,
            vector_store: &vs,
            embedding: &emb,
            graph: None,
            weights: None,
            query_embedding: QueryEmbedding::Generate,
            prepared_arms: None,
        };
        // 空向量库仍可通过本地词法召回。
        let result = AutoStrategy.search(&ctx, "咖啡", 5);
        assert!(result.iter().any(|m| m.content.contains("咖啡")));
    }

    #[test]
    fn test_factory_creates_correct_strategy() {
        use crate::memory::types::RetrievalStrategy;
        assert_eq!(create_strategy(&RetrievalStrategy::Auto).name(), "auto");
        assert_eq!(create_strategy(&RetrievalStrategy::Keyword).name(), "keyword");
        assert_eq!(create_strategy(&RetrievalStrategy::Vector).name(), "vector");
        assert_eq!(create_strategy(&RetrievalStrategy::Hybrid).name(), "hybrid");
        assert_eq!(create_strategy(&RetrievalStrategy::Graph).name(), "graph");
    }

    #[test]
    fn failed_or_timed_out_embedding_keeps_lexical_recall_without_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Offline(Arc<AtomicUsize>);
        impl MemoryEmbeddingProvider for Offline {
            fn dimension(&self) -> usize { 2 }
            fn embed(&self, _: &str) -> crate::error::VivianResult<Vec<f32>> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Err(crate::error::VivianError::Memory("offline".into()))
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let embedding: Arc<dyn MemoryEmbeddingProvider> = Arc::new(Offline(calls.clone()));
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryVectorStore::new(dir.path().join("vectors.db"), 2, "offline").unwrap();
        let items = make_items();
        let mut ctx = RetrievalContext { entries: &items, vector_store: &store, embedding: &embedding,
            graph: None, weights: None, query_embedding: QueryEmbedding::Generate, prepared_arms: None };
        let result = AutoStrategy.search(&ctx, "咖啡", 5);
        assert!(result.iter().any(|item| item.content.contains("咖啡")));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        ctx.query_embedding = QueryEmbedding::Prepared(None);
        let result = AutoStrategy.search(&ctx, "咖啡", 5);
        assert!(result.iter().any(|item| item.content.contains("咖啡")));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn prepared_semantic_match_survives_a_single_candidate_without_word_overlap() {
        let mut item=MemoryItem::new("雨停了，天边出现了彩虹".into(),Granularity::Summary,0.8);
        item.id="semantic-only".into();
        let items=vec![item];let dir=tempfile::tempdir().unwrap();
        let store=MemoryVectorStore::new(dir.path().join("vectors.db"),2,"test").unwrap();
        let embedding:Arc<dyn MemoryEmbeddingProvider>=Arc::new(HashingMemoryEmbedding::default());
        let arms=RecallArms{lexical:Vec::new(),vector:vec![("semantic-only".into(),0.9)]};
        let context=RetrievalContext{entries:&items,vector_store:&store,embedding:&embedding,
            graph:None,weights:None,query_embedding:QueryEmbedding::Prepared(None),prepared_arms:Some(&arms)};
        let hits=AutoStrategy.search(&context,"weather changed",5);
        assert_eq!(hits.first().map(|hit|hit.id.as_str()),Some("semantic-only"));
    }
}
