//! 系统托盘命令
//!
//! 托盘行为：
//! - **左键单击**：不响应（避免打乱离线状态，参见 useHiding::hideForOffline）
//! - **右键单击**：弹出原生右键菜单，内容与 Live2D 窗口内 ContextMenu 一致
//!   - 记忆管理 / 设置 / 微信 / 分隔 / 语音开关● / 智能避让● / 分隔 / 退出
//!
//! 多角色架构下，托盘事件 payload 携带活跃角色 character_id，
//! 由前端 SystemTray 组件按角色过滤后响应（活跃角色在 active_character_id 中维护）。
//!
//! 窗口内右键菜单由前端 `ContextMenu` 组件独立实现，与本组件互不依赖。

use std::sync::atomic::{AtomicBool, Ordering};

use once_cell::race::OnceBox;
use parking_lot::Mutex;
use serde_json::json;
use tauri::{
    AppHandle, Emitter, Manager,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::state::AppState;

/// 托盘 ID（用于 `app.tray_by_id` 查询）
pub const TRAY_ID: &str = "vivian-tray";

/// 托盘可见性运行时跟踪（Tauri 2 未提供 TrayIcon::visible() getter）
static TRAY_VISIBLE: AtomicBool = AtomicBool::new(true);

/// 菜单项 ID（与前端 SystemTray 路由一一对应）
mod menu_id {
    pub const MEMORY: &str = "memory";
    pub const SETTINGS: &str = "settings";
    pub const CHAT: &str = "chat";
    pub const VOICE: &str = "voice";
    pub const SMART_POSITIONING: &str = "smart_positioning";
    pub const QUIT: &str = "quit";
}

/// 缓存 CheckMenuItem 的全局句柄，供 `set_tray_menu_check` 命令更新勾选状态
///
/// 用 OnceBox 持有，避免全局 Mutex<Option<>> 的样板。初始化在 `setup_tray` 中完成。
struct CheckItems {
    voice: CheckMenuItem<tauri::Wry>,
    smart_positioning: CheckMenuItem<tauri::Wry>,
}

static CHECK_ITEMS: OnceBox<Mutex<CheckItems>> = OnceBox::new();

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 读取当前活跃角色 ID（无活跃角色时返回空字符串）
fn active_character_id(app: &AppHandle) -> String {
    match app.try_state::<std::sync::Arc<AppState>>() {
        Some(state) => state.active_character_id.read().clone(),
        None => String::new(),
    }
}

/// 在应用启动时创建托盘图标（由 `lib.rs::setup` 调用）
///
/// 仅注册右键原生菜单。菜单项点击通过 `tray:menu_action` 事件路由到前端 SystemTray 组件。
/// 左键单击不再触发任何动作（避免在两个角色都离线时打乱 hide_window 状态）。
pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or(tauri::Error::AssetNotFound("default window icon".into()))?;

    // 构建菜单项（中文文案与 ContextMenu.tsx 默认语言一致；
    // 若需切换语言，可通过 update_tray_menu_labels 命令更新，本实现暂未暴露）
    let memory = MenuItem::with_id(app, menu_id::MEMORY, "笔记本", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, menu_id::SETTINGS, "设置", true, None::<&str>)?;
    let chat = MenuItem::with_id(app, menu_id::CHAT, "微信", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let voice = CheckMenuItem::with_id(app, menu_id::VOICE, "语音开关", true, true, None::<&str>)?;
    let smart_positioning = CheckMenuItem::with_id(
        app,
        menu_id::SMART_POSITIONING,
        "智能避让",
        true,
        true,
        None::<&str>,
    )?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, menu_id::QUIT, "退出", true, None::<&str>)?;

    // 缓存 CheckMenuItem 句柄供 set_tray_menu_check 命令更新
    let _ = CHECK_ITEMS.set(Box::new(Mutex::new(CheckItems {
        voice: voice.clone(),
        smart_positioning: smart_positioning.clone(),
    })));

    let menu = Menu::with_items(
        app,
        &[
            &memory,
            &settings,
            &chat,
            &sep1,
            &voice,
            &smart_positioning,
            &sep2,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Vivian")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            handle_menu_event(app, &event);
        })
        .on_tray_icon_event(|_tray, event| {
            // 左键单击/双击不响应，避免打乱离线状态。
            // 右键菜单由 .menu() 自动弹出，无需在此处理。
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                tracing::debug!("[tray] 左键单击已忽略（避免打乱离线状态）");
            }
        })
        .build(app)?;

    tracing::info!("系统托盘已创建 (id={}, 含右键菜单)", TRAY_ID);
    Ok(())
}

/// 处理菜单项点击 → emit `tray:menu_action` 事件给前端 SystemTray 组件
///
/// 前端根据 action id 路由到 openStatus / openMemory / openChat / toggleVoice 等回调。
/// voice / smart_positioning 是 CheckMenuItem，前端需自行 toggle 状态后再 invoke
/// `set_tray_menu_check` 同步勾选标记（避免后端重复维护前端状态）。
fn handle_menu_event(app: &AppHandle, event: &MenuEvent) {
    let id = event.id().as_ref();
    let character_id = active_character_id(app);

    // 退出特殊处理：直接走 exit_app，不经过前端
    if id == menu_id::QUIT {
        tracing::info!("[tray] 菜单点击：退出");
        let _ = app.emit(
            "tray:menu_action",
            json!({ "action": menu_id::QUIT, "character_id": character_id }),
        );
        return;
    }

    tracing::debug!(
        "[tray] 菜单点击：{} (active_character={})",
        id,
        character_id
    );

    let _ = app.emit(
        "tray:menu_action",
        json!({ "action": id, "character_id": character_id }),
    );
}

/// 设置托盘 tooltip
#[tauri::command]
pub fn set_tray_tooltip(app: AppHandle, tooltip: String) -> Result<(), String> {
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| "托盘未初始化".to_string())?;
    tray.set_tooltip(Some(tooltip.clone()))
        .map_err(err_str)?;
    tracing::debug!("托盘 tooltip 已更新: {}", tooltip);
    Ok(())
}

/// 更新托盘图标（按路径加载）
#[tauri::command]
pub fn update_tray_icon(app: AppHandle, icon_path: String) -> Result<(), String> {
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| "托盘未初始化".to_string())?;

    let image = tauri::image::Image::from_path(&icon_path).map_err(err_str)?;
    tray.set_icon(Some(image)).map_err(err_str)?;
    tracing::debug!("托盘图标已更新: {}", icon_path);
    Ok(())
}

/// 显示托盘通知消息（对应 QSystemTrayIcon.showMessage）
#[tauri::command]
pub fn show_tray_message(
    app: AppHandle,
    title: String,
    body: String,
) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;

    app.notification()
        .builder()
        .title(&title)
        .body(&body)
        .show()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 获取托盘当前可见性
#[tauri::command]
pub fn is_tray_visible(app: AppHandle) -> Result<bool, String> {
    let _ = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| "托盘未初始化".to_string())?;
    Ok(TRAY_VISIBLE.load(Ordering::SeqCst))
}

/// 设置托盘可见性
#[tauri::command]
pub fn set_tray_visible(app: AppHandle, visible: bool) -> Result<(), String> {
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| "托盘未初始化".to_string())?;
    tray.set_visible(visible).map_err(err_str)?;
    TRAY_VISIBLE.store(visible, Ordering::SeqCst);
    Ok(())
}

/// 更新托盘菜单中 CheckMenuItem 的勾选状态
///
/// 由前端在 `voiceEnabled` / `smartPositioningEnabled` 变化时调用，
/// 让后端原生菜单的勾选标记与前端 store 保持一致。
///
/// `item_id` 取值：`"voice"` / `"smart_positioning"`
#[tauri::command]
pub fn set_tray_menu_check(item_id: String, checked: bool) -> Result<(), String> {
    let items = CHECK_ITEMS
        .get()
        .ok_or_else(|| "托盘菜单未初始化".to_string())?
        .lock();

    let target = match item_id.as_str() {
        menu_id::VOICE => &items.voice,
        menu_id::SMART_POSITIONING => &items.smart_positioning,
        other => {
            return Err(format!("未知菜单项 ID: {}", other));
        }
    };

    target.set_checked(checked).map_err(err_str)?;
    tracing::debug!("[tray] 菜单勾选更新: {} = {}", item_id, checked);
    Ok(())
}

/// 注销系统托盘图标（应用退出前调用，避免进程结束后残留图标）
#[tauri::command]
pub fn destroy_tray(app: AppHandle) -> Result<(), String> {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_visible(false).map_err(err_str)?;
        TRAY_VISIBLE.store(false, Ordering::SeqCst);
        tracing::info!("系统托盘已隐藏 (id={})", TRAY_ID);
    }
    Ok(())
}
