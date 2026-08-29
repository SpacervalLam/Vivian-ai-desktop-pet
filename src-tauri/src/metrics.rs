//! 性能与失败指标 — 轻量零依赖统计
//!
//! - `Counter`：线程安全单调计数器
//! - `Histogram`：耗时分布（桶计数 + 总数 + 总和）
//! - `Gauge`：可增可减的瞬时值
//! - `MetricsRegistry`：全局注册表，提供 timer 上下文管理器
//! - 降级检测：`record_degradation_attempt` 必须保持 0
//!
//! 持久化（任务要求）：`%APPDATA%\Vivian\logs\metrics.json`，每日轮转。
//! 提供 `get_metrics_summary` 命令。
//!
//! 用法：
//! ```ignore
//! use vivian_lib::metrics::METRICS;
//!
//! METRICS.counter("llm.calls").inc();
//! let _guard = METRICS.start_timer("llm.duration_ms");
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use once_cell::sync::OnceCell;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::utils::path;

// ────────────────────────────────────────────────────────────────
// 指标名常量
// ────────────────────────────────────────────────────────────────

/// LLM 调用次数
pub const LLM_CALLS: &str = "llm.calls";
/// LLM 调用耗时（毫秒）
pub const LLM_DURATION_MS: &str = "llm.duration_ms";
/// LLM 调用失败次数
pub const LLM_FAILURES: &str = "llm.failures";
/// 工具调用次数
pub const TOOL_CALLS: &str = "tool.calls";
/// 工具调用耗时（毫秒）
pub const TOOL_DURATION_MS: &str = "tool.duration_ms";
/// 工具失败次数
pub const TOOL_FAILURES: &str = "tool.failures";

// ── Embedding 嵌入子系统 ──
pub const EMB_EMBED_CALLS: &str = "embedding.embed.calls";
pub const EMB_EMBED_DURATION_MS: &str = "embedding.embed.duration_ms";
pub const EMB_EMBED_FAILURES: &str = "embedding.embed.failures";

// ── 向量存储子系统 ──
pub const VECTOR_ADD_CALLS: &str = "vector.add.calls";
pub const VECTOR_ADD_DURATION_MS: &str = "vector.add.duration_ms";
pub const VECTOR_QUERY_CALLS: &str = "vector.query.calls";
pub const VECTOR_QUERY_DURATION_MS: &str = "vector.query.duration_ms";
pub const VECTOR_FAILURES: &str = "vector.failures";

// ── RAG pipeline 子系统 ──
pub const RAG_QUERY_CALLS: &str = "rag.query.calls";
pub const RAG_QUERY_DURATION_MS: &str = "rag.query.duration_ms";
pub const RAG_RETRIEVE_RESULTS_TOTAL: &str = "rag.retrieve.results";
pub const RAG_RERANK_CALLS: &str = "rag.rerank.calls";
pub const RAG_RERANK_DURATION_MS: &str = "rag.rerank.duration_ms";
pub const RAG_STEP_RETRIES: &str = "rag.step.retries";
pub const RAG_STEP_FAILURES: &str = "rag.step.failures";

// ── Memory backend 子系统 ──
/// 记忆操作次数（add_turn / add_session / add_summary 等）
pub const MEM_ADD_TURN: &str = "memory.add_turn";
pub const MEM_ADD_SESSION: &str = "memory.add_session";
pub const MEM_ADD_SUMMARY: &str = "memory.add_summary";
pub const MEM_ADD_KEYWORD: &str = "memory.add_keyword";
pub const MEM_RETRIEVE: &str = "memory.retrieve";
pub const MEM_RETRIEVE_DURATION_MS: &str = "memory.retrieve.duration_ms";
pub const MEM_FAILURES: &str = "memory.failures";
/// 降级尝试计数（必须保持 0）
pub const MEM_DEGRADATION_ATTEMPTS: &str = "memory.degradation_attempts";

// ── Memory manager 子系统 ──
pub const MGR_ADD_MEMORY: &str = "manager.add_memory";
pub const MGR_RETRIEVE_MEMORY: &str = "manager.retrieve_memory";
pub const MGR_UPDATE_MEMORY: &str = "manager.update_memory";

// ── Auto-extractor 子系统 ──
pub const EXTRACT_BATCH_PROCESSED: &str = "extractor.batches.processed";
pub const EXTRACT_OPERATIONS: &str = "extractor.operations";
pub const EXTRACT_LLM_CALLS: &str = "extractor.llm.calls";
pub const EXTRACT_LLM_DURATION_MS: &str = "extractor.llm.duration_ms";
pub const EXTRACT_FAILURES: &str = "extractor.failures";

// ── Topic 主题切分子系统 ──
pub const TOPIC_SEGMENT_CALLS: &str = "topic_segment.calls";
pub const TOPIC_SEGMENT_BOUNDARIES: &str = "topic_segment.boundaries";
pub const TOPIC_SEGMENT_TOPICS: &str = "topic_segment.topics";
pub const TOPIC_STS_PUTS: &str = "topic_sts.puts";
pub const TOPIC_STS_TRIGGERS: &str = "topic_sts.triggers";
pub const TOPIC_STS_TOPICS: &str = "topic_sts.topics";

// ── Token 压缩子系统 ──
pub const TOKEN_COMPRESS_CALLS: &str = "token_compress.calls";
pub const TOKEN_COMPRESS_ORIGINAL_TOKENS: &str = "token_compress.original_tokens";
pub const TOKEN_COMPRESS_COMPRESSED_TOKENS: &str = "token_compress.compressed_tokens";
pub const TOKEN_COMPRESS_FAILURES: &str = "token_compress.failures";

// ── LightMem pipeline 子系统 ──
pub const LIGHTMEM_INGEST_CALLS: &str = "lightmem.ingest";
pub const LIGHTMEM_FLUSH_CALLS: &str = "lightmem.flush";
pub const LIGHTMEM_LIGHT1_COMPRESSED: &str = "lightmem.light1_compressed";
pub const LIGHTMEM_LIGHT2_TRIGGERED: &str = "lightmem.light2_triggered";
pub const LIGHTMEM_LIGHT3_PROMOTED: &str = "lightmem.light3_promoted";
pub const ONLINE_UPDATER_ENQUEUED: &str = "online_updater.enqueued";
pub const ONLINE_UPDATER_DRAINED: &str = "online_updater.drained";
pub const OFFLINE_BATCH_RUNS: &str = "offline_batch.runs";
pub const OFFLINE_BATCH_DEDUPED: &str = "offline_batch.deduped";
pub const OFFLINE_BATCH_MERGED: &str = "offline_batch.merged";
pub const OFFLINE_BATCH_REPAIRED: &str = "offline_batch.repaired";

// ── Entropy router / LLM filter 子系统 ──
pub const ENTROPY_ROUTER_WEIGHTS: &str = "entropy_router.weights";
pub const ENTROPY_ROUTER_FUSED: &str = "entropy_router.fused";
pub const LLM_FILTER_CALLS: &str = "llm_filter.calls";
pub const LLM_FILTER_KEPT: &str = "llm_filter.kept";
pub const LLM_FILTER_REJECTED: &str = "llm_filter.rejected";

// ── Selector 同步子系统 ──
pub const SELECTOR_SYNC_CALLS: &str = "selector.sync.calls";
pub const SELECTOR_SYNC_SKIPPED: &str = "selector.sync.skipped";

/// 用户消息总数
pub const USER_MESSAGES: &str = "user.messages";
/// 缓存命中 / 未命中
pub const CACHE_HITS: &str = "cache.hits";
pub const CACHE_MISSES: &str = "cache.misses";
/// 限流获取 / 限流触发
pub const RATE_LIMITER_ACQUIRED: &str = "ratelimit.acquired";
pub const RATE_LIMITER_THROTTLED: &str = "ratelimit.throttled";

// ────────────────────────────────────────────────────────────────
// Counter
// ────────────────────────────────────────────────────────────────

/// 线程安全的单调计数器
pub struct Counter {
    name: String,
    description: String,
    value: Mutex<u64>,
}

impl Counter {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            value: Mutex::new(0),
        }
    }

    pub fn inc(&self, n: u64) {
        if n == 0 {
            return;
        }
        let mut v = self.value.lock();
        *v += n;
    }

    pub fn inc_one(&self) {
        self.inc(1);
    }

    pub fn value(&self) -> u64 {
        *self.value.lock()
    }

    pub fn reset(&self) {
        *self.value.lock() = 0;
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

// ────────────────────────────────────────────────────────────────
// Gauge
// ────────────────────────────────────────────────────────────────

/// 可增可减的瞬时值（任务要求：gauge 类型）
pub struct Gauge {
    name: String,
    description: String,
    value: Mutex<f64>,
}

impl Gauge {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            value: Mutex::new(0.0),
        }
    }

    pub fn set(&self, v: f64) {
        *self.value.lock() = v;
    }

    pub fn inc(&self, delta: f64) {
        *self.value.lock() += delta;
    }

    pub fn dec(&self, delta: f64) {
        *self.value.lock() -= delta;
    }

    pub fn value(&self) -> f64 {
        *self.value.lock()
    }

    pub fn reset(&self) {
        *self.value.lock() = 0.0;
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

// ────────────────────────────────────────────────────────────────
// Histogram
// ────────────────────────────────────────────────────────────────

/// 默认桶边界（毫秒）
pub const DEFAULT_BUCKETS_MS: &[f64] = &[1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0];

/// 轻量 Histogram（仅记录桶分布和总数）
pub struct Histogram {
    name: String,
    description: String,
    buckets_ms: Vec<f64>,
    inner: Mutex<HistogramInner>,
}

#[derive(Default)]
struct HistogramInner {
    bucket_counts: Vec<u64>,
    count: u64,
    sum_ms: f64,
}

impl Histogram {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        buckets_ms: &[f64],
    ) -> Self {
        let mut sorted: Vec<f64> = buckets_ms.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let bucket_count = sorted.len() + 1; // +1 for +Inf
        Self {
            name: name.into(),
            description: description.into(),
            buckets_ms: sorted,
            inner: Mutex::new(HistogramInner {
                bucket_counts: vec![0; bucket_count],
                count: 0,
                sum_ms: 0.0,
            }),
        }
    }

    pub fn with_default_buckets(
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self::new(name, description, DEFAULT_BUCKETS_MS)
    }

    pub fn observe(&self, value_ms: f64) {
        if value_ms < 0.0 {
            return;
        }
        let mut inner = self.inner.lock();
        inner.count += 1;
        inner.sum_ms += value_ms;
        for (i, b) in self.buckets_ms.iter().enumerate() {
            if value_ms <= *b {
                inner.bucket_counts[i] += 1;
                return;
            }
        }
        // 落入 +Inf 桶
        let last_idx = inner.bucket_counts.len() - 1;
        inner.bucket_counts[last_idx] += 1;
    }

    pub fn snapshot(&self) -> HistogramSnapshot {
        let inner = self.inner.lock();
        let avg = if inner.count > 0 {
            inner.sum_ms / inner.count as f64
        } else {
            0.0
        };
        let mut buckets: HashMap<String, u64> = HashMap::new();
        for (i, b) in self.buckets_ms.iter().enumerate() {
            buckets.insert(format!("<={b}ms"), inner.bucket_counts[i]);
        }
        let last_idx = inner.bucket_counts.len() - 1;
        buckets.insert(">max".to_string(), inner.bucket_counts[last_idx]);
        HistogramSnapshot {
            count: inner.count,
            sum_ms: inner.sum_ms,
            avg_ms: avg,
            buckets,
        }
    }

    pub fn reset(&self) {
        let mut inner = self.inner.lock();
        for c in inner.bucket_counts.iter_mut() {
            *c = 0;
        }
        inner.count = 0;
        inner.sum_ms = 0.0;
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramSnapshot {
    pub count: u64,
    pub sum_ms: f64,
    pub avg_ms: f64,
    pub buckets: HashMap<String, u64>,
}

// ────────────────────────────────────────────────────────────────
// Timer guard
// ────────────────────────────────────────────────────────────────

/// 计时器守卫 — drop 时自动记录耗时
///
/// 持有 `Arc<Histogram>` 和可选的 `Arc<Counter>` 以便独立于 `MetricsRegistry` 生命周期。
pub struct TimerGuard {
    histogram: Arc<Histogram>,
    start: Instant,
    failure_counter: Option<Arc<Counter>>,
    completed: bool,
}

impl TimerGuard {
    /// 显式完成（提前 stop 计时）
    pub fn stop(&mut self) {
        if self.completed {
            return;
        }
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        self.histogram.observe(elapsed_ms);
        self.completed = true;
    }

    /// 标记为失败（额外递增 failure 计数器）
    pub fn fail(&mut self) {
        if self.completed {
            return;
        }
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        self.histogram.observe(elapsed_ms);
        if let Some(c) = &self.failure_counter {
            c.inc_one();
        }
        self.completed = true;
    }
}

impl Drop for TimerGuard {
    fn drop(&mut self) {
        if !self.completed {
            let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
            self.histogram.observe(elapsed_ms);
        }
    }
}

// ────────────────────────────────────────────────────────────────
// MetricsRegistry
// ────────────────────────────────────────────────────────────────

/// 全局指标注册表
pub struct MetricsRegistry {
    counters: RwLock<HashMap<String, Arc<Counter>>>,
    histograms: RwLock<HashMap<String, Arc<Histogram>>>,
    gauges: RwLock<HashMap<String, Arc<Gauge>>>,
    degradation_total: Mutex<u64>,
    persist_path: RwLock<PathBuf>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        let persist_path = persistence_path();
        if let Some(parent) = persist_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self {
            counters: RwLock::new(HashMap::new()),
            histograms: RwLock::new(HashMap::new()),
            gauges: RwLock::new(HashMap::new()),
            degradation_total: Mutex::new(0),
            persist_path: RwLock::new(persist_path),
        }
    }

    /// 获取或创建 counter
    pub fn counter(&self, name: &str) -> Arc<Counter> {
        {
            let map = self.counters.read();
            if let Some(c) = map.get(name) {
                return c.clone();
            }
        }
        let mut map = self.counters.write();
        if let Some(c) = map.get(name) {
            return c.clone();
        }
        let c = Arc::new(Counter::new(name, ""));
        map.insert(name.to_string(), c.clone());
        c
    }

    /// 获取或创建 counter（带描述）
    pub fn counter_with_desc(&self, name: &str, desc: &str) -> Arc<Counter> {
        {
            let map = self.counters.read();
            if let Some(c) = map.get(name) {
                return c.clone();
            }
        }
        let mut map = self.counters.write();
        if let Some(c) = map.get(name) {
            return c.clone();
        }
        let c = Arc::new(Counter::new(name, desc));
        map.insert(name.to_string(), c.clone());
        c
    }

    /// 获取或创建 histogram（默认桶）
    pub fn histogram(&self, name: &str) -> Arc<Histogram> {
        {
            let map = self.histograms.read();
            if let Some(h) = map.get(name) {
                return h.clone();
            }
        }
        let mut map = self.histograms.write();
        if let Some(h) = map.get(name) {
            return h.clone();
        }
        let h = Arc::new(Histogram::with_default_buckets(name, ""));
        map.insert(name.to_string(), h.clone());
        h
    }

    /// 获取或创建 gauge
    pub fn gauge(&self, name: &str) -> Arc<Gauge> {
        {
            let map = self.gauges.read();
            if let Some(g) = map.get(name) {
                return g.clone();
            }
        }
        let mut map = self.gauges.write();
        if let Some(g) = map.get(name) {
            return g.clone();
        }
        let g = Arc::new(Gauge::new(name, ""));
        map.insert(name.to_string(), g.clone());
        g
    }

    /// 启动计时器（drop 时自动记录耗时）
    pub fn start_timer(&self, name: &str) -> TimerGuard {
        let h = self.histogram(name);
        TimerGuard {
            histogram: h,
            start: Instant::now(),
            failure_counter: None,
            completed: false,
        }
    }

    /// 启动计时器（带失败计数器；`fail()` 时递增）
    pub fn start_timer_with_failure(&self, name: &str, failure_name: &str) -> TimerGuard {
        let h = self.histogram(name);
        let c = self.counter(failure_name);
        TimerGuard {
            histogram: h,
            start: Instant::now(),
            failure_counter: Some(c),
            completed: false,
        }
    }

    /// 记录一次"尝试走降级路径"的事件
    ///
    /// 一旦增加就是严重问题：用户数据可能被劣势信息污染。
    /// 永远不应在生产中调用此方法。
    pub fn record_degradation_attempt(&self, location: &str) {
        {
            let mut total = self.degradation_total.lock();
            *total += 1;
        }
        tracing::error!(
            "[METRICS] 检测到降级路径被触发: {} (总计 {}，必须为 0)",
            location,
            self.get_degradation_total()
        );
        self.counter(MEM_DEGRADATION_ATTEMPTS).inc_one();
    }

    pub fn get_degradation_total(&self) -> u64 {
        *self.degradation_total.lock()
    }

    /// 获取快照（所有 counter / histogram / gauge + 降级总数）
    pub fn get_snapshot(&self) -> MetricsSnapshot {
        let counters: HashMap<String, u64> = self
            .counters
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.value()))
            .collect();
        let histograms: HashMap<String, HistogramSnapshot> = self
            .histograms
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.snapshot()))
            .collect();
        let gauges: HashMap<String, f64> = self
            .gauges
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.value()))
            .collect();
        MetricsSnapshot {
            counters,
            histograms,
            gauges,
            degradation_attempts_total: self.get_degradation_total(),
            captured_at: chrono::Local::now().to_rfc3339(),
        }
    }

    /// 重置所有指标（主要供测试使用）
    pub fn reset(&self) {
        for c in self.counters.read().values() {
            c.reset();
        }
        for h in self.histograms.read().values() {
            h.reset();
        }
        for g in self.gauges.read().values() {
            g.reset();
        }
        *self.degradation_total.lock() = 0;
    }

    /// 断言：从未尝试走降级路径（失败时 panic）
    pub fn assert_no_degradation(&self) {
        let total = self.get_degradation_total();
        if total > 0 {
            panic!("检测到 {total} 次降级尝试（必须为 0）");
        }
    }

    /// 持久化到 `metrics.json`（每日轮转文件名）
    pub fn persist(&self) -> Result<(), std::io::Error> {
        let snapshot = self.get_snapshot();
        let path = self.persist_path.read().clone();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// 从持久化文件加载（仅恢复 counter / gauge，histogram 不恢复）
    pub fn load_persisted(&self) -> Result<(), std::io::Error> {
        let path = self.persist_path.read().clone();
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&path)?;
        if content.trim().is_empty() {
            return Ok(());
        }
        match serde_json::from_str::<MetricsSnapshot>(&content) {
            Ok(snap) => {
                for (name, value) in snap.counters {
                    self.counter(&name).set(value);
                }
                for (name, value) in snap.gauges {
                    self.gauge(&name).set(value);
                }
                tracing::info!("[Metrics] 从 {} 加载了持久化状态", path.display());
                Ok(())
            }
            Err(e) => {
                tracing::warn!("[Metrics] 解析持久化文件失败: {e}");
                Ok(())
            }
        }
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// 为 Counter 增加 set（仅用于 load_persisted 恢复）
impl Counter {
    pub fn set(&self, v: u64) {
        *self.value.lock() = v;
    }
}

/// 全局单例
pub static METRICS: OnceCell<Arc<MetricsRegistry>> = OnceCell::new();

/// 获取全局单例
pub fn metrics() -> Arc<MetricsRegistry> {
    METRICS
        .get_or_init(|| {
            let m = Arc::new(MetricsRegistry::new());
            if let Err(e) = m.load_persisted() {
                tracing::warn!("[Metrics] 加载持久化状态失败: {e}");
            }
            m
        })
        .clone()
}

/// 模块级简写：启动计时器
pub fn time_block(name: &str) -> TimerGuard {
    metrics().start_timer(name)
}

// ────────────────────────────────────────────────────────────────
// 持久化路径（每日轮转）
// ────────────────────────────────────────────────────────────────

fn persistence_path() -> PathBuf {
    let dir = path::get_user_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    dir.join(format!("metrics_{today}.json"))
}

// ────────────────────────────────────────────────────────────────
// Snapshot
// ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub counters: HashMap<String, u64>,
    pub histograms: HashMap<String, HistogramSnapshot>,
    pub gauges: HashMap<String, f64>,
    pub degradation_attempts_total: u64,
    pub captured_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter() {
        let reg = MetricsRegistry::new();
        let c = reg.counter("test.counter");
        c.inc_one();
        c.inc(2);
        assert_eq!(c.value(), 3);
    }

    #[test]
    fn test_histogram() {
        let reg = MetricsRegistry::new();
        let h = reg.histogram("test.h");
        h.observe(5.0);
        h.observe(100.0);
        h.observe(10000.0);
        let snap = h.snapshot();
        assert_eq!(snap.count, 3);
        assert!((snap.sum_ms - 10105.0).abs() < 0.01);
        assert!(snap.avg_ms > 0.0);
    }

    #[test]
    fn test_gauge() {
        let reg = MetricsRegistry::new();
        let g = reg.gauge("test.g");
        g.set(42.0);
        g.inc(8.0);
        g.dec(10.0);
        assert!((g.value() - 40.0).abs() < 0.001);
    }

    #[test]
    fn test_timer_guard() {
        let reg = MetricsRegistry::new();
        {
            let _t = reg.start_timer("test.timer");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let h = reg.histogram("test.timer");
        assert!(h.snapshot().count >= 1);
    }

    #[test]
    fn test_degradation() {
        let reg = MetricsRegistry::new();
        assert_eq!(reg.get_degradation_total(), 0);
        reg.record_degradation_attempt("test");
        assert_eq!(reg.get_degradation_total(), 1);
        assert_eq!(reg.counter(MEM_DEGRADATION_ATTEMPTS).value(), 1);
    }

    #[test]
    fn test_snapshot_structure() {
        let reg = MetricsRegistry::new();
        reg.counter("a").inc_one();
        reg.gauge("b").set(1.5);
        let snap = reg.get_snapshot();
        assert_eq!(snap.counters.get("a"), Some(&1));
        assert!(snap.gauges.get("b").is_some());
    }

    #[test]
    fn test_persist_and_load() {
        let temp_dir = std::env::temp_dir().join("vivian_metrics_test");
        let _ = std::fs::create_dir_all(&temp_dir);
        let path = temp_dir.join("metrics_test.json");
        let reg = MetricsRegistry::new();
        *reg.persist_path.write() = path.clone();
        reg.counter("persist.counter").inc(5);
        reg.gauge("persist.gauge").set(1.25);
        reg.persist().unwrap();

        let reg2 = MetricsRegistry::new();
        *reg2.persist_path.write() = path.clone();
        reg2.load_persisted().unwrap();
        assert_eq!(reg2.counter("persist.counter").value(), 5);
        assert!((reg2.gauge("persist.gauge").value() - 1.25).abs() < 0.001);

        let _ = std::fs::remove_file(&path);
    }
}
