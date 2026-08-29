//! 工具命令 - 工具列表、执行与历史

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

/// List all registered tools
///
/// `description` 字段按当前界面语言返回（调用 `Tool::description_in`），
/// 与 ToolSemanticFilter 语义匹配使用的描述保持一致。
#[tauri::command]
pub fn list_tools(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let tool_system = &state.tool_system;
    let lang = state.config.read().get_all().base.language;
    let normalized = crate::pipeline::prompt_modules::normalize_lang(&lang);
    let tools = tool_system.list_tools();
    let tool_defs: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name(),
                "description": t.description_in(normalized),
                "input_schema": t.parameters_schema_in(normalized),
                "is_read_only": t.is_read_only(),
                "category": t.category(),
                "is_custom": t.is_custom(),
            })
        })
        .collect();
    Ok(json!({
        "tools": tool_defs,
        "total": tool_defs.len(),
    }))
}

/// Get tool call history and observability summary
#[tauri::command]
pub fn get_tool_history(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let tool_system = &state.tool_system;
    let summary = tool_system.get_observability_summary();
    let cache_stats = tool_system.get_cache_stats();
    Ok(json!({
        "observability": summary,
        "cache": cache_stats,
        "tool_count": tool_system.list_tool_names().len(),
    }))
}

/// After user confirms tool execution request on frontend, return result via this command
///
/// When tool requires user confirmation (e.g. file operations, screenshots, launching apps),
/// backend emits `tool:confirmation_request` event, frontend shows a confirmation toast,
/// user selects then calls this command to return result.
///
/// - `request_id`: from ConfirmationRequest
/// - `action`: "deny" / "allow_once" / "allow_always"
#[tauri::command]
pub fn confirm_tool_execution(
    state: State<'_, Arc<AppState>>,
    request_id: u64,
    action: String,
) -> Result<bool, String> {
    use crate::tools::confirmation::ConfirmationResponse;

    let response = match action.as_str() {
        "deny" => ConfirmationResponse::Deny,
        "allow_once" => ConfirmationResponse::AllowOnce,
        "allow_always" => ConfirmationResponse::AllowAlways,
        other => return Err(format!("无效的确认动作: {}", other)),
    };

    let tool_system = &*state.tool_system;
    let resolved = tool_system
        .confirmation
        .resolve_request(request_id, response);
    if resolved {
        tracing::info!(
            "[Command] 工具确认 {} 已解决: action={}",
            request_id,
            action
        );
    } else {
        tracing::warn!(
            "[Command] 工具确认 {} 未找到（可能已超时或重复解决）",
            request_id
        );
    }
    Ok(resolved)
}

/// 回传用户对 `ask_user` 提问的回答，唤醒正在等待的工具。
///
/// 后端 emit `chat:question` 事件后，前端弹输入框；用户提交回答后调用本命令，
/// 携带 question_id 与回答文本，注册表据此把答案交回工具执行流。
#[tauri::command]
pub fn respond_question(question_id: u64, answer: String) -> Result<bool, String> {
    use crate::tools::question::global_question_registry;
    let registry = global_question_registry();
    let resolved = registry.respond(question_id, answer);
    if !resolved {
        tracing::warn!(
            "[Command] 提问 {} 未找到（可能已超时或重复回答）",
            question_id
        );
    }
    Ok(resolved)
}

/// 用户对 `plan_task` 计划的批准/否决决定。
///
/// 后端 emit `plan:created` 事件后，前端弹确认框；用户选择后调用本命令，
/// 携带 plan_id 与 decision（approve / reject），计划状态据此迁移。
#[tauri::command]
pub fn plan_decision(plan_id: String, decision: String) -> Result<bool, String> {
    use crate::brain::plan_mode::PlanService;
    use once_cell::sync::Lazy;
    static SVC: Lazy<PlanService> = Lazy::new(|| (*PlanService::new()).clone());
    let ok = match decision.as_str() {
        "approve" => SVC.approve(&plan_id),
        "reject" => SVC.reject(&plan_id),
        other => return Err(format!("无效的决策: {}", other)),
    };
    if !ok {
        tracing::warn!("[Command] 计划 {} 决策失败（可能已决定或不存在）", plan_id);
    }
    Ok(ok)
}

// ===== MCP server 管理 =====

/// 列出所有 MCP server（含运行时状态）
#[tauri::command]
pub async fn list_mcp_servers(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Value>, String> {
    let servers = state.mcp_manager.list_servers().await;
    Ok(servers
        .into_iter()
        .map(|s| {
            json!({
                "id": s.id,
                "name": s.name,
                "enabled": s.enabled,
                "tool_count": s.tool_count,
                "alive": s.alive,
            })
        })
        .collect())
}

/// 读取已持久化的 MCP server 配置（含未连接的）
#[tauri::command]
pub fn list_mcp_server_configs(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Value>, String> {
    let configs = state.mcp_manager.load_configs();
    Ok(configs
        .into_iter()
        .map(serde_json::to_value)
        .filter_map(Result::ok)
        .collect())
}

/// 添加并连接 MCP server
#[tauri::command]
pub async fn add_mcp_server(
    state: State<'_, Arc<AppState>>,
    config: crate::tools::McpServerConfig,
) -> Result<Vec<String>, String> {
    state
        .mcp_manager
        .add_server(config)
        .await
        .map_err(|e| e.to_string())
}

/// 断开并移除 MCP server
#[tauri::command]
pub async fn remove_mcp_server(
    state: State<'_, Arc<AppState>>,
    server_id: String,
) -> Result<(), String> {
    state
        .mcp_manager
        .remove_server(&server_id)
        .await
        .map_err(|e| e.to_string())
}
