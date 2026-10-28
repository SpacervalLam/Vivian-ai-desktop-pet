//! Serial desktop input with bounded process lifetime and user-input yielding.
use crate::desktop_contract::{DesktopConfig, ScreenBounds};
use tauri::Manager;


pub fn config() -> DesktopConfig {
    let handle = super::system_ops::APP_HANDLE.read().clone();
    handle.and_then(|handle| handle.try_state::<std::sync::Arc<crate::state::AppState>>()
        .map(|state| state.config.read().get_typed("tools.desktop", DesktopConfig::default())))
        .unwrap_or_default().bounded()
}

pub fn screen_bounds() -> Result<ScreenBounds, String> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN};
        let bounds = unsafe { ScreenBounds {left:GetSystemMetrics(SM_XVIRTUALSCREEN),top:GetSystemMetrics(SM_YVIRTUALSCREEN),width:GetSystemMetrics(SM_CXVIRTUALSCREEN),height:GetSystemMetrics(SM_CYVIRTUALSCREEN)} };
        if bounds.width <= 0 || bounds.height <= 0 { return Err("Cannot read desktop bounds".into()); }
        Ok(bounds)
    }
    #[cfg(not(windows))]
    { Err("Native desktop input is currently supported on Windows only; use the browser bridge on this platform".into()) }
}

pub fn validate_point(x:i64,y:i64) -> Result<(),String> {
    if screen_bounds()?.contains(x,y) { Ok(()) } else { Err("Coordinates are outside the physical virtual desktop".into()) }
}
