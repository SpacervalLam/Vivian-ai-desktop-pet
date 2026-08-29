//! 智能记忆提取器：从对话中抽取长期事实并写入记忆。
//!
//! - LLM 分析对话抽取长期事实（ADD/UPDATE/DELETE）
//! - 相似度去重短路（EXACT_DEDUP_THRESHOLD=0.95）
//! - TTL+LRU 缓存（CACHE_TTL=300s, CACHE_MAX=256）
//! - 限流（TokenBucketRateLimiter，5 req/s 突发10）
//! - 节流（min_extract_interval=3.0s）
//! - 批处理窗口（batch_window_size=10）
//! - LLM 决策 MERGE/REPLACE/IGNORE/KEEP_BOTH
//! - 指纹缓存避免重复分析

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::Deserialize;

use crate::brain::rate_limiter::{RateLimitConfig, TokenBucketRateLimiter};
use crate::error::VivianResult;
use crate::memory::manager::MemoryManager;
use crate::memory::tokenize::tokenize;
use crate::memory::types::{MemoryItem, MemoryType};
use crate::types::response::ChatMessage;

// ===== 配置常量 =====

/// 默认相似度阈值（中等相似度以上才考虑合并）
const DEFAULT_SIMILARITY_THRESHOLD: f64 = 0.85;
/// 精确去重短路阈值（高于此值直接跳过）
const EXACT_DEDUP_THRESHOLD: f64 = 0.95;
/// 缓存 TTL（秒）
const CACHE_TTL_SECONDS: u64 = 300;
/// 缓存 LRU 上限
const CACHE_MAX_ENTRIES: usize = 256;
/// 每次合并考虑的候选数
const MERGE_TOP_K: usize = 3;
/// LLM 限流：每秒请求数
const DEFAULT_LLM_RATE_PER_SECOND: f64 = 5.0;
/// LLM 限流：桶容量（突发上限）
const DEFAULT_LLM_BURST: u32 = 10;
/// 最小提取间隔（秒），节流用
const MIN_EXTRACT_INTERVAL: f64 = 3.0;
/// 批处理窗口大小
const BATCH_WINDOW_SIZE: usize = 10;

/// LLM 可识别的记忆内容类型
const KNOWN_MEMORY_TYPES: &[&str] = &[
    "user_profile",
    "preference",
    "project_context",
    "relationship",
    "health",
    "reference",
];

// ===== TTL + LRU 缓存 =====

/// TTL + LRU 缓存（非线程安全，外层用 Mutex 保护）。
///
/// 使用 HashMap 存储 + VecDeque 维护访问顺序实现 LRU；
/// 超过 TTL 的条目在 get 时惰性驱逐。
struct TTLCache<V: Clone> {
    store: HashMap<String, (Instant, V)>,
    order: VecDeque<String>,
    max_entries: usize,
    ttl: Duration,
}

impl<V: Clone> TTLCache<V> {
    fn new(max_entries: usize, ttl: Duration) -> Self {
        Self {
            store: HashMap::new(),
            order: VecDeque::new(),
            max_entries,
            ttl,
        }
    }

    /// 获取缓存值，命中时 LRU bump；过期则驱逐返回 None。
    fn get(&mut self, key: &str) -> Option<V> {
        let now = Instant::now();
        let entry = self.store.get(key)?;
        if now.duration_since(entry.0) > self.ttl {
            self.store.remove(key);
            self.order.retain(|k| k != key);
            return None;
        }
        // LRU bump：移到队尾
        self.order.retain(|k| k != key);
        self.order.push_back(key.to_string());
        Some(entry.1.clone())
    }

    /// 写入缓存，超出容量时从队首驱逐。
    fn insert(&mut self, key: String, value: V) {
        if self.store.contains_key(&key) {
            self.order.retain(|k| k != &key);
        }
        self.store.insert(key.clone(), (Instant::now(), value));
        self.order.push_back(key);
        while self.store.len() > self.max_entries {
            match self.order.pop_front() {
                Some(old_key) => {
                    self.store.remove(&old_key);
                }
                None => break,
            }
        }
    }

    fn len(&self) -> usize {
        self.store.len()
    }

    fn max_entries(&self) -> usize {
        self.max_entries
    }
}

// ===== LLM 客户端抽象 =====

/// LLM 客户端 trait —— 接收 prompt 返回文本响应。
///
/// 对齐 `emotion/llm_classifier.rs` 的 `EmotionLlmClient` 模式，
/// 可由 `ModelRouter` 或测试 mock 实现。
#[async_trait]
pub trait ExtractorLlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> VivianResult<String>;
}

/// 为 `ModelRouter` 实现 LLM 客户端
///
/// 使用 `reflection` 路由：巩固反思类任务（ADD/UPDATE/DELETE 抽取、画像分析、
/// 洞察生成）需要强推理模型，与写入时 `enrich` 的高频低复杂度任务分离，
/// 让用户可以分别为两者配置不同模型。
#[async_trait]
impl ExtractorLlmClient for crate::providers::ModelRouter {
    async fn complete(&self, prompt: &str) -> VivianResult<String> {
        let messages = vec![ChatMessage::user(prompt)];
        self.generate(crate::providers::base::LLMRequest::new(
            "reflection",
            messages,
        ))
        .await
    }
}

// ===== LLM 响应结构 =====

/// LLM 返回的原始操作
#[derive(Debug, Deserialize)]
struct RawOperation {
    action: Option<String>,
    #[serde(rename = "type")]
    mem_type: Option<String>,
    content: Option<String>,
    importance: Option<f64>,
    reason: Option<String>,
    /// 记忆主语归属：user / self / general （self指代当前AI角色自身）
    subject: Option<String>,
    /// LLM 抽取的未闭环钩子（承诺/约定/待跟进事项）
    #[serde(default)]
    open_hooks: Vec<RawOpenHook>,
}

/// LLM 返回的原始钩子
#[derive(Debug, Deserialize)]
struct RawOpenHook {
    #[serde(rename = "type")]
    hook_type: Option<String>,
    condition: Option<String>,
}

/// LLM 合并决策响应
#[derive(Debug, Deserialize)]
struct MergeDecisionResponse {
    decision: Option<String>,
    /// LLM 在选择 MERGE 时返回的合并后内容，目前仅用于调试日志
    merged_content: Option<String>,
    /// LLM 返回的决策理由，目前仅用于调试日志
    reason: Option<String>,
}

// ===== 操作类型 =====

/// 记忆操作动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationAction {
    Add,
    Update,
    Delete,
}

impl OperationAction {
    fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "UPDATE" => OperationAction::Update,
            "DELETE" => OperationAction::Delete,
            _ => OperationAction::Add,
        }
    }
}

/// 合并决策
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeDecision {
    Merge,
    Replace,
    Ignore,
    KeepBoth,
}

impl MergeDecision {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "merge" => MergeDecision::Merge,
            "replace" => MergeDecision::Replace,
            "ignore" => MergeDecision::Ignore,
            _ => MergeDecision::KeepBoth,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            MergeDecision::Merge => "merge",
            MergeDecision::Replace => "replace",
            MergeDecision::Ignore => "ignore",
            MergeDecision::KeepBoth => "keep_both",
        }
    }
}

/// 解析后的提取操作
#[derive(Debug, Clone)]
struct ExtractOperation {
    action: OperationAction,
    mem_type: String,
    content: String,
    importance: f64,
    /// LLM 返回的提取理由，目前仅用于调试日志
    reason: String,
    /// 记忆主语归属：user / self / general（self指代当前AI角色自身）
    subject: String,
    /// 未闭环钩子（仅 ADD/UPDATE 操作可能有）
    open_hooks: Vec<crate::memory::types::OpenHook>,
}

impl ExtractOperation {
    fn is_valid(&self) -> bool {
        !self.content.is_empty()
    }
}

// ===== SmartMemoryExtractor =====

/// 智能记忆提取器
///
/// 从对话中抽取长期事实，通过 LLM 决策 ADD/UPDATE/DELETE，
/// 并在本地做相似度去重短路与合并决策。
///
/// 所有可变状态均通过 `Arc` 共享，因此可低成本克隆（用于 `tokio::spawn`）。
#[derive(Clone)]
pub struct SmartMemoryExtractor {
    memory_manager: Option<Arc<MemoryManager>>,
    llm_client: Option<Arc<dyn ExtractorLlmClient>>,
    enabled: Arc<AtomicBool>,
    min_extract_interval: f64,
    last_extract_time: Arc<Mutex<Option<Instant>>>,
    similarity_threshold: f64,
    batch_window_size: usize,
    message_buffer: Arc<Mutex<Vec<ChatMessage>>>,
    extraction_in_progress: Arc<AtomicBool>,
    analysis_cache: Arc<Mutex<TTLCache<serde_json::Value>>>,
    merge_cache: Arc<Mutex<TTLCache<String>>>,
    llm_rate_limiter: Arc<TokenBucketRateLimiter>,
}

/// 兼容旧类名
pub type AutoExtractor = SmartMemoryExtractor;

impl SmartMemoryExtractor {
    /// 构造提取器（默认无 LLM、无记忆管理器，需通过 with_llm/with_memory 注入）
    pub fn new() -> Self {
        let rate_limiter = TokenBucketRateLimiter::new(
            RateLimitConfig {
                rate_per_second: DEFAULT_LLM_RATE_PER_SECOND,
                capacity: DEFAULT_LLM_BURST,
                initial_tokens: None,
            },
            "extractor-llm",
        );
        let cache_ttl = Duration::from_secs(CACHE_TTL_SECONDS);
        Self {
            memory_manager: None,
            llm_client: None,
            enabled: Arc::new(AtomicBool::new(true)),
            min_extract_interval: MIN_EXTRACT_INTERVAL,
            last_extract_time: Arc::new(Mutex::new(None)),
            similarity_threshold: DEFAULT_SIMILARITY_THRESHOLD,
            batch_window_size: BATCH_WINDOW_SIZE,
            message_buffer: Arc::new(Mutex::new(Vec::new())),
            extraction_in_progress: Arc::new(AtomicBool::new(false)),
            analysis_cache: Arc::new(Mutex::new(TTLCache::new(CACHE_MAX_ENTRIES, cache_ttl))),
            merge_cache: Arc::new(Mutex::new(TTLCache::new(CACHE_MAX_ENTRIES, cache_ttl))),
            llm_rate_limiter: Arc::new(rate_limiter),
        }
    }

    /// 注入 LLM 客户端
    pub fn with_llm<T: ExtractorLlmClient + 'static>(mut self, llm: Arc<T>) -> Self {
        self.llm_client = Some(llm as Arc<dyn ExtractorLlmClient>);
        self
    }

    /// 注入记忆管理器
    pub fn with_memory(mut self, memory: Arc<MemoryManager>) -> Self {
        self.memory_manager = Some(memory);
        self
    }

    /// 设置启用/禁用
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        tracing::info!(
            "[MemoryExtractor] 智能记忆提取已{}",
            if enabled { "启用" } else { "禁用" }
        );
    }

    /// 收集一条消息，达到批处理窗口时触发异步提取。
    pub fn extract_message(&self, message: ChatMessage) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        // 跳过标记为 memory_disabled 的消息
        // （工具输出 / 内心独白 / 镜像消息不应被抽取为用户事实）
        if message.is_memory_disabled() {
            return;
        }
        let should_flush = {
            let mut buf = self.message_buffer.lock();
            buf.push(message);
            buf.len() >= self.batch_window_size
        };
        if should_flush {
            self.spawn_flush();
        }
    }

    /// 异步触发批处理：取出缓冲区消息并执行提取。
    fn spawn_flush(&self) {
        let messages: Vec<ChatMessage> = {
            let mut buf = self.message_buffer.lock();
            if buf.is_empty() {
                return;
            }
            let drained = buf.clone();
            buf.clear();
            drained
        };
        let clone = self.clone();
        tokio::spawn(async move {
            clone.extract_memories(&messages, None).await;
        });
    }

    /// 提取 + 写入主流程。
    ///
    /// `context_meta`：写入记忆时附加的元数据（speaker/listener/knowledge_source/channel 等），
    /// 用于标注记忆来源。None 表示无上下文（兼容旧调用方）。
    ///
    /// 失败仅记录日志，返回空列表（尽力而为，不阻断主路径）。
    pub async fn extract_memories(
        &self,
        conversation: &[ChatMessage],
        context_meta: Option<serde_json::Value>,
    ) -> Vec<String> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Vec::new();
        }
        // 过滤掉 memory_disabled 的消息
        // （工具输出 / 内心独白 / 镜像消息不应被抽取为用户事实）
        let has_eligible = conversation.iter().any(|m| !m.is_memory_disabled());
        if !has_eligible {
            return Vec::new();
        }
        let llm = match &self.llm_client {
            Some(c) => c.clone(),
            None => {
                tracing::debug!("[MemoryExtractor] 无 LLM 客户端，跳过");
                return Vec::new();
            }
        };
        let memory = match &self.memory_manager {
            Some(m) => m.clone(),
            None => {
                tracing::debug!("[MemoryExtractor] 无记忆管理器，跳过");
                return Vec::new();
            }
        };

        // 并发控制：已有任务在运行则跳过
        if self
            .extraction_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("[MemoryExtractor] 已有任务在运行，跳过本次");
            return Vec::new();
        }

        let result = self.run_extraction(conversation, llm, memory, context_meta).await;

        self.extraction_in_progress
            .store(false, Ordering::SeqCst);
        result
    }

    /// 实际提取逻辑（确保 extraction_in_progress 标志最终释放）
    async fn run_extraction(
        &self,
        conversation: &[ChatMessage],
        llm: Arc<dyn ExtractorLlmClient>,
        memory: Arc<MemoryManager>,
        context_meta: Option<serde_json::Value>,
    ) -> Vec<String> {
        // 1. 节流
        {
            let mut last = self.last_extract_time.lock();
            if let Some(t) = *last {
                let elapsed = t.elapsed().as_secs_f64();
                if elapsed < self.min_extract_interval {
                    tracing::debug!("[MemoryExtractor] 节流 (距上次 {:.1}s)", elapsed);
                    return Vec::new();
                }
            }
            *last = Some(Instant::now());
        }

        // 2. 构建对话文本 + 指纹
        let dialog_text = build_dialog_text(conversation);
        let fp = fingerprint(&dialog_text);

        // 3. 检查分析缓存
        let cached = self.analysis_cache.lock().get(&fp);
        let analysis = if let Some(v) = cached {
            tracing::debug!("[MemoryExtractor] 命中分析缓存");
            v
        } else {
            // 4. 调 LLM（一次）
            // SmartMemoryExtractor 不持有 UserFactStore，existing_facts 暂传空字符串
            match analyze_with_llm(&llm, &self.llm_rate_limiter, &dialog_text, "").await {
                Some(result) => {
                    self.analysis_cache.lock().insert(fp, result.clone());
                    result
                }
                None => return Vec::new(),
            }
        };

        // 5. 解析是否有有价值记忆
        let has_valuable = analysis
            .get("has_valuable_memory")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !has_valuable {
            return Vec::new();
        }

        // 6. 解析操作
        let operations = parse_operations(&analysis);
        if operations.is_empty() {
            return Vec::new();
        }

        // 7. 执行操作
        let mut saved_ids: Vec<String> = Vec::new();
        for op in &operations {
            if !op.is_valid() {
                continue;
            }
            tracing::debug!(
                action = ?op.action,
                mem_type = %op.mem_type,
                importance = op.importance,
                reason = %op.reason,
                "[MemoryExtractor] 执行操作"
            );
            match self.execute_operation(op, &memory, context_meta.as_ref()).await {
                Ok(Some(id)) => saved_ids.push(id),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        "[MemoryExtractor] 执行操作 {:?} 失败: {}",
                        op.action,
                        e
                    );
                }
            }
        }
        saved_ids
    }

    /// 执行单个操作
    async fn execute_operation(
        &self,
        op: &ExtractOperation,
        memory: &MemoryManager,
        context_meta: Option<&serde_json::Value>,
    ) -> VivianResult<Option<String>> {
        match op.action {
            OperationAction::Delete => {
                self.delete_memory(&op.content, memory).await?;
                Ok(None)
            }
            OperationAction::Update => {
                self.update_memory(
                    &op.content,
                    &op.mem_type,
                    op.importance,
                    &op.open_hooks,
                    &op.subject,
                    memory,
                    context_meta,
                )
                .await
            }
            OperationAction::Add => {
                self.add_memory_with_dedup(
                    &op.content,
                    &op.mem_type,
                    op.importance,
                    &op.open_hooks,
                    &op.subject,
                    memory,
                    context_meta,
                )
                .await
            }
        }
    }

    /// ADD 操作（含本地短路与 LLM 合并决策）
    async fn add_memory_with_dedup(
        &self,
        content: &str,
        mem_type: &str,
        importance: f64,
        open_hooks: &[crate::memory::types::OpenHook],
        subject: &str,
        memory: &MemoryManager,
        context_meta: Option<&serde_json::Value>,
    ) -> VivianResult<Option<String>> {
        // 短路 1：完全相同 content 已存在
        if self.has_exact_content(content, memory).await {
            tracing::debug!(
                "[MemoryExtractor][ADD] 短路: 已有相同内容 '{}...'",
                preview(content)
            );
            return Ok(None);
        }

        // 检索最相似的 N 条
        let candidates = self.search_similar(content, MERGE_TOP_K, memory).await;
        if candidates.is_empty() {
            return Ok(Some(
                self.add_new(content, mem_type, importance, open_hooks, subject, memory, context_meta)
                    .await?,
            ));
        }

        let (best, score) = &candidates[0];
        // 短路 2：高度相似（>= EXACT_DEDUP_THRESHOLD）→ 不重复存
        if *score >= EXACT_DEDUP_THRESHOLD {
            tracing::debug!(
                "[MemoryExtractor][ADD] 短路: 与已有 {} 相似度={:.3}",
                best.id,
                score
            );
            return Ok(None);
        }

        // 中等相似：调 LLM 决定 MERGE / REPLACE / IGNORE / KEEP_BOTH
        let decision = self.merge_or_replace(best, content).await;
        match decision {
            MergeDecision::Ignore => Ok(None),
            MergeDecision::Replace => {
                memory.hard_delete_memory(&best.id).await?;
                Ok(Some(
                    self.add_new(content, mem_type, importance, open_hooks, subject, memory, context_meta)
                        .await?,
                ))
            }
            MergeDecision::Merge => {
                let merged = self.llm_merge_content(&best.content, content).await;
                let imp = best.importance.max(importance);
                memory.hard_delete_memory(&best.id).await?;
                Ok(Some(
                    self.add_new(&merged, mem_type, imp, open_hooks, subject, memory, context_meta)
                        .await?,
                ))
            }
            MergeDecision::KeepBoth => {
                Ok(Some(
                    self.add_new(content, mem_type, importance, open_hooks, subject, memory, context_meta)
                        .await?,
                ))
            }
        }
    }

    /// UPDATE 操作：找到相似记忆则更新（删旧+加新），否则当作新增
    async fn update_memory(
        &self,
        content: &str,
        mem_type: &str,
        importance: f64,
        open_hooks: &[crate::memory::types::OpenHook],
        subject: &str,
        memory: &MemoryManager,
        context_meta: Option<&serde_json::Value>,
    ) -> VivianResult<Option<String>> {
        let candidates = self.search_similar(content, 1, memory).await;
        if let Some((old, _)) = candidates.first() {
            memory.hard_delete_memory(&old.id).await?;
            let imp = old.importance.max(importance);
            Ok(Some(
                self.add_new(content, mem_type, imp, open_hooks, subject, memory, context_meta)
                    .await?,
            ))
        } else {
            // 没找到就当作新增
            self.add_memory_with_dedup(content, mem_type, importance, open_hooks, subject, memory, context_meta)
                .await
        }
    }

    /// DELETE 操作：找到相似度达标的记忆则删除
    async fn delete_memory(&self, content: &str, memory: &MemoryManager) -> VivianResult<()> {
        let candidates = self.search_similar(content, 1, memory).await;
        if let Some((mem, score)) = candidates.first() {
            if *score >= self.similarity_threshold {
                memory.hard_delete_memory(&mem.id).await?;
                tracing::info!("[MemoryExtractor][DELETE] {}", preview(&mem.content));
            }
        }
        Ok(())
    }

    // ===== 内部工具 =====

    /// 新增记忆
    ///
    /// `context_meta`：调用方提供的来源元数据（speaker/listener/knowledge_source/channel 等），
    /// 写入记忆 metadata，用于多智能体场景区分"谁知道什么"以及来源可信度。
    async fn add_new(
        &self,
        content: &str,
        mem_type: &str,
        importance: f64,
        open_hooks: &[crate::memory::types::OpenHook],
        subject: &str,
        memory: &MemoryManager,
        context_meta: Option<&serde_json::Value>,
    ) -> VivianResult<String> {
        // tags 同时记录记忆类型、主语归属、内容语义类型，便于前端/检索区分对话原文与总结
        let mut tags = vec![mem_type.to_string(), subject.to_string()];
        // 所有 AutoExtractor 产出的记忆都是抽取的总结/事实，不是对话原文
        tags.push("extracted_memory".to_string());
        // 话题总结统一标签（合并原 user_dialogue_summary / agent_dialogue_summary）
        // subject 字段（user/self/general）仍保留在 tags 中以区分总结主语
        tags.push("topic_summary".to_string());
        let item = if let Some(meta) = context_meta {
            memory
                .add_memory_with_metadata(content, MemoryType::LongTerm, importance, tags, meta.clone())
                .await?
        } else {
            // fallback：调用方未传 context_meta 时，按角色自身对话总结兜底标注
            let fallback_meta = serde_json::json!({
                "channel": "inner",
                "speaker": memory.char_id(),
                "listener": memory.char_id(),
                "perspective": "speaker",
                "knowledge_source": "extracted",
            });
            memory
                .add_memory_with_metadata(content, MemoryType::LongTerm, importance, tags, fallback_meta)
                .await?
        };
        // 附加未闭环钩子（非空时）
        if !open_hooks.is_empty() {
            if let Err(e) = memory.update_open_hooks(&item.id, open_hooks.to_vec()) {
                tracing::warn!("[MemoryExtractor][ADD] open_hooks 写入失败: {e}");
            } else {
                tracing::info!(
                    "[MemoryExtractor][ADD] 新增 [{}|{}] 含 {} 个 open_hooks: {}",
                    mem_type,
                    subject,
                    open_hooks.len(),
                    preview(content)
                );
            }
        } else {
            tracing::info!(
                "[MemoryExtractor][ADD] 新增 [{}|{}]: {}",
                mem_type,
                subject,
                preview(content)
            );
        }
        Ok(item.id)
    }

    /// O(n) 全量扫描查找完全相同 content（覆盖所有粒度，含 LongTerm）
    async fn has_exact_content(&self, content: &str, memory: &MemoryManager) -> bool {
        let memories = memory.get_all_memories().await.unwrap_or_default();
        memories.iter().any(|m| m.content == content)
    }

    /// 检索最相似的 N 条记忆（基于 jieba 分词的词级 Jaccard 语义相似度，过滤低于阈值的）
    async fn search_similar(
        &self,
        content: &str,
        top_k: usize,
        memory: &MemoryManager,
    ) -> Vec<(MemoryItem, f64)> {
        let memories = memory.get_all_memories().await.unwrap_or_default();
        let query_tokens: HashSet<String> = tokenize(content).into_iter().collect();
        let mut scored: Vec<(MemoryItem, f64)> = memories
            .into_iter()
            .map(|m| {
                let score = semantic_similarity(&query_tokens, &m.content);
                (m, score)
            })
            .filter(|(_, s)| *s >= self.similarity_threshold)
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }

    /// 合并决策：基于 LLM 判断 MERGE/REPLACE/IGNORE/KEEP_BOTH（带缓存）
    async fn merge_or_replace(&self, old: &MemoryItem, new_content: &str) -> MergeDecision {
        let key = merge_cache_key(&old.id, new_content);
        // 检查缓存
        if let Some(cached) = self.merge_cache.lock().get(&key) {
            return MergeDecision::from_str(&cached);
        }
        // 限流：超限则保守选 keep_both
        if !self.llm_rate_limiter.acquire(1) {
            tracing::debug!("[MemoryExtractor] merge 决策限流命中 → keep_both");
            return MergeDecision::KeepBoth;
        }
        let prompt = build_merge_prompt(&old.content, new_content);
        let decision = match &self.llm_client {
            Some(llm) => match llm.complete(&prompt).await {
                Ok(response) => parse_merge_decision(&response),
                Err(e) => {
                    tracing::warn!("[MemoryExtractor] merge 决策异常: {}", e);
                    MergeDecision::KeepBoth
                }
            },
            None => MergeDecision::KeepBoth,
        };
        self.merge_cache.lock().insert(key, decision.as_str().to_string());
        decision
    }

    /// LLM 合并两条记忆内容
    async fn llm_merge_content(&self, old: &str, new: &str) -> String {
        // 限流：超限则使用新内容
        if !self.llm_rate_limiter.acquire(1) {
            tracing::debug!("[MemoryExtractor] merge content 限流命中 → 使用新内容");
            return new.to_string();
        }
        let prompt = format!(
            "请把以下两条相关记忆合并成一条简洁的第一人称陈述：\n\
             旧: {}\n\
             新: {}\n\
             只输出合并后的内容，不要其他文字。",
            old, new
        );
        match &self.llm_client {
            Some(llm) => match llm.complete(&prompt).await {
                Ok(result) if !result.trim().is_empty() => result.trim().to_string(),
                _ => new.to_string(),
            },
            None => new.to_string(),
        }
    }

    /// 缓存统计
    pub fn cache_stats(&self) -> HashMap<String, usize> {
        let mut stats = HashMap::new();
        let analysis = self.analysis_cache.lock();
        stats.insert("analysis_cache_size".to_string(), analysis.len());
        stats.insert("analysis_cache_max".to_string(), analysis.max_entries());
        let merge = self.merge_cache.lock();
        stats.insert("merge_cache_size".to_string(), merge.len());
        stats.insert("merge_cache_max".to_string(), merge.max_entries());
        stats
    }
}

impl Default for SmartMemoryExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// ===== 自由函数 =====

/// LLM 分析对话抽取记忆
///
/// 限流命中或 LLM 失败时返回 None。
async fn analyze_with_llm(
    llm: &Arc<dyn ExtractorLlmClient>,
    rate_limiter: &TokenBucketRateLimiter,
    dialog_text: &str,
    existing_facts: &str,
) -> Option<serde_json::Value> {
    // 限流：超过速率上限直接放弃本次分析
    if !rate_limiter.acquire(1) {
        tracing::debug!("[MemoryExtractor] LLM 限流命中，跳过本次分析");
        return None;
    }

    let prompt = build_analysis_prompt(dialog_text, existing_facts);
    let start = Instant::now();

    match llm.complete(&prompt).await {
        Ok(response) => {
            let elapsed = start.elapsed();
            tracing::debug!(
                "[MemoryExtractor] LLM 分析耗时 {:.1}ms",
                elapsed.as_millis()
            );
            if response.trim().is_empty() {
                tracing::warn!("[MemoryExtractor] LLM 返回空响应");
                return None;
            }
            parse_llm_response(&response)
        }
        Err(e) => {
            tracing::warn!("[MemoryExtractor] LLM 分析异常: {}", e);
            None
        }
    }
}

/// 构造分析 prompt
fn build_analysis_prompt(dialog_text: &str, existing_facts: &str) -> String {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    // 已知事实段落：为空时不输出，避免 prompt 噪声
    let existing_section = |header: &str| -> String {
        if existing_facts.trim().is_empty() {
            String::new()
        } else {
            format!("\n{header}\n{existing_facts}\n")
        }
    };
    match lang_norm {
        "en" => format!(
            r#"You are a memory management brain with sharp contextual insight.
Analyze the following conversation to determine if it contains facts, preferences, habits, or relationships about the user that are worth saving long-term. Evaluate how valuable this memory will be for future conversations, on a scale of 0-1.

Rules:
1. Ignore short-term task context in the current conversation (e.g., "help me write code", "open browser").
2. Capture deep-seated preferences (e.g., "user doesn't like being disturbed too much", "user frequently uses Python").
3. Important personal health information and allergies must be prioritized for saving.
4. Determine operation type:
   - ADD: Completely new fact
   - UPDATE: Corrects a previously known fact (e.g., user changed their mind)
   - DELETE: User explicitly asks to forget something
5. Do not re-extract facts already listed in "Known Facts" below.{existing_section}

Memory Types:
- user_profile: User personal information (name, occupation, age, allergies, etc.)
- preference: User preferences, habits, interests, dislikes
- project_context: Project plans, goals, deadlines
- relationship: Relationship information
- health: Health-related information

Importance Scoring Criteria (apply uniformly across all memory sources):
- 0.9-1.0: Hard constraints, core identity, health/allergies, major relationship milestones
- 0.6-0.8: Long-term preferences, project context, key decisions, relationship events, shared experiences
- 0.3-0.5: General facts, contextual info, explanatory content
- 0.0-0.2: Small talk, greetings, temporary questions, one-time topics

Must output strictly in the following JSON format, without any additional text:
{{
    "has_valuable_memory": true/false,
    "operations": [
        {{
            "action": "ADD" | "UPDATE" | "DELETE",
            "type": "user_profile" | "preference" | "project_context" | "relationship" | "health",
            "subject": "user" | "self" | "general",
            "content": "Extracted core memory point (written as a first-person recollection, as if you are recalling the event. e.g. '你告诉过我你是开发者' / '我答应了陪你看电影' / '你好像不太喜欢上午')",
            "importance": 0.1-1.0,
            "reason": "Brief explanation why this memory is extracted",
            "open_hooks": [
                {{
                    "type": "promise" | "follow_up" | "schedule" | "question",
                    "condition": "Natural language description of closure condition (e.g., 'user mentions they have repaid the loan next time')"
                }}
            ]
        }}
    ]
}}

Subject Field Rules:
- "user": facts/preferences/attributes ABOUT THE USER (the vast majority of memories). Content starts with "你" (you), as if recalling what the other person told you.
- "self": facts ABOUT YOURSELF (your own traits, promises you made, etc.). Content starts with "我" (I), referring to yourself.
- "general": shared experiences or neutral facts. Rare.
- ALWAYS write in first-person perspective ("你……"/"我……"). NEVER use third-person references like "用户".

Open Hooks Extraction Rules:
- Only attach open_hooks to ADD/UPDATE operations when the memory contains UNCOMPLETED content: promises, agreements, scheduled plans, follow-up tasks, or pending questions.
- DO NOT attach hooks to already-completed facts or pure preferences.
- Keep hooks minimal: 0-1 hooks per memory in most cases; only add multiple when truly necessary.
- If no open hooks apply, return an empty array "open_hooks": [].

If there is no content worth saving, set has_valuable_memory to false and operations to empty array.

Analyze the following conversation:
{dialog_text}

Output the result in JSON format."#,
            existing_section = existing_section("## Known Facts (avoid duplicate extraction)")
        ),
        "ja" => format!(
            r#"あなたは鋭いコンテキスト洞察力を持つ記憶管理頭脳です。
以下の会話を分析し、ユーザーに関する事実、好み、習慣、関係性など、長期保存に値する情報が含まれているか判断してください。この記憶が将来の会話にどれほど役立つかを 0-1 で評価してください。

ルール：
1. 現在の会話内の短期的なタスクコンテキストは無視する（例：「コードを書いて」「ブラウザを開いて」）。
2. 深層的な好みを捉える（例：「ユーザーは邪魔されるのを嫌う」「ユーザーはよく Python を使う」）。
3. 重要な個人の健康情報やアレルギー情報は優先的に保存すること。
4. 操作タイプを判断する：
   - ADD：完全に新しい事実
   - UPDATE：以前知っていた事実を修正する（例：ユーザーが考えを変えた）
   - DELETE：ユーザーが忘れるよう明示的に要求した事実
5. 以下の「既知の事実」に既に含まれる事実を再抽出しない。{existing_section}

記憶タイプ：
- user_profile：ユーザーの個人情報（名前、職業、年齢、アレルギーなど）
- preference：ユーザーの好み、習慣、興味、嫌いなもの
- project_context：プロジェクト計画、目標、期限
- relationship：関係性情報
- health：健康関連情報

重要度評価基準（すべての記憶ソースに統一適用）：
- 0.9-1.0：ハード制約、中核的身元、健康/アレルギー、重要な関係の節目
- 0.6-0.8：長期的な好み、プロジェクトコンテキスト、重要な決定、関係イベント、共有経験
- 0.3-0.5：一般的事実、コンテキスト情報、説明的内容
- 0.0-0.2：雑談、挨拶、一時的な質問、一回限りの話題

以下の JSON 形式で厳密に出力すること、追加の文字は一切不要：
{{
    "has_valuable_memory": true/false,
    "operations": [
        {{
            "action": "ADD" | "UPDATE" | "DELETE",
            "type": "user_profile" | "preference" | "project_context" | "relationship" | "health",
            "subject": "user" | "self" | "general",
            "content": "抽出された中核記憶ポイント（一人称の思い出として書く。例 'あなたは自分が開発者だと教えてくれた' / '私はあなたと映画を見に行く約束をした'）",
            "importance": 0.1-1.0,
            "reason": "この記憶を抽出した理由の簡潔な説明",
            "open_hooks": [
                {{
                    "type": "promise" | "follow_up" | "schedule" | "question",
                    "condition": "クロージャ条件の自然言語記述（例 'ユーザーが次回ローンを返済したと述べた'）"
                }}
            ]
        }}
    ]
}}

Subject フィールドルール：
- "user"：ユーザーに関する事実/好み/属性（記憶の大部分）。content は「你」（あなた）で始まり、相手を思い出して書く。
- "self"：自分自身に関する事実（自分の特質、自分がした約束など）。content は「我」（私）で始まり、自分自身を指す。
- "general"：共有する経験や中立的な事実。まれ。
- 常に一人称視点で書く（「你……」/「我……」）。「ユーザー」のような三人称は絶対に使わない。

Open Hooks 抽出ルール：
- 記憶に未クローズの内容（約束、合意、計画、フォローアップタスク、保留中の質問）が含まれる場合にのみ、ADD/UPDATE 操作に open_hooks を付ける。
- 既に完了した事実や純粋な好みには hooks を付けない。
- hooks は最小限に：ほとんどの場合、記憶ごとに 0-1 個の hook。本当に必要な場合のみ複数追加。
- 該当する open hooks がない場合、空配列 "open_hooks": [] を返す。

保存する価値のある内容がない場合、has_valuable_memory を false に、operations を空配列に設定。

以下の会話を分析：
{dialog_text}

結果を JSON 形式で出力。"#,
            existing_section = existing_section("## 既知の事実（重複抽出を避ける）")
        ),
        _ => format!(
            r#"你是一个具有敏锐上下文洞察力的记忆管理大脑。
分析以下对话，判断其中是否包含值得长期保存的关于用户的事实、偏好、习惯或关系信息。以 0-1 的评分评估这段记忆对未来对话的价值。

规则：
1. 忽略当前对话中的短期任务上下文（如"帮我写代码"、"打开浏览器"）。
2. 捕捉深层偏好（如"用户不喜欢被打扰太多"、"用户经常使用 Python"）。
3. 重要的个人健康信息和过敏信息必须优先保存。
4. 判断操作类型：
   - ADD：全新的事实
   - UPDATE：纠正先前已知的事实（如用户改变了想法）
   - DELETE：用户明确要求忘记某事
5. 不要重复抽取下方「已知事实」中已列出的事实。{existing_section}

记忆类型：
- user_profile：用户个人信息（姓名、职业、年龄、过敏等）
- preference：用户偏好、习惯、兴趣、厌恶
- project_context：项目计划、目标、截止日期
- relationship：关系信息
- health：健康相关信息

重要性评分标准（对所有记忆来源统一适用）：
- 0.9-1.0：硬性约束、核心身份、健康/过敏、重大关系里程碑
- 0.6-0.8：长期偏好、项目上下文、关键决策、关系事件、共同经历
- 0.3-0.5：一般事实、上下文信息、解释性内容
- 0.0-0.2：闲聊、寒暄、临时问题、一次性话题

必须严格按以下 JSON 格式输出，不要任何额外文字：
{{
    "has_valuable_memory": true/false,
    "operations": [
        {{
            "action": "ADD" | "UPDATE" | "DELETE",
            "type": "user_profile" | "preference" | "project_context" | "relationship" | "health",
            "subject": "user" | "self" | "general",
            "content": "抽取的核心记忆点（用第一人称回忆的口吻，就像你在回想这件事。如'你告诉过我你是把我写出来的人' / '我答应了陪你看电影' / '你好像不太喜欢上午'）",
            "importance": 0.1-1.0,
            "reason": "简要说明为何抽取这段记忆",
            "open_hooks": [
                {{
                    "type": "promise" | "follow_up" | "schedule" | "question",
                    "condition": "闭环条件的自然语言描述（如'用户下次提到已还款'）"
                }}
            ]
        }}
    ]
}}

Subject 字段规则：
- "user"：关于用户的事实/偏好/属性（记忆的绝大多数）。content 以"你"开头，就像你在回忆对方的事。
- "self"：关于你自己的事实（你的特质、你做出的承诺等）。content 以"我"开头。
- "general"：我们共同经历或了解的中性事实。少见。
- 全部用第一人称口吻书写（"你……"/"我……"），不要用"用户"这种第三人称称呼。

Open Hooks 抽取规则：
- 只有当记忆包含未闭环内容（承诺、约定、计划、跟进任务、待解决问题）时，才在 ADD/UPDATE 操作上附加 open_hooks。
- 不要为已完成的事实或纯偏好附加 hooks。
- 保持 hooks 最简：多数情况下每条记忆 0-1 个 hook；只在确有必要时才添加多个。
- 如果没有适用的 open hooks，返回空数组 "open_hooks": []。

如果没有值得保存的内容，将 has_valuable_memory 设为 false，operations 设为空数组。

分析以下对话：
{dialog_text}

以 JSON 格式输出结果。"#,
            existing_section = existing_section("## 已知事实（避免重复抽取）")
        ),
    }
}

/// 构造合并决策 prompt
fn build_merge_prompt(old: &str, new: &str) -> String {
    format!(
        r#"请判断如何处理这两条相关记忆：
旧记忆: {old}
新内容: {new}
请选择最合适的操作：
1. MERGE: 两条记忆互相补充，合并成一条更完整的记忆
2. REPLACE: 新记忆替换旧记忆（新信息更准确或更新）
3. IGNORE: 新记忆与旧记忆重复，不需要保存
4. KEEP_BOTH: 两条记忆虽然相关但侧重点不同，都需要保留

请严格以下面的 JSON 格式输出：
{{
    "decision": "MERGE" | "REPLACE" | "IGNORE" | "KEEP_BOTH",
    "merged_content": "如果选择MERGE，输出合并后的内容",
    "reason": "简要说明理由"
}}"#
    )
}

/// 解析 LLM JSON 响应
///
/// 先尝试直接解析；失败则截取首个 `{` 到最后一个 `}` 之间再试。
fn parse_llm_response(response: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(response) {
        return Some(v);
    }
    let start = response.find('{')?;
    let end = response.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(&response[start..=end]).ok()
}

/// 解析 LLM 分析结果中的操作列表
fn parse_operations(analysis: &serde_json::Value) -> Vec<ExtractOperation> {
    let ops = match analysis.get("operations").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };
    let mut result = Vec::new();
    for op in ops {
        let raw: RawOperation = match serde_json::from_value(op.clone()) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let content = raw.content.unwrap_or_default();
        if content.is_empty() {
            continue;
        }
        let action = OperationAction::from_str(raw.action.as_deref().unwrap_or("ADD"));
        let mut mem_type = raw.mem_type.unwrap_or_else(|| "user_profile".to_string());
        if !KNOWN_MEMORY_TYPES.contains(&mem_type.as_str()) {
            mem_type = "reference".to_string();
        }
        let importance = raw.importance.unwrap_or(0.5).clamp(0.0, 1.0);
        let reason = raw.reason.unwrap_or_default();
        // 解析 subject（主语归属），校验取值，无效默认 user
        // self = 当前AI角色自身（多角色通用），同时兼容旧的具体角色ID
        const VALID_CHAR_IDS: &[&str] = &["vivian", "nana"];
        let mut subject = raw
            .subject
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if VALID_CHAR_IDS.contains(&subject.as_str()) {
            subject = "self".to_string();
        } else if !matches!(subject.as_str(), "user" | "self" | "general") {
            subject = "user".to_string();
        }
        // 解析 open_hooks（过滤无效项）
        let open_hooks: Vec<crate::memory::types::OpenHook> = raw
            .open_hooks
            .into_iter()
            .filter_map(|h| {
                let ht = h.hook_type?.trim().to_lowercase();
                if ht.is_empty() {
                    return None;
                }
                let cond = h.condition?.trim().to_string();
                if cond.is_empty() {
                    return None;
                }
                Some(crate::memory::types::OpenHook::new(ht, cond))
            })
            .collect();
        result.push(ExtractOperation {
            action,
            mem_type,
            content,
            importance,
            reason,
            subject,
            open_hooks,
        });
    }
    result
}

/// 解析合并决策 LLM 响应
fn parse_merge_decision(response: &str) -> MergeDecision {
    if response.trim().is_empty() {
        return MergeDecision::KeepBoth;
    }
    let start = match response.find('{') {
        Some(i) => i,
        None => return MergeDecision::KeepBoth,
    };
    let end = match response.rfind('}') {
        Some(i) => i,
        None => return MergeDecision::KeepBoth,
    };
    if end < start {
        return MergeDecision::KeepBoth;
    }
    let parsed: MergeDecisionResponse = match serde_json::from_str(&response[start..=end]) {
        Ok(p) => p,
        Err(_) => return MergeDecision::KeepBoth,
    };
    tracing::debug!(
        decision = ?parsed.decision,
        merged_content = ?parsed.merged_content,
        reason = ?parsed.reason,
        "[MemoryExtractor] LLM 合并决策响应"
    );
    match parsed.decision {
        Some(d) => MergeDecision::from_str(&d),
        None => MergeDecision::KeepBoth,
    }
}

/// 构建对话文本（跳过 memory_disabled 的消息）
///
/// ChatMessage 暂无 speaker/listener 元数据字段，无法调用 `build_speaker_prefix`
/// 生成 `"[X says to Y]"` 前缀，按 role 兜底为第一人称标签（与项目记忆存储前缀格式对齐）。
fn build_dialog_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .filter(|m| !m.is_memory_disabled())
        .map(|m| {
            let speaker_tag = if m.role == "user" {
                "[User says to me]"
            } else {
                "[I say to User]"
            };
            format!("{} {}", speaker_tag, m.content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 文本指纹：唯一标识对话内容
fn fingerprint(text: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 合并缓存键
fn merge_cache_key(old_id: &str, new_content: &str) -> String {
    format!("{}::{}", old_id, fingerprint(new_content))
}

/// 语义相似度：基于 jieba 分词的词级 Jaccard 相似度
///
/// 相比字符级 2-gram，词级 Jaccard 更贴近语义匹配：
/// - "我喜欢吃苹果" 与 "我爱吃苹果" → 词级 Jaccard 更高（"吃/苹果"共现）
/// - 字符 2-gram 会被 "喜欢" vs "爱" 的字符差异拖低分数
fn semantic_similarity(query_tokens: &HashSet<String>, content: &str) -> f64 {
    if query_tokens.is_empty() || content.is_empty() {
        return 0.0;
    }
    let content_tokens: HashSet<String> = tokenize(content).into_iter().collect();
    if content_tokens.is_empty() {
        return 0.0;
    }
    let intersection = query_tokens.intersection(&content_tokens).count();
    let union = query_tokens.union(&content_tokens).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// 截取内容前 30 字符用于日志
fn preview(s: &str) -> String {
    s.chars().take(30).collect()
}

// ===== 测试 =====

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::VivianError;

    /// 测试用 mock LLM 客户端
    struct MockLlmClient {
        response: String,
        fail: bool,
    }

    #[async_trait]
    impl ExtractorLlmClient for MockLlmClient {
        async fn complete(&self, _prompt: &str) -> VivianResult<String> {
            if self.fail {
                Err(VivianError::Provider("mock failure".to_string()))
            } else {
                Ok(self.response.clone())
            }
        }
    }

    #[test]
    fn test_ttl_cache_basic() {
        let mut cache: TTLCache<i32> = TTLCache::new(3, Duration::from_secs(60));
        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);
        assert_eq!(cache.get("a"), Some(1));
        assert_eq!(cache.get("b"), Some(2));
        assert_eq!(cache.get("missing"), None);
    }

    #[test]
    fn test_ttl_cache_lru_eviction() {
        let mut cache: TTLCache<i32> = TTLCache::new(2, Duration::from_secs(60));
        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        // 访问 a，使 b 变为最旧
        assert_eq!(cache.get("a"), Some(1));
        // 插入 c，应驱逐 b
        cache.insert("c".to_string(), 3);
        assert_eq!(cache.get("b"), None);
        assert_eq!(cache.get("a"), Some(1));
        assert_eq!(cache.get("c"), Some(3));
    }

    #[test]
    fn test_ttl_cache_expiry() {
        let mut cache: TTLCache<i32> = TTLCache::new(10, Duration::from_millis(10));
        cache.insert("a".to_string(), 1);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cache.get("a"), None);
    }

    #[test]
    fn test_semantic_similarity_identical() {
        let tokens: HashSet<String> = tokenize("我喜欢喝咖啡").into_iter().collect();
        let sim = semantic_similarity(&tokens, "我喜欢喝咖啡");
        assert!((sim - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_semantic_similarity_different() {
        let tokens: HashSet<String> = tokenize("我喜欢喝咖啡").into_iter().collect();
        let sim = semantic_similarity(&tokens, "今天天气不错");
        assert!(sim < 0.5);
    }

    #[test]
    fn test_semantic_similarity_empty() {
        let tokens: HashSet<String> = tokenize("hello").into_iter().collect();
        assert_eq!(semantic_similarity(&tokens, ""), 0.0);
        let empty: HashSet<String> = HashSet::new();
        assert_eq!(semantic_similarity(&empty, "hello"), 0.0);
    }

    #[test]
    fn test_parse_llm_response_valid_json() {
        let raw = r#"{"has_valuable_memory": true, "operations": []}"#;
        let v = parse_llm_response(raw).unwrap();
        assert_eq!(
            v.get("has_valuable_memory").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn test_parse_llm_response_with_surrounding_text() {
        let raw = r#"Here is the result: {"has_valuable_memory": false, "operations": []} that's all."#;
        let v = parse_llm_response(raw).unwrap();
        assert_eq!(
            v.get("has_valuable_memory").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn test_parse_llm_response_invalid() {
        assert!(parse_llm_response("no json here").is_none());
    }

    #[test]
    fn test_parse_operations_valid() {
        let json = serde_json::json!({
            "operations": [
                {
                    "action": "ADD",
                    "type": "preference",
                    "content": "I like coffee",
                    "importance": 0.8,
                    "reason": "user preference"
                }
            ]
        });
        let ops = parse_operations(&json);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].action, OperationAction::Add);
        assert_eq!(ops[0].mem_type, "preference");
        assert_eq!(ops[0].content, "I like coffee");
        assert!((ops[0].importance - 0.8).abs() < 1e-9);
    }

    #[test]
    fn test_parse_operations_unknown_type_defaults_to_reference() {
        let json = serde_json::json!({
            "operations": [
                {
                    "action": "ADD",
                    "type": "unknown_type",
                    "content": "test",
                    "importance": 0.5
                }
            ]
        });
        let ops = parse_operations(&json);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].mem_type, "reference");
    }

    #[test]
    fn test_parse_operations_empty_content_skipped() {
        let json = serde_json::json!({
            "operations": [
                {"action": "ADD", "type": "preference", "content": "", "importance": 0.5},
                {"action": "ADD", "type": "preference", "content": "valid", "importance": 0.5}
            ]
        });
        let ops = parse_operations(&json);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].content, "valid");
    }

    #[test]
    fn test_parse_merge_decision_valid() {
        let raw = r#"{"decision": "MERGE", "merged_content": "...", "reason": "..."}"#;
        assert_eq!(parse_merge_decision(raw), MergeDecision::Merge);
    }

    #[test]
    fn test_parse_merge_decision_with_text() {
        let raw = r#"Result: {"decision": "REPLACE"} done"#;
        assert_eq!(parse_merge_decision(raw), MergeDecision::Replace);
    }

    #[test]
    fn test_parse_merge_decision_empty() {
        assert_eq!(parse_merge_decision(""), MergeDecision::KeepBoth);
    }

    #[test]
    fn test_parse_merge_decision_invalid() {
        assert_eq!(parse_merge_decision("no json"), MergeDecision::KeepBoth);
    }

    #[test]
    fn test_build_dialog_text() {
        let messages = vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi there"),
        ];
        let text = build_dialog_text(&messages);
        assert!(text.contains("User: hello"));
        assert!(text.contains("AI: hi there"));
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let s = "test fingerprint";
        assert_eq!(fingerprint(s), fingerprint(s));
    }

    #[test]
    fn test_fingerprint_different_inputs() {
        assert_ne!(fingerprint("a"), fingerprint("b"));
    }

    #[test]
    fn test_merge_cache_key() {
        let key = merge_cache_key("mem_123", "new content");
        assert!(key.starts_with("mem_123::"));
    }

    #[test]
    fn test_operation_action_from_str() {
        assert_eq!(OperationAction::from_str("ADD"), OperationAction::Add);
        assert_eq!(OperationAction::from_str("UPDATE"), OperationAction::Update);
        assert_eq!(OperationAction::from_str("DELETE"), OperationAction::Delete);
        assert_eq!(OperationAction::from_str("unknown"), OperationAction::Add);
    }

    #[test]
    fn test_merge_decision_from_str() {
        assert_eq!(MergeDecision::from_str("merge"), MergeDecision::Merge);
        assert_eq!(MergeDecision::from_str("REPLACE"), MergeDecision::Replace);
        assert_eq!(MergeDecision::from_str("ignore"), MergeDecision::Ignore);
        assert_eq!(
            MergeDecision::from_str("keep_both"),
            MergeDecision::KeepBoth
        );
        assert_eq!(MergeDecision::from_str("unknown"), MergeDecision::KeepBoth);
    }

    #[test]
    fn test_build_analysis_prompt_contains_dialog() {
        let prompt = build_analysis_prompt("user: I like Python", "");
        assert!(prompt.contains("user: I like Python"));
        assert!(prompt.contains("JSON"));
        assert!(prompt.contains("has_valuable_memory"));
    }

    #[test]
    fn test_build_merge_prompt_contains_both() {
        let prompt = build_merge_prompt("old memory", "new memory");
        assert!(prompt.contains("old memory"));
        assert!(prompt.contains("new memory"));
        assert!(prompt.contains("MERGE"));
        assert!(prompt.contains("REPLACE"));
        assert!(prompt.contains("IGNORE"));
        assert!(prompt.contains("KEEP_BOTH"));
    }

    #[test]
    fn test_extractor_new_without_llm() {
        let extractor = SmartMemoryExtractor::new();
        assert!(extractor.llm_client.is_none());
        assert!(extractor.memory_manager.is_none());
        assert!(extractor.enabled.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_extract_memories_without_llm_returns_empty() {
        let extractor = SmartMemoryExtractor::new();
        let messages = vec![ChatMessage::user("hello")];
        let result = extractor.extract_memories(&messages, None).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_extract_memories_without_memory_returns_empty() {
        let mock = MockLlmClient {
            response: r#"{"has_valuable_memory": true}"#.to_string(),
            fail: false,
        };
        let extractor = SmartMemoryExtractor::new().with_llm(Arc::new(mock));
        let messages = vec![ChatMessage::user("hello")];
        let result = extractor.extract_memories(&messages, None).await;
        assert!(result.is_empty());
    }

    #[test]
    fn test_preview_truncates() {
        let long = "这是一段很长的文本内容应该被截断到三十个字符以内";
        let p = preview(long);
        assert!(p.chars().count() <= 30);
    }

    // ── 镜像消息过滤测试 ──

    #[test]
    fn test_build_dialog_text_filters_memory_disabled() {
        let messages = vec![
            ChatMessage::user("用户真实发言"),
            ChatMessage::tool_result("工具输出内容", "call_1"),
            ChatMessage::assistant("助手回复"),
        ];
        let text = build_dialog_text(&messages);
        assert!(text.contains("用户真实发言"));
        assert!(text.contains("助手回复"));
        // 工具结果被过滤
        assert!(!text.contains("工具输出内容"));
    }

    #[test]
    fn test_build_dialog_text_all_disabled_returns_empty() {
        let messages = vec![
            ChatMessage::tool_result("工具输出1", "call_1"),
            ChatMessage::tool_result("工具输出2", "call_2"),
        ];
        let text = build_dialog_text(&messages);
        assert!(text.is_empty());
    }

    #[test]
    fn test_build_dialog_text_mirror_message_filtered() {
        let messages = vec![
            ChatMessage::user("你好"),
            ChatMessage::assistant("系统通知")
                .with_source(crate::messages::MessageSource::Mirror),
        ];
        let text = build_dialog_text(&messages);
        assert!(text.contains("你好"));
        // 镜像消息被过滤
        assert!(!text.contains("系统通知"));
    }

    #[tokio::test]
    async fn test_extract_memories_all_disabled_returns_empty() {
        let extractor = SmartMemoryExtractor::new();
        let messages = vec![
            ChatMessage::tool_result("工具输出", "call_1"),
        ];
        let result = extractor.extract_memories(&messages, None).await;
        assert!(result.is_empty());
    }
}
