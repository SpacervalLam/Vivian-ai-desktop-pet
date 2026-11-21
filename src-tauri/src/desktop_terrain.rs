//! Read-only window geometry. No titles, contents or application control are needed.
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Serialize)]
pub struct TerrainRect {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub landable: bool,
    /// 桌面宠物窗口的角色 label；外部应用窗口为 `None`。
    /// 前端据此把其它桌宠当作实体（可碰撞），而不是地形平台。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pet: Option<String>,
}

/// 枚举桌面地形。`pets` 是「HWND → 角色 label」映射，用于把自己的桌宠窗口
/// 作为可碰撞实体返回（此前按进程 pid 整体排除，导致桌宠之间互相看不见）。
/// 桌宠窗口统一追加在末尾：它们 `landable=false`，不会成为平台，
/// 也不会参与前方窗口对平台边缘的遮挡裁剪。
pub fn windows(pets: &HashMap<isize, String>) -> Vec<TerrainRect> {
    #[cfg(windows)]
    unsafe {
        use windows_core::BOOL;
        use windows::Win32::{Foundation::{HWND, LPARAM, RECT}, Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS}, UI::WindowsAndMessaging::*};

        struct Collect<'a> {
            windows: Vec<TerrainRect>,
            pets: &'a HashMap<isize, String>,
            pet_rects: Vec<TerrainRect>,
        }

        unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let own = pid == std::process::id();
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            // 外部窗口保持前景到背景顺序，包括无边框遮挡窗口；排除本进程的浮层。
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() { return BOOL(1); }
            if !own && ex & (WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0) != 0 { return BOOL(1); }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).is_err() || cloaked != 0 { return BOOL(1); }
            let mut rect = RECT::default();
            if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut rect as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32).is_err() { return BOOL(1); }
            if rect.right <= rect.left || rect.bottom <= rect.top { return BOOL(1); }
            let output = &mut *(data.0 as *mut Collect);
            if own {
                // 只有角色窗口参与桌宠互碰；输入法/浮层等本进程窗口仍然排除。
                if let Some(label) = output.pets.get(&(hwnd.0 as isize)) {
                    output.pet_rects.push(TerrainRect { id: format!("pet-{label}"), x: rect.left,
                        y: rect.top, width: rect.right - rect.left, height: rect.bottom - rect.top,
                        landable: false, pet: Some(label.clone()) });
                }
                return BOOL(1);
            }
            output.windows.push(TerrainRect { id: format!("window-{}", hwnd.0 as usize), x: rect.left, y: rect.top,
                width: rect.right - rect.left, height: rect.bottom - rect.top,
                landable: style & WS_CAPTION.0 == WS_CAPTION.0, pet: None });
            BOOL(1)
        }
        let mut sink = Collect { windows: Vec::new(), pets, pet_rects: Vec::new() };
        let _ = EnumWindows(Some(collect), LPARAM((&mut sink as *mut Collect) as isize));
        let mut result = sink.windows;
        result.extend(sink.pet_rects);
        result
    }
    #[cfg(not(windows))]
    {
        let _ = pets;
        Vec::new()
    }
}

#[tauri::command]
pub fn get_desktop_terrain(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, std::sync::Arc<crate::state::AppState>>,
) -> Result<serde_json::Value, String> {
    let floors: Vec<TerrainRect> = window.available_monitors().map_err(|e| e.to_string())?.iter().enumerate().map(|(index, monitor)| {
        let position = monitor.position();
        let size = monitor.size();
        TerrainRect { id: format!("floor-{index}"), x: position.x, y: position.y, width: size.width as i32, height: size.height as i32, landable: true, pet: None }
    }).collect();
    #[cfg(windows)]
    let pets: HashMap<isize, String> = {
        use tauri::Manager;
        let handle = window.app_handle();
        state.characters.read().keys().filter_map(|label| {
            handle.get_webview_window(label)
                .and_then(|w| w.hwnd().ok())
                .map(|hwnd| (hwnd.0 as isize, label.clone()))
        }).collect()
    };
    #[cfg(not(windows))]
    let pets: HashMap<isize, String> = HashMap::new();
    Ok(serde_json::json!({ "windows": windows(&pets), "floors": floors }))
}
