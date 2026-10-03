//! Opportunity estimates are heuristic model scores, not calibrated probabilities.
//! Time is a bounded penalty; only delivered speech advances the speech timestamp.
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc};

#[derive(Debug, Clone, Copy)]
pub struct Opportunity {
    pub relevance: f64,
    pub novelty: f64,
    pub timing: f64,
}

impl Opportunity {
    pub fn parse(text: &str) -> Option<Self> {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        let value: serde_json::Value = serde_json::from_str(text.get(start..=end)?).ok()?;
        Self::new(
            value.get("relevance")?.as_f64()?,
            value.get("novelty")?.as_f64()?,
            value.get("timing")?.as_f64()?,
        )
    }

    pub fn new(relevance: f64, novelty: f64, timing: f64) -> Option<Self> {
        [relevance, novelty, timing]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            .then_some(Self {
                relevance,
                novelty,
                timing,
            })
    }

    pub fn evaluate(self, now: f64, last_spoken: Option<f64>) -> Decision {
        let confidence = 0.45 * self.relevance + 0.30 * self.novelty + 0.25 * self.timing;
        let time_penalty = last_spoken
            .map(|last| {
                let elapsed = (now - last).max(0.0);
                0.20 * (-elapsed / 60.0).exp()
            })
            .unwrap_or(0.0);
        let score = confidence - time_penalty;
        Decision {
            confidence,
            time_penalty,
            score,
            accepted: self.relevance >= 0.55
                && self.novelty >= 0.55
                && self.timing >= 0.55
                && score >= 0.65,
        }
    }
}

pub struct Decision {
    pub confidence: f64,
    pub time_penalty: f64,
    pub score: f64,
    pub accepted: bool,
}

#[derive(Default)]
pub struct OpportunityState {
    in_flight: bool,
    seen: VecDeque<(String, f64)>,
    pub last_spoken: Option<f64>,
    pub last_text: String,
}

pub struct OpportunityPermit {
    state: Arc<Mutex<OpportunityState>>,
}

impl OpportunityPermit {
    pub fn reserve(
        state: &Arc<Mutex<OpportunityState>>,
        exchange: String,
        now: f64,
    ) -> Option<Self> {
        let mut data = state.lock();
        data.seen.retain(|(_, time)| now - time < 30.0);
        if data.in_flight || data.seen.iter().any(|(old, _)| old == &exchange) {
            return None;
        }
        data.in_flight = true;
        data.seen.push_back((exchange, now));
        while data.seen.len() > 16 {
            data.seen.pop_front();
        }
        Some(Self {
            state: Arc::clone(state),
        })
    }
}

impl Drop for OpportunityPermit {
    fn drop(&mut self) {
        self.state.lock().in_flight = false;
    }
}

pub struct PendingInterjection {
    pub directive: String,
    permit: OpportunityPermit,
}

impl PendingInterjection {
    pub fn new(directive: String, permit: OpportunityPermit) -> Self {
        Self { directive, permit }
    }
    pub fn mark_spoken(&mut self, now: f64, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let mut state = self.permit.state.lock();
        state.last_spoken = Some(now);
        state.last_text = text.chars().take(500).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn high_opportunity_breaks_time_penalty_but_medium_waits() {
        let high = Opportunity::new(0.95, 0.95, 0.95).unwrap();
        assert!(high.evaluate(100.0, Some(100.0)).accepted);
        let medium = Opportunity::new(0.75, 0.75, 0.75).unwrap();
        assert!(!medium.evaluate(100.0, Some(100.0)).accepted);
        assert!(medium.evaluate(220.0, Some(100.0)).accepted);
        assert!(medium.evaluate(100.0, None).accepted);
    }
    #[test]
    fn time_cannot_make_irrelevant_or_repetitive_comments_speak() {
        for scores in [
            (0.3, 1.0, 1.0),
            (1.0, 0.3, 1.0),
            (1.0, 1.0, 0.3),
            (0.5, 0.5, 0.5),
        ] {
            assert!(
                !Opportunity::new(scores.0, scores.1, scores.2)
                    .unwrap()
                    .evaluate(10000.0, Some(0.0))
                    .accepted
            );
        }
    }
    #[test]
    fn skipped_or_failed_attempts_do_not_consume_speech_time() {
        let state = Arc::new(Mutex::new(OpportunityState::default()));
        let permit = OpportunityPermit::reserve(&state, "first".into(), 100.0).unwrap();
        assert!(OpportunityPermit::reserve(&state, "parallel".into(), 101.0).is_none());
        drop(permit); // rejection, failure or cancellation
        assert!(state.lock().last_spoken.is_none());
        assert!(OpportunityPermit::reserve(&state, "first".into(), 101.0).is_none());
        let permit =
            OpportunityPermit::reserve(&state, "new meaningful exchange".into(), 101.0).unwrap();
        let mut pending = PendingInterjection::new("directive".into(), permit);
        pending.mark_spoken(102.0, "");
        assert!(state.lock().last_spoken.is_none());
        pending.mark_spoken(103.0, "actual speech");
        assert_eq!(state.lock().last_spoken, Some(103.0));
        drop(pending);
        assert!(OpportunityPermit::reserve(&state, "another new exchange".into(), 104.0).is_some());
    }
    #[test]
    fn malformed_scores_are_silent_and_clock_rollback_is_bounded() {
        assert!(Opportunity::parse("{\"relevance\":1,\"novelty\":0.9,\"timing\":0.9}").is_some());
        for text in [
            "",
            "{\"should_interject\":true}",
            "{\"relevance\":2,\"novelty\":1,\"timing\":1}",
        ] {
            assert!(Opportunity::parse(text).is_none());
        }
        assert!(Opportunity::new(f64::NAN, 1.0, 1.0).is_none());
        assert_eq!(
            Opportunity::new(1.0, 1.0, 1.0)
                .unwrap()
                .evaluate(90.0, Some(100.0))
                .time_penalty,
            0.20
        );
    }
}
