//! Atomic individual histories plus a lightweight versioned catalog.
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Serialize, Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    revision: u64,
    version: u32,
    sessions: BTreeMap<String, Value>,
    #[serde(default)]
    deleted: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct Patch {
    version: u32,
    revision: u64,
    upserts: BTreeMap<String, Value>,
    deleted: BTreeSet<String>,
}

fn journal(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    match std::fs::read_dir(dir.join("index-journal")) {
        Ok(entries) => {
            let mut files = entries
                .map(|e| e.map(|v| v.path()).map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            files.retain(|p| p.extension().is_some_and(|e| e == "json"));
            files.sort();
            Ok(files)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

fn patch_revision(path: &Path) -> Result<u64, String> {
    path.file_stem()
        .and_then(|v| v.to_str())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| "无效的会话索引日志文件名".into())
}

fn replay(dir: &Path, manifest: &mut Manifest, recovery: bool) -> Result<(), String> {
    for path in journal(dir)? {
        let revision = patch_revision(&path)?;
        if revision <= manifest.revision {
            continue;
        }
        let patch: Patch = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("会话索引日志损坏，未忽略更新: {e}"))?;
        if patch.version != 1
            || patch.revision != revision
            || (!recovery && revision != manifest.revision + 1)
        {
            return Err("会话索引日志版本或顺序无效，未覆盖历史".into());
        }
        for id in patch.deleted {
            manifest.sessions.remove(&id);
            manifest.deleted.insert(id);
        }
        for (id, value) in patch.upserts {
            manifest.deleted.remove(&id);
            manifest.sessions.insert(id, value);
        }
        manifest.revision = revision;
    }
    Ok(())
}

fn commit_index(dir: &Path, previous: &Manifest, mut next: Manifest) -> Result<(), String> {
    if !dir.join("index.json").exists() {
        crate::utils::fs::write_atomic(
            &dir.join("deleted.json"),
            &serde_json::to_string(&next.deleted).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        return write_index(dir, &next);
    }
    let upserts: BTreeMap<_, _> = next
        .sessions
        .iter()
        .filter(|(id, v)| previous.sessions.get(*id) != Some(*v))
        .map(|(id, v)| (id.clone(), v.clone()))
        .collect();
    let deleted: BTreeSet<_> = next
        .deleted
        .difference(&previous.deleted)
        .cloned()
        .collect();
    if upserts.is_empty() && deleted.is_empty() {
        return Ok(());
    }
    next.revision = next.revision.checked_add(1).ok_or("会话索引版本溢出")?;
    let patch = Patch {
        version: 1,
        revision: next.revision,
        upserts,
        deleted,
    };
    crate::utils::fs::write_atomic(
        &dir.join("index-journal")
            .join(format!("{:020}.json", next.revision)),
        &serde_json::to_string(&patch).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let files = journal(dir)?;
    let base: Manifest =
        serde_json::from_slice(&std::fs::read(dir.join("index.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let active = files
        .iter()
        .filter(|p| patch_revision(p).is_ok_and(|r| r > base.revision))
        .count();
    let active_bytes: u64 = files
        .iter()
        .filter(|p| patch_revision(p).is_ok_and(|r| r > base.revision))
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();
    if active >= 128 || active_bytes >= 2 * 1024 * 1024 {
        crate::utils::fs::write_atomic(
            &dir.join("deleted.json"),
            &serde_json::to_string(&next.deleted).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        write_index(dir, &next)?;
        // Keep patches needed to replay the previous checkpoint after corruption.
        for path in files {
            if patch_revision(&path)? <= base.revision {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Index {
    Current(Manifest),
    Legacy(Vec<String>),
}

fn filename(id: &str) -> String {
    format!(
        "{}.json",
        id.as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn head(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        if object.get("history_loaded").and_then(Value::as_bool) != Some(false) {
            let count = object
                .get("messages")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let preview = object
                .get("messages")
                .and_then(Value::as_array)
                .and_then(|messages| {
                    messages.iter().rev().find(|m| {
                        m.get("role").and_then(Value::as_str) == Some("assistant")
                            && m.get("content")
                                .and_then(Value::as_str)
                                .is_some_and(|text| !text.trim().is_empty())
                    })
                })
                .and_then(|m| m.get("content").and_then(Value::as_str))
                .map(|text| text.chars().take(1000).collect::<String>());
            object.insert(
                "last_reply_preview".into(),
                serde_json::to_value(preview).unwrap(),
            );
            object.insert("stored_message_count".into(), count.into());
        }
        object.insert("messages".into(), serde_json::json!([]));
        object.insert("history_loaded".into(), false.into());
        for key in [
            "input_queue",
            "file_changes",
            "message_changes",
            "message_feedback",
            "deliverables",
            "feedback",
            "compacted",
            "goal",
            "plan",
            "work_todos",
        ] {
            object.remove(key);
        }
    }
    value
}

fn write_index(dir: &Path, manifest: &Manifest) -> Result<(), String> {
    let index = dir.join("index.json");
    if let Ok(previous) = std::fs::read_to_string(&index) {
        crate::utils::fs::write_atomic(&dir.join("index.previous.json"), &previous)
            .map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string(manifest).map_err(|e| e.to_string())?;
    crate::utils::fs::write_atomic(&index, &text).map_err(|e| e.to_string())?;
    crate::utils::fs::write_atomic(&dir.join("migration_complete"), "1").map_err(|e| e.to_string())
}

fn base_unlocked(dir: &Path) -> Result<Option<Manifest>, String> {
    let index: Option<Index> = crate::utils::fs::load_json_or_backup(&dir.join("index.json"))
        .or_else(|| crate::utils::fs::load_json_or_backup(&dir.join("index.previous.json")));
    let deleted: BTreeSet<String> = match std::fs::read(dir.join("deleted.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("会话删除记录损坏，未恢复已删除历史: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
        Err(e) => return Err(e.to_string()),
    };
    match index {
        Some(Index::Current(mut manifest)) if manifest.version == 2 => {
            manifest.deleted.extend(deleted);
            manifest
                .sessions
                .retain(|id, _| !manifest.deleted.contains(id));
            Ok(Some(manifest))
        }
        Some(Index::Current(_)) => Err("不支持的会话索引版本，未覆盖历史".into()),
        legacy => {
            let ids = match legacy {
                Some(Index::Legacy(ids)) => ids,
                None if !dir.join("migration_complete").exists() => return Ok(None),
                None => {
                    let files = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
                    files
                        .flatten()
                        .filter_map(|file| {
                            let name = file.file_name().to_string_lossy().to_string();
                            let stem = name.strip_suffix(".json")?;
                            if stem.is_empty() || stem.len() % 2 != 0 {
                                return None;
                            }
                            let bytes: Option<Vec<_>> = (0..stem.len())
                                .step_by(2)
                                .map(|i| u8::from_str_radix(stem.get(i..i + 2)?, 16).ok())
                                .collect();
                            String::from_utf8(bytes?).ok()
                        })
                        .collect()
                }
                _ => unreachable!(),
            };
            // One-time upgrade/recovery scans files individually, retaining only
            // their headers. Normal startup never reads archived history here.
            let mut manifest = Manifest {
                version: 2,
                deleted,
                ..Default::default()
            };
            for id in ids {
                if manifest.deleted.contains(&id) {
                    continue;
                }
                if let Some(value) = read_one::<Value>(dir, &id) {
                    manifest.sessions.insert(id, head(value));
                }
            }
            replay(dir, &mut manifest, true)?;
            write_index(dir, &manifest)?;
            Ok(Some(manifest))
        }
    }
}

fn index_unlocked(dir: &Path) -> Result<Option<Manifest>, String> {
    let Some(mut manifest) = base_unlocked(dir)? else {
        return Ok(None);
    };
    replay(dir, &mut manifest, false)?;
    Ok(Some(manifest))
}

pub fn load_heads<T: DeserializeOwned>(dir: &Path) -> Result<Option<Vec<(String, T)>>, String> {
    super::coding_memory_persistence::transaction(&dir.join("index.json"), || {
        let Some(manifest) = index_unlocked(dir)? else {
            return Ok(None);
        };
        manifest
            .sessions
            .into_iter()
            .map(|(id, value)| {
                let parsed = serde_json::from_value(value)
                    .map_err(|e| format!("会话索引 {id} 无效: {e}"))?;
                Ok((id, parsed))
            })
            .collect::<Result<Vec<_>, String>>()
            .map(Some)
    })
}

pub fn read_one<T: DeserializeOwned>(dir: &Path, id: &str) -> Option<T> {
    crate::utils::fs::load_json_or_backup(&body_path(dir, id))
}

pub fn body_path(dir: &Path, id: &str) -> std::path::PathBuf {
    dir.join(filename(id))
}

fn save_unlocked<T: Serialize>(
    dir: &Path,
    mut manifest: Manifest,
    loaded: Vec<(String, T)>,
    heads: Vec<(String, T)>,
    deleted: Vec<String>,
) -> Result<(), String> {
    let previous = manifest.clone();
    for (id, summary) in heads {
        if manifest.deleted.contains(&id) {
            continue;
        }
        manifest.sessions.insert(
            id,
            head(serde_json::to_value(summary).map_err(|e| e.to_string())?),
        );
    }
    for (id, session) in loaded {
        let value = serde_json::to_value(session).map_err(|e| e.to_string())?;
        let text = serde_json::to_string(&value).map_err(|e| e.to_string())?;
        let path = dir.join(filename(&id));
        if std::fs::read_to_string(&path).ok().as_deref() != Some(&text) {
            crate::utils::fs::write_atomic(&path, &text).map_err(|e| e.to_string())?;
        }
        manifest.deleted.remove(&id);
        manifest.sessions.insert(id, head(value));
    }
    for id in &deleted {
        manifest.sessions.remove(id);
        manifest.deleted.insert(id.clone());
    }
    manifest.version = 2;
    commit_index(dir, &previous, manifest)?;
    for id in deleted {
        let path = dir.join(filename(&id));
        if path.exists() {
            if let Err(e) = std::fs::remove_file(path) {
                tracing::warn!("[CodingSession] 清理已删除会话失败: {e}");
            }
        }
    }
    Ok(())
}

/// Only cached histories are saved; missing unloaded rows are never deletions.
pub fn save_cached<T: Serialize>(
    dir: &Path,
    collect: impl FnOnce() -> (Vec<(String, T)>, Vec<(String, T)>, Vec<String>),
) -> Result<(), String> {
    super::coding_memory_persistence::transaction(&dir.join("index.json"), || {
        let manifest = index_unlocked(dir)?.unwrap_or_else(|| Manifest {
            version: 2,
            ..Default::default()
        });
        let (loaded, heads, deleted) = collect();
        save_unlocked(dir, manifest, loaded, heads, deleted)
    })
}

/// Full snapshot entry point for legacy migration and portable callers.
pub fn save<T: Serialize>(
    dir: &Path,
    collect: impl FnOnce() -> Vec<(String, T)>,
) -> Result<(), String> {
    super::coding_memory_persistence::transaction(&dir.join("index.json"), || {
        let manifest = index_unlocked(dir)?.unwrap_or_else(|| Manifest {
            version: 2,
            ..Default::default()
        });
        let loaded = collect();
        let retained: BTreeSet<_> = loaded.iter().map(|(id, _)| id.clone()).collect();
        let deleted = manifest
            .sessions
            .keys()
            .filter(|id| !retained.contains(*id))
            .cloned()
            .collect();
        save_unlocked(dir, manifest, loaded, Vec::new(), deleted)
    })
}

pub fn load<T: DeserializeOwned>(dir: &Path) -> Option<Vec<T>> {
    let heads = load_heads::<Value>(dir).ok()??;
    Some(
        heads
            .into_iter()
            .filter_map(|(id, _)| read_one(dir, &id))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_attachments_stay_in_the_history_body_and_out_of_sidebar_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let payload = serde_json::json!({"session_id":"queued","messages":[],"input_queue":{
            "revision":3,"paused":true,"recent_ids":["accepted"],"items":[{"id":"accepted","payload":{"images":[{"data":"IMAGE_BYTES"}],"content":"pending"}}]
        }});
        save(dir.path(), || vec![("queued".into(), payload.clone())]).unwrap();
        let body: Value = read_one(dir.path(), "queued").unwrap();
        assert_eq!(body["input_queue"], payload["input_queue"]);
        let heads = load_heads::<Value>(dir.path()).unwrap().unwrap();
        assert!(heads[0].1.get("input_queue").is_none());
        assert!(!std::fs::read_to_string(dir.path().join("index.json")).unwrap().contains("IMAGE_BYTES"));
    }
    #[test]
    fn delta_index_preserves_checkpoints_and_rejects_damaged_journal() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), || {
            vec![(
                "a".into(),
                serde_json::json!({"title":"first","messages":[]}),
            )]
        })
        .unwrap();
        let checkpoint = std::fs::read(dir.path().join("index.json")).unwrap();
        let mut heads = load_heads::<Value>(dir.path()).unwrap().unwrap();
        save_cached(dir.path(), || {
            (Vec::<(String, Value)>::new(), heads.clone(), vec![])
        })
        .unwrap();
        assert!(journal(dir.path()).unwrap().is_empty());
        heads[0].1["title"] = "changed".into();
        save_cached(dir.path(), || (vec![], heads, vec![])).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("index.json")).unwrap(),
            checkpoint
        );
        assert_eq!(
            load_heads::<Value>(dir.path()).unwrap().unwrap()[0].1["title"],
            "changed"
        );
        std::fs::write(&journal(dir.path()).unwrap()[0], "broken journal").unwrap();
        assert!(load_heads::<Value>(dir.path()).is_err());
    }
    #[test]
    fn delta_compaction_recovery_keeps_tombstones_and_unopened_history() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), || {
            vec![
                (
                    "a".into(),
                    serde_json::json!({"messages":[{"content":"original"}]}),
                ),
                ("deleted".into(), serde_json::json!({"messages":[]})),
            ]
        })
        .unwrap();
        let history = std::fs::read(body_path(dir.path(), "a")).unwrap();
        let deleted = std::fs::read(body_path(dir.path(), "deleted")).unwrap();
        save_cached(dir.path(), || {
            (
                Vec::<(String, Value)>::new(),
                vec![],
                vec!["deleted".into()],
            )
        })
        .unwrap();
        for i in 0..270 {
            let mut heads = load_heads::<Value>(dir.path()).unwrap().unwrap();
            heads[0].1["title"] = format!("title-{i}").into();
            save_cached(dir.path(), || (vec![], heads, vec![])).unwrap();
        }
        assert!(journal(dir.path()).unwrap().len() <= 256);
        assert_eq!(std::fs::read(body_path(dir.path(), "a")).unwrap(), history);
        std::fs::write(body_path(dir.path(), "deleted"), deleted).unwrap();
        std::fs::write(dir.path().join("index.json"), "damaged").unwrap();
        let heads = load_heads::<Value>(dir.path()).unwrap().unwrap();
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].1["title"], "title-269");
        std::fs::write(dir.path().join("index.json"), "damaged").unwrap();
        std::fs::write(dir.path().join("index.previous.json"), "damaged").unwrap();
        assert_eq!(load_heads::<Value>(dir.path()).unwrap().unwrap().len(), 1);
    }
    #[test]
    fn cached_saves_preserve_unopened_archives_and_explicit_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let body = "经验".repeat(8192);
        save(dir.path(), || (0..75).map(|i| (format!("{i:02}"), serde_json::json!({
            "session_id": format!("{i:02}"), "messages": [{"role": "assistant", "content": body}], "goal": "keep goal", "context_start": 1
        }))).collect()).unwrap();
        let heads = load_heads::<Value>(dir.path()).unwrap().unwrap();
        assert_eq!(heads.len(), 75);
        assert!(heads
            .iter()
            .all(|(_, value)| value["messages"].as_array().unwrap().is_empty()));
        let archived_path = dir.path().join(filename("00"));
        let archived_before = std::fs::read(&archived_path).unwrap();
        let loaded: Vec<_> = (45..75)
            .map(|i| {
                let id = format!("{i:02}");
                (id.clone(), read_one::<Value>(dir.path(), &id).unwrap())
            })
            .collect();
        save_cached(dir.path(), || (loaded, heads, vec!["01".into()])).unwrap();
        assert_eq!(std::fs::read(&archived_path).unwrap(), archived_before);
        assert_eq!(
            read_one::<Value>(dir.path(), "00").unwrap()["goal"],
            "keep goal"
        );
        assert_eq!(load_heads::<Value>(dir.path()).unwrap().unwrap().len(), 74);
        assert!(!dir.path().join(filename("01")).exists());
        let index_size = std::fs::metadata(dir.path().join("index.json"))
            .unwrap()
            .len();
        assert!(index_size < archived_before.len() as u64 * 75 / 10);
    }

    #[test]
    fn legacy_id_index_is_upgraded_once_to_lightweight_headers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(filename("old")),
            r#"{"messages":[{"content":"original"}]}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("index.json"), r#"["old"]"#).unwrap();
        let heads = load_heads::<Value>(dir.path()).unwrap().unwrap();
        assert_eq!(heads[0].1["stored_message_count"], 1);
        assert_eq!(heads[0].1["history_loaded"], false);
        assert_eq!(
            read_one::<Value>(dir.path(), "old").unwrap()["messages"][0]["content"],
            "original"
        );
        let index: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("index.json")).unwrap())
                .unwrap();
        assert_eq!(index["version"], 2);
    }

    #[test]
    fn corrupted_indexes_recover_individual_files_without_restoring_deleted_sessions() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), || vec![("old".into(), 1), ("kept".into(), 2)]).unwrap();
        save(dir.path(), || vec![("kept".into(), 3)]).unwrap();
        std::fs::write(dir.path().join("index.json"), "broken").unwrap();
        std::fs::write(dir.path().join("index.previous.json"), "broken").unwrap();
        assert_eq!(load::<usize>(dir.path()).unwrap(), vec![3]);
    }
    #[test]
    fn more_than_thirty_sessions_survive_and_deleted_orphans_do_not_return() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), || {
            (0..45).map(|i| (format!("session-{i}"), i)).collect()
        })
        .unwrap();
        assert_eq!(load::<usize>(dir.path()).unwrap().len(), 45);
        save(dir.path(), || vec![("session-44".into(), 99)]).unwrap();
        assert_eq!(load::<usize>(dir.path()).unwrap(), vec![99]);
    }
    #[test]
    fn uncommitted_files_and_unsafe_ids_cannot_affect_index_recovery() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), || vec![("../unsafe".into(), "saved")]).unwrap();
        std::fs::write(dir.path().join("orphan.json"), "\"not committed\"").unwrap();
        assert_eq!(load::<String>(dir.path()).unwrap(), vec!["saved"]);
    }
}
