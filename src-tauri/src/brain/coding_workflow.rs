//! Lazy request snapshots for the workbench Workflow view. Kept outside session catalogs.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

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
    write(&body, detail)?;
    write(&meta, &detail.summary)
}

/// A late completion must not recreate snapshots after session deletion.
pub fn settle(root: &Path, session_id: &str, detail: &WorkflowRequestDetail) -> Result<(), String> {
    let _guard = WRITER.lock().map_err(|e| e.to_string())?;
    if !paths(root, session_id, &detail.summary.id).0.exists() { return Ok(()); }
    save_unlocked(root, session_id, detail)
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
    read(&paths(root, session_id, request_id).0)
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
