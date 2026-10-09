//! User-controlled temporary silence. Office/work execution is deliberately independent.
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Instant};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use crate::{desktop_menu_policy::QuietTransitions, state::AppState};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static TRANSITIONS: Lazy<Mutex<QuietTransitions>> = Lazy::new(Default::default);
static RESTORE: Lazy<Mutex<Vec<String>>> = Lazy::new(Default::default);

pub fn active() -> bool { ACTIVE.load(Ordering::Acquire) }

pub fn blocks_route(task: &str) -> bool {
    active() && crate::providers::task_catalog::find(task).is_some_and(|spec| spec.spoken)
}

#[tauri::command]
pub fn companion_quiet_status() -> Value { json!({"active":active()}) }

#[tauri::command]
pub async fn set_companion_quiet(app: AppHandle, state: tauri::State<'_, Arc<AppState>>, enabled: bool) -> Result<Value, String> {
    let (revision, characters, notice) = {
        let mut transitions = TRANSITIONS.lock();
        let changed = transitions.set(enabled, Instant::now())
            .map_err(|remaining| format!("请在 {} 秒后再次开启勿扰", remaining.as_secs() + 1))?;
        if !changed { return Ok(json!({"active":transitions.active,"changed":false})); }
        ACTIVE.store(enabled, Ordering::Release);
        let revision = transitions.revision;
    let characters: Vec<_> = state.characters.read().values().cloned().collect();
    if enabled {
        let mut restore = RESTORE.lock();
        restore.clear();
        for character in &characters {
            state.set_generation_cancel(&character.id, true);
            let _ = character.brain.tts.stop();
            character.realtime_voice.stop_call();
            character.brain.proactive.drain_messages();
            for label in [character.id.clone(), format!("{}_bubble", character.id), format!("{}_toast", character.id)] {
                if let Some(window) = app.get_webview_window(&label) {
                    if window.is_visible().unwrap_or(false) { restore.push(label); let _ = window.hide(); }
                }
            }
        }
    } else {
        for label in RESTORE.lock().drain(..) {
            if let Some(window) = app.get_webview_window(&label) {
                // Old speech bubbles are never replayed on returning from quiet mode.
                if characters.iter().any(|character| character.id == label) { let _ = window.show(); }
            }
        }
    }
    let _ = app.emit("companion:quiet-changed", json!({"active":enabled,"revision":revision}));
    let notice = if enabled {
        "我现在要专注于手头的事情，请你们暂时保持安静。等我关闭勿扰模式后，再回来陪我。"
    } else { "我已结束专注，勿扰模式关闭了。请你们回来陪我，我们可以继续交流了。" };
    for character in &characters {
        character.brain.dialogue.add_message_with_metadata(crate::types::response::ChatMessage::user(notice),
            json!({"source":"quiet_control","channel":"control","speaker":"user","listener":character.id,"revision":revision}));
    }
        (revision, characters, notice)
    };
    // Stop queued audio too, including speech that was already scheduled before the gate closed.
    if enabled {
        let planner = crate::speech::get_planner().await;
        for character in &characters { let _ = planner.stop_speaker(&character.id).await; }
        state.playback_gate.mark_finished();
    }
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        for character in characters {
            // Every transition is recorded; only accepted entry generates one private thought per pet.
            let metadata = json!({"source":"quiet_control","channel":"control","revision":revision,"active":enabled});
            let _ = character.brain.memory.add_memory_with_metadata(notice,
                crate::memory::types::MemoryType::CasualConversation, 0.4,
                vec!["user".into(), "quiet_control".into()], metadata).await;
            if !enabled || !active() || TRANSITIONS.lock().revision != revision { continue; }
            let persona = character.brain.persona.get_config();
            let instructions = format!("你是 {}。角色设定：{persona:?}。用户要专注，已开启勿扰。请生成一段符合人设的第一人称内心独白（40-100字），表达理解和安静等待。只写内心活动，不向用户说话，不调用工具。", character.name);
            let request = crate::providers::base::LLMRequest::new("inner_monologue", vec![
                crate::types::response::ChatMessage::system(instructions), crate::types::response::ChatMessage::user(notice)])
                .with_character_id(character.id.clone()).with_usage_tag("quiet_transition").with_max_tokens(250);
            match tokio::time::timeout(std::time::Duration::from_secs(30), character.brain.router.generate(request)).await {
                Ok(Ok(thought)) if !thought.trim().is_empty() => {
                    let _ = character.brain.memory.add_memory_with_metadata(&thought,
                        crate::memory::types::MemoryType::InnerMonologue, 0.5,
                        vec!["inner_monologue".into(), "quiet_control".into()],
                        json!({"source":"inner_monologue","private":true,"speaker":character.id,"revision":revision,"trigger":"quiet_mode"})).await;
                }
                result => tracing::warn!(character = %character.id, ?result, "勿扰独白未生成，保持静默"),
            }
        }
        drop(state);
    });
    Ok(json!({"active":active(),"changed":true}))
}
