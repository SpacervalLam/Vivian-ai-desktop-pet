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

/// 获取当前前台窗口信息（Windows 平台通过 Win32 API 直接获取，无进程创建开销）。
///
/// 非 Windows 平台返回默认值。
pub fn get_foreground_window() -> ForegroundWindowSnapshot {
    #[cfg(target_os = "windows")]
    {
        try_get_foreground_windows().unwrap_or_default()
    }
    #[cfg(not(target_os = "windows"))]
    {
        ForegroundWindowSnapshot::default()
    }
}

#[cfg(target_os = "windows")]
fn try_get_foreground_windows() -> Option<ForegroundWindowSnapshot> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    unsafe {
        let hwnd: HWND = GetForegroundWindow();
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

        // 跳过应用自身的窗口（主窗口 + 所有子窗口）
        if pid == std::process::id() {
            return None;
        }

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
static FOREGROUND_NOTIFY: std::sync::OnceLock<Arc<tokio::sync::Notify>> = std::sync::OnceLock::new();

#[cfg(windows)]
unsafe extern "system" fn win_event_proc(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    _hwnd: windows::Win32::Foundation::HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
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

#[cfg(not(windows))]
pub fn subscribe_foreground_events(
    _notify: Arc<tokio::sync::Notify>,
) -> Option<()> {
    None
}
