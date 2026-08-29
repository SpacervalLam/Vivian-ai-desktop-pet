//! 历史命令 - 聊天历史查询与清空

use std::sync::Arc;

use serde_json::Value;
use tauri::State;

use crate::dialogue::DialogueManager;
use crate::state::AppState;

#[tauri::command]
pub async fn get_chat_history(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let dialogue = instance.brain.dialogue.clone();
    let entries = tokio::task::spawn_blocking(move || dialogue.get_all_history())
        .await
        .map_err(|e| format!("任务执行失败: {}", e))?
        .map_err(|e| e.to_string())?;
    let values: Vec<Value> = entries
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "role": e.role,
                "content": e.content,
                "timestamp": e.timestamp,
                "session_id": e.session_id,
                "metadata": e.metadata,
            })
        })
        .collect();
    Ok(values)
}

#[tauri::command]
pub async fn clear_chat_history(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let instance = state.get_character(character_id.as_deref())?;
    let dialogue = instance.brain.dialogue.clone();
    dialogue.clear_history_file().map_err(|e| e.to_string())?;
    tracing::info!("[{}] 聊天历史已清空", instance.id);
    Ok(())
}

/// 获取所有在线角色的聊天历史（综合视图）
///
/// 合并所有在线角色的聊天历史，每条标注 `character_id` 和 `character_name`，
/// 按时间戳升序排列（时间线视图）。
#[tauri::command]
pub async fn get_chat_history_all(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Value>, String> {
    let chars: Vec<(Arc<DialogueManager>, String, String)> = {
        let characters = state.characters.read();
        characters
            .values()
            .filter(|c| *c.online.read())
            .map(|c| {
                (
                    c.brain.dialogue.clone(),
                    c.id.clone(),
                    c.name.clone(),
                )
            })
            .collect()
    };
    let mut all_entries = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        let mut entries: Vec<Value> = Vec::new();
        for (dialogue, cid, cname) in chars {
            let history = dialogue.get_all_history().map_err(|e| e.to_string())?;
            for e in history {
                entries.push(serde_json::json!({
                    "id": e.id,
                    "role": e.role,
                    "content": e.content,
                    "timestamp": e.timestamp,
                    "session_id": e.session_id,
                    "metadata": e.metadata,
                    "character_id": cid,
                    "character_name": cname,
                }));
            }
        }
        Ok(entries)
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))??;

    all_entries.sort_by(|a, b| {
        let ta = a
            .get("timestamp")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let tb = b
            .get("timestamp")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(all_entries)
}

/// 轻量预览：仅返回每个会话（私聊角色 / 群聊）的最新一条消息 + 未读计数，供主页列表展示。
///
/// 相比 `get_chat_history_all` 全量传输后在前端过滤，本命令在 Rust 端完成聚合，
/// 避免大历史文件场景下的序列化与 IPC 传输开销。
/// `last_seen` 为前端传入的各会话已读水位线（key: 角色ID 或 "group"，value: 时间戳毫秒），
/// 据此统计新于水位线的 assistant 消息数作为未读计数。
#[tauri::command]
pub async fn search_chat_history(
    state: State<'_, Arc<AppState>>,
    query: String,
) -> Result<Vec<Value>, String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    const MAX_RESULTS: usize = 50;
    let chars: Vec<(Arc<DialogueManager>, String, String)> = {
        let characters = state.characters.read();
        characters
            .values()
            .filter(|c| *c.online.read())
            .map(|c| (c.brain.dialogue.clone(), c.id.clone(), c.name.clone()))
            .collect()
    };
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        let mut groups: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        let mut order: Vec<(f64, String)> = Vec::new();
        for (dialogue, cid, cname) in chars {
            let entries = dialogue.get_all_history().map_err(|e| e.to_string())?;
            for e in entries {
                if e.role == "system" {
                    continue;
                }
                let content = e.content.as_str();
                if !content.to_lowercase().contains(&q) {
                    continue;
                }
                let ts = e.timestamp as f64;
                let key = format!("{}|{}", content, ts);
                if let Some(existing) = groups.get_mut(&key) {
                    if let Some(chars) = existing.get_mut("matched_chars") {
                        if let Some(arr) = chars.as_array_mut() {
                            if !arr.iter().any(|v| v.as_str() == Some(&cid)) {
                                arr.push(serde_json::Value::String(cid.clone()));
                            }
                        }
                    }
                } else {
                    let mut arr = Vec::new();
                    arr.push(serde_json::Value::String(cid.clone()));
                    let value = serde_json::json!({
                        "id": e.id,
                        "content": content,
                        "role": e.role,
                        "timestamp": e.timestamp,
                        "character_id": cid,
                        "character_name": cname,
                        "matched_chars": arr,
                    });
                    groups.insert(key.clone(), value);
                    order.push((ts, key));
                }
            }
        }
        order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let result: Vec<Value> = order
            .into_iter()
            .take(MAX_RESULTS)
            .filter_map(|(_, k)| {
                let mut v = groups.remove(&k)?;
                let is_group = v
                    .get("matched_chars")
                    .and_then(|c| c.as_array())
                    .map(|a| a.len() > 1)
                    .unwrap_or(false);
                v["source"] = if is_group {
                    serde_json::Value::String("group".to_string())
                } else {
                    serde_json::Value::String("private".to_string())
                };
                if let Some(obj) = v.as_object_mut() {
                    obj.remove("matched_chars");
                }
                Some(v)
            })
            .collect();
        Ok(result)
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))??;
    Ok(result)
}

#[tauri::command]
pub async fn get_latest_previews(
    state: State<'_, Arc<AppState>>,
    last_seen: Option<std::collections::HashMap<String, f64>>,
) -> Result<Value, String> {
    let chars: Vec<(Arc<DialogueManager>, String, String)> = {
        let characters = state.characters.read();
        characters
            .values()
            .filter(|c| *c.online.read())
            .map(|c| (c.brain.dialogue.clone(), c.id.clone(), c.name.clone()))
            .collect()
    };
    let is_first_load = last_seen.is_none();
    let seen = last_seen.unwrap_or_default();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let mut private_latest: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        let mut group_latest: Option<Value> = None;
        let mut group_latest_ts: f64 = 0.0;
        let mut unread: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

        for (dialogue, cid, cname) in chars {
            let entries = dialogue.get_all_history().map_err(|e| e.to_string())?;
            let mut char_latest_ts: f64 = 0.0;
            for e in entries {
                if e.role == "system" {
                    continue;
                }
                let ch = e
                    .metadata
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let ts = e.timestamp as f64;
                if ch.as_deref() == Some("wechat") {
                    if ts > char_latest_ts {
                        char_latest_ts = ts;
                        private_latest.insert(cid.clone(), serde_json::json!({
                            "id": e.id,
                            "role": e.role,
                            "content": e.content,
                            "timestamp": e.timestamp,
                            "metadata": e.metadata,
                            "character_id": cid,
                            "character_name": cname,
                        }));
                    }
                    if !is_first_load && e.role == "assistant" {
                        let watermark = seen.get(&cid).copied().unwrap_or(0.0);
                        if ts > watermark {
                            *unread.entry(cid.clone()).or_insert(0) += 1;
                        }
                    }
                }
                if ch.as_deref() == Some("wechat_group") {
                    if ts > group_latest_ts {
                        group_latest_ts = ts;
                        group_latest = Some(serde_json::json!({
                            "id": e.id,
                            "role": e.role,
                            "content": e.content,
                            "timestamp": e.timestamp,
                            "metadata": e.metadata,
                            "character_id": cid,
                            "character_name": cname,
                        }));
                    }
                    if !is_first_load && e.role == "assistant" {
                        let watermark = seen.get("group").copied().unwrap_or(0.0);
                        if ts > watermark {
                            *unread.entry("group".to_string()).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
        let mut previews: Vec<Value> = private_latest.into_values().collect();
        if let Some(g) = group_latest {
            previews.push(g);
        }
        Ok(serde_json::json!({
            "previews": previews,
            "unread": unread,
        }))
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))??;
    Ok(result)
}
