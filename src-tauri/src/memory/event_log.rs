//! 记忆事件溯源
//!
//! append-only ndjson 事件日志，保证记忆操作的 crash-safe 与可回溯。
//!
//! 核心设计：
//! - 每角色一个 `events.ndjson`，每行一个 JSON 事件
//! - 15 种事件类型覆盖记忆生命周期
//! - 写入契约：**append BEFORE mutate**（事件先落盘，再修改视图）
//! - Reconciler：启动时尾部重放，handler 必须幂等
//! - 前向兼容：未知事件类型暂停（非崩溃），保留 sentinel
//! - 使用 tokio 异步文件 IO，单角色实例（桌面应用，无多角色并发）

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::error::{VivianError, VivianResult};

use super::types::current_timestamp;

// ============================================================================
// 事件类型
// ============================================================================

/// 15 种事件类型
///
/// 值不可变（wire-format schema id）。新增类型只能追加，不能修改已有值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    // ── 12 种基础事件 ──
    /// 新事实被抽取
    FactAdded,
    /// 事实被吸收进反思
    FactAbsorbed,
    /// 事实被归档
    FactArchived,
    /// 反思被合成
    ReflectionSynthesized,
    /// 反思状态变更（confirmed/denied/promoted）
    ReflectionStateChanged,
    /// 反思被浮现到检索
    ReflectionSurfaced,
    /// 反思被用户反驳
    ReflectionRebutted,
    /// 人格事实被添加
    PersonaFactAdded,
    /// 人格事实被提及
    PersonaFactMentioned,
    /// 人格事实被抑制
    PersonaSuppressed,
    /// 纠正被排队
    CorrectionQueued,
    /// 纠正被解决
    CorrectionResolved,
    // ── 3 种证据系统事件 ──
    /// 反思证据变更
    ReflectionEvidenceUpdated,
    /// 人格证据变更
    PersonaEvidenceUpdated,
    /// 人格条目文本重写（merge-on-promote）
    PersonaEntryUpdated,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::FactAdded => "fact.added",
            EventType::FactAbsorbed => "fact.absorbed",
            EventType::FactArchived => "fact.archived",
            EventType::ReflectionSynthesized => "reflection.synthesized",
            EventType::ReflectionStateChanged => "reflection.state_changed",
            EventType::ReflectionSurfaced => "reflection.surfaced",
            EventType::ReflectionRebutted => "reflection.rebutted",
            EventType::PersonaFactAdded => "persona.fact_added",
            EventType::PersonaFactMentioned => "persona.fact_mentioned",
            EventType::PersonaSuppressed => "persona.suppressed",
            EventType::CorrectionQueued => "correction.queued",
            EventType::CorrectionResolved => "correction.resolved",
            EventType::ReflectionEvidenceUpdated => "reflection.evidence_updated",
            EventType::PersonaEvidenceUpdated => "persona.evidence_updated",
            EventType::PersonaEntryUpdated => "persona.entry_updated",
        }
    }

    /// 从字符串解析事件类型（前向兼容：未知类型返回 None）
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fact.added" => Some(EventType::FactAdded),
            "fact.absorbed" => Some(EventType::FactAbsorbed),
            "fact.archived" => Some(EventType::FactArchived),
            "reflection.synthesized" => Some(EventType::ReflectionSynthesized),
            "reflection.state_changed" => Some(EventType::ReflectionStateChanged),
            "reflection.surfaced" => Some(EventType::ReflectionSurfaced),
            "reflection.rebutted" => Some(EventType::ReflectionRebutted),
            "persona.fact_added" => Some(EventType::PersonaFactAdded),
            "persona.fact_mentioned" => Some(EventType::PersonaFactMentioned),
            "persona.suppressed" => Some(EventType::PersonaSuppressed),
            "correction.queued" => Some(EventType::CorrectionQueued),
            "correction.resolved" => Some(EventType::CorrectionResolved),
            "reflection.evidence_updated" => Some(EventType::ReflectionEvidenceUpdated),
            "persona.evidence_updated" => Some(EventType::PersonaEvidenceUpdated),
            "persona.entry_updated" => Some(EventType::PersonaEntryUpdated),
            _ => None,
        }
    }
}

// ============================================================================
// 事件记录
// ============================================================================

/// 单条事件记录（ndjson 的一行）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    /// 事件唯一 ID（uuid4）
    pub event_id: String,
    /// 事件类型字符串（序列化用 as_str）
    #[serde(rename = "type")]
    pub event_type: String,
    /// ISO8601 时间戳（Unix 秒，浮点）
    pub ts: f64,
    /// 事件负载（全快照，保证 handler 幂等重放）
    pub payload: Value,
}

impl EventRecord {
    /// 创建新事件记录
    pub fn new(event_type: EventType, payload: Value) -> Self {
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.as_str().to_string(),
            ts: current_timestamp(),
            payload,
        }
    }

    /// 解析事件类型（前向兼容）
    pub fn parsed_type(&self) -> Option<EventType> {
        EventType::from_str(&self.event_type)
    }
}

// ============================================================================
// Sentinel（已应用事件游标）
// ============================================================================

/// 已应用事件的游标。
///
/// 持久化到 `<character>.sentinel.json`。记录最后成功应用的事件 ID。
/// Reconciler 启动时读取此文件，只重放其后的事件。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Sentinel {
    last_applied_event_id: Option<String>,
    ts: f64,
}

// ============================================================================
// EventLog
// ============================================================================

/// 压缩阈值：超过此行数触发 compaction
const COMPACT_LINES_THRESHOLD: usize = 10_000;
/// 压缩天数阈值：超过此天数触发 compaction
const COMPACT_DAYS_THRESHOLD: f64 = 90.0;

/// append-only 事件日志。
///
/// 每角色一个实例。文件路径：`<memory_dir>/events.ndjson`。
/// Sentinel 路径：`<memory_dir>/events.sentinel.json`。
///
/// **写入契约**：调用方必须遵循 `append → mutate → save → advance_sentinel` 顺序。
/// 先 append 事件（落盘），再修改内存视图，再保存视图，最后推进 sentinel。
/// 若 append 失败，视图未动，无状态泄露；若 mutate/save 失败，事件已在 log，
/// reconciler 会补齐。
pub struct EventLog {
    /// 事件日志文件路径
    log_path: PathBuf,
    /// sentinel 文件路径
    sentinel_path: PathBuf,
    /// sentinel 内存缓存（避免每次 IO）
    sentinel: Arc<Mutex<Sentinel>>,
}

impl EventLog {
    /// 创建事件日志实例。
    ///
    /// `memory_dir` 是角色记忆目录。若目录不存在会在首次写入时创建。
    pub fn new(memory_dir: impl AsRef<Path>) -> Self {
        let dir = memory_dir.as_ref();
        Self {
            log_path: dir.join("events.ndjson"),
            sentinel_path: dir.join("events.sentinel.json"),
            sentinel: Arc::new(Mutex::new(Sentinel::default())),
        }
    }

    /// 追加一条事件到日志（append-only）。
    ///
    /// 使用 `OpenOptions::append` 保证原子追加。写入后 flush + sync_all
    /// 保证落盘（fsync 失败传播，因为持久性契约要求事件落盘后视图才能推进）。
    pub async fn append(&self, record: &EventRecord) -> VivianResult<()> {
        if let Some(parent) = self.log_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let line = serde_json::to_string(record)
            .map_err(|e| VivianError::Serialization(e.to_string()))?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        // sync_all 失败必须传播：持久性契约要求事件落盘后视图才能推进
        file.sync_all().await?;
        Ok(())
    }

    /// 读取 sentinel 之后的所有事件。
    ///
    /// - sentinel 为 None → 返回全部事件
    /// - sentinel 在日志中找到 → 返回其后的事件
    /// - sentinel 不在日志中（compaction 清除）→ 回退到全量重放
    pub async fn read_since(&self) -> VivianResult<Vec<EventRecord>> {
        let sentinel_id = {
            let s = self.sentinel.lock();
            s.last_applied_event_id.clone()
        };

        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.log_path).await?;
        let mut records: Vec<EventRecord> = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<EventRecord>(line) {
                Ok(r) => records.push(r),
                Err(_) => {
                    // 损坏行跳过（前向兼容，单次扫描仅 warn 一次）
                    tracing::warn!(target: "event_log", "事件日志损坏行已跳过");
                    continue;
                }
            }
        }

        match &sentinel_id {
            None => Ok(records),
            Some(id) => {
                if let Some(pos) = records.iter().position(|r| r.event_id == *id) {
                    // sentinel 在日志中，返回其后的事件
                    Ok(records.into_iter().skip(pos + 1).collect())
                } else {
                    // sentinel 不在日志中（compaction 清除）→ 全量重放
                    tracing::info!(
                        target: "event_log",
                        "sentinel 未在日志中找到，回退到全量重放"
                    );
                    Ok(records)
                }
            }
        }
    }

    /// 推进 sentinel 到指定事件 ID。
    pub async fn advance_sentinel(&self, event_id: &str) -> VivianResult<()> {
        let new_sentinel = Sentinel {
            last_applied_event_id: Some(event_id.to_string()),
            ts: current_timestamp(),
        };
        let json = serde_json::to_string_pretty(&new_sentinel)
            .map_err(|e| VivianError::Serialization(e.to_string()))?;

        // 原子写入：先写临时文件，再 rename
        let tmp_path = self.sentinel_path.with_extension("json.tmp");
        fs::write(&tmp_path, json.as_bytes()).await?;
        fs::rename(&tmp_path, &self.sentinel_path).await?;

        // 更新内存缓存
        {
            let mut s = self.sentinel.lock();
            *s = new_sentinel;
        }
        Ok(())
    }

    /// 加载 sentinel（启动时调用）。
    pub async fn load_sentinel(&self) -> VivianResult<()> {
        if !self.sentinel_path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&self.sentinel_path).await?;
        match serde_json::from_str::<Sentinel>(&content) {
            Ok(s) => {
                let mut guard = self.sentinel.lock();
                *guard = s;
                Ok(())
            }
            Err(e) => {
                // sentinel 损坏：重置为 None，触发全量重放（安全，因为 handler 幂等）
                tracing::warn!(
                    target: "event_log",
                    error = %e,
                    "sentinel 文件损坏，将触发全量重放"
                );
                Ok(())
            }
        }
    }

    /// 记录事件并推进 sentinel（便捷方法）。
    ///
    /// **注意**：此方法只负责事件落盘 + 推进 sentinel。
    /// 调用方仍需在 append 与 advance 之间执行视图变更（mutate + save）。
    ///
    /// 完整契约：
    /// ```text
    /// event_log.append(&record).await?;      // 1. 事件落盘
    /// mutate_view(&mut memory).await?;        // 2. 修改视图
    /// manager.save(&memory).await?;           // 3. 保存视图
    /// event_log.advance_sentinel(&record.event_id).await?;  // 4. 推进游标
    /// ```
    pub async fn record_and_save(
        &self,
        event_type: EventType,
        payload: Value,
    ) -> VivianResult<EventRecord> {
        let record = EventRecord::new(event_type, payload);
        self.append(&record).await?;
        Ok(record)
    }

    /// 检查是否需要压缩，若需要则执行。
    ///
    /// 压缩策略：保留最近 COMPACT_DAYS_THRESHOLD 天的事件 + 所有未推进的事件。
    /// 原子替换：先写临时文件，再 rename。
    pub async fn compact_if_needed(&self) -> VivianResult<usize> {
        if !self.log_path.exists() {
            return Ok(0);
        }

        let content = fs::read_to_string(&self.log_path).await?;
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

        if lines.len() < COMPACT_LINES_THRESHOLD {
            return Ok(0);
        }

        let now = current_timestamp();
        let cutoff = now - COMPACT_DAYS_THRESHOLD * 86400.0;

        let mut kept: Vec<String> = Vec::new();
        for line in &lines {
            match serde_json::from_str::<EventRecord>(line) {
                Ok(r) => {
                    if r.ts >= cutoff {
                        if let Ok(s) = serde_json::to_string(&r) {
                            kept.push(s);
                        }
                    }
                }
                Err(_) => continue,
            }
        }

        // 原子替换
        let tmp_path = self.log_path.with_extension("ndjson.tmp");
        let new_content = kept.join("\n") + "\n";
        fs::write(&tmp_path, new_content.as_bytes()).await?;
        fs::rename(&tmp_path, &self.log_path).await?;

        // 重置 sentinel（compaction 后旧 sentinel 可能不在新日志中）
        {
            let mut s = self.sentinel.lock();
            *s = Sentinel::default();
        }
        let sentinel_json = serde_json::to_string_pretty(&Sentinel::default())
            .map_err(|e| VivianError::Serialization(e.to_string()))?;
        fs::write(&self.sentinel_path, sentinel_json.as_bytes()).await?;

        tracing::info!(
            target: "event_log",
            original = lines.len(),
            kept = kept.len(),
            "事件日志已压缩"
        );
        Ok(kept.len())
    }

    /// 获取日志文件路径（用于诊断/调试）
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }
}

// ============================================================================
// Reconciler
// ============================================================================

/// Reconciler handler trait。
///
/// 所有 handler 必须：
/// - **同步语义**（在 async 上下文中通过 `tokio::task::spawn_blocking` 调用）
/// - **幂等**：N 次重放 = 1 次应用
/// - **返回是否变更**：true 表示视图被修改，false 表示 no-op
/// - **失败时 raise**：暂停整个 reconcile 循环，保留 sentinel
#[async_trait::async_trait]
pub trait ReconcilerHandler: Send + Sync {
    async fn handle(&self, payload: &Value) -> VivianResult<bool>;
}

/// Reconciler：启动时尾部重放事件。
///
/// 失败语义（关键设计决策）：
/// - handler 抛异常 → STOP 整个循环，sentinel 保留在上一条成功事件
/// - 未知事件类型也暂停（非跳过）：推进过未知事件会永久丢失它
///
/// 理由：复合转换有因果依赖，跳过失败的上游事件继续应用下游会产出不一致视图。
pub struct Reconciler {
    log: Arc<EventLog>,
    handlers: Vec<(EventType, Arc<dyn ReconcilerHandler>)>,
}

impl Reconciler {
    pub fn new(log: Arc<EventLog>) -> Self {
        Self {
            log,
            handlers: Vec::new(),
        }
    }

    /// 注册 handler。
    pub fn register(&mut self, event_type: EventType, handler: Arc<dyn ReconcilerHandler>) {
        self.handlers.push((event_type, handler));
    }

    /// 执行重放。
    ///
    /// 返回成功应用的事件数。
    pub async fn reconcile(&self) -> VivianResult<usize> {
        let events = self.log.read_since().await?;
        if events.is_empty() {
            return Ok(0);
        }

        let mut applied = 0usize;
        for event in events {
            let event_type = match event.parsed_type() {
                Some(t) => t,
                None => {
                    // 未知事件类型：暂停（非跳过）
                    tracing::warn!(
                        target: "event_log",
                        event_type = %event.event_type,
                        "未知事件类型，暂停 reconcile"
                    );
                    return Ok(applied);
                }
            };

            // 查找 handler
            let handler = self.handlers.iter().find(|(t, _)| *t == event_type).map(|(_, h)| h);
            let handler = match handler {
                Some(h) => h,
                None => {
                    // 未注册 handler：暂停（前向兼容）
                    tracing::warn!(
                        target: "event_log",
                        event_type = %event.event_type,
                        "事件类型未注册 handler，暂停 reconcile"
                    );
                    return Ok(applied);
                }
            };

            // 执行 handler
            match handler.handle(&event.payload).await {
                Ok(changed) => {
                    if changed {
                        applied += 1;
                    }
                    // 推进 sentinel
                    self.log.advance_sentinel(&event.event_id).await?;
                }
                Err(e) => {
                    // handler 失败：暂停循环，保留 sentinel
                    tracing::warn!(
                        target: "event_log",
                        error = %e,
                        event_type = %event.event_type,
                        "handler 失败，暂停 reconcile"
                    );
                    return Ok(applied);
                }
            }
        }

        tracing::info!(
            target: "event_log",
            applied = applied,
            "事件重放完成"
        );
        Ok(applied)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn make_log() -> (TempDir, EventLog) {
        let dir = TempDir::new().unwrap();
        let log = EventLog::new(dir.path());
        log.load_sentinel().await.unwrap();
        (dir, log)
    }

    #[tokio::test]
    async fn test_append_and_read() {
        let (_dir, log) = make_log().await;
        let record = EventRecord::new(
            EventType::FactAdded,
            serde_json::json!({"fact_id": "f1", "content": "test"}),
        );
        log.append(&record).await.unwrap();

        let events = log.read_since().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, record.event_id);
    }

    #[tokio::test]
    async fn test_sentinel_skips_applied() {
        let (_dir, log) = make_log().await;
        let r1 = log.record_and_save(EventType::FactAdded, json!({"id": 1})).await.unwrap();
        log.advance_sentinel(&r1.event_id).await.unwrap();
        let r2 = log.record_and_save(EventType::FactAdded, json!({"id": 2})).await.unwrap();

        let events = log.read_since().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, r2.event_id);
    }

    #[tokio::test]
    async fn test_unknown_event_type_preserved() {
        let (_dir, log) = make_log().await;
        // 手动写入一个未知事件类型
        let record = EventRecord {
            event_id: "test-unknown".to_string(),
            event_type: "future.unknown_event".to_string(),
            ts: current_timestamp(),
            payload: json!({}),
        };
        log.append(&record).await.unwrap();

        let events = log.read_since().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].parsed_type(), None);
    }

    #[tokio::test]
    async fn test_compact_removes_old() {
        let (_dir, log) = make_log().await;
        // 写入大量旧事件
        let old_ts = current_timestamp() - 100.0 * 86400.0; // 100 天前
        for i in 0..(COMPACT_LINES_THRESHOLD + 10) {
            let record = EventRecord {
                event_id: format!("old-{}", i),
                event_type: EventType::FactAdded.as_str().to_string(),
                ts: old_ts,
                payload: json!({"i": i}),
            };
            log.append(&record).await.unwrap();
        }

        let removed = log.compact_if_needed().await.unwrap();
        assert!(removed < COMPACT_LINES_THRESHOLD);
    }

    #[tokio::test]
    async fn test_event_type_roundtrip() {
        for t in [
            EventType::FactAdded,
            EventType::ReflectionEvidenceUpdated,
            EventType::PersonaEntryUpdated,
        ] {
            assert_eq!(EventType::from_str(t.as_str()), Some(t));
        }
        assert_eq!(EventType::from_str("nonexistent"), None);
    }
}
