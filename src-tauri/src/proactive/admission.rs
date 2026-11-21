//! 主动交互的「准入 + 去向」层。
//!
//! 借鉴 Super-agent 的事件驱动设计：每个候选主动行为先被写成一条不可变的
//! [`TriggerEvent`]，由确定性快路径判断「此刻能不能说」（origin / 在场 / 发言权 /
//! 本轮预算），通过后才进入既有 `check_trigger` 与 LLM 生成的慢路径。
//!
//! 每一次判断都写一条 [`Disposition`]（去向）：触发是什么、属于哪条 Thread、
//! 结果是被采纳、被静音、被顺延还是已投递，以及原因码。这样「她这次为什么没说话」
//! 不再只能靠日志猜，而是随 `get_proactive_status` 一起可读。
//!
//! 两条刻意的取舍：
//! - 去向里只放摘要与事件 ID，不放原文（上下文窗口当缓存用，完整材料仍在记忆库）。
//! - 同一 (trigger, thread, outcome, reason) 在短窗口内折叠成一条并累加 `count`，
//!   避免每 10 秒的 tick 把台账刷成噪声。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::Serialize;

use super::triggers::ProactiveTrigger;

/// 同一条去向在多少秒内重复时折叠计数。
const COLLAPSE_SECS: f64 = 120.0;
/// 单条去向保留的摘要字符数（完整材料在别处，台账只留引用）。
const MAX_DETAIL_CHARS: usize = 80;
/// 每个角色默认保留的去向条数。
pub const DEFAULT_CAPACITY: usize = 400;

/// 事件从哪来：决定这轮预算里它能不能插队。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// 用户直接动作（拖拽、点击）。
    Human,
    /// 室友发言、旁观对话。
    Peer,
    /// 工作侧交付：后台完成了，或卡在等人。
    Work,
    /// 环境事实（窗口、音乐、系统压力、日出日落）。
    Ambient,
    /// 只是时钟/规则到了（问候、破冰、回忆）。
    Timer,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Peer => "peer",
            Self::Work => "work",
            Self::Ambient => "ambient",
            Self::Timer => "timer",
        }
    }

    /// 由用户直接引发、或已经停下等用户的事：可以占用额外的即时机会。
    /// 环境事实与时钟规则不在此列，避免一轮里连说两句无关的闲话。
    pub fn immediate(self) -> bool {
        matches!(self, Self::Human | Self::Peer | Self::Work)
    }
}

/// 同一类事件流，去向按 Thread 归组（对应 Super-agent 的 Thread 概念）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Thread {
    /// 和用户本人的对话。
    User,
    /// 室友/旁观对话。
    Peer,
    /// 工作会话（后台任务、待转达）。
    Work,
    /// 前台窗口这条线。
    Window,
    /// 音乐播放这条线。
    Music,
    /// 同一应用持续使用这条线。
    App,
    /// 设备健康（内存、电量类）。
    Device,
    /// 日常关怀（熬夜、健康提醒、日出日落）。
    Care,
    /// 角色自己的思绪周期。
    Own,
}

impl Thread {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Peer => "peer",
            Self::Work => "work",
            Self::Window => "window",
            Self::Music => "music",
            Self::App => "app",
            Self::Device => "device",
            Self::Care => "care",
            Self::Own => "own",
        }
    }
}

/// 触发器属于哪条 Thread、来自谁。
pub fn classify(trigger: ProactiveTrigger) -> (Origin, Thread) {
    use ProactiveTrigger::*;
    match trigger {
        TeasingResponse => (Origin::Human, Thread::User),
        WelcomeBack => (Origin::Human, Thread::User),
        CrossCharacterReply | BystanderInterjection => (Origin::Peer, Thread::Peer),
        WorkNotice => (Origin::Work, Thread::Work),
        WindowTrigger | ScreenPeek => (Origin::Ambient, Thread::Window),
        MusicChanged => (Origin::Ambient, Thread::Music),
        AppDuration => (Origin::Ambient, Thread::App),
        SystemPressure => (Origin::Ambient, Thread::Device),
        LateNight | HealthReminder | Sunrise | Sunset => (Origin::Ambient, Thread::Care),
        HourlyGreeting | IdleGreeting | Icebreaker | TopicExtension | MemoryRecall | Spontaneous
        | MoodDriven => (Origin::Timer, Thread::Own),
    }
}

/// 一条候选主动事件。
#[derive(Debug, Clone)]
pub struct TriggerEvent {
    /// 便于追溯的稳定标识（按触发器 + 线程 + 整秒生成，不用于去重）。
    pub id: String,
    pub trigger: ProactiveTrigger,
    pub origin: Origin,
    pub thread: Thread,
    /// 事件被观察到的时刻（Unix 秒）。
    pub at: f64,
}

impl TriggerEvent {
    pub fn new(trigger: ProactiveTrigger, at: f64) -> Self {
        let (origin, thread) = classify(trigger);
        Self {
            id: format!("trg-{}-{}-{}", trigger.as_str(), thread.as_str(), at as i64),
            trigger,
            origin,
            thread,
            at,
        }
    }
}

/// 决策结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// 取到机会，进入生成。
    Admitted,
    /// 已经投递。
    Delivered,
    /// 确定性快路径判定此刻不说。
    Silenced,
    /// 本轮预算已满，留到下一轮。
    Deferred,
    /// 生成被模型弃权或失败。
    Aborted,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Delivered => "delivered",
            Self::Silenced => "silenced",
            Self::Deferred => "deferred",
            Self::Aborted => "aborted",
        }
    }
}

/// 原因码：只记稳定枚举，不记自由文本，便于统计与调参。
///
/// 分层顺序与门控顺序一致：先看「该不该在这个时刻开口」，再看「这个触发器自己
/// 有没有条件」，最后才是「生成有没有产出」。整轮级去向只会保留主导的那条原因码，
/// 具体是哪些触发器被哪个门挡下写在 `detail` 里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Ok,
    /// 主动交互整体关闭。
    Disabled,
    /// 安静模式窗口内。
    QuietMode,
    /// 防打扰 / 主导驱动要求本轮安静。
    LayLow,
    /// 用户不在屏幕前。
    UserAway,
    /// 本轮发言权不在本角色（由思考决策决定）。
    NotLeader,
    /// 用户正在对话中。
    UserChatting,
    /// 自己刚说过话的静默期（含普通回复落地、跨类型发言间隔、问候静默）。
    SettleWindow,
    /// 触发器自己的冷却时间没到。
    Cooldown,
    /// 说话欲望低于角色阈值（问候类）。
    DesireLow,
    /// 时机分数低于阈值。
    TimingLow,
    /// 概率门控没中（含触发类型亲和度、学习修正）。
    Probability,
    /// 多级冷却系数过低，被整体压制。
    Cooling,
    /// 触发器自身条件不满足（空闲不够、没离开过、没有旁观记忆等），
    /// 或该路径由事件驱动、不走常规门控。
    Specific,
    /// 兜底：门控拒绝但没归到具体原因。
    GateRejected,
    /// 本轮已有发言名额。
    BudgetFull,
    /// 模型弃权或生成失败。
    GenerationDeclined,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Disabled => "disabled",
            Self::QuietMode => "quiet_mode",
            Self::LayLow => "lay_low",
            Self::UserAway => "user_away",
            Self::NotLeader => "not_leader",
            Self::UserChatting => "user_chatting",
            Self::SettleWindow => "settle_window",
            Self::Cooldown => "cooldown",
            Self::DesireLow => "desire_low",
            Self::TimingLow => "timing_low",
            Self::Probability => "probability",
            Self::Cooling => "cooling",
            Self::Specific => "specific",
            Self::GateRejected => "gate_rejected",
            Self::BudgetFull => "budget_full",
            Self::GenerationDeclined => "generation_declined",
        }
    }
}

/// 把一轮里被门控挡下的触发器按原因归组，挑出次数最多的那条。
///
/// 返回 `(主导原因, 摘要)`，摘要形如 `cooldown: hourly_greeting, icebreaker · timing_low: spontaneous`，
/// 供整轮级去向使用——一次 tick 只留一条记录，而不是让二十多个触发器各自刷屏。
pub fn dominant_rejection(rejections: &[(&'static str, Reason)]) -> Option<(Reason, String)> {
    if rejections.is_empty() {
        return None;
    }
    let mut groups: Vec<(Reason, u32, Vec<&'static str>)> = Vec::new();
    for (trigger, reason) in rejections {
        match groups.iter_mut().find(|(r, _, _)| *r == *reason) {
            Some((_, count, names)) => {
                *count += 1;
                names.push(*trigger);
            }
            None => groups.push((*reason, 1, vec![*trigger])),
        }
    }
    // 稳定排序：次数相同时保留首次出现的顺序，便于对照门控顺序排查。
    groups.sort_by(|a, b| b.1.cmp(&a.1));
    let dominant = groups[0].0;
    let detail = groups
        .iter()
        .map(|(reason, _, names)| format!("{}: {}", reason.as_str(), names.join(", ")))
        .collect::<Vec<_>>()
        .join(" · ");
    Some((dominant, detail))
}

/// 一条去向记录。
#[derive(Debug, Clone, Serialize)]
pub struct Disposition {
    /// 事件 ID（链式召回的入口）。
    pub id: String,
    pub at: f64,
    pub trigger: String,
    pub thread: &'static str,
    pub origin: &'static str,
    pub outcome: &'static str,
    pub reason: &'static str,
    /// 内联摘要；完整材料不复制进台账。
    pub detail: String,
    /// 同一去向在折叠窗口内出现的次数。
    pub count: u32,
}

/// 按 (outcome, reason) 聚合的计数，供调参时看「主要卡在哪一层」。
#[derive(Debug, Clone, Serialize)]
pub struct SummaryRow {
    pub outcome: &'static str,
    pub reason: &'static str,
    pub count: u32,
}

/// 每个角色一份的有界去向台账。
pub struct DispositionLog {
    capacity: usize,
    entries: VecDeque<Disposition>,
}

impl DispositionLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
        }
    }

    /// 记录一条事件级去向（有具体触发器）。
    pub fn record(
        &mut self,
        event: &TriggerEvent,
        outcome: Outcome,
        reason: Reason,
        detail: &str,
        now: f64,
    ) -> &Disposition {
        self.record_raw(
            event.id.clone(),
            event.trigger.as_str(),
            event.origin,
            event.thread,
            outcome,
            reason,
            detail,
            now,
        )
    }

    /// 记录一条整轮级去向：一次 tick 里「所有普通机会都被同一原因挡下」时只留一条，
    /// 不按触发器逐个刷屏。
    #[allow(clippy::too_many_arguments)]
    pub fn record_raw(
        &mut self,
        id: String,
        trigger: &'static str,
        origin: Origin,
        thread: Thread,
        outcome: Outcome,
        reason: Reason,
        detail: &str,
        now: f64,
    ) -> &Disposition {
        let detail = crate::utils::truncate_chars(detail.trim(), MAX_DETAIL_CHARS);
        let same_as_last = self.entries.back().is_some_and(|last| {
            last.trigger == trigger
                && last.thread == thread.as_str()
                && last.origin == origin.as_str()
                && last.outcome == outcome.as_str()
                && last.reason == reason.as_str()
                && now - last.at <= COLLAPSE_SECS
        });
        if same_as_last {
            let last = self.entries.back_mut().expect("checked above");
            last.at = now;
            last.count = last.count.saturating_add(1);
            if !detail.is_empty() {
                last.detail = detail;
            }
        } else {
            if self.entries.len() >= self.capacity {
                self.entries.pop_front();
            }
            self.entries.push_back(Disposition {
                id,
                at: now,
                trigger: trigger.to_string(),
                thread: thread.as_str(),
                origin: origin.as_str(),
                outcome: outcome.as_str(),
                reason: reason.as_str(),
                detail,
                count: 1,
            });
        }
        self.entries.back().expect("just pushed")
    }

    /// 最近 `limit` 条，最新在前。
    pub fn recent(&self, limit: usize) -> Vec<Disposition> {
        self.entries.iter().rev().take(limit).cloned().collect()
    }

    /// `window_secs` 内按 (outcome, reason) 聚合，次数多的在前。
    pub fn summary(&self, now: f64, window_secs: f64) -> Vec<SummaryRow> {
        let mut rows: HashMap<(&'static str, &'static str), u32> = HashMap::new();
        for entry in &self.entries {
            if now - entry.at > window_secs {
                continue;
            }
            *rows.entry((entry.outcome, entry.reason)).or_default() += entry.count;
        }
        let mut rows: Vec<SummaryRow> = rows
            .into_iter()
            .map(|((outcome, reason), count)| SummaryRow {
                outcome,
                reason,
                count,
            })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then(a.reason.cmp(b.reason)));
        rows
    }

    /// 是否已有过投递记录（测试与调试用）。
    pub fn delivered_count(&self) -> u32 {
        self.entries
            .iter()
            .filter(|e| e.outcome == Outcome::Delivered.as_str())
            .map(|e| e.count)
            .sum()
    }
}

static LOGS: Lazy<RwLock<HashMap<String, Arc<RwLock<DispositionLog>>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// 取（必要时创建）某角色的去向台账。
pub fn log_for(character_id: &str) -> Arc<RwLock<DispositionLog>> {
    if let Some(log) = LOGS.read().get(character_id) {
        return Arc::clone(log);
    }
    let mut logs = LOGS.write();
    Arc::clone(
        logs.entry(character_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(DispositionLog::new(DEFAULT_CAPACITY)))),
    )
}

/// 记录一条去向。
pub fn record(
    character_id: &str,
    event: &TriggerEvent,
    outcome: Outcome,
    reason: Reason,
    detail: &str,
    now: f64,
) {
    log_for(character_id).write().record(event, outcome, reason, detail, now);
}

/// 记录一条整轮级去向：没有具体触发器时（例如「用户不在，面向用户的机会全部顺延」），
/// 用一条聚合记录代替逐触发器刷屏。
pub fn record_tick(
    character_id: &str,
    thread: Thread,
    outcome: Outcome,
    reason: Reason,
    detail: &str,
    now: f64,
) {
    log_for(character_id).write().record_raw(
        format!("tick-{}-{}", thread.as_str(), now as i64),
        "tick",
        Origin::Timer,
        thread,
        outcome,
        reason,
        detail,
        now,
    );
}

/// 给 `get_proactive_status` 用的快照：最近去向 + 一小时内聚合。
pub fn snapshot(character_id: &str, now: f64) -> serde_json::Value {
    let log = log_for(character_id);
    let log = log.read();
    serde_json::json!({
        "recent": log.recent(12),
        "summary": log.summary(now, 3600.0),
    })
}

/// 清空某角色的台账（恢复初始状态时使用）。
pub fn forget(character_id: &str) {
    LOGS.write().remove(character_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: f64) -> f64 {
        secs
    }

    #[test]
    fn origin_and_thread_map_to_event_sources() {
        let (origin, thread) = classify(ProactiveTrigger::TeasingResponse);
        assert_eq!(origin, Origin::Human);
        assert_eq!(thread, Thread::User);
        assert!(origin.immediate());

        let (origin, thread) = classify(ProactiveTrigger::WorkNotice);
        assert_eq!(origin, Origin::Work);
        assert_eq!(thread, Thread::Work);

        let (origin, thread) = classify(ProactiveTrigger::WindowTrigger);
        assert_eq!(origin, Origin::Ambient);
        assert_eq!(thread, Thread::Window);
        assert!(!origin.immediate(), "ambient facts must not claim the extra slot");

        let (origin, thread) = classify(ProactiveTrigger::IdleGreeting);
        assert_eq!(origin, Origin::Timer);
        assert_eq!(thread, Thread::Own);
    }

    #[test]
    fn event_ids_stay_traceable_and_distinct_per_second() {
        let event = TriggerEvent::new(ProactiveTrigger::MusicChanged, 1234.5);
        assert_eq!(event.id, "trg-music_changed-music-1234");
        let later = TriggerEvent::new(ProactiveTrigger::MusicChanged, 1235.0);
        assert_ne!(event.id, later.id);
    }

    #[test]
    fn repeated_silences_collapse_instead_of_flooding_the_log() {
        let mut log = DispositionLog::new(8);
        let event = TriggerEvent::new(ProactiveTrigger::IdleGreeting, 0.0);
        for i in 0..4 {
            log.record(&event, Outcome::Silenced, Reason::LayLow, "防打扰", at(i as f64));
        }
        assert_eq!(log.recent(1).len(), 1);
        assert_eq!(log.recent(1)[0].count, 4);
        assert_eq!(log.recent(1)[0].reason, "lay_low");
    }

    #[test]
    fn different_outcome_or_long_gaps_are_not_collapsed() {
        let mut log = DispositionLog::new(8);
        let event = TriggerEvent::new(ProactiveTrigger::HourlyGreeting, 0.0);
        log.record(&event, Outcome::Admitted, Reason::Ok, "", 0.0);
        log.record(&event, Outcome::Delivered, Reason::Ok, "问候", 5.0);
        assert_eq!(log.recent(2).len(), 2);

        log.record(&event, Outcome::Admitted, Reason::Ok, "", 500.0);
        assert_eq!(log.recent(1)[0].count, 1, "outside the collapse window it is a new entry");
    }

    #[test]
    fn capacity_keeps_the_newest_entries() {
        let mut log = DispositionLog::new(3);
        // 间隔大于折叠窗口，让五次记录真的是五条，才能验证容量淘汰。
        for i in 0..5 {
            let now = i as f64 * 200.0;
            let event = TriggerEvent::new(ProactiveTrigger::WindowTrigger, now);
            log.record(&event, Outcome::Silenced, Reason::GateRejected, "", now);
        }
        let recent = log.recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].at, 800.0);
        assert_eq!(recent[2].at, 400.0);
    }

    #[test]
    fn summary_counts_within_the_window_and_sorts_by_count() {
        let mut log = DispositionLog::new(16);
        let event = TriggerEvent::new(ProactiveTrigger::IdleGreeting, 0.0);
        log.record(&event, Outcome::Silenced, Reason::UserAway, "", 10.0);
        log.record(&event, Outcome::Silenced, Reason::UserAway, "", 11.0);
        let other = TriggerEvent::new(ProactiveTrigger::WindowTrigger, 0.0);
        log.record(&other, Outcome::Delivered, Reason::Ok, "", 12.0);
        log.record(&event, Outcome::Silenced, Reason::UserAway, "", 5000.0);

        // 紧窗口里只剩 5000s 那条；10-11s 的折叠对与 12s 的投递都在窗口外。
        let rows = log.summary(5000.0, 3600.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].count, 1);
        assert_eq!(rows[0].outcome, "silenced");

        let rows = log.summary(5000.0, 10_000.0);
        assert_eq!(rows[0].count, 3);
        assert_eq!(rows[0].reason, "user_away");
    }

    #[test]
    fn details_are_summarised_not_copied() {
        let mut log = DispositionLog::new(4);
        let event = TriggerEvent::new(ProactiveTrigger::TopicExtension, 0.0);
        let long = "很长的材料".repeat(40);
        log.record(&event, Outcome::Deferred, Reason::BudgetFull, &long, 0.0);
        let detail = &log.recent(1)[0].detail;
        assert!(detail.chars().count() <= MAX_DETAIL_CHARS);
        assert_eq!(log.recent(1)[0].id, event.id);
    }

    #[test]
    fn delivered_count_sums_collapsed_entries() {
        let mut log = DispositionLog::new(8);
        let event = TriggerEvent::new(ProactiveTrigger::WelcomeBack, 0.0);
        log.record(&event, Outcome::Delivered, Reason::Ok, "", 0.0);
        log.record(&event, Outcome::Delivered, Reason::Ok, "", 1.0);
        assert_eq!(log.delivered_count(), 2);
    }

    #[test]
    fn dominant_rejection_picks_the_outermost_gate_that_blocked_most_triggers() {
        let rejected = [
            ("hourly_greeting", Reason::Cooldown),
            ("icebreaker", Reason::Cooldown),
            ("topic_extension", Reason::Cooldown),
            ("spontaneous", Reason::TimingLow),
            ("window_trigger", Reason::Specific),
        ];
        let (reason, detail) = dominant_rejection(&rejected).expect("rejections are not empty");
        assert_eq!(reason, Reason::Cooldown);
        assert_eq!(
            detail,
            "cooldown: hourly_greeting, icebreaker, topic_extension · timing_low: spontaneous · specific: window_trigger"
        );
    }

    #[test]
    fn dominant_rejection_ties_keep_gate_order() {
        let rejected = [
            ("idle_greeting", Reason::DesireLow),
            ("spontaneous", Reason::TimingLow),
        ];
        let (reason, _) = dominant_rejection(&rejected).expect("rejections are not empty");
        assert_eq!(reason, Reason::DesireLow, "ties resolve to the first gate hit");
        assert!(dominant_rejection(&[]).is_none());
    }

    #[test]
    fn a_tick_level_silence_is_one_entry_not_one_per_trigger() {
        let mut log = DispositionLog::new(8);
        for tick in 0..10 {
            let now = tick as f64 * 10.0;
            log.record_raw(
                format!("tick-own-{now}"),
                "tick",
                Origin::Timer,
                Thread::Own,
                Outcome::Silenced,
                Reason::UserAway,
                "用户不在，面向用户的机会全部顺延",
                now,
            );
        }
        let recent = log.recent(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].count, 10);
        assert_eq!(recent[0].trigger, "tick");
        assert_eq!(recent[0].reason, "user_away");
    }
}
