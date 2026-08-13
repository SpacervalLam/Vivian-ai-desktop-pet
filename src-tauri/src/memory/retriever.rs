//! 混合检索器：BM25 + 向量并行检索 + RRF 融合 + 简单 rerank。
//!
//! ## 算法
//! 1. **BM25 检索**：jieba 分词，OKAPI BM25 打分，top-k
//! 2. **向量检索**：余弦相似度 top-k
//! 3. **RRF 融合**：`score = 1 / (RRF_K + rank)`，两路融合后排序
//! 4. **简单 rerank**：基于 match_score + importance + vector_sim 的二次打分
//!
//! RRF（Reciprocal Rank Fusion）对分数尺度不敏感，适合融合不同打分体系。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use jieba_rs::Jieba;
use once_cell::sync::Lazy;
use parking_lot::Mutex;

use super::graph_store::KnowledgeGraph;
use super::relational_recall::build_relational_arm;
use super::retention::QuadraticDecay;
use super::types::{current_timestamp, MemoryItem};
use super::vector_search::MemoryVectorStore;

/// RRF 常数：典型值 60，越大会让低排名项权重越大
const RRF_K: f64 = 60.0;

/// 精确实体/专名命中时的 fused_score 放大系数。
/// 用于让专有名词查询（如人名/产品名）能命中对应记忆，弥补长文本稀释向量相似度的问题。
const ENTITY_BOOST: f64 = 3.0;

/// 检索结果归属的认知层级
///
/// 让四层认知（Facts/Episodes/Memories/Beliefs）的召回结果走同一融合管线，
/// 下游可按 `result_type` 决定渲染策略与排序权重。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetrievalResultType {
    /// 用户事实/偏好/身份/重要事件/知识
    Fact,
    /// 情景记忆（带 episode_id 关联）
    Episode,
    /// 一般记忆（短/中/长期、闲聊、会话摘要、洞察、独白、旁观）
    Memory,
    /// 信念（Mind 层产物，通常不直接检索，保留枚举用于未来融合）
    Belief,
}

impl RetrievalResultType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Episode => "episode",
            Self::Memory => "memory",
            Self::Belief => "belief",
        }
    }

    /// 根据 MemoryItem 的 memory_type 字符串与 episode_id 推导认知层级
    pub fn from_memory_item(memory_type: &str, episode_id: &Option<String>) -> Self {
        if episode_id.is_some() {
            return Self::Episode;
        }
        match memory_type {
            "user" | "preference" | "identity" | "important_event" | "knowledge" => Self::Fact,
            _ => Self::Memory,
        }
    }
}

/// BM25 参数
const BM25_K1: f64 = 1.5;
const BM25_B: f64 = 0.75;

/// 全局 jieba 实例（与 filter.rs 共享）
static JIEBA: Lazy<Jieba> = Lazy::new(Jieba::new);

/// 单条记忆的分词结果缓存：词频表 + 总词数
struct DocTokens {
    freqs: HashMap<String, usize>,
    token_count: usize,
}

/// 分词缓存：key = memory_id，value = (内容指纹, 分词结果)。
/// 指纹由 content/tags/description 哈希得到，记忆内容变更后指纹不同会自动重算，
/// 避免每次对话都对全部候选记忆重复 jieba 分词。
/// 有界容量，超出阈值整体清空（分词重建成本可接受，换取内存上界）。
static TOKEN_CACHE: Lazy<Mutex<HashMap<String, (u64, DocTokens)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 分词缓存最大条目数
const TOKEN_CACHE_MAX: usize = 8000;

/// 计算记忆内容指纹（content + tags + description 的哈希）
fn content_fingerprint(content: &str, tags: &[String], desc: &Option<String>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut h);
    tags.hash(&mut h);
    desc.hash(&mut h);
    h.finish()
}

/// 对单条记忆分词，返回 (词频表, 总词数)
fn tokenize_entry(e: &MemoryItem) -> (HashMap<String, usize>, usize) {
    let mut tokens = tokenize(&e.content);
    for tag in &e.tags {
        tokens.extend(tokenize(tag));
    }
    if let Some(desc) = &e.description {
        tokens.extend(tokenize(desc));
    }
    let len = tokens.len();
    let mut freqs = HashMap::with_capacity(tokens.len());
    for t in tokens {
        *freqs.entry(t).or_insert(0) += 1;
    }
    (freqs, len)
}

/// 检索五因子加权配置
///
/// 综合 score = α·recency + β·relevance + γ·importance + δ·hook_boost + ε·need_sim
/// - recency = exp(-age_hours / tau)
/// - relevance = RRF fused_score（归一化到 [0,1]，含 semantic_boost）
/// - importance = decayed_importance × (1 + 0.2 × mood_intensity)
///   （时间衰减的有效重要性 × 情感增强系数；模拟人类遗忘曲线与情绪增强记忆）
/// - hook_boost = 含未闭环 open_hooks 的记忆获得加成（1.0 / 0.0）
/// - need_sim = 当前用户输入与记忆内容的 Jaccard 相似度（jieba 分词）
///
/// 参考Generative Agents (Park 2023)：recency + relevance + importance 三因子加权，
/// 各分量先 min-max 归一化到 [0,1] 再加权求和。本实现在此基础上扩展为五因子。
#[derive(Debug, Clone, Copy)]
pub struct RetrievalWeights {
    pub recency: f64,
    pub relevance: f64,
    pub importance: f64,
    pub hook_boost: f64,
    pub need_sim: f64,
    pub recency_tau_hours: f64,
    /// 最终融合分数的下限阈值：低于此值的命中会被过滤，避免噪声记忆污染 prompt 上下文。
    /// 默认 0.15，取值范围 [0, 1]；设为 0 等价于不启用过滤。
    pub min_score: f64,
}

impl Default for RetrievalWeights {
    fn default() -> Self {
        Self {
            recency: 0.25,
            relevance: 0.40,
            importance: 0.15,
            hook_boost: 0.10,
            need_sim: 0.10,
            recency_tau_hours: 24.0,
            min_score: 0.15,
        }
    }
}

impl RetrievalWeights {
    /// 从 `RetrievalWeightsConfig` 构造
    pub fn from_config(cfg: &crate::config::manager::RetrievalWeightsConfig) -> Self {
        Self {
            recency: cfg.recency,
            relevance: cfg.relevance,
            importance: cfg.importance,
            hook_boost: cfg.hook_boost,
            need_sim: cfg.need_sim,
            recency_tau_hours: cfg.recency_tau_hours,
            min_score: cfg.min_score,
        }
    }
}

/// 结构化/元数据过滤条件（企业级检索预过滤）。
///
/// 在混合检索前按 memory_type / 标签 / 时间窗口对候选记忆做精确过滤，
/// 使调用方可以定向检索（如"只要重要事件""只要某标签的近期记忆"）。
/// 所有字段均为可选；命中任一字段即参与 AND 语义（多条件同时满足）。
#[derive(Debug, Clone, Default)]
pub struct MemoryRetrievalFilter {
    /// 仅保留这些 memory_type（空/None = 不限）
    pub memory_types: Option<Vec<String>>,
    /// 命中任一标签即可（空/None = 不限）
    pub tags_any: Option<Vec<String>>,
    /// 仅保留 timestamp >= time_after
    pub time_after: Option<f64>,
    /// 仅保留 timestamp < time_before
    pub time_before: Option<f64>,
}

impl MemoryRetrievalFilter {
    /// 判断一条记忆是否满足过滤条件
    pub fn matches(&self, item: &MemoryItem) -> bool {
        if let Some(types) = &self.memory_types {
            if !types.iter().any(|t| t == &item.memory_type) {
                return false;
            }
        }
        if let Some(tags) = &self.tags_any {
            if !tags.iter().any(|t| item.tags.iter().any(|it| it == t)) {
                return false;
            }
        }
        if let Some(after) = self.time_after {
            if item.timestamp < after {
                return false;
            }
        }
        if let Some(before) = self.time_before {
            if item.timestamp >= before {
                return false;
            }
        }
        true
    }
}

/// 单条检索结果（融合后）
pub struct RetrievalHit {
    pub item: MemoryItem,
    pub bm25_score: f64,
    pub vector_score: f64,
    /// 图谱路分数（0 = 未命中图谱）
    pub graph_score: f64,
    pub fused_score: f64,
    /// 归属认知层级，由 `attach_items_with_weights` 填充真实 item 时推导
    pub result_type: RetrievalResultType,
}

/// 执行 BM25 + 向量 + 图谱并行检索，RRF 融合后返回 top-K
///
/// - `entries`：候选记忆集合
/// - `query_emb`：查询向量（None 时跳过向量检索）
/// - `vector_store`：向量库（None 时跳过向量检索）
/// - `graph`：知识图谱（None 时跳过图谱路检索）
/// - `query`：查询文本（用于 BM25 + 关系型查询解析）
/// - `candidate_k`：每路检索的候选数（建议 ≥ 2*limit）
/// - `limit`：最终返回数
pub fn hybrid_search(
    entries: &[MemoryItem],
    query: &str,
    query_emb: Option<&[f32]>,
    vector_store: Option<&MemoryVectorStore>,
    graph: Option<&Arc<KnowledgeGraph>>,
    candidate_k: usize,
    limit: usize,
) -> Vec<RetrievalHit> {
    if entries.is_empty() {
        return Vec::new();
    }

    let bm25_hits = bm25_search(entries, query, candidate_k);
    let vector_hits = match (query_emb, vector_store) {
        (Some(emb), Some(store)) if !store.is_empty() => vector_search(store, emb, entries, candidate_k),
        _ => Vec::new(),
    };
    // 实体/专名补充召回路：从 query 提取显著词（人名/产品名等），对每个显著词单独做
    // 词面检索，补回长 query 中被 BM25 稀释的专名命中。作为第四路参与 RRF 融合。
    let entity_hits = entity_arm_search(entries, query, candidate_k);

    // 图谱路：仅当查询为关系型（"谁投资了X"/"X和Y的关系"）时命中
    let graph_hits = match graph {
        Some(g) => {
            let relational_hits = build_relational_arm(g, query, entries, candidate_k);
            relational_hits
                .into_iter()
                .map(|h| (h.memory_id, 1.0 / (h.hop as f64)))
                .collect::<Vec<_>>()
        }
        None => Vec::new(),
    };

    let fused = rrf_fuse(bm25_hits, vector_hits, graph_hits, entity_hits);
    let mut reranked = rerank(fused, query);

    apply_post_graph_compensation(&mut reranked, entries, query, 2, 0.30);

    // 截断到 limit
    reranked.truncate(limit);
    reranked
}

fn apply_post_graph_compensation(
    reranked: &mut Vec<RetrievalHit>,
    entries: &[MemoryItem],
    query: &str,
    target_n: usize,
    min_score: f64,
) {
    if reranked.is_empty() || target_n == 0 {
        return;
    }
    let hit_ids: std::collections::HashSet<&str> =
        reranked.iter().map(|h| h.item.id.as_str()).collect();
    let candidates: Vec<MemoryItem> = entries
        .iter()
        .filter(|e| !hit_ids.contains(e.id.as_str()))
        .cloned()
        .collect();
    if candidates.is_empty() {
        return;
    }
    let hits_refs: Vec<&MemoryItem> = reranked.iter().map(|h| &h.item).collect();
    let compensated = post_graph_compensation(&candidates, query, &hits_refs, target_n, min_score);
    let base_score = reranked
        .last()
        .map(|h| h.fused_score)
        .unwrap_or(0.0)
        .max(0.30);
    for (item, score) in compensated {
        reranked.push(RetrievalHit {
            item,
            bm25_score: 0.0,
            vector_score: 0.0,
            graph_score: score.total,
            fused_score: base_score * 0.5 + score.total * 0.5,
            result_type: RetrievalResultType::Memory,
        });
    }
    reranked.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// BM25 检索：返回 (memory_id, bm25_score) 列表，按分数降序
fn bm25_search(entries: &[MemoryItem], query: &str, k: usize) -> Vec<(String, f64)> {
    let query_terms = tokenize(query);
    if query_terms.is_empty() {
        return Vec::new();
    }

    // 从缓存获取每条记忆的分词频次与长度，命中且指纹一致时跳过重复分词
    let mut cache = TOKEN_CACHE.lock();
    let mut doc_freqs: Vec<HashMap<String, usize>> = Vec::with_capacity(entries.len());
    let mut doc_lens: Vec<usize> = Vec::with_capacity(entries.len());
    for e in entries {
        let fp = content_fingerprint(&e.content, &e.tags, &e.description);
        if let Some((cached_fp, dt)) = cache.get(&e.id) {
            if *cached_fp == fp {
                doc_freqs.push(dt.freqs.clone());
                doc_lens.push(dt.token_count);
                continue;
            }
        }
        let (freqs, len) = tokenize_entry(e);
        doc_lens.push(len);
        doc_freqs.push(freqs.clone());
        cache.insert(e.id.clone(), (fp, DocTokens { freqs, token_count: len }));
    }
    if cache.len() > TOKEN_CACHE_MAX {
        cache.clear();
    }

    let n_docs = entries.len() as f64;
    let avg_dl: f64 = doc_lens.iter().map(|&l| l as f64).sum::<f64>() / n_docs.max(1.0);

    // IDF（BM25 变体，避免分母为 0）
    let mut df: HashMap<String, usize> = HashMap::new();
    for freq in &doc_freqs {
        for term in freq.keys() {
            *df.entry(term.clone()).or_insert(0) += 1;
        }
    }
    let idf: HashMap<String, f64> = df
        .iter()
        .map(|(term, &d)| {
            let idf = ((n_docs - d as f64 + 0.5) / (d as f64 + 0.5) + 1.0).ln();
            (term.clone(), idf)
        })
        .collect();

    let mut scored: Vec<(String, f64)> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let freqs = &doc_freqs[i];
            let dl = doc_lens[i] as f64;
            let norm = 1.0 - BM25_B + BM25_B * (dl / avg_dl.max(1.0));
            let mut score = 0.0;
            for term in &query_terms {
                if let Some(&tf) = freqs.get(term) {
                    let idf_v = idf.get(term).copied().unwrap_or(0.0);
                    let tf_norm = (tf as f64 * (BM25_K1 + 1.0))
                        / (tf as f64 + BM25_K1 * norm);
                    score += idf_v * tf_norm;
                }
            }
            (e.id.clone(), score.max(0.0))
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

/// 向量检索：返回 (memory_id, vector_score) 列表，按分数降序
fn vector_search(
    store: &MemoryVectorStore,
    query_emb: &[f32],
    entries: &[MemoryItem],
    k: usize,
) -> Vec<(String, f64)> {
    let hits = store.search(query_emb, k);
    // 过滤掉已不在 entries 中的（防止 vector_store 与 entries 不同步）
    let valid_ids: std::collections::HashSet<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    hits.into_iter()
        .filter(|(_, mid, _)| valid_ids.contains(mid.as_str()))
        .map(|(_, mid, score)| (mid, score))
        .collect()
}

/// 实体/专名补充召回路（查询增强）
///
/// 从 query 中提取"显著词"（非纯中文短词，如人名/产品名 "AlenTinn"，或长度 ≥ 3 的中文词），
/// 对每个显著词在记忆 content/description/tags 中做词面出现检索，合并为一路候选。
///
/// 价值：长 query 中稀有专名会被 BM25 的 IDF 稀释（专名只在极少数文档中出现，
/// 但和 query 其它共现词一起打分时未必能排进 top-k）。本路把专名单独作为检索词，
/// 补回被稀释的专名命中，作为 RRF 的第四路参与融合。
fn entity_arm_search(entries: &[MemoryItem], query: &str, k: usize) -> Vec<(String, f64)> {
    let query_tokens: std::collections::HashSet<String> = tokenize(query).into_iter().collect();
    if query_tokens.is_empty() {
        return Vec::new();
    }
    // 仅保留显著词作为独立检索词
    let significant: Vec<&String> = query_tokens
        .iter()
        .filter(|t| {
            t.chars().count() >= 3 && t.chars().any(|c| !is_cjk(c))
        })
        .collect();
    if significant.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(String, f64)> = Vec::new();
    for e in entries {
        let mut haystack = e.content.to_lowercase();
        if let Some(desc) = &e.description {
            haystack.push(' ');
            haystack.push_str(&desc.to_lowercase());
        }
        if !e.tags.is_empty() {
            haystack.push(' ');
            haystack.push_str(&e.tags.join(" ").to_lowercase());
        }
        // 命中的显著词越多分越高
        let hits = significant
            .iter()
            .filter(|t| haystack.contains(t.as_str()))
            .count();
        if hits > 0 {
            scored.push((e.id.clone(), hits as f64));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

/// RRF 融合三路检索结果
///
/// 输入：bm25_hits / vector_hits / graph_hits / entity_hits 已各自按 score 降序排列
/// 输出：融合后的 (memory_id, bm25_score, vector_score, graph_score, fused_score) 列表，按 fused 降序
fn rrf_fuse(
    bm25_hits: Vec<(String, f64)>,
    vector_hits: Vec<(String, f64)>,
    graph_hits: Vec<(String, f64)>,
    entity_hits: Vec<(String, f64)>,
) -> Vec<(String, f64, f64, f64, f64)> {
    use std::collections::BTreeMap;

    let mut bm25_map: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    for (rank, (id, score)) in bm25_hits.iter().enumerate() {
        bm25_map.insert(id.clone(), (rank, *score));
    }
    let mut vec_map: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    for (rank, (id, score)) in vector_hits.iter().enumerate() {
        vec_map.insert(id.clone(), (rank, *score));
    }
    let mut graph_map: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    for (rank, (id, score)) in graph_hits.iter().enumerate() {
        graph_map.insert(id.clone(), (rank, *score));
    }
    let mut entity_map: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    for (rank, (id, score)) in entity_hits.iter().enumerate() {
        entity_map.insert(id.clone(), (rank, *score));
    }

    let mut all_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for id in bm25_map.keys() {
        all_ids.insert(id.clone());
    }
    for id in vec_map.keys() {
        all_ids.insert(id.clone());
    }
    for id in graph_map.keys() {
        all_ids.insert(id.clone());
    }
    for id in entity_map.keys() {
        all_ids.insert(id.clone());
    }

    let mut fused: Vec<(String, f64, f64, f64, f64)> = all_ids
        .into_iter()
        .map(|id| {
            let bm25 = bm25_map.get(&id);
            let vec = vec_map.get(&id);
            let graph = graph_map.get(&id);
            let entity = entity_map.get(&id);
            let bm25_rank = bm25.map(|(r, _)| *r);
            let vec_rank = vec.map(|(r, _)| *r);
            let graph_rank = graph.map(|(r, _)| *r);
            let entity_rank = entity.map(|(r, _)| *r);
            let bm25_score = bm25.map(|(_, s)| *s).unwrap_or(0.0);
            let vec_score = vec.map(|(_, s)| *s).unwrap_or(0.0);
            let graph_score = graph.map(|(_, s)| *s).unwrap_or(0.0);

            let mut rrf = 0.0;
            if let Some(r) = bm25_rank {
                rrf += 1.0 / (RRF_K + (r as f64 + 1.0));
            }
            if let Some(r) = vec_rank {
                rrf += 1.0 / (RRF_K + (r as f64 + 1.0));
            }
            if let Some(r) = graph_rank {
                rrf += 1.0 / (RRF_K + (r as f64 + 1.0));
            }
            if let Some(r) = entity_rank {
                rrf += 1.0 / (RRF_K + (r as f64 + 1.0));
            }
            (id, bm25_score, vec_score, graph_score, rrf)
        })
        .collect();

    fused.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

/// 二次 rerank：在 RRF 融合基础上叠加 match_score 与 importance 加成
fn rerank(
    fused: Vec<(String, f64, f64, f64, f64)>,
    query: &str,
) -> Vec<RetrievalHit> {
    let mut hits: Vec<RetrievalHit> = fused
        .into_iter()
        .map(|(id, bm25, vec, graph, fused_score)| {
            // 通过 id 找回 item 由调用方做（这里只产 id + 分数）
            RetrievalHit {
                item: MemoryItem {
                    id,
                    content: String::new(),
                    granularity: String::new(),
                    memory_type: String::new(),
                    importance: 0.0,
                    timestamp: 0.0,
                    embedding: None,
                    tags: Vec::new(),
                    metadata: serde_json::json!({}),
                    related_ids: Vec::new(),
                    description: None,
                    visit_count: 0,
                    last_visit_at: 0.0,
                    heat_score: 0.0,
                    open_hooks: Vec::new(),
                    reinforcement: 0.0,
                    disputation: 0.0,
                    rein_last_signal_at: 0.0,
                    disp_last_signal_at: 0.0,
                    sub_zero_days: 0,
                    sub_zero_last_increment_date: String::new(),
                    user_fact_reinforce_count: 0,
                    protected: false,
                    episode_id: None,
                    consolidated: false,
                    rebuttal_grace_remaining: 0,
                },
                bm25_score: bm25,
                vector_score: vec,
                graph_score: graph,
                fused_score,
                result_type: RetrievalResultType::Memory,
            }
        })
        .collect();

    // 对 query 完全包含的命中做小幅加权
    let q_lower = query.to_lowercase();
    for hit in &mut hits {
        if !q_lower.is_empty() && hit.item.content.to_lowercase().contains(&q_lower) {
            hit.fused_score += 0.001;
        }
    }

    hits.sort_by(|a, b| b.fused_score.partial_cmp(&a.fused_score).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

/// jieba 分词
fn tokenize(text: &str) -> Vec<String> {
    JIEBA
        .cut(text, true)
        .into_iter()
        .map(|s| s.to_lowercase())
        .filter(|s| !s.trim().is_empty() && s.chars().count() > 1)
        .collect()
}

/// 判断一个字符是否为 CJK 统一表意文字（用于识别"显著词"，如专有名词）
fn is_cjk(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// 判断记忆是否命中查询中的"显著词"（实体/专名）。
///
/// 显著词 = 长度 ≥ 3，或包含非 CJK 字符（如英文人名/产品名 "AlenTinn"）。
/// 命中判定：该词在记忆 content / description / tags 中逐字出现。
fn entity_token_hit(query_tokens: &std::collections::HashSet<String>, item: &MemoryItem) -> bool {
    let mut haystack = item.content.to_lowercase();
    if let Some(desc) = &item.description {
        haystack.push(' ');
        haystack.push_str(&desc.to_lowercase());
    }
    if !item.tags.is_empty() {
        haystack.push(' ');
        haystack.push_str(&item.tags.join(" ").to_lowercase());
    }
    query_tokens.iter().any(|t| {
        let significant = t.chars().count() >= 3 || t.chars().any(|c| !is_cjk(c));
        significant && haystack.contains(t.as_str())
    })
}

/// 语义类型对 fused_score 的加权系数。
///
/// 陪伴型 AI 检索优先级：用户偏好/反馈 > 关系事件 > 共同经历 > 项目 > 引用 > 一般闲聊。
/// 当多条记忆 BM25/Vector 分数接近时，高价值语义类型应优先 surfaced。
fn semantic_type_boost(semantic_type: &super::types::SemanticType) -> f64 {
    use super::types::SemanticType;
    match semantic_type {
        SemanticType::User | SemanticType::Feedback => 1.15,
        SemanticType::Relationship => 1.10,
        SemanticType::SharedMemory | SemanticType::Project => 1.05,
        SemanticType::Reference => 1.00,
        SemanticType::General => 0.95,
    }
}

/// 知识来源对 fused_score 的加权系数（交互对象权重差异化）。
///
/// 用户直接对话（direct）权重 > 跨角色听闻（heard）权重 > 旁观（observed）权重。
/// 确保用户记忆优先于室友记忆被检索到，避免"把用户话记成室友说的"这类认知混乱。
fn knowledge_source_boost(metadata: &serde_json::Value) -> f64 {
    metadata
        .get("knowledge_source")
        .and_then(|v| v.as_str())
        .map(|s| match s {
            "direct" => 1.20,
            "heard" => 0.90,
            "observed" => 0.50,
            _ => 1.00,
        })
        .unwrap_or(1.00)
}

/// 把检索结果中的 item 替换为真实 MemoryItem，并写入 metadata 分数
///
/// 使用默认权重（α=0.25, β=0.40, γ=0.15, δ=0.10, ε=0.10, τ=24h）。
pub fn attach_items(
    hits: Vec<RetrievalHit>,
    entries: &[MemoryItem],
    query: &str,
) -> Vec<RetrievalHit> {
    attach_items_with_weights(hits, entries, RetrievalWeights::default(), query)
}

/// 加权版的 `attach_items`：按 `RetrievalWeights` 计算五因子综合分数。
///
/// 综合 score = α·recency + β·relevance + γ·importance + δ·hook_boost + ε·need_sim
/// - recency = exp(-age_hours / tau)，age 基于 memory.timestamp
/// - relevance = 归一化的 fused_score（含 semantic_boost）
/// - importance = decayed_importance × (1 + 0.2 × mood_intensity)
///   （时间衰减的有效重要性 × 情感增强系数）
/// - hook_boost = 含未闭环 open_hooks 的记忆获得 1.0 加成，否则 0.0
/// - need_sim = 当前用户输入与记忆内容的 Jaccard 相似度（jieba 分词）
///
/// 所有分量先 min-max 归一化到 [0,1]，再加权求和（参考 Generative Agents）。
pub fn attach_items_with_weights(
    mut hits: Vec<RetrievalHit>,
    entries: &[MemoryItem],
    weights: RetrievalWeights,
    query: &str,
) -> Vec<RetrievalHit> {
    let index: HashMap<&str, &MemoryItem> = entries.iter().map(|e| (e.id.as_str(), e)).collect();

    // 预计算 query token 集合（用于 need_sim）
    let query_tokens: std::collections::HashSet<String> =
        tokenize(query).into_iter().collect();

    // 第一遍：填充真实 item、计算 fused_score（含 semantic_boost）与 raw 分量
    let now = current_timestamp();
    // (idx, raw_relevance, raw_recency, raw_importance, raw_hook_boost, raw_need_sim)
    let mut raw_components: Vec<(usize, f64, f64, f64, f64, f64)> = Vec::with_capacity(hits.len());

    for (idx, hit) in hits.iter_mut().enumerate() {
        if let Some(&item) = index.get(hit.item.id.as_str()) {
            hit.item = item.clone();
            hit.result_type =
                RetrievalResultType::from_memory_item(&item.memory_type, &item.episode_id);
            // 语义类型 boost + 知识来源 boost（交互对象权重差异化）
            let sem_boost = semantic_type_boost(&item.semantic_type());
            let ks_boost = knowledge_source_boost(&item.metadata);
            hit.fused_score *= sem_boost * ks_boost;

            // 精确实体/专名命中 boost：查询中的"显著词"（非纯中文短词，如人名/产品名）在记忆
            // 内容/描述/标签中逐字出现时，显著提高 fused_score，确保实体查询（如"AlenTinn"）
            // 能召回对应记忆，避免因长文本稀释向量相似度而被 min_score 过滤掉。
            if entity_token_hit(&query_tokens, &hit.item) {
                hit.fused_score *= ENTITY_BOOST;
            }

            // relevance：直接用 fused_score（已是 RRF 分数，含 boost）
            let raw_relevance = hit.fused_score;
            // recency：基于 memory.timestamp（创建时间），24h 后 ≈ 0.368
            let age_hours = ((now - item.timestamp).max(0.0)) / 3600.0;
            let raw_recency = (-age_hours / weights.recency_tau_hours.max(1e-6)).exp();
            // importance：时间衰减后的有效重要性（人类遗忘曲线——远期记忆重要性模糊）
            // 叠加情感增强（人类情绪增强记忆——情绪强烈的记忆更难忘，杏仁核-海马回路）
            let decayed_imp = QuadraticDecay::decayed_importance(item, now);
            let mood_intensity = (item.mood_tags().len() as f64 / 3.0).min(1.0);
            let raw_importance = decayed_imp * (1.0 + 0.2 * mood_intensity);
            // hook_boost：含未闭环 open_hooks 的记忆获得加成
            let raw_hook_boost = if item.open_hooks.iter().any(|h| h.is_open()) {
                1.0
            } else {
                0.0
            };
            // need_sim：当前用户输入与记忆内容的 Jaccard 相似度
            let raw_need_sim = jaccard_similarity(&query_tokens, &item.content);

            raw_components.push((
                idx,
                raw_relevance,
                raw_recency,
                raw_importance,
                raw_hook_boost,
                raw_need_sim,
            ));
        }
    }

    // 第二遍：min-max 归一化各分量（参考 Generative Agents）
    let n = raw_components.len() as f64;
    let (rel_min, rel_max) = min_max(raw_components.iter().map(|(_, r, _, _, _, _)| *r));
    let (rec_min, rec_max) = min_max(raw_components.iter().map(|(_, _, rec, _, _, _)| *rec));
    let (imp_min, imp_max) = min_max(raw_components.iter().map(|(_, _, _, imp, _, _)| *imp));
    let (hook_min, hook_max) = min_max(raw_components.iter().map(|(_, _, _, _, hb, _)| *hb));
    let (need_min, need_max) = min_max(raw_components.iter().map(|(_, _, _, _, _, ns)| *ns));

    let rel_range = (rel_max - rel_min).max(1e-9);
    let rec_range = (rec_max - rec_min).max(1e-9);
    let imp_range = (imp_max - imp_min).max(1e-9);
    let hook_range = (hook_max - hook_min).max(1e-9);
    let need_range = (need_max - need_min).max(1e-9);

    for (idx, raw_rel, raw_rec, raw_imp, raw_hb, raw_ns) in &raw_components {
        let hit = &mut hits[*idx];
        let norm_rel = (raw_rel - rel_min) / rel_range;
        let norm_rec = (raw_rec - rec_min) / rec_range;
        let norm_imp = (raw_imp - imp_min) / imp_range;
        let norm_hb = (raw_hb - hook_min) / hook_range;
        let norm_ns = (raw_ns - need_min) / need_range;

        let final_score = weights.recency * norm_rec
            + weights.relevance * norm_rel
            + weights.importance * norm_imp
            + weights.hook_boost * norm_hb
            + weights.need_sim * norm_ns;

        hit.fused_score = final_score;

        // 写入分数到 metadata
        let mut meta = hit.item.metadata.clone();
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        if let Some(obj) = meta.as_object_mut() {
            obj.insert("bm25_score".to_string(), serde_json::json!(hit.bm25_score));
            obj.insert("vector_similarity".to_string(), serde_json::json!(hit.vector_score));
            obj.insert("fused_score".to_string(), serde_json::json!(final_score));
            obj.insert(
                "recency_score".to_string(),
                serde_json::json!(raw_rec),
            );
            obj.insert(
                "relevance_score".to_string(),
                serde_json::json!(raw_rel),
            );
            obj.insert(
                "importance_score".to_string(),
                serde_json::json!(raw_imp),
            );
            obj.insert(
                "hook_boost_score".to_string(),
                serde_json::json!(raw_hb),
            );
            obj.insert(
                "need_sim_score".to_string(),
                serde_json::json!(raw_ns),
            );
            obj.insert(
                "norm_recency".to_string(),
                serde_json::json!(norm_rec),
            );
            obj.insert(
                "norm_relevance".to_string(),
                serde_json::json!(norm_rel),
            );
            obj.insert(
                "norm_importance".to_string(),
                serde_json::json!(norm_imp),
            );
            obj.insert(
                "norm_hook_boost".to_string(),
                serde_json::json!(norm_hb),
            );
            obj.insert(
                "norm_need_sim".to_string(),
                serde_json::json!(norm_ns),
            );
            obj.insert("semantic_boost".to_string(), serde_json::json!(semantic_type_boost(&hit.item.semantic_type())));
            obj.insert("knowledge_source_boost".to_string(), serde_json::json!(knowledge_source_boost(&hit.item.metadata)));
            // n 用于诊断：归一化样本数过少时分数意义有限
            obj.insert("normalize_n".to_string(), serde_json::json!(n));
        }
        hit.item.metadata = meta;
    }

    hits.retain(|h| !h.item.content.is_empty());

    // min_score 过滤：低于阈值的命中视为噪声，避免污染 prompt 上下文。
    // 仅当阈值 > 0 时启用（设为 0 等价于不启用），过滤发生在归一化重排之后，
    // 因此阈值是相对值（0.15 对应"最低保留 15% 的归一化综合分"）。
    if weights.min_score > 0.0 {
        let before = hits.len();
        hits.retain(|h| h.fused_score >= weights.min_score);
        if before != hits.len() {
            tracing::debug!(
                "[Retriever] min_score={:.3} 过滤掉 {} 条噪声命中（{} → {}）",
                weights.min_score,
                before - hits.len(),
                before,
                hits.len()
            );
        }
    }

    // 重新按 final fused_score 排序
    hits.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits
}

/// 计算 query token 集合与文本的 Jaccard 相似度
///
/// `query_tokens`：已分词的查询 token 集合
/// `content`：记忆内容文本（函数内部用 jieba 分词）
/// 返回 |A ∩ B| / |A ∪ B|，二者均为空时返回 0.0
fn jaccard_similarity(
    query_tokens: &std::collections::HashSet<String>,
    content: &str,
) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let content_tokens_vec = tokenize(content);
    if content_tokens_vec.is_empty() {
        return 0.0;
    }
    let content_tokens: std::collections::HashSet<&str> =
        content_tokens_vec.iter().map(|s| s.as_str()).collect();
    let qset: std::collections::HashSet<&str> =
        query_tokens.iter().map(|s| s.as_str()).collect();
    let inter = qset.intersection(&content_tokens).count() as f64;
    let union = qset.union(&content_tokens).count() as f64;
    if union < 1e-9 {
        0.0
    } else {
        inter / union
    }
}

/// 计算一组数值的 (min, max)
fn min_max<I>(iter: I) -> (f64, f64)
where
    I: Iterator<Item = f64>,
{
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in iter {
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
    }
    if !min.is_finite() {
        min = 0.0;
    }
    if !max.is_finite() {
        max = 0.0;
    }
    (min, max)
}

/// 上下文扩窗：对命中的 raw 对话按 seq_no ±1 取邻接记忆。
/// 当检索命中一条 raw 对话（granularity=turn，有 seq_no）时，
/// 该对话可能被截断或缺少上下文。本函数从全量记忆中找到 seq_no 相邻的记忆，
/// 追加到结果列表末尾（标记为 expansion）。
///
/// - `hits`：检索命中的记忆列表
/// - `all_entries`：全量记忆（用于查找邻接 seq_no）
/// - `max_expansion`：每条命中最多扩窗的邻接记忆数（上下各 1 条 = 2 条）
///
/// 返回扩窗后的记忆列表（命中在前，扩窗在后，去重）。
pub fn expand_context(
    hits: Vec<MemoryItem>,
    all_entries: &[MemoryItem],
    max_expansion: usize,
) -> Vec<MemoryItem> {
    if hits.is_empty() || max_expansion == 0 {
        return hits;
    }

    // 收集命中记忆的 seq_no
    let hit_seq_nos: Vec<(String, u64)> = hits
        .iter()
        .filter_map(|m| m.seq_no().map(|n| (m.id.clone(), n)))
        .collect();

    if hit_seq_nos.is_empty() {
        return hits; // 无 seq_no 的命中无需扩窗
    }

    // 构建 seq_no → MemoryItem 索引映射（仅 raw 层级）
    let seq_index: HashMap<u64, &MemoryItem> = all_entries
        .iter()
        .filter(|m| m.source_layer() == "raw")
        .filter_map(|m| m.seq_no().map(|n| (n, m)))
        .collect();

    // 已有的 id 集合（去重）
    let mut existing_ids: std::collections::HashSet<String> =
        hits.iter().map(|m| m.id.clone()).collect();

    let mut expanded = hits;
    let mut expansion_count = 0;

    for (_, seq_no) in &hit_seq_nos {
        if expansion_count >= max_expansion {
            break;
        }
        // 查找 seq_no - 1 和 seq_no + 1
        for delta in [1i64, -1] {
            if expansion_count >= max_expansion {
                break;
            }
            let target_seq = (*seq_no as i64 + delta) as u64;
            if let Some(&neighbor) = seq_index.get(&target_seq) {
                if !existing_ids.contains(&neighbor.id) {
                    let mut clone = neighbor.clone();
                    if let Some(obj) = clone.metadata.as_object_mut() {
                        obj.insert("expansion".to_string(), serde_json::json!(true));
                    }
                    existing_ids.insert(clone.id.clone());
                    expanded.push(clone);
                    expansion_count += 1;
                }
            }
        }
    }

    if expansion_count > 0 {
        tracing::debug!(
            "[ContextExpansion] 扩窗 {} 条邻接记忆（上限 {}）",
            expansion_count,
            max_expansion
        );
    }

    expanded
}

/// 语义去重：对候选结果做聚类（cosine ≥ threshold），每簇只保留得分最高的一条
///
/// 解决"语义相同但表述不同的记忆同时返回挤占 token"问题（如"用户喜欢晚上看书" vs
/// "用户偏好夜间阅读"）。字面去重（dedup_by_content）无法识别这种语义重复。
///
/// 簇内保留优先级：evidence_score + importance（证据强、重要性高的记忆优先保留）。
/// 跨簇保留，返回去重后的列表（顺序保持原排序）。
pub fn dedup_by_semantic(
    items: Vec<MemoryItem>,
    embedding: &dyn crate::memory::MemoryEmbeddingProvider,
    threshold: f64,
) -> Vec<MemoryItem> {
    use super::types::current_timestamp;

    if items.len() <= 1 {
        return items;
    }

    let now = current_timestamp();

    // 为每条记忆计算 embedding
    let embeddings: Vec<Option<Vec<f32>>> = items
        .iter()
        .map(|m| embedding.embed(&m.content).ok())
        .collect();

    // 若任一 embedding 失败，降级为字面去重（保守策略，不丢数据）
    if embeddings.iter().any(|e| e.is_none()) {
        return super::manager::dedup_by_content(items);
    }

    let n = items.len();
    // Union-Find 聚类
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut Vec<usize>, x: usize) -> usize {
        if parent[x] != x {
            let root = find(parent, parent[x]);
            parent[x] = root;
            root
        } else {
            x
        }
    }

    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_sim(
                embeddings[i].as_ref().unwrap(),
                embeddings[j].as_ref().unwrap(),
            );
            if sim >= threshold {
                let ri = find(&mut parent, i);
                let rj = find(&mut parent, j);
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // 每簇保留得分最高的一条
    let mut cluster_best: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        let score_i = items[i].importance + super::evidence::evidence_score(&items[i], now);
        let should_replace = match cluster_best.get(&root) {
            Some(&best_idx) => {
                let score_best =
                    items[best_idx].importance + super::evidence::evidence_score(&items[best_idx], now);
                score_i > score_best
            }
            None => true,
        };
        if should_replace {
            cluster_best.insert(root, i);
        }
    }

    let keep_indices: std::collections::HashSet<usize> =
        cluster_best.values().copied().collect();

    items
        .into_iter()
        .enumerate()
        .filter_map(|(i, m)| if keep_indices.contains(&i) { Some(m) } else { None })
        .collect()
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| (*x as f64) * (*y as f64)).sum();
    let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// 后验图补位四维打分：novelty / complementarity / specificity / gap_fill
#[derive(Debug, Clone, Default)]
pub struct PostGraphScore {
    pub novelty: f64,
    pub complementarity: f64,
    pub specificity: f64,
    pub gap_fill: f64,
    pub total: f64,
}

impl PostGraphScore {
    pub fn total(&self) -> f64 {
        self.novelty * 0.25 + self.complementarity * 0.30 + self.specificity * 0.20 + self.gap_fill * 0.25
    }
}

/// 对单条候选记忆计算四维补位分数
///
/// - novelty：与已命中结果的差异度（基于 content token 重叠）
/// - complementarity：与已命中结果的语义互补（不同 tags/episode）
/// - specificity：细节丰富度（content 长度 + metadata 字段数）
/// - gap_fill：覆盖 query 中未被命中结果覆盖的词项
pub fn post_graph_score(
    candidate: &MemoryItem,
    query_tokens: &[String],
    hits: &[&MemoryItem],
) -> PostGraphScore {
    let cand_tokens: std::collections::HashSet<String> = tokenize(&candidate.content)
        .into_iter()
        .collect();

    // novelty：与所有命中结果的最小 token 重叠率（越低越新颖）
    let novelty = if hits.is_empty() {
        1.0
    } else {
        let mut min_overlap = 1.0;
        for h in hits {
            let h_tokens: std::collections::HashSet<String> = tokenize(&h.content).into_iter().collect();
            let inter = cand_tokens.intersection(&h_tokens).count();
            let union = cand_tokens.union(&h_tokens).count().max(1);
            let overlap = inter as f64 / union as f64;
            if overlap < min_overlap {
                min_overlap = overlap;
            }
        }
        1.0 - min_overlap
    };

    // complementarity：与命中结果 tags/episode 的差异度
    let cand_tags: std::collections::HashSet<&String> = candidate.tags.iter().collect();
    let complementarity = if hits.is_empty() {
        1.0
    } else {
        let mut diff_sum = 0.0;
        for h in hits {
            let h_tags: std::collections::HashSet<&String> = h.tags.iter().collect();
            let common = cand_tags.intersection(&h_tags).count();
            let total = cand_tags.union(&h_tags).count().max(1);
            diff_sum += 1.0 - (common as f64 / total as f64);
            // episode 不同则加分
            if candidate.episode_id != h.episode_id {
                diff_sum += 0.1;
            }
        }
        (diff_sum / hits.len() as f64).clamp(0.0, 1.0)
    };

    // specificity：content 长度（对数阻尼）+ metadata 字段数
    let content_len = candidate.content.chars().count() as f64;
    let specificity_len = (1.0 + content_len / 50.0).ln() / 3.0;
    let metadata_fields = candidate
        .metadata
        .as_object()
        .map(|o| o.len())
        .unwrap_or(0);
    let specificity = (specificity_len + (metadata_fields as f64 * 0.05)).clamp(0.0, 1.0);

    // gap_fill：覆盖 query 中未被命中结果覆盖的词项比例
    let gap_fill = if query_tokens.is_empty() {
        0.0
    } else {
        let covered_by_hits: std::collections::HashSet<String> = hits
            .iter()
            .flat_map(|h| tokenize(&h.content))
            .collect();
        let uncovered = query_tokens
            .iter()
            .filter(|t| !covered_by_hits.contains(*t) && cand_tokens.contains(*t))
            .count();
        uncovered as f64 / query_tokens.len() as f64
    };

    let total = novelty * 0.25 + complementarity * 0.30 + specificity * 0.20 + gap_fill * 0.25;
    PostGraphScore {
        novelty,
        complementarity,
        specificity,
        gap_fill,
        total,
    }
}

/// 后验图补位：从候选池中挑选能补全现有命中结果空缺的记忆
///
/// 返回值：补位记忆列表（按 total 降序），数量不超过 `target_n`
pub fn post_graph_compensation(
    candidates: &[MemoryItem],
    query: &str,
    hits: &[&MemoryItem],
    target_n: usize,
    min_score: f64,
) -> Vec<(MemoryItem, PostGraphScore)> {
    if candidates.is_empty() || target_n == 0 {
        return Vec::new();
    }
    let query_tokens: Vec<String> = tokenize(query)
        .into_iter()
        .filter(|t| t.len() > 1)
        .collect();
    let mut scored: Vec<(MemoryItem, PostGraphScore)> = candidates
        .iter()
        .map(|c| {
            let score = post_graph_score(c, &query_tokens, hits);
            (c.clone(), score)
        })
        .filter(|(_, s)| s.total >= min_score)
        .collect();
    scored.sort_by(|a, b| b.1.total.partial_cmp(&a.1.total).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(target_n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(id: &str, content: &str, importance: f64) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            content: content.to_string(),
            granularity: "turn".to_string(),
            importance,
            timestamp: current_timestamp(),
            embedding: None,
            tags: Vec::new(),
            metadata: serde_json::json!({}),
            related_ids: Vec::new(),
            description: None,
            visit_count: 0,
            last_visit_at: 0.0,
            heat_score: 0.0,
            open_hooks: Vec::new(),
            reinforcement: 0.0,
            disputation: 0.0,
            rein_last_signal_at: 0.0,
            disp_last_signal_at: 0.0,
            sub_zero_days: 0,
            sub_zero_last_increment_date: String::new(),
            user_fact_reinforce_count: 0,
            protected: false,
            episode_id: None,
            consolidated: false,
            memory_type: String::new(),
            rebuttal_grace_remaining: 0,
        }
    }

    #[test]
    fn bm25_returns_relevant() {
        let entries = vec![
            make_item("m1", "我喜欢吃苹果", 0.5),
            make_item("m2", "今天天气很好", 0.5),
            make_item("m3", "苹果公司发布了新产品", 0.5),
        ];
        let hits = bm25_search(&entries, "苹果", 5);
        assert!(!hits.is_empty());
        // 至少包含 m1 或 m3
        let ids: Vec<_> = hits.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"m1") || ids.contains(&"m3"));
    }

    #[test]
    fn bm25_empty_query_returns_empty() {
        let entries = vec![make_item("m1", "test", 0.5)];
        let hits = bm25_search(&entries, "", 5);
        assert!(hits.is_empty());
    }

    #[test]
    fn rrf_fuses_two_lists() {
        let bm25 = vec![("m1".to_string(), 1.5), ("m2".to_string(), 0.8)];
        let vec_ = vec![("m2".to_string(), 0.9), ("m3".to_string(), 0.6)];
        let graph = vec![];
        let entity = vec![];
        let fused = rrf_fuse(bm25, vec_, graph, entity);
        assert_eq!(fused.len(), 3);
        // m2 在两路都出现，应排第一
        assert_eq!(fused[0].0, "m2");
    }

    #[test]
    fn attach_items_fills_content() {
        let entries = vec![make_item("m1", "hello", 0.5)];
        let hits = vec![RetrievalHit {
            item: make_item("m1", "", 0.0),
            bm25_score: 1.0,
            vector_score: 0.0,
            graph_score: 0.0,
            fused_score: 0.5,
            result_type: RetrievalResultType::Memory,
        }];
        let attached = attach_items(hits, &entries, "hello");
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].item.content, "hello");
    }

    fn make_item_with_semantic(id: &str, content: &str, importance: f64, semantic: &str) -> MemoryItem {
        let mut item = make_item(id, content, importance);
        item.metadata = serde_json::json!({"semantic_type": semantic});
        item
    }

    #[test]
    fn semantic_boost_prioritizes_user_type_over_general() {
        let entries = vec![
            make_item_with_semantic("m_general", "苹果好吃", 0.5, "general"),
            make_item_with_semantic("m_user", "我喜欢苹果", 0.5, "user"),
        ];
        // 两条记忆 fused_score 相同
        let hits = vec![
            RetrievalHit {
                item: make_item("m_general", "", 0.0),
                bm25_score: 1.0, vector_score: 0.0, graph_score: 0.0, fused_score: 0.5,
                result_type: RetrievalResultType::Memory,
            },
            RetrievalHit {
                item: make_item("m_user", "", 0.0),
                bm25_score: 1.0, vector_score: 0.0, graph_score: 0.0, fused_score: 0.5,
                result_type: RetrievalResultType::Memory,
            },
        ];
        let attached = attach_items(hits, &entries, "苹果");
        // user 类型 boost=1.15 > general 类型 boost=0.95，应排第一
        assert_eq!(attached[0].item.id, "m_user");
        assert_eq!(attached[1].item.id, "m_general");
    }

    #[test]
    fn semantic_boost_written_to_metadata() {
        let entries = vec![make_item_with_semantic("m1", "hello", 0.5, "user")];
        let hits = vec![RetrievalHit {
            item: make_item("m1", "", 0.0),
            bm25_score: 1.0, vector_score: 0.0, graph_score: 0.0, fused_score: 0.5,
            result_type: RetrievalResultType::Memory,
        }];
        let attached = attach_items(hits, &entries, "hello");
        assert_eq!(attached.len(), 1);
        let boost = attached[0].item.metadata.get("semantic_boost")
            .and_then(|v| v.as_f64())
            .expect("semantic_boost 应写入 metadata");
        assert!((boost - 1.15).abs() < 1e-6, "user 类型 boost 应为 1.15");
    }

    #[test]
    fn bm25_cache_reflects_content_update() {
        // 同一 memory_id，内容从"苹果"改为"香蕉"后，检索"香蕉"仍应命中（指纹失效触发重算）
        let a = make_item("m1", "我喜欢吃苹果", 0.5);
        let _ = bm25_search(&[a], "苹果", 5); // 首次调用填充缓存（内容A）
        let b = make_item("m1", "我喜欢吃香蕉", 0.5);
        let hits = bm25_search(&[b], "香蕉", 5); // 内容B 指纹不同，应重算而非命中过期缓存
        assert!(!hits.is_empty(), "内容变更后应能命中");
        assert_eq!(hits[0].0, "m1");
    }

    #[test]
    fn bm25_cache_stable_across_calls() {
        let entries = vec![
            make_item("m1", "我喜欢吃苹果", 0.5),
            make_item("m2", "今天天气很好", 0.5),
        ];
        let r1 = bm25_search(&entries, "苹果", 5);
        let r2 = bm25_search(&entries, "苹果", 5); // 第二次命中缓存
        assert!(!r1.is_empty());
        assert_eq!(r1, r2);
    }
}
