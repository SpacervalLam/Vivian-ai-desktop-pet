//! Serial desktop input with bounded process lifetime and user-input yielding.
use std::sync::LazyLock;
use std::time::Duration;
use crate::desktop_contract::{DesktopConfig, ScreenBounds};
use tauri::Manager;

static INPUT_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

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

pub async fn run_input(script: &str) -> Result<String,String> {
    screen_bounds()?;
    let cfg=config();
    let _guard=tokio::time::timeout(Duration::from_millis(cfg.max_yield_wait_ms),INPUT_LOCK.lock()).await
        .map_err(|_|"Another desktop operation is still running".to_string())?;
    let started=tokio::time::Instant::now();
    loop {
        let idle=crate::utils::system_idle::get_system_idle_seconds()
            .ok_or_else(||"Cannot determine user input activity; desktop input was not sent".to_string())?;
        if idle*1000.0 >= cfg.user_idle_ms as f64 { break; }
        if started.elapsed() >= Duration::from_millis(cfg.max_yield_wait_ms) {
            return Err("User input is still active; desktop input was not sent".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let wrapped=format!(r#"$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8;
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class VivianInputDpi{{[DllImport("user32.dll")]public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr value);}}';
$previousDpi=[VivianInputDpi]::SetThreadDpiAwarenessContext([IntPtr](-4));
if($previousDpi -eq [IntPtr]::Zero){{throw 'Cannot enable physical desktop coordinates'}};
try{{{script}}}finally{{[VivianInputDpi]::SetThreadDpiAwarenessContext($previousDpi)|Out-Null}}
"#);
    let mut command=crate::utils::process::silent_command_async("powershell");
    command.args(["-NoProfile","-NonInteractive","-STA","-Command",&wrapped])
        .kill_on_drop(true).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let child=command.spawn().map_err(|e|format!("Cannot start desktop input engine: {e}"))?;
    crate::utils::process::assign_child_to_job(&child);
    let output=tokio::time::timeout(Duration::from_millis(cfg.operation_timeout_ms),child.wait_with_output()).await
        .map_err(|_|"Desktop input timed out; outcome is unknown. Observe the screen before retrying".to_string())?
        .map_err(|e|e.to_string())?;
    if !output.status.success() { return Err(String::from_utf8_lossy(&output.stderr).trim().to_string()); }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
