//! 编程智能体 Tauri 命令（前端 CodeAgentPage 调用）
//!
//! 会话数据由全局 `CodingAgentService` 管理（持久化到用户数据目录），
//! LLM 路由与工具系统复用 AppState 的共享实例。

use std::sync::Arc;

use once_cell::sync::Lazy;
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::brain::coding_agent::{
    CodingAgentService, CodingSession, CodingWorkspace,
};
use crate::state::AppState;

/// 文件树节点（前端侧边栏与 @-mention 文件选择共用）。
#[derive(Debug, Clone, Serialize)]
pub struct CodingFileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<CodingFileNode>,
}

/// 遍历目录构建文件树（与 list_dir 工具同规则：目录优先、跳过依赖目录）。
fn build_file_tree(dir: &std::path::Path, depth: usize) -> Vec<CodingFileNode> {
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(|e| (e.path().is_file(), e.file_name()));
    let skip: &[&str] = &[".git", "node_modules", "target", "dist", ".venv", "__pycache__", ".next"];
    let mut nodes = Vec::new();
    for entry in items {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.path().is_dir();
        if is_dir && skip.contains(&name.as_str()) {
            continue;
        }
        nodes.push(CodingFileNode {
            name: name.clone(),
            path: entry.path().to_string_lossy().into_owned(),
            is_dir,
            children: if is_dir { build_file_tree(&entry.path(), depth - 1) } else { Vec::new() },
        });
    }
    nodes
}

/// 列出工作目录文件树（前端侧边栏与 @-mention 文件选择）。
#[tauri::command]
pub fn coding_list_dir_tree(
    directory: String,
    max_depth: Option<u8>,
) -> Result<Vec<CodingFileNode>, String> {
    let dir = std::path::Path::new(&directory);
    if !dir.is_dir() {
        return Err(format!("目录不存在: {directory}"));
    }
    let depth = (max_depth.unwrap_or(2)).clamp(1, 4) as usize;
    Ok(build_file_tree(dir, depth))
}

/// 全局编程智能体服务单例。
pub static CODING_AGENT: Lazy<Arc<CodingAgentService>> =
    Lazy::new(|| Arc::new(CodingAgentService::new()));

/// 解析会话的上下文窗口：工作模型显式配置 → 主配置 → 厂商默认窗口。
fn resolve_context_window(cfg: &crate::config::manager::AppConfig, model_id: Option<&str>) -> u64 {
    if let Some(mid) = model_id {
        if let Some(m) = cfg.work_models.iter().find(|m| m.id == mid) {
            if let Some(w) = m.route.context_window {
                return w;
            }
            return crate::providers::capabilities::default_context_window(&m.route.model);
        }
    }
    if let Some(w) = cfg.ai.context_window {
        return w;
    }
    crate::providers::capabilities::default_context_window(&cfg.ai.model)
}

/// 新建编程会话。
#[tauri::command]
pub fn coding_new_session(
    state: State<'_, Arc<AppState>>,
    char_id: String,
    working_directory: String,
    mode: Option<String>,
) -> Result<CodingSession, String> {
    // 工作目录必须存在
    if !working_directory.is_empty() && !std::path::Path::new(&working_directory).is_dir() {
        return Err(format!("工作目录不存在: {working_directory}"));
    }
    let session = CODING_AGENT.create_session(&char_id, &working_directory, mode.as_deref().unwrap_or("standard"));
    // 按当前配置解析会话上下文窗口（active 工作模型 → 主配置 → 厂商默认）
    let cfg = state.config.read().get_all();
    let active_id = cfg.active_work_model.as_deref();
    let window = resolve_context_window(&cfg, active_id);
    CODING_AGENT.set_context_window(&session.session_id, window);
    Ok(session)
}

/// 切换会话工作模式（standard / code / minimal；运行中拒绝）。
#[tauri::command]
pub fn coding_set_mode(session_id: String, mode: String) -> Result<(), String> {
    CODING_AGENT.set_mode(&session_id, &mode)
}

/// 会话历史中出现过的工作区列表（去重，按最近使用倒序）。
#[tauri::command]
pub fn coding_list_workspaces() -> Vec<CodingWorkspace> {
    CODING_AGENT.list_workspaces()
}

/// 切换会话工作目录（目录必须存在；运行中拒绝）。
#[tauri::command]
pub fn coding_set_workspace(session_id: String, workspace: String) -> Result<(), String> {
    CODING_AGENT.set_workspace(&session_id, &workspace)
}

/// 设置会话权限等级（read_only / workspace_write / full_access；运行中拒绝）。
#[tauri::command]
pub fn coding_set_permission(session_id: String, permission: String) -> Result<(), String> {
    CODING_AGENT.set_permission(&session_id, &permission)
}

/// 设置会话选中的工作模型 id（与 select_work_model 同步；运行中拒绝）。
#[tauri::command]
pub fn coding_set_model(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    model_id: String,
) -> Result<(), String> {
    CODING_AGENT.set_model(&session_id, &model_id)?;
    // 同步会话上下文窗口（该工作模型的配置或厂商默认）
    let cfg = state.config.read().get_all();
    let window = resolve_context_window(&cfg, Some(&model_id));
    CODING_AGENT.set_context_window(&session_id, window);
    Ok(())
}

/// 设置会话推理等级（low / medium / high；运行中拒绝）。
#[tauri::command]
pub fn coding_set_reasoning_level(session_id: String, level: String) -> Result<(), String> {
    CODING_AGENT.set_reasoning_level(&session_id, &level)
}

/// 可用的工作模型列表（id + name，供编程页模型下拉选择）。
#[tauri::command]
pub fn coding_list_available_models(
    state: State<'_, Arc<AppState>>,
) -> Vec<serde_json::Value> {
    let cfg = state.config.read().get_all();
    cfg.work_models
        .iter()
        .map(|m| serde_json::json!({ "id": m.id, "name": m.name }))
        .collect()
}

/// 会话简表（含完整消息，供列表与恢复）。
#[tauri::command]
pub fn coding_list_sessions() -> Vec<CodingSession> {
    CODING_AGENT.list_sessions()
}

/// 删除会话。
#[tauri::command]
pub fn coding_delete_session(session_id: String) -> Result<bool, String> {
    Ok(CODING_AGENT.delete_session(&session_id))
}

/// 取消正在运行的会话任务。
#[tauri::command]
pub fn coding_cancel_session(session_id: String) -> Result<bool, String> {
    Ok(CODING_AGENT.cancel(&session_id))
}

/// 发送用户消息并驱动 agent loop（事件实时广播 coding:*）。图片与文件引用随消息注入上下文。
/// `interjected` 标记任务执行期间排队的插话消息，构建 LLM 上下文时加插话标注。
#[tauri::command]
pub fn coding_send_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    message: String,
    images: Option<Vec<crate::brain::coding_agent::CodingImage>>,
    file_refs: Option<Vec<crate::brain::coding_agent::CodingFileRef>>,
    interjected: Option<bool>,
) -> Result<(), String> {
    let router = state
        .model_router
        .read()
        .clone()
        .ok_or("模型路由未初始化（请先配置模型）")?;
    let tool_system = state.tool_system.clone();
    // 单轮 LLM↔工具 循环预算（设置-工具-编程智能体最大轮次，默认 48）
    let max_rounds = state.config.read().get_all().tools.max_coding_rounds as usize;
    CODING_AGENT.send_message(
        app,
        session_id,
        router,
        tool_system,
        message,
        images.unwrap_or_default(),
        file_refs.unwrap_or_default(),
        max_rounds,
        interjected.unwrap_or(false),
    )
}

/// 设置单条消息级反馈（up / down，空串清除）。
#[tauri::command]
pub fn coding_set_message_feedback(
    session_id: String,
    message_index: usize,
    rating: String,
) -> Result<(), String> {
    CODING_AGENT.set_message_feedback(&session_id, message_index, &rating)
}

/// 从指定消息处 fork 出新的独立会话。
#[tauri::command]
pub fn coding_fork_session(session_id: String, message_index: usize) -> Result<CodingSession, String> {
    CODING_AGENT.fork_session(&session_id, message_index)
}
