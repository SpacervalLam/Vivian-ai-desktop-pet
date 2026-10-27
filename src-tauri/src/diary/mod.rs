//! 日记系统 - 日记条目的持久化与管理 + 智能生成
//!
//! 模块结构：
//! - 顶层（本文件）：日记条目 CRUD、配置持久化、统计 — 与原 `diary.rs` 完全一致
//! - `intelligent_generator`：基于 LLM 的智能日记生成

pub mod growth;
pub mod cycle;
pub mod dream;
pub mod intelligent_generator;
pub mod ongoing_stories;

use crate::brain::Brain;
use crate::error::{VivianError, VivianResult};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiaryEntry {
    pub id: String,
    pub date: String,
    pub start_time: i64,
    pub end_time: i64,
    pub content: String,
    pub key_events: Vec<String>,
    pub mood_average: serde_json::Value,
    pub word_count: usize,
    pub interaction_count: usize,
    pub trigger_type: String,
    pub trigger_score: u32,
    pub mood_tag: String,
    pub created_at: i64,
    #[serde(default)]
    pub structured_keywords: Option<StructuredKeywords>,
    #[serde(default)]
    pub story_update: Option<StoryUpdate>,
    #[serde(default)]
    pub relationship_delta: Option<RelationshipDelta>,
    #[serde(default)]
    pub mood_samples: Vec<MoodSample>,
    #[serde(default = "default_diary_version")]
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dream: Option<cycle::DreamContext>,
}

fn default_diary_version() -> u8 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredKeywords {
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub emotions: Vec<String>,
    #[serde(default)]
    pub people: Vec<String>,
    #[serde(default)]
    pub themes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryUpdate {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipDelta {
    pub date: String,
    pub intimacy_before: f64,
    pub intimacy_after: f64,
    pub trust_before: f64,
    pub trust_after: f64,
    #[serde(default)]
    pub signal_summary: String,
    #[serde(default)]
    pub highlight: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoodSample {
    pub period: String,
    pub dominant_emotion: String,
    #[serde(default)]
    pub valence: f64,
    #[serde(default)]
    pub arousal: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiaryConfig {
    pub enable_auto_diary: bool,
    pub min_interaction_threshold: usize,
    pub max_diary_length: usize,
}

impl Default for DiaryConfig {
    fn default() -> Self {
        Self {
            enable_auto_diary: true,
            min_interaction_threshold: 20,
            max_diary_length: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiaryFile {
    entries: Vec<DiaryEntry>,
    saved_at: f64,
}

impl Default for DiaryFile {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            saved_at: 0.0,
        }
    }
}

pub fn diary_dir(char_id: &str) -> std::path::PathBuf {
    let dir = crate::utils::path::get_character_data_dir(char_id).join("diary");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn diaries_file(char_id: &str) -> std::path::PathBuf {
    diary_dir(char_id).join("diaries.json")
}

fn config_file(char_id: &str) -> std::path::PathBuf {
    diary_dir(char_id).join("config.json")
}

pub fn get_entries(char_id: &str, date_filter: Option<&str>) -> VivianResult<Vec<DiaryEntry>> {
    let path = diaries_file(char_id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file_data: DiaryFile = serde_json::from_str(&content)
        .map_err(|e| VivianError::Serialization(e.to_string()))?;

    let mut entries = file_data.entries;
    let mut seen_ids = std::collections::HashSet::new();
    entries.retain(|e| seen_ids.insert(e.id.clone()));
    if let Some(date) = date_filter {
        entries.retain(|e| e.date == date);
    }
    entries.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(entries)
}

/// 时间窗口 [after, before) 内的日记（Unix 秒），供图谱懒加载。
///
/// 复用 `get_entries` 的去重逻辑，按 `created_at` 升序返回（与图谱时间轴方向一致）。
pub fn entries_in_range(char_id: &str, after: i64, before: i64) -> VivianResult<Vec<DiaryEntry>> {
    let mut entries = get_entries(char_id, None)?;
    entries.retain(|e| e.created_at >= after && e.created_at < before);
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(entries)
}

pub fn get_entry(char_id: &str, entry_id: &str) -> VivianResult<Option<DiaryEntry>> {
    let entries = get_entries(char_id, None)?;
    Ok(entries.into_iter().find(|e| e.id == entry_id))
}

pub fn get_latest_entry(char_id: &str) -> VivianResult<Option<DiaryEntry>> {
    let entries = get_entries(char_id, None)?;
    Ok(entries.into_iter().next())
}

pub fn delete_entry(char_id: &str, entry_id: &str) -> VivianResult<()> {
    let path = diaries_file(char_id);
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path)?;
    let mut file_data: DiaryFile = if content.trim().is_empty() {
        DiaryFile::default()
    } else {
        serde_json::from_str(&content).map_err(|e| VivianError::Serialization(e.to_string()))?
    };

    file_data.entries.retain(|e| e.id != entry_id);
    file_data.saved_at = current_timestamp();
    save_diary_file(char_id, &file_data)?;
    Ok(())
}

pub fn clear_all_entries(char_id: &str) -> VivianResult<()> {
    let file_data = DiaryFile::default();
    save_diary_file(char_id, &file_data)?;
    Ok(())
}

pub fn update_entry(char_id: &str, entry_id: &str, content: &str) -> VivianResult<()> {
    let path = diaries_file(char_id);
    if !path.exists() {
        return Ok(());
    }
    let content_str = fs::read_to_string(&path)?;
    let mut file_data: DiaryFile = if content_str.trim().is_empty() {
        DiaryFile::default()
    } else {
        serde_json::from_str(&content_str).map_err(|e| VivianError::Serialization(e.to_string()))?
    };

    if let Some(entry) = file_data.entries.iter_mut().find(|e| e.id == entry_id) {
        entry.content = content.to_string();
        entry.word_count = content.chars().count();
    }
    file_data.saved_at = current_timestamp();
    save_diary_file(char_id, &file_data)?;
    Ok(())
}

pub fn add_entry(char_id: &str, mut entry: DiaryEntry) -> VivianResult<DiaryEntry> {
    let path = diaries_file(char_id);
    let mut file_data = if path.exists() {
        let content = fs::read_to_string(&path)?;
        if content.trim().is_empty() {
            DiaryFile::default()
        } else {
            serde_json::from_str(&content).map_err(|e| VivianError::Serialization(e.to_string()))?
        }
    } else {
        DiaryFile::default()
    };

    if entry.id.is_empty() {
        entry.id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    }
    entry.created_at = chrono::Local::now().timestamp();

    // 同一天已存在则替换，避免重复落盘
    if let Some(existing) = file_data.entries.iter_mut().find(|e| e.date == entry.date) {
        *existing = entry.clone();
    } else {
        file_data.entries.push(entry.clone());
    }
    file_data.saved_at = current_timestamp();
    save_diary_file(char_id, &file_data)?;

    Ok(entry)
}

pub fn get_config(char_id: &str) -> VivianResult<DiaryConfig> {
    let path = config_file(char_id);
    if !path.exists() {
        let config = DiaryConfig::default();
        let json = serde_json::to_string_pretty(&config)
            .map_err(|e| VivianError::Serialization(e.to_string()))?;
        fs::write(&path, json)?;
        return Ok(config);
    }
    let content = fs::read_to_string(&path)?;
    if content.trim().is_empty() {
        return Ok(DiaryConfig::default());
    }
    serde_json::from_str(&content).map_err(|e| VivianError::Serialization(e.to_string()))
}

pub fn save_config(char_id: &str, config: &DiaryConfig) -> VivianResult<()> {
    let path = config_file(char_id);
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| VivianError::Serialization(e.to_string()))?;
    fs::write(&path, json)?;
    Ok(())
}

pub fn set_config(char_id: &str, enable_auto_diary: Option<bool>, min_interaction_threshold: Option<usize>, max_diary_length: Option<usize>) -> VivianResult<DiaryConfig> {
    let mut config = get_config(char_id)?;
    if let Some(v) = enable_auto_diary {
        config.enable_auto_diary = v;
    }
    if let Some(v) = min_interaction_threshold {
        config.min_interaction_threshold = v;
    }
    if let Some(v) = max_diary_length {
        config.max_diary_length = v;
    }
    save_config(char_id, &config)?;
    Ok(config)
}

/// 安排请求工具的可见性缓存；由环境智能体的程序资格检查更新。
static DIARY_TOOL_AVAILABILITY: once_cell::sync::Lazy<parking_lot::RwLock<std::collections::HashMap<String, bool>>> =
    once_cell::sync::Lazy::new(|| parking_lot::RwLock::new(std::collections::HashMap::new()));

/// 更新完整做梦与日记流程的工具可见性。
pub fn set_tool_availability(char_id: &str, available: bool) {
    DIARY_TOOL_AVAILABILITY.write().insert(char_id.to_string(), available);
}

/// 查询 write_diary 工具当前是否对 LLM 可见（PromptBuildingStep 同步读取）
pub fn is_tool_available(char_id: &str) -> bool {
    DIARY_TOOL_AVAILABILITY.read().get(char_id).copied().unwrap_or(false)
}

pub fn get_stats(char_id: &str) -> VivianResult<serde_json::Value> {
    let entries = get_entries(char_id, None)?;
    let total = entries.len();
    let total_words: usize = entries.iter().map(|e| e.word_count).sum();
    let total_interactions: usize = entries.iter().map(|e| e.interaction_count).sum();
    let first_date = entries.last().map(|e| e.date.clone()).unwrap_or_default();
    let last_date = entries.first().map(|e| e.date.clone()).unwrap_or_default();
    let avg_words = if total > 0 { total_words / total } else { 0 };

    Ok(serde_json::json!({
        "total_entries": total,
        "first_date": first_date,
        "last_date": last_date,
        "average_word_count": avg_words,
        "total_interactions": total_interactions,
    }))
}

fn save_diary_file(char_id: &str, file_data: &DiaryFile) -> VivianResult<()> {
    let path = diaries_file(char_id);
    let json = serde_json::to_string_pretty(file_data)
        .map_err(|e| VivianError::Serialization(e.to_string()))?;
    crate::utils::fs::write_atomic(&path, &json)?;
    Ok(())
}

fn current_timestamp() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ============================================================================
// 自动触发 / 启动补记 / Markdown 导出
// ============================================================================

/// 检查做梦与日记的候选资格；是否启动由环境智能体规划，不能直接用此结果触发写入。
///
/// 条件（全部满足）：
/// - 自动日记已开启
/// - 今天尚未生成过日记
/// - 目标日期内微信渠道与直接渠道的原始对话消息
///   累计达 `min_interaction_threshold` 条（默认 20），素材不足时不触发
/// - 当前位于本地时间 20:00–24:00；满足条件仅提供候选，不自动启动。
pub async fn should_trigger(brain: &Brain) -> (bool, String) {
    match dream::candidate(brain, chrono::Local::now()) {
        Ok(Some(candidate)) => (true,candidate.reason),
        Ok(None) => (false,"素材量、时间窗、开关或进度条件不满足，不送入环境智能体判断".into()),
        Err(error) => (false,error.to_string()),
    }
}

/// 检查遗漏的日记并提交补记候选，等待环境智能体安排。
///
/// 检测从上次日记日期到昨天
/// 的断层，异步补记遗漏日记。
pub async fn check_missed_diaries_on_startup(brain: &Brain) -> VivianResult<()> {
    let last_entry = match get_latest_entry(&brain.char_id)? {
        Some(e) => e,
        None => return Ok(()),
    };

    let last_date = match chrono::NaiveDate::parse_from_str(&last_entry.date, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };
    let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);

    if last_date < yesterday {
        tracing::warn!(
            "[DiarySystem] 检测到日记断层！最后记录日期为 {}，启动后台异步补记任务...",
            last_date
        );
        let brain = brain.clone();
        tokio::spawn(async move {
            if let Err(e) = catch_up_missed_diaries(&brain, last_date, yesterday).await {
                tracing::error!("[DiarySystem] 补记失败: {e}");
            }
        });
    }
    Ok(())
}

/// 逐日提交遗漏日记的候选请求；不直接触发生成。
///
/// 从 start_date 次日逐日补记到 end_date，
/// 原始对话不足配置阈值时跳过，仍由程序时间窗及环境智能体决定是否执行。
pub async fn catch_up_missed_diaries(
    brain: &Brain,
    start_date: chrono::NaiveDate,
    end_date: chrono::NaiveDate,
) -> VivianResult<()> {
    let mut current = start_date + chrono::Duration::days(1);
    while current <= end_date {
        tracing::info!("[DiarySystem] 补记 {} 的日记", current.format("%Y-%m-%d"));
        if let Err(e) = catch_up_one_day(brain, current).await {
            tracing::error!("[DiarySystem] 补记 {} 失败: {e}", current);
        }
        current += chrono::Duration::days(1);
        // 批量扫描让出运行时，实际模型调用由环境智能体另行安排。
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(())
}

/// 补记单日日记
async fn catch_up_one_day(brain: &Brain, date: chrono::NaiveDate) -> VivianResult<()> {
    if !get_entries(&brain.char_id, Some(&date.to_string()))?.is_empty() { return Ok(()); }
    let ctx = intelligent_generator::collect_daily_context(brain, date).await;
    let threshold = get_config(&brain.char_id)?.min_interaction_threshold.max(1);
    if ctx.interactions.iter().filter(|record| record.channel == "direct" || record.channel == "wechat").count() < threshold {
        tracing::info!("[DreamDiary] {} 交互过少，跳过补记", date);
        return Ok(());
    }
    cycle::cycle_for(&brain.char_id).request(date)?;
    tracing::info!("[DreamDiary] 补记安排已提交，等待环境智能体决定: {}", date);
    Ok(())
}

/// 导出所有日记为 Markdown 文件
///
/// 每篇日记用 `## 📅 日期 emoji`，统计信息用列表，
/// 关键事件用列表，正文随后。
pub fn export_to_markdown(char_id: &str, file_path: &str) -> VivianResult<bool> {
    let entries = get_entries(char_id, None)?;
    let mut md = String::from("# Vivian的日记\n\n---\n\n");

    for entry in &entries {
        let mood_emoji = match entry.mood_tag.as_str() {
            "happy" => "☀️",
            "good" => "😊",
            "neutral" => "😐",
            "sad" => "😢",
            "angry" => "😠",
            "bored" => "😴",
            "tired" => "😪",
            _ => "📝",
        };

        md.push_str(&format!("## 📅 {} {}\n\n", entry.date, mood_emoji));
        md.push_str(&format!("- **触发方式**: {}\n", entry.trigger_type));
        md.push_str(&format!("- **互动次数**: {}\n", entry.interaction_count));
        let created = chrono::DateTime::<chrono::Utc>::from_timestamp(entry.created_at, 0)
            .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        md.push_str(&format!("- **生成时间**: {}\n\n", created));

        if !entry.key_events.is_empty() {
            md.push_str("**今日要事**:\n");
            for ev in &entry.key_events {
                md.push_str(&format!("- {}\n", ev));
            }
            md.push('\n');
        }

        md.push_str(&format!("{}\n\n---\n\n", entry.content));
    }

    std::fs::write(file_path, md)?;
    tracing::info!("[DiarySystem] 日记已导出到: {}", file_path);
    Ok(true)
}

// ============ 共同日记（两个角色共享的日记） ============

fn common_diaries_file() -> std::path::PathBuf {
    crate::utils::path::get_common_diary_dir().join("diaries.json")
}

/// 读取共同日记
pub fn get_common_entries(date_filter: Option<&str>) -> VivianResult<Vec<DiaryEntry>> {
    let path = common_diaries_file();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file_data: DiaryFile = serde_json::from_str(&content)
        .map_err(|e| VivianError::Serialization(e.to_string()))?;
    let mut entries = file_data.entries;
    let mut seen_ids = std::collections::HashSet::new();
    entries.retain(|e| seen_ids.insert(e.id.clone()));
    if let Some(date) = date_filter {
        entries.retain(|e| e.date == date);
    }
    entries.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(entries)
}

/// 添加共同日记
pub fn add_common_entry(mut entry: DiaryEntry) -> VivianResult<DiaryEntry> {
    if entry.id.is_empty() {
        entry.id = format!("common-{}", chrono::Local::now().timestamp_millis());
    }
    let path = common_diaries_file();
    let _ = fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
    let mut file_data: DiaryFile = if path.exists() {
        let content = fs::read_to_string(&path)?;
        if content.trim().is_empty() {
            DiaryFile::default()
        } else {
            serde_json::from_str(&content).map_err(|e| VivianError::Serialization(e.to_string()))?
        }
    } else {
        DiaryFile::default()
    };
    file_data.entries.push(entry.clone());
    file_data.saved_at = current_timestamp();
    let json = serde_json::to_string_pretty(&file_data)
        .map_err(|e| VivianError::Serialization(e.to_string()))?;
    crate::utils::fs::write_atomic(&path, &json)?;
    Ok(entry)
}

/// 删除共同日记
pub fn delete_common_entry(entry_id: &str) -> VivianResult<()> {
    let path = common_diaries_file();
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path)?;
    let mut file_data: DiaryFile = if content.trim().is_empty() {
        DiaryFile::default()
    } else {
        serde_json::from_str(&content).map_err(|e| VivianError::Serialization(e.to_string()))?
    };
    file_data.entries.retain(|e| e.id != entry_id);
    file_data.saved_at = current_timestamp();
    let json = serde_json::to_string_pretty(&file_data)
        .map_err(|e| VivianError::Serialization(e.to_string()))?;
    crate::utils::fs::write_atomic(&path, &json)?;
    Ok(())
}

/// 清空共同日记
pub fn clear_common_entries() -> VivianResult<()> {
    let path = common_diaries_file();
    let file_data = DiaryFile::default();
    let json = serde_json::to_string_pretty(&file_data)
        .map_err(|e| VivianError::Serialization(e.to_string()))?;
    let _ = fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
    fs::write(&path, &json)?;
    Ok(())
}
