//! 日记命令 - 日记条目查询与生成（按角色 char_id 路由）

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{Emitter, State};

use crate::diary;
use crate::diary::intelligent_generator;
use crate::state::AppState;

#[tauri::command]
pub async fn get_diary_entries(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    date_filter: Option<String>,
) -> Result<Vec<Value>, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let char_id = instance.id.clone();
    // diary::get_entries 内部做同步文件 I/O + JSON 反序列化，移入阻塞线程池避免卡住 async 运行时
    let entries = tokio::task::spawn_blocking(move || {
        diary::get_entries(&char_id, date_filter.as_deref())
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))?
    .map_err(|e| e.to_string())?;
    let values: Vec<Value> = entries
        .into_iter()
        .map(|e| serde_json::to_value(e).unwrap_or(json!({})))
        .collect();
    Ok(values)
}

/// 按时间窗口获取日记条目（图谱懒加载内容层）
///
/// 返回 `created_at` 落在 `[after, before)` 区间内的日记条目，按时间升序。
#[tauri::command]
pub async fn get_diary_range(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    after: i64,
    before: i64,
) -> Result<Vec<Value>, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let char_id = instance.id.clone();
    let entries = tokio::task::spawn_blocking(move || {
        diary::entries_in_range(&char_id, after, before)
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))?
    .map_err(|e| e.to_string())?;
    let values: Vec<Value> = entries
        .into_iter()
        .map(|e| serde_json::to_value(e).unwrap_or(json!({})))
        .collect();
    Ok(values)
}

#[tauri::command]
pub async fn generate_diary(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let start_of_day = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let start_ts = chrono::DateTime::<chrono::Local>::from_naive_utc_and_offset(
        start_of_day,
        *chrono::Local::now().offset(),
    ).timestamp();
    let end_ts = now.timestamp();

    let entry = diary::DiaryEntry {
        id: String::new(),
        date,
        start_time: start_ts,
        end_time: end_ts,
        content: "今日暂无足够的互动数据来生成日记。".to_string(),
        key_events: Vec::new(),
        mood_average: json!({"pet_valence": 0.0, "pet_energy": 50}),
        word_count: 0,
        interaction_count: 0,
        trigger_type: "manual".to_string(),
        trigger_score: 0,
        mood_tag: "neutral".to_string(),
        created_at: end_ts,
        structured_keywords: None,
        story_update: None,
        relationship_delta: None,
        mood_samples: Vec::new(),
        version: 2,
    };

    let saved = diary::add_entry(&instance.id, entry).map_err(|e| e.to_string())?;
    serde_json::to_value(saved).map_err(|e| e.to_string())
}

/// 智能日记生成 — 调用 LLM 基于当日对话历史与情绪状态生成
///
/// 需要 Brain 已初始化（包含 ModelRouter + MemoryManager + Relationship）。
#[tauri::command]
pub async fn generate_diary_intelligent(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    trigger_type: Option<String>,
    app: tauri::AppHandle,
) -> Result<Value, String> {
    // 必须在 await 之前释放，Brain 内部字段全部为 Arc，clone 开销极低
    let instance = state.get_character(character_id.as_deref())?;
    let brain = instance.brain.clone();
    let char_id = instance.id.clone();

    // 主 LLM API 必须配置，否则发 `llm:not_configured` 通知用户
    let api_configured = state
        .model_router
        .read()
        .as_ref()
        .map_or(false, |r| r.has_main_provider());
    if !api_configured {
        let _ = app.emit(
            "llm:not_configured",
            json!({ "scene": "diary", "character_id": char_id }),
        );
        return Err("MAIN_API_NOT_CONFIGURED".to_string());
    }

    let trigger = trigger_type.as_deref().unwrap_or("manual");
    let entry = intelligent_generator::generate_intelligent_diary(&brain, trigger)
        .await
        .map_err(|e| e.to_string())?;

    let _ = app.emit("diary:written", json!({ "character_id": char_id, "character_name": instance.name }));
    serde_json::to_value(entry).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_diary_entry(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    entry_id: String,
) -> Result<Option<Value>, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let entry = diary::get_entry(&instance.id, &entry_id).map_err(|e| e.to_string())?;
    Ok(entry.map(|e| serde_json::to_value(e).unwrap_or(json!({}))))
}

#[tauri::command]
pub async fn delete_diary_entry(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    entry_id: String,
) -> Result<(), String> {
    let instance = state.get_character(character_id.as_deref())?;
    diary::delete_entry(&instance.id, &entry_id).map_err(|e| e.to_string())?;

    // 同步删除记忆系统中的日记索引
    let memory = instance.brain.memory.clone();
    let _ = memory.delete_diary_memory(&entry_id).await;

    Ok(())
}

#[tauri::command]
pub async fn get_diary_config(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let config = diary::get_config(&instance.id).map_err(|e| e.to_string())?;
    serde_json::to_value(config).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_diary_config(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    enable_auto_diary: Option<bool>,
    min_interaction_threshold: Option<usize>,
    max_diary_length: Option<usize>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let config = diary::set_config(
        &instance.id,
        enable_auto_diary,
        min_interaction_threshold,
        max_diary_length,
    ).map_err(|e| e.to_string())?;
    serde_json::to_value(config).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_diary_stats(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    diary::get_stats(&instance.id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_diary_entry(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    entry_id: String,
    content: String,
) -> Result<(), String> {
    let instance = state.get_character(character_id.as_deref())?;
    diary::update_entry(&instance.id, &entry_id, &content).map_err(|e| e.to_string())?;

    // 同步更新记忆系统中的日记索引：先删旧条目，再用新内容重建
    let memory = instance.brain.memory.clone();
    let _ = memory.delete_diary_memory(&entry_id).await;
    // 获取更新后的日记条目，重建索引
    if let Ok(Some(entry)) = diary::get_entry(&instance.id, &entry_id) {
        let _ = memory
            .add_diary_entry(&entry.id, &entry.date, &entry.content, &entry.mood_tag)
            .await;
    }

    Ok(())
}

/// 检查并补记遗漏的日记
///
/// 在启动时调用，检测日记断层并触发后台异步补记。需要 Brain 已初始化。
#[tauri::command]
pub async fn check_missed_diaries(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let instance = state.get_character(character_id.as_deref())?;
    let brain = instance.brain.clone();
    diary::check_missed_diaries_on_startup(&brain)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 校验导出路径的安全性
///
/// 拒绝路径穿越、系统敏感目录、非 .md 扩展名以及非绝对路径。
/// 建议前端传入用户文档目录下的绝对路径。
fn validate_export_path(file_path: &str) -> Result<(), String> {
    use std::path::Path;

    let p = Path::new(file_path);

    // 必须是 .md 扩展名
    if p.extension().and_then(|e| e.to_str()) != Some("md") {
        return Err("导出文件必须是 .md 扩展名".to_string());
    }

    // 拒绝路径穿越
    if file_path.contains("..") {
        return Err("路径不安全：包含目录穿越序列".to_string());
    }

    // 路径必须是绝对路径（相对路径依赖当前工作目录，存在不确定性）
    if !p.is_absolute() {
        return Err(format!(
            "导出路径必须是绝对路径（建议放在用户文档目录下）: {}",
            file_path
        ));
    }

    // 拒绝写入系统敏感目录
    let lower = file_path.to_lowercase();
    let normalized = lower.replace('/', "\\");
    const FORBIDDEN_PREFIXES: &[&str] = &[
        "c:\\windows",
        "c:\\program files",
        "c:\\program files (x86)",
        "c:\\system volume information",
        "c:\\$recycle.bin",
    ];
    for prefix in FORBIDDEN_PREFIXES {
        if normalized.starts_with(prefix) {
            return Err(format!("拒绝写入系统敏感目录: {}", file_path));
        }
    }

    Ok(())
}

/// 导出指定角色所有日记为 Markdown 文件
///
/// 返回导出文件路径。
#[tauri::command]
pub async fn export_diaries_markdown(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    file_path: String,
) -> Result<String, String> {
    validate_export_path(&file_path)?;
    let instance = state.get_character(character_id.as_deref())?;
    diary::export_to_markdown(&instance.id, &file_path).map_err(|e| e.to_string())?;
    Ok(file_path)
}

/// 判断是否应该自动触发日记生成
///
/// 返回 `{ should_trigger, reason }`。
#[tauri::command]
pub async fn should_trigger_diary(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let brain = instance.brain.clone();
    let (trigger, reason) = diary::should_trigger(&brain).await;
    Ok(json!({ "should_trigger": trigger, "reason": reason }))
}

/// 获取所有在线角色的日记（综合视图）
///
/// 合并所有在线角色的日记，每条标注 `character_id` 和 `character_name`，
/// 按日期倒序排列。
#[tauri::command]
pub async fn get_diary_entries_all(
    state: State<'_, Arc<AppState>>,
    date_filter: Option<String>,
) -> Result<Vec<Value>, String> {
    // 先在锁内快照在线角色信息（id, name），避免持锁期间做磁盘 I/O
    let chars_snapshot: Vec<(String, String)> = {
        let characters = state.characters.read();
        characters
            .values()
            .filter(|c| *c.online.read())
            .map(|c| (c.id.clone(), c.name.clone()))
            .collect()
    };
    // 所有角色的日记读取 + JSON 反序列化 + 序列化 + 排序均在阻塞线程完成
    let all_entries = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        let mut all_entries: Vec<Value> = Vec::new();
        for (cid, cname) in chars_snapshot {
            let entries =
                diary::get_entries(&cid, date_filter.as_deref()).map_err(|e| e.to_string())?;
            for entry in entries {
                let mut v = serde_json::to_value(&entry).unwrap_or(json!({}));
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("character_id".to_string(), json!(cid));
                    obj.insert("character_name".to_string(), json!(cname));
                }
                all_entries.push(v);
            }
        }
        all_entries.sort_by(|a, b| {
            let da = a.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let db = b.get("date").and_then(|v| v.as_str()).unwrap_or("");
            db.cmp(da)
        });
        Ok(all_entries)
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))??;
    Ok(all_entries)
}

/// 获取共同日记
#[tauri::command]
pub async fn get_common_diary_entries(
    date_filter: Option<String>,
) -> Result<Vec<Value>, String> {
    let entries = diary::get_common_entries(date_filter.as_deref()).map_err(|e| e.to_string())?;
    let values: Vec<Value> = entries
        .into_iter()
        .map(|e| serde_json::to_value(e).unwrap_or(json!({})))
        .collect();
    Ok(values)
}

/// 添加共同日记
#[tauri::command]
pub async fn add_common_diary_entry(
    content: String,
    mood_tag: Option<String>,
) -> Result<Value, String> {
    let now = chrono::Local::now();
    let entry = diary::DiaryEntry {
        id: String::new(),
        date: now.format("%Y-%m-%d").to_string(),
        start_time: now.timestamp(),
        end_time: now.timestamp(),
        content,
        key_events: Vec::new(),
        mood_average: json!({"pet_valence": 0.0, "pet_energy": 50}),
        word_count: 0,
        interaction_count: 0,
        trigger_type: "manual".to_string(),
        trigger_score: 0,
        mood_tag: mood_tag.unwrap_or_else(|| "neutral".to_string()),
        created_at: now.timestamp(),
        structured_keywords: None,
        story_update: None,
        relationship_delta: None,
        mood_samples: Vec::new(),
        version: 2,
    };
    let saved = diary::add_common_entry(entry).map_err(|e| e.to_string())?;
    serde_json::to_value(&saved).map_err(|e| e.to_string())
}

/// 删除共同日记
#[tauri::command]
pub async fn delete_common_diary_entry(entry_id: String) -> Result<(), String> {
    diary::delete_common_entry(&entry_id).map_err(|e| e.to_string())
}

/// 清空共同日记
#[tauri::command]
pub async fn clear_common_diary_entries() -> Result<(), String> {
    diary::clear_common_entries().map_err(|e| e.to_string())
}
