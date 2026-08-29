//! 待办与定时任务管理命令 - 供专属 UI 窗口调用

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;
use crate::tools::builtin::todo_tools;

/// 列出待办事项
#[tauri::command]
pub fn list_todos(include_completed: Option<bool>) -> Result<Value, String> {
    let include = include_completed.unwrap_or(false);
    let items = todo_tools::list_todo_items(include, None);
    Ok(json!({ "items": items, "total": items.len() }))
}

/// 添加待办（含 Scheduler 联动）
#[tauri::command]
pub fn add_todo_item(
    title: String,
    description: Option<String>,
    priority: Option<u32>,
    due_date: Option<String>,
) -> Result<Value, String> {
    let item = todo_tools::add_todo_item(
        &title,
        description.as_deref().unwrap_or(""),
        priority.unwrap_or(1),
        due_date.as_deref(),
    );
    Ok(json!({ "item": item }))
}

/// 更新待办（含 Scheduler 联动）
///
/// `due_date`: None=不修改，Some("")=清除，Some("YYYY-MM-DD")=设置
#[tauri::command]
pub fn update_todo_item(
    id: String,
    title: Option<String>,
    description: Option<String>,
    priority: Option<u32>,
    due_date: Option<String>,
) -> Result<Value, String> {
    let item = todo_tools::update_todo_item(
        &id,
        title.as_deref(),
        description.as_deref(),
        priority,
        due_date.as_deref(),
    )?;
    Ok(json!({ "item": item }))
}

/// 标记待办完成
#[tauri::command]
pub fn complete_todo_item(id: String) -> Result<Value, String> {
    let item = todo_tools::complete_todo_item(&id)?;
    Ok(json!({ "item": item }))
}

/// 删除待办
#[tauri::command]
pub fn delete_todo_item(id: String) -> Result<bool, String> {
    if todo_tools::delete_todo_item(&id) {
        Ok(true)
    } else {
        Err("待办不存在".to_string())
    }
}

/// 列出所有定时任务
#[tauri::command]
pub fn list_scheduled_tasks(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let tasks = state.scheduler.list_tasks();
    Ok(json!({ "tasks": tasks, "total": tasks.len() }))
}

/// 添加定时提醒（手动创建，非待办联动）
#[tauri::command]
pub fn add_scheduled_reminder(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    message: String,
    scheduled_time: f64,
    repeat_interval: Option<u64>,
) -> Result<Value, String> {
    let id = if let Some(interval) = repeat_interval {
        let task = crate::brain::scheduler::ScheduledTask::new_reminder(&message, scheduled_time);
        state.scheduler.schedule_repeat(task, interval)
    } else {
        state.scheduler.schedule_reminder(&message, scheduled_time)
    };
    let _ = app.emit(
        "scheduler:changed",
        json!({
            "action": "added",
            "task": {
                "id": id,
                "task_type": "reminder",
                "scheduled_time": scheduled_time,
                "message": message,
                "repeat_interval": repeat_interval,
            },
            "source": "manual",
        }),
    );
    Ok(json!({ "id": id }))
}

/// 取消定时任务
#[tauri::command]
pub fn cancel_scheduled_task(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<bool, String> {
    let ok = state.scheduler.cancel_task(&id);
    if ok {
        let _ = app.emit(
            "scheduler:changed",
            json!({ "action": "cancelled", "task": { "id": id } }),
        );
    }
    Ok(ok)
}

/// 暂停定时任务
#[tauri::command]
pub fn pause_scheduled_task(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<bool, String> {
    let ok = state.scheduler.pause_task(&id);
    if ok {
        let _ = app.emit(
            "scheduler:changed",
            json!({ "action": "paused", "task": { "id": id } }),
        );
    }
    Ok(ok)
}

/// 恢复定时任务
#[tauri::command]
pub fn resume_scheduled_task(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<bool, String> {
    let ok = state.scheduler.resume_task(&id);
    if ok {
        let _ = app.emit(
            "scheduler:changed",
            json!({ "action": "resumed", "task": { "id": id } }),
        );
    }
    Ok(ok)
}
