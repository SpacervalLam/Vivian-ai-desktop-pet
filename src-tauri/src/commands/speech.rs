//! 语音命令 - 识别启停与状态查询
//!
//! 语音识别命令入口
//! 通过 `AppState.asr`（`AsrManager`）委托识别操作。

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::speech::{AsrConfig, AsrEvent, AsrManager, WhisperConfig};
use crate::state::AppState;

/// 启动 ASR 事件转发器：把 `AsrManager` 的 broadcast 事件桥接到 Tauri 前端事件 `asr:event`
///
/// 在 app setup 时调用一次，永久运行。事件载荷结构（与 `AsrEvent` serde 标签一致）：
/// - `{ "type": "started" }`
/// - `{ "type": "stopped" }`
/// - `{ "type": "partial_result", "text": "..." }`
/// - `{ "type": "final_result", "text": "...", "confidence": 0.9 }`
/// - `{ "type": "error", "message": "..." }`
pub fn start_asr_event_forwarder(app: AppHandle, manager: AsrManager) {
    let mut rx = manager.subscribe();
    tauri::async_runtime::spawn(async move {
        while let Ok(event) = rx.recv().await {
            // 后端自动停止（静默超时/会话结束）时同步管理器状态
            if matches!(event, AsrEvent::Stopped) {
                manager.mark_stopped();
            }
            // AsrEvent 已派生 Serialize 且带 `#[serde(tag = "type")]`，可直接 emit
            let _ = app.emit("asr:event", &event);
            if matches!(event, AsrEvent::Error { .. }) {
                tracing::debug!("ASR 事件已转发: {:?}", event);
            }
        }
        tracing::info!("ASR 事件转发器结束（broadcast 通道已关闭）");
    });
}

/// 开始语音识别
#[tauri::command]
pub async fn start_recognition(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    if let Some(cid) = character_id {
        state.reset_generation_cancel(&cid);
    }
    state.asr.start_recognition().await.map_err(|e| e.to_string())
}

/// 停止语音识别
#[tauri::command]
pub async fn stop_recognition(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.asr.stop_recognition().await.map_err(|e| e.to_string())
}

/// 获取识别状态
#[tauri::command]
pub fn get_recognition_status(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    Ok(state.asr.is_recording())
}

/// 即时更新 ASR 运行时配置（设置面板保存后调用，无需重启应用）
///
/// 从当前 AppConfig.speech_recognition 读取并注入 AsrManager，
/// 让 engine / language / silence_timeout_ms 三个字段立即影响后续识别。
#[tauri::command]
pub async fn update_asr_config(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let cfg = state.config.read().get_all().speech_recognition.clone();
    let asr_config = AsrConfig::from_speech_config(&cfg);
    state.asr.set_config(asr_config).await.map_err(|e| e.to_string())
}

/// 注册三个文字输入快捷键（Vivian/Nana 私聊 + 群发总框）
///
/// 在 app setup 时调用一次。已注册的快捷键通过 AppState.text_shortcuts 跟踪，
/// key 为角色标识（"vivian"/"nana"/"broadcast"），value 为快捷键字符串。
pub fn register_text_shortcuts(app: AppHandle, state: &Arc<AppState>) {
    let base = state.config.read().get_all().base.clone();
    let entries: [(&str, &str); 3] = [
        ("vivian", &base.shortcut),
        ("nana", &base.shortcut_nana),
        ("broadcast", &base.shortcut_broadcast),
    ];
    let mut map = state.text_shortcuts.lock();
    for (role, sc) in entries {
        if sc.is_empty() {
            continue;
        }
        if let Err(e) = app.global_shortcut().register(sc) {
            tracing::warn!("[text_shortcut] 注册 {} 快捷键 {} 失败: {}", role, sc, e);
        } else {
            tracing::info!("[text_shortcut] 已注册 {} 快捷键: {}", role, sc);
            map.insert(role.to_string(), sc.to_string());
        }
    }
}

/// 更新文字输入快捷键（设置面板保存后调用）
///
/// 先解绑所有旧快捷键，再从配置读取新值重新注册。
#[tauri::command]
pub fn update_text_shortcuts(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    // 先解绑所有旧快捷键
    let old_map = state.text_shortcuts.lock().clone();
    for (_role, sc) in &old_map {
        let _ = app.global_shortcut().unregister(sc.as_str());
    }

    // 从配置读取新值重新注册
    let base = state.config.read().get_all().base.clone();
    let entries: [(&str, &str); 3] = [
        ("vivian", &base.shortcut),
        ("nana", &base.shortcut_nana),
        ("broadcast", &base.shortcut_broadcast),
    ];
    let mut new_map = std::collections::HashMap::new();
    for (role, sc) in entries {
        if sc.is_empty() {
            continue;
        }
        app.global_shortcut()
            .register(sc)
            .map_err(|e| format!("注册快捷键失败: {}", e))?;
        new_map.insert(role.to_string(), sc.to_string());
    }

    *state.text_shortcuts.lock() = new_map;
    tracing::info!("[text_shortcut] 文字快捷键已更新");
    Ok(())
}

// ── Whisper 本地服务一键部署 ──
//
// 通过子进程启动 faster-whisper-server（OpenAI 兼容）推理服务，
// 默认监听 127.0.0.1:8000，与 Whisper ASR 后端的 /v1/audio/transcriptions 调用直连。
// 启动参数取自 SpeechRecognitionConfig.whisper 的 service_* 字段（可在设置面板配置）。
// 启动成功后自动回写 server_url=http://127.0.0.1:<port> 与 api_format=openai 并触发 update_asr_config。

/// 一键启动 faster-whisper-server 服务
///
/// 启动参数取自当前 SpeechRecognitionConfig.whisper 的 service_* 字段
/// （Python 路径/安装路径/模型/设备/精度/端口）。
/// 启动后异步等待健康检查通过（默认 60s 超时）；前端可轮询 `get_whisper_service_status`
/// 获取最新状态。
/// 启动成功后自动把 server_url 回写为 `http://127.0.0.1:<port>`、api_format 回写为 `openai`，
/// 并调用 `update_asr_config` 让 ASR 后端立即生效。
#[tauri::command]
pub async fn start_whisper_service(
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    tracing::info!("[Whisper] 收到启动服务请求");
    let cfg = {
        let c = state.config.read();
        c.get_all().speech_recognition.whisper.clone()
    };
    if cfg.service_python_path.as_deref().map(|s| s.is_empty()).unwrap_or(true)
        && which_faster_whisper_server().is_none()
    {
        return Err(
            "未在 PATH 中找到 faster-whisper-server。请先执行 `pip install faster-whisper-server`，或在\"Python 路径\"字段填入 python.exe 路径".to_string(),
        );
    }

    let svc = crate::speech::whisper_service().await;
    let new_state = svc.start(&cfg).await.map_err(|e| e.to_string())?;

    // 启动成功后回写 server_url 与 api_format，让 ASR 后端立即指向本地服务
    let port = cfg.service_port.unwrap_or(8000);
    let endpoint = format!("http://127.0.0.1:{port}");
    let app_state = state.inner().clone();
    persist_whisper_runtime_config(&app_state, &endpoint).await?;

    Ok(serde_json::to_value(new_state).map_err(|e| e.to_string())?)
}

/// 停止 faster-whisper-server 服务
#[tauri::command]
pub async fn stop_whisper_service() -> Result<Value, String> {
    let svc = crate::speech::whisper_service().await;
    let new_state = svc.stop().await.map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(new_state).map_err(|e| e.to_string())?)
}

/// 查询 faster-whisper-server 服务状态
///
/// 内部会先调用 `refresh()` 检查子进程是否仍存活（防止状态失真），
/// 再返回当前 `WhisperServiceState`。前端可定时轮询此接口（建议 2s 一次）。
#[tauri::command]
pub async fn get_whisper_service_status() -> Result<Value, String> {
    let svc = crate::speech::whisper_service().await;
    let cur = svc.refresh().await;
    Ok(serde_json::to_value(cur).map_err(|e| e.to_string())?)
}

/// 启动成功后回写 WhisperConfig 的 server_url 与 api_format，并触发 update_asr_config
async fn persist_whisper_runtime_config(
    state: &Arc<AppState>,
    endpoint: &str,
) -> Result<(), String> {
    // 修改配置并持久化
    {
        let cm = state.config.read();
        cm.set_no_save(
            "speech_recognition.whisper.server_url",
            serde_json::Value::String(endpoint.to_string()),
        )
        .map_err(|e| e.to_string())?;
        cm.set_no_save(
            "speech_recognition.whisper.api_format",
            serde_json::Value::String("openai".to_string()),
        )
        .map_err(|e| e.to_string())?;
        cm.save().map_err(|e| e.to_string())?;
    }

    // 让 ASR 后端即时生效
    let cfg = state.config.read().get_all().speech_recognition.clone();
    let asr_config = AsrConfig::from_speech_config(&cfg);
    state
        .asr
        .set_config(asr_config)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("[Whisper] 已自动回写 server_url={endpoint} 并热重载 ASR 配置");
    Ok(())
}

/// 检查 PATH 中是否存在 faster-whisper-server 可执行文件
fn which_faster_whisper_server() -> Option<std::path::PathBuf> {
    let exe_name = if cfg!(target_os = "windows") {
        "faster-whisper-server.exe"
    } else {
        "faster-whisper-server"
    };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(exe_name);
            if full.is_file() {
                Some(full)
            } else {
                None
            }
        })
    })
}

/// 在 ASR 引擎切换到 whisper 或应用启动时检查是否需要自动拉起本地 Whisper 服务
///
/// 仅当 `service_auto_start = true` 时触发。前端无需调用，由 `lib.rs` 在 setup 阶段调用。
pub async fn maybe_autostart_whisper_service(state: &Arc<AppState>) {
    let cfg = {
        let c = state.config.read();
        c.get_all().speech_recognition.whisper.clone()
    };
    if !cfg.service_auto_start {
        return;
    }
    // 若已配置外部 server_url（非本地 127.0.0.1）则跳过自动启动
    if !cfg.server_url.is_empty()
        && !cfg.server_url.contains("127.0.0.1")
        && !cfg.server_url.contains("localhost")
    {
        tracing::info!(
            "[Whisper] 已配置外部服务地址 {}，跳过自动启动",
            cfg.server_url
        );
        return;
    }
    let svc = crate::speech::whisper_service().await;
    match svc.start(&cfg).await {
        Ok(s) => {
            tracing::info!("[lib] Whisper 服务自动启动已触发: {:?}", s.status);
            if matches!(
                s.status,
                crate::speech::WhisperServiceStatus::Starting
                    | crate::speech::WhisperServiceStatus::Running
            ) {
                let port = cfg.service_port.unwrap_or(8000);
                let endpoint = format!("http://127.0.0.1:{port}");
                if let Err(e) = persist_whisper_runtime_config(state, &endpoint).await {
                    tracing::warn!("[lib] 回写 Whisper 运行时配置失败: {e}");
                }
            }
        }
        Err(e) => tracing::warn!("[lib] Whisper 服务自动启动失败: {e}"),
    }
}

/// 仅供测试或外部模块使用：返回当前 WhisperConfig 快照
pub fn current_whisper_config(state: &Arc<AppState>) -> WhisperConfig {
    state.config.read().get_all().speech_recognition.whisper.clone()
}
