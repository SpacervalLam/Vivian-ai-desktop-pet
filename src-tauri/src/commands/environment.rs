//! 环境命令 - 系统环境信息查询与更新

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

/// 获取环境信息
#[tauri::command]
pub fn get_environment_info(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let info = brain.environment.get_environment_info();
    serde_json::to_value(info).map_err(|e| e.to_string())
}

/// 获取当前精简状态
#[tauri::command]
pub fn get_current_state(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let state = brain.environment.get_current_state();
    serde_json::to_value(state).map_err(|e| e.to_string())
}

/// 获取用户活动状态
#[tauri::command]
pub fn get_user_activity(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let activity = brain.environment.get_user_activity();
    serde_json::to_value(activity).map_err(|e| e.to_string())
}

/// 更新环境信息（前端定时调用）
#[tauri::command]
pub fn update_environment(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    mouse_x: i32,
    mouse_y: i32,
    active_window: String,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain.environment.update((mouse_x, mouse_y), active_window);
    Ok(())
}

/// 获取启动问候（通过 LLM 生成）
///
/// 返回 `{ greeting: String, error: Option<String> }`：
/// - `greeting` 非空 → 正常问候
/// - `greeting` 为空且 `error` 非空 → LLM 调用失败（前端据此 show toast 提示配置）
/// - 两者皆空 → LLM 返回空内容（无错误）
#[tauri::command]
pub async fn get_startup_greeting(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    match brain.generate_startup_greeting().await {
        Some(greeting) => Ok(json!({ "greeting": greeting, "error": null })),
        None => {
            let error = brain.last_greeting_error().await;
            Ok(json!({ "greeting": "", "error": error }))
        }
    }
}
