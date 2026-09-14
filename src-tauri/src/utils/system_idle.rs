//! 系统级用户空闲检测
//!
//! 通过 Windows `GetLastInputInfo` 获取跨应用的最后一次键鼠输入时间，
//! 判定用户当前是否正在设备前使用设备。与前端 webview 事件追踪互补：
//! 前端只能感知宠物窗口内的活动，本模块可感知全系统的键鼠活动。

/// 获取系统级用户空闲秒数（最后一次键鼠输入至今的秒数）。
///
/// 返回 `None` 表示当前平台不支持或调用失败，调用方应回退到其它信号。
pub fn get_system_idle_seconds() -> Option<f64> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        use windows::Win32::System::SystemInformation::GetTickCount;

        unsafe {
            let mut info = LASTINPUTINFO {
                cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0,
            };
            // GetLastInputInfo 失败通常意味着参数错误，几乎不会发生
            if GetLastInputInfo(&mut info).as_bool() {
                let now_ticks = GetTickCount();
                // dwTime 与 GetTickCount 同源（均为系统启动以来的毫秒，u32）
                // 处理 u32 回绕：差值取模
                let elapsed_ms = now_ticks.wrapping_sub(info.dwTime);
                let secs = (elapsed_ms as f64) / 1000.0;
                // 极端回绕场景下可能得到超大值，做一次合理性裁剪
                if secs < 0.0 || secs > 86400.0 * 7.0 {
                    return Some(0.0);
                }
                return Some(secs);
            }
        }
        None
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}
