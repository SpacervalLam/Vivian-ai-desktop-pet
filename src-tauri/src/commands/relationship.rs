//! 关系命令 - 关系状态查询与管理。
//!
//! 关系系统已整合到 PsychologyManager，所有操作通过 brain.psychology 进行。

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

/// 获取关系完整状态
#[tauri::command]
pub fn get_relationship(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let rel = brain.psychology.relationship();
    Ok(json!({
        "intimacy": rel.intimacy * 100.0,
        "trust": rel.trust * 100.0,
        "respect": rel.respect * 100.0,
        "dependency": rel.dependency * 100.0,
        "familiarity": rel.familiarity * 100.0,
        "interaction_count": rel.interaction_count,
        "consecutive_positive": rel.consecutive_positive,
        "consecutive_negative": rel.consecutive_negative,
        "permanent_stage": rel.permanent_stage.as_str(),
        "temporary_stage": rel.temporary_stage.as_ref().map(|t| t.as_str()),
        "effective_stage_label": rel.get_effective_stage_label(),
        "last_interaction_time": rel.last_interaction_time,
    }))
}

/// 获取当前关系阶段
#[tauri::command]
pub fn get_relationship_stage(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<String, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    Ok(brain.psychology.get_stage().as_str().to_string())
}

/// 获取里程碑列表
#[tauri::command]
pub fn get_milestones(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let milestones = brain.psychology.get_milestones();
    serde_json::to_value(milestones).map_err(|e| e.to_string())
}

/// 重置关系（回到陌生人）
#[tauri::command]
pub fn reset_relationship(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain.psychology.reset_relationship().map_err(|e| e.to_string())
}
