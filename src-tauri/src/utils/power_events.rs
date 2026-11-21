//! 系统睡眠/唤醒感知
//!
//! 通过 Windows `PowerRegisterSuspendResumeNotification` 订阅系统级
//! suspend/resume 事件，修正桌宠的在场/离开判定：
//!
//! - **睡眠前**（suspend）：立刻为所有角色标记用户离开。`GetLastInputInfo`
//!   基于 tick 计数（不含睡眠时间），若依赖 idle 阈值检测，通宵睡眠场景下
//!   唤醒后 idle 仍显示睡前的几秒——"离开"状态从未落账，回归摘要（recap）
//!   永远不触发。睡前强制落账后，唤醒时的 away_secs 才能正确涵盖整段睡眠。
//! - **唤醒后**（resume）：不主动标记在场——等用户真实键鼠活动（proactive
//!   tick 的 idle < 60 检测）才触发 Present 转换，保持"用户回来了"而非
//!   "机器醒了"的语义。睡眠时长超过阈值时把事件写入统一事件账本，
//!   作为回归摘要与角色感知的素材。
//!
//! 回调运行在系统电源线程，suspend 分支只做微秒级内存写；
//! resume 分支的账本写入派发到后台线程，避免阻塞唤醒流程。

use std::sync::Arc;

use once_cell::sync::OnceCell;

use crate::state::AppState;

/// AppState 全局引用（回调为 extern "system" 无捕获，经静态通道传递）
static STATE: OnceCell<Arc<AppState>> = OnceCell::new();

/// 最近一次睡眠开始时刻（Unix 秒；0 = 无记录）
static SLEPT_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 睡眠时长达到该阈值才写入事件账本（太短的合盖/瞬断不值得记录）
const MIN_SLEEP_SECS_TO_LOG: f64 = 300.0;

/// 启动电源事件监听（幂等：重复调用直接返回）
pub fn spawn_power_event_listener(state: Arc<AppState>) {
    if STATE.set(state).is_err() {
        return;
    }

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;

        unsafe {
            let mut registration: *mut core::ffi::c_void = std::ptr::null_mut();
            // recipient 传回调函数指针（经 HANDLE 透传到系统电源服务）
            let recipient = HANDLE(power_callback as *mut core::ffi::c_void);
            let hr = windows::Win32::System::Power::PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                recipient,
                &mut registration,
            );
            if hr.is_ok() {
                // 注册句柄进程生命周期内保持有效（不注销，随进程退出回收）
                tracing::info!("[PowerEvents] 系统睡眠/唤醒监听已注册");
            } else {
                tracing::warn!("[PowerEvents] 注册失败（code={}），睡眠感知不可用", hr.0);
            }
        }
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn power_callback(
    _context: *const core::ffi::c_void,
    event_type: u32,
    _setting: *const core::ffi::c_void,
) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{
        PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    };

    if let Some(state) = STATE.get() {
        match event_type {
            PBT_APMSUSPEND => on_suspend(state),
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => on_resume(state),
            _ => {}
        }
    }
    // 回调返回值未被系统使用，固定返回 0
    0
}

/// 睡眠前：记录时刻并为所有角色强制标记用户离开（幂等，微秒级内存写）
fn on_suspend(state: &Arc<AppState>) {
    let now = chrono::Local::now().timestamp() as f64;
    SLEPT_AT.store(now as u64, std::sync::atomic::Ordering::Relaxed);
    for inst in state.characters.read().values() {
        inst.brain.world_state.mark_user_away();
    }
    tracing::info!("[PowerEvents] 系统即将睡眠，已标记用户离开（away_since={now:.0}）");
}

/// 唤醒后：睡眠时长达标时派发后台线程写入统一事件账本
fn on_resume(state: &Arc<AppState>) {
    let slept_at = SLEPT_AT.swap(0, std::sync::atomic::Ordering::Relaxed) as f64;
    if slept_at <= 0.0 {
        return;
    }
    let slept_secs = (chrono::Local::now().timestamp() as f64 - slept_at).max(0.0);
    tracing::info!("[PowerEvents] 系统唤醒，本次睡眠 {slept_secs:.0} 秒");
    if slept_secs < MIN_SLEEP_SECS_TO_LOG {
        return;
    }

    // 账本写入含文件 IO，派发后台线程避免阻塞唤醒流程。
    // 按角色隔离约定，为每个角色各注册一条关联事件（recap 检索按角色可见性过滤）。
    let char_ids: Vec<String> = state
        .characters
        .read()
        .keys()
        .cloned()
        .collect();
    std::thread::spawn(move || {
        let now = chrono::Local::now().timestamp() as f64;
        let hours = (slept_secs / 3600.0).round();
        let text = if hours >= 1.0 {
            format!("系统睡眠事件：电脑休眠了约 {hours} 小时后刚刚唤醒")
        } else {
            let minutes = (slept_secs / 60.0).round();
            format!("系统睡眠事件：电脑休眠了约 {minutes} 分钟后刚刚唤醒")
        };
        for cid in &char_ids {
            crate::memory::unified_event_ledger::register_world_event(
                "system_sleep",
                &text,
                vec!["environment".to_string(), "system_sleep".to_string()],
                now,
                Some(cid),
            );
        }
    });
}
