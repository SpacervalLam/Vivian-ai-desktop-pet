//! Read-only window geometry. No titles, contents or application control are needed.
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct TerrainRect { pub id: String, pub x: i32, pub y: i32, pub width: i32, pub height: i32 }

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
            // Only ordinary application frames; exclude our own windows and overlays.
            if pid == std::process::id() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() ||
                style & WS_CAPTION.0 != WS_CAPTION.0 || ex & (WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0) != 0 {
                return BOOL(1);
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).is_err() || cloaked != 0 { return BOOL(1); }
            let mut rect = RECT::default();
            if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut rect as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32).is_err() { return BOOL(1); }
            if rect.right > rect.left && rect.bottom > rect.top {
                let output = &mut *(data.0 as *mut Vec<TerrainRect>);
                output.push(TerrainRect { id: format!("window-{}", hwnd.0 as usize), x: rect.left, y: rect.top,
                    width: rect.right - rect.left, height: rect.bottom - rect.top });
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
        let area = monitor.work_area();
        TerrainRect { id: format!("floor-{index}"), x: area.position.x, y: area.position.y, width: area.size.width as i32, height: area.size.height as i32 }
    }).collect();
    Ok(serde_json::json!({ "windows": windows(), "floors": floors }))
}
