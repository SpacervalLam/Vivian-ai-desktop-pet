//! 后台循环看门狗 —— 心跳记录 + 停摆检测 + 自动重启
//!
//! 每个常驻后台循环注册一个名字与期望心跳间隔，循环体内每轮调用 [`beat`]。
//! 看门狗守护任务周期性检查：超过阈值（3× 期望间隔，下限 120 秒）未心跳
//! 判定停摆，error 级报错并调用注册时提供的重启回调重新拉起循环；
//! 无重启回调的循环只报错不重启。
//!
//! - 心跳在注册时初始化为当前时间，进程启动即武装（重启后恢复检查语义）
//! - 事件驱动型循环（如 SpeechPlanner pump、NetworkWatch）无固定心跳间隔，不应注册
//! - 重启回调需自行检查取消信号，避免退出阶段被拉起

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Serialize;

type RestartFn = Arc<dyn Fn() + Send + Sync>;

struct LoopEntry {
    expected_interval: f64,
    last_beat: Mutex<f64>,
    restart: Option<RestartFn>,
    restart_count: Mutex<u32>,
    /// 当前这次停摆是否已报过警（心跳恢复时复位，避免刷屏）
    stalled_notified: Mutex<bool>,
}

static REGISTRY: Lazy<Mutex<HashMap<String, LoopEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn now_ts() -> f64 {
    chrono::Utc::now().timestamp() as f64
}

/// 判定停摆的阈值：3× 期望间隔，下限 120 秒
fn stall_threshold(expected: f64) -> f64 {
    (expected * 3.0).max(120.0)
}

/// 注册后台循环。重复注册同名循环时刷新期望间隔与重启回调，保留心跳。
pub fn register(name: &str, expected_interval_secs: f64, restart: Option<RestartFn>) {
    let mut reg = REGISTRY.lock();
    let entry = reg.entry(name.to_string()).or_insert_with(|| LoopEntry {
        expected_interval: expected_interval_secs,
        last_beat: Mutex::new(now_ts()),
        restart: None,
        restart_count: Mutex::new(0),
        stalled_notified: Mutex::new(false),
    });
    entry.expected_interval = expected_interval_secs;
    entry.restart = restart;
}

/// 循环心跳：每轮循环体调用一次
pub fn beat(name: &str) {
    let reg = REGISTRY.lock();
    if let Some(e) = reg.get(name) {
        *e.last_beat.lock() = now_ts();
        *e.stalled_notified.lock() = false;
    }
}

/// 注销循环（循环自然退出时调用，避免看门狗误报停摆）
pub fn unregister(name: &str) {
    REGISTRY.lock().remove(name);
}

/// 停摆检查：超时未心跳 → error 报警（每次停摆只报一次）→ 调用重启回调
fn check_all() {
    // 先在锁内收集需要重启的动作，再在锁外执行（回调可能再次 register）
    let mut restarts: Vec<(String, RestartFn)> = Vec::new();
    {
        let reg = REGISTRY.lock();
        let now = now_ts();
        for (name, e) in reg.iter() {
            let elapsed = now - *e.last_beat.lock();
            let threshold = stall_threshold(e.expected_interval);
            if elapsed <= threshold {
                continue;
            }
            let mut notified = e.stalled_notified.lock();
            if *notified {
                continue;
            }
            *notified = true;
            drop(notified);
            tracing::error!(
                "[Watchdog] 后台循环 `{name}` 停摆：{elapsed:.0}s 无心跳（阈值 {threshold:.0}s）"
            );
            if let Some(restart) = &e.restart {
                *e.last_beat.lock() = now;
                let mut rc = e.restart_count.lock();
                *rc += 1;
                tracing::info!("[Watchdog] 重启循环 `{name}`（累计重启 {} 次）", *rc);
                restarts.push((name.clone(), Arc::clone(restart)));
            }
        }
    }
    for (_, restart) in restarts {
        restart();
    }
}

/// 循环状态快照（供健康接口读取）
#[derive(Debug, Clone, Serialize)]
pub struct LoopStatus {
    pub name: String,
    /// 距上次心跳的秒数
    pub last_beat_age_secs: f64,
    pub expected_interval_secs: f64,
    pub restart_count: u32,
    pub stalled: bool,
}

/// 全部循环的状态快照
pub fn snapshot() -> Vec<LoopStatus> {
    let reg = REGISTRY.lock();
    let now = now_ts();
    reg.iter()
        .map(|(name, e)| {
            let age = now - *e.last_beat.lock();
            LoopStatus {
                name: name.clone(),
                last_beat_age_secs: age,
                expected_interval_secs: e.expected_interval,
                restart_count: *e.restart_count.lock(),
                stalled: age > stall_threshold(e.expected_interval),
            }
        })
        .collect()
}

/// 启动看门狗守护任务（全局一次）
pub fn spawn_daemon(check_interval: Duration) {
    tauri::async_runtime::spawn(async move {
        let cancel = crate::utils::cancel_token::cancel_token();
        tracing::info!("[Watchdog] 看门狗已启动（检查间隔 {:?}）", check_interval);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(check_interval) => {}
                _ = cancel.cancelled() => {
                    tracing::info!("[Watchdog] 收到取消信号，退出");
                    return;
                }
            }
            check_all();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_updates_and_stall_detected() {
        register("test-loop", 10.0, None);
        beat("test-loop");
        // 刚注册 + 心跳：不停摆
        assert!(snapshot().into_iter().all(|s| !s.stalled));

        // 模拟长时间无心跳：直接改注册表中的时间戳
        {
            let reg = REGISTRY.lock();
            let e = reg.get("test-loop").unwrap();
            *e.last_beat.lock() = now_ts() - 500.0;
        }
        let snap = snapshot();
        let s = snap.iter().find(|s| s.name == "test-loop").unwrap();
        assert!(s.stalled);
        assert!(s.last_beat_age_secs > 400.0);
    }
}
