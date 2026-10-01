//! 前台窗口感知 —— 获取用户当前正在使用的应用。
//!
//! 事件驱动：通过 SetWinEventHook(EVENT_SYSTEM_FOREGROUND) 监听前台窗口切换。
//! 10s 兜底刷新防止事件丢失。

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 前台窗口快照
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForegroundWindowSnapshot {
    /// 窗口标题
    pub title: String,
    /// 进程名
    pub process: String,
    /// 进程 ID
    pub pid: u32,
}

/// 仅保存在本次运行内；历史应用不等同于当前前台或仍在运行。
#[derive(Debug, Clone, Serialize)]
pub struct ExternalAppObservation {
    pub window: ForegroundWindowSnapshot,
    pub observed_at_unix_ms: i64,
}

static LAST_EXTERNAL_APP: parking_lot::Mutex<Option<ExternalAppObservation>> = parking_lot::Mutex::new(None);

fn remember_external_app(
    cache: &mut Option<ExternalAppObservation>,
    window: &ForegroundWindowSnapshot,
    companion_pid: u32,
    observed_at_unix_ms: i64,
) {
    if window.pid != 0 && window.pid != companion_pid {
        *cache = Some(ExternalAppObservation { window: window.clone(), observed_at_unix_ms });
    }
}

pub fn last_external_app() -> Option<ExternalAppObservation> {
    LAST_EXTERNAL_APP.lock().clone()
}

fn record_external_app(window: &ForegroundWindowSnapshot) {
    remember_external_app(&mut LAST_EXTERNAL_APP.lock(), window, std::process::id(), chrono::Utc::now().timestamp_millis());
}

/// 获取当前前台窗口信息（Windows 平台通过 Win32 API 直接获取，无进程创建开销）。
///
/// 非 Windows 平台返回默认值。
pub fn get_foreground_window() -> ForegroundWindowSnapshot {
    let window = get_current_foreground_window();
    if window.pid == std::process::id() {
        ForegroundWindowSnapshot::default()
    } else {
        window
    }
}

/// 用户主动查询时保留自身窗口，避免把聊天窗口获得焦点误报为感知失败。
pub fn get_current_foreground_window() -> ForegroundWindowSnapshot {
    #[cfg(target_os = "windows")]
    {
        let window = try_get_foreground_windows().unwrap_or_default();
        record_external_app(&window);
        window
    }
    #[cfg(not(target_os = "windows"))]
    {
        ForegroundWindowSnapshot::default()
    }
}

#[cfg(target_os = "windows")]
fn try_get_foreground_windows() -> Option<ForegroundWindowSnapshot> {
    unsafe { try_get_window_windows(windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow()) }
}

#[cfg(target_os = "windows")]
fn try_get_window_windows(hwnd: windows::Win32::Foundation::HWND) -> Option<ForegroundWindowSnapshot> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowTextW, GetWindowThreadProcessId,
    };

    unsafe {
        if hwnd.0.is_null() {
            return None;
        }

        let mut buf = [0u16; 512];
        let title_len = GetWindowTextW(hwnd, &mut buf);
        let title = if title_len > 0 {
            String::from_utf16_lossy(&buf[..title_len as usize])
        } else {
            String::new()
        };

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        let process = if pid > 0 {
            get_process_name(pid).unwrap_or_default()
        } else {
            String::new()
        };

        Some(ForegroundWindowSnapshot {
            title,
            process,
            pid,
        })
    }
}

/// 通过 PID 获取进程可执行文件名（不含路径和扩展名）
#[cfg(target_os = "windows")]
fn get_process_name(pid: u32) -> Option<String> {
    use std::path::Path;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buf = [0u16; MAX_PATH as usize];
        let mut size = buf.len() as u32;
        let pwstr = PWSTR::from_raw(buf.as_mut_ptr());
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, pwstr, &mut size);
        CloseHandle(handle).ok();

        if ok.is_err() || size == 0 {
            return None;
        }

        let full_path = String::from_utf16_lossy(&buf[..size as usize]);
        Path::new(&full_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    }
}

// ─── 前台窗口切换事件订阅 ──────────────────────────────────────────────────────

/// 前台窗口事件守卫 —— Drop 时停止钩子线程。
#[cfg(windows)]
pub struct ForegroundEventGuard {
    thread: Option<std::thread::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread_id: Arc<std::sync::atomic::AtomicU32>,
}

#[cfg(windows)]
impl Drop for ForegroundEventGuard {
    fn drop(&mut self) {
        self.stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let tid = self.thread_id.load(std::sync::atomic::Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                    tid,
                    windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(windows)]
impl ForegroundEventGuard {
    /// 钩子线程是否仍在运行（`GetMessageW` 循环尚未退出）。
    fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

/// 判断前台钩子是否需要（重）安装。
///
/// 钩子健康时应长期持有同一个 guard，**不要周期性卸载重装**——重装间隙
/// （`UnhookWinEvent` → `SetWinEventHook`）会丢掉期间的前台切换事件。
///
/// 返回 `true` 表示「需要重装」：guard 缺失，或钩子线程已退出。
/// 非 Windows 平台不安装钩子，恒返回 `true`（调用方走退避重试分支）。
#[cfg(windows)]
pub fn foreground_hook_needs_install(guard: &Option<ForegroundEventGuard>) -> bool {
    match guard {
        Some(g) => !g.is_alive(),
        None => true,
    }
}

#[cfg(not(windows))]
pub fn foreground_hook_needs_install(_guard: &Option<()>) -> bool {
    true
}

#[cfg(windows)]
static FOREGROUND_NOTIFY: std::sync::OnceLock<Arc<tokio::sync::Notify>> = std::sync::OnceLock::new();

#[cfg(windows)]
unsafe extern "system" fn win_event_proc(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    hwnd: windows::Win32::Foundation::HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // 用事件携带的 HWND 保存观察，避免异步消费者醒来时焦点已切到聊天窗口。
    if let Some(window) = try_get_window_windows(hwnd) {
        record_external_app(&window);
    }
    if let Some(n) = FOREGROUND_NOTIFY.get() {
        n.notify_one();
    }
}

/// 订阅前台窗口切换事件（启动专用消息泵线程）。
///
/// 通过 SetWinEventHook(EVENT_SYSTEM_FOREGROUND) 监听窗口切换，
/// 事件触发时通过 Notify 通知异步循环。
/// 返回守卫结构，Drop 时停止钩子线程。
#[cfg(windows)]
pub fn subscribe_foreground_events(
    notify: Arc<tokio::sync::Notify>,
) -> Option<ForegroundEventGuard> {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Accessibility::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    FOREGROUND_NOTIFY.get_or_init(|| notify);

    let stop = Arc::new(AtomicBool::new(false));
    let thread_id = Arc::new(AtomicU32::new(0));

    let stop_clone = stop.clone();
    let tid_clone = thread_id.clone();

    let thread = std::thread::Builder::new()
        .name("foreground-hook".into())
        .spawn(move || {
            unsafe {
                tid_clone.store(GetCurrentThreadId(), Ordering::SeqCst);

                let hook = SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(win_event_proc),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                );

                if hook.0.is_null() {
                    tracing::warn!("[ForegroundHook] SetWinEventHook 失败");
                    return;
                }

                tracing::info!("[ForegroundHook] 前台窗口事件钩子已安装");
                let _ = get_current_foreground_window();

                let mut msg = std::mem::zeroed();
                while !stop_clone.load(Ordering::SeqCst) {
                    let ret = GetMessageW(&mut msg, None, 0, 0);
                    if !ret.as_bool() {
                        break;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                let _ = UnhookWinEvent(hook);
                tracing::info!("[ForegroundHook] 前台窗口事件钩子已卸载");
            }
        })
        .ok()?;

    Some(ForegroundEventGuard {
        thread: Some(thread),
        stop,
        thread_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_focus_and_missing_window_preserve_last_external_observation() {
        let mut cache = None;
        let external = ForegroundWindowSnapshot { title: "Editing".into(), process: "editor".into(), pid: 7 };
        remember_external_app(&mut cache, &external, 42, 1000);
        remember_external_app(&mut cache, &ForegroundWindowSnapshot { pid: 42, ..Default::default() }, 42, 2000);
        remember_external_app(&mut cache, &ForegroundWindowSnapshot::default(), 42, 3000);
        let observation = cache.unwrap();
        assert_eq!(observation.window.pid, 7);
        assert_eq!(observation.observed_at_unix_ms, 1000);
    }

    #[test]
    fn next_external_app_replaces_previous_observation() {
        let mut cache = None;
        remember_external_app(&mut cache, &ForegroundWindowSnapshot { pid: 7, ..Default::default() }, 42, 1000);
        remember_external_app(&mut cache, &ForegroundWindowSnapshot { pid: 8, ..Default::default() }, 42, 4000);
        let observation = cache.unwrap();
        assert_eq!(observation.window.pid, 8);
        assert_eq!(observation.observed_at_unix_ms, 4000);
    }
}

#[cfg(not(windows))]
pub fn subscribe_foreground_events(
    _notify: Arc<tokio::sync::Notify>,
) -> Option<()> {
    None
}
