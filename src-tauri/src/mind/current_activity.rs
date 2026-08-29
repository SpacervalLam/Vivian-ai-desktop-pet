//! Current Activity —— 跨分钟级"正在做什么"状态机。
//!
//! 与 Working Memory（30 秒级瞬时想法）和 WorldSnapshot（外部世界快照）不同，
//! CurrentActivity 记录"我现在正在做什么 + 持续多久 + 最近相关事件"——
//! 它不是记忆（不进 MemoryManager），不是瞬时想法（不进 WorkingMemory），
//! 而是跨分钟级的活动上下文。
//!
//! ## 与现有 CurrentActivity 枚举的关系
//!
//! `self_state::CurrentActivity` 是瞬时视图（每次 snapshot 从多源推导），
//! 本模块是带时间维度的状态机：记录活动开始时间、最近相关事件、活动上下文摘要。
//! 两者协作：本模块是"活动状态机"，self_state 的枚举是"活动标签"。
//!
//! ## 设计原则
//!
//! - **纯运行时**：不持久化，每次启动从空开始
//! - **单一活动**：同一时刻只有一个主活动（人类也无法真正多线程）
//! - **带时间维度**：记录 started_at，可计算"已经持续多久"
//! - **上下文摘要**：≤120 字的活动上下文（如"在打 Minecraft，刚死了一次"）
//! - **自动过期**：超过最大持续时间（默认 30 分钟）的活动自动回到 Idle
//! - **事件追加**：活动期间的相关事件追加到 recent_events（最多 5 条）
//!
//! ## 使用场景
//!
//! - prompt 注入：让 LLM 知道"我现在在做什么"，避免回复与当前活动脱节
//! - Cognitive Tick：Think 阶段决策是否切换活动
//! - 跨角色感知：通过 Public State 暴露给其他角色（"Vivian 正在专注工作"）

use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 活动类型（与 self_state::CurrentActivity 对齐，但带时间维度）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// 空闲
    Idle,
    /// 正在与用户对话
    Talking,
    /// 凝神/专注（深度思考、工作、学习）
    Focusing,
    /// 桌面观察（用户在前台做事，角色在观察）
    Observing,
    /// 内心独白（没说话但在脑内思考）
    Thinking,
    /// 后台任务（知识采集/记忆沉淀）
    BackgroundTask,
    /// 影随/守护/陪伴模式
    Companion,
}

impl Default for ActivityKind {
    fn default() -> Self {
        Self::Idle
    }
}

impl ActivityKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Talking => "talking",
            Self::Focusing => "focusing",
            Self::Observing => "observing",
            Self::Thinking => "thinking",
            Self::BackgroundTask => "background_task",
            Self::Companion => "companion",
        }
    }

    /// 是否属于"沉浸"类活动（不应被打断）
    pub fn is_immersive(&self) -> bool {
        matches!(self, Self::Focusing | Self::BackgroundTask)
    }

    /// 是否属于"社交"类活动（用户在场且交互中）
    pub fn is_social(&self) -> bool {
        matches!(self, Self::Talking | Self::Companion)
    }
}

/// 活动期间的相关事件（简短描述）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    /// 事件摘要（≤50 字）
    pub summary: String,
    /// 事件时间（Unix 秒）
    pub at: i64,
}

/// 当前活动状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityState {
    /// 活动类型
    pub kind: ActivityKind,
    /// 活动开始时间（Unix 秒）
    pub started_at: i64,
    /// 活动上下文摘要（≤120 字，如"在打 Minecraft，刚死了一次"）
    pub context: String,
    /// 活动期间的相关事件（最多 5 条，FIFO）
    pub recent_events: Vec<ActivityEvent>,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            kind: ActivityKind::Idle,
            started_at: 0,
            context: String::new(),
            recent_events: Vec::new(),
        }
    }
}

impl ActivityState {
    /// 持续时间（秒）
    pub fn duration_secs(&self, now: i64) -> f64 {
        if self.started_at == 0 {
            0.0
        } else {
            (now - self.started_at).max(0) as f64
        }
    }

    /// 是否已超时（超过最大持续时间）
    pub fn is_expired(&self, now: i64, max_duration_secs: f64) -> bool {
        self.duration_secs(now) > max_duration_secs
    }
}

/// Current Activity 状态机
///
/// 纯运行时，不持久化。Cognitive Tick 的 SelfUpdate 阶段可调 update_from_snapshot
/// 根据当前世界/自我状态自动切换活动；Think 阶段可调 push_event 追加活动事件。
#[derive(Debug, Clone)]
pub struct CurrentActivityTracker {
    state: Arc<RwLock<ActivityState>>,
    /// 最大活动持续时间（秒），超过自动回 Idle
    max_duration_secs: f64,
}

const MAX_RECENT_EVENTS: usize = 5;
const DEFAULT_MAX_DURATION: f64 = 1800.0; // 30 分钟

impl Default for CurrentActivityTracker {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(ActivityState::default())),
            max_duration_secs: DEFAULT_MAX_DURATION,
        }
    }
}

impl CurrentActivityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前活动快照
    pub fn snapshot(&self) -> ActivityState {
        self.state.read().clone()
    }

    /// 当前活动类型
    pub fn current_kind(&self) -> ActivityKind {
        self.state.read().kind
    }

    /// 切换到新活动
    ///
    /// 若新活动与当前相同，仅更新 context（不重置 started_at）。
    /// 若不同，重置 started_at 为 now，清空 recent_events。
    pub fn transition_to(&self, kind: ActivityKind, context: impl Into<String>, now: i64) {
        let ctx: String = context.into();
        let mut state = self.state.write();
        if state.kind == kind {
            // 同类活动：仅更新 context
            let truncated: String = ctx.chars().take(120).collect();
            state.context = truncated;
        } else {
            // 切换活动：重置时间和事件
            state.kind = kind;
            state.started_at = now;
            let truncated: String = ctx.chars().take(120).collect();
            state.context = truncated;
            state.recent_events.clear();
        }
    }

    /// 追加活动事件
    ///
    /// 超过容量时移除最旧的事件（FIFO）。
    pub fn push_event(&self, summary: impl Into<String>, now: i64) {
        let s: String = summary.into();
        let truncated: String = s.chars().take(50).collect();
        if truncated.trim().is_empty() {
            return;
        }
        let mut state = self.state.write();
        if state.recent_events.len() >= MAX_RECENT_EVENTS {
            state.recent_events.remove(0);
        }
        state.recent_events.push(ActivityEvent {
            summary: truncated,
            at: now,
        });
    }

    /// 自动过期检查：超过最大持续时间 → 回 Idle
    ///
    /// 返回 true 表示触发了过期切换。
    pub fn check_expiry(&self, now: i64) -> bool {
        let state = self.state.read();
        if state.kind == ActivityKind::Idle {
            return false;
        }
        if !state.is_expired(now, self.max_duration_secs) {
            return false;
        }
        let old_kind = state.kind;
        drop(state);

        // 过期回 Idle
        self.transition_to(ActivityKind::Idle, format!("{:?}活动超时", old_kind), now);
        tracing::debug!(
            "[current_activity] activity {:?} expired after {:.0}min, back to Idle",
            old_kind,
            self.max_duration_secs / 60.0
        );
        true
    }

    /// 根据外部快照自动更新活动状态
    ///
    /// 由 Cognitive Tick 的 SelfUpdate 阶段调用。规则：
    /// - presence=Busy/Rest → BackgroundTask
    /// - 用户在对话（last_spoken < 60s）→ Talking
    /// - 用户长时间不在（idle > 5min）+ 非沉浸 → Idle
    /// - behavior_mode=follow/guardian/companion → Companion
    /// - 其他保持现状
    ///
    /// 注意：此方法只做"显而易见"的切换，复杂的活动判定留给 Think 阶段。
    pub fn update_from_snapshot(
        &self,
        presence_busy: bool,
        presence_rest: bool,
        behavior_mode: &str,
        last_spoken_secs_ago: Option<f64>,
        user_idle_secs: f64,
        now: i64,
    ) {
        // 优先级 1：后台任务（presence 占主导）
        if presence_busy || presence_rest {
            let ctx = if presence_busy {
                "后台知识采集".to_string()
            } else {
                "后台记忆沉淀".to_string()
            };
            let current = self.current_kind();
            if current != ActivityKind::BackgroundTask {
                self.transition_to(ActivityKind::BackgroundTask, ctx, now);
            }
            return;
        }

        // 优先级 2：behavior_mode
        match behavior_mode {
            "follow" | "guardian" | "companion" => {
                let current = self.current_kind();
                if current != ActivityKind::Companion {
                    let ctx = match behavior_mode {
                        "follow" => "影随用户",
                        "guardian" => "守护用户",
                        "companion" => "陪伴用户",
                        _ => "陪伴",
                    };
                    self.transition_to(ActivityKind::Companion, ctx, now);
                }
                return;
            }
            _ => {}
        }

        // 优先级 3：用户在对话（60s 内有发言）
        if let Some(secs) = last_spoken_secs_ago {
            if secs < 60.0 {
                let current = self.current_kind();
                if current != ActivityKind::Talking {
                    self.transition_to(ActivityKind::Talking, "与用户对话中", now);
                }
                return;
            }
        }

        // 优先级 4：用户长时间不在 → Idle（除非正在沉浸）
        if user_idle_secs > 300.0 {
            let current = self.current_kind();
            if !current.is_immersive() && current != ActivityKind::Idle {
                self.transition_to(ActivityKind::Idle, "用户不在，空闲", now);
            }
            return;
        }

        // 其他情况保持现状
    }

    /// 序列化为 prompt 段落
    ///
    /// 让 LLM 感知"我现在在做什么 + 持续多久 + 最近相关事件"。
    /// 空活动或 Idle 不输出，避免污染 prompt。
    pub fn serialize_for_prompt(&self, now: i64, lang: &str) -> Option<String> {
        let state = self.state.read();
        if state.kind == ActivityKind::Idle {
            return None;
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let (activity_label, context_label, events_label, min_ago_fmt) = match lang_norm {
            "en" => ("Current activity", "Context", "Recent events", "{:.0}min ago"),
            "ja" => ("現在の活動", "コンテキスト", "最近のイベント", "{:.0}分前"),
            _ => ("当前活动", "上下文", "最近事件", "{:.0}分钟前"),
        };

        let mut lines = vec![format!(
            "- {}: {}",
            activity_label,
            state.kind.as_str()
        )];

        if !state.context.is_empty() {
            lines.push(format!("- {}: {}", context_label, state.context));
        }

        if !state.recent_events.is_empty() {
            let events: Vec<String> = state
                .recent_events
                .iter()
                .map(|e| {
                    let mins_ago = ((now - e.at).max(0) as f64) / 60.0;
                    format!("[{}] {}", min_ago_fmt.replace("{:.0}", &format!("{:.0}", mins_ago)), e.summary)
                })
                .collect();
            let sep = match lang_norm {
                "en" => "; ",
                _ => "；",
            };
            lines.push(format!("- {}: {}", events_label, events.join(sep)));
        }

        let header = crate::pipeline::prompt_modules::section_heading("current_activity", lang);
        Some(format!("{}\n{}", header, lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_kind_immersive_check() {
        assert!(ActivityKind::Focusing.is_immersive());
        assert!(ActivityKind::BackgroundTask.is_immersive());
        assert!(!ActivityKind::Idle.is_immersive());
        assert!(!ActivityKind::Talking.is_immersive());
    }

    #[test]
    fn activity_kind_social_check() {
        assert!(ActivityKind::Talking.is_social());
        assert!(ActivityKind::Companion.is_social());
        assert!(!ActivityKind::Idle.is_social());
        assert!(!ActivityKind::Focusing.is_social());
    }

    #[test]
    fn transition_to_same_kind_keeps_started_at() {
        let tracker = CurrentActivityTracker::new();
        tracker.transition_to(ActivityKind::Focusing, "工作", 1000);
        let snap1 = tracker.snapshot();
        tracker.transition_to(ActivityKind::Focusing, "继续工作", 1100);
        let snap2 = tracker.snapshot();
        // 同类活动不重置 started_at
        assert_eq!(snap1.started_at, snap2.started_at);
        // 但更新 context
        assert_eq!(snap2.context, "继续工作");
    }

    #[test]
    fn transition_to_different_kind_resets_started_at() {
        let tracker = CurrentActivityTracker::new();
        tracker.transition_to(ActivityKind::Focusing, "工作", 1000);
        tracker.transition_to(ActivityKind::Talking, "对话", 1100);
        let snap = tracker.snapshot();
        assert_eq!(snap.kind, ActivityKind::Talking);
        assert_eq!(snap.started_at, 1100);
        assert_eq!(snap.context, "对话");
        assert!(snap.recent_events.is_empty());
    }

    #[test]
    fn push_event_respects_capacity() {
        let tracker = CurrentActivityTracker::new();
        tracker.transition_to(ActivityKind::Focusing, "工作", 0);
        for i in 0..10 {
            tracker.push_event(format!("事件 {}", i), i);
        }
        let snap = tracker.snapshot();
        assert_eq!(snap.recent_events.len(), MAX_RECENT_EVENTS);
        // FIFO：保留最后 5 条
        assert_eq!(snap.recent_events[0].summary, "事件 5");
        assert_eq!(snap.recent_events[4].summary, "事件 9");
    }

    #[test]
    fn check_expiry_returns_false_for_idle() {
        let tracker = CurrentActivityTracker::new();
        assert!(!tracker.check_expiry(1000000));
    }

    #[test]
    fn check_expiry_triggers_after_max_duration() {
        let tracker = CurrentActivityTracker::new();
        tracker.transition_to(ActivityKind::Focusing, "工作", 0);
        // 30 分钟 + 1 秒
        let expired = tracker.check_expiry((DEFAULT_MAX_DURATION + 1.0) as i64);
        assert!(expired);
        assert_eq!(tracker.current_kind(), ActivityKind::Idle);
    }

    #[test]
    fn update_from_snapshot_priority_background_task() {
        let tracker = CurrentActivityTracker::new();
        tracker.update_from_snapshot(true, false, "none", None, 0.0, 1000);
        assert_eq!(tracker.current_kind(), ActivityKind::BackgroundTask);
    }

    #[test]
    fn update_from_snapshot_priority_companion_mode() {
        let tracker = CurrentActivityTracker::new();
        tracker.update_from_snapshot(false, false, "follow", None, 0.0, 1000);
        assert_eq!(tracker.current_kind(), ActivityKind::Companion);
    }

    #[test]
    fn update_from_snapshot_priority_talking() {
        let tracker = CurrentActivityTracker::new();
        tracker.update_from_snapshot(false, false, "none", Some(30.0), 0.0, 1000);
        assert_eq!(tracker.current_kind(), ActivityKind::Talking);
    }

    #[test]
    fn update_from_snapshot_user_away_to_idle() {
        let tracker = CurrentActivityTracker::new();
        tracker.transition_to(ActivityKind::Observing, "观察", 0);
        tracker.update_from_snapshot(false, false, "none", Some(120.0), 400.0, 1000);
        assert_eq!(tracker.current_kind(), ActivityKind::Idle);
    }

    #[test]
    fn update_from_snapshot_user_away_keeps_immersive() {
        let tracker = CurrentActivityTracker::new();
        tracker.transition_to(ActivityKind::Focusing, "深度工作", 0);
        tracker.update_from_snapshot(false, false, "none", Some(120.0), 400.0, 1000);
        // 沉浸活动不被"用户不在"打断
        assert_eq!(tracker.current_kind(), ActivityKind::Focusing);
    }

    #[test]
    fn serialize_for_prompt_returns_none_for_idle() {
        let tracker = CurrentActivityTracker::new();
        assert_eq!(tracker.serialize_for_prompt(0, "zh"), None);
    }

    #[test]
    fn serialize_for_prompt_includes_context_and_events() {
        let tracker = CurrentActivityTracker::new();
        tracker.transition_to(ActivityKind::Focusing, "写代码", 1000);
        tracker.push_event("完成了一个函数", 1100);
        let s = tracker.serialize_for_prompt(1200, "en").unwrap();
        assert!(s.contains("focusing"));
        assert!(s.contains("写代码"));
        assert!(s.contains("完成了一个函数"));
        assert!(s.contains("Current Activity"));
    }
}
