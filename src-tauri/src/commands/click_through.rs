//! 窗口点击穿透 —— 补充机制：WebView2 子窗口子类化（WM_NCHITTEST / HTTRANSPARENT）
//!
//! ## 背景
//!
//! Vivian 桌宠角色窗口为 `decorations:false + transparent:true + always_on_top` 的
//! 无边框窗口，需要在"中心 1/3 宽 × 4/9 高"矩形区域响应鼠标（拖动、点击、右键呼出菜单），
//! 外围区域（模型透明背景）则把鼠标事件穿透到下层窗口（桌面/其他应用）。
//!
//! ## 主方案：`set_ignore_cursor_events` 动态切换
//!
//! 真正的穿透切换由 `cursor_tracking` 线程负责：每 60ms 检查光标位置，
//! 在中心矩形外时调用 `WebviewWindow::set_ignore_cursor_events(true)` 让窗口穿透，
//! 在中心矩形内（或 suspend/拖动期间）调用 `set_ignore_cursor_events(false)` 恢复响应。
//!
//! `set_ignore_cursor_events(true)` 底层给顶层窗口加 `WS_EX_TRANSPARENT`，
//! 这会让**整个窗口**（包括所有子窗口）对鼠标透明，鼠标事件直接传递给下层窗口
//! （桌面/其他应用），不会进入子窗口的命中测试。因此该 API 在 Tauri v2 + WebView2
//! 上是可用的，之前注释中"不可用"的结论有误。
//!
//! ## 补充方案：子类化（本文件）
//!
//! 作为额外保障，本文件对顶层 Tauri 窗口和 WebView2 相关后代 HWND 安装子类化，
//! 在 `WM_NCHITTEST` 中按中心矩形判定返回 `HTCLIENT` / `HTTRANSPARENT`。
//!
//! 注意：`SetWindowLongPtrW` 对 WebView2 深层渲染子窗口（`Chrome_RenderWidgetHostHWND`、
//! `Intermediate D3D Window` 等）会因跨进程限制失败（返回 `prev_wndproc=0x0`），
//! 所以子类化主要对顶层 `Tauri Window` 和 `Chrome_WidgetWin_0` 生效。
//! 主穿透逻辑不依赖子类化，而是依赖上述 `set_ignore_cursor_events` 动态切换。
//!
//! `suspend_click_through` / `resume_click_through` 由前端在右键菜单、Toast、气泡、
//! InputDialog 显示期间调用；当暂停计数器 > 0 时，`cursor_tracking` 线程会
//! 调用 `set_ignore_cursor_events(false)` 让整窗响应鼠标。
//!
//! 拖动窗口时（`DRAG_OFFSET` 表中存在本窗口 label），同样不穿透，避免拖动中
//! mouseup 事件丢失。

#![cfg(windows)]

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::Ordering;

use serde_json::{json, Value};
use tauri::Manager;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, EnumChildWindows, GetClassNameW, IsChild, IsWindow, SetWindowLongPtrW,
    GWLP_WNDPROC, HTCLIENT, HTTRANSPARENT, WM_NCHITTEST,
};

use super::window::{CLICK_THROUGH_SUSPEND_COUNT, DRAG_OFFSET};

// ============ 类型定义 ============

/// WebView2 渲染子窗口的 WNDPROC 签名
type WndProcFn = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// 每个被安装子类的 HWND 的元数据
struct Entry {
    /// 所属 Tauri 父窗口 HWND（屏幕物理坐标空间，用于 GetWindowRect 计算中心矩形）
    parent_hwnd: isize,
    /// 所属角色窗口 label（用于反查 DRAG_OFFSET）
    label: String,
    /// 安装时保存的原始 WNDPROC（卸载时恢复）
    original_wndproc: isize,
    /// 被子类化的窗口类名（诊断用）
    class_name: String,
    /// 上次命中的 hit test 结果（用于状态切换日志，0=未知 1=HTCLIENT 2=HTTRANSPARENT）
    last_result: std::sync::atomic::AtomicI32,
}

/// 被子类化的 HWND → 安装元数据
static ENTRIES: Lazy<Mutex<HashMap<isize, Entry>>> = Lazy::new(|| Mutex::new(HashMap::new()));

// ============ 安装 / 卸载 ============

/// 为指定 Tauri 角色窗口安装点击穿透：
///
/// 1. 子类化顶层 Tauri 窗口（`parent.hwnd()`）
/// 2. 递归枚举所有后代 HWND，子类化类名包含 `render` / `widget` / `d3d` / `intermediate` 的窗口
/// 3. 新 WNDPROC 在 `WM_NCHITTEST` 时按"中心 1/3 宽 × 4/9 高"判定返回
///    `HTCLIENT` / `HTTRANSPARENT`，其余消息走 `CallWindowProcW` 转发给原处理函数
///
/// 幂等：同一父窗口重复安装会被静默忽略。
///
/// 前端一般在 `Live2DCanvas` 模型加载完成后（与 `start_cursor_tracking` 同一时机）调用。
pub fn install(parent: &tauri::WebviewWindow, label: &str) -> Result<(), String> {
    let parent_hwnd_raw = parent.hwnd().map_err(|e| e.to_string())?.0;
    let parent_hwnd = parent_hwnd_raw as isize;

    // 幂等：同一父窗口重复安装
    let already_installed = ENTRIES
        .lock()
        .values()
        .any(|e| e.parent_hwnd == parent_hwnd);
    if already_installed {
        return Ok(());
    }

    // 收集所有要子类化的 HWND：顶层窗口 + 所有相关后代
    let mut targets: Vec<(HWND, String)> = Vec::new();

    // 1. 顶层 Tauri 窗口本身（HTTRANSPARENT 在顶层才能穿透到桌面）
    let top_class = get_class_name(HWND(parent_hwnd_raw));
    targets.push((HWND(parent_hwnd_raw), top_class.clone()));

    // 2. 所有相关后代 HWND
    let descendants = find_all_candidate_hwnds(HWND(parent_hwnd_raw));
    targets.extend(descendants);

    tracing::info!(
        "[click_through] install 开始: label={}, parent_hwnd={:#x} (class={}), 候选 HWND 数量={}",
        label, parent_hwnd, top_class, targets.len()
    );

    let subclass_fn: WndProcFn = click_through_subclass;
    let mut installed_count = 0u32;

    for (hwnd, class_name) in &targets {
        let wv = hwnd.0 as isize;

        // 跳过已子类化的（避免重复子类化同一 HWND）
        let dup = ENTRIES.lock().contains_key(&wv);
        if dup {
            continue;
        }

        // 验证 HWND 仍然有效
        if !unsafe { IsWindow(Some(*hwnd)) }.as_bool() {
            tracing::warn!(
                "[click_through] 跳过无效 HWND: {:#x} (class={})",
                wv,
                class_name
            );
            continue;
        }

        let prev = unsafe {
            SetWindowLongPtrW(*hwnd, GWLP_WNDPROC, subclass_fn as isize)
        };

        {
            let mut g = ENTRIES.lock();
            g.insert(
                wv,
                Entry {
                    parent_hwnd,
                    label: label.to_string(),
                    original_wndproc: prev,
                    class_name: class_name.clone(),
                    last_result: std::sync::atomic::AtomicI32::new(0),
                },
            );
        }
        installed_count += 1;
        tracing::info!(
            "[click_through] 已子类化: hwnd={:#x}, class={}, prev_wndproc={:#x}",
            wv,
            class_name,
            prev
        );
    }

    if installed_count == 0 {
        return Err(format!(
            "未能子类化任何 HWND (parent_hwnd={:#x})",
            parent_hwnd
        ));
    }

    tracing::info!(
        "[click_through] install 完成: label={}, 共子类化 {} 个 HWND",
        label,
        installed_count
    );
    Ok(())
}

/// 卸载指定 Tauri 窗口的点击穿透子类化，恢复所有相关 HWND 的原始 WNDPROC
pub fn remove(parent: &tauri::WebviewWindow) {
    let parent_hwnd_raw = match parent.hwnd() {
        Ok(h) => h.0,
        Err(_) => return,
    };
    let parent_hwnd = parent_hwnd_raw as isize;

    // 收集所有属于该 parent 的子类化 HWND
    let to_remove: Vec<isize> = ENTRIES
        .lock()
        .iter()
        .filter(|(_, e)| e.parent_hwnd == parent_hwnd)
        .map(|(&k, _)| k)
        .collect();

    for wv in to_remove {
        let prev = ENTRIES
            .lock()
            .remove(&wv)
            .map(|e| e.original_wndproc)
            .unwrap_or(0);
        if prev != 0 {
            unsafe {
                SetWindowLongPtrW(HWND(wv as *mut c_void), GWLP_WNDPROC, prev);
            }
        }
        tracing::info!("[click_through] 已卸载子类化: hwnd={:#x}", wv);
    }
}

// ============ 子类回调 ============

/// WebView2 渲染子窗口的 WNDPROC 子类化回调
///
/// 仅处理 `WM_NCHITTEST`：
/// - 拖动中（`DRAG_OFFSET` 中存在本窗口 label）→ `HTCLIENT`
/// - `CLICK_THROUGH_SUSPEND_COUNT > 0`（右键菜单/气泡/Toast/InputDialog 显示中）→ `HTCLIENT`
/// - 光标在中心 1/3 宽 × 4/9 高矩形内 → `HTCLIENT`
/// - 其他情况 → `HTTRANSPARENT`（事件穿透到下层窗口）
///
/// 其余消息一律转发给原始 WNDPROC，不修改 WebView2 的其他行为。
///
/// 日志策略：仅在 hit test 结果发生切换时打印，避免高频刷屏。
unsafe extern "system" fn click_through_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCHITTEST {
        // try_lock 避免重入死锁
        let entry_snapshot: Option<(isize, String, isize)> = ENTRIES
            .try_lock()
            .and_then(|g| {
                g.get(&(hwnd.0 as isize))
                    .map(|e| (e.parent_hwnd, e.label.clone(), hwnd.0 as isize))
            });

        let (parent_hwnd, label, _self_hwnd) = match entry_snapshot {
            Some(v) => v,
            // 元数据丢失或锁获取失败：保守返回 HTCLIENT（不穿透），避免锁死鼠标
            None => return LRESULT(HTCLIENT as isize),
        };

        // 拖动期间：全窗口响应鼠标
        let drag_active = DRAG_OFFSET
            .try_lock()
            .map_or(true, |g| g.contains_key(&label));
        if drag_active {
            log_hit_test_transition(hwnd, HTCLIENT as i32, "drag_active");
            return LRESULT(HTCLIENT as isize);
        }

        // 前端子窗口（右键菜单、Toast、气泡、InputDialog）显示期间：全窗口响应
        if CLICK_THROUGH_SUSPEND_COUNT.load(Ordering::SeqCst) > 0 {
            log_hit_test_transition(hwnd, HTCLIENT as i32, "suspend>0");
            return LRESULT(HTCLIENT as isize);
        }

        if is_cursor_in_center_rect(HWND(parent_hwnd as *mut c_void)) {
            log_hit_test_transition(hwnd, HTCLIENT as i32, "in_center_rect");
            return LRESULT(HTCLIENT as isize);
        }
        log_hit_test_transition(hwnd, HTTRANSPARENT as i32, "outside_center_rect");
        return LRESULT(HTTRANSPARENT as isize);
    }

    // 其他消息：转发给原始 WNDPROC
    // try_lock 避免重入死锁
    let prev = ENTRIES
        .try_lock()
        .and_then(|g| g.get(&(hwnd.0 as isize)).map(|e| e.original_wndproc))
        .unwrap_or(0);
    if prev == 0 {
        return LRESULT(0);
    }
    // isize → WNDPROC fn pointer
    let prev_fn: WndProcFn = unsafe { std::mem::transmute(prev) };
    unsafe { CallWindowProcW(Some(prev_fn), hwnd, msg, wparam, lparam) }
}

/// 仅在 hit test 结果发生切换时打印日志，避免高频刷屏
///
/// try_lock 避免重入死锁，失败时跳过日志
fn log_hit_test_transition(hwnd: HWND, current: i32, reason: &str) {
    let key = hwnd.0 as isize;
    let encoded = if current == HTCLIENT as i32 { 1 } else { 2 };
    if let Some(g) = ENTRIES.try_lock() {
        if let Some(entry) = g.get(&key) {
            let prev = entry.last_result.load(Ordering::Relaxed);
            if prev != encoded {
                entry.last_result.store(encoded, Ordering::Relaxed);
                let result_str = if encoded == 1 { "HTCLIENT" } else { "HTTRANSPARENT" };
                tracing::debug!(
                    "[click_through] hit test 切换: hwnd={:#x} (class={}) → {} ({})",
                    key,
                    entry.class_name,
                    result_str,
                    reason
                );
            }
        }
    }
}

// ============ 判定辅助 ============

/// 判断光标当前是否位于 Tauri 父窗口的"中心 1/3 宽 × 4/9 高"矩形内
///
/// 坐标系：屏幕物理像素（与 Tauri `outer_position` / `outer_size` 一致）
fn is_cursor_in_center_rect(parent: HWND) -> bool {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(lp_point: *mut Point) -> i32;
        fn GetWindowRect(hwnd: HWND, lp_rect: *mut Rect) -> i32;
    }

    let mut cursor = Point { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return false;
    }
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(parent, &mut rect) } == 0 {
        return false;
    }

    let win_x = rect.left;
    let win_y = rect.top;
    let win_w = rect.right - rect.left;
    let win_h = rect.bottom - rect.top;
    if win_w <= 0 || win_h <= 0 {
        return false;
    }

    // 中心 1/3 宽 × 4/9 高
    let iw = win_w / 3;
    let ih = (win_h * 4) / 9;
    let ox = (win_w - iw) / 2;
    let oy = (win_h - ih) / 2;
    let left = win_x + ox;
    let top = win_y + oy;
    let right = left + iw;
    let bottom = top + ih;

    cursor.x >= left
        && cursor.x <= right
        && cursor.y >= top
        && cursor.y <= bottom
}

/// 获取窗口类名（诊断用）
fn get_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len > 0 {
        String::from_utf16_lossy(&buf[..usize::try_from(len).unwrap_or(0) as usize])
    } else {
        "<unknown>".to_string()
    }
}

/// 递归枚举 Tauri 父窗口下的所有后代 HWND，返回所有类名包含
/// `render` / `widget` / `d3d` / `intermediate` 的候选（含 HWND 和类名）
///
/// 这些关键字覆盖了 WebView2 的所有典型 HWND 类名变体：
/// - `Chrome_RenderWidgetHostHWND`（render + widget）
/// - `Chrome_WidgetWin_1`（widget）
/// - `Intermediate D3D Window`（intermediate + d3d）
/// - 任何未来 WebView2 版本的渲染表面类名变体
fn find_all_candidate_hwnds(parent: HWND) -> Vec<(HWND, String)> {
    struct Ctx {
        parent: HWND,
        /// 所有匹配的候选 HWND
        results: Vec<(HWND, String)>,
        /// 去重后的所有子类名（诊断日志用）
        all_classes: Vec<String>,
        /// 子孙 HWND 总数
        total_count: u32,
    }

    unsafe extern "system" fn enum_cb(
        hwnd: HWND,
        lparam: LPARAM,
    ) -> windows::core::BOOL {
        let ctx = unsafe { &mut *(lparam.0 as *mut Ctx) };

        if unsafe { IsChild(ctx.parent, hwnd) }.as_bool() {
            ctx.total_count += 1;
            let class = get_class_name(hwnd);
            let class_lower = class.to_lowercase();

            // 匹配 WebView2 渲染表面相关类名
            if class_lower.contains("render")
                || class_lower.contains("widget")
                || class_lower.contains("d3d")
                || class_lower.contains("intermediate")
            {
                ctx.results.push((hwnd, class.clone()));
            }

            if !ctx.all_classes.contains(&class) {
                ctx.all_classes.push(class);
            }
        }
        windows::core::BOOL(1)
    }

    let mut ctx = Ctx {
        parent,
        results: Vec::new(),
        all_classes: Vec::new(),
        total_count: 0,
    };
    unsafe {
        let _ = EnumChildWindows(
            Some(parent),
            Some(enum_cb),
            LPARAM(&mut ctx as *mut Ctx as isize),
        );
    }

    tracing::info!(
        "[click_through] 枚举后代 HWND: parent={:#x}, 共 {} 个子孙, 去重类名={:?}, 候选={:?}",
        parent.0 as isize,
        ctx.total_count,
        ctx.all_classes,
        ctx.results.iter().map(|(_, c)| c.as_str()).collect::<Vec<_>>()
    );

    ctx.results
}

// ============ 诊断 ============

/// 返回点击穿透的当前诊断状态（供前端调用排查）
///
/// 返回 JSON：
/// ```json
/// {
///   "suspend_count": 0,
///   "entries": [
///     {
///       "hwnd": "0x1234",
///       "parent_hwnd": "0x5678",
///       "label": "vivian",
///       "class_name": "Chrome_WidgetWin_1",
///       "original_wndproc": "0x9abc",
///       "is_window_valid": true
///     }
///   ],
///   "characters": [
///     {
///       "label": "vivian",
///       "installed": true,
///       "entry_count": 3,
///       "cursor_in_center_rect": true
///     }
///   ]
/// }
/// ```
pub fn get_status(
    app: &tauri::AppHandle,
    state: &std::sync::Arc<crate::state::AppState>,
) -> Value {
    let suspend_count = CLICK_THROUGH_SUSPEND_COUNT.load(Ordering::SeqCst);

    let entries: Vec<Value> = ENTRIES
        .lock()
        .iter()
        .map(|(&hwnd, e)| {
            let hwnd_valid = unsafe { IsWindow(Some(HWND(hwnd as *mut c_void))) }.as_bool();
            json!({
                "hwnd": format!("0x{:x}", hwnd),
                "parent_hwnd": format!("0x{:x}", e.parent_hwnd),
                "label": e.label,
                "class_name": e.class_name,
                "original_wndproc": format!("0x{:x}", e.original_wndproc),
                "is_window_valid": hwnd_valid,
            })
        })
        .collect();

    // 按角色分组统计
    let chars = state.characters.read();
    let characters: Vec<Value> = chars
        .values()
        .map(|c| {
            let label = c.id.clone();
            let (installed, entry_count) = {
                let g = ENTRIES.lock();
                let matches: Vec<&Entry> =
                    g.values().filter(|e| e.label == label).collect();
                (matches.len() > 0, matches.len())
            };

            // 检查光标是否在该角色窗口的中心矩形内
            let cursor_in_center = app
                .get_webview_window(&label)
                .and_then(|win| win.hwnd().ok())
                .map(|h| is_cursor_in_center_rect(HWND(h.0)))
                .unwrap_or(false);

            json!({
                "label": label,
                "online": *c.online.read(),
                "installed": installed,
                "entry_count": entry_count,
                "cursor_in_center_rect": cursor_in_center,
            })
        })
        .collect();

    json!({
        "suspend_count": suspend_count,
        "total_entries": entries.len(),
        "entries": entries,
        "characters": characters,
    })
}
