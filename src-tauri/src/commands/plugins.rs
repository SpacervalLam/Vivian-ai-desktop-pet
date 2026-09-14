//! 插件 / 技能清单查询与运行时装卸命令
//!
//! 查询类供设置窗口「插件」页盘点展示；装卸类（reload/unload/delete/trust）是
//! 运行时插件生命周期的界面入口——`create_plugin` 元工具落盘后走的也是
//! 同一套 `plugins::load_one` 装载逻辑（经授权通道自动写信任）。
//! 装卸与信任命令在**后端校验调用窗口**（仅设置窗口可发起），前端确认
//! 只是体验层，不是安全边界。

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::state::AppState;

/// 单个技能条目（含来源与作用域徽章所需信息）
#[derive(Debug, Clone, Serialize)]
pub struct SkillEntryInfo {
    pub name: String,
    pub description: String,
    /// 作用域：`None` = 全局（所有角色可见）；`Some(char_id)` = 仅该角色
    pub scope: Option<String>,
    /// 来源：`builtin`（内置风格预设）/ `user`（用户技能目录）/ `plugin`（插件贡献）
    pub origin: String,
    /// 正文长度（字符数，供概览判断体量）
    pub body_len: usize,
}

/// 内置风格预设技能名（用于区分技能来源，与 create_skill 的防覆盖名单共用）。
use crate::skills::BUILTIN_SKILL_NAMES;

/// 插件清单（盘点插件目录，只读不装载；附带信任状态供设置页展示）。
#[tauri::command]
pub fn list_plugins() -> Vec<crate::plugins::PluginInventoryEntry> {
    crate::plugins::scan_inventory()
}

/// 敏感插件管理命令的调用边界：只允许设置窗口（label=config）发起。
///
/// 前端 `window.confirm` 不是安全控制——任何拿到 IPC 能力的 WebView 都能直接
/// invoke。重载/卸载/删除/信任在这里做后端窗口校验，WebView 侧越权调用直接拒绝。
fn ensure_config_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "config" {
        Ok(())
    } else {
        Err(format!(
            "插件管理命令只允许设置窗口调用（当前窗口: {}）",
            window.label()
        ))
    }
}

/// 信任插件：记录当前清单指纹并立即装载（设置页信任按钮的后端入口）。
///
/// 信任是把「目录里的文件」升级为「可执行贡献」的唯一用户授权面——
/// 信任后清单再变更即回到未信任，须重新确认。
#[tauri::command]
pub async fn trust_plugin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    key: String,
) -> Result<serde_json::Value, String> {
    ensure_config_window(&window)?;
    let report = crate::plugins::load_one(
        &state.skill_service,
        &state.mcp_manager,
        &state.tool_system,
        &key,
        true,
    )
    .await?;
    Ok(serde_json::json!({
        "plugin": key,
        "skills": report.skills,
        "tools": report.tools,
        "mcp_servers": report.mcp_servers,
        "skipped": report.skipped,
    }))
}

/// 重载单个插件：撤销旧贡献并按磁盘当前内容重新装载（含连接 MCP server）。
/// 手工编辑插件目录后用它即时生效，无需重启。
///
/// 仅受信且清单未变更的插件可重载；清单变更后须先重新信任。
#[tauri::command]
pub async fn reload_plugin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<serde_json::Value, String> {
    ensure_config_window(&window)?;
    let report = crate::plugins::load_one(
        &state.skill_service,
        &state.mcp_manager,
        &state.tool_system,
        &name,
        false,
    )
    .await?;
    Ok(serde_json::json!({
        "plugin": name,
        "skills": report.skills,
        "tools": report.tools,
        "mcp_servers": report.mcp_servers,
        "skipped": report.skipped,
    }))
}

/// 卸载单个插件的运行时贡献（不动磁盘文件；重启后按目录重新装载）。
#[tauri::command]
pub async fn unload_plugin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<(), String> {
    ensure_config_window(&window)?;
    crate::plugins::unload_one(
        &state.skill_service,
        &state.mcp_manager,
        &state.tool_system,
        &name,
    )
    .await
}

/// 删除插件：撤销运行时贡献并移除目录（内置插件禁删）。
#[tauri::command]
pub async fn delete_plugin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<(), String> {
    ensure_config_window(&window)?;
    crate::plugins::delete_plugin(
        &state.skill_service,
        &state.mcp_manager,
        &state.tool_system,
        &name,
    )
    .await
}

/// 插件贡献的 LLM 供应商预设（设置 → LLM 页厂商卡片数据源）。
///
/// 先确保内置 `llm-providers` 插件已播种（不存在或版本落后时写入），再从磁盘
/// 读取全部插件贡献的预设；每次调用都重新读盘——用户/技能直接编辑
/// `plugins/llm-providers/providers.json` 后重开设置窗口即生效。
#[tauri::command]
pub fn list_provider_presets() -> Vec<crate::plugins::ProviderPresetData> {
    crate::plugins::ensure_builtin_plugins();
    crate::plugins::load_provider_presets()
}

/// 插件 / 技能目录路径（供界面提示用户放置位置）。
#[tauri::command]
pub fn plugin_paths() -> serde_json::Value {
    serde_json::json!({
        "plugins_dir": crate::plugins::plugins_dir().display().to_string(),
        "skills_dir": crate::skills::SkillService::default_dir().display().to_string(),
    })
}

/// 插件运行时诊断（设置页「诊断」区数据源）。
///
/// - `last_report`：最近一次装载（启动 load_all 或设置页 load_one）的报告——
///   装载了哪些插件/技能/工具/MCP/协议，以及 skipped 清单（未信任、清单
///   损坏、JS 装载失败等）
/// - `events`：运行时静默决策的登记（MCP id 被占用、工具重名跳过、provider
///   预设冲突等），最新在前，有界去重
#[tauri::command]
pub fn plugin_diagnostics() -> serde_json::Value {
    let last_report = crate::plugins::last_report().map(|r| {
        serde_json::json!({
            "plugins": r.plugins,
            "skills": r.skills,
            "tools": r.tools,
            "mcp_servers": r.mcp_servers,
            "protocols": r.protocols,
            "skipped": r.skipped,
        })
    });
    serde_json::json!({
        "last_report": last_report,
        "events": crate::plugins::diag_events(),
    })
}

/// 技能清单（用户/插件技能，含来源与作用域；内置风格预设不展示）。
#[tauri::command]
pub fn list_skills(state: State<'_, Arc<AppState>>) -> Vec<SkillEntryInfo> {
    state
        .skill_service
        .list_all()
        .into_iter()
        // 内置风格预设不出现在设置窗口，只列用户与插件技能
        .filter(|s| !BUILTIN_SKILL_NAMES.contains(&s.name.as_str()))
        .map(|s| {
            // 命名空间 `plugin/xxx` → 插件贡献；其余为用户目录技能
            let origin = if s.name.contains('/') { "plugin" } else { "user" };
            SkillEntryInfo {
                body_len: s.body.chars().count(),
                name: s.name,
                description: s.description,
                scope: s.scope,
                origin: origin.to_string(),
            }
        })
        .collect()
}
