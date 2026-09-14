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
    if let Some(cid) = &character_id {
        state.reset_generation_cancel(cid);
    }
    // 半双工协调：若 TTS 正在播放，先停止 TTS 再启动录音，
    // 避免麦克风录到扬声器声音（回声/自激）
    if state.playback_gate.is_playing() {
        if let Ok(character) = state.get_character(character_id.as_deref()) {
            let planner = crate::speech::get_planner().await;
            let _ = planner.stop_speaker(&character.id).await;
        }
        state.playback_gate.mark_finished();
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

/// 对音频文件进行一次性语音转文字（非实时，用于 cpal 类后端避免麦克风冲突）。
///
/// `samples_b64`：base64 编码的小端 f32 PCM 字节流（16kHz 单声道）。
/// 前端用 Web Audio API 解码录制的 webm/ogg 并重采样到 16kHz 后编码传入。
/// 返回识别文本。
#[tauri::command]
pub async fn transcribe_audio(
    state: State<'_, Arc<AppState>>,
    samples_b64: String,
) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(samples_b64.trim())
        .map_err(|e| format!("base64 解码失败: {e}"))?;
    if bytes.len() % 4 != 0 {
        return Err("音频数据长度不是 4 的倍数（f32 PCM）".into());
    }
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    state.asr.transcribe(&samples).await.map_err(|e| e.to_string())
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

/// 语音识别结果 LLM 整理重写
///
/// 识别文本可能含同音字错误、冗余语气词、标点缺失，经 LLM 修正后返回。
/// 走路由矩阵 `asr_polish` 任务（建议配置便宜快速模型），未配置时回退主 LLM API。
/// 失败或结果为空时原样返回输入文本（尽力而为，不向前端报错）。
#[tauri::command]
pub async fn polish_asr_text(
    state: State<'_, Arc<AppState>>,
    text: String,
) -> Result<String, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(text);
    }

    let character = state.get_character(None)?;
    let system_prompt = "你是语音识别文本的整理助手。用户输入是语音识别（ASR）的原始结果，\
        可能存在同音字/近音字错误、多余口头语气词、标点缺失或断句不当。\n\
        请整理重写这段文本，要求：\n\
        1. 修正明显的语音识别错误\n\
        2. 去除无意义的口头语气词（呃、嗯、啊、这个、那个等），保留有语义的语气词\n\
        3. 补全标点符号，合理断句\n\
        4. 保持原意、原语言和口语化口吻，不翻译、不扩写、不增删信息\n\
        5. 仅输出整理后的文本，不带任何解释、前缀或引号";

    let messages = vec![
        crate::types::response::ChatMessage::system(system_prompt),
        crate::types::response::ChatMessage::user(&text),
    ];
    let request = crate::providers::base::LLMRequest::new("asr_polish", messages)
        .with_temperature(0.3);

    match character.brain.router.generate(request).await {
        Ok(raw) => {
            let polished = parse_polished_text(&raw);
            if polished.is_empty() {
                Ok(text)
            } else {
                Ok(polished)
            }
        }
        Err(e) => {
            tracing::warn!("[ASR] 识别文本润色失败，返回原文: {e}");
            Ok(text)
        }
    }
}

/// 清理 LLM 润色响应：去代码块包裹与首尾引号
fn parse_polished_text(raw: &str) -> String {
    let mut result = raw.trim().to_string();

    if result.starts_with("```") {
        if let Some(end) = result.find('\n') {
            result = result[end + 1..].to_string();
        }
        if result.ends_with("```") {
            result = result[..result.len() - 3].to_string();
        }
        result = result.trim().to_string();
    }

    let len = result.chars().count();
    if len >= 2 {
        let first = result.chars().next().unwrap();
        let last = result.chars().last().unwrap();
        if (first == '"' && last == '"')
            || (first == '“' && last == '”')
            || (first == '\'' && last == '\'')
        {
            result = result.chars().skip(1).take(len - 2).collect();
        }
    }

    result.trim().to_string()
}

/// 注册文字输入快捷键（Vivian/Nana 私聊 + 群发总框）和窗口快捷键（微信/设置/笔记本）
///
/// 在 app setup 时调用一次。文字快捷键通过 AppState.text_shortcuts 跟踪，
/// 窗口快捷键通过 AppState.window_shortcuts 跟踪，
/// key 为标识（"vivian"/"nana"/"broadcast" 或 "chat"/"settings"/"memory"），value 为快捷键字符串。
pub fn register_text_shortcuts(app: AppHandle, state: &Arc<AppState>) {
    let base = state.config.read().get_all().base.clone();

    // 文字快捷键
    let text_entries: [(&str, &str); 3] = [
        ("vivian", &base.shortcut),
        ("nana", &base.shortcut_nana),
        ("broadcast", &base.shortcut_broadcast),
    ];
    let mut text_map = state.text_shortcuts.lock();
    for (role, sc) in text_entries {
        if sc.is_empty() {
            continue;
        }
        if let Err(e) = app.global_shortcut().register(sc) {
            tracing::warn!("[text_shortcut] 注册 {} 快捷键 {} 失败: {}", role, sc, e);
        } else {
            tracing::info!("[text_shortcut] 已注册 {} 快捷键: {}", role, sc);
            text_map.insert(role.to_string(), sc.to_string());
        }
    }
    drop(text_map);

    // 窗口快捷键
    let win_entries: [(&str, &str); 3] = [
        ("chat", &base.shortcut_chat),
        ("settings", &base.shortcut_settings),
        ("memory", &base.shortcut_memory),
    ];
    let mut win_map = state.window_shortcuts.lock();
    for (action, sc) in win_entries {
        if sc.is_empty() {
            continue;
        }
        if let Err(e) = app.global_shortcut().register(sc) {
            tracing::warn!("[window_shortcut] 注册 {} 快捷键 {} 失败: {}", action, sc, e);
        } else {
            tracing::info!("[window_shortcut] 已注册 {} 快捷键: {}", action, sc);
            win_map.insert(action.to_string(), sc.to_string());
        }
    }
}

/// 更新文字输入快捷键和窗口快捷键（设置面板保存后调用）
///
/// 先解绑所有旧快捷键，再从配置读取新值重新注册。
#[tauri::command]
pub fn update_text_shortcuts(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    // 先解绑所有旧文字快捷键
    let old_text = state.text_shortcuts.lock().clone();
    for (_role, sc) in &old_text {
        let _ = app.global_shortcut().unregister(sc.as_str());
    }
    // 先解绑所有旧窗口快捷键
    let old_win = state.window_shortcuts.lock().clone();
    for (_action, sc) in &old_win {
        let _ = app.global_shortcut().unregister(sc.as_str());
    }

    // 从配置读取新值重新注册
    let base = state.config.read().get_all().base.clone();

    // 文字快捷键
    let text_entries: [(&str, &str); 3] = [
        ("vivian", &base.shortcut),
        ("nana", &base.shortcut_nana),
        ("broadcast", &base.shortcut_broadcast),
    ];
    let mut new_text = std::collections::HashMap::new();
    for (role, sc) in text_entries {
        if sc.is_empty() {
            continue;
        }
        app.global_shortcut()
            .register(sc)
            .map_err(|e| format!("注册快捷键失败: {}", e))?;
        new_text.insert(role.to_string(), sc.to_string());
    }
    *state.text_shortcuts.lock() = new_text;

    // 窗口快捷键
    let win_entries: [(&str, &str); 3] = [
        ("chat", &base.shortcut_chat),
        ("settings", &base.shortcut_settings),
        ("memory", &base.shortcut_memory),
    ];
    let mut new_win = std::collections::HashMap::new();
    for (action, sc) in win_entries {
        if sc.is_empty() {
            continue;
        }
        app.global_shortcut()
            .register(sc)
            .map_err(|e| format!("注册快捷键失败: {}", e))?;
        new_win.insert(action.to_string(), sc.to_string());
    }
    *state.window_shortcuts.lock() = new_win;

    tracing::info!("[shortcut] 文字快捷键与窗口快捷键已更新");
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
    // 解析应用代理配置，透传给 pip 安装子进程
    let proxy_url = {
        let c = state.config.read();
        let pc = crate::network::proxy::ProxyConfig::from_app_config(&c.get_all());
        pc.effective_proxy_url()
    };

    let svc = crate::speech::whisper_service().await;
    let new_state = svc.start(&cfg, proxy_url).await.map_err(|e| e.to_string())?;

    // 仅在服务已进入 Starting/Running 状态时回写 server_url。
    // Installing 状态由后台异步安装+启动，回写交由 get_whisper_service_status
    // 轮询时检测 Running 跃迁完成，避免在服务未就绪时写入无效端点。
    if matches!(
        new_state.status,
        crate::speech::WhisperServiceStatus::Starting
            | crate::speech::WhisperServiceStatus::Running
    ) {
        let port = cfg.service_port.unwrap_or(8000);
        let endpoint = format!("http://127.0.0.1:{port}");
        let app_state = state.inner().clone();
        if let Err(e) = persist_whisper_runtime_config(&app_state, &endpoint).await {
            tracing::warn!("[Whisper] 回写运行时配置失败: {e}");
        }
    }

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
///
/// 副作用：若服务已进入 Running 但配置中的 server_url 尚未指向本地端点
/// （例如从 Installing 后台安装完成跃迁而来），此处自动回写 server_url
/// 并触发 ASR 配置热重载，保证一键启动流程闭环。
#[tauri::command]
pub async fn get_whisper_service_status(
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let svc = crate::speech::whisper_service().await;
    let cur = svc.refresh().await;

    // 检测 Running 跃迁：服务已就绪但 server_url 未指向本地端点时回写
    if matches!(
        cur.status,
        crate::speech::WhisperServiceStatus::Running
    ) {
        if let Some(port) = cur.port {
            let expected = format!("http://127.0.0.1:{port}");
            let current_url = {
                let c = state.config.read();
                c.get_all().speech_recognition.whisper.server_url.clone()
            };
            if current_url != expected {
                let app_state = state.inner().clone();
                if let Err(e) = persist_whisper_runtime_config(&app_state, &expected).await {
                    tracing::warn!("[Whisper] Running 跃迁回写 server_url 失败: {e}");
                }
            }
        }
    }

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

/// 在 ASR 引擎切换到 whisper 或应用启动时检查是否需要自动拉起本地 Whisper 服务
///
/// 仅当 `service_auto_start = true` 时触发。前端无需调用，由 `lib.rs` 在 setup 阶段调用。
pub async fn maybe_autostart_whisper_service(state: &Arc<AppState>) {
    let (cfg, proxy_url) = {
        let c = state.config.read();
        let whisper_cfg = c.get_all().speech_recognition.whisper.clone();
        let pc = crate::network::proxy::ProxyConfig::from_app_config(&c.get_all());
        (whisper_cfg, pc.effective_proxy_url())
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
    match svc.start(&cfg, proxy_url).await {
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
