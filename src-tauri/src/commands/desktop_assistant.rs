use std::sync::Arc;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use crate::{companion_policy::DesktopShortcut, state::AppState};

#[tauri::command]
pub async fn user_quick_notes_list() -> Result<Vec<crate::user_quick_notes::QuickNote>, String> {
    tokio::task::spawn_blocking(crate::user_quick_notes::list).await.map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn user_quick_notes_save(app: AppHandle, content: String) -> Result<crate::user_quick_notes::QuickNote, String> {
    let note = tokio::task::spawn_blocking(move || crate::user_quick_notes::save(&content)).await.map_err(|e| e.to_string())??;
    let _ = app.emit("user_quick_notes:changed", ());
    Ok(note)
}
#[tauri::command]
pub async fn user_quick_notes_delete(app: AppHandle, id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::user_quick_notes::delete(&id)).await.map_err(|e| e.to_string())??;
    let _ = app.emit("user_quick_notes:changed", ());
    Ok(())
}

#[tauri::command]
pub fn companion_desktop_context(state: State<'_, Arc<AppState>>) -> Value {
    let (weather, music) = state.world_provider.companion_scene();
    json!({"weather":weather,"music":music,"shortcuts":state.config.read().get_all().companion.shortcuts,
        "quiet_reason":crate::companion_runtime::quiet_reason(&state)})
}

#[tauri::command]
pub fn companion_save_shortcuts(app: AppHandle, state: State<'_, Arc<AppState>>, shortcuts: Vec<DesktopShortcut>) -> Result<(), String> {
    if shortcuts.len() > 30 { return Err("最多保存 30 个快捷入口。".into()); }
    let mut ids = std::collections::HashSet::new();
    for shortcut in &shortcuts {
        if shortcut.name.trim().is_empty() || shortcut.name.chars().count() > 80 || shortcut.id.is_empty() || !ids.insert(&shortcut.id) {
            return Err("快捷入口名称或 ID 无效。".into());
        }
        match shortcut.kind.as_str() {
            "website" => {
                let url = reqwest::Url::parse(&shortcut.target).map_err(|_| "网址无效")?;
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
                    return Err("书签仅支持不带登录信息的 HTTP/HTTPS 网址。".into());
                }
            }
            "application" if std::path::Path::new(&shortcut.target).is_absolute() && std::path::Path::new(&shortcut.target).is_file() => {}
            _ => return Err("请填写已存在的应用完整路径。".into()),
        }
    }
    state.config.read().set("companion.shortcuts", serde_json::to_value(shortcuts).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let _ = app.emit("config:saved", ());
    Ok(())
}

async fn explicit_tool(state: &AppState, character_id: &str, name: &str, args: Value) -> Result<Value, String> {
    state.get_character(Some(character_id))?;
    let mut context = crate::tools::types::ToolUseContext::new("desktop-shortcut", "user");
    context.char_id = character_id.into();
    context.user_message = Some(format!("User clicked the desktop {name} action"));
    let approved_name = name.to_string();
    let approved_args = args.clone();
    let approval: crate::tools::CanUseTool = Arc::new(move |name, args| name == approved_name && *args == approved_args);
    let result = crate::tools::execute_tool_use(name, args, &state.tool_system, &context, Some(approval)).await;
    if !result.success { return Err(result.error.unwrap_or_else(|| result.data.map(|d| d.to_string()).unwrap_or_else(|| "操作失败".into()))); }
    Ok(result.data.unwrap_or(Value::Null))
}

#[tauri::command]
pub async fn companion_launch_shortcut(state: State<'_, Arc<AppState>>, shortcut_id: String, character_id: String) -> Result<Value, String> {
    let shortcut = state.config.read().get_all().companion.shortcuts.into_iter().find(|item| item.id == shortcut_id).ok_or("快捷入口不存在")?;
    let (name, args) = if shortcut.kind == "website" { ("open_url", json!({"url":shortcut.target})) }
        else if shortcut.kind == "application" { ("open_application", json!({"application":shortcut.target})) }
        else { return Err("快捷入口类型无效".into()); };
    explicit_tool(&state, &character_id, name, args).await
}

#[tauri::command]
pub async fn companion_screen_analyze(app: AppHandle, state: State<'_, Arc<AppState>>, character_id: String) -> Result<Value, String> {
    analyze_screen(&app, &state, &character_id).await
}

static SCREEN_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SCREEN_CAPTURING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn screen_capture_in_progress() -> bool {
    SCREEN_CAPTURING.load(std::sync::atomic::Ordering::Acquire)
}
struct ScreenGuard(Vec<tauri::WebviewWindow>);
impl Drop for ScreenGuard {
    fn drop(&mut self) {
        for window in &self.0 { let _ = window.show(); }
        SCREEN_CAPTURING.store(false, std::sync::atomic::Ordering::Release);
        SCREEN_BUSY.store(false, std::sync::atomic::Ordering::Release);
    }
}
pub async fn analyze_screen(app: &AppHandle, state: &Arc<AppState>, character_id: &str) -> Result<Value, String> {
    if SCREEN_BUSY.compare_exchange(false, true, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire).is_err() {
        return Err("截图分析正在进行中".into());
    }
    let mut guard = ScreenGuard(Vec::new());
    SCREEN_CAPTURING.store(true, std::sync::atomic::Ordering::Release);
    let hide_started = std::time::Instant::now();
    guard.0 = crate::screen_selection::hide_capture_entry(app).await?;
    tracing::info!(hide_ms = hide_started.elapsed().as_millis(), "截图入口窗口已隐藏并完成合成");
    state.get_character(Some(character_id))?;
    let Some(selected) = crate::screen_selection::select_region(app).await? else {
        return Ok(json!({"cancelled":true}));
    };
    for window in guard.0.drain(..) { let _ = window.show(); }
    SCREEN_CAPTURING.store(false, std::sync::atomic::Ordering::Release);
    let copied = selected.action.copies();
    if copied { crate::desktop_clipboard::copy_png(&selected.png).await?; }
    let saved_path = if selected.action.saves() {
        Some(crate::screen_capture::save_selected_png(&selected.png).await?)
    } else { None };
    if !selected.action.analyzes() {
        return Ok(json!({"saved_path":saved_path,"copied":copied}));
    }
    let analysis = async {
        if crate::companion_quiet::active() { return Err("勿扰模式下已暂停陪伴分析，请先关闭勿扰".into()); }
        if state.tool_system.is_tool_disabled("screenshot_analyze", crate::tools::registry::AgentSide::Companion) {
            return Err("截图分析已在工具设置中禁用".to_string());
        }
        if !state.config.read().get_typed::<bool>("ai.enable_vision", false) {
            return Err("请先在设置中启用视觉功能".to_string());
        }
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let image_detail = state.config.read().get_typed::<String>("ai.image_detail", "auto".into());
        let image = crate::types::response::MessageImage { media_type: "image/png".into(), data: STANDARD.encode(selected.png), url: None, detail: Some(image_detail) };
        crate::commands::chat::respond_to_image(state, app, character_id, "wechat", image,
            "📷 [图片]", None, "screen_selection").await
    }.await;
    analysis.map_err(|error| if saved_path.is_some() {
        format!("截图已保存至系统截图目录，但分析失败：{error}")
    } else { error })?;
    Ok(json!({"handled":true,"saved_path":saved_path,"copied":copied}))
}

#[tauri::command]
pub fn companion_read_clipboard() -> Result<Value, String> {
    let (sequence, text) = crate::desktop_clipboard::read()?;
    Ok(json!({"sequence":sequence,"text":text}))
}
#[tauri::command]
pub async fn companion_remember_preference(state: State<'_, Arc<AppState>>, character_id: String, content: String) -> Result<String, String> {
    let content = content.trim();
    if content.is_empty() || content.chars().count() > 1000 { return Err("请填写 1–1000 字的偏好。".into()); }
    let character = state.get_character(Some(&character_id))?;
    let memory = character.brain.memory.add_memory_with_metadata(content, crate::memory::types::MemoryType::Preference,
        0.8, vec!["desktop_preference".into()], json!({"source":"user_desktop_preferences","source_text":content,"user_confirmed":true})).await.map_err(|e| e.to_string())?;
    Ok(memory.id)
}

#[tauri::command]
pub fn companion_voice_diagnostics() -> Value { crate::voice_diagnostics::report() }

#[path = "shortcut_icons.rs"]
mod shortcut_icons;

#[tauri::command]
pub async fn companion_shortcut_icon(state: State<'_, Arc<AppState>>, shortcut_id: String) -> Result<String, String> {
    let shortcut = state.config.read().get_all().companion.shortcuts.into_iter()
        .find(|item| item.id == shortcut_id).ok_or("快捷入口不存在")?;
    match shortcut.kind.as_str() {
        "website" => shortcut_icons::website(&shortcut.target).await,
        "application" => tokio::task::spawn_blocking(move || shortcut_icons::application(&shortcut.target)).await.map_err(|e| e.to_string())?,
        _ => Err("快捷入口类型无效".into()),
    }
}
