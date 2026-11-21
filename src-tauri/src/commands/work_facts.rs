//! 跨会话工作事实的命令面。
//!
//! 状态权威在 `brain::work_facts`：命令只做读取、用户写入与事件转达，
//! 不自己推导条目状态。用户写入绕过「是否值得共享」的判断——判断用来挡
//! 模型噪音，不用来限制用户。

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::brain::work_facts::{global_work_facts, WRITER_USER};

fn snapshot() -> Value {
    let store = global_work_facts();
    json!({ "revision": store.revision(), "facts": store.list() })
}

fn broadcast(app: &AppHandle) {
    let _ = app.emit("work_facts:changed", snapshot());
}

/// 读取当前清单（含版本号）。
#[tauri::command]
pub fn work_facts_list() -> Result<Value, String> {
    Ok(snapshot())
}

/// 用户新增或改写一条事实：带 id 为改写，不带 id 为新增。
#[tauri::command]
pub fn work_facts_save(
    app: AppHandle,
    text: String,
    id: Option<String>,
) -> Result<Value, String> {
    let store = global_work_facts();
    let result = match id.as_deref() {
        Some(id) if !id.trim().is_empty() => store.update(id, &text, WRITER_USER).map(|_| ()),
        _ => store.add(&text, "", WRITER_USER).map(|_| ()),
    };
    result?;
    broadcast(&app);
    Ok(snapshot())
}

/// 删除一条事实。
#[tauri::command]
pub fn work_facts_remove(app: AppHandle, id: String) -> Result<Value, String> {
    let store = global_work_facts();
    if !store.remove(&id)? {
        return Err(format!("找不到事实 {id}"));
    }
    broadcast(&app);
    Ok(snapshot())
}

/// 清空清单。
#[tauri::command]
pub fn work_facts_clear(app: AppHandle) -> Result<Value, String> {
    global_work_facts().clear()?;
    broadcast(&app);
    Ok(snapshot())
}
