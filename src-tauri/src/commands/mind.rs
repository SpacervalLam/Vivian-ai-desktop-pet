//! Mind 命令 - 心智观察器（Mind Inspector）数据接口。
//!
//! 与 PsychologyManager 的差异：Mind 模块聚合 Belief / Goal / Attention 三个一等公民，
//! 加上 FocusState 的认知模式与心理状态合成 thought，构成"实时心智快照"。
//! 前端 Mind Inspector 用这些数据展示两个智能体的大脑运转状态。
//!
//! - `get_mind_state`：单角色心智快照（Attention + Goals + CognitionMode + 最近 thought）
//! - `get_world_snapshot`：世界快照 + 用户研究任务
//! - `list_beliefs`：BeliefStore 全量信念
//! - `get_memory_health`：记忆巩固流水线步骤健康快照

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

/// 获取单角色实时心智快照
///
/// 一次性返回 Mind Inspector 首页所需的全部字段，避免前端多次 IPC 往返：
/// - `attention_top`: 注意力 Top-N 实体（实体名 + 权重）
/// - `goals`: 当前活跃目标列表
/// - `cognition_mode`: 认知模式（regular / focus / true_name）
/// - `focus_charge`: 凝神电荷值（0.0-1.0）
/// - `current_thought`: 从心理状态合成的一句话摘要（情绪+需求+活动+注意力）
#[tauri::command]
pub async fn get_mind_state(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let brain = instance.brain.clone();

    // Attention Top-5
    let attention_top: Vec<Value> = brain
        .mind
        .attention_top_n(5)
        .into_iter()
        .map(|(entity, weight)| {
            json!({ "entity": entity, "weight": weight })
        })
        .collect();

    // Goals active Top-3
    let goals: Vec<Value> = brain
        .mind
        .goals
        .read()
        .active_top_n(3)
        .into_iter()
        .map(|g| {
            json!({
                "id": g.id,
                "description": g.description,
                "priority": g.priority,
                "active": g.active,
            })
        })
        .collect();

    // CognitionMode + charge（tokio Mutex，须 await）
    let focus = brain.focus_state.lock().await;
    let cognition_mode = focus.mode.as_str().to_string();
    let focus_charge = focus.charge;
    drop(focus);

    // current_thought: 仅使用 LLM 合成缓存，不存在时留空
    let current_thought = brain.mind.current_thought_snapshot().unwrap_or_default();
    // 内心独白/当前想法总开关（前端据此决定是否显示占位文本）
    let inner_monologue_enabled = brain.config.world.enable_inner_monologue;

    Ok(json!({
        "character_id": instance.id,
        "character_name": instance.name,
        "attention_top": attention_top,
        "goals": goals,
        "cognition_mode": cognition_mode,
        "focus_charge": focus_charge,
        "current_thought": current_thought,
        "inner_monologue_enabled": inner_monologue_enabled,
    }))
}

/// 获取世界快照 + 用户研究任务
///
/// 返回 WorldSnapshot（时间/天气/季节/音乐/上次交互）与研究任务列表，
/// 供 Mind Inspector World 页展示。
/// 若天气缓存为空且天气功能已启用，会异步触发一次天气刷新（fire-and-forget）。
#[tauri::command]
pub async fn get_world_snapshot(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let brain = instance.brain.clone();

    // 将用户在场状态同步到 world_provider 缓存（供前端展示）
    // 直接从系统空闲时间计算，避免依赖 proactive_tick 的更新延迟
    let mut user_presence = brain.world_state.user_entity_snapshot();
    let idle_secs = crate::utils::get_system_idle_seconds();
    
    if idle_secs.is_none() {
        if user_presence.presence != crate::world::entity_state::UserPresence::Present {
            let _ = brain.world_state.mark_user_present();
            user_presence = brain.world_state.user_entity_snapshot();
        }
    } else {
        let away_threshold = brain.config.proactive.away_threshold_seconds as f64;
        if idle_secs.unwrap() < 60.0 && user_presence.presence != crate::world::entity_state::UserPresence::Present {
            let _ = brain.world_state.mark_user_present();
            user_presence = brain.world_state.user_entity_snapshot();
        } else if idle_secs.unwrap() >= away_threshold && user_presence.presence != crate::world::entity_state::UserPresence::Away {
            brain.world_state.mark_user_away();
            user_presence = brain.world_state.user_entity_snapshot();
        }
    }
    brain.world_provider.set_user_presence(user_presence);

    // 上次交互秒数：从 presence 取
    let seconds_since_last_interaction: Option<f64> = None; // brain 内部未暴露，留空由前端从其他源补
    let snapshot = brain
        .world_provider
        .snapshot(seconds_since_last_interaction);

    // 天气缓存为空且天气功能启用时，异步触发一次刷新
    // 避免心智观察器页面打开后天气始终为空（proactive_tick 可能未在运行）
    if snapshot.weather.is_none() {
        let wp = brain.world_provider.clone();
        let cfg = wp.config();
        if cfg.enable_weather && cfg.latitude.is_some() && cfg.longitude.is_some() && wp.has_weather_source() {
            tauri::async_runtime::spawn(async move {
                tracing::info!("[WorldSnapshot] 天气缓存为空，主动触发一次刷新");
                wp.refresh_weather().await;
            });
        }
    }

    // 音乐缓存为空且音乐源已注入时，异步触发一次刷新
    if snapshot.music.is_none() && brain.world_provider.has_music_source() {
        let wp = brain.world_provider.clone();
        tauri::async_runtime::spawn(async move {
            tracing::debug!("[WorldSnapshot] 音乐缓存为空，主动触发一次刷新");
            wp.refresh_music().await;
        });
    }

    let snapshot_value = serde_json::to_value(&snapshot).map_err(|e| e.to_string())?;

    // 用户研究任务快照（活跃课题 + 已确认习惯）
    let research: Vec<Value> = brain
        .research
        .tasks_snapshot()
        .into_iter()
        .map(|t| serde_json::to_value(&t).unwrap_or(Value::Null))
        .collect();

    // 用户行为日志（Event 层）：最近 50 条，按时间降序
    let behaviors: Vec<Value> = {
        let log_handle = brain.world_state.behavior_log();
        let log = log_handle.read();
        let recent: Vec<_> = log.recent(50).into_iter().cloned().collect();
        drop(log);
        recent
            .into_iter()
            .map(|e| serde_json::to_value(&e).unwrap_or(Value::Null))
            .collect()
    };

    // 用户认知 Belief（Knowledge 层）：subject="user" 且未取代的，按 confidence 降序
    let user_beliefs: Vec<Value> = {
        let store = brain.mind.beliefs.read();
        let mut beliefs: Vec<Value> = store
            .active_by_subject("user")
            .into_iter()
            .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
            .collect();
        beliefs.sort_by(|a, b| {
            let ca = a.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cb = b.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
            cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
        });
        beliefs
    };

    Ok(json!({
        "snapshot": snapshot_value,
        "research": research,
        "behaviors": behaviors,
        "user_beliefs": user_beliefs,
    }))
}

/// 列出角色全部信念
///
/// 按 confidence 降序返回 BeliefStore.beliefs，供 Mind Inspector Belief 页展示。
/// 每条 Belief 携带 source_memory_ids / source_episode_ids 用于前端展开 Evidence。
#[tauri::command]
pub fn list_beliefs(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Value>, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let store = instance.brain.mind.beliefs.read();
    let mut beliefs: Vec<Value> = store
        .beliefs
        .iter()
        .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
        .collect();
    beliefs.sort_by(|a, b| {
        let ca = a.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cb = b.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(beliefs)
}

/// 获取记忆巩固流水线的步骤健康快照
///
/// 返回每个巩固步骤（pipeline / belief）的健康状态：
/// `last_success_at` / `last_error_at` / `last_error_msg` / `fail_count`（连续失败，
/// 一次成功清零）。`healthy=false` 表示存在故障步骤（冷却退化为 30 分钟快速重试）。
/// 巩固功能关闭（enable_memory_consolidation=false）时 `enabled=false`。
#[tauri::command]
pub fn get_memory_health(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let brain = instance.brain.clone();
    match &brain.consolidator {
        Some(consolidator) => {
            let steps = consolidator.health_status();
            let healthy = steps
                .values()
                .all(|h| h.fail_count == 0 && h.paused_reason.is_none());
            let paused_steps: Vec<String> = steps
                .iter()
                .filter(|(_, h)| h.paused_reason.is_some())
                .map(|(name, h)| {
                    format!("{}: {}", name, h.paused_reason.clone().unwrap_or_default())
                })
                .collect();
            let steps_json: serde_json::Map<String, Value> = steps
                .into_iter()
                .map(|(k, v)| (k, serde_json::to_value(v).unwrap_or(Value::Null)))
                .collect();
            Ok(json!({
                "enabled": true,
                "healthy": healthy,
                "paused_steps": paused_steps,
                "steps": steps_json,
            }))
        }
        None => Ok(json!({
            "enabled": false,
            "healthy": true,
            "steps": {},
        })),
    }
}
