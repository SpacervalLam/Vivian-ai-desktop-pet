//! 值守 Loop 的命令面：读取、用户增删改与启停。
//!
//! 状态权威在 `brain::work_loops`；命令只做校验与转达，调度由后台 tick 负责。

use serde_json::{json, Value};
use tauri::AppHandle;

use crate::brain::work_loops::{global_work_loops, MIN_CADENCE_SECS};

fn snapshot() -> Value {
    json!({ "loops": global_work_loops().list().iter().map(|loop_| loop_.summary()).collect::<Vec<_>>() })
}

#[tauri::command]
pub fn work_loop_list() -> Result<Value, String> {
    Ok(snapshot())
}

/// 用户新建或改写值守。带 id 为改写，不带 id 时按 dedup_key 幂等。
#[tauri::command]
pub fn work_loop_upsert(
    app: AppHandle,
    session_id: String,
    id: Option<String>,
    title: String,
    prompt: String,
    cadence_minutes: u64,
    max_runs: Option<u32>,
    ends_at: Option<i64>,
    dedup_key: Option<String>,
) -> Result<Value, String> {
    let cadence_secs = cadence_minutes.saturating_mul(60).max(MIN_CADENCE_SECS);
    global_work_loops().upsert(
        &session_id,
        id.as_deref(),
        &title,
        &prompt,
        cadence_secs,
        max_runs.unwrap_or(0),
        ends_at,
        dedup_key.as_deref().unwrap_or(""),
    )?;
    crate::tools::builtin::work_loops_tools::broadcast_loops(&app);
    Ok(snapshot())
}

#[tauri::command]
pub fn work_loop_set_enabled(app: AppHandle, id: String, enabled: bool) -> Result<Value, String> {
    global_work_loops().set_enabled(&id, enabled)?;
    crate::tools::builtin::work_loops_tools::broadcast_loops(&app);
    Ok(snapshot())
}

#[tauri::command]
pub fn work_loop_remove(app: AppHandle, id: String) -> Result<Value, String> {
    if !global_work_loops().remove(&id)? {
        return Err(format!("找不到值守 {id}"));
    }
    crate::tools::builtin::work_loops_tools::broadcast_loops(&app);
    Ok(snapshot())
}
