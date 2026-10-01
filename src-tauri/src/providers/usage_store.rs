//! Token 用量持久化存储。
//!
//! 存储位置：`<用户数据目录>/token_usage.json`
//! 数据结构：按天（ISO 日期）聚合 + 按模型细分，含缓存命中 / 写入统计。
//!
//! 写入策略：`record_usage` 立即更新内存缓存，1 秒防抖落盘
//! （临时文件 + rename 原子写，避免高频流式回调写穿磁盘）。
//! 读取策略：首次访问时从磁盘加载到内存，后续直接读缓存。

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::utils::path::get_user_data_dir;

/// 落盘防抖间隔。
const FLUSH_DEBOUNCE: Duration = Duration::from_secs(1);

/// 单个模型（或"未归类"）的用量累计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input: u64,
    pub output: u64,
    /// 厂商明确上报的缓存读取 token。
    pub hit: u64,
    /// 已上报缓存明细的输入中，未命中缓存的部分。
    pub miss: u64,
    /// 厂商明确上报的缓存创建 token。
    pub cache_creation: u64,
    /// 厂商实际返回 usage 的请求数。
    pub requests: u64,
}

/// 一天的用量汇总（含按模型细分）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DayUsage {
    pub input: u64,
    pub output: u64,
    pub hit: u64,
    pub miss: u64,
    pub cache_creation: u64,
    pub requests: u64,
    #[serde(default)]
    pub models: HashMap<String, ModelUsage>,
    /// Actual provider-reported usage attributed to the request's purpose.
    #[serde(default)]
    pub tasks: HashMap<String, ModelUsage>,
    /// Actual route → actual model. Historical files have no route attribution.
    #[serde(default)]
    pub route_models: HashMap<String, HashMap<String, ModelUsage>>,
}

tokio::task_local! { static CURRENT_TASK: String; }
tokio::task_local! { static CURRENT_ROUTE: String; }

pub async fn with_context<T>(task: &str, route: &str, future: impl Future<Output = T>) -> T {
    CURRENT_ROUTE.scope(route.to_string(), with_task(task, future)).await
}

/// A stream can report cumulative usage more than once. Count one call and retain
/// the largest provider-reported count for each category, never sum snapshots.
#[derive(Default)]
pub struct StreamUsageAccumulator { tokens: [u64; 4] }

impl StreamUsageAccumulator {
    pub fn observe(&mut self, input: u64, output: u64, read: u64, write: u64) {
        for (stored, reported) in self.tokens.iter_mut().zip([input, output, read, write]) {
            *stored = (*stored).max(reported);
        }
    }

    pub fn record(self, task: Option<&str>, route: Option<&str>, model: &str) {
        let [input, output, read, write] = self.tokens;
        record_usage_for_context(task, route, model, input, output, read, write);
    }

    pub fn record_current(self, model: &str) {
        let [input, output, read, write] = self.tokens;
        record_usage(model, input, output, read, write);
    }
}

pub async fn with_task<T>(tag: &str, future: impl Future<Output = T>) -> T {
    CURRENT_TASK.scope(tag.to_string(), future).await
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageStore {
    #[serde(default)]
    days: HashMap<String, DayUsage>,
}

struct StoreState {
    cache: UsageStore,
    loaded: bool,
    read_error: Option<String>,
}

static STATE: Lazy<Mutex<StoreState>> = Lazy::new(|| {
    Mutex::new(StoreState {
        cache: UsageStore::default(),
        loaded: false,
        read_error: None,
    })
});

/// 防抖落盘调度标记。
static FLUSH_SCHEDULED: AtomicBool = AtomicBool::new(false);

fn store_path() -> PathBuf {
    get_user_data_dir().join("token_usage.json")
}

/// 今日的 ISO 日期键（本地时区）。
fn today_key() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 距今 n 天的 ISO 日期键（本地时区）。
fn day_key_offset(offset: u64) -> String {
    chrono::Local::now()
        .checked_sub_signed(chrono::Duration::days(offset as i64))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn load_from_disk() -> Result<UsageStore, String> {
    match std::fs::read_to_string(store_path()) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|_| "Local usage file is invalid".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UsageStore::default()),
        Err(_) => Err("Local usage file could not be read".to_string()),
    }
}

fn ensure_loaded(state: &mut StoreState) {
    if !state.loaded {
        match load_from_disk() {
            Ok(store) => { state.cache = store; state.read_error = None; state.loaded = true; }
            Err(error) => { state.read_error = Some(error); }
        }
    }
}

/// 原子落盘：先写 .tmp 再 rename，避免半写文件。
fn flush_locked(state: &StoreState) {
    if !state.loaded || state.read_error.is_some() { return; }
    let path = store_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(&state.cache) {
        if let Err(error) = crate::utils::fs::write_atomic(&path, &json) {
            tracing::error!("[usage_store] 用量落盘失败 {}: {}", path.display(), error);
        }
    }
}

/// 防抖落盘：已有待执行的落盘任务时不重复调度。
fn schedule_flush() {
    if FLUSH_SCHEDULED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(FLUSH_DEBOUNCE);
        FLUSH_SCHEDULED.store(false, Ordering::Relaxed);
        if let Ok(state) = STATE.lock() {
            flush_locked(&state);
        }
    });
}

/// 把一次模型调用的用量累加到指定日期与模型条目。
fn apply_usage(
    day: &mut DayUsage,
    model: &str,
    task: Option<&str>,
    route: Option<&str>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) {
    // Provider parsing already removes cached input from `input` when needed.
    let miss = input;

    day.input += input;
    day.output += output;
    day.hit += cache_read;
    day.miss += miss;
    day.cache_creation += cache_write;
    day.requests += 1;

    let entry = day.models.entry(model.to_string()).or_default();
    entry.input += input;
    entry.output += output;
    entry.hit += cache_read;
    entry.miss += miss;
    entry.cache_creation += cache_write;
    entry.requests += 1;
    let task = task.filter(|t| !t.trim().is_empty()).unwrap_or("unattributed");
    let entry = day.tasks.entry(task.to_string()).or_default();
    entry.input += input;
    entry.output += output;
    entry.hit += cache_read;
    entry.miss += miss;
    entry.cache_creation += cache_write;
    entry.requests += 1;
    let route = route.filter(|r| !r.trim().is_empty()).unwrap_or("unattributed");
    let entry = day.route_models.entry(route.to_string()).or_default().entry(model.to_string()).or_default();
    entry.input += input;
    entry.output += output;
    entry.hit += cache_read;
    entry.miss += miss;
    entry.cache_creation += cache_write;
    entry.requests += 1;
}

/// 记录一次模型调用的 token 用量（异步防抖落盘）。
///
/// `input_tokens` 语义与 `StreamEvent::Usage` 一致：未命中缓存的输入，
/// 缓存读取 / 写入单独计。
pub fn record_usage(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) {
    let tag = CURRENT_TASK.try_with(Clone::clone).ok();
    record_usage_for_task(tag.as_deref(), model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens);
}

/// Stream forwarding runs in a spawned task, so it passes the tag explicitly.
pub fn record_usage_for_task(
    task: Option<&str>,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) {
    let route = CURRENT_ROUTE.try_with(Clone::clone).ok();
    record_usage_for_context(task, route.as_deref(), model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens);
}

/// Spawned stream forwarding passes both attribution dimensions explicitly.
pub fn record_usage_for_context(
    task: Option<&str>, route: Option<&str>, model: &str,
    input_tokens: u64, output_tokens: u64, cache_read_tokens: u64, cache_write_tokens: u64,
) {
    if input_tokens == 0 && output_tokens == 0 && cache_read_tokens == 0 && cache_write_tokens == 0 {
        return;
    }
    let model = if model.trim().is_empty() { "未归类" } else { model.trim() };
    let key = today_key();
    if let Ok(mut state) = STATE.lock() {
        ensure_loaded(&mut state);
        if state.read_error.is_some() { return; }
        let day = state.cache.days.entry(key).or_default();
        apply_usage(
            day,
            model,
            task,
            route,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        );
        schedule_flush();
    }
}

/// 立即落盘（应用退出时调用）。
pub fn flush() {
    if let Ok(state) = STATE.lock() {
        flush_locked(&state);
    }
}

/// 清空所有本地用量记录。
pub fn clear() {
    if let Ok(mut state) = STATE.lock() {
        state.cache = UsageStore::default();
        state.loaded = true;
        state.read_error = None;
        flush_locked(&state);
    }
}

/// 模型用量报表条目。
#[derive(Debug, Clone, Serialize)]
pub struct ModelUsageReport {
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub hit: u64,
    pub miss: u64,
    pub cache_creation: u64,
    pub requests: u64,
}

/// 日用量报表条目。
#[derive(Debug, Clone, Serialize)]
pub struct DayUsageReport {
    pub date: String,
    pub models: Vec<ModelUsageReport>,
    pub tasks: Vec<TaskUsageReport>,
    pub routes: Vec<RouteUsageReport>,
    pub input: u64,
    pub output: u64,
    pub hit: u64,
    pub miss: u64,
    pub cache_creation: u64,
    pub requests: u64,
}

/// 用量报表（近 N 天日汇总 + 模型占比）。
#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    pub days: Vec<DayUsageReport>,
    pub models: Vec<ModelUsageReport>,
    pub tasks: Vec<TaskUsageReport>,
    pub routes: Vec<RouteUsageReport>,
    pub read_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskUsageReport {
    pub task: String,
    pub input: u64,
    pub output: u64,
    pub hit: u64,
    pub cache_creation: u64,
    pub requests: u64,
}

/// Route usage includes the actually observed model breakdown, including fallbacks.
#[derive(Debug, Clone, Serialize)]
pub struct RouteUsageReport {
    pub route: String,
    #[serde(flatten)]
    pub usage: ModelUsage,
    pub models: Vec<ModelUsageReport>,
}

fn add_usage(target: &mut ModelUsage, source: &ModelUsage) {
    target.input += source.input;
    target.output += source.output;
    target.hit += source.hit;
    target.miss += source.input;
    target.cache_creation += source.cache_creation;
    target.requests += source.requests;
}

fn model_reports(models: &HashMap<String, ModelUsage>) -> Vec<ModelUsageReport> {
    let mut rows: Vec<_> = models.iter().map(|(model, u)| ModelUsageReport {
        model: model.clone(), input: u.input, output: u.output, hit: u.hit,
        miss: u.input, cache_creation: u.cache_creation, requests: u.requests,
    }).collect();
    rows.sort_by_key(|row| (std::cmp::Reverse(row.input + row.output + row.hit + row.cache_creation), row.model.clone()));
    rows
}

fn task_reports(tasks: &HashMap<String, ModelUsage>) -> Vec<TaskUsageReport> {
    let mut rows: Vec<_> = tasks.iter().map(|(task, u)| TaskUsageReport {
        task: task.clone(), input: u.input, output: u.output, hit: u.hit,
        cache_creation: u.cache_creation, requests: u.requests,
    }).collect();
    rows.sort_by_key(|row| (std::cmp::Reverse(row.input + row.output + row.hit + row.cache_creation), row.task.clone()));
    rows
}

fn route_reports(routes: &HashMap<String, HashMap<String, ModelUsage>>) -> Vec<RouteUsageReport> {
    let mut rows: Vec<_> = routes.iter().map(|(route, models)| {
        let mut usage = ModelUsage::default();
        for model in models.values() { add_usage(&mut usage, model); }
        RouteUsageReport { route: route.clone(), usage, models: model_reports(models) }
    }).collect();
    rows.sort_by_key(|row| (std::cmp::Reverse(row.usage.input + row.usage.output + row.usage.hit + row.usage.cache_creation), row.route.clone()));
    rows
}

fn build_report(store: &UsageStore, keys: Vec<String>, read_error: Option<String>) -> UsageReport {
    let mut day_reports = Vec::with_capacity(keys.len());
    let mut by_model: HashMap<String, ModelUsage> = HashMap::new();
    let mut by_task: HashMap<String, ModelUsage> = HashMap::new();
    let mut by_route: HashMap<String, HashMap<String, ModelUsage>> = HashMap::new();
    for date in keys {
        let empty = DayUsage::default();
        let day = store.days.get(&date).unwrap_or(&empty);
        day_reports.push(DayUsageReport {
            date, input: day.input, output: day.output, hit: day.hit, miss: day.input,
            cache_creation: day.cache_creation, requests: day.requests,
            models: model_reports(&day.models), tasks: task_reports(&day.tasks), routes: route_reports(&day.route_models),
        });
        for (name, usage) in &day.models { add_usage(by_model.entry(name.clone()).or_default(), usage); }
        for (name, usage) in &day.tasks { add_usage(by_task.entry(name.clone()).or_default(), usage); }
        for (route, models) in &day.route_models {
            for (model, usage) in models {
                add_usage(by_route.entry(route.clone()).or_default().entry(model.clone()).or_default(), usage);
            }
        }
    }
    UsageReport { days: day_reports, models: model_reports(&by_model), tasks: task_reports(&by_task), routes: route_reports(&by_route), read_error }
}

/// Query local provider-reported usage; missing dates are unrecorded, not verified zero usage.
pub fn get_usage_report(days: u32) -> UsageReport {
    let keys = (0..days.clamp(1, 365)).rev().map(|i| day_key_offset(i as u64)).collect();
    let Ok(mut state) = STATE.lock() else {
        return build_report(&UsageStore::default(), keys, Some("Local usage store is unavailable".to_string()));
    };
    ensure_loaded(&mut state);
    build_report(&state.cache, keys, state.read_error.clone())
}

#[cfg(test)]
mod attribution_tests {
    use super::*;

    #[test]
    fn cache_and_task_usage_are_counted_once() {
        let mut day = DayUsage::default();
        apply_usage(&mut day, "model", Some("proactive_channel"), Some("simple_judge"), 80, 12, 40, 0);
        assert_eq!(day.input, 80);
        assert_eq!(day.hit, 40);
        assert_eq!(day.miss, 80);
        assert_eq!(day.tasks["proactive_channel"].requests, 1);
        assert_eq!(day.tasks["proactive_channel"].input, 80);
        assert_eq!(day.models["model"].requests, 1);
        assert_eq!(day.route_models["simple_judge"]["model"].input, 80);
        assert!(!day.route_models.contains_key("proactive_channel"));
    }

    #[test]
    fn old_usage_files_deserialize_without_task_breakdown() {
        let old = r#"{"days":{"2026-09-29":{"input":5,"output":1,"hit":0,"miss":5,"cache_creation":0,"requests":1,"models":{}}}}"#;
        let usage: UsageStore = serde_json::from_str(old).unwrap();
        assert!(usage.days["2026-09-29"].tasks.is_empty());
        assert!(usage.days["2026-09-29"].route_models.is_empty());
        let report = build_report(&usage, vec!["2026-09-29".into(), "2026-09-30".into()], None);
        assert_eq!(report.days[0].input, 5);
        assert_eq!(report.days[1].requests, 0);
        assert!(report.routes.is_empty(), "Do not infer historical routes");
    }

    #[test]
    fn route_reports_preserve_models_cache_and_daily_totals() {
        let mut store = UsageStore::default();
        let day = store.days.entry("2026-09-30".into()).or_default();
        apply_usage(day, "primary", Some("proactive_message"), Some("chat"), 10, 2, 30, 5);
        apply_usage(day, "fallback", Some("proactive_message"), Some("chat"), 20, 3, 40, 6);
        apply_usage(day, "primary", Some("reflection"), Some("reflection"), 50, 4, 0, 0);
        let report = build_report(&store, vec!["2026-09-30".into()], None);
        assert_eq!(report.days[0].requests, 3);
        assert_eq!(report.days[0].cache_creation, 11);
        let chat = report.routes.iter().find(|row| row.route == "chat").unwrap();
        assert_eq!(chat.usage.requests, 2);
        assert_eq!(chat.usage.input, 30);
        assert_eq!(chat.usage.hit, 70);
        assert_eq!(chat.models.len(), 2);
        assert_eq!(report.days[0].routes.len(), 2);
        let json = serde_json::to_value(chat).unwrap();
        assert_eq!(json["input"], 30);
        assert_eq!(json["cache_creation"], 11);
        assert!(json.get("usage").is_none(), "Keep the frontend report contract flat");
    }

    #[test]
    fn cumulative_usage_snapshots_do_not_multiply_tokens_or_calls() {
        let mut usage = StreamUsageAccumulator::default();
        usage.observe(100, 5, 30, 0);
        usage.observe(100, 10, 30, 0);
        usage.observe(0, 10, 0, 0);
        assert_eq!(usage.tokens, [100, 10, 30, 0]);
        let mut day = DayUsage::default();
        let [input, output, read, write] = usage.tokens;
        apply_usage(&mut day, "model", Some("chat"), Some("chat"), input, output, read, write);
        assert_eq!(day.requests, 1);
        assert_eq!(day.input, 100);
        assert_eq!(day.output, 10);
    }

    #[tokio::test]
    async fn route_and_purpose_contexts_are_distinct_and_do_not_leak() {
        with_context("proactive_message", "chat", async {
            assert_eq!(CURRENT_TASK.with(Clone::clone), "proactive_message");
            assert_eq!(CURRENT_ROUTE.with(Clone::clone), "chat");
            with_context("reflection", "reflection", async {
                assert_eq!(CURRENT_ROUTE.with(Clone::clone), "reflection");
            }).await;
            assert_eq!(CURRENT_ROUTE.with(Clone::clone), "chat");
        }).await;
        assert!(CURRENT_ROUTE.try_with(Clone::clone).is_err());
    }
}
