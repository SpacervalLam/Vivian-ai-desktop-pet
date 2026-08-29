//! 插件 / 技能清单查询命令（供设置窗口「插件」页盘点展示）

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

/// 插件清单（盘点插件目录，只读不装载）。
#[tauri::command]
pub fn list_plugins() -> Vec<crate::plugins::PluginInventoryEntry> {
    crate::plugins::scan_inventory()
}

/// 插件 / 技能目录路径（供界面提示用户放置位置）。
#[tauri::command]
pub fn plugin_paths() -> serde_json::Value {
    serde_json::json!({
        "plugins_dir": crate::plugins::plugins_dir().display().to_string(),
        "skills_dir": crate::skills::SkillService::default_dir().display().to_string(),
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
