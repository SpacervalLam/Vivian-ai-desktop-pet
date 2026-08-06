//! 用户活动日志 —— 被动接收前台窗口切换事件
//!
//! 设计目标：
//! - 由 WorldStateProvider 的 SetWinEventHook 回调驱动，不再独立轮询
//! - 仅在窗口标题变化时记录一条带时间戳的日志
//! - 日志上限 100 条（FIFO 滚动），防止无限增长
//! - 日志作为内心独白生成的信息源之一，生成后清空重置
//!
//! 性能特性：
//! - 无后台线程，事件回调零延迟
//! - Mutex 持有时间极短（仅 push 一条 String）

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::TimeZone;
use parking_lot::Mutex;
use serde::Serialize;

/// 单条活动日志
#[derive(Debug, Clone, Serialize)]
pub struct ActivityEntry {
    /// Unix 时间戳（秒）
    pub timestamp: i64,
    /// 本地时间字符串（如 "14:30:05"），便于 LLM 阅读
    pub time_str: String,
    /// 前台窗口标题
    pub window_title: String,
    /// 进程名（如 "Code"/"chrome"）
    pub process: String,
    /// 语义分类（如 "编程"/"浏览"/"社交"），由窗口标题推断
    pub category: Option<String>,
}

/// 日志上限（FIFO 滚动）
const MAX_ENTRIES: usize = 100;

/// 用户活动日志记录器
///
/// 由 WorldStateProvider 在前台窗口切换事件中调用 `record()` 写入。
/// `enabled` 标志由 `start()`/`stop()` 翻转，用于总开关切换时暂停/恢复记录。
/// `drain()` 取出全部日志并清空，供内心独白生成消费。
pub struct ActivityJournal {
    /// 日志条目（FIFO）
    entries: Arc<Mutex<Vec<ActivityEntry>>>,
    /// 上次窗口标题（去重，避免连续记录相同窗口）
    last_window: Arc<Mutex<Option<String>>>,
    /// 是否启用记录（由 start/stop 翻转）
    enabled: Arc<AtomicBool>,
}

impl ActivityJournal {
    /// 创建记录器（默认不启用，需 start() 启用）
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            last_window: Arc::new(Mutex::new(None)),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 启用记录（幂等）
    pub fn start(&self) {
        if !self.enabled.swap(true, Ordering::SeqCst) {
            tracing::info!("[activity_journal] 已启用前台窗口事件记录");
        }
    }

    /// 停用记录（幂等）
    pub fn stop(&self) {
        if self.enabled.swap(false, Ordering::SeqCst) {
            tracing::info!("[activity_journal] 已停用前台窗口事件记录");
        }
    }

    /// 是否启用
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// 由 WorldStateProvider 在前台窗口切换时调用
    ///
    /// - 未启用时直接返回
    /// - 标题与上次相同时跳过（去重）
    /// - 标题变化时记录一条日志，FIFO 滚动
    pub fn record(&self, title: String, process: String) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }

        let trimmed = title.trim();
        if trimmed.is_empty() {
            return;
        }

        let mut last = self.last_window.lock();
        let changed = match &*last {
            Some(prev) => prev != trimmed,
            None => true,
        };
        if !changed {
            return;
        }

        let now = chrono::Local::now();
        let category = classify_window_title(trimmed, &process);
        let entry = ActivityEntry {
            timestamp: now.timestamp(),
            time_str: now.format("%H:%M:%S").to_string(),
            window_title: trimmed.to_string(),
            process,
            category,
        };
        let mut ents = self.entries.lock();
        if ents.len() >= MAX_ENTRIES {
            ents.remove(0); // FIFO 滚动
        }
        ents.push(entry);
        *last = Some(trimmed.to_string());
    }

    /// 取出全部日志并清空（供内心独白生成消费）
    ///
    /// 调用后日志重置，重新开始记录前台窗口。
    pub fn drain(&self) -> Vec<ActivityEntry> {
        let mut entries = self.entries.lock();
        let drained = entries.drain(..).collect::<Vec<_>>();
        // 重置 last_window，使下一个窗口（即使与之前相同）也会被记录
        *self.last_window.lock() = None;
        drained
    }

    /// 当前日志条数
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// 将日志格式化为 LLM 可读的摘要文本
    ///
    /// 按语义分类聚合后输出摘要，而非逐条罗列窗口标题。
    /// 格式示例：`用户最近主要在：编程（VS Code 12次、Terminal 3次）、浏览（Chrome 5次）、社交（WeChat 2次），共切换 22 次`
    /// 空日志返回空字符串。
    pub fn to_brief(&self) -> String {
        let entries = self.entries.lock();
        if entries.is_empty() {
            return String::new();
        }

        let total = entries.len();

        // 按 category 分组统计，同时记录每个 category 下的 app 频次
        let mut category_counts: HashMap<String, u32> = HashMap::new();
        let mut category_apps: HashMap<String, HashMap<String, u32>> = HashMap::new();

        for e in entries.iter() {
            let cat = e
                .category
                .as_deref()
                .unwrap_or("其他")
                .to_string();
            *category_counts.entry(cat.clone()).or_insert(0) += 1;

            let app = extract_app_name(&e.window_title, &e.process);
            let apps = category_apps.entry(cat).or_default();
            *apps.entry(app).or_insert(0) += 1;
        }

        // 按频次降序排列 category
        let mut sorted_cats: Vec<(String, u32)> = category_counts.into_iter().collect();
        sorted_cats.sort_by(|a, b| b.1.cmp(&a.1));

        // 渲染每个分类：分类名（Top3 app 频次）
        const TOP_APPS: usize = 3;
        let parts: Vec<String> = sorted_cats
            .iter()
            .take(5) // 最多显示 5 个分类
            .map(|(cat, _)| {
                let apps = category_apps.get(cat).cloned().unwrap_or_default();
                let mut sorted_apps: Vec<(String, u32)> = apps.into_iter().collect();
                sorted_apps.sort_by(|a, b| b.1.cmp(&a.1));
                let app_strs: Vec<String> = sorted_apps
                    .iter()
                    .take(TOP_APPS)
                    .map(|(name, n)| format!("{} {}次", name, n))
                    .collect();
                if app_strs.is_empty() {
                    cat.clone()
                } else {
                    format!("{}（{}）", cat, app_strs.join("、"))
                }
            })
            .collect();

        format!(
            "用户最近主要在：{}，共切换 {} 次",
            parts.join("、"),
            total
        )
    }

    pub fn snapshot(&self) -> Vec<ActivityEntry> {
        self.entries.lock().clone()
    }

    pub fn to_daily_brief(&self) -> String {
        let entries = self.entries.lock();
        if entries.is_empty() {
            return String::new();
        }

        let mut category_counts: HashMap<String, u32> = HashMap::new();
        let mut first_ts: Option<i64> = None;
        let mut last_ts: Option<i64> = None;

        for e in entries.iter() {
            let cat = e.category.as_deref().unwrap_or("其他").to_string();
            *category_counts.entry(cat).or_insert(0) += 1;
            first_ts = Some(first_ts.map_or(e.timestamp, |f| f.min(e.timestamp)));
            last_ts = Some(last_ts.map_or(e.timestamp, |l| l.max(e.timestamp)));
        }

        let mut sorted_cats: Vec<(String, u32)> = category_counts.into_iter().collect();
        sorted_cats.sort_by(|a, b| b.1.cmp(&a.1));

        let time_range = match (first_ts, last_ts) {
            (Some(f), Some(l)) => {
                let fmt = |ts: i64| {
                    chrono::Local
                        .timestamp_opt(ts, 0)
                        .single()
                        .map(|dt| dt.format("%H:%M").to_string())
                        .unwrap_or_default()
                };
                format!("（{}~{}）", fmt(f), fmt(l))
            }
            _ => String::new(),
        };

        let parts: Vec<String> = sorted_cats
            .iter()
            .take(4)
            .map(|(cat, n)| format!("{} {}次", cat, n))
            .collect();

        format!("用户活动{}：{}", time_range, parts.join("、"))
    }
}

impl Default for ActivityJournal {
    fn default() -> Self {
        Self::new()
    }
}

/// 从窗口标题与进程名中提取应用名称
///
/// 优先使用进程名（更稳定），无进程名时从标题末尾段提取。
/// 常见标题格式：`"main.rs - vivian-rs - VS Code"` → `"VS Code"`
fn extract_app_name(title: &str, process: &str) -> String {
    // 进程名优先（更稳定，不受窗口标题格式影响）
    if !process.is_empty() {
        let p = process.trim();
        if !p.is_empty() && p.len() < 40 {
            return p.to_string();
        }
    }

    // 回退到标题分隔符提取
    for sep in &[" - ", " — ", " | ", " – "] {
        if let Some(pos) = title.rfind(sep) {
            let last_part = title[pos + sep.len()..].trim();
            if !last_part.is_empty() && last_part.len() < 30 {
                return last_part.to_string();
            }
        }
    }
    // 无分隔符，取整个标题（截断）
    let trimmed = title.trim();
    if trimmed.len() > 40 {
        trimmed[..40].to_string()
    } else {
        trimmed.to_string()
    }
}

/// 根据窗口标题与进程名推断语义分类（中文标签）
///
/// 轻量级规则分类，与 `SmartAppClassifier`（brain 模块）共享分类逻辑，但此处为独立实现，
/// 避免回调路径依赖 brain 模块的 Arc/Mutex 开销。
fn classify_window_title(title: &str, process: &str) -> Option<String> {
    let lower = title.to_lowercase();
    let app = extract_app_name(title, process).to_lowercase();
    let process_lower = process.to_lowercase();
    let combined = format!("{} {} {}", lower, app, process_lower);

    // 编程开发
    for kw in &[
        "code", "vscode", "intellij", "pycharm", "idea", "terminal",
        "powershell", "cmd", "git", "vim", "emacs", "sublime",
        "rust-rover", "goland", "webstorm", "devenv", "android studio",
        "xcode", "cargo", "npm",
    ] {
        if combined.contains(kw) {
            return Some("编程".to_string());
        }
    }

    // 浏览器
    for kw in &["chrome", "firefox", "edge", "safari", "opera", "brave", "arc "] {
        if combined.contains(kw) {
            return Some("浏览".to_string());
        }
    }

    // 游戏
    for kw in &["steam", "epic", "battle.net", "minecraft", "game", "gog", "origin", "ea app"] {
        if combined.contains(kw) {
            return Some("游戏".to_string());
        }
    }

    // 视频
    for kw in &["bilibili", "youtube", "netflix", "vlc", "potplayer", "mpv", "iqiyi", "video"] {
        if combined.contains(kw) {
            return Some("视频".to_string());
        }
    }

    // 社交聊天
    for kw in &[
        "wechat", "qq", "telegram", "discord", "slack", "teams",
        "skype", "钉钉", "飞书", "line",
    ] {
        if combined.contains(kw) {
            return Some("社交".to_string());
        }
    }

    // 办公
    for kw in &[
        "word", "excel", "powerpoint", "wps", "notion", "onenote",
        "google docs", "obsidian", "印象笔记",
    ] {
        if combined.contains(kw) {
            return Some("办公".to_string());
        }
    }

    // 邮件
    for kw in &["mail", "outlook", "foxmail", "gmail"] {
        if combined.contains(kw) {
            return Some("邮件".to_string());
        }
    }

    // 媒体创作
    for kw in &[
        "spotify", "photoshop", "illustrator", "premiere", "after effects",
        "figma", "blender", "foobar", "music",
    ] {
        if combined.contains(kw) {
            return Some("媒体".to_string());
        }
    }

    // 系统工具
    for kw in &[
        "explorer", "taskmgr", "control", "settings", "registry",
        "system", "antivirus", "文件资源管理器",
    ] {
        if combined.contains(kw) {
            return Some("系统".to_string());
        }
    }

    Some("其他".to_string())
}
