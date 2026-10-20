//! Authored backstory uses the runtime HistoryEntry / MemoryItem schemas.
//! Originals remain intact; summaries point to original IDs, never to prose chunks.
use super::{kinds, types::MemoryItem};
use crate::dialogue::HistoryEntry;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

pub const SCHEMA_VERSION: u64 = 2;
pub const ENVIRONMENT_REVISION: &str = "identity-open-arrival-1";
// This shared memory path is already removed by the factory-reset sweep.
pub const CLOCK_RELATIVE_PATH: &str = "companion-v2/memory/seed_clock.json";

#[derive(Serialize, Deserialize)]
struct SeedClock { first_started_at: f64 }

pub fn load_or_create_anchor(path: &Path, now: f64) -> crate::error::VivianResult<f64> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let clock = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<SeedClock>(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let clock = SeedClock { first_started_at: now };
            if !now.is_finite() || now <= 0.0 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "无效的首次启动时间").into());
            }
            crate::utils::fs::write_atomic(path, &serde_json::to_string(&clock)?)?;
            clock
        }
        Err(e) => return Err(e.into()),
    };
    if !clock.first_started_at.is_finite() || clock.first_started_at <= 0.0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "种子记忆时间锚点无效").into());
    }
    Ok(clock.first_started_at)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedBundle {
    pub seed_schema_version: u64,
    pub seed_revision: String,
    pub timeline_anchor: f64,
    pub character_id: String,
    pub history: Vec<HistoryEntry>,
    pub memories: Vec<MemoryItem>,
}

pub fn load(character: &str) -> Result<SeedBundle, String> {
    let text = match character {
        "vivian" => include_str!("../../prompts/characters/vivian/seed_memories.md"),
        "nana" => include_str!("../../prompts/characters/nana/seed_memories.md"),
        _ => return Ok(SeedBundle { seed_schema_version: SCHEMA_VERSION,
            seed_revision: String::new(),
            timeline_anchor: 1.0,
            character_id: character.into(), history: vec![], memories: vec![] }),
    };
    parse(text, character)
}

pub fn parse(text: &str, character: &str) -> Result<SeedBundle, String> {
    let text = text.replace("\r\n", "\n");
    let json = if text.trim_start().starts_with('{') { text.trim() } else {
        text.split_once("```json\n").and_then(|(_, rest)| rest.split_once("\n```"))
            .map(|(json, _)| json).ok_or("种子记忆缺少 JSON 数据块")?
    };
    let mut bundle: SeedBundle = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if bundle.seed_schema_version != SCHEMA_VERSION || bundle.character_id != character
        || bundle.seed_revision.trim().is_empty()
        || !bundle.timeline_anchor.is_finite() || bundle.timeline_anchor <= 0.0 {
        return Err("种子记忆版本或角色不匹配".into());
    }
    let mut ids = HashSet::new();
    for (id, content, ts, meta) in bundle.history.iter().map(|h|
        (&h.id, &h.content, h.timestamp, &h.metadata)).chain(bundle.memories.iter().map(|m|
        (&m.id, &m.content, m.timestamp, &m.metadata))) {
        if !id.starts_with("seed_") || !ids.insert(id.as_str()) || content.trim().is_empty()
            || !ts.is_finite() || ts <= 0.0 || ts > bundle.timeline_anchor || meta["source"] != "system_seed"
            || meta["fictional_backstory"] != true || meta["timestamp_origin"] != "authored"
            || meta["memory_owner"] != character
            || !super::provenance::visible_to(meta, character) {
            return Err(format!("无效的种子原始记录/记忆: {id}"));
        }
    }
    for h in &bundle.history {
        if !matches!(h.role.as_str(), "user" | "assistant")
            || h.metadata["speaker"].as_str().is_none()
            || h.metadata["listener"].as_str().is_none()
            || h.metadata["speaker"] == "user" || h.metadata["listener"] == "user" {
            return Err(format!("角色前史不得冒充当前用户的对话: {}", h.id));
        }
    }
    for m in &bundle.memories {
        if super::types::MemoryType::from_str(&m.memory_type).is_none()
            || m.granularity.parse::<super::types::Granularity>().is_err()
            || !m.importance.is_finite() || !(0.0..=1.0).contains(&m.importance) {
            return Err(format!("种子记忆类型、粒度或重要度无效: {}", m.id));
        }
        let mut source_count = 0;
        for key in ["source_message_ids", "source_memory_ids"] {
            let Some(value) = m.metadata.get(key) else { continue; };
            let sources = value.as_array().ok_or("种子来源 ID 必须是数组")?;
            for source in sources {
                let id = source.as_str().ok_or("种子来源 ID 必须是字符串")?;
                let ts = if key == "source_message_ids" {
                    bundle.history.iter().find(|h| h.id == id).map(|h| h.timestamp)
                } else {
                    bundle.memories.iter().find(|s| s.id == id && s.metadata["evidence_kind"] == "recorded")
                        .map(|s| s.timestamp)
                };
                if id == m.id || ts.is_none_or(|ts| ts > m.timestamp) {
                    return Err(format!("种子记忆来源不存在或晚于派生记录: {} → {id}", m.id));
                }
                source_count += 1;
            }
        }
        if m.metadata["evidence_kind"] == "derived" && source_count == 0 {
            return Err(format!("派生种子记忆缺少原始来源: {}", m.id));
        }
    }
    for m in &mut bundle.memories {
        m.metadata["seed_schema_version"] = SCHEMA_VERSION.into();
        m.metadata["seed_revision"] = bundle.seed_revision.clone().into();
        kinds::initialize_record(m);
    }
    Ok(bundle)
}

impl SeedBundle {
    pub fn anchor_to(&mut self, first_started_at: f64) -> Result<(), String> {
        let delta = first_started_at - self.timeline_anchor;
        let earliest = self.history.iter().map(|h| h.timestamp)
            .chain(self.memories.iter().map(|m| m.timestamp)).fold(self.timeline_anchor, f64::min);
        if !first_started_at.is_finite() || earliest + delta <= 0.0 {
            return Err("首次启动时间不足以容纳种子前史".into());
        }
        for h in &mut self.history {
            h.timestamp += delta;
            shift_source_times(&mut h.metadata, delta);
            h.metadata["timestamp_origin"] = "first_start_relative".into();
            h.metadata["seed_anchor"] = first_started_at.into();
        }
        for m in &mut self.memories {
            m.timestamp += delta;
            shift_source_times(&mut m.metadata, delta);
            for hook in &mut m.open_hooks {
                hook.created_at += delta;
                if let Some(closed) = &mut hook.closed_at { *closed += delta; }
            }
            m.metadata["timestamp_origin"] = "first_start_relative".into();
            m.metadata["seed_anchor"] = first_started_at.into();
        }
        self.timeline_anchor = first_started_at;
        Ok(())
    }
}

fn shift_source_times(value: &mut serde_json::Value, delta: f64) {
    match value {
        serde_json::Value::Object(map) => for (key, value) in map {
            if matches!(key.as_str(), "timestamp" | "created_at" | "source_timestamp"
                | "started_at" | "ended_at" | "recorded_at" | "updated_at") {
                if let Some(ts) = value.as_f64().filter(|ts| ts.is_finite() && *ts > 0.0) {
                    *value = serde_json::json!(ts + delta);
                }
            } else { shift_source_times(value, delta); }
        },
        serde_json::Value::Array(values) => for value in values { shift_source_times(value, delta); },
        _ => {}
    }
}

/// Same JSONL format as runtime dialogue, kept out of the current user's chat.
/// Rewriting this authored archive never touches runtime history or retrieval heat.
pub fn persist_history(path: &Path, history: &[HistoryEntry]) -> crate::error::VivianResult<()> {
    let text = history.iter().map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?.join("\n") + "\n";
    if std::fs::read_to_string(path).ok().as_deref() != Some(&text) {
        crate::utils::fs::write_atomic(path, &text)?;
    }
    Ok(())
}

pub fn read_history(path: &Path) -> crate::error::VivianResult<Vec<HistoryEntry>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    text.lines().filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into)).collect()
}

/// Replace outdated application-authored seeds. Live records and environment presets survive.
/// Stable IDs preserve visit counts and lifecycle state on every subsequent startup.
pub fn upgrade_entries(entries: &mut Vec<MemoryItem>, bundle: &SeedBundle) -> Vec<String> {
    if bundle.memories.is_empty() || entries.iter().any(|m|
        m.metadata["source"] == "system_seed" && m.metadata["seed_schema_version"] == SCHEMA_VERSION
            && m.metadata["seed_revision"] == bundle.seed_revision
            && m.metadata["seed_anchor"] == bundle.memories[0].metadata["seed_anchor"]) {
        return vec![];
    }
    let mut removed = vec![];
    entries.retain(|m| {
        let obsolete = m.id.starts_with("seed_") && m.metadata["source"] == "system_seed";
        if obsolete { removed.push(m.id.clone()); }
        !obsolete
    });
    entries.extend(bundle.memories.iter().cloned());
    removed
}

/// Refresh only application-authored environment defaults; runtime memories survive.
pub fn retire_outdated_environment(entries: &mut Vec<MemoryItem>) -> Option<Vec<String>> {
    let current = entries.iter().any(|m| m.metadata["source"] == "environment_preset");
    if current && entries.iter().filter(|m| m.metadata["source"] == "environment_preset")
        .all(|m| m.metadata["environment_revision"] == ENVIRONMENT_REVISION) {
        return None;
    }
    let mut removed = vec![];
    entries.retain(|m| {
        let obsolete = m.metadata["source"] == "environment_preset";
        if obsolete { removed.push(m.id.clone()); }
        !obsolete
    });
    Some(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn fixture() -> Value {
        let bundle = load("vivian").unwrap();
        json!({"seed_schema_version":SCHEMA_VERSION,"seed_revision":bundle.seed_revision,"timeline_anchor":bundle.timeline_anchor,"character_id":"vivian",
            "history":bundle.history,"memories":bundle.memories})
    }
    fn parse_value(value: &Value) -> Result<SeedBundle, String> {
        parse(&format!("```json\n{}\n```", value), "vivian")
    }

    #[test]
    fn native_records_keep_original_times_and_resolvable_sources() {
        for character in ["vivian", "nana"] {
            let bundle = load(character).unwrap();
            assert!(!bundle.history.is_empty() && !bundle.memories.is_empty());
            let conversations = super::super::conversations::build_conversations(&bundle.history, character);
            for m in &bundle.memories {
                let serialized = serde_json::to_string(m).unwrap();
                let restored: MemoryItem = serde_json::from_str(&serialized).unwrap();
                assert_eq!(restored.timestamp, m.timestamp);
                assert_eq!(restored.content, m.content);
                assert_eq!(restored.related_ids, m.related_ids);
                assert_eq!(restored.metadata["memory_schema_version"], 2);
                assert!(!super::super::companion_policy::is_durable_fact(m));
                assert!(super::super::provenance::recall_label(&m.metadata).contains("角色前史"));
                if m.memory_type == "session_summary" {
                    let c = conversations.iter().find(|c| m.metadata["conversation_id"] == c.id).unwrap();
                    assert_eq!(m.metadata["started_at"], c.started_at);
                    assert_eq!(m.metadata["ended_at"], c.ended_at);
                    let ids: Vec<_> = c.turns.iter().map(|t| t.id.as_str()).collect();
                    assert_eq!(m.metadata["source_message_ids"], json!(ids));
                    assert!(m.timestamp > c.ended_at);
                }
            }
        }
    }

    #[test]
    fn shared_dialogue_is_identical_and_private_notes_stay_private() {
        let v = load("vivian").unwrap();
        let n = load("nana").unwrap();
        let mut shared = 0;
        for h in &v.history {
            if let Some(other) = n.history.iter().find(|other| other.id == h.id) {
                assert_eq!(h.content, other.content);
                assert_eq!(h.timestamp, other.timestamp);
                assert_eq!(h.metadata["speaker"], other.metadata["speaker"]);
                assert_eq!(h.metadata["listener"], other.metadata["listener"]);
                shared += 1;
            }
        }
        assert!(shared > 0);
        for (owner, other, bundle) in [("vivian", "nana", &v), ("nana", "vivian", &n)] {
            for m in &bundle.memories {
                assert!(super::super::provenance::visible_to(&m.metadata, owner));
                assert!(!super::super::provenance::visible_to(&m.metadata, other));
            }
        }
    }

    #[test]
    fn invalid_sources_times_roles_and_windows_line_endings_are_checked() {
        let good = fixture();
        let mut bad = good.clone();
        bad["memories"][0]["metadata"]["source_message_ids"] = json!(["missing"]);
        assert!(parse_value(&bad).is_err());
        bad = good.clone(); bad["memories"][0]["created_at"] = json!(1.0);
        assert!(parse_value(&bad).is_err());
        bad = good.clone(); bad["history"][0]["metadata"]["speaker"] = json!("user");
        assert!(parse_value(&bad).is_err());
        bad = good.clone(); bad["history"][1]["id"] = bad["history"][0]["id"].clone();
        assert!(parse_value(&bad).is_err());
        bad = good.clone(); bad["memories"][0]["metadata"]["source_message_ids"] = json!([]);
        assert!(parse_value(&bad).is_err());
        assert!(parse(&format!("```json\r\n{}\r\n```", good), "vivian").is_ok());
    }

    #[test]
    fn upgrade_preserves_live_records_and_restart_keeps_seed_state() {
        let bundle = load("vivian").unwrap();
        let mut old = bundle.memories[0].clone(); old.id = "seed_legacy".into();
        old.metadata.as_object_mut().unwrap().remove("seed_schema_version");
        let mut live = old.clone(); live.id = "seed_user_saved".into(); live.metadata["source"] = json!("live");
        let mut environment = old.clone(); environment.id = "seed_environment".into();
        environment.metadata["source"] = json!("environment_preset");
        let mut entries = vec![old, live, environment];
        assert_eq!(upgrade_entries(&mut entries, &bundle), vec!["seed_legacy"]);
        assert_eq!(entries.len(), bundle.memories.len() + 2);
        assert_eq!(entries[0].id, "seed_user_saved");
        assert_eq!(entries[1].id, "seed_environment");
        entries[2].visit_count = 7; entries[2].importance = 0.33;
        let snapshot = serde_json::to_string(&entries).unwrap();
        assert!(upgrade_entries(&mut entries, &bundle).is_empty());
        assert_eq!(serde_json::to_string(&entries).unwrap(), snapshot);
    }

    #[test]
    fn environment_upgrade_preserves_user_identity_and_runs_once() {
        let bundle = load("vivian").unwrap();
        let mut old = bundle.memories[0].clone();
        old.id = "seed_old_environment".into();
        old.metadata = json!({"source":"environment_preset"});
        let mut user = old.clone();
        user.id = "user_identity".into();
        user.content = "用户自述是应用开发者 AlenTinn".into();
        user.metadata = json!({"source":"direct","source_quote":"我就是开发你的 AlenTinn"});
        let mut entries = vec![old.clone(), user.clone(), bundle.memories[0].clone()];
        assert_eq!(retire_outdated_environment(&mut entries), Some(vec![old.id.clone()]));
        assert_eq!(serde_json::to_value(&entries[0]).unwrap(), serde_json::to_value(&user).unwrap());
        old.metadata["environment_revision"] = ENVIRONMENT_REVISION.into();
        old.visit_count = 8;
        entries.push(old);
        let before = serde_json::to_value(&entries).unwrap();
        assert_eq!(retire_outdated_environment(&mut entries), None);
        assert_eq!(serde_json::to_value(&entries).unwrap(), before);
        assert_eq!(retire_outdated_environment(&mut vec![]), Some(vec![]));
    }

    #[test]
    fn new_story_revision_replaces_old_content_with_the_same_runtime_schema() {
        let bundle = load("vivian").unwrap();
        let mut old = bundle.memories[0].clone();
        old.metadata["seed_revision"] = json!("previous-daily-stories");
        old.content = "previous seed text".into();
        let old_id = old.id.clone();
        let mut entries = vec![old];
        assert_eq!(upgrade_entries(&mut entries, &bundle), vec![old_id]);
        assert_eq!(entries.len(), bundle.memories.len());
        assert!(entries.iter().all(|m| m.metadata["seed_revision"] == bundle.seed_revision));
        assert!(entries.iter().all(|m| m.content != "previous seed text"));

        // The first structured seed release had schema=2 but no content revision.
        entries.iter_mut().for_each(|m| { m.metadata.as_object_mut().unwrap().remove("seed_revision"); });
        assert_eq!(upgrade_entries(&mut entries, &bundle).len(), bundle.memories.len());
        assert!(upgrade_entries(&mut entries, &bundle).is_empty());
    }

    #[test]
    fn first_start_shifts_originals_summaries_and_source_times_together() {
        let launch = 2_000_000_000.0;
        let mut v = load("vivian").unwrap();
        let mut n = load("nana").unwrap();
        let original_span = v.history.last().unwrap().timestamp - v.history[0].timestamp;
        v.anchor_to(launch).unwrap(); n.anchor_to(launch).unwrap();
        assert_eq!(v.history.last().unwrap().timestamp - v.history[0].timestamp, original_span);
        for bundle in [&v, &n] {
            assert!(bundle.history.iter().all(|h| h.timestamp < launch));
            assert!(bundle.memories.iter().all(|m| m.timestamp < launch));
            for m in &bundle.memories {
                assert_eq!(m.metadata["seed_anchor"], launch);
                assert_eq!(m.metadata["timestamp_origin"], "first_start_relative");
                if let Some(attributions) = m.metadata["source_attributions"].as_array() {
                    for attribution in attributions {
                        let h = bundle.history.iter().find(|h| attribution["message_id"] == h.id).unwrap();
                        assert_eq!(attribution["timestamp"], h.timestamp);
                    }
                    let ids = m.metadata["source_message_ids"].as_array().unwrap();
                    let first = bundle.history.iter().find(|h| ids[0] == h.id).unwrap();
                    let last = bundle.history.iter().find(|h| ids.last().unwrap() == &serde_json::json!(h.id)).unwrap();
                    assert_eq!(m.metadata["started_at"], first.timestamp);
                    assert_eq!(m.metadata["ended_at"], last.timestamp);
                    assert!(m.timestamp > last.timestamp);
                }
                if let Some(ids) = m.metadata["source_memory_ids"].as_array().filter(|ids| !ids.is_empty()) {
                    let source = bundle.memories.iter().find(|s| ids[0] == s.id).unwrap();
                    assert_eq!(m.metadata["source_timestamp"], source.timestamp);
                    assert_eq!(source.metadata["recorded_at"], source.timestamp);
                }
            }
        }
        for h in &v.history {
            if let Some(other) = n.history.iter().find(|other| other.id == h.id) {
                assert_eq!(h.timestamp, other.timestamp);
            }
        }
        let before = serde_json::to_value(&v.history).unwrap();
        v.anchor_to(launch).unwrap();
        assert_eq!(serde_json::to_value(&v.history).unwrap(), before);
    }

    #[test]
    fn persisted_launch_clock_prevents_restart_drift_and_preserves_seed_heat() {
        let dir = std::env::temp_dir().join(format!("seed-clock-{}", uuid::Uuid::new_v4()));
        let path = dir.join(CLOCK_RELATIVE_PATH);
        let first = load_or_create_anchor(&path, 2_000_000_000.0).unwrap();
        let mut original = load("vivian").unwrap(); original.anchor_to(first).unwrap();
        let mut entries = vec![]; upgrade_entries(&mut entries, &original);
        entries[0].visit_count = 9;
        let snapshot = serde_json::to_string(&entries).unwrap();
        let restart = load_or_create_anchor(&path, first + 86400.0).unwrap();
        assert_eq!(first, restart);
        let mut reloaded = load("vivian").unwrap(); reloaded.anchor_to(restart).unwrap();
        assert!(upgrade_entries(&mut entries, &reloaded).is_empty());
        assert_eq!(serde_json::to_string(&entries).unwrap(), snapshot);
        let mut other = load("nana").unwrap(); other.anchor_to(restart).unwrap();
        assert_eq!(other.timeline_anchor, original.timeline_anchor);
        std::fs::write(&path, "corrupt clock").unwrap();
        assert!(load_or_create_anchor(&path, first + 172800.0).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "corrupt clock");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn original_jsonl_survives_roundtrip_without_touching_user_history() {
        let dir = std::env::temp_dir().join(format!("seed-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("seed_history.jsonl");
        let user = dir.join("history.jsonl");
        std::fs::write(&user, "user history sentinel").unwrap();
        let bundle = load("nana").unwrap();
        persist_history(&archive, &bundle.history).unwrap();
        let first = std::fs::read(&archive).unwrap();
        persist_history(&archive, &bundle.history).unwrap();
        assert_eq!(std::fs::read(&archive).unwrap(), first);
        let restored = read_history(&archive).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), serde_json::to_value(&bundle.history).unwrap());
        assert_eq!(std::fs::read_to_string(&user).unwrap(), "user history sentinel");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
