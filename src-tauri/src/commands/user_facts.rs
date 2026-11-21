//! 用户事实画像命令 —— 为前端提供查看/编辑/锁定用户画像能力。
//!
//! 五个命令：
//! - [`get_user_facts`]：获取指定角色的全部用户事实（L0 + L0.5 + L1 + L2）
//! - [`set_user_fact`]：手动设置/覆盖一条事实
//! - [`pin_user_fact`]：锁定/解锁基础字段（锁定后不被自动覆盖）
//! - [`delete_user_fact`]：删除一条事实
//! - [`get_user_fact_types`]：获取所有支持的事实类型枚举（前端下拉用）

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::memory::user_facts::{L1RecentState, UserFact, UserFactType};
use crate::state::AppState;

/// 前端视图：单条用户事实
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFactView {
    pub fact_type: String,
    pub label: String,
    pub content: String,
    pub confidence: f64,
    pub timestamp: f64,
    pub is_pinned: bool,
    pub is_manual: bool,
}

/// 前端视图：完整用户画像
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfileView {
    /// L0 + L0.5 唯一字段（按固定顺序）
    pub basic_facts: Vec<UserFactView>,
    /// L1 近期状态
    pub recent_state: L1RecentState,
    /// L2 自由事实
    pub custom_facts: Vec<UserFactView>,
}

/// 获取指定角色的用户画像
#[tauri::command]
pub async fn get_user_facts(
    state: State<'_, Arc<AppState>>,
    character_id: String,
) -> Result<UserProfileView, String> {
    let brain = {
        let characters = state.characters.read();
        characters.get(&character_id).ok_or_else(|| format!("角色不存在: {character_id}"))?.brain.clone()
    };
    let chat_chain = brain
        .chat_chain
        .as_ref()
        .ok_or_else(|| "ChatChain 未初始化".to_string())?;
    let store = &chat_chain.user_facts;

    // 补全自动画像停用期间漏记的资料；只读取真实用户原话，不读取角色总结。
    if store.needs_identity_recovery() {
        if let Ok(mut memories) = brain.memory.get_all_memories().await {
            memories.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
            let raw_turns: Vec<String> = memories.into_iter().filter(|item| {
                matches!(item.memory_type.as_str(), "short_term" | "casual_conversation")
                    && item.tags.iter().any(|tag| tag == "dialogue_turn" || tag == "user")
                    && !item.tags.iter().any(|tag| tag == "topic_summary" || tag == "session_summary")
                    && item.metadata.get("speaker").and_then(serde_json::Value::as_str) == Some("user")
                    && !item.metadata.get("system_directive").and_then(serde_json::Value::as_bool).unwrap_or(false)
                    && !item.content.contains("现在你想插话")
            }).map(|item| {
                crate::cross_character::parse_any_speaker_prefix(&item.content).0
            }).collect();
            // 优先保留自我介绍，避免名字在最近几十轮之外时再次被遗漏。
            let mut remaining = 8000usize;
            let mut seen = std::collections::HashSet::new();
            let mut selected: Vec<(usize, String)> = raw_turns.iter().enumerate().rev().filter(|(_, text)| {
                ["我叫", "我是", "名字", "叫我", "my name", "call me", "I'm", "名前"]
                    .iter().any(|cue| text.contains(cue))
            }).chain(raw_turns.iter().enumerate().rev().take(20)).filter_map(|(index, text)| {
                if !seen.insert(index) { return None; }
                let length = text.chars().count();
                if length > remaining { return None; }
                remaining -= length;
                Some((index, text.clone()))
            }).take(40).collect();
            selected.sort_by_key(|(index, _)| *index);
            let turns: Vec<String> = selected.into_iter().map(|(_, text)| text).collect();
            if let Err(error) = store.recover_from_user_turns(&turns).await {
                tracing::warn!("[UserFacts] 历史用户资料补全失败: {}", error);
            }
        }
    }

    let (basic_data, custom_facts) = store.get_all_facts();
    let recent_state = store.get_recent_state();

    // 按固定顺序输出基础字段
    let ordered_types = [
        UserFactType::Name,
        UserFactType::Age,
        UserFactType::Gender,
        UserFactType::Occupation,
        UserFactType::Location,
        UserFactType::Birthday,
        UserFactType::SleepSchedule,
        UserFactType::FavoriteWebsite,
        UserFactType::FavoriteGame,
        UserFactType::Hobby,
    ];
    let basic_facts: Vec<UserFactView> = ordered_types
        .iter()
        .filter_map(|t| {
            basic_data.get(t).map(|f| fact_to_view(f, t.label_zh()))
        })
        .collect();

    let custom_views: Vec<UserFactView> = custom_facts
        .iter()
        .map(|f| fact_to_view(f, UserFactType::Custom.label_zh()))
        .collect();

    Ok(UserProfileView {
        basic_facts,
        recent_state,
        custom_facts: custom_views,
    })
}

/// 手动设置/覆盖一条用户事实
#[tauri::command]
pub fn set_user_fact(
    state: State<'_, Arc<AppState>>,
    character_id: String,
    fact_type: String,
    content: String,
    pinned: Option<bool>,
) -> Result<(), String> {
    let fact_type = UserFactType::from_str(&fact_type)
        .ok_or_else(|| format!("未知的事实类型: {fact_type}"))?;
    let characters = state.characters.read();
    let instance = characters
        .get(&character_id)
        .ok_or_else(|| format!("角色不存在: {character_id}"))?;
    let chat_chain = instance
        .brain
        .chat_chain
        .as_ref()
        .ok_or_else(|| "ChatChain 未初始化".to_string())?;
    chat_chain
        .user_facts
        .set_fact(fact_type, &content, pinned.unwrap_or(false))
        .map_err(|e| format!("设置用户事实失败: {e}"))
}

/// 锁定/解锁基础字段
#[tauri::command]
pub fn pin_user_fact(
    state: State<'_, Arc<AppState>>,
    character_id: String,
    fact_type: String,
    pinned: bool,
) -> Result<(), String> {
    let fact_type = UserFactType::from_str(&fact_type)
        .ok_or_else(|| format!("未知的事实类型: {fact_type}"))?;
    let characters = state.characters.read();
    let instance = characters
        .get(&character_id)
        .ok_or_else(|| format!("角色不存在: {character_id}"))?;
    let chat_chain = instance
        .brain
        .chat_chain
        .as_ref()
        .ok_or_else(|| "ChatChain 未初始化".to_string())?;
    chat_chain
        .user_facts
        .set_pinned(fact_type, pinned)
        .map_err(|e| format!("锁定用户事实失败: {e}"))
}

/// 删除一条用户事实
#[tauri::command]
pub fn delete_user_fact(
    state: State<'_, Arc<AppState>>,
    character_id: String,
    fact_type: String,
    content: Option<String>,
) -> Result<(), String> {
    let fact_type = UserFactType::from_str(&fact_type)
        .ok_or_else(|| format!("未知的事实类型: {fact_type}"))?;
    let characters = state.characters.read();
    let instance = characters
        .get(&character_id)
        .ok_or_else(|| format!("角色不存在: {character_id}"))?;
    let chat_chain = instance
        .brain
        .chat_chain
        .as_ref()
        .ok_or_else(|| "ChatChain 未初始化".to_string())?;
    chat_chain
        .user_facts
        .delete_fact(fact_type, content.as_deref())
        .map_err(|e| format!("删除用户事实失败: {e}"))
}

/// 获取所有支持的事实类型（前端下拉选项用）
#[tauri::command]
pub fn get_user_fact_types() -> Result<Vec<HashMap<String, String>>, String> {
    let types = [
        UserFactType::Name,
        UserFactType::Age,
        UserFactType::Gender,
        UserFactType::Occupation,
        UserFactType::Location,
        UserFactType::Birthday,
        UserFactType::SleepSchedule,
        UserFactType::FavoriteWebsite,
        UserFactType::FavoriteGame,
        UserFactType::Hobby,
        UserFactType::Custom,
    ];
    Ok(types
        .iter()
        .map(|t| {
            let mut m = HashMap::new();
            m.insert("value".to_string(), t.as_str().to_string());
            m.insert("label".to_string(), t.label_zh().to_string());
            m
        })
        .collect())
}

fn fact_to_view(f: &UserFact, label: &str) -> UserFactView {
    UserFactView {
        fact_type: f.fact_type.as_str().to_string(),
        label: label.to_string(),
        content: f.content.clone(),
        confidence: f.confidence,
        timestamp: f.timestamp,
        is_pinned: f.is_pinned,
        is_manual: f.reasoning.as_deref() == Some("manual_edit"),
    }
}
