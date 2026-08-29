//! 工作流编排工具 — 让 LLM 提交多步工作流（可扇出并行步骤）给编排引擎执行。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::brain::workflow::{run_workflow, WorkflowStep};
use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// run_workflow 工具：提交 JSON 编排给引擎执行。
pub struct RunWorkflowTool;

impl RunWorkflowTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RunWorkflowTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RunWorkflowTool {
    fn name(&self) -> &str {
        "run_workflow"
    }

    fn description(&self) -> &str {
        "Run a multi-step workflow: steps is an array of {tool, arguments, parallel}. Consecutive steps marked parallel:true run concurrently as a group (fan-out); others run in order. Each step is a full tool call going through the sandbox/approval pipeline. Returns a per-step outcome summary. A step failure does not abort the workflow."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "运行多步工作流：steps 为 {tool, arguments, parallel} 数组。连续标记 parallel:true 的步骤作为一组并发执行（扇出）；其余顺序执行。每步都是完整工具调用（经过沙箱/审批管线）。返回逐步结果汇总；单步失败不中止整个工作流。",
            "ja" => "複数ステップのワークフローを実行：steps は {tool, arguments, parallel} の配列。parallel:true が連続するステップは1グループとして並行実行（ファンアウト）、それ以外は順次実行。各ステップは完全なツール呼び出し（サンドボックス/承認パイプラインを通る）。ステップごとの結果サマリーを返す。1ステップの失敗でワークフロー全体は中断されない。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Workflow name (for logging/display)"},
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {"type": "string", "description": "Tool name to call"},
                            "arguments": {"type": "object", "description": "Tool arguments"},
                            "parallel": {"type": "boolean", "description": "true to run concurrently with adjacent parallel steps (default false)"}
                        },
                        "required": ["tool", "arguments"]
                    }
                }
            },
            "required": ["name", "steps"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "工作流名称（用于日志/展示）"},
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": {"type": "string", "description": "要调用的工具名"},
                                "arguments": {"type": "object", "description": "工具参数"},
                                "parallel": {"type": "boolean", "description": "true 表示与相邻并行步骤并发执行（默认 false）"}
                            },
                            "required": ["tool", "arguments"]
                        }
                    }
                },
                "required": ["name", "steps"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "ワークフロー名（ログ/表示用）"},
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": {"type": "string", "description": "呼び出すツール名"},
                                "arguments": {"type": "object", "description": "ツール引数"},
                                "parallel": {"type": "boolean", "description": "true は隣接する並行ステップと同時実行（デフォルト false）"}
                            },
                            "required": ["tool", "arguments"]
                        }
                    }
                },
                "required": ["name", "steps"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let name_ok = input
            .get("name")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        let steps_ok = input
            .get("steps")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
        if !name_ok {
            return ValidationResult::failure("name 是必填项", 2);
        }
        if !steps_ok {
            return ValidationResult::failure("steps 是必填项且至少一步", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::ask("工作流会执行多步工具调用（逐步经过沙箱/审批），确认运行？")
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let Some(app) = APP_HANDLE.read().clone() else {
            return ToolResult::standard_error("无法执行工作流（后端未初始化）", Some("WorkflowUnavailable"), None);
        };
        let state = app.state::<Arc<AppState>>();
        let tool_system = state.tool_system.clone();

        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("workflow")
            .to_string();
        let steps: Vec<WorkflowStep> = args
            .get("steps")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|s| WorkflowStep {
                tool: s.get("tool").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                arguments: s.get("arguments").cloned().unwrap_or(Value::Null),
                parallel: s.get("parallel").and_then(|v| v.as_bool()).unwrap_or(false),
            })
            .collect();
        if steps.is_empty() {
            return ToolResult::standard_error("steps 不能为空", Some("EmptyWorkflow"), None);
        }

        let run = run_workflow(&name, steps, &tool_system, context).await;
        ToolResult::success(json!({
            "name": run.name,
            "total": run.total,
            "succeeded": run.succeeded,
            "failed": run.failed,
            "steps": run.steps,
        }))
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
        "workflow orchestration steps parallel fanout"
    }
}
