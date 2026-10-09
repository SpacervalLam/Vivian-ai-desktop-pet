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
    event_time: Option<String>,
) -> Result<Value, String> {
    let item = todo_tools::add_todo_item(
        &title,
        description.as_deref().unwrap_or(""),
        priority.unwrap_or(1),
        due_date.as_deref(),
        event_time.as_deref(),
        // UI 窗口手动操作无角色归属，不弹 toast
        "",
    );
    Ok(json!({ "item": item }))
}

/// 更新待办（含 Scheduler 联动）
///
/// `due_date` / `event_time`: None=不修改，Some("")=清除，Some("value")=设置
#[tauri::command]
pub fn update_todo_item(
    id: String,
    title: Option<String>,
    description: Option<String>,
    priority: Option<u32>,
    due_date: Option<String>,
    event_time: Option<String>,
) -> Result<Value, String> {
    let item = todo_tools::update_todo_item(
        &id,
        title.as_deref(),
        description.as_deref(),
        priority,
        due_date.as_deref(),
        event_time.as_deref(),
        // UI 窗口手动操作无角色归属，不弹 toast
        "",
    )?;
    Ok(json!({ "item": item }))
}

/// 标记待办完成
#[tauri::command]
pub fn complete_todo_item(id: String) -> Result<Value, String> {
    let item = todo_tools::complete_todo_item(&id, "")?;
    Ok(json!({ "item": item }))
}

/// 删除待办
#[tauri::command]
pub fn delete_todo_item(id: String) -> Result<bool, String> {
    if todo_tools::delete_todo_item(&id, "") {
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
    important: Option<bool>,
    advance_minutes: Option<u32>,
) -> Result<Value, String> {
    let message = message.trim().to_string();
    let advance = advance_minutes.unwrap_or(0);
    let fire_at = scheduled_time - f64::from(advance) * 60.0;
    if message.is_empty() || message.chars().count() > 2000 || !scheduled_time.is_finite()
        || fire_at <= crate::brain::scheduler::now_ts_public() || advance > 60
        || repeat_interval.is_some_and(|interval| interval < 60) {
        return Err("请填写有效事项和未来的提醒时间；提前量不超过 60 分钟，重复间隔至少 60 秒。".into());
    }
    let mut task = crate::brain::scheduler::ScheduledTask::new_reminder(&message, fire_at);
    task.repeat_interval = repeat_interval;
    if important.unwrap_or(false) { task.priority = crate::brain::scheduler::Priority::Urgent; }
    task.metadata = json!({"advance_minutes":advance});
    let id = state.scheduler.insert_reminder_checked(task)?;
    // 手动 UI 创建的任务无角色归属（char_id 为空），前端据此不弹 toast
    let character_id = state
        .scheduler
        .get_task(&id)
        .map(|t| t.char_id.clone())
        .unwrap_or_default();
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
            "character_id": character_id,
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
        // 归属取任务自身 char_id（角色创建的提醒取消时弹回该角色）
        let character_id = state
            .scheduler
            .get_task(&id)
            .map(|t| t.char_id.clone())
            .unwrap_or_default();
        let _ = app.emit(
            "scheduler:changed",
            json!({
                "action": "cancelled",
                "task": { "id": id },
                "character_id": character_id,
            }),
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
        let character_id = state
            .scheduler
            .get_task(&id)
            .map(|t| t.char_id.clone())
            .unwrap_or_default();
        let _ = app.emit(
            "scheduler:changed",
            json!({
                "action": "paused",
                "task": { "id": id },
                "character_id": character_id,
            }),
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
        let character_id = state
            .scheduler
            .get_task(&id)
            .map(|t| t.char_id.clone())
            .unwrap_or_default();
        let _ = app.emit(
            "scheduler:changed",
            json!({
                "action": "resumed",
                "task": { "id": id },
                "character_id": character_id,
            }),
        );
    }
    Ok(ok)
}

/// An acknowledgement means accepted by a presentation surface, not read by the user.
#[tauri::command]
pub fn acknowledge_reminder_delivery(delivery_id: String) -> bool {
    crate::brain::reminder_delivery::acknowledge(&delivery_id)
}

#[tauri::command]
pub fn pending_reminder_notices(state: State<'_, Arc<AppState>>) -> Vec<crate::brain::reminder_delivery::ReminderNotice> {
    state.scheduler.pending_reminder_notices()
}

#[tauri::command]
pub fn acknowledge_reminder_notice(app: AppHandle, state: State<'_, Arc<AppState>>, notice_id: String, snooze: bool) -> Result<Option<String>, String> {
    let result = state.scheduler.acknowledge_reminder_notice(&notice_id, snooze)?;
    let _ = app.emit("reminder:changed", json!({"notice_id":notice_id}));
    let _ = app.emit("scheduler:changed", json!({"action":"notice_acknowledged","source":"manual"}));
    Ok(result)
}
