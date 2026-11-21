//! 值守 Loop 工具。
//!
//! 模型给自己（或用户）排一个周期性值守：到点由宿主唤醒一次主会话，其余时间不花钱。
//! 与 `schedule_wakeup` 的分工：那是一次性的「稍后再来」，这里是可重复、有配额的周期值守。

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::brain::work_loops::{global_work_loops, MIN_CADENCE_SECS};
use crate::tools::types::{PermissionResult, Tool, ToolResult, ToolUseContext, ValidationResult};

static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

fn emit_changed() {
    if let Some(handle) = APP_HANDLE.read().clone() {
        let loops: Vec<Value> = global_work_loops().list().iter().map(|loop_| loop_.summary()).collect();
        let _ = handle.emit("work_loop:changed", json!({ "loops": loops }));
    }
}

fn render_list(session_id: &str) -> String {
    let loops = global_work_loops().list_for(session_id);
    if loops.is_empty() {
        return "本会话还没有值守条目。".to_string();
    }
    let mut text = format!("本会话值守 {} 条：", loops.len());
    for loop_ in loops {
        text.push_str(&format!(
            "\n- [{}] {}（{}，已运行 {}/{}{}）{}",
            loop_.id,
            loop_.title,
            loop_.cadence_label(),
            loop_.runs,
            if loop_.max_runs == 0 { "不限".to_string() } else { loop_.max_runs.to_string() },
            if loop_.enabled { "" } else { "，已停用" },
            loop_.prompt.chars().take(80).collect::<String>()
        ));
    }
    text.push_str("\n值守只在应用运行时生效；到点由宿主唤醒本会话一次。");
    text
}

const DESC_ZH: &str = "管理周期性值守：到点由宿主唤醒本会话一次，其余时间不调用模型。\
action=list 查看；action=create 新建（prompt 必填，cadence_minutes 最短 1 分钟，可选 max_runs、ends_at、dedup_key）；\
action=update 按 id 改写；action=enable / disable 启停；action=remove 删除。\
同一个 dedup_key 在同一会话内幂等：重复创建会更新原条目而不是新增。\
只在用户明确要求「定期/每天/隔一会儿再看看」时创建；不要用它替代一次性提醒（用 schedule_reminder）\
或一次性「稍后再来」（用 schedule_wakeup）。值守内容要写成能独立执行的一轮任务，不要依赖你现在的上下文。";

const DESC_EN: &str = "Manage recurring watch loops: when due, the host wakes this session once; other times cost no model calls. \
action=list; action=create (prompt required, cadence_minutes at least 1, optional max_runs / ends_at / dedup_key); \
action=update by id; action=enable / disable; action=remove. A dedup_key is idempotent per session: creating again updates the \
existing loop instead of adding another. Create loops only when the user explicitly asks for recurring checks. Write the prompt \
as a self-contained round of work that does not depend on your current context.";

#[async_trait]
impl Tool for ManageWorkLoopTool {
    fn name(&self) -> &str {
        "manage_work_loop"
    }

    fn description(&self) -> &str {
        DESC_EN
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => DESC_ZH,
            _ => DESC_EN,
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "定时值守\n每隔一会儿看看\n每天检查一次",
            "en" => "recurring watch loop\ncheck again every so often\ndaily check",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "create", "update", "enable", "disable", "remove"],
                    "description": "list / create / update / enable / disable / remove"
                },
                "id": { "type": "string", "description": "Loop id such as loop_2. Required for update / enable / disable / remove." },
                "title": { "type": "string", "description": "Short label shown in the panel. Defaults to the prompt." },
                "prompt": { "type": "string", "description": "One self-contained round of work. Required for create and update." },
                "cadence_minutes": { "type": "integer", "minimum": 1, "description": "Interval in minutes (minimum 1)." },
                "max_runs": { "type": "integer", "minimum": 0, "description": "Run quota; 0 means unlimited." },
                "ends_at": { "type": "integer", "minimum": 0, "description": "Stop after this Unix timestamp (seconds)." },
                "dedup_key": { "type": "string", "description": "Idempotency key within this session." }
            },
            "required": ["action"]
        })
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "list" => ValidationResult::success(Some(input.clone())),
            "create" | "update" => {
                if action == "update" && input.get("id").and_then(Value::as_str).is_none() {
                    return ValidationResult::failure("update 需要 id", 2);
                }
                if input.get("prompt").and_then(Value::as_str).map(str::trim).is_none_or(str::is_empty) {
                    return ValidationResult::failure("prompt 不能为空（写成能独立执行的一轮任务）", 2);
                }
                if input.get("cadence_minutes").and_then(Value::as_u64).is_none_or(|value| value < 1) {
                    return ValidationResult::failure("cadence_minutes 必须是 >= 1 的整数", 2);
                }
                ValidationResult::success(Some(input.clone()))
            }
            "enable" | "disable" | "remove" => {
                if input.get("id").and_then(Value::as_str).is_none() {
                    ValidationResult::failure("该动作需要 id", 2)
                } else {
                    ValidationResult::success(Some(input.clone()))
                }
            }
            _ => ValidationResult::failure("action 必须是 list / create / update / enable / disable / remove", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let store = global_work_loops();
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        let session_id = context.session_id.as_str();
        match action {
            "list" => {
                let loops: Vec<Value> = store.list_for(session_id).iter().map(|loop_| loop_.summary()).collect();
                ToolResult::standard_success(&render_list(session_id), Some(json!({ "loops": loops })))
            }
            "create" | "update" => {
                let cadence_minutes = args.get("cadence_minutes").and_then(Value::as_u64).unwrap_or(0);
                let cadence_secs = cadence_minutes.saturating_mul(60).max(MIN_CADENCE_SECS);
                let result = store.upsert(
                    session_id,
                    args.get("id").and_then(Value::as_str),
                    args.get("title").and_then(Value::as_str).unwrap_or(""),
                    args.get("prompt").and_then(Value::as_str).unwrap_or(""),
                    cadence_secs,
                    args.get("max_runs").and_then(Value::as_u64).unwrap_or(0) as u32,
                    args.get("ends_at").and_then(Value::as_i64),
                    args.get("dedup_key").and_then(Value::as_str).unwrap_or(""),
                );
                match result {
                    Ok(loop_) => {
                        emit_changed();
                        ToolResult::standard_success(
                            &format!(
                                "已安排值守 [{}]「{}」：{}，下一次约在 {} 分钟后。",
                                loop_.id,
                                loop_.title,
                                loop_.cadence_label(),
                                loop_.next_run_at.saturating_sub(crate::brain::work_loops::now_secs()) / 60
                            ),
                            Some(json!({ "loop": loop_.summary() })),
                        )
                    }
                    Err(error) => ToolResult::standard_error(&error, Some("WorkLoopError"), None),
                }
            }
            "enable" | "disable" => {
                let id = args.get("id").and_then(Value::as_str).unwrap_or("");
                match store.set_enabled(id, action == "enable") {
                    Ok(()) => {
                        emit_changed();
                        ToolResult::standard_success(
                            &format!("值守 [{id}] 已{}", if action == "enable" { "启用" } else { "停用" }),
                            None,
                        )
                    }
                    Err(error) => ToolResult::standard_error(&error, Some("WorkLoopError"), None),
                }
            }
            "remove" => {
                let id = args.get("id").and_then(Value::as_str).unwrap_or("");
                match store.remove(id) {
                    Ok(true) => {
                        emit_changed();
                        ToolResult::standard_success(&format!("已删除值守 [{id}]"), None)
                    }
                    Ok(false) => ToolResult::standard_error(&format!("找不到值守 {id}"), Some("WorkLoopError"), None),
                    Err(error) => ToolResult::standard_error(&error, Some("WorkLoopError"), None),
                }
            }
            _ => ToolResult::standard_error("未知 action", Some("WorkLoopError"), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> crate::tools::types::ToolCategory {
        crate::tools::types::ToolCategory::System
    }
}

/// 管理值守条目。
pub struct ManageWorkLoopTool;

impl ManageWorkLoopTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ManageWorkLoopTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 供命令面复用的事件广播。
pub fn broadcast_loops(app: &AppHandle) {
    let loops: Vec<Value> = global_work_loops().list().iter().map(|loop_| loop_.summary()).collect();
    let _ = app.emit("work_loop:changed", json!({ "loops": loops }));
}
