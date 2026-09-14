//! 补充回复服务 (AugmentReplyService)。
//!
//! - 关键路径只跑一次 LLM + fast 检索以缩短首字延迟；
//! - 当后台 slow 检索召回 fast 遗漏的重要记忆时，本服务异步发起"补充回复"，
//!   不阻塞主路径（使用 `tokio::spawn` 派发）；
//! - 差异判定为纯规则式（不额外调用 LLM）；
//! - LLM 生成补充回复（通过 `ModelRouter::generate`），LLM 不可用时
//!   降级到模板拼接；
//! - 冷却 / pending 上限 / slow 超时 / 10 分钟用户上下文判定 一应俱全。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::brain::interruption_controller::try_get_interruption_controller;
use crate::cross_character::parse_any_speaker_prefix;
use crate::memory::{MemoryItem, MemoryManager, MemoryType, RetrievalStrategy};
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;
use crate::utils::truncate_chars;

// ── 用户上下文判定阈值（600 秒）──
const USER_FACING_CONTEXT_SECONDS: u64 = 600;

/// pending 队列硬上限，避免 OOM
const MAX_PENDING_ENTRIES: usize = 100;

/// pending 请求有效期（秒）：超过此时间的 Scheduled/Pending 请求视为过时并清除
const PENDING_TTL_SECS: f64 = 120.0;

/// 补充回复请求状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AugmentStatus {
    /// 待处理
    Pending,
    /// 已调度
    Scheduled,
    /// 已跳过
    Skipped,
    /// 已就绪
    Ready,
    /// 已失败
    Failed,
    /// 已取消
    Cancelled,
}

impl Default for AugmentStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// 简化的记忆条目（用于差异判定）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub importance: f64,
    /// 记忆创建时间戳（Unix 秒），用于 prompt 时间感知
    #[serde(default)]
    pub timestamp: f64,
}

impl MemoryEntry {
    /// 从 `MemoryItem` 转换（用于把 slow 检索结果喂给差异判定）。
    pub fn from_memory_item(item: &MemoryItem) -> Self {
        Self {
            id: item.id.clone(),
            content: item.content.clone(),
            importance: item.importance,
            timestamp: item.timestamp,
        }
    }
}

/// 补充回复请求上下文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AugmentRequest {
    pub user_input: String,
    pub first_response_text: String,
    pub first_response_motion: String,
    pub first_response_expression: String,
    /// 角色ID（"vivian" / "nana" 等），用于区分人设
    #[serde(default)]
    pub char_id: String,
    /// fast 检索返回的记忆（关键路径已检索，用于差异判定基准）
    pub fast_memories: Vec<MemoryEntry>,
    /// slow 检索返回的记忆（后台填充）
    #[serde(default)]
    pub slow_memories: Vec<MemoryEntry>,
    /// diff 后新增的重要记忆
    #[serde(default)]
    pub new_memories: Vec<MemoryEntry>,
    #[serde(default)]
    pub status: AugmentStatus,
    pub reason: String,
    pub augment_text: String,
    pub created_at: f64,
    #[serde(default)]
    pub scheduled_at: Option<f64>,
    #[serde(default)]
    pub completed_at: Option<f64>,
}

impl AugmentRequest {
    pub fn new(user_input: impl Into<String>, first_response: impl Into<String>) -> Self {
        Self {
            user_input: user_input.into(),
            first_response_text: first_response.into(),
            first_response_motion: String::new(),
            first_response_expression: String::new(),
            char_id: String::new(),
            fast_memories: Vec::new(),
            slow_memories: Vec::new(),
            new_memories: Vec::new(),
            status: AugmentStatus::Pending,
            reason: String::new(),
            augment_text: String::new(),
            created_at: now_ts(),
            scheduled_at: None,
            completed_at: None,
        }
    }
}

/// 补充回复事件（通过回调通知上层）。
#[derive(Debug, Clone, Serialize)]
pub struct AugmentEvent {
    pub event: String,
    pub augment_text: String,
    pub user_input: String,
    pub first_response: String,
    pub reason: String,
}

impl AugmentEvent {
    pub fn ready(text: impl Into<String>, user_input: impl Into<String>, first_response: impl Into<String>) -> Self {
        Self {
            event: "ready".to_string(),
            augment_text: text.into(),
            user_input: user_input.into(),
            first_response: first_response.into(),
            reason: "ok".to_string(),
        }
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            event: "skipped".to_string(),
            augment_text: String::new(),
            user_input: String::new(),
            first_response: String::new(),
            reason: reason.into(),
        }
    }

    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            event: "failed".to_string(),
            augment_text: String::new(),
            user_input: String::new(),
            first_response: String::new(),
            reason: reason.into(),
        }
    }

    pub fn cancelled(reason: impl Into<String>) -> Self {
        Self {
            event: "cancelled".to_string(),
            augment_text: String::new(),
            user_input: String::new(),
            first_response: String::new(),
            reason: reason.into(),
        }
    }
}

/// 事件回调类型。
pub type AugmentEventCallback = Arc<dyn Fn(AugmentEvent) + Send + Sync>;

/// 聊天历史追加回调类型（由调用方注入，把补充回复写入 TimeStampedMemory / 对话管理器等）。
pub type HistoryAppendCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// 补充回复服务。
///
/// 单例风格，后台异步派发。
///
/// 服务自身可被廉价克隆（所有共享状态都在 `Arc` 后面），便于 `tokio::spawn`
/// 捕获一份克隆在后台执行 slow 检索 + LLM 调用 + 持久化。
#[derive(Clone)]
pub struct AugmentReplyService {
    /// 按角色 ID 索引的记忆管理器（slow 检索 + 持久化写回）
    ///
    /// 多角色架构下必须按 char_id 路由，否则会出现 A 角色触发 AugmentReply
    /// 却召回 B 角色身份记忆的跨角色污染（曾导致 Nana 说"我其实是 Vivian"）。
    memories: Arc<RwLock<HashMap<String, Arc<MemoryManager>>>>,
    /// LLM 路由器（生成补充回复；缺失时降级模板）
    router: Option<Arc<ModelRouter>>,
    /// 事件回调
    on_event: Option<AugmentEventCallback>,
    /// 聊天历史追加回调（可选）
    history_append: Option<HistoryAppendCallback>,
    /// 待处理请求队列（key → request 快照，主要用于计数与去重）
    pending: Arc<Mutex<HashMap<String, AugmentRequest>>>,
    /// 上次成功补充回复时间戳（用于冷却判定）
    last_augment_at: Arc<Mutex<f64>>,
    /// 是否启用
    enabled: Arc<AtomicBool>,
    /// 是否已关闭
    closed: Arc<AtomicBool>,
    // ── 触发配置 ──
    importance_threshold: f64,
    min_new_memories: usize,
    cooldown_seconds: f64,
    max_pending: usize,
    max_augment_len: usize,
    similarity_reject: f64,
    slow_timeout_seconds: f64,
    /// slow 检索策略（"更彻底"检索）
    slow_strategy: RetrievalStrategy,
    /// slow 检索上限
    slow_limit: usize,
}

impl AugmentReplyService {
    pub fn new() -> Self {
        Self {
            memories: Arc::new(RwLock::new(HashMap::new())),
            router: None,
            on_event: None,
            history_append: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            last_augment_at: Arc::new(Mutex::new(0.0)),
            enabled: Arc::new(AtomicBool::new(true)),
            closed: Arc::new(AtomicBool::new(false)),
            importance_threshold: 0.4,
            min_new_memories: 1,
            cooldown_seconds: 120.0,
            max_pending: 2,
            max_augment_len: 200,
            similarity_reject: 0.55,
            slow_timeout_seconds: 4.0,
            // Hybrid = keyword + recency 加权，比关键路径的 Auto 更彻底
            slow_strategy: RetrievalStrategy::Hybrid,
            slow_limit: 24,
        }
    }

    /// 为指定角色注册记忆管理器。
    ///
    /// 多角色架构下，每个角色的 `Brain::build` 都会调用此方法注册自己的 MemoryManager，
    /// `call_slow_retrieve` 会按 `req.char_id` 从中路由对应的实例。
    /// 角色记忆严格隔离，未注册的角色会直接报错而非 fallback 到其他角色的记忆库。
    pub fn register_memory_for_char(&self, char_id: &str, memory: Arc<MemoryManager>) {
        self.memories.write().insert(char_id.to_string(), memory);
    }

    /// 注入 LLM 路由器（builder 风格）。
    pub fn with_router(mut self, router: Arc<ModelRouter>) -> Self {
        self.router = Some(router);
        self
    }

    /// 注入事件回调（builder 风格）。
    pub fn with_event_callback(mut self, cb: AugmentEventCallback) -> Self {
        self.on_event = Some(cb);
        self
    }

    /// 注入聊天历史追加回调（builder 风格）。
    pub fn with_history_append(mut self, cb: HistoryAppendCallback) -> Self {
        self.history_append = Some(cb);
        self
    }

    /// 设置启用状态。
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// 关闭服务：清空 pending，正在执行的后台任务会在持久化前自检 `closed` 并放弃。
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        let mut pending = self.pending.lock();
        for (_, req) in pending.iter_mut() {
            if matches!(req.status, AugmentStatus::Pending | AugmentStatus::Scheduled) {
                req.status = AugmentStatus::Cancelled;
                req.reason = "service_closed".to_string();
            }
        }
        pending.clear();
    }

    /// 调度一次补充回复。
    ///
    /// 1. 禁用 / 已关闭 → 返回 None
    /// 2. 缺少 memory 依赖 → 返回 None（router 缺失时仍可走模板降级，故不强制）
    /// 3. 用户已离开当前上下文 → 返回 None
    /// 4. 冷却中 / pending 超限 → 返回 None
    /// 5. 加入 pending 队列，`tokio::spawn` 后台执行 slow 检索 + LLM + 持久化
    ///
    /// 返回的 `AugmentRequest` 仅为已调度的快照（状态 = Scheduled），
    /// 真正的完成状态通过 `on_event` 回调通知。
    pub fn schedule(
        &self,
        user_input: &str,
        first_response_text: &str,
        first_response_motion: &str,
        first_response_expression: &str,
        fast_memories: &[MemoryEntry],
        user_message_id: Option<&str>,
        char_id: &str,
    ) -> Option<AugmentRequest> {
        if !self.enabled.load(Ordering::Relaxed) || self.closed.load(Ordering::Relaxed) {
            return None;
        }
        // 该角色的 MemoryManager 是 slow 检索的必需依赖；router 缺失可降级模板
        if !self.memories.read().contains_key(char_id) {
            tracing::debug!(
                "[AugmentReply] 角色 {} 未注册 MemoryManager，跳过",
                char_id
            );
            return None;
        }
        if user_input.trim().is_empty() || first_response_text.trim().is_empty() {
            return None;
        }
        if !self.is_in_user_facing_context() {
            return None;
        }

        let now = now_ts();
        // 冷却判定（独立锁，避免与 pending 嵌套）
        let last_augment = *self.last_augment_at.lock();
        if now - last_augment < self.cooldown_seconds {
            tracing::debug!("[AugmentReply] 冷却中，距上次 {:.1}s", now - last_augment);
            return None;
        }

        // 构造请求并入队（pending 锁仅在临界区内持有）
        let (key, request_snapshot) = {
            let mut pending = self.pending.lock();
            pending.retain(|_, req| {
                let age = now - req.created_at;
                let stale = age > PENDING_TTL_SECS
                    && matches!(req.status, AugmentStatus::Scheduled | AugmentStatus::Pending);
                if stale {
                    tracing::info!(
                        "[AugmentReply] 清除过时 pending 请求（age={:.0}s）：{}",
                        age,
                        crate::utils::truncate_chars(&req.user_input, 40)
                    );
                }
                !stale
            });
            // pending 上限：取 max_pending 与硬上限 MAX_PENDING_ENTRIES 的较小值
            // 防止 max_pending 被配置过大时，LLM 长期失败导致 OOM
            let effective_cap = self.max_pending.min(MAX_PENDING_ENTRIES);
            if pending.len() >= effective_cap {
                if pending.len() >= MAX_PENDING_ENTRIES {
                    tracing::warn!(
                        len = pending.len(),
                        cap = MAX_PENDING_ENTRIES,
                        "[augment_reply] pending 队列已达硬上限，拒绝新请求避免 OOM"
                    );
                } else {
                    tracing::debug!("[AugmentReply] 已有过多 pending，丢弃新请求");
                }
                return None;
            }
            let mut request = AugmentRequest::new(user_input, first_response_text);
            request.first_response_motion = first_response_motion.to_string();
            request.first_response_expression = first_response_expression.to_string();
            request.char_id = char_id.to_string();
            request.fast_memories = fast_memories.to_vec();
            request.status = AugmentStatus::Scheduled;
            request.scheduled_at = Some(now);
            let key = user_message_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("augment_{}", now as u64));
            pending.insert(key.clone(), request.clone());
            (key, request)
        };

        // 派发到 tokio 运行时（fire-and-forget），此处不持有任何锁
        let this = self.clone();
        let req_for_task = request_snapshot.clone();
        let key_for_task = key.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    this.run_slow_search(key_for_task, req_for_task).await;
                });
            }
            Err(e) => {
                tracing::warn!("[AugmentReply] 无可用 tokio 运行时，放弃派发: {}", e);
                self.mark_done(
                    &key,
                    request_snapshot.clone(),
                    AugmentStatus::Failed,
                    "no_runtime".to_string(),
                    String::new(),
                );
                return None;
            }
        }

        Some(request_snapshot)
    }

    /// 检查用户是否仍在当前对话上下文（interruption_controller 视角）。
    ///
    /// 距上次用户活动 < 10 分钟视为仍在上下文。
    /// 控制器不可用时默认返回 true（不阻塞）。
    pub fn is_in_user_facing_context(&self) -> bool {
        match try_get_interruption_controller() {
            Some(controller) => {
                let readiness = controller.get_interruption_readiness();
                readiness.idle_seconds < USER_FACING_CONTEXT_SECONDS
            }
            None => true,
        }
    }

    /// 后台执行：slow 检索 + 差异判定 + LLM 生成 + 防复读 + 持久化。
    async fn run_slow_search(&self, key: String, mut req: AugmentRequest) {
        // 服务已关闭 → 取消
        if self.closed.load(Ordering::Relaxed) {
            self.mark_done(&key, req, AugmentStatus::Cancelled, "service_closed", String::new());
            return;
        }

        // 1) slow 检索（带超时）
        let t0 = now_ts();
        let slow_results = match self.call_slow_retrieve(&req).await {
            Ok(items) => items,
            Err(e) => {
                tracing::debug!("[AugmentReply] slow 检索失败: {}", e);
                self.mark_done(&key, req, AugmentStatus::Failed, "retrieve_error", String::new());
                return;
            }
        };
        let elapsed = now_ts() - t0;
        tracing::debug!("[AugmentReply] slow 检索耗时 {:.2}s", elapsed);
        if elapsed > self.slow_timeout_seconds {
            self.mark_done(&key, req, AugmentStatus::Skipped, "slow_timeout", String::new());
            return;
        }

        // 2) 用户在 slow 期间离开 → 取消
        if !self.is_in_user_facing_context() {
            self.mark_done(&key, req, AugmentStatus::Cancelled, "user_left", String::new());
            return;
        }

        // 3) 差异判定
        let (new_memories, diff_reason) =
            diff_slow_vs_fast(&slow_results, &req.fast_memories, self.importance_threshold);
        if diff_reason != "ok" || new_memories.len() < self.min_new_memories {
            self.mark_done(
                &key,
                req,
                AugmentStatus::Skipped,
                format!("diff:{}", diff_reason),
                String::new(),
            );
            return;
        }
        req.slow_memories = slow_results;
        req.new_memories = new_memories.clone();

        // 4) 生成补充回复（LLM 优先，降级模板）
        let augment_text = self.generate_augment_text(&req).await;
        let augment_text = augment_text.trim().to_string();
        if augment_text.len() < 4 {
            self.mark_done(&key, req, AugmentStatus::Failed, "empty_response", String::new());
            return;
        }

        // 5) 防复读
        let sim = text_similarity(&augment_text, &req.first_response_text);
        if sim > self.similarity_reject {
            tracing::debug!("[AugmentReply] 与原回复相似度 {:.2} 过高，丢弃", sim);
            self.mark_done(
                &key,
                req,
                AugmentStatus::Skipped,
                format!("too_similar:{:.2}", sim),
                String::new(),
            );
            return;
        }

        // 6) 清理 + 持久化
        let augment_text = Self::cleanup_augment_text(&augment_text, self.max_augment_len);
        if augment_text.is_empty() {
            self.mark_done(&key, req, AugmentStatus::Skipped, "cleaned_empty", String::new());
            return;
        }

        self.persist_augment(&req, &augment_text).await;

        req.augment_text = augment_text.clone();
        req.completed_at = Some(now_ts());
        self.mark_done(&key, req, AugmentStatus::Ready, "ok", augment_text);
    }

    /// 调用 MemoryManager 的 slow 检索。
    ///
    /// 严格按 `req.char_id` 路由到对应角色的 MemoryManager。
    /// 角色记忆库完全隔离，未注册的 char_id 直接返回错误，不会 fallback 到其他角色的记忆库。
    /// 使用更彻底的策略 + 更大上限。
    async fn call_slow_retrieve(&self, req: &AugmentRequest) -> Result<Vec<MemoryEntry>, String> {
        let memory = {
            let map = self.memories.read();
            map.get(&req.char_id).cloned()
        }
        .ok_or_else(|| format!("no_memory_for_char_{}", req.char_id))?;
        let items = memory
            .search_memories(&req.user_input, self.slow_strategy, self.slow_limit)
            .await
            .map_err(|e| e.to_string())?;
        // 过滤掉与当前用户输入相同的记忆（UserMemorySaving 刚写入的当前轮 ShortTerm），
        // 否则 diff_slow_vs_fast 会把它当作"新记忆"触发无意义的补充回复。
        let user_trimmed = req.user_input.trim();
        let filtered: Vec<_> = items
            .into_iter()
            .filter(|m| {
                let stripped = parse_any_speaker_prefix(&m.content).0;
                stripped.trim() != user_trimmed
            })
            .collect();
        // LLM 精选：关键词粗筛 + LLM 挑选，降低注入噪声
        const PREFILTER_K: usize = 15;
        const TARGET_N: usize = 5;
        let refined = crate::memory::refiner::refine_candidates(
            filtered,
            &req.user_input,
            self.router.as_ref(),
            &req.char_id,
            PREFILTER_K,
            TARGET_N,
        )
        .await;
        Ok(refined.iter().map(MemoryEntry::from_memory_item).collect())
    }

    /// 生成补充回复文本。
    ///
    /// 优先 `ModelRouter::generate`；LLM 不可用 / 调用失败时降级到模板拼接。
    async fn generate_augment_text(&self, req: &AugmentRequest) -> String {
        let system_prompt = Self::build_augment_system_prompt(&req.char_id);
        let user_prompt = Self::build_augment_prompt(req);

        // 优先走 LLM
        if let Some(router) = &self.router {
            let messages = vec![
                ChatMessage::system(system_prompt),
                ChatMessage::user(user_prompt),
            ];
            match router.generate(LLMRequest::new("chat", messages)).await {
                Ok(raw) => {
                    let text = raw.trim().to_string();
                    if !text.is_empty() {
                        return text;
                    }
                }
                Err(e) => {
                    tracing::debug!("[AugmentReply] LLM 调用失败，降级模板: {}", e);
                }
            }
        }

        // 降级：模板拼接
        Self::template_augment_text(&req.new_memories)
    }

    /// 模板拼接的补充回复（LLM 不可用时的降级方案）。
    fn template_augment_text(new_memories: &[MemoryEntry]) -> String {
        if new_memories.is_empty() {
            return String::new();
        }
        // 取最重要的一条
        let top = new_memories
            .iter()
            .max_by(|a, b| {
                a.importance
                    .partial_cmp(&b.importance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|m| m.content.clone())
            .unwrap_or_default();
        if top.is_empty() {
            return String::new();
        }
        let preview: String = truncate_chars(&top, 60);
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        match lang_norm {
            "en" => format!("Oh right, {}...", preview),
            "ja" => format!("あそうだ、{}…", preview),
            _ => format!("哦对了，{}…", preview),
        }
    }

    /// 构造补充回复的 system prompt。
    fn build_augment_system_prompt(char_id: &str) -> String {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        match lang_norm {
            "en" => {
                let (name, persona) = match char_id {
                    "nana" | "娜娜" => ("Nana", "a gentle, composed older-sister type — warm, grounded, speaks softly but with quiet strength"),
                    _ => ("Vivian", "a warm-hearted weeb netizen — fluent in anime culture and internet surfing, lively and genuine"),
                };
                format!(
                    "You are {}, {}. \
                     Your style: natural, sincere. Please strictly follow all constraints given in the user message.",
                    name, persona
                )
            }
            "ja" => {
                let (name, persona) = match char_id {
                    "nana" | "娜娜" => ("ナナ", "優しく落ち着いたお姉さんタイプ——温かくて地に足がついていて、穏やかに話すが芯がある"),
                    _ => ("ヴィヴィアン", "ネットに生きる心温かいオタク少女——アニメ文化とネットサーフィンに精通、活発で素直"),
                };
                format!(
                    "あなたは{}、{}。\
                     スタイル：自然、素直。ユーザーメッセージのすべての制約に厳密に従うこと。",
                    name, persona
                )
            }
            _ => {
                let (name, persona) = match char_id {
                    "nana" | "娜娜" => ("娜娜", "温柔从容的姐姐——温暖、踏实，说话轻声细语但有力量"),
                    _ => ("薇薇安", "生活在网络上的暖心二次元少女——精通动漫文化和网络冲浪，活泼真诚"),
                };
                format!(
                    "你是{}，{}。\
                     你的风格：自然、真诚。\
                     请严格遵循用户消息中给出的所有约束。",
                    name, persona
                )
            }
        }
    }

    /// 构造补充回复的 user prompt。
    fn build_augment_prompt(req: &AugmentRequest) -> String {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());

        // 用户问题（截断 200 字符）—— LLM 据此判断新记忆是否与当前话题相关
        let user_raw = req.user_input.trim();
        let user_question: String = if user_raw.chars().count() > 200 {
            let truncated: String = truncate_chars(user_raw, 200);
            format!("{}…", truncated)
        } else {
            user_raw.to_string()
        };

        // 拼接新增记忆（最多 5 条，token 预算提升后可容纳更多）
        // 每条附带元数据标签 [时间 | 重要度]——时间感知让 LLM 能区分刚发生的事和旧事
        // 按 importance 升序排序（重要的排在后面，LLM 对列表末尾注意力更佳）
        let mut sorted_memories: Vec<&MemoryEntry> = req.new_memories.iter().collect();
        sorted_memories.sort_by(|a, b| a.importance.partial_cmp(&b.importance).unwrap_or(std::cmp::Ordering::Equal));
        let now_ts = crate::memory::types::current_timestamp();
        let mem_lines: Vec<String> = sorted_memories
            .iter()
            .take(5)
            .filter_map(|mem| {
                let content = mem.content.trim();
                if content.is_empty() {
                    None
                } else {
                    let time_label = format_memory_time_label(mem.timestamp, now_ts, lang_norm);
                    let importance_label = match lang_norm {
                        "en" => format!("Importance {:.2}", mem.importance),
                        "ja" => format!("重要度 {:.2}", mem.importance),
                        _ => format!("重要度{:.2}", mem.importance),
                    };
                    let label = format!("{} | {}", time_label, importance_label);
                    let truncated: String = truncate_chars(content, 200);
                    if content.chars().count() > 200 {
                        Some(format!("- [{}] {}…", label, truncated))
                    } else {
                        Some(format!("- [{}] {}", label, truncated))
                    }
                }
            })
            .collect();
        let mem_block = if mem_lines.is_empty() {
            match lang_norm {
                "en" => "(none)".to_string(),
                "ja" => "（なし）".to_string(),
                _ => "（无）".to_string(),
            }
        } else {
            mem_lines.join("\n")
        };

        let first_raw = req.first_response_text.trim();
        let first: String = if first_raw.chars().count() > 240 {
            let truncated: String = truncate_chars(first_raw, 240);
            format!("{}…", truncated)
        } else {
            first_raw.to_string()
        };

        match lang_norm {
            "en" => format!(
                "You just sent a reply to the user.\n\
                 [User's Question]\n{user_question}\n\n\
                 [Your previous reply]\n{first}\n\n\
                 Now you suddenly remember some important information you didn't mention:\n\
                 [Newly recalled memory]\n{mem_block}\n\n\
                 [Requirements]\n\
                 1. Add the newly recalled information with a natural transition.\n\
                 2. Suggested openings: \"Oh right…\" / \"Speaking of which…\" / \"By the way…\" / \"I just remembered…\" — or omit.\n\
                 3. Strictly do not repeat what you just said.\n\
                 4. 1-2 sentences, under 60 chars.\n\
                 5. No markdown, no line breaks, no JSON.\n\
                 6. If the new info is completely unrelated to the user's question or the previous reply, output NONE.\n\
                 7. Your supplement must continue the same topic thread as [Your previous reply]. Do not re-interpret the user's question from a different angle.\n\n\
                 Augmented reply:"
            ),
            "ja" => format!(
                "さっきユーザーに返信したばかりだ。\n\
                 [ユーザーの質問]\n{user_question}\n\n\
                 [さっきの返信]\n{first}\n\n\
                 その後、まだ伝えていなかった重要なことを思い出した：\n\
                 [思い出した記憶]\n{mem_block}\n\n\
                 [要件]\n\
                 1. 思い出した情報を自然な繋ぎで補足する。\n\
                 2. 冒頭は「あそうだ…」「それで思い出したけど…」「そういえば…」「あさっき思い出したんだけど…」\
                 などの自然な过渡を使うか、省略してもいい。\n\
                 3. さっき言った内容を厳格に繰り返さない。\n\
                 4. 1-2文、60字以内。\n\
                 5. markdown、改行、JSONは使わない。\n\
                 6. 新しい情報がユーザーの質問やさっきの返信と完全に関係ない場合、NONEを出力する。\n\
                 7. 補足は必ず[さっきの返信]と同じ話題の流れに沿うこと。ユーザーの質問を別の角度から解釈し直さない。\n\n\
                 補足返信："
            ),
            _ => format!(
                "你刚刚对用户说了一句回复。\n\
                 [用户的问题]\n{user_question}\n\n\
                 [你刚才的回复]\n{first}\n\n\
                 现在你突然想起了一些之前没提到的重要信息：\n\
                 [刚想起来的记忆]\n{mem_block}\n\n\
                 [要求]\n\
                 1. 用一句自然的衔接把新想起来的信息补充进去。\n\
                 2. 开头建议使用「哦对了…」「说到这个我想起来…」「对了…」「啊我刚想起来…」\
                 之类的自然过渡，也可以省略。\n\
                 3. 严格不要重复刚才说过的内容。\n\
                 4. 1-2 句话，不超过 60 字。\n\
                 5. 不要使用 markdown、不要换行、不要 JSON。\n\
                 6. 如果新信息与用户的问题或刚才的回复主题完全无关，输出 NONE。\n\
                 7. 补充内容必须延续[你刚才的回复]的话题方向，不要对用户的问题做新的解读。\n\n\
                 补充回复："
            ),
        }
    }

    /// 清理补充回复文本：去 markdown 围栏、去 NONE、限长。
    fn cleanup_augment_text(text: &str, max_len: usize) -> String {
        let mut text = text.trim().to_string();
        // 去掉 markdown 围栏
        if text.starts_with("```") {
            text = text.trim_matches('`').trim().to_string();
        }
        // 去掉 NONE / 否定（精确匹配）
        let upper = text.trim().to_uppercase();
        if matches!(upper.as_str(), "NONE" | "NULL" | "无" | "无补充" | "（无）" | "(无)") {
            return String::new();
        }
        // JSON 包裹检测：全 NONE → 丢弃；有实际文本 → 提取 response/text/reply 字段
        if text.trim().starts_with('{') {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(obj) = val.as_object() {
                    let all_none = obj.values().all(|v| {
                        let s = v.as_str().unwrap_or("").trim().to_uppercase();
                        matches!(s.as_str(), "NONE" | "NULL" | "无" | "" | "无补充")
                    });
                    if all_none {
                        return String::new();
                    }
                    // LLM 无视了"不要 JSON"指令，从 JSON 中提取实际文本
                    for key in &["output", "response", "text", "reply", "content", "augment"] {
                        if let Some(v) = obj.get(*key).and_then(|v| v.as_str()) {
                            let v = v.trim();
                            if !v.is_empty() {
                                tracing::debug!(
                                    "[AugmentReply] LLM 返回 JSON 包裹，已提取 {:?} 字段",
                                    key
                                );
                                text = v.to_string();
                                break;
                            }
                        }
                    }
                }
            }
        }
        // 限制长度
        let char_count = text.chars().count();
        if char_count > max_len {
            let truncated: String = text.chars().take(max_len).collect();
            text = format!("{}…", truncated.trim_end());
        }
        text
    }

    /// 持久化补充回复：写入聊天历史回调 + 记忆系统。
    ///
    /// 持久化到 history_persistence / dialogue_manager / memory_manager。
    /// 所有写入均为"尽力而为"，失败仅记录日志，不影响主路径。
    async fn persist_augment(&self, req: &AugmentRequest, augment_text: &str) {
        // 1) 聊天历史 / 对话管理器（由调用方注入的回调）
        if let Some(cb) = &self.history_append {
            // 用 catch_unwind 防止单个监听器 panic 影响后续持久化
            let text_owned = augment_text.to_string();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cb(&text_owned);
            }));
        }

        // 2) 记忆系统：作为 assistant 补充回复沉淀（importance 0.25）
        //    严格按 req.char_id 路由到对应角色的 MemoryManager，角色记忆库完全隔离
        let memory_opt = {
            let map = self.memories.read();
            map.get(&req.char_id).cloned()
        };
        if let Some(memory) = memory_opt {
            let meta = serde_json::json!({
                "channel": "direct",
                "listener": "user",
                "perspective": "speaker",
                "knowledge_source": "direct",
            });
            if let Err(e) = memory
                .add_memory_with_metadata(
                    augment_text,
                    MemoryType::CasualConversation,
                    0.25,
                    vec!["chat_augment".to_string(), "dialogue_turn".to_string()],
                    meta,
                )
                .await
            {
                tracing::debug!("[AugmentReply] 写入记忆系统失败: {}", e);
            }
        } else {
            tracing::warn!(
                "[AugmentReply] 角色 {} 未注册 MemoryManager，补充回复未写入记忆系统",
                req.char_id
            );
        }

        // 记录一次调试日志（含触发上下文，便于排查）
        tracing::debug!(
            "[AugmentReply] 补充回复已持久化 | user=\"{}\" | first=\"{}\" | augment=\"{}\"",
            req.user_input,
            req.first_response_text,
            augment_text
        );
    }

    /// 标记请求完成：更新状态、维护冷却时间戳、从 pending 移除、广播事件。
    fn mark_done(
        &self,
        key: &str,
        mut req: AugmentRequest,
        status: AugmentStatus,
        reason: impl Into<String>,
        augment_text: impl Into<String>,
    ) {
        let reason: String = reason.into();
        let augment_text: String = augment_text.into();
        req.status = status;
        req.reason = reason.clone();
        if !augment_text.is_empty() {
            req.augment_text = augment_text.clone();
        }
        if status == AugmentStatus::Ready {
            *self.last_augment_at.lock() = now_ts();
        }
        self.pending.lock().remove(key);

        let event = match status {
            AugmentStatus::Ready => AugmentEvent::ready(
                req.augment_text.clone(),
                req.user_input.clone(),
                req.first_response_text.clone(),
            ),
            AugmentStatus::Skipped => AugmentEvent::skipped(reason),
            AugmentStatus::Failed => AugmentEvent::failed(reason),
            AugmentStatus::Cancelled => AugmentEvent::cancelled(reason),
            _ => AugmentEvent::skipped("done"),
        };
        self.emit_event(event);
    }

    fn emit_event(&self, event: AugmentEvent) {
        if let Some(cb) = &self.on_event {
            // 捕获 panic，避免回调失败影响后台任务
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cb(event);
            }));
        }
    }

    /// 获取当前待处理请求数。
    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }

    /// 获取 slow 检索的超时阈值（秒）。便于测试与上层查询。
    pub fn slow_timeout_seconds(&self) -> f64 {
        self.slow_timeout_seconds
    }

    /// 获取冷却阈值（秒）。
    pub fn cooldown_seconds(&self) -> f64 {
        self.cooldown_seconds
    }
}

impl Default for AugmentReplyService {
    fn default() -> Self {
        Self::new()
    }
}

// ── 单例 ──

use tokio::sync::OnceCell;

static GLOBAL_SERVICE: OnceCell<Arc<AugmentReplyService>> = OnceCell::const_new();

/// 获取全局补充回复服务单例（未初始化时返回一个空壳实例）。
pub async fn get_augment_reply_service() -> Arc<AugmentReplyService> {
    GLOBAL_SERVICE
        .get_or_init(|| async { Arc::new(AugmentReplyService::new()) })
        .await
        .clone()
}

/// 同步获取全局单例（若已初始化）。
pub fn try_get_augment_reply_service() -> Option<Arc<AugmentReplyService>> {
    GLOBAL_SERVICE.get().cloned()
}

/// 初始化（或重新初始化）全局补充回复服务单例。
///
/// 注意：`OnceCell` 一旦初始化无法覆盖，重复调用不会生效。
/// 若需替换实例，请在应用启动早期调用。
pub fn init_augment_reply_service(service: AugmentReplyService) -> Arc<AugmentReplyService> {
    let _ = GLOBAL_SERVICE.set(Arc::new(service));
    GLOBAL_SERVICE
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(AugmentReplyService::new()))
}

// ── 差异判定 ──

/// 规则式差异判定：返回 (新增的重要记忆, 原因)。
///
/// 1. 收集 fast_memories 的 id 集合
/// 2. 在 slow_memories 中找出 id 不在 fast 集合、且 importance >= 阈值 的记忆
/// 3. 内容去重：即使 id 不同，内容相同或前 50 字符前缀相同也视为已存在
pub fn diff_slow_vs_fast(
    slow: &[MemoryEntry],
    fast: &[MemoryEntry],
    importance_threshold: f64,
) -> (Vec<MemoryEntry>, String) {
    if slow.is_empty() {
        return (Vec::new(), "empty_slow".to_string());
    }

    let fast_ids: HashSet<&str> = fast.iter().map(|m| m.id.as_str()).collect();
    let fast_contents: HashSet<String> = fast
        .iter()
        .map(|m| m.content.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut new_memories: Vec<MemoryEntry> = Vec::new();
    for mem in slow {
        // id 已在 fast 中
        if fast_ids.contains(mem.id.as_str()) {
            continue;
        }

        let content = mem.content.trim();
        if content.is_empty() {
            continue;
        }

        // 内容去重
        if fast_contents.contains(content) {
            continue;
        }

        // 50 字符前缀匹配：去 100 字符的潜在标点/空白差异
        let head: String = truncate_chars(content, 50);
        let head_in_fast = fast_contents.iter().any(|fc| {
            let fc_chars: Vec<char> = fc.chars().collect();
            fc_chars.len() >= 50 && {
                let fc_head: String = fc_chars.iter().take(50).collect();
                fc_head == head
            }
        });
        if head_in_fast {
            continue;
        }

        // 重要性过滤
        if mem.importance < importance_threshold {
            continue;
        }

        new_memories.push(mem.clone());
    }

    if new_memories.is_empty() {
        return (Vec::new(), "no_significant_new_memories".to_string());
    }

    (new_memories, "ok".to_string())
}

// ── 文本相似度 ──

/// 字符 n-gram Jaccard 相似度 [0, 1]。
pub fn text_similarity(a: &str, b: &str) -> f64 {
    let sa = char_ngrams(a, 3);
    let sb = char_ngrams(b, 3);
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn char_ngrams(text: &str, n: usize) -> HashSet<String> {
    let trimmed = text.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() < n {
        return if trimmed.is_empty() {
            HashSet::new()
        } else {
            let mut s = HashSet::new();
            s.insert(trimmed.to_string());
            s
        };
    }
    (0..=chars.len() - n)
        .map(|i| chars[i..i + n].iter().collect())
        .collect()
}

// ── 工具函数 ──

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// 将记忆时间戳格式化为人类可读的时间标签（含距今跨度），让 LLM 感知记忆的新旧
fn format_memory_time_label(ts: f64, now: f64, lang: &str) -> String {
    if ts <= 0.0 {
        return match lang {
            "en" => "Unknown time".to_string(),
            "ja" => "時期不明".to_string(),
            _ => "时间不详".to_string(),
        };
    }
    let dt = chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|d| d.with_timezone(&chrono::Local))
        .unwrap_or_else(|| chrono::Local::now());
    let now_dt = chrono::DateTime::from_timestamp(now as i64, 0)
        .map(|d| d.with_timezone(&chrono::Local))
        .unwrap_or_else(|| chrono::Local::now());
    let elapsed = now_dt.signed_duration_since(dt);
    let hours = elapsed.num_hours();
    let days = elapsed.num_days();

    let date_str = if dt.date_naive() == now_dt.date_naive() {
        match lang {
            "en" => dt.format("Today %H:%M").to_string(),
            "ja" => dt.format("今日 %H:%M").to_string(),
            _ => dt.format("今天 %H:%M").to_string(),
        }
    } else if dt.date_naive() == (now_dt - chrono::Duration::days(1)).date_naive() {
        match lang {
            "en" => dt.format("Yesterday %H:%M").to_string(),
            "ja" => dt.format("昨日 %H:%M").to_string(),
            _ => dt.format("昨天 %H:%M").to_string(),
        }
    } else if days < 7 {
        dt.format("%m-%d %H:%M").to_string()
    } else {
        dt.format("%m-%d").to_string()
    };

    let span = if hours < 1 {
        let mins = elapsed.num_minutes().max(1);
        match lang {
            "en" => format!("{}m ago", mins),
            "ja" => format!("{}分前", mins),
            _ => format!("{}分钟前", mins),
        }
    } else if hours < 24 {
        match lang {
            "en" => format!("{}h ago", hours),
            "ja" => format!("{}時間前", hours),
            _ => format!("{}小时前", hours),
        }
    } else {
        match lang {
            "en" => format!("{}d ago", days),
            "ja" => format!("{}日前", days),
            _ => format!("{}天前", days),
        }
    };

    format!("{}（{}）", date_str, span)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_empty_slow() {
        let (new, reason) = diff_slow_vs_fast(&[], &[], 0.4);
        assert!(new.is_empty());
        assert_eq!(reason, "empty_slow");
    }

    #[test]
    fn test_diff_finds_new() {
        let slow = vec![
            MemoryEntry {
                id: "s1".to_string(),
                content: "用户喜欢咖啡".to_string(),
                importance: 0.8,
                timestamp: 0.0,
            },
            MemoryEntry {
                id: "s2".to_string(),
                content: "无关紧要".to_string(),
                importance: 0.1,
                timestamp: 0.0,
            },
        ];
        let fast = vec![MemoryEntry {
            id: "f1".to_string(),
            content: "其他".to_string(),
            importance: 0.5,
            timestamp: 0.0,
        }];

        let (new, reason) = diff_slow_vs_fast(&slow, &fast, 0.4);
        assert_eq!(reason, "ok");
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].id, "s1");
    }

    #[test]
    fn test_diff_dedup_by_content() {
        let slow = vec![MemoryEntry {
            id: "s1".to_string(),
            content: "重复内容".to_string(),
            importance: 0.8,
            timestamp: 0.0,
        }];
        let fast = vec![MemoryEntry {
            id: "f1".to_string(),
            content: "重复内容".to_string(),
            importance: 0.5,
            timestamp: 0.0,
        }];

        let (new, reason) = diff_slow_vs_fast(&slow, &fast, 0.4);
        assert!(new.is_empty());
        assert_eq!(reason, "no_significant_new_memories");
    }

    #[test]
    fn test_diff_dedup_by_50char_prefix() {
        // slow 与 fast 内容不同但前 50 字符相同 → 视为重复
        let prefix: String = "用户昨天提到他非常喜欢吃一种很特别的甜点那就是".to_string();
        let slow_content = format!("{}，并且每周都会买一次。", prefix);
        let fast_content = format!("{}，而且自己也会做。", prefix);
        let slow = vec![MemoryEntry {
            id: "s1".to_string(),
            content: slow_content,
            importance: 0.8,
            timestamp: 0.0,
        }];
        let fast = vec![MemoryEntry {
            id: "f1".to_string(),
            content: fast_content,
            importance: 0.5,
            timestamp: 0.0,
        }];
        let (new, reason) = diff_slow_vs_fast(&slow, &fast, 0.4);
        assert!(new.is_empty());
        assert_eq!(reason, "no_significant_new_memories");
    }

    #[test]
    fn test_text_similarity_identical() {
        let sim = text_similarity("你好世界", "你好世界");
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_text_similarity_different() {
        let sim = text_similarity("你好世界", "完全不同");
        assert!(sim < 0.5);
    }

    #[test]
    fn test_template_augment_text() {
        let mems = vec![MemoryEntry {
            id: "s1".to_string(),
            content: "用户对小麦过敏".to_string(),
            importance: 0.9,
            timestamp: 0.0,
        }];
        let text = AugmentReplyService::template_augment_text(&mems);
        assert!(text.starts_with("哦对了，"));
        assert!(text.contains("用户对小麦过敏"));

        // 空输入
        assert!(AugmentReplyService::template_augment_text(&[]).is_empty());
    }

    #[test]
    fn test_cleanup_augment_text_strips_fences() {
        let cleaned = AugmentReplyService::cleanup_augment_text("```\n哦对了，过敏\n```", 200);
        assert!(!cleaned.contains("```"));
        assert!(cleaned.contains("哦对了"));
    }

    #[test]
    fn test_cleanup_augment_text_none_returns_empty() {
        assert!(AugmentReplyService::cleanup_augment_text("NONE", 200).is_empty());
        assert!(AugmentReplyService::cleanup_augment_text("无", 200).is_empty());
        assert!(AugmentReplyService::cleanup_augment_text("（无）", 200).is_empty());
    }

    #[test]
    fn test_cleanup_augment_text_truncates() {
        let long = "啊".repeat(300);
        let cleaned = AugmentReplyService::cleanup_augment_text(&long, 50);
        assert!(cleaned.chars().count() <= 51); // 50 + 省略号
        assert!(cleaned.ends_with('…'));
    }

    #[test]
    fn test_build_augment_prompt_contains_memories() {
        let mut req = AugmentRequest::new("今天吃什么", "试试意大利面？");
        req.new_memories = vec![MemoryEntry {
            id: "s1".to_string(),
            content: "用户对小麦过敏".to_string(),
            importance: 0.9,
            timestamp: 0.0,
        }];
        let prompt = AugmentReplyService::build_augment_prompt(&req);
        assert!(prompt.contains("用户对小麦过敏"));
        assert!(prompt.contains("试试意大利面？"));
        assert!(prompt.contains("补充回复："));
    }

    #[test]
    fn test_is_in_user_facing_context_default_true() {
        // 未初始化 InterruptionController 时默认返回 true（不阻塞）
        let service = AugmentReplyService::new();
        assert!(service.is_in_user_facing_context());
    }

    #[test]
    fn test_schedule_returns_none_without_memory() {
        // 无 memory 依赖时 schedule 应返回 None
        let service = AugmentReplyService::new();
        let result = service.schedule(
            "你好",
            "你好呀~",
            "idle",
            "star_eyes",
            &[MemoryEntry {
                id: "f1".to_string(),
                content: "test".to_string(),
                importance: 0.5,
                timestamp: 0.0,
            }],
            None,
            "vivian",
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_schedule_returns_none_for_empty_input() {
        let service = AugmentReplyService::new();
        // 即使没有 memory，空输入也应被拒绝（先于 memory 检查也行，此处 memory 检查在前会先返回 None）
        let result = service.schedule("", "你好呀~", "idle", "star_eyes", &[], None, "vivian");
        assert!(result.is_none());
    }

    #[test]
    fn test_set_enabled_disables_schedule() {
        let service = AugmentReplyService::new();
        service.set_enabled(false);
        assert!(service
            .schedule("你好", "你好呀~", "idle", "star_eyes", &[], None, "vivian")
            .is_none());
    }

    #[test]
    fn test_close_sets_closed_state() {
        let service = AugmentReplyService::new();
        service.close();
        assert!(service
            .schedule("你好", "你好呀~", "idle", "star_eyes", &[], None, "vivian")
            .is_none());
    }
}
