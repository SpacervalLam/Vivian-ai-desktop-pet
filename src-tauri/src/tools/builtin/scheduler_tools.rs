//! 定时任务工具 - schedule_reminder / manage_scheduled
//!
//! 让 LLM 能管理定时任务：
//! - 定时提醒（Reminder）：到时发系统通知 + 对话气泡
//! - 任务管理（Manage）：列出/取消/暂停/恢复已创建的定时任务
//!
//! 时间参数支持两种格式：
//! - ISO 8601 日期时间：`2024-01-15T10:30:00`
//! - ISO 8601 持续时间：`PT2M`（2分钟后）、`PT2H30M`、`P1DT2H`、`PT30S`

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::brain::scheduler::{parse_time_spec, Priority, Scheduler, TaskType};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};

/// 获取全局 Scheduler（由 state.rs::initialize 注入到 todo_tools，这里复用）
fn get_scheduler() -> Option<std::sync::Arc<Scheduler>> {
    crate::tools::builtin::todo_tools::get_scheduler()
}

/// 把 Priority 转为可读字符串
fn priority_str(p: Priority) -> &'static str {
    match p {
        Priority::Low => "low",
        Priority::Normal => "normal",
        Priority::High => "high",
        Priority::Urgent => "urgent",
    }
}

/// 解析 priority 参数（1-4 数字或 low/normal/high/urgent 字符串）
fn parse_priority(v: Option<&Value>) -> Priority {
    match v {
        Some(Value::Number(n)) => match n.as_u64() {
            Some(0) => Priority::Low,
            Some(1) => Priority::Normal,
            Some(2) => Priority::High,
            Some(3) => Priority::Urgent,
            _ => Priority::Normal,
        },
        Some(Value::String(s)) => match s.to_lowercase().as_str() {
            "low" => Priority::Low,
            "high" => Priority::High,
            "urgent" => Priority::Urgent,
            _ => Priority::Normal,
        },
        _ => Priority::Normal,
    }
}

/// 把 ScheduledTask 序列化为前端友好的 JSON
fn task_to_json(task: &crate::brain::scheduler::ScheduledTask) -> Value {
    json!({
        "id": task.id,
        "task_type": match task.task_type {
            TaskType::Reminder => "reminder",
            TaskType::ToolCall => "tool_call",
        },
        "scheduled_time": task.scheduled_time,
        "message": task.message,
        "tool_name": task.tool_name,
        "tool_arguments": task.tool_arguments,
        "repeat_interval": task.repeat_interval,
        "status": format!("{:?}", task.status).to_lowercase(),
        "priority": priority_str(task.priority),
        "remaining_seconds": task.remaining_seconds(),
    })
}

/// emit scheduler:changed 事件
fn emit_scheduler_changed(action: &str, task_id: &str) {
    crate::tools::builtin::todo_tools::emit_scheduler_changed(action, task_id);
}

// ===== schedule_reminder 工具 =====

pub struct ScheduleReminderTool;

impl ScheduleReminderTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScheduleReminderTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ScheduleReminderTool {
    fn name(&self) -> &str {
        "schedule_reminder"
    }

    fn description(&self) -> &str {
        "Create a scheduled reminder. When the time arrives, Vivian will send a system desktop notification and proactively remind the user in the chat window.\n\n\
         Time format (time_spec parameter):\n\
         - ISO 8601 datetime: 2024-01-15T10:30:00\n\
         - ISO 8601 duration: PT2M (2 minutes later), PT2H30M (2 hours 30 minutes later), P1DT2H (1 day 2 hours later), PT30S (30 seconds later)\n\n\
         For repeating reminders use repeat_interval_seconds (in seconds), e.g. daily=86400, hourly=3600.\n\n\
         Examples:\n\
         - \"Remind me to drink water in 2 minutes\" -> time_spec=\"PT2M\", message=\"Time to drink water\"\n\
         - \"Remind me to check in at 9am every day\" -> time_spec=\"<today 9am>\", message=\"Check-in time\", repeat_interval_seconds=86400"
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "创建定时提醒。当时间到达时，Vivian 会发送系统桌面通知并在聊天窗口主动提醒用户。\n\n\
         时间格式（time_spec 参数）：\n\
         - ISO 8601 日期时间：2024-01-15T10:30:00\n\
         - ISO 8601 持续时间：PT2M（2分钟后）、PT2H30M（2小时30分钟后）、P1DT2H（1天2小时后）、PT30S（30秒后）\n\n\
         如需重复提醒使用 repeat_interval_seconds（单位秒），例如 daily=86400、hourly=3600。\n\n\
         示例：\n\
         - \"2分钟后提醒我喝水\" -> time_spec=\"PT2M\", message=\"该喝水了\"\n\
         - \"每天早上9点提醒我打卡\" -> time_spec=\"<今天9点>\", message=\"打卡时间\", repeat_interval_seconds=86400",
            "ja" => "スケジュールされたリマインダーを作成する。時間が来ると、Vivian はシステムデスクトップ通知を送信し、チャットウィンドウでユーザーに自発的にリマインドする。\n\n\
         時間形式（time_spec パラメータ）：\n\
         - ISO 8601 日時：2024-01-15T10:30:00\n\
         - ISO 8601 持続時間：PT2M（2分後）、PT2H30M（2時間30分後）、P1DT2H（1日2時間後）、PT30S（30秒後）\n\n\
         繰り返しリマインダーには repeat_interval_seconds（秒単位）を使用、例：daily=86400、hourly=3600。\n\n\
         例：\n\
         - \"2分後に水を飲むようリマインド\" -> time_spec=\"PT2M\", message=\"水を飲む時間です\"\n\
         - \"毎朝9時にチェックインをリマインド\" -> time_spec=\"<今日9時>\", message=\"チェックイン時間\", repeat_interval_seconds=86400",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "Reminder message content"},
                "time_spec": {"type": "string", "description": "Trigger time: ISO 8601 datetime (2024-01-15T10:30:00) or duration (PT2M/PT2H30M/P1DT2H/PT30S)"},
                "repeat_interval_seconds": {"type": "integer", "description": "Repeat interval in seconds. Omit = one-time reminder; 86400 = daily; 3600 = hourly"},
                "priority": {"type": "string", "description": "Priority: low/normal/high/urgent (default normal)", "enum": ["low", "normal", "high", "urgent"]}
            },
            "required": ["message", "time_spec"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string", "description": "提醒消息内容"},
                    "time_spec": {"type": "string", "description": "触发时间：ISO 8601 日期时间（2024-01-15T10:30:00）或持续时间（PT2M/PT2H30M/P1DT2H/PT30S）"},
                    "repeat_interval_seconds": {"type": "integer", "description": "重复间隔（秒）。省略=单次提醒；86400=每天；3600=每小时"},
                    "priority": {"type": "string", "description": "优先级：low/normal/high/urgent（默认 normal）", "enum": ["low", "normal", "high", "urgent"]}
                },
                "required": ["message", "time_spec"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string", "description": "リマインダーメッセージ内容"},
                    "time_spec": {"type": "string", "description": "トリガー時間：ISO 8601 日時（2024-01-15T10:30:00）または持続時間（PT2M/PT2H30M/P1DT2H/PT30S）"},
                    "repeat_interval_seconds": {"type": "integer", "description": "繰り返し間隔（秒）。省略=1回限り；86400=毎日；3600=毎時"},
                    "priority": {"type": "string", "description": "優先度：low/normal/high/urgent（デフォルト normal）", "enum": ["low", "normal", "high", "urgent"]}
                },
                "required": ["message", "time_spec"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let message = input.get("message").and_then(|v| v.as_str());
        let time_spec = input.get("time_spec").and_then(|v| v.as_str());

        match (message, time_spec) {
            (Some(m), Some(t)) if !m.is_empty() && !t.is_empty() => {
                // 预校验时间格式
                if let Err(e) = parse_time_spec(t) {
                    return ValidationResult::failure(
                        &format!("time_spec 格式错误：{}", e),
                        2,
                    );
                }
                // repeat_interval_seconds 最小值校验：防止 interval=0 导致死循环
                if let Some(interval) = input.get("repeat_interval_seconds").and_then(|v| v.as_u64()) {
                    if interval < 60 {
                        return ValidationResult::failure(
                            "repeat_interval_seconds 最小值为 60（1分钟）",
                            2,
                        );
                    }
                }
                ValidationResult::success(Some(input.clone()))
            }
            (None, _) => ValidationResult::failure("message 是必填项", 2),
            (_, None) => ValidationResult::failure("time_spec 是必填项", 2),
            _ => ValidationResult::failure("message 和 time_spec 不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let scheduler = match get_scheduler() {
            Some(s) => s,
            None => {
                return ToolResult::standard_error(
                    "调度器未初始化",
                    Some("SchedulerNotInitialized"),
                    None,
                );
            }
        };

        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let time_spec = args
            .get("time_spec")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let repeat = args
            .get("repeat_interval_seconds")
            .and_then(|v| v.as_u64());
        let priority = parse_priority(args.get("priority"));

        let mut scheduled_time = match parse_time_spec(&time_spec) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult::standard_error(
                    &format!("时间解析失败：{}", e),
                    Some("InvalidTimeSpec"),
                    None,
                );
            }
        };

        // 重复任务：若首次时间已过期，顺延到未来的正确时间点
        // 例如"每天9点"在下午3点创建 → 首次触发顺延到明天9点，而不是立即触发
        if let Some(interval) = repeat {
            let interval_f = interval as f64;
            let now = crate::brain::scheduler::now_ts_public();
            while scheduled_time <= now {
                scheduled_time += interval_f;
            }
        }

        let task_id = if let Some(interval) = repeat {
            let mut task =
                crate::brain::scheduler::ScheduledTask::new_reminder(message, scheduled_time);
            task.priority = priority;
            task.char_id = ctx.char_id.clone();
            scheduler.schedule_repeat(task, interval)
        } else {
            let mut task =
                crate::brain::scheduler::ScheduledTask::new_reminder(message, scheduled_time);
            task.priority = priority;
            task.char_id = ctx.char_id.clone();
            let id = task.id.clone();
            scheduler.insert_task_public(task);
            id
        };

        emit_scheduler_changed("added", &task_id);

        ToolResult::standard_success(
            "已创建定时提醒",
            Some(json!({
                "id": task_id,
                "scheduled_time": scheduled_time,
                "repeat_interval": repeat,
                "priority": priority_str(priority),
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "create scheduled reminder"
    }
}

// ===== manage_scheduled =====

pub struct ManageScheduledTool;

impl ManageScheduledTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ManageScheduledTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ManageScheduledTool {
    fn name(&self) -> &str {
        "manage_scheduled"
    }

    fn description(&self) -> &str {
        "Manage scheduled tasks. action: list/cancel/pause/resume. list: list all tasks (optional include_completed, default false); cancel/pause/resume: pass id to operate on a single task."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "管理定时任务。action：list/cancel/pause/resume。list：列出所有任务（可选 include_completed，默认 false）；cancel/pause/resume：传入 id 操作单个任务。",
            "ja" => "スケジュールされたタスクを管理する。action：list/cancel/pause/resume。list：すべてのタスクを一覧表示（任意 include_completed、デフォルト false）；cancel/pause/resume：id を渡して単一タスクを操作。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "cancel", "pause", "resume"], "description": "Management action"},
                "id": {"type": "string", "description": "Task ID (not required for list)"},
                "include_completed": {"type": "boolean", "description": "Whether to include completed/cancelled tasks (list only, default false)"}
            },
            "required": ["action"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "cancel", "pause", "resume"], "description": "管理操作"},
                    "id": {"type": "string", "description": "任务 ID（list 不需要）"},
                    "include_completed": {"type": "boolean", "description": "是否包含已完成/已取消的任务（仅 list，默认 false）"}
                },
                "required": ["action"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "cancel", "pause", "resume"], "description": "管理アクション"},
                    "id": {"type": "string", "description": "タスク ID（list には不要）"},
                    "include_completed": {"type": "boolean", "description": "完了/キャンセル済みタスクを含めるか（list のみ、デフォルト false）"}
                },
                "required": ["action"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let action = match input.get("action").and_then(|v| v.as_str()) {
            Some(a) if !a.is_empty() => a.to_string(),
            _ => return ValidationResult::failure("action 是必填项", 2),
        };
        if !matches!(action.as_str(), "list" | "cancel" | "pause" | "resume") {
            return ValidationResult::failure(
                "action 必须是 list/cancel/pause/resume 之一",
                2,
            );
        }
        // list 不需要 id；其他操作必须提供 id
        if action != "list" {
            match input.get("id").and_then(|v| v.as_str()) {
                Some(id) if !id.is_empty() => {}
                _ => return ValidationResult::failure("id 是必填项（list 除外）", 2),
            }
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if action == "cancel" {
            PermissionResult::ask("取消定时任务不可逆，需要用户确认")
        } else {
            PermissionResult::allow()
        }
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let scheduler = match get_scheduler() {
            Some(s) => s,
            None => {
                return ToolResult::standard_error(
                    "调度器未初始化",
                    Some("SchedulerNotInitialized"),
                    None,
                );
            }
        };

        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let task_id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");

        match action {
            "list" => {
                let include_completed = args
                    .get("include_completed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let all_tasks = scheduler.list_tasks();
                let filtered: Vec<Value> = all_tasks
                    .iter()
                    .filter(|t| {
                        if include_completed {
                            true
                        } else {
                            !matches!(
                                t.status,
                                crate::brain::scheduler::TaskStatus::Completed
                                    | crate::brain::scheduler::TaskStatus::Cancelled
                                    | crate::brain::scheduler::TaskStatus::Failed
                            )
                        }
                    })
                    .map(task_to_json)
                    .collect();

                ToolResult::standard_success(
                    &format!("共 {} 个定时任务", filtered.len()),
                    Some(json!({ "tasks": filtered, "count": filtered.len() })),
                )
            }
            "cancel" => {
                if scheduler.cancel_task(task_id) {
                    emit_scheduler_changed("cancelled", task_id);
                    ToolResult::standard_success("已取消定时任务", Some(json!({ "id": task_id })))
                } else {
                    ToolResult::standard_error(
                        &format!("未找到任务 {}", task_id),
                        Some("TaskNotFound"),
                        None,
                    )
                }
            }
            "pause" => {
                if scheduler.pause_task(task_id) {
                    emit_scheduler_changed("paused", task_id);
                    ToolResult::standard_success("已暂停定时任务", Some(json!({ "id": task_id })))
                } else {
                    ToolResult::standard_error(
                        &format!("暂停失败：任务 {} 不存在或非 pending 状态", task_id),
                        Some("TaskNotPausable"),
                        None,
                    )
                }
            }
            "resume" => {
                if scheduler.resume_task(task_id) {
                    emit_scheduler_changed("resumed", task_id);
                    ToolResult::standard_success("已恢复定时任务", Some(json!({ "id": task_id })))
                } else {
                    ToolResult::standard_error(
                        &format!("恢复失败：任务 {} 不存在或非 paused 状态", task_id),
                        Some("TaskNotResumable"),
                        None,
                    )
                }
            }
            _ => ToolResult::standard_error(
                "不支持的 action",
                Some("action 必须是 list/cancel/pause/resume"),
                None,
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "list cancel pause resume scheduled task"
    }
}
