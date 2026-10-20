//! Shared scene policy. Model output selects observed opportunities; it cannot create facts.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, Serialize)]
pub struct Participant {
    pub id: String,
    pub available: bool,
    pub can_address_user: bool,
    pub loneliness: f64,
    pub current_activity: String,
    pub last_spoken_secs_ago: Option<f64>,
    pub seconds_since_user_interaction: f64,
    pub fatigue: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SceneEvent {
    pub id: String,
    pub stimulus: Stimulus,
    pub fact: String,
    pub owner: Option<String>,
    pub at: f64,
}

/// Native program inputs, not coordinator prose for the character prompt.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stimulus {
    ActivityChanged,
    WorldChanged,
    CharacterState,
    UserDialogue,
}

#[derive(Clone, Debug, Serialize)]
pub struct UserTurn {
    pub id: String,
    pub character_id: String,
    pub user_text: String,
    pub assistant_text: String,
    pub at: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntentKind {
    NeedsSpace,
    Closing,
    TaskRequest,
    Question,
    SocialInvitation,
    Continuing,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntentAssessment {
    pub turn_id: String,
    pub kind: IntentKind,
    /// Exact user quote, checked against the cited turn; never copied into a pet prompt.
    pub evidence: String,
    pub confidence: f64,
    #[serde(default)]
    pub pause_seconds: u64,
}

#[derive(Debug, Deserialize)]
pub struct PlanningDecision {
    pub user_intent: Option<IntentAssessment>,
    pub opportunity: Option<Selection>,
}

impl PlanningDecision {
    pub fn parse(raw: &str) -> Option<Self> {
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        serde_json::from_str(raw.get(start..=end)?).ok()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Selection {
    pub event_id: String,
    pub speaker_id: String,
    pub audience: String,
    pub confidence: f64,
    #[serde(default)]
    pub delay_seconds: u64,
}

impl Selection {
    pub fn parse(raw: &str) -> Option<Self> {
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        let value: serde_json::Value = serde_json::from_str(raw.get(start..=end)?).ok()?;
        serde_json::from_value(value.get("opportunity")?.clone()).ok()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Opportunity {
    pub event_id: String,
    pub speaker_id: String,
    /// Frozen recipient, never redirected if this character leaves.
    pub listener_id: String,
    pub stimulus: Stimulus,
    pub not_before: f64,
    pub expires_at: f64,
}

#[derive(Default)]
pub struct ScenePolicy {
    events: VecDeque<SceneEvent>,
    seen: HashMap<String, f64>,
    pending: Option<Opportunity>,
    last_plan: Option<f64>,
    last_user_opportunity: Option<f64>,
    last_peer_opportunity: Option<f64>,
    user_turns: VecDeque<UserTurn>,
    turn_sequence: u64,
    intent: Option<IntentAssessment>,
    suppressed_until: f64,
}

impl ScenePolicy {
    pub fn invalidate(&mut self) {
        self.events.clear();
        self.pending = None;
        self.user_turns.clear();
        self.intent = None;
        self.suppressed_until = 0.0;
    }

    /// Public, completed exchanges only. A new turn supersedes a queued older opportunity.
    pub fn record_user_turn(
        &mut self,
        character_id: &str,
        user_text: &str,
        assistant_text: &str,
        now: f64,
    ) {
        if user_text.trim().is_empty() {
            return;
        }
        self.turn_sequence = self.turn_sequence.wrapping_add(1);
        let id = format!("user_turn:{}", self.turn_sequence);
        self.user_turns.push_back(UserTurn {
            id: id.clone(),
            character_id: character_id.to_owned(),
            user_text: user_text.chars().take(768).collect(),
            assistant_text: assistant_text.chars().take(384).collect(),
            at: now,
        });
        while self.user_turns.len() > 4 {
            self.user_turns.pop_front();
        }
        self.pending = None;
        self.events
            .retain(|event| event.stimulus != Stimulus::UserDialogue);
        self.observe(SceneEvent {
            id,
            stimulus: Stimulus::UserDialogue,
            fact: "A public user dialogue turn completed; consult the cited user turn for intent."
                .into(),
            owner: None,
            at: now,
        });
    }

    pub fn recent_user_turns(&self, now: f64) -> Vec<UserTurn> {
        self.user_turns
            .iter()
            .filter(|turn| now - turn.at < 600.0)
            .cloned()
            .collect()
    }

    pub fn current_intent(&self, now: f64) -> Option<&IntentAssessment> {
        self.intent.as_ref().filter(|intent| {
            self.user_turns
                .back()
                .is_some_and(|turn| turn.id == intent.turn_id && now - turn.at < 600.0)
        })
    }
    pub fn suppressed_until(&self) -> f64 { self.suppressed_until }

    pub fn contact_suppressed(&self, now: f64) -> bool {
        now < self.suppressed_until
    }

    pub fn apply_intent(&mut self, assessment: IntentAssessment, now: f64) -> bool {
        if !assessment.confidence.is_finite()
            || !(0.8..=1.0).contains(&assessment.confidence)
            || assessment.evidence.trim().is_empty()
        {
            return false;
        }
        if !self.user_turns.back().is_some_and(|turn| {
            turn.id == assessment.turn_id
                && now - turn.at < 600.0
                && turn.user_text.contains(&assessment.evidence)
        }) {
            return false;
        }
        if !matches!(
            assessment.kind,
            IntentKind::NeedsSpace | IntentKind::Closing
        ) && assessment.pause_seconds != 0
        {
            return false;
        }
        // Reconsidering the same utterance after a world event must not extend its pause.
        if self
            .intent
            .as_ref()
            .is_some_and(|previous| previous.turn_id == assessment.turn_id)
        {
            return false;
        }
        match assessment.kind {
            IntentKind::NeedsSpace | IntentKind::Closing => {
                if !(30..=900).contains(&assessment.pause_seconds) {
                    return false;
                }
                self.suppressed_until = now + assessment.pause_seconds as f64;
                self.pending = None;
            }
            IntentKind::SocialInvitation => self.suppressed_until = 0.0,
            _ => (),
        }
        self.intent = Some(assessment);
        true
    }

    pub fn reconcile(
        &mut self,
        participants: &[Participant],
        user_available: bool,
        user_chatting: bool,
        now: f64,
    ) {
        if self.pending.as_ref().is_some_and(|p| {
            (user_chatting && p.not_before <= now)
                || !participants.iter().any(|r| {
                    r.id == p.speaker_id
                        && r.available
                        && (p.listener_id != "user" || r.can_address_user)
                })
                || (p.listener_id == "user" && !user_available)
                || (p.listener_id != "user"
                    && !participants
                        .iter()
                        .any(|r| r.id == p.listener_id && r.available))
        }) {
            self.pending = None;
        }
    }
    pub fn observe(&mut self, event: SceneEvent) {
        self.events.retain(|e| event.at - e.at < 1800.0);
        self.seen.retain(|_, at| event.at - *at < 3600.0);
        if self.seen.contains_key(&event.id) {
            return;
        }
        self.seen.insert(event.id.clone(), event.at);
        self.events.push_back(event);
        while self.events.len() > 16 {
            self.events.pop_front();
        }
    }

    pub fn events(&self, now: f64) -> Vec<SceneEvent> {
        self.events
            .iter()
            .filter(|e| now - e.at < 1800.0)
            .cloned()
            .collect()
    }

    pub fn finish_evaluation(&mut self, ids: &[String]) {
        self.events.retain(|event| !ids.contains(&event.id));
    }

    /// Single shared budget across all character windows, including failures and abstentions.
    pub fn begin_plan(&mut self, now: f64, participants: &[Participant]) -> bool {
        if self.events(now).is_empty()
            || !participants.iter().any(|p| p.available)
            || self.pending.as_ref().is_some_and(|p| p.expires_at > now)
            || self.last_plan.is_some_and(|last| {
                now - last
                    < if self
                        .events
                        .iter()
                        .any(|e| e.stimulus == Stimulus::UserDialogue && now - e.at < 90.0)
                    {
                        15.0
                    } else {
                        90.0
                    }
            })
        {
            return false;
        }
        self.last_plan = Some(now);
        true
    }

    pub fn accept(
        &mut self,
        selection: Selection,
        participants: &[Participant],
        user_available: bool,
        user_chatting: bool,
        now: f64,
    ) -> bool {
        if self.contact_suppressed(now)
            || selection.delay_seconds > 900
            || !selection.confidence.is_finite()
            || !(0.65..=1.0).contains(&selection.confidence)
        {
            return false;
        }
        let Some(event) = self
            .events
            .iter()
            .find(|e| e.id == selection.event_id && now - e.at < 1800.0)
        else {
            return false;
        };
        let Some(speaker) = participants
            .iter()
            .find(|p| p.id == selection.speaker_id && p.available)
        else {
            return false;
        };
        if event.owner.as_ref().is_some_and(|id| id != &speaker.id) {
            return false;
        }
        let listener_id = match selection.audience.as_str() {
            "user"
                if user_available
                    && (!user_chatting || selection.delay_seconds > 0)
                    && speaker.can_address_user
                    && self
                        .last_user_opportunity
                        .is_none_or(|at| now - at >= 900.0) =>
            {
                "user".to_owned()
            }
            "roommate"
                if (!user_chatting || selection.delay_seconds > 0)
                    && speaker.seconds_since_user_interaction >= 120.0
                    && self
                        .last_peer_opportunity
                        .is_none_or(|at| now - at >= 300.0) =>
            {
                let Some(peer) = participants
                    .iter()
                    .filter(|p| p.id != speaker.id && p.available)
                    .min_by_key(|p| &p.id)
                else {
                    return false;
                };
                peer.id.clone()
            }
            _ => return false,
        };
        let opportunity = Opportunity {
            event_id: event.id.clone(),
            speaker_id: speaker.id.clone(),
            listener_id,
            stimulus: event.stimulus,
            not_before: now + selection.delay_seconds as f64,
            expires_at: now + selection.delay_seconds as f64 + 120.0,
        };
        if opportunity.listener_id == "user" {
            self.last_user_opportunity = Some(now);
        } else {
            self.last_peer_opportunity = Some(now);
        }
        self.events.retain(|e| e.id != opportunity.event_id);
        self.pending = Some(opportunity);
        true
    }

    pub fn nominated_user_speaker(&self, now: f64) -> Option<String> {
        self.pending
            .as_ref()
            .filter(|p| p.not_before <= now && p.expires_at > now && p.listener_id == "user")
            .map(|p| p.speaker_id.clone())
    }

    pub fn take(&mut self, id: &str, now: f64) -> Option<Opportunity> {
        if self.pending.as_ref().is_some_and(|p| p.expires_at <= now) {
            self.pending = None;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.speaker_id == id && p.not_before <= now)
        {
            self.pending.take()
        } else {
            None
        }
    }
}

/// Leave an actionable idle interval before absence. Misconfigured thresholds remain reachable.
pub fn idle_threshold(requested: u64, away: u64) -> f64 {
    (requested as f64).min((away.max(1) as f64) * 0.8)
}

pub fn interaction_probability(probability: f64, since_interaction: f64) -> f64 {
    if since_interaction < 120.0 {
        0.0
    } else if since_interaction < 300.0 {
        probability.clamp(0.05, 0.90) * 0.4
    } else {
        probability.clamp(0.05, 0.90)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roles() -> Vec<Participant> {
        ["vivian", "nana"]
            .map(|id| Participant {
                id: id.into(),
                available: true,
                can_address_user: true,
                loneliness: 0.7,
                current_activity: "idle".into(),
                last_spoken_secs_ago: None,
                seconds_since_user_interaction: 1000.0,
                fatigue: 0.0,
            })
            .to_vec()
    }
    fn selection(id: &str, audience: &str) -> Selection {
        Selection {
            event_id: "rain".into(),
            speaker_id: id.into(),
            audience: audience.into(),
            confidence: 0.8,
            delay_seconds: 0,
        }
    }
    fn policy() -> ScenePolicy {
        let mut p = ScenePolicy::default();
        p.observe(SceneEvent {
            stimulus: Stimulus::WorldChanged,
            id: "rain".into(),
            fact: "Weather reports rain".into(),
            owner: None,
            at: 1000.0,
        });
        p
    }
    #[test]
    fn budget_and_events_are_shared_across_windows() {
        let mut p = policy();
        assert!(p.begin_plan(1000.0, &roles()));
        assert!(!p.begin_plan(1000.0, &roles()));
        assert!(!p.begin_plan(1089.0, &roles()));
        assert!(p.accept(selection("nana", "user"), &roles(), true, false, 1090.0));
        assert_eq!(p.nominated_user_speaker(1091.0).as_deref(), Some("nana"));
        assert!(p.take("vivian", 1091.0).is_none());
        assert_eq!(
            p.take("nana", 1091.0).unwrap().stimulus,
            Stimulus::WorldChanged
        );
        assert!(p.take("nana", 1091.0).is_none());
        assert!(p.events(1091.0).is_empty());
    }
    #[test]
    fn model_cannot_invent_events_characters_or_override_gates() {
        assert!(!policy().accept(selection("unknown", "user"), &roles(), true, false, 1000.0));
        let mut made_up = selection("nana", "user");
        made_up.event_id = "invented".into();
        assert!(!policy().accept(made_up, &roles(), true, false, 1000.0));
        assert!(!policy().accept(selection("nana", "user"), &roles(), false, false, 1000.0));
        assert!(!policy().accept(selection("nana", "roommate"), &roles(), true, true, 1000.0));
        let mut disabled = roles();
        disabled[1].available = false;
        assert!(!policy().accept(selection("nana", "user"), &disabled, true, false, 1000.0));
        assert!(!policy().accept(
            selection("vivian", "roommate"),
            &disabled,
            false,
            false,
            1000.0
        ));
        assert!(!policy().accept(selection("nana", "user"), &roles(), true, false, 2800.0));
    }
    #[test]
    fn absence_allows_peer_contact_and_freezes_recipient() {
        let mut p = policy();
        assert!(p.accept(
            selection("nana", "roommate"),
            &roles(),
            false,
            false,
            1000.0
        ));
        let op = p.take("nana", 1001.0).unwrap();
        assert_eq!(op.listener_id, "vivian");
        let mut p = policy();
        assert!(p.accept(selection("nana", "user"), &roles(), true, false, 1000.0));
        assert!(p.take("nana", 1120.0).is_none());
    }
    #[test]
    fn personal_state_is_owned_and_duplicate_observations_do_not_rearm() {
        let mut p = policy();
        p.events[0].owner = Some("nana".into());
        assert!(!p.accept(
            selection("vivian", "roommate"),
            &roles(),
            false,
            false,
            1000.0
        ));
        assert!(p.accept(
            selection("nana", "roommate"),
            &roles(),
            false,
            false,
            1000.0
        ));
        p.take("nana", 1001.0);
        p.observe(SceneEvent {
            stimulus: Stimulus::WorldChanged,
            id: "rain".into(),
            fact: "rain".into(),
            owner: None,
            at: 1002.0,
        });
        assert!(p.events(1002.0).is_empty());
    }
    #[test]
    fn settings_changes_new_input_and_departures_cancel_pending_contact() {
        let mut p = policy();
        assert!(p.accept(
            selection("nana", "roommate"),
            &roles(),
            false,
            false,
            1000.0
        ));
        let mut gone = roles();
        gone[0].available = false;
        p.reconcile(&gone, false, false, 1001.0);
        assert!(p.take("nana", 1001.0).is_none());
        let mut p = policy();
        assert!(p.accept(selection("nana", "user"), &roles(), true, false, 1000.0));
        p.reconcile(&roles(), true, true, 1001.0);
        assert!(p.nominated_user_speaker(1001.0).is_none());
        let mut p = policy();
        assert!(p.accept(selection("nana", "user"), &roles(), true, false, 1000.0));
        p.invalidate();
        assert!(p.take("nana", 1001.0).is_none());
    }
    #[test]
    fn abstention_is_terminal_and_new_events_survive_an_older_evaluation() {
        let mut p = policy();
        assert!(p.begin_plan(1000.0, &roles()));
        p.observe(SceneEvent {
            stimulus: Stimulus::WorldChanged,
            id: "new".into(),
            fact: "new observation".into(),
            owner: None,
            at: 1001.0,
        });
        p.finish_evaluation(&["rain".into()]);
        assert_eq!(p.events(1002.0).len(), 1);
        assert!(p.begin_plan(1090.0, &roles()));
        p.finish_evaluation(&["new".into()]);
        assert!(!p.begin_plan(1180.0, &roles()));
    }
    #[test]
    fn idle_greeting_has_a_window_and_recent_chat_is_strictly_silent() {
        assert!(idle_threshold(600, 600) < 600.0);
        assert_eq!(idle_threshold(300, 600), 300.0);
        assert_eq!(interaction_probability(0.9, 119.0), 0.0);
        assert!((interaction_probability(0.9, 120.0) - 0.36).abs() < 1e-12);
    }
    #[test]
    fn malformed_or_abstaining_model_output_cannot_become_an_opportunity() {
        for raw in [
            "DONT_NOTIFY",
            "{\"opportunity\":",
            "{\"opportunity\":null}",
            "{\"opportunity\":{\"speaker_id\":\"nana\"}}",
            "text without a selection",
        ] {
            assert!(Selection::parse(raw).is_none());
        }
        let raw = r#"{"opportunity":{"event_id":"rain","speaker_id":"nana","audience":"user","confidence":0.8}}"#;
        assert!(policy().accept(
            Selection::parse(raw).unwrap(),
            &roles(),
            true,
            false,
            1000.0
        ));
    }

    fn intent(
        p: &ScenePolicy,
        kind: IntentKind,
        evidence: &str,
        pause_seconds: u64,
    ) -> IntentAssessment {
        IntentAssessment {
            turn_id: p.user_turns.back().unwrap().id.clone(),
            kind,
            evidence: evidence.into(),
            confidence: 0.9,
            pause_seconds,
        }
    }

    #[test]
    fn intent_controls_only_timing_and_requires_actual_latest_user_evidence() {
        let mut p = policy();
        p.record_user_turn("vivian", "先安静五分钟，我忙完再聊", "好，你先忙", 1000.0);
        let mut fake = intent(&p, IntentKind::NeedsSpace, "assistant says be quiet", 300);
        assert!(!p.apply_intent(fake.clone(), 1001.0));
        fake.evidence = "先安静五分钟".into();
        fake.turn_id = "invented".into();
        assert!(!p.apply_intent(fake, 1001.0));
        let valid = intent(&p, IntentKind::NeedsSpace, "先安静五分钟", 300);
        assert!(p.apply_intent(valid.clone(), 1001.0));
        assert!(!p.apply_intent(valid.clone(), 1200.0));
        assert!(p.contact_suppressed(1300.0));
        assert!(!p.contact_suppressed(1301.0));
        assert!(!p.accept(selection("nana", "user"), &roles(), true, false, 1002.0));
        p.record_user_turn("vivian", "现在可以一起聊了", "好呀", 1003.0);
        assert!(!p.apply_intent(valid, 1004.0));
        assert!(p.current_intent(1004.0).is_none());
        let resume = intent(&p, IntentKind::SocialInvitation, "现在可以一起聊了", 0);
        assert!(p.apply_intent(resume, 1004.0));
        assert!(!p.contact_suppressed(1004.0));
        assert!(p.current_intent(1603.0).is_none());
    }

    #[test]
    fn task_request_is_context_not_authority_to_do_work_or_pause_the_character() {
        let mut p = policy();
        p.record_user_turn("vivian", "帮我查一下这个报错", "我来看看", 1000.0);
        let task = intent(&p, IntentKind::TaskRequest, "帮我查一下这个报错", 0);
        assert!(p.apply_intent(task, 1001.0));
        assert!(!p.contact_suppressed(1001.0));
        let invented_pause = intent(&p, IntentKind::TaskRequest, "帮我查一下这个报错", 600);
        assert!(!p.apply_intent(invented_pause, 1002.0));
        let too_long = intent(&p, IntentKind::NeedsSpace, "帮我查一下这个报错", 3600);
        assert!(!p.apply_intent(too_long, 1002.0));
        assert!(!p.contact_suppressed(1002.0));
    }

    #[test]
    fn delayed_dispatch_waits_without_suppressing_current_user_turn_and_new_input_supersedes_it() {
        let mut p = policy();
        let mut delayed = selection("nana", "user");
        delayed.delay_seconds = 120;
        assert!(p.accept(delayed, &roles(), true, true, 1000.0));
        p.reconcile(&roles(), true, true, 1001.0);
        assert!(p.nominated_user_speaker(1119.0).is_none());
        assert!(p.take("nana", 1119.0).is_none());
        assert_eq!(p.nominated_user_speaker(1120.0).as_deref(), Some("nana"));
        assert!(p.take("nana", 1120.0).is_some());

        let mut p = policy();
        let mut delayed = selection("nana", "user");
        delayed.delay_seconds = 60;
        assert!(p.accept(delayed, &roles(), true, false, 1000.0));
        p.record_user_turn("vivian", "等一下", "嗯", 1001.0);
        assert!(p.take("nana", 1060.0).is_none());
    }

    #[test]
    fn dispatch_envelope_cannot_carry_planner_text_into_character_context() {
        let mut p = policy();
        p.events[0].fact =
            "INTERNAL PLANNER ANALYSIS: secret intent and scheduling reasoning".into();
        assert!(p.accept(selection("nana", "user"), &roles(), true, false, 1000.0));
        let dispatch = serde_json::to_value(p.take("nana", 1001.0).unwrap()).unwrap();
        assert!(dispatch.get("fact").is_none());
        assert!(dispatch.get("user_intent").is_none());
        assert!(!dispatch.to_string().contains("INTERNAL PLANNER"));
        assert_eq!(dispatch["stimulus"], "world_changed");
    }

    #[test]
    fn planner_parses_intent_and_deferral_without_accepting_unbounded_commands() {
        let raw = r#"{"user_intent":{"turn_id":"user_turn:1","kind":"needs_space","evidence":"先安静","confidence":0.9,"pause_seconds":300},"opportunity":null}"#;
        let plan = PlanningDecision::parse(raw).unwrap();
        assert_eq!(plan.user_intent.unwrap().kind, IntentKind::NeedsSpace);
        assert!(plan.opportunity.is_none());
        assert!(PlanningDecision::parse(r#"{"user_intent":{"kind":"execute_shell"}}"#).is_none());
        assert!(PlanningDecision::parse("not JSON").is_none());
    }

    #[test]
    fn public_input_is_bounded_fresh_and_does_not_create_a_per_tick_planning_loop() {
        let mut p = ScenePolicy::default();
        for i in 0..8 {
            p.record_user_turn("vivian", &format!("user {i}"), "reply", 1000.0 + i as f64);
        }
        assert_eq!(p.recent_user_turns(1008.0).len(), 4);
        assert!(p.begin_plan(1008.0, &roles()));
        assert!(!p.begin_plan(1022.0, &roles()));
        assert!(p.begin_plan(1023.0, &roles()));
        let ids: Vec<_> = p.events(1023.0).iter().map(|e| e.id.clone()).collect();
        p.finish_evaluation(&ids);
        assert!(!p.begin_plan(1040.0, &roles()));
        assert!(p.recent_user_turns(1607.0).is_empty());
    }
}
