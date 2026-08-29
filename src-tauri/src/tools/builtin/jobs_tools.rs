//! 后台任务工具 — 让 LLM 启动/查询/终止后台命令任务。
//!
//! - `run_job`：在后台启动一个 PowerShell 命令，立即返回 job_id（不阻塞）。
//! - `manage_job`：action 为 get（查单个）/ list（列全部）/ kill（终止）。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::brain::jobs::{JobInfo, JobManager, JobStatus};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 全局 JobManager 单例（由工具首次使用即建，进程生命周期内持存）。
fn manager() -> &'static JobManager {
    use once_cell::sync::Lazy;
    static MGR: Lazy<JobManager> = Lazy::new(|| (*JobManager::new()).clone());
    &MGR
}

fn job_to_json(j: &JobInfo) -> Value {
    json!({
        "job_id": j.job_id,
        "command": j.command,
        "status": match j.status {
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Killed => "killed",
        },
        "exit_code": j.exit_code,
        "output": j.output,
        "truncated": j.truncated,
        "created_at_ms": j.created_at_ms,
        "finished_at_ms": j.finished_at_ms,
    })
}

// ===== run_job =====

/// 后台启动任务工具。
pub struct RunJobTool;

impl RunJobTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RunJobTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RunJobTool {
    fn name(&self) -> &str {
        "run_job"
    }

    fn description(&self) -> &str {
        "Start a long-running PowerShell command in the background and immediately return a job_id (does not block). Use for slow operations like builds, installs or downloads. Then poll with manage_job (action=get) to read progress/output, and kill with manage_job (action=kill) when needed."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在后台启动一个耗时较长的 PowerShell 命令，立即返回 job_id（不阻塞）。用于构建、安装、下载等慢操作。之后用 manage_job（action=get）轮询进度/输出，需要时用 manage_job（action=kill）终止。",
            "ja" => "長時間実行される PowerShell コマンドをバックグラウンドで起動し、即座に job_id を返す（ブロックしない）。ビルド、インストール、ダウンロードなどの遅い操作に使用。その後 manage_job（action=get）で進捗/出力を確認、必要に応じて manage_job（action=kill）で終了。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "The PowerShell command to run in the background"}
            },
            "required": ["command"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "要在后台执行的 PowerShell 命令"}
                },
                "required": ["command"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "バックグラウンドで実行する PowerShell コマンド"}
                },
                "required": ["command"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("command").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("command 是必填项且不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::ask("是否允许在后台执行此 PowerShell 命令？")
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let job_id = manager().start(command.clone());
        ToolResult::standard_success(
            &format!("后台任务已启动：{job_id}"),
            Some(json!({ "job_id": job_id, "command": command })),
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
        "background job run command"
    }
}

// ===== manage_job =====

/// 后台任务管理工具（get / list / kill）。
pub struct ManageJobTool;

impl ManageJobTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ManageJobTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ManageJobTool {
    fn name(&self) -> &str {
        "manage_job"
    }

    fn description(&self) -> &str {
        "Manage background jobs. action: get (query one job by job_id), list (list all jobs), kill (terminate a running job by job_id)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "管理后台任务。action：get（按 job_id 查询单个任务）、list（列出全部任务）、kill（按 job_id 终止运行中的任务）。",
            "ja" => "バックグラウンドジョブを管理する。action：get（job_id で1件取得）、list（全件一覧）、kill（job_id で実行中ジョブを終了）。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["get", "list", "kill"], "description": "Operation: get / list / kill"},
                "job_id": {"type": "string", "description": "Job ID (required for get and kill)"}
            },
            "required": ["action"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["get", "list", "kill"], "description": "操作：get / list / kill"},
                    "job_id": {"type": "string", "description": "任务 ID（get 和 kill 必填）"}
                },
                "required": ["action"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["get", "list", "kill"], "description": "操作：get / list / kill"},
                    "job_id": {"type": "string", "description": "ジョブ ID（get と kill で必須）"}
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
        if !matches!(action.as_str(), "get" | "list" | "kill") {
            return ValidationResult::failure("action 必须是 get / list / kill", 2);
        }
        if (action == "get" || action == "kill")
            && input.get("job_id").and_then(|v| v.as_str()).map_or(true, |s| s.is_empty())
        {
            return ValidationResult::failure("get / kill 需要 job_id", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if action == "kill" {
            PermissionResult::ask("终止后台任务会中断其进程，需要确认")
        } else {
            PermissionResult::allow()
        }
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");

        match action {
            "get" => match manager().get(job_id) {
                Some(j) => ToolResult::standard_success(&format!("任务 {} 状态：{:?}", j.job_id, j.status), Some(job_to_json(&j))),
                None => ToolResult::standard_error("任务不存在", Some(&format!("未找到 job_id={job_id}")), None),
            },
            "list" => {
                let jobs = manager().list();
                let arr: Vec<Value> = jobs.iter().map(job_to_json).collect();
                ToolResult::standard_success(&format!("共 {} 个后台任务", jobs.len()), Some(json!({ "jobs": arr })))
            }
            "kill" => {
                if manager().kill(job_id) {
                    ToolResult::standard_success(&format!("任务 {job_id} 已终止"), Some(json!({ "job_id": job_id })))
                } else {
                    ToolResult::standard_error("任务不存在或已结束", Some(&format!("未找到运行中的 job_id={job_id}")), None)
                }
            }
            _ => ToolResult::standard_error("不支持的 action", Some("action 必须是 get / list / kill"), None),
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
        "background job get list kill"
    }
}
