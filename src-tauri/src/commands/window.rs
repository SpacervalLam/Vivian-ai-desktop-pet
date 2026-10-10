//! 窗口命令 - 窗口位置、尺寸、透明度、可见性、置顶与多窗口管理
//!
//! 已有的三个命令（`set_window_position` / `get_window_position` / `toggle_always_on_top`）
//! 签名保持不变，新增命令为增量补全。

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::utils::fnv1a_64_bytes;
use crate::drag_motion::{release_velocity, FLING_SAMPLE_WINDOW_MS};
#[cfg(test)]
use crate::drag_motion::FLING_MAX_VELOCITY;

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ============ WebView 冻结（隐藏窗口渲染进程挂起省内存） ============
//
// 隐藏窗口用 WebView2 TrySuspend 冻结渲染进程；chat 关闭即销毁。
// edge_menu 常驻屏外并保持渲染，避免边缘呼出时恢复合成造成闪烁。
// 边缘触发由原生线程承担，WebView 冻结不影响功能；仅隐藏期间发往前端的
// 事件投递会丢失，由前端在 visibilitychange（恢复+show 触发）时刷新消息兜底。
//
// 时序契约：suspend 在 hide() 之后调用（即发即忘），resume 在 show() 之前
// 调用（阻塞等待，~100-300ms，被 220ms 滑入动画与用户反应时间掩盖）。
// 代计数器防止快速 hide→show 时迟到的 TrySuspend 冻结已重新显示的窗口。

/// 每个窗口独立的代号和失败退避，避免一个窗口解冻取消另一个窗口的冻结。
#[derive(Default)]
struct WebviewFreezeState {
    generation: AtomicU32,
    desired_frozen: AtomicBool,
    retry_after: Mutex<Option<Instant>>,
}
static WEBVIEW_FREEZE_STATES: Lazy<Mutex<std::collections::HashMap<String, Arc<WebviewFreezeState>>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

fn webview_freeze_state(label: &str) -> Arc<WebviewFreezeState> {
    WEBVIEW_FREEZE_STATES.lock().entry(label.to_string()).or_default().clone()
}

pub(crate) fn freeze_webview(win: &WebviewWindow) {
    #[cfg(windows)]
    {
        let state = webview_freeze_state(win.label());
        state.desired_frozen.store(true, Ordering::SeqCst);
        let generation = state.generation.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        if state.retry_after.lock().is_some_and(|deadline| Instant::now() < deadline) { return; }
        let label = win.label().to_string();
        let _ = win.with_webview(move |wv| unsafe {
            if state.generation.load(Ordering::SeqCst) != generation { return; }
            webview_freeze_op(&wv, true, state, generation, label);
        });
    }
    #[cfg(not(windows))]
    let _ = win;
}

pub(crate) fn thaw_webview(win: &WebviewWindow) {
    #[cfg(windows)]
    {
        let state = webview_freeze_state(win.label());
        state.desired_frozen.store(false, Ordering::SeqCst);
        let generation = state.generation.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        let label = win.label().to_string();
        let _ = win.with_webview(move |wv| unsafe {
            webview_freeze_op(&wv, false, state, generation, label);
        });
    }
    #[cfg(not(windows))]
    let _ = win;
}

fn suspend_failed(state: &WebviewFreezeState, label: &str, detail: impl std::fmt::Display) {
    let mut retry_after = state.retry_after.lock();
    if retry_after.is_none_or(|deadline| Instant::now() >= deadline) {
        tracing::debug!("[webview_freezer:{label}] suspend 未成功: {detail}；60 秒后再尝试");
    }
    *retry_after = Some(Instant::now() + Duration::from_secs(60));
}

#[cfg(windows)]
unsafe fn webview_freeze_op(
    wv: &tauri::webview::PlatformWebview,
    freeze: bool,
    state: Arc<WebviewFreezeState>,
    generation: u32,
    label: String,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use webview2_com::TrySuspendCompletedHandler;
    use windows_core::Interface;

    let controller = wv.controller();
    let Ok(core) = controller.CoreWebView2() else { return; };
    let Ok(wv3) = core.cast::<ICoreWebView2_3>() else {
        if freeze { suspend_failed(&state, &label, "ICoreWebView2_3 不支持"); }
        return;
    };
    if !freeze {
        if let Err(error) = wv3.Resume() {
            tracing::debug!("[webview_freezer:{label}] resume 失败: {error}");
        }
        if let Err(error) = controller.SetIsVisible(true) {
            tracing::warn!("[webview_freezer:{label}] 恢复 WebView 可见性失败: {error}");
        }
        return;
    }
    // hide() 隐藏的是原生窗口；TrySuspend 要求 WebView2 控制器自身也不可见。
    // https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2.trysuspendasync
    if let Err(error) = controller.SetIsVisible(false) {
        suspend_failed(&state, &label, error);
        return;
    }
    let callback_state = state.clone();
    let callback_label = label.clone();
    let callback_webview = wv3.clone();
    let result = wv3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(move |error, success| {
        let current = callback_state.generation.load(Ordering::SeqCst) == generation;
        if error.is_err() || !success {
            if current {
                suspend_failed(&callback_state, &callback_label, format!("HRESULT={error:?}, success={success}"));
            }
        } else if !callback_state.desired_frozen.load(Ordering::SeqCst) {
            // 已请求显示时撤销迟到挂起；新的 hide 请求仍期望冻结时不要误解冻。
            let _ = callback_webview.Resume();
        } else if current {
            *callback_state.retry_after.lock() = None;
        }
        Ok(())
    })));
    if let Err(error) = result { suspend_failed(&state, &label, error); }
}

#[cfg(test)]
mod freeze_state_tests {
    #[test]
    fn resume_invalidates_only_its_own_window() {
        use super::*;
        let a = webview_freeze_state("test_chat");
        let b = webview_freeze_state("test_banner");
        let request = a.generation.fetch_add(1, Ordering::SeqCst) + 1;
        b.generation.fetch_add(1, Ordering::SeqCst);
        assert_eq!(a.generation.load(Ordering::SeqCst), request);
        a.generation.fetch_add(1, Ordering::SeqCst);
        assert_ne!(a.generation.load(Ordering::SeqCst), request);
        suspend_failed(&a, "test_chat", "test failure");
        assert!(a.retry_after.lock().is_some_and(|deadline| deadline > Instant::now()));
        assert!(b.retry_after.lock().is_none());
    }
}

// ============ 消息横幅窗口（低频窗口，空闲即冻结） ============
//
// message_banner 只在收到微信类消息时短暂显示，其余时间一直是隐藏的
// WebView，却常驻一个完整的渲染进程（约 50-120MB）。空闲时把它冻结，
// 与 chat / 聊天窗口 同等处理。
//
// 冻结期间 JS 挂起、投递的事件会丢失，所以所有发送点统一走
// [`emit_message_banner`]：先解冻再 emit，避免每个调用方各自处理时序。

/// 消息横幅窗口的 label
pub(crate) const MESSAGE_BANNER_LABEL: &str = "message_banner";

/// 发送消息横幅事件（先确保窗口已解冻，再投递）
pub(crate) fn emit_message_banner(app: &AppHandle, payload: Value) {
    if let Some(win) = app.get_webview_window(MESSAGE_BANNER_LABEL) {
        thaw_webview(&win);
    }
    let _ = app.emit("wechat:message_banner", payload);
}

/// 冻结指定 label 窗口的 WebView（仅隐藏窗口生效，省渲染进程内存）
///
/// 由前端在横幅全部消失后调用；窗口不存在或仍可见时是无操作。
#[tauri::command]
pub fn freeze_window_webview(app: AppHandle, label: String) -> Result<(), String> {
    let Some(win) = app.get_webview_window(&label) else {
        return Err(format!("窗口不存在: {label}"));
    };
    freeze_webview(&win);
    Ok(())
}

/// 窗口当前是否为系统前台窗口。
///
/// 两个来源都查，任一为 true 即视为前台，避免单个来源在透明/无边框窗口上
/// 误判 false（房间的 ESC 看护与心智观察器的注意力判定都依赖它，误判 false
/// 分别会让 ESC 失灵、让陪伴角色误以为用户没在看而多嘴）。
///
/// Windows 下先比对 GetForegroundWindow：它是纯本地调用，而 win.is_focused()
/// 每次都要往主线程发一条消息再阻塞等回执——20ms 一轮的轮询里，房间不在前台时
/// 那就是每秒 50 次无谓的主线程往返。
pub(crate) fn is_window_foreground(win: &WebviewWindow) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        if let Ok(h) = win.hwnd() {
            if unsafe { GetForegroundWindow().0 == h.0 } {
                return true;
            }
        }
    }
    win.is_focused().unwrap_or(false)
}

// ============ 全屏光标追踪 ============
// 由 Rust 原生线程定时获取全局光标位置（同线程负责窗口拖动与拖拽物理）。
// 本线程不再向前端推送 `cursor:position`：鼠标跟随已改由前端 pointermove 实现
// （WebView2 在窗口失焦/不可见时会节流前端定时器，但 pointermove 由输入事件驱动、
// 不受节流影响），后端坐标事件全仓无人消费，已删除。
//
// 注意：桌宠窗口**不做**点击穿透，整窗响应鼠标。历史上本线程会按光标是否落在
// 窗口中心矩形内反复调用 set_ignore_cursor_events，而该调用在 Windows 上会
// 改写 GWL_EXSTYLE 并触发 SetWindowPos(SWP_FRAMECHANGED)，使透明窗口整块重绘，
// 表现为桌宠持续闪烁。该逻辑已整体移除。

/// 按角色隔离的光标追踪线程：character_id → (停止标志, 线程句柄)
/// 每个角色窗口拥有独立的追踪线程，互不干扰
static CURSOR_TRACKING_THREADS: Lazy<Mutex<std::collections::HashMap<String, (Arc<AtomicBool>, JoinHandle<()>)>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

// ⚠️ 本子系统里所有事件都必须用 `emit_to(&label, ...)`，不能用 `win.emit(...)`。
//
// `WebviewWindow::emit` 走的是 `Manager::emit`，语义是**全量广播给所有 webview**
// （见 tauri `Emitter` trait 默认实现），不是"发给这个窗口"。桌宠每个角色一个窗口、
// 各自跑一份 App.tsx，所以广播会让两只桌宠同时响应同一件事——实际表现是拖 A 触发晕眩时
// B 也一起晕。`emit_to` 传 label 走 `Manager::emit_to`，按 `AnyLabel` 匹配前端注册的
// `WebviewWindow { label }` 监听器（`getCurrentWindow().listen()` 正是这种注册），
// 只投给本角色窗口。label 就是 character_id（`get_webview_window(&char_id)` 取窗口）。

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

/// 先快照拖动偏移并释放锁，再调用窗口移动接口。
/// Windows 的跨线程 SetWindowPos 会等待窗口线程处理消息；如果此时窗口线程
/// 正在执行 stop_window_drag 并等待 DRAG_OFFSET，就会互相等待，连 toast 的
/// 窗口命中轮询也会阻塞。不能把 lock() 写进 if let 的匹配表达式中：
/// Rust 2021 会让那个临时锁一直存活到整个 if let 结束。
fn update_drag_position(label: &str, cursor_x: i32, cursor_y: i32, move_to: impl FnOnce(i32, i32)) {
    let offset = { DRAG_OFFSET.lock().get(label).copied() };
    if let Some((offset_x, offset_y)) = offset {
        move_to(cursor_x - offset_x, cursor_y - offset_y);
    }
}

/// 查询左键当前物理按下状态（不依赖事件投递）
///
/// 用于拖动 watchdog：当窗口追逐延迟导致 mouseup 无法到达 WebView 时，
/// 前端无法感知拖动已结束，只有硬件状态能作为最终裁决。
#[cfg(windows)]
pub(crate) fn is_left_mouse_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 != 0 }
}

#[cfg(not(windows))]
pub(crate) fn is_left_mouse_button_down() -> bool {
    false
}

/// ESC 键当前是否按下（边缘检测线程轮询用，配合下降沿避免重复触发）
#[cfg(windows)]
pub(crate) fn is_escape_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
    unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000 != 0 }
}

#[cfg(not(windows))]
pub(crate) fn is_escape_down() -> bool {
    false
}

/// 启动指定角色的光标追踪线程（每 ~60ms 一帧）。
///
/// 每个角色窗口拥有独立的追踪线程，职责：
/// - 窗口拖动（DRAG_OFFSET 驱动 SetWindowPos）
/// - 拖拽速度采样与「拖太快/撞边 → 晕乎乎」判定
/// - 松手时按光标轨迹触发惯性甩飞
///
/// 桌宠窗口不做点击穿透：整窗响应鼠标，因此本线程不再改写窗口样式。
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

        // 拖拽期间的光标轨迹采样（时刻, x, y），松手时计算惯性甩飞初速度
        let mut drag_samples: std::collections::VecDeque<(std::time::Instant, f64, f64)> =
            std::collections::VecDeque::new();
        // 上一帧是否处于拖拽状态，用于检测拖拽结束的瞬间
        let mut prev_is_dragging = false;
        let mut drag_observation: Option<(Instant, f64, f64, f64, f64)> = None;
        // 上次发出「拖太快 → 晕乎乎」事件的时刻，用于节流（每次新拖拽会话重置）
        let mut last_dizzy_emit: Option<Instant> = None;
        // 连续超速帧计数：仅当连续 DRAG_FAST_MIN_STREAK 帧都在阈值之上才判「拖太快」，
        // 单帧速度尖峰（手抖、采样抖动）不计入，避免缓慢挪动被误判晕眩
        let mut fast_drag_streak: usize = 0;
        // 当前超速连续段内的峰值速度：连续帧都超阈值还不够，还要这段里确实冲到过
        // DRAG_FAST_PEAK_VELOCITY，把「稳定快拖」挡在门外，只留爆发甩动
        let mut fast_drag_peak: f64 = 0.0;

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
                // 清掉本角色的拖动状态，避免 DRAG_OFFSET 残留导致 is_dragging 恒为 true
                if DRAG_OFFSET.lock().remove(&char_id_for_thread).is_some() {
                    tracing::info!(
                        "[cursor_tracking] 窗口隐藏，清除残留拖动状态: {char_id_for_thread}"
                    );
                }
                // 拖拽会话随隐藏终止：丢弃轨迹采样，避免恢复显示后用陈旧速度触发甩飞
                drag_samples.clear();
                fast_drag_streak = 0;
                fast_drag_peak = 0.0;
                prev_is_dragging = false;
                drag_observation = None;
                thread::sleep(Duration::from_millis(60));
                continue;
            }

            // 获取光标位置
            let c = match app_clone.cursor_position() {
                Ok(c) => c,
                Err(_) => {
                    thread::sleep(Duration::from_millis(60));
                    continue;
                }
            };

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
                    // 通知前端重置拖动会话状态（dragSessionRef、拖拽表情）。
                    // 必须 emit_to：广播的话另一只桌宠也会收到，跟着复位拖拽表情、
                    // 并打断它自己的长按召唤进度环（见本子系统顶部的说明）。
                    let _ = win.emit_to(&label, "drag:cancelled", json!({}));
                }
                is_dragging = false;
            }

            // 点击穿透已移除：桌宠窗口始终整窗响应鼠标，这里不再改写窗口样式。
            // （旧实现在此每 60ms 按中心矩形/左键状态调用 set_ignore_cursor_events，
            //   每次翻转都会触发 SetWindowPos(SWP_FRAMECHANGED) 使透明窗口整块重绘，
            //   导致桌宠持续闪烁。）

            if is_dragging {
                update_drag_position(&label, c.x as i32, c.y as i32, |x, y| {
                    move_window(&win, x, y);
                });
                // 光标轨迹采样（全局物理坐标，不受窗口追逐延迟影响），
                // 供松手瞬间计算惯性甩飞初速度
                let now = std::time::Instant::now();
                let observation = drag_observation.get_or_insert((now, c.x, c.y, 0.0, 0.0));
                observation.3 += (c.x - observation.1).hypot(c.y - observation.2);
                observation.1 = c.x;
                observation.2 = c.y;
                drag_samples.push_back((now, c.x, c.y));
                while drag_samples.len() > 1
                    && now.duration_since(drag_samples[0].0).as_millis() as u64
                        > FLING_SAMPLE_WINDOW_MS
                {
                    drag_samples.pop_front();
                }

                // 拖得太快 → 临时晕乎乎：用相邻两帧全局光标采样估计瞬时速度，
                // 需同时满足两个条件才认定「极端疯狂甩动」：
                //   1. 连续 DRAG_FAST_MIN_STREAK 帧都超过 DRAG_FAST_VELOCITY；
                //   2. 这段连续区间里峰值冲到过 DRAG_FAST_PEAK_VELOCITY。
                // 任一不满足都只算「快」不算「疯狂」，缓慢拖动更是完全不会触发。
                // 采样是全局物理坐标，不受窗口追逐延迟影响，所以量的是手速本身。
                if drag_samples.len() >= 2 {
                    let prev = drag_samples[drag_samples.len() - 2];
                    let last = *drag_samples.back().unwrap();
                    let speed = drag_speed(prev, last);
                    if let (Some(observation), Some(speed)) = (drag_observation.as_mut(), speed) {
                        observation.4 = observation.4.max(speed);
                    }
                    if speed.is_some_and(|v| v >= DRAG_FAST_VELOCITY) {
                        let v = speed.unwrap_or(0.0);
                        fast_drag_streak = fast_drag_streak.saturating_add(1);
                        fast_drag_peak = fast_drag_peak.max(v);
                    } else {
                        fast_drag_streak = 0;
                        fast_drag_peak = 0.0;
                    }
                    if fast_drag_streak >= DRAG_FAST_MIN_STREAK
                        && fast_drag_peak >= DRAG_FAST_PEAK_VELOCITY
                    {
                        let due = last_dizzy_emit
                            .map(|t| t.elapsed().as_millis() as u64 >= DRAG_FAST_EMIT_INTERVAL_MS)
                            .unwrap_or(true);
                        if due {
                            last_dizzy_emit = Some(Instant::now());
                            // 触发后清空连续段：下一次触发需重新累计一整段疯狂甩动，
                            // 避免持续超速时峰值一旦达标就永远保有资格
                            let reaction_peak = fast_drag_peak;
                            fast_drag_streak = 0;
                            fast_drag_peak = 0.0;
                            // emit_to：晕乎乎是「这一只被抓着甩懵了」，广播会让另一只也晕
                            let _ = win.emit_to(
                                &label,
                                "drag:dizzy",
                                json!({
                                    "duration_ms": DRAG_FAST_DIZZY_MS,
                                    "reason": "fast_drag",
                                    "speed_px_per_ms": reaction_peak,
                                }),
                            );
                        }
                    }
                }
            }

            // 拖拽结束瞬间：按松手前的光标轨迹计算初速度，触发惯性甩飞。
            // 覆盖正常 mouseup（前端 stop_window_drag）和 watchdog 兜底两条路径。
            if prev_is_dragging && !is_dragging {
                let v = release_velocity(
                    &drag_samples,
                    (Instant::now(), c.x, c.y),
                    win.scale_factor().unwrap_or(1.0),
                );
                if let Some((started, _, _, distance, peak)) = drag_observation.take() {
                    // Ignore click jitter, holds and reactions already covered by dizzy/fling.
                    if distance >= 20.0 && last_dizzy_emit.is_none() && v.is_none() {
                        let _ = win.emit_to(&label, "drag:interaction", json!({
                            "duration_ms": started.elapsed().as_millis() as u64,
                            "distance_px": distance,
                            "speed_px_per_ms": peak,
                        }));
                    }
                }
                drag_samples.clear();
                // 拖拽会话结束，晕眩节流窗口与连续超速计数一并作废：
                // 下一次拖拽可以立即触发
                last_dizzy_emit = None;
                fast_drag_streak = 0;
                fast_drag_peak = 0.0;
                let physics_enabled = app_clone.state::<Arc<crate::state::AppState>>().config.read().get_all().window.desktop_physics_enabled;
                if physics_enabled {
                    let (vx, vy) = v.unwrap_or((0.0, 0.0));
                    let _ = win.emit_to(&label, "drag:released", json!({ "vx": vx * 1000.0, "vy": vy * 1000.0 }));
                } else if let Some((fvx, fvy)) = v {
                    start_fling(win.clone(), &label, fvx, fvy);
                }
            }
            prev_is_dragging = is_dragging;

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
/// 收尾时对每个角色窗口做一次幂等的 `set_ignore_cursor_events(false)`，
/// 保证窗口不会带着鼠标穿透样式退出（点击穿透功能已移除，这是兜底不变量）。
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

    // 点击穿透已移除：桌宠窗口始终响应鼠标。这里做一次幂等收尾，
    // 确保没有窗口残留 WS_EX_TRANSPARENT（已处于该状态时为空操作，不触发重绘）。
    {
        let chars = state.characters.read();
        for c in chars.values() {
            if let Some(win) = app.get_webview_window(&c.id) {
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
//   （与点击穿透中心矩形同口径）。桌宠 模型主体只在该范围内渲染，
//   周围全是透明像素，因此窗口最多可滑出屏幕外 1/3 宽度 / 5/18 高度，
//   视觉上是角色本体撞到屏幕边缘被弹回
// - 碰撞法向速度乘回弹系数（restitution）损失能量，配合摩擦衰减，几次反弹后静止

/// 速度指数摩擦系数（每 ms）：v *= exp(-k·dt)，0.002 ≈ 350ms 半衰期
const FLING_FRICTION: f64 = 0.002;
/// 边缘碰撞的法向速度保留系数（能量损失后的反弹速度）
const FLING_RESTITUTION: f64 = 0.6;
/// 速度低于该值（物理像素/ms）视为静止，结束模拟
const FLING_STOP_VELOCITY: f64 = 0.06;
/// 物理帧间隔（ms）
const FLING_TICK_MS: u64 = 12;

// ---------- 晕乎乎临时表情 ----------
//
// 两种「被甩晕」的情形共用 `drag:dizzy` 事件，前端收到后把桌宠切到 dizzy 格位
// `duration_ms` 毫秒：
// - 拖动过快：拖动期间相邻两帧全局光标采样估计的瞬时速度超过阈值，
//   且需连续多帧超阈值 + 区间峰值达标，只有**极端疯狂甩动**才触发
// - 甩飞撞边：惯性甩飞线程检测到屏幕边缘碰撞，撞击越重晕得越久
//
// 时长由后端给出，前端只负责按给定毫秒数播放与回落，不再自行判定。

/// 拖动过程中判定「拖得太快」的光标速度阈值（物理像素/ms，约 3200px/s）
///
/// 该值需明显高于日常「缓慢摆弄」的手速：阈值定得偏低时，正常挪动桌宠
/// 也会被判为快速拖动而触发晕眩，表情变成噪声。约 3200px/s 只有**整条手臂
/// 猛地甩动**才够得到（普通人日常拖动多在 300–800px/s，用力甩也就 1500–2500px/s），
/// 因此定在这里意味着「只有极端疯狂甩动才切换晕乎乎」。
const DRAG_FAST_VELOCITY: f64 = 3.2;
/// 超速判定所需的连续超速帧数：仅当连续多帧都在阈值之上才认定「确实很快」，
/// 单帧抖动（手抖、采样时序抖动）不计入。光标线程约 60ms 一帧，4 帧 ≈ 240ms，
/// 要求疯狂甩动**持续**这么久，而非一个瞬时尖峰。
const DRAG_FAST_MIN_STREAK: usize = 4;
/// 超速连续帧内必须出现过的峰值速度（物理像素/ms，约 4200px/s）。
///
/// 光靠「连续 N 帧都 > 阈值」还不够：贴着阈值（3.2）连续几帧也算数，但那更接近
/// 「快」而非「疯狂」。这里再要求这只手在窗口内确实冲到过 4200px/s 以上，
/// 把「稳定快拖」挡在门外，只留真正的爆发甩动。
const DRAG_FAST_PEAK_VELOCITY: f64 = 4.2;
/// 快速拖动晕眩事件的最小间隔（ms）：持续超速拖动也只按此频率刷新表情
const DRAG_FAST_EMIT_INTERVAL_MS: u64 = 450;
/// 快速拖动触发的晕眩持续时长（ms）
const DRAG_FAST_DIZZY_MS: u64 = 1200;
/// 甩飞撞边触发晕眩的最小法向撞击速度（物理像素/ms），轻贴边缘不触发
const FLING_BOUNCE_MIN_IMPACT: f64 = 0.25;
/// 甩飞撞边晕眩的基准时长（ms）
const FLING_BOUNCE_DIZZY_BASE_MS: u64 = 1400;
/// 甩飞撞边晕眩的时长上限（ms）
const FLING_BOUNCE_DIZZY_MAX_MS: u64 = 2400;

/// 每窗口甩飞代号：新一轮甩飞开始时自增，被取代的旧线程检测到代号变化后自行退出
static FLING_GEN: Lazy<Mutex<std::collections::HashMap<String, u64>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 由相邻两帧全局光标采样估计瞬时速度（物理像素/ms）。
///
/// 两帧间隔太短（≤1ms）时速度不可信，返回 `None`。
///
/// 注意：本函数只回答「这一帧有多快」，是否真的判定「拖得太快」还需调用方
/// 累计连续超速帧数（见 `DRAG_FAST_MIN_STREAK`）并核对峰值（见
/// `DRAG_FAST_PEAK_VELOCITY`），避免缓慢挪动被单帧抖动带出晕眩。
fn drag_speed(prev: (Instant, f64, f64), last: (Instant, f64, f64)) -> Option<f64> {
    let dt_ms = last.0.duration_since(prev.0).as_secs_f64() * 1000.0;
    if dt_ms <= 1.0 {
        return None;
    }
    Some((last.1 - prev.1).hypot(last.2 - prev.2) / dt_ms)
}

/// 拖动过快判定：由相邻两帧全局光标采样估计瞬时速度，是否达到 `DRAG_FAST_VELOCITY`。
///
/// 两帧间隔太短（≤1ms）时速度不可信，一律返回 false。
///
/// 注意：本函数只回答「这一帧快不快」，是否真的判定「拖得太快」还需调用方
/// 累计连续超速帧数（见 `DRAG_FAST_MIN_STREAK`）并核对峰值，避免缓慢挪动
/// 被单帧抖动带出晕眩。
#[cfg(test)]
fn is_drag_too_fast(prev: (Instant, f64, f64), last: (Instant, f64, f64)) -> bool {
    drag_speed(prev, last).is_some_and(|v| v >= DRAG_FAST_VELOCITY)
}

/// 单轴边缘碰撞求解：把 `p` 夹紧到 `[min, max]`，并对朝外的法向速度做反弹。
///
/// 返回 `(夹紧后的位置, 反弹后的速度, 本轴撞击速度)`。
///
/// 撞击速度只在「积分前位置 `from` 严格在界内、且法向速度朝外」时给出：
/// 这才是甩飞速度把窗口从界内推出界的真撞击。窗口本来就在墙上/墙外
/// （被智能避让、环境走动、全屏隐藏等外部移动摆过去）时返回 0，
/// 不参与「撞边晕乎乎」判定。
fn resolve_axis_collision(
    from: f64,
    p: f64,
    v: f64,
    min: f64,
    max: f64,
    restitution: f64,
) -> (f64, f64, f64) {
    // 用 `!(a >= b)` 而非 `a < b`，顺带把 NaN 归入「不构成撞击」
    if !(from > min) {
        // 积分前已在/低于下界：夹紧但不记撞击
        if p < min {
            return (min, if v < 0.0 { -v * restitution } else { v }, 0.0);
        }
        return (p, v, 0.0);
    }
    if !(from < max) {
        // 积分前已在/高于上界：夹紧但不记撞击
        if p > max {
            return (max, if v > 0.0 { -v * restitution } else { v }, 0.0);
        }
        return (p, v, 0.0);
    }
    // 积分前在界内：只有朝外的速度才构成撞击
    if p < min {
        let impact = if v < 0.0 { -v } else { 0.0 };
        return (min, if v < 0.0 { -v * restitution } else { v }, impact);
    }
    if p > max {
        let impact = if v > 0.0 { v } else { 0.0 };
        return (max, if v > 0.0 { -v * restitution } else { v }, impact);
    }
    (p, v, 0.0)
}

/// 撞边晕眩时长：撞击越重晕得越久；轻贴边缘（法向速度低于阈值）返回 `None` 不触发。
///
/// 注意：本函数只做「给定时长」的换算，**是否构成撞击**由调用方判定。
/// 调用方只把「甩飞速度把窗口从界内推出界」的撞击记进 `impact`，
/// 智能避让等外部移动把桌宠摆到墙边不算撞击（`impact` 保持 0，这里自然返回 `None`）。
fn bounce_dizzy_ms(impact: f64) -> Option<u64> {
    // 用 `!(x >= min)` 而非 `x < min`，顺带把 NaN 归入「不触发」
    if !(impact >= FLING_BOUNCE_MIN_IMPACT) {
        return None;
    }
    let extra = ((impact - FLING_BOUNCE_MIN_IMPACT) * 1200.0).min(
        (FLING_BOUNCE_DIZZY_MAX_MS - FLING_BOUNCE_DIZZY_BASE_MS) as f64,
    );
    Some(FLING_BOUNCE_DIZZY_BASE_MS + extra as u64)
}

/// 启动惯性甩飞物理线程（Windows：以虚拟屏幕为碰撞边界）
///
/// 线程在下列任一情况下退出：
/// - 速度衰减到 FLING_STOP_VELOCITY 以下（自然停止）
/// - 窗口被重新抓起（DRAG_OFFSET 出现本窗口，可中途"接住"桌宠）
/// - 窗口隐藏 / 应用退出 / 代号被更新的甩飞取代
///
/// 每帧重读窗口实际位置作为积分基点，智能避让等外部移动不会被甩飞覆盖。
///
/// 「撞边晕乎乎」仅在**甩飞速度本帧把窗口从界内推出边界**时触发；
/// 窗口本来就在墙边（被智能避让/环境走动/全屏隐藏挪过去）时，边缘夹紧只是
/// 几何修正，不触发晕眩——避免桌宠自己走到墙边也晕乎乎。
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

                    // 本帧积分基点：窗口当前实际位置。
                    // 之所以每帧重读，是为了让智能避让等外部移动能「接管」桌宠位置，
                    // 甩飞不会把窗口硬拽回自己算出来的轨迹上。
                    let (pos_x, pos_y) = match win.outer_position() {
                        Ok(p) => (p.x as f64, p.y as f64),
                        Err(_) => break,
                    };
                    let (mut x, mut y) = (pos_x, pos_y);
                    x += vx * dt_ms;
                    y += vy * dt_ms;

                    // 边缘碰撞：位置夹紧 + 法向速度反弹。
                    //
                    // 「撞边晕乎乎」只认本帧由**甩飞速度**把窗口从界内推出界的撞击：
                    // 积分前（pos_*）该轴必须严格在界内，且法向速度确实朝外。
                    // 若积分前窗口已经在边界上/界外（智能避让、环境走动、全屏隐藏等
                    // 外部移动把它摆到了墙边），那这一帧的夹紧只是几何修正，不是撞击，
                    // 不记 impact——否则桌宠自己走到墙边也会「晕乎乎」。
                    let (nx, nvx, ix) = resolve_axis_collision(
                        pos_x,
                        x,
                        vx,
                        min_x,
                        max_x,
                        FLING_RESTITUTION,
                    );
                    x = nx;
                    vx = nvx;
                    let (ny, nvy, iy) = resolve_axis_collision(
                        pos_y,
                        y,
                        vy,
                        min_y,
                        max_y,
                        FLING_RESTITUTION,
                    );
                    y = ny;
                    vy = nvy;
                    let impact = ix.max(iy);

                    // 撞上屏幕边缘 → 临时晕乎乎：撞得越狠晕得越久。
                    // 法向速度低于阈值的轻贴边缘不触发，避免贴边滑行时抖出表情。
                    // emit_to：撞边晕的是这一只，别把另一只也叫醒（见本子系统顶部说明）
                    if let Some(dizzy_ms) = bounce_dizzy_ms(impact) {
                        let _ = win.emit_to(
                            &label,
                            "drag:dizzy",
                            json!({
                                "duration_ms": dizzy_ms,
                                "reason": "edge_bounce",
                                "impact": impact,
                            }),
                        );
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

// ============ 点击穿透：已移除 ============
//
// 桌宠角色窗口现在始终整窗响应鼠标，不再有"穿透 / 响应"两种状态。
// 因此以下内容已全部删除：
//   - commands/click_through.rs（WM_NCHITTEST 子类化）
//   - CLICK_THROUGH_SUSPEND_COUNT 暂停计数器
//   - suspend_click_through / resume_click_through / get_click_through_status 命令
//   - cursor_tracking 线程里每 60ms 的 set_ignore_cursor_events 切换
//
// 旧实现的副作用：每次状态翻转都会改写 GWL_EXSTYLE 并触发
// SetWindowPos(SWP_FRAMECHANGED)，使透明窗口整块重绘 —— 桌宠持续闪烁的根因。

// ============ 独立右缘菜单 / 按需聊天窗口 / 外部点击 ============
static CHAT_OUTSIDE_HOOK_RUNNING: AtomicBool = AtomicBool::new(false);
static CHAT_OUTSIDE_HOOK_TX: std::sync::OnceLock<std::sync::mpsc::Sender<(u8, i32, i32, isize)>> = std::sync::OnceLock::new();
struct ChatOutsideHookThreads {
    stop: Arc<AtomicBool>, hook_tid: Arc<AtomicU32>, hook_handle: JoinHandle<()>, consumer_handle: JoinHandle<()>,
}
static CHAT_OUTSIDE_HOOK_STOP: Lazy<Mutex<Option<ChatOutsideHookThreads>>> = Lazy::new(|| Mutex::new(None));

#[cfg(windows)]
unsafe extern "system" fn chat_outside_mouse_ll_proc(code: i32, wparam: windows::Win32::Foundation::WPARAM, lparam: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, GetAncestor, WindowFromPoint, GA_ROOT, MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MBUTTONDOWN, WM_XBUTTONDOWN};
    let message = wparam.0 as u32;
    if code >= 0 && matches!(message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN) {
        let event = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        // Capture the actual window under this press, including overlapping windows.
        // The hook only queues metadata; all Tauri calls stay on the consumer thread.
        let target = GetAncestor(WindowFromPoint(event.pt), GA_ROOT).0 as isize;
        if let Some(tx) = CHAT_OUTSIDE_HOOK_TX.get() { let _ = tx.send((u8::from(message != WM_LBUTTONDOWN), event.pt.x, event.pt.y, target)); }
    }
    CallNextHookEx(None, code, wparam, lparam)
}
fn handle_chat_outside_hook_event(app: &AppHandle, kind: u8, x: i32, y: i32, target: isize) {
    if kind == 0 { crate::edge_menu::outside_pointer_down(app, x, y); }
    #[cfg(windows)]
    for (label, window) in app.webview_windows() {
        if window.hwnd().is_ok_and(|hwnd| hwnd.0 as isize != target) {
            let _ = app.emit_to(&label, "companion:clipboard-outside-press", ());
        }
    }
    #[cfg(not(windows))]
    let _ = target;
}

#[tauri::command]
pub fn show_chat_animated(app: AppHandle) -> Result<(), String> {
    let win = app.get_webview_window("chat").ok_or("窗口不存在")?;
    crate::edge_menu::show_chat(&win)
}

#[tauri::command]
pub fn close_chat_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("chat") { win.destroy().map_err(err_str)?; }
    Ok(())
}

#[tauri::command]
pub fn start_edge_menu_watcher(app: AppHandle) -> Result<(), String> { crate::edge_menu::start(app); Ok(()) }
pub(crate) fn stop_edge_menu_watcher_internal() { crate::edge_menu::stop(); }

/// 启动 聊天窗口 全局鼠标 Hook（hook 线程 + 消费线程，幂等）。
/// 转发全局鼠标按下位置，供快捷工具窗口判断外部点击。
#[tauri::command]
pub fn start_chat_outside_click_hook(app: AppHandle) -> Result<(), String> {
    start_chat_outside_click_hook_internal(app)
}

#[cfg(windows)]
fn start_chat_outside_click_hook_internal(app: AppHandle) -> Result<(), String> {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, WH_MOUSE_LL,
    };

    if CHAT_OUTSIDE_HOOK_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(()); // 已在运行
    }

    let (tx, rx) = std::sync::mpsc::channel::<(u8, i32, i32, isize)>();
    let _ = CHAT_OUTSIDE_HOOK_TX.set(tx); // OnceLock；忽略 Err（仅重启场景，此处不发生）

    let stop = Arc::new(AtomicBool::new(false));
    let hook_tid = Arc::new(AtomicU32::new(0));

    // hook 线程：安装 WH_MOUSE_LL + 消息泵（低级钩子必须配消息泵才派发）
    let stop_h = Arc::clone(&stop);
    let tid_h = Arc::clone(&hook_tid);
    let hook_handle = thread::Builder::new()
        .name("chat-outside-mouse-hook".into())
        .spawn(move || unsafe {
            tid_h.store(GetCurrentThreadId(), Ordering::SeqCst);

            let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(chat_outside_mouse_ll_proc), None, 0)
            {
                Ok(h) if !h.0.is_null() => h,
                _ => {
                    tracing::warn!("[chat_outside_hook] SetWindowsHookExW 失败");
                    return;
                }
            };
            tracing::info!("[chat_outside_hook] WH_MOUSE_LL 已安装");

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
            tracing::info!("[chat_outside_hook] WH_MOUSE_LL 已卸载");
        })
        .map_err(err_str)?;

    // 消费线程处理窗口关闭，鼠标钩子只投递坐标。
    let stop_c = Arc::clone(&stop);
    let app_c = app.clone();
    let consumer_handle = thread::Builder::new()
        .name("chat-outside-mouse-consumer".into())
        .spawn(move || {
            while !stop_c.load(Ordering::SeqCst) && !APP_EXITING.load(Ordering::SeqCst) {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok((kind, x, y, target)) => handle_chat_outside_hook_event(&app_c, kind, x, y, target),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(err_str)?;

    *CHAT_OUTSIDE_HOOK_STOP.lock() = Some(ChatOutsideHookThreads {
        stop,
        hook_tid,
        hook_handle,
        consumer_handle,
    });
    Ok(())
}

#[cfg(not(windows))]
fn start_chat_outside_click_hook_internal(_app: AppHandle) -> Result<(), String> {
    Ok(())
}

/// 停止 聊天窗口 全局鼠标 Hook 线程（内部实现，应用退出时调用）。
/// 置停止标志 + PostThreadMessageW(WM_QUIT) 唤醒消息泵，限时 1s join 两线程，避免退出死锁。
#[cfg(windows)]
pub(crate) fn stop_chat_outside_click_hook_internal() {
    let entry = CHAT_OUTSIDE_HOOK_STOP.lock().take();
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
            tracing::warn!("[chat_outside_hook] 线程未在 1s 内退出，放弃等待");
        }
    }
    CHAT_OUTSIDE_HOOK_RUNNING.store(false, Ordering::SeqCst);
}

#[cfg(not(windows))]
pub(crate) fn stop_chat_outside_click_hook_internal() {}

/// Full chat windows always accept pointer input.
#[tauri::command]
pub fn ensure_chat_interactive(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("chat") { win.set_ignore_cursor_events(false).map_err(err_str)?; }
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
    if crate::companion_quiet::active() && window.app_handle().state::<std::sync::Arc<crate::state::AppState>>().characters.read().contains_key(window.label()) {
        return Ok(());
    }
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
/// 排除角色桌宠窗口：label = character_id（如 "nana" / "vivian"），
/// 由角色窗口自身管理生命周期。
///
/// 仅返回真正的子窗口（chat / config / memory / bubble / toast / startup_toast 等），
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
/// - **足迹口径**：桌宠 模型只占窗口中央 1/3 宽度（左右两侧全透明），
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

        // 桌宠 模型只占据窗口中央 1/3 宽度，左右两侧全透明、不产生遮挡。
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

                // 追加 ChatWindow（微信聊天窗口）和 聊天面板
                // 的完整矩形作为避让区域，防止桌宠移动到这些窗口上方遮挡用户视图。
                // 桌宠窗口用中央 1/3 足迹，但这些聊天窗口整体都有可见内容，用完整矩形。
                for win_label in ["chat"] {
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

#[cfg(test)]
mod dizzy_tests {
    use super::*;

    #[test]
    fn drag_move_releases_state_lock_before_window_dispatch() {
        let label = "test-drag-window-dispatch";
        DRAG_OFFSET.lock().insert(label.into(), (12, 34));
        let mut moved = false;
        update_drag_position(label, 100, 200, |x, y| {
            // 模拟窗口线程在移动期间收到松手命令。用 try_lock 避免回归时测试挂死。
            let mut offsets = DRAG_OFFSET
                .try_lock()
                .expect("窗口移动时仍持有拖动锁，松手命令会与窗口线程死锁");
            assert_eq!(offsets.remove(label), Some((12, 34)));
            assert_eq!((x, y), (88, 166));
            moved = true;
        });
        assert!(moved);
        update_drag_position(label, 100, 200, |_, _| {
            panic!("松手后不应继续移动窗口");
        });
    }

    fn at(ms: u64, x: f64, y: f64) -> (Instant, f64, f64) {
        (Instant::now() + Duration::from_millis(ms), x, y)
    }

    #[test]
    fn drag_too_fast_needs_speed_above_threshold() {
        // 阈值 3.2 px/ms。正向用例留足够余量，避免 Instant 纳秒抖动让跨度略大于
        // 名义 60ms 而把速度压到阈值之下：60ms 内 240px → 4.0 px/ms（余量 25%）
        assert!(is_drag_too_fast(at(0, 0.0, 0.0), at(60, 240.0, 0.0)));
        // 60ms 内移动 60px → 1.0 px/ms：早期阈值 0.9 会误判为「快」，
        // 这正是「缓慢拖动也晕乎乎」的成因，现在必须不触发
        assert!(!is_drag_too_fast(at(0, 0.0, 0.0), at(60, 60.0, 0.0)));
        // 旧阈值 1.8 下的「快拖」现在也算慢：60ms 内 120px → 2.0 px/ms（用力甩动量级）不触发
        assert!(!is_drag_too_fast(at(0, 0.0, 0.0), at(60, 120.0, 0.0)));
        // 即使冲到 3.0 px/ms（≈3000px/s）也仍在逐帧门槛之下
        assert!(!is_drag_too_fast(at(0, 0.0, 0.0), at(60, 180.0, 0.0)));
        // 斜向移动同样按合速度判定：60ms 内 (144, 192) → 240px → 4.0 px/ms
        assert!(is_drag_too_fast(at(0, 0.0, 0.0), at(60, 144.0, 192.0)));
    }

    #[test]
    fn drag_too_fast_ignores_unreliable_time_span() {
        // 同一时刻（跨度 0ms）：速度不可信，不触发。
        // 用**同一个** Instant 保证跨度恰好 0——两个独立 Instant::now() 之间有
        // 调度抖动，不适合构造「恰好 1ms」这种贴边用例
        let same = Instant::now();
        assert!(!is_drag_too_fast((same, 0.0, 0.0), (same, 999.0, 999.0)));
        // 跨度足够（≥2ms）时同样的位移可信：999px/2ms 远超阈值
        assert!(is_drag_too_fast(at(0, 0.0, 0.0), at(2, 999.0, 999.0)));
    }

    #[test]
    fn drag_speed_reports_magnitude() {
        // 合速度：约 60ms 内 (60, 80) 位移 100px → ≈1.667 px/ms
        // （Instant 是纳秒精度，实际跨度不会正好 60ms，用相对容差比较）
        let v = drag_speed(at(0, 0.0, 0.0), at(60, 60.0, 80.0)).unwrap();
        let expected = 100.0 / 60.0;
        assert!(
            (v - expected).abs() / expected < 0.01,
            "期望 ≈{expected}，实际 {v}"
        );
        // 斜向与轴向都按合速度：与轴对齐的 (100, 0) 应得到同样的速度
        let v_axis = drag_speed(at(0, 0.0, 0.0), at(60, 100.0, 0.0)).unwrap();
        assert!(
            (v_axis - expected).abs() / expected < 0.01,
            "轴向速度应与合速度一致：{v_axis} vs {expected}"
        );
        // 跨度不可信时返回 None。用**同一个** Instant，保证跨度恰好 0
        //（两个独立 Instant::now() 之间会有调度抖动，1ms 的用例并不稳定）
        let same = Instant::now();
        assert!(drag_speed((same, 0.0, 0.0), (same, 999.0, 999.0)).is_none());
    }

    #[test]
    fn drag_fast_streak_requires_multiple_frames() {
        // 连续超速帧数门槛：只有极端疯狂甩动能连续这么多帧，越高越保守
        assert!(
            DRAG_FAST_MIN_STREAK >= 4,
            "连续超速帧数门槛过低: {DRAG_FAST_MIN_STREAK}"
        );
    }

    #[test]
    fn drag_fast_peak_must_exceed_streak_threshold() {
        // 峰值门槛必须严格高于逐帧门槛，否则「贴着阈值连超几帧」也会触发，
        // 就失去了「只认爆发甩动」的意义
        assert!(
            DRAG_FAST_PEAK_VELOCITY > DRAG_FAST_VELOCITY,
            "峰值门槛 {DRAG_FAST_PEAK_VELOCITY} 必须高于逐帧门槛 {DRAG_FAST_VELOCITY}"
        );
    }

    #[test]
    fn drag_fast_threshold_is_far_above_normal_hand_speed() {
        // 日常拖动 300–800px/s、用力甩 1500–2500px/s（= 1.5–2.5 px/ms）都不该触发；
        // 逐帧门槛至少要高过「用力甩」的上沿
        assert!(
            DRAG_FAST_VELOCITY >= 3.0,
            "逐帧门槛 {DRAG_FAST_VELOCITY} 仍会让日常用力甩动触发晕眩"
        );
    }

    #[test]
    fn bounce_dizzy_skips_gentle_touch() {
        assert_eq!(bounce_dizzy_ms(0.0), None);
        assert_eq!(bounce_dizzy_ms(0.24), None);
        assert_eq!(bounce_dizzy_ms(-1.0), None);
        assert_eq!(bounce_dizzy_ms(f64::NAN), None);
    }

    #[test]
    fn bounce_dizzy_grows_with_impact_and_is_capped() {
        let base = bounce_dizzy_ms(FLING_BOUNCE_MIN_IMPACT).unwrap();
        assert_eq!(base, FLING_BOUNCE_DIZZY_BASE_MS);

        let medium = bounce_dizzy_ms(0.75).unwrap();
        assert!(medium > base, "撞击更重应当晕更久: {medium} <= {base}");

        // 极端撞击（甩飞初速度上限 4.0 px/ms）封顶
        let hardest = bounce_dizzy_ms(FLING_MAX_VELOCITY).unwrap();
        assert_eq!(hardest, FLING_BOUNCE_DIZZY_MAX_MS);
    }

    // ---------- 单轴边缘碰撞：区分「甩飞撞墙」与「被摆到墙边」 ----------

    const R: f64 = 0.6;

    #[test]
    fn axis_collision_counts_real_impact_from_inside() {
        // 界 [0,1000]，积分前 from=900 在界内，积分后 1100 越上界，速度 +2.0 朝外
        // → 真撞击：位置夹到 1000，撞速 2.0，反弹速度 -1.2
        let (p, v, impact) = resolve_axis_collision(900.0, 1100.0, 2.0, 0.0, 1000.0, R);
        assert_eq!(p, 1000.0, "应夹紧到上界");
        assert!((impact - 2.0).abs() < 1e-9, "真撞击撞速应为 2.0，实际 {impact}");
        assert!((v - (-1.2)).abs() < 1e-9, "反弹速度应为 -1.2，实际 {v}");

        // 下界对称：from=100 界内，积分后 -50 越下界，速度 -2.0 朝外
        let (p, v, impact) = resolve_axis_collision(100.0, -50.0, -2.0, 0.0, 1000.0, R);
        assert_eq!(p, 0.0, "应夹紧到下界");
        assert!((impact - 2.0).abs() < 1e-9, "真撞击撞速应为 2.0，实际 {impact}");
        assert!((v - 1.2).abs() < 1e-9, "反弹速度应为 1.2，实际 {v}");
    }

    #[test]
    fn axis_collision_bypasses_impact_when_already_at_wall() {
        // 智能避让把窗口挪到上界：积分前 from 已 == max，积分后仍越界，
        // 速度即便朝外也不记撞击（否则「自己走到墙边也晕乎乎」）
        let (p, v, impact) = resolve_axis_collision(1000.0, 1100.0, 2.0, 0.0, 1000.0, R);
        assert_eq!(p, 1000.0, "仍应夹紧位置");
        assert_eq!(impact, 0.0, "已在墙上不应记撞击");
        assert!((v - (-1.2)).abs() < 1e-9, "反弹仍生效，避免卡在墙里");

        // 已越界（from 在上界之外）同样是外部移动摆过去的结果，不记撞击
        let (_p, _v, impact) = resolve_axis_collision(1200.0, 1300.0, 2.0, 0.0, 1000.0, R);
        assert_eq!(impact, 0.0, "界外夹紧不应记撞击");
    }

    #[test]
    fn axis_collision_bypasses_impact_when_velocity_inward() {
        // 积分前在界内、位置越界，但速度朝内（负）——不可能由甩飞造成，
        // 不给撞击（防守性分支）
        let (_p, _v, impact) = resolve_axis_collision(900.0, 1100.0, -2.0, 0.0, 1000.0, R);
        assert_eq!(impact, 0.0, "朝内速度不构成撞击");
    }

    #[test]
    fn axis_collision_no_impact_within_bounds() {
        let (p, v, impact) = resolve_axis_collision(100.0, 200.0, 1.5, 0.0, 1000.0, R);
        assert_eq!((p, impact), (200.0, 0.0));
        assert!((v - 1.5).abs() < 1e-9, "界内速度不变");
    }

    #[test]
    fn axis_collision_handles_nan_from_safely() {
        // from 为 NaN 时按「不可信」处理：不记撞击
        let (_p, _v, impact) = resolve_axis_collision(f64::NAN, 1100.0, 2.0, 0.0, 1000.0, R);
        assert_eq!(impact, 0.0);
    }
}
