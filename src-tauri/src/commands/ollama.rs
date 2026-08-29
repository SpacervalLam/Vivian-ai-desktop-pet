//! Ollama 本地嵌入服务管理命令

use std::sync::Arc;

use serde_json::Value;
use tauri::State;

use crate::memory::ollama_service::ollama_service;
use crate::state::AppState;

/// 启动 Ollama serve 进程
#[tauri::command]
pub async fn start_ollama(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let ollama_path = {
        let config = state.config.read();
        config.get_all().memory.embedding.ollama_path.clone()
    };

    let svc = ollama_service().await;
    let new_state = svc.start(&ollama_path).await.map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(new_state).map_err(|e| e.to_string())?)
}

/// 停止 Ollama 进程
#[tauri::command]
pub async fn stop_ollama() -> Result<Value, String> {
    let svc = ollama_service().await;
    let new_state = svc.stop().await.map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(new_state).map_err(|e| e.to_string())?)
}

/// 查询 Ollama 服务状态（内部先 refresh 检测进程存活）
#[tauri::command]
pub async fn get_ollama_status() -> Result<Value, String> {
    let svc = ollama_service().await;
    let cur = svc.refresh().await;
    Ok(serde_json::to_value(cur).map_err(|e| e.to_string())?)
}

/// 拉取 Ollama 模型
#[tauri::command]
pub async fn pull_ollama_model(
    state: State<'_, Arc<AppState>>,
    model: String,
) -> Result<Value, String> {
    let ollama_path = {
        let config = state.config.read();
        config.get_all().memory.embedding.ollama_path.clone()
    };

    let result = crate::memory::ollama_service::OllamaServiceManager::pull_model(&model, &ollama_path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
}

/// 修复 Ollama models 目录权限（弹 UAC 提权）
#[tauri::command]
pub async fn fix_ollama_permission(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    let ollama_path = {
        let config = state.config.read();
        config.get_all().memory.embedding.ollama_path.clone()
    };

    crate::memory::ollama_service::OllamaServiceManager::fix_permission(&ollama_path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "success": true }))
}

/// 列出已安装的 Ollama 模型
#[tauri::command]
pub async fn list_ollama_models() -> Result<Value, String> {
    let models = crate::memory::ollama_service::OllamaServiceManager::list_models()
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "models": models }))
}
