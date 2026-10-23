//! Lazy request snapshots for the workbench Workflow view. Kept outside session catalogs.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};

static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowToolCall {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRequestSummary {
    pub id: String,
    pub turn_message_index: usize,
    pub turn: usize,
    pub step: usize,
    pub attempt: usize,
    pub purpose: String,
    /// Configured route label; provider fallback may select another model.
    pub model: String,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<u64>,
    pub status: String,
    pub response_preview: String,
    pub tool_calls: Vec<WorkflowToolCall>,
    pub usage: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRequestDetail {
    pub summary: WorkflowRequestSummary,
    /// The agent input before router repair and provider-specific transformations.
    pub capture_stage: String,
    pub system: Vec<Value>,
    pub messages: Value,
    pub tools: Value,
    pub reasoning: Value,
    pub response: Value,
}

fn encoded(value: &str) -> String {
    value.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn session_dir(root: &Path, session_id: &str) -> PathBuf {
    root.join(encoded(session_id))
}

fn paths(root: &Path, session_id: &str, request_id: &str) -> (PathBuf, PathBuf) {
    let dir = session_dir(root, session_id);
    let name = encoded(request_id);
    (dir.join(format!("{name}.json")), dir.join(format!("{name}.meta.json")))
}

fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    crate::utils::fs::atomic_write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write detail first so a published summary always has a readable snapshot.
pub fn save(root: &Path, session_id: &str, detail: &WorkflowRequestDetail) -> Result<(), String> {
    let _guard = WRITER.lock().map_err(|e| e.to_string())?;
    save_unlocked(root, session_id, detail)
}

fn save_unlocked(root: &Path, session_id: &str, detail: &WorkflowRequestDetail) -> Result<(), String> {
    let (body, meta) = paths(root, session_id, &detail.summary.id);
    let assets = session_dir(root, session_id).join("assets");
    let mut stored = serde_json::to_value(detail).map_err(|e| e.to_string())?;
    stored.as_object_mut().unwrap().remove("system");
    let tools_ref = save_asset(&assets, &serde_json::to_vec(&detail.tools).map_err(|e| e.to_string())?)?;
    stored["tools"] = Value::Null;
    stored["tools_ref"] = tools_ref.into();
    externalize_images(&assets, &mut stored["messages"])?;
    write(&body, &stored)?;
    write(&meta, &detail.summary)
}

fn save_asset(root: &Path, bytes: &[u8]) -> Result<String, String> {
    let id = format!("{:x}", Sha256::digest(bytes));
    let path = root.join(&id);
    if !path.exists() { crate::utils::fs::atomic_write(&path, bytes).map_err(|e| e.to_string())?; }
    Ok(id)
}
fn read_asset(root: &Path, id: &str) -> Result<Vec<u8>, String> {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) { return Err("Invalid workflow asset reference".into()); }
    let bytes = std::fs::read(root.join(id)).map_err(|e| e.to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != id { return Err("Workflow asset checksum mismatch".into()); }
    Ok(bytes)
}
fn externalize_images(root: &Path, value: &mut Value) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            if fields.get("media_type").and_then(Value::as_str).is_some_and(|s| s.starts_with("image/")) {
                if let Some(data) = fields.remove("data") {
                    if let Some(text) = data.as_str() {
                        fields.insert("data_ref".into(), save_asset(root, text.as_bytes())?.into());
                    } else { fields.insert("data".into(), data); }
                }
            }
            for child in fields.values_mut() { externalize_images(root, child)?; }
        }
        Value::Array(items) => for child in items { externalize_images(root, child)?; },
        _ => {}
    }
    Ok(())
}
fn restore_images(root: &Path, value: &mut Value) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            if fields.get("media_type").and_then(Value::as_str).is_some_and(|s| s.starts_with("image/")) {
                if let Some(id) = fields.remove("data_ref") {
                    let bytes = read_asset(root, id.as_str().ok_or("Invalid image reference")?)?;
                    fields.insert("data".into(), String::from_utf8(bytes).map_err(|e| e.to_string())?.into());
                }
            }
            for child in fields.values_mut() { restore_images(root, child)?; }
        }
        Value::Array(items) => for child in items { restore_images(root, child)?; },
        _ => {}
    }
    Ok(())
}

fn result_path(root: &Path, session_id: &str, request_id: &str) -> PathBuf {
    session_dir(root, session_id).join(format!("{}.result.json", encoded(request_id)))
}

/// A late completion must not recreate snapshots after session deletion.
pub fn settle(root: &Path, session_id: &str, detail: &WorkflowRequestDetail) -> Result<(), String> {
    let _guard = WRITER.lock().map_err(|e| e.to_string())?;
    if !paths(root, session_id, &detail.summary.id).0.exists() { return Ok(()); }
    write(&result_path(root, session_id, &detail.summary.id), &serde_json::json!({
        "summary": detail.summary, "response": detail.response,
    }))?;
    write(&paths(root, session_id, &detail.summary.id).1, &detail.summary)
}

pub fn remove_session(root: &Path, session_id: &str) -> Result<(), String> {
    let _guard = WRITER.lock().map_err(|e| e.to_string())?;
    match std::fs::remove_dir_all(session_dir(root, session_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn detail(root: &Path, session_id: &str, request_id: &str) -> Result<WorkflowRequestDetail, String> {
    let mut value: Value = read(&paths(root, session_id, request_id).0)?;
    let assets = session_dir(root, session_id).join("assets");
    if let Some(id) = value.get("tools_ref").and_then(Value::as_str) {
        value["tools"] = serde_json::from_slice(&read_asset(&assets, id)?).map_err(|e| e.to_string())?;
    }
    restore_images(&assets, &mut value["messages"])?;
    if value.get("system").is_none() {
        value["system"] = Value::Array(value["messages"].as_array().into_iter().flatten()
            .filter(|m| m["role"] == "system").cloned().collect());
    }
    let settled = result_path(root, session_id, request_id);
    if settled.exists() {
        let result: Value = read(&settled)?;
        value["summary"] = result["summary"].clone(); value["response"] = result["response"].clone();
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Listing reads only small metadata files, never message histories or tool schemas.
pub fn list(root: &Path, session_id: &str) -> Result<Vec<WorkflowRequestSummary>, String> {
    let dir = session_dir(root, session_id);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut requests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().ends_with(".meta.json") {
            requests.push(read::<WorkflowRequestSummary>(&entry.path())?);
        }
    }
    requests.sort_by(|a, b| a.started_at.cmp(&b.started_at).then_with(|| a.id.cmp(&b.id)));
    Ok(requests)
}

/// A process loss can leave an unsettled request; readers must not show it running forever.
pub fn recover_summary(summary: &mut WorkflowRequestSummary, session_running: bool) {
    if !session_running && summary.status == "running" {
        summary.status = "interrupted".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(id: &str) -> WorkflowRequestDetail {
        WorkflowRequestDetail {
            summary: WorkflowRequestSummary {
                id: id.into(), turn_message_index: 0, turn: 0, step: 1, attempt: 1,
                purpose: "assistant".into(), model: "test".into(), started_at: 100,
                completed_at: None, duration_ms: None, status: "running".into(),
                response_preview: String::new(), tool_calls: vec![], usage: None, error: None,
            },
            capture_stage: "agent_input".into(), system: vec![json!({"content":"system"})],
            messages: json!([{"role":"user","content":"hello"}]), tools: json!([]),
            reasoning: Value::Null, response: Value::Null,
        }
    }

    #[test]
    fn input_is_immutable_on_settlement_and_shared_assets_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let mut first = request("first");
        first.messages = json!([{"role":"system","content":"fixed"},
            {"role":"user","content":"look","images":[{"media_type":"image/png","data":"a".repeat(10000)}]}]);
        first.tools = json!([{"name":"read","parameters":{"type":"object"}}]);
        save(root.path(), "session", &first).unwrap();
        let input = std::fs::read(paths(root.path(), "session", "first").0).unwrap();
        assert!(!String::from_utf8_lossy(&input).contains(&"a".repeat(1000)));
        first.summary.status = "complete".into(); first.response = json!({"content":"done"});
        settle(root.path(), "session", &first).unwrap();
        assert_eq!(std::fs::read(paths(root.path(), "session", "first").0).unwrap(), input);
        let restored = detail(root.path(), "session", "first").unwrap();
        assert_eq!(restored.messages, first.messages); assert_eq!(restored.tools, first.tools);
        assert_eq!(restored.system, [json!({"role":"system","content":"fixed"})]);
        assert_eq!(restored.response, first.response);
        let mut second = first.clone(); second.summary.id = "second".into();
        save(root.path(), "session", &second).unwrap();
        assert_eq!(std::fs::read_dir(session_dir(root.path(), "session").join("assets")).unwrap().count(), 2);
    }

    #[test]
    fn legacy_inputs_remain_readable_and_asset_paths_cannot_escape() {
        let root = tempfile::tempdir().unwrap(); let legacy = request("old");
        write(&paths(root.path(), "session", "old").0, &legacy).unwrap();
        assert_eq!(detail(root.path(), "session", "old").unwrap().messages, legacy.messages);
        let mut malformed = serde_json::to_value(&legacy).unwrap(); malformed["tools_ref"] = "../outside".into();
        write(&paths(root.path(), "session", "old").0, &malformed).unwrap();
        assert!(detail(root.path(), "session", "old").is_err());
    }

    #[test]
    fn snapshots_are_lazy_and_settlement_replaces_metadata() {
        let root = tempfile::tempdir().unwrap();
        assert!(list(root.path(), "session").unwrap().is_empty());
        let mut value = request("one");
        save(root.path(), "session", &value).unwrap();
        value.summary.status = "complete".into();
        value.summary.completed_at = Some(120);
        value.summary.usage = Some(json!({"input_tokens":3,"output_tokens":2}));
        value.response = json!({"content":"done"});
        save(root.path(), "session", &value).unwrap();
        let summaries = list(root.path(), "session").unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].status, "complete");
        assert_eq!(detail(root.path(), "session", "one").unwrap().response["content"], "done");
        // Detail corruption cannot force listing to parse a large request body.
        std::fs::write(paths(root.path(), "session", "one").0, "broken").unwrap();
        assert_eq!(list(root.path(), "session").unwrap().len(), 1);
        assert!(detail(root.path(), "session", "one").is_err());
    }

    #[test]
    fn identifiers_cannot_escape_storage_and_interruption_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let value = request("../../request");
        save(root.path(), "../session", &value).unwrap();
        assert!(session_dir(root.path(), "../session").starts_with(root.path()));
        let mut summary = list(root.path(), "../session").unwrap().remove(0);
        recover_summary(&mut summary, true);
        assert_eq!(summary.status, "running");
        recover_summary(&mut summary, false);
        assert_eq!(summary.status, "interrupted");
        assert!(detail(root.path(), "different", "../../request").is_err());
        remove_session(root.path(), "../session").unwrap();
        settle(root.path(), "../session", &value).unwrap();
        assert!(list(root.path(), "../session").unwrap().is_empty());
    }
}
