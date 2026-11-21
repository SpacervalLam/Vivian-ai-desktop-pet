//! 路由结果报表与「一键采用」。
//!
//! 报表把人工验收结果与用量成本合并成建议；采用动作只在**用户点击**后执行，
//! 且只填 `routing_matrix` 里原本为空的条目，绝不覆盖任何手配。

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use crate::brain::work_route_stats::{donor_task, global_route_stats, summarize, RouteCost};
use crate::state::AppState;

fn cost_rows(days: u32) -> Vec<RouteCost> {
    let report = crate::providers::usage_store::get_usage_report(days);
    report
        .routes
        .iter()
        .flat_map(|route| {
            route.models.iter().map(move |model| RouteCost {
                task: route.route.clone(),
                model: model.model.clone(),
                requests: model.requests,
                tokens: model.input + model.output + model.hit + model.cache_creation,
            })
        })
        .collect()
}

/// 报表：每个任务类的验收率、按模型的每次请求 token，以及是否可一键采用。
#[tauri::command]
pub fn work_route_report(state: State<'_, Arc<AppState>>, days: Option<u32>) -> Result<Value, String> {
    let days = days.unwrap_or(30);
    let config = state.config.read().get_all();
    let mut rows = summarize(&global_route_stats().records(), &cost_rows(days));
    let entries: Vec<(&str, &str)> = config
        .routing_matrix
        .iter()
        .map(|(task, route)| (task.as_str(), route.model.as_str()))
        .collect();
    for row in rows.iter_mut() {
        let task = row["task"].as_str().unwrap_or("").to_string();
        let current = config
            .routing_matrix
            .get(&task)
            .map(|route| route.model.trim().to_string())
            .filter(|model| !model.is_empty());
        let suggested = row["suggestion"]["model"].as_str().map(str::to_string);
        let donor = suggested
            .as_deref()
            .and_then(|model| donor_task(entries.iter().copied(), model, &task))
            .map(str::to_string);
        row["current_model"] = json!(current.clone().unwrap_or_else(|| "default".to_string()));
        row["donor_task"] = json!(donor);
        row["can_apply"] = json!(current.is_none() && suggested.is_some() && donor.is_some());
    }
    Ok(json!({
        "tasks": rows,
        "days": days,
        "note": "建议不会自动生效；采用后写入 routing_matrix 并立即重建路由。",
    }))
}

/// 采用建议：仅当该任务当前没有手配模型时，复制一个同模型的现有配置作为模板。
#[tauri::command]
pub fn work_route_apply(
    state: State<'_, Arc<AppState>>,
    task: String,
    model: String,
) -> Result<Value, String> {
    let task = task.trim().to_string();
    let model = model.trim().to_string();
    if task.is_empty() || model.is_empty() {
        return Err("task 与 model 都不能为空".to_string());
    }
    let config = state.config.read();
    let all = config.get_all();
    if all
        .routing_matrix
        .get(&task)
        .is_some_and(|route| !route.model.trim().is_empty())
    {
        return Err("该任务已有手配模型，建议不会覆盖它".to_string());
    }
    let donor = donor_task(
        all.routing_matrix
            .iter()
            .map(|(key, route)| (key.as_str(), route.model.as_str())),
        &model,
        &task,
    )
    .ok_or_else(|| format!("没有使用 {model} 的现有路由配置可作模板；请先在设置里手动配置一次"))?
    .to_string();
    let mut template = all
        .routing_matrix
        .get(&donor)
        .cloned()
        .ok_or("模板配置缺失")?;
    template.model = model.clone();
    let mut matrix = all.routing_matrix.clone();
    matrix.insert(task.clone(), template);
    let value = serde_json::to_value(&matrix).map_err(|error| error.to_string())?;
    config
        .set_no_save("routing_matrix", value)
        .map_err(|error| error.to_string())?;
    config.save().map_err(|error| error.to_string())?;
    drop(config);
    state.reload_model_router()?;
    Ok(json!({ "applied": { "task": task, "model": model, "donor": donor } }))
}
