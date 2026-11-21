//! Pinned-window capture: never fall back to a desktop screenshot.
use crate::{screen_capture::CaptureRegion, visual_stream::Frame};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub id: i64,
    pub pid: u32,
    pub title: String,
    pub bounds: [i32; 4],
}

#[cfg(windows)]
fn inspect(id: i64) -> Result<Target, String> {
    use windows::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        UI::WindowsAndMessaging::*,
    };
    unsafe {
        let hwnd = HWND(id as *mut _);
        if !IsWindow(Some(hwnd)).as_bool()
            || !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
        {
            return Err("window_unavailable".into());
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            return Err("select_other_app".into());
        }
        let mut text = [0u16; 2048];
        let length = GetWindowTextW(hwnd, &mut text);
        if length as usize >= text.len() - 1 {
            return Err("window_title_too_long".into());
        }
        let mut rect = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .map_err(|_| "window_unavailable")?;
        Ok(Target {
            id,
            pid,
            title: String::from_utf16_lossy(&text[..length as usize]),
            bounds: [rect.left, rect.top, rect.right, rect.bottom],
        })
    }
}

pub fn pin(x: i32, y: i32, width: u32, height: u32) -> Result<Target, String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{Foundation::POINT, UI::HiDpi::*, UI::WindowsAndMessaging::*};
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hwnd = GetAncestor(
            WindowFromPoint(POINT {
                x: x + (width / 2) as i32,
                y: y + (height / 2) as i32,
            }),
            GA_ROOT,
        );
        let result = inspect(hwnd.0 as i64).and_then(|target| {
            let [left, top, right, bottom] = target.bounds;
            if x < left
                || y < top
                || x as i64 + width as i64 > right as i64
                || y as i64 + height as i64 > bottom as i64
            {
                Err("select_inside_one_window".into())
            } else {
                Ok(target)
            }
        });
        SetThreadDpiAwarenessContext(old);
        result
    }
    #[cfg(not(windows))]
    {
        let _ = (x, y, width, height);
        Err("windows_required".into())
    }
}

impl Target {
    /// Only the window's existence matters. Title, position, size, visibility and
    /// minimized state may change freely; capture keeps running or waits for frames.
    pub fn validate(&self) -> Result<(), String> {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::{
                UI::HiDpi::*,
                UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow},
            };
            let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let hwnd = windows::Win32::Foundation::HWND(self.id as *mut _);
            let alive = IsWindow(Some(hwnd)).as_bool();
            let mut pid = 0;
            if alive {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            SetThreadDpiAwarenessContext(old);
            if !alive {
                return Err("window_unavailable".into());
            }
            if pid != self.pid || pid == std::process::id() {
                return Err("window_changed".into());
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            Err("windows_required".into())
        }
    }
}

/// What a watch session is allowed to look at.
#[derive(Clone, PartialEq, Eq)]
pub enum Source {
    Window(Target),
    /// One monitor of the virtual desktop; never the whole multi-monitor desktop.
    Desktop(DesktopTarget),
}
impl Source {
    pub fn bounds(&self) -> [i32; 4] {
        match self {
            Self::Window(target) => target.bounds,
            Self::Desktop(target) => target.bounds,
        }
    }
    pub fn is_desktop(&self) -> bool {
        matches!(self, Self::Desktop(_))
    }
    pub fn is_window(&self) -> bool {
        matches!(self, Self::Window(_))
    }
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Window(target) => target.validate(),
            Self::Desktop(target) => target.validate(),
        }
    }
    /// Hidden/minimized is a normal suspended state, not a capture failure.
    pub fn window_suspended(&self) -> bool {
        let Self::Window(target) = self else {
            return false;
        };
        #[cfg(windows)]
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
            let hwnd = windows::Win32::Foundation::HWND(target.id as *mut _);
            !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    pub fn work_area(&self) -> Option<[i32; 4]> {
        match self {
            Self::Window(_) => None,
            Self::Desktop(target) => Some(target.work),
        }
    }
}

/// A selection anchored to one monitor instead of one window.
#[derive(Clone, PartialEq, Eq)]
pub struct DesktopTarget {
    monitor: isize,
    pub bounds: [i32; 4],
    work: [i32; 4],
}
#[cfg(windows)]
impl DesktopTarget {
    pub fn pin(x: i32, y: i32, width: u32, height: u32) -> Result<Self, String> {
        use windows::Win32::{
            Foundation::POINT,
            Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
            UI::HiDpi::*,
        };
        unsafe {
            let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let result = (|| {
                let monitor = MonitorFromPoint(
                    POINT {
                        x: x + (width / 2) as i32,
                        y: y + (height / 2) as i32,
                    },
                    MONITOR_DEFAULTTONEAREST,
                );
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                    return Err("display_unavailable".to_string());
                }
                let rect = info.rcMonitor;
                if x < rect.left
                    || y < rect.top
                    || x as i64 + width as i64 > rect.right as i64
                    || y as i64 + height as i64 > rect.bottom as i64
                {
                    return Err("select_inside_one_monitor".to_string());
                }
                Ok(Self {
                    monitor: monitor.0 as isize,
                    bounds: [rect.left, rect.top, rect.right, rect.bottom],
                    work: [
                        info.rcWork.left,
                        info.rcWork.top,
                        info.rcWork.right,
                        info.rcWork.bottom,
                    ],
                })
            })();
            SetThreadDpiAwarenessContext(old);
            result
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        use windows::Win32::{
            Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
            UI::HiDpi::*,
        };
        let [left, top, right, bottom] = self.bounds;
        if right <= left || bottom <= top {
            return Err("display_changed".into());
        }
        unsafe {
            let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let result = (|| {
                let monitor = MonitorFromPoint(
                    windows::Win32::Foundation::POINT {
                        x: left + (right - left) / 2,
                        y: top + (bottom - top) / 2,
                    },
                    MONITOR_DEFAULTTONEAREST,
                );
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                    return Err("display_changed".to_string());
                }
                let rect = info.rcMonitor;
                if self.bounds != [rect.left, rect.top, rect.right, rect.bottom] {
                    return Err("display_changed".to_string());
                }
                Ok(())
            })();
            SetThreadDpiAwarenessContext(old);
            result
        }
    }
}
#[cfg(not(windows))]
impl DesktopTarget {
    pub fn pin(_x: i32, _y: i32, _width: u32, _height: u32) -> Result<Self, String> {
        Err("windows_required".into())
    }
    pub fn validate(&self) -> Result<(), String> {
        Err("windows_required".into())
    }
}

#[cfg(windows)]
pub struct Capture {
    // Session must be released before COM is uninitialized.
    stream: Option<crate::desktop_native::capture::WindowStream>,
}
#[cfg(windows)]
impl Capture {
    pub fn new(source: &Source) -> Result<Self, String> {
        use windows::Win32::System::Com::*;
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|_| "capture_unavailable")?;
        }
        let mut capture = Self { stream: None };
        source.validate()?;
        capture.stream = Some(
            match source {
                Source::Window(target) => {
                    crate::desktop_native::capture::WindowStream::new(
                        windows::Win32::Foundation::HWND(target.id as *mut _),
                    )
                }
                Source::Desktop(target) => {
                    crate::desktop_native::capture::WindowStream::new_monitor(
                        windows::Win32::Graphics::Gdi::HMONITOR(target.monitor as *mut _),
                    )
                }
            }
            .map_err(|_| "capture_unavailable")?,
        );
        capture.stream.as_ref().unwrap().set_watch_interval();
        Ok(capture)
    }
    pub fn frame(
        &self,
        source: &Source,
        crop: Option<CaptureRegion>,
        at: Duration,
        cancel: Arc<AtomicBool>,
    ) -> Result<Frame, String> {
        source.validate()?;
        let deadline = crate::desktop_native::Deadline {
            until: Instant::now() + Duration::from_millis(1800),
            cancelled: cancel,
        };
        let (rgba, width, height) = self
            .stream
            .as_ref()
            .ok_or("capture_unavailable")?
            .next_rgba(&deadline)
            .map_err(|e| {
                if e.contains("produced no capture frame") {
                    "no_new_frame"
                } else if e.contains("resized") {
                    "capture_resized"
                } else {
                    "capture_unavailable"
                }
            })?;
        source.validate()?;
        // Window mode always sends the whole window: the click interaction selects
        // a window, and a resize must not silently cut it down to an old rect.
        if source.is_window() {
            return Frame::new(width, height, rgba, at);
        }
        let region = crop.ok_or("capture_unavailable")?;
        let [left, top, right, bottom] = source.bounds();
        // Desktop capture must still match the monitor geometry.
        if width as i64 != right as i64 - left as i64 || height as i64 != bottom as i64 - top as i64
        {
            return Err("capture_geometry_changed".into());
        }
        let region = region.validate(width, height)?;
        let mut pixels = Vec::with_capacity(region.width as usize * region.height as usize * 4);
        for y in region.y..region.y + region.height {
            let start = (y as usize * width as usize + region.x as usize) * 4;
            pixels.extend_from_slice(&rgba[start..start + region.width as usize * 4]);
        }
        Frame::new(region.width, region.height, pixels, at)
    }
}
#[cfg(windows)]
impl Drop for Capture {
    fn drop(&mut self) {
        self.stream.take();
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}

/// A visible top-level window under a screen point, ignoring this app's own
/// windows and the desktop shell.
#[derive(Clone)]
pub struct HoverWindow {
    pub title: String,
    pub bounds: [i32; 4],
}

#[cfg(windows)]
unsafe fn hover_bounds(hwnd: windows::Win32::Foundation::HWND) -> Option<[i32; 4]> {
    use windows::Win32::{
        Foundation::RECT,
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        UI::WindowsAndMessaging::{GetWindowRect, IsWindowVisible},
    };
    if !IsWindowVisible(hwnd).as_bool() {
        return None;
    }
    let mut rect = RECT::default();
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        (&mut rect as *mut RECT).cast(),
        std::mem::size_of::<RECT>() as u32,
    )
    .is_err()
        && GetWindowRect(hwnd, &mut rect).is_err()
    {
        return None;
    }
    (rect.right > rect.left && rect.bottom > rect.top)
        .then_some([rect.left, rect.top, rect.right, rect.bottom])
}

#[cfg(windows)]
pub fn window_at(x: i32, y: i32) -> Option<HoverWindow> {
    use windows::Win32::{
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
        UI::HiDpi::*,
        UI::WindowsAndMessaging::*,
    };
    let shell = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];
    unsafe {
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = (|| {
            let mut hwnd = GetTopWindow(None).ok()?;
            while !hwnd.0.is_null() {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let mut cloaked = 0u32;
                let _ = DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAKED,
                    (&mut cloaked as *mut u32).cast(),
                    std::mem::size_of::<u32>() as u32,
                );
                let mut class = [0u16; 64];
                let class_len = GetClassNameW(hwnd, &mut class);
                let class = String::from_utf16_lossy(&class[..class_len.max(0) as usize]);
                if pid != 0
                    && pid != std::process::id()
                    && cloaked == 0
                    && !IsIconic(hwnd).as_bool()
                    && !shell.contains(&class.as_str())
                {
                    if let Some([left, top, right, bottom]) = hover_bounds(hwnd) {
                        if x >= left && x < right && y >= top && y < bottom {
                            let mut title = [0u16; 1024];
                            let length = GetWindowTextW(hwnd, &mut title).max(0) as usize;
                            return Some(HoverWindow {
                                title: String::from_utf16_lossy(&title[..length]),
                                bounds: [left, top, right, bottom],
                            });
                        }
                    }
                }
                hwnd = GetWindow(hwnd, GW_HWNDNEXT).ok()?;
            }
            None
        })();
        SetThreadDpiAwarenessContext(old);
        result
    }
}
#[cfg(not(windows))]
pub fn window_at(_x: i32, _y: i32) -> Option<HoverWindow> {
    None
}
