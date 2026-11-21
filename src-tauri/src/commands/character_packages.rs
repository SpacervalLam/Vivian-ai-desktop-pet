use crate::character_packages::{self, CharacterPackage};
#[tauri::command]
pub fn character_package_status(
    state: tauri::State<'_, std::sync::Arc<crate::state::AppState>>,
) -> serde_json::Value {
    let mut status = character_packages::character_package_status();
    status["initialized"] =
        serde_json::json!(state.characters.read().keys().cloned().collect::<Vec<_>>());
    status["initializing"] = (!state.is_initialized()).into();
    status
}
use tauri::Emitter;
#[tauri::command]
pub async fn import_character_package(
    source: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, std::sync::Arc<crate::state::AppState>>,
) -> Result<CharacterPackage, String> {
    let lifecycle = state.reinitialize_lock.clone().lock_owned().await;
    let manifest = tauri::async_runtime::spawn_blocking(move || {
        character_packages::import_character_package(source)
    })
    .await
    .map_err(|e| e.to_string())??;
    let id = manifest.id.clone();
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let _lifecycle = lifecycle;
        let result = load_live(&app, &state, &id).await;
        if let Err(error) = &result {
            character_packages::report_error(format!("角色 {id} 热装载失败: {error}"));
        }
        let _=app.emit("character-package:changed",serde_json::json!({"id":id,"phase":if result.is_ok(){"loaded"}else{"error"},"error":result.err()}));
    });
    Ok(manifest)
}
async fn load_live(
    app: &tauri::AppHandle,
    state: &std::sync::Arc<crate::state::AppState>,
    id: &str,
) -> Result<(), String> {
    if !state.is_initialized() {
        return Err("应用仍在初始化，请稍后重新导入".into());
    }
    let existing = state.characters.read().get(id).cloned();
    let _idle = if let Some(c) = &existing {
        loop {
            if !c.brain.presence.is_task_in_progress() && !state.playback_gate.is_playing() {
                let guard = c.think_lock.lock().await;
                if !c.brain.presence.is_task_in_progress() && !state.playback_gate.is_playing() {
                    break Some(guard);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    } else {
        None
    };
    if state.is_factory_reset_in_progress() || state.is_rebuild_in_progress() {
        return Err("正在维护数据，请稍后重新导入".into());
    }
    let previous = character_packages::active_root(id);
    let old_factory = crate::persona::schemas::default_persona_for(id);
    let changed = character_packages::activate(id)?;
    if !changed && existing.is_some() {
        return Ok(());
    }
    let result: Result<(), String> = async {
        let config_manager = state.config.read().clone();
        config_manager
            .replace_all(config_manager.get_all())
            .map_err(|e| e.to_string())?;
        state.reload_model_router()?;
        if let Some(router) = state.model_router.read().as_ref() {
            router.set_app_handle(app.clone());
        }
        if let Some(character) = &existing {
            character
                .brain
                .persona
                .refresh_package(id, &old_factory)
                .map_err(|e| e.to_string())?;
        } else {
            let config = state.config.read().get_all();
            let entry = config
                .characters
                .list
                .iter()
                .find(|c| c.id == id)
                .ok_or("角色配置未注册")?;
            let router = state.model_router.read().clone().ok_or("模型路由未就绪")?;
            let instance = state.build_character(entry, router.clone()).await?;
            crate::character_registry::register_character(
                id,
                (*instance.brain.memory).clone(),
                instance.brain.psychology.clone(),
                instance.manifest.clone(),
                std::sync::Arc::new(router),
            );
            crate::character_registry::register_brain(id, instance.brain.clone());
            state.characters.write().insert(id.into(), instance.clone());
            crate::start_character_services(app, &instance);
            state.sync_tts_managers().await;
            if *instance.online.read() {
                instance.pet_controller.start();
            }
            crate::brain::resident::ensure_started(app, state);
        }
        state.notify_residency();
        crate::create_character_windows(app, state);
        let online = state
            .get_character(Some(id))
            .map(|c| *c.online.read())
            .unwrap_or(false);
        let _ = app.emit(
            "character:online_changed",
            serde_json::json!({"character_id":id,"online":online}),
        );
        Ok(())
    }
    .await;
    if let Err(error) = &result {
        character_packages::restore_active(id, previous)
            .map_err(|rollback| format!("{error}；恢复角色包失败: {rollback}"))?;
        let manager = state.config.read().clone();
        let _ = manager.replace_all(manager.get_all());
        let _ = state.reload_model_router();
        if let Some(router) = state.model_router.read().as_ref() {
            router.set_app_handle(app.clone());
        }
        // Reconcile only the factory corpus; preserve local edits in its three-way merge.
        if existing.is_some() {
            let _ = crate::persona::example_library::ensure_base(id);
        }
    }
    result
}
