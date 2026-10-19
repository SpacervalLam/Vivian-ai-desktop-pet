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
    // 面板只消费 shortcuts（分组与搜索都在前端做，条目上限也由保存侧校验）。
    // weather / music / quiet_reason 是独立窗口时代的字段，宿主删除后无人读取，
    // 这两个快照另有 companion_runtime 的消费点，不需要经由这个命令中转。
    json!({"shortcuts": state.config.read().get_all().companion.shortcuts})
}

#[tauri::command]
pub fn companion_save_shortcuts(app: AppHandle, state: State<'_, Arc<AppState>>, mut shortcuts: Vec<DesktopShortcut>) -> Result<(), String> {
    if shortcuts.len() > 30 { return Err("最多保存 30 个快捷入口。".into()); }
    let mut ids = std::collections::HashSet::new();
    for shortcut in &mut shortcuts {
        // 分组只是归并用的标签：去掉首尾空白，空串即未分组（旧配置本来就没有这个字段）
        shortcut.group = shortcut.group.trim().to_string();
        if shortcut.group.chars().count() > 40 { return Err("分组名称最多 40 个字符。".into()); }
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
    // open_url 工具要求带协议头，而用户经常只填 `bilibili.com`：这里和图标那条路径
    // 用同一份补全规则，免得出现「图标能取到但点了打不开」。
    let (name, args) = if shortcut.kind == "website" { ("open_url", json!({"url": shortcut_icons::normalize_target(&shortcut.target)})) }
        else if shortcut.kind == "application" { ("open_application", json!({"application":shortcut.target})) }
        else { return Err("快捷入口类型无效".into()); };
    explicit_tool(&state, &character_id, name, args).await
}

#[tauri::command]
pub async fn companion_screen_analyze(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    // 归因到当前活跃角色：聊天窗口没有「当前会话角色」这个概念，后端才是活跃角色的真源。
    let character = state.active_character_id.read().clone();
    Box::pin(analyze_screen(&app, &state, &character)).await
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
    let Some(mut selected) = crate::screen_selection::select_region(app).await? else {
        return Ok(json!({"cancelled":true}));
    };
    if selected.action == crate::screen_capture::SelectionAction::Freeze {
        let overlay = selected.overlay.take();
        crate::screen_freeze::open(app, &selected, character_id).await?;
        if let Some(overlay) = overlay { overlay.dismiss(app).await?; }
        return Ok(json!({"frozen":true}));
    }
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
    let analysis = analyze_snapshot(app, state, character_id, selected.png).await;
    let turn = analysis.map_err(|error| if saved_path.is_some() {
        format!("截图已保存至系统截图目录，但分析失败：{error}")
    } else { error })?;
    relay_visual_description(app, state, character_id, turn.description);
    Ok(json!({"handled":true,"saved_path":saved_path,"copied":copied}))
}

pub(crate) async fn analyze_snapshot(app: &AppHandle, state: &Arc<AppState>, character_id: &str, png: Vec<u8>) -> Result<crate::visual_evidence::ImageTurnResult, String> {
    if crate::companion_quiet::active() { return Err("勿扰模式下已暂停陪伴分析，请先关闭勿扰".into()); }
    if state.tool_system.is_tool_disabled("screenshot_analyze", crate::tools::registry::AgentSide::Companion) {
        return Err("截图分析已在工具设置中禁用".to_string());
    }
    if !state.config.read().get_typed::<bool>("ai.enable_vision", false) {
        return Err("请先在设置中启用视觉功能".to_string());
    }
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let image_detail = state.config.read().get_typed::<String>("ai.image_detail", "auto".into());
    let image = crate::types::response::MessageImage { media_type: "image/png".into(), data: STANDARD.encode(png), url: None, detail: Some(image_detail) };
    tracing::info!(character_id, "开始分析截图快照");
    Box::pin(crate::commands::chat::respond_to_image(state, app, character_id, "wechat", image,
        "📷 [图片]", None, "screen_selection")).await
}

pub(crate) async fn analyze_frozen_snapshot(app: &AppHandle, state: &Arc<AppState>, character_id: &str, png: Vec<u8>) -> Result<(), String> {
    if SCREEN_BUSY.compare_exchange(false, true, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire).is_err() {
        return Err("截图分析正在进行中".into());
    }
    let _guard = ScreenGuard(Vec::new());
    let turn = analyze_snapshot(app, state, character_id, png).await?;
    relay_visual_description(app, state, character_id, turn.description);
    Ok(())
}

/// 把识图模型的客观分析交给桌宠转述（`companion:interaction` → 气泡 + 表情）。
///
/// **不 await**：识图加角色回复已经让用户等了几秒，转述再串在后面就成了
/// 「截完图半天没动静」。spawn 出去，生成完直接 emit；生成失败静默，不弹任何提示。
pub(crate) fn relay_visual_description(app: &AppHandle, state: &Arc<AppState>, character_id: &str, description: String) {
    let (app, state, character) = (app.clone(), state.clone(), character_id.to_string());
    tauri::async_runtime::spawn(async move {
        let Some(line) = crate::commands::pet_reaction::relay_screen_analysis(&state, &character, &description).await else { return; };
        let _ = app.emit("companion:interaction", json!({
            "character_id": character,
            "content": line,
            "motion": "happy",
            // 图片轮的回复气泡/朗读通常还在进行，前端那道 `currentBubble` 门会把这句
            // 悄悄吃掉；标 replace 让它替换掉旧气泡，保证「用户要的结果」一定说得出来。
            "replace": true,
        }));
    });
}

#[tauri::command]
pub fn companion_read_clipboard() -> Result<Value, String> {
    let (sequence, text) = crate::desktop_clipboard::read()?;
    Ok(json!({"sequence":sequence,"text":text}))
}

#[path = "shortcut_icons.rs"]
pub(crate) mod shortcut_icons;

#[tauri::command]
pub async fn companion_shortcut_icon(state: State<'_, Arc<AppState>>, shortcut_id: String) -> Result<String, String> {
    let shortcut = state.config.read().get_all().companion.shortcuts.into_iter()
        .find(|item| item.id == shortcut_id).ok_or("快捷入口不存在")?;
    // 代理必须显式解析后传进去：环境变量里的 HTTP_PROXY/HTTPS_PROXY 会指向一个
    // 代理软件关掉后的死端口，裸客户端被带偏后所有站点都取不到图标。
    let proxy_url = {
        let config = state.config.read();
        crate::network::proxy::ProxyConfig::from_app_config(&config.get_all()).effective_proxy_url()
    };
    let (name, kind, target) = (shortcut.name.clone(), shortcut.kind.clone(), shortcut.target.clone());
    let result = match kind.as_str() {
        "website" => shortcut_icons::website(&target, proxy_url.as_deref()).await,
        "application" => {
            let path = target.clone();
            tokio::task::spawn_blocking(move || shortcut_icons::application(&path)).await.map_err(|e| e.to_string())?
        }
        _ => Err("快捷入口类型无效".into()),
    };
    if let Err(error) = &result {
        tracing::warn!(target: "shortcut_icon", name = %name, kind = %kind, url = %target, error = %error, "shortcut_icon_failed");
    }
    result
}
