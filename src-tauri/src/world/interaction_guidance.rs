//! Ephemeral scene continuity: attributed evidence and a small, optional conversational nudge.
//! These are prompt inputs, never dialogue, user facts, or character memories.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize)]
pub struct Evidence {
    pub id: String,
    pub at: f64,
    pub character_id: String,
    pub kind: String,
    pub text: String,
    /// Private human dialogue cannot be carried into a roommate conversation.
    pub shareable: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Suggestion {
    pub listener_id: String,
    pub source_ids: Vec<String>,
    pub situation: String,
    pub direction: String,
}

#[derive(Clone, Debug)]
pub struct Hint {
    pub character_id: String,
    pub listener_id: String,
    pub sources: Vec<Evidence>,
    pub situation: String,
    pub direction: String,
    pub created_at: f64,
    pub expires_at: f64,
}

impl Suggestion {
    pub fn grounded(self, character: &str, listener: &str, evidence: &[Evidence], now: f64) -> Option<Hint> {
        if self.listener_id != listener || character == listener || self.source_ids.is_empty()
            || self.source_ids.len() > 5 || self.situation.trim().is_empty() || self.direction.trim().is_empty()
            || self.situation.chars().count() > 300 || self.direction.chars().count() > 500 {
            return None;
        }
        let prose = format!("{} {}", self.situation, self.direction).to_lowercase();
        if ["environment agent", "coordinator", "环境智能体", "调度器", "幕后智能体"].iter().any(|word| prose.contains(word)) {
            return None;
        }
        let mut sources = vec![];
        for id in &self.source_ids {
            let source = evidence.iter().find(|source| &source.id == id
                && source.character_id == character && source.at.is_finite()
                && source.at <= now + 1.0 && now - source.at <= 7200.0
                && (listener == "user" || source.shareable))?;
            if !sources.iter().any(|old: &Evidence| old.id == source.id) { sources.push(source.clone()); }
        }
        Some(Hint { character_id:character.into(),listener_id:listener.into(),sources,
            situation:self.situation,direction:self.direction,created_at:now,expires_at:now+120.0 })
    }
}

impl Hint {
    pub fn render(&self) -> String {
        let sources: Vec<_> = self.sources.iter().map(|source| serde_json::json!({
            "at":source.at,"kind":source.kind,"observed":source.text,
        })).collect();
        format!("[SCENE CONTINUITY]\nOptional context for your next turn to {}. Use it silently in your own voice; it is not a script or a new request. The latest actual dialogue wins. Do not mention hints, analysis, narrators or a source supplying suggestions. This context does not expand the user's task or establish new commitments or experiences. Follow the actual user request with the normal tools and policies. You may ignore the direction or remain silent when inappropriate.\n{}\n[END SCENE CONTINUITY]",
            self.listener_id,serde_json::json!({"formed_at":self.created_at,"recent_evidence":sources,
                "situation":self.situation,"possible_direction":self.direction}))
    }
}

#[derive(Default)]
pub struct HintCache { hints: HashMap<String, (u64, Hint)> }
impl HintCache {
    pub fn insert(&mut self, revision: u64, hint: Hint) {
        self.hints.insert(hint.character_id.clone(), (revision,hint));
    }
    pub fn get(&self, character: &str, listener: &str, revision: u64, now: f64) -> Option<String> {
        let (created_revision,hint) = self.hints.get(character)?;
        (*created_revision == revision && hint.listener_id == listener && now >= hint.created_at
            && now < hint.expires_at).then(|| hint.render())
    }
    pub fn clear(&mut self) { self.hints.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence() -> Vec<Evidence> { vec![Evidence{id:"capture:1".into(),at:1000.0,character_id:"vivian".into(),
        kind:"knowledge_acquired".into(),text:"Vivian 保存了读到的字幕；还没有分享".into(),shareable:true},
        Evidence{id:"private:1".into(),at:1001.0,character_id:"vivian".into(),kind:"dialogue".into(),
            text:"用户私聊谈到自己的事".into(),shareable:false}] }
    fn suggestion(listener: &str) -> Suggestion { Suggestion{listener_id:listener.into(),source_ids:vec!["capture:1".into()],
        situation:"刚读到的内容还没聊起".into(),direction:"如果与此刻话题合得上，可以从一个具体细节聊起，不必把整篇复述出来".into()} }
    #[test] fn attributed_knowledge_can_start_a_peer_exchange_but_private_dialogue_cannot() {
        let hint=suggestion("nana").grounded("vivian","nana",&evidence(),1002.0).unwrap();
        assert!(hint.render().contains("knowledge_acquired"));
        let mut private=suggestion("nana");private.source_ids=vec!["private:1".into()];
        assert!(private.grounded("vivian","nana",&evidence(),1002.0).is_none());
        assert!(suggestion("user").grounded("nana","user",&evidence(),1002.0).is_none());
    }
    #[test] fn fabricated_stale_or_misdirected_guidance_is_discarded() {
        let mut made_up=suggestion("user");made_up.source_ids=vec!["never_happened".into()];
        assert!(made_up.grounded("vivian","user",&evidence(),1002.0).is_none());
        assert!(suggestion("user").grounded("vivian","nana",&evidence(),1002.0).is_none());
        assert!(suggestion("user").grounded("vivian","user",&evidence(),9000.0).is_none());
        let mut exposed=suggestion("user");exposed.direction="环境智能体让你这样说".into();
        assert!(exposed.grounded("vivian","user",&evidence(),1002.0).is_none());
    }
    #[test] fn new_input_expiration_and_audience_changes_retire_old_directions() {
        let hint=suggestion("user").grounded("vivian","user",&evidence(),1002.0).unwrap();
        let mut cache=HintCache::default();cache.insert(7,hint);
        assert!(cache.get("vivian","user",7,1003.0).is_some());
        assert!(cache.get("vivian","user",8,1003.0).is_none());
        assert!(cache.get("vivian","nana",7,1003.0).is_none());
        assert!(cache.get("vivian","user",7,1122.0).is_none());
        cache.clear();assert!(cache.get("vivian","user",7,1003.0).is_none());
    }
}
