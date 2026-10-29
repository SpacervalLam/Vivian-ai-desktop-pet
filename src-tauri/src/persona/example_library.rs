//! Dialogue examples and sourced expression fragments. References protect edits.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const MAX_EDITS: usize = 4;
pub const MAX_OVERRIDES: usize = 64;
pub const AUDIT_WINDOW: usize = 24;
pub const BASE_FILE: &str = "example_library.base.json";
pub const FACTORY_FILE: &str = "example_library.factory.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceExample {
    pub id: String,
    pub scope: String,
    pub cues: Vec<String>,
    #[serde(default)]
    pub context_cues: Vec<String>,
    pub situation: String,
    #[serde(default)]
    pub attention: String,
    pub user: String,
    pub response: serde_json::Value,
    // Synthetic citations exist only in isolated unit tests, never in app data.
    #[cfg(test)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,
}

impl VoiceExample {
    /// A sourced utterance has no invented user turn. Keep the marker in the
    /// portable base array so it survives restarts and factory upgrades.
    pub fn is_fragment(&self) -> bool {
        self.id.starts_with("voice-") && self.user.is_empty()
    }

    pub fn is_approved_utterance(&self) -> bool {
        // The bundled corpus was verified offline. Match its id, user and exact
        // response instead of shipping research metadata with every utterance.
        static APPROVED: once_cell::sync::Lazy<BTreeSet<String>> = once_cell::sync::Lazy::new(|| {
            [include_str!("../../prompts/characters/nana/voice_fragments.json"),
             include_str!("../../prompts/characters/vivian/voice_fragments.json")]
                .into_iter().flat_map(|raw| serde_json::from_str::<Vec<VoiceExample>>(raw).expect("valid voice corpus"))
                .map(|entry| fingerprint(&(&entry.id, &entry.user, &entry.response))).collect()
        });
        if APPROVED.contains(&fingerprint(&(&self.id, &self.user, &self.response))) { return true; }
        #[cfg(test)]
        return self.has_fixture_source();
        #[cfg(not(test))]
        false
    }

    #[cfg(test)]
    fn has_fixture_source(&self) -> bool {
        let Some(source) = self.source.as_ref() else { return false; };
        if source["dataset"] != "unit-test fixtures (never shipped)" { return false; }
        let Some(text) = self.response["text"].as_str() else { return false; };
        ["dataset", "url", "locator", "snapshot_sha256"].iter()
            .all(|key| source[*key].as_str().is_some_and(|value| !value.trim().is_empty()))
            && source["snapshot_sha256"].as_str().is_some_and(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            && source["text_sha256"].as_str() == Some(format!("{:x}", Sha256::digest(text.as_bytes())).as_str())
    }
}

#[cfg(test)]
pub(crate) fn fixture_source(text: &str) -> serde_json::Value {
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    serde_json::json!({"dataset":"unit-test fixtures (never shipped)",
        "url":"https://example.invalid/unit-test-corpus", "locator":"fixture", "snapshot_sha256":hash, "text_sha256":hash})
}

pub fn fingerprint(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).unwrap_or_default())
    )
}

pub fn factory_examples(char_id: &str) -> Vec<VoiceExample> {
    let fragments = match char_id {
        "nana" => include_str!("../../prompts/characters/nana/voice_fragments.json"),
        "vivian" => include_str!("../../prompts/characters/vivian/voice_fragments.json"),
        _ => "[]",
    };
    let examples: Vec<VoiceExample> = serde_json::from_str(fragments).expect("valid sourced voice fragments");
    assert!(examples.iter().all(VoiceExample::is_approved_utterance), "factory utterances require dataset provenance");
    examples
}

/// Base arrays remain portable; a separate factory snapshot makes upgrades a
/// three-way merge instead of overwriting local edits or pinning factory bugs.
pub fn read_base_at(path: &std::path::Path) -> Result<Vec<VoiceExample>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取模仿语料基础库失败: {e}"))?;
    let entries: Vec<VoiceExample> =
        serde_json::from_slice(&bytes).map_err(|e| format!("解析模仿语料基础库失败: {e}"))?;
    let mut ids = BTreeSet::new();
    if entries.iter().any(|e| {
        e.id.is_empty()
            || !ids.insert(&e.id)
            || e.cues.is_empty()
            || !e.response["text"].is_string()
            || !e.response["intent"].is_string()
    }) {
        return Err("模仿语料基础库存在重复 ID 或不完整样例".into());
    }
    Ok(entries)
}

pub fn save_base_at(path: &std::path::Path, entries: &[VoiceExample]) -> Result<(), String> {
    let data = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    crate::utils::fs::write_atomic(path, &data).map_err(|e| format!("保存模仿语料基础库失败: {e}"))
}

pub fn ensure_base(char_id: &str) -> Result<Vec<VoiceExample>, String> {
    ensure_base_at(&crate::utils::path::get_character_data_dir(char_id).join("persona"), char_id)
}

pub fn base_examples(char_id: &str) -> Vec<VoiceExample> {
    match ensure_base(char_id) {
        Ok(entries) => entries.into_iter().filter(VoiceExample::is_approved_utterance).collect(),
        Err(error) => {
            tracing::warn!("[VoiceCorpus:{char_id}] {error}");
            factory_examples(char_id)
        }
    }
}

fn revision(example: &VoiceExample) -> String {
    fn canonical(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Array(items) => items.into_iter().map(canonical).collect(),
            serde_json::Value::Object(items) => {
                let sorted: std::collections::BTreeMap<_, _> = items.into_iter().collect();
                serde_json::Value::Object(sorted.into_iter().map(|(key, value)| (key, canonical(value))).collect())
            }
            other => other,
        }
    }
    fingerprint(&canonical(serde_json::to_value(example).expect("serializable example")))
}

fn legacy_revisions(char_id: &str) -> BTreeSet<String> {
    static KNOWN: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(||
        serde_json::from_str(include_str!("../../prompts/characters/example_revisions.json")).expect("factory revision index"));
    KNOWN[char_id].as_object().into_iter().flat_map(|entries| entries.values())
        .filter_map(serde_json::Value::as_array).flatten()
        .filter_map(serde_json::Value::as_str).map(str::to_string).collect()
}

fn merge_factory(base: &[VoiceExample], previous: Option<&[VoiceExample]>, factory: &[VoiceExample], legacy: &BTreeSet<String>) -> Vec<VoiceExample> {
    let mut merged = Vec::new();
    for current in base {
        let old = previous.and_then(|items| items.iter().find(|item| item.id == current.id));
        let untouched = old.map(|item| revision(item) == revision(current))
            .unwrap_or_else(|| previous.is_none() && legacy.contains(&revision(current)));
        match factory.iter().find(|item| item.id == current.id) {
            Some(new) if untouched => merged.push(new.clone()),
            None if untouched => {}, // A retired factory item, not a local addition.
            _ => merged.push(current.clone()),
        }
    }
    for new in factory {
        // A user deletion relative to a known factory snapshot stays deleted.
        if !base.iter().any(|item| item.id == new.id)
            && !previous.is_some_and(|items| items.iter().any(|item| item.id == new.id)) {
            merged.push(new.clone());
        }
    }
    merged
}

fn ensure_base_at(directory: &std::path::Path, char_id: &str) -> Result<Vec<VoiceExample>, String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let path = directory.join(BASE_FILE);
    let snapshot = directory.join(FACTORY_FILE);
    let factory = factory_examples(char_id);
    let base = if path.exists() { read_base_at(&path)? } else { factory.clone() };
    // A malformed version marker must never authorize replacing local entries.
    let previous = if snapshot.exists() { Some(read_base_at(&snapshot)?) } else { None };
    let merged = merge_factory(&base, previous.as_deref(), &factory, &legacy_revisions(char_id));
    if !path.exists() || fingerprint(&base) != fingerprint(&merged) { save_base_at(&path, &merged)?; }
    if previous.as_ref().is_none_or(|items| fingerprint(items) != fingerprint(&factory)) {
        save_base_at(&snapshot, &factory)?;
    }
    Ok(merged)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleEdit {
    pub id: String,
    /// Required for replace/remove; absent only for a new grown-* example.
    #[serde(default)]
    pub reference: Option<String>,
    /// null retires an existing example.
    #[serde(deserialize_with = "deserialize_example")]
    pub example: Option<VoiceExample>,
    pub reason: String,
}

fn deserialize_example<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<VoiceExample>, D::Error> {
    Option::<VoiceExample>::deserialize(deserializer)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleOverride {
    pub edit: ExampleEdit,
    pub scope: String,
    /// A learned interpretation owns its examples. Invalidating/replacing it also
    /// withdraws those examples, rather than leaving an obsolete voice behind.
    pub understanding: Option<String>,
    pub updated_at: f64,
}

impl ExampleOverride {
    pub fn applies(&self, learned: &[(String, String)]) -> bool {
        self.understanding.as_ref()
            == learned
                .iter()
                .find(|(scope, _)| scope == &self.scope)
                .map(|(_, text)| text)
    }
}

pub fn materialize(
    base: &[VoiceExample],
    overrides: &[ExampleOverride],
    learned: &[(String, String)],
) -> Vec<VoiceExample> {
    let mut entries: Vec<_> = base.iter().filter(|entry| entry.is_approved_utterance()).cloned().collect();
    for update in overrides.iter().filter(|update| update.applies(learned)) {
        // Old generated overrides must neither reach the diary audit nor hide a
        // valid dataset quote by replacing it before retrieval filters run.
        if update.edit.example.as_ref().is_some_and(|example| !example.is_approved_utterance()) { continue; }
        entries.retain(|entry| entry.id != update.edit.id);
        if let Some(example) = &update.edit.example {
            entries.push(example.clone());
        }
    }
    entries
}

fn bounded(s: &str, min: usize, max: usize) -> bool {
    let count = s.trim().chars().count();
    (min..=max).contains(&count) && !s.chars().any(|c| c.is_control() && !c.is_whitespace())
}

fn valid_example(e: &VoiceExample) -> bool {
    e.is_approved_utterance()
        && bounded(&e.id, 1, 80)
        && e.id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_:".contains(&c))
        && bounded(&e.scope, 1, 40)
        && e.scope
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        && bounded(&e.situation, 1, 240)
        && bounded(&e.attention, 0, 180)
        && bounded(&e.user, if e.is_fragment() { 0 } else { 1 }, 180)
        && (1..=8).contains(&e.cues.len())
        && e.context_cues.len() <= 8
        && e.cues
            .iter()
            .chain(&e.context_cues)
            .all(|cue| bounded(cue, 1, 60))
        && (e.is_fragment() || e.cues
            .iter()
            .any(|cue| e.user.to_lowercase().contains(&cue.to_lowercase())))
        && e.response
            .as_object()
            .is_some_and(|r| r.len() == 2 && r.contains_key("text") && r.contains_key("intent"))
        && e.response["intent"]
            .as_str()
            .is_some_and(|s| matches!(s, "reply" | "short_reply" | "no_reply"))
        && e.response["text"].as_str().is_some_and(|s| {
            bounded(
                s,
                if e.response["intent"] == "no_reply" {
                    0
                } else {
                    1
                },
                180,
            )
        })
}

/// Validate the entire small batch against the exact corpus the reviewer saw.
/// Never truncate a malformed example into a valid but semantically different one.
pub fn validate_edits(
    edits: &[ExampleEdit],
    current: &[VoiceExample],
    scope: Option<&str>,
) -> bool {
    let mut ids = BTreeSet::new();
    let mut responses = BTreeSet::new();
    edits.len() <= MAX_EDITS
        && edits.iter().all(|edit| {
            if !ids.insert(&edit.id) || !bounded(&edit.reason, 1, 240) {
                return false;
            }
            let previous = current.iter().find(|entry| entry.id == edit.id);
            match previous {
                Some(previous)
                    if edit.reference.as_deref() == Some(fingerprint(previous).as_str()) =>
                {
                    if scope.is_some_and(|scope| scope != previous.scope) {
                        return false;
                    }
                }
                None if edit.reference.as_deref().unwrap_or_default().is_empty()
                    && edit.id.starts_with("grown-")
                    && edit.example.is_some() => {}
                _ => return false,
            }
            let Some(example) = &edit.example else {
                return previous.is_some();
            };
            valid_example(example)
                && previous.is_none_or(|previous| previous.is_fragment() == example.is_fragment())
                // Diary review may refine retrieval guidance or retire a quote,
                // but must not silently rewrite the sourced wording.
                && previous.is_none_or(|previous| !previous.is_fragment() || previous.response == example.response)
                && example.id == edit.id
                && responses.insert(
                    example.response["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )
                && scope.is_none_or(|scope| scope == example.scope)
                && previous.is_none_or(|previous| previous.scope == example.scope)
                && !current.iter().any(|other| {
                    other.id != edit.id && other.response["text"] == example.response["text"]
                })
        })
}

/// Apply already validated edits to the same state file as the persona revision.
pub fn publish_edits(
    overrides: &mut Vec<ExampleOverride>,
    history: &mut Vec<ExampleOverride>,
    edits: &[ExampleEdit],
    current: &[VoiceExample],
    learned: &[(String, String)],
    now: f64,
) -> bool {
    let retires_generated =
        |edit: &ExampleEdit| edit.example.is_none() && edit.id.starts_with("grown-");
    let new_ids = edits
        .iter()
        .filter(|edit| {
            !retires_generated(edit) && !overrides.iter().any(|old| old.edit.id == edit.id)
        })
        .count();
    let retired = edits
        .iter()
        .filter(|edit| {
            retires_generated(edit) && overrides.iter().any(|old| old.edit.id == edit.id)
        })
        .count();
    if overrides.len() + new_ids - retired > MAX_OVERRIDES {
        return false;
    }
    for edit in edits {
        let scope = edit
            .example
            .as_ref()
            .or_else(|| current.iter().find(|e| e.id == edit.id))
            .expect("validated example or deletion target")
            .scope
            .clone();
        if let Some(index) = overrides.iter().position(|old| old.edit.id == edit.id) {
            history.push(overrides.remove(index));
        }
        let update = ExampleOverride {
            edit: edit.clone(),
            understanding: learned
                .iter()
                .find(|(s, _)| s == &scope)
                .map(|(_, text)| text.clone()),
            scope,
            updated_at: now,
        };
        if retires_generated(edit) {
            history.push(update);
        } else {
            overrides.push(update);
        }
    }
    if history.len() > 32 {
        history.drain(..history.len() - 32);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragments_survive_base_roundtrip_and_cannot_turn_into_paired_dialogue() {
        let fragment = factory_examples("vivian").into_iter().find(|e|e.is_fragment()).unwrap();
        assert!(valid_example(&fragment));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(BASE_FILE);
        save_base_at(&path, &[fragment.clone()]).unwrap();
        let restored = read_base_at(&path).unwrap();
        assert!(restored[0].is_fragment());
        assert_eq!(restored[0].response, fragment.response);
        assert_eq!(restored[0].source, fragment.source);
        let mut fabricated = fragment.clone();
        fabricated.user = fabricated.cues[0].clone();
        let edit = ExampleEdit {id:fragment.id.clone(),reference:Some(fingerprint(&fragment)),
            reason:"不能凭空生成问答".into(),example:Some(fabricated)};
        assert!(!validate_edits(&[edit], &[fragment], None));
    }
    #[test]
    fn legacy_unattributed_cache_entries_are_not_a_runtime_corpus() {
        let char_id = format!("legacy-source-test-{}", uuid::Uuid::new_v4());
        let path = crate::utils::path::get_character_data_dir(&char_id).join("persona").join(BASE_FILE);
        let mut legacy = sample("old-authored-example");
        legacy.source = None;
        save_base_at(&path, &[legacy]).unwrap();
        assert!(base_examples(&char_id).is_empty());
        assert_eq!(read_base_at(&path).unwrap().len(), 1, "do not erase an unrecognized user archive");
    }
    #[test]
    fn diary_review_cannot_rephrase_sourced_utterances() {
        let original = factory_examples("nana").into_iter().find(|e|e.is_fragment()).unwrap();
        let mut changed = original.clone();
        changed.response["text"] = "模型自行润色的回应".into();
        let mut edit = ExampleEdit {id:original.id.clone(),reference:Some(fingerprint(&original)),
            reason:"改写来源台词".into(),example:Some(changed)};
        assert!(!validate_edits(&[edit.clone()], &[original.clone()], None));
        edit.example = None;
        assert!(validate_edits(&[edit], &[original], None));
    }
    #[test]
    fn old_generated_override_cannot_hide_or_rewrite_a_sourced_quote() {
        let original = factory_examples("vivian").into_iter().next().unwrap();
        let mut invented = original.clone();
        invented.response["text"] = "历史版本生成的台词".into();
        invented.source = None;
        let update = ExampleOverride {scope:original.scope.clone(),understanding:None,updated_at:1.0,
            edit:ExampleEdit {id:original.id.clone(),reference:None,reason:"旧缓存".into(),example:Some(invented)}};
        let restored = materialize(&[original.clone()], &[update], &[]);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].response, original.response);
        assert_eq!(restored[0].source, original.source);
    }
    #[test]
    fn factory_upgrade_preserves_local_edits_deletions_and_learned_overrides() {
        let old = sample("factory");
        let edited = sample("edited");
        let retired = sample("retired");
        let deleted = sample("deleted");
        let custom = sample("custom");
        let mut local = edited.clone(); local.response["text"] = "用户保留的独立表达".into();
        let mut updated = old.clone(); updated.attention = "新版判断".into();
        let added = sample("added");
        let merged = merge_factory(&[old.clone(), local.clone(), retired.clone(), custom.clone()],
            Some(&[old, edited.clone(), retired, deleted.clone()]),
            &[updated.clone(), edited, deleted, added], &BTreeSet::new());
        assert_eq!(merged.iter().find(|item| item.id == "factory").unwrap().attention, "新版判断");
        assert_eq!(merged.iter().find(|item| item.id == "edited").unwrap().response, local.response);
        assert!(merged.iter().any(|item| item.id == "custom"));
        assert!(merged.iter().any(|item| item.id == "added"));
        assert!(merged.iter().all(|item| !["retired", "deleted"].contains(&item.id.as_str())));
        let override_entry = sample("factory");
        let overrides = [ExampleOverride { edit: ExampleEdit { id:"factory".into(), reference:Some(fingerprint(&updated)),
            example:Some(override_entry.clone()), reason:"学习后的表达".into() }, scope:"comfort".into(), understanding:None, updated_at:1.0 }];
        assert_eq!(materialize(&merged, &overrides, &[]).iter().find(|item| item.id == "factory").unwrap().response, override_entry.response);
    }

    #[test]
    fn legacy_upgrade_only_replaces_known_factory_revisions() {
        let old = sample("known");
        let mut latest = old.clone(); latest.response["intent"] = "short_reply".into();
        let mut local = sample("customized"); local.attention = "本地修改".into();
        let known = BTreeSet::from([revision(&old)]);
        let merged = merge_factory(&[old, local.clone()], None, &[latest.clone(), sample("customized")], &known);
        assert_eq!(merged[0].response, latest.response);
        assert_eq!(merged[1].attention, local.attention);
    }

    #[test]
    fn factory_snapshot_migration_is_idempotent_and_preserves_local_changes() {
        let directory = tempfile::tempdir().unwrap();
        let base = ensure_base_at(directory.path(), "vivian").unwrap();
        let path = directory.path().join(BASE_FILE);
        let before = std::fs::read(&path).unwrap();
        ensure_base_at(directory.path(), "vivian").unwrap();
        assert_eq!(before, std::fs::read(&path).unwrap());
        let mut custom = base; custom[0].response["text"] = "本地保留的回复".into();
        save_base_at(&path, &custom).unwrap();
        assert_eq!(ensure_base_at(directory.path(), "vivian").unwrap()[0].response, custom[0].response);
    }
    pub(super) fn sample(id: &str) -> VoiceExample {
        serde_json::from_value(
            serde_json::json!({"id":id,"scope":"comfort","cues":["不想讲"],
            "situation":"虚构示例：对方疲惫，不想复述事情。","user":"今天不想讲了",
            "response":{"text":"那就歇会儿，想说了我再听。","intent":"reply"},
            "source":fixture_source("那就歇会儿，想说了我再听。")}),
        )
        .unwrap()
    }
    #[test]
    fn edits_require_current_references_and_preserve_scope() {
        let old = sample("old");
        let mut edit = ExampleEdit {
            id: old.id.clone(),
            reference: Some(fingerprint(&old)),
            reason: "原示例不符合倾听的判断".into(),
            example: Some(old.clone()),
        };
        assert!(validate_edits(
            &[edit.clone()],
            &[old.clone()],
            Some("comfort")
        ));
        edit.reference = Some("outdated".into());
        assert!(!validate_edits(&[edit.clone()], &[old.clone()], None));
        edit.reference = Some(fingerprint(&old));
        edit.example.as_mut().unwrap().scope = "daily".into();
        assert!(!validate_edits(&[edit], &[old], None));
    }
    #[test]
    fn new_samples_are_bounded_distinct_and_match_their_cues() {
        let mut edit = ExampleEdit {
            id: "grown-listen".into(),
            reference: None,
            reason: "补充不追问的表达".into(),
            example: Some(sample("grown-listen")),
        };
        assert!(validate_edits(&[edit.clone()], &[], None));
        let mut other = edit.clone();
        other.id = "grown-copy".into();
        other.example.as_mut().unwrap().id = other.id.clone();
        assert!(!validate_edits(&[edit.clone(), other], &[], None));
        edit.example.as_mut().unwrap().response["text"] = "长".repeat(181).into();
        edit.example.as_mut().unwrap().source = Some(fixture_source(&"长".repeat(181)));
        assert!(!validate_edits(&[edit.clone()], &[], None));
        edit.example.as_mut().unwrap().response["text"] = "短句".into();
        edit.example.as_mut().unwrap().source = Some(fixture_source("短句"));
        edit.example.as_mut().unwrap().cues = vec!["不存在的匹配词".into()];
        assert!(!validate_edits(&[edit], &[], None));
    }
    #[test]
    fn superseded_interpretations_withdraw_their_samples() {
        let old = sample("old");
        let edit = ExampleEdit {
            id: old.id.clone(),
            reference: Some(fingerprint(&old)),
            reason: "不再适用".into(),
            example: None,
        };
        let update = ExampleOverride {
            edit,
            scope: "comfort".into(),
            understanding: Some("先听".into()),
            updated_at: 1.0,
        };
        assert!(materialize(
            &[old.clone()],
            &[update.clone()],
            &[("comfort".into(), "先听".into())]
        )
        .is_empty());
        assert_eq!(
            materialize(&[old], &[update], &[("comfort".into(), "新的判断".into())]).len(),
            1
        );
    }
    #[test]
    fn retiring_generated_samples_frees_capacity_for_new_expressions() {
        let mut overrides: Vec<_> = (0..MAX_OVERRIDES)
            .map(|index| {
                let mut e = sample(&format!("grown-{index}"));
                e.response["text"] = format!("独立表达{index}").into();
                e.source = Some(fixture_source(&format!("独立表达{index}")));
                ExampleOverride {
                    edit: ExampleEdit {
                        id: e.id.clone(),
                        reference: None,
                        reason: "补充表达".into(),
                        example: Some(e),
                    },
                    scope: "comfort".into(),
                    understanding: None,
                    updated_at: 1.0,
                }
            })
            .collect();
        let current = materialize(&[], &overrides, &[]);
        let edits = vec![
            ExampleEdit {
                id: current[0].id.clone(),
                reference: Some(fingerprint(&current[0])),
                reason: "淘汰重复场景".into(),
                example: None,
            },
            ExampleEdit {
                id: "grown-new".into(),
                reference: None,
                reason: "补充缺少的场景".into(),
                example: Some(sample("grown-new")),
            },
        ];
        assert!(validate_edits(&edits, &current, None));
        assert!(publish_edits(
            &mut overrides,
            &mut vec![],
            &edits,
            &current,
            &[],
            2.0
        ));
        assert_eq!(overrides.len(), MAX_OVERRIDES);
        assert!(!overrides.iter().any(|o| o.edit.id == "grown-0"));
        assert!(overrides.iter().any(|o| o.edit.id == "grown-new"));
    }
}
