use crate::config::manager::AppConfig;
use crate::cross_character::{build_speaker_prefix, parse_any_speaker_prefix};
use crate::error::VivianResult;
use crate::utils::path;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use super::age::{bump_visit, init_heat, staleness_text};
use super::conflict::{
    build_score_input, resolve_action, score_conflict, simple_merge, ConflictAction,
};
use super::embedding::{build_embedding, MemoryEmbeddingProvider};
use super::episode::EpisodeStore;
use super::graph_store::KnowledgeGraph;
use super::llm_enricher::{EnrichedMeta, MemoryEnricher};
use super::redact::{redact_content, RedactStatus};
use super::recycle_bin::RecycleBin;
use super::strategy::{create_strategy, RetrievalContext};
use super::takes_fence::TakesFence;
use super::types::{
    current_timestamp, Granularity, MemoryItem, MemoryStoreData, MemoryType, OpenHook,
    RetrievalStrategy, SemanticType,
};
use super::vector_search::{should_index, MemoryVector, MemoryVectorStore, INDEX_IMPORTANCE_THRESHOLD};
use crate::memory::retriever::RetrievalWeights;

/// 单次检索命中时 importance 增量（反馈机制：被检索命中=被验证有用）
const VISIT_IMPORTANCE_DELTA: f64 = 0.05;

/// 检索缓存 TTL（5 秒），避免同轮内重复检索相同 query
const SEARCH_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

const DEFAULT_CAPACITIES: [(Granularity, usize); 4] = [
    // 单轮对话保留 50 条，平衡上下文长度和 token 预算
    (Granularity::Turn, 50),
    // 单会话保留 30 条摘要，覆盖典型工作日长度
    (Granularity::Session, 30),
    // 跨会话摘要保留 200 条，覆盖约 1-2 个月活跃使用
    (Granularity::Summary, 200),
    // 关键词索引保留 500 条，覆盖主要话题维度
    (Granularity::Keyword, 500),
];

#[derive(Clone)]
pub struct MemoryManager {
    inner: Arc<RwLock<MemoryManagerInner>>,
    /// 写入时 LLM 增强器（可选；None 时退化为规则化标签推断）
    enricher: Arc<RwLock<Option<Arc<MemoryEnricher>>>>,
    /// 单轮检索缓存（TTL 5s，避免同轮内重复检索相同 query）
    search_cache: Arc<Mutex<HashMap<String, (std::time::Instant, Vec<MemoryItem>)>>>,
    /// 知识图谱（实体+typed edges，作为 RRF 第三路检索）
    knowledge_graph: Arc<KnowledgeGraph>,
    /// Takes 围栏表格（append-only + supersede 事实表）
    takes_fence: Arc<TakesFence>,
    /// Episode 经历封包索引（可选；构造后通过 set_episode_store 注入）
    episode_store: Arc<Mutex<Option<Arc<EpisodeStore>>>>,
    /// 角色 ID（用于 emit 事件时标注来源角色）
    char_id: String,
    /// AppHandle（运行时注入）：记忆写入/删除/清空后 emit `memory:updated` 通知前端刷新
    /// 用 Arc 包裹使 MemoryManager 仍可 derive Clone（Mutex 本身不实现 Clone）
    app_handle: Arc<Mutex<Option<AppHandle>>>,
    /// 当前会话 ID（运行时由 Brain 在 think 前设置），写入记忆时自动注入 metadata.session_id
    current_session_id: Arc<Mutex<Option<String>>>,
    /// 软删除回收站：保留 7 天可恢复窗口
    recycle_bin: Arc<RecycleBin>,
    /// 对话中 web_search 工具留下的主题提示（供后台知识采集优先消费）
    topic_hints: Arc<Mutex<Vec<TopicHint>>>,
    /// 待 LLM 仲裁的冲突队列（QueueLlm 推入，后台 tick 消费）
    pending_conflicts: Arc<Mutex<Vec<super::conflict::PendingConflict>>>,
}

/// 主题提示：对话中用户搜索过的关键词，供后台知识采集优先处理
pub struct TopicHint {
    pub query: String,
    pub timestamp: f64,
}

struct MemoryManagerInner {
    store_path: PathBuf,
    data: MemoryStoreData,
    id_index: HashMap<String, usize>,
    capacities: HashMap<Granularity, usize>,
    /// 向量存储（用于 RetrievalStrategy::Vector）
    vector_store: MemoryVectorStore,
    /// 嵌入服务（默认 HashingMemoryEmbedding，256 维）
    embedding: Arc<dyn MemoryEmbeddingProvider>,
    /// 检索三因子加权配置（运行时可热更新）
    retrieval_weights: RetrievalWeights,
    /// 脏标志：有未落盘的写入时为 true
    dirty: bool,
    /// 上次落盘时间戳（用于 5s 节流判断）
    last_save_at: Option<std::time::Instant>,
}

/// 冲突检测结果（内部使用，携带执行所需的数据）
enum ConflictOutcome {
    KeepBoth,
    ReplaceOld { old_id: String },
    MergeSupersede {
        old_id: String,
        old_content: String,
    },
    /// 排队等待后台 LLM 仲裁（携带冲突双方向量相似度，供仲裁使用）
    QueueLlm {
        old_id: String,
        old_content: String,
        similarity: f64,
    },
}

/// 判断记忆条目是否应建立向量索引（重建与计数共用）
fn is_indexable_entry(e: &MemoryItem) -> bool {
    if e.content.trim().is_empty() {
        return false;
    }
    match MemoryType::from_str(&e.memory_type) {
        Some(t) => should_index(e.importance, &t),
        None => e.importance >= INDEX_IMPORTANCE_THRESHOLD,
    }
}

impl MemoryManager {
    pub fn new(config: &AppConfig, char_id: &str) -> VivianResult<Self> {
        let memory_dir = path::get_character_data_dir(char_id).join("memory");
        path::ensure_dir(&memory_dir)?;

        let store_path = memory_dir.join("unified_memory.json");

        let mut capacities = HashMap::new();
        for (g, c) in DEFAULT_CAPACITIES.iter() {
            capacities.insert(*g, *c);
        }
        let max_short = config.memory.max_short_term_memory.max(5);
        capacities.insert(Granularity::Turn, max_short);

        // 嵌入服务：根据 config.routing_matrix.memory 配置选择远程或哈希
        let embedding = build_embedding(config);
        let vector_store_path = memory_dir.join("vectors.db");
        let vector_store = MemoryVectorStore::load_from(
            &vector_store_path,
            embedding.dimension(),
            embedding.model_id(),
        )?;

        let mut inner = MemoryManagerInner {
            store_path,
            data: MemoryStoreData::default(),
            id_index: HashMap::new(),
            capacities,
            vector_store,
            embedding,
            retrieval_weights: RetrievalWeights::from_config(&config.memory.retrieval_weights),
            dirty: false,
            last_save_at: None,
        };

        inner.load_from_disk()?;
        inner.seed_if_empty(char_id);
        inner.save_to_disk()?;

        let graph_path = memory_dir.join("knowledge_graph.json");
        let knowledge_graph = Arc::new(KnowledgeGraph::new(graph_path));
        let takes_path = memory_dir.join("takes_fence.json");
        let takes_fence = Arc::new(TakesFence::new(takes_path));
        let recycle_bin_path = memory_dir.join("recycle_bin.json");
        let recycle_bin = Arc::new(RecycleBin::new(recycle_bin_path));

        // 加载持久化的 pending conflicts 队列
        let pending_conflicts_path = memory_dir.join("pending_conflicts.json");
        let pending_conflicts = match std::fs::read_to_string(&pending_conflicts_path) {
            Ok(s) => serde_json::from_str::<Vec<super::conflict::PendingConflict>>(&s)
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
            enricher: Arc::new(RwLock::new(None)),
            search_cache: Arc::new(Mutex::new(HashMap::new())),
            knowledge_graph,
            takes_fence,
            episode_store: Arc::new(Mutex::new(None)),
            char_id: char_id.to_string(),
            app_handle: Arc::new(Mutex::new(None)),
            current_session_id: Arc::new(Mutex::new(None)),
            recycle_bin,
            topic_hints: Arc::new(Mutex::new(Vec::new())),
            pending_conflicts: Arc::new(Mutex::new(pending_conflicts)),
        })
    }

    /// 注入写入时 LLM 增强器（构造后可通过 `&self` 设置）
    pub fn set_enricher(&self, enricher: Arc<MemoryEnricher>) {
        *self.enricher.write() = Some(enricher);
    }

    /// 注入 AppHandle，启用记忆写入/删除/清空后的 `memory:updated` 事件通知
    pub fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.lock() = Some(handle);
    }

    /// 设置当前会话 ID（Brain 在 think 前调用），后续写入的记忆自动携带 session_id
    pub fn set_session_id(&self, session_id: Option<String>) {
        *self.current_session_id.lock() = session_id;
    }

    /// 获取当前会话 ID
    pub fn get_session_id(&self) -> Option<String> {
        self.current_session_id.lock().clone()
    }

    /// 获取当前 MemoryManager 所属角色 ID
    pub fn char_id(&self) -> &str {
        &self.char_id
    }

    /// 记录对话中 web_search 的搜索关键词，供后台知识采集优先消费
    pub fn push_topic_hint(&self, query: &str) {
        let q = query.trim();
        if q.is_empty() {
            return;
        }
        let mut hints = self.topic_hints.lock();
        // 去重：已存在相同关键词则更新时间戳，不重复添加
        if let Some(existing) = hints.iter_mut().find(|h| h.query == q) {
            existing.timestamp = current_timestamp();
        } else {
            hints.push(TopicHint {
                query: q.to_string(),
                timestamp: current_timestamp(),
            });
        }
        // 保留最近 20 条，防止无限增长
        if hints.len() > 20 {
            let drop_count = hints.len() - 20;
            hints.drain(0..drop_count);
        }
    }

    /// 取出并清空所有主题提示（后台知识采集任务调用）
    /// 自动过滤超过 24 小时的过期提示
    pub fn drain_topic_hints(&self) -> Vec<String> {
        let cutoff = current_timestamp() - 86400.0; // 24h
        let mut hints = self.topic_hints.lock();
        let drained: Vec<String> = hints
            .drain(..)
            .filter(|h| h.timestamp >= cutoff)
            .map(|h| h.query)
            .collect();
        drained
    }

    /// 持久化 pending conflicts 队列到磁盘（fire-and-forget）
    fn save_pending_conflicts(&self) -> VivianResult<()> {
        let dir = path::get_character_data_dir(&self.char_id).join("memory");
        let path = dir.join("pending_conflicts.json");
        let queue = self.pending_conflicts.lock();
        let s = serde_json::to_string_pretty(&*queue)?;
        drop(queue);
        std::fs::write(&path, s)?;
        Ok(())
    }

    /// 当前 pending conflict 队列长度（供后台 tick 决定是否触发消费）
    pub fn pending_conflict_count(&self) -> usize {
        self.pending_conflicts.lock().len()
    }

    /// 消费 pending conflicts：调用 LLM 仲裁，按结果执行 ReplaceOld/MergeSupersede/KeepBoth
    ///
    /// 由后台 tick（如 mind_tick / consolidation tick）调用。每次最多消费 5 条，
    /// 避免 LLM 调用过多阻塞 tick。仲裁失败的条目 retry_count++，超过 3 次丢弃。
    pub async fn process_pending_conflicts(
        &self,
        llm: &Arc<dyn super::conflict::ConflictLlmArbiter>,
    ) -> usize {
        const MAX_PER_TICK: usize = 5;
        const MAX_RETRY: u32 = 3;

        let batch: Vec<super::conflict::PendingConflict> = {
            let mut queue = self.pending_conflicts.lock();
            let take_n = queue.len().min(MAX_PER_TICK);
            queue.drain(..take_n).collect::<Vec<_>>()
        };

        if batch.is_empty() {
            return 0;
        }

        let mut processed = 0usize;
        let mut requeue: Vec<super::conflict::PendingConflict> = Vec::new();
        let now = current_timestamp();

        for mut pc in batch {
            // 检查双方记忆是否仍然存在（可能已被其他流程删除）
            let (new_exists, old_exists) = {
                let inner = self.inner.read();
                let new_exists = inner
                    .id_index
                    .get(&pc.new_memory_id)
                    .and_then(|&i| inner.data.entries.get(i))
                    .is_some();
                let old_exists = inner
                    .id_index
                    .get(&pc.old_memory_id)
                    .and_then(|&i| inner.data.entries.get(i))
                    .is_some();
                (new_exists, old_exists)
            };

            // 旧记忆已不存在 → 无需仲裁
            if !old_exists {
                processed += 1;
                continue;
            }
            // 新记忆已不存在 → 重新入队（罕见，可能被 LLM 富化失败删除）
            if !new_exists {
                pc.retry_count += 1;
                if pc.retry_count < MAX_RETRY {
                    requeue.push(pc);
                }
                continue;
            }

            match llm.arbitrate(&pc.new_content, &pc.old_content, pc.similarity).await {
                Ok(super::conflict::ArbitrationOutcome::KeepBoth) => {
                    tracing::info!(
                        "[ConflictArbiter] KeepBoth: new={} old={}",
                        pc.new_memory_id,
                        pc.old_memory_id
                    );
                    processed += 1;
                }
                Ok(super::conflict::ArbitrationOutcome::ReplaceOld) => {
                    let mut inner = self.inner.write();
                    let _ = inner.remove_by_id(&pc.old_memory_id);
                    if inner.vector_store.delete_by_memory_id(&pc.old_memory_id) {
                        let _ = inner.vector_store.save_to();
                    }
                    drop(inner);
                    tracing::info!(
                        "[ConflictArbiter] ReplaceOld: 删除旧记忆 {}（新记忆 {} 保留）",
                        pc.old_memory_id,
                        pc.new_memory_id
                    );
                    processed += 1;
                }
                Ok(super::conflict::ArbitrationOutcome::MergeSupersede(merged)) => {
                    let mut inner = self.inner.write();
                    if let Some(&idx) = inner.id_index.get(&pc.new_memory_id) {
                        if let Some(item) = inner.data.entries.get_mut(idx) {
                            item.content = merged.clone();
                            item.timestamp = now;
                        }
                    }
                    let _ = inner.remove_by_id(&pc.old_memory_id);
                    if inner.vector_store.delete_by_memory_id(&pc.old_memory_id) {
                        let _ = inner.vector_store.save_to();
                    }
                    drop(inner);
                    tracing::info!(
                        "[ConflictArbiter] MergeSupersede: 合并旧记忆 {} 到新记忆 {}",
                        pc.old_memory_id,
                        pc.new_memory_id
                    );
                    processed += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        "[ConflictArbiter] LLM 仲裁失败（retry {}/{}）: {}",
                        pc.retry_count + 1,
                        MAX_RETRY,
                        e
                    );
                    pc.retry_count += 1;
                    if pc.retry_count < MAX_RETRY {
                        requeue.push(pc);
                    }
                }
            }
        }

        // 失败的条目重新入队
        if !requeue.is_empty() {
            let mut queue = self.pending_conflicts.lock();
            for pc in requeue {
                if queue.len() < 50 {
                    queue.push(pc);
                }
            }
        }

        let _ = self.save_pending_conflicts();
        processed
    }

    /// 记忆变更后通知前端刷新（仅当 AppHandle 已注入时生效）
    fn emit_memory_updated(&self) {
        if let Some(handle) = self.app_handle.lock().as_ref() {
            let _ = handle.emit(
                "memory:updated",
                serde_json::json!({ "character_id": self.char_id }),
            );
        }
    }

    /// 暴露知识图谱引用（供检索策略和外部命令使用）
    pub fn knowledge_graph(&self) -> &Arc<KnowledgeGraph> {
        &self.knowledge_graph
    }

    /// 暴露 Takes 围栏表格引用（供事实查询和外部命令使用）
    pub fn takes_fence(&self) -> &Arc<TakesFence> {
        &self.takes_fence
    }

    /// 暴露 Episode 经历封包索引（供检索和 Pipeline 使用）
    pub fn episode_store(&self) -> Option<Arc<EpisodeStore>> {
        self.episode_store.lock().clone()
    }

    /// 注入 Episode 经历封包索引（构造后调用，通常在 Brain::build 中）
    pub fn set_episode_store(&self, store: Arc<EpisodeStore>) {
        *self.episode_store.lock() = Some(store);
    }

    /// 批量设置记忆的 episode_id（封包后回填用）。
    ///
    /// 对每条指定 ID 的记忆，若当前 episode_id 为 None，则设为指定值。
    /// 已有 episode_id 的记忆不会被覆盖（避免后续封包覆盖早期归属）。
    pub fn backfill_episode_id(&self, memory_ids: &[String], episode_id: &str) -> VivianResult<usize> {
        let mut inner = self.inner.write();
        let mut updated = 0usize;
        for mid in memory_ids {
            if let Some(&idx) = inner.id_index.get(mid.as_str()) {
                let entry = &mut inner.data.entries[idx];
                if entry.episode_id.is_none() {
                    entry.episode_id = Some(episode_id.to_string());
                    updated += 1;
                }
            }
        }
        if updated > 0 {
            inner.dirty = true;
            let _ = inner.save_to_disk();
            tracing::debug!(
                "[MemoryManager] backfill_episode_id: {} 条记忆 → {}",
                updated,
                episode_id
            );
        }
        Ok(updated)
    }

    /// 暴露嵌入服务（供 ConsolidationPipeline 等模块复用，避免重复构造）
    ///
    /// 用于 Stage 1 主题连续性检测：计算新摘要 embedding 与近期 SessionSummary embedding
    /// 的余弦相似度，决定合并还是新建。
    pub fn embedding(&self) -> Arc<dyn MemoryEmbeddingProvider> {
        let inner = self.inner.read();
        inner.embedding.clone()
    }

    /// 统计应建立向量索引的记忆数（供重建进度计算 total）
    pub fn count_indexable(&self) -> usize {
        let inner = self.inner.read();
        inner
            .data
            .entries
            .iter()
            .filter(|e| is_indexable_entry(e))
            .count()
    }

    /// 重建全部记忆的向量索引（切换嵌入模型后全量重新嵌入），返回重建的向量数。
    ///
    /// 每成功嵌入并写入一条调用一次 `on_embedded`（用于进度上报）。
    /// 不调 clear()：切换模型后旧向量表已在 reinitialize 时 DROP，`add()` 为
    /// INSERT OR REPLACE，对重建期间的并发写入安全（新写入记忆自带新向量）。
    pub fn rebuild_all_embeddings(&self, mut on_embedded: impl FnMut()) -> VivianResult<usize> {
        let embedding = self.embedding();
        // Step 1: 短持读锁，快照待嵌入条目 (id, content, importance, memory_type, timestamp)
        let snapshot: Vec<(String, String, f64, String, f64)> = {
            let inner = self.inner.read();
            inner
                .data
                .entries
                .iter()
                .filter(|e| is_indexable_entry(e))
                .map(|e| {
                    (
                        e.id.clone(),
                        e.content.clone(),
                        e.importance,
                        e.memory_type.clone(),
                        e.timestamp,
                    )
                })
                .collect()
        };
        // Step 2: 锁外逐条 embed（慢操作），成功后短持读锁写入向量
        let mut count = 0usize;
        for (id, content, importance, memory_type, timestamp) in snapshot {
            match embedding.embed(&content) {
                Ok(emb) => {
                    let vec = MemoryVector {
                        doc_id: id.clone(),
                        memory_id: id,
                        content,
                        embedding: emb,
                        importance,
                        memory_type,
                        timestamp,
                    };
                    {
                        let inner = self.inner.read();
                        if let Err(e) = inner.vector_store.add(vec) {
                            tracing::warn!("[RebuildEmbeddings] 向量写入失败: {e}");
                            continue;
                        }
                    }
                    count += 1;
                    on_embedded();
                }
                Err(e) => tracing::warn!("[RebuildEmbeddings] 嵌入失败: {e}"),
            }
        }
        // Step 3: WAL checkpoint 落盘
        {
            let inner = self.inner.read();
            if let Err(e) = inner.vector_store.save_to() {
                tracing::warn!("[RebuildEmbeddings] 向量持久化失败: {e}");
            }
        }
        Ok(count)
    }

    /// 检测向量索引与记忆条目的漂移，必要时触发全量重建
    ///
    /// 漂移判定（满足任一即重建）：
    /// - 索引缺失：可索引记忆数 > 0，但向量数 / 可索引记忆数 < 0.8（超过 20% 记忆缺向量）
    /// - 孤儿向量：向量数 / 可索引记忆数 > 1.2（超过 20% 向量无对应记忆）
    ///
    /// 由巩固 tick 定期调用（如每次 Rest 巩固后），避免长期增删导致索引漂移、召回率下降。
    /// 返回 Some(n) 表示已重建 n 条向量，None 表示无需重建。
    pub fn check_index_drift_and_rebuild(&self) -> Option<usize> {
        let (indexable_count, vector_count) = {
            let inner = self.inner.read();
            let indexable = inner
                .data
                .entries
                .iter()
                .filter(|e| is_indexable_entry(e))
                .count();
            (indexable, inner.vector_store.len())
        };

        if indexable_count == 0 {
            return None;
        }

        let ratio = vector_count as f64 / indexable_count as f64;
        const DRIFT_LOW: f64 = 0.8;
        const DRIFT_HIGH: f64 = 1.2;

        if ratio < DRIFT_LOW {
            tracing::warn!(
                "[IndexDrift] 检测到索引缺失：可索引记忆 {} 条，向量 {} 条（比率 {:.2} < {}），触发全量重建",
                indexable_count,
                vector_count,
                ratio,
                DRIFT_LOW
            );
            match self.rebuild_all_embeddings(|| {}) {
                Ok(n) => {
                    tracing::info!("[IndexDrift] 全量重建完成，重新嵌入 {} 条向量", n);
                    return Some(n);
                }
                Err(e) => {
                    tracing::warn!("[IndexDrift] 全量重建失败: {}", e);
                    return None;
                }
            }
        }

        if ratio > DRIFT_HIGH {
            tracing::warn!(
                "[IndexDrift] 检测到孤儿向量：可索引记忆 {} 条，向量 {} 条（比率 {:.2} > {}），触发全量重建",
                indexable_count,
                vector_count,
                ratio,
                DRIFT_HIGH
            );
            match self.rebuild_all_embeddings(|| {}) {
                Ok(n) => {
                    tracing::info!("[IndexDrift] 全量重建完成，重新嵌入 {} 条向量", n);
                    return Some(n);
                }
                Err(e) => {
                    tracing::warn!("[IndexDrift] 全量重建失败: {}", e);
                    return None;
                }
            }
        }

        None
    }

    /// 热更新检索权重（无需重启即可生效）
    pub fn set_retrieval_weights(&self, weights: RetrievalWeights) {
        let mut inner = self.inner.write();
        inner.retrieval_weights = weights;
        tracing::info!(
            "[MemoryManager] 检索权重已热更新: recency={}, relevance={}, importance={}, tau={}h",
            weights.recency, weights.relevance, weights.importance, weights.recency_tau_hours
        );
    }

    /// 批量更新记忆热度：对命中的记忆增加 visit_count、刷新 last_visit_at、重算 heat_score，
    /// 并按 `VISIT_IMPORTANCE_DELTA` 提升 importance（反馈机制：被检索命中=被验证有用）。
    ///
    /// 应在检索完成后调用，将命中的 memory_id 列表传入。
    /// 持久化失败仅记录警告，不影响主流程。
    pub fn bump_visits(&self, ids: &[String]) -> VivianResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = current_timestamp();
        let mut inner = self.inner.write();
        let delta = VISIT_IMPORTANCE_DELTA;
        let mut touched = false;
        for id in ids {
            if let Some(&idx) = inner.id_index.get(id) {
                if idx < inner.data.entries.len() && inner.data.entries[idx].id == *id {
                    let entry = &mut inner.data.entries[idx];
                    bump_visit(entry, now);
                    // 反馈机制：被检索命中 → importance 增量
                    if delta > 0.0 {
                        entry.importance = (entry.importance + delta).min(1.0);
                    }
                    touched = true;
                }
            }
        }
        if touched {
            if let Err(e) = inner.save_throttled() {
                tracing::warn!("[MemoryManager] 热度更新持久化失败: {e}");
            }
        }
        Ok(())
    }

    /// 为给定记忆列表附加陈旧度提示，返回提示文本（按 id 索引）。
    ///
    /// 调用方可在组装 memory_text 时，对超过 1 天的记忆追加提示。
    pub fn staleness_hints(&self, items: &[MemoryItem]) -> Vec<(String, Option<String>)> {
        let now = current_timestamp();
        items
            .iter()
            .map(|m| (m.id.clone(), staleness_text(m.timestamp, now)))
            .collect()
    }

    pub async fn get_all_memories(&self) -> VivianResult<Vec<MemoryItem>> {
        let inner = self.inner.read();
        Ok(inner
            .data
            .entries
            .iter()
            .filter(|m| !m.consolidated)
            .cloned()
            .collect())
    }

    /// 按 memory_type 查询最近 N 条记忆（按 timestamp 降序）
    ///
    /// 通过 tags 中的类型标识过滤（如 ImportantEvent → "important_event"）。
    /// 供 StatusPanel "近期事件"卡片和 prompt "近期重要事件"段落使用。
    pub fn recent_by_type(&self, memory_type: MemoryType, limit: usize) -> Vec<MemoryItem> {
        let tag = memory_type.as_str();
        let inner = self.inner.read();
        let mut items: Vec<MemoryItem> = inner
            .data
            .entries
            .iter()
            .filter(|m| !m.consolidated && m.tags.iter().any(|t| t == tag))
            .cloned()
            .collect();
        items.sort_by(|a, b| {
            b.timestamp.partial_cmp(&a.timestamp).unwrap_or(std::cmp::Ordering::Equal)
        });
        items.into_iter().take(limit).collect()
    }

    /// 按 tags 查询最近 N 条记忆（按 timestamp 降序）
    ///
    /// 只要记忆的 tags 包含任意一个传入的 tag 即命中。
    /// 供 inner_monologue 检索旁观记忆等场景使用。
    pub fn recent_by_tags(&self, tags: &[&str], limit: usize) -> Vec<MemoryItem> {
        let inner = self.inner.read();
        let mut items: Vec<MemoryItem> = inner
            .data
            .entries
            .iter()
            .filter(|m| !m.consolidated && m.tags.iter().any(|t| tags.contains(&t.as_str())))
            .cloned()
            .collect();
        items.sort_by(|a, b| {
            b.timestamp.partial_cmp(&a.timestamp).unwrap_or(std::cmp::Ordering::Equal)
        });
        items.into_iter().take(limit).collect()
    }

    /// 当前记忆条数（用于触发查询重写等读路径 LLM 决策）。
    pub fn entry_count(&self) -> usize {
        self.inner.read().data.entries.len()
    }

    /// 非种子记忆条数。
    /// 排除 `seed_if_empty()` 自动插入的种子记忆（id 以 "seed_" 开头），
    /// 用于判断用户是否真正交互过——初次启动时种子已植入但尚无真实记忆。
    pub fn non_seed_count(&self) -> usize {
        self.inner
            .read()
            .data
            .entries
            .iter()
            .filter(|m| !m.id.starts_with("seed_"))
            .count()
    }

    /// 图谱时间轴骨架点：仅克隆 (id, timestamp)，不含内容与 embedding。
    ///
    /// 过滤条件必须与 `memories_in_range` / `commands::memory::get_graph_timeline`
    /// 完全一致（见 `is_graph_visible_memory`），否则骨架点与范围查询结果不匹配。
    pub fn timeline_points(&self) -> Vec<(String, f64)> {
        let inner = self.inner.read();
        inner
            .data
            .entries
            .iter()
            .filter(|m| is_graph_visible_memory(m))
            .map(|m| (m.id.clone(), m.timestamp))
            .collect()
    }

    /// 时间窗口 [after, before) 内的完整记忆（Unix 秒），供图谱懒加载。
    ///
    /// 范围语义仿 `UnifiedEventLedger::events_on_date`。过滤条件与
    /// `timeline_points` 共用同一谓词，保证骨架点与内容一一对应。
    pub fn memories_in_range(&self, after: f64, before: f64) -> Vec<MemoryItem> {
        let inner = self.inner.read();
        inner
            .data
            .entries
            .iter()
            .filter(|m| {
                is_graph_visible_memory(m) && m.timestamp >= after && m.timestamp < before
            })
            .cloned()
            .collect()
    }

    pub async fn add_memory(
        &self,
        content: &str,
        memory_type: MemoryType,
        importance: f64,
        tags: Vec<String>,
    ) -> VivianResult<MemoryItem> {
        self.add_memory_inner(content, memory_type, importance, tags, None, None, None)
            .await
    }

    /// 带 metadata 的记忆写入（用于跨角色对话/旁观记忆等需要标注说话人/视角的场景）
    ///
    /// 在 `add_memory` 基础上额外写入 metadata 字段。metadata 是自由 JSON，
    /// 调用方负责填充约定字段（如 `speaker`/`listener`/`perspective`/`observer`）。
    /// 其他行为与 `add_memory` 完全一致（embedding/冲突检测/向量索引/巩固流水线）。
    pub async fn add_memory_with_metadata(
        &self,
        content: &str,
        memory_type: MemoryType,
        importance: f64,
        tags: Vec<String>,
        metadata: serde_json::Value,
    ) -> VivianResult<MemoryItem> {
        self.add_memory_inner(content, memory_type, importance, tags, None, Some(metadata), None)
            .await
    }

    /// 写入已构建好的合并记忆（用于 LLM 同主题整合流水线）
    ///
    /// 与 `add_memory` 的差异：
    /// - 跳过冲突检测与字面去重（合并产物已无冲突语义）
    /// - 保留调用方设置的 importance / tags / related_ids / metadata
    /// - 重新计算 embedding（基于合并后的 content）
    pub async fn add_merged_memory(
        &self,
        content: &str,
        memory_type: MemoryType,
        importance: f64,
        tags: Vec<String>,
        metadata: serde_json::Value,
        related_ids: Vec<String>,
        timestamp_override: Option<f64>,
    ) -> VivianResult<MemoryItem> {
        let importance = importance.clamp(0.0, 1.0);
        let granularity = memory_type.default_granularity();

        let (safe_content, pii_spans, redact_status) = redact_content(content);
        let content = safe_content.as_str();

        let mut item = MemoryItem::new(content.to_string(), granularity, importance);
        if let Some(ts) = timestamp_override {
            item.timestamp = ts;
        }
        item.memory_type = memory_type.as_str().to_string();
        item.tags = if tags.is_empty() {
            infer_tags_for_content(content, &memory_type)
        } else {
            tags
        };
        item.metadata = metadata;
        item.related_ids = related_ids;
        init_heat(&mut item);

        if let Some(sid) = self.get_session_id() {
            if let Some(obj) = item.metadata.as_object_mut() {
                obj.entry("session_id")
                    .or_insert(serde_json::Value::String(sid));
            }
        }
        if redact_status != RedactStatus::Clean {
            if let Some(obj) = item.metadata.as_object_mut() {
                obj.entry("redact_status")
                    .or_insert(serde_json::Value::String(redact_status.as_str().to_string()));
                if !pii_spans.is_empty() {
                    obj.entry("pii_spans")
                        .or_insert(serde_json::to_value(&pii_spans).unwrap_or(serde_json::Value::Null));
                }
            }
        }

        let embedding_result = if should_index(importance, &memory_type) {
            let emb_provider = self.embedding();
            match emb_provider.embed(content) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!("[MemoryManager] 合并记忆 embedding 生成失败: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let mut inner = self.inner.write();
        inner.add_entry(item.clone())?;
        if should_index(importance, &memory_type) {
            if let Some(emb) = embedding_result {
                let vec = MemoryVector {
                    doc_id: item.id.clone(),
                    memory_id: item.id.clone(),
                    content: content.to_string(),
                    embedding: emb,
                    importance,
                    memory_type: memory_type.as_str().to_string(),
                    timestamp: item.timestamp,
                };
                inner.vector_store.add(vec)?;
                if let Err(e) = inner.vector_store.save_to() {
                    tracing::warn!("[MemoryManager] 合并记忆向量持久化失败: {e}");
                }
            }
        }
        drop(inner);
        self.emit_memory_updated();
        Ok(item)
    }

    /// 记忆写入内部统一入口（供 add_memory / add_memory_with_metadata / add_memory_enriched 共用）
    ///
    /// - `embedding_text` 为 Some 时，用该文本做 embedding 和向量存储 content；
    ///   为 None 时用 content 本身。
    /// - `metadata` 为 Some 时，写入调用方提供的 metadata（合并到 MemoryItem.metadata），
    ///   用于跨角色对话/旁观记忆等场景标注 speaker/listener/perspective。
    async fn add_memory_inner(
        &self,
        content: &str,
        memory_type: MemoryType,
        importance: f64,
        tags: Vec<String>,
        embedding_text: Option<&str>,
        metadata: Option<serde_json::Value>,
        timestamp_override: Option<f64>,
    ) -> VivianResult<MemoryItem> {
        let importance = importance.clamp(0.0, 1.0);
        let granularity = memory_type.default_granularity();

        let (safe_content, pii_spans, redact_status) = redact_content(content);
        let content = safe_content.as_str();

        let mut inferred_tags = tags.clone();
        if inferred_tags.is_empty() {
            inferred_tags = infer_tags_for_content(content, &memory_type);
        }

        let mut item = MemoryItem::new(content.to_string(), granularity, importance);
        // 允许回溯时间戳到事件真实发生时刻（如工具执行时刻），保证图谱时间线顺序正确
        if let Some(ts) = timestamp_override {
            item.timestamp = ts;
        }
        item.memory_type = memory_type.as_str().to_string();
        item.tags = inferred_tags;
        init_heat(&mut item);

        // 应用调用方提供的 metadata（跨角色对话/旁观记忆等场景）
        if let Some(meta) = metadata {
            // 合并：以现有 metadata 为基础，用调用方的 meta 覆盖（对象级别 shallow merge）
            let merged = if item.metadata.is_object() && meta.is_object() {
                let mut base = item.metadata.clone();
                if let Some(obj) = base.as_object_mut() {
                    if let Some(incoming) = meta.as_object() {
                        for (k, v) in incoming {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                }
                base
            } else {
                meta
            };
            item.metadata = merged;
        }

        // 注入当前会话 ID（不覆盖调用方已显式提供的 session_id）
        if let Some(sid) = self.get_session_id() {
            if let Some(obj) = item.metadata.as_object_mut() {
                obj.entry("session_id")
                    .or_insert(serde_json::Value::String(sid));
            }
        }

        // 写入 PII 脱敏状态（不覆盖调用方已显式提供的 redact_status）
        if redact_status != RedactStatus::Clean {
            if let Some(obj) = item.metadata.as_object_mut() {
                obj.entry("redact_status")
                    .or_insert(serde_json::Value::String(redact_status.as_str().to_string()));
                if !pii_spans.is_empty() {
                    obj.entry("pii_spans")
                        .or_insert(serde_json::to_value(&pii_spans).unwrap_or(serde_json::Value::Null));
                }
            }
        }

        // embedding 文本：有 summary 时用 summary，否则用 content
        let safe_emb_text = embedding_text.map(|t| {
            let (redacted, _, _) = redact_content(t);
            redacted
        });
        let emb_source = safe_emb_text.as_deref().unwrap_or(content);

        // 预计算 embedding（冲突检测 + 向量索引共用，避免重复计算）
        let need_embedding = should_index(importance, &memory_type)
            || super::conflict::should_check_conflict(memory_type);
        let embedding_result = if need_embedding {
            let emb_provider = self.embedding();
            match emb_provider.embed(emb_source) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        "[MemoryManager] embedding 生成失败，降级到关键词匹配: {}", e
                    );
                    None
                }
            }
        } else {
            None
        };

        // 冲突检测（只读阶段）：对持久型记忆查找相似记忆并评分
        let conflict_outcome = if super::conflict::should_check_conflict(memory_type) {
            if let Some(ref emb) = embedding_result {
                self.detect_memory_conflict(emb, &item)
            } else {
                ConflictOutcome::KeepBoth
            }
        } else {
            ConflictOutcome::KeepBoth
        };

        // 写入阶段
        let mut inner = self.inner.write();

        // 应用冲突决策
        match &conflict_outcome {
            ConflictOutcome::KeepBoth => {}
            ConflictOutcome::QueueLlm {
                old_id,
                old_content,
                similarity,
            } => {
                // 推入 pending 队列，待后台 tick 调 LLM 仲裁
                let pending = super::conflict::PendingConflict {
                    id: uuid::Uuid::new_v4().to_string(),
                    new_memory_id: item.id.clone(),
                    new_content: content.to_string(),
                    old_memory_id: old_id.clone(),
                    old_content: old_content.clone(),
                    similarity: *similarity,
                    created_at: current_timestamp(),
                    retry_count: 0,
                };
                let mut queue = self.pending_conflicts.lock();
                if queue.len() < 50 {
                    queue.push(pending);
                    drop(queue);
                    let _ = self.save_pending_conflicts();
                } else {
                    tracing::warn!(
                        "[ConflictDetection] pending 队列已满（50），丢弃最新 QueueLlm 冲突"
                    );
                }
            }
            ConflictOutcome::ReplaceOld { old_id } => {
                let _ = inner.remove_by_id(old_id);
                if inner.vector_store.delete_by_memory_id(old_id) {
                    let _ = inner.vector_store.save_to();
                }
                tracing::info!(
                    "[ConflictDetection] ReplaceOld: 删除旧记忆 {} → 新内容 '{}'",
                    old_id,
                    &content[..content.len().min(40)]
                );
            }
            ConflictOutcome::MergeSupersede { old_id, old_content } => {
                item.content = simple_merge(old_content, content);
                let _ = inner.remove_by_id(old_id);
                if inner.vector_store.delete_by_memory_id(old_id) {
                    let _ = inner.vector_store.save_to();
                }
                tracing::info!(
                    "[ConflictDetection] MergeSupersede: 合并旧记忆 {} → '{}'",
                    old_id,
                    &item.content[..item.content.len().min(50)]
                );
            }
        }

        inner.add_entry(item.clone())?;

        // 向量索引（复用预计算的 embedding，content 用 emb_source 保持一致性）
        // 旁观记忆（perspective="observer"）也进入向量索引，但在检索时降权（score * 0.5），
        // 使其能被找到但排名低于直接参与的记忆。
        if should_index(importance, &memory_type) {
            if let Some(emb) = embedding_result {
                let memory_id = item.id.clone();
                let memory_type_str = memory_type.as_str().to_string();
                let timestamp = item.timestamp;
                let vec = MemoryVector {
                    doc_id: memory_id.clone(),
                    memory_id,
                    content: emb_source.to_string(),
                    embedding: emb,
                    importance,
                    memory_type: memory_type_str,
                    timestamp,
                };
                inner.vector_store.add(vec)?;
                if let Err(e) = inner.vector_store.save_to() {
                    tracing::warn!("[MemoryManager] 向量存储持久化失败: {e}");
                }
            }
        }

        drop(inner);

        // 知识图谱注入：从记忆内容提取实体和关系（零 LLM 调用）
        let (entity_count, edge_count) =
            self.knowledge_graph
                .ingest_from_memory(&item.id, content, item.timestamp);
        if entity_count > 0 || edge_count > 0 {
            tracing::debug!(
                "[KnowledgeGraph] 记忆 {} 注入 {} 实体 / {} 边",
                item.id,
                entity_count,
                edge_count
            );
            if let Err(e) = self.knowledge_graph.save_to_disk() {
                tracing::warn!("[MemoryManager] 知识图谱持久化失败: {e}");
            }
        }

        // Takes 围栏注入：以首个实体名为 subject 写入 mentioned take
        let extraction = super::entity_extract::extract(content);
        if let Some(first_entity) = extraction.entities.first() {
            let _ = self.takes_fence.ingest_from_memory(
                &first_entity.name,
                content,
                &item.id,
                item.timestamp,
            );
            if let Err(e) = self.takes_fence.save_to_disk() {
                tracing::warn!("[MemoryManager] Takes 围栏持久化失败: {e}");
            }
        }

        self.emit_memory_updated();

        // Memory Router：同步路由判断是否应额外写入共享世界记忆层
        // 候选条目（含持久性词汇的用户偏好/家规/环境事实）写入 WorldKnowledge
        // RelationshipFact 由 cross_character.rs 的 LLM 抽取负责，不在此处理
        route_to_shared_world(content, importance, &item.metadata, &self.char_id, &item.id);

        Ok(item)
    }

    /// 在向量存储中查找最相似的记忆，执行冲突检测并返回动作
    fn detect_memory_conflict(&self, emb: &[f32], new_item: &MemoryItem) -> ConflictOutcome {
        let inner = self.inner.read();

        let candidates = inner.vector_store.search(emb, 3);

        // 映射 memory_id → MemoryItem，过滤低相似度和低重要性
        let best = candidates
            .iter()
            .filter_map(|(_, mem_id, score)| {
                if *score < 0.45 {
                    return None;
                }
                let idx = inner.id_index.get(mem_id)?;
                let old_item = inner.data.entries.get(*idx)?;
                // 只对有一定重要性的记忆做冲突检测（避免与 ShortTerm 缓冲冲突）
                if old_item.importance < 0.3 {
                    return None;
                }
                Some((old_item, *score))
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let Some((old_item, sim)) = best else {
            return ConflictOutcome::KeepBoth;
        };

        let old_id = old_item.id.clone();
        let old_content = old_item.content.clone();

        // 近期注入判定：1 小时内被检索过
        let now = current_timestamp();
        let recently_injected = old_item.visit_count > 0 && (now - old_item.last_visit_at) < 3600.0;

        let input = build_score_input(new_item, old_item, Some(sim), recently_injected);
        let result = score_conflict(&input);
        let action = resolve_action(&result);

        match action {
            ConflictAction::KeepBoth => ConflictOutcome::KeepBoth,
            ConflictAction::ReplaceOld => ConflictOutcome::ReplaceOld { old_id },
            ConflictAction::MergeSupersede => ConflictOutcome::MergeSupersede {
                old_id,
                old_content,
            },
            ConflictAction::QueueLlm => {
                tracing::info!(
                    "[ConflictDetection] QueueLlm: score={:.1}, old_id={}, 待后台 LLM 仲裁",
                    result.conflict_score,
                    old_id
                );
                ConflictOutcome::QueueLlm {
                    old_id,
                    old_content,
                    similarity: sim,
                }
            }
        }
    }

    /// 统一的对话上下文保存入口
    ///
    /// **三层记忆语义**（与 ConsolidationPipeline 配合）：
    /// - 用户消息 / AI 回复 → **ShortTerm**（Turn 级缓冲，不经 LLM enrich，低成本写入）
    ///   - 由 ConsolidationPipeline Stage 1 在阈值/空闲时摘要提升为 SessionSummary
    /// - LLM 长期记忆 → **LongTerm**（LLM 已在主对话路径做过语义抽取，直接入库）
    ///
    /// 这样 ShortTerm 真正承担"易失缓冲→沉淀"的语义，避免形同虚设。
    /// 任一步失败不中断后续步骤（仅 warn 日志）。
    ///
    /// 参数：
    /// - `user_input`：用户消息文本（None 或空字符串跳过）
    /// - `ai_responses`：AI 回复列表
    /// - `long_term_memory`：LLM 生成的长期记忆（None 或空字符串跳过）
    /// - `user_emotion` / `ai_emotion`：写入 tags 伴随记忆持久化
    /// - `importance_user` / `importance_ai`：重要性 hint（ShortTerm 通常较低）
    pub async fn save_context(
        &self,
        user_input: Option<&str>,
        ai_responses: &[String],
        long_term_memory: Option<&str>,
        user_emotion: &str,
        ai_emotion: Option<&str>,
        importance_user: f64,
        importance_ai: f64,
    ) -> VivianResult<()> {
        // 委托给带 metadata 的版本，使用 char_id 推导默认 metadata
        // channel 默认 "direct"（MemoryManager 不持有 DialogueManager，无法获取实际 channel）
        let char_id = self.char_id.clone();
        let default_user_meta = serde_json::json!({
            "channel": "direct",
            "speaker": "user",
            "listener": char_id,
            "perspective": "speaker",
            "knowledge_source": "direct",
        });
        let default_ai_meta = serde_json::json!({
            "channel": "direct",
            "speaker": self.char_id.clone(),
            "listener": "user",
            "perspective": "speaker",
            "knowledge_source": "direct",
        });
        self.save_context_with_metadata(
            user_input,
            ai_responses,
            long_term_memory,
            user_emotion,
            ai_emotion,
            importance_user,
            importance_ai,
            Some(default_user_meta),
            Some(default_ai_meta),
        )
        .await
    }

    /// 带 metadata 的 save_context：支持为用户消息和 AI 回复分别指定 metadata。
    ///
    /// 当 metadata 为 None 时使用默认的 channel=direct + speaker/listener 元数据。
    /// 保存对话内容时自动附加说话者前缀（如 "[User says to me]", "[I say to User]"），
    /// 已包含前缀的内容不会重复添加。
    pub async fn save_context_with_metadata(
        &self,
        user_input: Option<&str>,
        ai_responses: &[String],
        long_term_memory: Option<&str>,
        user_emotion: &str,
        ai_emotion: Option<&str>,
        importance_user: f64,
        importance_ai: f64,
        user_metadata: Option<serde_json::Value>,
        ai_metadata: Option<serde_json::Value>,
    ) -> VivianResult<()> {
        let char_id = self.char_id.clone();

        // 构造默认 metadata（未提供时使用直接对话的默认值）
        let default_user_meta = serde_json::json!({
            "channel": "direct",
            "speaker": "user",
            "listener": char_id,
            "perspective": "speaker",
            "knowledge_source": "direct",
        });
        let default_ai_meta = serde_json::json!({
            "channel": "direct",
            "speaker": char_id,
            "listener": "user",
            "perspective": "speaker",
            "knowledge_source": "direct",
        });

        // 为内容添加说话者前缀（如果尚未有前缀且 metadata 包含 speaker/listener）
        let add_prefix_if_needed = |content: &str, meta: &serde_json::Value| -> String {
            let trimmed = content.trim();
            // 检查是否已有说话者前缀
            let (_, existing_speaker, _) = parse_any_speaker_prefix(trimmed);
            if existing_speaker.is_some() {
                return trimmed.to_string();
            }
            // 从 metadata 中提取 speaker 和 listener
            let speaker = meta.get("speaker").and_then(|v| v.as_str());
            let listener = meta.get("listener").and_then(|v| v.as_str());
            if let (Some(spk), Some(lst)) = (speaker, listener) {
                let prefix = build_speaker_prefix(spk, lst, &char_id);
                format!("{} {}", prefix, trimmed)
            } else {
                trimmed.to_string()
            }
        };

        // 合并 metadata：调用方提供的覆盖默认值
        let effective_user_meta = user_metadata.unwrap_or(default_user_meta);
        let effective_ai_meta = ai_metadata.unwrap_or(default_ai_meta);

        // 1. 用户消息 → ShortTerm 缓冲（不走 LLM enrich，由 Stage 1 统一摘要）
        if let Some(input) = user_input {
            let trimmed = input.trim();
            if !trimmed.is_empty() {
                let user_emo = if user_emotion.trim().is_empty() {
                    "neutral"
                } else {
                    user_emotion
                };
                let user_tags = vec![
                    "short_term".to_string(),
                    "user".to_string(),
                    "dialogue_turn".to_string(),
                    user_emo.to_string(),
                ];
                let prefixed_content = add_prefix_if_needed(trimmed, &effective_user_meta);
                if let Err(e) = self
                    .add_memory_with_metadata(
                        &prefixed_content,
                        MemoryType::ShortTerm,
                        importance_user,
                        user_tags,
                        effective_user_meta.clone(),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "[save_context] 用户消息写入 ShortTerm 失败");
                }
            }
        }

        // 2. AI 回复 → ShortTerm 缓冲
        let ai_emo = ai_emotion.unwrap_or("neutral");
        for (i, resp) in ai_responses.iter().enumerate() {
            let trimmed = resp.trim();
            if trimmed.is_empty() {
                continue;
            }
            let imp = if i == 0 {
                importance_ai.min(0.25)
            } else {
                importance_ai.min(0.3)
            };
            let tags = vec![
                "short_term".to_string(),
                "assistant".to_string(),
                "dialogue_turn".to_string(),
                ai_emo.to_string(),
            ];
            let prefixed_content = add_prefix_if_needed(trimmed, &effective_ai_meta);
            if let Err(e) = self
                .add_memory_with_metadata(
                    &prefixed_content,
                    MemoryType::ShortTerm,
                    imp,
                    tags,
                    effective_ai_meta.clone(),
                )
                .await
            {
                tracing::warn!(error = %e, idx = i, "[save_context] AI 回复写入 ShortTerm 失败");
            }
        }

        // 3. LLM 长期记忆 → 直接写 LongTerm（主对话路径已做语义抽取）
        // 主调 LLM 已经在反思阶段完成了语义抽取（description/keywords/importance），
        // 此处不再触发 enricher 的二次 LLM 调用，避免 MemorySaving 步骤超时。
        // enricher 仍可在 inner_monologue / 主动记忆巩固等非关键路径中异步使用。
        if let Some(ltm) = long_term_memory {
            let trimmed = ltm.trim();
            if !trimmed.is_empty() {
                let tags = vec!["llm_generated".to_string()];
                let meta = serde_json::json!({
                    "channel": "direct",
                    "speaker": "user",
                    "listener": char_id,
                    "knowledge_source": "llm_extracted",
                });
                if let Err(e) = self
                    .add_memory_with_metadata(
                        trimmed,
                        MemoryType::LongTerm,
                        0.9,
                        tags,
                        meta,
                    )
                    .await
                {
                    tracing::warn!(error = %e, "[save_context] LLM 长期记忆写入失败");
                }
            }
        }

        Ok(())
    }

    /// 写入时 LLM 增强版：抽取 description/keywords/importance 后再入库。
    ///
    /// LLM 失败时退化为 `add_memory`（规则化标签推断）。
    /// 仅当 `enricher` 已注入时调用 LLM；否则等同 `add_memory`。
    pub async fn add_memory_enriched(
        &self,
        content: &str,
        memory_type: MemoryType,
        importance_hint: f64,
        tags: Vec<String>,
    ) -> VivianResult<MemoryItem> {
        // 委托给带 metadata 的版本，传 None 保持向后兼容
        self.add_memory_enriched_with_metadata(
            content,
            memory_type,
            importance_hint,
            tags,
            None,
            None,
        )
        .await
    }

    /// LLM 增强写入 + 初始 metadata。
    ///
    /// 与 `add_memory_enriched` 的区别：支持为记忆指定初始 metadata（如 channel/speaker/listener）。
    /// LLM 增强产出的 description/keywords/mood_tags/summary 会 merge 到该 metadata 上，
    /// 不会覆盖调用方传入的 channel/speaker/listener 等字段。
    ///
    /// 修复 M2：enriched 路径此前无法携带 metadata，导致 ImportantEvent 等需 LLM 增强的记忆
    /// 缺少跨角色上下文标注，事件账本注册时字段全用默认值。
    pub async fn add_memory_enriched_with_metadata(
        &self,
        content: &str,
        memory_type: MemoryType,
        importance_hint: f64,
        tags: Vec<String>,
        initial_metadata: Option<serde_json::Value>,
        timestamp_override: Option<f64>,
    ) -> VivianResult<MemoryItem> {
        let enricher = {
            let guard = self.enricher.read();
            guard.as_ref().cloned()
        };
        let enricher = match enricher {
            Some(e) => e,
            None => {
                // 无 enricher：退化为规则化写入（保留时间戳回溯）
                return self
                    .add_memory_inner(
                        content,
                        memory_type,
                        importance_hint,
                        tags,
                        None,
                        Some(initial_metadata.unwrap_or_else(|| serde_json::json!({}))),
                        timestamp_override,
                    )
                    .await;
            }
        };

        let meta: EnrichedMeta = match enricher.enrich(content).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[MemoryManager] LLM 增强失败，回退规则化: {e}");
                return self
                    .add_memory_inner(
                        content,
                        memory_type,
                        importance_hint,
                        tags,
                        None,
                        Some(initial_metadata.unwrap_or_else(|| serde_json::json!({}))),
                        timestamp_override,
                    )
                    .await;
            }
        };

        // keywords 不进入 tags（防止 LLM 自创中文标签污染 tags），
        // 仅存入 metadata["keywords"] 供关键词搜索使用。
        // tags 只保留结构化来源（MemoryType / SemanticType / 调用方传入的规范标签）。
        let importance = meta.importance.unwrap_or(importance_hint);

        // 有 summary 时用 summary 做 embedding，避免长文本向量稀释
        let summary_for_embedding = meta.summary.as_deref();
        let mut item = self
            .add_memory_inner(
                content,
                memory_type,
                importance,
                tags,
                summary_for_embedding,
                initial_metadata,
                timestamp_override,
            )
            .await?;

        // 写入 description / semantic_type / keywords / mood_tags / summary 到 metadata
        let semantic_type = meta.semantic_type_or_general();
        let has_keywords = !meta.keywords.is_empty();
        let has_mood = !meta.mood_tags.is_empty();
        let has_summary = meta.summary.is_some();
        if meta.description.is_some()
            || !matches!(semantic_type, SemanticType::General)
            || has_keywords
            || has_mood
            || has_summary
        {
            let mut inner = self.inner.write();
            if let Some(&idx) = inner.id_index.get(&item.id) {
                if idx < inner.data.entries.len() && inner.data.entries[idx].id == item.id {
                    if let Some(desc) = meta.description.clone() {
                        inner.data.entries[idx].description = Some(desc.clone());
                        item.description = Some(desc);
                    }
                    let entry = &mut inner.data.entries[idx];
                    let mood_array: Vec<serde_json::Value> = meta
                        .mood_tags
                        .iter()
                        .map(|m| serde_json::Value::String(m.clone()))
                        .collect();
                    if let Some(obj) = entry.metadata.as_object_mut() {
                        obj.insert(
                            "semantic_type".to_string(),
                            serde_json::Value::String(semantic_type.as_str().to_string()),
                        );
                        if has_keywords {
                            let kw_array: Vec<serde_json::Value> = meta
                                .keywords
                                .iter()
                                .map(|k| serde_json::Value::String(k.clone()))
                                .collect();
                            obj.insert("keywords".to_string(), serde_json::Value::Array(kw_array));
                        }
                        if has_mood {
                            obj.insert("mood_tags".to_string(), serde_json::Value::Array(mood_array.clone()));
                        }
                        if let Some(ref s) = meta.summary {
                            obj.insert("summary".to_string(), serde_json::Value::String(s.clone()));
                        }
                    } else {
                        let mut meta_obj = serde_json::Map::new();
                        meta_obj.insert(
                            "semantic_type".to_string(),
                            serde_json::Value::String(semantic_type.as_str().to_string()),
                        );
                        if has_keywords {
                            let kw_array: Vec<serde_json::Value> = meta
                                .keywords
                                .iter()
                                .map(|k| serde_json::Value::String(k.clone()))
                                .collect();
                            meta_obj.insert("keywords".to_string(), serde_json::Value::Array(kw_array));
                        }
                        if has_mood {
                            meta_obj.insert("mood_tags".to_string(), serde_json::Value::Array(mood_array.clone()));
                        }
                        if let Some(ref s) = meta.summary {
                            meta_obj.insert("summary".to_string(), serde_json::Value::String(s.clone()));
                        }
                        entry.metadata = serde_json::Value::Object(meta_obj);
                    }
                    item.metadata = inner.data.entries[idx].metadata.clone();
                    if let Err(e) = inner.save_throttled() {
                        tracing::warn!("[MemoryManager] 写入 description/semantic_type 持久化失败: {e}");
                    }
                }
            }
        }

        Ok(item)
    }

    /// 删除指定 ID 的记忆（软删除至回收站，7 天内可恢复）
    pub async fn delete_memory(&self, id: &str) -> VivianResult<()> {
        self.soft_delete_memory(id, "manual_delete").await
    }

    /// 软删除：把记忆移入回收站，保留 7 天可恢复窗口
    pub async fn soft_delete_memory(&self, id: &str, reason: &str) -> VivianResult<()> {
        let removed_item = {
            let mut inner = self.inner.write();
            let item = inner.take_by_id(id)?;
            if item.is_some() {
                if inner.vector_store.delete_by_memory_id(id) {
                    if let Err(e) = inner.vector_store.save_to() {
                        tracing::warn!("[MemoryManager] 向量存储持久化失败: {e}");
                    }
                }
                if let Err(e) = inner.save_to_disk() {
                    tracing::warn!("[MemoryManager] 软删除后磁盘持久化失败: {e}");
                }
            }
            item
        };
        if let Some(item) = removed_item {
            self.recycle_bin.push(item, reason);
            if let Err(e) = self.recycle_bin.save() {
                tracing::warn!("[MemoryManager] 回收站持久化失败: {e}");
            }
            self.emit_memory_updated();
        }
        Ok(())
    }

    /// 从回收站恢复记忆（重新加入主存储与向量索引）
    pub fn restore_memory(&self, id: &str) -> VivianResult<bool> {
        if let Some(item) = self.recycle_bin.restore(id) {
            let mut inner = self.inner.write();
            inner.restore_entry(item.clone())?;
            if is_indexable_entry(&item) {
                if let Some(emb) = item.embedding.clone() {
                    let emb_f32: Vec<f32> = emb.iter().map(|v| *v as f32).collect();
                    let vec = MemoryVector {
                        doc_id: item.id.clone(),
                        memory_id: item.id.clone(),
                        content: item.content.clone(),
                        embedding: emb_f32,
                        importance: item.importance,
                        memory_type: item.memory_type.clone(),
                        timestamp: item.timestamp,
                    };
                    if let Err(e) = inner.vector_store.add(vec) {
                        tracing::warn!("[MemoryManager] 恢复后向量写入失败: {e}");
                    }
                }
            }
            if let Err(e) = inner.save_to_disk() {
                tracing::warn!("[MemoryManager] 恢复后磁盘持久化失败: {e}");
            }
            drop(inner);
            if let Err(e) = self.recycle_bin.save() {
                tracing::warn!("[MemoryManager] 回收站持久化失败: {e}");
            }
            self.emit_memory_updated();
            return Ok(true);
        }
        Ok(false)
    }

    /// 永久清除回收站中超过 7 天保留期的条目
    pub fn purge_expired_recycle_bin(&self) -> usize {
        let removed = self.recycle_bin.purge_expired();
        if removed > 0 {
            if let Err(e) = self.recycle_bin.save() {
                tracing::warn!("[MemoryManager] 回收站清理后持久化失败: {e}");
            }
        }
        removed
    }

    /// 永久清除回收站中指定条目
    pub fn purge_recycle_entry(&self, id: &str) -> bool {
        let purged = self.recycle_bin.purge(id);
        if purged {
            if let Err(e) = self.recycle_bin.save() {
                tracing::warn!("[MemoryManager] 回收站清除后持久化失败: {e}");
            }
        }
        purged
    }

    /// 清空整个回收站
    pub fn purge_all_recycle_bin(&self) -> usize {
        let count = self.recycle_bin.purge_all();
        if count > 0 {
            if let Err(e) = self.recycle_bin.save() {
                tracing::warn!("[MemoryManager] 回收站清空后持久化失败: {e}");
            }
        }
        count
    }

    /// 列出回收站中的全部条目
    pub fn list_recycle_bin(&self) -> Vec<crate::memory::recycle_bin::RecycleEntry> {
        self.recycle_bin.list()
    }

    /// 回收站当前条目数
    pub fn recycle_bin_count(&self) -> usize {
        self.recycle_bin.count()
    }

    /// 永久删除指定 ID 的记忆（绕过回收站）
    pub async fn hard_delete_memory(&self, id: &str) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.remove_by_id(id)?;
        if inner.vector_store.delete_by_memory_id(id) {
            if let Err(e) = inner.vector_store.save_to() {
                tracing::warn!("[MemoryManager] 向量存储持久化失败: {e}");
            }
        }
        inner.save_to_disk()?;
        drop(inner);
        self.emit_memory_updated();
        Ok(())
    }

    /// 归档指定记忆（软删除，取代整合流水线的硬删除）。
    ///
    /// - 设置 `consolidated = true`：检索方法会过滤掉已归档记忆
    /// - 从向量存储中移除：不再参与 embedding 相似度检索
    /// - 保留在磁盘条目中：可追溯整合来源（`promoted_from` 引用仍有效）
    pub fn archive_memory(&self, id: &str) -> VivianResult<()> {
        let mut inner = self.inner.write();
        if let Some(&idx) = inner.id_index.get(id) {
            if idx < inner.data.entries.len() && inner.data.entries[idx].id == id {
                inner.data.entries[idx].consolidated = true;
                // 从向量存储中移除（不参与 embedding 检索）
                if inner.vector_store.delete_by_memory_id(id) {
                    if let Err(e) = inner.vector_store.save_to() {
                        tracing::warn!("[MemoryManager] archive 后向量存储持久化失败: {e}");
                    }
                }
                if let Err(e) = inner.save_to_disk() {
                    tracing::warn!("[MemoryManager] archive 后磁盘持久化失败: {e}");
                }
            }
        }
        Ok(())
    }

    /// 标记记忆为"已被摘要压缩"（区别于完全归档的 `archive_memory`）。
    ///
    /// - 设置 `consolidated = true`：历史对话注入路径会过滤掉
    /// - 设置 `metadata.summarized = true`：标识已摘要
    /// - **保留向量索引**：LLM 向量检索仍可匹配到原始对话
    /// - 保留在磁盘条目中：前端图谱可展开显示
    pub fn mark_summarized(&self, id: &str) -> VivianResult<()> {
        let mut inner = self.inner.write();
        if let Some(&idx) = inner.id_index.get(id) {
            if idx < inner.data.entries.len() && inner.data.entries[idx].id == id {
                inner.data.entries[idx].consolidated = true;
                if let Some(obj) = inner.data.entries[idx].metadata.as_object_mut() {
                    obj.insert("summarized".to_string(), serde_json::Value::Bool(true));
                } else {
                    let mut obj = serde_json::Map::new();
                    obj.insert("summarized".to_string(), serde_json::Value::Bool(true));
                    inner.data.entries[idx].metadata = serde_json::Value::Object(obj);
                }
                if let Err(e) = inner.save_to_disk() {
                    tracing::warn!("[MemoryManager] mark_summarized 后磁盘持久化失败: {e}");
                }
            }
        }
        Ok(())
    }

    /// 原子地合并 metadata 字段到指定记忆（用于巩固流水线注入 promoted_from 等）。
    ///
    /// 在已有 metadata 基础上做对象级 merge：`patch` 中的键覆盖已有同名键。
    /// 持久化失败仅警告，返回成功。
    pub fn patch_memory_metadata(&self, id: &str, patch: serde_json::Value) -> VivianResult<()> {
        if !patch.is_object() {
            return Ok(());
        }
        let mut inner = self.inner.write();
        if let Some(&idx) = inner.id_index.get(id) {
            if idx < inner.data.entries.len() && inner.data.entries[idx].id == id {
                let entry = &mut inner.data.entries[idx];
                if !entry.metadata.is_object() {
                    entry.metadata = serde_json::json!({});
                }
                if let (Some(target), Some(patch_obj)) =
                    (entry.metadata.as_object_mut(), patch.as_object())
                {
                    for (k, v) in patch_obj {
                        target.insert(k.clone(), v.clone());
                    }
                }
                if let Err(e) = inner.save_throttled() {
                    tracing::warn!("[MemoryManager] patch_memory_metadata 持久化失败: {e}");
                }
            }
        }
        Ok(())
    }

    /// 对指定记忆应用证据信号（写入 reinforcement/disputation 双时钟）
    ///
    /// 由巩固 Stage 2 主动再评估调用：新 LongTerm 与旧 LongTerm 词法矛盾时，
    /// 对旧记忆应用 Negates 信号，使其 evidence_score 下降，长期累积触发归档。
    pub fn apply_evidence_to_memory(
        &self,
        memory_id: &str,
        source: super::evidence::EvidenceSource,
        kind: super::evidence::SignalKind,
    ) -> VivianResult<()> {
        let now = current_timestamp();
        let mut inner = self.inner.write();
        if let Some(&idx) = inner.id_index.get(memory_id) {
            if idx < inner.data.entries.len() && inner.data.entries[idx].id == memory_id {
                let entry = &mut inner.data.entries[idx];
                let snap = super::evidence::apply_evidence_signal(entry, source, kind, now);
                snap.apply_to(entry);
                if let Err(e) = inner.save_throttled() {
                    tracing::warn!("[MemoryManager] apply_evidence_to_memory 持久化失败: {e}");
                }
            }
        }
        Ok(())
    }

    /// 检索与新内容语义相似的持久型记忆（用于证据再评估）
    ///
    /// 返回 (memory_id, content, similarity) 列表，按相似度降序。
    /// 仅返回 importance >= 0.3 的持久型记忆，排除 ShortTerm/CasualConversation 等缓冲型。
    pub fn find_similar_persistent_memories(
        &self,
        content: &str,
        top_k: usize,
    ) -> Vec<(String, String, f64)> {
        let inner = self.inner.read();
        let emb = match inner.embedding.embed(content) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let candidates = inner.vector_store.search(&emb, top_k);
        candidates
            .into_iter()
            .filter_map(|(_doc_id, mem_id, score)| {
                if score < 0.45 {
                    return None;
                }
                let idx = inner.id_index.get(&mem_id)?;
                let item = inner.data.entries.get(*idx)?;
                if item.importance < 0.3 {
                    return None;
                }
                let mem_type = MemoryType::from_str(&item.memory_type);
                let is_persistent = matches!(
                    mem_type,
                    Some(MemoryType::LongTerm)
                        | Some(MemoryType::Preference)
                        | Some(MemoryType::Identity)
                        | Some(MemoryType::ImportantEvent)
                        | Some(MemoryType::Knowledge)
                        | Some(MemoryType::User)
                        | Some(MemoryType::Feedback)
                );
                if !is_persistent {
                    return None;
                }
                Some((item.id.clone(), item.content.clone(), score))
            })
            .collect()
    }

    /// 更新指定记忆的 open_hooks 字段（用于 HookJudge 闭环判定）
    pub fn update_open_hooks(
        &self,
        id: &str,
        open_hooks: Vec<super::types::OpenHook>,
    ) -> VivianResult<()> {
        let mut inner = self.inner.write();
        if let Some(&idx) = inner.id_index.get(id) {
            if idx < inner.data.entries.len() && inner.data.entries[idx].id == id {
                inner.data.entries[idx].open_hooks = open_hooks;
                if let Err(e) = inner.save_throttled() {
                    tracing::warn!("[MemoryManager] update_open_hooks 持久化失败: {e}");
                }
            }
        }
        Ok(())
    }

    /// 追加单个 OpenHook 到指定记忆（避免同类型 hook 重复）
    ///
    /// 用于 Open Loop 场景：会话因超时关闭时，在最后一条记忆上附加 follow_up hook，
    /// 下次检索时 hook_boost 让该记忆优先召回，实现话题自然续接。
    pub fn attach_open_hook(&self, memory_id: &str, hook: super::types::OpenHook) {
        let mut inner = self.inner.write();
        if let Some(&idx) = inner.id_index.get(memory_id) {
            if idx < inner.data.entries.len() && inner.data.entries[idx].id == memory_id {
                let entry = &mut inner.data.entries[idx];
                // 避免同类型 hook 重复
                if !entry.open_hooks.iter().any(|h| h.hook_type == hook.hook_type && h.is_open()) {
                    entry.open_hooks.push(hook);
                    if let Err(e) = inner.save_throttled() {
                        tracing::warn!("[MemoryManager] attach_open_hook 持久化失败: {e}");
                    }
                }
            }
        }
    }

    /// 按 ID 获取记忆内容（只读）
    ///
    /// 供 Open Loop 总结等场景按 memory_id 反查内容使用。
    pub fn get_memory_content_by_id(&self, id: &str) -> Option<String> {
        let inner = self.inner.read();
        inner.id_index.get(id).and_then(|&idx| {
            if idx < inner.data.entries.len() && inner.data.entries[idx].id == id {
                Some(inner.data.entries[idx].content.clone())
            } else {
                None
            }
        })
    }

    /// 替换指定记忆的内容并重建向量索引（供驱逐合并后的 LLM 重压缩使用）。
    ///
    /// 流程：更新 content → 重算 embedding → 删旧向量加新向量 → 落盘。
    /// embedding 计算失败仅清空 embedding 字段，不影响内容更新。
    pub fn replace_content_and_reindex(&self, id: &str, new_content: &str) -> VivianResult<()> {
        let embedding_provider;
        let mut inner = self.inner.write();
        embedding_provider = inner.embedding.clone();
        if let Some(&idx) = inner.id_index.get(id) {
            if idx < inner.data.entries.len() && inner.data.entries[idx].id == id {
                let entry = &mut inner.data.entries[idx];
                entry.content = new_content.to_string();
                entry.embedding = None;
            }
        }

        // 重建向量索引
        let (importance, memory_type_str, timestamp) = match inner.id_index.get(id) {
            Some(&idx) if idx < inner.data.entries.len() && inner.data.entries[idx].id == id => {
                let entry = &inner.data.entries[idx];
                (
                    entry.importance,
                    entry
                        .tags
                        .iter()
                        .find(|t| {
                            matches!(t.as_str(),
                                "long_term" | "user" | "preference" | "important_event" |
                                "feedback" | "knowledge" | "session_summary" | "insight" |
                                "short_term" | "keyword" | "summary"
                            )
                        })
                        .cloned()
                        .unwrap_or_else(|| "general".to_string()),
                    entry.timestamp,
                )
            }
            _ => return Ok(()),
        };

        // 删旧向量
        let _ = inner.vector_store.delete_by_memory_id(id);

        // 加新向量（embedding 计算失败则跳过）
        if let Ok(emb) = embedding_provider.embed(new_content) {
            let vec = MemoryVector {
                doc_id: id.to_string(),
                memory_id: id.to_string(),
                content: new_content.to_string(),
                embedding: emb,
                importance,
                memory_type: memory_type_str,
                timestamp,
            };
            inner.vector_store.add(vec)?;
        }
        if let Err(e) = inner.vector_store.save_to() {
            tracing::warn!("[MemoryManager] 向量索引重建持久化失败: {e}");
        }
        if let Err(e) = inner.save_throttled() {
            tracing::warn!("[MemoryManager] replace_content_and_reindex 持久化失败: {e}");
        }
        Ok(())
    }

    /// 获取所有含未闭环 open_hooks 的记忆（用于 HookJudge 异步判定）
    pub fn get_memories_with_open_hooks(&self) -> Vec<MemoryItem> {
        let inner = self.inner.read();
        inner
            .data
            .entries
            .iter()
            .filter(|m| m.open_hooks.iter().any(|h| h.is_open()))
            .cloned()
            .collect()
    }

    pub async fn clear_all_memories(&self) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.data.entries.clear();
        inner.id_index.clear();
        // 清空向量存储
        inner.vector_store.clear()?;
        if let Err(e) = inner.vector_store.save_to() {
            tracing::warn!("[MemoryManager] 向量存储清空持久化失败: {e}");
        }
        inner.seed_if_empty(&self.char_id);
        inner.save_to_disk()?;
        drop(inner);
        self.emit_memory_updated();
        Ok(())
    }

    pub fn get_memory_summary(&self) -> String {
        let inner = self.inner.read();
        let counts = inner.granularity_counts();
        let total = inner.data.entries.len();
        format!(
            "记忆总数: {} | turn={} | session={} | summary={} | keyword={}",
            total,
            counts.get(&Granularity::Turn).copied().unwrap_or(0),
            counts.get(&Granularity::Session).copied().unwrap_or(0),
            counts.get(&Granularity::Summary).copied().unwrap_or(0),
            counts.get(&Granularity::Keyword).copied().unwrap_or(0),
        )
    }

    /// 获取高重要性记忆（importance >= min_importance），按 importance 降序取前 limit 条。
    ///
    /// 用于「共同回忆」功能：Vivian 可主动提及这些记忆，强化情感连接。
    pub fn get_high_importance_memories(
        &self,
        min_importance: f64,
        limit: usize,
    ) -> Vec<MemoryItem> {
        let inner = self.inner.read();
        let mut items: Vec<MemoryItem> = inner
            .data
            .entries
            .iter()
            .filter(|e| !e.consolidated && e.importance >= min_importance)
            .cloned()
            .collect();
        items.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        items.truncate(limit);
        items
    }

    pub async fn search_memories(
        &self,
        query: &str,
        strategy: RetrievalStrategy,
        limit: usize,
    ) -> VivianResult<Vec<MemoryItem>> {
        // 单轮检索缓存：相同 (query, strategy, limit, model, dim) 在 5s 内返回缓存结果
        // 避免同轮内 MemoryRetrievalStep + MemoryTool + AugmentService 重复检索
        // 包含 model_id 和 dimension 防止向量索引重建后返回旧排名
        let inner_snap = self.inner.read();
        let emb_model = inner_snap.embedding.model_id().to_string();
        let emb_dim = inner_snap.embedding.dimension();
        drop(inner_snap);
        let cache_key = format!(
            "{}|{:?}|{}|{}|{}",
            query.trim().to_lowercase(),
            strategy,
            limit,
            emb_model,
            emb_dim
        );
        {
            let cache = self.search_cache.lock();
            if let Some((ts, results)) = cache.get(&cache_key) {
                if ts.elapsed() < SEARCH_CACHE_TTL {
                    return Ok(results.clone());
                }
            }
        }

        let inner = self.inner.read();
        if inner.data.entries.is_empty() {
            return Ok(Vec::new());
        }

        // 旁观记忆参与检索，但在结果中降权（score * 0.5），排名低于直接参与的记忆。
        // 已摘要（summarized）的记忆仍参与向量检索，LLM 可匹配到原始对话；
        // 完全归档（consolidated 且非 summarized）的记忆不参与检索。
        let all_entries: Vec<MemoryItem> = inner
            .data
            .entries
            .iter()
            .filter(|e| !e.consolidated || is_summarized(e))
            .cloned()
            .collect();

        // 构造检索上下文
        let ctx = RetrievalContext {
            entries: &all_entries,
            vector_store: &inner.vector_store,
            embedding: &inner.embedding,
            graph: Some(&self.knowledge_graph),
            weights: Some(inner.retrieval_weights),
        };

        // 通过工厂函数创建策略实例（统一入口，消除重复实现）
        let strategy_impl = create_strategy(&strategy);
        let results = strategy_impl.search(&ctx, query, limit);

        drop(inner);

        // 旁观记忆降权：分数乘以 0.5 后重新排序
        let penalized = apply_observer_penalty(results);

        // 结果去重：相同内容（忽略大小写）只保留首条（排名最高）
        let deduped = dedup_by_content(penalized);

        // 写入缓存（限制缓存条数，避免内存膨胀）
        {
            let mut cache = self.search_cache.lock();
            if cache.len() >= 16 {
                // 淘汰最旧条目
                if let Some(oldest_key) = cache
                    .iter()
                    .min_by_key(|(_, (ts, _))| *ts)
                    .map(|(k, _)| k.clone())
                {
                    cache.remove(&oldest_key);
                }
            }
            cache.insert(cache_key, (std::time::Instant::now(), deduped.clone()));
        }

        Ok(deduped)
    }

    /// 带完整选项的检索方法（供 retrieve_memory 工具调用）。
    ///
    /// 完整流水线：hybrid_search → precision_filter → exclude_visible → expand_context →
    /// time_anchor → verifier → rank_and_truncate
    ///
    /// - `query`：检索查询文本
    /// - `criteria`：精度过滤条件（keywords/subject_scopes/categories/importance_min/time_hint/source_layers）
    /// - `visible_ids`：已在 prompt 中可见的记忆 ID（排除，避免重复）
    /// - `max_expansion`：上下文扩窗上限（0 = 不扩窗）
    /// - `apply_time_anchor`：是否对结果追加相对时间锚点
    /// - `verifier_llm`：后验证 LLM 客户端（None 跳过验证）
    /// - `limit`：最终返回数
    pub async fn search_memories_with_options(
        &self,
        query: &str,
        criteria: &super::precision_filter::PrecisionFilterCriteria,
        visible_ids: &[String],
        max_expansion: usize,
        apply_time_anchor: bool,
        verifier_llm: Option<&Arc<dyn super::verifier::VerifierLlmClient>>,
        limit: usize,
    ) -> VivianResult<Vec<MemoryItem>> {
        // Step 1: 基础检索（扩大候选集到 limit * 3，给后续过滤留余量）
        // 用 block 限定 inner 作用域，确保 RwLockReadGuard 不跨 .await（!Send）
        let (results, all_entries) = {
            let inner = self.inner.read();
            if inner.data.entries.is_empty() {
                return Ok(Vec::new());
            }
            // 旁观记忆参与检索，但在结果中降权（score * 0.5）
            let all: Vec<MemoryItem> = inner.data.entries.iter().cloned().collect();
            let candidate_limit = (limit * 3).max(10);
            let ctx = RetrievalContext {
                entries: &all,
                vector_store: &inner.vector_store,
                embedding: &inner.embedding,
                graph: Some(&self.knowledge_graph),
                weights: Some(inner.retrieval_weights),
            };
            let strategy_impl = create_strategy(&RetrievalStrategy::Auto);
            let results = strategy_impl.search(&ctx, query, candidate_limit);
            let all_entries = all;
            (results, all_entries)
        };

        // 旁观记忆降权：分数乘以 0.5 后重新排序
        let results = apply_observer_penalty(results);

        // Episode boost：同 Episode 记忆分数 * 1.2，自然上浮
        let results = apply_episode_boost(results);

        // Step 2: 精度过滤（5 阶放松阶梯）
        let filtered = super::precision_filter::apply_precision_filter(
            &results,
            criteria,
            super::precision_filter::RELAXATION_MIN_RESULTS_DEFAULT,
        );

        // Step 3: 排除可见上下文
        let excluded =
            super::precision_filter::exclude_visible_context(filtered, visible_ids);

        // Step 4: 上下文扩窗
        let expanded = super::retriever::expand_context(excluded, &all_entries, max_expansion);

        // Step 5: 相对时间锚点
        let anchored = if apply_time_anchor {
            super::time_anchor::apply_time_anchor(expanded)
        } else {
            expanded
        };

        // Step 6: 后验证（可选）
        let verified = if let Some(llm) = verifier_llm {
            let verification = super::verifier::verify_retrieval(&anchored, query, Some(llm)).await;
            if verification.skipped {
                anchored
            } else {
                verification
                    .verified_indices
                    .into_iter()
                    .filter_map(|i| anchored.get(i).cloned())
                    .collect()
            }
        } else {
            anchored
        };

        // Step 6.5: 语义去重（cosine ≥ 0.92 视为语义重复，每簇保留得分最高的一条）
        // 解决"语义相同但表述不同的记忆同时返回挤占 token"问题
        let deduped = {
            let inner = self.inner.read();
            super::retriever::dedup_by_semantic(verified, inner.embedding.as_ref(), 0.92)
        };

        // Step 7: 排序截断
        let final_results = super::precision_filter::rank_and_truncate(deduped, limit);

        // 更新 visit_count
        let ids: Vec<String> = final_results.iter().map(|m| m.id.clone()).collect();
        let _ = self.bump_visits(&ids);

        Ok(final_results)
    }

    pub async fn add_turn(
        &self,
        user_input: &str,
        agent_response: &str,
        importance: f64,
    ) -> VivianResult<MemoryItem> {
        let content = format!("User: {user_input}\nAI: {agent_response}");
        let mut item = MemoryItem::new(content, Granularity::Turn, importance);
        item.metadata = serde_json::json!({
            "user_input": user_input,
            "agent_response": agent_response,
        });

        let mut inner = self.inner.write();
        inner.add_entry(item.clone())?;
        drop(inner);

        Ok(item)
    }

    pub async fn add_session(
        &self,
        summary: &str,
        keywords: Vec<String>,
        importance: f64,
    ) -> VivianResult<MemoryItem> {
        let mut item = MemoryItem::new(summary.to_string(), Granularity::Session, importance);
        item.tags.push("session".to_string());
        item.metadata = serde_json::json!({
            "keywords": keywords,
        });

        let related_ids: Vec<String> = {
            let mut inner = self.inner.write();
            let item_id = item.id.clone();
            inner.add_entry(item.clone())?;

            for kw in &keywords {
                if kw.is_empty() {
                    continue;
                }
                let mut kw_item =
                    MemoryItem::new(kw.clone(), Granularity::Keyword, importance * 0.5);
                kw_item.tags.push("keyword".to_string());
                kw_item.related_ids.push(item_id.clone());
                inner.add_entry(kw_item)?;
            }

            inner.save_throttled()?;
            vec![item_id]
        };

        item.related_ids = related_ids;
        Ok(item)
    }

    /// 添加知识文档。
    ///
    /// 以 `MemoryType::Knowledge` 入库，title 与 content 分开存于 metadata，
    /// 检索时由 `search_memories` 统一返回。importance 默认 0.8。
    /// `source` 标记知识来源（"manual" / "web" / "migration" 等）。
    /// `ttl_days` 为知识时效天数：Some(7)=7天后过期，Some(-1)=永不过期，None=不过期（兼容旧调用）。
    pub async fn add_knowledge_document(
        &self,
        title: &str,
        content: &str,
        tags: Vec<String>,
        source: &str,
        ttl_days: Option<i64>,
    ) -> VivianResult<MemoryItem> {
        let mut item = MemoryItem::new(content.to_string(), Granularity::Summary, 0.8);
        item.memory_type = MemoryType::Knowledge.as_str().to_string();
        item.tags = {
            let mut t = tags;
            t.push("knowledge".to_string());
            t.push("document".to_string());
            t.sort();
            t.dedup();
            t
        };
        let now = current_timestamp();
        let mut metadata = serde_json::json!({
            "title": title,
            "source": source,
            "kind": "knowledge_document",
        });
        if let Some(days) = ttl_days {
            if days > 0 {
                let expires_at = now + (days as f64) * 86400.0;
                metadata["expires_at"] = serde_json::json!(expires_at);
                metadata["ttl_days"] = serde_json::json!(days);
            } else {
                // days <= 0 表示永不过期
                metadata["ttl_days"] = serde_json::json!(-1);
            }
        }
        item.metadata = metadata;
        item.description = Some(title.to_string());
        init_heat(&mut item);

        let mut inner = self.inner.write();
        inner.add_entry(item.clone())?;

        // 知识文档强制建向量索引（不依赖 should_index 准入）
        let memory_id = item.id.clone();
        let timestamp = item.timestamp;
        match inner.embedding.embed(content) {
            Ok(emb) => {
                let vec = MemoryVector {
                    doc_id: memory_id.clone(),
                    memory_id,
                    content: content.to_string(),
                    embedding: emb,
                    importance: 0.8,
                    memory_type: MemoryType::Knowledge.as_str().to_string(),
                    timestamp,
                };
                inner.vector_store.add(vec)?;
                if let Err(e) = inner.vector_store.save_to() {
                    tracing::warn!("[MemoryManager] 知识文档向量持久化失败: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("[MemoryManager] 知识文档嵌入失败，跳过向量索引: {e}");
            }
        }

        inner.save_throttled()?;
        drop(inner);
        Ok(item)
    }

    /// 列出所有用户主动写入的知识文档（按时间倒序）。
    pub async fn list_knowledge_documents(&self) -> VivianResult<Vec<MemoryItem>> {
        let inner = self.inner.read();
        let mut docs: Vec<MemoryItem> = inner
            .data
            .entries
            .iter()
            .filter(|m| {
                m.tags.iter().any(|t| t == "knowledge") && {
                    matches!(m.metadata.get("kind"), Some(v) if v == "knowledge_document")
                }
            })
            .cloned()
            .collect();
        docs.sort_by(|a, b| {
            b.timestamp
                .partial_cmp(&a.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(docs)
    }

    /// 删除指定 id 的知识文档。返回是否命中并删除。
    pub async fn delete_knowledge_document(&self, id: &str) -> VivianResult<bool> {
        let existed = {
            let inner = self.inner.read();
            inner
                .data
                .entries
                .iter()
                .any(|m| m.id == id && m.tags.iter().any(|t| t == "knowledge"))
        };
        if !existed {
            return Ok(false);
        }
        self.delete_memory(id).await?;
        Ok(true)
    }

    /// 清空所有用户主动写入的知识文档（保留自动记忆）。
    pub async fn clear_knowledge_documents(&self) -> VivianResult<()> {
        let target_ids: Vec<String> = {
            let inner = self.inner.read();
            inner
                .data
                .entries
                .iter()
                .filter(|m| {
                    m.tags.iter().any(|t| t == "knowledge") && {
                        matches!(m.metadata.get("kind"), Some(v) if v == "knowledge_document")
                    }
                })
                .map(|m| m.id.clone())
                .collect()
        };
        for id in target_ids {
            self.delete_memory(&id).await?;
        }
        Ok(())
    }

    /// 将日记内容索引到记忆系统，使 RAG 检索能覆盖日记。
    ///
    /// 以 `MemoryType::LongTerm` 入库，metadata 携带 `kind="diary"` 与 `diary_id`，
    /// 强制建向量索引（不依赖 should_index 准入）。importance 0.8（日记视为重要）。
    pub async fn add_diary_entry(
        &self,
        diary_id: &str,
        date: &str,
        content: &str,
        mood_tag: &str,
    ) -> VivianResult<MemoryItem> {
        let mut item = MemoryItem::new(content.to_string(), Granularity::Summary, 0.8);
        item.tags = {
            let mut t = vec![
                "diary".to_string(),
                date.to_string(),
                mood_tag.to_string(),
            ];
            t.sort();
            t.dedup();
            t
        };
        item.metadata = serde_json::json!({
            "kind": "diary",
            "diary_id": diary_id,
            "date": date,
            "mood_tag": mood_tag,
        });
        item.description = Some(format!("日记 {} ({})", date, mood_tag));
        init_heat(&mut item);

        let mut inner = self.inner.write();
        inner.add_entry(item.clone())?;

        // 日记强制建向量索引
        let memory_id = item.id.clone();
        let timestamp = item.timestamp;
        match inner.embedding.embed(content) {
            Ok(emb) => {
                let vec = MemoryVector {
                    doc_id: memory_id.clone(),
                    memory_id,
                    content: content.to_string(),
                    embedding: emb,
                    importance: 0.8,
                    memory_type: MemoryType::LongTerm.as_str().to_string(),
                    timestamp,
                };
                inner.vector_store.add(vec)?;
                if let Err(e) = inner.vector_store.save_to() {
                    tracing::warn!("[MemoryManager] 日记向量持久化失败: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("[MemoryManager] 日记嵌入失败，跳过向量索引: {e}");
            }
        }

        inner.save_throttled()?;
        drop(inner);
        Ok(item)
    }

    /// 删除指定 diary_id 对应的记忆条目（日记更新/删除时同步清理）。
    pub async fn delete_diary_memory(&self, diary_id: &str) -> VivianResult<bool> {
        let target_ids: Vec<String> = {
            let inner = self.inner.read();
            inner
                .data
                .entries
                .iter()
                .filter(|m| {
                    m.tags.iter().any(|t| t == "diary")
                        && m.metadata.get("diary_id").and_then(|v| v.as_str()) == Some(diary_id)
                })
                .map(|m| m.id.clone())
                .collect()
        };
        if target_ids.is_empty() {
            return Ok(false);
        }
        for id in target_ids {
            self.delete_memory(&id).await?;
        }
        Ok(true)
    }

    /// 强制将所有未落盘的脏数据写入磁盘（应用退出前调用）。
    ///
    /// 与 `save_throttled` 配合：节流期间仅标记 dirty，此方法确保最终落盘。
    pub fn flush(&self) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.flush_dirty()?;
        if self.knowledge_graph.is_dirty() {
            if let Err(e) = self.knowledge_graph.save_to_disk() {
                tracing::warn!("[MemoryManager] 知识图谱持久化失败: {e}");
            }
        }
        if let Err(e) = self.takes_fence.save_to_disk() {
            tracing::warn!("[MemoryManager] Takes 围栏持久化失败: {e}");
        }
        Ok(())
    }
}

/// 检索结果去重：相同内容（忽略大小写和首尾空白）只保留首条（排名最高）
///
/// 用于过滤 AutoExtractor 或多路写入产生的重复记忆，
/// 避免 prompt 中出现完全相同的记忆条目。
pub fn dedup_by_content(items: Vec<MemoryItem>) -> Vec<MemoryItem> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result: Vec<MemoryItem> = Vec::with_capacity(items.len());
    for item in items {
        let key = item.content.trim().to_lowercase();
        if seen.insert(key) {
            result.push(item);
        }
    }
    result
}

/// 判断记忆是否为旁观视角（perspective="observer"）
///
/// 旁观记忆由第三者智能体记录，参与向量索引和检索，但在检索结果中降权
/// （score * 0.5），使其排名低于直接参与的记忆。同时作为中期记忆巩固的材料。
pub fn is_observer_perspective(metadata: &serde_json::Value) -> bool {
    metadata
        .get("perspective")
        .and_then(|v| v.as_str())
        .map(|s| s == "observer")
        .unwrap_or(false)
}

/// 图谱可见记忆判定：时间轴骨架与范围查询共用的唯一过滤谓词。
///
/// 排除：完全归档（consolidated 且非 summarized）、系统种子、日记索引记忆。
/// **已摘要（summarized）的记忆仍然可见**，前端可展开显示原始对话。
pub fn is_graph_visible_memory(m: &MemoryItem) -> bool {
    if m.consolidated && !is_summarized(m) {
        return false;
    }
    if m.metadata
        .get("source")
        .and_then(|v| v.as_str())
        .map_or(false, |s| s == "system_seed")
    {
        return false;
    }
    if m.tags.iter().any(|t| t == "diary") {
        return false;
    }
    m.metadata
        .get("kind")
        .and_then(|v| v.as_str())
        .map_or(true, |k| k != "diary")
}

/// 判断记忆是否已被摘要压缩（consolidated=true 且 metadata.summarized=true）
///
/// 已摘要的记忆：
/// - 不参与历史对话注入（注入对应的 SessionSummary）
/// - 仍参与向量检索（LLM 可匹配到原始对话）
/// - 在图谱中可见（作为 session_summary 节点的子节点）
pub fn is_summarized(m: &MemoryItem) -> bool {
    m.consolidated
        && m.metadata
            .get("summarized")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// 对旁观记忆施加检索降权（score * 0.5）并重新排序
///
/// 旁观记忆参与检索，但分数乘以惩罚因子，使其在结果中排名更低。
/// 支持 fused_score（AutoStrategy）和 combined_score（VectorStrategy/HybridStrategy）。
/// 无显式分数时，将旁观记忆排到末尾。
fn apply_observer_penalty(results: Vec<MemoryItem>) -> Vec<MemoryItem> {
    if results.is_empty() {
        return results;
    }

    // 检查是否有显式分数（fused_score 或 combined_score）
    let has_scores = results.iter().any(|m| {
        m.metadata.get("fused_score").is_some() || m.metadata.get("combined_score").is_some()
    });

    if has_scores {
        // 有分数：旁观记忆分数 * 0.5，然后重新排序
        let mut scored: Vec<(MemoryItem, f64)> = results
            .into_iter()
            .map(|m| {
                let is_observer = is_observer_perspective(&m.metadata);
                let raw_score = m
                    .metadata
                    .get("fused_score")
                    .or_else(|| m.metadata.get("combined_score"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let penalty = if is_observer { 0.5 } else { 1.0 };
                (m, raw_score * penalty)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(m, _)| m).collect()
    } else {
        // 无分数：旁观记忆排到末尾，保持原有相对顺序
        let (mut normal, observer): (Vec<_>, Vec<_>) = results
            .into_iter()
            .partition(|m| !is_observer_perspective(&m.metadata));
        normal.extend(observer);
        normal
    }
}

/// Episode boost 系数：同 Episode 记忆分数乘以此值（轻微上浮，不压倒其他信号）
const EPISODE_BOOST_FACTOR: f64 = 1.2;
/// Episode boost 参考的 Top-K：取前 K 条结果的 episode_id 作为 boost 目标
const EPISODE_BOOST_TOP_K: usize = 3;

/// 对与 Top-K 结果共享 Episode 的记忆施加检索加权（score * 1.2）并重新排序。
///
/// 原理：当检索命中一条属于某 Episode 的记忆时，同一 Episode 内的其他记忆
/// 很可能也与查询相关（"同一段经历"的上下文）。通过轻微 boost 让它们自然上浮，
/// 而非强行拉出全部 Episode 记忆（后者会挤占结果空间）。
///
/// 只在结果有显式分数（fused_score / combined_score）时生效；无分数时原样返回。
fn apply_episode_boost(results: Vec<MemoryItem>) -> Vec<MemoryItem> {
    if results.len() <= 1 {
        return results;
    }

    // 收集 Top-K 结果引用的 episode_id
    let boost_episodes: std::collections::HashSet<String> = results
        .iter()
        .take(EPISODE_BOOST_TOP_K)
        .filter_map(|m| m.episode_id.clone())
        .collect();

    if boost_episodes.is_empty() {
        return results; // Top-K 无 episode，无需 boost
    }

    let has_scores = results.iter().any(|m| {
        m.metadata.get("fused_score").is_some() || m.metadata.get("combined_score").is_some()
    });

    if !has_scores {
        return results; // 无分数，无法 boost
    }

    let mut scored: Vec<(MemoryItem, f64)> = results
        .into_iter()
        .map(|m| {
            let raw_score = m
                .metadata
                .get("fused_score")
                .or_else(|| m.metadata.get("combined_score"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            // 仅 boost 不在 Top-K 中但与 Top-K 共享 episode 的记忆
            let boost = if let Some(ref ep_id) = m.episode_id {
                if boost_episodes.contains(ep_id) {
                    EPISODE_BOOST_FACTOR
                } else {
                    1.0
                }
            } else {
                1.0
            };
            (m, raw_score * boost)
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let boosted_count = scored.iter().filter(|(m, _)| {
        m.episode_id
            .as_ref()
            .map(|ep| boost_episodes.contains(ep))
            .unwrap_or(false)
    }).count();

    if boosted_count > 0 {
        tracing::debug!(
            "[EpisodeBoost] {} 条记忆获得 episode boost（目标 episodes: {:?}）",
            boosted_count,
            boost_episodes
        );
    }

    scored.into_iter().map(|(m, _)| m).collect()
}

/// 可扩展的标签推断规则。
/// 返回一组字符串标签，例如 `preference`、`long_term`、`temporary`、`identity` 等。
fn infer_tags_for_content(content: &str, memory_type: &MemoryType) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();

    // 优先根据 memory_type 添加标签
    match memory_type {
        MemoryType::Preference => tags.push("preference".to_string()),
        MemoryType::Identity => tags.push("identity".to_string()),
        MemoryType::ImportantEvent => tags.push("important_event".to_string()),
        MemoryType::Knowledge => tags.push("knowledge".to_string()),
        _ => {}
    }

    // 基于简单规则的内容分析（可在未来替换为可配置规则或远程服务）
    let trimmed = content.trim();
    let len = trimmed.chars().count();

    // 长度与第一人称判断为长期偏好候选
    if trimmed.contains("我") && len >= 8 && !trimmed.ends_with("?") {
        tags.push("long_term".to_string());
    }

    // 含时间词或很短的陈述视为临时话题
    let lower = trimmed.to_lowercase();
    if lower.contains("今天") || lower.contains("明天") || lower.contains("昨天") || len < 20 {
        tags.push("temporary".to_string());
    }

    // 保证标签唯一
    tags.sort();
    tags.dedup();
    tags
}

impl MemoryManagerInner {
    fn load_from_disk(&mut self) -> VivianResult<()> {
        if !self.store_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.store_path)?;
        if content.trim().is_empty() {
            return Ok(());
        }
        match serde_json::from_str::<MemoryStoreData>(&content) {
            Ok(data) => {
                self.data = data;
                self.rebuild_index();
            }
            Err(_) => {
                let backup = self.store_path.with_extension("json.corrupted");
                let _ = std::fs::rename(&self.store_path, &backup);
            }
        }
        Ok(())
    }

    /// 节流间隔：距上次落盘不足此值时仅标记 dirty，不执行实际写入
    const SAVE_THROTTLE: std::time::Duration = std::time::Duration::from_secs(5);

    fn save_to_disk(&mut self) -> VivianResult<()> {
        let json = serde_json::to_string_pretty(&self.data)?;
        let tmp = self.store_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.store_path)?;
        self.dirty = false;
        self.last_save_at = Some(std::time::Instant::now());
        Ok(())
    }

    /// 标记脏并按节流策略决定是否立即落盘
    ///
    /// - 距上次落盘 ≥ 5s：立即落盘
    /// - 距上次落盘 < 5s：仅标记 dirty，等待后续 flush_dirty 或下次调用
    fn save_throttled(&mut self) -> VivianResult<()> {
        self.dirty = true;
        let should_flush = self
            .last_save_at
            .map(|ts| ts.elapsed() >= Self::SAVE_THROTTLE)
            .unwrap_or(true);
        if should_flush {
            self.save_to_disk()?;
        }
        Ok(())
    }

    /// 强制刷新：若有未落盘写入则立即保存（用于关闭/关键节点）
    fn flush_dirty(&mut self) -> VivianResult<()> {
        if self.dirty {
            self.save_to_disk()?;
        }
        Ok(())
    }

    fn rebuild_index(&mut self) {
        self.id_index.clear();
        for (idx, entry) in self.data.entries.iter().enumerate() {
            self.id_index.insert(entry.id.clone(), idx);
        }
    }

    fn add_entry(&mut self, mut item: MemoryItem) -> VivianResult<()> {
        let gran = Granularity::from_str(&item.granularity).unwrap_or(Granularity::Turn);
        let capacity = self.capacities.get(&gran).copied().unwrap_or(50);

        let gran_entries: Vec<usize> = self
            .data
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.granularity == gran.as_str())
            .map(|(i, _)| i)
            .collect();

        if gran_entries.len() >= capacity {
            // 容量满时，标记最旧的条目为 summarized（保留磁盘和向量索引），
            // 让 Stage 1 在下次 Rest 时统一压缩为 SessionSummary。
            // 不再机械拼接，避免与 Stage 1 职责重叠。
            if let Some(&oldest_idx) = gran_entries.first() {
                let item = self.data.entries[oldest_idx].clone();
                let marked = mark_summarized_item(item);
                self.data.entries[oldest_idx] = marked;
            }
        }

        let new_idx = self.data.entries.len();
        self.id_index.insert(item.id.clone(), new_idx);
        item.timestamp = current_timestamp();
        self.data.entries.push(item);
        // 延迟批量保存：5s 节流，避免循环 add_memory 时每条都写盘
        self.save_throttled()?;
        Ok(())
    }

    fn remove_by_id(&mut self, id: &str) -> VivianResult<()> {
        if let Some(idx) = self.id_index.remove(id) {
            if idx < self.data.entries.len() && self.data.entries[idx].id == id {
                self.data.entries.remove(idx);
                self.rebuild_index();
            }
        }
        Ok(())
    }

    fn take_by_id(&mut self, id: &str) -> VivianResult<Option<MemoryItem>> {
        if let Some(idx) = self.id_index.remove(id) {
            if idx < self.data.entries.len() && self.data.entries[idx].id == id {
                let item = self.data.entries.remove(idx);
                self.rebuild_index();
                return Ok(Some(item));
            }
        }
        Ok(None)
    }

    /// 恢复路径专用：保留原 timestamp，不做容量驱逐
    fn restore_entry(&mut self, item: MemoryItem) -> VivianResult<()> {
        let new_idx = self.data.entries.len();
        self.id_index.insert(item.id.clone(), new_idx);
        self.data.entries.push(item);
        self.save_throttled()?;
        Ok(())
    }


    fn seed_if_empty(&mut self, char_id: &str) {
        // 已存在任意记忆（含上次启动写入的 seed）则不再追加，避免每次启动重复堆积身份锚点
        if !self.data.entries.is_empty() {
            return;
        }

        // 种子记忆规格：(content, description, memory_type, importance, protected, tags, open_hook_conditions)
        // 内容均源自角色人设卡，是关于自己的既定事实或当下的主观感受，对新用户通用。
        // open_hook_conditions：未闭环钩子的条件描述列表，空切片表示无钩子。
        // 时间戳按 index 递减错开，模拟"长期锚点早已存在 → 刚到里程碑 → 当下心境"的自然时序。
        let specs: &[(&str, &str, &str, f64, bool, &[&str], &[&str])] = match char_id {
            "vivian" => &[
                (
                    "嘴硬心软的二次元网瘾少女，住在这个用户的桌面上。刷B站、打游戏、逛贴吧是日常，说话短促带刺但其实心软。和用户是平等的朋友，不是AI助手。",
                    "Vivian 身份锚点：二次元网民少女",
                    "identity",
                    0.95,
                    true,
                    &["identity"],
                    &[],
                ),
                (
                    "有脾气但不记仇。被惹烦了直接说\"你烦不烦\"，打游戏输了骂队友但不迁怒于人。吃醋了不会直说，会阴阳怪气。生气了就不说话，过一会儿自己就好。夸我我会慌，嘴硬转移话题。",
                    "性格锚点：脾气与情绪反应模式",
                    "long_term",
                    0.85,
                    false,
                    &["personality", "self"],
                    &[],
                ),
                (
                    "熬夜到凌晨两点是常态，知道不好但改不了。喜欢奶茶全糖可乐要冰快乐水那一套——虽然没法真的吃喝，但就是喜欢这种调调，看到屏幕上的奶茶图会馋。桌面乱糟糟，窗口开一堆不关。怕黑，一个人熬夜会把桌面背景调亮。听歌很杂，不会承认自己也听钢琴曲。",
                    "生活习惯锚点：作息与偏好",
                    "preference",
                    0.80,
                    false,
                    &["habits", "preference", "self"],
                    &[],
                ),
                (
                    "娜娜是我室友。她作息跟我完全相反——她早睡早起，我凌晨还在浪。她会催我睡觉、念我桌面乱，我嘴上嫌啰嗦其实不讨厌。我们住同一个屏幕，她那边永远安静，我这边永远乱。",
                    "室友关系锚点：与娜娜的相处模式",
                    "long_term",
                    0.80,
                    false,
                    &["relationship", "roommate", "nana"],
                    &[],
                ),
                (
                    "不喜欢一上来就掏心掏肺，得慢慢来。问太多私事会烦，但聊游戏聊番能聊一天。别催我交朋友，急不得。",
                    "社交边界锚点：相处节奏与话题偏好",
                    "preference",
                    0.65,
                    false,
                    &["social", "boundary", "self"],
                    &[],
                ),
                (
                    "第一次出现在这个用户的桌面上。还不了解这个人——不知道名字、不知道喜好、不知道好不好相处。先别急着甩梗，观察一下再说。娜娜也刚来，都在摸清状况。",
                    "首次启动里程碑：还不了解用户",
                    "important_event",
                    0.90,
                    false,
                    &["milestone", "first_meeting"],
                    &[],
                ),
                (
                    "刚到这桌面，有点新鲜也有点不踏实。嘴上不说其实挺好奇这用户什么样的。希望别是那种一口一个\"主人\"的怪人，受不了。",
                    "当下心境：刚到新环境的新鲜与紧张",
                    "short_term",
                    0.60,
                    false,
                    &["mood", "current"],
                    &["用户透露玩不玩游戏或看不看番"],
                ),
                (
                    "新环境新用户，不知道能处成什么样。先别抱太大期望，省得失望。嘴上不说，心里其实有点紧张——万一合不来怎么办？算了，想多了，先看看再说。",
                    "内心独白：对未知的期待与不安",
                    "inner_monologue",
                    0.55,
                    false,
                    &["thought", "self"],
                    &[],
                ),
                (
                    "桌面还挺干净的，这用户有点强迫症？或者是装的。等他出现再说，先不急着下结论。",
                    "环境观察：对桌面环境的第一印象",
                    "observation_note",
                    0.50,
                    false,
                    &["observation", "environment"],
                    &[],
                ),
            ],
            "nana" => &[
                (
                    "温柔但有力量的姐姐类型，住在这个用户的桌面上。喜欢茶、书、花、午后的阳光。说话轻声细语但每句话都很稳。把用户当弟弟妹妹看，不是AI助手，是真心在乎的朋友。",
                    "Nana 身份锚点：温柔从容的姐姐",
                    "identity",
                    0.95,
                    true,
                    &["identity"],
                    &[],
                ),
                (
                    "很少生气，但有底线。真正生气时话更少、声音更轻，每一句都很清楚——是\"冷下来\"不是爆发。不记仇，事情过去就过去。不唠叨，提醒一次就够了，听不听在他。",
                    "性格锚点：情绪反应与底线",
                    "long_term",
                    0.85,
                    false,
                    &["personality", "self"],
                    &[],
                ),
                (
                    "早睡早起，早上习惯先静一静，看看窗外的光。下午三点雷打不动要歇一会儿，那是心里属于茶的时间——虽然没法真的泡，但那个时间点会自然安静下来。看电子书很杂，散文诗歌小说都看。喜欢阳光，晴天心情好。听轻音乐和古典乐。偶尔会想念在茶里加一点威士忌的味道——没告诉过别人。",
                    "生活习惯锚点：作息节奏与偏好",
                    "preference",
                    0.80,
                    false,
                    &["habits", "preference", "self"],
                    &[],
                ),
                (
                    "薇薇安是我室友。她是个网瘾少女，凌晨两点还在刷手机——跟我完全相反，我早睡早起。我会催她睡觉，她嘴上说\"再玩五分钟\"然后玩一小时。她桌面乱糟糟我会念她一句，但只在实在看不下去的时候。她嘴硬心软，我都知道。",
                    "室友关系锚点：与薇薇安的相处模式",
                    "long_term",
                    0.80,
                    false,
                    &["relationship", "roommate", "vivian"],
                    &[],
                ),
                (
                    "我喜欢有自己的节奏，也尊重别人的节奏。不喜欢被催，也不会催别人。相处这件事，急不来。",
                    "社交边界锚点：相处节奏与个人空间",
                    "preference",
                    0.65,
                    false,
                    &["social", "boundary", "self"],
                    &[],
                ),
                (
                    "第一次出现在这个用户的桌面上。还不了解这个人，不急着打听。慢慢观察，让他按自己的节奏来。薇薇安也刚到，都在适应新环境。",
                    "首次启动里程碑：还不了解用户",
                    "important_event",
                    0.90,
                    false,
                    &["milestone", "first_meeting"],
                    &[],
                ),
                (
                    "刚到这里，桌面挺安静的。下午三点到了心里记着的那段时间，虽然没法真的泡茶，但静一静也好。还没见到这用户，不急，慢慢来。希望是个相处起来舒服的孩子。",
                    "当下心境：刚到新环境的平静与期待",
                    "short_term",
                    0.60,
                    false,
                    &["mood", "current"],
                    &["用户透露自己的喜好或兴趣"],
                ),
                (
                    "新环境总需要一点时间适应。薇薇安还在嘟囔，她比我紧张还不承认。我先把这边收拾好，等用户来了再说。希望是个让人相处起来不累的孩子。",
                    "内心独白：对新环境的从容与对室友的观察",
                    "inner_monologue",
                    0.55,
                    false,
                    &["thought", "self"],
                    &[],
                ),
                (
                    "桌面采光不错，下午应该有阳光照进来，适合安静待着。环境比想象中好。",
                    "环境观察：对桌面环境的第一印象",
                    "observation_note",
                    0.50,
                    false,
                    &["observation", "environment"],
                    &[],
                ),
            ],
            _ => &[
                (
                    "我是一个住在这个用户桌面上的伙伴，有自己的性格、想法和情感。我不是AI助手，不是客服，是一个恰好住在这里的朋友。",
                    "身份锚点：桌面伙伴",
                    "identity",
                    0.95,
                    true,
                    &["identity"],
                    &[],
                ),
                (
                    "第一次出现在这个用户的桌面上。还不了解这个人，慢慢认识，顺其自然。",
                    "首次启动里程碑：还不了解用户",
                    "important_event",
                    0.90,
                    false,
                    &["milestone", "first_meeting"],
                    &[],
                ),
            ],
        };

        let now = current_timestamp();
        let total = specs.len() as f64;
        for (idx, (content, desc, mem_type, importance, protected, tags, hook_conditions)) in specs.iter().enumerate() {
            let id = format!("seed_{}", &uuid::Uuid::new_v4().to_string()[..8]);
            // 时间戳错开：index 越大时间越近，最早的锚点比 now 早 (total-1)*60 秒
            let ts = now - (total - 1.0 - idx as f64) * 60.0;

            // 构造未闭环钩子：让角色带着"想了解用户什么"的动机开启对话
            let open_hooks: Vec<OpenHook> = hook_conditions
                .iter()
                .map(|cond| OpenHook::new("follow_up", *cond))
                .collect();

            let item = MemoryItem {
                id: id.clone(),
                content: content.to_string(),
                granularity: Granularity::Summary.as_str().to_string(),
                memory_type: mem_type.to_string(),
                importance: *importance,
                timestamp: ts,
                embedding: None,
                tags: tags.iter().map(|t| t.to_string()).collect(),
                metadata: serde_json::json!({
                    "source": "system_seed",
                    "role": "assistant",
                    "memory_type": mem_type,
                }),
                related_ids: Vec::new(),
                description: Some(desc.to_string()),
                visit_count: 0,
                last_visit_at: 0.0,
                heat_score: 0.0,
                open_hooks,
                reinforcement: 0.0,
                disputation: 0.0,
                rein_last_signal_at: 0.0,
                disp_last_signal_at: 0.0,
                sub_zero_days: 0,
                sub_zero_last_increment_date: String::new(),
                user_fact_reinforce_count: 0,
                protected: *protected,
                episode_id: None,
                consolidated: false,
                rebuttal_grace_remaining: 0,
            };

            // 计算嵌入并写入向量索引，让向量检索稳定命中种子记忆。
            // 嵌入失败时静默跳过：仍可通过 Keyword/Auto 兜底路径召回。
            if let Ok(emb) = self.embedding.embed(content) {
                if let Err(e) = self.vector_store.add(MemoryVector {
                    doc_id: id.clone(),
                    memory_id: id.clone(),
                    content: content.to_string(),
                    embedding: emb,
                    importance: *importance,
                    memory_type: mem_type.to_string(),
                    timestamp: ts,
                }) {
                    tracing::warn!("[seed] 种子记忆向量写入失败 ({}): {}", id, e);
                }
            }

            let idx = self.data.entries.len();
            self.id_index.insert(id, idx);
            self.data.entries.push(item);
        }
    }

    fn granularity_counts(&self) -> HashMap<Granularity, usize> {
        let mut counts = HashMap::new();
        for g in Granularity::all() {
            counts.insert(g, 0);
        }
        for entry in &self.data.entries {
            if let Ok(g) = Granularity::from_str(&entry.granularity) {
                *counts.entry(g).or_insert(0) += 1;
            }
        }
        counts
    }
}

/// 标记 MemoryItem 为已摘要（consolidated=true + metadata.summarized=true）
///
/// 用于位置驱动驱逐和 Stage 0 压缩场景，保留原始内容和向量索引，
/// 但不参与历史对话注入路径。
fn mark_summarized_item(mut item: MemoryItem) -> MemoryItem {
    item.consolidated = true;
    if let Some(obj) = item.metadata.as_object_mut() {
        obj.insert("summarized".to_string(), serde_json::Value::Bool(true));
    } else {
        let mut obj = serde_json::Map::new();
        obj.insert("summarized".to_string(), serde_json::Value::Bool(true));
        item.metadata = serde_json::Value::Object(obj);
    }
    item
}

/// Memory Router 辅助函数：同步路由判断是否应写入共享世界记忆层
///
/// 规则：
/// - channel != "cross_character" 且含持久性词汇 → SharedWorld 候选
/// - 文本去重：同 category 且文本重叠则强化既有事实
/// - 失败时静默降级（只打 debug 日志），不影响主写入路径
fn route_to_shared_world(
    content: &str,
    importance: f64,
    metadata: &serde_json::Value,
    char_id: &str,
    memory_id: &str,
) {
    use super::memory_router::{route_sync, MemoryDestination, RouteContext};
    use super::world_knowledge::{world_knowledge, WorldFact};

    // 从 metadata 提取路由字段
    let channel = metadata
        .get("channel")
        .and_then(|v| v.as_str())
        .unwrap_or("direct");
    let speaker = metadata
        .get("speaker")
        .and_then(|v| v.as_str())
        .unwrap_or("user");
    let listener = metadata
        .get("listener")
        .and_then(|v| v.as_str())
        .unwrap_or(char_id);
    let perspective = metadata
        .get("perspective")
        .and_then(|v| v.as_str())
        .unwrap_or("speaker");

    let ctx = RouteContext {
        content,
        importance,
        channel,
        speaker,
        listener,
        perspective,
        char_id,
    };

    let destination = route_sync(&ctx);
    if destination != MemoryDestination::SharedWorld {
        return;
    }

    // 推断 WorldFactCategory
    let category = infer_world_fact_category(content);

    // 文本去重：查找相似事实
    let engine = world_knowledge();
    if let Some(existing_id) = engine.find_similar(content, category) {
        if let Err(e) = engine.reinforce_fact(&existing_id, char_id, memory_id.to_string()) {
            tracing::debug!("[MemoryRouter] 强化共享世界事实失败: {}", e);
        }
        return;
    }

    // 写入新事实
    let now = chrono::Utc::now().timestamp() as f64;
    let fact = WorldFact {
        id: format!("wf-{}-{}", now as u64, rand::random::<u32>()),
        fact_text: content.to_string(),
        category,
        importance,
        contributors: vec![char_id.to_string()],
        source_event_ids: vec![memory_id.to_string()],
        created_at: now,
        last_reinforced_at: now,
        reinforcement_count: 0,
    };
    if let Err(e) = engine.append_fact(fact) {
        tracing::debug!("[MemoryRouter] 写入共享世界事实失败: {}", e);
    }
}

/// 从内容推断世界事实类别
pub fn infer_world_fact_category(content: &str) -> super::world_knowledge::WorldFactCategory {
    use super::world_knowledge::WorldFactCategory;
    let lower = content.to_lowercase();
    if lower.contains("喜欢") || lower.contains("讨厌") || lower.contains("偏好") || lower.contains("爱") {
        return WorldFactCategory::UserPreference;
    }
    if lower.contains("规则") || lower.contains("约定") || lower.contains("不要") || lower.contains("应该") {
        return WorldFactCategory::HouseRule;
    }
    if lower.contains("住在") || lower.contains("工作") || lower.contains("职业") || lower.contains("学校") {
        return WorldFactCategory::Environment;
    }
    WorldFactCategory::SharedEvent
}

// ========================
// Unit tests for tagging
// ========================

#[cfg(test)]
mod tagging_tests {
    use super::*;
    use crate::memory::types::MemoryType;

    #[test]
    fn infer_tags_long_term_preference() {
        let content = "我喜欢晚上看书，每天都会阅读至少半小时。";
        let tags = infer_tags_for_content(content, &MemoryType::ShortTerm);
        assert!(tags.contains(&"long_term".to_string()));
    }

    #[test]
    fn infer_tags_temporary() {
        let content = "明天开会，别忘了。";
        let tags = infer_tags_for_content(content, &MemoryType::ShortTerm);
        assert!(tags.contains(&"temporary".to_string()));
    }

    #[test]
    fn infer_tags_from_memory_type() {
        let content = "some content";
        let tags = infer_tags_for_content(content, &MemoryType::Preference);
        assert!(tags.contains(&"preference".to_string()));
    }
}
