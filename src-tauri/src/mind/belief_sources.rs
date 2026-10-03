//! Beliefs learn from stored experiences, not persona seed data.
use serde_json::Value;

pub fn matches(memory_type: &str, metadata: &Value, tags: &[String], wanted: &str) -> bool {
    if metadata.get("seed_chunk").and_then(Value::as_bool) == Some(true)
        || metadata.get("source").and_then(Value::as_str) == Some("system_seed")
        || tags.iter().any(|tag| tag == "backstory")
    {
        return false;
    }
    // Prefer the canonical type; fall back only for legacy records without it.
    if !memory_type.is_empty() {
        return memory_type == wanted;
    }
    if let Some(kind) = metadata
        .get("memory_type")
        .and_then(Value::as_str)
        .filter(|kind| !kind.is_empty())
    {
        return kind == wanted;
    }
    tags.iter().any(|tag| tag == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn canonical_and_legacy_experiences_are_eligible() {
        assert!(matches("long_term", &json!({}), &[], "long_term"));
        assert!(matches(
            "",
            &json!({"memory_type":"insight"}),
            &[],
            "insight"
        ));
        assert!(matches("", &json!({}), &["long_term".into()], "long_term"));
        assert!(!matches(
            "observation_note",
            &json!({}),
            &["long_term".into()],
            "long_term"
        ));
    }
    #[test]
    fn persona_seeds_are_not_learned_experience() {
        assert!(!matches(
            "long_term",
            &json!({"source":"system_seed"}),
            &[],
            "long_term"
        ));
        assert!(!matches(
            "long_term",
            &json!({"seed_chunk":true}),
            &[],
            "long_term"
        ));
        assert!(!matches(
            "long_term",
            &json!({}),
            &["backstory".into()],
            "long_term"
        ));
    }
}
