//! Mind Inspector 命令 —— 为「心智观察器」前端提供认知调试数据。
//!
//! 四个只读命令：
//! - [`get_recent_reasoning_traces`]：最近的推理轨迹列表（按角色索引）
//! - [`get_last_prompt_breakdown`]：最近一次 Prompt 组装分区分解（实际对话后缓存）
//! - [`get_prompt_template_preview`]：提示词模板预览（无需实际对话，展示模板结构）
//! - [`get_sessions`]：会话列表（含状态/能量/新颖度）

use std::sync::Arc;

use tauri::State;

use crate::conversation::CONVERSATION_MANAGER;
use crate::mind::reasoning_trace::{PromptBreakdown, ReasoningTrace, SessionView};
use crate::state::AppState;

/// 获取最近的推理轨迹列表
///
/// - `character_id`：角色 ID
/// - `limit`：最多返回条数，默认 20，上限 50
#[tauri::command]
pub fn get_recent_reasoning_traces(
    state: State<'_, Arc<AppState>>,
    character_id: String,
    limit: Option<usize>,
) -> Result<Vec<ReasoningTrace>, String> {
    let limit = limit.unwrap_or(20).min(50);
    Ok(state
        .reasoning_traces
        .read()
        .get_recent_traces(&character_id, limit))
}

/// 获取最近一次 Prompt 组装分解
#[tauri::command]
pub fn get_last_prompt_breakdown(
    state: State<'_, Arc<AppState>>,
    character_id: String,
) -> Result<Option<PromptBreakdown>, String> {
    Ok(state
        .reasoning_traces
        .read()
        .get_last_prompt(&character_id)
        .cloned())
}

/// 获取提示词模板预览（无需实际发起 API 请求）
///
/// 即使用户从未发送过消息，也能看到 prompt 的完整结构：
/// - 静态段落（身份、风格、规则、格式、示例等）填充真实内容
/// - 当前状态段落（关系、情绪、心智、环境、工具等）使用实时数据
/// - 对话依赖段落（记忆上下文、对话历史、用户输入、工作记忆、内心反应）
///   使用占位符说明此处将在实际对话中填充什么内容
#[tauri::command]
pub fn get_prompt_template_preview(
    state: State<'_, Arc<AppState>>,
    character_id: String,
) -> Result<Option<PromptBreakdown>, String> {
    let characters = state.characters.read();
    let char = characters.get(&character_id);
    match char {
        Some(c) => Ok(Some(c.brain.build_prompt_template_preview())),
        None => Ok(None),
    }
}

/// 获取会话列表
///
/// - `character_id`：可选角色 ID 过滤（None = 所有角色的会话）
/// - `limit`：最多返回条数，默认 50
///
/// 按 `last_active_at` 倒序排列。
#[tauri::command]
pub fn get_sessions(
    character_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<SessionView>, String> {
    let limit = limit.unwrap_or(50);
    let mut views: Vec<SessionView> = CONVERSATION_MANAGER
        .list_all()
        .into_iter()
        .filter(|conv| {
            // character_id 为 None 时返回所有；否则只返回该角色参与的会话
            match &character_id {
                None => true,
                Some(cid) => conv.participants.iter().any(|p| p == cid),
            }
        })
        .map(|conv| SessionView {
            id: conv.id,
            participants: conv.participants,
            started_at: conv.created_at,
            ended_at: conv.closed_at,
            rounds: conv.rounds as usize,
            energy: conv.energy as f32,
            novelty: conv.novelty as f32,
            status: conv.state.as_str().to_string(),
            close_reason: conv.close_reason.map(|r| r.as_str().to_string()),
            last_active_at: conv.last_active_at,
        })
        .collect();

    // 按 last_active_at 倒序
    views.sort_by(|a, b| b.last_active_at.partial_cmp(&a.last_active_at).unwrap_or(std::cmp::Ordering::Equal));
    views.truncate(limit);
    Ok(views)
}

/// 获取 Prompt Section Schema（模板引擎的 section 定义）
///
/// 返回所有 section 的 ID、名称、i18n key、层级、类型和可选性。
/// 前端 Context Pipeline 用此数据驱动可视化，而不是硬编码 section 列表。
/// 后端修改 section 结构时，前端自动反映。
#[tauri::command]
pub fn get_prompt_section_schema() -> Result<crate::pipeline::template_engine::PromptSectionSchema, String> {
    Ok(crate::pipeline::template_engine::section_schema())
}
