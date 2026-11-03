//! Dedicated, private persona-evolution agent. It proposes a bounded review;
//! only the host may validate evidence and publish persona/corpus changes.
use super::example_library::ExampleEdit;
use super::evolution::{MAX_PROPOSALS, MAX_UNDERSTANDING_CHARS, MAX_REASON_CHARS, SCOPES};
use crate::{
    error::{VivianError, VivianResult},
    providers::{base::LLMRequest, ModelRouter},
    types::response::ChatMessage,
};
use serde::{Deserialize, Serialize};

pub const TASK_ROUTE: &str = "persona_evolution";
pub const SYSTEM_PROMPT: &str = include_str!("../../prompts/agents/persona_evolution.md");

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvolutionProposal {
    pub scope: String,
    pub kind: String,
    pub understanding: String,
    #[serde(default)]
    pub reason: String,
    pub source_quote: String,
    #[serde(default)]
    pub revises: Option<String>,
    #[serde(default)]
    pub examples: Vec<ExampleEdit>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvolutionReview {
    pub proposals: Vec<EvolutionProposal>,
    pub example_updates: Vec<ExampleEdit>,
}

pub struct PersonaEvolutionAgent<'a> {
    router: &'a ModelRouter,
    character_id: &'a str,
}

impl<'a> PersonaEvolutionAgent<'a> {
    pub fn new(router: &'a ModelRouter, character_id: &'a str) -> Self {
        Self {
            router,
            character_id,
        }
    }

    pub fn messages(context: String) -> Vec<ChatMessage> {
        vec![
            ChatMessage::system(SYSTEM_PROMPT),
            ChatMessage::user(context),
        ]
    }

    pub async fn review(&self, context: String) -> VivianResult<EvolutionReview> {
        let response = self
            .router
            .generate(
                LLMRequest::new(TASK_ROUTE, Self::messages(context))
                    .without_framework_instructions()
                    .with_json_schema(review_schema())
                    .with_character_id(self.character_id.to_owned()),
            )
            .await
            .map_err(|e| VivianError::Other(format!("人设进化智能体调用失败: {e}")))?;
        parse_review(&response)
    }
}

fn review_schema() -> serde_json::Value {
    use serde_json::json;
    let edit = json!({"type":"object", "additionalProperties":false, "properties":{
        "id":{"type":"string"},"reference":{"type":["string","null"]},"reason":{"type":"string","maxLength":MAX_REASON_CHARS},
        "example":{"type":"null"}
    },"required":["id","reference","reason","example"]});
    let proposal = json!({"type":"object", "additionalProperties":false, "properties":{
        "scope":{"type":"string","enum":SCOPES},
        "kind":{"type":"string","enum":["personality","tone"]},
        "understanding":{"type":"string","minLength":1,"maxLength":MAX_UNDERSTANDING_CHARS},
        "reason":{"type":"string","maxLength":MAX_REASON_CHARS},"source_quote":{"type":"string","minLength":6,"maxLength":500},
        "revises":{"type":["string","null"]},"examples":{"type":"array","maxItems":0,"items":edit}
    },"required":["scope","kind","understanding","reason","source_quote","revises","examples"]});
    json!({"type":"object", "additionalProperties":false, "properties":{
        "proposals":{"type":"array","maxItems":MAX_PROPOSALS,"items":proposal},
        "example_updates":{"type":"array","maxItems":super::example_library::MAX_EDITS,"items":edit}
    },"required":["proposals","example_updates"]})
}

pub(crate) fn parse_review(raw: &str) -> VivianResult<EvolutionReview> {
    let raw = raw.trim();
    let text = if raw.starts_with("```") {
        raw.split_once('\n')
            .map(|(_, body)| body.trim_end().strip_suffix("```").unwrap_or(body).trim())
            .unwrap_or(raw)
    } else {
        raw
    };
    let review: EvolutionReview = serde_json::from_str(text)
        .map_err(|e| VivianError::Other(format!("人设进化结果格式不完整，未应用修订: {e}")))?;
    // Providers and fallback routes may not enforce JSON Schema. Budgets and
    // domains are host invariants, so reject the entire review before publishing.
    let mut scopes = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    if review.proposals.len() > MAX_PROPOSALS
        || review.example_updates.len() > super::example_library::MAX_EDITS
        || review.proposals.iter().any(|p| !SCOPES.contains(&p.scope.as_str())
            || !matches!(p.kind.as_str(), "personality" | "tone")
            || p.understanding.trim().is_empty()
            || p.understanding.chars().count() > MAX_UNDERSTANDING_CHARS
            || p.reason.chars().count() > MAX_REASON_CHARS
            || !(6..=500).contains(&p.source_quote.trim().chars().count())
            || !scopes.insert(&p.scope))
        || review.example_updates.iter().any(|e| e.reason.chars().count() > MAX_REASON_CHARS
            || !ids.insert(&e.id)) {
        return Err(VivianError::Other("人设进化提案超出修订预算或包含无效字段，未应用本次修订".into()));
    }
    if review.proposals.iter().any(|proposal| !proposal.examples.is_empty())
        || review.example_updates.iter().any(|edit| edit.example.is_some()) {
        return Err(VivianError::Other("语料只使用数据集原句；日记自省不能新增或改写台词".into()));
    }
    Ok(review)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_output_cannot_bypass_host_edit_budgets_or_protocol() {
        use serde_json::json;
        let proposal = json!({"scope":"comfort","kind":"tone","understanding":"先听完再回应",
            "reason":"真实反馈","source_quote":"今天请先听我说完","revises":null,"examples":[]});
        let valid = json!({"proposals":[proposal.clone()],"example_updates":[]});
        assert!(parse_review(&valid.to_string()).is_ok());
        assert!(parse_review(&format!("```json\n{valid}\n```")).is_ok());
        let mut other_scope = proposal.clone(); other_scope["scope"] = json!("care");
        assert!(parse_review(&json!({"proposals":[proposal.clone(),other_scope],"example_updates":[]}).to_string()).is_ok());
        for (key, value) in [("scope", json!("global")), ("kind", json!("identity")),
            ("understanding", json!("字".repeat(MAX_UNDERSTANDING_CHARS + 1))),
            ("reason", json!("字".repeat(MAX_REASON_CHARS + 1))), ("source_quote", json!("短"))] {
            let mut invalid = valid.clone(); invalid["proposals"][0][key] = value;
            assert!(parse_review(&invalid.to_string()).is_err(), "{key}");
        }
        assert!(parse_review(&json!({"proposals":[proposal.clone(),proposal.clone(),proposal.clone()],"example_updates":[]}).to_string()).is_err());
        assert!(parse_review(&json!({"proposals":[proposal.clone(),proposal],"example_updates":[]}).to_string()).is_err());
        let edits: Vec<_> = (0..5).map(|i| json!({"id":format!("voice-{i}"),"reference":"current","reason":"不适用","example":null})).collect();
        assert!(parse_review(&json!({"proposals":[],"example_updates":&edits[..4]}).to_string()).is_ok());
        assert!(parse_review(&json!({"proposals":[],"example_updates":[edits[0].clone(),edits[0].clone()]}).to_string()).is_err());
        assert!(parse_review(&json!({"proposals":[],"example_updates":edits}).to_string()).is_err());
        assert!(parse_review(r#"{"proposals":[],"example_updates":[],"rewrite":"new identity"}"#).is_err());
        let mut unknown = valid; unknown["proposals"][0]["explicit_feedback"] = json!(true);
        assert!(parse_review(&unknown.to_string()).is_err());
        assert!(parse_review("{}").is_err());
        assert!(parse_review(r#"{"proposals":[],"example_updates":[]}"#).is_ok());
    }
    #[test]
    fn corpus_review_can_retire_quotes_but_cannot_write_new_ones() {
        assert!(parse_review(r#"{"proposals":[],"example_updates":[{"id":"voice-existing","reference":"current","reason":"不适用","example":null}]}"#).is_ok());
        let authored = serde_json::json!({"proposals":[],"example_updates":[{
            "id":"grown-new","reference":null,"reason":"创作新台词","example":{
                "id":"grown-new","scope":"daily","cues":["你好"],"context_cues":[],
                "situation":"虚构场景","attention":"测试","user":"你好",
                "response":{"text":"自己编的新台词","intent":"reply"}
            }}]});
        assert!(parse_review(&authored.to_string()).is_err());
        assert_eq!(review_schema()["properties"]["proposals"]["items"]["properties"]["examples"]["maxItems"],0);
        assert_eq!(review_schema()["properties"]["example_updates"]["items"]["properties"]["example"]["type"],"null");
    }
}
