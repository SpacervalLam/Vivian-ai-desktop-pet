//! 任务轨的命令面：状态权威在 `WorkTaskStore`，这里只做读取、鉴权与转达。
//! 命令不自己拼状态字符串，也不绕过 `transition` 改状态。

use std::sync::Arc;

use serde_json::Value;
use tauri::State;

use crate::brain::work_jobs::global_work_job_registry;
use crate::state::AppState;

/// 跨会话不能读、不能验收、不能打回。
fn owned_job(session_id: &str, job_id: &str) -> Result<String, String> {
    let job = global_work_job_registry()
        .get(job_id)
        .ok_or_else(|| format!("找不到任务 {job_id}"))?;
    if job.session_id != session_id {
        return Err("该任务不属于当前会话".to_string());
    }
    Ok(job.job_id)
}

/// 记录验收结果时用的模型归因：会话显式选过工作模型就用它，
/// 否则取 `work_agent` 路由实际配置的模型；两者都没有才是默认路由。
fn attributed_model(state: &AppState, session_id: &str) -> String {
    let session_model = crate::commands::coding_agent::CODING_AGENT
        .get_session(session_id)
        .and_then(|session| session.model_id);
    session_model
        .or_else(|| {
            let route = state.config.read().get_all().routing_matrix.get("work_agent").cloned()?;
            (!route.model.trim().is_empty()).then(|| route.model.trim().to_string())
        })
        .unwrap_or_else(|| "default".to_string())
}

/// 跨会话的「需要你」总览：只返回等输入 / 待验收 / 未送达失败的任务摘要。
///
/// 任务是持久执行状态，会话只是界面；用户不该为了找一件等自己的事逐个切会话。
#[tauri::command]
pub fn work_task_attention() -> Result<Vec<Value>, String> {
    Ok(global_work_job_registry()
        .attention()
        .iter()
        .map(|job| job.summary())
        .collect())
}

/// 列出当前会话的任务投影。只发摘要，不把完整结果文本推给前端。
#[tauri::command]
pub fn work_task_list(session_id: String) -> Result<Vec<Value>, String> {
    Ok(global_work_job_registry()
        .list_for(&session_id)
        .iter()
        .map(|job| job.summary())
        .collect())
}

/// 人工验收，必须带当时看到的结果版本；版本变了旧验收作废。
#[tauri::command]
pub fn work_task_accept(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    job_id: String,
    revision: u32,
) -> Result<Value, String> {
    let id = owned_job(&session_id, &job_id)?;
    let registry = global_work_job_registry();
    registry.accept(&id, revision)?;
    crate::brain::work_route_stats::global_route_stats().record(
        "work_agent",
        &attributed_model(state.inner(), &session_id),
        true,
    );
    Ok(registry
        .get(&id)
        .map(|job| job.summary())
        .unwrap_or(Value::Null))
}

/// 打回修改：结果版本 +1，旧验收作废，结算重新排队。
#[tauri::command]
pub fn work_task_reject(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    job_id: String,
    reason: String,
) -> Result<Value, String> {
    let id = owned_job(&session_id, &job_id)?;
    let registry = global_work_job_registry();
    registry.reject(&id, &reason)?;
    crate::brain::work_route_stats::global_route_stats().record(
        "work_agent",
        &attributed_model(state.inner(), &session_id),
        false,
    );
    Ok(registry
        .get(&id)
        .map(|job| job.summary())
        .unwrap_or(Value::Null))
}

/// 回答「待你输入」那一问：任务回到进行中，结果版本 +1，
/// 由主会话决定续跑还是重新派 Worker。
#[tauri::command]
pub fn work_task_provide_input(
    session_id: String,
    job_id: String,
    note: String,
) -> Result<Value, String> {
    let id = owned_job(&session_id, &job_id)?;
    let registry = global_work_job_registry();
    registry.provide_input(&id, &note)?;
    Ok(registry
        .get(&id)
        .map(|job| job.summary())
        .unwrap_or(Value::Null))
}

/// 取消仍在跑的任务；已终结的返回错误，由前端提示。
#[tauri::command]
pub fn work_task_cancel(session_id: String, job_id: String) -> Result<Value, String> {
    let id = owned_job(&session_id, &job_id)?;
    let registry = global_work_job_registry();
    if !registry.cancel(&id) {
        return Err("任务已结束，无法取消".to_string());
    }
    Ok(registry
        .get(&id)
        .map(|job| job.summary())
        .unwrap_or(Value::Null))
}
