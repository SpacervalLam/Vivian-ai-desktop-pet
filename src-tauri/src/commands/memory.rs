//! 记忆命令 - 记忆的增删查改与摘要

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::memory::{MemoryType, RetrievalStrategy};
use crate::psychology::relationship::RelationshipState;
use crate::state::AppState;
use crate::utils::path;

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn item_to_value(item: &crate::memory::types::MemoryItem) -> Value {
    serde_json::to_value(item).unwrap_or(json!({}))
}

/// 获取所有记忆（记忆管理窗口使用）
///
/// 过滤掉 `metadata.source == "system_seed"` 的 seed 记忆 —— 这些是身份锚点与
/// 首次启动里程碑，属于系统内置恒久记忆，不应在 UI 中展示或被用户误删。
///
/// `character_id` 由前端按当前窗口角色传入（None 时回退到活跃角色），
/// 确保每个角色的记忆面板只展示该角色自己的记忆。
#[tauri::command]
pub async fn get_memories(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let character = state.get_character(character_id.as_deref())?;
    let memory = &character.brain.memory;
    let items = memory.get_all_memories().await.map_err(err_str)?;
    Ok(items
        .iter()
        .filter(|m| {
            m.metadata
                .get("source")
                .and_then(|v| v.as_str())
                .map_or(true, |s| s != "system_seed")
        })
        .map(item_to_value)
        .collect())
}

/// 获取图谱时间轴骨架点（记忆 + 日记）
///
/// 每个点仅含 `{id, ts, kind}`，不含内容与 embedding，用于驱动时间比例尺与迷你地图。
/// 过滤条件与范围查询共享 `is_graph_visible_memory` 谓词，避免骨架与内容不一致产生幽灵点。
#[tauri::command]
pub async fn get_graph_timeline(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let character = state.get_character(character_id.as_deref())?;
    let mut pts: Vec<Value> = character
        .brain
        .memory
        .timeline_points()
        .into_iter()
        .map(|(id, ts)| json!({ "id": id, "ts": ts, "kind": "memory" }))
        .collect();
    let diaries = crate::diary::get_entries(&character.id, None).map_err(err_str)?;
    pts.extend(diaries.iter().map(|d| {
        json!({ "id": d.id, "ts": d.created_at, "kind": "diary" })
    }));
    pts.sort_by(|a, b| {
        a["ts"]
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&b["ts"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(pts)
}

/// 按时间窗口获取完整记忆（图谱懒加载内容层）
///
/// 返回 `[after, before)` 区间内的完整记忆条目，序列化前剥离 embedding 以减小载荷。
#[tauri::command]
pub async fn get_memories_range(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    after: f64,
    before: f64,
) -> Result<Vec<Value>, String> {
    let character = state.get_character(character_id.as_deref())?;
    Ok(character
        .brain
        .memory
        .memories_in_range(after, before)
        .into_iter()
        .map(|mut m| {
            m.embedding = None;
            item_to_value(&m)
        })
        .collect())
}

/// 添加记忆
#[tauri::command]
pub async fn add_memory(
    state: State<'_, Arc<AppState>>,
    content: String,
    memory_type: String,
    importance: f64,
    character_id: Option<String>,
) -> Result<Value, String> {
    if content.trim().is_empty() {
        return Err("记忆内容不能为空".to_string());
    }
    let importance = importance.clamp(0.0, 1.0);
    let mt = MemoryType::from_str(&memory_type)
        .ok_or_else(|| format!("未知的记忆类型: {}", memory_type))?;

    let character = state.get_character(character_id.as_deref())?;
    let char_id = character.brain.memory.char_id().to_string();
    let item = character
        .brain
        .memory
        .add_memory_with_metadata(
            &content,
            mt,
            importance,
            Vec::new(),
            serde_json::json!({
                "channel": "direct",
                "speaker": "user",
                "listener": char_id,
                "perspective": "speaker",
                "knowledge_source": "direct",
            }),
        )
        .await
        .map_err(err_str)?;
    Ok(item_to_value(&item))
}

/// 删除指定记忆
#[tauri::command]
pub async fn delete_memory(
    state: State<'_, Arc<AppState>>,
    id: String,
    character_id: Option<String>,
) -> Result<(), String> {
    let character = state.get_character(character_id.as_deref())?;
    character
        .brain
        .memory
        .delete_memory(&id)
        .await
        .map_err(err_str)
}

/// 永久删除指定记忆（绕过回收站）
#[tauri::command]
pub async fn hard_delete_memory(
    state: State<'_, Arc<AppState>>,
    id: String,
    character_id: Option<String>,
) -> Result<(), String> {
    let character = state.get_character(character_id.as_deref())?;
    character
        .brain
        .memory
        .hard_delete_memory(&id)
        .await
        .map_err(err_str)
}

/// 从回收站恢复记忆
#[tauri::command]
pub async fn restore_memory(
    state: State<'_, Arc<AppState>>,
    id: String,
    character_id: Option<String>,
) -> Result<bool, String> {
    let character = state.get_character(character_id.as_deref())?;
    character
        .brain
        .memory
        .restore_memory(&id)
        .map_err(err_str)
}

/// 列出回收站中的全部条目
#[tauri::command]
pub async fn list_recycle_bin(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let character = state.get_character(character_id.as_deref())?;
    let entries = character.brain.memory.list_recycle_bin();
    Ok(entries
        .iter()
        .map(|e| {
            json!({
                "id": e.item.id,
                "content": e.item.content,
                "memory_type": e.item.memory_type,
                "importance": e.item.importance,
                "timestamp": e.item.timestamp,
                "deleted_at": e.deleted_at,
                "reason": e.reason,
                "description": e.item.description,
                "tags": e.item.tags,
            })
        })
        .collect())
}

/// 永久清除回收站中指定条目
#[tauri::command]
pub async fn purge_recycle_entry(
    state: State<'_, Arc<AppState>>,
    id: String,
    character_id: Option<String>,
) -> Result<bool, String> {
    let character = state.get_character(character_id.as_deref())?;
    Ok(character.brain.memory.purge_recycle_entry(&id))
}

/// 清空整个回收站
#[tauri::command]
pub async fn clear_recycle_bin(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<usize, String> {
    let character = state.get_character(character_id.as_deref())?;
    Ok(character.brain.memory.purge_all_recycle_bin())
}

/// 清理回收站中已过期的条目（超过 7 天保留期）
#[tauri::command]
pub async fn purge_expired_recycle_bin(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<usize, String> {
    let character = state.get_character(character_id.as_deref())?;
    Ok(character.brain.memory.purge_expired_recycle_bin())
}

/// 清空所有记忆（恢复出厂设置）
///
/// 清空范围（按角色隔离 + 衍生层 + 全局共享层）：
///
/// **原有记忆-关系层**：
/// 1. 角色私有记忆（MemoryManager）
/// 2. 聊天历史记录文件
/// 3. 关系数值与交互统计
/// 4. 该角色相关的关系认知事实（clear_for_character）
/// 5. 共享世界知识：全量清空（clear_all）—— 世界知识无角色归属，
///    且多由被清空的角色对话衍生，保留会造成数据不一致
/// 6. 统一事件账本：全量清空（clear_all）—— 事件账本是无角色归属的衍生索引层，
///    且历史脏数据（associated_char_id 缺失的旧系统事件）无法被 clear_for_character 识别，
///    保留会造成事件页残留，与 WorldKnowledge/Episode 处理保持一致
/// 7. Episode 经历封包索引：全量清空
///
/// **心理-认知层（恢复出厂设置新增）**：
/// 8. 心理快照整体重置（emotion/needs/persona/relationship/events 等）→ reset_to_initial
/// 9. Mind 认知层清空（beliefs/goals/attention/working_memory/current_thought）→ mind.reset_all
/// 10. 角色日记清空（diary/clear_all_entries）
///
/// **运行时状态持久化层（删除文件，下次启动重新初始化）**：
/// 11. Proactive 状态文件（state.json / topics.json / habits.json / trigger_preferences.json）
/// 12. Presence 在场状态文件（state.json）
/// 13. User facts 用户事实画像（shared/user_facts.json）—— 让角色"重新认识用户"
///
/// **内容资产层**：
/// 14. 笔记目录（notebook/）—— 删除整个目录及其下所有 note.json / note.html /
///     .memory_ref / index.json；知识库条目已随 MemoryManager.entries.clear() 一并清空
///
/// 启动问候的「首次见面」判定基于记忆是否为空，由 clear_all_memories 保证。
/// 由于运行时部分模块（Proactive/Presence/UserFacts 等）的状态在内存中缓存，
/// 建议前端在清空成功后提示用户重启应用以完全生效。
#[tauri::command]
pub async fn clear_all_memories(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let character = state.get_character(character_id.as_deref())?;
    let char_id = character.id.clone();
    let char_data_dir = path::get_character_data_dir(&char_id);

    // ===== 1. 记忆-关系层（原有清空逻辑）=====
    character
        .brain
        .memory
        .clear_all_memories()
        .await
        .map_err(err_str)?;
    // 一并清空聊天历史记录文件，避免聊天窗口仍显示已清空的详细对话
    character
        .brain
        .dialogue
        .clear_history_file()
        .map_err(err_str)?;
    // 清空多级对话存档（由聊天历史压缩而来，历史清空后失去依据）
    if let Some(chain) = &character.brain.chat_chain {
        chain.conversation_archive.write().clear();
    }
    // 重置关系数值与交互统计
    character
        .brain
        .psychology
        .reset_relationship()
        .map_err(err_str)?;
    // 清除该角色相关的关系认知事实（保留其他角色之间的事实）
    crate::psychology::relationship_facts::relationship_facts()
        .clear_for_character(&char_id)
        .map_err(err_str)?;
    // 共享世界知识：全量清空（无角色归属，且多为对话衍生）
    crate::memory::world_knowledge::world_knowledge()
        .clear_all()
        .map_err(err_str)?;
    // 统一事件账本：全量清空（衍生索引层，无角色归属；历史脏数据 associated_char_id
    // 缺失无法被 clear_for_character 识别，按角色清空会残留系统事件）
    crate::memory::unified_event_ledger::unified_event_ledger()
        .clear_all()
        .map_err(err_str)?;
    // 清空 Episode 经历封包索引（记忆聚合视图，私有记忆清空后索引无意义）
    if let Some(episode_store) = character.brain.memory.episode_store() {
        episode_store.clear_all();
    }
    // 关系日志清空
    crate::psychology::relationship_log().clear().map_err(err_str)?;

    // ===== 2. 心理-认知层（恢复出厂设置新增）=====
    // 心理快照整体重置（emotion/needs/persona/relationship/events）
    // 注意：persona 重置为 default 后，下次启动时 with_persona 会从 PersonaEngine 重新推导
    character
        .brain
        .psychology
        .reset_to_initial()
        .map_err(err_str)?;
    // Mind 认知层清空（beliefs/goals/attention/working_memory/current_thought）
    character
        .brain
        .mind
        .reset_all()
        .map_err(|e| format!("Mind 重置失败: {}", e))?;
    // 角色日记清空
    crate::diary::clear_all_entries(&char_id).map_err(err_str)?;

    // ===== 3. 运行时状态持久化层（删除文件，下次启动重新初始化）=====
    // Proactive 状态文件：state.json / topics.json / habits.json / trigger_preferences.json
    let proactive_dir = char_data_dir.join("proactive");
    for name in ["state.json", "topics.json", "habits.json", "trigger_preferences.json"] {
        let p = proactive_dir.join(name);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    // Presence 在场状态文件：state.json
    let presence_path = char_data_dir.join("presence").join("state.json");
    if presence_path.exists() {
        let _ = std::fs::remove_file(&presence_path);
    }

    // ===== 4. 用户事实画像层（按角色隔离存储）=====
    // 路径：characters/<char_id>/user_facts.json（让角色"重新认识用户"）
    let user_facts_path = char_data_dir.join("user_facts.json");
    if user_facts_path.exists() {
        let _ = std::fs::remove_file(&user_facts_path);
    }

    // ===== 5. 内容资产层：笔记目录 =====
    // 删除整个 notebook/ 目录（note.json / note.html / .memory_ref / index.json）。
    // 知识库条目已随上面的 memory.clear_all_memories() 的 entries.clear() 一并清空，
    // 无需逐个读 .memory_ref 调用 delete_knowledge_document。
    if let Err(e) = crate::notebook::storage::clear_all(&char_id) {
        tracing::warn!("[clear_all_memories] 清空角色 {} 笔记目录失败: {e}", char_id);
    }

    let _ = app.emit(
        "chat:history-cleared",
        json!({ "character_id": char_id }),
    );
    let _ = app.emit(
        "memory:updated",
        json!({ "character_id": char_id }),
    );
    Ok(())
}

/// 获取记忆摘要
#[tauri::command]
pub async fn get_memory_summary(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<String, String> {
    let character = state.get_character(character_id.as_deref())?;
    Ok(character.brain.memory.get_memory_summary())
}

/// 搜索记忆
#[tauri::command]
pub async fn search_memories(
    state: State<'_, Arc<AppState>>,
    query: String,
    limit: Option<usize>,
    character_id: Option<String>,
) -> Result<Vec<Value>, String> {
    if query.trim().is_empty() {
        return Err("搜索关键词不能为空".to_string());
    }
    let limit = limit.unwrap_or(10);

    let character = state.get_character(character_id.as_deref())?;
    let items = character
        .brain
        .memory
        .search_memories(&query, RetrievalStrategy::Auto, limit)
        .await
        .map_err(err_str)?;
    Ok(items.iter().map(item_to_value).collect())
}

/// 获取所有在线角色的记忆（综合视图）
///
/// 合并所有在线角色的记忆，每条记忆标注 `character_id` 和 `character_name`，
/// 按时间戳倒序排列。过滤掉 system_seed 记忆。
#[tauri::command]
pub async fn get_memories_all(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Value>, String> {
    // 先在锁内 clone 出需要的数据，drop guard 后再 await（guard 非 Send）
    let chars_snapshot: Vec<(String, String, crate::brain::Brain)> = {
        let characters = state.characters.read();
        characters
            .values()
            .filter(|c| *c.online.read())
            .map(|c| (c.id.clone(), c.name.clone(), c.brain.clone()))
            .collect()
    };

    let mut all_items: Vec<Value> = Vec::new();
    for (cid, cname, brain) in chars_snapshot {
        let items = brain.memory.get_all_memories().await.map_err(err_str)?;
        for item in items.iter() {
            if item
                .metadata
                .get("source")
                .and_then(|v| v.as_str())
                .map_or(true, |s| s != "system_seed")
            {
                let mut v = item_to_value(item);
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("character_id".to_string(), json!(cid));
                    obj.insert("character_name".to_string(), json!(cname));
                }
                all_items.push(v);
            }
        }
    }
    all_items.sort_by(|a, b| {
        let ta = a
            .get("timestamp")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let tb = b
            .get("timestamp")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(all_items)
}

/// 读取共同记忆（存储在 common/memory/unified_memory.json）
///
/// 共同记忆是两个角色共享的记忆，如世界设定、共同经历。
fn load_common_memories() -> Result<Vec<Value>, String> {
    let store_path = path::get_common_memory_dir().join("unified_memory.json");
    if !store_path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&store_path).map_err(err_str)?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let data: serde_json::Value = serde_json::from_str(&content).map_err(err_str)?;
    let items = data
        .get("memories")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let filtered: Vec<Value> = items
        .into_iter()
        .filter(|m| {
            m.get("metadata")
                .and_then(|m| m.get("source"))
                .and_then(|v| v.as_str())
                .map_or(true, |s| s != "system_seed")
        })
        .collect();
    Ok(filtered)
}

/// 保存共同记忆（全量覆盖写）
fn save_common_memories(items: &[Value]) -> Result<(), String> {
    let store_path = path::get_common_memory_dir().join("unified_memory.json");
    let data = json!({ "memories": items });
    let content = serde_json::to_string_pretty(&data).map_err(err_str)?;
    std::fs::write(&store_path, content).map_err(err_str)?;
    Ok(())
}

/// 获取共同记忆
#[tauri::command]
pub async fn get_common_memories() -> Result<Vec<Value>, String> {
    load_common_memories()
}

/// 添加共同记忆
#[tauri::command]
pub async fn add_common_memory(
    app: AppHandle,
    content: String,
    memory_type: String,
    importance: f64,
) -> Result<Value, String> {
    if content.trim().is_empty() {
        return Err("记忆内容不能为空".to_string());
    }
    let importance = importance.clamp(0.0, 1.0);
    let _ = MemoryType::from_str(&memory_type)
        .ok_or_else(|| format!("未知的记忆类型: {}", memory_type))?;

    let mut items = load_common_memories()?;
    let id = format!("common-{}", chrono::Utc::now().timestamp_millis());
    let new_item = json!({
        "id": id,
        "content": content,
        "memory_type": memory_type,
        "importance": importance,
        "timestamp": chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
        "tags": [],
        "metadata": { "source": "manual_common" },
    });
    items.push(new_item.clone());
    save_common_memories(&items)?;
    let _ = app.emit("memory:updated", json!({ "character_id": null }));
    Ok(new_item)
}

/// 删除共同记忆
#[tauri::command]
pub async fn delete_common_memory(app: AppHandle, id: String) -> Result<(), String> {
    let mut items = load_common_memories()?;
    let before = items.len();
    items.retain(|m| m.get("id").and_then(|v| v.as_str()) != Some(&id));
    if items.len() == before {
        return Err(format!("共同记忆 {} 不存在", id));
    }
    save_common_memories(&items)?;
    let _ = app.emit("memory:updated", json!({ "character_id": null }));
    Ok(())
}

/// 清空共同记忆
#[tauri::command]
pub async fn clear_common_memories(app: AppHandle) -> Result<(), String> {
    save_common_memories(&[])?;
    let _ = app.emit("memory:updated", json!({ "character_id": null }));
    Ok(())
}

// ====================================================================
// 四层记忆架构 - 新增数据层 command
//
// 供记忆管理面板展示统一事件账本、共享世界知识、关系认知事实、社交状态。
// 所有 command 接受可选的 character_id 参数以保持与其他 command 的一致性，
// 但新数据层多为全局数据，character_id 仅用于过滤视角。
// ====================================================================

/// 列出统一事件账本中指定角色可见的事件（按时间倒序）
///
/// 无 character_id 时返回全部 Public 事件。
/// 支持 offset 分页：跳过前 offset 条事件，返回后续 limit 条。
#[tauri::command]
pub async fn list_unified_events(
    _state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<Value>, String> {
    let ledger = crate::memory::unified_event_ledger::unified_event_ledger();
    let n = limit.unwrap_or(500);
    let skip = offset.unwrap_or(0);
    let events = match character_id.as_deref() {
        Some(cid) if !cid.is_empty() => ledger.recent_events_visible_to(cid, n + skip),
        _ => ledger.recent_public_events(n + skip),
    };
    Ok(events
        .into_iter()
        .skip(skip)
        .take(n)
        .map(|e| serde_json::to_value(&e).unwrap_or(json!({})))
        .collect())
}

/// 列出全部共享世界知识（按 created_at 降序）
#[tauri::command]
pub async fn list_world_facts() -> Result<Vec<Value>, String> {
    let engine = crate::memory::world_knowledge::world_knowledge();
    let facts = engine.list_all();
    Ok(facts.into_iter().map(|f| serde_json::to_value(&f).unwrap_or(json!({}))).collect())
}

/// 列出全部关系认知事实（按 created_at 降序）
#[tauri::command]
pub async fn list_relationship_facts() -> Result<Vec<Value>, String> {
    let engine = crate::psychology::relationship_facts::relationship_facts();
    let facts = engine.list_all();
    Ok(facts.into_iter().map(|f| serde_json::to_value(&f).unwrap_or(json!({}))).collect())
}

/// 获取三方社交状态快照（用户↔A、用户↔B、A↔B）
///
/// 需要从两个在线角色的 PsychologyManager 读取各自与用户的关系状态，
/// 再从 SocialStateEngine 读取 A↔B 关系数值。
#[tauri::command]
pub async fn get_social_state_snapshot(
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    use crate::psychology::social_state::social_state;
    let engine = social_state();

    let characters = state.characters.read();
    let mut entries: Vec<(String, RelationshipState)> = characters
        .iter()
        .map(|(id, c)| (id.clone(), c.brain.psychology.relationship()))
        .collect();
    // 按 id 排序确保 A/B 顺序稳定（vivian 在前，nana 在后）
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    match entries.len() {
        0 => Ok(json!({
            "user_agent_a": null,
            "user_agent_b": null,
            "agent_a_agent_b": null,
            "agent_a_id": null,
            "agent_b_id": null,
        })),
        1 => Ok(json!({
            "user_agent_a": serde_json::to_value(&entries[0].1).ok(),
            "user_agent_b": null,
            "agent_a_agent_b": null,
            "agent_a_id": entries[0].0,
            "agent_b_id": null,
        })),
        _ => {
            let (a_id, a_rel) = &entries[0];
            let (b_id, b_rel) = &entries[1];
            let ab = engine.get_pair(a_id, b_id);
            Ok(json!({
                "user_agent_a": serde_json::to_value(a_rel).ok(),
                "user_agent_b": serde_json::to_value(b_rel).ok(),
                "agent_a_agent_b": serde_json::to_value(&ab).ok(),
                "agent_a_id": a_id,
                "agent_b_id": b_id,
            }))
        }
    }
}

/// 重建所有角色记忆的向量索引（切换嵌入模型后调用）。
///
/// 秒回：置重建标志 + spawn 后台任务后立即返回，实际重建在后台进行，
/// 进度经 `memory:rebuild_progress` 事件推送，完成发 `memory:rebuild_done`。
/// 这样设置窗口可以立刻关闭，重建不阻塞前端。
#[tauri::command]
pub async fn rebuild_memory_embeddings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    if state.is_rebuild_in_progress() {
        return Ok(json!({ "started": false, "reason": "already_in_progress" }));
    }
    // 先收集各角色 memory manager（Arc clone）再放锁，避免重建期间长时间持有 characters 读锁
    let managers: Vec<Arc<crate::memory::MemoryManager>> = {
        let chars = state.characters.read();
        chars.values().map(|c| c.brain.memory.clone()).collect()
    };
    let total: usize = managers.iter().map(|m| m.count_indexable()).sum();
    state.set_rebuild_in_progress(true);
    let state_arc: Arc<AppState> = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let current = AtomicUsize::new(0);
        let mut rebuilt = 0usize;
        let _ = app.emit("memory:rebuild_progress", json!({ "current": 0, "total": total }));
        for m in &managers {
            let res = m.rebuild_all_embeddings(|| {
                let c = current.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = app.emit("memory:rebuild_progress", json!({ "current": c, "total": total }));
            });
            match res {
                Ok(n) => rebuilt += n,
                Err(e) => tracing::warn!("[RebuildEmbeddings] 部分失败: {e}"),
            }
        }
        let _ = app.emit("memory:rebuild_done", json!({ "rebuilt": rebuilt, "total": total }));
        state_arc.set_rebuild_in_progress(false);
        tracing::info!("[RebuildEmbeddings] 重建完成 {rebuilt}/{total}");
    });
    Ok(json!({ "started": true, "total": total }))
}

/// 返回内置已知嵌入模型元数据（供前端在设置表单选择模型时展示维度、自动填充 dimension）。
#[tauri::command]
pub fn get_embedding_models() -> Value {
    json!(
        crate::memory::embedding_registry::all_models()
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "dimension": m.dimension,
                    "source": match m.source {
                        crate::memory::embedding_registry::EmbeddingSource::Cloud => "cloud",
                        crate::memory::embedding_registry::EmbeddingSource::Local => "local",
                    },
                    "display_name": m.display_name,
                })
            })
            .collect::<Vec<_>>()
    )
}
