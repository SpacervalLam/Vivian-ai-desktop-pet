//! Version checks permit appended evidence but reject changed, consumed or repeated sources.
use serde::Serialize;
pub fn prefix_current<T: Serialize>(live: &[T], source: &[T]) -> bool {
    !source.is_empty()
        && live.len() >= source.len()
        && serde_json::to_value(&live[..source.len()])
            .ok()
            .zip(serde_json::to_value(source).ok())
            .is_some_and(|(a, b)| a == b)
}
pub fn sources_current<T: Serialize>(live: &[T], source: &[T]) -> bool {
    if source.is_empty() {
        return false;
    }
    let (Ok(live), Ok(source)) = (serde_json::to_value(live), serde_json::to_value(source)) else {
        return false;
    };
    let (Some(live), Some(source)) = (live.as_array(), source.as_array()) else {
        return false;
    };
    let mut ids = std::collections::HashSet::new();
    source.iter().all(|s| {
        s["id"].as_str().is_some_and(|id| {
            ids.insert(id)
                && live.iter().filter(|v| v["id"] == s["id"]).count() == 1
                && live.iter().any(|v| v == s)
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn appended_records_survive_and_stale_or_duplicate_sources_are_rejected() {
        let sources = vec![
            json!({"id":"a","summary":"old"}),
            json!({"id":"b","summary":"old b"}),
        ];
        let mut live = sources.clone();
        live.push(json!({"id":"new","summary":"new during LLM"}));
        assert!(sources_current(&live, &sources));
        assert!(prefix_current(&live, &sources));
        live[0]["summary"] = json!("edited");
        assert!(!sources_current(&live, &sources));
        assert!(!prefix_current(&live, &sources));
        live.remove(0);
        assert!(!sources_current(&live, &sources));
        assert!(!sources_current(
            &sources,
            &[sources[0].clone(), sources[0].clone()]
        ));
    }
}
