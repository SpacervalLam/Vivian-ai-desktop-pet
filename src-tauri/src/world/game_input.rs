//! Passive Raw Input transport owned by EnvironmentAgent. No interception, text or input history.
use super::game_input_policy::{InputActivity, InputIntensity};
use parking_lot::Mutex;
use std::sync::{Arc, atomic::{AtomicBool, AtomicU32, Ordering}};
use std::time::Instant;

struct Shared {
    target: AtomicU32,
    thread: AtomicU32,
    ready: AtomicBool,
    scope_valid: AtomicBool,
    stop: AtomicBool,
    started: Instant,
    intensity: Mutex<InputIntensity>,
}
impl Shared {
    fn now(&self) -> u64 { self.started.elapsed().as_millis() as u64 }
}

pub(super) struct GameInputMonitor { shared: Arc<Shared> }
impl GameInputMonitor {
    pub fn new() -> Self {
        let shared = Arc::new(Shared { target: AtomicU32::new(0), thread: AtomicU32::new(0),
            ready: AtomicBool::new(false), scope_valid: AtomicBool::new(false), stop: AtomicBool::new(false),
            started: Instant::now(), intensity: Mutex::new(InputIntensity::default()) });
        #[cfg(windows)]
        {
            let source = shared.clone();
            if let Err(error) = std::thread::Builder::new().name("environment-game-input".into()).spawn(move || native::run(source)) {
                tracing::warn!("[EnvironmentAgent] Cannot start game input observer: {error}");
            }
        }
        Self { shared }
    }

    pub fn set_target(&self, pid: u32) {
        {
            let mut intensity = self.shared.intensity.lock();
            if self.shared.target.load(Ordering::Acquire) == pid { return; }
            self.shared.target.store(pid, Ordering::Release);
            self.shared.ready.store(false, Ordering::Release);
            self.shared.scope_valid.store(false, Ordering::Release);
            intensity.reset(self.shared.now());
        }
        #[cfg(windows)]
        native::wake(&self.shared, native::CONFIGURE);
    }

    pub fn snapshot(&self) -> InputActivity {
        let mut intensity = self.shared.intensity.lock();
        intensity.snapshot(self.shared.now(), self.shared.target.load(Ordering::Acquire) != 0
            && self.shared.ready.load(Ordering::Acquire) && self.shared.scope_valid.load(Ordering::Acquire))
    }
}
impl Drop for GameInputMonitor {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.target.store(0, Ordering::Release);
        #[cfg(windows)]
        native::wake(&self.shared, windows::Win32::UI::WindowsAndMessaging::WM_QUIT);
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use windows::{core::PCWSTR, Win32::{Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{Input::*, WindowsAndMessaging::*}}};
    pub(super) const CONFIGURE: u32 = WM_APP + 37;

    pub(super) fn wake(shared: &Shared, message: u32) {
        let thread = shared.thread.load(Ordering::Acquire);
        if thread != 0 { unsafe { let _ = PostThreadMessageW(thread, message, WPARAM(0), LPARAM(0)); } }
    }

    unsafe fn register(hwnd: HWND, enabled: bool) -> bool {
        let flags = if enabled { RIDEV_INPUTSINK } else { RIDEV_REMOVE };
        let target = if enabled { hwnd } else { HWND::default() };
        // Passive delivery only: never request NOLEGACY, capture or hotkey suppression.
        RegisterRawInputDevices(&[
            RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: flags, hwndTarget: target },
            RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: flags, hwndTarget: target },
        ], std::mem::size_of::<RAWINPUTDEVICE>() as u32).is_ok()
    }

    unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared;
        if message == WM_INPUT && !pointer.is_null() { observe(&*pointer, lparam); }
        if message == WM_NCDESTROY { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }

    unsafe fn observe(shared: &Shared, lparam: LPARAM) {
        let expected = shared.target.load(Ordering::Acquire);
        if expected == 0 || !shared.ready.load(Ordering::Acquire) { return; }
        // Scope validation only, not another application tracker. Alt-tab input is discarded
        // immediately even before the environment's foreground event consumer wakes up.
        let mut actual = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut actual));
        if actual != expected {
            if shared.scope_valid.swap(false, Ordering::AcqRel) { shared.intensity.lock().reset(shared.now()); }
            return;
        }
        let mut raw = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        let read = GetRawInputData(HRAWINPUT(lparam.0 as *mut _), RID_INPUT,
            Some((&mut raw as *mut RAWINPUT).cast()), &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32);
        if read == u32::MAX || read < std::mem::size_of::<RAWINPUTHEADER>() as u32 { return; }
        let mut intensity = shared.intensity.lock();
        if shared.target.load(Ordering::Acquire) != expected || !shared.ready.load(Ordering::Acquire) { return; }
        shared.scope_valid.store(true, Ordering::Release);
        let now = shared.now();
        if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
            let key = raw.data.keyboard;
            intensity.key(now, key.VKey, key.Flags & RI_KEY_BREAK as u16 == 0);
        } else if raw.header.dwType == RIM_TYPEMOUSE.0 {
            let mouse = raw.data.mouse;
            let buttons = mouse.Anonymous.Anonymous;
            let wheel = if buttons.usButtonFlags as u32 & (RI_MOUSE_WHEEL | RI_MOUSE_HWHEEL) != 0 {
                u32::from((buttons.usButtonData as i16).unsigned_abs()).div_ceil(120)
            } else { 0 };
            // Absolute tablet coordinates are positions, not relative camera motion.
            let (dx, dy) = if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 { (0, 0) } else { (mouse.lLastX, mouse.lLastY) };
            intensity.mouse(now, dx, dy, buttons.usButtonFlags & 0x03ff, wheel);
        }
    }

    pub(super) fn run(shared: Arc<Shared>) {
        unsafe {
            let Ok(module) = GetModuleHandleW(None) else { return; };
            let instance = HINSTANCE(module.0);
            let class: Vec<u16> = format!("VivianEnvironmentGameInput_{}\0", GetCurrentThreadId()).encode_utf16().collect();
            let name = PCWSTR(class.as_ptr());
            let descriptor = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance,
                lpszClassName: name, ..Default::default() };
            if RegisterClassW(&descriptor) == 0 { return; }
            let window = CreateWindowExW(WS_EX_NOACTIVATE, name, name, WINDOW_STYLE::default(),
                0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(instance), Some(Arc::as_ptr(&shared).cast()));
            let Ok(hwnd) = window else { let _ = UnregisterClassW(name, Some(instance)); return; };
            shared.thread.store(GetCurrentThreadId(), Ordering::Release);
            let mut registered = false;
            let configure = |registered: &mut bool| {
                let enabled = shared.target.load(Ordering::Acquire) != 0;
                let success = if enabled == *registered { true } else { register(hwnd, enabled) };
                if success { *registered = enabled; }
                shared.ready.store(success && enabled, Ordering::Release);
                shared.scope_valid.store(success && enabled, Ordering::Release);
            };
            configure(&mut registered);
            let mut message = MSG::default();
            while !shared.stop.load(Ordering::Acquire) {
                if GetMessageW(&mut message, None, 0, 0).0 <= 0 { break; }
                if message.message == CONFIGURE { configure(&mut registered); continue; }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            shared.ready.store(false, Ordering::Release);
            if registered { let _ = register(hwnd, false); }
            let _ = DestroyWindow(hwnd);
            let _ = UnregisterClassW(name, Some(instance));
            shared.thread.store(0, Ordering::Release);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn disabled_observer_creates_no_input_subscription_and_shuts_down_its_hidden_window() {
        let monitor = GameInputMonitor::new();
        let shared = monitor.shared.clone();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while shared.thread.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_ne!(shared.thread.load(Ordering::Acquire), 0, "native message window must initialize");
        assert!(!monitor.snapshot().available);
        assert!(!shared.ready.load(Ordering::Acquire), "disabled observer must not register input devices");
        drop(monitor);
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while shared.thread.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(shared.thread.load(Ordering::Acquire), 0, "shutdown must release the native window/thread");
    }
}
