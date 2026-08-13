//! 日记系统 - 日记条目的持久化与管理 + 智能生成
//!
//! 模块结构：
//! - 顶层（本文件）：日记条目 CRUD、配置持久化、统计 — 与原 `diary.rs` 完全一致
//! - `intelligent_generator`：基于 LLM 的智能日记生成

pub mod intelligent_generator;
pub mod ongoing_stories;

use tauri::Emitter;
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

/// write_diary 工具可见性缓存（按 char_id 索引，由 proactive tick 每 10s 更新）
static DIARY_TOOL_AVAILABILITY: once_cell::sync::Lazy<parking_lot::RwLock<std::collections::HashMap<String, bool>>> =
    once_cell::sync::Lazy::new(|| parking_lot::RwLock::new(std::collections::HashMap::new()));

/// 更新 write_diary 工具可见性（由 spawn_auto_diary_check 内部调用）
pub fn set_tool_availability(char_id: &str, available: bool) {
    DIARY_TOOL_AVAILABILITY.write().insert(char_id.to_string(), available);
}

/// 查询 write_diary 工具当前是否对 LLM 可见（PromptBuildingStep 同步读取）
pub fn is_tool_available(char_id: &str) -> bool {
    DIARY_TOOL_AVAILABILITY.read().get(char_id).copied().unwrap_or(false)
}

/// 检查自动日记条件，满足时在后台异步生成日记并持久化。
/// 同时更新 write_diary 工具可见性缓存（供 PromptBuildingStep 同步读取）。
/// 生成成功后通过 Tauri 事件 `diary:written` 通知前端。
/// 调用方（proactive_tick）无需等待结果。
pub fn spawn_auto_diary_check(brain: &crate::brain::Brain, char_name: String, app: tauri::AppHandle) {
    let brain = brain.clone();
    let char_id = brain.char_id.clone();
    tokio::spawn(async move {
        let (should, reason) = should_trigger(&brain).await;
        // 更新工具可见性缓存（无论是否触发，都更新可见性）
        set_tool_availability(&char_id, should);
        if should {
            tracing::info!("[DiarySystem] 自动日记触发: {}", reason);
            match intelligent_generator::generate_intelligent_diary(&brain, "auto").await {
                Ok(entry) => {
                    tracing::info!(
                        "[DiarySystem] 自动日记生成并保存成功: id={}, date={}",
                        entry.id,
                        entry.date
                    );
                    let _ = app.emit(
                        "diary:written",
                        serde_json::json!({ "character_id": char_id, "character_name": char_name }),
                    );
                }
                Err(e) => {
                    tracing::error!("[DiarySystem] 自动日记生成失败: {e}");
                }
            }
        }
    });
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
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &path)?;
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

/// 判断是否应该触发生成日记
///
/// 条件（全部满足）：
/// - 自动日记已开启
/// - 今天尚未生成过日记
/// - 自上次日记（或初次启动）以来，微信渠道与直接渠道的直接对话消息
///   累计达 `min_interaction_threshold` 条（默认 20），素材不足时不触发
/// - 触发分数 ≥ 30（含 23:00 后时间兜底，详见 calculate_trigger_score）
pub async fn should_trigger(brain: &Brain) -> (bool, String) {
    let config = match get_config(&brain.char_id) {
        Ok(c) => c,
        Err(_) => return (false, "配置加载失败".to_string()),
    };
    if !config.enable_auto_diary {
        return (false, "自动日记未开启".to_string());
    }

    // 仅在 20:00-24:00 之间允许自动触发
    use chrono::Timelike;
    let hour = chrono::Local::now().hour();
    if hour < 20 {
        return (false, "未到日记时间（20:00-24:00）".to_string());
    }

    // 今天是否已生成过日记
    let today_str = chrono::Local::now().format("%Y-%m-%d").to_string();
    match get_entries(&brain.char_id, Some(&today_str)) {
        Ok(e) if !e.is_empty() => return (false, "今天已生成过日记".to_string()),
        Ok(_) => {}
        Err(_) => {}
    }

    // 素材收集起点：上次日记生成时间；无日记则从初次启动（0.0）开始
    let since = match get_latest_entry(&brain.char_id) {
        Ok(Some(entry)) => entry.created_at as f64,
        _ => 0.0,
    };

    // 收集自上次日记（或初次启动）以来的交互记录
    let interactions = intelligent_generator::collect_interactions_since(&brain.memory, since).await;

    // 仅统计微信渠道与直接渠道的直接对话消息作为日记素材
    let direct_dialogue_count = interactions
        .iter()
        .filter(|r| r.channel == "direct" || r.channel == "wechat")
        .count();
    if direct_dialogue_count < config.min_interaction_threshold {
        return (
            false,
            format!(
                "直接对话素材不足（{}/{}）",
                direct_dialogue_count,
                config.min_interaction_threshold
            ),
        );
    }

    // 计算触发分数（含情绪变化维度 + 20:00-24:00 时间兜底）
    let mood = intelligent_generator::collect_mood_summary(brain);
    let score = intelligent_generator::calculate_trigger_score(&interactions, Some(&mood));
    if score < 30 {
        return (false, format!("触发分数不足（{}/100）", score));
    }

    (true, format!("满足条件（分数: {}）", score))
}

/// 启动时检查遗漏的日记并触发后台补记
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

/// 逐日补记遗漏的日记
///
/// 从 start_date 次日逐日补记到 end_date，
/// 交互过少（<3）则跳过，避免过于频繁调用 LLM。
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
        // 避免过于频繁调用 LLM
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(())
}

/// 补记单日日记
async fn catch_up_one_day(brain: &Brain, date: chrono::NaiveDate) -> VivianResult<()> {
    use chrono::TimeZone;

    let ctx = intelligent_generator::collect_daily_context(brain, date).await;
    if ctx.interactions.len() < 3 {
        tracing::info!("[DiarySystem] {} 交互过少，跳过补记", date);
        return Ok(());
    }

    let trigger_score =
        intelligent_generator::calculate_trigger_score(&ctx.interactions, Some(&ctx.mood_summary));
    let fallback_mood_tag = intelligent_generator::get_mood_tag(&ctx.mood_summary);

    let diary_result = intelligent_generator::generate_content_via_llm(brain, &ctx).await?;

    let mood_tag = diary_result
        .mood_tag
        .as_deref()
        .map(|t| {
            const VALID: &[&str] = &["happy", "good", "neutral", "sad", "angry", "tired"];
            if VALID.contains(&t) { t.to_string() } else { "neutral".to_string() }
        })
        .unwrap_or(fallback_mood_tag);

    let key_events: Vec<String> = if let Some(ref sk) = diary_result.structured_keywords {
        let mut flat: Vec<String> = Vec::new();
        flat.extend(sk.events.iter().cloned());
        flat.extend(sk.themes.iter().cloned());
        flat.truncate(5);
        flat
    } else {
        diary_result.keywords.clone()
    };

    let date_str = date.format("%Y-%m-%d").to_string();
    let start_of_day = date.and_hms_opt(0, 0, 0).unwrap_or_else(|| {
        date.and_hms_opt(0, 0, 1).unwrap()
    });
    let start_ts = chrono::Local
        .from_local_datetime(&start_of_day)
        .single()
        .map(|dt| dt.timestamp())
        .unwrap_or(0);
    let end_ts = start_ts + 24 * 3600;

    let entry = DiaryEntry {
        id: String::new(),
        date: date_str.clone(),
        start_time: start_ts,
        end_time: end_ts,
        content: diary_result.content.clone(),
        key_events,
        mood_average: ctx.mood_summary.clone(),
        word_count: diary_result.content.chars().count(),
        interaction_count: ctx.interactions.len(),
        trigger_type: "catch_up".to_string(),
        trigger_score,
        mood_tag,
        created_at: chrono::Local::now().timestamp(),
        structured_keywords: diary_result.structured_keywords.clone(),
        story_update: diary_result.story_update.clone(),
        relationship_delta: ctx.relationship_delta.clone(),
        mood_samples: ctx.mood_samples.clone(),
        version: 2,
    };
    add_entry(&brain.char_id, entry)?;

    if let Some(ref update) = diary_result.story_update {
        let _ = ongoing_stories::update_ongoing_stories(&brain.char_id, update, &date_str);
    }

    tracing::info!("[DiarySystem] 补记成功: {}", date);
    Ok(())
}

/// 导出所有日记为 Markdown 文件
///
/// 每篇日记用 `## 📅 日期 emoji`，统计信息用列表，
/// 关键事件用列表，正文随后。
pub fn export_to_markdown(char_id: &str, file_path: &str) -> VivianResult<bool> {
    let entries = get_entries(char_id, None)?;
    let mut md = String::from("# 薇薇安的日记\n\n---\n\n");

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
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &path)?;
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
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &path)?;
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
