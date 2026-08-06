//! TTS 命令 - 语音合成配置与朗读
//!
//! 与 `commands/live2d_lipsync.rs` 协作：`speak_text` 调用时自动触发 `start_lipsync`，
//! `stop_speaking` 调用时自动触发 `stop_lipsync`，前端监听 `lipsync:*` 事件驱动 Live2D 嘴形。
//!
//! TTS 事件回调(`tts:started` / `tts:word` / `tts:finished` / `tts:error` / `tts:fallback`)
//! 在 `speak_text` 期间向前端推送,前端可据此做音素级唇形同步。

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::commands::live2d_lipsync::LipsyncRuntime;
use crate::speech::{
    fish_speech_service, get_planner, gpt_sovits_service, speak_intent, Presentation,
    SpeechPriority, TtsConfig, TtsEvent, TtsEventCallback,
};
use crate::state::AppState;

/// 过滤掉文本中的括号动作描述（如 `(轻声笑了笑)`），避免 TTS 朗读动作文本
fn strip_action_text(text: &str) -> String {
    let re = regex::Regex::new(r"\([^)]*\)").unwrap();
    let result = re.replace_all(text, "").into_owned();
    // 清理多余空白
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 获取 TTS 配置
#[tauri::command]
pub fn get_tts_config(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let config = brain.tts.get_config();
    serde_json::to_value(config).map_err(|e| e.to_string())
}

/// 更新 TTS 配置
#[tauri::command]
pub async fn set_tts_config(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    config: Value,
) -> Result<(), String> {
    let tts_config: TtsConfig =
        serde_json::from_value(config).map_err(|e| e.to_string())?;
    let tts = state.get_character(character_id.as_deref())?.brain.tts.clone();
    tts.set_config(tts_config).map_err(|e| e.to_string())
}

/// 列出当前后端可用语音
#[tauri::command]
pub async fn list_tts_voices(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    let tts = state.get_character(character_id.as_deref())?.brain.tts.clone();
    let voices = tts.list_voices().await.map_err(|e| e.to_string())?;
    serde_json::to_value(voices).map_err(|e| e.to_string())
}

/// 测试当前 TTS 后端(合成一小段文本,不播放)
#[tauri::command]
pub async fn test_tts(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let tts = state.get_character(character_id.as_deref())?.brain.tts.clone();
    tts.test().await.map_err(|e| e.to_string())
}

/// 朗读文本
///
/// 朗读期间通过 `tts:started` / `tts:word` / `tts:finished` / `tts:error` 事件
/// 向前端推送合成进度,前端可据此驱动音素级唇形同步。
///
/// `emotion` 参数可选，用于 GPT-SoVITS emotionVoiceMap 音色切换：
/// 前端从 AI 响应的 expression 字段传入，后端查找 emotion_voice_map 覆盖参考音频。
///
/// 内部通过 SpeechPlanner 调度:构造 SpeakIntent 提交给全局 Planner,
/// Planner 根据 priority 仲裁(多角色冲突时谁先说、谁让路)。
#[tauri::command]
pub async fn speak_text(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    lipsync: State<'_, Arc<LipsyncRuntime>>,
    app: AppHandle,
    text: String,
    emotion: Option<String>,
    presentation: Option<Presentation>,
) -> Result<(), String> {
    tracing::info!(
        "[TTS] speak_text 调用: text={:?} len={} emotion={:?}",
        &text,
        text.chars().count(),
        &emotion
    );
    let character = state.get_character(character_id.as_deref())?;
    let speaker_id = character.id.clone();
    let tts = character.brain.tts.clone();
    let config_snapshot = tts.get_config();
    tracing::info!(
        "[TTS] 当前配置: enabled={} engine={:?} volume={} rate={}",
        config_snapshot.enabled, config_snapshot.engine, config_snapshot.volume, config_snapshot.rate
    );

    // 跨语言翻译：display_language 与 tts_language 不同时，先翻译文本再送 TTS
    let tts_text = if let (Some(from), Some(to)) =
        (config_snapshot.display_language.as_deref(), config_snapshot.tts_language.as_deref())
    {
        if from != to {
            let provider = config_snapshot.translation_provider.as_deref().unwrap_or("google");
            let svc = crate::translation::translation_service().await;

            let result = if provider == "llm" {
                let router = character.brain.router.clone();
                svc.translate_llm(&text, from, to, &router).await
            } else {
                let api_key = config_snapshot.translation_api_key.as_deref().unwrap_or("");
                let endpoint = config_snapshot.translation_endpoint.as_deref();
                if api_key.is_empty() {
                    tracing::warn!("[TTS] 翻译服务 API Key 未配置，跳过翻译");
                    Ok(text.clone())
                } else {
                    svc.translate(&text, from, to, provider, api_key, endpoint).await
                }
            };

            match result {
                Ok(translated) => {
                    if translated != text {
                        tracing::info!(
                            "[TTS] 翻译完成: {} → {} ({}字符)",
                            from, to, translated.chars().count()
                        );
                    }
                    translated
                }
                Err(e) => {
                    tracing::warn!("[TTS] 翻译失败，降级使用原文: {e}");
                    let _ = app.emit("toast:show", serde_json::json!({
                        "type": "error",
                        "message": format!("翻译失败，使用原文合成: {e}"),
                        "duration": 6000,
                        "character_id": character_id.clone(),
                    }));
                    text.clone()
                }
            }
        } else {
            text.clone()
        }
    } else {
        text.clone()
    };

    // 过滤括号动作描述，避免 TTS 朗读动作文本
    let tts_text = strip_action_text(&tts_text);
    if tts_text.trim().is_empty() {
        tracing::info!("[TTS] 过滤后文本为空，跳过朗读");
        return Ok(());
    }
    tracing::info!("[TTS] 过滤后文本: {:?} len={}", &tts_text, tts_text.chars().count());

    // 注册事件回调:将 TtsEvent 转发为 tauri 事件（附带来源 character_id 防止多窗口串扰）
    let app_for_cb = app.clone();
    let sid_for_cb = speaker_id.clone();
    let gate_for_cb = state.playback_gate.clone();
    let event_cb: TtsEventCallback = Arc::new(move |event: &TtsEvent| {
        let mut payload = serde_json::to_value(event).unwrap_or(Value::Null);
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("character_id".to_string(), serde_json::Value::String(sid_for_cb.clone()));
        }
        match event {
            TtsEvent::Started { .. } => {
                gate_for_cb.mark_started();
                let _ = app_for_cb.emit("tts:started", payload);
            }
            TtsEvent::WordBoundary { text, mouth_open, .. } => {
                let _ = app_for_cb.emit("tts:word", payload);
                let _ = app_for_cb.emit(
                    "lipsync:update",
                    serde_json::json!({
                        "text": text,
                        "mouth_open": mouth_open,
                        "character_id": sid_for_cb,
                    }),
                );
            }
            TtsEvent::Finished => {
                gate_for_cb.mark_finished();
                let _ = app_for_cb.emit("tts:finished", payload);
            }
            TtsEvent::Error { .. } => {
                gate_for_cb.mark_finished();
                let _ = app_for_cb.emit("tts:error", payload);
            }
            TtsEvent::Fallback { .. } => {
                let _ = app_for_cb.emit("tts:fallback", payload);
            }
        }
    });
    tts.set_event_callback(Some(event_cb));

    // 触发嘴形联动开始
    lipsync.set_state(crate::commands::live2d_lipsync::LipsyncState::Speaking);
    lipsync.set_target_open(0.25);
    lipsync.set_phoneme(None);
    let _ = app.emit(
        "lipsync:start",
        serde_json::json!({ "text": &text, "target_open": 0.25 }),
    );

    // 通过 SpeechPlanner 调度
    let mut builder = speak_intent(&tts_text, &speaker_id)
        .emotion(emotion.unwrap_or_default())
        .priority(SpeechPriority::Normal);
    if let Some(pres) = presentation {
        builder = builder.presentation(pres);
    }
    let intent = builder.build();

    let planner = get_planner().await;
    let handle = planner.submit(intent).await.map_err(|e| e.to_string())?;
    let result = match handle.done().await {
        crate::speech::SubmitResult::Played => Ok(()),
        crate::speech::SubmitResult::Dropped => {
            tracing::info!("[TTS] intent 被丢弃(让路或被抢占)");
            Ok(())
        }
        crate::speech::SubmitResult::Failed(msg) => Err(msg),
    };

    // 无论成功失败都恢复嘴形联动到 idle
    lipsync.set_state(crate::commands::live2d_lipsync::LipsyncState::Idle);
    lipsync.set_target_open(0.0);
    lipsync.set_phoneme(None);
    let _ = app.emit(
        "lipsync:stop",
        serde_json::json!({ "target_open": 0.0 }),
    );

    // 清除事件回调(避免下次 speak 重复发射)
    tts.set_event_callback(None);

    result
}

/// 停止朗读
///
/// 通过 SpeechPlanner 停止指定角色的播放并清空其队列。
#[tauri::command]
pub async fn stop_speaking(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    lipsync: State<'_, Arc<LipsyncRuntime>>,
    app: AppHandle,
) -> Result<(), String> {
    let character = state.get_character(character_id.as_deref())?;
    let speaker_id = character.id.clone();

    let planner = get_planner().await;
    planner
        .stop_speaker(&speaker_id)
        .await
        .map_err(|e| e.to_string())?;

    state.playback_gate.mark_finished();

    // 同步停止嘴形联动
    lipsync.set_state(crate::commands::live2d_lipsync::LipsyncState::Idle);
    lipsync.set_target_open(0.0);
    lipsync.set_phoneme(None);
    let _ = app.emit(
        "lipsync:stop",
        serde_json::json!({ "target_open": 0.0 }),
    );

    Ok(())
}

/// 获取朗读状态
///
/// 查询 SpeechPlanner 中该角色是否正在说话。
#[tauri::command]
pub async fn get_speaking_status(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<bool, String> {
    let character = state.get_character(character_id.as_deref())?;
    let speaker_id = character.id.clone();

    let planner = get_planner().await;
    Ok(planner.is_speaking(&speaker_id))
}

/// 预热 TTS 后端连接
///
/// 在 LLM 流式产出第一个 token 时调用,提前建立 Edge WSS / GPT-SoVITS HTTP 连接。
/// LLM 结束后 speak_text 时可直接复用连接,省去 100-300ms 连接建立时间。
#[tauri::command]
pub async fn prewarm_tts(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let tts = state.get_character(character_id.as_deref())?.brain.tts.clone();
    if !tts.is_enabled() {
        return Ok(());
    }
    tts.prewarm().await.map_err(|e| {
        tracing::debug!("[TTS] prewarm 失败(非致命): {}", e);
        e.to_string()
    })?;
    tracing::debug!("[TTS] prewarm 完成");
    Ok(())
}

/// 预合成文本（只写入缓存，不播放）
///
/// 前端在播放当前句子时调用此命令预合成下一句，让后续 speak_text 命中缓存
/// 直接播放，消除句间合成延迟（200-500ms → 0ms）。
///
/// 与 speak_text 共享相同的缓存键（text + voice + engine + rate + volume + pitch），
/// 确保 speak_text 能命中预合成写入的缓存。
#[tauri::command]
pub async fn prefetch_tts(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    text: String,
    emotion: Option<String>,
) -> Result<(), String> {
    let tts = state.get_character(character_id.as_deref())?.brain.tts.clone();
    if !tts.is_enabled() || text.trim().is_empty() {
        return Ok(());
    }

    tts.prefetch(&text, emotion.as_deref())
        .await
        .map_err(|e| e.to_string())
}

// ── GPT-SoVITS 服务一键部署 ──
//
// 通过子进程启动 GPT-SoVITS 仓库根目录下的 `api_v2.py` 推理 API 服务,
// 默认监听 127.0.0.1:9880,与 TTS 后端的 /tts 调用直连。
// 启动参数全部来自 TtsConfig 中的 gpt_sovits_* 字段(可在设置面板配置)。

/// 一键启动 GPT-SoVITS api_v2.py 服务
///
/// 启动参数取自当前 TtsConfig 的 gpt_sovits_* 字段(安装路径/模型/GPU/端口/参考音频等)。
/// 启动后异步等待健康检查通过(默认 60s 超时);前端可轮询 `get_gpt_sovits_service_status`
/// 获取最新状态,状态变化通过 `gpt_sovits:status` 事件推送。
#[tauri::command]
pub async fn start_gpt_sovits_service(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    tracing::info!("[GPT-SoVITS] 收到启动请求 character_id={:?}", character_id);
    // 取当前 TTS 配置
    let config = state.get_character(character_id.as_deref())?.brain.tts.get_config();

    let svc = gpt_sovits_service().await;
    let new_state = svc.start(&config).await.map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(new_state).map_err(|e| e.to_string())?)
}

/// 停止 GPT-SoVITS 服务
#[tauri::command]
pub async fn stop_gpt_sovits_service() -> Result<Value, String> {
    let svc = gpt_sovits_service().await;
    let new_state = svc.stop().await.map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(new_state).map_err(|e| e.to_string())?)
}

/// 查询 GPT-SoVITS 服务状态
///
/// 内部会先调用 `refresh()` 检查子进程是否仍存活(防止状态失真),
/// 再返回当前 ServiceState。前端可定时轮询此接口(建议 2s 一次)。
#[tauri::command]
pub async fn get_gpt_sovits_service_status() -> Result<Value, String> {
    let svc = gpt_sovits_service().await;
    let cur = svc.refresh().await;
    Ok(serde_json::to_value(cur).map_err(|e| e.to_string())?)
}

// ── Fish Speech 本地服务管理 ──

/// 启动 Fish Speech 本地服务子进程
///
/// 启动参数取自当前角色的 TtsConfig 的 fish_speech_* 字段
/// (安装路径/Python 路径/端口)。启动成功后自动回写 fish_speech_url。
#[tauri::command]
pub async fn start_fish_speech_service(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<Value, String> {
    tracing::info!("[FishSpeech] 收到启动请求 character_id={:?}", character_id);
    let character = state.get_character(character_id.as_deref())?;
    let config = character.brain.tts.get_config();

    let svc = fish_speech_service().await;
    let new_state = svc.start(&config).await.map_err(|e| e.to_string())?;

    // 启动成功后回写 fish_speech_url 指向本地服务
    if let Some(port) = config.fish_speech_port {
        let endpoint = format!("http://127.0.0.1:{port}");
        let mut updated = config.clone();
        updated.fish_speech_url = Some(endpoint);
        let _ = character.brain.tts.set_config(updated);
        tracing::info!("[FishSpeech] 已自动回写 fish_speech_url -> http://127.0.0.1:{port}");
    }

    Ok(serde_json::to_value(new_state).map_err(|e| e.to_string())?)
}

/// 停止 Fish Speech 服务
#[tauri::command]
pub async fn stop_fish_speech_service() -> Result<Value, String> {
    let svc = fish_speech_service().await;
    let new_state = svc.stop().await.map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(new_state).map_err(|e| e.to_string())?)
}

/// 查询 Fish Speech 服务状态
#[tauri::command]
pub async fn get_fish_speech_service_status() -> Result<Value, String> {
    let svc = fish_speech_service().await;
    let cur = svc.refresh().await;
    Ok(serde_json::to_value(cur).map_err(|e| e.to_string())?)
}

/// 测试翻译服务
///
/// 使用当前角色的 TTS 配置（display_language / tts_language / translation_*）翻译给定文本。
/// LLM 翻译使用路由矩阵中 translation 任务的 provider 配置。
#[tauri::command]
pub async fn test_translation(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    text: String,
) -> Result<String, String> {
    let character = state.get_character(character_id.as_deref())?;
    let config = character.brain.tts.get_config();

    let from = config.display_language.as_deref().ok_or("未设置显示语言")?;
    let to = config.tts_language.as_deref().ok_or("未设置 TTS 语言")?;
    let provider = config.translation_provider.as_deref().ok_or("未设置翻译服务")?;

    let svc = crate::translation::translation_service().await;
    if provider == "llm" {
        let router = character.brain.router.clone();
        svc.translate_llm(&text, from, to, &router)
            .await
            .map_err(|e| e.to_string())
    } else {
        let api_key = config.translation_api_key.as_deref().ok_or("未设置翻译 API Key")?;
        svc.translate(&text, from, to, provider, api_key, config.translation_endpoint.as_deref())
            .await
            .map_err(|e| e.to_string())
    }
}

/// 扫描 GPT-SoVITS 安装目录下的模型文件
///
/// 模仿 GPT-SoVITS WebUI 的 `get_weights_names`(见 `config.py`):
/// 1. 预训练底模字典(写死的几个路径,检查存在性)
/// 2. 训练输出目录 `GPT_weights*/` / `SoVITS_weights*/`(用户训练产物)
///
/// SoVITS 过滤 `s2D*.pth`(discriminator,推理用不上),只保留 generator。
/// 同时检测 `runtime/python.exe` 是否存在(整合包标志)。
///
/// `install_path` 参数由前端直接传入(当前 state 中的值),
/// 避免前后端配置不同步导致扫描到旧路径。
#[tauri::command]
pub fn list_gpt_sovits_models(
    install_path: String,
) -> Result<Value, String> {
    let install_path = install_path.trim();
    if install_path.is_empty() {
        return Err("未配置 GPT-SoVITS 安装路径".to_string());
    }

    let root = std::path::Path::new(install_path);
    if !root.is_dir() {
        return Err(format!("安装路径不存在: {}", install_path));
    }

    // 与 GPT-SoVITS/config.py 的 pretrained_gpt_name / pretrained_sovits_name 对齐
    let pretrained_gpt: &[(&str, &str)] = &[
        ("v1", "GPT_SoVITS/pretrained_models/s1bert25hz-2kh-longer-epoch=68e-step=50232.ckpt"),
        ("v2", "GPT_SoVITS/pretrained_models/gsv-v2final-pretrained/s1bert25hz-5kh-longer-epoch=12-step=369668.ckpt"),
        ("v3", "GPT_SoVITS/pretrained_models/s1v3.ckpt"),
    ];
    let pretrained_sovits: &[(&str, &str)] = &[
        ("v1", "GPT_SoVITS/pretrained_models/s2G488k.pth"),
        ("v2", "GPT_SoVITS/pretrained_models/gsv-v2final-pretrained/s2G2333k.pth"),
        ("v3", "GPT_SoVITS/pretrained_models/s2Gv3.pth"),
    ];

    let mut gpt_models = Vec::new();
    let mut sovits_models = Vec::new();

    // 1. 预训练底模
    for (ver, rel) in pretrained_gpt {
        let p = root.join(rel);
        if p.is_file() {
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            gpt_models.push(serde_json::json!({
                "name": format!("{} ({})", ver, fname),
                "path": path_to_str(&p),
            }));
        }
    }
    for (ver, rel) in pretrained_sovits {
        let p = root.join(rel);
        if p.is_file() {
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            sovits_models.push(serde_json::json!({
                "name": format!("{} ({})", ver, fname),
                "path": path_to_str(&p),
            }));
        }
    }

    // 2. 训练输出目录(根目录下的 GPT_weights*/  SoVITS_weights*/)
    let mut train_dirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("GPT_weights") || name.starts_with("SoVITS_weights") {
                train_dirs.push(path);
            }
        }
    }
    for dir in &train_dirs {
        let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_gpt_dir = dir_name.starts_with("GPT_weights");
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if is_gpt_dir && ext.eq_ignore_ascii_case("ckpt") {
                    gpt_models.push(serde_json::json!({
                        "name": format!("{}/{}", dir_name, fname),
                        "path": path_to_str(&path),
                    }));
                } else if !is_gpt_dir && ext.eq_ignore_ascii_case("pth") {
                    // 跳过 discriminator (s2D*.pth),只保留 generator
                    if !fname.to_lowercase().starts_with("s2d") {
                        sovits_models.push(serde_json::json!({
                            "name": format!("{}/{}", dir_name, fname),
                            "path": path_to_str(&path),
                        }));
                    }
                }
            }
        }
    }

    // 3. 检测整合包 runtime(runtime/python.exe)
    let runtime_python = root.join("runtime").join("python.exe");
    let has_runtime = runtime_python.is_file();

    Ok(serde_json::json!({
        "gpt_models": gpt_models,
        "sovits_models": sovits_models,
        "has_runtime": has_runtime,
    }))
}

/// 路径转字符串(统一用正斜杠,避免后端 JSON 转义)
fn path_to_str(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

