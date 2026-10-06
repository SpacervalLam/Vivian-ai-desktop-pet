//! TTS 命令 - 语音合成配置与朗读
//!
//! TTS 事件回调(`tts:started` / `tts:word` / `tts:finished` / `tts:error` / `tts:fallback`)
//! 在 `speak_text` 期间向前端推送。

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::speech::{
    fish_speech_service, get_planner, gpt_sovits_service, speak_intent, Presentation,
    SpeechPriority, TtsConfig, TtsEvent, TtsEventCallback,
};
use crate::state::AppState;

/// 应用全局语音开关后的副作用决策（纯函数，便于单测）
///
/// - `needs_gpt_sovits` / `needs_fish_speech`：是否需要拉起本地推理服务
/// - `changed`：配置是否真的发生了变化（未变化时不重复落盘）
#[derive(Debug, Default, PartialEq, Eq)]
struct EnableOutcome {
    changed: bool,
    needs_gpt_sovits: bool,
    needs_fish_speech: bool,
}

/// 把目标 `enabled` 应用到单个角色的 TTS 配置上，返回是否需要变更
fn apply_enabled(config: &mut TtsConfig, enabled: bool) -> EnableOutcome {
    let mut outcome = EnableOutcome::default();
    if config.enabled != enabled {
        config.enabled = enabled;
        outcome.changed = true;
    }
    if enabled {
        // 无论配置是否变化都要判定：托盘重复点击同一目标值时，
        // 服务可能已停（如用户手动停过），需要重新拉起
        outcome.needs_gpt_sovits = config.should_auto_start_gpt_sovits();
        outcome.needs_fish_speech = config.should_auto_start_fish_speech();
    }
    outcome
}

/// 全局启用/禁用所有角色的语音朗读（托盘「语音开关」的唯一入口）
///
/// 托盘是全局入口，不绑定单个角色窗口，因此这里对**所有**已加载角色统一写入
/// `TtsConfig.enabled` 并持久化到各自的 config.json。后端配置是朗读的唯一真相源：
/// 前端不再持有独立的静音开关（否则会出现"菜单显示开但后端拒绝合成"的双轨状态）。
///
/// 启用时按各角色配置的 `should_auto_start_*` 判定拉起本地推理服务
/// （GPT-SoVITS / Fish Speech 常驻数 GB 内存，只在真正启用时才拉起）；
/// 禁用时打断所有正在进行的朗读。
#[tauri::command]
pub async fn set_tts_enabled_all(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    // 1) 写入所有角色的 TTS 配置（先落盘，失败则不触发服务启停）
    // 服务为全局单例，多角色都要求启动时按首次命中的配置即可
    let mut gpt_sovits_cfg: Option<TtsConfig> = None;
    let mut fish_speech_cfg: Option<TtsConfig> = None;

    {
        let characters = state.characters.read();
        for character in characters.values() {
            let tts = &character.brain.tts;
            let mut config = tts.get_config();
            let outcome = apply_enabled(&mut config, enabled);
            if outcome.changed {
                tts.set_config(config.clone()).map_err(|e| e.to_string())?;
                tracing::info!(
                    "[TTS] 全局语音开关：角色 {} 的 enabled -> {}",
                    character.id,
                    enabled
                );
            }
            if outcome.needs_gpt_sovits && gpt_sovits_cfg.is_none() {
                gpt_sovits_cfg = Some(config.clone());
            }
            if outcome.needs_fish_speech && fish_speech_cfg.is_none() {
                fish_speech_cfg = Some(config.clone());
            }
        }
    }

    if enabled {
        // 2a) 启用：按需拉起本地推理服务（start 内部后台做健康检查，不阻塞）
        if let Some(config) = gpt_sovits_cfg {
            let svc = gpt_sovits_service().await;
            match svc.start(&config).await {
                Ok(s) => tracing::info!("[TTS] GPT-SoVITS 已按需启动: {:?}", s.status),
                Err(e) => tracing::warn!("[TTS] GPT-SoVITS 启动失败: {e}"),
            }
        }
        if let Some(config) = fish_speech_cfg {
            let svc = fish_speech_service().await;
            match svc.start(&config).await {
                Ok(s) => tracing::info!("[TTS] Fish Speech 已按需启动: {:?}", s.status),
                Err(e) => tracing::warn!("[TTS] Fish Speech 启动失败: {e}"),
            }
        }
    } else {
        // 2b) 禁用：打断正在朗读的语音（否则当前这句话会播完才停）
        let planner = get_planner().await;
        let _ = planner.stop_all().await;
        state.playback_gate.mark_finished();
    }

    // 3) 通知所有窗口同步（多角色窗口共享同一份 enabled 真相）
    let _ = app.emit("tts:config-changed", serde_json::json!({ "enabled": enabled }));
    Ok(())
}

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
        "[TTS] 当前配置: enabled={} engine={:?} volume={} rate={} display_lang={:?} tts_lang={:?} trans_provider={:?}",
        config_snapshot.enabled,
        config_snapshot.engine,
        config_snapshot.volume,
        config_snapshot.rate,
        config_snapshot.display_language,
        config_snapshot.tts_language,
        config_snapshot.translation_provider
    );

    // 跨语言翻译：display_language 与 tts_language 不同时，先翻译文本再送 TTS
    // 当 tts_language 已配置但 display_language 缺失时，默认 display_language = "zh"
    // （角色主要使用中文对话，用户配置了 tts_language=ja 显然是想翻译中文到日语）
    let display_lang = config_snapshot
        .display_language
        .clone()
        .or_else(|| config_snapshot.tts_language.as_ref().map(|_| "zh".to_string()));
    let tts_text = if let (Some(from), Some(to)) =
        (display_lang.as_deref(), config_snapshot.tts_language.as_deref())
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
            TtsEvent::WordBoundary { .. } => {
                let _ = app_for_cb.emit("tts:word", payload);
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

    // 通过 SpeechPlanner 调度
    let mut builder = speak_intent(&tts_text, &speaker_id)
        .emotion(emotion.unwrap_or_default())
        .priority(SpeechPriority::Normal);
    if let Some(pres) = presentation {
        builder = builder.presentation(pres);
    }
    let intent = builder.build();

    let planner = get_planner().await;
    // 自愈:确保 Planner 使用的正是上面刚检查过 enabled 的那个 TtsManager 实例。
    // reinitialize 重建角色后 Planner 可能仍持有旧实例(旧实例 enabled 可能是 false),
    // 若不校正,这里提交的 intent 会在 Planner 侧被判为"未启用"而静默丢弃。
    planner.ensure_registered(&speaker_id, tts.clone()).await;
    let handle = planner.submit(intent).await.map_err(|e| e.to_string())?;
    let result = match handle.done().await {
        crate::speech::SubmitResult::Played => Ok(()),
        crate::speech::SubmitResult::Disabled => {
            tracing::info!("[TTS] 角色 {} 的 TTS 未启用,跳过朗读", speaker_id);
            Ok(())
        }
        crate::speech::SubmitResult::Dropped => {
            tracing::info!("[TTS] intent 被丢弃(让路或被抢占)");
            Ok(())
        }
        crate::speech::SubmitResult::Failed(msg) => Err(msg),
    };

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
    _app: AppHandle,
) -> Result<(), String> {
    let character = state.get_character(character_id.as_deref())?;
    let speaker_id = character.id.clone();
    let tts = character.brain.tts.clone();

    let planner = get_planner().await;
    // 自愈:确保停止的是当前角色真正在用的后端实例(而非重建前的旧实例)
    planner.ensure_registered(&speaker_id, tts).await;
    planner
        .stop_speaker(&speaker_id)
        .await
        .map_err(|e| e.to_string())?;

    state.playback_gate.mark_finished();

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

#[cfg(test)]
mod global_voice_switch_tests {
    use super::apply_enabled;
    use crate::speech::{TtsConfig, TtsEngine};

    #[test]
    fn enabling_turns_on_every_role_config() {
        let mut config = TtsConfig::default();
        assert!(!config.enabled);

        let outcome = apply_enabled(&mut config, true);

        assert!(config.enabled, "托盘启用必须直接改写后端配置");
        assert!(outcome.changed);
    }

    #[test]
    fn disabling_turns_off_every_role_config() {
        let mut config = TtsConfig::default();
        config.enabled = true;

        let outcome = apply_enabled(&mut config, false);

        assert!(!config.enabled);
        assert!(outcome.changed);
    }

    #[test]
    fn repeated_toggle_is_idempotent_and_skips_persist() {
        let mut config = TtsConfig::default();
        config.enabled = true;

        // 重复点同一目标值：状态已一致，不需再次落盘
        let outcome = apply_enabled(&mut config, true);

        assert!(config.enabled);
        assert!(!outcome.changed, "配置未变化时不应重复写盘");
    }

    #[test]
    fn enabling_starts_gpt_sovits_only_when_auto_start_configured() {
        let mut config = TtsConfig::default();
        config.engine = TtsEngine::GptSoVits;
        config.gpt_sovits_auto_start = true;
        config.gpt_sovits_install_path = Some("C:\\GPT-SoVITS".to_string());

        let outcome = apply_enabled(&mut config, true);

        assert!(outcome.needs_gpt_sovits, "托盘启用应按需拉起本地模型");
        assert!(!outcome.needs_fish_speech);
    }

    #[test]
    fn enabling_without_auto_start_never_spawns_local_service() {
        let mut config = TtsConfig::default();
        config.engine = TtsEngine::GptSoVits;
        // auto_start 未开：托盘勾选只改 enabled，不擅自拉起占用数 GB 内存的本地服务
        config.gpt_sovits_install_path = Some("C:\\GPT-SoVITS".to_string());

        let outcome = apply_enabled(&mut config, true);

        assert!(!outcome.needs_gpt_sovits);
        assert!(!outcome.needs_fish_speech);
    }

    #[test]
    fn disabling_never_starts_services_and_keeps_existing_choice() {
        let mut config = TtsConfig::default();
        config.enabled = true;
        config.engine = TtsEngine::FishSpeech;
        config.fish_speech_auto_start = true;
        config.fish_speech_install_path = Some("C:\\fish-speech".to_string());

        let outcome = apply_enabled(&mut config, false);

        assert!(!outcome.needs_fish_speech, "禁用路径不应触发服务启动");
        assert!(!outcome.needs_gpt_sovits);
        // 禁用不应抹掉用户此前配置的 auto_start 偏好
        assert!(config.fish_speech_auto_start);
    }
}

