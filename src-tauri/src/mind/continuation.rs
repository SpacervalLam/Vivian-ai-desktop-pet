//! Durable, private next steps. A deadline wakes cognition; it grants no tools or speech.
use serde::{Deserialize, Serialize};

const MAX_SELF_STEPS: u8 = 2;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationRequest {
    pub concern_key: String,
    pub after_seconds: u64,
    pub purpose: String,
}

pub fn parse_request<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ContinuationRequest>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcernAnchor {
    pub key: String,
    pub revision: u64,
    /// Host-derived source identity, never a model-selected reset of the budget.
    pub source: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PendingThought {
    anchor: ConcernAnchor,
    due_at: i64,
    purpose: String,
    in_flight: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StepBudget {
    key: String,
    source: u64,
    used: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationState {
    pending: Option<PendingThought>,
    budgets: Vec<StepBudget>,
}

impl ContinuationState {
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn used(&self, anchor: &ConcernAnchor) -> u8 {
        self.budgets
            .iter()
            .find(|budget| budget.key == anchor.key && budget.source == anchor.source)
            .map_or(0, |budget| budget.used)
    }

    /// Every successful thought ends its previous step. Staying silent or returning
    /// null is a valid stop; only a grounded, explicit next step schedules another.
    pub fn finish(
        &mut self,
        request: Option<&ContinuationRequest>,
        anchors: &[ConcernAnchor],
        now: i64,
    ) -> bool {
        let previous = self.clone();
        self.pending = None;
        if let Some(request) = request {
            if (60..=3600).contains(&request.after_seconds)
                && !request.purpose.trim().is_empty()
                && request.purpose.chars().count() <= 240
            {
                if let Some(anchor) = anchors
                    .iter()
                    .find(|anchor| anchor.key == request.concern_key)
                {
                    if self.used(anchor) < MAX_SELF_STEPS {
                        self.pending = Some(PendingThought {
                            anchor: anchor.clone(),
                            due_at: now.saturating_add(request.after_seconds as i64),
                            purpose: request.purpose.trim().into(),
                            in_flight: false,
                        });
                    }
                }
            }
        }
        *self != previous
    }

    pub fn claim(&mut self, anchors: &[ConcernAnchor], now: i64) -> bool {
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        if !anchors.contains(&pending.anchor) {
            self.pending = None;
            return false;
        }
        if pending.in_flight || pending.due_at > now || self.used(&pending.anchor) >= MAX_SELF_STEPS
        {
            return false;
        }
        let anchor = pending.anchor.clone();
        if let Some(budget) = self
            .budgets
            .iter_mut()
            .find(|budget| budget.key == anchor.key && budget.source == anchor.source)
        {
            budget.used += 1;
        } else {
            self.budgets.push(StepBudget {
                key: anchor.key,
                source: anchor.source,
                used: 1,
            });
        }
        self.pending.as_mut().unwrap().in_flight = true;
        true
    }

    pub fn cancel(&mut self) -> bool {
        self.pending.take().is_some()
    }

    pub fn retain_sources(&mut self, live: &[ConcernAnchor]) {
        self.budgets.retain(|budget| {
            live.iter()
                .any(|anchor| anchor.key == budget.key)
        });
    }

    /// Only private inference is retried. The original source budget survives a
    /// provider error, cancellation and restart; it cannot turn into an endless loop.
    pub fn retry_interrupted(&mut self, now: i64) -> bool {
        let Some(pending) = self.pending.as_ref().filter(|pending| pending.in_flight) else {
            return false;
        };
        if self.used(&pending.anchor) >= MAX_SELF_STEPS {
            self.pending = None;
        } else {
            let pending = self.pending.as_mut().unwrap();
            pending.in_flight = false;
            pending.due_at = now.saturating_add(60);
        }
        true
    }

    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({"next_thought":self.pending.as_ref().map(|pending| serde_json::json!({
            "concern_key":pending.anchor.key,"revision":pending.anchor.revision,"due_at":pending.due_at,
            "purpose":pending.purpose,"in_flight":pending.in_flight,
            "remaining_self_steps":MAX_SELF_STEPS.saturating_sub(self.used(&pending.anchor)),
        })),"permission":"private reflection only; no new event, user request, tools or speaking authorization"})
    }
}

pub fn request_schema() -> serde_json::Value {
    serde_json::json!({"anyOf":[{"type":"null"},{"type":"object","additionalProperties":false,
        "properties":{"concern_key":{"type":"string"},"after_seconds":{"type":"integer","minimum":60,"maximum":3600},
            "purpose":{"type":"string"}},"required":["concern_key","after_seconds","purpose"]}]})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn anchor(revision: u64, source: u64) -> ConcernAnchor {
        ConcernAnchor {
            key: "mechanism".into(),
            revision,
            source,
        }
    }
    fn request() -> ContinuationRequest {
        ContinuationRequest {
            concern_key: "mechanism".into(),
            after_seconds: 60,
            purpose: "澄清尚未想清楚的机制".into(),
        }
    }
    #[test]
    fn a_private_step_waits_for_its_deadline_and_has_one_in_flight_owner() {
        let mut state = ContinuationState::default();
        let anchors = [anchor(1, 9)];
        assert!(state.finish(Some(&request()), &anchors, 100));
        assert!(!state.claim(&anchors, 159));
        assert!(state.claim(&anchors, 160));
        assert!(!state.claim(&anchors, 161));
        assert_eq!(state.status()["next_thought"]["remaining_self_steps"], 1);
    }
    #[test]
    fn rewording_and_revisions_cannot_reset_the_source_budget() {
        let mut state = ContinuationState::default();
        for revision in 1..=2 {
            state.finish(
                Some(&request()),
                &[anchor(revision, 9)],
                100 * revision as i64,
            );
            assert!(state.claim(&[anchor(revision, 9)], 100 * revision as i64 + 60));
        }
        state.finish(Some(&request()), &[anchor(3, 9)], 400);
        assert!(state.status()["next_thought"].is_null());
        state.finish(Some(&request()), &[anchor(4, 10)], 500);
        assert!(state.claim(&[anchor(4, 10)], 560));
    }
    #[test]
    fn restart_and_failure_keep_the_budget_and_do_not_replay_external_work() {
        let mut state = ContinuationState::default();
        let anchors = [anchor(1, 9)];
        state.finish(Some(&request()), &anchors, 100);
        assert!(state.claim(&anchors, 160));
        let mut restored: ContinuationState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        restored.retry_interrupted(200);
        assert!(!restored.claim(&anchors, 259));
        assert!(restored.claim(&anchors, 260));
        restored.retry_interrupted(270);
        assert!(restored.status()["next_thought"].is_null());
    }
    #[test]
    fn stopped_changed_and_fabricated_concerns_cannot_schedule_or_resume() {
        let mut state = ContinuationState::default();
        state.finish(Some(&request()), &[], 100);
        assert!(state.status()["next_thought"].is_null());
        state.finish(Some(&request()), &[anchor(1, 9)], 100);
        assert!(!state.claim(&[anchor(2, 9)], 160));
        state.finish(Some(&request()), &[anchor(2, 9)], 200);
        state.finish(None, &[anchor(2, 9)], 201);
        assert!(state.status()["next_thought"].is_null());
    }
}
