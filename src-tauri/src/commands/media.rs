//! 灵动岛（chat window 顶部黑色胶囊）用的媒体查询与来源跳转命令。

use std::sync::Arc;
use serde_json::{json, Value};
use tauri::State;
use crate::state::AppState;

/// 当前系统正在播放的媒体（供 chat window 灵动岛展示）。
///
/// 只读内存缓存，不触发 SMTC 读取，可按秒级轮询。无播放时为 `null`。
#[tauri::command]
pub fn companion_now_playing(state: State<'_, Arc<AppState>>) -> Value {
    let music = state.world_provider.now_playing().map(|snapshot| {
        let mut value = json!(&snapshot);
        value["artwork_data_url"] = json!(snapshot.artwork_data_url);
        value
    });
    json!({ "music": music })
}

/// 把正在播放的来源应用切到前台（灵动岛点击）。
///
/// `source_app` 是 SMTC 给出的 AUMID；窗口枚举是阻塞调用，放 `spawn_blocking`。
#[tauri::command]
pub async fn companion_focus_media_source(source_app: String) -> Result<String, String> {
    let app = source_app.trim().to_string();
    if app.is_empty() || app.chars().count() > 512 {
        return Err("媒体来源信息无效。".to_string());
    }
    tokio::task::spawn_blocking(move || crate::media_focus::focus(&app))
        .await
        .map_err(|e| e.to_string())?
}
