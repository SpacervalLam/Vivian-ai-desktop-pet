//! 后台任务（子代理/自治任务）查询与控制的 Tauri 命令
//!
//! 供心智观察器「任务」页展示任务列表、谱系与运行状态，
//! 并支持取消运行中的任务。

use std::sync::Arc;

use tauri::State;

use crate::brain::task_service::TaskSummary;
use crate::state::AppState;

/// 任务列表（char_id 为空时返回全部角色；否则仅该角色）。
#[tauri::command]
pub fn list_agent_tasks(
    state: State<'_, Arc<AppState>>,
    char_id: String,
) -> Vec<TaskSummary> {
    let ts = &state.task_service;
    if char_id.is_empty() {
        ts.all_summaries()
    } else {
        ts.summaries_for(&char_id)
    }
}

/// 单个任务详情（含后代谱系）。
#[tauri::command]
pub fn get_agent_task(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    let ts = &state.task_service;
    let task = ts.summary_of(&task_id).ok_or("任务不存在")?;
    let descendants: Vec<TaskSummary> = ts.descendants_of(&task_id);
    Ok(serde_json::json!({ "task": task, "descendants": descendants }))
}

/// 取消任务（仅运行中可取消）。
#[tauri::command]
pub fn cancel_agent_task(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> Result<bool, String> {
    Ok(state.task_service.cancel(&task_id))
}
