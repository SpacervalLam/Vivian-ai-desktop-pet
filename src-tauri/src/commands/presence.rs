//! 在场状态命令

use std::sync::Arc;

use serde_json::json;
use tauri::{Emitter, Manager, State};

use crate::state::AppState;

/// 获取指定角色的在场状态
#[tauri::command]
pub fn get_presence_state(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let presence = &instance.brain.presence;
    let current = presence.current();
    Ok(json!({
        "character_id": presence.char_id(),
        "state": current.as_str(),
        "display_zh": current.display_zh(),
        "can_direct": current.can_direct(),
        "is_in_presence": current.is_in_presence(),
        "since": presence.since(),
        "elapsed_seconds": presence.elapsed_seconds(),
    }))
}

/// 获取所有角色的在场状态
#[tauri::command]
pub fn get_all_presence_states(
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let characters = state.characters.read().clone();
    let states: Vec<serde_json::Value> = characters
        .iter()
        .map(|(id, inst)| {
            let presence = &inst.brain.presence;
            let current = presence.current();
            json!({
                "character_id": id,
                "state": current.as_str(),
                "display_zh": current.display_zh(),
                "can_direct": current.can_direct(),
                "is_in_presence": current.is_in_presence(),
                "since": presence.since(),
                "elapsed_seconds": presence.elapsed_seconds(),
            })
        })
        .collect();
    Ok(json!(states))
}

/// 手动设置在场状态（设置面板用）
#[tauri::command]
pub async fn set_presence_state(
    character_id: Option<String>,
    target: String,
    state: State<'_, Arc<AppState>>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let target_state = crate::presence::PresenceState::from_str(&target);
    let current_state = instance.brain.presence.current();
    let task_in_progress = instance.brain.presence.is_task_in_progress();
    let event = instance.brain.presence.transition(
        target_state,
        crate::presence::PresenceChangeReason::UserInteraction,
    );

    // 任务进行中切回 Online 时，transition 会自动延迟（返回 None + 置 pending_exit）。
    // 这种情况下前端应感知：状态没切，给一个 deferred 反馈。
    if event.is_none() && target_state == crate::presence::PresenceState::Online && task_in_progress {
        let _ = app.emit(
            "presence:wake_deferred",
            serde_json::json!({
                "character_id": instance.brain.presence.char_id(),
                "from_state": current_state.as_str(),
                "task": if current_state == crate::presence::PresenceState::Busy {
                    "knowledge_acquisition"
                } else {
                    "memory_consolidation"
                },
                "hint": "我手上的事还没做完，等我做完就好",
            }),
        );
        return Ok(serde_json::json!({
            "changed": false,
            "deferred": true,
            "current": instance.brain.presence.current().as_str(),
        }));
    }

    if let Some(ref ev) = event {
        // 写 presence_log 记忆（与 chat.rs 用户唤醒路径保持一致）
        let memory_text = instance.brain.presence.memory_text(ev);

        // 内心独白信号：从 Rest/Offline → Online 时触发"醒来"思绪
        let to_state = crate::presence::PresenceState::from_str(&ev.to);
        let from_state = crate::presence::PresenceState::from_str(&ev.from);
        if matches!(to_state, crate::presence::PresenceState::Online)
            && matches!(from_state, crate::presence::PresenceState::Rest | crate::presence::PresenceState::Offline)
        {
            instance.brain.proactive.signal_waking_up();
        }
        // → Rest/Offline 时触发"要去休息"思绪
        if matches!(to_state, crate::presence::PresenceState::Rest | crate::presence::PresenceState::Offline) {
            instance.brain.proactive.signal_going_to_rest(match to_state {
                crate::presence::PresenceState::Rest => "累了想休息一下",
                _ => "有点想离线独处一会儿",
            });
        }

        let memory = instance.brain.memory.clone();
        let text = memory_text;
        let char_id_for_mem = instance.brain.presence.char_id().to_string();
        tokio::spawn(async move {
            use crate::memory::types::MemoryType;
            let meta = serde_json::json!({
                "channel": "presence",
                "speaker": char_id_for_mem,
                "listener": char_id_for_mem,
                "perspective": "speaker",
            });
            let _ = memory
                .add_memory_with_metadata(&text, MemoryType::ShortTerm, 0.4, vec!["presence_log".to_string(), "assistant".to_string()], meta)
                .await;
        });

        let _ = app.emit(
            "presence:changed",
            serde_json::json!({
                "character_id": instance.brain.presence.char_id(),
                "from": ev.from,
                "to": ev.to,
                "reason": ev.reason,
            }),
        );

        // 后端直接联动 Live2D 窗口可见性（与 proactive.rs 自动触发路径保持一致）：
        // - 切到 Offline：hide 窗口
        // - 从 Offline 切回 Online：show 窗口
        let char_id_for_win = instance.brain.presence.char_id().to_string();
        if let Some(win) = app.get_webview_window(&char_id_for_win) {
            if matches!(to_state, crate::presence::PresenceState::Offline) {
                let _ = win.hide();
                tracing::info!(
                    "[Presence:{}] set_presence_state 后端联动 hide 窗口（Offline）",
                    char_id_for_win
                );
            } else if matches!(from_state, crate::presence::PresenceState::Offline) {
                let _ = win.show();
                let _ = win.set_focus();
                tracing::info!(
                    "[Presence:{}] set_presence_state 后端联动 show 窗口（从 Offline 恢复）",
                    char_id_for_win
                );
            }
        }
    }

    Ok(serde_json::json!({
        "changed": event.is_some(),
        "current": instance.brain.presence.current().as_str(),
    }))
}
