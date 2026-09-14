//! 记忆服务 - MemoryManager 的业务方法封装
//!
//! 提供记忆读写、偏好管理、日记记录、互动查询、上下文总结等业务方法。
//! 所有方法接收 `&MemoryManager` 参数，由调用方按角色 ID 从 character_registry 获取，
//! 不再持有全局单例，确保多角色记忆隔离。

use serde_json::{json, Value};

use crate::diary::{self, DiaryEntry};
use crate::memory::{MemoryItem, MemoryManager, MemoryType};

/// 记忆服务：无状态的业务方法集合
pub struct MemoryService;

impl MemoryService {
    /// 写入记忆（通用）
    pub async fn write_memory(
        mgr: &MemoryManager,
        content: &str,
        memory_type: &str,
        importance: f64,
        tags: Vec<String>,
        metadata: Value,
    ) -> Result<Value, String> {
        let mt = parse_memory_type_str(memory_type);
        // 把 memory_type 作为标签，便于后续按类型过滤
        let mut all_tags = tags;
        if !all_tags.iter().any(|t| t == memory_type) {
            all_tags.push(memory_type.to_string());
        }
        // 标注来源：工具调用由 LLM 替角色写入，speaker/listener 都是角色自身
        let mut meta_with_source = metadata;
        if let Some(obj) = meta_with_source.as_object_mut() {
            obj.entry("channel").or_insert(json!("inner"));
            obj.entry("speaker").or_insert(json!(mgr.char_id()));
            obj.entry("listener").or_insert(json!(mgr.char_id()));
            obj.entry("perspective").or_insert(json!("speaker"));
        }
        let item = mgr
            .add_memory_with_metadata(content, mt, importance, all_tags.clone(), meta_with_source)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "id": item.id,
            "content": item.content,
            "memory_type": memory_type,
            "importance": item.importance,
            "tags": item.tags,
            "metadata": item.metadata,
            "created_at": item.timestamp,
        }))
    }

    /// 读取记忆（按 ID 或类型筛选，按时间倒序返回）
    pub async fn read_memory(
        mgr: &MemoryManager,
        memory_id: Option<&str>,
        memory_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, String> {
        let all = mgr.get_all_memories().await.map_err(|e| e.to_string())?;

        let mut filtered: Vec<Value> = if let Some(id) = memory_id {
            all.into_iter()
                .filter(|m| m.id == id)
                .map(|m| memory_item_to_value(&m))
                .collect()
        } else if let Some(mt) = memory_type {
            all.into_iter()
                .filter(|m| m.tags.iter().any(|t| t == mt) || m.granularity == mt)
                .map(|m| memory_item_to_value(&m))
                .collect()
        } else {
            all.into_iter()
                .map(|m| memory_item_to_value(&m))
                .collect()
        };

        // 按时间倒序
        filtered.sort_by(|a, b| {
            let ta = a.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tb = b.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
            tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
        });
        filtered.truncate(limit);
        Ok(filtered)
    }

    /// 删除单条记忆，返回被删除内容
    pub async fn delete_memory(
        mgr: &MemoryManager,
        memory_id: &str,
    ) -> Result<Value, String> {
        let all = mgr.get_all_memories().await.map_err(|e| e.to_string())?;
        let item = all
            .into_iter()
            .find(|m| m.id == memory_id)
            .ok_or_else(|| format!("记忆不存在: {}", memory_id))?;
        let content = item.content.clone();
        mgr.delete_memory(memory_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "memory_id": memory_id,
            "content": content,
            "deleted_at": current_timestamp(),
        }))
    }

    /// 更新用户偏好（写入 Preference 类型记忆）
    pub async fn update_user_preference(
        mgr: &MemoryManager,
        key: &str,
        value: &Value,
        description: Option<&str>,
    ) -> Result<Value, String> {
        let content = format!("偏好 {}: {}", key, value);
        let tags = vec![
            "preference".to_string(),
            format!("pref-{}", key),
            "long_term_preference".to_string(),
        ];
        let mut metadata = json!({
            "preference_key": key,
            "preference_value": value,
            "channel": "inner",
            "speaker": mgr.char_id(),
            "listener": mgr.char_id(),
            "perspective": "speaker",
        });
        if let Some(desc) = description {
            metadata["description"] = json!(desc);
        }
        let item = mgr
            .add_memory_with_metadata(&content, MemoryType::Preference, 0.8, tags, metadata)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "preference_key": key,
            "preference_value": value,
            "description": description,
            "memory_id": item.id,
            "updated_at": item.timestamp,
        }))
    }

    /// 记录日记（同步写入记忆系统）
    pub async fn log_daily_diary(
        mgr: &MemoryManager,
        char_id: &str,
        content: &str,
        mood: Option<&str>,
        highlights: &[String],
        tags: &[String],
    ) -> Result<Value, String> {
        let now = chrono::Local::now();
        let date = now.format("%Y-%m-%d").to_string();
        let ts = now.timestamp();

        let entry = DiaryEntry {
            id: String::new(),
            date: date.clone(),
            start_time: ts,
            end_time: ts,
            content: content.to_string(),
            key_events: highlights.to_vec(),
            mood_average: json!(mood),
            word_count: content.chars().count(),
            interaction_count: 0,
            trigger_type: "manual".to_string(),
            trigger_score: 0,
            mood_tag: mood.unwrap_or("normal").to_string(),
            created_at: ts,
            structured_keywords: None,
            story_update: None,
            relationship_delta: None,
            mood_samples: Vec::new(),
            version: 2,
        };

        let saved = diary::add_entry(char_id, entry).map_err(|e| e.to_string())?;

        // 同步写入记忆系统，便于后续检索
        let mut mem_tags = vec!["daily_diary".to_string(), format!("diary-{}", date)];
        mem_tags.extend(tags.iter().cloned());
        let mem_content = format!("[日记 {}] {}", date, content);
        let mem_meta = json!({
            "kind": "diary",
            "channel": "inner",
            "speaker": char_id,
            "listener": char_id,
            "perspective": "speaker",
        });
        let _ = mgr
            .add_memory_with_metadata(&mem_content, MemoryType::General, 0.6, mem_tags, mem_meta)
            .await;

        Ok(json!({
            "diary_id": saved.id,
            "date": saved.date,
            "content": saved.content,
            "mood": mood,
            "highlights": highlights,
            "tags": tags,
            "created_at": saved.created_at,
        }))
    }

    /// 获取最近互动记录（granularity=turn 且在时间窗内）
    pub async fn get_recent_interactions(
        mgr: &MemoryManager,
        time_range_hours: u64,
        limit: usize,
    ) -> Result<Value, String> {
        let all = mgr.get_all_memories().await.map_err(|e| e.to_string())?;
        let now = current_timestamp();
        let window_start = now - (time_range_hours as f64) * 3600.0;

        let mut interactions: Vec<Value> = all
            .into_iter()
            .filter(|m| m.granularity == "turn" && m.timestamp >= window_start)
            .map(|m| {
                json!({
                    "id": m.id,
                    "content": m.content,
                    "timestamp": m.timestamp,
                    "importance": m.importance,
                    "tags": m.tags,
                })
            })
            .collect();

        let count = interactions.len();
        interactions.sort_by(|a, b| {
            let ta = a.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tb = b.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
            tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
        });
        interactions.truncate(limit);

        Ok(json!({
            "interactions": interactions,
            "count": count,
            "time_range_hours": time_range_hours,
        }))
    }

    /// 总结今日上下文（日期、偏好、今日记忆、桌宠状态）
    pub async fn summarize_today_context(mgr: &MemoryManager) -> Result<Value, String> {
        let all = mgr.get_all_memories().await.map_err(|e| e.to_string())?;

        let now = chrono::Local::now();
        let date = now.format("%Y-%m-%d").to_string();
        let weekday = now.format("%A").to_string();

        // 今日 0 点的时间戳
        let today_start = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp() as f64;

        let today_memories: Vec<Value> = all
            .iter()
            .filter(|m| m.timestamp >= today_start)
            .map(|m| {
                json!({
                    "id": m.id,
                    "content": m.content,
                    "timestamp": m.timestamp,
                    "importance": m.importance,
                })
            })
            .collect();

        let preferences: Vec<Value> = all
            .iter()
            .filter(|m| m.tags.iter().any(|t| t == "preference"))
            .map(|m| {
                json!({
                    "id": m.id,
                    "content": m.content,
                    "tags": m.tags,
                })
            })
            .collect();

        Ok(json!({
            "date": date,
            "weekday": weekday,
            "preferences": preferences,
            "today_memories": today_memories,
            "pet_state": {"mood": "idle"},
        }))
    }
}

/// 将 MemoryItem 序列化为 JSON
fn memory_item_to_value(m: &MemoryItem) -> Value {
    json!({
        "id": m.id,
        "content": m.content,
        "granularity": m.granularity,
        "importance": m.importance,
        "timestamp": m.timestamp,
        "tags": m.tags,
        "metadata": m.metadata,
        "related_ids": m.related_ids,
        "created_at": m.timestamp,
    })
}

/// 解析记忆类型字符串为 MemoryType
fn parse_memory_type_str(s: &str) -> MemoryType {
    match s {
        "long_term_preference" | "preference" | "preferences" => MemoryType::Preference,
        "short_term_context" | "short_term" => MemoryType::ShortTerm,
        "emotional_history" => MemoryType::General,
        "daily_diary" => MemoryType::General,
        "interaction_record" => MemoryType::CasualConversation,
        "user_fact" | "user" => MemoryType::User,
        "important_event" => MemoryType::ImportantEvent,
        "feedback" => MemoryType::Feedback,
        "project" => MemoryType::Project,
        "reference" => MemoryType::Reference,
        "long_term" => MemoryType::LongTerm,
        "mid_term" => MemoryType::MidTerm,
        "general" => MemoryType::General,
        _ => MemoryType::General,
    }
}

fn current_timestamp() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
