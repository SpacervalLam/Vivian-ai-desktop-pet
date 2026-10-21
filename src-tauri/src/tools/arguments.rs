//! Conservative argument-name repair: exact keys win; ambiguous guesses stay untouched.
use serde_json::Value;
use std::collections::HashMap;

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn normalize_tool_arguments(schema: &Value, arguments: Value) -> Value {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return arguments;
    };
    let Some(original) = arguments.as_object() else {
        return arguments;
    };
    let mut proposals: HashMap<&str, Vec<&str>> = HashMap::new();
    for key in original.keys().filter(|key| !properties.contains_key(*key)) {
        let normalized = normalize_key(key);
        if normalized.is_empty() {
            continue;
        }
        let mut candidates: Vec<_> = properties
            .keys()
            .filter(|canonical| normalize_key(canonical) == normalized)
            .collect();
        if candidates.is_empty() {
            candidates = properties
                .keys()
                .filter(|canonical| {
                    let other = normalize_key(canonical);
                    (normalized.len() >= 3 && other.starts_with(&normalized))
                        || (other.len() >= 3 && normalized.starts_with(&other))
                })
                .collect();
        }
        if candidates.len() == 1 && !original.contains_key(candidates[0]) {
            proposals.entry(candidates[0]).or_default().push(key);
        }
    }
    let mut repaired = original.clone();
    for (canonical, sources) in proposals {
        if sources.len() == 1 {
            if let Some(value) = repaired.remove(sources[0]) {
                repaired.insert(canonical.to_string(), value);
            }
        }
    }
    Value::Object(repaired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_keys_win_over_earlier_alias_keys() {
        let schema = json!({"properties":{"output_path":{}}});
        let input = json!({"OUTPUT-PATH":"wrong", "output_path":"correct"});
        assert_eq!(normalize_tool_arguments(&schema, input.clone()), input);
    }
    #[test]
    fn unique_repairs_preserve_types_and_unknown_fields() {
        let schema = json!({"properties":{"output_path":{},"timeout_secs":{}}});
        assert_eq!(
            normalize_tool_arguments(&schema, json!({"outputPath":"x","timeout":3,"unknown":[1]})),
            json!({"output_path":"x","timeout_secs":3,"unknown":[1]})
        );
    }
    #[test]
    fn ambiguous_targets_and_duplicate_aliases_are_not_guessed() {
        for (schema, input) in [
            (
                json!({"properties":{"file_path":{},"file_pattern":{}}}),
                json!({"file":"x"}),
            ),
            (
                json!({"properties":{"output_path":{}}}),
                json!({"outputPath":"x","OUTPUT-PATH":"y"}),
            ),
            (json!({"properties":{"a_b":{},"ab":{}}}), json!({"A-B":1})),
        ] {
            assert_eq!(normalize_tool_arguments(&schema, input.clone()), input);
        }
    }
}
