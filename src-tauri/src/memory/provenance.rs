//! Source attribution shared by recall, tools and conversation evidence.
use serde_json::Value;

pub fn field<'a>(meta: &'a Value, key: &str) -> Option<&'a str> {
    meta.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// Explicit knowledge boundaries cannot be relaxed by retrieval relevance.
pub fn visible_to(meta: &Value, character: &str) -> bool {
    if let Some(known) = meta.get("known_by").and_then(Value::as_array) {
        return known.iter().any(|v| v.as_str() == Some(character));
    }
    let speaker = field(meta, "speaker");
    let listener = field(meta, "listener");
    let observer = field(meta, "observer_id");
    if speaker == Some(character) || listener == Some(character) || observer == Some(character) {
        return true;
    }

    // Old per-character records without attribution remain usable, but never gain invented participants.
    if speaker.is_none() && listener.is_none() && observer.is_none() {
        return field(meta, "memory_owner").is_none_or(|owner| owner == character);
    }
    // Legacy broadcast records lack an audience list and live in the receiving character's own store.
    listener == Some("all") && field(meta, "knowledge_source") == Some("broadcast")
}

pub fn recall_label(meta: &Value) -> &'static str {
    if field(meta, "perspective") == Some("observer")
        || field(meta, "knowledge_source") == Some("observed")
    {
        return "旁观记录";
    }
    match field(meta, "record_kind") {
        Some("session_summary") => "对话摘要；非逐字原话",
        Some("subjective") => "角色想法；非已确认事实",
        Some("reference") => "参考资料；非用户经历",
        Some("fact")
            if field(meta, "subject") == Some("user")
                && field(meta, "evidence_kind") == Some("quoted") =>
        {
            "用户自述的提炼；非逐字原话"
        }
        Some("fact") if field(meta, "evidence_kind") == Some("inferred") => "推测；需核实",
        Some("fact") => "记忆结论；以原始证据为准",
        Some("dialogue") => "历史发言",
        _ => "历史资料；来源未完整记录",
    }
}

pub fn attribution(meta: &Value) -> String {
    let mut parts = Vec::new();
    if field(meta, "speaker").is_some() || field(meta, "listener").is_some() {
        parts.push(format!(
            "{} → {}",
            field(meta, "speaker").unwrap_or("说话者未记录"),
            match field(meta, "listener") {
                Some("all") => "当众发言",
                Some(v) => v,
                None => "接收者未记录",
            }
        ));
    }
    if let Some(observer) = field(meta, "observer_id") {
        parts.push(format!("{observer} 旁观得知"));
    }
    if let Some(people) = meta.get("participants").and_then(Value::as_array) {
        let names: Vec<_> = people.iter().filter_map(Value::as_str).collect();
        if !names.is_empty() {
            parts.push(format!("涉及：{}", names.join("、")));
        }
    }
    if field(meta, "evidence_kind") == Some("inferred") {
        parts.push("系统推测".into());
    }
    if let Some(sources) = meta
        .get("evidence_sources")
        .and_then(Value::as_array)
        .filter(|s| s.len() > 1)
    {
        parts.push(format!("含 {} 份原话证据；以上为当前来源", sources.len()));
    }
    parts.join("；")
}

/// Same text by different speakers or of different kinds is not the same evidence.
pub fn recall_key(meta: &Value, plain: &str) -> String {
    if field(meta, "record_kind") == Some("dialogue") {
        if let Some(id) = field(meta, "utterance_id") {
            return format!("utterance:{id}");
        }
        if let Some(ids) = meta
            .get("source_message_ids")
            .and_then(Value::as_array)
            .filter(|v| v.len() == 1)
        {
            if let Some(id) = ids[0].as_str() {
                return format!("utterance:{id}");
            }
        }
    }
    // Preserve punctuation: removing it collapses distinct utterances and code snippets.
    serde_json::json!([
        field(meta, "record_kind"),
        field(meta, "speaker"),
        field(meta, "listener"),
        field(meta, "subject"),
        field(meta, "perspective"),
        field(meta, "observer_id"),
        plain.trim()
    ])
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn direct_is_not_a_global_visibility_grant() {
        let m = json!({"speaker":"user","listener":"nana","knowledge_source":"direct"});
        assert!(visible_to(&m, "nana"));
        assert!(!visible_to(&m, "vivian"));
        let wrongly_copied = json!({"speaker":"user","listener":"nana","knowledge_source":"direct","memory_owner":"vivian"});
        assert!(!visible_to(&wrongly_copied, "vivian"));
    }
    #[test]
    fn observers_and_explicit_audiences_are_respected() {
        let m = json!({"speaker":"user","listener":"nana","observer_id":"vivian","knowledge_source":"observed"});
        assert!(visible_to(&m, "vivian"));
        assert!(!visible_to(&m, "third"));
        assert!(!visible_to(
            &json!({"known_by":["nana"],"listener":"all","knowledge_source":"broadcast"}),
            "vivian"
        ));
    }
    #[test]
    fn legacy_data_does_not_invent_speakers() {
        assert!(visible_to(&json!({}), "vivian"));
        assert_eq!(attribution(&json!({})), "");
    }
    #[test]
    fn identical_words_by_different_people_stay_distinct() {
        assert_ne!(
            recall_key(&json!({"speaker":"nana"}), "你好"),
            recall_key(&json!({"speaker":"vivian"}), "你好")
        );
        assert_ne!(
            recall_key(&json!({"record_kind":"fact"}), "不吃辣"),
            recall_key(&json!({"record_kind":"session_summary"}), "不吃辣")
        );
    }
    #[test]
    fn shared_event_id_deduplicates_copies() {
        assert_eq!(
            recall_key(
                &json!({"record_kind":"dialogue","utterance_id":"one"}),
                "你好"
            ),
            recall_key(
                &json!({"record_kind":"dialogue","source_message_ids":["one"]}),
                "你好"
            )
        );
    }
    #[test]
    fn recall_distinguishes_summary_and_observation() {
        assert!(recall_label(&json!({"record_kind":"session_summary"})).contains("非逐字"));
        assert_eq!(
            recall_label(&json!({"knowledge_source":"observed"})),
            "旁观记录"
        );
    }
}
