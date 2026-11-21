use crate::{
    sns_store::{Contact, ContactInput, Group, GroupMessage},
    state::AppState,
};
use serde_json::json;
use std::{collections::HashSet, sync::Arc};
use tauri::{AppHandle, Emitter, State};
pub(crate) static MUTATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
fn chat(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "chat" {
        Ok(())
    } else {
        Err("请在聊天窗口管理好友和群聊".into())
    }
}
#[tauri::command]
pub async fn sns_save_contact(
    window: tauri::WebviewWindow,
    id: Option<String>,
    input: ContactInput,
    app: AppHandle,
) -> Result<Contact, String> {
    chat(&window)?;
    let _guard = MUTATE.lock().await;
    let profile = crate::sns_runtime::store().save_contact(id.as_deref(), input)?;
    crate::chat_partner::register(profile.clone());
    let _ = app.emit("sns:changed", json!({"contact_id":profile.id}));
    Ok(profile)
}
#[tauri::command]
pub fn sns_get_contact(window: tauri::WebviewWindow, id: String) -> Result<Contact, String> {
    chat(&window)?;
    crate::sns_runtime::store().contact(&id)
}
#[tauri::command]
pub async fn sns_delete_contact(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: String,
    app: AppHandle,
) -> Result<(), String> {
    chat(&window)?;
    if !crate::sns_store::valid_contact_id(&id) {
        return Err("Vivian 和 Nana 无法删除".into());
    }
    let _guard = MUTATE.lock().await;
    let partner = crate::chat_partner::get(&id).ok_or("好友不存在")?;
    state.set_generation_cancel(&id, true);
    partner.delete_owned().await?;
    crate::sns_runtime::store().delete_contact(&id)?;
    crate::chat_partner::unregister(&id);
    crate::sns_runtime::refresh_groups();
    if id == "deepseek" {
        let config = state.config.read();
        config
            .set(
                "routing_matrix.chat_partner",
                serde_json::to_value(crate::config::manager::TaskRouteConfig::default())
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("sns:changed", json!({"deleted_contact":id}));
    let _ = app.emit("chat:history-cleared", json!({"character_id":id}));
    Ok(())
}
#[tauri::command]
pub fn sns_list_groups() -> Result<Vec<Group>, String> {
    crate::sns_runtime::store().groups()
}
#[tauri::command]
pub async fn sns_save_group(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: Option<String>,
    name: String,
    members: Vec<String>,
    app: AppHandle,
) -> Result<Group, String> {
    chat(&window)?;
    let _guard = MUTATE.lock().await;
    let mut allowed: HashSet<String> = state.characters.read().keys().cloned().collect();
    allowed.extend(
        crate::chat_partner::all()
            .into_iter()
            .filter(|p| !p.is_deleted())
            .map(|p| p.id.clone()),
    );
    let group = crate::sns_runtime::store().save_group(id.as_deref(), &name, members, &allowed)?;
    crate::sns_runtime::update_group(group.clone());
    let _ = app.emit("sns:changed", json!({"group_id":group.id}));
    Ok(group)
}
#[tauri::command]
pub async fn sns_delete_group(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: String,
    app: AppHandle,
) -> Result<(), String> {
    chat(&window)?;
    let _guard = MUTATE.lock().await;
    crate::sns_runtime::store().group(&id)?;
    crate::sns_runtime::remove_group(&id);
    crate::sns_runtime::store().delete_group(&id)?;
    let turn = crate::sns_runtime::group_lock(&id);
    let _turn =
        crate::chat_partner_policy::acquire_turn(&turn, std::time::Duration::from_secs(95)).await?;

    let channel = crate::sns_store::group_channel(&id);
    let mut ids = crate::sns_runtime::store()
        .contacts()?
        .into_iter()
        .map(|c| c.id)
        .collect::<Vec<_>>();
    ids.extend(state.characters.read().keys().cloned());
    for contact in ids {
        let path = crate::utils::path::get_companion_data_dir(&contact)
            .join("chat_partner_memory.sqlite3");
        if path.exists() {
            crate::chat_partner_memory::SemanticMemory::new(path).clear_channel(&channel)?;
        }
    }
    let root = crate::utils::path::get_user_data_dir();
    let path = crate::sns_store::owned_directory(&root, &id, true)?;
    if path.exists() {
        crate::sns_store::verify_removal(&root, &path)?;
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    }
    let _ = app.emit("sns:changed", json!({"deleted_group":id}));
    Ok(())
}
#[tauri::command]
pub fn sns_group_history(
    id: String,
    before: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<GroupMessage>, String> {
    crate::sns_runtime::store().history(&id, before.as_deref(), limit.unwrap_or(80))
}
#[tauri::command]
pub async fn sns_send_group_message(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: String,
    message: String,
    request_id: String,
    image_path: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    chat(&window)?;
    Box::pin(crate::sns_runtime::send_group(
        state.inner(),
        &app,
        id,
        message,
        request_id,
        image_path,
    ))
    .await
}
