//! 凝神/专注模式状态机 — 离散认知模式切换。
//!
//! 在心理学数值模型之上叠加一层认知模式状态机：漏桶累积器 + 迟滞设计，
//! 在用户情绪脆弱或问题复杂时进入 Focus 模式，开启思考（thinking-on）
//! 并提升 max_tokens 余量。
//!
//! 三种认知模式：
//! - `Regular`：日常轻量基线（思考关闭）
//! - `Focus`：信号触发，开启思考 + 提升余量
//! - `TrueName`：v2 破坏性层级（预留，v1 不触达）

use serde::{Deserialize, Serialize};

/// 认知模式枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitionMode {
    /// 日常轻量基线（思考全局关闭）
    Regular,
    /// 信号触发，开启思考 + 提升余量
    Focus,
    /// v2 破坏性层级（persona/memory 重写），v1 不触达
    TrueName,
}

impl Default for CognitionMode {
    fn default() -> Self {
        CognitionMode::Regular
    }
}

impl CognitionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            CognitionMode::Regular => "regular",
            CognitionMode::Focus => "focus",
            CognitionMode::TrueName => "true_name",
        }
    }

    pub fn is_focus(&self) -> bool {
        matches!(self, CognitionMode::Focus)
    }
}

/// 凝神模式阈值常量。
#[derive(Debug, Clone)]
pub struct FocusThresholds {
    /// 总开关
    pub enabled: bool,
    /// inline 路径每轮保留率（每轮留 50%、漏 50%）
    pub retention: f64,
    /// 进入阈值 = "完全激活"点
    pub enter: f64,
    /// 退出阈值（迟滞低门，须 < enter）
    pub exit: f64,
    /// 电荷天花板（≥ enter）
    pub cap: f64,
    /// 单次凝神最多持续 inline 轮数
    pub hard_cap_turns: u32,
    /// charge < enter 时每秒时间衰减（满电荷约 5 分钟漏完）
    pub time_decay_per_sec: f64,
    /// charge ≥ enter 时每秒时间衰减（减半，约 10 分钟漏完）
    pub time_decay_activated: f64,
    /// proactive 沉默轮的电荷保留率
    pub idle_silent_retention: f64,
    /// proactive 开口轮的电荷保留率（须 ≤ silent）
    pub idle_replied_retention: f64,
    /// Focus 模式下 max_tokens 余量加成
    pub thinking_extra_tokens: u32,
}

impl Default for FocusThresholds {
    fn default() -> Self {
        Self {
            enabled: true,
            retention: 0.5,
            enter: 0.6,
            exit: 0.3,
            cap: 1.0,
            hard_cap_turns: 8,
            time_decay_per_sec: 0.0033,
            time_decay_activated: 0.0017,
            idle_silent_retention: 0.8,
            idle_replied_retention: 0.8,
            thinking_extra_tokens: 800,
        }
    }
}

/// Focus 决策动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusAction {
    /// 进入 Focus 模式
    Enter,
    /// 退出 Focus 模式
    Exit,
    /// 保持当前模式
    Stay,
}

/// 退出原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    /// 电荷衰减到退出线以下
    Decayed,
    /// 达到硬顶轮数
    HardCap,
    /// 话题切换
    TopicSwitch,
}

impl ExitReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExitReason::Decayed => "decayed",
            ExitReason::HardCap => "hard_cap",
            ExitReason::TopicSwitch => "topic_switch",
        }
    }
}

/// `_focus_decide` 的输出。
#[derive(Debug, Clone)]
pub struct FocusDecision {
    pub action: FocusAction,
    pub turn_count: u32,
    pub charge: f64,
    pub reason: Option<ExitReason>,
}

impl FocusDecision {
    fn stay(turn_count: u32, charge: f64) -> Self {
        Self {
            action: FocusAction::Stay,
            turn_count,
            charge,
            reason: None,
        }
    }

    fn enter(charge: f64) -> Self {
        Self {
            action: FocusAction::Enter,
            turn_count: 1,
            charge,
            reason: None,
        }
    }

    fn exit(reason: ExitReason) -> Self {
        Self {
            action: FocusAction::Exit,
            turn_count: 0,
            charge: 0.0,
            reason: Some(reason),
        }
    }
}

/// 时间衰减（惰性计算，无 ticker）。
///
/// 一旦激活（charge ≥ enter），时间衰减只能把电荷降到 enter 线为止，
/// 绝不靠时间降到 enter 以下——退出激活必须靠一轮对话的 retention。
pub fn decay_charge_over_time(charge: f64, elapsed_secs: f64, th: &FocusThresholds) -> f64 {
    if charge <= 0.0 || elapsed_secs <= 0.0 {
        return charge.max(0.0);
    }

    if charge >= th.enter {
        // 慢速衰减，地板 = enter
        (charge - th.time_decay_activated * elapsed_secs).max(th.enter)
    } else {
        // 快速衰减，地板 = 0
        (charge - th.time_decay_per_sec * elapsed_secs).max(0.0)
    }
}

/// 核心决策纯函数。
///
/// 漏桶累积器：`new_charge = max(0.0, min(charge * retention + score, cap))`
///
/// - `score` 可为负（正效价情绪减 Focus），下限 `max(0.0, …)` 把好情绪的电荷漏到 0
/// - `count_turn=True`（inline 路径）消耗 hard_cap slot；`False`（idle 冷却）不消耗
pub fn focus_decide(
    mode: CognitionMode,
    focus_turn_count: u32,
    charge: f64,
    score: f64,
    topic_changed: bool,
    count_turn: bool,
    th: &FocusThresholds,
) -> FocusDecision {
    // TrueName (v2) — 不动作
    if mode == CognitionMode::TrueName {
        return FocusDecision::stay(focus_turn_count, charge);
    }

    // 话题切换：FOCUS 时 EXIT + 清零；REGULAR 时清旧累积器但用本轮 score 重新种子化
    if topic_changed {
        if mode == CognitionMode::Focus {
            return FocusDecision::exit(ExitReason::TopicSwitch);
        }
        // REGULAR: 丢弃旧累积器，用本轮 score 重新种子化（不漏电）
        let new_charge = score.clamp(0.0, th.cap);
        if new_charge >= th.enter {
            return FocusDecision::enter(new_charge);
        }
        return FocusDecision::stay(focus_turn_count, new_charge);
    }

    // 正常轮（topic_changed = false）
    let new_charge = (charge * th.retention + score).clamp(0.0, th.cap);

    match mode {
        CognitionMode::Regular => {
            if new_charge >= th.enter {
                FocusDecision::enter(new_charge)
            } else {
                FocusDecision::stay(focus_turn_count, new_charge)
            }
        }
        CognitionMode::Focus => {
            if focus_turn_count >= th.hard_cap_turns {
                FocusDecision::exit(ExitReason::HardCap)
            } else if new_charge < th.exit {
                FocusDecision::exit(ExitReason::Decayed)
            } else {
                let next_turn = if count_turn {
                    focus_turn_count + 1
                } else {
                    focus_turn_count
                };
                FocusDecision::stay(next_turn, new_charge)
            }
        }
        CognitionMode::TrueName => FocusDecision::stay(focus_turn_count, charge),
    }
}

/// 从本轮输入计算 Focus 信号评分。
///
/// 正向信号（推向 Focus）：输入较长、含问号、含复杂度关键词、用户情绪偏负面。
/// 负向信号（拉回 Regular）：短输入、轻松情绪。
///
/// 评分可为负，传给 `focus_decide` 后正效价情绪会把电荷漏向 0。
pub fn compute_focus_score(user_input: &str, user_emotion: &str) -> f64 {
    let chars = user_input.chars().count() as f64;
    let mut score: f64 = 0.0;

    // 长度信号：长输入暗示复杂问题
    if chars > 150.0 {
        score += 0.4;
    } else if chars > 50.0 {
        score += 0.2;
    } else if chars < 8.0 {
        score -= 0.2;
    }

    // 问号信号
    if user_input.contains('?') || user_input.contains('？') {
        score += 0.2;
    }

    // 复杂度关键词
    let lower = user_input.to_lowercase();
    for kw in &[
        "为什么", "解释", "分析", "帮我理解", "怎么办", "区别", "原理", "如何",
        "why", "explain", "analyze", "difference", "how",
    ] {
        if lower.contains(kw) {
            score += 0.2;
            break;
        }
    }

    // 情绪信号：负面情绪推向 Focus（用户需要更专注的陪伴）
    let emo = user_emotion.to_lowercase();
    if matches!(
        emo.as_str(),
        "sad" | "anxious" | "angry" | "fear" | "confused" | "lonely" | "悲伤" | "焦虑" | "愤怒" | "害怕" | "困惑" | "孤独"
    ) {
        score += 0.3;
    } else if matches!(emo.as_str(), "happy" | "joy" | "excited" | "开心" | "快乐") {
        score -= 0.2;
    }

    score
}

/// 凝神模式运行时状态。
#[derive(Debug, Clone)]
pub struct FocusState {
    /// 当前认知模式
    pub mode: CognitionMode,
    /// 当前电荷值
    pub charge: f64,
    /// 最近一次电荷变化的时间戳（秒）
    pub charge_at: f64,
    /// 当前 Focus episode 的 inline 轮计数
    pub turn_count: u32,
    /// 当前 Focus episode ID
    pub episode_id: Option<String>,
    /// 当前 Focus episode 开始时间戳
    pub episode_started_at: Option<f64>,
}

impl Default for FocusState {
    fn default() -> Self {
        Self {
            mode: CognitionMode::Regular,
            charge: 0.0,
            charge_at: 0.0,
            turn_count: 0,
            episode_id: None,
            episode_started_at: None,
        }
    }
}

impl FocusState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_focus(&self) -> bool {
        self.mode.is_focus()
    }

    /// 更新凝神状态（主入口）。
    ///
    /// - `score`：本轮信号评分（可为负）
    /// - `topic_changed`：是否发生话题切换
    /// - `count_turn`：inline 路径为 true（消耗 hard_cap slot），idle 路径为 false
    /// - `now`：当前时间戳（秒）
    /// - `th`：阈值常量
    ///
    /// 返回决策结果（动作 + 新电荷 + 退出原因）。
    pub fn update(
        &mut self,
        score: f64,
        topic_changed: bool,
        count_turn: bool,
        now: f64,
        th: &FocusThresholds,
    ) -> FocusDecision {
        if !th.enabled {
            self.clear();
            return FocusDecision::stay(0, 0.0);
        }

        // 时间衰减（惰性计算）
        if self.charge > 0.0 && self.charge_at > 0.0 {
            let elapsed = (now - self.charge_at).max(0.0);
            self.charge = decay_charge_over_time(self.charge, elapsed, th);
        }
        self.charge_at = now;

        let decision = focus_decide(
            self.mode,
            self.turn_count,
            self.charge,
            score,
            topic_changed,
            count_turn,
            th,
        );

        // 应用决策
        self.charge = decision.charge;
        match decision.action {
            FocusAction::Enter => {
                self.mode = CognitionMode::Focus;
                self.turn_count = decision.turn_count;
                self.episode_id = Some(format!("focus-{}", (now * 1000.0) as u64));
                self.episode_started_at = Some(now);
            }
            FocusAction::Exit => {
                self.mode = CognitionMode::Regular;
                self.turn_count = 0;
                self.episode_id = None;
                self.episode_started_at = None;
            }
            FocusAction::Stay => {
                self.turn_count = decision.turn_count;
            }
        }

        decision
    }

    /// idle 路径冷却（不评分，只衰减）。
    ///
    /// - `replied`：proactive 是否真开口
    /// - `now`：当前时间戳
    /// - `th`：阈值常量
    pub fn idle_cooldown(&mut self, replied: bool, now: f64, th: &FocusThresholds) {
        if self.mode != CognitionMode::Focus {
            return;
        }

        let retention = if replied {
            th.idle_replied_retention
        } else {
            th.idle_silent_retention
        };

        // 时间衰减 + idle retention 衰减，不消耗 hard_cap slot
        if self.charge > 0.0 && self.charge_at > 0.0 {
            let elapsed = (now - self.charge_at).max(0.0);
            self.charge = decay_charge_over_time(self.charge, elapsed, th);
        }
        self.charge = self.charge * retention;
        self.charge_at = now;

        // 若衰减到退出线以下，退出 Focus
        if self.charge < th.exit {
            self.clear();
        }
    }

    /// 清零 Focus 状态（静默，不发事件）。
    pub fn clear(&mut self) {
        self.mode = CognitionMode::Regular;
        self.charge = 0.0;
        self.turn_count = 0;
        self.episode_id = None;
        self.episode_started_at = None;
    }

    /// 重置（与新实例等价）。
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th() -> FocusThresholds {
        FocusThresholds::default()
    }

    #[test]
    fn decay_below_enter_goes_to_zero() {
        let th = th();
        // charge=0.5 (< enter=0.6), 30s elapsed → 0.5 - 0.02*30 = 0.5 - 0.6 = -0.1 → 0
        let result = decay_charge_over_time(0.5, 30.0, &th);
        assert!((result - 0.0).abs() < 0.001);
    }

    #[test]
    fn decay_above_enter_floors_at_enter() {
        let th = th();
        // charge=0.8 (≥ enter=0.6), 100s elapsed → 0.8 - 0.01*100 = 0.8 - 1.0 = -0.2 → enter=0.6
        let result = decay_charge_over_time(0.8, 100.0, &th);
        assert!((result - th.enter).abs() < 0.001);
    }

    #[test]
    fn regular_accumulate_then_enter() {
        let th = th();
        // score=0.3, charge=0 → 0*0.5+0.3=0.3 < enter → Stay
        let d = focus_decide(CognitionMode::Regular, 0, 0.0, 0.3, false, true, &th);
        assert_eq!(d.action, FocusAction::Stay);
        assert!((d.charge - 0.3).abs() < 0.001);

        // score=0.4, charge=0.3 → 0.3*0.5+0.4=0.55 < enter → Stay
        let d = focus_decide(CognitionMode::Regular, 0, 0.3, 0.4, false, true, &th);
        assert_eq!(d.action, FocusAction::Stay);
        assert!((d.charge - 0.55).abs() < 0.001);

        // score=0.4, charge=0.55 → 0.55*0.5+0.4=0.675 ≥ enter → Enter
        let d = focus_decide(CognitionMode::Regular, 0, 0.55, 0.4, false, true, &th);
        assert_eq!(d.action, FocusAction::Enter);
        assert!((d.charge - 0.675).abs() < 0.001);
        assert_eq!(d.turn_count, 1);
    }

    #[test]
    fn focus_exit_on_decay() {
        let th = th();
        // charge=0.2 (< exit=0.3) → Exit
        let d = focus_decide(CognitionMode::Focus, 3, 0.2, 0.0, false, true, &th);
        assert_eq!(d.action, FocusAction::Exit);
        assert_eq!(d.reason, Some(ExitReason::Decayed));
    }

    #[test]
    fn focus_exit_on_hard_cap() {
        let th = th();
        let d = focus_decide(CognitionMode::Focus, 8, 0.8, 0.0, false, true, &th);
        assert_eq!(d.action, FocusAction::Exit);
        assert_eq!(d.reason, Some(ExitReason::HardCap));
    }

    #[test]
    fn focus_exit_on_topic_switch() {
        let th = th();
        let d = focus_decide(CognitionMode::Focus, 3, 0.8, 0.0, true, true, &th);
        assert_eq!(d.action, FocusAction::Exit);
        assert_eq!(d.reason, Some(ExitReason::TopicSwitch));
    }

    #[test]
    fn topic_switch_regular_reseeds() {
        let th = th();
        // topic_changed=true, score=0.7 ≥ enter → Enter
        let d = focus_decide(CognitionMode::Regular, 0, 0.5, 0.7, true, true, &th);
        assert_eq!(d.action, FocusAction::Enter);
        assert!((d.charge - 0.7).abs() < 0.001);
    }

    #[test]
    fn negative_score_reduces_charge() {
        let th = th();
        // score=-0.3, charge=0.5 → 0.5*0.5+(-0.3)=0.25-0.3=-0.05 → max(0, -0.05)=0
        let d = focus_decide(CognitionMode::Regular, 0, 0.5, -0.3, false, true, &th);
        assert_eq!(d.action, FocusAction::Stay);
        assert!((d.charge - 0.0).abs() < 0.001);
    }

    #[test]
    fn count_turn_false_does_not_consume_slot() {
        let th = th();
        let d = focus_decide(CognitionMode::Focus, 3, 0.8, 0.0, false, false, &th);
        assert_eq!(d.action, FocusAction::Stay);
        assert_eq!(d.turn_count, 3); // 不增加
    }

    #[test]
    fn count_turn_true_increments() {
        let th = th();
        let d = focus_decide(CognitionMode::Focus, 3, 0.8, 0.0, false, true, &th);
        assert_eq!(d.action, FocusAction::Stay);
        assert_eq!(d.turn_count, 4);
    }

    #[test]
    fn state_update_enters_focus() {
        let th = th();
        let mut state = FocusState::new();
        let d = state.update(0.7, false, true, 1000.0, &th);
        assert_eq!(d.action, FocusAction::Enter);
        assert!(state.is_focus());
        assert!(state.episode_id.is_some());
    }

    #[test]
    fn state_update_exits_on_topic_switch() {
        let th = th();
        let mut state = FocusState::new();
        state.update(0.7, false, true, 1000.0, &th);
        assert!(state.is_focus());
        let d = state.update(0.0, true, true, 1001.0, &th);
        assert_eq!(d.action, FocusAction::Exit);
        assert!(!state.is_focus());
    }

    #[test]
    fn idle_cooldown_does_not_consume_slot() {
        let th = th();
        let mut state = FocusState::new();
        state.update(0.7, false, true, 1000.0, &th);
        let turns_before = state.turn_count;
        state.idle_cooldown(false, 1005.0, &th);
        assert_eq!(state.turn_count, turns_before);
    }
}
