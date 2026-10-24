//! Dedicated, private persona-evolution agent. It proposes a bounded review;
//! only the host may validate evidence and publish persona/corpus changes.
use super::example_library::ExampleEdit;
use crate::{
    error::{VivianError, VivianResult},
    providers::{base::LLMRequest, ModelRouter},
    types::response::ChatMessage,
};
use serde::{Deserialize, Serialize};

pub const TASK_ROUTE: &str = "persona_evolution";
pub const SYSTEM_PROMPT: &str = include_str!("../../prompts/agents/persona_evolution.md");

#[derive(Debug, Clone, Deserialize, Serialize)]
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
pub struct EvolutionReview {
    #[serde(default)]
    pub proposals: Vec<EvolutionProposal>,
    #[serde(default)]
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
    let response = json!({"type":"object", "additionalProperties":false, "properties":{
        "text":{"type":"string"}, "intent":{"type":"string","enum":["reply","short_reply","no_reply"]}
    },"required":["text","intent"]});
    let example = json!({"type":"object", "additionalProperties":false, "properties":{
        "id":{"type":"string"},"scope":{"type":"string"},
        "cues":{"type":"array","minItems":1,"maxItems":8,"items":{"type":"string"}},
        "context_cues":{"type":"array","maxItems":8,"items":{"type":"string"}},
        "situation":{"type":"string"},"attention":{"type":"string"},"user":{"type":"string"},"response":response
    },"required":["id","scope","cues","context_cues","situation","attention","user","response"]});
    let edit = json!({"type":"object", "additionalProperties":false, "properties":{
        "id":{"type":"string"},"reference":{"type":["string","null"]},"reason":{"type":"string"},
        "example":{"anyOf":[example,{"type":"null"}]}
    },"required":["id","reference","reason","example"]});
    let proposal = json!({"type":"object", "additionalProperties":false, "properties":{
        "scope":{"type":"string","enum":super::evolution::SCOPES},
        "kind":{"type":"string","enum":["personality","tone"]},
        "understanding":{"type":"string"},"reason":{"type":"string"},"source_quote":{"type":"string"},
        "revises":{"type":["string","null"]},"examples":{"type":"array","maxItems":4,"items":edit}
    },"required":["scope","kind","understanding","reason","source_quote","revises","examples"]});
    json!({"type":"object", "additionalProperties":false, "properties":{
        "proposals":{"type":"array","maxItems":2,"items":proposal},
        "example_updates":{"type":"array","maxItems":4,"items":edit}
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
    serde_json::from_str(text)
        .map_err(|e| VivianError::Other(format!("人设进化结果格式不完整，未应用修订: {e}")))
}
