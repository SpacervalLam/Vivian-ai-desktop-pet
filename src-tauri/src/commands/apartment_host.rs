//! Minimal native host adapter for the external apartment plugin.
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};
use super::window::{freeze_webview, thaw_webview, is_window_foreground, is_escape_down, APP_EXITING};
// ============ 房间模式：角色窗口批量显隐 + 冻结 ============
//
// 进入 3D 房间时桌面角色窗口整体让位：hide 之后 TrySuspend 冻结，
// 桌宠 的每帧渲染循环随之停止——窗口 hide 不会自动暂停 rAF，
// 不冻结的话两个角色在房间里仍持续空转 GPU/CPU，显存分配也照旧。
// 离开时先 Resume 再 show，恢复渲染。
//
// hide→freeze 与 thaw→show 必须在主线程 FIFO 队列里保持先后，
// 所以由本命令一次性完成，前端不再自己 show/hide 角色窗口。
//
// 进房间前的可见性记录在案：出房间只恢复原本可见的窗口，
// 避免把用户手动下线的角色误 show 回来。

/// 进入房间前各窗口的可见状态（label → visible）
static ROOM_PREV_VISIBILITY: Lazy<Mutex<std::collections::HashMap<String, bool>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 房间模式是否已激活
///
/// 退出路径有两条且都会到达：前端卸载时 invoke set_room_mode(false)，
/// 以及 room 窗口 CloseRequested / Destroyed 时 Rust 侧兜底。
///
/// 进入侧同样必须幂等，这比退出侧更容易被忽略：Tauri 命令在异步运行时上并发
/// 执行，多个 invoke 之间没有先后保证（React StrictMode 会连发 true→false→true
/// 三条）。若重复的「进入」不整体跳过，它会在角色窗口已被隐藏之后重新记一次档，
/// 把 prev 里的可见性覆盖成 false —— 退出时读到 false 就不 show，桌宠从此消失。
/// 「关闭房间后桌宠回不来」就是这个：不是恢复逻辑没跑，是它读到的是被覆盖过的档。
static ROOM_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 房间模式期间与角色窗口一起让位的常驻窗口
///
/// 心智观察器是全屏窗口，房间窗口（无边框透明全屏）叠在它上面时，
/// 场景之外的透明区域会透出它的内容；而且被完全盖住还在跑渲染没有意义。
/// 它与角色窗口的区别是不冻结 WebView——它没有 桌宠 渲染循环，
/// hide 之后 WebView2 自己就会节流，冻结反而会让期间的配置变更事件丢失。
const ROOM_MODE_EXTRA_WINDOWS: &[&str] = &["memory"];

/// 3D 公寓窗口的 label，与前端 utils/roomWindow.ts 的 ROOM_WINDOW_LABEL 保持一致
pub(crate) const ROOM_WINDOW_LABEL: &str = "room";

#[tauri::command]
pub fn set_room_mode(
    app: AppHandle,
    state: State<'_, std::sync::Arc<crate::state::AppState>>,
    active: bool,
) -> Result<(), String> {
    if active && !crate::commands::apartment::enabled(state.inner()) {
        return Err("3D 公寓插件未安装或已禁用".into());
    }
    set_room_mode_internal(&app, state.inner(), active);
    Ok(())
}

/// 房间模式的实际实现，命令与窗口事件两条路径共用
///
/// 幂等且串行：进入和退出都在 ROOM_PREV_VISIBILITY 的锁内完成，两个方向各自
/// 只在状态真的翻转时执行一次。
///
/// 为什么要整段加锁：切换一次要做「读可见性 → 记档 → hide/show」三步，
/// 并发的两个调用会读到彼此的中间态（一个刚记完档还没 hide，另一个就把
/// 已隐藏的窗口记成 prev=false）。锁内只有往主线程 FIFO 投递的非阻塞窗口
/// 操作，不会和主线程互相等待。
pub(crate) fn set_room_mode_internal(
    app: &AppHandle,
    state: &std::sync::Arc<crate::state::AppState>,
    active: bool,
) {
    let mut prev = ROOM_PREV_VISIBILITY.lock();

    if active {
        if ROOM_MODE_ACTIVE.swap(true, Ordering::SeqCst) {
            return; // 已在房间模式：重复的进入调用不重新记档
        }
        // 上一次退出漏清的残留会让恢复时读到过期的可见性
        prev.clear();

        let chars_cfg = state.config.read().get_all().characters;
        for entry in chars_cfg.list.iter() {
            toggle_window_for_room(app, &entry.id, true, &mut prev);
        }
        for label in ROOM_MODE_EXTRA_WINDOWS {
            toggle_window_for_room(app, label, false, &mut prev);
        }
    } else {
        if !ROOM_MODE_ACTIVE.swap(false, Ordering::SeqCst) {
            return; // 压根没进过房间模式，或已经退出过一次 → 重复的兜底调用
        }
        // 只恢复本次记过档的窗口：角色中途下线、或压根没建起来的窗口不在里面
        let labels: Vec<String> = prev.keys().cloned().collect();
        let chars_cfg = state.config.read().get_all().characters;
        for label in labels {
            let Some(was_visible) = prev.remove(&label) else {
                continue;
            };
            let freeze = chars_cfg.list.iter().any(|e| e.id == label);
            restore_window_for_room(app, &label, was_visible, freeze);
        }
    }
}

/// 进入房间：记录可见性后隐藏（`freeze` 为 true 时连同 WebView 一起冻结，
/// 角色窗口走这条）
fn toggle_window_for_room(
    app: &AppHandle,
    label: &str,
    freeze: bool,
    prev: &mut std::collections::HashMap<String, bool>,
) {
    let Some(win) = app.get_webview_window(label) else {
        return;
    };
    let visible = win.is_visible().unwrap_or(false);
    prev.insert(label.to_string(), visible);
    let _ = win.hide();
    if freeze {
        freeze_webview(&win);
    }
}

/// 离开房间：先解冻再按记档决定是否恢复显示
fn restore_window_for_room(app: &AppHandle, label: &str, was_visible: bool, freeze: bool) {
    let Some(win) = app.get_webview_window(label) else {
        return;
    };
    if freeze {
        thaw_webview(&win);
    }
    if was_visible {
        let _ = win.show();
    }
}

// ============ 房间窗口 ESC 看护线程 ============
//
// 第一人称 PointerLock 状态下，浏览器把 ESC 保留用于「退出指针锁定」，
// 不会向页面派发 keydown——前端 keydown 监听永远收不到 ESC，用户按 ESC
// 只会解锁鼠标、不会关闭窗口。这里改用原生 GetAsyncKeyState 轮询 ESC 的
// 下降沿（硬件级按键状态，不受 PointerLock 影响），在 room 窗口是前台窗口
// 时直接 close()。线程随窗口销毁 / 显式停止 / 应用退出自动结束。
//
// 生命周期用单调递增的「代号」而不是「运行中标志 + 停止标志」一对布尔。
// 旧写法下 watch 会因为「已在运行」直接返回，而 stop 只是置位、要等线程下一轮
// 轮询（≤20ms）才真正清掉运行中标志。于是 stop→start 间隔一旦短于这个退出延迟，
// start 就被挡掉、旧线程随后才退出，看护永久消失且无法自愈。
// React StrictMode 会把挂载 effect 跑成 mount→cleanup→mount，三条 invoke 挤在
// 几毫秒内，必然命中这个时序——这就是「房间里按 ESC 没反应」。
// 改成代号后：每次 watch 领一个新代号并让旧线程失效，stop 只是领一个没人认领的
// 代号，无论调用多密集都恰好剩一个活线程。

/// 当前有效的看护线程代号；每次启动或停止都 +1
static ROOM_ESC_GEN: AtomicU32 = AtomicU32::new(0);

/// 命令面板打开期间是否抑制硬件 ESC 看护。
/// 前端 Minecraft 风格命令面板输入时，ESC 要由 keydown 关面板而不是关窗口；
/// 硬件轮询不知道 DOM 状态，只能由前端显式开/关这个开关。
static ROOM_ESC_SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// 窗口暂时查不到时的宽限时长：命令可能比窗口注册早到一瞬间
const ROOM_ESC_MISSING_GRACE: Duration = Duration::from_secs(3);

/// 启动 room 窗口的 ESC 看护线程（前端 RoomWindow 挂载时调用，重复调用安全）
#[tauri::command]
pub fn watch_room_escape(app: AppHandle) {
    // 领号即让上一代看护失效，无需再判断它是否还在跑
    let gen = ROOM_ESC_GEN.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    // 新看护从非抑制态起步：上次会话面板没关干净留下的残留抑制不能带进来
    ROOM_ESC_SUPPRESSED.store(false, Ordering::SeqCst);

    thread::spawn(move || {
        // 开局就按着 ESC（例如按着 ESC 点开房间）不算一次按下
        let mut esc_prev = is_escape_down();
        let mut missing_since: Option<Instant> = None;
        // 上一轮是否处于抑制态。用于检测「抑制 → 恢复」沿，见下方对 esc_prev 的消耗。
        let mut suppressed_prev = ROOM_ESC_SUPPRESSED.load(Ordering::SeqCst);

        loop {
            // 主应用退出（APP_EXITING）或本看护被更新的代号取代/stop 时立刻退出
            if APP_EXITING.load(Ordering::SeqCst)
                || ROOM_ESC_GEN.load(Ordering::SeqCst) != gen
            {
                break;
            }
            let Some(win) = app.get_webview_window(ROOM_WINDOW_LABEL) else {
                // 窗口已销毁，或命令比窗口注册先到：给一段宽限期再退出，
                // 否则一次早到的 invoke 就能让看护静默消失
                let since = *missing_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= ROOM_ESC_MISSING_GRACE {
                    break;
                }
                esc_prev = false;
                thread::sleep(Duration::from_millis(20));
                continue;
            };
            missing_since = None;

            let esc_now = is_escape_down();
            let suppressed = ROOM_ESC_SUPPRESSED.load(Ordering::SeqCst);
            if suppressed {
                // 命令面板打开：硬件 ESC 看护暂停，前端 keydown 负责关面板。
                // 抑制期每次把 esc_prev 同步成现状——否则恢复后无法重建基线。
                esc_prev = esc_now;
                suppressed_prev = true;
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            if suppressed_prev {
                // 抑制刚解除（面板刚收掉）。关键竞态：关面板的那一次 ESC 此刻可能
                // 还物理按着，而抑制期间的第一拍往往在「按下之前」就已把 esc_prev
                // 同步为 false —— 若此时立即建档，会把同一次按键误判成新的下降沿
                // 关掉整个窗口。所以恢复时先把当前 ESC 状态消费掉：必须松开再按，
                // 才构成一次真正的关闭。
                esc_prev = esc_now;
                suppressed_prev = false;
            }
            if esc_now && !esc_prev && is_window_foreground(&win) {
                // 下降沿 + 前台窗口：直接关闭。close 触发 CloseRequested，
                // 由 lib.rs 的 on_window_event 兜底恢复角色窗口与心智观察器。
                let _ = win.close();
                break;
            }
            esc_prev = esc_now;
            // 20ms 轮询：快于人类点按 ESC 的最短持续时间（~40-60ms），
            // 避免"按下即松开"落在两次轮询之间被漏检
            thread::sleep(Duration::from_millis(20));
        }
        tracing::info!("[room_escape] 看护线程已退出 (gen={gen})");
    });
}

/// 命令面板开合时由前端调用：suppressed=true 期间硬件 ESC 看护暂停
/// （ESC 只关面板不关窗口），面板收起后置回 false 恢复看护。
#[tauri::command]
pub fn set_room_escape_suppressed(suppressed: bool) {
    ROOM_ESC_SUPPRESSED.store(suppressed, Ordering::SeqCst);
}

/// 停止 room 窗口的 ESC 看护线程（前端 RoomWindow 卸载时调用）
#[tauri::command]
pub fn stop_room_escape_watcher() {
    // 让现有看护失效；下一次 watch_room_escape 会领新代号重新起线程
    ROOM_ESC_GEN.fetch_add(1, Ordering::SeqCst);
}
