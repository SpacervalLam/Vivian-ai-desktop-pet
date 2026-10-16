//! Each snapshot belongs to one window; closing it releases its pixels.
use std::{collections::HashMap, sync::{Arc, LazyLock}};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use crate::{screen_selection::SelectedCapture, state::AppState};

struct Snapshot {
    png: Vec<u8>, width: u32, height: u32, left: i32, top: i32, character: String,
    presented: Mutex<Option<tokio::sync::oneshot::Sender<Result<(), String>>>>,
}
static SNAPSHOTS: LazyLock<Mutex<HashMap<String, Arc<Snapshot>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn snapshot(window: &WebviewWindow) -> Result<Arc<Snapshot>, String> {
    SNAPSHOTS.lock().get(window.label()).cloned().ok_or_else(|| "定格窗口已关闭".into())
}

/// Use the compositor's frame without adding a title bar to the snapshot.
fn style_snapshot(window: &WebviewWindow) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{Foundation::HWND, Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWMWA_BORDER_COLOR, DWMWA_COLOR_DEFAULT}};
        let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
        // Older Windows versions may not support these attributes; their native
        // shadow and border still work, so unsupported styling is non-fatal.
        let corner = DWMWCP_ROUND;
        let border = DWMWA_COLOR_DEFAULT;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const std::ffi::c_void, std::mem::size_of_val(&corner) as u32);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR,
            &border as *const _ as *const std::ffi::c_void, std::mem::size_of_val(&border) as u32);
    }
    Ok(())
}

/// Geometry describes screenshot pixels, not the invisible native shadow frame.
fn set_geometry(window: &WebviewWindow, left: i32, top: i32, width: u32, height: u32) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{Foundation::{HWND, POINT, RECT}, Graphics::Gdi::ClientToScreen,
            UI::WindowsAndMessaging::{GetClientRect, GetWindowRect}};
        let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
        // Move to the target monitor before measuring its DPI-dependent frame.
        window.set_position(tauri::PhysicalPosition::new(left, top)).map_err(|e| e.to_string())?;
        let mut client = RECT::default();
        let mut outer = RECT::default();
        let mut origin = POINT::default();
        GetClientRect(hwnd, &mut client).map_err(|e| e.to_string())?;
        GetWindowRect(hwnd, &mut outer).map_err(|e| e.to_string())?;
        ClientToScreen(hwnd, &mut origin).ok().map_err(|e| e.to_string())?;
        let horizontal = (outer.right - outer.left) - (client.right - client.left);
        let vertical = (outer.bottom - outer.top) - (client.bottom - client.top);
        crate::screen_selection::fill_webview_host(window,
            left - (origin.x - outer.left), top - (origin.y - outer.top),
            (width as i32 + horizontal).max(1) as u32, (height as i32 + vertical).max(1) as u32)?;
    }
    #[cfg(not(windows))]
    {
        window.set_position(tauri::PhysicalPosition::new(left, top)).map_err(|e| e.to_string())?;
        window.set_size(tauri::PhysicalSize::new(width, height)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub async fn open(app: &AppHandle, capture: &SelectedCapture, character: &str) -> Result<(), String> {
    let label = format!("screen_freeze_{}", uuid::Uuid::new_v4().simple());
    let width = capture.region.width;
    let height = capture.region.height;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    SNAPSHOTS.lock().insert(label.clone(), Arc::new(Snapshot {
        png: capture.png.clone(), width, height,
        left: capture.left + capture.region.x as i32, top: capture.top + capture.region.y as i32,
        character: character.into(), presented: Mutex::new(Some(sender)),
    }));
    let result = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html?view=screen_freeze&hidden=1".into()))
        .title("定格画面").decorations(false).resizable(false).always_on_top(true).shadow(true)
        .inner_size(320.0, 240.0).visible(false).build();
    let window = match result {
        Ok(window) => window,
        Err(error) => { SNAPSHOTS.lock().remove(&label); return Err(error.to_string()); }
    };
    let cleanup_label = label.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) { SNAPSHOTS.lock().remove(&cleanup_label); }
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), receiver).await
        .map_err(|_| "定格窗口显示超时，请重试".to_string())
        .and_then(|result| result.map_err(|_| "定格窗口已关闭".to_string()))
        .and_then(|result| result);
    if result.is_err() { let _ = window.destroy(); SNAPSHOTS.lock().remove(&label); }
    result
}

#[tauri::command]
pub async fn screen_freeze_frame(window: WebviewWindow) -> Result<tauri::ipc::Response, String> {
    let image = snapshot(&window)?;
    let geometry = image.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let app = window.app_handle().clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            snapshot(&window)?;
            crate::screen_selection::disable_transitions(&window)?;
            style_snapshot(&window)?;
            set_geometry(&window, geometry.left, geometry.top, geometry.width, geometry.height)?;
            Ok::<_, String>(())
        })();
        let _ = sender.send(result);
    }).map_err(|e| e.to_string())?;
    receiver.await.map_err(|_| "定格窗口准备中断".to_string())??;
    Ok(tauri::ipc::Response::new(image.png.clone()))
}

/// Called only after image decoding and two paints at the exact capture geometry.
#[tauri::command]
pub async fn screen_freeze_present(window: WebviewWindow, error: Option<String>) -> Result<(), String> {
    let image = snapshot(&window)?;
    if let Some(error) = error {
        if let Some(sender) = image.presented.lock().take() { let _ = sender.send(Err(error)); }
        return Ok(());
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let app = window.app_handle().clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            snapshot(&window)?;
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            #[cfg(windows)]
            unsafe { windows::Win32::Graphics::Dwm::DwmFlush().map_err(|e| e.to_string())?; }
            Ok::<_, String>(())
        })();
        if let Some(presented) = image.presented.lock().take() { let _ = presented.send(result.clone()); }
        let _ = sender.send(result);
    }).map_err(|e| e.to_string())?;
    receiver.await.map_err(|_| "定格窗口显示中断".to_string())?
}

#[tauri::command]
pub async fn screen_freeze_resize(window: WebviewWindow, scale: f64, edge: Option<String>) -> Result<f64, String> {
    let image = snapshot(&window)?;
    if !scale.is_finite() || !(0.1..=8.0).contains(&scale) { return Err("缩放比例无效".into()); }
    if edge.as_ref().is_some_and(|edge| !matches!(edge.as_str(), "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "nw")) {
        return Err("缩放边缘无效".into());
    }
    let dpi = window.scale_factor().map_err(|e| e.to_string())?;
    let (width, height, effective_scale) = crate::screen_freeze_geometry::dimensions(image.width, image.height, dpi, scale);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let app = window.app_handle().clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            snapshot(&window)?;
            let mut position = window.inner_position().map_err(|e| e.to_string())?;
            if let Some(edge) = edge {
                let size = window.inner_size().map_err(|e| e.to_string())?;
                let (x, y) = crate::screen_freeze_geometry::anchored_position(position.x, position.y, size.width, size.height, width, height, &edge);
                position = tauri::PhysicalPosition::new(x, y);
            } else if let Some(monitor) = window.current_monitor().map_err(|e| e.to_string())? {
                let area = monitor.work_area();
                position.x = position.x.clamp(area.position.x, (area.position.x + area.size.width as i32 - width as i32).max(area.position.x));
                position.y = position.y.clamp(area.position.y, (area.position.y + area.size.height as i32 - height as i32).max(area.position.y));
            }
            set_geometry(&window, position.x, position.y, width, height)?;
            window.show().map_err(|e| e.to_string())?;
            Ok::<_, String>(effective_scale)
        })();
        let _ = sender.send(result);
    }).map_err(|e| e.to_string())?;
    receiver.await.map_err(|_| "定格窗口缩放中断".to_string())?
}

#[tauri::command]
pub async fn screen_freeze_save(window: WebviewWindow) -> Result<String, String> {
    let image = snapshot(&window)?;
    crate::screen_capture::save_selected_png(&image.png).await
}

#[tauri::command]
pub async fn screen_freeze_analyze(window: WebviewWindow, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let image = snapshot(&window)?;
    let app = window.app_handle();
    crate::commands::desktop_assistant::analyze_frozen_snapshot(app, &state, &image.character, image.png.clone()).await
}
