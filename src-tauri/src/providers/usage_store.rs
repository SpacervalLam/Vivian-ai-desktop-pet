//! Token 用量持久化存储。
//!
//! 存储位置：`<用户数据目录>/token_usage.json`
//! 数据结构：按天（ISO 日期）聚合 + 按模型细分，含缓存命中 / 写入统计。
//!
//! 写入策略：`record_usage` 立即更新内存缓存，1 秒防抖落盘
//! （临时文件 + rename 原子写，避免高频流式回调写穿磁盘）。
//! 读取策略：首次访问时从磁盘加载到内存，后续直接读缓存。

use std::collections::HashMap;
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
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageStore {
    #[serde(default)]
    days: HashMap<String, DayUsage>,
}

struct StoreState {
    cache: UsageStore,
    loaded: bool,
}

static STATE: Lazy<Mutex<StoreState>> = Lazy::new(|| {
    Mutex::new(StoreState {
        cache: UsageStore::default(),
        loaded: false,
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

fn load_from_disk() -> UsageStore {
    let path = store_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => UsageStore::default(),
    }
}

/// 原子落盘：先写 .tmp 再 rename，避免半写文件。
fn flush_locked(state: &StoreState) {
    let path = store_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string_pretty(&state.cache) {
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
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
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) {
    let cache_read = cache_read.min(input);
    let miss = input - cache_read;

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
    if input_tokens == 0 && output_tokens == 0 {
        return;
    }
    let model = if model.trim().is_empty() { "未归类" } else { model.trim() };
    let key = today_key();
    if let Ok(mut state) = STATE.lock() {
        if !state.loaded {
            state.cache = load_from_disk();
            state.loaded = true;
        }
        let day = state.cache.days.entry(key).or_default();
        apply_usage(
            day,
            model,
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
}

/// 查询近 N 天的用量报表（无数据的天填 0，模型按总 token 降序）。
pub fn get_usage_report(days: u32) -> UsageReport {
    let days = days.clamp(1, 365);
    let Ok(mut state) = STATE.lock() else {
        return UsageReport { days: Vec::new(), models: Vec::new() };
    };
    if !state.loaded {
        state.cache = load_from_disk();
        state.loaded = true;
    }
    let store = &state.cache;

    // 日汇总（升序）
    let mut day_reports = Vec::with_capacity(days as usize);
    let mut by_model: HashMap<String, ModelUsage> = HashMap::new();
    for i in (0..days).rev() {
        let key = day_key_offset(i as u64);
        let day = store.days.get(&key);
        day_reports.push(DayUsageReport {
            date: key,
            input: day.map(|d| d.input).unwrap_or(0),
            output: day.map(|d| d.output).unwrap_or(0),
            hit: day.map(|d| d.hit).unwrap_or(0),
            miss: day.map(|d| d.miss).unwrap_or(0),
            cache_creation: day.map(|d| d.cache_creation).unwrap_or(0),
            requests: day.map(|d| d.requests).unwrap_or(0),
        });
        if let Some(day) = day {
            for (model, usage) in &day.models {
                let entry = by_model.entry(model.clone()).or_default();
                entry.input += usage.input;
                entry.output += usage.output;
                entry.hit += usage.hit;
                entry.miss += usage.miss;
                entry.cache_creation += usage.cache_creation;
                entry.requests += usage.requests;
            }
        }
    }

    // 模型占比（按总 token 降序）
    let mut models: Vec<ModelUsageReport> = by_model
        .into_iter()
        .map(|(model, u)| ModelUsageReport {
            model,
            input: u.input,
            output: u.output,
            hit: u.hit,
            miss: u.miss,
            cache_creation: u.cache_creation,
            requests: u.requests,
        })
        .collect();
    models.sort_by(|a, b| (b.input + b.output).cmp(&(a.input + a.output)));

    UsageReport { days: day_reports, models }
}
