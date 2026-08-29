//! 习惯追踪
//!
//! 核心职责：
//! - 记录用户日常开机时间
//! - 记录用户经常使用的应用（区分工作/娱乐）
//! - 生成个性化的习惯感知提示
//!
//! 持久化到 `%APPDATA%\Vivian\proactive\habits.json`

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

/// 习惯数据保留天数（滚动窗口，超过 90 天的条目自动清理）
const HABIT_RETENTION_DAYS: i64 = 90;

/// 时段习惯采样间隔（秒），避免频繁写盘
const ACTIVITY_SLOT_SAMPLE_INTERVAL: i64 = 600;

/// 时段习惯建模所需最少样本数
const MIN_SLOT_SAMPLES: usize = 3;

/// 历史最常见活动占比阈值，超过才视为"习惯"
const HABIT_RATIO_THRESHOLD: f64 = 0.5;

/// 应用分类（简化版）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCategory {
    Work,
    Entertainment,
    Other,
}

impl AppCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppCategory::Work => "work",
            AppCategory::Entertainment => "entertainment",
            AppCategory::Other => "other",
        }
    }
}

/// 根据应用名判断分类（关键词驱动，无 LLM 依赖）
///
/// 覆盖常见应用，未命中返回 `Other`。
pub fn classify_app(app_name: &str) -> &'static str {
    if app_name.is_empty() {
        return "other";
    }
    let lower = app_name.to_lowercase();
    // 工作类
    let work_kw = [
        "code", "studio", "ide", "vscode", "visual studio", "intellij", "pycharm",
        "webstorm", "goland", "rust", "cargo", "terminal", "powershell", "cmd",
        "word", "excel", "powerpoint", "outlook", "onenote", "wps", "notion",
        "obsidian", "typora", "vim", "emacs", "sublime", "git", "docker", "kubernetes",
        "figma", "photoshop", "illustrator", "premiere", "davinci", "blender",
        "autocad", "matlab", "jupyter", "postman", "navicat", "dbeaver", "sql",
    ];
    for kw in work_kw {
        if lower.contains(kw) {
            return "work";
        }
    }
    // 娱乐类
    let ent_kw = [
        "game", "steam", "epic", "battle.net", "origin", "uplay", "gog",
        "video", "bilibili", "youtube", "netflix", "iqiyi", "youku", "tencent video",
        "qqvideo", "mpc", "vlc", "potplayer", "music", "spotify", "netease cloud",
        "qqmusic", "kugou", "foobar", "browser", "chrome", "firefox", "edge",
        "opera", "brave", "wechat", "qq", "telegram", "discord", "slack",
        "weibo", "twitter", "reddit", "zhihu", "xiaohongshu",
    ];
    for kw in ent_kw {
        if lower.contains(kw) {
            return "entertainment";
        }
    }
    "other"
}

/// 习惯数据（持久化）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HabitData {
    /// "YYYY-MM-DD" → 开机时间戳
    #[serde(default)]
    pub startup_times: HashMap<String, f64>,
    /// "YYYY-MM-DD" → {app_name: 累计秒}
    #[serde(default)]
    pub app_usage: HashMap<String, HashMap<String, f64>>,
    /// 时段活动模式："workday_17" / "weekend_10" → {activity_label: count}
    ///
    /// workday/weekend 区分工作日和周末，hour_bucket 按小时分桶。
    /// 由反思 LLM 写入的 current_activity label 驱动，记录"用户在此时段通常做什么"。
    #[serde(default)]
    pub activity_by_slot: HashMap<String, HashMap<String, u32>>,
}

/// 用户行为模式识别器
pub struct HabitTracker {
    data: RwLock<HabitData>,
    persistence_path: PathBuf,
    /// 上次时段采样时间戳（Unix 秒），节流用
    last_slot_sample_ts: parking_lot::Mutex<f64>,
}

impl HabitTracker {
    pub fn new() -> VivianResult<Self> {
        let dir = get_user_data_dir().join("proactive");
        std::fs::create_dir_all(&dir)
            .map_err(|e| VivianError::Memory(format!("创建习惯目录失败: {e}")))?;
        let path = dir.join("habits.json");
        let data = crate::utils::fs::load_json_or_backup::<HabitData>(&path).unwrap_or_default();
        Ok(Self {
            data: RwLock::new(data),
            persistence_path: path,
            last_slot_sample_ts: parking_lot::Mutex::new(0.0),
        })
    }

    /// 记录一次开机（每日首次）
    pub fn record_startup(&self) {
        self.cleanup_old_entries();
        let now_ts = chrono::Local::now().timestamp() as f64;
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut data = self.data.write();
        if data.startup_times.contains_key(&today) {
            return;
        }
        data.startup_times.insert(today.clone(), now_ts);
        drop(data);
        if let Err(e) = self.save() {
            tracing::warn!("保存习惯数据失败: {e}");
        }
        tracing::debug!("记录开机: {today}");
    }

    /// 记录应用使用时长
    pub fn record_app_usage(&self, app_name: &str, duration_seconds: f64) {
        if app_name.is_empty() {
            return;
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut data = self.data.write();
        let day = data.app_usage.entry(today).or_default();
        let cur = day.get(app_name).copied().unwrap_or(0.0);
        day.insert(app_name.to_string(), cur + duration_seconds);
        drop(data);
        // 节流保存：每 10 分钟由调用方触发，此处不每条都存
    }

    /// 强制保存
    pub fn save(&self) -> VivianResult<()> {
        let data = self.data.read().clone();
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| VivianError::Memory(format!("序列化习惯数据失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)
            .map_err(|e| VivianError::Memory(format!("写入习惯临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换习惯文件失败: {e}")))?;
        Ok(())
    }

    /// 清理超过保留期的旧条目（滚动窗口）
    ///
    /// 在 `record_startup`（每日首次调用）时触发，避免日期 key 无上限累积。
    /// "YYYY-MM-DD" 格式可直接按字符串比较大小。
    fn cleanup_old_entries(&self) {
        let cutoff = chrono::Local::now() - chrono::Duration::days(HABIT_RETENTION_DAYS);
        let cutoff_str = cutoff.format("%Y-%m-%d").to_string();
        let mut data = self.data.write();
        let before = data.startup_times.len() + data.app_usage.len();
        data.startup_times.retain(|k, _| k.as_str() >= cutoff_str.as_str());
        data.app_usage.retain(|k, _| k.as_str() >= cutoff_str.as_str());
        let after = data.startup_times.len() + data.app_usage.len();
        if before != after {
            tracing::debug!(
                "[HabitTracker] 清理 {} 条过期习惯数据（>{}天）",
                before - after,
                HABIT_RETENTION_DAYS
            );
        }
    }

    /// 平均开机时间（小时，浮点）
    pub fn get_avg_startup_hour(&self) -> Option<f64> {
        let data = self.data.read();
        if data.startup_times.is_empty() {
            return None;
        }
        let mut hours: Vec<f64> = Vec::new();
        for ts in data.startup_times.values() {
            if let Some(dt) = chrono::DateTime::from_timestamp(*ts as i64, 0) {
                let local = dt.with_timezone(&chrono::Local);
                hours.push(local.hour() as f64 + local.minute() as f64 / 60.0);
            }
        }
        if hours.is_empty() {
            None
        } else {
            Some(hours.iter().sum::<f64>() / hours.len() as f64)
        }
    }

    /// 已记录天数
    pub fn get_day_count(&self) -> usize {
        self.data.read().startup_times.len()
    }

    /// 常用工作应用 Top N
    pub fn top_work_apps(&self, n: usize) -> Vec<String> {
        self.top_apps(n, "work")
    }

    /// 常用娱乐应用 Top N
    pub fn top_entertainment_apps(&self, n: usize) -> Vec<String> {
        self.top_apps(n, "entertainment")
    }

    fn top_apps(&self, n: usize, category: &str) -> Vec<String> {
        let data = self.data.read();
        let mut totals: HashMap<String, f64> = HashMap::new();
        for day_data in data.app_usage.values() {
            for (app, dur) in day_data {
                *totals.entry(app.clone()).or_insert(0.0) += dur;
            }
        }
        let mut filtered: Vec<(String, f64)> = totals
            .into_iter()
            .filter(|(app, _)| classify_app(app) == category)
            .collect();
        filtered.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        filtered.into_iter().take(n).map(|(a, _)| a).collect()
    }

    /// 习惯摘要
    pub fn get_habit_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "days_recorded": self.get_day_count(),
            "avg_startup_hour": self.get_avg_startup_hour(),
            "top_work_apps": self.top_work_apps(5),
            "top_entertainment_apps": self.top_entertainment_apps(5),
        })
    }

    /// 生成习惯感知提示词（供 prompt 注入）
    pub fn get_habit_prompt(&self) -> String {
        if self.get_day_count() < 3 {
            return String::new();
        }
        let now = chrono::Local::now();
        let hour = now.hour();
        let mut lines: Vec<String> = Vec::new();

        if let Some(avg) = self.get_avg_startup_hour() {
            let avg_hour = avg.floor() as u32;
            let avg_min = ((avg - avg.floor()) * 60.0) as u32;
            if (hour as f64) < avg - 1.0 {
                lines.push(format!(
                    "今天比平时早呢（平时约 {avg_hour}:{avg_min:02} 开机），可以用「今天这么早啊」「今天起得真早」等方式表达惊喜"
                ));
            } else if (hour as f64) > avg + 2.0 {
                lines.push(format!(
                    "今天比平时晚了不少（平时约 {avg_hour}:{avg_min:02} 开机），可以关心地问「今天怎么这么晚」「是不是昨天熬夜了」"
                ));
            } else if ((hour as f64) - avg).abs() <= 1.0 {
                lines.push(format!(
                    "和平时差不多的时间开机（约 {avg_hour}:{avg_min:02}），可以用「今天也很准时呢」来问候"
                ));
            }
        }

        if hour >= 22 || hour < 6 {
            let work_apps = self.top_work_apps(1);
            if let Some(app) = work_apps.first() {
                lines.push(format!(
                    "深夜还在用 {app}，可以用「又是深夜加班啊…」「这么晚还在工作呀」来表达关心"
                ));
            } else {
                lines.push("深夜还在线，可以关心地问「这么晚还不睡呀」".to_string());
            }
        }

        let work_apps = self.top_work_apps(3);
        let ent_apps = self.top_entertainment_apps(3);
        if !work_apps.is_empty() && !ent_apps.is_empty() {
            lines.push(format!(
                "用户常用工作应用: {}；常用娱乐应用: {}",
                work_apps.join(", "),
                ent_apps.join(", ")
            ));
        }

        if lines.is_empty() {
            String::new()
        } else {
            lines
                .into_iter()
                .map(|l| format!("- [习惯感知] {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    /// 今天是否比平时早
    pub fn is_early_today(&self) -> Option<bool> {
        let avg = self.get_avg_startup_hour()?;
        if self.get_day_count() < 3 {
            return None;
        }
        let now = chrono::Local::now();
        let now_hour = now.hour() as f64 + now.minute() as f64 / 60.0;
        Some(now_hour < avg - 1.0)
    }

    /// 今天是否比平时晚
    pub fn is_late_today(&self) -> Option<bool> {
        let avg = self.get_avg_startup_hour()?;
        if self.get_day_count() < 3 {
            return None;
        }
        let now = chrono::Local::now();
        let now_hour = now.hour() as f64 + now.minute() as f64 / 60.0;
        Some(now_hour > avg + 2.0)
    }

    /// 当前是否深夜工作
    pub fn is_working_late(&self) -> bool {
        let hour = chrono::Local::now().hour();
        if hour >= 6 && hour < 22 {
            return false;
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let data = self.data.read();
        let today_apps = data.app_usage.get(&today);
        match today_apps {
            Some(apps) => apps.keys().any(|a| classify_app(a) == "work"),
            None => false,
        }
    }

    /// 生成时段 key（"workday_17" / "weekend_10"）
    fn slot_key_for(now: chrono::DateTime<chrono::Local>) -> String {
        let dow = now.weekday().num_days_from_monday();
        let bucket = if dow >= 5 { "weekend" } else { "workday" };
        format!("{}_{}", bucket, now.hour())
    }

    /// 记录当前时段的活动 label（由反思 LLM 写入的 current_activity 驱动）
    ///
    /// 节流：每 [`ACTIVITY_SLOT_SAMPLE_INTERVAL`] 秒最多记录一次，避免频繁写盘。
    /// label 为空或仅为空白时跳过。
    pub fn record_activity_slot(&self, label: &str, now_ts: f64) {
        let label = label.trim();
        if label.is_empty() {
            return;
        }
        {
            let mut last = self.last_slot_sample_ts.lock();
            if now_ts - *last < ACTIVITY_SLOT_SAMPLE_INTERVAL as f64 {
                return;
            }
            *last = now_ts;
        }
        let now = chrono::Local::now();
        let key = Self::slot_key_for(now);
        let mut data = self.data.write();
        let slot = data.activity_by_slot.entry(key).or_default();
        *slot.entry(label.to_string()).or_insert(0) += 1;
        drop(data);
        if let Err(e) = self.save() {
            tracing::warn!("[HabitTracker] 保存时段活动数据失败: {e}");
        }
        tracing::debug!("[HabitTracker] 记录时段活动: {label}");
    }

    /// 查询当前时段最常见的活动 label 及其占比
    ///
    /// 返回 `Some((label, ratio))` 当样本数 ≥ [`MIN_SLOT_SAMPLES`] 且
    /// 最常见 label 占比 ≥ [`HABIT_RATIO_THRESHOLD`] 时；否则返回 None。
    pub fn get_typical_activity_now(&self) -> Option<(String, f64)> {
        let now = chrono::Local::now();
        let key = Self::slot_key_for(now);
        let data = self.data.read();
        let slot = data.activity_by_slot.get(&key)?;
        let total: u32 = slot.values().sum();
        if (total as usize) < MIN_SLOT_SAMPLES {
            return None;
        }
        let (label, count) = slot
            .iter()
            .max_by_key(|(_, c)| *c)
            .map(|(l, c)| (l.clone(), *c))?;
        let ratio = count as f64 / total as f64;
        if ratio < HABIT_RATIO_THRESHOLD {
            return None;
        }
        Some((label, ratio))
    }

    /// 检测当前活动是否偏离历史时段习惯
    ///
    /// 当历史时段有明确习惯（最常见活动占比 ≥ 阈值），且当前活动 label
    /// 与历史最常见 label 不同时，返回偏离信息。
    pub fn detect_deviation_now(&self, current_label: &str) -> Option<HabitDeviation> {
        let current_label = current_label.trim();
        if current_label.is_empty() {
            return None;
        }
        let (typical_label, ratio) = self.get_typical_activity_now()?;
        if typical_label == current_label {
            return None;
        }
        Some(HabitDeviation {
            typical_label,
            current_label: current_label.to_string(),
            confidence: ratio,
        })
    }
}

/// 时段习惯偏离信息
#[derive(Debug, Clone)]
pub struct HabitDeviation {
    /// 历史此时段最常见的活动 label
    pub typical_label: String,
    /// 当前实际活动 label
    pub current_label: String,
    /// 历史习惯的置信度（最常见活动占比）
    pub confidence: f64,
}

impl Default for HabitTracker {
    fn default() -> Self {
        Self::new().unwrap_or_else(|e| {
            tracing::warn!("习惯追踪器初始化失败，使用内存模式: {e}");
            Self {
                data: RwLock::new(HabitData::default()),
                persistence_path: PathBuf::from("habits.json"),
                last_slot_sample_ts: parking_lot::Mutex::new(0.0),
            }
        })
    }
}

use chrono::{Datelike, Timelike};
