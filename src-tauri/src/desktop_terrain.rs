//! Read-only window geometry. No titles, contents or application control are needed.
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct TerrainRect { pub id: String, pub x: i32, pub y: i32, pub width: i32, pub height: i32, pub landable: bool }

pub fn windows() -> Vec<TerrainRect> {
    #[cfg(windows)]
    unsafe {
        use windows_core::BOOL;
        use windows::Win32::{Foundation::{HWND, LPARAM, RECT}, Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS}, UI::WindowsAndMessaging::*};
        unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            // Keep front-to-back ordering, including borderless occluders; exclude our windows and overlays.
            if pid == std::process::id() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() ||
                ex & (WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0) != 0 {
                return BOOL(1);
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).is_err() || cloaked != 0 { return BOOL(1); }
            let mut rect = RECT::default();
            if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut rect as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32).is_err() { return BOOL(1); }
            if rect.right > rect.left && rect.bottom > rect.top {
                let output = &mut *(data.0 as *mut Vec<TerrainRect>);
                output.push(TerrainRect { id: format!("window-{}", hwnd.0 as usize), x: rect.left, y: rect.top,
                    width: rect.right - rect.left, height: rect.bottom - rect.top, landable: style & WS_CAPTION.0 == WS_CAPTION.0 });
            }
            BOOL(1)
        }
        let mut result = Vec::new();
        let _ = EnumWindows(Some(collect), LPARAM((&mut result as *mut Vec<TerrainRect>) as isize));
        result
    }
    #[cfg(not(windows))]
    Vec::new()
}

#[tauri::command]
pub fn get_desktop_terrain(window: tauri::WebviewWindow) -> Result<serde_json::Value, String> {
    let floors: Vec<TerrainRect> = window.available_monitors().map_err(|e| e.to_string())?.iter().enumerate().map(|(index, monitor)| {
        let position = monitor.position();
        let size = monitor.size();
        TerrainRect { id: format!("floor-{index}"), x: position.x, y: position.y, width: size.width as i32, height: size.height as i32, landable: true }
    }).collect();
    Ok(serde_json::json!({ "windows": windows(), "floors": floors }))
}
