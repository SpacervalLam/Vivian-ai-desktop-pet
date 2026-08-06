//! 人格命令 - 人格配置查询、人设段落编辑与人格卡片管理。
//!
//! Character 层 7 段（文本）支持用户在上下文流水线页面编辑覆盖：
//! identity / personality / background / interests / appearance / speech / relationships
//! - `set_persona_section` 保存自定义文本（空串恢复出厂）
//! - `reset_persona_section` 恢复出厂默认
//! - `get_persona_sections` 读取 7 段文本（含是否自定义标记）
//!
//! Few-shot 示例使用结构化表单编辑：
//! - `get_few_shot_examples` 获取结构化示例数据
//! - `set_few_shot_examples` 保存结构化示例数据
//!
//! 人格卡片（PersonaCard）是表达侧面的演化系统：可创建/更新/切换/归档/删除，
//! 但 Core Persona（Character 层）永远不被卡片覆盖。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::persona::{FewShotExamplesConfig, PersonaCard};
use crate::state::AppState;

const CHARACTER_SECTIONS: &[&str] = &[
    "identity", "personality", "background", "interests",
    "appearance", "speech", "relationships",
];

/// 人设段落数据（7 个 Character 层文本段落）
#[derive(Debug, Serialize, Deserialize)]
pub struct PersonaSections {
    pub identity: PersonaSection,
    pub personality: PersonaSection,
    pub background: PersonaSection,
    pub interests: PersonaSection,
    pub appearance: PersonaSection,
    pub speech: PersonaSection,
    pub relationships: PersonaSection,
}

/// 单个人设段落
#[derive(Debug, Serialize, Deserialize)]
pub struct PersonaSection {
    pub content: String,
    pub customized: bool,
}

fn load_section(persona: &crate::persona::PersonaEngine, key: &str) -> PersonaSection {
    PersonaSection {
        content: persona.get_section_definition(key).unwrap_or_default(),
        customized: persona.is_section_customized(key),
    }
}

/// 获取 Character 层 7 段文本
#[tauri::command]
pub fn get_persona_sections(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<PersonaSections, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let persona = &brain.persona;
    Ok(PersonaSections {
        identity: load_section(persona, "identity"),
        personality: load_section(persona, "personality"),
        background: load_section(persona, "background"),
        interests: load_section(persona, "interests"),
        appearance: load_section(persona, "appearance"),
        speech: load_section(persona, "speech"),
        relationships: load_section(persona, "relationships"),
    })
}

/// 设置人设段落文本
///
/// `section`: "identity" | "personality" | "background" | "interests" |
///            "appearance" | "speech" | "relationships"
/// `content`: 自定义文本，传空字符串则恢复出厂默认
#[tauri::command]
pub fn set_persona_section(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    section: String,
    content: String,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let persona = &brain.persona;
    if !CHARACTER_SECTIONS.contains(&section.as_str()) {
        return Err(format!("未知人设段落: {}", section));
    }
    persona.set_section_definition(&section, &content).map_err(|e| e.to_string())
}

/// 重置人设段落到出厂默认
#[tauri::command]
pub fn reset_persona_section(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    section: String,
) -> Result<String, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let persona = &brain.persona;
    if section == "examples" {
        persona.reset_persona_section("examples").map_err(|e| e.to_string())?;
        return Ok(String::new());
    }
    if !CHARACTER_SECTIONS.contains(&section.as_str()) {
        return Err(format!("未知人设段落: {}", section));
    }
    persona.reset_persona_section(&section).map_err(|e| e.to_string())?;
    persona.get_section_definition(&section).map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FewShotExamplesResponse {
    pub data: FewShotExamplesConfig,
    pub customized: bool,
}

/// 获取结构化 Few-shot 示例
#[tauri::command]
pub fn get_few_shot_examples(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<FewShotExamplesResponse, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let customized = brain.persona.is_section_customized("examples");
    Ok(FewShotExamplesResponse {
        data: brain.persona.get_few_shot_examples(),
        customized,
    })
}

/// 保存结构化 Few-shot 示例
#[tauri::command]
pub fn set_few_shot_examples(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    data: FewShotExamplesConfig,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain.persona.set_few_shot_examples(data).map_err(|e| e.to_string())
}

/// 获取人格配置
#[tauri::command]
pub fn get_persona(state: State<'_, Arc<AppState>>, character_id: Option<String>) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let config = brain.persona.get_config();
    serde_json::to_value(config).map_err(|e| e.to_string())
}

/// 获取人格名称
#[tauri::command]
pub fn get_persona_name(state: State<'_, Arc<AppState>>, character_id: Option<String>) -> Result<String, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    Ok(brain.persona.get_name())
}

/// 获取人格标语
#[tauri::command]
pub fn get_persona_tagline(state: State<'_, Arc<AppState>>, character_id: Option<String>) -> Result<String, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    Ok(brain.persona.get_tagline())
}

/// 生成当前场景的风格约束 prompt
#[tauri::command]
pub fn get_style_prompt(state: State<'_, Arc<AppState>>, character_id: Option<String>) -> Result<String, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    // 亲密度从 PsychologyManager 读取（关系系统已统一到心理架构）
    let intimacy = brain
        .psychology
        .relationship()
        .intimacy
        * 100.0;
    let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
    Ok(brain.persona.build_style_prompt(intimacy, hour))
}

// ===== 人格卡片管理 =====

/// 列出所有人格卡片
#[tauri::command]
pub fn list_persona_cards(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    include_archived: Option<bool>,
) -> Result<Vec<PersonaCard>, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    Ok(brain.persona.card_store().list_cards(include_archived.unwrap_or(false)))
}

/// 获取当前激活的卡片
#[tauri::command]
pub fn get_active_persona_card(state: State<'_, Arc<AppState>>, character_id: Option<String>) -> Result<Option<PersonaCard>, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    Ok(brain.persona.card_store().get_active_card())
}

/// 创建人格卡片
#[tauri::command]
pub fn create_persona_card(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    name: String,
    description: String,
) -> Result<PersonaCard, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain
        .persona
        .card_store()
        .create_card(&name, &description)
        .map_err(|e| e.to_string())
}

/// 更新人格卡片覆盖物
#[tauri::command]
pub fn update_persona_card(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    card_id: String,
    expression_override: Option<Value>,
    language_style_override: Option<Value>,
    style_preset: Option<String>,
    extra_instructions: Option<Vec<String>>,
    description: Option<String>,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let expr = expression_override
        .map(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
        .transpose()?;
    let ls = language_style_override
        .map(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
        .transpose()?;
    brain
        .persona
        .card_store()
        .update_card(&card_id, expr, ls, style_preset, extra_instructions, description)
        .map_err(|e| e.to_string())
}

/// 切换激活的人格卡片（传 None 取消激活，回到 Core Persona）
#[tauri::command]
pub fn switch_persona_card(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    card_id: Option<String>,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain
        .persona
        .card_store()
        .switch_card(card_id.as_deref())
        .map_err(|e| e.to_string())
}

/// 归档人格卡片
#[tauri::command]
pub fn archive_persona_card(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    card_id: String,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain
        .persona
        .card_store()
        .archive_card(&card_id)
        .map_err(|e| e.to_string())
}

/// 删除人格卡片
#[tauri::command]
pub fn delete_persona_card(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    card_id: String,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain
        .persona
        .card_store()
        .delete_card(&card_id)
        .map_err(|e| e.to_string())
}

/// 读取人格演化事件日志
#[tauri::command]
pub fn get_persona_events(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<Value>, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let events = brain.persona.card_store().read_events(limit.unwrap_or(50));
    events
        .into_iter()
        .map(|e| serde_json::to_value(e).map_err(|e| e.to_string()))
        .collect()
}

/// 查询冷却状态
#[tauri::command]
pub fn get_persona_card_cooldowns(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let store = brain.persona.card_store();
    Ok(serde_json::json!({
        "turns_until_can_switch": store.turns_until_can_switch(),
        "turns_until_can_create": store.turns_until_can_create(),
        "current_turn": store.current_turn(),
    }))
}
