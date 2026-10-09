//! Reusable selector: screenshot pixels are retained only during an active selection.
use std::{sync::{LazyLock, atomic::{AtomicBool, Ordering}}, time::Instant};
use parking_lot::Mutex;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow, WebviewWindowBuilder, WebviewUrl, WindowEvent};
use tokio::sync::{oneshot, Notify};
use crate::screen_capture::{CaptureRegion, SelectionAction};
pub struct SelectedCapture { pub png: Vec<u8>, pub action: SelectionAction }
struct SelectedArea { region: CaptureRegion, action: SelectionAction }
const WINDOW_LABEL: &str = "screen_selection";
struct SelectionSession {
    id: String, preview: Vec<u8>, width: u32, height: u32,
    sender: oneshot::Sender<Option<SelectedArea>>, started: Instant,
}
static SESSION: LazyLock<Mutex<Option<SelectionSession>>> = LazyLock::new(|| Mutex::new(None));
static CREATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static READY: AtomicBool = AtomicBool::new(false);
static READY_SIGNAL: Notify = Notify::const_new();

/// Scoped to our windows: never change Windows' global animation setting.
pub(crate) fn disable_transitions(window: &WebviewWindow) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{Foundation::HWND, Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED}};
        let hwnd = window.hwnd().map_err(|e| e.to_string())?;
        let disabled = 1_i32;
        DwmSetWindowAttribute(HWND(hwnd.0), DWMWA_TRANSITIONS_FORCEDISABLED,
            (&disabled as *const i32).cast(), std::mem::size_of::<i32>() as u32).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Complete hiding on the owning UI thread, then wait for composition before reading pixels.
pub async fn hide_capture_windows(app: &AppHandle, windows: Vec<WebviewWindow>) -> Result<(), String> {
    if windows.is_empty() { return Ok(()); }
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = (|| {
            for window in windows {
                disable_transitions(&window)?;
                // Tauri executes this synchronously on the owning thread, updating Tao's
                // visibility state as well as the HWND (direct ShowWindow would desync it).
                window.hide().map_err(|e| e.to_string())?;
            }
            #[cfg(windows)]
            unsafe { windows::Win32::Graphics::Dwm::DwmFlush().map_err(|e| e.to_string())?; }
            Ok::<_, String>(())
        })();
        let _ = sender.send(result);
    }).map_err(|e| e.to_string())?;
    receiver.await.map_err(|_| "截图窗口隐藏中断".to_string())?
}

/// Discover and hide both entry windows in one UI dispatch, before capture startup.
/// Return only the visible chat window for restoration after selection/cancellation.
pub async fn hide_capture_entry(app: &AppHandle) -> Result<Vec<WebviewWindow>, String> {
    let (sender, receiver) = oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        crate::edge_menu::suspend_for_capture();
        let mut restore = Vec::new();
        let result = (|| {
            for label in ["chat", "edge_menu"] {
                if let Some(window) = handle.get_webview_window(label) {
                    if label == "chat" && window.is_visible().map_err(|e| e.to_string())? {
                        restore.push(window.clone());
                    }
                    disable_transitions(&window)?;
                    window.hide().map_err(|e| e.to_string())?;
                }
            }
            #[cfg(windows)]
            unsafe { windows::Win32::Graphics::Dwm::DwmFlush().map_err(|e| e.to_string())?; }
            Ok::<_, String>(())
        })();
        if result.is_err() {
            for window in &restore { let _ = window.show(); }
        }
        let _ = sender.send(result.map(|_| restore));
    }).map_err(|e| e.to_string())?;
    receiver.await.map_err(|_| "截图入口隐藏中断".to_string())?
}

fn finish(id: &str, region: Option<CaptureRegion>, action: SelectionAction) -> Result<(), String> {
    let mut session = SESSION.lock();
    let current = session.as_ref().filter(|s| s.id == id).ok_or("截图选区已结束")?;
    let selected = if action == SelectionAction::Cancel { None } else {
        Some(SelectedArea { region: region.ok_or("请先框选区域")?.validate(current.width, current.height)?, action })
    };
    let current = session.take().expect("validated session");
    drop(session);
    let _ = current.sender.send(selected);
    Ok(())
}
struct SelectionGuard { id: String, window: WebviewWindow }
impl Drop for SelectionGuard {
    fn drop(&mut self) {
        let _ = finish(&self.id, None, SelectionAction::Cancel);
        let _ = self.window.hide();
        let _ = self.window.emit("screen_selection:reset", json!({"session_id":self.id}));
    }
}

async fn ensure_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    let _create = CREATE.lock().await;
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) { return Ok(window); }
    READY.store(false, Ordering::Release);
    let window = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App("index.html?view=screen_selection&hidden=1".into()))
        .title("截图分析").decorations(false).resizable(false).always_on_top(true)
        .skip_taskbar(true).shadow(false).visible(false).focused(false).build().map_err(|e| e.to_string())?;
    if let Err(error) = disable_transitions(&window) { let _ = window.destroy(); return Err(error); }
    window.on_window_event(|event| {
        if matches!(event, WindowEvent::Destroyed) {
            READY.store(false, Ordering::Release);
            if let Some(session) = SESSION.lock().take() { let _ = session.sender.send(None); }
        }
    });
    Ok(window)
}

pub fn prewarm(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = ensure_window(&app).await { tracing::warn!(%error, "截图选区预热失败"); }
    });
}

#[tauri::command]
pub fn screen_selection_ready(window: WebviewWindow) -> Result<(), String> {
    if window.label() != WINDOW_LABEL { return Err("无效截图窗口".into()); }
    READY.store(true, Ordering::Release);
    READY_SIGNAL.notify_one();
    Ok(())
}
#[tauri::command]
pub fn screen_selection_frame(window: WebviewWindow, session_id: String) -> Result<tauri::ipc::Response, String> {
    if window.label() != WINDOW_LABEL { return Err("无效截图窗口".into()); }
    let session = SESSION.lock();
    let current = session.as_ref().filter(|s| s.id == session_id).ok_or("截图选区已结束")?;
    Ok(tauri::ipc::Response::new(current.preview.clone()))
}
#[tauri::command]
pub async fn screen_selection_present(window: WebviewWindow, session_id: String) -> Result<(), String> {
    if window.label() != WINDOW_LABEL { return Err("无效截图窗口".into()); }
    let (sender, receiver) = oneshot::channel();
    let app = window.app_handle().clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            let session = SESSION.lock();
            let current = session.as_ref().filter(|s| s.id == session_id).ok_or("截图选区已结束")?;
            // Geometry and decoded preview are already ready; no native opening transition.
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            tracing::info!(session = %session_id, ready_ms = current.started.elapsed().as_millis(), "截图遮罩已准备显示");
            Ok::<_, String>(())
        })();
        let _ = sender.send(result);
    }).map_err(|e| e.to_string())?;
    receiver.await.map_err(|_| "截图遮罩显示中断".to_string())?
}
#[tauri::command]
pub fn screen_selection_finish(window: WebviewWindow, session_id: String, region: Option<CaptureRegion>, action: SelectionAction) -> Result<(), String> {
    if window.label() != WINDOW_LABEL { return Err("无效截图窗口".into()); }
    finish(&session_id, region, action)
}

pub async fn select_region(app: &AppHandle) -> Result<Option<SelectedCapture>, String> {
    let started = Instant::now();
    // Cold fallback overlaps WebView startup with capture + PNG encoding.
    let prepared = async {
        let capture_started = Instant::now();
        let frame = crate::screen_capture::capture_desktop().await?;
        tracing::info!(capture_ms = capture_started.elapsed().as_millis(), "屏幕像素读取完成");
        tokio::task::spawn_blocking(move || {
            let encode_started = Instant::now();
            let preview = frame.png()?;
            tracing::info!(encode_ms = encode_started.elapsed().as_millis(), preview_bytes = preview.len(), "截图预览编码完成");
            Ok::<_, String>((frame, preview))
        }).await.map_err(|e| e.to_string())?
    };
    let (window, (frame, preview)) = tokio::try_join!(ensure_window(app), prepared)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let (sender, receiver) = oneshot::channel();
    {
        let mut session = SESSION.lock();
        if session.is_some() { return Err("截图框选正在进行中".into()); }
        *session = Some(SelectionSession { id: id.clone(), preview, width: frame.width, height: frame.height, sender, started });
    }
    let guard = SelectionGuard { id: id.clone(), window: window.clone() };
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !READY.load(Ordering::Acquire) { READY_SIGNAL.notified().await; }
    }).await.map_err(|_| "截图遮罩加载超时，请重试".to_string())?;
    window.set_position(tauri::PhysicalPosition::new(frame.left, frame.top)).map_err(|e| e.to_string())?;
    window.set_size(tauri::PhysicalSize::new(frame.width, frame.height)).map_err(|e| e.to_string())?;
    window.emit("screen_selection:start", json!({"session_id":id,"width":frame.width,"height":frame.height})).map_err(|e| e.to_string())?;
    let region = tokio::time::timeout(std::time::Duration::from_secs(180), receiver).await
        .map_err(|_| "截图框选已超时，请重试".to_string())?
        .map_err(|_| "截图窗口已关闭".to_string())?;
    hide_capture_windows(app, vec![window]).await?;
    drop(guard);
    tokio::task::spawn_blocking(move || region.map(|selected| frame.crop_png(selected.region).map(|png| SelectedCapture { png, action: selected.action })).transpose())
        .await.map_err(|e| e.to_string())?
}
