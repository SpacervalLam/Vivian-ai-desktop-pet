//! 贴边停靠的唯一裁决层。
//!
//! 真源是窗口几何（outer_position / outer_size + 显示器边界），不是 presence 状态，
//! 也不是前端动画的进度。`window.rs` 在每次 SetWindowPos 之后同步调用 [`observe`]，
//! 因此「拖离边缘」不再依赖 60ms 光标采样，也不依赖前端事件的到达顺序。
//!
//! 所有会改写 Win32 region 的 IPC 都通过 [`accept`] 校验 epoch：迟到的动画帧
//! （松手后仍在飞行的 runSlide 采样）会被静默丢弃，不再和 undock 抢同一块 region。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub type Epoch = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockMode {
    /// 完整窗口，未贴边。
    Off,
    /// 正在走到边缘（主动贴边时用，本轮只预留状态与判据）。
    Docking,
    /// 已经压在边缘外，region 已装。
    Docked,
}

#[derive(Clone, Copy, Debug)]
pub struct DockState {
    pub epoch: Epoch,
    pub mode: DockMode,
    pub side: Option<&'static str>,
    /// 进入 Docked 时的实际越界量；之后只要比它小就算离开边缘。
    pub docked_overflow: f64,
    /// Docking 阶段要求的越界量。
    pub target_overflow: f64,
    /// leave_edge 次数：cursor tracking 用它判断这一趟拖拽是否已经拖离过。
    pub undocks: u64,
    /// Docking 开始时刻，供入场卡死的超时兜底判定。
    pub started_at: Option<Instant>,
    /// 入场动画尚未落位。
    ///
    /// 松手贴边后前端还要把窗口从松手位置滑到贴边位姿：如果用户把桌宠**推得比
    /// 贴边位姿更深**，这段动画反而要把窗口往回收，越界量会变小——按「越界变小 =
    /// 离开边缘」裁决就会把刚贴上的桌宠弹回完整窗口（推得越深越容易出现）。
    /// 入场期间只跟踪、不裁决，等前端 settle 或超时后再用实际位姿做阈值。
    pub entering: bool,
}

impl Default for DockState {
    fn default() -> Self {
        Self {
            epoch: 0,
            mode: DockMode::Off,
            side: None,
            docked_overflow: 0.0,
            target_overflow: 0.0,
            undocks: 0,
            started_at: None,
            entering: false,
        }
    }
}

static STATES: OnceLock<Mutex<HashMap<String, DockState>>> = OnceLock::new();

fn states() -> &'static Mutex<HashMap<String, DockState>> {
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, DockState>> {
    states().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn state(label: &str) -> DockState {
    lock().get(label).copied().unwrap_or_default()
}

fn update<R>(label: &str, f: impl FnOnce(&mut DockState) -> R) -> R {
    let mut map = lock();
    f(map.entry(label.to_string()).or_default())
}

/// 本轮几何裁决的结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockAction {
    None,
    /// Docking 阶段窗口已压到边缘外足够多，可以切 Docked。
    ReachedDock,
    /// Docked 阶段窗口不再压在边缘上，必须立刻恢复完整窗口。
    LeftEdge,
}

/// 入场动画的结算结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettleResult {
    /// 不在 Docked / 已经结算过，调用方不用管。
    Ignored,
    /// 入场结束：贴边位姿成为新的「离开边缘」阈值。
    Docked,
    /// 入场结束时窗口已经不在边缘：调用方应 leave_edge + 恢复完整窗口。
    LeftEdge,
}

/// 入场动画落位的结算。`overflow` 为当前几何越界量；None 表示窗口完全在屏内。
pub fn settle(label: &str, overflow: Option<(&'static str, f64)>) -> SettleResult {
    let st = state(label);
    if st.mode != DockMode::Docked || !st.entering {
        return SettleResult::Ignored;
    }
    match overflow {
        Some((side, amount)) if st.side == Some(side) && amount > 0.0 => {
            update(label, |s| {
                s.entering = false;
                s.started_at = None;
                s.docked_overflow = amount;
            });
            SettleResult::Docked
        }
        _ => {
            // 结算只清「入场中」；真正的 leave_edge 由调用方执行（要发事件、清 region）。
            update(label, |s| s.entering = false);
            SettleResult::LeftEdge
        }
    }
}

/// 入场动画卡死兜底：前端没发 settle（动画被打断、事件丢失）时按超时结算。
fn enter_expired(st: &DockState) -> bool {
    st.entering && st.started_at.is_some_and(|at| at.elapsed() >= ENTRY_GRACE)
}

/// 进入 Docked 后等待前端落位结算的最长时间。
const ENTRY_GRACE: Duration = Duration::from_millis(1500);

/// 窗口几何变化后的裁决。`overflow` 是 `edge_overflow(...)` 的结果；None 表示
/// 窗口已完全在屏幕内。只有 Docked 才做「离开边缘」判定，Docking 期间几何
/// 本来就没达标，不能被误判成离开。
pub fn observe(label: &str, overflow: Option<(&'static str, f64)>) -> DockAction {
    let st = state(label);
    match st.mode {
        DockMode::Off => DockAction::None,
        DockMode::Docking => match overflow {
            Some((side, amount)) if st.side == Some(side) && amount >= st.target_overflow => {
                DockAction::ReachedDock
            }
            _ => DockAction::None,
        },
        // 入场动画期间窗口本来就可能在往回收（推得越深收得越多），这一段的
        // 越界量变化不能当成用户把桌宠拉出来。等 settle / 超时后再裁决。
        DockMode::Docked if st.entering => {
            if !enter_expired(&st) {
                return DockAction::None;
            }
            match settle(label, overflow) {
                SettleResult::Docked => DockAction::None,
                _ => DockAction::LeftEdge,
            }
        }
        // 0.5px 容差：SetWindowPos 的整数舍入会让越界量抖 1px，不该被当成离开。
        DockMode::Docked => match overflow {
            Some((side, amount)) if st.side == Some(side) && amount >= st.docked_overflow - 0.5 => {
                DockAction::None
            }
            _ => DockAction::LeftEdge,
        },
    }
}

pub fn begin_docking(label: &str, side: &'static str, target_overflow: f64) -> Epoch {
    update(label, |st| {
        st.epoch += 1;
        st.mode = DockMode::Docking;
        st.side = Some(side);
        st.target_overflow = target_overflow;
        st.docked_overflow = 0.0;
        st.started_at = Some(Instant::now());
        st.entering = false;
        st.epoch
    })
}

pub fn mark_docked(label: &str, side: &'static str, overflow: f64) -> Epoch {
    update(label, |st| {
        st.epoch += 1;
        st.mode = DockMode::Docked;
        st.side = Some(side);
        st.docked_overflow = overflow;
        st.target_overflow = 0.0;
        st.started_at = Some(Instant::now());
        st.entering = true;
        st.epoch
    })
}

/// 跟随贴边动画把窗口推到 dockX 的过程，记录实际达到的最大越界量。
///
/// `mark_docked` 用的是「松手那一刻」的越界量，但贴边动画会把窗口再推出去。
/// 阈值如果停在松手位置，用户把窗口拉回时得先抵消这段动画位移才算离开边缘，
/// 期间一直保持贴边姿态；跟踪实际最大值后，往回拉一点就能撤销。
pub fn update_docked_overflow(label: &str, overflow: f64) -> Option<f64> {
    update(label, |st| {
        if st.mode != DockMode::Docked {
            return None;
        }
        // 入场期间窗口会被动画收回到贴边位姿，这段越界量不是用户意图，不能抬高阈值。
        if st.entering {
            return None;
        }
        if overflow > st.docked_overflow {
            st.docked_overflow = overflow;
        }
        Some(st.docked_overflow)
    })
}

/// Docking 达标后切 Docked，**不推进 epoch**。
/// 入场动画是一整轮，后续仍在飞行的采样帧必须继续被 accept()，否则窗口会停在半路。
/// 只有开启新一轮贴边（begin_docking / mark_docked）或离开边缘才推进 epoch。
pub fn promote_docked(label: &str, side: &'static str, overflow: f64) -> Epoch {
    update(label, |st| {
        if st.mode != DockMode::Docking {
            return st.epoch;
        }
        st.mode = DockMode::Docked;
        st.side = Some(side);
        st.docked_overflow = overflow;
        st.target_overflow = 0.0;
        st.started_at = Some(Instant::now());
        // 达标后前端还会继续滑到 dockX：这一段同样不能按越界变化裁决。
        st.entering = true;
        st.epoch
    })
}

/// 入场是否卡死：只认同一 epoch 的 Docking，换了一轮或已贴好都不算。
pub fn expired(label: &str, epoch: Epoch, timeout: Duration) -> bool {
    let st = state(label);
    st.mode == DockMode::Docking
        && st.epoch == epoch
        && st.started_at.is_some_and(|at| at.elapsed() >= timeout)
}

/// 离开边缘 / 用户中途取消：回 Off 并推进 epoch。返回 None 表示本来就不在贴边。
pub fn leave_edge(label: &str) -> Option<Epoch> {
    leave_edge_matching(label, None)
}

pub fn leave_edge_for_epoch(label: &str, epoch: Epoch) -> Option<Epoch> {
    leave_edge_matching(label, Some(epoch))
}

fn leave_edge_matching(label: &str, epoch: Option<Epoch>) -> Option<Epoch> {
    update(label, |st| {
        if st.mode == DockMode::Off || epoch.is_some_and(|epoch| epoch != st.epoch) {
            return None;
        }
        st.epoch += 1;
        st.mode = DockMode::Off;
        st.side = None;
        st.docked_overflow = 0.0;
        st.target_overflow = 0.0;
        st.started_at = None;
        st.entering = false;
        st.undocks += 1;
        Some(st.epoch)
    })
}

pub fn can_dock(label: &str) -> bool {
    state(label).mode == DockMode::Off
}

/// 位置 / region 写入的准入。
/// - 带有效 epoch：必须是当前 epoch，迟到的动画帧直接丢弃。
/// - 未带 epoch，或 epoch 为 0（`dock:changed` 事件丢失时的兜底）：
///   只在仍处于贴边流程时放行；Off 之后一律拒绝装 region。
/// 真正的保护是 mode：leave_edge 之后无论 epoch 是什么，旧帧都进不来。
pub fn accept(label: &str, epoch: Option<Epoch>) -> bool {
    let st = state(label);
    match epoch {
        Some(candidate) if candidate > 0 => candidate == st.epoch,
        _ => st.mode != DockMode::Off,
    }
}
