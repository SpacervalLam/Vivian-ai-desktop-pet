//! 实时语音通话命令 - 豆包端到端实时语音大模型（SC2.0）
//!
//! 与现有 ASR/LLM/TTS pipeline 平行的独立通话模式。
//! 前端通过 `invoke('start_realtime_call')` 启动，监听 `realtime:event` 事件驱动 UI。
//!
//! 多角色适配：所有命令接受 `character_id` 参数，路由到对应角色的 RealtimeVoiceManager。

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, State};

use crate::config::manager::RealtimeVoiceConfig;
use crate::state::AppState;

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 获取实时语音通话状态
#[tauri::command]
pub fn get_realtime_status(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let instance = state.get_character(character_id.as_deref())?;
    let status = instance.realtime_voice.state();
    Ok(serde_json::json!({ "state": status }))
}

/// 启动实时语音通话
///
/// 从配置中读取 RealtimeVoiceConfig，建立 WebSocket 连接，
/// 启动麦克风采集和扬声器播放。通话期间持续向前端 emit `realtime:event` 事件。
#[tauri::command]
pub async fn start_realtime_call(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let config = {
        let cm = state.config.read();
        cm.get_all().realtime_voice.clone()
    };
    if !config.enabled {
        return Err("实时语音通话未启用，请先在设置中开启".to_string());
    }
    if config.app_id.is_empty() || config.access_key.is_empty() {
        return Err("未配置豆包 App ID 或 Access Key".to_string());
    }
    let instance = state.get_character(character_id.as_deref())?;
    instance
        .realtime_voice
        .start_call(app, config)
        .await
        .map_err(err_str)
}

/// 停止实时语音通话
#[tauri::command]
pub fn stop_realtime_call(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let instance = state.get_character(character_id.as_deref())?;
    instance.realtime_voice.stop_call();
    // 持久化 dialog_id，下次通话时传入以恢复最近20轮上下文
    let dialog_id = instance.realtime_voice.last_dialog_id();
    if !dialog_id.is_empty() {
        let cm = state.config.read();
        if let Err(e) = cm.set_no_save(
            "realtime_voice.dialog_id",
            serde_json::Value::String(dialog_id),
        ) {
            tracing::warn!("保存实时通话 dialog_id 失败: {}", e);
        }
        let _ = cm.save();
    }
    Ok(())
}

/// 获取实时语音配置
#[tauri::command]
pub fn get_realtime_config(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let config = {
        let cm = state.config.read();
        cm.get_all().realtime_voice.clone()
    };
    serde_json::to_value(config).map_err(err_str)
}

/// 更新实时语音配置（仅修改内存，需调用 save_config 持久化）
#[tauri::command]
pub fn set_realtime_config(
    state: State<'_, Arc<AppState>>,
    config: Value,
) -> Result<(), String> {
    let realtime_config: RealtimeVoiceConfig =
        serde_json::from_value(config).map_err(err_str)?;
    let cm = state.config.read();
    cm.set_no_save(
        "realtime_voice",
        serde_json::to_value(&realtime_config).map_err(err_str)?,
    )
    .map_err(err_str)
}

/// 在通话中发送文本 query（代替音频输入）
#[tauri::command]
pub fn send_realtime_text(
    state: State<'_, Arc<AppState>>,
    text: String,
    character_id: Option<String>,
) -> Result<(), String> {
    let instance = state.get_character(character_id.as_deref())?;
    instance
        .realtime_voice
        .send_text_query(&text)
        .map_err(err_str)
}
