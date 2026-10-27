//! Conversation opportunities belong to meanings and observed changes, not clock slots.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationIntent { #[default] Silent, Greet, FollowUp, Share, AskUser }

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSource { #[default] None, UserMessage, WorldObservation, SceneEvent, ThoughtThread }

#[derive(Debug, Clone, Deserialize)]
pub struct ThoughtSynthesisOutput {
    pub thought: String,
    #[serde(default = "default_social_urge")] pub social_urge: f32,
    #[serde(default)] pub conversation_intent: ConversationIntent,
    #[serde(default)] pub conversation_source: ConversationSource,
    #[serde(default)] pub conversation_topic: String,
    #[serde(default)] pub conversation_evidence: String,
    /// Short communicative purpose, not private reasoning or a script to read aloud.
    #[serde(default)] pub conversation_reason: String,
    /// Stable name for the actual point; rewording the same point keeps the same key.
    #[serde(default)] pub conversation_key: String,
    #[serde(default)] pub conversation_event_id: String,
    #[serde(default, deserialize_with = "parse_thread_updates")]
    pub thread_updates: Vec<super::thought_threads::ThreadUpdate>,
}
pub fn default_social_urge() -> f32 { 0.5 }

// An invalid optional state update must not discard a valid thought or authorize speech.
fn parse_thread_updates<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<super::thought_threads::ThreadUpdate>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_array().into_iter().flatten().take(2)
        .filter_map(|item| serde_json::from_value(item.clone()).ok()).collect())
}

/// Prevent overlapping thought calls; cancellation releases admission too.
pub struct ThoughtRefreshPermit(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl ThoughtRefreshPermit {
    pub fn acquire(flag: &std::sync::Arc<std::sync::atomic::AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false,true,std::sync::atomic::Ordering::SeqCst,std::sync::atomic::Ordering::SeqCst)
            .ok().map(|_| Self(flag.clone()))
    }
}
impl Drop for ThoughtRefreshPermit {
    fn drop(&mut self) { self.0.store(false,std::sync::atomic::Ordering::SeqCst); }
}

#[derive(Debug, Clone)]
pub struct ConversationOpportunity {
    pub intent: ConversationIntent,
    pub source: ConversationSource,
    pub topic: String,
    pub evidence: String,
    pub reason: String,
    pub meaning_key: String,
    pub event_id: String,
    pub generated_at: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEvent { pub id: String, pub kind: String, pub fact: String, pub at: f64 }
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationScene {
    pub user_present: bool,
    pub music: Option<String>,
    pub window: Option<String>,
    /// Full date + festival. A calendar day alone creates no obligation.
    pub festival: Option<String>,
}

impl ThoughtSynthesisOutput {
    pub fn validated_opportunity(&self, user_quotes: &[(&str, i64)], world_observations: &[&str],
        events: &[ConversationEvent], user_present: bool, now: f64) -> Option<ConversationOpportunity> {
        if !user_present || !now.is_finite() || self.conversation_intent == ConversationIntent::Silent
            || self.conversation_topic.trim().is_empty() || self.conversation_reason.trim().is_empty()
            || normalize(&self.conversation_key).is_empty() { return None; }
        let event = events.iter().find(|event| event.id == self.conversation_event_id && event.at <= now);
        if !self.conversation_event_id.is_empty() && event.is_none() { return None; }
        let evidence = self.conversation_evidence.trim();
        if self.conversation_intent == ConversationIntent::AskUser && self.conversation_source != ConversationSource::ThoughtThread { return None; }
        let grounded = match self.conversation_source {
            ConversationSource::None => self.conversation_intent == ConversationIntent::Greet && evidence.is_empty(),
            // Provided context and corrections decide relevance, not a ten-minute cutoff.
            ConversationSource::UserMessage => evidence.chars().count() >= 3
                && user_quotes.iter().any(|(text, at)| *at as f64 <= now && text.contains(evidence)),
            ConversationSource::WorldObservation => self.conversation_intent == ConversationIntent::Share
                && evidence.chars().count() >= 3 && world_observations.iter().any(|text| text.contains(evidence)),
            ConversationSource::SceneEvent => evidence.chars().count() >= 3
                && event.is_some_and(|event| event.kind != "personal_thought" && event.fact.contains(evidence)),
            ConversationSource::ThoughtThread => matches!(self.conversation_intent, ConversationIntent::Share | ConversationIntent::AskUser)
                && evidence.chars().count() >= 3
                && event.is_some_and(|event| event.kind == "personal_thought" && event.fact.contains(evidence)),
        };
        grounded.then(|| ConversationOpportunity {
            intent: self.conversation_intent, source: self.conversation_source,
            topic: self.conversation_topic.trim().chars().take(80).collect(),
            evidence: evidence.chars().take(160).collect(),
            reason: self.conversation_reason.trim().chars().take(160).collect(),
            meaning_key: self.conversation_key.trim().chars().take(100).collect(),
            event_id: self.conversation_event_id.clone(), generated_at: now,
        })
    }
}
fn normalize(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}
fn same_meaning(a: &ConversationOpportunity, b: &ConversationOpportunity) -> bool {
    normalize(&a.meaning_key) == normalize(&b.meaning_key) && a.source == b.source
        && normalize(&a.evidence) == normalize(&b.evidence) && a.event_id == b.event_id
}
#[derive(Debug, Default)]
pub struct ConversationOpportunities {
    pub threads: super::thought_threads::ThoughtThreads,
    pending: Option<ConversationOpportunity>,
    prepared: Vec<(ConversationOpportunity, String)>,
    spoken: Vec<ConversationOpportunity>,
    attempted: Vec<ConversationOpportunity>,
    scene: Option<ConversationScene>,
    events: Vec<ConversationEvent>,
    event_sequence: u64,
}
impl ConversationOpportunities {
    pub fn apply_synthesis(&mut self, output: &ThoughtSynthesisOutput,
        grounding: &super::thought_threads::ThoughtGrounding<'_>, user_present: bool, now: f64) -> bool {
        let mut proposal = output.validated_opportunity(grounding.user_quotes, grounding.world, grounding.scene, user_present, now);
        let changed = self.threads.apply(&output.thread_updates, grounding, now as i64);
        if proposal.as_ref().is_some_and(|candidate| candidate.source == ConversationSource::ThoughtThread
            && !self.threads.permits(&candidate.event_id, candidate.intent, now as i64)) { proposal = None; }
        self.replace(proposal);
        changed
    }
    pub fn replace(&mut self, proposal: Option<ConversationOpportunity>) { self.pending = proposal; }
    /// Polling an unchanged scene does nothing. Edges request thought synthesis once.
    pub fn observe_scene(&mut self, scene: ConversationScene, now: f64) -> bool {
        if !now.is_finite() { return false; }
        let previous = self.scene.replace(scene.clone());
        let Some(previous) = previous else {
            if let Some(fact) = scene.festival { self.observe("festival", fact, now); return true; }
            return false;
        };
        let mut changed = false;
        if previous.user_present != scene.user_present {
            self.events.retain(|event| event.kind != "user_returned");
            if scene.user_present { self.observe("user_returned", "用户刚从离开状态回到桌面。".into(), now); }
            changed = true;
        }
        for (kind, before, after) in [
            ("music_changed", previous.music, scene.music),
            ("window_changed", previous.window, scene.window),
            ("festival", previous.festival, scene.festival),
        ] {
            if before == after { continue; }
            self.events.retain(|event| event.kind != kind);
            if let Some(fact) = after { self.observe(kind, fact, now); }
            changed = true;
        }
        changed
    }
    fn observe(&mut self, kind: &str, fact: String, now: f64) {
        self.event_sequence = self.event_sequence.wrapping_add(1);
        self.events.push(ConversationEvent {id:format!("{kind}:{}",self.event_sequence),kind:kind.into(),fact,at:now});
        if self.events.len() > 8 { self.events.remove(0); }
    }
    /// Completed reading/work material is an actual stimulus; a private inference is not.
    pub fn observe_material(&mut self, kind: &str, fact: String, now: f64) -> bool {
        let fact: String = fact.chars().take(1200).collect();
        if fact.trim().is_empty() || !now.is_finite()
            || self.events.iter().any(|event| event.kind == kind && event.fact == fact) { return false; }
        self.events.retain(|event| event.kind != kind);
        self.observe(kind, fact, now);
        true
    }
    pub fn events(&self) -> &[ConversationEvent] { &self.events }
    /// Deferral keeps the proposal. Context changes or handling retire it; its age is
    /// supplied to the final generator so an awkward delayed remark can be declined.
    pub fn fresh(&self, now: f64) -> Option<ConversationOpportunity> {
        let proposal = self.pending.as_ref()?;
        if !now.is_finite() || now < proposal.generated_at || normalize(&proposal.meaning_key).is_empty()
            || (proposal.source == ConversationSource::ThoughtThread && !self.threads.permits(&proposal.event_id, proposal.intent, now as i64))
            || (proposal.source != ConversationSource::ThoughtThread && !proposal.event_id.is_empty() && !self.events.iter().any(|event| event.id == proposal.event_id))
            || self.spoken.iter().chain(&self.attempted).any(|previous| same_meaning(previous, proposal))
            || self.prepared.iter().any(|(previous, _)| same_meaning(previous, proposal)) { return None; }
        Some(proposal.clone())
    }
    pub fn take(&mut self, now: f64) -> Option<ConversationOpportunity> {
        let proposal = self.fresh(now);
        self.pending = None;
        if let Some(proposal) = &proposal {
            if proposal.source == ConversationSource::ThoughtThread { self.threads.attempt(&proposal.event_id); }
            self.attempted.push(proposal.clone());
            if self.attempted.len() > 32 { self.attempted.remove(0); }
        }
        proposal
    }
    pub fn restore_after_failure(&mut self, proposal: ConversationOpportunity) {
        if proposal.source == ConversationSource::ThoughtThread { self.threads.restore(&proposal.event_id); }
        self.attempted.retain(|previous| !same_meaning(previous, &proposal));
        if self.pending.is_none() { self.pending=Some(proposal); }
    }
    pub fn record_prepared(&mut self, proposal: ConversationOpportunity, text: &str, _now: f64) {
        self.prepared.retain(|(_, content)| content != text);
        self.prepared.push((proposal,text.to_owned()));
        if self.prepared.len() > 16 { self.prepared.remove(0); }
    }
    pub fn record_delivered(&mut self, text: &str, now: f64) {
        if let Some(index)=self.prepared.iter().position(|(_,content)|content==text) {
            let (proposal,_)=self.prepared.remove(index);
            if proposal.source == ConversationSource::ThoughtThread { self.threads.delivered(&proposal.event_id, proposal.intent, now as i64); }
            self.spoken.push(proposal);
            if self.spoken.len()>32 {self.spoken.remove(0);}
        }
    }
    pub fn recent_topics(&self, _now:f64)->Vec<&str> {self.spoken.iter().map(|p|p.topic.as_str()).collect()}
    pub fn evaluated_meanings(&self) -> Vec<serde_json::Value> {
        self.spoken.iter().rev().chain(self.attempted.iter().rev()).take(12).map(|proposal| serde_json::json!({
            "meaning_key":proposal.meaning_key,"topic":proposal.topic,"evidence":proposal.evidence,
            "event_id":proposal.event_id,
        })).collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn output(intent:&str,source:&str,evidence:&str)->ThoughtSynthesisOutput {
        serde_json::from_value(serde_json::json!({"thought":"陪你待着","social_urge":0.9,
            "conversation_intent":intent,"conversation_source":source,"conversation_topic":"问候",
            "conversation_reason":"想轻轻确认这次相遇","conversation_key":"acknowledge_encounter",
            "conversation_evidence":evidence})).unwrap()
    }
    fn scene(present:bool,music:Option<&str>)->ConversationScene {
        ConversationScene{user_present:present,music:music.map(str::to_owned),window:None,festival:None}
    }
    fn greeting()->ConversationOpportunity {output("greet","none","").validated_opportunity(&[],&[],&[],true,1000.0).unwrap()}
    #[test] fn score_and_missing_purpose_cannot_authorize_speech() {
        assert!(output("silent","none","").validated_opportunity(&[],&[],&[],true,1000.0).is_none());
        let old:ThoughtSynthesisOutput=serde_json::from_str(r#"{"thought":"想聊聊","social_urge":1.0,"conversation_intent":"greet"}"#).unwrap();
        assert!(old.validated_opportunity(&[],&[],&[],true,1000.0).is_none());
        assert!(output("greet","none","").validated_opportunity(&[],&[],&[],false,1000.0).is_none());
    }
    #[test] fn unresolved_quotes_do_not_expire_but_claims_still_need_actual_evidence() {
        let follow=output("follow_up","user_message","明天考试");
        assert!(follow.validated_opportunity(&[("我明天考试",100)],&[],&[],true,3600.0).is_some());
        assert!(follow.validated_opportunity(&[],&["我明天考试"],&[],true,3600.0).is_none());
        assert!(follow.validated_opportunity(&[("我明天考试",3601)],&[],&[],true,3600.0).is_none());
    }
    #[test] fn busy_deferral_preserves_the_opportunity_until_context_cancels_it() {
        let mut state=ConversationOpportunities::default();state.replace(Some(greeting()));
        assert!(state.fresh(999.0).is_none());assert!(state.fresh(1800.0).is_some());
        state.replace(None);assert!(state.fresh(1801.0).is_none());
    }
    #[test] fn time_alone_never_rearms_a_spoken_or_declined_meaning() {
        let mut state=ConversationOpportunities::default();state.replace(Some(greeting()));
        let proposal=state.take(1001.0).unwrap();state.replace(Some(proposal.clone()));
        assert!(state.fresh(1002.0).is_none());assert!(state.fresh(100000.0).is_none());
        state.record_prepared(proposal,"嗨",1002.0);state.record_delivered("嗨",1003.0);
        assert_eq!(state.recent_topics(100000.0),vec!["问候"]);
    }
    #[test] fn a_new_point_or_material_change_can_follow_immediately() {
        let mut state=ConversationOpportunities::default();state.replace(Some(greeting()));
        let mut proposal=state.take(1001.0).unwrap();proposal.meaning_key="new_song_observation".into();
        proposal.intent=ConversationIntent::Share;proposal.source=ConversationSource::WorldObservation;
        proposal.evidence="Song A".into();state.replace(Some(proposal.clone()));
        assert!(state.take(1002.0).is_some());proposal.evidence="Song B".into();state.replace(Some(proposal));
        assert!(state.fresh(1003.0).is_some());
    }
    #[test] fn observed_edges_are_unique_and_stale_event_ids_are_rejected() {
        let mut state=ConversationOpportunities::default();assert!(!state.observe_scene(scene(true,None),1000.0));
        assert!(!state.observe_scene(scene(true,None),1001.0));assert!(state.observe_scene(scene(false,None),1002.0));
        assert!(state.observe_scene(scene(true,None),1003.0));let events=state.events().to_vec();
        let mut reply=output("greet","scene_event","用户刚从离开状态回到桌面");reply.conversation_event_id=events[0].id.clone();
        state.replace(reply.validated_opportunity(&[],&[],&events,true,1004.0));assert!(state.fresh(1004.0).is_some());
        state.observe_scene(scene(false,None),1005.0);assert!(state.fresh(1006.0).is_none());
        reply.conversation_event_id="invented".into();assert!(reply.validated_opportunity(&[],&[],&events,true,1007.0).is_none());
    }
    #[test] fn unchanged_polling_and_new_days_alone_never_create_a_reason_to_speak() {
        let mut state=ConversationOpportunities::default();state.observe_scene(scene(true,Some("Song A")),1000.0);
        assert!(!state.observe_scene(scene(true,Some("Song A")),100000.0));assert!(state.events().is_empty());
        assert!(state.observe_scene(scene(true,Some("Song B")),100001.0));assert_eq!(state.events().len(),1);
        assert!(!state.observe_scene(scene(true,Some("Song B")),100002.0));
    }
    #[test] fn only_receipts_record_a_topic_as_spoken() {
        let mut state=ConversationOpportunities::default();state.record_prepared(greeting(),"嗨",1000.0);
        state.record_delivered("other",1001.0);assert!(state.recent_topics(1002.0).is_empty());
        state.record_delivered("嗨",1003.0);assert_eq!(state.recent_topics(1004.0),vec!["问候"]);
    }
    #[test] fn thought_admission_is_exclusive_and_drop_releases_it() {
        let flag=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let permit=ThoughtRefreshPermit::acquire(&flag).unwrap();
        assert!(ThoughtRefreshPermit::acquire(&flag).is_none());drop(permit);
        assert!(ThoughtRefreshPermit::acquire(&flag).is_some());
    }
    #[test] fn a_technical_failure_can_retry_but_does_not_overwrite_a_newer_proposal() {
        let mut state=ConversationOpportunities::default();state.replace(Some(greeting()));
        let failed=state.take(1001.0).unwrap();state.restore_after_failure(failed.clone());
        assert!(state.fresh(1002.0).is_some());
        let mut newer=failed.clone();newer.meaning_key="new_material_point".into();
        state.replace(Some(newer));state.restore_after_failure(failed);
        assert_eq!(state.fresh(1003.0).unwrap().meaning_key,"new_material_point");
    }
}
