use crate::config::manager::AppConfig;
use crate::cross_character::{build_speaker_prefix, parse_any_speaker_prefix};
use crate::error::{VivianError, VivianResult};
use crate::utils::path;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
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
    current_timestamp, Granularity, MemoryItem, MemoryStoreData, MemoryType,
    RetrievalStrategy, SemanticType,
};
use super::vector_search::{should_index, MemoryVector, MemoryVectorStore};
use crate::memory::retriever::{MemoryRetrievalFilter, RetrievalWeights};

/// 单次检索命中时 importance 增量（反馈机制：被检索命中=被验证有用）
const VISIT_IMPORTANCE_DELTA: f64 = 0.05;

/// 允许走 LLM 增强写入的记忆类型（高价值、低频）
///
/// 其余类型（ShortTerm/CasualConversation/General/InnerMonologue 等）为高频低信息，
/// 直接规则化写入，避免写入路径的 LLM 开销随对话量线性增长。
fn should_enrich(memory_type: MemoryType) -> bool {
    matches!(
        memory_type,
        MemoryType::ImportantEvent
            | MemoryType::LongTerm
            | MemoryType::Knowledge
            | MemoryType::User
            | MemoryType::Preference
            | MemoryType::Identity
            | MemoryType::SessionSummary
    )
}

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
    /// 独立精排器（cross-encoder reranker；未配置/失败时回退 NoopReranker）
    reranker: Arc<dyn super::reranker::Reranker>,
    /// 参与精排的候选数上限（召回后仅对前 N 条精排）
    reranker_top_k: usize,
}

/// 主题提示：对话中用户搜索过的关键词，供后台知识采集优先处理
pub struct TopicHint {
    pub query: String,
    pub timestamp: f64,
}

struct MemoryManagerInner {
    data: MemoryStoreData,
    id_index: HashMap<String, usize>,
    capacities: HashMap<Granularity, usize>,
    /// 条目 SQLite 存储（行级 upsert，替代全量 JSON 重写）
    entry_store: super::entry_store::MemoryEntryStore,
    /// 已落盘条目指纹（id → 内容哈希），差异比对确定需写入/删除的行
    persisted: HashMap<String, u64>,
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
        None => true,
    }
}

/// 统一的向量嵌入输入构造：上下文前缀 + 内容 + 描述。
///
/// 播种、向量补建、全量重建三路共用，保证同一条记忆在任何路径下产生
/// 相同的嵌入向量；description 中的关键实体（如"AlenTinn 创造了我"）一并
/// 进入向量空间，提升专名召回。
fn embed_input_for(item: &MemoryItem) -> String {
    let prefix = build_context_prefix(item);
    let desc = item.description.as_deref().unwrap_or("").trim();
    let mut parts: Vec<&str> = Vec::with_capacity(3);
    if !prefix.is_empty() {
        parts.push(&prefix);
    }
    parts.push(item.content.trim());
    if !desc.is_empty() {
        parts.push(desc);
    }
    parts.join("：")
}

/// 构建记忆向量检索的上下文感知前缀
///
/// 将时间戳与说话者/听者背景拼入 embedding 文本，使近似检索能感知
/// "谁在何时对谁说了什么" 的上下文，而非仅匹配孤立内容。
/// 无 speaker 元数据时仅保留日期；日期与说话者都缺失时返回空串，调用方直接使用原文。
fn build_context_prefix(item: &MemoryItem) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(dt) = chrono::DateTime::<chrono::Utc>::from_timestamp(item.timestamp as i64, 0) {
        let local = dt.with_timezone(&chrono::Local);
        parts.push(local.format("%Y-%m-%d").to_string());
    }

    let meta = item.metadata.as_object();
    let speaker = meta
        .and_then(|o| o.get("speaker").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let listener = meta
        .and_then(|o| o.get("listener").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    if !speaker.is_empty() {
        if !listener.is_empty() && listener != speaker {
            parts.push(format!("{}对{}说", speaker, listener));
        } else {
            parts.push(format!("{}说", speaker));
        }
    }

    if parts.is_empty() {
        return String::new();
    }
    format!("[{}]", parts.join(" "))
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
        let vector_store = MemoryVectorStore::open_configured(
            vector_store_path,
            embedding.dimension(),
            embedding.model_id(),
            &config.memory.vector_store,
        )?;
        tracing::info!(
            "[MemoryManager] 向量索引后端: {} (角色{})",
            vector_store.backend_name(),
            char_id
        );

        let mut inner = MemoryManagerInner {
            data: MemoryStoreData::default(),
            id_index: HashMap::new(),
            capacities,
            entry_store: super::entry_store::MemoryEntryStore::open(
                memory_dir.join("entries.db"),
                &store_path,
            )?,
            persisted: HashMap::new(),
            vector_store,
            embedding,
            retrieval_weights: RetrievalWeights::from_config(&config.memory.retrieval_weights),
            dirty: false,
            last_save_at: None,
        };

        inner.load_from_disk()?;
        if let Err(e) = inner.seed_if_empty(char_id) {
            // 种子条目已经写入内存；即使本次嵌入失败，也先落盘，
            // 下次启动时 seed_if_empty 会继续补建缺失向量，不会重复播种。
            let _ = inner.save_to_disk();
            return Err(e);
        }
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
            reranker: super::reranker::build_reranker(config),
            reranker_top_k: config.memory.rerank.top_k,
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

    /// 重建记忆的向量索引（增量/断点续传），返回重建的向量数。
    ///
    /// - 增量：仅重嵌"尚未用当前嵌入模型嵌入"的条目（通过向量库 `model` 列判定），
    ///   已用当前模型嵌入的条目跳过，实现断点续传与渐进迁移。
    /// - 每成功嵌入并写入一条调用一次 `on_embedded`（用于进度上报）。
    /// - 不调 clear()：切换模型后旧向量表已在 reinitialize 时 DROP，`add()` 为
    ///   INSERT OR REPLACE，对重建期间的并发写入安全（新写入记忆自带新向量）。
    pub fn rebuild_all_embeddings(&self, mut on_embedded: impl FnMut()) -> VivianResult<usize> {
        let embedding = self.embedding();
        // 已用当前模型嵌入的 doc_id（增量重建的跳过集合）
        let already_embedded = {
            let inner = self.inner.read();
            inner.vector_store.doc_ids_with_model(embedding.model_id())
        };
        // Step 1: 短持读锁，快照待嵌入条目（含统一嵌入输入），跳过已用当前模型
        // 嵌入的条目（断点续传）；同时记录全量可索引记忆集合供 Step 3 清理孤儿
        let (snapshot, all_indexable_ids): (Vec<MemoryItem>, std::collections::HashSet<String>) = {
            let inner = self.inner.read();
            let all_indexable: Vec<&MemoryItem> =
                inner.data.entries.iter().filter(|e| is_indexable_entry(e)).collect();
            let all_indexable_ids: std::collections::HashSet<String> =
                all_indexable.iter().map(|e| e.id.clone()).collect();
            let snapshot = all_indexable
                .into_iter()
                .filter(|e| !already_embedded.contains(&e.id))
                .cloned()
                .collect();
            (snapshot, all_indexable_ids)
        };
        // Step 2: 锁外批量 embed（分块 + 批量写入），失败块逐条降级重试
        let mut count = 0usize;
        const REBUILD_CHUNK: usize = 16;
        for chunk in snapshot.chunks(REBUILD_CHUNK) {
            let inputs: Vec<String> = chunk.iter().map(embed_input_for).collect();
            let embeddings = match embedding.embed_batch_chunked(
                &inputs,
                REBUILD_CHUNK,
                &(|_, _| {}),
            ) {
                Ok(all) => all,
                Err(e) => {
                    tracing::warn!(
                        "[RebuildEmbeddings] 批量嵌入失败（{} 条），降级逐条重试: {e}",
                        chunk.len()
                    );
                    let mut fallback = Vec::with_capacity(chunk.len());
                    for input in &inputs {
                        match embedding.embed(input) {
                            Ok(emb) => fallback.push(emb),
                            Err(e2) => {
                                tracing::warn!("[RebuildEmbeddings] 嵌入失败: {e2}");
                                fallback.push(Vec::new());
                            }
                        }
                    }
                    fallback
                }
            };
            let mut batch: Vec<MemoryVector> = Vec::with_capacity(chunk.len());
            for (item, emb) in chunk.iter().zip(embeddings) {
                if emb.is_empty() {
                    continue;
                }
                batch.push(MemoryVector {
                    doc_id: item.id.clone(),
                    memory_id: item.id.clone(),
                    content: item.content.clone(),
                    embedding: emb,
                    importance: item.importance,
                    memory_type: item.memory_type.clone(),
                    timestamp: item.timestamp,
                });
            }
            if !batch.is_empty() {
                let n = batch.len();
                let inner = self.inner.read();
                if let Err(e) = inner.vector_store.add_batch(&batch) {
                    tracing::warn!("[RebuildEmbeddings] 批量向量写入失败: {e}");
                    continue;
                }
                count += n;
                for _ in 0..n {
                    on_embedded();
                }
            }
        }
        // Step 3: 清理孤儿向量——删除不在可索引记忆集合中的向量。
        // 避免 hashing→bge-m3 等模型切换后，低重要性种子向量作为孤儿长期残留，
        // 导致 IndexDrift 反复触发、vec0 表重复行累积。
        {
            let inner = self.inner.read();
            let orphan_ids: Vec<String> = inner
                .vector_store
                .all_doc_ids()
                .into_iter()
                .filter(|id| !all_indexable_ids.contains(id))
                .collect();
            for id in orphan_ids {
                let _ = inner.vector_store.remove(&id);
            }
        }
        // Step 4: WAL checkpoint 落盘
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

    /// 计算单条记忆的健康度（0..1），供外部决策使用。
    pub fn memory_health(&self, id: &str) -> f64 {
        let now = current_timestamp();
        let inner = self.inner.read();
        inner
            .data
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| super::lifecycle::health_score(e, now))
            .unwrap_or(0.0)
    }

    /// 全量生命周期健康度评估：为每条记忆写入 `metadata.health` / `metadata.health_grade`，
    /// 并返回 (memory_id, health_score, health_grade) 列表。
    ///
    /// 反馈闭环：检索命中（`bump_visits`）提升 importance 与热度，会反映到后续健康度；
    /// 本方法把健康度持久化到 metadata，供诊断与压缩/归档流水线消费。
    /// 不修改现有删除/归档行为，仅做可观测性增强。
    pub fn evaluate_lifecycle_health(&self) -> Vec<(String, f64, String)> {
        let now = current_timestamp();
        let mut inner = self.inner.write();
        let mut out = Vec::with_capacity(inner.data.entries.len());
        for e in inner.data.entries.iter_mut() {
            let score = super::lifecycle::health_score(e, now);
            let grade = super::lifecycle::HealthGrade::from_score(score);
            let usage = super::lifecycle::usage_term(e, now);
            if let Some(obj) = e.metadata.as_object_mut() {
                obj.insert("health".to_string(), serde_json::json!(score));
                obj.insert("health_grade".to_string(), serde_json::json!(grade.as_str()));
                obj.insert("health_evidence".to_string(), serde_json::json!(usage));
            }
            out.push((e.id.clone(), score, grade.as_str().to_string()));
        }
        if let Err(err) = inner.save_throttled() {
            tracing::warn!("[MemoryManager] 生命周期健康度持久化失败: {err}");
        }
        out
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

    /// 按 ID 列表批量返回记忆（保留传入顺序，去重）。
    ///
    /// 供图谱概念路等"已知 ID 取内容"的场景使用，避免构造查询字符串再检索。
    pub fn get_memories_by_ids(&self, ids: &[String]) -> Vec<MemoryItem> {
        let inner = self.inner.read();
        // 按 id_index 直接定位，O(n) 建索引、O(1) 定位
        let mut out: Vec<MemoryItem> = Vec::with_capacity(ids.len());
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for id in ids {
            if !seen.insert(id.as_str()) {
                continue;
            }
            if let Some(&idx) = inner.id_index.get(id.as_str()) {
                let entry = &inner.data.entries[idx];
                if !entry.consolidated {
                    out.push(entry.clone());
                }
            }
        }
        out
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

        // 纯凭据消息跳过：脱敏后只剩占位符的内容无语义价值，
        // 不写入条目库/向量索引/图谱/事件路由（调用方仍拿到带标记的 item，返回契约不变）
        if super::redact::is_pure_placeholder_content(content) {
            tracing::info!("[MemoryManager] 纯凭据消息跳过记忆写入");
            let mut item = MemoryItem::new(content.to_string(), granularity, importance);
            item.memory_type = memory_type.as_str().to_string();
            if let Some(obj) = item.metadata.as_object_mut() {
                obj.insert("pii_skipped".to_string(), serde_json::Value::Bool(true));
            }
            return Ok(item);
        }

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
        let emb_body = safe_emb_text.as_deref().unwrap_or(content);
        // 上下文感知检索前缀：拼接时间 + 说话者/听者背景，让向量检索感知"何时谁对谁说了什么"
        let context_prefix = build_context_prefix(&item);
        let emb_source = if context_prefix.is_empty() {
            emb_body.to_string()
        } else {
            format!("{}：{}", context_prefix, emb_body)
        };

        // 预计算 embedding（冲突检测 + 向量索引共用，避免重复计算）
        let need_embedding = should_index(importance, &memory_type)
            || super::conflict::should_check_conflict(memory_type);
        let embedding_result = if need_embedding {
            let emb_provider = self.embedding();
            match emb_provider.embed(&emb_source) {
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

        // 种子记忆生命周期管理：当真实记忆积累后，逐步降低种子记忆的 importance
        // 这样种子记忆不会被突然删除，而是在检索排序中自然被真实记忆替代
        {
            let mut inner = self.inner.write();
            inner.decay_seed_memories();
        }

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
        // LLM 增强仅对高价值记忆类型开放；低价值类型直接走规则化写入，
        // 避免高频低信息条目（闲聊/工具调用/独白等）每条消耗一次 LLM 调用
        if !should_enrich(memory_type) {
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
        if let Err(e) = inner.seed_if_empty(&self.char_id) {
            // 清空操作本身已成功；种子条目已写入并落盘。
            // 向量补建若当前失败，会在下次启动时由 MemoryManager::new 继续补建。
            tracing::warn!("[MemoryManager] 种子记忆向量补建未完成，下次启动将继续: {}", e);
        }
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

    /// 带结构化/元数据过滤的检索（企业级预过滤）。
    ///
    /// 在混合检索前按 `filter`（memory_type / 标签 / 时间窗口）对候选记忆做精确过滤，
    /// 使调用方能定向检索特定类型、标签或时间段的记忆。其余行为与 `search_memories` 一致。
    pub async fn search_memories_filtered(
        &self,
        query: &str,
        strategy: RetrievalStrategy,
        limit: usize,
        filter: &MemoryRetrievalFilter,
    ) -> VivianResult<Vec<MemoryItem>> {
        let inner = self.inner.read();
        if inner.data.entries.is_empty() {
            return Ok(Vec::new());
        }
        let all_entries: Vec<MemoryItem> = inner
            .data
            .entries
            .iter()
            .filter(|e| !e.consolidated || is_summarized(e))
            .filter(|e| filter.matches(e))
            .cloned()
            .collect();
        if all_entries.is_empty() {
            return Ok(Vec::new());
        }
        let ctx = RetrievalContext {
            entries: &all_entries,
            vector_store: &inner.vector_store,
            embedding: &inner.embedding,
            graph: Some(&self.knowledge_graph),
            weights: Some(inner.retrieval_weights),
        };
        let strategy_impl = create_strategy(&strategy);
        let results = strategy_impl.search(&ctx, query, limit);
        drop(inner);
        let penalized = apply_observer_penalty(results);
        Ok(dedup_by_content(penalized))
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

        // Step 6.6: 独立精排（cross-encoder reranker）
        // 精排启用且候选 > 1 时，对 top-k 候选按 query-doc 细粒度相关度重排，
        // 覆盖召回阶段的启发式排序；精排后直接按精排顺序截断到 limit。
        // 未启用（回退 NoopReranker）时沿用原有启发式排序截断。
        let final_results: Vec<MemoryItem> =
            if self.reranker.is_active() && deduped.len() > 1 {
                let top_k = self.reranker_top_k.max(1);
                let (head, tail) = if deduped.len() > top_k {
                    let split = top_k.min(deduped.len());
                    (deduped[..split].to_vec(), deduped[split..].to_vec())
                } else {
                    (deduped, Vec::new())
                };
                let scores = self.reranker.rerank(query, &head).await;
                let mut scored: Vec<(MemoryItem, f64)> = head
                    .into_iter()
                    .zip(scores)
                    .map(|(mut m, s)| {
                        if let Some(obj) = m.metadata.as_object_mut() {
                            obj.insert("rerank_score".into(), serde_json::json!(s));
                            obj.insert("reranker".into(), serde_json::json!(self.reranker.name()));
                        }
                        (m, s)
                    })
                    .collect();
                scored.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                });
                let mut reranked: Vec<MemoryItem> = scored.into_iter().map(|(m, _)| m).collect();
                reranked.extend(tail);
                // 精排分数即最终相关度：截断前做 MMR 多样化，近重复条目让位给不同侧面
                let diversified =
                    super::retriever::mmr_diversify(reranked, super::retriever::MMR_LAMBDA, limit);
                diversified.into_iter().take(limit).collect()
            } else {
                // Step 6.7: 综合权重全量排序（不截断，排序结果作为 MMR 的相关度序）
                let ranked = super::precision_filter::rank_only(deduped);
                // Step 7: MMR 多样化挑选 + 截断（未启用精排时的最终选择点）
                super::retriever::mmr_diversify(ranked, super::retriever::MMR_LAMBDA, limit)
                    .into_iter()
                    .take(limit)
                    .collect()
            };

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
        // SQLite 条目库为准；旧版 unified_memory.json 已在 MemoryEntryStore::open 迁移
        let entries = self.entry_store.load_all()?;
        self.data.entries = entries;
        self.data.version = self
            .entry_store
            .get_meta("version")
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        self.rebuild_index();
        self.persisted = super::entry_store::fingerprint_all(&self.data.entries);
        Ok(())
    }

    /// 节流间隔：距上次落盘不足此值时仅标记 dirty，不执行实际写入
    const SAVE_THROTTLE: std::time::Duration = std::time::Duration::from_secs(5);

    /// 差异落盘：对比当前条目指纹与已落盘指纹，仅写入变更行、删除移除行
    ///
    /// 兼容旧调用点（全量保存语义），但实际 IO 为 O(变更条目) 而非 O(全量)。
    fn save_to_disk(&mut self) -> VivianResult<()> {
        let current = super::entry_store::fingerprint_all(&self.data.entries);
        let mut upserts: Vec<(String, MemoryItem)> = Vec::new();
        for entry in &self.data.entries {
            let fp = current.get(&entry.id).copied().unwrap_or(0);
            if self.persisted.get(&entry.id).copied() != Some(fp) {
                upserts.push((entry.id.clone(), entry.clone()));
            }
        }
        let deletes: Vec<String> = self
            .persisted
            .keys()
            .filter(|id| !current.contains_key(*id))
            .cloned()
            .collect();

        if !upserts.is_empty() || !deletes.is_empty() {
            self.entry_store.write_rows(upserts, &deletes)?;
        }
        // version 变化时同步元数据
        if self.data.version.to_string() != self.entry_store.get_meta("version").unwrap_or_default() {
            self.entry_store.set_meta("version", &self.data.version.to_string())?;
        }
        self.persisted = current;
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


    fn seed_if_empty(&mut self, char_id: &str) -> VivianResult<()> {
        // 只在存储中完全没有种子记忆时（首次启动 / 记忆被清空后）才从文件播种。
        // 之后种子记忆连同其向量索引、积累的 visit_count/heat_score 等状态一并持久化，
        // 每次启动不会重建，避免重复计算嵌入并保留检索热度和生命周期状态。
        crate::startup::emit_progress(60, 100, "正在嵌入种子记忆…");
        let has_seed = self.data.entries.iter().any(|m| m.id.starts_with("seed_"));
        if !has_seed {
            self.seed_from_file(char_id);
        }

        // 修复：即使种子条目已存在，也可能因上次播种时嵌入服务不可用/并发超限
        // 导致向量库中缺失种子向量。这里统一补齐，确保“种子记忆注入”包含向量索引。
        self.ensure_seed_vectors()
    }

    /// 补齐缺失的种子记忆向量。
    ///
    /// 种子条目以 `seed_` id 持久化在条目库（entries.db）中，向量则持久化在
    /// `vectors.db` / Qdrant。若恢复出厂设置时嵌入失败，条目会保存但向量缺失；
    /// 重启后 `seed_if_empty` 看到 seed 条目直接跳过，导致种子记忆永远无法召回。
    /// 这里按 doc_id 逐条检查并补建缺失向量，不重复计算已有向量。
    fn ensure_seed_vectors(&self) -> VivianResult<()> {
        let doc_ids = self.vector_store.all_doc_ids();
        let missing_entries: Vec<MemoryItem> = self
            .data
            .entries
            .iter()
            .filter(|m| m.id.starts_with("seed_") && !doc_ids.contains(&m.id))
            .cloned()
            .collect();

        if missing_entries.is_empty() {
            return Ok(());
        }

        let total = missing_entries.len();
        crate::startup::emit_progress(60, 100, &format!("正在嵌入种子记忆… 0/{}", total));

        // 批量嵌入（分块 + 进度上报），失败块降级逐条重试
        let inputs: Vec<String> = missing_entries.iter().map(embed_input_for).collect();
        const SEED_CHUNK: usize = 16;
        let embeddings = match self.embedding.embed_batch_chunked(
            &inputs,
            SEED_CHUNK,
            &|done, total| {
                crate::startup::emit_progress(
                    60,
                    100,
                    &format!("正在嵌入种子记忆… {}/{}", done, total),
                );
            },
        ) {
            Ok(all) => all,
            Err(e) => {
                tracing::warn!(
                    "[seed] 批量嵌入失败，降级逐条重试: {}（剩余 {} 条）",
                    e,
                    total
                );
                let mut fallback = Vec::with_capacity(total);
                for input in &inputs {
                    match self.embedding.embed(input) {
                        Ok(emb) => fallback.push(emb),
                        Err(e2) => {
                            tracing::warn!("[seed] 种子记忆嵌入失败: {}", e2);
                            fallback.push(Vec::new());
                        }
                    }
                }
                fallback
            }
        };

        let mut batch: Vec<MemoryVector> = Vec::with_capacity(total);
        let mut missing = 0usize;
        for (item, emb) in missing_entries.iter().zip(embeddings) {
            if emb.is_empty() {
                missing += 1;
                continue;
            }
            batch.push(MemoryVector {
                doc_id: item.id.clone(),
                memory_id: item.id.clone(),
                content: item.content.clone(),
                embedding: emb,
                importance: item.importance,
                memory_type: item.memory_type.clone(),
                timestamp: item.timestamp,
            });
        }

        let repaired = batch.len();
        if !batch.is_empty() {
            if let Err(e) = self.vector_store.add_batch(&batch) {
                tracing::warn!("[seed] 种子记忆向量批量写入失败: {}", e);
                missing += batch.len();
            } else if let Err(e) = self.vector_store.save_to() {
                tracing::warn!("[seed] 种子向量补建后持久化失败: {}", e);
            }
        }

        if repaired > 0 {
            tracing::info!("[seed] 已补建 {} 条种子记忆向量", repaired);
        }

        if missing > 0 {
            return Err(VivianError::Memory(format!(
                "种子记忆向量未就绪，仍缺少 {} 条",
                missing
            )));
        }
        Ok(())
    }

    /// 种子长叙事切块的最大字符数：超过则按句子切块，提升专名/关键词的检索密度。
    const SEED_CHUNK_MAX_CHARS: usize = 60;

    /// 从文件加载种子记忆并写入。
    fn seed_from_file(&mut self, char_id: &str) {

        // 从 prompt 文件加载种子记忆定义（方向三：代码与内容解耦）
        // 种子记忆只记录 system prompt 无法表达的内容：此刻的状态、感受、动机。
        // 身份/性格/关系/习惯由 system prompt 定义，不在此重复（方向一：消除重复）。
        // 首次启动额外播种"环境预设"种子（environment_presets.md）：只描述环境与
        // 自身处境、不假设用户信息，作为冷启动的环境上下文兜底。
        let mut specs = Self::parse_seed_file(char_id);
        specs.extend(Self::parse_environment_presets(char_id));
        if specs.is_empty() {
            tracing::warn!("[seed] 角色 {} 的种子记忆文件为空或不存在", char_id);
            return;
        }
        tracing::info!("[seed] 角色 {} 从文件解析到 {} 条种子记忆（含环境预设），开始播种", char_id, specs.len());

        let now = current_timestamp();
        let total = specs.len() as f64;
        for (idx, spec) in specs.iter().enumerate() {
            // 时间戳错开：index 越大时间越近，最早的锚点比 now 早 (total-1)*60 秒
            let ts = now - (total - 1.0 - idx as f64) * 60.0;
            // 长叙事切块：短块内词频密度更高，向量余弦与 BM25 都更易命中关键实体，
            // 避免长文本把稀有专名（如人名"AlenTinn"）稀释到无法召回。
            let chunks = Self::chunk_seed_content(&spec.content, Self::SEED_CHUNK_MAX_CHARS);
            let chunk_total = chunks.len();

            for (ci, chunk) in chunks.iter().enumerate() {
                let id = format!("seed_{}", &uuid::Uuid::new_v4().to_string()[..8]);
                let cross_char = spec.tags.iter().any(|t| t == "cross_character");

                let mut metadata = if cross_char {
                    let listener = if char_id == "vivian" { "nana" } else { "vivian" };
                    serde_json::json!({
                        "source": spec.source,
                        "role": "assistant",
                        "memory_type": spec.memory_type,
                        "channel": "cross_character",
                        "speaker": char_id,
                        "listener": listener,
                        "perspective": "speaker",
                    })
                } else {
                    serde_json::json!({
                        "source": spec.source,
                        "role": "assistant",
                        "memory_type": spec.memory_type,
                    })
                };
                // 标记该条目是种子长文本的切块，供检索/图谱识别与调试
                if chunk_total > 1 {
                    if let Some(obj) = metadata.as_object_mut() {
                        obj.insert("seed_chunk".into(), serde_json::json!(true));
                        obj.insert("chunk_index".into(), serde_json::json!(ci));
                        obj.insert("chunk_total".into(), serde_json::json!(chunk_total));
                    }
                }

                let item = MemoryItem {
                    id: id.clone(),
                    content: chunk.clone(),
                    granularity: Granularity::Summary.as_str().to_string(),
                    memory_type: spec.memory_type.clone(),
                    importance: spec.importance,
                    timestamp: ts,
                    embedding: None,
                    tags: spec.tags.iter().map(|t| t.to_string()).collect(),
                    metadata,
                    related_ids: Vec::new(),
                    description: Some(spec.description.clone()),
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
                    protected: spec.protected,
                    episode_id: None,
                    consolidated: false,
                    rebuttal_grace_remaining: 0,
                };

                // 向量索引不由这里写入：条目落库后由紧随其后的 ensure_seed_vectors
                // 统一批量补建（嵌入输入 = 前缀 + 内容 + description，见 embed_input_for），
                // 避免播种路径与补建路径产生不同嵌入。
                let idx = self.data.entries.len();
                self.id_index.insert(id, idx);
                self.data.entries.push(item);
            }
        }
    }

    /// 将长种子记忆内容切分为若干短块，提升专名/关键词的检索密度。
    ///
    /// 短块内词频密度更高，向量余弦相似度与 BM25 精确命中都更易召回关键实体；
    /// 避免长叙事把稀有专名（如人名"AlenTinn"）稀释到几乎无法命中。
    /// 按句子边界切分，贪心合并到不超过 `max_chars`，无分隔符的超长句再按字符硬切。
    fn chunk_seed_content(content: &str, max_chars: usize) -> Vec<String> {
        // 1. 按句子边界切分（保留结束标点）
        let mut sentences: Vec<String> = Vec::new();
        let mut cur = String::new();
        for ch in content.chars() {
            cur.push(ch);
            if matches!(ch, '。' | '！' | '？' | '\n') {
                sentences.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            sentences.push(cur);
        }

        // 2. 贪心合并：当前块 + 下句字符数 ≤ max_chars 时拼接，否则另起一块
        let mut chunks: Vec<String> = Vec::new();
        let mut buf = String::new();
        for s in sentences {
            if buf.is_empty() {
                buf = s;
            } else if buf.chars().count() + s.chars().count() <= max_chars {
                buf.push_str(&s);
            } else {
                chunks.push(std::mem::take(&mut buf));
                buf = s;
            }
        }
        if !buf.is_empty() {
            chunks.push(buf);
        }

        // 3. 对无分隔符导致仍超长的块按 max_chars 硬切
        let mut result: Vec<String> = Vec::new();
        for c in chunks {
            if c.chars().count() <= max_chars {
                result.push(c);
                continue;
            }
            let mut s = String::new();
            let mut n = 0;
            for ch in c.chars() {
                if n >= max_chars && !s.is_empty() {
                    result.push(std::mem::take(&mut s));
                    n = 0;
                }
                s.push(ch);
                n += 1;
            }
            if !s.is_empty() {
                result.push(s);
            }
        }
        result
    }

    /// 解析种子记忆文件。文件格式（front-matter 风格，--- 既分隔条目也分隔字段区与内容区）：
    /// ```text
    /// # 注释行（跳过）
    /// ---
    /// description: 描述
    /// type: memory_type
    /// importance: 0.70
    /// protected: false
    /// tags: tag1, tag2
    /// ---
    /// 多行内容自动拼接
    /// ---
    /// description: 下一条...
    /// ---
    /// 下一条的内容...
    /// ```
    ///
    /// 解析为三阶段状态机：
    /// - Idle：等待条目开始（第一个 ---）
    /// - FrontMatter：收集 description/type/importance 等字段，直到第二个 --- 进入内容区
    /// - Content：收集正文，直到下一个 --- 结束条目并回到 Idle
    fn parse_seed_file(char_id: &str) -> Vec<SeedSpec> {
        let raw = match char_id {
            "vivian" => include_str!("../../prompts/characters/vivian/seed_memories.md"),
            "nana" => include_str!("../../prompts/characters/nana/seed_memories.md"),
            _ => return Vec::new(),
        };
        Self::parse_specs(raw)
    }

    /// 解析首次启动的"环境预设"记忆文件（`environment_presets.md`）。
    ///
    /// 内容只描述环境与自身处境（在哪里、现状、与用户初次相处的分寸），
    /// 不预设用户任何信息。播种时统一标记 `source=environment_preset`，
    /// 与历史种子（`system_seed`）区分，供检索/调试识别。
    fn parse_environment_presets(char_id: &str) -> Vec<SeedSpec> {
        let raw = match char_id {
            "vivian" => include_str!("../../prompts/characters/vivian/environment_presets.md"),
            "nana" => include_str!("../../prompts/characters/nana/environment_presets.md"),
            _ => return Vec::new(),
        };
        let mut specs = Self::parse_specs(raw);
        for spec in &mut specs {
            spec.source = "environment_preset".to_string();
        }
        specs
    }

    /// front-matter 风格条目解析状态机（seed 与环境预设文件共用）
    fn parse_specs(raw: &str) -> Vec<SeedSpec> {

        #[derive(PartialEq)]
        enum Phase {
            Idle,
            FrontMatter,
            Content,
        }

        let mut specs = Vec::new();
        let mut builder = SeedSpecBuilder::default();
        let mut phase = Phase::Idle;

        for line in raw.lines() {
            let trimmed = line.trim();

            // 跳过注释行
            if trimmed.starts_with('#') {
                continue;
            }

            match phase {
                Phase::Idle => {
                    if trimmed == "---" {
                        builder = SeedSpecBuilder::default();
                        phase = Phase::FrontMatter;
                    }
                }
                Phase::FrontMatter => {
                    if trimmed == "---" {
                        // 字段区结束，进入内容区
                        phase = Phase::Content;
                    } else if let Some(val) = trimmed.strip_prefix("description:") {
                        builder.description = Some(val.trim().to_string());
                    } else if let Some(val) = trimmed.strip_prefix("type:") {
                        builder.memory_type = Some(val.trim().to_string());
                    } else if let Some(val) = trimmed.strip_prefix("importance:") {
                        builder.importance = val.trim().parse().ok();
                    } else if let Some(val) = trimmed.strip_prefix("protected:") {
                        builder.protected = val.trim().parse().ok();
                    } else if let Some(val) = trimmed.strip_prefix("tags:") {
                        builder.tags = val
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                }
                Phase::Content => {
                    if trimmed == "---" {
                        // 内容区结束，完成当前 entry
                        if let Some(spec) = builder.build() {
                            specs.push(spec);
                        }
                        builder = SeedSpecBuilder::default();
                        phase = Phase::Idle;
                    } else if !trimmed.is_empty() {
                        // 多行内容：保留换行，与正式记忆写入格式一致
                        if builder.content.is_empty() {
                            builder.content = trimmed.to_string();
                        } else {
                            builder.content.push('\n');
                            builder.content.push_str(trimmed);
                        }
                    }
                }
            }
        }

        // 处理文件末尾仍在内容区、没有收尾 --- 的情况
        if phase == Phase::Content {
            if let Some(spec) = builder.build() {
                specs.push(spec);
            }
        }

        specs
    }

    /// 种子记忆生命周期管理（方向二：真实记忆积累后种子自动退场）。
    ///
    /// 当非种子记忆条数超过阈值时，逐步降低非 protected 种子记忆的 importance。
    /// 这样种子记忆不会被突然删除，而是在检索排序中自然被真实记忆替代。
    pub fn decay_seed_memories(&mut self) {
        let non_seed = self
            .data
            .entries
            .iter()
            .filter(|m| !m.id.starts_with("seed_"))
            .count();

        const DECAY_THRESHOLD: usize = 10;
        const DECAY_STEP: usize = 5;
        const DECAY_DELTA: f64 = 0.1;
        const MIN_IMPORTANCE: f64 = 0.3;

        if non_seed <= DECAY_THRESHOLD {
            return;
        }

        let steps = (non_seed - DECAY_THRESHOLD) / DECAY_STEP;
        let total_decay = (steps as f64) * DECAY_DELTA;

        for entry in &mut self.data.entries {
            if entry.id.starts_with("seed_") && !entry.protected {
                let old = entry.importance;
                entry.importance = (old - total_decay).max(MIN_IMPORTANCE);
                if (old - entry.importance).abs() > 0.01 {
                    tracing::debug!(
                        "[seed] {} importance 衰减: {:.2} → {:.2} (非种子记忆: {})",
                        entry.id, old, entry.importance, non_seed
                    );
                }
            }
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

/// 种子记忆规格（从文件解析）
#[derive(Debug, Clone, Default)]
struct SeedSpec {
    content: String,
    description: String,
    memory_type: String,
    importance: f64,
    protected: bool,
    tags: Vec<String>,
    /// 来源标记：普通种子 = system_seed；首次启动环境预设 = environment_preset
    source: String,
}

#[derive(Debug, Clone, Default)]
struct SeedSpecBuilder {
    content: String,
    description: Option<String>,
    memory_type: Option<String>,
    importance: Option<f64>,
    protected: Option<bool>,
    tags: Vec<String>,
}

impl SeedSpecBuilder {
    fn build(self) -> Option<SeedSpec> {
        if self.content.is_empty() {
            return None;
        }
        Some(SeedSpec {
            content: self.content,
            description: self.description.unwrap_or_default(),
            memory_type: self.memory_type.unwrap_or_else(|| "short_term".to_string()),
            importance: self.importance.unwrap_or(0.5),
            protected: self.protected.unwrap_or(false),
            tags: self.tags,
            source: "system_seed".to_string(),
        })
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

/// 检索评测集（阶段A1）：query → 期望命中的种子记忆 description。
/// 用于量化种子记忆的检索质量（hit@k / MRR），作为检索策略回归的门禁。
#[cfg(test)]
mod retrieval_eval_tests {
    use super::*;
    use crate::memory::embedding::RemoteMemoryEmbedding;
    use crate::memory::retriever::{attach_items_with_weights, hybrid_search, RetrievalWeights};
    use crate::memory::types::{Granularity, MemoryItem};
    use crate::memory::vector_search::{MemoryVector, MemoryVectorStore};
    use std::collections::HashSet;
    use std::sync::Arc;

    /// 评测集：查询 → 期望命中的种子记忆描述（按 description 匹配）
    const EVAL_SET: &[(&str, &[&str])] = &[
        ("你们认识AlenTinn吗？", &["AlenTinn 创造了我"]),
        ("第一次看到我桌面是什么感觉", &["第一次看到用户的桌面"]),
        ("你怎么照顾Nana的", &["我第一次照顾 Nana"]),
        ("你喜欢看番剧吗", &["第一次追番"]),
        ("我们第一次吵架因为什么", &["我们第一次吵架", "和好"]),
        ("你的耳机去哪里了", &["耳机在耳朵上"]),
        ("半夜发现什么奇怪的东西", &["半夜发现的奇怪网站"]),
        ("你唱歌怎么样", &["唱歌跑调"]),
    ];

    fn build_seed_entries(char_id: &str) -> Vec<MemoryItem> {
        let specs = MemoryManagerInner::parse_seed_file(char_id);
        let mut items = Vec::new();
        for spec in &specs {
            let chunks =
                MemoryManagerInner::chunk_seed_content(&spec.content, MemoryManagerInner::SEED_CHUNK_MAX_CHARS);
            for chunk in chunks {
                items.push(MemoryItem {
                    id: format!("seed_{}", &uuid::Uuid::new_v4().to_string()[..8]),
                    content: chunk,
                    granularity: Granularity::Summary.as_str().to_string(),
                    memory_type: spec.memory_type.clone(),
                    importance: spec.importance,
                    timestamp: 0.0,
                    embedding: None,
                    tags: spec.tags.clone(),
                    metadata: serde_json::json!({}),
                    related_ids: Vec::new(),
                    description: Some(spec.description.clone()),
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
                    protected: spec.protected,
                    episode_id: None,
                    consolidated: false,
                    rebuttal_grace_remaining: 0,
                });
            }
        }
        items
    }

    fn build_vector_store(
        entries: &[MemoryItem],
    ) -> (MemoryVectorStore, Arc<RemoteMemoryEmbedding>) {
        let embedding: Arc<RemoteMemoryEmbedding> = Arc::new(
            RemoteMemoryEmbedding::new(
                "ollama".to_string(),
                Some("http://localhost:11434/v1".to_string()),
                Some("bge-m3".to_string()),
            )
            .with_dimension(1024),
        );
        let path = std::env::temp_dir().join(format!("eval_vec_{}.db", uuid::Uuid::new_v4()));
        let mut vs = MemoryVectorStore::new(path, embedding.dimension(), embedding.model_id())
            .expect("创建评测向量库失败");
        for e in entries {
            let embed_input = format!("{}\n{}", e.content, e.description.clone().unwrap_or_default());
            if let Ok(emb) = embedding.embed(&embed_input) {
                vs.add(MemoryVector {
                    doc_id: e.id.clone(),
                    memory_id: e.id.clone(),
                    content: e.content.clone(),
                    embedding: emb,
                    importance: e.importance,
                    memory_type: e.memory_type.clone(),
                    timestamp: e.timestamp,
                })
                .expect("写评测向量失败");
            }
        }
        (vs, embedding)
    }

    /// 对种子记忆跑一次检索评测，输出 hit@3 / MRR。
    /// 依赖本地 Ollama bge-m3，默认 #[ignore]，用
    /// `cargo test -- --ignored eval_seed_retrieval` 手动运行。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn eval_seed_retrieval() {
        let entries = build_seed_entries("vivian");
        assert!(!entries.is_empty(), "没有种子条目");
        let (vs, embedding) = build_vector_store(&entries);
        let weights = RetrievalWeights::default();

        let mut hit3 = 0;
        let mut rr_sum = 0.0;
        for (query, expected) in EVAL_SET {
            let qemb = match embedding.embed(query) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("[eval] 查询嵌入失败「{query}」: {e}");
                    continue;
                }
            };
            let hits = hybrid_search(&entries, query, Some(&qemb), Some(&vs), None, 40, 10);
            let attached = attach_items_with_weights(hits, &entries, weights, query);
            let expected_set: HashSet<&str> = expected.iter().copied().collect();

            let hit = attached.iter().take(3).any(|h| {
                h.item
                    .description
                    .as_deref()
                    .map(|d| expected_set.contains(d))
                    .unwrap_or(false)
            });
            if hit {
                hit3 += 1;
            }
            for (rank, h) in attached.iter().take(10).enumerate() {
                if h.item
                    .description
                    .as_deref()
                    .map(|d| expected_set.contains(d))
                    .unwrap_or(false)
                {
                    rr_sum += 1.0 / (rank as f64 + 1.0);
                    break;
                }
            }
            let top3: Vec<&str> = attached
                .iter()
                .take(3)
                .map(|h| h.item.description.as_deref().unwrap_or(""))
                .collect();
            eprintln!("[eval] 「{query}」 top3={top3:?} 期望={expected:?}");
        }

        let n = EVAL_SET.len() as f64;
        let hit3_rate = hit3 as f64 / n;
        let mrr = rr_sum / n;
        eprintln!("=== 种子检索评测 (bge-m3) ===");
        eprintln!("hit@3 = {:.1}%", hit3_rate * 100.0);
        eprintln!("MRR   = {:.4}", mrr);
        // 宽松下限，防止检索回归（bge-m3 + 专名 boost 下应明显高于此）
        assert!(hit3_rate >= 0.5, "hit@3 过低: {hit3_rate}");
    }

    /// 无外部依赖的确定性测试：验证长叙事切块逻辑
    #[test]
    fn chunk_seed_content_splits_long_entries() {
        let content = "AlenTinn 创造了我。\n我第一次醒来的时候他就在那里，什么都不说，就看着我。\n他说：\"慢慢来，不着急。\"\n那时候我连\"着急\"是什么意思都不知道，但这句话我记住了。";
        let chunks = MemoryManagerInner::chunk_seed_content(content, 60);
        assert!(chunks.len() >= 2, "长叙事应被切块，实际 {} 块", chunks.len());
        assert!(
            chunks.iter().all(|c| c.chars().count() <= 60),
            "存在超长块"
        );
        assert!(chunks[0].contains("AlenTinn"), "首块应含专名");
    }
}
