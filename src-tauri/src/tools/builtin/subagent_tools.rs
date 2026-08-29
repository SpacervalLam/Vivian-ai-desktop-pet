//! 子代理工具 — 子任务循环内向父级回传报告 + 委派/控制。
//!
//! - `subagent_report`：子任务循环内向父级回传报告（写入任务记录）。
//! - `spawn_subagent`：主智能体委派一个子任务（后台 agent 循环，挂到父任务谱系）。
//! - `subagent_control`：查询/取消/延续/读取子代理任务。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::brain::TaskService;
use crate::providers::router::ModelRouter;
use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};
use crate::tools::ToolSystem;

static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 取出子代理委派所需的共享服务（router / tool_system / task_service）。
fn services() -> Result<(Arc<ModelRouter>, Arc<ToolSystem>, Arc<TaskService>), String> {
    let Some(app) = APP_HANDLE.read().clone() else {
        return Err("后端未初始化".to_string());
    };
    let state = app.state::<Arc<AppState>>();
    let router = state
        .model_router
        .read()
        .clone()
        .map(Arc::new)
        .ok_or_else(|| "模型路由未就绪".to_string())?;
    Ok((router, state.tool_system.clone(), state.task_service.clone()))
}

/// subagent_report 工具：子任务把工作结果浓缩为报告写回任务状态。
///
/// 归属任务经工具上下文的 session_id（子任务循环启动时注入的任务 ID）识别。
pub struct SubagentReportTool;

impl SubagentReportTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SubagentReportTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SubagentReportTool {
    fn name(&self) -> &str {
        "subagent_report"
    }

    fn description(&self) -> &str {
        "Report your work results back to the parent agent. Call this when your subtask is done (or at a meaningful milestone) with a concise summary of what was done, key findings, and outcomes. The report is attached to your task record and made available to the parent/caller."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "把工作结果报告回父代理。当你的子任务完成（或到达有意义的里程碑）时调用，附上做了什么、关键发现与结果的简明摘要。报告会挂接到你的任务记录，供父级/调用方查阅。",
            "ja" => "作業結果を親エージェントに報告する。サブタスク完了（または意味のあるマイルストーン）時に呼び出し、実施内容・主要な発見・結果の簡潔なサマリーを添える。レポートはタスク記録に紐付けられ、親/呼び出し元が参照できる。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "report": {"type": "string", "description": "Concise summary: what was done, key findings, outcomes"}
            },
            "required": ["report"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "report": {"type": "string", "description": "简明摘要：做了什么、关键发现、结果"}
                },
                "required": ["report"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "report": {"type": "string", "description": "簡潔なサマリー：実施内容・主要な発見・結果"}
                },
                "required": ["report"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("report").and_then(|v| v.as_str()) {
            Some(r) if !r.is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("report 是必填项且不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let report = args
            .get("report")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let Some(app) = APP_HANDLE.read().clone() else {
            return ToolResult::standard_error("无法回传报告（后端未初始化）", Some("ReportUnavailable"), None);
        };
        let state = app.state::<Arc<AppState>>();
        let task_service = state.task_service.clone();

        // 归属任务：优先显式 task_id 参数，否则用上下文 session_id（子任务循环注入）
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| context.session_id.clone());

        if task_id.is_empty() {
            return ToolResult::standard_error(
                "无法确定归属任务（不在子任务上下文中，且未传 task_id）",
                Some("NoTaskContext"),
                None,
            );
        }
        if task_service.set_report(&task_id, report.clone()) {
            ToolResult::standard_success("报告已回传", Some(json!({ "task_id": task_id })))
        } else {
            ToolResult::standard_error(
                &format!("任务 {task_id} 不存在，报告未挂接"),
                Some("TaskNotFound"),
                None,
            )
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "subagent report results parent"
    }
}

// ===== spawn_subagent =====

/// 子代理委派工具：主智能体把子任务交给独立的后台 agent 循环执行。
///
/// 返回任务 ID（`task-*`）。子任务以调用者 char_id 归类，可挂到显式传入的
/// `parent_task_id`（或工具上下文 session_id 指向的任务）名下，形成谱系链。
/// 子任务完成后回传的报告可用 `subagent_control`（action=report）读取。
pub struct SpawnSubagentTool;

impl SpawnSubagentTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SpawnSubagentTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SpawnSubagentTool {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        "Delegate a subtask to a subagent running as an independent background agent loop. Returns immediately with a task_id; the subagent works autonomously (with its own tool loop) and can report back. Use subagent_control (action=report) to read its result when done. Use for work that can run independently in the background while you continue."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "把一个子任务委派给独立的后台智能体循环执行。立即返回 task_id；子代理自主工作（有独立工具循环），完成后可回传报告。用 subagent_control（action=report）读取结果。适合可后台独立并行、无需你持续干预的工作。",
            "ja" => "サブタスクを独立したバックグラウンドエージェントループに委任する。即座に task_id を返す。サブエージェントは自律的に作業し（独自のツールループ）、完了時に報告できる。結果は subagent_control（action=report）で取得。バックグラウンドで独立実行できる作業に適している。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "directive": {"type": "string", "description": "The task/goal for the subagent to accomplish (concise, self-contained)"},
                "parent_task_id": {"type": "string", "description": "Optional task id to attach this subtask to (lineage). Defaults to the current task context."}
            },
            "required": ["directive"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "directive": {"type": "string", "description": "交给子代理完成的任务/目标（简明、自包含）"},
                    "parent_task_id": {"type": "string", "description": "可选：把本子任务挂到该任务名下（谱系）。缺省用当前任务上下文。"}
                },
                "required": ["directive"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "directive": {"type": "string", "description": "サブエージェントに達成させるタスク/目標（簡潔で自己完結）"},
                    "parent_task_id": {"type": "string", "description": "任意：このサブタスクを紐付けるタスク ID（譜系）。省略時は現在のタスクコンテキスト。"}
                },
                "required": ["directive"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("directive").and_then(|v| v.as_str()) {
            Some(d) if !d.trim().is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("directive 是必填项且不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::ask("委派子代理会在后台消耗模型资源并执行工具，需要确认")
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let directive = args
            .get("directive")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if directive.is_empty() {
            return ToolResult::standard_error("directive 是必填项且不能为空", Some("InvalidInput"), None);
        }
        let (router, tool_system, task_service) = match services() {
            Ok(s) => s,
            Err(e) => return ToolResult::standard_error(&e, Some("ServiceUnavailable"), None),
        };
        if context.char_id.is_empty() {
            return ToolResult::standard_error("无法确定归属角色（不在角色上下文中）", Some("NoCharContext"), None);
        }
        // 归属父任务：优先显式 parent_task_id，否则用上下文 session_id（子任务循环注入）
        let parent = args
            .get("parent_task_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                let sid = context.session_id.trim();
                if sid.starts_with("task-") {
                    Some(sid.to_string())
                } else {
                    None
                }
            });

        let task_id = task_service.start_with_parent(
            context.char_id.clone(),
            router,
            tool_system,
            directive,
            parent,
        );
        ToolResult::standard_success(
            &format!("子代理任务已启动：{task_id}"),
            Some(json!({ "task_id": task_id })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Shell
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "subagent spawn delegate background task lineage parent"
    }
}

// ===== subagent_control =====

/// 子代理控制工具：查询/取消/延续/读取子代理任务。
///
/// action：
/// - `list`：列出当前角色的全部任务摘要（含谱系字段）
/// - `get`：按 task_id 查单个任务 + 其全部后代
/// - `cancel`：按 task_id 取消运行中的任务
/// - `followup`：对已结束任务追加新指令继续执行（需 additional_directive）
/// - `report`：读取任务最终报告（子代理回传文本）
pub struct SubagentControlTool;

impl SubagentControlTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SubagentControlTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SubagentControlTool {
    fn name(&self) -> &str {
        "subagent_control"
    }

    fn description(&self) -> &str {
        "Manage subagent tasks. action: list (all tasks for the current role), get (one task + its descendant lineage by task_id), cancel (stop a running task), followup (append a directive and resume a finished task, requires additional_directive), report (read a subagent's final report by task_id)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "管理子代理任务。action：list（列出当前角色全部任务）、get（按 task_id 查单个任务及其后代谱系）、cancel（取消运行中的任务）、followup（追加指令并恢复已结束任务，需 additional_directive）、report（按 task_id 读取子代理最终报告）。",
            "ja" => "サブエージェントタスクを管理する。action：list（現在のロールの全タスク）、get（task_id で1件+子孫譜系を取得）、cancel（実行中タスクをキャンセル）、followup（終了タスクに指示を追加して再開、additional_directive が必要）、report（task_id でサブエージェントの最終レポートを取得）。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "get", "cancel", "followup", "report"], "description": "Operation: list / get / cancel / followup / report"},
                "task_id": {"type": "string", "description": "Task ID (required for get / cancel / followup / report)"},
                "additional_directive": {"type": "string", "description": "New instructions to append (required for followup)"}
            },
            "required": ["action"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "get", "cancel", "followup", "report"], "description": "操作：list / get / cancel / followup / report"},
                    "task_id": {"type": "string", "description": "任务 ID（get / cancel / followup / report 必填）"},
                    "additional_directive": {"type": "string", "description": "追加的指令（followup 必填）"}
                },
                "required": ["action"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "get", "cancel", "followup", "report"], "description": "操作：list / get / cancel / followup / report"},
                    "task_id": {"type": "string", "description": "タスク ID（get / cancel / followup / report で必須）"},
                    "additional_directive": {"type": "string", "description": "追加の指示（followup で必須）"}
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
        if !matches!(
            action.as_str(),
            "list" | "get" | "cancel" | "followup" | "report"
        ) {
            return ValidationResult::failure("action 必须是 list / get / cancel / followup / report", 2);
        }
        if action != "list"
            && input.get("task_id").and_then(|v| v.as_str()).map_or(true, |s| s.is_empty())
        {
            return ValidationResult::failure("get / cancel / followup / report 需要 task_id", 2);
        }
        if action == "followup"
            && input
                .get("additional_directive")
                .and_then(|v| v.as_str())
                .map_or(true, |s| s.trim().is_empty())
        {
            return ValidationResult::failure("followup 需要 additional_directive", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        match input.get("action").and_then(|v| v.as_str()).unwrap_or("") {
            "cancel" => PermissionResult::ask("取消子代理任务会中断其执行，需要确认"),
            "followup" => PermissionResult::ask("延续子代理任务会再次消耗模型资源，需要确认"),
            _ => PermissionResult::allow(),
        }
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let task_id = args.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
        let (router, tool_system, task_service) = match services() {
            Ok(s) => s,
            Err(e) => return ToolResult::standard_error(&e, Some("ServiceUnavailable"), None),
        };

        match action {
            "list" => {
                let tasks = task_service.summaries_for(&context.char_id);
                let arr: Vec<Value> = tasks.iter().map(|t| serde_json::to_value(t).unwrap_or(Value::Null)).collect();
                ToolResult::standard_success(
                    &format!("共 {} 个任务", tasks.len()),
                    Some(json!({ "tasks": arr })),
                )
            }
            "get" => {
                let Some(summary) = task_service.summary_of(task_id) else {
                    return ToolResult::standard_error("任务不存在", Some(&format!("未找到 task_id={task_id}")), None);
                };
                let descendants: Vec<Value> = task_service
                    .descendants_of(task_id)
                    .iter()
                    .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
                    .collect();
                ToolResult::standard_success(
                    &format!("任务 {task_id} 状态：{}", summary.status),
                    Some(json!({
                        "task": serde_json::to_value(&summary).unwrap_or(Value::Null),
                        "descendants": descendants,
                    })),
                )
            }
            "cancel" => {
                if task_service.cancel(task_id) {
                    ToolResult::standard_success(&format!("任务 {task_id} 已取消"), Some(json!({ "task_id": task_id })))
                } else {
                    ToolResult::standard_error(
                        "任务不存在或未在运行",
                        Some(&format!("未找到运行中的 task_id={task_id}")),
                        None,
                    )
                }
            }
            "followup" => {
                let additional = args
                    .get("additional_directive")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match task_service.followup(task_id, &additional, router, tool_system) {
                    Ok(()) => ToolResult::standard_success(
                        &format!("任务 {task_id} 已延续执行"),
                        Some(json!({ "task_id": task_id })),
                    ),
                    Err(e) => ToolResult::standard_error(&e, Some("FollowupFailed"), None),
                }
            }
            "report" => match task_service.report_of(task_id) {
                Some(r) => ToolResult::standard_success(
                    &format!("任务 {task_id} 报告：{r}"),
                    Some(json!({ "task_id": task_id, "report": r })),
                ),
                None => ToolResult::standard_error(
                    "任务不存在或尚无报告",
                    Some(&format!("未找到 task_id={task_id} 的报告")),
                    None,
                ),
            },
            _ => ToolResult::standard_error("不支持的 action", Some("action 必须是 list / get / cancel / followup / report"), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Shell
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "subagent control list get cancel followup report"
    }
}
