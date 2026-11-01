//! Small, durable personal concerns. These are subjective state, never user facts.
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

const CAPACITY: usize = 6;
const ACTIVE_TTL: i64 = 14 * 86400;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThoughtStep { #[default] Reflect, AskUser, AskPeer, Share, SharePeer, Rest }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionScope { General, #[default] UserOnly }
pub const PEER_FALLBACK_AFTER_SECS: i64 = 180;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus { #[default] Active, Resolved, Parked }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThoughtOrigin { Persona, UserMessage, WorldObservation, SceneEvent, Reflection }

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadUpdate {
    pub key: String,
    pub topic: String,
    pub personal_relevance: String,
    pub stance: String,
    pub open_question: String,
    pub next_step: ThoughtStep,
    pub status: ThreadStatus,
    pub origin: ThoughtOrigin,
    pub evidence: String,
    #[serde(default)] pub question_scope: QuestionScope,
    #[serde(default)] pub peer_fallback: bool,
}

pub struct ThoughtGrounding<'a> {
    pub persona: &'a str,
    pub user_quotes: &'a [(&'a str, i64)],
    pub world: &'a [&'a str],
    pub scene: &'a [super::conversation::ConversationEvent],
    pub reflections: &'a [&'a str],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThoughtThread {
    pub key: String,
    pub topic: String,
    pub personal_relevance: String,
    pub stance: String,
    pub open_question: String,
    pub next_step: ThoughtStep,
    pub status: ThreadStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub revision: u64,
    pub awaiting_reply: bool,
    pub last_spoken_at: Option<i64>,
    attempted_revision: u64,
    source_token: u64,
    reflections: u8,
    #[serde(default)] pub question_scope: QuestionScope,
    #[serde(default)] pub peer_fallback: bool,
    #[serde(default)] peer_attempted_revision: u64,
    #[serde(default)] fallback_ready: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThoughtThreads {
    entries: Vec<ThoughtThread>,
    sequence: u64,
    #[serde(default)]
    retired: Vec<RetiredThought>,
    #[serde(default)]
    pub continuation: super::continuation::ContinuationState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RetiredThought { key: String, topic: String, source_token: u64, at: i64 }

fn clip(text: &str, length: usize) -> String { text.trim().chars().take(length).collect() }
fn normalized(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}
fn token(origin: ThoughtOrigin, evidence: &str, at: Option<i64>) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    format!("{origin:?}").hash(&mut hash);
    evidence.hash(&mut hash);
    at.hash(&mut hash);
    hash.finish()
}

impl ThoughtGrounding<'_> {
    fn source(&self, update: &ThreadUpdate, now: i64) -> Option<(u64, Option<i64>)> {
        let quote = update.evidence.trim();
        if quote.chars().count() < 3 { return None; }
        let at = match update.origin {
            ThoughtOrigin::Persona if self.persona.contains(quote) => None,
            ThoughtOrigin::UserMessage => Some(self.user_quotes.iter()
                .filter(|(text, at)| *at <= now && text.contains(quote)).map(|(_, at)| *at).max()?),
            ThoughtOrigin::WorldObservation if self.world.iter().any(|text| text.contains(quote)) => None,
            ThoughtOrigin::SceneEvent => Some(self.scene.iter()
                .filter(|event| event.kind != "personal_thought" && event.at <= now as f64 && event.fact.contains(quote))
                .map(|event| event.at as i64).max()?),
            ThoughtOrigin::Reflection if self.reflections.iter().any(|text| text.contains(quote)) => None,
            _ => return None,
        };
        // Different excerpts of the same cached observation are the same source.
        let source = if update.origin == ThoughtOrigin::WorldObservation {
            *self.world.iter().find(|text| text.contains(quote))?
        } else { quote };
        Some((token(update.origin, source, at), at))
    }
}

impl ThoughtThread {
    pub fn event_id(&self) -> String { format!("thought:{}:{}", self.key, self.revision) }
    fn speakable(&self, now: i64) -> bool {
        self.status == ThreadStatus::Active && !self.awaiting_reply
            && self.revision > self.attempted_revision
            && self.updated_at <= now && now - self.updated_at <= ACTIVE_TTL
            && matches!(self.next_step, ThoughtStep::AskUser | ThoughtStep::AskPeer | ThoughtStep::Share | ThoughtStep::SharePeer)
            && (!matches!(self.next_step, ThoughtStep::AskUser | ThoughtStep::AskPeer) || !self.open_question.is_empty())
            && (self.next_step != ThoughtStep::AskPeer || self.question_scope == QuestionScope::General)
    }
    fn fallback_due(&self, now: i64) -> bool {
        self.status == ThreadStatus::Active && self.awaiting_reply && self.next_step == ThoughtStep::AskUser
            && self.question_scope == QuestionScope::General && self.peer_fallback
            && !self.open_question.is_empty() && self.peer_attempted_revision < self.revision
            && self.last_spoken_at.is_some_and(|at| now >= at && now - at >= PEER_FALLBACK_AFTER_SECS)
            && now >= self.updated_at && now - self.updated_at <= ACTIVE_TTL
    }
}

impl ThoughtThreads {
    pub fn continuation_anchors(&self, now: i64, eligible_only: bool) -> Vec<super::continuation::ConcernAnchor> {
        self.entries.iter().filter(|thread| !eligible_only || (
            thread.status == ThreadStatus::Active && !thread.awaiting_reply
            && thread.updated_at <= now && now - thread.updated_at <= ACTIVE_TTL
            && thread.next_step != ThoughtStep::Rest && thread.reflections < 2
            && (thread.next_step == ThoughtStep::Reflect || thread.revision > thread.attempted_revision)
        )).map(|thread| super::continuation::ConcernAnchor {
            key: thread.key.clone(), revision: thread.revision, source: thread.source_token,
        }).collect()
    }

    pub fn plan_continuation(&mut self, request: Option<&super::continuation::ContinuationRequest>, now: i64) -> bool {
        let before = self.continuation.clone();
        let live = self.continuation_anchors(now, false);
        self.continuation.retain_sources(&live);
        self.continuation.finish(request, &self.continuation_anchors(now, true), now);
        self.continuation != before
    }

    /// Also sanitizes restored state. Expiry never manufactures a new speaking reason.
    pub fn prune(&mut self, now: i64) {
        self.entries.retain(|thread| now.saturating_sub(thread.updated_at) <= ACTIVE_TTL
            && thread.created_at <= now && thread.updated_at <= now);
        for thread in &mut self.entries {
            thread.key = clip(&thread.key, 64);
            thread.topic = clip(&thread.topic, 80);
            thread.personal_relevance = clip(&thread.personal_relevance, 120);
            thread.stance = clip(&thread.stance, 180);
            thread.open_question = clip(&thread.open_question, 120);
            self.sequence = self.sequence.max(thread.revision);
        }
        self.entries.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
        self.entries.truncate(CAPACITY);
        self.retired.retain(|item| item.at <= now && now - item.at <= ACTIVE_TTL);
        self.retired.truncate(12);
    }

    /// At most two changes per existing synthesis request. No extra model loop.
    pub fn apply(&mut self, updates: &[ThreadUpdate], grounding: &ThoughtGrounding<'_>, now: i64) -> bool {
        self.prune(now);
        let mut changed = false;
        for update in updates.iter().take(2) {
            let key = clip(&update.key, 64);
            if key.is_empty() || key != update.key || !key.chars().all(|c| c.is_alphanumeric() || c == '_') { continue; }
            let Some((source_token, source_at)) = grounding.source(update, now) else { continue; };
            let topic = clip(&update.topic, 80);
            let relevance = clip(&update.personal_relevance, 120);
            let stance = clip(&update.stance, 180);
            let question = clip(&update.open_question, 120);
            if topic.is_empty() || relevance.is_empty() || stance.is_empty()
                || (matches!(update.next_step, ThoughtStep::AskUser | ThoughtStep::AskPeer) && question.is_empty())
                || (update.next_step == ThoughtStep::AskPeer && update.question_scope != QuestionScope::General) { continue; }
            let is_external = matches!(update.origin, ThoughtOrigin::UserMessage | ThoughtOrigin::WorldObservation | ThoughtOrigin::SceneEvent);
            if let Some(thread) = self.entries.iter_mut().find(|thread| thread.key == key) {
                let fresh_external = is_external && source_token != thread.source_token
                    && source_at.is_none_or(|at| at >= thread.updated_at);
                // Silence is not an answer. Only a cited subsequent user reply can clear a pending question.
                let answered = update.origin == ThoughtOrigin::UserMessage
                    && source_at.zip(thread.last_spoken_at).is_some_and(|(at, spoken)| at > spoken);
                let peer_answered = thread.question_scope == QuestionScope::General && update.origin == ThoughtOrigin::SceneEvent
                    && grounding.scene.iter().any(|event| event.kind == "peer_answer"
                        && event.id.starts_with(&format!("peer_answer:{}:", thread.event_id()))
                        && event.fact.contains(update.evidence.trim())
                        && event.at as i64 >= thread.last_spoken_at.unwrap_or(thread.created_at));
                let parking = update.status == ThreadStatus::Parked && update.next_step == ThoughtStep::Rest;
                if (thread.awaiting_reply && !answered && !peer_answered && !parking)
                    || (thread.status != ThreadStatus::Active && !fresh_external)
                    || (!fresh_external && thread.reflections >= 2) { continue; }
                let semantic_change = normalized(&thread.stance) != normalized(&stance)
                    || normalized(&thread.open_question) != normalized(&question)
                    || thread.status != update.status || thread.next_step != update.next_step;
                if !semantic_change { continue; }
                self.sequence = self.sequence.saturating_add(1);
                thread.topic = topic;
                thread.personal_relevance = relevance;
                thread.stance = stance;
                thread.open_question = question;
                thread.next_step = update.next_step;
                thread.question_scope = update.question_scope;
                thread.peer_fallback = update.peer_fallback && update.question_scope == QuestionScope::General;
                thread.fallback_ready = false;
                thread.status = update.status;
                thread.updated_at = now;
                thread.revision = self.sequence;
                thread.reflections = if fresh_external { 0 } else { thread.reflections + 1 };
                if fresh_external { thread.source_token = source_token; }
                if answered || peer_answered { thread.awaiting_reply = false; }
            } else {
                // Reflective prose is not independent evidence for inventing a new concern.
                if update.origin == ThoughtOrigin::Reflection || update.status != ThreadStatus::Active { continue; }
                // Rewording a concern does not give it another identity.
                if self.entries.iter().any(|thread| normalized(&thread.topic) == normalized(&topic)
                    || (!question.is_empty() && normalized(&thread.open_question) == normalized(&question))) { continue; }
                if self.retired.iter().any(|item| (item.key == key || normalized(&item.topic) == normalized(&topic))
                    && (!is_external || item.source_token == source_token)) { continue; }
                if self.entries.len() >= CAPACITY {
                    let Some(index) = self.entries.iter().enumerate().filter(|(_, thread)| thread.status != ThreadStatus::Active)
                        .min_by_key(|(_, thread)| thread.updated_at).map(|(index, _)| index) else { continue; };
                    let old = self.entries.remove(index);
                    self.retired.insert(0, RetiredThought { key: old.key, topic: old.topic, source_token: old.source_token, at: now });
                    self.retired.truncate(12);
                }
                self.sequence = self.sequence.saturating_add(1);
                self.entries.push(ThoughtThread { key, topic, personal_relevance: relevance, stance,
                    open_question: question, next_step: update.next_step, status: update.status,
                    created_at: now, updated_at: now, revision: self.sequence, awaiting_reply: false,
                    last_spoken_at: None, attempted_revision: 0, source_token, reflections: 0,
                    question_scope: update.question_scope, peer_fallback: update.peer_fallback && update.question_scope == QuestionScope::General,
                    peer_attempted_revision: 0, fallback_ready: false });
            }
            changed = true;
        }
        changed
    }

    pub fn snapshot(&self, now: i64, limit: usize) -> Vec<ThoughtThread> {
        let mut entries: Vec<_> = self.entries.iter().filter(|thread|
            thread.updated_at <= now && now - thread.updated_at <= ACTIVE_TTL).cloned().collect();
        entries.sort_by_key(|thread| (thread.status != ThreadStatus::Active, std::cmp::Reverse(thread.updated_at)));
        entries.truncate(limit.min(CAPACITY));
        entries
    }

    pub fn offers(&self, now: i64) -> Vec<super::conversation::ConversationEvent> {
        self.snapshot(now, CAPACITY).into_iter().filter(|thread| thread.speakable(now) || (thread.fallback_ready && thread.fallback_due(now))).take(3)
            .map(|thread| super::conversation::ConversationEvent { id: thread.event_id(),
                kind: "personal_thought".into(), at: thread.updated_at as f64,
                fact: serde_json::json!({"topic":thread.topic,"personal_relevance":thread.personal_relevance,
                    "stance":thread.stance,"open_question":thread.open_question,
                    "next_step":if thread.awaiting_reply { ThoughtStep::AskPeer } else { thread.next_step },
                    "question_scope":thread.question_scope,"fallback_from_unanswered_user":thread.awaiting_reply,
                    "subjective":true}).to_string() }).collect()
    }

    pub fn activate_peer_fallbacks(&mut self, now: i64) -> bool {
        let mut changed = false;
        for thread in &mut self.entries {
            if !thread.fallback_ready && thread.fallback_due(now) { thread.fallback_ready = true; changed = true; }
        }
        changed
    }

    pub fn prompt_snapshot(&self, now: i64, limit: usize) -> Vec<serde_json::Value> {
        self.snapshot(now, limit).into_iter().map(|thread| serde_json::json!({
            "key":thread.key,"topic":thread.topic,"personal_relevance":thread.personal_relevance,
            "stance":thread.stance,"open_question":thread.open_question,"next_step":thread.next_step,
            "status":thread.status,"awaiting_reply":thread.awaiting_reply,
            "question_scope":thread.question_scope,"peer_fallback":thread.peer_fallback,
        })).collect()
    }

    pub fn permits(&self, event_id: &str, intent: super::conversation::ConversationIntent, now: i64) -> bool {
        use super::conversation::ConversationIntent;
        self.entries.iter().any(|thread| thread.event_id() == event_id && (
            (thread.speakable(now) && matches!((thread.next_step, intent),
                (ThoughtStep::AskUser, ConversationIntent::AskUser) | (ThoughtStep::Share, ConversationIntent::Share)
                | (ThoughtStep::AskPeer, ConversationIntent::AskPeer) | (ThoughtStep::SharePeer, ConversationIntent::SharePeer)))
            || (intent == ConversationIntent::AskPeer && thread.fallback_ready && thread.fallback_due(now))))
    }

    pub fn attempt(&mut self, event_id: &str, intent: super::conversation::ConversationIntent) {
        if let Some(thread) = self.entries.iter_mut().find(|thread| thread.event_id() == event_id) {
            if intent.is_peer() { thread.peer_attempted_revision = thread.revision; }
            if !thread.awaiting_reply { thread.attempted_revision = thread.revision; }
        }
    }
    pub fn restore(&mut self, event_id: &str, intent: super::conversation::ConversationIntent) {
        if let Some(thread) = self.entries.iter_mut().find(|thread| thread.event_id() == event_id) {
            if intent.is_peer() { thread.peer_attempted_revision = thread.revision.saturating_sub(1); }
            if !thread.awaiting_reply { thread.attempted_revision = thread.revision.saturating_sub(1); }
        }
    }
    pub fn delivered(&mut self, event_id: &str, intent: super::conversation::ConversationIntent, now: i64) {
        if let Some(thread) = self.entries.iter_mut().find(|thread| thread.event_id() == event_id) {
            thread.attempted_revision = thread.revision;
            if !intent.is_peer() { thread.last_spoken_at = Some(now); }
            if !intent.is_peer() { thread.awaiting_reply = intent == super::conversation::ConversationIntent::AskUser; }
        }
    }
}

pub fn update_schema() -> serde_json::Value {
    serde_json::json!({"type":"array","maxItems":2,"items":{"type":"object","additionalProperties":false,
        "properties":{"key":{"type":"string"},"topic":{"type":"string"},"personal_relevance":{"type":"string"},
            "stance":{"type":"string"},"open_question":{"type":"string"},
            "next_step":{"type":"string","enum":["reflect","ask_user","ask_peer","share","share_peer","rest"]},
            "question_scope":{"type":"string","enum":["general","user_only"]},"peer_fallback":{"type":"boolean"},
            "status":{"type":"string","enum":["active","resolved","parked"]},
            "origin":{"type":"string","enum":["persona","user_message","world_observation","scene_event","reflection"]},
            "evidence":{"type":"string"}},
        "required":["key","topic","personal_relevance","stance","open_question","next_step","status","origin","evidence","question_scope","peer_fallback"]}})
}
