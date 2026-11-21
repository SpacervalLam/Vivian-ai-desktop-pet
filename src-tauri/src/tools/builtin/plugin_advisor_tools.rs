//! Discovery can propose capabilities; installing executable packages is a separate reviewed action.
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct PluginAdvisorTool;
#[async_trait]
impl Tool for PluginAdvisorTool {
    fn name(&self) -> &str {
        "plugin_advisor"
    }
    fn description(&self) -> &str {
        "Track real reusable capability gaps and propose Vivian plugins. Actions: list; record_gap(need, public generic wording only); search(gapId) searches public GitHub sources and checks an explicit Vivian api:1 manifest; inspect(gapId,source OR recommendationId) downloads a pinned repository snapshot or reads a local ZIP/folder and returns full code/dependency review; propose_build(gapId) records a self-built alternative. Search and inspection NEVER install, trust, start servers or certify business usability. Do not treat DSH/npm packages as Vivian plugins. Discuss relevant suggestions with the user before installing via install_plugin or creating via create_plugin. Those work-side tools require a concrete approval. A missing model credential or user refusal is not a capability gap."
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{
        "action":{"type":"string","enum":["list","record_gap","search","inspect","propose_build","dismiss"]},
        "need":{"type":"string","maxLength":120},"gapId":{"type":"string"},"source":{"type":"string"},"recommendationId":{"type":"string"}
    },"required":["action"],"additionalProperties":false})
    }
    async fn validate_input(&self, input: &Value, _: &ToolUseContext) -> ValidationResult {
        let action = input["action"].as_str().unwrap_or("");
        if !matches!(
            action,
            "list" | "record_gap" | "search" | "inspect" | "propose_build" | "dismiss"
        ) {
            return ValidationResult::failure("action 无效", 2);
        }
        if action == "record_gap" && input["need"].as_str().is_none_or(|s| s.trim().is_empty()) {
            return ValidationResult::failure("需要通用能力描述 need", 2);
        }
        if matches!(action, "search" | "inspect" | "propose_build")
            && input["gapId"].as_str().is_none_or(|s| s.is_empty())
        {
            return ValidationResult::failure("请先记录能力缺口并提供 gapId", 2);
        }
        ValidationResult::success(None)
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }
    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        use crate::plugin_advisor as advisor;
        let id = args["gapId"].as_str().unwrap_or("");
        if matches!(args["action"].as_str(), Some("search" | "inspect")) {
            use tauri::Manager;
            if let Some(app) = crate::network::web::current_app_handle() {
                let state = app.state::<Arc<crate::state::AppState>>();
                let network_tool = if args["action"] == "search" {
                    "web_search"
                } else {
                    "read_content"
                };
                if state.tool_system.is_tool_disabled(
                    network_tool,
                    crate::tools::registry::AgentSide::from_agent_kind(&ctx.agent_kind),
                ) {
                    return ToolResult::standard_error(
                        "对应联网能力已被禁用，不能通过插件发现绕过",
                        Some("ToolDisabled"),
                        None,
                    );
                }
            }
        }
        if args["action"] == "inspect" && args.get("recommendationId").is_none() {
            let source = args["source"].as_str().unwrap_or("");
            if !source.starts_with("https://")
                && (!ctx.is_work_agent()
                    || ctx.working_directory.is_empty()
                    || !ctx.is_path_authorized(source))
            {
                return ToolResult::standard_error(
                    "本地候选必须位于已授权工作区；也可让用户在设置中选择 ZIP/目录检查",
                    Some("UnauthorizedPluginSource"),
                    None,
                );
            }
        }
        let result = match args["action"].as_str().unwrap_or("") {
            "list" => Ok(advisor::view(&ctx.char_id)),
            "dismiss" => advisor::dismiss(
                &ctx.char_id,
                args["recommendationId"].as_str().unwrap_or(""),
            )
            .map(|_| advisor::view(&ctx.char_id)),
            "record_gap" => advisor::record(
                &ctx.char_id,
                args["need"].as_str().unwrap_or(""),
                &ctx.session_id,
            )
            .map(|id| json!({"gapId":id})),
            "search" => advisor::search(&ctx.char_id, id).await,
            "inspect" => {
                advisor::inspect(
                    &ctx.char_id,
                    id,
                    args["source"].as_str().unwrap_or(""),
                    args["recommendationId"].as_str(),
                )
                .await
            }
            "propose_build" => {
                advisor::propose_build(&ctx.char_id, id).map(|_| advisor::view(&ctx.char_id))
            }
            _ => Err("action 无效".into()),
        };
        match result {
            Ok(data) => ToolResult::standard_success(
                "能力建议记录（候选不是安装授权；已加载也不代表业务验证）",
                Some(data),
            ),
            Err(error) => ToolResult::standard_error(&error, Some("PluginAdvisorError"), None),
        }
    }
    fn is_read_only(&self) -> bool {
        false
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
    fn search_hint(&self) -> &str {
        "plugin discover recommend capability gap self evolution 插件 发现 推荐 能力缺口 自进化"
    }
}

pub struct InstallPluginTool {
    pub skills: Arc<crate::skills::SkillService>,
    pub mcp: Arc<crate::tools::McpManager>,
    pub tools: Arc<crate::tools::ToolSystem>,
}
#[async_trait]
impl Tool for InstallPluginTool {
    fn name(&self) -> &str {
        "install_plugin"
    }
    fn description(&self) -> &str {
        "Install AND trust one reviewed Vivian plugin suggestion after the user approves its full package preview. First call plugin_advisor inspect and obtain recommendationId and fingerprint. The host regenerates the preview; changes invalidate approval. Scripts, JS, protocols and MCP commands are shown before approval. Missing dependencies must be resolved separately. Success means runtime contributions loaded; report business behavior as unverified until a real successful invocation. Never install an unrelated candidate or bypass a refusal with shell commands."
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{"recommendationId":{"type":"string"},"fingerprint":{"type":"string"}},"required":["recommendationId","fingerprint"],"additionalProperties":false})
    }
    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        match crate::plugin_advisor::installation_preview(
            &ctx.char_id,
            input["recommendationId"].as_str().unwrap_or(""),
            input["fingerprint"].as_str().unwrap_or(""),
        ) {
            Ok(preview) => ValidationResult::success(Some(preview)),
            Err(error) => ValidationResult::failure(error, 2),
        }
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::ask(
            "将安装并信任本次预览中的插件，可能执行脚本或启动 MCP 服务，请审核完整内容",
        )
    }
    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        match crate::plugin_advisor::install(
            &ctx.char_id,
            args["recommendationId"].as_str().unwrap_or(""),
            args["fingerprint"].as_str().unwrap_or(""),
            &self.skills,
            &self.mcp,
            &self.tools,
        )
        .await
        {
            Ok(data) => ToolResult::standard_success(
                "插件已安装并装载；实际业务能力仍待真实调用验证",
                Some(data),
            ),
            Err(error) => ToolResult::standard_error(&error, Some("PluginInstallFailed"), None),
        }
    }
    fn is_read_only(&self) -> bool {
        false
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Shell
    }
    fn is_destructive(&self) -> bool {
        true
    }
}
