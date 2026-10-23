use crate::{
    config::{transfer::*, AppConfig},
    state::AppState,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::{AppHandle, Emitter, State};

fn settings_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "config" {
        Ok(())
    } else {
        Err("请在设置窗口迁移配置".into())
    }
}

fn read_package(path: &str) -> Result<SettingsPackage, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    decode(&bytes)
}

fn read_optional(path: &Path) -> Result<Option<Value>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub fn export_settings_share(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    path: String,
    include_characters: bool,
) -> Result<(), String> {
    settings_window(&window)?;
    let destination = PathBuf::from(&path);
    if !destination
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("altn"))
    {
        return Err("请选择 .altn 文件".into());
    }
    let root = crate::utils::path::get_user_data_dir();
    if crate::storage::backup::is_path_inside(&destination, &root) {
        return Err("请将配置分享文件保存到应用数据目录之外".into());
    }
    let config = state.config.read().get_all();
    let mut settings = serde_json::to_value(&config).map_err(|e| e.to_string())?;
    clear_secrets(&mut settings);
    // Learned observations are memory, not user preferences.
    settings["companion"]["game_processes"] = json!([]);
    let mut character_settings = BTreeMap::new();
    for entry in &config.characters.list {
        let speech = if let Some(character) = state.characters.read().get(&entry.id) {
            serde_json::to_value(character.brain.tts.get_config()).map_err(|e| e.to_string())?
        } else {
            read_optional(
                &root
                    .join("characters")
                    .join(&entry.id)
                    .join("sound/config.json"),
            )?
            .unwrap_or(
                serde_json::to_value(crate::speech::TtsConfig::default())
                    .map_err(|e| e.to_string())?,
            )
        };
        let mut speech = speech;
        clear_secrets(&mut speech);
        character_settings.insert(
            entry.id.clone(),
            CharacterSettings {
                speech,
                diary: serde_json::to_value(
                    crate::diary::get_config(&entry.id).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?,
            },
        );
    }
    let mut extra_settings = BTreeMap::new();
    for path in EXTRA_FILES {
        let mut value = read_optional(&root.join(path))?.unwrap_or_else(|| match *path {
            "mcp/servers.json" | "plugins/.disabled.json" | "stickers/catalog.json" => json!([]),
            "plugins/llm-providers/providers.json"
            | "plugins/llm-providers/embedding-providers.json" => Value::Null,
            _ => json!({}),
        });
        clear_secrets(&mut value);
        extra_settings.insert(path.to_string(), value);
    }
    let chat_partners = if include_characters {
        crate::sns_runtime::store()
            .contacts()?
            .into_iter()
            .map(|contact| {
                let mut model = contact.model;
                if let Some(model) = &mut model {
                    clear_secrets(model);
                }
                crate::sns_store::ContactInput {
                    name: contact.name,
                    persona: contact.persona,
                    model,
                    avatar: contact.avatar,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let user_avatar = match &config.base.user_avatar_path {
        Some(path) if !path.is_empty() => {
            let avatar = root.join(path);
            if !crate::storage::backup::is_path_inside(&avatar, &root) {
                return Err("用户头像路径无效".into());
            }
            crate::commands::config::image_to_data_url(&avatar)?
        }
        _ => None,
    };
    let mut sticker_images = BTreeMap::new();
    if let Some(catalog) = extra_settings.get("stickers/catalog.json") {
        let rows: Vec<crate::stickers::StickerEntry> =
            serde_json::from_value(catalog.clone()).map_err(|e| e.to_string())?;
        for row in rows {
            if !valid_sticker_filename(&row.filename) {
                return Err("贴纸文件名无效".into());
            }
            let bytes = std::fs::read(root.join("stickers").join(&row.filename))
                .map_err(|e| e.to_string())?;
            sticker_images.insert(row.filename, STANDARD.encode(bytes));
        }
    }
    let package = SettingsPackage {
        format: FORMAT.into(),
        version: VERSION,
        settings,
        character_settings,
        extra_settings,
        chat_partners,
        user_avatar,
        sticker_images,
    };
    let bytes = serde_json::to_vec_pretty(&package).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("配置分享文件超过 64 MiB，请减少角色头像大小".into());
    }
    crate::utils::fs::atomic_write(&destination, &bytes).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn inspect_settings_share(window: tauri::WebviewWindow, path: String) -> Result<Value, String> {
    settings_window(&window)?;
    let package = read_package(&path)?;
    Ok(json!({"characters": package.chat_partners.iter().map(|c| &c.name).collect::<Vec<_>>()}))
}

#[tauri::command]
pub async fn import_settings_share(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: String,
    include_characters: bool,
) -> Result<usize, String> {
    settings_window(&window)?;
    let _guard = crate::commands::sns::MUTATE.lock().await;
    let mut package = read_package(&path)?;
    let manager = state.config.read().clone();
    let previous = manager.get_all();
    retain_local_secrets(
        &mut package.settings,
        &serde_json::to_value(&previous).map_err(|e| e.to_string())?,
    );
    let mut next: AppConfig =
        serde_json::from_value(package.settings).map_err(|e| format!("设置数据无效: {e}"))?;
    next.companion.game_processes = previous.companion.game_processes.clone();
    let safe_id = |id: &str| {
        !id.is_empty()
            && id.len() <= 100
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    if next.characters.list.iter().any(|c| !safe_id(&c.id))
        || package
            .character_settings
            .keys()
            .any(|id| !safe_id(id) || !next.characters.list.iter().any(|c| &c.id == id))
    {
        return Err("角色设置标识无效".into());
    }
    let root = crate::utils::path::get_user_data_dir();
    let mut writes: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for (filename, encoded) in &package.sticker_images {
        if !valid_sticker_filename(filename) {
            return Err("贴纸文件名无效".into());
        }
        let bytes = STANDARD.decode(encoded).map_err(|_| "贴纸图片编码无效")?;
        if bytes.len() < 24
            || bytes.len() > 2 * 1024 * 1024
            || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return Err("贴纸图片格式或大小无效".into());
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        if width == 0 || height == 0 || width > 2048 || height > 2048 {
            return Err("贴纸图片尺寸无效".into());
        }
        tauri::image::Image::from_bytes(&bytes).map_err(|_| "贴纸 PNG 图片损坏")?;
        writes.push((root.join("stickers").join(filename), bytes));
    }
    for (id, mut config) in package.character_settings {
        let directory = root.join("characters").join(&id);
        let speech_path = directory.join("sound/config.json");
        if let Some(local) = read_optional(&speech_path)? {
            retain_local_secrets(&mut config.speech, &local);
        }
        let speech: crate::speech::TtsConfig =
            serde_json::from_value(config.speech).map_err(|e| format!("语音设置无效: {e}"))?;
        let diary: crate::diary::DiaryConfig =
            serde_json::from_value(config.diary).map_err(|e| format!("日记设置无效: {e}"))?;
        writes.push((
            speech_path,
            serde_json::to_vec_pretty(&speech).map_err(|e| e.to_string())?,
        ));
        writes.push((
            directory.join("diary/config.json"),
            serde_json::to_vec_pretty(&diary).map_err(|e| e.to_string())?,
        ));
    }
    for (path, mut value) in package.extra_settings {
        if value.is_null() {
            continue;
        }
        let target = root.join(&path);
        if let Some(local) = read_optional(&target)? {
            retain_local_secrets(&mut value, &local);
        }
        match path.as_str() {
            "mcp/servers.json" => {
                serde_json::from_value::<Vec<crate::tools::mcp::McpServerConfig>>(value.clone())
                    .map_err(|e| format!("MCP 设置无效: {e}"))?;
            }
            "plugins/.disabled.json" => {
                serde_json::from_value::<Vec<String>>(value.clone()).map_err(|e| e.to_string())?;
            }
            "stickers/settings.json" => {
                serde_json::from_value::<BTreeMap<String, String>>(value.clone())
                    .map_err(|e| e.to_string())?;
            }
            "stickers/catalog.json" => {
                let rows: Vec<crate::stickers::StickerEntry> =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                if rows.iter().any(|row| {
                    !valid_sticker_filename(&row.filename)
                        || row.filename != format!("{}-{}.png", row.id, row.version)
                        || !matches!(row.character_id.as_str(), "vivian" | "nana")
                        || !package.sticker_images.contains_key(&row.filename)
                }) {
                    return Err("贴纸目录无效或缺少图片".into());
                }
            }
            _ => {}
        }
        writes.push((
            target,
            serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?,
        ));
    }
    next.base.user_avatar_path = None;
    if let Some(avatar) = package.user_avatar {
        let (_, encoded) = avatar
            .split_once(";base64,")
            .filter(|(prefix, _)| prefix.starts_with("data:image/"))
            .ok_or("用户头像格式无效")?;
        let bytes = STANDARD.decode(encoded).map_err(|_| "用户头像编码无效")?;
        if bytes.len() < 12 || bytes.len() > 8 * 1024 * 1024 {
            return Err("用户头像大小无效".into());
        }
        next.base.user_avatar_path = Some("avatar-shared.png".into());
        writes.push((root.join("avatar-shared.png"), bytes));
    }
    // Snapshot all targets before the first write. Roll back on any failed write or
    // contact validation/transaction, so a malformed role cannot partially import settings.
    let mut main_replaced = false;
    let result = apply_files(&writes, || {
        manager.replace_all(next).map_err(|e| e.to_string())?;
        main_replaced = true;
        if include_characters {
            crate::sns_runtime::store().append_shared_contacts(package.chat_partners)
        } else {
            Ok(Vec::new())
        }
    });
    let profiles = match result {
        Ok(profiles) => profiles,
        Err(error) => {
            if main_replaced {
                if let Err(e) = manager.replace_all(previous) {
                    return Err(format!("{error}；主设置回滚失败：{e}"));
                }
            }
            return Err(error);
        }
    };
    let count = profiles.len();
    for profile in profiles {
        crate::chat_partner::register(profile);
    }
    let _ = app.emit("sns:changed", json!({}));
    Ok(count)
}

#[tauri::command]
pub fn restart_after_settings_import(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<(), String> {
    settings_window(&window)?;
    app.request_restart();
    Ok(())
}
