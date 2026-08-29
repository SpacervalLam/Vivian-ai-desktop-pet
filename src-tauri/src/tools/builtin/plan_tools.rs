//! 计划模式工具 — 让 LLM 在动手前产出计划并等待用户批准。
//!
//! - `plan_task`：创建一份待批准计划（objective + 步骤列表），广播 `plan:created`
//!   事件给前端弹确认框；用户批准后（`plan_decision` 命令）计划进入 Approved，
//!   模型可继续执行。用户否决则计划 Rejected，模型应停止。

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::brain::plan_mode::{PlanService, PlanStep};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 全局 AppHandle（lib.rs setup 注入）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 全局 PlanService 单例。
fn service() -> &'static PlanService {
    use once_cell::sync::Lazy;
    static SVC: Lazy<PlanService> = Lazy::new(|| (*PlanService::new()).clone());
    &SVC
}

/// plan_task 工具：产出计划并等待用户批准。
pub struct PlanTaskTool;

impl PlanTaskTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlanTaskTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PlanTaskTool {
    fn name(&self) -> &str {
        "plan_task"
    }

    fn description(&self) -> &str {
        "Before executing a complex or multi-step task, produce a plan (objective + ordered steps) and wait for user approval. The user reviews and approves/rejects. Only proceed with execution after approval. Use for risky, multi-file, or destructive operations."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在执行复杂或多步骤任务前，先产出一份计划（目标 + 有序步骤）并等待用户批准。用户审阅后批准/否决。只有批准后才继续执行。用于高风险、多文件或破坏性操作。",
            "ja" => "複雑または多段階のタスクを実行する前に、計画（目標＋順序付きステップ）を作成し、ユーザーの承認を待つ。ユーザーが確認して承認/却下する。承認後にのみ実行を続行する。リスクが高い、多ファイル、または破壊的な操作に使用。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "objective": {"type": "string", "description": "What the plan aims to accomplish"},
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "description": {"type": "string", "description": "What this step does"},
                            "tool_hint": {"type": "string", "description": "Optional hint of the tool to use (e.g. read_file, edit_file)"}
                        },
                        "required": ["description"]
                    }
                }
            },
            "required": ["objective", "steps"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "objective": {"type": "string", "description": "计划要达成的目标"},
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "description": {"type": "string", "description": "这一步做什么"},
                                "tool_hint": {"type": "string", "description": "可选的工具提示（如 read_file、edit_file）"}
                            },
                            "required": ["description"]
                        }
                    }
                },
                "required": ["objective", "steps"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "objective": {"type": "string", "description": "計画が達成しようとする目標"},
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "description": {"type": "string", "description": "このステップの内容"},
                                "tool_hint": {"type": "string", "description": "使用ツールの任意ヒント（例：read_file、edit_file）"}
                            },
                            "required": ["description"]
                        }
                    }
                },
                "required": ["objective", "steps"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let objective_ok = input
            .get("objective")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        let steps_ok = input
            .get("steps")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
        if !objective_ok {
            return ValidationResult::failure("objective 是必填项且不能为空", 2);
        }
        if !steps_ok {
            return ValidationResult::failure("steps 是必填项且至少一个步骤", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let objective = args
            .get("objective")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let steps: Vec<PlanStep> = args
            .get("steps")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, s)| PlanStep {
                index: i + 1,
                description: s
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                tool_hint: s.get("tool_hint").and_then(|v| v.as_str()).map(|x| x.to_string()),
            })
            .collect();

        let plan = service().create_plan(context.char_id.clone(), objective.clone(), steps);

        // 广播给前端弹确认框（携带 plan_id + objective + steps）
        if let Some(handle) = APP_HANDLE.read().as_ref() {
            let _ = handle.emit(
                "plan:created",
                json!({
                    "plan_id": plan.plan_id,
                    "char_id": plan.char_id,
                    "objective": plan.objective,
                    "steps": plan.steps,
                }),
            );
        }

        ToolResult::standard_success(
            &format!("已创建计划等待批准：{}（{} 步）", plan.objective, plan.steps.len()),
            Some(json!({ "plan_id": plan.plan_id, "objective": plan.objective, "steps": plan.steps })),
        )
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
        "plan approve task steps"
    }
}
