//! Toast 窗口的「区域级」点击穿透。
//!
//! 问题：toast 窗口是一整块真实窗口（400 × 半屏的透明矩形）。整窗响应鼠标时，屏幕右下
//! 那一大片透明区域会把点击全吃掉——包括它压住的任务栏和其他窗口。真正需要鼠标的只有
//! 「渲染出来的确认卡 / 带按钮的 toast」那几块矩形，其余都该让给下面的窗口。
//!
//! 机制：前端把可交互矩形（客户区**逻辑**像素）推进来，本模块起一条后台线程轮询全局
//! 光标，命中矩形才把窗口切成可交互；离开矩形即恢复穿透。没有可交互区域时线程退出，
//! 空闲期零开销。
//!
//! ⚠️ 不可违反的约束：`set_ignore_cursor_events` 在 Windows 上会改写 GWL_EXSTYLE 并触发
//! SetWindowPos(SWP_FRAMECHANGED)，透明窗口会**整块重绘**。桌宠历史上正是"每 60ms 无条件
//! 调用一次"导致持续闪烁，那段逻辑被整块删除。所以这里：
//!   - 只在状态真正翻转时下发（`APPLIED` 记住上次值，相同值直接返回）；
//!   - 没有可交互区域时线程直接退出，连一次调用都不发。
//!
//! 为什么不用 WH_MOUSE_LL 钩子（响应更快）：低级钩子回调里不能直接调 Tauri API——它可能
//! 阻塞消息泵，钩子会被系统悄悄摘掉，要另起消费线程转发（side_chat 那套就是这个结构）；
//! 而收益只是把最坏 8ms 的判定延迟降到 1ms。人手不可能在 8ms 内完成"移进矩形并按下左键"，
//! 不值得那份复杂度。

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewWindow};

/// 命中轮询间隔（毫秒）。只在存在可交互区域时以该频率运行
const TICK: Duration = Duration::from_millis(8);
/// 空闲轮询间隔（毫秒）：空转两轮仍是空表就让线程退出
const IDLE_TICK: Duration = Duration::from_millis(200);

/// 窗口 label → 可交互矩形（客户区逻辑像素，`[x, y, w, h]`）
static REGIONS: Lazy<Mutex<HashMap<String, Vec<[f64; 4]>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 窗口 label → 最近一次下发给系统的穿透状态（true = 穿透）。用于抑制重复下发
static APPLIED: Lazy<Mutex<HashMap<String, bool>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// 轮询线程是否在运行
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 登记某个 toast 窗口的可交互矩形（空数组 = 整窗穿透）。
///
/// 前端每次「可交互区域或它们的位置」变化时调用一次即可，不必按帧调用。
#[tauri::command]
pub fn set_toast_hit_regions(
    app: AppHandle,
    window: WebviewWindow,
    regions: Vec<[f64; 4]>,
) -> Result<(), String> {
    let label = window.label().to_string();
    if regions.is_empty() {
        REGIONS.lock().remove(&label);
    } else {
        REGIONS.lock().insert(label.clone(), regions);
    }
    // 立刻按当前光标重判一次：卡片弹出/关闭、上方条目进出导致的整体位移都可能发生在
    // 光标静止时，只等下一轮轮询会差一拍。区域清空时这一下也是"马上恢复穿透"的保证。
    refresh(&app, &label);
    if !REGIONS.lock().is_empty() {
        ensure_watcher(app);
    }
    Ok(())
}

/// 确保命中轮询线程在跑（幂等）。幂等靠 `RUNNING` 的 swap，线程退出前会检查残留登记并自我重启。
fn ensure_watcher(app: AppHandle) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = thread::Builder::new()
        .name("toast-hit".into())
        .spawn(move || {
            loop {
                if crate::commands::window::APP_EXITING.load(Ordering::SeqCst) {
                    break;
                }
                if REGIONS.lock().is_empty() {
                    // 没有可交互区域：连睡两轮仍为空就退出。空表期间的轮询毫无意义，
                    // 而这条线程的存在意义只是"卡片在屏幕上那几秒"。
                    thread::sleep(IDLE_TICK);
                    if REGIONS.lock().is_empty() {
                        break;
                    }
                    continue;
                }
                tick(&app);
                thread::sleep(TICK);
            }
            RUNNING.store(false, Ordering::SeqCst);
            // 退出与「新登记到达」可能交错：此处兜底重启，避免出现"表里有区域却没有线程"
            if !REGIONS.lock().is_empty()
                && !crate::commands::window::APP_EXITING.load(Ordering::SeqCst)
            {
                ensure_watcher(app.clone());
            }
        });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// 一轮判定：遍历所有已登记的窗口
fn tick(app: &AppHandle) {
    let labels: Vec<String> = REGIONS.lock().keys().cloned().collect();
    for label in labels {
        refresh(app, &label);
    }
}

/// 按当前光标位置重判某个窗口该不该接收鼠标
fn refresh(app: &AppHandle, label: &str) {
    let Some(win) = app.get_webview_window(label) else {
        // 窗口已销毁：清掉登记，别留下无人认领的条目
        REGIONS.lock().remove(label);
        APPLIED.lock().remove(label);
        return;
    };
    let inside = win.is_visible().unwrap_or(false) && cursor_inside(app, &win, label);
    apply(&win, label, !inside);
}

/// 光标是否落在该窗口的某个可交互矩形内
fn cursor_inside(app: &AppHandle, win: &WebviewWindow, label: &str) -> bool {
    let regions = match REGIONS.lock().get(label) {
        Some(r) if !r.is_empty() => r.clone(),
        _ => return false,
    };
    // 光标与窗口原点同为物理屏幕坐标；前端推的是逻辑像素，除回缩放比再比较。
    // 原点必须取**客户区**原点（inner_position）：命中矩形是相对客户区量的，
    // 无边框窗口下二者通常相同，但不该靠这个巧合成立。取不到时退回 outer_position。
    let (Ok(cursor), Ok(origin)) = (
        app.cursor_position(),
        win.inner_position().or_else(|_| win.outer_position()),
    ) else {
        return false;
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    if scale <= 0.0 {
        return false;
    }
    let lx = (cursor.x - origin.x as f64) / scale;
    let ly = (cursor.y - origin.y as f64) / scale;
    regions
        .iter()
        .any(|r| lx >= r[0] && lx <= r[0] + r[2] && ly >= r[1] && ly <= r[1] + r[3])
}

/// 下发穿透状态。与上次相同则直接返回——重复下发正是闪烁的来源（见模块头注释）。
fn apply(win: &WebviewWindow, label: &str, ignore: bool) {
    {
        let mut applied = APPLIED.lock();
        if applied.get(label).copied() == Some(ignore) {
            return;
        }
        applied.insert(label.to_string(), ignore);
    }
    let _ = win.set_ignore_cursor_events(ignore);
}
