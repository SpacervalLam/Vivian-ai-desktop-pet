//! 指标命令 - 暴露运行时指标给前端
//!
//! 对齐任务要求：提供 `get_metrics_summary` 命令。

use serde_json::Value;

use crate::metrics;

/// 获取指标摘要（counter / histogram / gauge / 降级总数）
#[tauri::command]
pub fn get_metrics_summary() -> Result<Value, String> {
    let snapshot = metrics::metrics().get_snapshot();
    serde_json::to_value(snapshot).map_err(|e| e.to_string())
}

/// 持久化指标到 `metrics_<date>.json`
#[tauri::command]
pub fn persist_metrics() -> Result<(), String> {
    metrics::metrics()
        .persist()
        .map_err(|e| e.to_string())
}

/// 重置所有指标（主要供调试使用）
#[tauri::command]
pub fn reset_metrics() -> Result<(), String> {
    metrics::metrics().reset();
    Ok(())
}

/// 递增指定 counter（供前端埋点使用）
#[tauri::command]
pub fn increment_metric(name: String, value: Option<u64>) -> Result<(), String> {
    let c = metrics::metrics().counter(&name);
    c.inc(value.unwrap_or(1));
    Ok(())
}

/// 设置指定 gauge 值
#[tauri::command]
pub fn set_gauge_metric(name: String, value: f64) -> Result<(), String> {
    let g = metrics::metrics().gauge(&name);
    g.set(value);
    Ok(())
}
