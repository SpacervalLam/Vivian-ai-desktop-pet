//! 待办事项工具 - add_todo / list_todo / complete_todo / update_todo / delete_todo
//!
//! 持久化到 `%APPDATA%\Vivian\todo\todos.json`。
//! 与 Scheduler 联动：设置 due_date 时自动创建定时提醒。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::brain::scheduler::Scheduler;
use crate::brain::scheduler::ScheduledTask;
use crate::brain::scheduler::TaskType;
use crate::tools::ToolSystem;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};
use crate::utils::path::get_user_data_dir;

/// 待办条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub completed: bool,
    #[serde(default = "default_priority")]
    pub priority: u32,
    pub created_at: i64,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub due_date: Option<String>,
    /// 关联的定时提醒任务 ID（Scheduler 联动）
    #[serde(default)]
    pub reminder_id: Option<String>,
}

fn default_priority() -> u32 {
    1
}

/// 待办列表持久化结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TodoFile {
    items: Vec<TodoItem>,
    saved_at: f64,
}

/// 全局待办列表
static TODO_LIST: Lazy<Arc<RwLock<TodoFile>>> =
    Lazy::new(|| Arc::new(RwLock::new(TodoFile::default())));

/// 全局 AppHandle（由 lib.rs setup 注入，用于 emit 事件给前端）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 全局 Scheduler（由 state.rs::initialize 注入，用于联动定时提醒）
static SCHEDULER: Lazy<RwLock<Option<Arc<Scheduler>>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 注入 Scheduler（state.rs::initialize 调用）
pub fn set_scheduler(scheduler: Arc<Scheduler>) {
    *SCHEDULER.write() = Some(scheduler);
}

/// 获取全局 Scheduler（供 scheduler_tools 复用）
pub fn get_scheduler() -> Option<Arc<Scheduler>> {
    SCHEDULER.read().clone()
}

/// emit scheduler:changed 事件（供 scheduler_tools 调用）
pub fn emit_scheduler_changed(action: &str, task_id: &str) {
    emit_event(
        "scheduler:changed",
        &json!({
            "action": action,
            "task_id": task_id,
            "source": "tool",
        }),
    );
}

fn todo_file_path() -> std::path::PathBuf {
    let dir = get_user_data_dir().join("todo");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("todos.json")
}

/// 从磁盘加载待办列表（若文件存在）
pub fn load_todo_list() {
    let path = todo_file_path();
    if !path.exists() {
        return;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if !content.trim().is_empty() {
            if let Ok(file) = serde_json::from_str::<TodoFile>(&content) {
                *TODO_LIST.write() = file;
            }
        }
    }
}

/// 清空所有待办（内存 + 持久化文件）
/// 供 factory_reset 调用：先取消所有关联的定时提醒，再清空内存列表，最后删除磁盘文件
pub fn clear_all_todos() {
    // 取消所有关联的 scheduler reminder，避免遗留孤儿任务
    let reminder_ids: Vec<String> = {
        let list = TODO_LIST.read();
        list.items
            .iter()
            .filter_map(|t| t.reminder_id.clone())
            .collect()
    };
    if let Some(scheduler) = get_scheduler() {
        for rid in reminder_ids {
            scheduler.remove_task(&rid);
        }
    }
    // 清空内存列表
    {
        let mut list = TODO_LIST.write();
        list.items.clear();
        list.saved_at = chrono::Local::now().timestamp_millis() as f64 / 1000.0;
    }
    // 删除磁盘文件
    let path = todo_file_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    tracing::info!("[todo] 已清空所有待办（factory_reset）");
}

fn save_todo_list() {
    // 持读锁直接序列化，避免克隆整个 TodoFile
    let (json, path) = {
        let file = TODO_LIST.read();
        let json = serde_json::to_string_pretty(&*file).unwrap_or_default();
        (json, todo_file_path())
    };
    if !json.is_empty() {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn now_ts() -> i64 {
    chrono::Local::now().timestamp()
}

fn gen_id() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

/// emit 事件给前端（若 AppHandle 已注入）
fn emit_event(event: &str, payload: &Value) {
    if let Some(handle) = APP_HANDLE.read().clone() {
        let _ = handle.emit(event, payload);
    }
}

/// 解析 due_date → Unix 时间戳
///
/// 支持三种格式（复用 Scheduler 的 parse_time_spec）：
/// - ISO 8601 日期时间: "2024-01-15T10:30:00", "2024-01-15T10:30"
/// - 纯日期: "2024-01-15"（默认当日 09:00）
/// - ISO 8601 持续时间: "PT2M"（2分钟后）, "PT2H30M"
///
/// 当解析后的时间已过时返回 None，避免入库后被 Scheduler 立即触发并标记 Completed。
fn parse_due_date(due_date: &str) -> Option<f64> {
    let ts = crate::brain::scheduler::parse_time_spec(due_date).ok()?;
    let now = chrono::Local::now().timestamp() as f64;
    if ts <= now {
        tracing::warn!(
            due_date,
            "[Todo] due_date 时间已过，跳过创建提醒"
        );
        return None;
    }
    Some(ts)
}

/// 为待办创建/更新定时提醒，返回 reminder_id
fn schedule_reminder_for(title: &str, due_date: &str) -> Option<String> {
    let ts = parse_due_date(due_date)?;
    let scheduler = SCHEDULER.read().clone()?;
    let msg = format!("待办提醒：{}", title);
    let rid = scheduler.schedule_reminder(msg.clone(), ts);
    // emit scheduler:changed 事件（action=added）
    emit_event(
        "scheduler:changed",
        &json!({
            "action": "added",
            "task": {
                "id": rid,
                "task_type": "reminder",
                "scheduled_time": ts,
                "message": msg,
            },
            "source": "todo",
        }),
    );
    Some(rid)
}

/// 取消关联的定时提醒（若存在）
fn cancel_reminder(reminder_id: &Option<String>) {
    if let Some(rid) = reminder_id {
        if let Some(scheduler) = SCHEDULER.read().clone() {
            scheduler.remove_task(rid);
        }
    }
}

/// emit todo:changed 事件
fn emit_todo_changed(action: &str, item: &TodoItem) {
    emit_event(
        "todo:changed",
        &json!({
            "action": action,
            "item": item,
        }),
    );
}

// ===== 公开 CRUD API（供 commands/todo.rs 调用）=====

/// 添加待办（含 Scheduler 联动）
pub fn add_todo_item(
    title: &str,
    description: &str,
    priority: u32,
    due_date: Option<&str>,
) -> TodoItem {
    let mut reminder_id = None;
    if let Some(dd) = due_date {
        if !dd.is_empty() {
            reminder_id = schedule_reminder_for(title, dd);
        }
    }

    let item = TodoItem {
        id: gen_id(),
        title: title.to_string(),
        description: description.to_string(),
        completed: false,
        priority: priority.clamp(1, 3),
        created_at: now_ts(),
        completed_at: None,
        due_date: due_date.filter(|s| !s.is_empty()).map(|s| s.to_string()),
        reminder_id,
    };

    {
        let mut list = TODO_LIST.write();
        list.items.push(item.clone());
        list.saved_at = chrono::Local::now().timestamp_millis() as f64;
    }
    save_todo_list();
    emit_todo_changed("added", &item);
    item
}

/// 更新待办（含 Scheduler 联动）
///
/// `due_date`: `None` = 不修改，`Some("")` = 清除，`Some("date")` = 设置
pub fn update_todo_item(
    id: &str,
    title: Option<&str>,
    description: Option<&str>,
    priority: Option<u32>,
    due_date: Option<&str>,
) -> Result<TodoItem, String> {
    let mut list = TODO_LIST.write();
    let found = list.items.iter_mut().find(|it| it.id == id);
    match found {
        Some(it) => {
            if let Some(t) = title {
                it.title = t.to_string();
            }
            if let Some(d) = description {
                it.description = d.to_string();
            }
            if let Some(p) = priority {
                it.priority = p.clamp(1, 3);
            }
            if let Some(dd) = due_date {
                let new_due = if dd.is_empty() {
                    None
                } else {
                    Some(dd.to_string())
                };
                let old_due = it.due_date.clone();
                if new_due != old_due {
                    // due_date 变更：取消旧 reminder
                    cancel_reminder(&it.reminder_id);
                    it.reminder_id = None;
                    // 创建新 reminder
                    if let Some(ref nd) = new_due {
                        it.reminder_id = schedule_reminder_for(&it.title, nd);
                    }
                    it.due_date = new_due;
                }
            }
            let updated = it.clone();
            drop(list);
            save_todo_list();
            emit_todo_changed("updated", &updated);
            Ok(updated)
        }
        None => Err(format!("未找到 id={}", id)),
    }
}

/// 标记待办完成（取消关联的 reminder）
pub fn complete_todo_item(id: &str) -> Result<TodoItem, String> {
    let mut list = TODO_LIST.write();
    let found = list.items.iter_mut().find(|it| it.id == id);
    match found {
        Some(it) => {
            it.completed = true;
            it.completed_at = Some(now_ts());
            cancel_reminder(&it.reminder_id);
            it.reminder_id = None;
            let updated = it.clone();
            drop(list);
            save_todo_list();
            emit_todo_changed("completed", &updated);
            Ok(updated)
        }
        None => Err(format!("未找到 id={}", id)),
    }
}

/// 删除待办（取消关联的 reminder）
pub fn delete_todo_item(id: &str) -> bool {
    let mut list = TODO_LIST.write();
    let before = list.items.len();
    let removed_item = list.items.iter().find(|it| it.id == id).cloned();
    list.items.retain(|it| it.id != id);
    let removed = before != list.items.len();
    drop(list);
    if removed {
        if let Some(item) = removed_item {
            cancel_reminder(&item.reminder_id);
        }
        save_todo_list();
        emit_event(
            "todo:changed",
            &json!({ "action": "deleted", "item": { "id": id } }),
        );
    }
    removed
}

/// 列出待办
pub fn list_todo_items(include_completed: bool, priority_filter: Option<u32>) -> Vec<TodoItem> {
    let list = TODO_LIST.read();
    list.items
        .iter()
        .filter(|it| {
            if !include_completed && it.completed {
                return false;
            }
            if let Some(p) = priority_filter {
                if it.priority != p {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// 预触发处理：在 `scheduled_time - 5s` 到 `scheduled_time` 之间发起一次主 LLM 调用。
///
/// 把定时任务内容说明作为 `user_input` 注入完整提示词链路（persona/memory/上下文），
/// 让 LLM 提前决定如何进行该定时任务。回复通过 `chat:assistant_message` emit 给前端。
///
/// 与 `handle_task_trigger`（到期触发，走桌面通知 + 标准气泡）分离：
/// - 预触发让 LLM 智能决定（可能调用工具、可能自然提醒、也可能判定不该打扰而简短回复）
/// - 到期触发保留原有 Reminder 行为作为兜底（系统通知 + 标准气泡），确保到点一定有反馈
///
/// `brain.think(directive, false)` 会复用完整提示词构造，并按正常对话流程更新
/// 记忆/对话历史/关系等运行时状态——预触发被视为智能体的一次真实交互。
pub async fn handle_task_pre_trigger(task: ScheduledTask, brain: crate::brain::Brain) {
    use chrono::TimeZone;

    let app_handle = match APP_HANDLE.read().clone() {
        Some(h) => h,
        None => {
            tracing::warn!(
                "[Scheduler] 预触发任务 {} 但 AppHandle 未注入，跳过",
                task.id
            );
            return;
        }
    };

    let task_message = task.message.as_deref().unwrap_or("(无内容)");
    let scheduled_ts = task.scheduled_time as i64;
    let scheduled_local = chrono::Local
        .timestamp_opt(scheduled_ts, 0)
        .single()
        .unwrap_or_else(chrono::Local::now);
    let time_str = scheduled_local.format("%Y-%m-%d %H:%M:%S").to_string();
    let remaining =
        (task.scheduled_time - crate::brain::scheduler::now_ts_public()).round() as i64;

    // 构造指令：作为 user_input 注入完整提示词，替换原本的用户消息部分
    let directive = format!(
        "[定时任务即将触发]\n任务内容：{}\n计划时间：{}\n剩余：{}秒\n\n\
         这是你设定的定时任务，马上就要到点触发。请基于你的角色设定、记忆和当前情境，\
         自然地决定如何进行这个定时任务：可以提醒用户、调用相关工具执行、或自然回复。\
         你的回复将作为对话气泡发送给用户。",
        task_message, time_str, remaining
    );

    tracing::info!(
        task_id = %task.id,
        char_id = %task.char_id,
        "[Scheduler] 预触发：发起主 LLM 调用让智能体决定如何进行定时任务"
    );

    // 调用 brain.think（非流式）：复用完整提示词构造，user_input 部分被替换为定时任务说明
    let result = brain.think(&directive, false).await;

    match result {
        Ok(ai_response) => {
            if !ai_response.text.is_empty() {
                let _ = app_handle.emit(
                    "chat:assistant_message",
                    json!({
                        "content": ai_response.text,
                        "timestamp": chrono::Local::now().to_rfc3339(),
                    }),
                );
            }
            tracing::info!(
                task_id = %task.id,
                text_len = ai_response.text.chars().count(),
                "[Scheduler] 预触发 LLM 调用完成，已发出回复"
            );
        }
        Err(e) => {
            tracing::warn!(
                task_id = %task.id,
                error = %e,
                "[Scheduler] 预触发 LLM 调用失败，到期时仍会走原有 Reminder 流程兜底"
            );
        }
    }
}

/// Complete handling logic when Scheduler task triggers (called by state.rs callback).
///
/// Dispatches by task_type:
/// - **Reminder**: Send system desktop notification + emit `chat:assistant_message` (chat bubble + history)
/// - **ToolCall**: Call ToolSystem to execute tool, emit result as chat bubble to frontend
///
/// Both types additionally emit `scheduler:changed` (action=triggered) to refresh frontend task list.
pub async fn handle_task_trigger(task: ScheduledTask, tool_system: Arc<ToolSystem>) {
    use tauri_plugin_notification::NotificationExt;

    let app_handle = match APP_HANDLE.read().clone() {
        Some(h) => h,
        None => {
            tracing::warn!(
                "[Scheduler] 任务 {} 触发但 AppHandle 未注入，无法呈现",
                task.id
            );
            return;
        }
    };

    match task.task_type {
        TaskType::Reminder => {
            let message = task.message.as_deref().unwrap_or("定时提醒");

            // 1. 系统桌面通知
            let _ = app_handle
                .notification()
                .builder()
                .title("Vivian 提醒")
                .body(message)
                .show();

            // 2. 对话气泡 + 聊天记录（前端 ChatWindow 监听 chat:assistant_message 自动追加）
            let _ = app_handle.emit(
                "chat:assistant_message",
                json!({
                    "content": message,
                    "timestamp": chrono::Local::now().to_rfc3339(),
                }),
            );

            // 3. 刷新前端任务列表
            emit_event(
                "scheduler:changed",
                &json!({ "action": "triggered", "task": task }),
            );
        }
        TaskType::ToolCall => {
            let tool_name = match task.tool_name.as_deref() {
                Some(n) => n.to_string(),
                None => {
                    tracing::warn!(
                        "[Scheduler] ToolCall 任务 {} 缺少 tool_name，跳过执行",
                        task.id
                    );
                    return;
                }
            };
            let args = task.tool_arguments.clone();

            // 走完整执行管线（沙箱/权限/确认），定时任务不再"可信跳过"。
            // 用任务记录的 char_id 构造上下文，使工具调用能路由回正确的角色窗口。
            let context = ToolUseContext::default().with_char_id(task.char_id.clone());
            let result =
                crate::tools::execute_tool_use(&tool_name, args, &tool_system, &context, None)
                    .await;

            let content = if result.success {
                let detail = result
                    .data
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_default())
                    .unwrap_or_default();
                if detail.is_empty() {
                    format!("定时任务已执行：{}", tool_name)
                } else {
                    format!("定时任务已执行 {}：{}", tool_name, detail)
                }
            } else {
                format!(
                    "定时任务 {} 执行失败：{}",
                    tool_name,
                    result.error.unwrap_or_else(|| "未知错误".to_string())
                )
            };

            let _ = app_handle.emit(
                "chat:assistant_message",
                json!({
                    "content": content,
                    "timestamp": chrono::Local::now().to_rfc3339(),
                }),
            );

            emit_event(
                "scheduler:changed",
                &json!({ "action": "triggered", "task": task }),
            );
        }
    }
}

// ===== add_todo 工具 =====

pub struct AddTodoTool;

impl AddTodoTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AddTodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AddTodoTool {
    fn name(&self) -> &str {
        "add_todo"
    }

    fn description(&self) -> &str {
        "Add a todo item. priority: 1(normal)/2(important)/3(urgent). due_date supports three formats: specific datetime (2024-01-15T10:30:00), date only (2024-01-15, defaults to 09:00), or duration (PT2M=2min later, PT2H=2hr later). A reminder is created at the specified time; if the time has already passed no reminder is created."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "添加待办事项。priority：1(普通)/2(重要)/3(紧急)。due_date 支持三种格式：具体时间（2024-01-15T10:30:00）、纯日期（2024-01-15，默认 09:00）、持续时间（PT2M=2分钟后，PT2H=2小时后）。会在指定时间创建提醒；若时间已过则不创建。",
            "ja" => "ToDoアイテムを追加する。priority：1(通常)/2(重要)/3(緊急)。due_date は3形式対応：特定時刻（2024-01-15T10:30:00）、日付のみ（2024-01-15、09:00）、継続時間（PT2M=2分後、PT2H=2時間後）。指定時刻にリマインダーを作成（時刻経過済みなら作成しない）。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Todo title"},
                "description": {"type": "string", "description": "Detailed description (optional)"},
                "priority": {"type": "integer", "description": "Priority 1-3", "minimum": 1, "maximum": 3, "default": 1},
                "due_date": {"type": "string", "description": "Due time. Supports: specific datetime (2024-01-15T10:30:00), date only (2024-01-15, defaults to 09:00), or duration (PT2M=2min, PT2H=2hr). Optional."}
            },
            "required": ["title"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "待办标题"},
                    "description": {"type": "string", "description": "详细描述（可选）"},
                    "priority": {"type": "integer", "description": "优先级 1-3", "minimum": 1, "maximum": 3, "default": 1},
                    "due_date": {"type": "string", "description": "截止时间。支持：具体时间（2024-01-15T10:30:00）、纯日期（2024-01-15，默认 09:00）、持续时间（PT2M=2分钟后，PT2H=2小时后）。可选。"}
                },
                "required": ["title"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "ToDoタイトル"},
                    "description": {"type": "string", "description": "詳細な説明（任意）"},
                    "priority": {"type": "integer", "description": "優先度 1-3", "minimum": 1, "maximum": 3, "default": 1},
                    "due_date": {"type": "string", "description": "期限時間。対応形式：特定時刻（2024-01-15T10:30:00）、日付のみ（2024-01-15、09:00）、継続時間（PT2M=2分後、PT2H=2時間後）。任意。"}
                },
                "required": ["title"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("title").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("title 是必填项且不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let priority = args.get("priority").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
        let due_date = args.get("due_date").and_then(|v| v.as_str()).map(|s| s.to_string());

        let item = add_todo_item(
            &title,
            &description,
            priority,
            due_date.as_deref(),
        );

        // due_date 已提供但 reminder_id 为 None：说明指定时间已过，提醒被跳过
        let message = if due_date.as_deref().is_some_and(|s| !s.is_empty())
            && item.reminder_id.is_none()
        {
            "待办已添加，但 due_date 指定的时间已过，未创建定时提醒。请用 update_todo 设置一个未来的时间。"
        } else {
            "已添加待办"
        };

        ToolResult::standard_success(
            message,
            Some(json!({ "id": item.id, "title": item.title, "priority": item.priority, "reminder_id": item.reminder_id })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 始终全量加载（核心工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "create todo task"
    }
}

// ===== list_todo =====

pub struct ListTodoTool;

impl ListTodoTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ListTodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ListTodoTool {
    fn name(&self) -> &str {
        "list_todo"
    }

    fn description(&self) -> &str {
        "List todo items. Optional include_completed (default false, only shows incomplete)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "列出待办事项。可选 include_completed（默认 false，仅显示未完成项）。",
            "ja" => "ToDoアイテムを一覧表示する。オプション include_completed（デフォルト false、未完了のみ表示）。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "include_completed": {"type": "boolean", "default": false},
                "priority_filter": {"type": "integer", "description": "Filter by priority (optional)"}
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "include_completed": {"type": "boolean", "default": false},
                    "priority_filter": {"type": "integer", "description": "按优先级过滤（可选）"}
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "include_completed": {"type": "boolean", "default": false},
                    "priority_filter": {"type": "integer", "description": "優先度で絞り込む（任意）"}
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let include_completed = args
            .get("include_completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let priority_filter = args.get("priority_filter").and_then(|v| v.as_u64()).map(|p| p as u32);

        let filtered = list_todo_items(include_completed, priority_filter);
        // 合并两次读锁为一次，避免重复加锁
        let (total, pending) = {
            let file = TODO_LIST.read();
            let total = file.items.len();
            let pending = file.items.iter().filter(|it| !it.completed).count();
            (total, pending)
        };
        let completed = total - pending;

        ToolResult::standard_success(
            &format!("共 {} 条（未完成 {}，已完成 {}）", filtered.len(), pending, completed),
            Some(json!({
                "items": filtered,
                "total": total,
                "pending": pending,
                "completed": completed,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 始终全量加载（核心工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "list todo tasks"
    }
}

// ===== complete_todo =====

pub struct CompleteTodoTool;

impl CompleteTodoTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CompleteTodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CompleteTodoTool {
    fn name(&self) -> &str {
        "complete_todo"
    }

    fn description(&self) -> &str {
        "Mark a todo as completed. Pass in id. Automatically cancels the associated scheduled reminder."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "将待办事项标记为已完成。传入 id。自动取消关联的定时提醒。",
            "ja" => "ToDoアイテムを完了としてマークする。id を渡す。関連付けられたスケジュール済みリマインダーを自動的にキャンセルする。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Todo ID"}
            },
            "required": ["id"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "待办 ID"}
                },
                "required": ["id"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "ToDo ID"}
                },
                "required": ["id"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("id").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("id 是必填项", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        match complete_todo_item(&id) {
            Ok(item) => ToolResult::standard_success(
                "已完成待办",
                Some(json!({ "id": id, "title": item.title })),
            ),
            Err(e) => ToolResult::standard_error("待办不存在", Some(&e), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }
}

// ===== manage_todo =====

pub struct ManageTodoTool;

impl ManageTodoTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ManageTodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ManageTodoTool {
    fn name(&self) -> &str {
        "manage_todo"
    }

    fn description(&self) -> &str {
        "Update or delete a todo item. action: update/delete. For update, pass title/description/priority/due_date to modify (modifying due_date synchronously updates the scheduled reminder). For delete, only id is needed; automatically cancels the associated scheduled reminder."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "更新或删除待办事项。action：update/delete。update 时传入 title/description/priority/due_date 进行修改（修改 due_date 会同步更新定时提醒）；delete 时只需 id，自动取消关联的定时提醒。",
            "ja" => "ToDoアイテムを更新または削除する。action：update/delete。update 時は title/description/priority/due_date を渡して変更（due_date の変更はスケジュールされたリマインダーを同期的に更新）。delete 時は id のみ必要、関連付けられたスケジュール済みリマインダーを自動的にキャンセル。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["update", "delete"], "description": "Action: update or delete"},
                "id": {"type": "string"},
                "title": {"type": "string", "description": "New title (update only)"},
                "description": {"type": "string", "description": "New description (update only)"},
                "priority": {"type": "integer", "minimum": 1, "maximum": 3, "description": "New priority (update only)"},
                "due_date": {"type": "string", "description": "New due time (update only). Supports: specific datetime (2024-01-15T10:30:00), date only (2024-01-15, defaults to 09:00), or duration (PT2M=2min, PT2H=2hr). Pass empty string to clear."}
            },
            "required": ["action", "id"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["update", "delete"], "description": "操作：update 或 delete"},
                    "id": {"type": "string"},
                    "title": {"type": "string", "description": "新标题（仅 update）"},
                    "description": {"type": "string", "description": "新描述（仅 update）"},
                    "priority": {"type": "integer", "minimum": 1, "maximum": 3, "description": "新优先级（仅 update）"},
                    "due_date": {"type": "string", "description": "新截止时间（仅 update）。支持：具体时间（2024-01-15T10:30:00）、纯日期（2024-01-15，默认 09:00）、持续时间（PT2M=2分钟后）。传空字符串清除。"}
                },
                "required": ["action", "id"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["update", "delete"], "description": "アクション：update または delete"},
                    "id": {"type": "string"},
                    "title": {"type": "string", "description": "新しいタイトル（update のみ）"},
                    "description": {"type": "string", "description": "新しい説明（update のみ）"},
                    "priority": {"type": "integer", "minimum": 1, "maximum": 3, "description": "新しい優先度（update のみ）"},
                    "due_date": {"type": "string", "description": "新しい期限時間（update のみ）。対応形式：特定時刻（2024-01-15T10:30:00）、日付のみ（2024-01-15、09:00）、継続時間（PT2M=2分後）。空文字列でクリア。"}
                },
                "required": ["action", "id"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let action = match input.get("action").and_then(|v| v.as_str()) {
            Some(a) if !a.is_empty() => a.to_string(),
            _ => return ValidationResult::failure("action 是必填项", 2),
        };
        if !matches!(action.as_str(), "update" | "delete") {
            return ValidationResult::failure("action 必须是 update 或 delete", 2);
        }
        match input.get("id").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("id 是必填项", 2),
        }
    }

    async fn check_permissions(&self, input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if action == "delete" {
            PermissionResult::ask("删除待办事项不可逆，需要用户确认")
        } else {
            PermissionResult::allow()
        }
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();

        match action {
            "update" => {
                let title = args.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
                let description = args.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
                let priority = args.get("priority").and_then(|v| v.as_u64()).map(|p| p as u32);
                let due_date = args.get("due_date").and_then(|v| v.as_str()).map(|s| s.to_string());

                match update_todo_item(
                    &id,
                    title.as_deref(),
                    description.as_deref(),
                    priority,
                    due_date.as_deref(),
                ) {
                    Ok(item) => ToolResult::standard_success(
                        "已更新待办",
                        Some(json!({ "id": item.id, "reminder_id": item.reminder_id })),
                    ),
                    Err(e) => ToolResult::standard_error("待办不存在", Some(&e), None),
                }
            }
            "delete" => {
                if delete_todo_item(&id) {
                    ToolResult::standard_success("已删除待办", Some(json!({ "id": id })))
                } else {
                    ToolResult::standard_error(
                        "待办不存在",
                        Some(&format!("未找到 id={}", id)),
                        None,
                    )
                }
            }
            _ => ToolResult::standard_error(
                "不支持的 action",
                Some("action 必须是 update 或 delete"),
                None,
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 始终全量加载（核心工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "update delete todo"
    }
}
