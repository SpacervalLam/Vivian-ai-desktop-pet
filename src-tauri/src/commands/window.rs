//! 窗口命令 - 窗口位置、尺寸、透明度、可见性、置顶与多窗口管理
//!
//! 已有的三个命令（`set_window_position` / `get_window_position` / `toggle_always_on_top`）
//! 签名保持不变，新增命令为增量补全。

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::utils::fnv1a_64_bytes;

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ============ WebView 冻结（隐藏窗口渲染进程挂起省内存） ============
//
// chat / side_chat 隐藏期间用 WebView2 TrySuspend 冻结渲染进程：JS 暂停执行、
// 渲染停止，进程工作集可被系统回收。三态交互（边缘检测/单击展开/收回动画）
// 全部由本文件的原生线程承担，WebView 冻结不影响功能；仅隐藏期间发往前端的
// 事件投递会丢失，由前端在 visibilitychange（恢复+show 触发）时刷新消息兜底。
//
// 时序契约：suspend 在 hide() 之后调用（即发即忘），resume 在 show() 之前
// 调用（阻塞等待，~100-300ms，被 220ms 滑入动画与用户反应时间掩盖）。
// 代计数器防止快速 hide→show 时迟到的 TrySuspend 冻结已重新显示的窗口。

/// 冻结请求代号：resume 使未执行的 suspend 请求失效
static WEBVIEW_FREEZE_GEN: AtomicU32 = AtomicU32::new(0);

/// 冻结窗口 WebView（窗口隐藏后调用）。非 Windows 平台为空操作。
/// with_webview 与窗口操作同走主线程 FIFO 队列，后续 thaw_webview 天然排在本次冻结之后。
pub(crate) fn freeze_webview(win: &WebviewWindow) {
    #[cfg(windows)]
    {
        // 已重新可见（快速 hide→show 竞态）：跳过冻结
        if win.is_visible().ok().unwrap_or(true) {
            return;
        }
        let gen = WEBVIEW_FREEZE_GEN.fetch_add(1, Ordering::SeqCst);
        let _ = win.with_webview(move |wv| unsafe {
            // 迟到的冻结请求被后续 thaw 取代：跳过
            if WEBVIEW_FREEZE_GEN.load(Ordering::SeqCst) != gen + 1 {
                return;
            }
            webview_freeze_op(&wv, true);
        });
    }
    #[cfg(not(windows))]
    let _ = win;
}

/// 恢复窗口 WebView（窗口 show 之前调用）。非 Windows 平台为空操作。
/// 提交的 Resume 在主线程队列中先于随后调用的 show() 执行，渲染就绪后才显示窗口。
pub(crate) fn thaw_webview(win: &WebviewWindow) {
    #[cfg(windows)]
    {
        // 使所有排队中的冻结请求失效
        WEBVIEW_FREEZE_GEN.fetch_add(1, Ordering::SeqCst);
        let _ = win.with_webview(|wv| unsafe {
            webview_freeze_op(&wv, false);
        });
    }
    #[cfg(not(windows))]
    let _ = win;
}

/// WebView2 挂起/恢复的原生操作（仅 Windows）
#[cfg(windows)]
unsafe fn webview_freeze_op(
    wv: &tauri::webview::PlatformWebview,
    freeze: bool,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use webview2_com::TrySuspendCompletedHandler;
    use windows_core::Interface;

    let controller = wv.controller();
    let Ok(core) = controller.CoreWebView2() else {
        return;
    };
    let Ok(wv3) = core.cast::<ICoreWebView2_3>() else {
        tracing::debug!("[webview_freezer] ICoreWebView2_3 不支持，跳过");
        return;
    };
    // TrySuspend 完成回调忽略结果（失败仅意味着内存照旧，无功能影响）
    let result = if freeze {
        wv3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(
            |_error, _success| Ok(()),
        )))
    } else {
        wv3.Resume()
    };
    if let Err(e) = result {
        tracing::debug!("[webview_freezer] {:?} 失败: {e}", if freeze { "suspend" } else { "resume" });
    }
}

// ============ 全屏光标追踪（绕过 WebView2 timer / IPC 节流） ============
// WebView2 在窗口失去焦点/不可见时会节流 setInterval、requestAnimationFrame，
// 同时 Tauri 的 emit+listen 基于 WebView2 的 IPC 桥接，窗口失焦时也会被节流，
// 导致鼠标移出窗口后前端收不到事件、Ticker 也不更新。
// 改用 Rust 原生线程定时获取光标位置，通过 WebviewWindow::eval() 直接注入 JS 执行，
// 同时在前端手动驱动 PIXI Ticker.update()，绕过 RAF 节流，确保窗口外鼠标跟随始终生效。

/// 按角色隔离的光标追踪线程：character_id → (停止标志, 线程句柄)
/// 每个角色窗口拥有独立的追踪线程，互不干扰
static CURSOR_TRACKING_THREADS: Lazy<Mutex<std::collections::HashMap<String, (Arc<AtomicBool>, JoinHandle<()>)>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 应用正在退出的全局标志，光标追踪线程在循环顶部检查本标志并立即退出
pub(crate) static APP_EXITING: AtomicBool = AtomicBool::new(false);

// ============ 自定义窗口拖动 ============
//
// Tauri 的 startDragging() 底层调用 Win32 标准标题栏拖动机制
// (ReleaseCapture + SendMessage(WM_NCLBUTTONDOWN, HTCAPTION, ...))。
// Windows 会自动限制窗口顶部不能超出屏幕工作区——拖到顶部会被"弹回"。
// 对于 decorations:false 的无边框窗口这个限制依然存在。
//
// 解决方案：不使用 startDragging，而是在 cursor tracking 线程中直接用
// SetWindowPos 移动窗口。SetWindowPos 不受 Windows 工作区限制，
// 窗口可以被拖到任意位置（包括顶部超出屏幕边缘）。

/// 每窗口拖动偏移（key = window label）
pub(crate) static DRAG_OFFSET: Lazy<Mutex<std::collections::HashMap<String, (i32, i32)>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 查询左键当前物理按下状态（不依赖事件投递）
///
/// 用于拖动 watchdog：当窗口追逐延迟导致 mouseup 无法到达 WebView 时，
/// 前端无法感知拖动已结束，只有硬件状态能作为最终裁决。
#[cfg(windows)]
fn is_left_mouse_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 != 0 }
}

#[cfg(not(windows))]
fn is_left_mouse_button_down() -> bool {
    false
}

/// ESC 键当前是否按下（边缘检测线程轮询用，配合下降沿避免重复触发）
#[cfg(windows)]
fn is_escape_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
    unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000 != 0 }
}

#[cfg(not(windows))]
fn is_escape_down() -> bool {
    false
}

/// 启动指定角色的光标追踪线程（每 ~60ms 一帧）。
///
/// 每个角色窗口拥有独立的追踪线程，职责：
/// - 窗口拖动（DRAG_OFFSET 驱动 SetWindowPos）
/// - 点击穿透切换（中心 1/3 宽 × 4/9 高矩形外穿透）
/// - 向本窗口推送全局光标坐标（`cursor:position` 事件），
///   前端据此实现跨窗口鼠标跟随，不受点击穿透影响
#[tauri::command]
pub fn start_cursor_tracking(
    app: AppHandle,
    character_id: Option<String>,
    state: State<'_, std::sync::Arc<crate::state::AppState>>,
) -> Result<(), String> {
    let char_id = character_id
        .map(String::from)
        .unwrap_or_else(|| state.active_character_id.read().clone());

    // 若该角色已有线程在运行，不重复启动
    {
        let threads = CURSOR_TRACKING_THREADS.lock();
        if threads.contains_key(&char_id) {
            return Ok(());
        }
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    let app_clone = app.clone();
    let char_id_for_thread = char_id.clone();
    let stop_flag_clone = Arc::clone(&stop_flag);

    let thread = thread::spawn(move || {
        tracing::info!("[cursor_tracking] 线程启动: {char_id_for_thread}");

        // 安装点击穿透子类化（基于 WebView2 子 HWND 的 WM_NCHITTEST）。
        // 幂等：同一父窗口重复安装会被静默忽略。
        // 注意：子类化对顶层 Tauri Window 有效，但对深层 WebView2 渲染子窗口
        // （Chrome_RenderWidgetHostHWND 等）因跨进程限制会失败（prev_wndproc=0x0）。
        // 因此真正的穿透切换由本线程的 set_ignore_cursor_events 动态调用实现，
        // 子类化仅作为补充保障（例如拖动期间通过 DRAG_OFFSET 判定）。
        if let Some(win) = app_clone.get_webview_window(&char_id_for_thread) {
            if let Err(e) =
                super::click_through::install(&win, &char_id_for_thread)
            {
                tracing::warn!(
                    "[cursor_tracking] 安装点击穿透子类化失败: {} (角色 {})",
                    e,
                    char_id_for_thread
                );
            }
        }

        // 上一帧光标坐标，用于跳过未变化帧的 emit
        let mut last_cursor: (i32, i32) = (i32::MIN, i32::MIN);
        // 拖拽期间的光标轨迹采样（时刻, x, y），松手时计算惯性甩飞初速度
        let mut drag_samples: std::collections::VecDeque<(std::time::Instant, f64, f64)> =
            std::collections::VecDeque::new();
        // 上一帧是否处于拖拽状态，用于检测拖拽结束的瞬间
        let mut prev_is_dragging = false;
        // 上一次的穿透状态，避免频繁调用 set_ignore_cursor_events
        let mut last_ignore_state: Option<bool> = None;
        // 上一帧窗口是否可见，用于检测"从隐藏到显示"的转换
        let mut last_window_visible = true;
        // 定期强制重应用穿透状态的计数器（每 ~2 秒一次），防止 Tauri/WebView2
        // 在焦点切换、窗口重绘、样式重置等事件后丢失 WS_EX_TRANSPARENT 状态
        let mut force_reapply_tick: u32 = 0;
        const FORCE_REAPPLY_INTERVAL: u32 = 33; // 33 * 60ms ≈ 2s

        while !stop_flag_clone.load(Ordering::SeqCst) && !APP_EXITING.load(Ordering::SeqCst) {
            // 获取本角色窗口
            let win = match app_clone.get_webview_window(&char_id_for_thread) {
                Some(w) => w,
                None => {
                    // 窗口已关闭，退出线程
                    tracing::info!("[cursor_tracking] 窗口已关闭，线程退出: {char_id_for_thread}");
                    break;
                }
            };

            // 窗口不可见时跳过（隐藏/睡眠）
            let window_visible = win.is_visible().ok().unwrap_or(false);
            if !window_visible {
                // 隐藏期间的 mouseup 到不了 WebView，正常停止拖动的链路断开；
                // 清掉本角色的拖动状态，避免 DRAG_OFFSET 残留导致 is_dragging
                // 恒为 true、窗口恢复后全窗口响应鼠标（穿透永久失效）
                if DRAG_OFFSET.lock().remove(&char_id_for_thread).is_some() {
                    tracing::info!(
                        "[cursor_tracking] 窗口隐藏，清除残留拖动状态: {char_id_for_thread}"
                    );
                }
                // 拖拽会话随隐藏终止：丢弃轨迹采样，避免恢复显示后用陈旧速度触发甩飞
                drag_samples.clear();
                prev_is_dragging = false;
                last_window_visible = false;
                thread::sleep(Duration::from_millis(60));
                continue;
            }

            // 窗口刚从隐藏变为可见：重置穿透状态缓存，强制下一帧重新应用
            // 防止 hide/show 后 WS_EX_TRANSPARENT 样式丢失但 last_ignore_state 未变
            if !last_window_visible {
                last_window_visible = true;
                last_ignore_state = None;
                tracing::info!(
                    "[cursor_tracking] 窗口从隐藏恢复，重置穿透状态缓存: {char_id_for_thread}"
                );
            }

            // 获取光标位置
            let c = match app_clone.cursor_position() {
                Ok(c) => c,
                Err(_) => {
                    thread::sleep(Duration::from_millis(60));
                    continue;
                }
            };

            let cursor_moved = (c.x as i32) != last_cursor.0 || (c.y as i32) != last_cursor.1;
            let label = char_id_for_thread.clone();

            let mut is_dragging = DRAG_OFFSET
                .lock()
                .get(&label)
                .cloned()
                .is_some();

            // === 拖动 watchdog ===
            // 前端 mouseup 监听绑在 WebView 的 window 上；当窗口追逐延迟导致
            // 松手瞬间鼠标恰好不在窗口内时，mouseup 到不了 WebView，
            // stop_window_drag 永远不会被调用，DRAG_OFFSET 残留 → is_dragging 恒真。
            // 直接轮询硬件按键状态作为最终裁决：按键已抬起且拖动状态仍在 → 主动清掉。
            if is_dragging && !is_left_mouse_button_down() {
                if DRAG_OFFSET.lock().remove(&label).is_some() {
                    tracing::info!(
                        "[cursor_tracking] 左键已抬起但拖动状态残留（mouseup 丢失），强制清除: {label}"
                    );
                    // 通知前端重置拖动会话状态（dragSessionRef、拖拽表情）
                    let _ = win.emit("drag:cancelled", json!({}));
                }
                is_dragging = false;
            }

            // === 点击穿透切换 ===
            // 决定是否应该让窗口忽略鼠标事件（穿透到下层窗口）：
            // - 拖动中：不穿透（全窗口响应，避免 mouseup 丢失）
            // - suspend_count > 0：不穿透（右键菜单/Toast/气泡/InputDialog 显示期间）
            // - 光标在中心 1/3 宽 × 4/9 高矩形内：不穿透（响应交互）
            // - 左键按下：不穿透（可能是文件拖放，需让 OLE 拖放能检测到窗口；
            //   子类化 WM_NCHITTEST 仍按中心矩形判定，外围区域照样 HTTRANSPARENT 穿透）
            // - 其他：穿透（外围区域事件传递到桌面/其他应用）
            let suspend_active = CLICK_THROUGH_SUSPEND_COUNT.load(Ordering::SeqCst) > 0;

            // 左键按下时可能是从外部拖入文件，此时不能 set_ignore_cursor_events(true)，
            // 否则 WS_EX_TRANSPARENT 会让 OLE 拖放跳过整个窗口（包括中心矩形），
            // onDragDropEvent 的 enter 永远不触发。子类化仍保证外围区域穿透。
            let possible_file_drop = is_left_mouse_button_down();

            let in_center_rect = if let (Ok(pos), Ok(size)) =
                (win.outer_position(), win.outer_size())
            {
                let win_w = size.width as i32;
                let win_h = size.height as i32;
                if win_w <= 0 || win_h <= 0 {
                    true // 异常情况保守返回 true（不穿透）
                } else {
                    // 中心 1/3 宽 × 4/9 高
                    let iw = win_w / 3;
                    let ih = (win_h * 4) / 9;
                    let ox = (win_w - iw) / 2;
                    let oy = (win_h - ih) / 2;
                    let left = pos.x + ox;
                    let top = pos.y + oy;
                    let right = left + iw;
                    let bottom = top + ih;
                    (c.x as i32) >= left
                        && (c.x as i32) <= right
                        && (c.y as i32) >= top
                        && (c.y as i32) <= bottom
                }
            } else {
                true // 获取窗口矩形失败时保守不穿透
            };

            let should_ignore = !is_dragging && !suspend_active && !in_center_rect && !possible_file_drop;

            // 定期强制重应用：每 ~2s 无条件调用一次 set_ignore_cursor_events，
            // 防止 Tauri/WebView2/Windows 在焦点切换、窗口重绘等事件后重置 WS_EX_TRANSPARENT。
            force_reapply_tick += 1;
            let force_reapply = force_reapply_tick >= FORCE_REAPPLY_INTERVAL;
            if force_reapply {
                force_reapply_tick = 0;
            }

            // 状态变化时立即切换；定期强制重应用作为兜底
            if last_ignore_state != Some(should_ignore) || force_reapply {
                if let Err(e) = win.set_ignore_cursor_events(should_ignore) {
                    tracing::warn!(
                        "[cursor_tracking] set_ignore_cursor_events({}) 失败: {} (角色 {})",
                        should_ignore,
                        e,
                        char_id_for_thread
                    );
                } else {
                    if last_ignore_state != Some(should_ignore) {
                        tracing::info!(
                            "[cursor_tracking] 穿透状态切换: {} → {} (drag={}, suspend={}, in_center={}, 角色 {})",
                            last_ignore_state.map(|b| if b { "穿透" } else { "响应" }).unwrap_or("初始"),
                            if should_ignore { "穿透" } else { "响应" },
                            is_dragging,
                            suspend_active,
                            in_center_rect,
                            char_id_for_thread
                        );
                    }
                    last_ignore_state = Some(should_ignore);
                }
            }

            if is_dragging {
                if let Some(offset) = DRAG_OFFSET.lock().get(&label).copied() {
                    let new_x = c.x as i32 - offset.0;
                    let new_y = c.y as i32 - offset.1;
                    move_window(&win, new_x, new_y);
                }
                // 光标轨迹采样（全局物理坐标，不受窗口追逐延迟影响），
                // 供松手瞬间计算惯性甩飞初速度
                let now = std::time::Instant::now();
                drag_samples.push_back((now, c.x, c.y));
                while drag_samples.len() > 1
                    && now.duration_since(drag_samples[0].0).as_millis() as u64
                        > FLING_SAMPLE_WINDOW_MS
                {
                    drag_samples.pop_front();
                }
            } else {
                // 光标坐标推送（前端据此实现鼠标跟随，不受穿透影响）
                if let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) {
                    if cursor_moved {
                        let _ = win.emit(
                            "cursor:position",
                            serde_json::json!({
                                "character_id": &label,
                                "cursor_x": c.x as i32,
                                "cursor_y": c.y as i32,
                                "window_x": pos.x,
                                "window_y": pos.y,
                                "window_w": size.width as i32,
                                "window_h": size.height as i32,
                            }),
                        );
                    }
                }
            }

            // 拖拽结束瞬间：按松手前的光标轨迹计算初速度，触发惯性甩飞。
            // 覆盖正常 mouseup（前端 stop_window_drag）和 watchdog 兜底两条路径。
            if prev_is_dragging && !is_dragging {
                let v = fling_velocity_from_samples(&drag_samples);
                drag_samples.clear();
                if let Some((fvx, fvy)) = v {
                    start_fling(win.clone(), &label, fvx, fvy);
                }
            }
            prev_is_dragging = is_dragging;

            if cursor_moved {
                last_cursor = (c.x as i32, c.y as i32);
            }

            thread::sleep(Duration::from_millis(60));
        }

        // 线程退出时从全局表移除自己
        CURSOR_TRACKING_THREADS.lock().remove(&char_id_for_thread);
        tracing::info!("[cursor_tracking] 线程已退出: {char_id_for_thread}");
    });

    let mut threads = CURSOR_TRACKING_THREADS.lock();
    threads.insert(char_id, (stop_flag, thread));
    Ok(())
}

/// 停止所有角色的光标追踪线程（内部实现）
///
/// 同时卸载点击穿透子类化（恢复 WebView2 渲染子窗口的原始 WNDPROC），
/// 避免窗口关闭/线程停止后仍持有我们的 WNDPROC 引用导致悬垂指针。
pub(crate) fn stop_cursor_tracking_internal(
    app: &AppHandle,
    state: &std::sync::Arc<crate::state::AppState>,
) {
    // 取出所有线程并设置停止标志
    let threads_to_join: Vec<(String, Arc<AtomicBool>, JoinHandle<()>)> = {
        let mut threads = CURSOR_TRACKING_THREADS.lock();
        threads
            .drain()
            .map(|(k, (flag, handle))| (k, flag, handle))
            .collect()
    };
    // 先设置全部停止标志，再统一等待退出
    for (_id, flag, _handle) in &threads_to_join {
        flag.store(true, Ordering::SeqCst);
    }
    if !threads_to_join.is_empty() {
        // join 放在辅助线程中执行并限时等待：退出期间事件循环停止泵送，
        // 个别线程可能卡在阻塞式窗口调用中无法及时退出，无限 join 会卡死调用方
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for (id, _flag, handle) in threads_to_join {
                let _ = handle.join();
                tracing::info!("[cursor_tracking] 已停止线程: {id}");
            }
            let _ = done_tx.send(());
        });
        if done_rx
            .recv_timeout(Duration::from_millis(1000))
            .is_err()
        {
            tracing::warn!("[cursor_tracking] 线程未在 1s 内退出，放弃等待");
        }
    }

    // 重置暂停计数器
    CLICK_THROUGH_SUSPEND_COUNT.store(0, Ordering::SeqCst);
    // 卸载所有角色窗口的点击穿透子类化，恢复 WebView2 原始 WNDPROC
    {
        let chars = state.characters.read();
        for c in chars.values() {
            if let Some(win) = app.get_webview_window(&c.id) {
                super::click_through::remove(&win);
                // 恢复窗口鼠标响应（避免线程停止后窗口卡在穿透状态）
                let _ = win.set_ignore_cursor_events(false);
            }
        }
    }
    DRAG_OFFSET.lock().clear();
    // 清空甩飞代号表：运行中的甩飞线程检测到代号丢失后自行退出
    FLING_GEN.lock().clear();
}

/// 是否还有任何在线角色窗口存在
///
/// 用于"窗口关闭后判定是否应当停止光标追踪线程"。
/// 只要还有一个 online 角色的窗口存在，全局追踪线程就必须继续运行。
pub(crate) fn any_online_character_window_exists(
    app: &AppHandle,
    state: &std::sync::Arc<crate::state::AppState>,
) -> bool {
    let chars = state.characters.read();
    chars.values().any(|c| {
        *c.online.read() && app.get_webview_window(&c.id).is_some()
    })
}

/// 当没有任何在线角色窗口时停止光标追踪线程
///
/// 供 lib.rs 的 `on_window_event`（CloseRequested）调用：
/// 单个角色窗口关闭不会停线程（其他角色窗口可能仍在线），
/// 仅当所有角色窗口都已关闭时才停止，避免误杀全局线程。
pub(crate) fn stop_cursor_tracking_if_no_windows(
    app: &AppHandle,
    state: &std::sync::Arc<crate::state::AppState>,
) {
    if !any_online_character_window_exists(app, state) {
        tracing::info!("[cursor_tracking] 所有角色窗口已关闭，停止光标追踪线程");
        stop_cursor_tracking_internal(app, state);
    }
}

/// 停止后台光标追踪线程（Tauri command 入口，前端调用）
#[tauri::command]
pub fn stop_cursor_tracking(
    app: AppHandle,
    state: State<'_, std::sync::Arc<crate::state::AppState>>,
) -> Result<(), String> {
    stop_cursor_tracking_internal(&app, state.inner());
    Ok(())
}

/// 用 SetWindowPos 移动窗口到指定位置，绕过 Windows 工作区限制
///
/// Tauri 的 set_position / startDragging 底层都会受 Windows 工作区限制：
/// 窗口顶部不能超出屏幕工作区顶部。SetWindowPos 直接调用 Win32 API，
/// 不受此限制，窗口可被移动到任意位置（包括顶部超出屏幕边缘）。
fn move_window(window: &tauri::WebviewWindow, x: i32, y: i32) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOCOPYBITS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        };

        let hwnd_tauri = match window.hwnd() {
            Ok(h) => h,
            Err(_) => return,
        };
        let hwnd = HWND(hwnd_tauri.0);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE | SWP_NOCOPYBITS,
            );
        }
    }

    #[cfg(not(windows))]
    {
        use tauri::PhysicalPosition;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
}

/// 启动自定义窗口拖动
///
/// 记录鼠标相对窗口左上角的偏移，cursor tracking 线程会在后续每帧用
/// SetWindowPos 移动窗口。绕过 Windows 工作区限制，窗口顶部可超出屏幕边缘。
/// 前端在 mouseup 时调用 stop_window_drag 结束拖动。
#[tauri::command]
pub fn start_window_drag(
    window: tauri::WebviewWindow,
    cursor_x: i32,
    cursor_y: i32,
) -> Result<(), String> {
    let win_pos = window.outer_position().map_err(err_str)?;
    let offset_x = cursor_x - win_pos.x;
    let offset_y = cursor_y - win_pos.y;
    let label = window.label().to_string();
    DRAG_OFFSET.lock().insert(label, (offset_x, offset_y));
    Ok(())
}

/// 停止自定义窗口拖动
#[tauri::command]
pub fn stop_window_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    let label = window.label().to_string();
    DRAG_OFFSET.lock().remove(&label);
    Ok(())
}

// ============ 拖拽物理：惯性甩飞 + 屏幕边缘碰撞回弹 ============
//
// 快速拖拽松手时，用松手前 ~120ms 的全局光标轨迹计算初速度，窗口带惯性滑行：
// - 指数摩擦衰减（空气阻力模型），速度低于阈值后自然停住
// - 碰撞边界不是窗口矩形，而是桌宠「身体」足迹：窗口中央 1/3 宽 × 4/9 高
//   （与点击穿透中心矩形同口径）。Live2D 模型主体只在该范围内渲染，
//   周围全是透明像素，因此窗口最多可滑出屏幕外 1/3 宽度 / 5/18 高度，
//   视觉上是角色本体撞到屏幕边缘被弹回
// - 碰撞法向速度乘回弹系数（restitution）损失能量，配合摩擦衰减，几次反弹后静止

/// 触发甩飞的最小释放速度（物理像素/ms，约 500px/s）
const FLING_MIN_VELOCITY: f64 = 0.5;
/// 甩飞初速度上限（物理像素/ms），防止极端甩动瞬间横穿整屏
const FLING_MAX_VELOCITY: f64 = 4.0;
/// 速度指数摩擦系数（每 ms）：v *= exp(-k·dt)，0.002 ≈ 350ms 半衰期
const FLING_FRICTION: f64 = 0.002;
/// 边缘碰撞的法向速度保留系数（能量损失后的反弹速度）
const FLING_RESTITUTION: f64 = 0.6;
/// 速度低于该值（物理像素/ms）视为静止，结束模拟
const FLING_STOP_VELOCITY: f64 = 0.06;
/// 物理帧间隔（ms）
const FLING_TICK_MS: u64 = 12;
/// 松手前速度采样窗口长度（ms）
const FLING_SAMPLE_WINDOW_MS: u64 = 120;
/// 采样跨度低于该值（ms）时速度不可信，不触发甩飞
const FLING_MIN_SAMPLE_SPAN_MS: f64 = 40.0;

/// 每窗口甩飞代号：新一轮甩飞开始时自增，被取代的旧线程检测到代号变化后自行退出
static FLING_GEN: Lazy<Mutex<std::collections::HashMap<String, u64>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 由松手前的光标采样轨迹计算甩飞初速度，速度过小 / 采样过短时返回 None
fn fling_velocity_from_samples(
    samples: &std::collections::VecDeque<(std::time::Instant, f64, f64)>,
) -> Option<(f64, f64)> {
    let first = *samples.front()?;
    let last = *samples.back()?;
    let span_ms = last.0.duration_since(first.0).as_secs_f64() * 1000.0;
    if span_ms < FLING_MIN_SAMPLE_SPAN_MS {
        return None;
    }
    let vx = (last.1 - first.1) / span_ms;
    let vy = (last.2 - first.2) / span_ms;
    let speed = vx.hypot(vy);
    if speed < FLING_MIN_VELOCITY {
        return None;
    }
    if speed > FLING_MAX_VELOCITY {
        let scale = FLING_MAX_VELOCITY / speed;
        return Some((vx * scale, vy * scale));
    }
    Some((vx, vy))
}

/// 启动惯性甩飞物理线程（Windows：以虚拟屏幕为碰撞边界）
///
/// 线程在下列任一情况下退出：
/// - 速度衰减到 FLING_STOP_VELOCITY 以下（自然停止）
/// - 窗口被重新抓起（DRAG_OFFSET 出现本窗口，可中途"接住"桌宠）
/// - 窗口隐藏 / 应用退出 / 代号被更新的甩飞取代
///
/// 每帧重读窗口实际位置作为积分基点，智能避让等外部移动不会被甩飞覆盖。
fn start_fling(win: WebviewWindow, label: &str, vx: f64, vy: f64) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };

        // 碰撞边界：虚拟屏幕（多显示器并集）物理坐标
        let (vs_x, vs_y, vs_w, vs_h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        if vs_w <= 0 || vs_h <= 0 {
            return;
        }

        // 身体足迹：窗口中央 1/3 宽 × 4/9 高（与点击穿透中心矩形同口径）
        let (win_w, win_h) = match win.outer_size() {
            Ok(s) => (s.width as i32, s.height as i32),
            Err(_) => return,
        };
        if win_w <= 0 || win_h <= 0 {
            return;
        }
        let body_w = win_w / 3;
        let body_h = (win_h * 4) / 9;
        let body_l = (win_w - body_w) / 2;
        let body_t = (win_h - body_h) / 2;

        // 窗口可移动范围：保证身体足迹始终留在虚拟屏幕内，
        // 全透明边缘最多探出屏幕外 (win_w - body_w)/2 / (win_h - body_h)/2
        let min_x = (vs_x - body_l) as f64;
        let max_x = ((vs_x + vs_w - body_l - body_w).max(vs_x - body_l)) as f64;
        let min_y = (vs_y - body_t) as f64;
        let max_y = ((vs_y + vs_h - body_t - body_h).max(vs_y - body_t)) as f64;

        let label = label.to_string();
        let gen = {
            let mut gens = FLING_GEN.lock();
            let g = gens.entry(label.clone()).or_insert(0);
            *g += 1;
            *g
        };
        tracing::info!("[fling] 甩飞开始: {label} v=({vx:.2}, {vy:.2}) px/ms");

        let _ = thread::Builder::new()
            .name(format!("fling-{label}"))
            .spawn(move || {
                let mut vx = vx;
                let mut vy = vy;
                let mut last = std::time::Instant::now();
                loop {
                    thread::sleep(Duration::from_millis(FLING_TICK_MS));
                    if APP_EXITING.load(Ordering::SeqCst) {
                        break;
                    }
                    // 被新一轮甩飞取代
                    if FLING_GEN.lock().get(&label).copied().unwrap_or(0) != gen {
                        break;
                    }
                    // 被重新抓起：立即让位给用户拖动
                    if DRAG_OFFSET.lock().contains_key(&label) {
                        break;
                    }
                    if !win.is_visible().ok().unwrap_or(false) {
                        break;
                    }

                    let now = std::time::Instant::now();
                    let dt_ms = now.duration_since(last).as_secs_f64() * 1000.0;
                    last = now;
                    if dt_ms <= 0.0 {
                        continue;
                    }

                    let (mut x, mut y) = match win.outer_position() {
                        Ok(p) => (p.x as f64, p.y as f64),
                        Err(_) => break,
                    };
                    x += vx * dt_ms;
                    y += vy * dt_ms;

                    // 边缘碰撞：位置夹紧 + 法向速度反弹
                    if x < min_x {
                        x = min_x;
                        if vx < 0.0 {
                            vx = -vx * FLING_RESTITUTION;
                        }
                    } else if x > max_x {
                        x = max_x;
                        if vx > 0.0 {
                            vx = -vx * FLING_RESTITUTION;
                        }
                    }
                    if y < min_y {
                        y = min_y;
                        if vy < 0.0 {
                            vy = -vy * FLING_RESTITUTION;
                        }
                    } else if y > max_y {
                        y = max_y;
                        if vy > 0.0 {
                            vy = -vy * FLING_RESTITUTION;
                        }
                    }

                    // 指数摩擦（空气阻力）
                    let damp = (-FLING_FRICTION * dt_ms).exp();
                    vx *= damp;
                    vy *= damp;

                    move_window(&win, x.round() as i32, y.round() as i32);

                    if vx.hypot(vy) < FLING_STOP_VELOCITY {
                        break;
                    }
                }
                tracing::debug!("[fling] 甩飞结束: {label}");
            });
    }

    #[cfg(not(windows))]
    {
        let _ = (win, label, vx, vy);
    }
}

/// 通过标签获取指定 webview 窗口；找不到时返回错误字符串
fn window_by_label(app: &AppHandle, label: &str) -> Result<WebviewWindow, String> {
    app.get_webview_window(label)
        .ok_or_else(|| format!("窗口 '{label}' 不存在"))
}

/// 设置窗口位置
#[tauri::command]
pub fn set_window_position(window: tauri::WebviewWindow, x: i32, y: i32) -> Result<(), String> {
    use tauri::PhysicalPosition;
    window
        .set_position(PhysicalPosition::new(x, y))
        .map_err(err_str)
}

/// 获取窗口位置
#[tauri::command]
pub fn get_window_position(window: tauri::WebviewWindow) -> Result<Value, String> {
    let pos = window.outer_position().map_err(err_str)?;
    Ok(json!({
        "x": pos.x,
        "y": pos.y,
    }))
}

/// 获取全局鼠标位置（屏幕物理坐标，用于窗口外鼠标跟随）
#[tauri::command]
pub fn get_cursor_position(app: AppHandle) -> Result<Value, String> {
    let pos = app.cursor_position().map_err(err_str)?;
    Ok(json!({
        "x": pos.x,
        "y": pos.y,
    }))
}

/// 切换窗口置顶状态
#[tauri::command]
pub fn toggle_always_on_top(window: tauri::WebviewWindow) -> Result<(), String> {
    let current = window.is_always_on_top().map_err(err_str)?;
    window
        .set_always_on_top(!current)
        .map_err(err_str)?;
    tracing::info!("窗口置顶状态切换为: {}", !current);
    Ok(())
}

/// 设置窗口尺寸
#[tauri::command]
pub fn set_window_size(
    window: tauri::WebviewWindow,
    width: u32,
    height: u32,
) -> Result<(), String> {
    use tauri::PhysicalSize;
    window
        .set_size(PhysicalSize::new(width, height))
        .map_err(err_str)
}

/// 同时设置窗口位置和尺寸（物理像素），绕过 resizable 限制
///
/// 用于 Ctrl+滚轮缩放：resizable=false 禁用了 Aero Snap，
/// 但 Tauri 的 set_size 可能拒绝调整不可拉伸窗口的大小，
/// 因此直接调用 Win32 SetWindowPos 确保缩放生效。
///
/// 防闪烁策略：
/// - 使用 SWP_NOREDRAW 延迟重绘，避免 SetWindowPos 触发 DWM 立即更新
///   窗口几何而 WebView2 canvas 纹理还是旧尺寸的中间态闪烁
/// - resize 完成后立即调用 RedrawWindow 强制同步重绘，
///   让几何更新与纹理更新落在同一帧内
/// - 不使用 SWP_NOCOPYBITS（会让新区域直接清空，透明窗口表现为空白闪烁）
#[tauri::command]
pub fn set_window_rect(
    window: tauri::WebviewWindow,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::Graphics::Gdi::{RedrawWindow, RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW};
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOCOPYBITS, SWP_NOACTIVATE, SWP_NOREDRAW,
            SWP_NOZORDER,
        };

        let hwnd_tauri = window.hwnd().map_err(err_str)?;
        let hwnd = HWND(hwnd_tauri.0);
        unsafe {
            // SWP_NOREDRAW：延迟重绘，避免中间态被显示
            // SWP_NOCOPYBITS：保留（不复制旧客户区位图，但对透明窗口影响小）
            SetWindowPos(
                hwnd,
                None,
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOCOPYBITS | SWP_NOREDRAW,
            )
            .map_err(|e| e.to_string())?;

            // 立即同步重绘：强制窗口及所有子窗口（WebView2）在同一帧内重绘
            // RDW_INVALIDATE：使整个客户区失效
            // RDW_UPDATENOW：立即发送 WM_PAINT，不等消息队列
            // RDW_ALLCHILDREN：递归到子窗口（WebView2 渲染窗口）
            let rect = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            let _ = RedrawWindow(
                Some(hwnd),
                Some(&rect),
                None,
                RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
            );
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        use tauri::{PhysicalPosition, PhysicalSize};
        window
            .set_position(PhysicalPosition::new(x, y))
            .map_err(err_str)?;
        window
            .set_size(PhysicalSize::new(width, height))
            .map_err(err_str)?;
        Ok(())
    }
}

// ============ 点击穿透暂停计数器（子窗口显示期间全窗口响应鼠标） ============
//
// 点击穿透的核心判定由 click_through 模块的 WM_NCHITTEST 子类完成：
//   - 鼠标在中心 1/3 宽 × 4/9 高矩形内 → HTCLIENT（WebView 处理事件）
//   - 矩形外 → HTTRANSPARENT（事件穿透到下层窗口）
//
// 本文件只负责"暂停穿透"的原子计数器：当 toast/bubble/右键菜单/InputDialog 等
// 前端子窗口显示时，前端调用 suspend_click_through() (+1)，隐藏时 resume (-1)。
// 计数器 > 0 时，WM_NCHITTEST 无条件返回 HTCLIENT，整个窗口响应鼠标，
// 让菜单项等 UI 在任意位置都能被点击。

/// 点击穿透暂停计数器：> 0 时 WM_NCHITTEST 全窗口返回 HTCLIENT（不穿透）
///
/// toast/bubble/右键菜单/InputDialog 等子窗口显示时，前端调用
/// suspend_click_through() (+1)，隐藏时调用 resume_click_through() (-1)。
pub(crate) static CLICK_THROUGH_SUSPEND_COUNT: AtomicI32 = AtomicI32::new(0);

/// 暂停点击穿透：计数器 +1，子窗口显示期间角色窗口全区域响应鼠标
///
/// `reason` 由前端传入，标识调用来源（如 "toast"/"bubble"/"context_menu"/"input_dialog"），
/// 用于日志追踪 suspend/resume 配对情况。
#[tauri::command]
pub fn suspend_click_through(reason: Option<String>) -> Result<(), String> {
    let new_val = CLICK_THROUGH_SUSPEND_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::info!(
        "[click_through] suspend +1 (reason={:?}) → count={}",
        reason.unwrap_or_default(),
        new_val
    );
    Ok(())
}

/// 恢复点击穿透：计数器 -1（不小于 0），归零后恢复中心矩形穿透逻辑
///
/// `reason` 由前端传入，标识调用来源，用于日志追踪 suspend/resume 配对情况。
#[tauri::command]
pub fn resume_click_through(reason: Option<String>) -> Result<(), String> {
    let prev = CLICK_THROUGH_SUSPEND_COUNT.fetch_sub(1, Ordering::SeqCst);
    let new_val = if prev <= 0 {
        // 防止 resume 多于 suspend 导致下溢
        CLICK_THROUGH_SUSPEND_COUNT.store(0, Ordering::SeqCst);
        0
    } else {
        prev - 1
    };
    if prev <= 0 {
        // 无配对的 resume：某处 suspend/resume 不对称，是穿透状态异常的排查信号
        tracing::warn!(
            "[click_through] resume 无配对 suspend (reason={:?})，计数器已钳制为 0",
            reason.unwrap_or_default()
        );
    } else {
        tracing::info!(
            "[click_through] resume -1 (reason={:?}) → count={} (was {})",
            reason.unwrap_or_default(),
            new_val,
            prev
        );
    }
    Ok(())
}

/// 获取点击穿透的诊断状态（供前端排查）
///
/// 返回 JSON，包含：
/// - `suspend_count`: 当前暂停计数器值（>0 时全窗口响应鼠标）
/// - `total_entries`: 已子类化的 HWND 总数
/// - `entries`: 每个被子类化的 HWND 详情（hwnd/parent_hwnd/label/class_name/is_window_valid）
/// - `characters`: 每个角色的安装状态和光标是否在中心矩形内
#[tauri::command]
pub fn get_click_through_status(
    app: AppHandle,
    state: State<'_, std::sync::Arc<crate::state::AppState>>,
) -> Result<Value, String> {
    Ok(super::click_through::get_status(&app, state.inner()))
}

// ============ side_chat 右缘三态侧边栏 ============
//
// side_chat 侧边栏停靠屏幕右缘，默认完全隐藏（窗口整体位于屏外右侧）。
// 三态流转（全部带滑动动画）：
//   Hidden   完全隐藏：窗口 x = 显示器右缘，整体在屏外
//   Peek     探出：鼠标靠近屏幕右缘（EDGE_PX 内、垂直中间 2/5）→ 滑出 PEEK_PX 像素
//            探出条保持鼠标穿透，由全局鼠标 Hook 检测单击命中 → 展开
//   Expanded 展开：点击探出条 → 整窗滑入；锁定/输入框打开时常驻；
//            光标离开宽限后自动收回；退出按钮立即收回
//
// 可见性的唯一权威是本 Rust 线程，直接调用 win.show()/hide()。原因：WebView2
// 在窗口隐藏/失焦时会节流前端 setInterval 与 emit+listen IPC（见本文件顶部注释），
// 前端事件驱动不可靠。锁定/输入框状态由前端通过原子量告知线程。

/// 边缘检测线程幂等守卫：双角色窗口都会调用 start，仅首个生效
static SIDE_CHAT_EDGE_RUNNING: AtomicBool = AtomicBool::new(false);
/// 锁定标志：true 时窗口常驻不自动隐藏（双击或快捷键设置）
static SIDE_CHAT_LOCKED: AtomicBool = AtomicBool::new(false);
/// 输入框打开标志：true 时禁止自动隐藏，避免打字途中窗口被收回
static SIDE_CHAT_INPUT_OPEN: AtomicBool = AtomicBool::new(false);
/// Peek 态标志：true = 仅探出 PEEK_PX 像素（单击展开），false = 展开或隐藏
static SIDE_CHAT_PEEK: AtomicBool = AtomicBool::new(false);
/// Peek 探出宽度（物理像素）
const SIDE_CHAT_PEEK_PX: i32 = 10;
/// 边缘检测线程停止标志与句柄
static SIDE_CHAT_EDGE_STOP: Lazy<Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>> =
    Lazy::new(|| Mutex::new(None));

/// 滑动动画时长与步长（物理像素位移，ease-out cubic）
const SIDE_CHAT_ANIM_MS: u64 = 220;
const SIDE_CHAT_ANIM_STEP_MS: u64 = 8;
/// 滑动动画进行中标志：true 时边缘循环跳过 show/hide 决策，避免与位移竞争
static SIDE_CHAT_ANIMATING: AtomicBool = AtomicBool::new(false);
/// 动画代号：每次启动自增，被新动画取代的旧动画自行终止（防止快速进出边缘时叠加）
static SIDE_CHAT_ANIM_GEN: AtomicU32 = AtomicU32::new(0);
/// 窗口展开静止位的物理 x 坐标（展开动画目标 / 收回动画起点）
static SIDE_CHAT_REST_X: AtomicI32 = AtomicI32::new(0);

// ---- 状态化鼠标穿透 + 全局 WH_MOUSE_LL 双击检测 ----
//
// 被动展示态（输入框关闭）：side_chat 设为鼠标穿透不挡桌面，webview 收不到鼠标事件，
// 由全局低级鼠标 Hook 识别双击并切换锁定；交互态（输入框打开）：关闭穿透可打字，
// 双击交给前端 React onDoubleClick。两条路径靠 SIDE_CHAT_CLICK_THROUGH 互斥，不会双触发。

/// 穿透态唯一权威标志：true=被动穿透（Hook 负责双击），false=可交互（React 负责双击）
static SIDE_CHAT_CLICK_THROUGH: AtomicBool = AtomicBool::new(false);
/// 鼠标 Hook 线程幂等守卫
static SIDE_CHAT_HOOK_RUNNING: AtomicBool = AtomicBool::new(false);
/// 回调→消费线程的非阻塞转发通道（无界 channel，send 永不阻塞，满足回调快返回约束）
static SIDE_CHAT_HOOK_TX: std::sync::OnceLock<std::sync::mpsc::Sender<(u8, i32, i32)>> =
    std::sync::OnceLock::new();
/// 上次左键按下时间（MSLLHOOKSTRUCT.time 硬件事件时间戳）与位置，供回调内双击检测
static SIDE_CHAT_LAST_CLICK_TIME: AtomicU32 = AtomicU32::new(0);
static SIDE_CHAT_LAST_CLICK_X: AtomicI32 = AtomicI32::new(i32::MIN);
static SIDE_CHAT_LAST_CLICK_Y: AtomicI32 = AtomicI32::new(i32::MIN);
/// 双击阈值：时间（GetDoubleClickTime）与位移矩形（SM_CXDOUBLECLK/SM_CYDOUBLECLK），
/// 装钩子时一次性写入，回调内只 load，避免在回调里调 Win32
static SIDE_CHAT_DBLCLK_TIME: AtomicU32 = AtomicU32::new(500);
static SIDE_CHAT_DBLCLK_CX: AtomicI32 = AtomicI32::new(4);
static SIDE_CHAT_DBLCLK_CY: AtomicI32 = AtomicI32::new(4);

/// 鼠标 Hook 两个线程的停止标志、hook 线程 id 与句柄，供退出守卫限时 join
struct SideChatHookThreads {
    stop: Arc<AtomicBool>,
    hook_tid: Arc<AtomicU32>,
    hook_handle: JoinHandle<()>,
    consumer_handle: JoinHandle<()>,
}
/// 鼠标 Hook 线程停止守卫
static SIDE_CHAT_HOOK_STOP: Lazy<Mutex<Option<SideChatHookThreads>>> =
    Lazy::new(|| Mutex::new(None));

/// 在独立线程上把 side_chat 窗口的物理 x 从 from_x 平滑移动到 to_x（ease-out cubic）。
/// then_hide=true 时移动到位后调用 hide()。动画期间置 SIDE_CHAT_ANIMATING，
/// 被更新代号取代或应用退出时提前终止并让出标志。
fn spawn_side_chat_slide(win: WebviewWindow, from_x: i32, to_x: i32, y: i32, then_hide: bool) {
    let gen = SIDE_CHAT_ANIM_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    SIDE_CHAT_ANIMATING.store(true, Ordering::SeqCst);
    thread::spawn(move || {
        let steps = (SIDE_CHAT_ANIM_MS / SIDE_CHAT_ANIM_STEP_MS).max(1) as i32;
        let mut superseded = false;
        for i in 1..=steps {
            if APP_EXITING.load(Ordering::SeqCst)
                || SIDE_CHAT_ANIM_GEN.load(Ordering::SeqCst) != gen
            {
                superseded = true;
                break;
            }
            let t = i as f64 / steps as f64;
            let eased = 1.0 - (1.0 - t).powi(3); // ease-out cubic
            let x = from_x + ((to_x - from_x) as f64 * eased).round() as i32;
            let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
            thread::sleep(Duration::from_millis(SIDE_CHAT_ANIM_STEP_MS));
        }
        // 仅当代号仍为最新才收尾并释放标志（被取代则由新动画负责）
        if !superseded && SIDE_CHAT_ANIM_GEN.load(Ordering::SeqCst) == gen {
            let _ = win.set_position(tauri::PhysicalPosition::new(to_x, y));
            if then_hide {
                let _ = win.hide();
                freeze_webview(&win);
            }
            SIDE_CHAT_ANIMATING.store(false, Ordering::SeqCst);
        }
    });
}

/// 右缘三态的窗口矩形信息。
/// 返回 `(right_x, win_top_y, win_height)`：
/// - `right_x`：显示器右缘（x 锚点）
/// - `win_top_y / win_height`：窗口在显示器上的「垂直中段位置与高度」，
///   用于将 Peek/Expand/Hide 动画与命中检测锚定到窗口自身 y 区间，
///   避免窗口高度 < 屏幕高度（iPhone 比例）时判定落在屏高 2/5 之外导致识别错位。
fn side_chat_right_frame(win: &WebviewWindow) -> Option<(i32, i32, i32)> {
    let m = win.current_monitor().ok().flatten()?;
    let mp = m.position();
    let ms = m.size();
    let right_x = mp.x + ms.width as i32;
    match (win.outer_position(), win.outer_size()) {
        (Ok(pos), Ok(size)) => {
            // 已创建：用实际位置/尺寸（可能居中垂直或已被用户摆放）
            Some((right_x, pos.y, size.height as i32))
        }
        _ => {
            // 未知尺寸：退化为显示器中间 2/5 区间作虚拟矩形（iPhone 比例）
            let mh = ms.height as i32;
            let h = (mh * 2) / 5;
            let y = mp.y + (mh - h) / 2;
            Some((right_x, y, h))
        }
    }
}

/// Hidden → Peek：瞬移到屏外右侧（显示器右缘、当前 y 位置），show 后滑入探出 PEEK_PX。
fn peek_side_chat_slide(win: &WebviewWindow) {
    SIDE_CHAT_CLICK_THROUGH.store(true, Ordering::SeqCst);
    let _ = win.set_ignore_cursor_events(true);
    let Some((right, y, _)) = side_chat_right_frame(win) else { return };
    let peek_x = right - SIDE_CHAT_PEEK_PX;
    let _ = win.set_position(tauri::PhysicalPosition::new(right, y));
    thaw_webview(win);
    if let Err(e) = win.show() {
        tracing::warn!("[side_chat_edge] show 失败: {e}");
        return;
    }
    SIDE_CHAT_PEEK.store(true, Ordering::SeqCst);
    spawn_side_chat_slide(win.clone(), right, peek_x, y, false);
}

/// Peek → Expanded：从当前位置滑入展开静止位（显示器右缘 − 窗口宽）。
fn expand_side_chat_slide(win: &WebviewWindow) {
    let Some((right, y, _)) = side_chat_right_frame(win) else { return };
    let width = win.outer_size().map(|s| s.width as i32).unwrap_or(0);
    let from_x = win.outer_position().map(|p| p.x).unwrap_or(right - SIDE_CHAT_PEEK_PX);
    let rest_x = right - width;
    SIDE_CHAT_REST_X.store(rest_x, Ordering::SeqCst);
    SIDE_CHAT_PEEK.store(false, Ordering::SeqCst);
    SIDE_CHAT_CLICK_THROUGH.store(false, Ordering::SeqCst);
    let _ = win.set_ignore_cursor_events(false);
    if !win.is_visible().ok().unwrap_or(false) {
        let _ = win.set_position(tauri::PhysicalPosition::new(from_x, y));
        thaw_webview(win);
        if let Err(e) = win.show() {
            tracing::warn!("[side_chat_expand] show 失败: {e}");
            return;
        }
    }
    spawn_side_chat_slide(win.clone(), from_x, rest_x, y, false);
}

/// Expanded/Peek → Hidden：滑出到屏外右侧后 hide。
fn hide_side_chat_slide(win: &WebviewWindow) {
    let Some((right, y, _)) = side_chat_right_frame(win) else { return };
    let pos = win.outer_position().ok();
    let from_x = pos.map(|p| p.x).unwrap_or_else(|| SIDE_CHAT_REST_X.load(Ordering::SeqCst));
    SIDE_CHAT_PEEK.store(false, Ordering::SeqCst);
    spawn_side_chat_slide(win.clone(), from_x, right, y, true);
}

/// 展开 side_chat（快捷键 / Hook 单击探出条路径）。
/// Peek 态 → 展开动画；隐藏态 → 从探出位直接滑入展开；已展开时不重复动画。
fn show_or_expand_side_chat(win: &WebviewWindow) {
    if SIDE_CHAT_PEEK.load(Ordering::SeqCst) || !win.is_visible().ok().unwrap_or(false) {
        expand_side_chat_slide(win);
    }
}

/// 以滑动动画展开侧边栏窗口（供前端快捷键路径调用，与探出条点击展开动画一致）。
/// `label` 缺省为 `chat`（微信窗口，本机制的主要目标）。
#[tauri::command]
pub fn show_side_chat_animated(app: AppHandle, label: Option<String>) -> Result<(), String> {
    let label = label.unwrap_or_else(|| "chat".to_string());
    let win = app
        .get_webview_window(&label)
        .ok_or(format!("{label} 窗口不存在"))?;
    // 直接对话侧边栏（side_chat）停靠屏幕左缘：位置由前端预置，仅需直接显示，
    // 不参与 chat 的右缘三态边缘机制。
    if label == "side_chat" {
        if !win.is_visible().ok().unwrap_or(false) {
            // 每次重新打开默认未锁定、无输入：光标离开自动隐藏（调用方可随后显式 set locked）
            SIDE_CHAT_LEFT_LOCKED.store(false, Ordering::SeqCst);
            SIDE_CHAT_LEFT_INPUT_OPEN.store(false, Ordering::SeqCst);
            let _ = app.emit("sidechat:lock_changed", json!({ "locked": false }));
            side_chat_left_show(&win);
        }
        return Ok(());
    }
    if win.is_visible().ok().unwrap_or(false) && !SIDE_CHAT_PEEK.load(Ordering::SeqCst) {
        return Ok(());
    }
    show_or_expand_side_chat(&win);
    Ok(())
}

/// 点击探出条：Peek → Expanded（前端不直接调用，由全局鼠标 Hook 消费线程触发）。
#[tauri::command]
pub fn expand_side_chat(app: AppHandle, label: Option<String>) -> Result<(), String> {
    let label = label.unwrap_or_else(|| "chat".to_string());
    let win = app
        .get_webview_window(&label)
        .ok_or(format!("{label} 窗口不存在"))?;
    show_or_expand_side_chat(&win);
    Ok(())
}

/// 点击退出按钮：立即收回屏幕右侧（滑动动画），到位后完全隐藏。
/// 同时复位锁定/输入标志并广播，前端清理残留输入态。
/// `label` 缺省为 `chat`（微信窗口）。
#[tauri::command]
pub fn collapse_side_chat(app: AppHandle, label: Option<String>) -> Result<(), String> {
    let label = label.unwrap_or_else(|| "chat".to_string());
    let win = app
        .get_webview_window(&label)
        .ok_or(format!("{label} 窗口不存在"))?;
    if !win.is_visible().ok().unwrap_or(false) {
        return Ok(());
    }
    // 直接对话侧边栏（side_chat）停靠屏幕左缘：不参与三态全局状态，直接隐藏。
    if label == "side_chat" {
        let _ = app.emit("sidechat:lock_changed", json!({ "locked": false }));
        let _ = app.emit("sidechat:input_reset", json!({}));
        let _ = win.hide();
        freeze_webview(&win);
        return Ok(());
    }
    SIDE_CHAT_LOCKED.store(false, Ordering::SeqCst);
    SIDE_CHAT_INPUT_OPEN.store(false, Ordering::SeqCst);
    let _ = app.emit("sidechat:lock_changed", json!({ "locked": false }));
    let _ = app.emit("sidechat:input_reset", json!({}));
    hide_side_chat_slide(&win);
    Ok(())
}

// ============ side_chat 全局鼠标 Hook（Peek 单击展开 + 穿透态双击锁定） ============

/// Hook 事件类别：0 = Peek 态单击探出条（展开），1 = 穿透态双击（切换锁定）
const SIDE_CHAT_HOOK_CLICK: u8 = 0;
const SIDE_CHAT_HOOK_DBLCLK: u8 = 1;

/// WH_MOUSE_LL 回调：Peek 态检测单击命中探出条 → 展开；穿透展开态检测双击 → 锁定切换。
/// 命中后非阻塞转发给消费线程。
/// 回调必须微秒级返回（否则触发 LowLevelHooksTimeout 被系统静默移除），故只做原子操作 + channel send。
#[cfg(windows)]
unsafe extern "system" fn side_chat_mouse_ll_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, MSLLHOOKSTRUCT, WM_LBUTTONDOWN};

    // nCode < 0 必须原样传递，不做任何处理
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    if (wparam.0 as u32) == WM_LBUTTONDOWN {
        // lparam 指向 MSLLHOOKSTRUCT，仅回调期间有效 → 立即拷出 pt/time 值
        let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let now = ms.time;
        let (x, y) = (ms.pt.x, ms.pt.y);
        if SIDE_CHAT_PEEK.load(Ordering::SeqCst) {
            // Peek 态：单击探出条 → 展开请求（消费线程复检窗口矩形）
            if let Some(tx) = SIDE_CHAT_HOOK_TX.get() {
                let _ = tx.send((SIDE_CHAT_HOOK_CLICK, x, y));
            }
            // 复位双击序列：避免快速双击探出条时第二击误判为"锁定切换"
            SIDE_CHAT_LAST_CLICK_TIME.store(0, Ordering::SeqCst);
        } else if SIDE_CHAT_CLICK_THROUGH.load(Ordering::SeqCst) {
            // 穿透展开态：双击检测 → 锁定切换（交互态由前端 React onDoubleClick 处理）
            let last_t = SIDE_CHAT_LAST_CLICK_TIME.load(Ordering::SeqCst);
        let dx = ((x as i64) - (SIDE_CHAT_LAST_CLICK_X.load(Ordering::SeqCst) as i64)).abs();
        let dy = ((y as i64) - (SIDE_CHAT_LAST_CLICK_Y.load(Ordering::SeqCst) as i64)).abs();
        let is_dbl = last_t != 0
            && now.wrapping_sub(last_t) <= SIDE_CHAT_DBLCLK_TIME.load(Ordering::SeqCst)
            && dx <= SIDE_CHAT_DBLCLK_CX.load(Ordering::SeqCst) as i64
            && dy <= SIDE_CHAT_DBLCLK_CY.load(Ordering::SeqCst) as i64;
        if is_dbl {
            if let Some(tx) = SIDE_CHAT_HOOK_TX.get() {
                let _ = tx.send((SIDE_CHAT_HOOK_DBLCLK, x, y)); // 无界 channel，非阻塞
            }
            SIDE_CHAT_LAST_CLICK_TIME.store(0, Ordering::SeqCst); // 复位防三连击歧义
        } else {
            SIDE_CHAT_LAST_CLICK_TIME.store(now, Ordering::SeqCst);
            SIDE_CHAT_LAST_CLICK_X.store(x, Ordering::SeqCst);
            SIDE_CHAT_LAST_CLICK_Y.store(y, Ordering::SeqCst);
        }
        }
    }
    // 永不吞事件：始终调用 CallNextHookEx 并返回其结果
    CallNextHookEx(None, code, wparam, lparam)
}

/// 消费线程命中处理：按事件类别分流（复检窗口可见性与落点矩形）。
fn handle_side_chat_hook_event(app: &AppHandle, kind: u8, x: i32, y: i32) {
    // 直接对话侧边栏（side_chat，左缘）：穿透态双击切换其独立锁定；解锁即隐藏。
    // 无输入框时窗口为穿透态（ignore_cursor_events=true），React onDoubleClick 收不到，
    // 故双击锁定由本全局 Hook 独占处理。
    if kind == SIDE_CHAT_HOOK_DBLCLK && SIDE_CHAT_CLICK_THROUGH.load(Ordering::SeqCst) {
        if let Some(win) = app.get_webview_window("side_chat") {
            if win.is_visible().ok().unwrap_or(false) {
                if let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) {
                    let hit = x >= pos.x
                        && x <= pos.x + size.width as i32
                        && y >= pos.y
                        && y <= pos.y + size.height as i32;
                    if hit {
                        let new_locked = !SIDE_CHAT_LEFT_LOCKED.load(Ordering::SeqCst);
                        SIDE_CHAT_LEFT_LOCKED.store(new_locked, Ordering::SeqCst);
                        let _ = app.emit("sidechat:lock_changed", json!({ "locked": new_locked }));
                        tracing::info!("[side_chat_hook] side_chat 双击切换锁定 → {new_locked}");
                        if !new_locked {
                            let _ = win.hide();
                            freeze_webview(&win);
                        }
                        return;
                    }
                }
            }
        }
    }

    // 微信抽屉（chat）：既有右缘逻辑
    let win = match app.get_webview_window("chat") {
        Some(w) => w,
        None => return,
    };
    if !win.is_visible().ok().unwrap_or(false) {
        return;
    }
    let (pos, size) = match (win.outer_position(), win.outer_size()) {
        (Ok(p), Ok(s)) => (p, s),
        _ => return,
    };
    // 物理像素直接比较：MSLLHOOKSTRUCT.pt 与 outer_position/outer_size 同为物理屏幕坐标
    let inside = x >= pos.x
        && x <= pos.x + size.width as i32
        && y >= pos.y
        && y <= pos.y + size.height as i32;

    match kind {
        SIDE_CHAT_HOOK_CLICK => {
            // Peek 单击：复检 Peek 态仍有效 + 落点在探出条窗口内 → 展开
            if SIDE_CHAT_PEEK.load(Ordering::SeqCst) && inside {
                tracing::info!("[side_chat_hook] 单击探出条，展开侧边栏");
                expand_side_chat_slide(&win);
            }
        }
        _ => {
            // 穿透展开态双击：切换锁定（复检穿透态 + 落点命中）
            if !SIDE_CHAT_CLICK_THROUGH.load(Ordering::SeqCst) || !inside {
                return;
            }
            let new_locked = !SIDE_CHAT_LOCKED.load(Ordering::SeqCst);
            SIDE_CHAT_LOCKED.store(new_locked, Ordering::SeqCst);
            let _ = app.emit("sidechat:lock_changed", json!({ "locked": new_locked }));
            tracing::info!("[side_chat_hook] 双击切换锁定 → {new_locked}");
            // 解锁时立即收起隐藏（锁定时不动作）
            if !new_locked {
                hide_side_chat_slide(&win);
            }
        }
    }
}

/// 启动 side_chat 全局鼠标 Hook（hook 线程 + 消费线程，幂等）。
/// 穿透态下识别双击切换锁定；交互态由前端 React onDoubleClick 处理。
#[tauri::command]
pub fn start_side_chat_mouse_hook(app: AppHandle) -> Result<(), String> {
    start_side_chat_mouse_hook_internal(app)
}

#[cfg(windows)]
fn start_side_chat_mouse_hook_internal(app: AppHandle) -> Result<(), String> {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, GetSystemMetrics, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, SM_CXDOUBLECLK, SM_CYDOUBLECLK, WH_MOUSE_LL,
    };

    if SIDE_CHAT_HOOK_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(()); // 已在运行
    }

    let (tx, rx) = std::sync::mpsc::channel::<(u8, i32, i32)>();
    let _ = SIDE_CHAT_HOOK_TX.set(tx); // OnceLock；忽略 Err（仅重启场景，此处不发生）

    let stop = Arc::new(AtomicBool::new(false));
    let hook_tid = Arc::new(AtomicU32::new(0));

    // hook 线程：安装 WH_MOUSE_LL + 消息泵（低级钩子必须配消息泵才派发）
    let stop_h = Arc::clone(&stop);
    let tid_h = Arc::clone(&hook_tid);
    let hook_handle = thread::Builder::new()
        .name("sidechat-mouse-hook".into())
        .spawn(move || unsafe {
            tid_h.store(GetCurrentThreadId(), Ordering::SeqCst);
            // 阈值一次性写入静态量，回调内只 load
            SIDE_CHAT_DBLCLK_TIME.store(GetDoubleClickTime(), Ordering::SeqCst);
            SIDE_CHAT_DBLCLK_CX.store(GetSystemMetrics(SM_CXDOUBLECLK), Ordering::SeqCst);
            SIDE_CHAT_DBLCLK_CY.store(GetSystemMetrics(SM_CYDOUBLECLK), Ordering::SeqCst);

            let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(side_chat_mouse_ll_proc), None, 0)
            {
                Ok(h) if !h.0.is_null() => h,
                _ => {
                    tracing::warn!("[side_chat_hook] SetWindowsHookExW 失败");
                    return;
                }
            };
            tracing::info!("[side_chat_hook] WH_MOUSE_LL 已安装");

            let mut msg = std::mem::zeroed();
            while !stop_h.load(Ordering::SeqCst) {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if !ret.as_bool() {
                    break; // WM_QUIT(0) 或错误 → 退出
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(hook); // 在持有 hook 的本线程卸载
            tracing::info!("[side_chat_hook] WH_MOUSE_LL 已卸载");
        })
        .map_err(err_str)?;

    // 消费线程：收双击点 → 切换锁定（重逻辑在此，不在回调）
    let stop_c = Arc::clone(&stop);
    let app_c = app.clone();
    let consumer_handle = thread::Builder::new()
        .name("sidechat-mouse-consumer".into())
        .spawn(move || {
            while !stop_c.load(Ordering::SeqCst) && !APP_EXITING.load(Ordering::SeqCst) {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok((kind, x, y)) => handle_side_chat_hook_event(&app_c, kind, x, y),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(err_str)?;

    *SIDE_CHAT_HOOK_STOP.lock() = Some(SideChatHookThreads {
        stop,
        hook_tid,
        hook_handle,
        consumer_handle,
    });
    Ok(())
}

#[cfg(not(windows))]
fn start_side_chat_mouse_hook_internal(_app: AppHandle) -> Result<(), String> {
    Ok(())
}

/// 停止 side_chat 全局鼠标 Hook 线程（内部实现，应用退出时调用）。
/// 置停止标志 + PostThreadMessageW(WM_QUIT) 唤醒消息泵，限时 1s join 两线程，避免退出死锁。
#[cfg(windows)]
pub(crate) fn stop_side_chat_mouse_hook_internal() {
    let entry = SIDE_CHAT_HOOK_STOP.lock().take();
    if let Some(threads) = entry {
        threads.stop.store(true, Ordering::SeqCst);
        let tid = threads.hook_tid.load(Ordering::SeqCst);
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
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = threads.hook_handle.join();
            let _ = threads.consumer_handle.join();
            let _ = done_tx.send(());
        });
        if done_rx.recv_timeout(Duration::from_millis(1000)).is_err() {
            tracing::warn!("[side_chat_hook] 线程未在 1s 内退出，放弃等待");
        }
    }
    SIDE_CHAT_HOOK_RUNNING.store(false, Ordering::SeqCst);
}

#[cfg(not(windows))]
pub(crate) fn stop_side_chat_mouse_hook_internal() {}

/// 启动 side_chat 边缘检测线程（每 ~60ms 一帧，幂等）。
///
/// 线程职责（右缘三态）：
/// - 隐藏态：光标进入右缘 zone → Peek 探出（重置锁定/输入标志，通知前端清理残留输入态）
/// - Peek 态：光标离开右缘 zone / 探出条 → 宽限后收回完全隐藏；
///   单击探出条由全局鼠标 Hook 检测并展开（不经本线程）
/// - 展开态：锁定或输入框打开 → 常驻；光标在窗口/右缘 zone 内 → 保持；
///   否则宽限 GRACE_TICKS 后收回隐藏
#[tauri::command]
pub fn start_side_chat_edge_watcher(app: AppHandle) -> Result<(), String> {
    if SIDE_CHAT_EDGE_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(()); // 已有线程在运行
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_clone = Arc::clone(&stop_flag);

    let thread = thread::spawn(move || {
        tracing::info!("[side_chat_edge] 线程启动");

        const EDGE_PX: i32 = 12; // 右缘触发宽度（物理像素）
        const GRACE_TICKS: u32 = 7; // 自动隐藏宽限（7 * 60ms ≈ 420ms）
        const MONITOR_REFRESH_TICKS: u32 = 30; // 显示器缓存刷新周期
        // 隐藏态延迟冻结：等前端预创建后的初始化（React 挂载/历史加载）完成再挂起
        const FREEZE_DELAY_TICKS: u32 = 50; // 50 * 60ms = 3s

        let mut hide_countdown: u32 = 0;
        let mut freeze_delay: u32 = 0;
        let mut tick: u32 = 0;
        // ESC 下降沿跟踪：仅在 false→true 跳变时触发解锁，避免按住时重复触发
        let mut esc_prev = false;
        // 缓存窗口所在显示器的原点与尺寸（物理像素）
        let mut mon: Option<(i32, i32, i32, i32)> = None;

        while !stop_flag_clone.load(Ordering::SeqCst) && !APP_EXITING.load(Ordering::SeqCst) {
            let win = match app.get_webview_window("chat") {
                Some(w) => w,
                None => {
                    // 窗口尚未预创建/已销毁：空闲等待
                    thread::sleep(Duration::from_millis(60));
                    continue;
                }
            };

            let c = match app.cursor_position() {
                Ok(c) => c,
                Err(_) => {
                    thread::sleep(Duration::from_millis(60));
                    continue;
                }
            };
            let cx = c.x as i32;
            let cy = c.y as i32;

            // 定期刷新显示器缓存（窗口可能被重新定位到其他显示器）
            tick += 1;
            if mon.is_none() || tick % MONITOR_REFRESH_TICKS == 0 {
                if let Ok(Some(m)) = win.current_monitor() {
                    let mp = m.position();
                    let ms = m.size();
                    mon = Some((mp.x, mp.y, ms.width as i32, ms.height as i32));
                }
            }

            let visible = win.is_visible().ok().unwrap_or(false);

            // 滑动动画进行中：跳过本帧 show/hide 决策，避免与原生位移竞争
            if SIDE_CHAT_ANIMATING.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(60));
                continue;
            }

            // 边缘 zone：显示器右缘 EDGE_PX 内；垂直判定改为窗口自身 y 区间
            // （当窗口按 iPhone 比例缩小时，不再与屏高 2/5 强绑定），
            // 同时向上下各扩张 EDGE_PX 以方便鼠标靠近。
            let in_edge_zone = match (mon, side_chat_right_frame(&win)) {
                (Some((_mx, my, _mw, mh)), Some((right, win_y, win_h))) => {
                    let lo = (win_y - EDGE_PX).max(my);
                    let hi = (win_y + win_h + EDGE_PX).min(my + mh);
                    cx >= right - EDGE_PX
                        && cx <= right
                        && cy >= lo
                        && cy <= hi
                }
                _ => false,
            };

            // 光标是否在窗口矩形内（含 4px 容差）：展开态保持/Peek 态判定共用
            let in_window = if let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) {
                cx >= pos.x - 4
                    && cx <= pos.x + size.width as i32 + 4
                    && cy >= pos.y - 4
                    && cy <= pos.y + size.height as i32 + 4
            } else {
                false
            };

            let peek = SIDE_CHAT_PEEK.load(Ordering::SeqCst);

            if !visible {
                // 隐藏态延迟冻结（幂等：每轮隐藏只触发一次，后续 hide 动画收尾也会冻结）
                freeze_delay += 1;
                if freeze_delay == FREEZE_DELAY_TICKS {
                    freeze_webview(&win);
                }
                if in_edge_zone {
                    // 右缘悬停 → Peek：重置锁定/输入标志（本次为未锁定悬停）
                    SIDE_CHAT_LOCKED.store(false, Ordering::SeqCst);
                    SIDE_CHAT_INPUT_OPEN.store(false, Ordering::SeqCst);
                    hide_countdown = 0;
                    peek_side_chat_slide(&win);
                    tracing::info!("[side_chat_edge] 右缘悬停探出");
                    // 通知前端同步锁图标 + 清理上次残留的输入态
                    let _ = app.emit("sidechat:lock_changed", json!({ "locked": false }));
                    let _ = app.emit("sidechat:input_reset", json!({}));
                }
            } else if peek {
                freeze_delay = 0;
                // Peek 态：光标仍在右缘 zone / 探出条附近保持，离开宽限后收回完全隐藏
                if in_edge_zone || in_window {
                    hide_countdown = 0;
                } else {
                    hide_countdown += 1;
                    if hide_countdown >= GRACE_TICKS {
                        hide_countdown = 0;
                        hide_side_chat_slide(&win);
                        tracing::info!("[side_chat_edge] 光标离开探出条，收回隐藏");
                    }
                }
            } else {
                freeze_delay = 0;
                let locked = SIDE_CHAT_LOCKED.load(Ordering::SeqCst);
                let input_open = SIDE_CHAT_INPUT_OPEN.load(Ordering::SeqCst);
                if locked || input_open {
                    hide_countdown = 0; // 锁定/输入中：常驻
                } else if in_window || in_edge_zone {
                    hide_countdown = 0; // 光标仍在窗口 / 屏幕右缘：保持
                } else {
                    hide_countdown += 1;
                    if hide_countdown >= GRACE_TICKS {
                        hide_countdown = 0;
                        hide_side_chat_slide(&win);
                        tracing::info!("[side_chat_edge] 光标离开，自动收回隐藏");
                    }
                }

                // 鼠标悬浮在窗口上 + ESC 下降沿：锁定→解锁并立即收起隐藏
                let esc_now = is_escape_down();
                if esc_now && !esc_prev && locked && in_window {
                    SIDE_CHAT_LOCKED.store(false, Ordering::SeqCst);
                    let _ = app.emit("sidechat:lock_changed", json!({ "locked": false }));
                    hide_side_chat_slide(&win);
                    tracing::info!("[side_chat_edge] 悬浮时按 ESC 解锁并隐藏");
                }
                esc_prev = esc_now;
            }

            thread::sleep(Duration::from_millis(60));
        }

        // 线程退出：清空句柄并释放幂等守卫
        *SIDE_CHAT_EDGE_STOP.lock() = None;
        SIDE_CHAT_EDGE_RUNNING.store(false, Ordering::SeqCst);
        tracing::info!("[side_chat_edge] 线程已退出");
    });

    *SIDE_CHAT_EDGE_STOP.lock() = Some((stop_flag, thread));
    Ok(())
}

/// 停止 side_chat 边缘检测线程（内部实现，应用退出时调用）。
///
/// 设停止标志后在有界时间内等待退出，避免卡在阻塞式窗口调用导致退出死锁。
pub(crate) fn stop_side_chat_edge_watcher_internal() {
    let entry = SIDE_CHAT_EDGE_STOP.lock().take();
    if let Some((flag, handle)) = entry {
        flag.store(true, Ordering::SeqCst);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = handle.join();
            let _ = done_tx.send(());
        });
        if done_rx.recv_timeout(Duration::from_millis(1000)).is_err() {
            tracing::warn!("[side_chat_edge] 线程未在 1s 内退出，放弃等待");
        }
    }
}

// ============ side_chat 左缘看护线程（直接对话侧边栏：自动隐藏 + 锁定 + ESC 解锁） ============
//
// 与 chat（微信）右缘三态机制解耦：side_chat 停靠左缘，位置固定。恢复「双击解锁后光标
// 离开自动隐藏」「ESC 解锁并隐藏」语义：未锁定且光标离开窗口到宽限后自动隐藏；锁定或
// 输入框打开时保持；悬浮按 ESC（下降沿）解锁并立即隐藏。

/// side_chat 独立锁定标志（不共享 chat 的三态 SIDE_CHAT_LOCKED）
static SIDE_CHAT_LEFT_LOCKED: AtomicBool = AtomicBool::new(false);
/// side_chat 输入框打开标志：打字期间不自动隐藏
static SIDE_CHAT_LEFT_INPUT_OPEN: AtomicBool = AtomicBool::new(false);
/// 左缘看护线程幂等守卫
static SIDE_CHAT_LEFT_WATCH_RUNNING: AtomicBool = AtomicBool::new(false);
/// 左缘看护线程停止标志与句柄
static SIDE_CHAT_LEFT_WATCH_STOP: Lazy<Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>> =
    Lazy::new(|| Mutex::new(None));

/// 左缘滑动动画独立标志（不与 chat 右缘的 SIDE_CHAT_ANIM_* 共享，避免两窗互相争抢）
static SIDE_CHAT_LEFT_ANIMATING: AtomicBool = AtomicBool::new(false);
static SIDE_CHAT_LEFT_ANIM_GEN: AtomicU32 = AtomicU32::new(0);

/// 取窗口所在显示器的左缘物理 x 坐标（左缘呼出/收回的锚点）。
fn side_chat_left_edge(win: &WebviewWindow) -> Option<i32> {
    win.current_monitor().ok().flatten().map(|m| m.position().x)
}

/// 左缘滑动动画：物理 x 从 from_x 平滑移动到 to_x（ease-out cubic）。
/// then_hide=true 时移动到位后调用 hide()。动画期间置左缘独立 ANIMATING，被更新代号
/// 取代或应用退出时提前终止并释放标志。
fn spawn_side_chat_left_slide(win: WebviewWindow, from_x: i32, to_x: i32, y: i32, then_hide: bool) {
    let gen = SIDE_CHAT_LEFT_ANIM_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    SIDE_CHAT_LEFT_ANIMATING.store(true, Ordering::SeqCst);
    thread::spawn(move || {
        let steps = (SIDE_CHAT_ANIM_MS / SIDE_CHAT_ANIM_STEP_MS).max(1) as i32;
        let mut superseded = false;
        for i in 1..=steps {
            if APP_EXITING.load(Ordering::SeqCst)
                || SIDE_CHAT_LEFT_ANIM_GEN.load(Ordering::SeqCst) != gen
            {
                superseded = true;
                break;
            }
            let t = i as f64 / steps as f64;
            let eased = 1.0 - (1.0 - t).powi(3); // ease-out cubic
            let x = from_x + ((to_x - from_x) as f64 * eased).round() as i32;
            let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
            thread::sleep(Duration::from_millis(SIDE_CHAT_ANIM_STEP_MS));
        }
        if !superseded && SIDE_CHAT_LEFT_ANIM_GEN.load(Ordering::SeqCst) == gen {
            let _ = win.set_position(tauri::PhysicalPosition::new(to_x, y));
            if then_hide {
                let _ = win.hide();
                freeze_webview(&win);
            }
            SIDE_CHAT_LEFT_ANIMATING.store(false, Ordering::SeqCst);
        }
    });
}

/// Hidden → 呼出：瞬移到屏外左侧（显示器左缘 − 窗口宽），show 后滑入左缘静止位。
fn side_chat_left_show(win: &WebviewWindow) {
    let Some(left) = side_chat_left_edge(win) else { return };
    let width = win.outer_size().map(|s| s.width as i32).unwrap_or(0);
    let y = win.outer_position().map(|p| p.y).unwrap_or(0);
    let from_x = left - width;
    let _ = win.set_position(tauri::PhysicalPosition::new(from_x, y));
    thaw_webview(win);
    if let Err(e) = win.show() {
        tracing::warn!("[side_chat_left] show 失败: {e}");
        return;
    }
    spawn_side_chat_left_slide(win.clone(), from_x, left, y, false);
}

/// Expanded → 收回：从当前位置滑出到屏外左侧后 hide。
fn side_chat_left_hide(win: &WebviewWindow) {
    let Some(left) = side_chat_left_edge(win) else { return };
    let width = win.outer_size().map(|s| s.width as i32).unwrap_or(0);
    let pos = win.outer_position().ok();
    let from_x = pos.map(|p| p.x).unwrap_or(left);
    let y = pos.map(|p| p.y).unwrap_or(0);
    let to_x = left - width;
    spawn_side_chat_left_slide(win.clone(), from_x, to_x, y, true);
}

/// 启动 side_chat 左缘看护线程（每 ~60ms 一帧，幂等）。
#[tauri::command]
pub fn start_side_chat_left_watcher(app: AppHandle) -> Result<(), String> {
    if SIDE_CHAT_LEFT_WATCH_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(()); // 已有线程在运行
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = Arc::clone(&stop);
    let thread = thread::spawn(move || {
        tracing::info!("[side_chat_left] 线程启动");
        const GRACE_TICKS: u32 = 7; // 7 * 60ms ≈ 420ms 自动收回宽限
        const EDGE_PX: i32 = 12; // 左缘触发宽度（物理像素）
        const MONITOR_REFRESH_TICKS: u32 = 30;
        // 隐藏态延迟冻结：等前端预创建后的初始化完成再挂起（50 * 60ms = 3s）
        const FREEZE_DELAY_TICKS: u32 = 50;
        let mut hide_countdown: u32 = 0;
        let mut freeze_delay: u32 = 0;
        let mut esc_prev = false;
        let mut tick: u32 = 0;
        let mut mon: Option<(i32, i32, i32, i32)> = None; // (left, top, w, h)

        while !stop_c.load(Ordering::SeqCst) && !APP_EXITING.load(Ordering::SeqCst) {
            let win = match app.get_webview_window("side_chat") {
                Some(w) => w,
                None => {
                    thread::sleep(Duration::from_millis(60));
                    continue;
                }
            };

            let c = match app.cursor_position() {
                Ok(c) => c,
                Err(_) => {
                    thread::sleep(Duration::from_millis(60));
                    continue;
                }
            };
            let cx = c.x as i32;
            let cy = c.y as i32;

            // 定期刷新显示器缓存（窗口可能被重新定位到其他显示器）
            tick += 1;
            if mon.is_none() || tick % MONITOR_REFRESH_TICKS == 0 {
                if let Ok(Some(m)) = win.current_monitor() {
                    let mp = m.position();
                    let ms = m.size();
                    mon = Some((mp.x, mp.y, ms.width as i32, ms.height as i32));
                }
            }

            let visible = win.is_visible().ok().unwrap_or(false);

            // 左缘滑动动画进行中：跳过本帧 show/hide 决策，避免与原生位移竞争
            if SIDE_CHAT_LEFT_ANIMATING.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(60));
                continue;
            }

            // 左缘 zone：显示器左缘 EDGE_PX 内、垂直中间 2/5（与窗口高度一致）
            let in_edge_zone = match mon {
                Some((mx, my, _mw, mh)) => {
                    cx >= mx
                        && cx <= mx + EDGE_PX
                        && cy >= my + (mh * 3) / 10
                        && cy <= my + (mh * 7) / 10
                }
                None => false,
            };

            // 光标是否在窗口矩形内（含 4px 容差）
            let in_window = match (win.outer_position(), win.outer_size()) {
                (Ok(pos), Ok(size)) => {
                    cx >= pos.x - 4
                        && cx <= pos.x + size.width as i32 + 4
                        && cy >= pos.y - 4
                        && cy <= pos.y + size.height as i32 + 4
                }
                _ => false,
            };

            let locked = SIDE_CHAT_LEFT_LOCKED.load(Ordering::SeqCst);
            let input_open = SIDE_CHAT_LEFT_INPUT_OPEN.load(Ordering::SeqCst);

            if !visible {
                // 隐藏态延迟冻结（幂等：每轮隐藏只触发一次）
                freeze_delay += 1;
                if freeze_delay == FREEZE_DELAY_TICKS {
                    freeze_webview(&win);
                }
                // 隐藏态：鼠标靠近左缘 → 呼出（复位锁定/输入，未锁定状态）
                if in_edge_zone {
                    SIDE_CHAT_LEFT_LOCKED.store(false, Ordering::SeqCst);
                    SIDE_CHAT_LEFT_INPUT_OPEN.store(false, Ordering::SeqCst);
                    hide_countdown = 0;
                    side_chat_left_show(&win);
                    tracing::info!("[side_chat_left] 左缘悬停呼出");
                    let _ = app.emit("sidechat:lock_changed", json!({ "locked": false }));
                    let _ = app.emit("sidechat:input_reset", json!({}));
                }
            } else {
                freeze_delay = 0;
                // 锁定/输入框打开/光标在窗内/仍在左缘 → 保持；否则宽限后收回
                if locked || input_open || in_window || in_edge_zone {
                    hide_countdown = 0;
                } else {
                    hide_countdown += 1;
                    if hide_countdown >= GRACE_TICKS {
                        hide_countdown = 0;
                        side_chat_left_hide(&win);
                        tracing::info!("[side_chat_left] 光标离开，自动收回");
                    }
                }

                // 鼠标悬停在窗口上按 ESC（下降沿）：解锁并立即收回隐藏
                let esc_now = is_escape_down();
                if esc_now && !esc_prev && in_window {
                    SIDE_CHAT_LEFT_LOCKED.store(false, Ordering::SeqCst);
                    let _ = app.emit("sidechat:lock_changed", json!({ "locked": false }));
                    side_chat_left_hide(&win);
                    tracing::info!("[side_chat_left] 悬浮按 ESC 收回");
                }
                esc_prev = esc_now;
            }

            thread::sleep(Duration::from_millis(60));
        }

        // 线程退出：清空句柄并释放幂等守卫
        *SIDE_CHAT_LEFT_WATCH_STOP.lock() = None;
        SIDE_CHAT_LEFT_WATCH_RUNNING.store(false, Ordering::SeqCst);
        tracing::info!("[side_chat_left] 线程已退出");
    });
    *SIDE_CHAT_LEFT_WATCH_STOP.lock() = Some((stop, thread));
    Ok(())
}

/// 停止 side_chat 左缘看护线程（应用退出时调用）。
pub(crate) fn stop_side_chat_left_watcher_internal() {
    let entry = SIDE_CHAT_LEFT_WATCH_STOP.lock().take();
    if let Some((flag, handle)) = entry {
        flag.store(true, Ordering::SeqCst);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = handle.join();
            let _ = done_tx.send(());
        });
        if done_rx.recv_timeout(Duration::from_millis(1000)).is_err() {
            tracing::warn!("[side_chat_left] 线程未在 1s 内退出，放弃等待");
        }
    }
}

/// 设置 side_chat 锁定状态（双击 header 或快捷键调用）。
///
/// 锁定后边缘检测线程不会自动隐藏窗口；同时广播 `sidechat:lock_changed`
/// 供前端同步锁图标。
#[tauri::command]
pub fn set_side_chat_locked(app: AppHandle, locked: bool, label: Option<String>) -> Result<(), String> {
    if label.as_deref().map_or(true, |l| l == "chat") {
        // 状态化三态仅作用于微信抽屉（chat）
        SIDE_CHAT_LOCKED.store(locked, Ordering::SeqCst);
    } else {
        // 直接对话侧边栏（side_chat）使用左缘独立锁定状态
        SIDE_CHAT_LEFT_LOCKED.store(locked, Ordering::SeqCst);
    }
    let _ = app.emit("sidechat:lock_changed", json!({ "locked": locked }));
    Ok(())
}

/// 设置侧边栏输入框打开状态（前端 InputDialog 显隐时调用）。
///
/// 打开期间边缘检测线程不会自动隐藏窗口，避免打字途中被收回。
/// 同时驱动状态化穿透：输入框打开 → 关闭穿透可交互打字；关闭 → 开启穿透不挡桌面
/// （穿透态下双击由全局鼠标 Hook 识别，交互态下由前端 React onDoubleClick 识别）。
/// 仅 `chat`（微信抽屉）参与全局状态；side_chat 输入面板不共享该状态。
#[tauri::command]
pub fn set_side_chat_input_open(app: AppHandle, open: bool, label: Option<String>) -> Result<(), String> {
    let is_chat = label.as_deref().map_or(true, |l| l == "chat");
    if is_chat {
        SIDE_CHAT_INPUT_OPEN.store(open, Ordering::SeqCst);
    } else {
        SIDE_CHAT_LEFT_INPUT_OPEN.store(open, Ordering::SeqCst);
    }
    if let Some(win) = app.get_webview_window(if is_chat { "chat" } else { "side_chat" }) {
        if open {
            SIDE_CHAT_CLICK_THROUGH.store(false, Ordering::SeqCst);
            let _ = win.set_ignore_cursor_events(false);
        } else {
            SIDE_CHAT_CLICK_THROUGH.store(true, Ordering::SeqCst);
            let _ = win.set_ignore_cursor_events(true);
        }
    }
    Ok(())
}

/// 获取窗口尺寸
#[tauri::command]
pub fn get_window_size(window: tauri::WebviewWindow) -> Result<Value, String> {
    let size = window.outer_size().map_err(err_str)?;
    Ok(json!({
        "width": size.width,
        "height": size.height,
    }))
}

/// 设置窗口透明度（Windows 使用分层窗口，跨平台回退到 set_effects）
#[tauri::command]
pub fn set_window_opacity(window: tauri::WebviewWindow, opacity: f64) -> Result<(), String> {
    let opacity = opacity.clamp(0.0, 1.0);

    #[cfg(windows)]
    {
        use windows::Win32::Foundation::COLORREF;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE,
            LWA_ALPHA, WS_EX_LAYERED,
        };

        let hwnd_tauri = window.hwnd().map_err(err_str)?;
        // 构造 windows 0.58 兼容的 HWND（与 pet_controller.rs 同款转换）
        let hwnd = windows::Win32::Foundation::HWND(hwnd_tauri.0);
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_LAYERED.0 as isize));
            let alpha = (opacity * 255.0) as u8;
            SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = (window, opacity);
        Err("当前平台不支持设置窗口透明度".to_string())
    }
}

/// 显示窗口
#[tauri::command]
pub fn show_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(err_str)?;
    window.set_focus().map_err(err_str)
}

/// 隐藏窗口
#[tauri::command]
pub fn hide_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.hide().map_err(err_str)
}

/// 切换调用方窗口可见性（角色窗口或子窗口均可）
#[tauri::command]
pub fn toggle_window_visibility(window: tauri::WebviewWindow) -> Result<bool, String> {
    let visible = window.is_visible().map_err(err_str)?;
    if visible {
        window.hide().map_err(err_str)?;
        Ok(false)
    } else {
        window.show().map_err(err_str)?;
        window.set_focus().map_err(err_str)?;
        Ok(true)
    }
}

/// 最小化到托盘（隐藏窗口，不退出进程）
#[tauri::command]
pub fn minimize_to_tray(window: tauri::WebviewWindow) -> Result<(), String> {
    window.hide().map_err(err_str)
}

/// 从托盘恢复（显示并聚焦）
#[tauri::command]
pub fn restore_from_tray(window: tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(err_str)?;
    window.set_focus().map_err(err_str)
}

/// 聚焦指定标签的窗口
#[tauri::command]
pub fn focus_window(app: AppHandle, label: String) -> Result<(), String> {
    let win = window_by_label(&app, &label)?;
    win.set_focus().map_err(err_str)
}

/// 打开子窗口（chat / config / memory / diary）。若已存在则聚焦。
///
/// 统一通过 Tauri 的 WebviewWindow 创建。所有子窗口均为 decorations:false（borderless）+
/// data-tauri-drag-region draggable，符合项目硬约束。
#[tauri::command]
pub async fn open_child_window(
    app: AppHandle,
    label: String,
    url: String,
    title: String,
    width: u32,
    height: u32,
) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(&label) {
        existing.show().map_err(err_str)?;
        existing.set_focus().map_err(err_str)?;
        return Ok(());
    }

    let webview_window = tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::App(url.into()),
    )
    .title(title)
    .inner_size(width as f64, height as f64)
    .min_inner_size(320.0, 300.0)
    .decorations(false)
    .transparent(true)
    .resizable(true)
    .center()
    .build()
    .map_err(err_str)?;

    // 子窗口加载完成后由其自身的 data-tauri-drag-region 区域负责拖拽
    let _ = webview_window;
    Ok(())
}

/// 关闭指定标签的子窗口
#[tauri::command]
pub fn close_child_window(app: AppHandle, label: String) -> Result<(), String> {
    let win = window_by_label(&app, &label)?;
    win.close().map_err(err_str)
}

/// 获取所有子窗口标签
///
/// 排除以下窗口：
/// - `"main"`：tauri.conf.json 预定义的隐藏控制器，无 UI
/// - 角色桌宠窗口：label = character_id（如 "nana" / "vivian"），由角色窗口自身管理生命周期
///
/// 仅返回真正的子窗口（chat / config / memory / diary / bubble / toast / status 等），
/// 供退出时批量关闭使用。
#[tauri::command]
pub fn list_child_windows(
    app: AppHandle,
    state: State<'_, std::sync::Arc<crate::state::AppState>>,
) -> Result<Vec<String>, String> {
    // 收集所有角色 ID（角色桌宠窗口的 label = character_id）
    let character_ids: std::collections::HashSet<String> = {
        let chars = state.characters.read();
        chars.keys().cloned().collect()
    };

    let labels: Vec<String> = app
        .webview_windows()
        .keys()
        .filter(|k| *k != "main")
        .filter(|k| !character_ids.contains(*k))
        .cloned()
        .collect();
    Ok(labels)
}

/// 设置窗口是否可调整大小
#[tauri::command]
pub fn set_window_resizable(window: tauri::WebviewWindow, resizable: bool) -> Result<(), String> {
    window.set_resizable(resizable).map_err(err_str)
}

/// 设置窗口是否跳过任务栏
#[tauri::command]
pub fn set_skip_taskbar(window: tauri::WebviewWindow, skip: bool) -> Result<(), String> {
    window.set_skip_taskbar(skip).map_err(err_str)
}

/// 居中窗口
#[tauri::command]
pub fn center_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.center().map_err(err_str)
}

/// 检测当前前台是否处于全屏应用（用于桌宠智能隐藏）
///
/// 采用组合检测，覆盖三类全屏场景：
///
/// 1. **D3D 独占全屏**（原生全屏游戏 / PotPlayer 独占模式）：
///    通过 `SHQueryUserNotificationState` 返回 `QUNS_RUNNING_D3D_FULL_SCREEN`。
///    由 Windows 图形栈显式声明，不依赖尺寸比对。
///
/// 2. **窗口化全屏**（浏览器 F11 / 视频全屏按钮 / 播放器普通全屏）：
///    前台窗口矩形覆盖整个显示器 **且** 缺少 `WS_CAPTION`（标题栏）与
///    `WS_THICKFRAME`（可调边框）样式。全屏窗口会移除这些样式，
///    而最大化窗口即使任务栏隐藏、矩形相同，仍保留这些样式 ——
///    这是区分"全屏"与"最大化"的关键，不依赖任务栏可见性或分辨率。
///
/// 3. 排除调用方自身窗口、不可见窗口与桌面 shell 窗口（Progman / WorkerW），
///    桌面窗口同样覆盖全屏且无标题栏，不加排除会被误判为全屏应用。
#[tauri::command]
pub fn is_foreground_fullscreen(window: tauri::WebviewWindow) -> Result<bool, String> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITOR_DEFAULTTONEAREST, MONITORINFO,
        };
        use windows::Win32::UI::Shell::{
            QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowLongPtrW,
            GetWindowTextLengthW, GetWindowTextW, GetWindowRect, IsWindowVisible, GWL_STYLE,
            WS_CAPTION, WS_THICKFRAME,
        };

        let self_hwnd_tauri = window.hwnd().map_err(err_str)?;

        unsafe {
            // 1. D3D 独占全屏（游戏 / 播放器独占模式）
            let state = SHQueryUserNotificationState().map_err(err_str)?;
            if state == QUNS_RUNNING_D3D_FULL_SCREEN {
                tracing::debug!("[fullscreen] D3D 独占全屏 → true");
                return Ok(true);
            }

            // 2. 窗口化全屏（浏览器 F11 / 视频全屏）
            let fg = GetForegroundWindow();
            if fg.0.is_null() {
                tracing::debug!("[fullscreen] GetForegroundWindow 为 null → false");
                return Ok(false);
            }
            // 排除调用方自身窗口
            let self_hwnd = HWND(self_hwnd_tauri.0);
            if fg == self_hwnd {
                tracing::debug!("[fullscreen] 前台为调用方自身 → false");
                return Ok(false);
            }
            if !IsWindowVisible(fg).as_bool() {
                tracing::debug!("[fullscreen] 前台窗口不可见 → false");
                return Ok(false);
            }

            // 排除桌面 shell 窗口（Progman / WorkerW），
            // 桌面窗口覆盖全屏且无标题栏，会被误判为全屏应用
            let shell = GetShellWindow();
            if fg == shell {
                tracing::debug!("[fullscreen] 前台为桌面 shell (GetShellWindow) → false");
                return Ok(false);
            }
            let mut class_buf = [0u16; 256];
            let class_len = GetClassNameW(fg, &mut class_buf) as usize;
            let class_name = String::from_utf16_lossy(&class_buf[..class_len]);
            if class_name == "Progman" || class_name == "WorkerW" {
                tracing::debug!(
                    "[fullscreen] 前台为桌面 shell (class='{}') → false",
                    class_name
                );
                return Ok(false);
            }

            // 获取前台窗口标题（诊断用）
            let title_len = GetWindowTextLengthW(fg);
            let mut title_buf = vec![0u16; (title_len as usize) + 1];
            let _ = GetWindowTextW(fg, &mut title_buf);
            let title = String::from_utf16_lossy(
                &title_buf[..title_buf.iter().position(|&c| c == 0).unwrap_or(title_buf.len())],
            );

            // 获取前台窗口矩形
            let mut rect = std::mem::zeroed();
            if GetWindowRect(fg, &mut rect).is_err() {
                tracing::debug!("[fullscreen] title='{}' GetWindowRect 失败 → false", title);
                return Ok(false);
            }

            // 获取显示器矩形（rcMonitor 是整个屏幕，含任务栏区域）
            let hmon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            if hmon.0.is_null() {
                tracing::debug!("[fullscreen] title='{}' MonitorFromWindow 为 null → false", title);
                return Ok(false);
            }
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
                tracing::debug!("[fullscreen] title='{}' GetMonitorInfoW 失败 → false", title);
                return Ok(false);
            }
            let mw = mi.rcMonitor;

            // 检查窗口是否覆盖整个显示器（带容差，兼容边框数像素偏差）
            const TOLERANCE: i32 = 8;
            let covers_screen = rect.left <= mw.left + TOLERANCE
                && rect.top <= mw.top + TOLERANCE
                && rect.right >= mw.right - TOLERANCE
                && rect.bottom >= mw.bottom - TOLERANCE;
            if !covers_screen {
                tracing::debug!(
                    "[fullscreen] title='{}' 未覆盖屏幕 rect={{l={},t={},r={},b={}}} monitor={{l={},t={},r={},b={}}} → false",
                    title, rect.left, rect.top, rect.right, rect.bottom,
                    mw.left, mw.top, mw.right, mw.bottom
                );
                return Ok(false);
            }

            // 关键判定：全屏窗口会移除标题栏(WS_CAPTION)与可调边框(WS_THICKFRAME)，
            // 而最大化窗口即使矩形相同（任务栏隐藏时）仍保留这些样式。
            let style = GetWindowLongPtrW(fg, GWL_STYLE) as u32;
            let has_caption = (style & WS_CAPTION.0) != 0;
            let has_thickframe = (style & WS_THICKFRAME.0) != 0;
            let result = !has_caption && !has_thickframe;
            tracing::debug!(
                "[fullscreen] title='{}' style=0x{:08x} caption={} thickframe={} covers={} → {}",
                title, style, has_caption, has_thickframe, covers_screen, result
            );
            Ok(result)
        }
    }

    #[cfg(not(windows))]
    {
        let _ = window;
        Ok(false)
    }
}

/// 临时诊断命令：前端把状态写到后端日志文件
#[tauri::command]
pub fn debug_log(msg: String, label: String) {
    tracing::info!("[frontend:{}] {}", label, msg);
}

// ─── 智能避让：纯色区域检测 ───────────────────────────────────────────

/// 屏幕变化检测的哈希状态：按 window label 分桶，每个角色窗口独立维护。
/// 多角色场景下每个窗口排除自身捕获，哈希不同，若全局共享会导致 unchanged 优化失效。
static LAST_SCREEN_HASH: Lazy<Mutex<std::collections::HashMap<String, u64>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 角色窗口移动意图注册表：label → (x, y, w, h, 记录时刻)。
/// find_safe_position 选定目标后写入，后续调用在搜索时避开近期意图区域，
/// 避免两个角色窗口竞态下同时移动到同一位置。
static PET_MOVE_INTENTS: Lazy<
    Mutex<std::collections::HashMap<String, (i32, i32, i32, i32, std::time::Instant)>>,
> = Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 保护 WDA 设置→截图→恢复全过程的互斥锁。
/// 多个窗口同时调用 find_safe_position 时，如果不互斥，线程 A 刚设置完
/// WDA_EXCLUDEFROMCAPTURE、线程 B 可能在 A 截图前将其恢复为 WDA_NONE，
/// 导致截图中仍包含桌宠图像，干扰纯色区域检测。
static SCREEN_CAPTURE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// 轻量级预检缓存：记录上次成功截图时的前台窗口句柄和时间戳。
/// 在进入昂贵的全屏截图临界区之前，先比对前台窗口是否变化——如果前台
/// 应用完全没变，屏幕内容极大概率也没变，直接返回 unchanged，跳过截图
/// 和全部图像分析。GetForegroundWindow() 是几乎零开销的 Win32 调用。
static LAST_FOREGROUND_CHECK: Lazy<Mutex<Option<(isize, std::time::Instant)>>> =
    Lazy::new(|| Mutex::new(None));

/// 每个窗口上次成功移动的时间戳，用于移动冷却期。
/// 刚移动后短时间内（MOVE_COOLDOWN_MS）拒绝再次移动，防止乒乓跳动。
static LAST_MOVE_TIME: Lazy<Mutex<std::collections::HashMap<String, std::time::Instant>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 移动冷却期（毫秒）：窗口移动后在此时间内不再次触发移动。
/// 这是防止乒乓的第一道防线——即使评分算法认为需要移动，刚动完也先歇一会。
/// 设为 8 秒：足够长让视觉稳定，也不会让真正的内容变化等太久。
const MOVE_COOLDOWN_MS: u64 = 8_000;

/// 预检命中后，距上次截图至少需要等待的时间（毫秒）。
/// 前台窗口未变且在此时间内，跳过截图。force=true 时忽略此限制。
/// 设为1.5秒：两个错峰窗口的连续调用（间隔800ms）会被快速跳过，
/// 但下一次正常轮询（2.5s后）一定会执行截图，不会漏检内容变化。
const PRECHECK_SKIP_MS: u64 = 1_500;

/// 在像素缓冲区中擦除桌宠区域，用左右边缘像素的平均值填充。
///
/// 用于 `WDA_EXCLUDEFROMCAPTURE` 不可用时的 fallback：截图包含了桌宠本身，
/// 桌宠区域的非纯色图案会切断原本连通的纯色区域。此函数将桌宠矩形内部
/// 的像素替换为左右边缘外侧像素的平均值，近似桌宠背后的桌面内容，
/// 从而恢复纯色区域的连通性。
///
/// 每行独立处理：左边缘 + 右边缘 → 平均值填充整行。若某侧越界（桌宠贴边），
/// 则只用可用的一侧。
fn erase_pet_region(
    pixels: &mut [u8],
    cw: i32,
    ch: i32,
    pet_x: i32,
    pet_y: i32,
    pet_w: i32,
    pet_h: i32,
    vx: i32,
    vy: i32,
    downscale: i32,
) {
    // 桌宠矩形从物理屏幕坐标转降采样缓冲区坐标
    let px0 = ((pet_x - vx) / downscale).max(0).min(cw);
    let py0 = ((pet_y - vy) / downscale).max(0).min(ch);
    let px1 = ((pet_x - vx + pet_w) / downscale).max(0).min(cw);
    let py1 = ((pet_y - vy + pet_h) / downscale).max(0).min(ch);
    if px1 <= px0 || py1 <= py0 {
        return;
    }

    let stride = cw as usize * 4;
    for y in py0..py1 {
        let row_start = (y as usize) * stride;
        // 采样左边缘外侧像素（px0 - 1）
        let left: Option<[u8; 3]> = if px0 > 0 {
            let idx = row_start + ((px0 - 1) as usize) * 4;
            Some([pixels[idx], pixels[idx + 1], pixels[idx + 2]])
        } else {
            None
        };
        // 采样右边缘外侧像素（px1）
        let right: Option<[u8; 3]> = if px1 < cw {
            let idx = row_start + (px1 as usize) * 4;
            Some([pixels[idx], pixels[idx + 1], pixels[idx + 2]])
        } else {
            None
        };

        let fill = match (left, right) {
            (Some(l), Some(r)) => [
                ((l[0] as u16 + r[0] as u16) / 2) as u8,
                ((l[1] as u16 + r[1] as u16) / 2) as u8,
                ((l[2] as u16 + r[2] as u16) / 2) as u8,
            ],
            (Some(l), None) | (None, Some(l)) => l,
            (None, None) => continue, // 桌宠占满整行，无法采样
        };

        for x in px0..px1 {
            let idx = row_start + (x as usize) * 4;
            pixels[idx] = fill[0];
            pixels[idx + 1] = fill[1];
            pixels[idx + 2] = fill[2];
            // alpha 通道保持不变（32bpp BGRA，alpha 通常 255）
        }
    }
}

/// 捕获整个虚拟屏幕并分析图像信息量，为桌宠推荐最不遮挡内容的安放位置。
///
/// 三项性能优化：
/// 1. **降采样捕获**：用 `StretchBlt` 直接捕获到 1/4 分辨率位图，
///    内存与 CPU 开销降至 1/16。32px 块在降采样后对应物理屏幕 128px 区域，
///    对信息量评估精度无影响。
/// 2. **屏幕变化检测**：对降采样像素计算 FNV-1a 哈希，跨调用比对。
///    若哈希一致，直接返回 `{ unchanged: true }`，跳过边缘/方差全部分析。
///    空闲桌面场景下命中率 >95%。
/// 3. **空闲延长轮询**：前端收到 `unchanged: true` 后动态延长轮询间隔，
///    从 2.5s 逐步延长到 30s；一旦检测到变化立即恢复 2.5s。
///
/// 算法（信息量评估而非纯色检测）：
/// 1. `StretchBlt` 捕获虚拟屏幕到 1/4 分辨率 32bpp BGRA 缓冲区
/// 2. FNV-1a 哈希比对 —— 一致则返回 `{ unchanged: true }`
/// 3. 提取亮度通道（Y = 0.299R + 0.587G + 0.114B）
/// 4. 计算 Sobel 边缘强度图（L1 范数，避免 sqrt）
/// 5. 对亮度与边缘强度构建积分图（平铺数组，O(1) 区域查询）
/// 6. 计算桌宠当前足迹区域信息量分数 `current_score`：
///    - 分数低于 `SCORE_GOOD_ENOUGH` 直接返回 null（已足够安静）
/// 7. 滑动窗口搜索全屏，对每个候选区域：
///    - 跳过与桌宠当前重叠的位置
///    - 计算 (var, edge_density) → score
///    - 仅考虑 score < current_score 的候选
///    - 综合排序键 = score + 距屏幕中心距离 * 0.05（轻微偏好边缘）
/// 8. 若最优候选 score 明显优于当前（< current_score * 0.75），返回其物理坐标
///
/// 关键设计：
/// - **足迹口径**：Live2D 模型只占窗口中央 1/3 宽度（左右两侧全透明），
///   评分、候选搜索与其他桌宠避让矩形都收窄到中央 1/3，而非整个窗口
/// - **边缘密度为主**：文字、UI 控件、图标都产生强边缘；纯色背景、白墙几乎无边缘
/// - **方差为辅**：捕捉渐变/纹理等低边缘但非纯色的情况
/// - **避免扎堆角落**：用距屏幕中心距离的轻微权重替代曼哈顿距离最近优先
fn score_region(var: f64, edge_density: f64) -> f64 {
    // 归一化方差：log 压缩，把 0-2000+ 映射到 0-~10 的稳定区间
    let var_norm = if var <= 1.0 {
        0.0
    } else {
        (var.ln() * 2.0).min(20.0)
    };
    // 边缘密度直接使用（0-255）；权重 0.6 > 方差 0.4
    edge_density * 0.6 + var_norm * 0.4
}

#[tauri::command]
pub fn find_safe_position(
    window: tauri::WebviewWindow,
    state: State<'_, std::sync::Arc<crate::state::AppState>>,
    pet_x: i32,
    pet_y: i32,
    pet_w: i32,
    pet_h: i32,
    force: Option<bool>,
) -> Result<Value, String> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
            GetDIBits, ReleaseDC, SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO,
            BITMAPINFOHEADER, COLORONCOLOR, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetSystemMetrics, SetWindowDisplayAffinity, SM_CXVIRTUALSCREEN,
            SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
            WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
        };

        // 降采样倍数：5 = 1/5 分辨率捕获，内存/CPU 降至 1/25
        const DOWNSCALE: i32 = 5;
        // 滑动窗口大小（降采样缓冲区的像素）。对应物理屏幕 = WINDOW * DOWNSCALE = 160px
        const WINDOW: i32 = 32;
        // 滑动步长（降采样缓冲区的像素）= 物理100px，大幅减少搜索点数
        const STEP: i32 = 20;
        // WDA 设置后等待系统生效的时间（ms）
        const WDA_WAIT_MS: u64 = 15;

        // Live2D 模型只占据窗口中央 1/3 宽度，左右两侧全透明、不产生遮挡。
        // 把前端传来的整窗矩形收窄为模型实际足迹，评分与候选搜索只覆盖
        // 角色真正遮挡的区域。
        let pet_x = pet_x + pet_w / 3;
        let pet_w = (pet_w / 3).max(1);

        unsafe {
            // 虚拟屏幕范围（多显示器时原点可能为负）
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            if vw <= 0 || vh <= 0 {
                return Ok(json!({ "unchanged": false, "region": null }));
            }

            // ── 零成本预检：前台窗口句柄未变 + 非强制 + 间隔较短 → 跳过截图 ──
            // GetForegroundWindow() 是极轻量的 Win32 调用，避免 90%+ 无意义的全屏截图。
            if !force.unwrap_or(false) {
                let fg_hwnd = GetForegroundWindow();
                let fg_val = fg_hwnd.0 as isize;
                {
                    let mut pre = LAST_FOREGROUND_CHECK.lock();
                    if let Some((last_fg, last_at)) = *pre {
                        if last_fg == fg_val && last_at.elapsed().as_millis() < PRECHECK_SKIP_MS as u128 {
                            return Ok(json!({ "unchanged": true, "region": null }));
                        }
                    }
                    *pre = Some((fg_val, std::time::Instant::now()));
                }
            }

            // 降采样后的捕获尺寸（向上取整，避免边缘丢失）
            let cw = (vw + DOWNSCALE - 1) / DOWNSCALE;
            let ch = (vh + DOWNSCALE - 1) / DOWNSCALE;

            // ── 受 SCREEN_CAPTURE_LOCK 保护的临界区：收集窗口→设置WDA→截图→恢复WDA ──
            // 确保同一时间只有一个线程在操作WDA状态和截图，防止并发互相干扰。
            //
            // 关键设计：对所有桌宠窗口（包括自己）设置 WDA_EXCLUDEFROMCAPTURE，
            // 这样截图中完全看不到任何桌宠图像，所有像素都是真实的桌面背景。
            // 这避免了"自我擦除导致当前位置评分被污染"的乒乓效应——
            // 如果用erase_pet_region填充边缘像素，当前位置会被伪造成高边缘密度，
            // 候选位置却看到真实背景，评分不公，导致在两个位置间来回跳动。
            let capture_result = {
                let _capture_guard = SCREEN_CAPTURE_LOCK.lock();

                let app = window.app_handle();
                let self_label = window.label().to_string();

                let self_hwnd = HWND(window.hwnd().map_err(err_str)?.0);

                // 收集所有桌宠窗口（包括自己）
                let all_pet_windows: Vec<(HWND, i32, i32, i32, i32)> = {
                    let characters_guard = state.characters.read();
                    let mut wins = Vec::new();
                    for (label, w) in app.webview_windows() {
                        if !characters_guard.contains_key(&label) {
                            continue;
                        }
                        let Ok(pos) = w.outer_position() else { continue };
                        let Ok(size) = w.outer_size() else { continue };
                        if size.width == 0 || size.height == 0 {
                            continue;
                        }
                        let hwnd = match w.hwnd() {
                            Ok(h) => HWND(h.0),
                            Err(_) => continue,
                        };
                        wins.push((hwnd, pos.x, pos.y, size.width as i32, size.height as i32));
                    }
                    wins
                };

                // 对所有桌宠窗口设置WDA（包括自己），让截图中排除所有桌宠
                let mut affinity_success: Vec<HWND> = Vec::new();
                let mut all_affinity_ok = true;
                for &(hwnd, _, _, _, _) in &all_pet_windows {
                    if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok() {
                        affinity_success.push(hwnd);
                    } else {
                        all_affinity_ok = false;
                        break;
                    }
                }
                if !all_affinity_ok {
                    for hwnd in &affinity_success {
                        let _ = SetWindowDisplayAffinity(*hwnd, WDA_NONE);
                    }
                    affinity_success.clear();
                }
                if !affinity_success.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(WDA_WAIT_MS));
                }

                let rollback_affinity = |hwnds: &[HWND]| {
                    for hwnd in hwnds {
                        let _ = SetWindowDisplayAffinity(*hwnd, WDA_NONE);
                    }
                };

                let hdc_screen = GetDC(None);
                if hdc_screen.is_invalid() {
                    rollback_affinity(&affinity_success);
                    return Ok(json!({ "unchanged": false, "region": null }));
                }
                let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
                if hdc_mem.is_invalid() {
                    ReleaseDC(None, hdc_screen);
                    rollback_affinity(&affinity_success);
                    return Ok(json!({ "unchanged": false, "region": null }));
                }
                let hbitmap = CreateCompatibleBitmap(hdc_screen, cw, ch);
                if hbitmap.is_invalid() {
                    let _ = DeleteDC(hdc_mem);
                    ReleaseDC(None, hdc_screen);
                    rollback_affinity(&affinity_success);
                    return Ok(json!({ "unchanged": false, "region": null }));
                }
                let old_obj = SelectObject(hdc_mem, HGDIOBJ(hbitmap.0));
                let _ = SetStretchBltMode(hdc_mem, COLORONCOLOR);
                let blit_ok = StretchBlt(
                    hdc_mem, 0, 0, cw, ch, Some(hdc_screen), vx, vy, vw, vh, SRCCOPY,
                )
                .as_bool();

                let mut bi: BITMAPINFO = std::mem::zeroed();
                bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bi.bmiHeader.biWidth = cw;
                bi.bmiHeader.biHeight = -ch;
                bi.bmiHeader.biPlanes = 1;
                bi.bmiHeader.biBitCount = 32;
                bi.bmiHeader.biCompression = 0;

                let mut pixels = vec![0u8; (cw * ch * 4) as usize];
                let got = GetDIBits(
                    hdc_mem, hbitmap, 0, ch as u32,
                    Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
                    &mut bi, DIB_RGB_COLORS,
                );

                SelectObject(hdc_mem, old_obj);
                let _ = DeleteObject(HGDIOBJ(hbitmap.0));
                let _ = DeleteDC(hdc_mem);
                ReleaseDC(None, hdc_screen);

                rollback_affinity(&affinity_success);

                if !blit_ok || got == 0 {
                    return Ok(json!({ "unchanged": false, "region": null }));
                }

                // 收集其他桌宠窗口信息（用于避让区域），排除自己。
                // 与自己一致，只取中央 1/3 宽度足迹，两侧透明区不参与避让。
                let mut other_pet_windows: Vec<(i32, i32, i32, i32)> = all_pet_windows
                    .iter()
                    .filter(|(hwnd, _, _, _, _)| *hwnd != self_hwnd)
                    .map(|&(_, px, py, pw, ph)| (px + pw / 3, py, (pw / 3).max(1), ph))
                    .collect();

                // 追加 ChatWindow（微信聊天窗口）和 SideChatPanel（侧边聊天面板）
                // 的完整矩形作为避让区域，防止桌宠移动到这些窗口上方遮挡用户视图。
                // 桌宠窗口用中央 1/3 足迹，但这些聊天窗口整体都有可见内容，用完整矩形。
                for win_label in ["chat", "side_chat"] {
                    if let Some(ui_win) = app.get_webview_window(win_label) {
                        if let (Ok(pos), Ok(size)) = (ui_win.outer_position(), ui_win.outer_size()) {
                            if size.width > 0 && size.height > 0 {
                                other_pet_windows.push((
                                    pos.x,
                                    pos.y,
                                    size.width as i32,
                                    size.height as i32,
                                ));
                            }
                        }
                    }
                }

                // WDA失败时需要erase回退（但这种情况下评分可能不公平，
                // 因此提高移动阈值避免乒乓）
                let need_erase = !all_affinity_ok;

                (pixels, other_pet_windows, need_erase, self_label, self_hwnd)
            };

            let (mut pixels, other_pet_windows, need_erase, self_label, _self_hwnd) = capture_result;

            // WDA设置成功时截图中已排除所有桌宠（包括自己），像素是真实背景。
            // WDA失败时需要手动擦除所有桌宠区域（作为回退）。
            if need_erase {
                // 擦除自己
                erase_pet_region(
                    &mut pixels, cw, ch, pet_x, pet_y, pet_w, pet_h, vx, vy, DOWNSCALE,
                );
                // 擦除其他桌宠窗口
                for &(px, py, pw, ph) in &other_pet_windows {
                    erase_pet_region(
                        &mut pixels, cw, ch, px, py, pw, ph, vx, vy, DOWNSCALE,
                    );
                }
            }

            // ── 优化 2：屏幕变化检测（FNV-1a 哈希，按窗口 label 分桶） ──
            let current_hash = fnv1a_64_bytes(&pixels);
            let mut hash_guard = LAST_SCREEN_HASH.lock();
            let unchanged = match hash_guard.get(&self_label) {
                Some(&last) if last == current_hash => true,
                _ => {
                    hash_guard.insert(self_label.clone(), current_hash);
                    false
                }
            };
            drop(hash_guard);

            if unchanged && !force.unwrap_or(false) {
                // 屏幕无变化，跳过全部分析。前端据此延长轮询间隔。
                return Ok(json!({ "unchanged": true, "region": null }));
            }

            if cw < WINDOW || ch < WINDOW {
                return Ok(json!({ "unchanged": false, "region": null }));
            }

            let cw_us = cw as usize;
            let ch_us = ch as usize;

            // ── 1. 提取亮度通道并计算 Sobel 边缘强度图 ──
            // 亮度 Y = 0.299R + 0.587G + 0.114B；用 u16 中间值避免 u8 溢出
            let mut lum = vec![0u8; cw_us * ch_us];
            for y in 0..ch_us {
                for x in 0..cw_us {
                    let idx = (y * cw_us + x) * 4;
                    let r = pixels[idx] as u32;
                    let g = pixels[idx + 1] as u32;
                    let b = pixels[idx + 2] as u32;
                    // 等价于 (77*r + 150*g + 29*b) >> 8
                    lum[y * cw_us + x] = ((77 * r + 150 * g + 29 * b) >> 8) as u8;
                }
            }

            // Sobel 边缘强度（无符号，0-255 范围内饱和）
            // gx/gy 使用 i16 存储；最终 edge[i] = |gx|+|gy|（L1 范数，避免 sqrt）
            let mut edge = vec![0u16; cw_us * ch_us];
            for y in 1..ch as i32 - 1 {
                for x in 1..cw as i32 - 1 {
                    let at = |xx: i32, yy: i32| -> i32 {
                        lum[(yy as usize) * cw_us + (xx as usize)] as i32
                    };
                    let gx = -at(x - 1, y - 1) + at(x + 1, y - 1)
                        - 2 * at(x - 1, y) + 2 * at(x + 1, y)
                        - at(x - 1, y + 1) + at(x + 1, y + 1);
                    let gy = -at(x - 1, y - 1) - 2 * at(x, y - 1) - at(x + 1, y - 1)
                        + at(x - 1, y + 1) + 2 * at(x, y + 1) + at(x + 1, y + 1);
                    let mag = (gx.abs() + gy.abs()).min(255) as u16;
                    edge[(y as usize) * cw_us + (x as usize)] = mag;
                }
            }

            // ── 2. 对亮度与边缘强度构建积分图（平铺数组，缓存友好） ──
            // sum[i][j] 表示从 (0,0) 到 (i-1,j-1) 的累加和；维度 (ch+1) x (cw+1)
            let iw = cw_us + 1;
            let ih = ch_us + 1;
            // 亮度一阶/二阶积分（用于方差）
            let mut sum_l = vec![0f64; iw * ih];
            let mut sum_l2 = vec![0f64; iw * ih];
            // 边缘强度积分（用于区域边缘密度）
            let mut sum_e = vec![0f64; iw * ih];
            // 亮度均值积分（用于纯色检测 —— 区域内亮度极差）
            // 不需要二阶积分即可得到方差，已包含在 sum_l2 中

            for y in 0..ch_us {
                for x in 0..cw_us {
                    let l = lum[y * cw_us + x] as f64;
                    let e = edge[y * cw_us + x] as f64;
                    let i = y + 1;
                    let j = x + 1;
                    sum_l[i * iw + j] = l + sum_l[(i - 1) * iw + j] + sum_l[i * iw + j - 1]
                        - sum_l[(i - 1) * iw + j - 1];
                    sum_l2[i * iw + j] = l * l + sum_l2[(i - 1) * iw + j]
                        + sum_l2[i * iw + j - 1] - sum_l2[(i - 1) * iw + j - 1];
                    sum_e[i * iw + j] = e + sum_e[(i - 1) * iw + j] + sum_e[i * iw + j - 1]
                        - sum_e[(i - 1) * iw + j - 1];
                }
            }

            // 矩形区域查询：返回 (亮度方差, 边缘密度均值)
            let rect_stats = |x0: i32, y0: i32, w: i32, h: i32| -> Option<(f64, f64)> {
                let x0 = x0.max(0) as usize;
                let y0 = y0.max(0) as usize;
                let x1 = (x0 + w as usize).min(cw_us);
                let y1 = (y0 + h as usize).min(ch_us);
                if x1 <= x0 || y1 <= y0 {
                    return None;
                }
                let count = ((x1 - x0) * (y1 - y0)) as f64;
                if count < 1.0 {
                    return None;
                }
                let i0 = y0;
                let j0 = x0;
                let i1 = y1;
                let j1 = x1;
                let sl = sum_l[i1 * iw + j1] - sum_l[i0 * iw + j1] - sum_l[i1 * iw + j0]
                    + sum_l[i0 * iw + j0];
                let sl2 = sum_l2[i1 * iw + j1] - sum_l2[i0 * iw + j1] - sum_l2[i1 * iw + j0]
                    + sum_l2[i0 * iw + j0];
                let se = sum_e[i1 * iw + j1] - sum_e[i0 * iw + j1] - sum_e[i1 * iw + j0]
                    + sum_e[i0 * iw + j0];
                let mean_l = sl / count;
                let var_l = (sl2 / count - mean_l * mean_l).max(0.0);
                let edge_density = se / count;
                Some((var_l, edge_density))
            };

            // ── 3. 检查桌宠当前区域信息量 ──
            let p_local_x = (pet_x - vx) / DOWNSCALE;
            let p_local_y = (pet_y - vy) / DOWNSCALE;
            let p_w = (pet_w + DOWNSCALE - 1) / DOWNSCALE;
            let p_h = (pet_h + DOWNSCALE - 1) / DOWNSCALE;
            let (current_var, current_edge) = rect_stats(p_local_x, p_local_y, p_w, p_h)
                .unwrap_or((f64::MAX, f64::MAX));
            // 复合信息量分数：边缘密度为主，方差为辅
            // edge_density 范围 0-255，var 通常 0-2000+；都归一化后加权
            let current_score = score_region(current_var, current_edge);
            const SCORE_GOOD_ENOUGH: f64 = 8.0;
            if current_score < SCORE_GOOD_ENOUGH {
                return Ok(json!({ "unchanged": false, "region": null }));
            }

            // ── 4. 滑动窗口搜索最低信息量区域 ──
            let win_w = p_w.max(WINDOW);
            let win_h = p_h.max(WINDOW);
            let step = STEP;

            let screen_cx = cw as f64 / 2.0;
            let screen_cy = ch as f64 / 2.0;
            let half_diag = ((screen_cx * screen_cx + screen_cy * screen_cy)).sqrt().max(1.0);
            const EDGE_PREFERENCE_WEIGHT: f64 = 8.0;

            // 复用截图前已收集的其他桌宠窗口物理坐标，转换为降采样坐标作为避让矩形。
            // other_pet_windows 已经排除了自己，无需再次过滤。
            let mut other_pet_rects: Vec<(i32, i32, i32, i32)> = other_pet_windows
                .iter()
                .map(|&(px, py, pw, ph)| {
                    let ox = (px - vx) / DOWNSCALE;
                    let oy = (py - vy) / DOWNSCALE;
                    let ow = (pw + DOWNSCALE - 1) / DOWNSCALE;
                    let oh = (ph + DOWNSCALE - 1) / DOWNSCALE;
                    (ox, oy, ow, oh)
                })
                .collect();

            // ── 反乒乓机制 ──
            //
            // 机制 1：移动冷却期。刚移动完的窗口在 MOVE_COOLDOWN_MS 内拒绝再次移动。
            // 这直接切断了乒乓环路：A→B 后，在冷却期内不会 B→A。
            let is_in_cooldown = {
                let move_times = LAST_MOVE_TIME.lock();
                move_times
                    .get(&self_label)
                    .map(|t| t.elapsed().as_millis() < MOVE_COOLDOWN_MS as u128)
                    .unwrap_or(false)
            };

            // 机制 2：驻留偏好（Homing Bias）。
            // 把当前位置的评分人为打 9 折（乘以 HOMING_BIAS），让"待在原地"
            // 在比较时看起来比实际略好。这打破了评分对称性：
            // 在位置 A 时 A 的有效分 = score_A * 0.9，B 是 score_B，B 要赢需要 score_B < score_A * 0.9 * threshold；
            // 一旦到了 B，B 的有效分 = score_B * 0.9，A 变成 score_A，此时 A 要赢回去需要 score_A < score_B * 0.9 * threshold。
            // 当 score_A ≈ score_B 时，两边都无法赢对方，自然就稳定了。
            const HOMING_BIAS: f64 = 0.90;

            // 机制 3：更高的移动阈值 + 绝对分差门槛。
            // 相对改善要求从 15% 提高到 35%（WDA 成功）/ 55%（WDA 失败回退），
            // 同时要求绝对分差 >= MIN_ABSOLUTE_IMPROVEMENT，
            // 两者都满足才移动，避免评分微小差异触发无意义移动。
            let move_threshold = if need_erase { 0.45 } else { 0.65 };
            const MIN_ABSOLUTE_IMPROVEMENT: f64 = 2.0;

            // 应用驻留偏好：当前位置的有效评分更低（更好）
            let effective_current_score = current_score * HOMING_BIAS;

            // 冷却期内直接不搜索、不移动，保持原位
            let best: Option<(i32, i32, f64, f64)> = if is_in_cooldown && !force.unwrap_or(false) {
                None
            } else {
                let mut intents = PET_MOVE_INTENTS.lock();
                let now = std::time::Instant::now();

                intents.retain(|lbl, (_, _, _, _, at)| {
                    if lbl == &self_label {
                        false
                    } else {
                        now.duration_since(*at).as_millis() <= 2000
                    }
                });
                for (_, (ix, iy, iw, ih, _)) in intents.iter() {
                    let ox = (*ix - vx) / DOWNSCALE;
                    let oy = (*iy - vy) / DOWNSCALE;
                    let ow = (*iw + DOWNSCALE - 1) / DOWNSCALE;
                    let oh = (*ih + DOWNSCALE - 1) / DOWNSCALE;
                    other_pet_rects.push((ox, oy, ow, oh));
                }

                let mut b: Option<(i32, i32, f64, f64)> = None;
                let mut y = 0;
                while y + win_h <= ch {
                    let mut x = 0;
                    while x + win_w <= cw {
                        let overlap_x = x < p_local_x + p_w && x + win_w > p_local_x;
                        let overlap_y = y < p_local_y + p_h && y + win_h > p_local_y;
                        if overlap_x && overlap_y {
                            x += step;
                            continue;
                        }
                        let overlaps_other = other_pet_rects.iter().any(|(ox, oy, ow, oh)| {
                            x < *ox + *ow && x + win_w > *ox && y < *oy + *oh && y + win_h > *oy
                        });
                        if overlaps_other {
                            x += step;
                            continue;
                        }
                        if let Some((var, edge)) = rect_stats(x, y, win_w, win_h) {
                            let score = score_region(var, edge);
                            if score < effective_current_score {
                                let region_cx = (x + win_w / 2) as f64;
                                let region_cy = (y + win_h / 2) as f64;
                                let dx = region_cx - screen_cx;
                                let dy = region_cy - screen_cy;
                                let norm_dist = ((dx * dx + dy * dy).sqrt() / half_diag).min(1.0);
                                let combined = score - norm_dist * EDGE_PREFERENCE_WEIGHT;
                                let is_better = match b {
                                    None => true,
                                    Some((_, _, _, bc)) => combined < bc,
                                };
                                if is_better {
                                    b = Some((x, y, score, combined));
                                }
                            }
                        }
                        x += step;
                    }
                    y += step;
                }

                // 双重门槛检查（在 intents 锁内完成，确保原子性）：
                // 1. 相对改善：best_score < current_score * move_threshold
                // 2. 绝对改善：current_score - best_score >= MIN_ABSOLUTE_IMPROVEMENT
                // 两者都满足才注册移动意图，否则丢弃结果（返回 None）
                if let Some((bx, by, best_score, _)) = &b {
                    let relative_ok = *best_score < current_score * move_threshold;
                    let absolute_ok = (current_score - best_score) >= MIN_ABSOLUTE_IMPROVEMENT;
                    if relative_ok && absolute_ok {
                        intents.insert(
                            self_label.clone(),
                            (bx * DOWNSCALE + vx, by * DOWNSCALE + vy, win_w * DOWNSCALE, win_h * DOWNSCALE, now),
                        );
                    } else {
                        b = None;
                    }
                }

                b
            };

            // ── 5. 选定目标位置 ──
            if let Some((bx, by, _best_score, _)) = best {
                let phys_x = bx * DOWNSCALE + vx;
                let phys_y = by * DOWNSCALE + vy;
                let phys_w = win_w * DOWNSCALE;
                let phys_h = win_h * DOWNSCALE;

                // 记录本次移动时间戳，启动冷却期
                {
                    let mut move_times = LAST_MOVE_TIME.lock();
                    move_times.insert(self_label.clone(), std::time::Instant::now());
                }

                return Ok(json!({
                    "unchanged": false,
                    "region": {
                        "x": phys_x,
                        "y": phys_y,
                        "width": phys_w,
                        "height": phys_h,
                    }
                }));
            }
            Ok(json!({ "unchanged": false, "region": null }))
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (window, state, pet_x, pet_y, pet_w, pet_h, force);
        Ok(json!({ "unchanged": false, "region": null }))
    }
}
