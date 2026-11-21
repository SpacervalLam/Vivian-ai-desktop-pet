//! Temporary desktop-capture exclusion for our own pet windows only.
//! Window-scoped WGC watchers neither use this guard nor share its lock.
#[cfg(windows)]
pub(crate) struct CaptureExclusion {
    previous: Vec<(windows::Win32::Foundation::HWND, u32)>,
}
// Window handles are opaque process-wide values. Both affinity calls made by
// this guard are valid from any thread, so the guard can travel with a session.
#[cfg(windows)]
unsafe impl Send for CaptureExclusion {}
#[cfg(windows)]
impl CaptureExclusion {
    pub(crate) fn new(
        windows: impl IntoIterator<Item = windows::Win32::Foundation::HWND>,
    ) -> Result<Self, ()> {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let mut guard = Self {
            previous: Vec::new(),
        };
        for hwnd in windows {
            let mut previous = 0;
            unsafe {
                GetWindowDisplayAffinity(hwnd, &mut previous).map_err(|_| ())?;
                if previous != WDA_EXCLUDEFROMCAPTURE.0 {
                    SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).map_err(|_| ())?;
                    guard.previous.push((hwnd, previous));
                }
            }
        }
        Ok(guard)
    }
}
#[cfg(windows)]
impl Drop for CaptureExclusion {
    fn drop(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::*;
        for &(hwnd, previous) in self.previous.iter().rev() {
            let _ = unsafe { SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(previous)) };
        }
    }
}
