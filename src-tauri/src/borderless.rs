//! 无边框窗口的非客户区绘制保护。
//!
//! `decorations(false)` 只去掉窗口的标题栏样式，Windows 仍会按带边框窗口给它发
//! `WM_NCPAINT` / `WM_NCACTIVATE`。桌宠、气泡这些都是逐像素透明的窗口：非客户区一旦
//! 被 `DefWindowProc` 画上标题栏，这层像素不会被 WebView 的透明内容覆盖，于是表现为
//!
//! - 窗口顶部凭空多出一条原生标题栏（标题 + 最小化/关闭按钮）；
//! - 客户区被标题栏占掉一截，贴边裁剪后桌宠显示残缺；
//! - 贴边平移/显示隐藏之后仍然残留，只有整窗重绘才会消失。
//!
//! 实测真正的绘制入口是 `DefWindowProc` 对非客户区消息的默认处理：窗口被激活或重新
//! 显示时，它会按系统帧样式把整条标题栏画进窗口表面，即使 `GWL_STYLE` 里的
//! `WS_CAPTION` 已经被去掉。配套的 tao 补丁（见 `../vendor/tao/PATCH.md`）让无边框
//! 窗口的 `WM_NCACTIVATE` 不再落到 `DefWindowProc`，这里再兜住零散的 `WM_NCPAINT`。
//!
//! `WM_NCACTIVATE` 本身不能吞：tao 依赖它维护窗口激活状态并派发 `Focused` 事件，
//! 所以这里只拦 `WM_NCPAINT`，其余消息原样交回窗口过程。

/// 给无边框窗口挂上非客户区绘制保护（幂等，可对同一窗口重复调用）。
pub fn guard(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::SetWindowSubclass;

        let Ok(raw) = window.hwnd() else { return };
        let hwnd = HWND(raw.0);
        unsafe {
            if window.label() == "config" {
                use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR};
                // Remove tao's shadow insets as well as the native border.
                // Disabling DWM painting alone leaves a white rectangle.
                let _ = window.set_shadow(false);
                let no_border: u32 = 0xfffffffe;
                let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&no_border as *const u32).cast(), 4);
            }
            let _ = SetWindowSubclass(hwnd, Some(nc_paint_guard), SUBCLASS_ID, 0);
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
const SUBCLASS_ID: usize = 0x5649_564E; // "VIVN"

#[cfg(windows)]
unsafe extern "system" fn nc_paint_guard(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};

    const WM_NCDESTROY: u32 = 0x0082;
    const WM_NCPAINT: u32 = 0x0085;

    match msg {
        // 无边框窗口没有非客户区可画；返回 0 表示已处理。
        WM_NCPAINT => LRESULT(0),
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(nc_paint_guard), id);
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}
