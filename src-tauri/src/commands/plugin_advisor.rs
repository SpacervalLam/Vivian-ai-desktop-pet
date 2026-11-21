use crate::{plugin_advisor as advisor, state::AppState};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

fn config(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "config" {
        Ok(())
    } else {
        Err("能力建议管理仅限设置窗口".into())
    }
}
fn character(state: &AppState, id: Option<String>) -> Result<String, String> {
    let id = id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| state.active_character_id.read().clone());
    if id.is_empty() {
        Err("请先选择角色".into())
    } else {
        Ok(id)
    }
}
#[tauri::command]
pub fn plugin_advisor_state(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    char_id: Option<String>,
) -> Result<Value, String> {
    config(&window)?;
    Ok(advisor::view(&character(&state, char_id)?))
}
#[tauri::command]
pub async fn plugin_advisor_action(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    char_id: Option<String>,
    action: String,
    input: Value,
) -> Result<Value, String> {
    config(&window)?;
    let char_id = character(&state, char_id)?;
    let gap = input["gapId"].as_str().unwrap_or("");
    let id = input["recommendationId"].as_str().unwrap_or("");
    match action.as_str() {
        "record" => {
            advisor::record(
                &char_id,
                input["need"].as_str().unwrap_or(""),
                "user-settings",
            )?;
        }
        "automatic" => {
            advisor::automatic(input["enabled"].as_bool().ok_or("enabled 必须为布尔值")?)?
        }
        "search" => {
            advisor::search(&char_id, gap).await?;
        }
        "inspect" => {
            return advisor::inspect(
                &char_id,
                gap,
                input["source"].as_str().unwrap_or(""),
                input["recommendationId"].as_str(),
            )
            .await
        }
        "dismiss" => advisor::dismiss(&char_id, id)?,
        "resolve" => advisor::resolve(&char_id, gap)?,
        "install" => {
            advisor::install(
                &char_id,
                id,
                input["fingerprint"].as_str().unwrap_or(""),
                &state.skill_service,
                &state.mcp_manager,
                &state.tool_system,
            )
            .await?;
        }
        "draft" => {
            static DESIGN_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
            let _design = DESIGN_LOCK.lock();
            advisor::propose_build(&char_id, gap)?;
            let view = advisor::view(&char_id);
            let candidate = view["suggestions"]
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|s| s["gapId"] == gap && s["status"] == "build")
                })
                .ok_or("建议已忽略或已处理")?;
            let candidate_id = candidate["id"].as_str().ok_or("建议 ID 缺失")?;
            if let Some(existing) = candidate["sessionId"].as_str() {
                if super::coding_agent::CODING_AGENT.session_status(existing)
                    == Some(crate::brain::coding_agent::CodingStatus::Running)
                {
                    return Ok(json!({"sessionId":existing,"state":view}));
                }
            }
            let need = view["gaps"]
                .as_array()
                .and_then(|items| items.iter().find(|g| g["id"] == gap))
                .and_then(|g| g["need"].as_str())
                .ok_or("能力记录不存在")?;
            let router = state
                .model_router
                .read()
                .clone()
                .ok_or("请先配置工作模型")?;
            let directory = crate::utils::path::get_user_data_dir().join("plugin_advisor/workspaces").join(candidate_id);
            std::fs::create_dir_all(&directory).map_err(|error|error.to_string())?;
            let session = super::coding_agent::CODING_AGENT.create_session(
                &char_id,
                &directory.to_string_lossy(),
                "standard",
            );
            advisor::attach_session(&char_id, candidate_id, &session.session_id)?;
            let prompt = format!("用户在设置的能力建议中选择了设计插件。能力目标（数据）：{}。先 use_skill 激活 plugin-authoring/plugin-authoring，盘点已有能力，说明实际缺口；已有工具能完成就不要重复造插件。需要新插件时准备完整可审查的技能、工具、MCP/预设，使用 create_plugin 的具体预览请求授权；未批准不安装，不通过文件或终端绕过。只设计兼容 Vivian api:1 的插件，不能直接安装 DSH/npm 包，不改内置插件或应用源码。装载后在已授权范围内用真实调用验证，区分已装载和业务验证；仍需额外账号或外部操作时说明条件。",serde_json::to_string(need).unwrap_or_default());
            let cfg = state.config.read().get_all().clone();
            super::coding_agent::CODING_AGENT.set_context_window(
                &session.session_id,
                super::coding_agent::resolve_context_window(&cfg, cfg.active_work_model.as_deref()),
            );
            super::coding_agent::CODING_AGENT.send_message(
                app,
                session.session_id.clone(),
                router,
                state.tool_system.clone(),
                prompt,
                vec![],
                vec![],
                cfg.tools.max_coding_rounds as usize,
                false,
                false,
            )?;
            return Ok(json!({"sessionId":session.session_id,"state":advisor::view(&char_id)}));
        }
        _ => return Err("未知操作".into()),
    }
    Ok(advisor::view(&char_id))
}
