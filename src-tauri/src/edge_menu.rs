//! Lightweight independent edge menu; chat WebViews only exist while in use.
use std::{sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc}, thread, time::{Duration, Instant}};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use crate::state::AppState;

const MENU_WIDTH: f64 = 60.0; // physical pixels, matching the previous edge entrance
static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(true);
static MENU_OPEN: AtomicBool = AtomicBool::new(false);
static DISMISS_CHAT: AtomicBool = AtomicBool::new(false);
static CREATE_CHAT: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));
static CREATE_INPUT: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));
static INPUT_READY: AtomicBool = AtomicBool::new(false);
static INPUT_CONFIG: Lazy<Mutex<Value>> = Lazy::new(|| Mutex::new(json!({})));
static CHAT_NAV: Lazy<Mutex<Value>> = Lazy::new(|| Mutex::new(json!({"view":"home"})));
static CHAT_FROM_MENU: AtomicBool = AtomicBool::new(false);
static CHAT_READY: AtomicBool = AtomicBool::new(false);
static CHAT_GENERATION: AtomicU64 = AtomicU64::new(0);
static MENU_MOTION_GENERATION: AtomicU64 = AtomicU64::new(0);
static CHAT_MOTION_GENERATION: AtomicU64 = AtomicU64::new(0);
static CHAT_ENTERING: AtomicBool = AtomicBool::new(false);

fn geometry(app: &AppHandle) -> Result<(f64, f64, f64, f64, f64), String> {
    let active = app.state::<Arc<AppState>>().active_character_id.read().clone();
    let monitor = app.get_webview_window(&active).and_then(|window| window.current_monitor().ok().flatten())
        .or(app.primary_monitor().map_err(|e| e.to_string())?).ok_or("显示器不可用")?;
    let scale = monitor.scale_factor().max(1.0);
    let screen_height = monitor.size().height as f64 / scale;
    let height = (390.0_f64 * 852.0 / 393.0).round().min((screen_height - 24.0).max(300.0));
    let width = (height * 393.0 / 852.0).round();
    let right = (monitor.position().x + monitor.size().width as i32) as f64 / scale;
    let top = monitor.position().y as f64 / scale + (screen_height - height) / 2.0;
    Ok((right, top, width, height, scale))
}

fn create_menu(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window("edge_menu").is_some() { return Ok(()); }
    let (right, top, _, height, scale) = geometry(app)?;
    let menu = WebviewWindowBuilder::new(app, "edge_menu", WebviewUrl::App("index.html?view=edge_menu&hidden=1".into()))
        .title("桌面快捷菜单").inner_size(MENU_WIDTH / scale, height).position(right, top)
        .decorations(false).transparent(true).resizable(false).always_on_top(true)
        .skip_taskbar(true).shadow(false).visible(false).focused(false).build().map_err(|e| e.to_string())?;
    if let Err(error) = crate::screen_selection::disable_transitions(&menu) { let _ = menu.destroy(); return Err(error); }
    Ok(())
}

/// Keep the compositor and WebView alive, entirely beyond the virtual desktop.
fn park_menu(menu: &WebviewWindow) -> Result<(), String> {
    let right = menu.available_monitors().map_err(|e| e.to_string())?.iter()
        .map(|monitor| monitor.position().x + monitor.size().width as i32)
        .max().ok_or("显示器不可用")?;
    let top = menu.outer_position().map_err(|e| e.to_string())?.y;
    let window = menu.clone();
    menu.app_handle().run_on_main_thread(move || {
        let _ = window.set_position(tauri::PhysicalPosition::new(right + 16, top));
        // Capture's UI-thread hide barrier must remain authoritative.
        if !crate::commands::desktop_assistant::screen_capture_in_progress() && !window.is_visible().unwrap_or(false) {
            let _ = window.show();
        }
    }).map_err(|e| e.to_string())
}

fn move_slide_frame(window: &WebviewWindow, x: i32, top: i32, generation: u64, motion_generation: &'static AtomicU64) -> bool {
    let moving = window.clone();
    window.app_handle().run_on_main_thread(move || {
        if motion_generation.load(Ordering::Acquire) == generation && !STOP.load(Ordering::Acquire)
            && !crate::commands::desktop_assistant::screen_capture_in_progress() {
            let _ = moving.set_position(tauri::PhysicalPosition::new(x, top));
        }
    }).is_ok()
}

/// Move the native window, so even its first visible frame starts outside the screen.
fn slide_window_in(menu: &WebviewWindow, right: i32, left: i32, top: i32, motion_generation: &'static AtomicU64) {
    let generation = motion_generation.fetch_add(1, Ordering::AcqRel) + 1;
    let window = menu.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let queued = menu.app_handle().run_on_main_thread(move || {
        // A screenshot may have started while the suspended renderer was resuming.
        // Check on the UI thread so it cannot race with capture's hide barrier.
        let shown = !STOP.load(Ordering::Acquire)
            && motion_generation.load(Ordering::Acquire) == generation
            && !crate::commands::desktop_assistant::screen_capture_in_progress()
            && window.set_position(tauri::PhysicalPosition::new(right, top)).is_ok()
            && (window.is_visible().unwrap_or(false) || window.show().is_ok());
        let _ = sender.send(shown);
    });
    if queued.is_err() || receiver.recv_timeout(Duration::from_secs(1)) != Ok(true) {
        let _ = motion_generation.compare_exchange(generation, generation + 1, Ordering::AcqRel, Ordering::Acquire);
        return;
    }
    let started = Instant::now();
    loop {
        if STOP.load(Ordering::Acquire) || motion_generation.load(Ordering::Acquire) != generation
            || crate::commands::desktop_assistant::screen_capture_in_progress() { break; }
        let elapsed = started.elapsed();
        let x = crate::desktop_menu_policy::menu_slide_x(right, left, elapsed);
        if !move_slide_frame(menu, x, top, generation, motion_generation) { break; }
        if elapsed >= crate::desktop_menu_policy::MENU_SLIDE_DURATION { break; }
        thread::sleep(Duration::from_millis(8));
    }
    // A dismissal may already have parked the resident menu; never move it back.
    if motion_generation.load(Ordering::Acquire) == generation
        && !STOP.load(Ordering::Acquire) && !crate::commands::desktop_assistant::screen_capture_in_progress() {
        move_slide_frame(menu, left, top, generation, motion_generation);
    }
}

/// 鼠标离开时的收起：**先把窗口滑出右边缘，再停到屏外**。
///
/// 入场是滑入，退场若直接 `park_menu` 瞬移到屏外，视觉上就是"闪一下消失"。
/// 这里用同一条缓动反向播一遍，让退场与入场对称。
///
/// 从当前实际位置起滑：若呼出动画还没播完，就从半途接着退回去，不会先弹到位再消失。
/// 滑出途中若被更新的动作（重新呼出 / 折叠收起）接管，就放弃且**不**停靠 ——
/// 那一步的拥有者负责窗口位置，这里再停一次会把刚滑入的菜单又拽走。
fn dismiss_edge_menu(app: &AppHandle, right: i32, top: i32) {
    MENU_OPEN.store(false, Ordering::Release);
    ARMED.store(false, Ordering::Release);
    let Some(menu) = app.get_webview_window("edge_menu") else { return; };
    let generation = MENU_MOTION_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let Ok(position) = menu.outer_position() else { let _ = park_menu(&menu); return; };
    if position.x < right {
        let started = Instant::now();
        loop {
            if MENU_MOTION_GENERATION.load(Ordering::Acquire) != generation { return; }
            if STOP.load(Ordering::Acquire) { break; }
            let elapsed = started.elapsed();
            let x = crate::desktop_menu_policy::menu_slide_x(position.x, right, elapsed);
            if !move_slide_frame(&menu, x, top, generation, &MENU_MOTION_GENERATION) { break; }
            if elapsed >= crate::desktop_menu_policy::MENU_SLIDE_DURATION { break; }
            thread::sleep(Duration::from_millis(8));
        }
    }
    if MENU_MOTION_GENERATION.load(Ordering::Acquire) == generation { let _ = park_menu(&menu); }
}

pub fn start(app: AppHandle) {
    if RUNNING.swap(true, Ordering::AcqRel) { return; }
    STOP.store(false, Ordering::Release);
    thread::spawn(move || {
        if let Err(error) = create_menu(&app) { tracing::warn!(%error, "边缘菜单创建失败"); RUNNING.store(false, Ordering::Release); return; }
        crate::screen_selection::prewarm(app.clone());
        let mut left_at: Option<Instant> = None;
        let mut last_geometry = Instant::now();
        let mut bounds = geometry(&app).ok();
        while !STOP.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(40));
            let Some(menu) = app.get_webview_window("edge_menu") else { break; };
            if !READY.load(Ordering::Acquire) { continue; }
            if crate::commands::desktop_assistant::screen_capture_in_progress() {
                MENU_OPEN.store(false, Ordering::Release);
                ARMED.store(false, Ordering::Release);
                left_at = None;
                continue;
            }
            if !menu.is_visible().unwrap_or(false) {
                MENU_OPEN.store(false, Ordering::Release);
                ARMED.store(false, Ordering::Release);
                let _ = park_menu(&menu);
            }
            if last_geometry.elapsed() > Duration::from_secs(2) && !MENU_OPEN.load(Ordering::Acquire) {
                bounds = geometry(&app).ok();
                if let Some((_, _, _, height, scale)) = bounds {
                    let _ = menu.set_size(tauri::LogicalSize::new(MENU_WIDTH / scale, height));
                    let _ = park_menu(&menu);
                }
                last_geometry = Instant::now();
            }
            let Some((right, top, _, height, scale)) = bounds else { continue; };
            let (right, top, height) = ((right * scale).round() as i32, (top * scale).round() as i32, (height * scale).round() as i32);
            let Ok(cursor) = app.cursor_position() else { continue; };
            let in_edge = cursor.x >= (right - 12) as f64 && cursor.x < right as f64
                && cursor.y >= (top - 12) as f64 && cursor.y <= (top + height + 12) as f64;
            if !in_edge { ARMED.store(true, Ordering::Release); }
            if !MENU_OPEN.load(Ordering::Acquire) {
                if in_edge && ARMED.swap(false, Ordering::AcqRel) {
                    let theme = app.state::<Arc<AppState>>().config.read().get_typed::<String>("base.theme", "system".into());
                    let _ = menu.emit("edge_menu:shown", json!({"theme":theme}));
                    MENU_OPEN.store(true, Ordering::Release);
                    slide_window_in(&menu, right, right - MENU_WIDTH as i32, top, &MENU_MOTION_GENERATION);
                    left_at = None;
                }
            } else {
                let (Ok(pos), Ok(size)) = (menu.outer_position(), menu.outer_size()) else { continue; };
                let inside = cursor.x >= (pos.x - 4) as f64 && cursor.x <= right as f64
                    && cursor.y >= (pos.y - 4) as f64 && cursor.y <= (pos.y + size.height as i32 + 4) as f64;
                if inside || in_edge { left_at = None; }
                else if left_at.get_or_insert_with(Instant::now).elapsed() >= Duration::from_millis(420) {
                    dismiss_edge_menu(&app, right, top);
                    left_at = None;
                }
            }
        }
        RUNNING.store(false, Ordering::Release);
    });
}

pub fn stop() { STOP.store(true, Ordering::Release); }

#[tauri::command]
pub fn edge_menu_ready(window: WebviewWindow) -> Result<(), String> {
    if window.label() != "edge_menu" { return Err("无效快捷菜单窗口".into()); }
    park_menu(&window)?;
    READY.store(true, Ordering::Release);
    Ok(())
}

/// Invalidate queued entrance/exit frames before hiding screenshot entry windows.
pub(crate) fn suspend_for_capture() {
    MENU_MOTION_GENERATION.fetch_add(1, Ordering::AcqRel);
    CHAT_MOTION_GENERATION.fetch_add(1, Ordering::AcqRel);
    MENU_OPEN.store(false, Ordering::Release);
    ARMED.store(false, Ordering::Release);
    CHAT_ENTERING.store(false, Ordering::Release);
}

#[tauri::command]
pub fn hide_edge_menu(app: AppHandle) {
    MENU_MOTION_GENERATION.fetch_add(1, Ordering::AcqRel);
    MENU_OPEN.store(false, Ordering::Release);
    ARMED.store(false, Ordering::Release);
    if let Some(menu) = app.get_webview_window("edge_menu") {
        let _ = park_menu(&menu);
    }
}

#[tauri::command]
pub fn edge_menu_status(app: AppHandle) -> Value {
    let state = app.state::<Arc<AppState>>();
    json!({"quiet":crate::companion_quiet::active(),
        "character_id":state.active_character_id.read().clone()})
}

#[tauri::command]
pub fn set_chat_close_policy(origin: String) {
    DISMISS_CHAT.store(crate::desktop_menu_policy::dismiss_on_outside_click(&origin), Ordering::Release);
}

pub async fn open_chat(app: &AppHandle, origin: &str, panel: Option<&str>, character_id: Option<&str>) -> Result<(), String> {
    let _creation = CREATE_CHAT.lock().await;
    set_chat_close_policy(origin.into());
    let state = app.state::<Arc<AppState>>();
    let character = character_id.map(str::to_string).unwrap_or_else(|| state.active_character_id.read().clone());
    let navigation = panel.map_or_else(|| json!({"view":"home"}), |panel| json!({"view":"assistant","panel":panel,"character_id":character}));
    *CHAT_NAV.lock() = navigation.clone();
    CHAT_FROM_MENU.store(origin == "menu_chat" || origin == "menu_tool", Ordering::Release);
    if let Some(window) = app.get_webview_window("chat") {
        if !CHAT_READY.load(Ordering::Acquire) { return Ok(()); }
        show_chat(&window)?;
        window.emit("chatwindow:navigate", navigation).map_err(|e| e.to_string())?;
        fold_menu_when_chat_ready(app);
        return Ok(());
    }
    let (right, top, width, height, _) = geometry(app)?;
    CHAT_READY.store(false, Ordering::Release);
    let generation = CHAT_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let suffix = panel.map(|panel| format!("&assistant=1&assistant_character={}&assistant_panel={}", urlencoding::encode(&character), urlencoding::encode(panel))).unwrap_or_default();
    let window = WebviewWindowBuilder::new(app, "chat", WebviewUrl::App(format!("index.html?view=chat&hidden=1{suffix}").into()))
        .title("聊天").inner_size(width, height).position(right, top)
        .decorations(false).transparent(true).resizable(false).always_on_top(true)
        .skip_taskbar(true).shadow(false).visible(false).focused(false).build().map_err(|e| e.to_string())?;
    if let Err(error) = crate::screen_selection::disable_transitions(&window) { let _ = window.destroy(); return Err(error); }
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) && CHAT_GENERATION.load(Ordering::Acquire) == generation {
            CHAT_READY.store(false, Ordering::Release);
            CHAT_ENTERING.store(false, Ordering::Release);
            CHAT_MOTION_GENERATION.fetch_add(1, Ordering::AcqRel);
        }
    });
    Ok(())
}

fn fold_menu_when_chat_ready(app: &AppHandle) {
    if CHAT_FROM_MENU.swap(false, Ordering::AcqRel) {
        if let Some(menu) = app.get_webview_window("edge_menu") { let _ = menu.emit("edge_menu:fold", json!({})); }
    }
}

pub fn show_chat(window: &WebviewWindow) -> Result<(), String> {
    crate::commands::window::thaw_webview(window);
    window.set_ignore_cursor_events(false).map_err(|e| e.to_string())?;
    // Focus and navigation of an already visible chat never replay its entrance.
    if window.is_visible().map_err(|e| e.to_string())? || CHAT_ENTERING.load(Ordering::Acquire) { return Ok(()); }
    let (right, _, _, _, scale) = geometry(window.app_handle())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let right = (right * scale).round() as i32;
    if CHAT_ENTERING.swap(true, Ordering::AcqRel) { return Ok(()); }
    let window = window.clone();
    let generation = CHAT_GENERATION.load(Ordering::Acquire);
    thread::spawn(move || {
        slide_window_in(&window, right, right - size.width as i32, position.y, &CHAT_MOTION_GENERATION);
        if CHAT_GENERATION.load(Ordering::Acquire) == generation { CHAT_ENTERING.store(false, Ordering::Release); }
    });
    Ok(())
}

#[tauri::command]
pub async fn open_chat_window(app: AppHandle, origin: Option<String>, panel: Option<String>, character_id: Option<String>) -> Result<(), String> {
    open_chat(&app, origin.as_deref().unwrap_or("chat"), panel.as_deref(), character_id.as_deref()).await
}

#[tauri::command]
pub async fn open_quick_input(app: AppHandle, character_id: Option<String>, broadcast: Option<bool>, auto_voice: Option<bool>) -> Result<(), String> {
    let _creation = CREATE_INPUT.lock().await;
    let cursor = app.cursor_position().map_err(|e| e.to_string())?;
    let monitor = app.available_monitors().map_err(|e| e.to_string())?.into_iter().find(|m| {
        let p = m.position(); let s = m.size();
        cursor.x >= p.x as f64 && cursor.x < p.x as f64 + s.width as f64 && cursor.y >= p.y as f64 && cursor.y < p.y as f64 + s.height as f64
    }).or(app.primary_monitor().map_err(|e| e.to_string())?).ok_or("显示器不可用")?;
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let (width, height, x, y) = quick_input_geometry((cursor.x, cursor.y),
        (area.position.x, area.position.y, area.size.width, area.size.height), scale);
    let character = character_id.unwrap_or_else(|| app.state::<Arc<AppState>>().active_character_id.read().clone());
    *INPUT_CONFIG.lock() = json!({"character_id":character,"broadcast":broadcast.unwrap_or(false),"auto_voice":auto_voice.unwrap_or(false)});
    let window = if let Some(window) = app.get_webview_window("input") { window } else {
        INPUT_READY.store(false, Ordering::Release);
        WebviewWindowBuilder::new(&app, "input", WebviewUrl::App("index.html?view=input&hidden=1".into()))
            .title("快捷输入").inner_size(width, height).decorations(false).transparent(true)
            .always_on_top(true).skip_taskbar(true).shadow(false).resizable(false).visible(false).focused(false)
            .build().map_err(|e| e.to_string())?
    };
    window.set_position(tauri::PhysicalPosition::new(x, y)).map_err(|e| e.to_string())?;
    window.set_size(tauri::PhysicalSize::new((width * scale).round() as u32, (height * scale).round() as u32)).map_err(|e| e.to_string())?;
    if INPUT_READY.load(Ordering::Acquire) {
        window.emit("quick_input:configure", INPUT_CONFIG.lock().clone()).map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn quick_input_geometry(cursor: (f64, f64), area: (i32, i32, u32, u32), scale: f64) -> (f64, f64, i32, i32) {
    let width = 560.0_f64.min((area.2 as f64 / scale - 16.0).max(1.0));
    let height = 220.0_f64.min((area.3 as f64 / scale - 16.0).max(1.0));
    let left = area.0 as f64 + 8.0 * scale;
    let top = area.1 as f64 + 8.0 * scale;
    let right = (area.0 as f64 + area.2 as f64 - (width + 8.0) * scale).max(left);
    let bottom = (area.1 as f64 + area.3 as f64 - (height + 8.0) * scale).max(top);
    let x = (cursor.0 - width * scale / 2.0).clamp(left, right);
    let y = (cursor.1 + 16.0 * scale - height * scale / 2.0).clamp(top, bottom);
    (width, height, x.round() as i32, y.round() as i32)
}

#[tauri::command]
pub fn quick_input_ready(window: WebviewWindow) -> Result<(), String> {
    if window.label() != "input" { return Err("无效输入窗口".into()); }
    INPUT_READY.store(true, Ordering::Release);
    window.emit("quick_input:configure", INPUT_CONFIG.lock().clone()).map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_window_ready(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    if window.label() != "chat" { return Err("无效聊天窗口".into()); }
    CHAT_READY.store(true, Ordering::Release);
    window.emit("chatwindow:navigate", CHAT_NAV.lock().clone()).map_err(|e| e.to_string())?;
    show_chat(&window)?;
    fold_menu_when_chat_ready(&app);
    Ok(())
}

pub fn outside_pointer_down(app: &AppHandle, x: i32, y: i32) {
    if !DISMISS_CHAT.load(Ordering::Acquire) || crate::commands::desktop_assistant::screen_capture_in_progress() { return; }
    let Some(chat) = app.get_webview_window("chat") else { return; };
    if !chat.is_visible().unwrap_or(false) { return; }
    let contains = |window: &WebviewWindow| {
        match (window.outer_position(), window.outer_size()) {
            (Ok(pos), Ok(size)) => x >= pos.x && x < pos.x + size.width as i32 && y >= pos.y && y < pos.y + size.height as i32,
            _ => true, // Failed geometry must never destroy an interactive window.
        }
    };
    if contains(&chat) { return; }
    if app.get_webview_window("edge_menu").is_some_and(|menu| menu.is_visible().unwrap_or(false) && contains(&menu)) { return; }
    let _ = chat.destroy();
}

fn open_office(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("memory") {
        crate::commands::window::thaw_webview(&window);
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        return window.emit("memory:navigate", json!({"page":"code"})).map_err(|e| e.to_string());
    }
    WebviewWindowBuilder::new(app, "memory", WebviewUrl::App("index.html?view=memory&nav=code".into()))
        .title("心智观察器").inner_size(1260.0, 896.0).min_inner_size(900.0, 600.0).center()
        .decorations(false).visible(false).build().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn edge_menu_action(app: AppHandle, action: String, enabled: Option<bool>) -> Result<Value, String> {
    edge_menu_action_task(app, action, enabled).await
}

// Tauri constructs command futures in the WebView callback on the UI thread.
// Keep that future small: screenshot/vision embeds the entire chat pipeline,
// even when another match arm is selected. Build it only when the runtime polls
// this wrapper, behind a non-inlined heap allocation boundary.
#[inline(never)]
fn edge_menu_action_task(app: AppHandle, action: String, enabled: Option<bool>)
    -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>> {
    Box::pin(async move {
    match action.as_str() {
        "chat" => open_chat(&app, "menu_chat", None, None).await?,
        "office" => { open_office(&app)?; hide_edge_menu(app.clone()); },
        "dnd" => return crate::companion_quiet::set_companion_quiet(app.clone(), app.state(), enabled.unwrap_or(!crate::companion_quiet::active())).await,
        "screen" => {
            let state = app.state::<Arc<AppState>>();
            let character = state.active_character_id.read().clone();
            return crate::commands::desktop_assistant::analyze_screen(&app, &state, &character).await;
        }
        action => {
            let panel = crate::desktop_menu_policy::menu_panel(action).ok_or("未知快捷入口")?;
            open_chat(&app, "menu_tool", Some(panel), None).await?;
        }
    }
    Ok(json!({}))
    })
}

#[cfg(test)]
mod action_stack_tests {
    #[test]
    fn command_future_fits_in_webview_callback_stack() {
        fn size<A, B, C, F: std::future::Future>(_: fn(A, B, C) -> F) -> usize {
            std::mem::size_of::<F>()
        }
        let bytes = size(super::edge_menu_action);
        assert!(bytes < 4096, "edge menu command future uses {bytes} bytes before runtime dispatch");
    }
}
