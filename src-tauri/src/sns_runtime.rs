use crate::{
    chat_partner::ChatPartner,
    sns_store::{Contact, Group, SnsStore},
    state::AppState,
};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tauri::{AppHandle, Emitter};

pub fn store() -> SnsStore {
    SnsStore::new(crate::utils::path::get_user_data_dir().join("sns/index.sqlite3"))
}
static PET_ACTORS: Lazy<RwLock<HashMap<String, Arc<ChatPartner>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
static GROUPS: Lazy<RwLock<HashMap<String, Option<Group>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
static GROUP_LOCKS: Lazy<parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));
pub fn group_lock(id: &str) -> Arc<tokio::sync::Mutex<()>> {
    GROUP_LOCKS
        .lock()
        .entry(id.into())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}
pub fn group_alive(id: &str) -> bool {
    GROUPS.read().get(id).map_or(true, |group| group.is_some())
}
pub fn update_group(group: Group) {
    GROUPS.write().insert(group.id.clone(), Some(group));
}
pub fn remove_group(id: &str) {
    GROUPS.write().insert(id.into(), None);
}
pub fn refresh_groups() {
    if let Ok(groups) = store().groups() {
        for group in groups {
            update_group(group);
        }
    }
}
pub fn scope_alive(speaker: &str, channel: &str) -> bool {
    let Some(id) = crate::sns_store::group_id(channel) else {
        return true;
    };
    if let Some(group) = GROUPS.read().get(id) {
        return group
            .as_ref()
            .is_some_and(|g| g.members.iter().any(|member| member == speaker));
    }
    let group = store().group(id).ok();
    let mut cached = GROUPS.write();
    cached
        .entry(id.into())
        .or_insert(group)
        .as_ref()
        .is_some_and(|g| g.members.iter().any(|member| member == speaker))
}
pub fn actor(state: &AppState, id: &str) -> Result<Arc<ChatPartner>, String> {
    if let Some(partner) = crate::chat_partner::get(id) {
        if !partner.is_deleted() {
            return Ok(partner);
        }
    }
    let instance = state.get_character(Some(id))?;
    let profile = Contact {
        id: id.into(),
        name: instance.name.clone(),
        persona: format!(
            "{}\n{}",
            instance.brain.persona.get_character_block(),
            instance.brain.persona.get_short_style_block(None)
        ),
        model: None,
        avatar: format!("/icons/charaters/{id}.webp"),
    };
    if let Some(partner) = PET_ACTORS.read().get(id) {
        *partner.profile.write() = profile;
        return Ok(partner.clone());
    }
    let partner = Arc::new(ChatPartner::new(profile));
    PET_ACTORS.write().insert(id.into(), partner.clone());
    Ok(partner)
}
/// Every private attachment has an owned copy; group assets belong to the group.
pub fn own_media(id: &str, mut metadata: Value) -> Result<Value, String> {
    let root = crate::utils::path::get_user_data_dir();
    let owned = crate::sns_store::owned_directory(&root, id, false)?.join("media");
    for key in ["image_path", "audio_path", "file_path"] {
        let Some(value) = metadata.get(key).and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let source = PathBuf::from(&value);
        let source = if source.is_absolute() {
            source
        } else {
            root.join(source)
        };
        if !source.is_file() {
            continue;
        }
        let source = source.canonicalize().map_err(|e| e.to_string())?;
        let trusted = root.canonicalize().map_err(|e| e.to_string())?;
        if !source.starts_with(&trusted) {
            return Err("附件不在应用数据目录内".into());
        }
        std::fs::create_dir_all(&owned).map_err(|e| e.to_string())?;
        if source.starts_with(owned.canonicalize().map_err(|e| e.to_string())?) {
            continue;
        }
        let ext = source.extension().and_then(|v| v.to_str()).unwrap_or("bin");
        let target = owned.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
        std::fs::copy(source, &target).map_err(|e| e.to_string())?;
        metadata[key] = json!(if key == "file_path" {
            target.to_string_lossy().to_string()
        } else {
            target
                .strip_prefix(&root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/")
        });
    }
    Ok(metadata)
}

pub async fn send_group(
    state: &Arc<AppState>,
    app: &AppHandle,
    id: String,
    message: String,
    request_id: String,
    image: Option<String>,
) -> Result<(), String> {
    let lock = group_lock(&id);
    let _turn =
        crate::chat_partner_policy::acquire_turn(&lock, std::time::Duration::from_secs(95)).await?;
    let group = store().group(&id)?;
    {
        let mut cached = GROUPS.write();
        if cached.get(&id).is_some_and(Option::is_none) {
            return Err("群已解散".into());
        }
        cached
            .entry(id.clone())
            .or_insert_with(|| Some(group.clone()));
    }
    if message.trim().is_empty() && image.is_none() {
        return Err("请输入消息或选择图片".into());
    }
    if crate::companion_quiet::active() {
        return Err("勿扰模式下暂不生成群回复".into());
    }
    let mut metadata = json!({});
    if let Some(path) = image {
        let (recognized, relative) = recognize_group_image(state, &id, path).await?;
        metadata = json!({"kind":"image","image_path":relative,"visual_description":recognized});
    }
    store().append(
        &id,
        "user",
        "user",
        if message.trim().is_empty() {
            "📷 [图片]"
        } else {
            message.trim()
        },
        metadata,
    )?;
    let _ = app.emit("sns:changed", json!({"group_id":id}));
    let channel = crate::sns_store::group_channel(&id);
    let participants = group
        .members
        .iter()
        .filter_map(|id| actor(state, id).ok())
        .collect::<Vec<_>>();
    let mentioned = participants
        .iter()
        .filter(|p| message.contains(&format!("@{}", p.profile.read().name)))
        .map(|p| p.id.clone())
        .collect::<Vec<_>>();
    for partner in participants {
        if !mentioned.is_empty() && !mentioned.contains(&partner.id) {
            continue;
        }
        if !scope_alive(&partner.id, &channel) {
            continue;
        }
        let stream = format!("{request_id}:{}", partner.id);
        if let Err(error) = Box::pin(crate::chat_partner::stream_reply(
            state,
            app,
            partner.clone(),
            message.clone(),
            stream.clone(),
            channel.clone(),
            None,
        ))
        .await
        {
            let _=app.emit("chat:error",json!({"error":error,"stream_id":stream,"character_id":partner.id,"channel":channel}));
        }
    }
    Ok(())
}
async fn recognize_group_image(
    state: &Arc<AppState>,
    id: &str,
    path: String,
) -> Result<(String, String), String> {
    use base64::Engine;
    if !state
        .config
        .read()
        .get_typed::<bool>("ai.enable_vision", false)
    {
        return Err("请先在设置中启用图片识别".into());
    }
    let root = crate::utils::path::get_user_data_dir();
    let owned = crate::sns_store::owned_directory(&root, id, true)?.join("media");
    let (image, relative) = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let source = PathBuf::from(path);
        let bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
        let mime = crate::commands::config::detect_image_mime(&bytes).to_string();
        std::fs::create_dir_all(&owned).map_err(|e| e.to_string())?;
        let target = owned.join(format!(
            "{}.{}",
            uuid::Uuid::new_v4(),
            source.extension().and_then(|v| v.to_str()).unwrap_or("png")
        ));
        std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
        let relative = target
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        Ok((
            crate::types::response::MessageImage {
                media_type: mime,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                url: None,
                detail: Some("auto".into()),
            },
            relative,
        ))
    })
    .await
    .map_err(|e| e.to_string())??;
    let router = state
        .model_router
        .read()
        .as_ref()
        .cloned()
        .ok_or("模型路由未初始化")?;
    let description = crate::chat_partner_policy::wait_for_model(
        crate::vision::recognize_image(&router, image, "用户在SNS群聊中分享的图片", None),
        || !group_alive(id),
        std::time::Duration::from_secs(90),
    )
    .await?
    .ok_or("群已解散")??;
    Ok((description, relative))
}

pub async fn private_image(
    state: &Arc<AppState>,
    app: &AppHandle,
    partner: Arc<ChatPartner>,
    source: String,
    channel: String,
) -> Result<String, String> {
    use base64::Engine;
    if channel != "wechat" {
        return Err("群图片请通过群聊界面发送".into());
    }
    crate::chat_partner_policy::validate_channel(&channel)?;
    if !state
        .config
        .read()
        .get_typed::<bool>("ai.enable_vision", false)
    {
        return Err("请先在设置中启用图片识别".into());
    }
    let root = crate::utils::path::get_user_data_dir();
    let owned = crate::sns_store::owned_directory(&root, &partner.id, false)?.join("media");
    let loader = partner.clone();
    let (image, relative, data_url) = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let _files = loader.memory_write_lock.lock();
        if loader.is_deleted() {
            return Err("好友已删除".into());
        }
        let bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
        let mime = crate::commands::config::detect_image_mime(&bytes).to_string();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        std::fs::create_dir_all(&owned).map_err(|e| e.to_string())?;
        let target = owned.join(format!("{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
        let relative = target
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let data_url = format!("data:{mime};base64,{encoded}");
        Ok((
            crate::types::response::MessageImage {
                media_type: mime,
                data: encoded,
                url: None,
                detail: Some("auto".into()),
            },
            relative,
            data_url,
        ))
    })
    .await
    .map_err(|e| e.to_string())??;
    let stream = uuid::Uuid::new_v4().to_string();
    let _=app.emit("chat:user_image",json!({"data_url":data_url,"image_path":relative,"timestamp":crate::sns_store::now(),"character_id":partner.id,"channel":channel}));
    let _=app.emit("chat:start",json!({"message":"📷 [图片]","stream_id":stream,"character_id":partner.id,"channel":channel,"source":"image"}));
    let router = state
        .model_router
        .read()
        .as_ref()
        .cloned()
        .ok_or("模型路由未初始化")?;
    let description = match crate::chat_partner_policy::wait_for_model(
        crate::vision::recognize_image(
            &router,
            image,
            "用户在SNS聊天里分享的图片",
            Some(&partner.id),
        ),
        || partner.is_deleted(),
        std::time::Duration::from_secs(90),
    )
    .await
    .and_then(|result| result.ok_or("好友已删除".into()))
    .and_then(|result| result)
    {
        Ok(description) => description,
        Err(error) => {
            let _=app.emit("chat:error",json!({"error":error,"stream_id":stream,"character_id":partner.id,"channel":channel}));
            return Err(error);
        }
    };
    if partner.is_deleted() {
        return Err("好友已删除".into());
    }
    let response = Box::pin(crate::chat_partner::stream_reply(
        state,
        app,
        partner,
        "📷 [图片]".into(),
        stream,
        channel,
        Some(json!({"kind":"image","image_path":relative,"visual_description":description})),
    ))
    .await?;
    Ok(response.map(|r| r.text).unwrap_or_default())
}
