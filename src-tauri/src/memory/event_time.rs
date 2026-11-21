//! Host-owned event time and subjective recall heuristics, separate from speaking urgency.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EventAppraisal {
    pub personal_relevance: Option<f64>,
    pub novelty: Option<f64>,
    pub emotional_intensity: Option<f64>,
    pub agency: Option<f64>,
    pub unresolved: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EventTimeline {
    /// Host receipt time, never substituted for occurrence time during replay.
    pub recorded_at: Option<f64>,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    pub episode_id: Option<String>,
    pub caused_by: Option<String>,
    pub memory_id: Option<String>,
    pub phase: Option<String>,
    pub sequence: u64,
    pub appraisal: EventAppraisal,
}

impl EventTimeline {
    pub fn valid(&self, occurred_at: f64, now: f64) -> bool {
        if !occurred_at.is_finite()
            || occurred_at < 0.0
            || !now.is_finite()
            || occurred_at > now + 1.0
        {
            return false;
        }
        if self
            .recorded_at
            .is_some_and(|v| !v.is_finite() || v < occurred_at || v > now + 1.0)
            || self
                .started_at
                .is_some_and(|v| !v.is_finite() || v < 0.0 || v > occurred_at)
            || self.ended_at.is_some_and(|v| {
                !v.is_finite() || v < self.started_at.unwrap_or(0.0) || v > occurred_at
            })
        {
            return false;
        }
        [
            &self.episode_id,
            &self.caused_by,
            &self.memory_id,
            &self.phase,
        ]
        .into_iter()
        .flatten()
        .all(|v| v.chars().count() <= 240)
    }

    pub fn context(&self, occurred_at: f64, now: f64) -> String {
        let mut parts = vec![format!("发生于距今 {:.0} 秒", (now - occurred_at).max(0.0))];
        if let Some(start) = self.started_at {
            parts.push(format!(
                "活动跨度 {:.0} 秒",
                (self.ended_at.unwrap_or(occurred_at) - start).max(0.0)
            ));
        }
        if let Some(at) = self.recorded_at.filter(|at| *at > occurred_at + 5.0) {
            parts.push(format!("延后 {:.0} 秒入账", at - occurred_at));
        }
        if let Some(phase) = &self.phase {
            parts.push(format!("阶段 {phase}"));
        }
        if let Some(id) = &self.episode_id {
            parts.push(format!("同一活动 {id}"));
        }
        if let Some(id) = &self.caused_by {
            parts.push(format!("来源 {id}"));
        }
        parts.join("；")
    }
}

fn unit(value: Option<f64>, fallback: f64) -> f64 {
    value
        .filter(|v| v.is_finite())
        .unwrap_or(fallback)
        .clamp(0.0, 1.0)
}

/// Continuous decay: immediate details fade quickly; personally meaningful
/// events have a longer tail. Repetition is familiarity, not more importance.
pub fn recall_weight(
    kind: &str,
    occurred_at: f64,
    now: f64,
    timeline: &EventTimeline,
    personally_involved: bool,
    repetitions: usize,
) -> f64 {
    if !occurred_at.is_finite() || !now.is_finite() || occurred_at > now + 1.0 {
        return 0.0;
    }
    let age = (now - occurred_at).max(0.0);
    let transient = matches!(
        kind,
        "user_pet_action"
            | "user_app_switched"
            | "user_media_changed"
            | "presence_log"
            | "learning_started"
            | "learning_read"
    );
    let base = match kind {
        "dialogue" | "user_message" | "agent_message" => 0.8,
        "knowledge_acquired" | "learning_completed" => 0.75,
        "action" | "tool_result" => 0.65,
        "compacted_summary" => 0.6,
        "user_ignored" | "ignored_message" | "mood_shift" | "mood_event" => 0.25,
        "presence_log" | "quiet_mode" | "long_idle" => 0.12,
        _ => 0.4,
    };
    let appraisal = &timeline.appraisal;
    let relevance = unit(
        appraisal.personal_relevance,
        if personally_involved { 0.6 } else { 0.25 },
    );
    let emotion = unit(appraisal.emotional_intensity, 0.0);
    let novelty = unit(appraisal.novelty, 0.4);
    let agency = unit(appraisal.agency, 0.0);
    let meaningful = relevance
        .max(emotion)
        .max(if appraisal.unresolved { 0.9 } else { 0.0 });
    let fast = if transient { 900.0 } else { 4.0 * 3600.0 };
    let slow = if transient { 7200.0 } else { 21.0 * 86400.0 };
    let tail = if transient {
        0.08
    } else {
        0.12 + 0.35 * meaningful
    };
    let decay = (1.0 - tail) * (-age / fast).exp() + tail * (-age / slow).exp();
    let strength = base * (0.65 + 0.35 * relevance) * (0.8 + 0.2 * novelty)
        + 0.2 * emotion
        + 0.1 * agency
        + if appraisal.unresolved { 0.18 } else { 0.0 };
    strength * decay / (1.0 + repetitions.saturating_sub(1) as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn historical_replay_keeps_occurrence_and_receipt_distinct() {
        let timeline = EventTimeline {
            recorded_at: Some(10000.0),
            started_at: Some(10.0),
            ended_at: Some(100.0),
            ..Default::default()
        };
        assert!(timeline.valid(100.0, 10000.0));
        assert!(timeline
            .context(100.0, 10000.0)
            .contains("延后 9900 秒入账"));
        assert!(
            recall_weight("dialogue", 100.0, 10000.0, &timeline, true, 1)
                < recall_weight("dialogue", 9999.0, 10000.0, &Default::default(), true, 1)
        );
    }
    #[test]
    fn malformed_intervals_and_future_events_are_not_observations() {
        assert!(!EventTimeline {
            started_at: Some(30.0),
            ended_at: Some(20.0),
            ..Default::default()
        }
        .valid(40.0, 40.0));
        assert!(!EventTimeline::default().valid(f64::NAN, 100.0));
        assert!(!EventTimeline::default().valid(200.0, 100.0));
        assert_eq!(
            recall_weight("dialogue", 200.0, 100.0, &Default::default(), true, 1),
            0.0
        );
    }
    #[test]
    fn decay_has_no_midnight_or_day_boundary_cliff() {
        let before = recall_weight("dialogue", 0.0, 86399.0, &Default::default(), true, 1);
        let after = recall_weight("dialogue", 0.0, 86401.0, &Default::default(), true, 1);
        assert!(before > after && before - after < 0.0001);
    }
    #[test]
    fn personally_meaningful_learning_outlasts_routine_state_changes() {
        let mut t = EventTimeline::default();
        t.appraisal.personal_relevance = Some(0.95);
        assert!(
            recall_weight("knowledge_acquired", 0.0, 86400.0, &t, true, 1)
                > recall_weight(
                    "presence_log",
                    86390.0,
                    86400.0,
                    &Default::default(),
                    true,
                    1
                )
        );
    }
    #[test]
    fn repeated_signals_do_not_rehearse_themselves_into_importance() {
        let t = EventTimeline::default();
        assert!(
            recall_weight("dialogue", 10.0, 20.0, &t, true, 20)
                < recall_weight("dialogue", 10.0, 20.0, &t, true, 1)
        );
        assert!(
            recall_weight("user_pet_action", 10.0, 3600.0, &t, true, 1)
                < recall_weight("dialogue", 10.0, 3600.0, &t, true, 1)
        );
    }
    #[test]
    fn uncertainty_does_not_create_emotion_and_unresolved_is_not_urgency() {
        let plain = EventTimeline::default();
        let invalid = EventTimeline {
            appraisal: EventAppraisal {
                emotional_intensity: Some(f64::NAN),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            recall_weight("dialogue", 10.0, 30.0, &plain, true, 1),
            recall_weight("dialogue", 10.0, 30.0, &invalid, true, 1)
        );
        let unresolved = EventTimeline {
            appraisal: EventAppraisal {
                unresolved: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            recall_weight("dialogue", 10.0, 86400.0, &unresolved, true, 1)
                > recall_weight("dialogue", 10.0, 86400.0, &plain, true, 1)
        );
    }
}
