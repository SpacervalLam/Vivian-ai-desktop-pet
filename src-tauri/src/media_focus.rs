//! 把「正在播放的媒体来源应用」切到前台 —— chat window 灵动岛点击后的落点。
//!
//! SMTC 给出的 `source_app` 是一个 AUMID（如 `Spotify.exe` /
//! `Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic`），不是可启动路径。
//! 这里只做两件事，不猜、不遍历注册表：
//!   1. 按进程匹配顶层窗口，优先可见窗口，再恢复托盘隐藏的主窗口；
//!   2. 没命中且是 UWP 包（AUMID 含 `!`）时，交给 `shell:AppsFolder\<AUMID>` 由系统激活。
//!
//! 置前之所以通常能成功：用户是**点击我们窗口里的灵动岛**触发的，此刻本进程
//! 就是前台进程，`SetForegroundWindow` 的前台锁对本进程放行，无需 AttachThreadInput 绕行。
//!
//! 非 Windows 平台为空实现。

/// 从 AUMID 里取出用于匹配进程可执行名的主干（小写、去掉 `.exe`）。
///
/// - Win32 应用：`Spotify.exe` → `spotify`
/// - UWP 包：`Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic` → `microsoft.zunemusic`
///   （这种主干通常匹配不到真实进程名，交给 AppsFolder 兜底）
#[cfg_attr(not(windows), allow(dead_code))]
pub fn needle_of(source_app: &str) -> String {
    let raw = source_app.trim();
    // UWP 的 AUMID 形如 `包名!入口`，取 `!` 之后那一段
    let after_bang = raw.rsplit('!').next().unwrap_or(raw);
    // 去掉可能存在的目录前缀
    let base = after_bang.rsplit(|c| c == '\\' || c == '/').next().unwrap_or(after_bang);
    let stem = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    stem.trim().to_lowercase()
}

/// 把来源应用切到前台；返回一句可直接展示给用户的说明。
#[cfg(windows)]
pub fn focus(source_app: &str) -> Result<String, String> {
    let needle = needle_of(source_app);
    if needle.is_empty() {
        return Err("没有可用的媒体来源信息。".to_string());
    }
    if let Some(hwnd) = native::find_window(&needle) {
        if !native::bring_to_front(hwnd) {
            return Err("已找到播放器，但未能将窗口切到前台，请重试。".to_string());
        }
        return Ok("已切到正在播放的应用。".to_string());
    }
    // UWP 包的顶层窗口常挂在 ApplicationFrameHost 上，按进程名匹配不到，
    // 交给系统按 AUMID 激活：已启动则置前，未启动则拉起。
    if source_app.contains('!') {
        crate::utils::process::silent_command("explorer")
            .arg(format!("shell:AppsFolder\\{}", source_app.trim()))
            .spawn()
            .map_err(|e| format!("无法唤起该应用：{e}"))?;
        return Ok("已尝试唤起该应用。".to_string());
    }
    Err("没有找到正在播放的应用主窗口。".to_string())
}

#[cfg(not(windows))]
pub fn focus(_source_app: &str) -> Result<String, String> {
    Err("当前平台尚未支持媒体来源跳转。".to_string())
}

#[cfg(windows)]
mod native {
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GW_OWNER, SW_RESTORE, SW_SHOW, WS_EX_TOOLWINDOW,
    };

    /// 穿过 C 回调的检索上下文：要找谁、找到没有。
    struct Search {
        needle: String,
        self_pid: u32,
        hit: Option<HWND>,
        hidden_hit: Option<HWND>,
    }

    /// 取进程可执行名主干（小写、无扩展名）。
    fn process_stem(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; MAX_PATH as usize];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR::from_raw(buf.as_mut_ptr()),
                &mut size,
            );
            let _ = CloseHandle(handle);
            if ok.is_err() || size == 0 {
                return None;
            }
            let full = String::from_utf16_lossy(&buf[..size as usize]);
            let name = full.rsplit(|c| c == '\\' || c == '/').next().unwrap_or(full.as_str());
            let stem = name
                .strip_suffix(".exe")
                .or_else(|| name.strip_suffix(".EXE"))
                .unwrap_or(name);
            Some(stem.trim().to_lowercase())
        }
    }

    /// 顶层窗口枚举回调。返回 `BOOL(0)` 停止枚举（已命中）。
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        // 托盘主窗口不可见，但仍可枚举；排除工具窗、无标题辅助窗和有所有者的弹窗。
        if GetWindowTextLengthW(hwnd) == 0 {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == search.self_pid {
            return BOOL(1);
        }
        let Some(stem) = process_stem(pid) else {
            return BOOL(1);
        };
        if stem == search.needle || (search.needle.len() >= 4 && stem.contains(&search.needle)) {
            if IsWindowVisible(hwnd).as_bool() {
                search.hit = Some(hwnd);
                return BOOL(0);
            }
            let tool_window = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
            let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.0.is_null());
            if !tool_window && !owned && search.hidden_hit.is_none() {
                search.hidden_hit = Some(hwnd);
            }
        }
        BOOL(1)
    }

    /// 可见窗口优先；没有可见窗口时返回托盘隐藏的主窗口。
    pub fn find_window(needle: &str) -> Option<HWND> {
        let mut search = Search {
            needle: needle.to_string(),
            self_pid: std::process::id(),
            hit: None,
            hidden_hit: None,
        };
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
        }
        search.hit.or(search.hidden_hit)
    }

    /// 还原（若最小化）并置前。
    pub fn bring_to_front(hwnd: HWND) -> bool {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            } else if !IsWindowVisible(hwnd).as_bool() {
                // 托盘隐藏不是最小化，SW_SHOW 保留窗口原来的最大化状态。
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            IsWindowVisible(hwnd).as_bool() && SetForegroundWindow(hwnd).as_bool()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::needle_of;

    #[test]
    fn strips_exe_and_lowercases() {
        assert_eq!(needle_of("Spotify.exe"), "spotify");
        assert_eq!(needle_of("  QQMusic.EXE "), "qqmusic");
        assert_eq!(needle_of("cloudmusic"), "cloudmusic");
    }

    #[test]
    fn takes_uwp_entry_after_bang() {
        assert_eq!(
            needle_of("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "microsoft.zunemusic"
        );
    }

    #[test]
    fn drops_directory_prefix() {
        assert_eq!(needle_of("C:\\Program Files\\App\\cloudmusic.exe"), "cloudmusic");
        assert_eq!(needle_of(""), "");
    }
}
