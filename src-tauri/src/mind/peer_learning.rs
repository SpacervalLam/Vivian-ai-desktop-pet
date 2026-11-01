//! Attributed learning from actual received utterances, never mutual verification.
use serde::{Deserialize, Serialize};

const MAX_STATEMENTS: usize = 8;
const MAX_NOTES: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatement {
    pub id: String,
    pub speaker: String,
    pub text: String,
    pub at: i64,
    pub concern_event_id: Option<String>,
}

impl PeerStatement {
    pub fn event(&self) -> super::conversation::ConversationEvent {
        let (id, kind) = match &self.concern_event_id {
            Some(concern) => (format!("peer_answer:{concern}:{}", self.id), "peer_answer"),
            None => (format!("peer_statement:{}", self.id), "peer_statement"),
        };
        super::conversation::ConversationEvent {
            id,
            kind: kind.into(),
            at: self.at as f64,
            fact: format!(
                "{} said (unverified peer report; source={}): {}",
                self.speaker, self.id, self.text
            ),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningUpdate {
    pub key: String,
    pub source_id: String,
    pub quote: String,
    pub understanding: String,
}

pub fn parse_updates<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<LearningUpdate>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .take(2)
        .filter_map(|value| serde_json::from_value(value.clone()).ok())
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedNote {
    pub key: String,
    pub understanding: String,
    pub source_id: String,
    pub speaker: String,
    pub quote: String,
    pub observed_at: i64,
    pub learned_at: i64,
    pub verified: bool,
}

impl LearnedNote {
    pub fn memory_record(&self, owner: &str) -> (String, serde_json::Value) {
        let text = format!("从{}的发言中暂时学到：{}\n来源原话：{}\n此为同伴观点或待核实知识，不代表用户确认，也不等同于独立验证。",
            self.speaker, self.understanding, self.quote);
        let metadata = serde_json::json!({"channel":"cross_character","content_type":"peer_learning",
            "record_kind":"reference","subject":"general","evidence_kind":"peer_report","known_by":[owner],
            "speaker":self.speaker,"listener":owner,"perspective":"listener",
            "knowledge_source":"peer_report","verified":false,"source_utterance_id":self.source_id,
            "source_quote":self.quote,"observed_at":self.observed_at,"learned_at":self.learned_at,"semantic_type":"general"});
        (text, metadata)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerLearning {
    statements: Vec<PeerStatement>,
    notes: Vec<LearnedNote>,
}

impl PeerLearning {
    /// An utterance identity may be delivered again; it is still one observation.
    pub fn receive(
        &mut self,
        owner: &str,
        speaker: &str,
        id: &str,
        text: &str,
        concern: Option<String>,
        now: i64,
    ) -> Option<PeerStatement> {
        if owner == speaker
            || speaker == "user"
            || speaker.trim().is_empty()
            || id.trim().is_empty()
            || text.trim().is_empty()
            || self.statements.iter().any(|statement| statement.id == id)
            || self.notes.iter().any(|note| note.source_id == id)
        {
            return None;
        }
        let statement = PeerStatement {
            id: id.chars().take(160).collect(),
            speaker: speaker.into(),
            text: text.chars().take(2400).collect(),
            at: now,
            concern_event_id: concern,
        };
        self.statements.push(statement.clone());
        if self.statements.len() > MAX_STATEMENTS {
            self.statements.remove(0);
        }
        Some(statement)
    }

    /// Exact source IDs and quotes establish attribution, not factual truth.
    pub fn learn(&mut self, updates: &[LearningUpdate], now: i64) -> Vec<LearnedNote> {
        let mut learned = Vec::new();
        for update in updates.iter().take(2) {
            let key = update.key.trim();
            let quote = update.quote.trim();
            let understanding = update.understanding.trim();
            if key.is_empty()
                || key.len() > 64
                || !key.chars().all(|c| c.is_alphanumeric() || c == '_')
                || quote.chars().count() < 4
                || quote.chars().count() > 600
                || understanding.is_empty()
                || understanding.chars().count() > 600
            {
                continue;
            }
            let Some(source) = self.statements.iter().find(|source| {
                source.id == update.source_id && source.at <= now && source.text.contains(quote)
            }) else {
                continue;
            };
            if self.notes.iter().any(|note| {
                (note.key == key && note.source_id == source.id)
                    || note.understanding == understanding
            }) {
                continue;
            }
            let note = LearnedNote {
                key: key.into(),
                understanding: understanding.into(),
                source_id: source.id.clone(),
                speaker: source.speaker.clone(),
                quote: quote.into(),
                observed_at: source.at,
                learned_at: now,
                verified: false,
            };
            self.notes.push(note.clone());
            learned.push(note);
        }
        if self.notes.len() > MAX_NOTES {
            self.notes.drain(..self.notes.len() - MAX_NOTES);
        }
        learned
    }

    pub fn context(&self) -> serde_json::Value {
        serde_json::json!({"received_peer_statements":self.statements,
            "learned_from_peers":self.notes.iter().rev().take(6).collect::<Vec<_>>(),
            "attribution":"Peer reports and interpretations are unverified; agreement is not independent evidence or a user answer."})
    }

    pub fn prompt_notes(&self) -> Option<String> {
        (!self.notes.is_empty()).then(|| {
            format!(
                "[Learned from peers — attributed, unverified, not user profile facts]\n{}",
                serde_json::to_string(&self.notes.iter().rev().take(6).collect::<Vec<_>>())
                    .unwrap_or_default()
            )
        })
    }
}

pub fn update_schema() -> serde_json::Value {
    serde_json::json!({"type":"array","maxItems":2,"items":{"type":"object","additionalProperties":false,
        "properties":{"key":{"type":"string"},"source_id":{"type":"string"},"quote":{"type":"string"},"understanding":{"type":"string"}},
        "required":["key","source_id","quote","understanding"]}})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn update(id: &str, quote: &str) -> LearningUpdate {
        LearningUpdate {
            key: "sampling".into(),
            source_id: id.into(),
            quote: quote.into(),
            understanding: "先观察再选择，可能更容易发现偏差".into(),
        }
    }
    #[test]
    fn only_received_peer_quotes_can_ground_learning_and_echoes_never_verify_it() {
        let mut book = PeerLearning::default();
        assert!(book
            .receive("vivian", "vivian", "self", "先观察再选择", None, 10)
            .is_none());
        assert!(book
            .receive("vivian", "user", "forged", "先观察再选择", None, 10)
            .is_none());
        book.receive(
            "vivian",
            "nana",
            "actual",
            "我倾向于先观察再选择，不过也不确定。",
            None,
            10,
        )
        .unwrap();
        assert!(book
            .learn(&[update("invented", "先观察再选择")], 11)
            .is_empty());
        assert!(book
            .learn(&[update("actual", "你肯定已经验证了")], 11)
            .is_empty());
        let learned = book.learn(&[update("actual", "先观察再选择")], 11);
        assert_eq!(learned.len(), 1);
        assert!(!learned[0].verified);
        assert_eq!(learned[0].speaker, "nana");
        let (text, metadata) = learned[0].memory_record("vivian");
        assert!(text.contains("不代表用户确认"));
        assert_eq!(metadata["listener"], "vivian");
        assert_eq!(metadata["source_utterance_id"], "actual");
        assert_eq!(metadata["verified"], false);
        assert!(book
            .learn(&[update("actual", "先观察再选择")], 12)
            .is_empty());
        assert!(book
            .receive("vivian", "nana", "actual", "先观察再选择", None, 12)
            .is_none());
        let mut restored: PeerLearning =
            serde_json::from_str(&serde_json::to_string(&book).unwrap()).unwrap();
        assert!(restored
            .learn(&[update("actual", "先观察再选择")], 13)
            .is_empty());
        assert!(!restored.notes[0].verified);
    }
    #[test]
    fn answers_keep_the_original_concern_reference_and_empty_feedback_is_not_speech() {
        let mut book = PeerLearning::default();
        assert!(book
            .receive("nana", "vivian", "empty", "  ", None, 10)
            .is_none());
        let statement = book
            .receive(
                "nana",
                "vivian",
                "answer",
                "我觉得有几个不同的因素",
                Some("thought:choice:7".into()),
                10,
            )
            .unwrap();
        assert_eq!(statement.event().id, "peer_answer:thought:choice:7:answer");
        assert_eq!(statement.event().kind, "peer_answer");
        assert!(statement.event().fact.contains("我觉得有几个不同的因素"));
    }
}
