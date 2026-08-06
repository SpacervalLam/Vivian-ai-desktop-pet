//! 统一事件账本（Unified Event Ledger）—— 全局环境事件索引层。
//!
//! 在保留各角色 MemoryManager 隔离存储的前提下，新增一个全局共享的事件账本，
//! 作为"环境感知层"。所有对话/动作/交互都抽象为一条"环境事件"，
//! 智能体可按可见性权限查询与自己相关的事件，获得清晰的多角色上下文。
//!
//! 设计要点：
//! - **只存元数据 + 内容预览**：完整记忆仍在各角色的 MemoryManager 中，
//!   事件流只存事件指针 + 前 80 字预览 + 关系元数据，避免破坏角色记忆隔离。
//! - **按可见性分级**：
//!   - `Public`：跨角色对话、广播——所有角色可见
//!   - `Participants`：用户↔智能体对话——只参与方可见
//!   - `Private(char_id)`：旁观记忆——仅指定角色可见
//! - **按角色分桶存储**：Public 事件入共享桶，Participants/Private 事件按关联角色入独立桶，
//!   各桶独立计数与压缩，避免单角色事件挤占全局配额。
//! - **实体-实体检索**：支持按 sender/receiver 过滤，定位两实体间的事件流。
//!
//! 集成点：
//! - `DialogueManager::add_message_with_metadata` 写入对话消息后直接注册事件
//! - `cross_character.rs` 跨角色对话时显式注册 Public 事件
//! - `commands/chat.rs` 旁观记忆注册为 Private 事件
//! - `PromptBuildingStep` 读取近期事件注入 prompt 的"环境事件"段落

use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use tauri::{AppHandle, Emitter};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

/// 事件内容预览的最大字符数
const CONTENT_PREVIEW_MAX_CHARS: usize = 80;
/// 压缩摘要事件的内容上限（比普通预览更大，保留摘要完整性）
const COMPACTED_SUMMARY_MAX_CHARS: usize = 800;
/// 共享桶（Public 事件）保留上限
const MAX_PUBLIC_EVENTS: usize = 1000;
/// 每角色独立桶保留上限
const MAX_CHARACTER_EVENTS: usize = 2000;
/// 每次压缩时取最旧的事件批次大小
const COMPACT_BATCH: usize = 80;

/// 统一环境事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedEvent {
    /// 唯一事件 ID
    pub id: String,
    /// 创建时间戳（秒）
    pub timestamp: f64,
    /// 发起方：`"user"` / `"vivian"` / `"nana"` / `"system"`
    pub sender: String,
    /// 接收方：`"user"` / `"vivian"` / `"nana"` / `"all"`（广播）
    pub receiver: String,
    /// 事件类型：`"dialogue"` / `"action"` / `"observer_note"` / `"system"`
    pub event_type: String,
    /// 内容预览（前 80 字）
    pub content_preview: String,
    /// 上下文标签（如 `["cross_character", "teasing"]`）
    #[serde(default)]
    pub context_tags: Vec<String>,
    /// 可见性
    pub visibility: EventVisibility,
    /// 关联角色 ID（用于系统事件标记所属角色，如 mood_shift / idle_timeout 等）
    #[serde(default)]
    pub associated_char_id: Option<String>,
}

/// 事件可见性
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventVisibility {
    /// 公开——所有角色可见（跨角色对话、广播）
    Public,
    /// 仅参与方可见——用户↔智能体对话
    Participants,
    /// 仅指定角色可见——旁观记忆
    Private(String),
}

impl EventVisibility {
    /// 判断事件对指定角色是否可见
    pub fn visible_to(&self, char_id: &str) -> bool {
        match self {
            EventVisibility::Public => true,
            EventVisibility::Participants => true,
            EventVisibility::Private(owner) => owner == char_id,
        }
    }
}

/// 事件账本内部状态
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LedgerInner {
    /// 旧版格式的兼容字段：单 Vec 事件列表
    /// 反序列化旧文件时填充，迁移完成后清空
    #[serde(default, skip_serializing)]
    events: Vec<UnifiedEvent>,
    /// 共享桶：Public 事件（跨角色对话、世界事件）
    #[serde(default)]
    public_events: Vec<UnifiedEvent>,
    /// 角色独立桶：key = char_id，value = 该角色相关的 Participants/Private 事件
    #[serde(default)]
    character_events: std::collections::HashMap<String, Vec<UnifiedEvent>>,
}

impl LedgerInner {
    /// 判定事件应归属的桶 key
    /// Public 事件返回 None（共享桶），其他返回关联角色 ID
    fn bucket_key(event: &UnifiedEvent) -> Option<String> {
        if event.visibility == EventVisibility::Public {
            return None;
        }
        // 优先用 associated_char_id，其次用 sender/receiver 中第一个非 "user" 的角色
        if let Some(c) = &event.associated_char_id {
            return Some(c.clone());
        }
        if event.sender != "user" && event.sender != "system" {
            return Some(event.sender.clone());
        }
        if event.receiver != "user" && event.receiver != "all" {
            return Some(event.receiver.clone());
        }
        // 兜底：无法归属的事件入共享桶
        None
    }

    /// 从旧版单 Vec 迁移到分桶结构
    fn migrate_from_legacy(&mut self) {
        if self.events.is_empty() {
            return;
        }
        tracing::info!(
            "[UnifiedEventLedger] 迁移旧格式事件 {} 条到分桶结构",
            self.events.len()
        );
        let legacy = std::mem::take(&mut self.events);
        for event in legacy {
            match Self::bucket_key(&event) {
                None => self.public_events.push(event),
                Some(char_id) => {
                    self.character_events
                        .entry(char_id)
                        .or_default()
                        .push(event);
                }
            }
        }
        self.public_events.sort_by(|a, b| {
            a.timestamp
                .partial_cmp(&b.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for bucket in self.character_events.values_mut() {
            bucket.sort_by(|a, b| {
                a.timestamp
                    .partial_cmp(&b.timestamp)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
}

/// 统一事件账本引擎
pub struct UnifiedEventLedger {
    inner: RwLock<LedgerInner>,
    persistence_path: PathBuf,
    /// Tauri AppHandle（注入后启用 `ledger:event-added` 事件通知）
    app_handle: Mutex<Option<AppHandle>>,
    /// 是否正在进行 LLM 摘要压缩（防止并发压缩）
    compacting: std::sync::atomic::AtomicBool,
    /// ModelRouter 引用（用于 LLM 摘要压缩）
    router: Mutex<Option<Arc<crate::providers::router::ModelRouter>>>,
}

static UNIFIED_EVENT_LEDGER: Lazy<Arc<UnifiedEventLedger>> = Lazy::new(|| {
    Arc::new(UnifiedEventLedger::new().unwrap_or_else(|e| {
        tracing::error!("[UnifiedEventLedger] 引擎初始化失败，使用空状态: {e}");
        UnifiedEventLedger {
            inner: RwLock::new(LedgerInner::default()),
            persistence_path: PathBuf::from("unified_event_ledger.json"),
            app_handle: Mutex::new(None),
            compacting: std::sync::atomic::AtomicBool::new(false),
            router: Mutex::new(None),
        }
    }))
});

impl UnifiedEventLedger {
    fn new() -> VivianResult<Self> {
        let dir = get_user_data_dir().join("memory");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("unified_event_ledger.json");
        let mut engine = Self {
            inner: RwLock::new(LedgerInner::default()),
            persistence_path: path,
            app_handle: Mutex::new(None),
            compacting: std::sync::atomic::AtomicBool::new(false),
            router: Mutex::new(None),
        };
        engine.load()?;
        Ok(engine)
    }

    fn load(&mut self) -> VivianResult<()> {
        if !self.persistence_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.persistence_path)?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let inner: LedgerInner = serde_json::from_str(&content).map_err(|e| {
            VivianError::Other(format!("unified_event_ledger.json 解析失败: {e}"))
        })?;
        let mut inner = inner;
        // 旧格式迁移：单 Vec → 分桶
        if !inner.events.is_empty() {
            inner.migrate_from_legacy();
        }
        *self.inner.write() = inner;
        Ok(())
    }

    fn save_inner(inner: &LedgerInner, path: &std::path::Path) -> VivianResult<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(inner)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 追加一条事件
    ///
    /// 事件按可见性与关联角色路由到对应桶。各桶独立计数，超过上限时触发 LLM 压缩。
    pub fn append(self: &Arc<Self>, event: UnifiedEvent) -> VivianResult<()> {
        {
            let mut inner = self.inner.write();
            match LedgerInner::bucket_key(&event) {
                None => {
                    let bucket = &mut inner.public_events;
                    bucket.push(event);
                    // 临时 FIFO 保护（compaction 完成前的安全网）
                    if bucket.len() > MAX_PUBLIC_EVENTS + COMPACT_BATCH {
                        let drop_n = bucket.len() - MAX_PUBLIC_EVENTS;
                        bucket.drain(0..drop_n);
                    }
                }
                Some(char_id) => {
                    let bucket = inner.character_events.entry(char_id).or_default();
                    bucket.push(event);
                    if bucket.len() > MAX_CHARACTER_EVENTS + COMPACT_BATCH {
                        let drop_n = bucket.len() - MAX_CHARACTER_EVENTS;
                        bucket.drain(0..drop_n);
                    }
                }
            }
            Self::save_inner(&inner, &self.persistence_path)?;
        }
        self.emit_event_added();
        self.maybe_compact();
        Ok(())
    }

    /// 注入 AppHandle，启用事件追加后的 `ledger:event-added` 前端通知
    pub fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.lock() = Some(handle);
    }

    /// 注入 ModelRouter（用于 LLM 摘要压缩）
    pub fn set_router(&self, router: Arc<crate::providers::router::ModelRouter>) {
        *self.router.lock() = Some(router);
    }

    /// 通知前端有新事件写入
    fn emit_event_added(&self) {
        if let Some(handle) = self.app_handle.lock().as_ref() {
            let _ = handle.emit("ledger:event-added", serde_json::json!({}));
        }
    }

    /// 检查是否需要压缩，按桶分别触发 LLM 压缩任务
    fn maybe_compact(self: &Arc<Self>) {
        // 收集所有需要压缩的桶
        let mut pending: Vec<PendingCompaction> = Vec::new();
        {
            let inner = self.inner.read();
            if inner.public_events.len() > MAX_PUBLIC_EVENTS {
                pending.push(PendingCompaction {
                    bucket: BucketKind::Public,
                    count: inner.public_events.len(),
                });
            }
            for (char_id, bucket) in &inner.character_events {
                if bucket.len() > MAX_CHARACTER_EVENTS {
                    pending.push(PendingCompaction {
                        bucket: BucketKind::Character(char_id.clone()),
                        count: bucket.len(),
                    });
                }
            }
        }
        if pending.is_empty() {
            return;
        }
        // 竞态防护：已在压缩则跳过（下次 append 会再次检查）
        if self
            .compacting
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        // 取最需要压缩的桶（超出配额最多）
        pending.sort_by(|a, b| b.count.cmp(&a.count));
        let target = pending.into_iter().next().unwrap();
        let ledger = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            ledger.run_compaction(target.bucket).await;
            ledger
                .compacting
                .store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// 执行 LLM 摘要压缩：将某桶最旧的一批事件压缩为单条摘要
    ///
    /// LLM 是唯一压缩路径。失败时保留原事件，下次触发时重试，不降级到本地规则。
    async fn run_compaction(&self, bucket: BucketKind) {
        let router = self.router.lock().clone();
        let router = match router {
            Some(r) => r,
            None => {
                tracing::warn!("[UnifiedEventLedger] 无 ModelRouter，跳过压缩等待下次重试");
                return;
            }
        };

        // 取出最旧的 COMPACT_BATCH 条事件
        let batch: Vec<UnifiedEvent> = {
            let mut inner = self.inner.write();
            let target = match bucket.target_bucket(&mut inner) {
                Some(b) => b,
                None => return,
            };
            if target.len() <= COMPACT_BATCH {
                return;
            }
            target.drain(0..COMPACT_BATCH).collect()
        };

        let ts_end = batch.last().map(|e| e.timestamp).unwrap_or(0.0);
        let bucket_label = bucket.label();

        let prompt = build_compaction_prompt(&batch);
        let summary_text = match router
            .generate(crate::providers::base::LLMRequest::new(
                "reflection",
                vec![crate::types::response::ChatMessage::user(prompt)],
            ))
            .await
        {
            Ok(text) if !text.trim().is_empty() => {
                tracing::info!(
                    "[UnifiedEventLedger] LLM 压缩 {} 桶 {} 条事件为摘要",
                    bucket_label,
                    batch.len()
                );
                text.trim().to_string()
            }
            Ok(_) => {
                // LLM 返回空：回填事件，下次重试
                tracing::warn!(
                    "[UnifiedEventLedger] LLM 返回空摘要，回填 {} 桶 {} 条事件待重试",
                    bucket_label,
                    batch.len()
                );
                self.restore_batch(batch, &bucket).await;
                return;
            }
            Err(e) => {
                tracing::warn!(
                    "[UnifiedEventLedger] LLM 压缩失败: {}，回填 {} 桶事件待重试",
                    e,
                    bucket_label
                );
                self.restore_batch(batch, &bucket).await;
                return;
            }
        };

        let summary_event = UnifiedEvent {
            id: format!("evt-compact-{}-{}", bucket_label, ts_end as u64),
            timestamp: ts_end,
            sender: "system".to_string(),
            receiver: "all".to_string(),
            event_type: "compacted_summary".to_string(),
            content_preview: summary_text
                .chars()
                .take(COMPACTED_SUMMARY_MAX_CHARS)
                .collect(),
            context_tags: vec![
                "compacted".to_string(),
                format!("bucket:{}", bucket_label),
                format!("{}events", batch.len()),
            ],
            visibility: EventVisibility::Public,
            associated_char_id: match &bucket {
                BucketKind::Character(cid) => Some(cid.clone()),
                _ => None,
            },
        };
        let mut inner = self.inner.write();
        match bucket.target_bucket(&mut inner) {
            Some(target) => {
                target.insert(0, summary_event);
                let cap = bucket.capacity();
                if target.len() > cap {
                    let drop_n = target.len() - cap;
                    target.drain(0..drop_n);
                }
            }
            None => {
                // 桶被清空了（如 clear_for_character）：丢弃摘要
            }
        }
        let _ = Self::save_inner(&inner, &self.persistence_path);
        drop(inner);
        self.emit_event_added();
    }

    /// 将压缩失败的事件批次回填到原桶，等待下次重试
    async fn restore_batch(&self, batch: Vec<UnifiedEvent>, bucket: &BucketKind) {
        if batch.is_empty() {
            return;
        }
        let mut inner = self.inner.write();
        if let Some(target) = bucket.target_bucket(&mut inner) {
            // 按时间升序合并回原桶
            let mut merged = batch.clone();
            merged.extend(target.drain(..));
            merged.sort_by(|a, b| {
                a.timestamp
                    .partial_cmp(&b.timestamp)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            *target = merged;
        }
        let _ = Self::save_inner(&inner, &self.persistence_path);
    }

    /// 查询某角色可见的最近 N 条事件（按时间倒序）
    ///
    /// 合并共享桶与该角色独立桶，按 (importance, recency) 联合排序。
    pub fn recent_events_visible_to(&self, char_id: &str, n: usize) -> Vec<UnifiedEvent> {
        let inner = self.inner.read();
        let pool_size = (n * 3).max(n);
        let mut candidates: Vec<UnifiedEvent> = Vec::with_capacity(pool_size);

        // 共享桶：Public 事件所有角色可见
        for e in inner.public_events.iter().rev() {
            if candidates.len() >= pool_size {
                break;
            }
            candidates.push(e.clone());
        }
        // 角色独立桶：仅取该角色相关事件
        if let Some(bucket) = inner.character_events.get(char_id) {
            for e in bucket.iter().rev() {
                if candidates.len() >= pool_size {
                    break;
                }
                if self.is_visible(e, char_id) {
                    candidates.push(e.clone());
                }
            }
        }

        let now = chrono::Local::now().timestamp() as f64;
        candidates.sort_by(|a, b| {
            let ia = event_importance(&a.event_type, now - a.timestamp);
            let ib = event_importance(&b.event_type, now - b.timestamp);
            ib.partial_cmp(&ia)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.timestamp
                        .partial_cmp(&a.timestamp)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        let mut result: Vec<UnifiedEvent> = candidates.into_iter().take(n).collect();
        result.reverse();
        result
    }

    pub fn events_on_date(
        &self,
        char_id: &str,
        date: chrono::NaiveDate,
        limit: usize,
    ) -> Vec<UnifiedEvent> {
        use chrono::{Local, TimeZone};
        let start = date
            .and_hms_opt(0, 0, 0)
            .and_then(|dt| Local.from_local_datetime(&dt).single())
            .map(|dt| dt.timestamp() as f64)
            .unwrap_or(0.0);
        let end = start + 86400.0;

        let inner = self.inner.read();
        let mut result: Vec<UnifiedEvent> = Vec::new();
        // 共享桶
        for e in &inner.public_events {
            if e.timestamp >= start && e.timestamp < end && self.is_visible(e, char_id) {
                result.push(e.clone());
            }
        }
        // 角色桶
        if let Some(bucket) = inner.character_events.get(char_id) {
            for e in bucket {
                if e.timestamp >= start && e.timestamp < end && self.is_visible(e, char_id) {
                    result.push(e.clone());
                }
            }
        }
        result.sort_by(|a, b| {
            a.timestamp
                .partial_cmp(&b.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result.truncate(limit);
        result
    }

    /// 判断事件对指定角色是否可见
    fn is_visible(&self, event: &UnifiedEvent, char_id: &str) -> bool {
        match &event.visibility {
            EventVisibility::Public => true,
            EventVisibility::Participants => {
                event.sender == char_id || event.receiver == char_id
            }
            EventVisibility::Private(owner) => owner == char_id,
        }
    }

    /// 查询两个实体之间的最近 N 条事件（双向：A→B 和 B→A）
    pub fn events_between(&self, entity_a: &str, entity_b: &str, n: usize) -> Vec<UnifiedEvent> {
        let inner = self.inner.read();
        let mut matched: Vec<UnifiedEvent> = Vec::new();
        let matches = |e: &UnifiedEvent| {
            (e.sender == entity_a && e.receiver == entity_b)
                || (e.sender == entity_b && e.receiver == entity_a)
        };
        // 共享桶
        for e in inner.public_events.iter().rev() {
            if matched.len() >= n {
                break;
            }
            if matches(e) {
                matched.push(e.clone());
            }
        }
        // 两角色独立桶
        for char_id in &[entity_a, entity_b] {
            if let Some(bucket) = inner.character_events.get(*char_id) {
                for e in bucket.iter().rev() {
                    if matched.len() >= n {
                        break;
                    }
                    if matches(e) {
                        matched.push(e.clone());
                    }
                }
            }
        }
        matched.sort_by(|a, b| {
            b.timestamp
                .partial_cmp(&a.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matched.truncate(n);
        matched.reverse();
        matched
    }

    /// 查询全局公开事件（用于环境感知，所有角色共享）
    pub fn recent_public_events(&self, n: usize) -> Vec<UnifiedEvent> {
        let inner = self.inner.read();
        let mut public: Vec<UnifiedEvent> = inner
            .public_events
            .iter()
            .rev()
            .take(n)
            .cloned()
            .collect();
        public.reverse();
        public
    }

    /// 生成可注入 prompt 的近期环境事件段落
    pub fn build_prompt_section(&self, char_id: &str, n: usize, lang: &str) -> Option<String> {
        let events = self.recent_events_visible_to(char_id, n);
        if events.is_empty() {
            return None;
        }
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let broadcast_label = match lang_norm {
            "en" => "broadcast",
            "ja" => "ブロードキャスト",
            _ => "广播",
        };
        let mut lines: Vec<String> = Vec::with_capacity(events.len() + 1);
        let header = crate::pipeline::prompt_modules::section_heading("recent_environment_events", lang);
        lines.push(header.to_string());
        for e in &events {
            let direction = if e.receiver == "all" {
                broadcast_label.to_string()
            } else {
                format!("{} → {}", e.sender, e.receiver)
            };
            let tags = if e.context_tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", e.context_tags.join(", "))
            };
            lines.push(format!(
                "- [{}] {}{}: {}",
                format_timestamp(e.timestamp),
                direction,
                tags,
                e.content_preview
            ));
        }
        Some(lines.join("\n"))
    }

    /// 清空全部事件
    pub fn clear_all(&self) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.public_events.clear();
        inner.character_events.clear();
        inner.events.clear();
        Self::save_inner(&inner, &self.persistence_path)?;
        Ok(())
    }

    /// 清除指定角色参与的事件
    ///
    /// 保留其他角色独立桶和共享桶中不涉及该角色的事件。
    pub fn clear_for_character(&self, char_id: &str) -> VivianResult<()> {
        let mut inner = self.inner.write();
        // 清空该角色独立桶
        let dropped_bucket = inner.character_events.remove(char_id).map(|b| b.len()).unwrap_or(0);
        // 从共享桶中移除该角色参与的事件
        let before_pub = inner.public_events.len();
        inner.public_events.retain(|e| {
            let is_participant = e.sender == char_id || e.receiver == char_id;
            let is_associated = e.associated_char_id.as_deref() == Some(char_id);
            !is_participant && !is_associated
        });
        let dropped_pub = before_pub - inner.public_events.len();
        // 从其他角色桶中移除该角色参与的事件
        let mut dropped_other = 0;
        for bucket in inner.character_events.values_mut() {
            let before = bucket.len();
            bucket.retain(|e| {
                let is_participant = e.sender == char_id || e.receiver == char_id;
                let is_private_owner =
                    matches!(&e.visibility, EventVisibility::Private(owner) if owner == char_id);
                let is_associated = e.associated_char_id.as_deref() == Some(char_id);
                !is_participant && !is_private_owner && !is_associated
            });
            dropped_other += before - bucket.len();
        }
        let dropped = dropped_bucket + dropped_pub + dropped_other;
        if dropped > 0 {
            Self::save_inner(&inner, &self.persistence_path)?;
            tracing::info!("[UnifiedEventLedger] 已清除 {} 相关事件 {} 条", char_id, dropped);
        }
        Ok(())
    }
}

/// 压缩任务目标桶
#[derive(Clone, Debug)]
enum BucketKind {
    Public,
    Character(String),
}

impl BucketKind {
    fn label(&self) -> &'static str {
        match self {
            BucketKind::Public => "public",
            BucketKind::Character(_) => "character",
        }
    }

    fn target_bucket<'a>(&self, inner: &'a mut LedgerInner) -> Option<&'a mut Vec<UnifiedEvent>> {
        match self {
            BucketKind::Public => Some(&mut inner.public_events),
            BucketKind::Character(cid) => inner.character_events.get_mut(cid),
        }
    }

    fn capacity(&self) -> usize {
        match self {
            BucketKind::Public => MAX_PUBLIC_EVENTS,
            BucketKind::Character(_) => MAX_CHARACTER_EVENTS,
        }
    }
}

struct PendingCompaction {
    bucket: BucketKind,
    count: usize,
}

/// 事件重要性权重（按 event_type 推断）
fn event_base_importance(event_type: &str) -> f64 {
    match event_type {
        "dialogue" => 0.9,
        "action" => 0.7,
        "mood_shift" | "mood_event" => 0.6,
        "observer_note" => 0.5,
        "compacted_summary" => 0.85,
        "presence_log" | "long_idle" | "quiet_mode" => 0.3,
        "system" | _ => 0.4,
    }
}

/// 时间衰减因子（年龄越大，因子越小）
fn time_decay_factor(age_secs: f64) -> f64 {
    let days = age_secs / 86400.0;
    if days < 1.0 {
        0.95
    } else if days < 3.0 {
        0.70
    } else if days < 7.0 {
        0.40
    } else if days < 30.0 {
        0.15
    } else {
        0.05
    }
}

/// 事件重要性（带时间衰减）
fn event_importance(event_type: &str, age_secs: f64) -> f64 {
    event_base_importance(event_type) * time_decay_factor(age_secs)
}

/// 格式化时间戳为简短的可读时间（如 "07-16 14:30"）
fn format_timestamp(ts: f64) -> String {
    let dt: chrono::DateTime<chrono::Local> =
        chrono::DateTime::<chrono::Utc>::from_timestamp(ts as i64, 0)
            .map(|dt| dt.with_timezone(&chrono::Local))
            .unwrap_or_else(|| chrono::Utc::now().with_timezone(&chrono::Local));
    dt.format("%m-%d %H:%M").to_string()
}

/// 构建 LLM 摘要压缩 prompt
///
/// 引导 LLM 提取模式和要点（而非逐条复述），摘要上限提升至 500 字以保留更多细节。
fn build_compaction_prompt(batch: &[UnifiedEvent]) -> String {
    let lines: Vec<String> = batch
        .iter()
        .map(|e| {
            let time = format_timestamp(e.timestamp);
            format!(
                "[{}] {} → {} ({}): {}",
                time, e.sender, e.receiver, e.event_type, e.content_preview
            )
        })
        .collect();

    let ts_start = batch.first().map(|e| format_timestamp(e.timestamp)).unwrap_or_default();
    let ts_end = batch.last().map(|e| format_timestamp(e.timestamp)).unwrap_or_default();

    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    match lang_norm {
        "en" => format!(
            "You are an event summarization assistant. Below are {n} historical event records (time range: {start} ~ {end}).\n\
            Please compress these events into a structured summary (no more than 500 words), preserving:\n\
            - Key conversation topics, conclusions, and any decisions made\n\
            - Important emotional changes, relationship shifts, and bonding moments\n\
            - Notable world events (e.g., long periods offline, mood shifts, presence changes)\n\
            - Specific mentions of activities, preferences, or plans discussed\n\
            - Time range information and event frequency patterns\n\n\
            Group related events thematically. Do not recap item by item; extract patterns and key points.\n\
            Output summary text directly, without titles or prefixes.\n\n\
            --- Event Records ---\n{events}",
            n = batch.len(),
            start = ts_start,
            end = ts_end,
            events = lines.join("\n"),
        ),
        "ja" => format!(
            "あなたはイベント要約アシスタントです。以下は {n} 件の歴史イベントの記録です（期間：{start} ~ {end}）。\n\
            これらのイベントを構造化された要約に圧縮してください（500文字以内）。以下を保持すること：\n\
            - 重要な会話のテーマ、結論、決定事項\n\
            - 重要な感情の変化、関係の変化、絆の瞬間\n\
            - 注目すべき世界イベント（長時間のオフライン、気分の変化、在席状態の変化など）\n\
            - 議論された活動、好み、計画の具体的な言及\n\
            - 期間情報とイベント頻度のパターン\n\n\
            関連するイベントをテーマ別にグループ化すること。逐一再述するのではなく、パターンと要点を抽出すること。\n\
            タイトルや接頭辞をつけずに要約テキストを直接出力すること。\n\n\
            --- イベント記録 ---\n{events}",
            n = batch.len(),
            start = ts_start,
            end = ts_end,
            events = lines.join("\n"),
        ),
        _ => format!(
            "你是一个事件摘要助手。以下是 {n} 条历史事件的记录（时间范围：{start} ~ {end}）。\n\
            请将这些事件压缩为一段结构化摘要（不超过500字），保留：\n\
            - 关键对话主题、结论和任何决定\n\
            - 重要的情绪变化、关系转变和亲密时刻\n\
            - 值得注意的世界事件（如长时间离线、心情转变、在场状态变化）\n\
            - 具体提及的活动、偏好或计划\n\
            - 时间范围信息和事件频率模式\n\n\
            按主题分组相关事件，不要逐条复述，提取模式和要点。\n\
            直接输出摘要文本，不要加标题或前缀。\n\n\
            --- 事件记录 ---\n{events}",
            n = batch.len(),
            start = ts_start,
            end = ts_end,
            events = lines.join("\n"),
        ),
    }
}

/// 获取全局统一事件账本
pub fn unified_event_ledger() -> Arc<UnifiedEventLedger> {
    Arc::clone(&UNIFIED_EVENT_LEDGER)
}

/// 显式注册一条环境事件
pub fn register_event(event: UnifiedEvent) {
    let ledger = unified_event_ledger();
    if let Err(e) = ledger.append(event) {
        tracing::warn!("[UnifiedEventLedger] 事件注册失败: {}", e);
    }
}

/// 注册一条 **World Event**（世界事件）。
///
/// 世界事件是程序确定的事实，不属于任何单一角色：
/// - `sender = "system"`、`receiver = "all"`、`visibility = Public`
/// - 双角色各自感知，产生不同的 Memory / Belief / 情绪反应
pub fn register_world_event(
    event_type: &str,
    content_preview: &str,
    context_tags: Vec<String>,
    timestamp: f64,
    associated_char_id: Option<&str>,
) {
    let ledger = unified_event_ledger();
    let event = UnifiedEvent {
        id: format!("evt-{}-{}", timestamp as u64, rand::random::<u32>()),
        timestamp,
        sender: "system".to_string(),
        receiver: "all".to_string(),
        event_type: event_type.to_string(),
        content_preview: content_preview.chars().take(CONTENT_PREVIEW_MAX_CHARS).collect(),
        context_tags,
        visibility: EventVisibility::Public,
        associated_char_id: associated_char_id.map(|s| s.to_string()),
    };
    if let Err(e) = ledger.append(event) {
        tracing::warn!("[UnifiedEventLedger] 世界事件注册失败: {}", e);
    }
}

/// 从对话消息注册一条环境事件
///
/// 在 `DialogueManager::add_message_with_metadata` 中调用。
pub fn register_event_from_dialogue(
    char_id: &str,
    content: &str,
    metadata: &serde_json::Value,
    timestamp: f64,
) {
    let has_speaker_or_listener = metadata.is_object()
        && (metadata.get("speaker").is_some()
            || metadata.get("listener").is_some()
            || metadata.get("perspective").is_some());
    if !has_speaker_or_listener {
        return;
    }

    let ledger = unified_event_ledger();

    let speaker = metadata
        .get("speaker")
        .and_then(|v| v.as_str())
        .unwrap_or("user")
        .to_string();
    let listener = metadata
        .get("listener")
        .and_then(|v| v.as_str())
        .unwrap_or(char_id)
        .to_string();
    let channel = metadata
        .get("channel")
        .and_then(|v| v.as_str())
        .unwrap_or("direct")
        .to_string();
    let perspective = metadata
        .get("perspective")
        .and_then(|v| v.as_str())
        .unwrap_or("speaker")
        .to_string();
    let knowledge_source = metadata
        .get("knowledge_source")
        .and_then(|v| v.as_str())
        .unwrap_or("direct")
        .to_string();

    let visibility = if perspective == "observer" {
        let observer_id = metadata
            .get("observer_id")
            .and_then(|v| v.as_str())
            .unwrap_or(char_id)
            .to_string();
        EventVisibility::Private(observer_id)
    } else if channel == "cross_character" {
        EventVisibility::Public
    } else {
        EventVisibility::Participants
    };

    let event_type = if perspective == "observer" {
        "observer_note".to_string()
    } else if channel == "cross_character" {
        "cross_character".to_string()
    } else {
        "dialogue".to_string()
    };

    let content_preview: String = content.chars().take(CONTENT_PREVIEW_MAX_CHARS).collect();

    let mut context_tags = vec![channel.clone(), knowledge_source.clone()];
    if perspective == "observer" {
        context_tags.push("observer".to_string());
    }

    let event = UnifiedEvent {
        id: format!("evt-{}-{}", timestamp as u64, rand::random::<u32>()),
        timestamp,
        sender: speaker,
        receiver: listener,
        event_type,
        content_preview,
        context_tags,
        visibility,
        associated_char_id: None,
    };

    if let Err(e) = ledger.append(event) {
        tracing::warn!("[UnifiedEventLedger] 对话事件注册失败: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(
        sender: &str,
        receiver: &str,
        visibility: EventVisibility,
        ts: f64,
    ) -> UnifiedEvent {
        UnifiedEvent {
            id: format!("evt-{ts}"),
            timestamp: ts,
            sender: sender.to_string(),
            receiver: receiver.to_string(),
            event_type: "dialogue".to_string(),
            content_preview: "test".to_string(),
            context_tags: vec![],
            visibility,
            associated_char_id: None,
        }
    }

    #[test]
    fn test_visibility_public_visible_to_all() {
        let e = make_event("vivian", "nana", EventVisibility::Public, 1.0);
        assert!(e.visibility.visible_to("vivian"));
        assert!(e.visibility.visible_to("nana"));
        assert!(e.visibility.visible_to("user"));
    }

    #[test]
    fn test_visibility_private_only_owner() {
        let e = make_event("nana", "nana", EventVisibility::Private("nana".to_string()), 2.0);
        assert!(e.visibility.visible_to("nana"));
        assert!(!e.visibility.visible_to("vivian"));
    }

    #[test]
    fn test_events_between_bidirectional() {
        let mut inner = LedgerInner::default();
        inner.public_events = vec![
            make_event("vivian", "nana", EventVisibility::Public, 1.0),
            make_event("nana", "vivian", EventVisibility::Public, 2.0),
        ];
        inner.character_events.insert(
            "vivian".to_string(),
            vec![make_event("user", "vivian", EventVisibility::Participants, 3.0)],
        );
        let ledger = UnifiedEventLedger {
            inner: RwLock::new(inner),
            persistence_path: PathBuf::from("test.json"),
            app_handle: Mutex::new(None),
            compacting: std::sync::atomic::AtomicBool::new(false),
            router: Mutex::new(None),
        };
        let between = ledger.events_between("vivian", "nana", 10);
        assert_eq!(between.len(), 2);
        let between_user = ledger.events_between("user", "vivian", 10);
        assert_eq!(between_user.len(), 1);
    }

    #[test]
    fn test_bucket_key_routing() {
        let public_event = make_event("vivian", "nana", EventVisibility::Public, 1.0);
        assert_eq!(LedgerInner::bucket_key(&public_event), None);

        let participants_event =
            make_event("user", "vivian", EventVisibility::Participants, 2.0);
        assert_eq!(
            LedgerInner::bucket_key(&participants_event),
            Some("vivian".to_string())
        );

        let private_event = make_event(
            "nana",
            "nana",
            EventVisibility::Private("nana".to_string()),
            3.0,
        );
        assert_eq!(
            LedgerInner::bucket_key(&private_event),
            Some("nana".to_string())
        );
    }

    #[test]
    fn test_legacy_migration() {
        let mut inner = LedgerInner::default();
        inner.events = vec![
            make_event("vivian", "nana", EventVisibility::Public, 1.0),
            make_event("user", "vivian", EventVisibility::Participants, 2.0),
            make_event("user", "nana", EventVisibility::Participants, 3.0),
        ];
        inner.migrate_from_legacy();
        assert!(inner.events.is_empty());
        assert_eq!(inner.public_events.len(), 1);
        assert_eq!(inner.character_events.get("vivian").map(|b| b.len()).unwrap_or(0), 1);
        assert_eq!(inner.character_events.get("nana").map(|b| b.len()).unwrap_or(0), 1);
    }
}
