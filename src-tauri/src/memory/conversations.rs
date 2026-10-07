//! 对话历史是原始发言的唯一来源；同一会话在 UI 与巩固中共用此投影。
use super::conversation_semantics::{assess_boundary, BoundaryCandidate, BoundaryDecision};
use crate::dialogue::HistoryEntry;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: String,
    pub speaker: String,
    pub listener: String,
    pub text: String,
    pub timestamp: f64,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub id: String,
    pub source_session_id: Option<String>,
    pub title: String,
    pub participants: Vec<String>,
    pub channels: Vec<String>,
    pub started_at: f64,
    pub ended_at: f64,
    pub turns: Vec<ConversationTurn>,
}

fn meta_str<'a>(entry: &'a HistoryEntry, key: &str) -> Option<&'a str> {
    entry
        .metadata
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
}

/// Legacy reverse deliveries saved one brain record and one bus mirror with
/// different UUIDs. Only collapse that adjacent, same-session provenance pair.
fn legacy_cross_mirror(previous: &HistoryEntry, entry: &HistoryEntry) -> bool {
    if meta_str(previous, "utterance_id").is_some() || meta_str(entry, "utterance_id").is_some() {
        return false;
    }
    let session = |e: &HistoryEntry| meta_str(e, "session_id").map(str::to_string)
        .or_else(|| e.session_id.clone());
    let origin = |e: &HistoryEntry| {
        if meta_str(e, "content_type") == Some("dialogue_turn")
            && meta_str(e, "conversation_id").is_some() { 1 }
        else if meta_str(e, "content_type").is_none()
            && meta_str(e, "knowledge_source") == Some("heard") { 2 }
        else { 0 }
    };
    let a = origin(previous);
    let b = origin(entry);
    a != 0 && b != 0 && a != b
        && meta_str(previous, "channel") == Some("cross_character")
        && meta_str(entry, "channel") == Some("cross_character")
        && session(previous).is_some() && session(previous) == session(entry)
        && previous.role == entry.role
        && meta_str(previous, "speaker").is_some()
        && meta_str(previous, "speaker") == meta_str(entry, "speaker")
        && meta_str(previous, "listener").is_some()
        && meta_str(previous, "listener") == meta_str(entry, "listener")
        && crate::cross_character::parse_any_speaker_prefix(&previous.content).0
            == crate::cross_character::parse_any_speaker_prefix(&entry.content).0
        && previous.metadata.get("sticker") == entry.metadata.get("sticker")
        && (0.0..=30.0).contains(&(entry.timestamp - previous.timestamp))
}

/// Runtime session IDs describe response scheduling, not human conversation episodes.
pub fn build_conversations(history: &[HistoryEntry], character: &str) -> Vec<ConversationRecord> {
    project_conversations(history, character, &HashMap::new()).0
}

pub fn project_conversations(
    history: &[HistoryEntry],
    character: &str,
    decisions: &HashMap<String, BoundaryDecision>,
) -> (Vec<ConversationRecord>, Vec<BoundaryCandidate>) {
    let mut entries: Vec<_> = history.iter().collect();
    entries.sort_by(|a, b| {
        a.timestamp
            .total_cmp(&b.timestamp)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen = HashSet::new();
    let mut groups: Vec<ConversationRecord> = Vec::new();
    let mut active: HashMap<String, usize> = HashMap::new();
    let mut lanes: HashMap<String, Vec<ConversationTurn>> = HashMap::new();
    let mut candidates = Vec::new();
    let mut candidate_positions = Vec::new();
    let mut previous_entry = None;
    for entry in entries {
        if !matches!(entry.role.as_str(), "user" | "assistant")
            || !seen.insert(meta_str(entry, "utterance_id").unwrap_or(&entry.id).to_string())
            || matches!(
                meta_str(entry, "content_type"),
                Some("response_status" | "exchange_record" | "internal_directive")
            )
            || entry.metadata["memory_disabled"] == true
            || (entry.content.trim().is_empty() && entry.metadata.get("sticker").is_none())
        {
            continue;
        }
        if previous_entry.is_some_and(|previous| legacy_cross_mirror(previous, entry)) { continue; }
        previous_entry = Some(entry);
        let (text, _, _) = crate::cross_character::parse_any_speaker_prefix(&entry.content);
        let normalize_person = |person: &str| match person.to_lowercase().as_str() {
            "i" | "me" => character.to_string(),
            "everyone" => "all".into(),
            other => other.into(),
        };
        let speaker = normalize_person(meta_str(entry, "speaker").unwrap_or(
            if entry.role == "user" {
                "user"
            } else {
                character
            },
        ));
        let listener =
            normalize_person(meta_str(entry, "listener").unwrap_or(if speaker == "user" {
                character
            } else {
                "user"
            }));
        let mut participants = BTreeSet::from([speaker.clone(), character.to_string()]);
        if listener == "all" {
            participants.extend(["user".into(), "vivian".into(), "nana".into()]);
        } else {
            participants.insert(listener.clone());
        }
        // One social floor includes direct speech, group speech and interjections.
        // An independent private roommate exchange has a separate floor, even when interleaved.
        let lane = if participants.contains("user") {
            "user".to_string()
        } else {
            participants.iter().cloned().collect::<Vec<_>>().join(",")
        };
        let turn = ConversationTurn {
            id: entry.id.clone(),
            speaker,
            listener,
            text,
            timestamp: entry.timestamp,
            channel: meta_str(entry, "channel").unwrap_or("direct").into(),
            sticker: entry.metadata.get("sticker").cloned(),
        };
        let context = lanes.entry(lane.clone()).or_default();
        let explicit_new = entry.metadata["conversation_boundary"] == "new";
        let mut split = explicit_new;
        if let Some(candidate) = assess_boundary(context, &turn) {
            let reviewed = decisions
                .get(&turn.id)
                .filter(|decision| decision.fingerprint == candidate.fingerprint);
            split |= reviewed
                .map(|decision| decision.starts_new)
                .unwrap_or(candidate.fallback_new);
            if reviewed.is_none_or(|decision| !decision.settled) && !explicit_new {
                candidate_positions.push((candidates.len(), lane.clone(), context.len()));
                candidates.push(candidate);
            }
        }
        let idx = match active.get(&lane).copied().filter(|_| !split) {
            Some(idx) => idx,
            None => {
                let idx = groups.len();
                groups.push(ConversationRecord {
                    id: format!("conversation:{character}:{}", entry.id),
                    source_session_id: meta_str(entry, "session_id")
                        .or(entry.session_id.as_deref())
                        .map(str::to_string),
                    title: String::new(),
                    participants: vec![],
                    channels: vec![],
                    started_at: entry.timestamp,
                    ended_at: entry.timestamp,
                    turns: vec![],
                });
                active.insert(lane, idx);
                idx
            }
        };
        let group = &mut groups[idx];
        group.ended_at = entry.timestamp;
        let mut combined: BTreeSet<String> = group.participants.iter().cloned().collect();
        combined.extend(participants);
        group.participants = combined.into_iter().collect();
        let names: Vec<_> = group
            .participants
            .iter()
            .filter(|p| p.as_str() != character)
            .map(|p| match p.as_str() {
                "user" => "你",
                "vivian" => "Vivian",
                "nana" => "Nana",
                other => other,
            })
            .collect();
        group.title = format!("与{}的对话", names.join("、"));
        if !group.channels.contains(&turn.channel) {
            group.channels.push(turn.channel.clone());
        }
        group.turns.push(turn.clone());
        context.push(turn);
    }
    // Right-side replies help resolve delayed answers and actual new openings. Only raw turns.
    for (candidate_idx, lane, position) in candidate_positions {
        if let Some(turns) = lanes.get(&lane) {
            candidates[candidate_idx].after = turns[position..].iter().take(3).cloned().collect();
        }
    }
    candidates.retain(|candidate| {
        decisions
            .get(&candidate.message_id)
            .filter(|decision| decision.fingerprint == candidate.fingerprint)
            .is_none_or(|decision| {
                !decision.settled && candidate.after.len() > decision.reviewed_after
            })
    });
    (groups, candidates)
}

/// Rebuild derived references from stable original message IDs after regrouping.
/// Returns true when an obsolete summary must leave the retrieval index.
pub fn refresh_derived_links(
    item: &mut super::types::MemoryItem,
    valid: &HashSet<&str>,
    owners: &HashMap<&str, &str>,
) -> bool {
    let expected = item.metadata["conversation_id"].as_str().unwrap_or("");
    let ids_moved = item.metadata["source_message_ids"]
        .as_array()
        .is_some_and(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str())
                .any(|id| owners.get(id).copied() != Some(expected))
        });
    let parts_moved = item.metadata["summary_parts"]
        .as_array()
        .is_some_and(|parts| {
            parts
                .iter()
                .filter_map(|part| part["slices"].as_array())
                .flatten()
                .filter_map(|slice| slice["message_id"].as_str())
                .any(|id| owners.get(id).copied() != Some(expected))
        });
    let retired = item.metadata["source"] == "session_consolidation"
        && !item.consolidated
        && (!valid.contains(expected) || ids_moved || parts_moved);
    if retired {
        item.consolidated = true;
        item.metadata["index_active"] = serde_json::json!(false);
        item.metadata["superseded_by_grouping"] = serde_json::json!(true);
    }
    if super::kinds::kind(item) == super::kinds::RecordKind::Fact {
        let locate = |meta: &serde_json::Value, key: &str| {
            meta[key].as_array().and_then(|ids| {
                ids.iter()
                    .find_map(|id| id.as_str().and_then(|id| owners.get(id).copied()))
            })
        };
        if let Some(id) = locate(&item.metadata, "source_message_ids") {
            item.metadata["conversation_id"] = serde_json::json!(id);
        }
        if let Some(sources) = item.metadata["evidence_sources"].as_array_mut() {
            for source in sources {
                if let Some(id) = locate(source, "message_ids") {
                    source["conversation_id"] = serde_json::json!(id);
                }
            }
        }
    }
    retired
}

/// 证据定位只匹配原话及其说话者，绝不拿摘要或 OS 作事实来源。
pub fn locate_quote(
    groups: &[ConversationRecord],
    quote: &str,
    subject: &str,
    character: &str,
) -> Option<(String, String)> {
    if quote.trim().is_empty() {
        return None;
    }
    groups.iter().rev().find_map(|group| {
        group
            .turns
            .iter()
            .rev()
            .find(|turn| {
                turn.text.contains(quote)
                    && match subject {
                        "user" => turn.speaker == "user",
                        "self" => turn.speaker == character,
                        _ => true,
                    }
            })
            .map(|turn| (group.id.clone(), turn.id.clone()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(id: &str, time: f64, role: &str, meta: serde_json::Value) -> HistoryEntry {
        HistoryEntry {
            id: id.into(),
            timestamp: time,
            role: role.into(),
            content: "你好".into(),
            session_id: Some("same-session".into()),
            metadata: meta,
        }
    }
    #[test]
    fn legacy_reverse_delivery_collapses_only_the_mirror_pair() {
        let brain = entry("brain", 10.0, "assistant", serde_json::json!({
            "channel":"cross_character","speaker":"nana","listener":"vivian","knowledge_source":"heard"}));
        let mirror = entry("mirror", 13.0, "assistant", serde_json::json!({
            "channel":"cross_character","speaker":"nana","listener":"vivian",
            "session_id":"same-session","conversation_id":"same-session","content_type":"dialogue_turn"}));
        let answer = entry("answer", 13.1, "user", serde_json::json!({
            "channel":"cross_character","speaker":"vivian","listener":"nana","content_type":"dialogue_turn"}));
        let groups = build_conversations(&[brain.clone(), mirror.clone(), answer.clone()], "nana");
        assert_eq!(groups[0].turns.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["brain", "answer"]);
        // An intervening reply or another session makes it a separate utterance.
        let mut repeated = brain.clone(); repeated.id = "repeat".into(); repeated.timestamp = 14.0;
        assert_eq!(build_conversations(&[brain.clone(), answer, repeated], "nana")[0].turns.len(), 3);
        let mut other_session = mirror.clone(); other_session.metadata["session_id"] = serde_json::json!("other");
        assert!(!legacy_cross_mirror(&brain, &other_session));
        let mut other_listener = mirror.clone(); other_listener.metadata["listener"] = serde_json::json!("user");
        assert!(!legacy_cross_mirror(&brain, &other_listener));
        // Explicit new identities never use legacy text/provenance matching.
        let mut original = brain; original.metadata["utterance_id"] = serde_json::json!("new-one");
        let mut distinct = mirror; distinct.metadata["utterance_id"] = serde_json::json!("new-two");
        assert_eq!(build_conversations(&[original.clone(), distinct.clone()], "nana")[0].turns.len(), 2);
        distinct.metadata["utterance_id"] = serde_json::json!("new-one");
        assert_eq!(build_conversations(&[original, distinct], "nana")[0].turns.len(), 1);
    }

    #[test]
    fn independent_social_floors_do_not_mix() {
        let a = entry("a", 10.0, "user", serde_json::json!({"channel":"direct"}));
        let b = entry(
            "b",
            12.0,
            "assistant",
            serde_json::json!({"channel":"wechat"}),
        );
        let c = entry(
            "c",
            13.0,
            "user",
            serde_json::json!({"speaker":"nana", "listener":"vivian"}),
        );
        let d = entry("d", 4000.0, "assistant", serde_json::json!({}));
        let tool = entry("tool", 14.0, "tool", serde_json::json!({}));
        let status = entry(
            "status",
            14.0,
            "assistant",
            serde_json::json!({"content_type":"response_status"}),
        );
        let groups = build_conversations(&[a.clone(), a, b, c, d, tool, status], "vivian");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].turns.len(), 3);
        assert_eq!(groups[0].channels, vec!["direct", "wechat"]);
        assert_eq!(groups[1].turns[0].speaker, "nana");
        assert_ne!(groups[0].id, groups[1].id);
    }
    #[test]
    fn greeting_broadcast_and_direct_reply_share_a_stable_conversation() {
        let mut greeting = entry(
            "greeting",
            10.0,
            "assistant",
            serde_json::json!({"channel":"proactive", "session_id":"same-session"}),
        );
        greeting.session_id = None;
        let initial = build_conversations(&[greeting.clone()], "nana");
        let broadcast = entry(
            "broadcast",
            20.0,
            "user",
            serde_json::json!({"channel":"broadcast", "speaker":"user", "listener":"all"}),
        );
        let response = entry(
            "response",
            30.0,
            "assistant",
            serde_json::json!({"channel":"broadcast", "speaker":"nana", "listener":"all"}),
        );
        let direct = entry(
            "direct",
            40.0,
            "user",
            serde_json::json!({"channel":"direct"}),
        );
        let groups = build_conversations(&[greeting, broadcast, response, direct], "nana");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, initial[0].id);
        assert_eq!(groups[0].turns.len(), 4);
        assert_eq!(groups[0].participants, vec!["nana", "user", "vivian"]);
        assert_eq!(groups[0].title, "与你、Vivian的对话");
        assert_eq!(groups[0].turns[1].listener, "all");
    }
    #[test]
    fn regrouping_relinks_facts_and_retires_only_derived_summaries() {
        use crate::memory::types::{Granularity, MemoryItem};
        let valid = HashSet::from(["scene:new"]);
        let owners = HashMap::from([("original-user-message", "scene:new")]);
        let mut fact = MemoryItem::new("周五见".into(), Granularity::Summary, 0.7);
        fact.metadata = serde_json::json!({"record_kind":"fact", "source_message_ids":["original-user-message"],
            "conversation_id":"obsolete", "source_quote":"周五见", "evidence_sources":[{"quote":"周五见",
            "message_ids":["original-user-message"],"conversation_id":"obsolete"}]});
        assert!(!refresh_derived_links(&mut fact, &valid, &owners));
        assert_eq!(fact.metadata["conversation_id"], "scene:new");
        assert_eq!(
            fact.metadata["evidence_sources"][0]["conversation_id"],
            "scene:new"
        );
        assert_eq!(
            fact.metadata["source_message_ids"],
            serde_json::json!(["original-user-message"])
        );
        assert_eq!(fact.content, "周五见");
        assert!(!fact.consolidated);
        let mut summary = fact.clone();
        summary.metadata = serde_json::json!({"record_kind":"session_summary",
            "source":"session_consolidation","conversation_id":"obsolete"});
        assert!(refresh_derived_links(&mut summary, &valid, &owners));
        assert!(summary.consolidated);
        assert!(!crate::memory::kinds::recallable(&summary));
        assert_eq!(summary.content, "周五见");
        let mut mixed = fact.clone();
        mixed.metadata = serde_json::json!({"record_kind":"session_summary",
            "source":"session_consolidation", "conversation_id":"scene:new", "source_message_ids":[],
            "summary_parts":[{"slices":[{"message_id":"message-moved-to-other-scene"}]}]});
        assert!(refresh_derived_links(&mut mixed, &valid, &owners));
        assert!(mixed.consolidated);
    }
    #[test]
    fn identical_utterances_with_distinct_ids_are_preserved() {
        let groups = build_conversations(
            &[
                entry("a", 1.0, "user", serde_json::json!({})),
                entry("b", 2.0, "user", serde_json::json!({})),
            ],
            "nana",
        );
        assert_eq!(groups[0].turns.len(), 2);
        assert!(locate_quote(&groups, "你好", "self", "nana").is_none());
    }
}
