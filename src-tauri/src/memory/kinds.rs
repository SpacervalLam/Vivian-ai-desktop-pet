//! 内容、证据与保留策略各自独立。写入时明确类别，读取时只使用明确类别。
use super::types::MemoryItem;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Dialogue,
    SessionSummary,
    Fact,
    /// 采集来的外部资料（联网搜索总结、分享链接抓取、笔记成文）。
    /// 可被检索召回，但没有用户原话支撑，不参与事实合并与共享世界路由。
    Reference,
    Subjective,
    Observation,
    Internal,
}

/// Persisted records carry their own category; missing categories are never inferred.
pub fn kind(item: &MemoryItem) -> RecordKind {
    item.metadata
        .get("record_kind")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or(RecordKind::Internal)
}

/// Stamp a newly created record from the write API, never reinterpret stored data.
pub fn initialize_record(item: &mut MemoryItem) {
    let record_kind = item
        .metadata
        .get("record_kind")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_else(|| match item.memory_type.as_str() {
            "short_term" | "casual_conversation" | "temporary_context" | "mid_term" => {
                RecordKind::Dialogue
            }
            "session_summary" => RecordKind::SessionSummary,
            "inner_monologue" | "insight" => RecordKind::Subjective,
            "observation_note" => RecordKind::Observation,
            // 采集资料是外部内容，不是「关于用户的事实」：`reference` 是用户口述的
            // 资料性事实（save_memory 的 category），仍属事实层。
            "knowledge" => RecordKind::Reference,
            "long_term" | "user" | "feedback" | "project" | "reference" | "general"
            | "preference" | "identity" | "important_event" => RecordKind::Fact,
            _ => RecordKind::Internal,
        });
    if !item.metadata.is_object() {
        item.metadata = serde_json::json!({});
    }
    let meta = item.metadata.as_object_mut().unwrap();
    meta.insert(
        "record_kind".into(),
        serde_json::to_value(record_kind).unwrap(),
    );
    meta.insert("memory_schema_version".into(), serde_json::json!(2));
    meta.entry("retention").or_insert_with(|| {
        serde_json::json!(match record_kind {
            RecordKind::Dialogue => "window",
            RecordKind::Subjective | RecordKind::Internal | RecordKind::Observation => "ephemeral",
            // 采集资料按自身 TTL/expires_at 过期，不因类别被提前淘汰。
            RecordKind::Reference | RecordKind::Fact | RecordKind::SessionSummary => "durable",
        })
    });
    let evidence = if record_kind == RecordKind::Subjective {
        "inferred"
    } else if matches!(record_kind, RecordKind::SessionSummary | RecordKind::Reference) {
        // 采集资料由搜索结果总结而来，不是谁的原话。
        "derived"
    } else if meta
        .get("source_quote")
        .and_then(|v| v.as_str())
        .is_some_and(|v| !v.trim().is_empty())
    {
        "quoted"
    } else {
        "unspecified"
    };
    meta.entry("evidence_kind")
        .or_insert_with(|| serde_json::json!(evidence));
    meta.entry("important_event")
        .or_insert_with(|| serde_json::json!(item.memory_type == "important_event"));
    meta.entry("topics").or_insert_with(|| {
        serde_json::json!(item
            .tags
            .iter()
            .filter(|tag| matches!(
                tag.as_str(),
                "preference"
                    | "identity"
                    | "user_profile"
                    | "project_context"
                    | "relationship"
                    | "health"
                    | "reference"
                    | "knowledge"
            ))
            .collect::<Vec<_>>())
    });
}

/// 能进入检索与对话上下文的类别。
///
/// Reference（采集资料）在这里是**有意保留**的：角色需要能引用联网查到的热梗与
/// 知识。它不进入长期记忆展示，也不参与事实合并，靠类别而非可达性区分。
pub fn recallable(item: &MemoryItem) -> bool {
    !item.content.trim().is_empty()
        && !item.tags.iter().any(|tag| tag == "quick_note")
        && !item.consolidated
        && item.metadata["index_active"] != false
        && matches!(
            kind(item),
            RecordKind::Fact
                | RecordKind::SessionSummary
                | RecordKind::Dialogue
                | RecordKind::Reference
        )
}

/// 原话索引是窗口缓存，退窗不意味着已经摘要，更不能淘汰事实/约定。
/// 保留内容与 ID，供旧成长证据接口及审计使用；规范原话仍在 DialogueManager。
pub fn retire_dialogue_window(
    entries: &mut [MemoryItem],
    incoming: &MemoryItem,
    capacity: usize,
) -> Option<String> {
    if kind(incoming) != RecordKind::Dialogue {
        return None;
    }
    let active: Vec<_> = entries
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            kind(item) == RecordKind::Dialogue
                && item.granularity == incoming.granularity
                && !item.consolidated
                && item.metadata["index_active"] != false
                && !item.protected
        })
        .collect();
    if active.len() < capacity {
        return None;
    }
    let oldest = active
        .into_iter()
        .min_by(|a, b| a.1.timestamp.total_cmp(&b.1.timestamp))
        .map(|(idx, _)| idx);
    if let Some(idx) = oldest {
        if !entries[idx].metadata.is_object() {
            entries[idx].metadata = serde_json::json!({});
        }
        entries[idx].metadata["index_active"] = serde_json::json!(false);
        return Some(entries[idx].id.clone());
    }
    None
}

/// 合并事实时保留每份原话证据，来源可以跨会话，摘要本身不参与这条链路。
pub fn with_evidence(
    mut incoming: serde_json::Value,
    old: &serde_json::Value,
) -> serde_json::Value {
    if !incoming.is_object() {
        incoming = serde_json::json!({});
    }
    let mut sources = Vec::new();
    for meta in [old, &incoming] {
        if let Some(existing) = meta["evidence_sources"].as_array() {
            for source in existing {
                if !sources.contains(source) {
                    sources.push(source.clone());
                }
            }
        }
        if let Some(quote) = meta["source_quote"]
            .as_str()
            .filter(|q| !q.trim().is_empty())
        {
            let source = serde_json::json!({"quote":quote, "conversation_id":meta["conversation_id"],
                "message_ids":meta["source_message_ids"], "subject":meta["subject"],
                "session_id":meta["source_session_id"], "channel":meta["channel"], "role":meta["source_role"],
                "speaker":meta["speaker"], "listener":meta["listener"],
                "knowledge_source":meta["knowledge_source"], "observer_id":meta["observer_id"],
                "timestamp":meta["source_timestamp"], "known_by":meta["known_by"]});
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
    }
    incoming["evidence_sources"] = serde_json::json!(sources);
    incoming
}

#[cfg(test)]
mod tests {
    use super::super::types::Granularity;
    use super::*;
    fn item(t: &str) -> MemoryItem {
        let mut m = MemoryItem::new("原文".into(), Granularity::Turn, 0.5);
        m.memory_type = t.into();
        initialize_record(&mut m);
        m
    }
    #[test]
    fn merged_quotes_preserve_each_speakers_source_and_time() {
        let old = serde_json::json!({"source_quote":"不吃辣", "speaker":"user", "listener":"nana", "source_timestamp":10, "source_message_ids":["a"], "known_by":["nana"]});
        let new = serde_json::json!({"source_quote":"我也不吃辣", "speaker":"vivian", "listener":"nana", "source_timestamp":20, "source_message_ids":["b"], "known_by":["nana"]});
        let merged = with_evidence(new, &old);
        let sources = merged["evidence_sources"].as_array().unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0]["speaker"], "user");
        assert_eq!(sources[1]["speaker"], "vivian");
        assert_eq!(sources[0]["timestamp"], 10);
        assert_eq!(sources[1]["message_ids"], serde_json::json!(["b"]));
    }
    #[test]
    fn legacy_quick_note_is_not_character_recall() {
        let mut note = item("knowledge");
        note.tags.push("quick_note".into());
        assert!(!recallable(&note));
    }
    #[test]
    fn stored_labels_are_not_inferred_or_migrated() {
        let mut unsupported = MemoryItem::new("旧原文".into(), Granularity::Turn, 0.5);
        unsupported.memory_type = "long_term".into();
        unsupported.tags.push("topic_summary".into());
        assert_eq!(kind(&unsupported), RecordKind::Internal);
        assert!(!recallable(&unsupported));
        let mut fact = item("long_term");
        fact.tags.push("inner_os".into());
        assert_eq!(kind(&fact), RecordKind::Fact);
        fact.metadata["record_kind"] = serde_json::json!("subjective");
        assert!(!recallable(&fact));
    }
    #[test]
    fn window_pressure_keeps_facts_and_never_claims_summary_success() {
        let mut entries = vec![item("identity"), item("short_term")];
        let fact = item("preference");
        retire_dialogue_window(&mut entries, &fact, 1);
        assert!(recallable(&entries[0]) && recallable(&entries[1]));
        retire_dialogue_window(&mut entries, &item("short_term"), 1);
        assert!(recallable(&entries[0]));
        assert!(!recallable(&entries[1]));
        assert!(!entries[1].consolidated);
        assert!(entries[1].metadata.get("summarized").is_none());
        assert_eq!(entries[1].content, "原文");
    }

    #[test]
    fn collected_knowledge_is_reference_not_fact_but_still_recallable() {
        let collected = item("knowledge");
        // 采集资料不是「关于用户的事实」：不能被当成已知事实注入或参与合并。
        assert_eq!(kind(&collected), RecordKind::Reference);
        assert_ne!(kind(&collected), RecordKind::Fact);
        // 但仍可被检索召回，角色要能引用查到的热梗与资料。
        assert!(recallable(&collected));
        assert_eq!(collected.metadata["evidence_kind"], serde_json::json!("derived"));
        assert_eq!(collected.metadata["retention"], serde_json::json!("durable"));

        // 用户口述的资料性事实仍属事实层，不受采集资料拆分影响。
        assert_eq!(kind(&item("reference")), RecordKind::Fact);
    }
}
