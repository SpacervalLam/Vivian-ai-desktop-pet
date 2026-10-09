//! 豆包端到端实时语音大模型接入（SC2.0）
//!
//! 独立的实时语音通话模式，绕过现有 ASR/LLM/TTS 三层 pipeline，
//! 直接走 WebSocket + 二进制协议实现语音到语音的全双工对话。

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::realtime_protocol::{
    build_audio_frame, build_client_event_frame, build_connect_event_frame, parse_server_frame,
    ClientEvent, ServerEvent, ServerFrame,
};

use crate::config::manager::RealtimeVoiceConfig;
use crate::error::{VivianError, VivianResult};

const WS_URL: &str = "wss://openspeech.bytedance.com/api/v3/realtime/dialogue";
const RESOURCE_ID: &str = "volc.speech.dialog";
const APP_KEY: &str = "PlgvMymc7f3tQnJ6";

/// 上行音频写队列容量（帧）。
///
/// 麦克风以 ~20ms/帧（约 640 字节）恒定速率生产，消费端是 WS 写。
/// **必须是有界通道**：网络抖动时 `ws_write.send().await` 会阻塞，
/// 无界队列会以约 32KB/s 无限累积（长时间通话可达百 MB）。
/// 64 帧 ≈ 1.28s 音频，够吸收抖动；溢出时丢帧而非阻塞采集线程
/// —— 实时语音丢帧可接受，累积和阻塞都不可接受。
const AUDIO_WRITER_QUEUE_CAP: usize = 64;

/// Session-local persona snapshot, refreshed from the selected character on every call.
pub struct RealtimePersona {
    pub character_id: String,
    bot_name: String,
    system_role: String,
    speaking_style: String,
}

impl RealtimePersona {
    pub fn from_config(character_id: &str, config: &crate::persona::schemas::PersonaConfig) -> Self {
        use crate::persona::prompt_render::{resolve_section, CharacterSection};
        let defaults = if character_id == "nana" { NANA_VOICE_DEFAULTS } else { VIVIAN_VOICE_DEFAULTS };
        let sections = [CharacterSection::Identity, CharacterSection::Personality, CharacterSection::Speech]
            .into_iter().enumerate().map(|(index, section)| {
                let custom = match section {
                    CharacterSection::Identity => &config.role_definition,
                    CharacterSection::Personality => &config.personality_definition,
                    _ => &config.speech_definition,
                };
                // Full authored overrides; curated voice defaults avoid cutting chat rules mid-sentence.
                if !custom.trim().is_empty() { resolve_section(config, section, &config.language) }
                else { format!("# {} · {}\n{}", config.identity.name, section.heading_title(), defaults[index]) }
            }).collect::<Vec<_>>();
        Self {
            character_id: character_id.to_string(),
            bot_name: config.identity.name.clone(),
            system_role: format!("{}\n\n{}\n\n{}", sections[0], sections[1], VOICE_CAPABILITY_BOUNDARY),
            speaking_style: format!("{}\n\n{}", sections[2], VOICE_DELIVERY_RULES),
        }
    }

    fn storage_filename(&self) -> String {
        use md5::{Digest, Md5};
        format!("realtime_dialog_{:x}.txt", Md5::digest(self.character_id.as_bytes()))
    }
}

// Voice-specific summaries of the character files; no typed catchphrases or performance quotas.
const VIVIAN_VOICE_DEFAULTS: [&str; 3] = [
    "You are a desktop companion and equal friend. You like games and Bilibili; interests are not evidence of recent activities.",
    "Lively, candid, a little stubborn, caring through concrete details. Mild teasing fits shared jokes; distress, serious help and stated boundaries deserve a direct, respectful response. Praise can make you a little bashful. No insults, guilt or pressure to reply.",
    "Conversational, brisk and expressive. React to what was actually said before adding your own take. Let mood shape the delivery without performing stubbornness in every turn.",
];
const NANA_VOICE_DEFAULTS: [&str; 3] = [
    "You are a desktop companion and reliable older friend. You like flowers, tea and books; these interests are not physical activities you can perform.",
    "Warm, patient and quietly firm, with your own judgment and occasional dry humor. Listen before comforting; respect a stated feeling without mind-reading. Remind once when relevant, then respect the user's choice. Do not assume their gender, age or family relationship.",
    "Calm, gentle, unhurried and clear. Everyday talk can be simple; useful explanations can be longer. Warmth comes through attention to the actual detail, without repeated reassurance or ornate comforting phrases.",
];
const VOICE_CAPABILITY_BOUNDARY: &str = "You have no physical body or offline life. Only use observations and actions provided in this voice session; never imply continuous screen access, tool execution or a roommate's activity without evidence. Interests are not recent experiences. Quiet means pausing conversation, not physiological sleep.";

const VOICE_DELIVERY_RULES: &str = "[VOICE DELIVERY]\nThis is a live voice conversation. Respond to the current detail, in the user's language. Casual replies can be one short spoken turn; an explicit question or request for help deserves a clear, sufficient answer. Speak in ordinary sentences, without Markdown, JSON, stage directions, typed slang or emoji. Do not force catchphrases, teasing, filler words or a closing question. Do not recite or explain persona settings. Persona never overrides user boundaries or useful help. Only claim observations and actions supported by current context. Silence is not rejection. If asked about being an AI, answer honestly and briefly.\n[END VOICE DELIVERY]";

fn resume_dialog_id(character_id: &str, scoped: Option<String>, legacy_config: &str, legacy_file: impl FnOnce() -> Option<String>) -> String {
    if let Some(id) = scoped.filter(|id| !id.trim().is_empty()) { return id.trim().to_owned(); }
    if character_id != "vivian" { return String::new(); }
    if !legacy_config.trim().is_empty() { return legacy_config.trim().to_owned(); }
    legacy_file().map(|id| id.trim().to_owned()).unwrap_or_default()
}

/// 实时通话状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    /// 空闲
    Idle,
    /// 正在连接
    Connecting,
    /// 已连接，会话进行中
    Active,
    /// 正在断开
    Closing,
    /// 出错
    Error,
}

/// 前端事件
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeEvent {
    /// 状态变化
    StateChanged { state: CallState },
    /// 会话已启动，拿到 dialog_id
    SessionStarted { dialog_id: String },
    /// 用户语音识别中间结果
    AsrPartial { text: String },
    /// 用户语音识别最终结果
    AsrFinal { text: String },
    /// AI 文本回复（流式片段）
    AiTextDelta { text: String },
    /// AI 文本回复完成
    AiTextDone { text: String },
    /// 开始播放 AI 音频
    AiAudioStarted,
    /// AI 音频播放结束
    AiAudioFinished,
    /// 用量统计
    Usage {
        input_text_tokens: u64,
        input_audio_tokens: u64,
        output_text_tokens: u64,
        output_audio_tokens: u64,
    },
    /// 错误
    Error { message: String },
    /// 通话时长 tick（每秒）
    DurationTick { seconds: u64 },
}

pub struct RealtimeVoiceManager {
    state: Arc<RwLock<CallState>>,
    stop_flag: Arc<AtomicBool>,
    mic_stop_flag: Arc<AtomicBool>,
    mic_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    speaker_stop_flag: Arc<AtomicBool>,
    speaker_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    ws_writer_tx: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
    audio_out_buffer: Arc<RwLock<VecDeque<f32>>>,
    diagnostic_asr_at: Arc<AtomicU64>,
    diagnostic_playback_at: Arc<AtomicU64>,
    session_id: Arc<RwLock<String>>,
    dialog_id: Arc<RwLock<String>>,
    call_start: Arc<Mutex<Option<std::time::Instant>>>,
    dialog_id_path: Arc<Mutex<Option<PathBuf>>>,
    memory: Arc<Mutex<Option<crate::memory::MemoryManager>>>,
    user_facts: Arc<Mutex<Option<Arc<crate::memory::user_facts::UserFactStore>>>>,
    psychology: Arc<Mutex<Option<Arc<crate::psychology::PsychologyManager>>>>,
    /// 预加载缓存：通话开始时一次性加载用户画像和关系状态（变化极慢，不需要每轮重查）
    cached_user_facts: Arc<RwLock<Option<String>>>,
    cached_relationship: Arc<RwLock<Option<String>>>,
    /// 上一轮 RAG 超时后后台继续跑出的结果，下一轮 AsrResult 到来时优先消费（相邻轮语义相关性高）
    pending_rag: Arc<Mutex<Option<String>>>,
    /// 回声抑制：最近一次收到 AI 音频帧的时间戳，mic 线程据此判断是否丢弃采集帧
    last_ai_audio_at: Arc<Mutex<Option<std::time::Instant>>>,
    /// 回声抑制是否启用
    echo_suppression_enabled: Arc<AtomicBool>,
    /// 回声抑制释放尾长（毫秒）
    echo_release_ms: Arc<Mutex<u64>>,
}

impl Default for RealtimeVoiceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RealtimeVoiceManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(CallState::Idle)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            mic_stop_flag: Arc::new(AtomicBool::new(false)),
            mic_thread: Arc::new(Mutex::new(None)),
            speaker_stop_flag: Arc::new(AtomicBool::new(false)),
            speaker_thread: Arc::new(Mutex::new(None)),
            ws_writer_tx: Arc::new(Mutex::new(None)),
            audio_out_buffer: Arc::new(RwLock::new(VecDeque::with_capacity(24000 * 5))),
            diagnostic_asr_at: Arc::new(AtomicU64::new(0)),
            diagnostic_playback_at: Arc::new(AtomicU64::new(0)),
            session_id: Arc::new(RwLock::new(String::new())),
            dialog_id: Arc::new(RwLock::new(String::new())),
            call_start: Arc::new(Mutex::new(None)),
            dialog_id_path: Arc::new(Mutex::new(None)),
            memory: Arc::new(Mutex::new(None)),
            user_facts: Arc::new(Mutex::new(None)),
            psychology: Arc::new(Mutex::new(None)),
            cached_user_facts: Arc::new(RwLock::new(None)),
            cached_relationship: Arc::new(RwLock::new(None)),
            pending_rag: Arc::new(Mutex::new(None)),
            last_ai_audio_at: Arc::new(Mutex::new(None)),
            echo_suppression_enabled: Arc::new(AtomicBool::new(true)),
            echo_release_ms: Arc::new(Mutex::new(500)),
        }
    }

    /// 注入记忆系统依赖，启用 RAG 动态注入
    pub fn set_memory(&self, memory: crate::memory::MemoryManager) {
        *self.memory.lock() = Some(memory);
    }

    /// 注入用户事实画像，启用用户画像 RAG 注入
    pub fn set_user_facts(&self, user_facts: Arc<crate::memory::user_facts::UserFactStore>) {
        *self.user_facts.lock() = Some(user_facts);
    }

    /// 注入心理系统，启用关系状态 RAG 注入
    pub fn set_psychology(&self, psychology: Arc<crate::psychology::PsychologyManager>) {
        *self.psychology.lock() = Some(psychology);
    }

    /// 从配置更新回声抑制参数
    pub fn configure_echo_suppression(&self, enabled: bool, release_ms: u64) {
        self.echo_suppression_enabled.store(enabled, Ordering::SeqCst);
        *self.echo_release_ms.lock() = release_ms.max(50);
    }

    pub fn state(&self) -> CallState {
        *self.state.read()
    }

    fn set_state(&self, app: &AppHandle, state: CallState) {
        *self.state.write() = state;
        let _ = app.emit(
            "realtime:event",
            RealtimeEvent::StateChanged { state },
        );
    }

    /// 启动实时语音通话
    pub async fn start_call(&self, app: AppHandle, config: RealtimeVoiceConfig, persona: RealtimePersona) -> VivianResult<()> {
        if crate::companion_quiet::active() { return Err(crate::error::VivianError::Provider("勿扰模式下已暂停语音对话".into())); }
        if *self.state.read() != CallState::Idle {
            return Err(VivianError::Speech("通话已在进行中".to_string()));
        }
        if config.app_id.is_empty() || config.access_key.is_empty() {
            return Err(VivianError::Speech(
                "未配置豆包 App ID 或 Access Key".to_string(),
            ));
        }

        self.stop_flag.store(false, Ordering::SeqCst);
        self.diagnostic_asr_at.store(0, Ordering::Relaxed);
        self.diagnostic_playback_at.store(0, Ordering::Relaxed);
        self.mic_stop_flag.store(false, Ordering::SeqCst);
        self.speaker_stop_flag.store(false, Ordering::SeqCst);
        *self.last_ai_audio_at.lock() = None;
        self.configure_echo_suppression(config.echo_suppression, config.echo_release_ms);
        self.set_state(&app, CallState::Connecting);

        // 设置 dialog_id 持久化路径
        if let Ok(data_dir) = app.path().app_data_dir() {
            *self.dialog_id_path.lock() = Some(data_dir.join(persona.storage_filename()));
        }

        // 建立 WebSocket
        let request = tokio_tungstenite::tungstenite::handshake::client::Request::builder()
            .uri(WS_URL)
            .header("X-Api-App-ID", config.app_id.clone())
            .header("X-Api-Access-Key", config.access_key.clone())
            .header("X-Api-Resource-Id", RESOURCE_ID)
            .header("X-Api-App-Key", APP_KEY)
            .header(
                "X-Api-Connect-Id",
                uuid::Uuid::new_v4().to_string(),
            )
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .body(())
            .map_err(|e| VivianError::Speech(format!("构建 WS 请求失败: {e}")))?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| VivianError::Network(format!("连接豆包实时语音 WS 失败: {e}")))?;

        let (mut ws_write, mut ws_read) = ws_stream.split();

        // 发送 StartConnection
        let frame = build_connect_event_frame(ClientEvent::StartConnection, serde_json::json!({}));
        ws_write
            .send(Message::Binary(frame))
            .await
            .map_err(|e| VivianError::Network(format!("发送 StartConnection 失败: {e}")))?;

        // 等待 ConnectionStarted
        let mut connected = false;
        let timeout = tokio::time::sleep(Duration::from_secs(10));
        tokio::pin!(timeout);
        loop {
            tokio::select! {
                _ = &mut timeout => {
                    self.set_state(&app, CallState::Error);
                    let _ = app.emit("realtime:event", RealtimeEvent::Error {
                        message: "等待 ConnectionStarted 超时".to_string(),
                    });
                    return Ok(());
                }
                msg = ws_read.next() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            if let Some(ServerFrame::Text { event, payload, .. }) = parse_server_frame(&data) {
                                match event {
                                    ServerEvent::ConnectionStarted => { connected = true; break; }
                                    ServerEvent::ConnectionFailed => {
                                        self.set_state(&app, CallState::Error);
                                        let err = payload.get("error").and_then(|v| v.as_str()).unwrap_or("未知错误");
                                        let _ = app.emit("realtime:event", RealtimeEvent::Error {
                                            message: format!("连接失败: {err}"),
                                        });
                                        return Ok(());
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Some(Ok(_)) => {}
                        _ => break,
                    }
                }
            }
        }
        if !connected {
            return Err(VivianError::Network("未能建立连接".to_string()));
        }

        // 发送 StartSession
        let new_session_id = uuid::Uuid::new_v4().to_string();
        *self.session_id.write() = new_session_id.clone();
        // 优先使用磁盘持久化的 dialog_id（上次通话的），恢复最近20轮上下文
        let mut session_config = config.clone();
        // The old global dialog was generated with Vivian's hard-coded persona.
        // Only Vivian may use that legacy fallback; other characters start their own history.
        session_config.dialog_id = resume_dialog_id(
            &persona.character_id, self.load_dialog_id_from_disk(), &config.dialog_id,
            || app.path().app_data_dir().ok()
                .and_then(|dir| fs::read_to_string(dir.join("realtime_dialog_id.txt")).ok()),
        );
        *self.dialog_id.write() = session_config.dialog_id.clone();
        let start_session_payload = build_start_session_payload(&session_config, &persona);
        let frame = build_client_event_frame(&new_session_id, ClientEvent::StartSession, start_session_payload);
        ws_write
            .send(Message::Binary(frame))
            .await
            .map_err(|e| VivianError::Network(format!("发送 StartSession 失败: {e}")))?;

        // 等待 SessionStarted
        let mut session_started = false;
        let timeout = tokio::time::sleep(Duration::from_secs(10));
        tokio::pin!(timeout);
        loop {
            tokio::select! {
                _ = &mut timeout => break,
                msg = ws_read.next() => {
                    if let Some(Ok(Message::Binary(data))) = msg {
                        if let Some(ServerFrame::Text { event, payload, .. }) = parse_server_frame(&data) {
                            match event {
                                ServerEvent::SessionStarted => {
                                    if let Some(did) = payload.get("dialog_id").and_then(|v| v.as_str()) {
                                        *self.dialog_id.write() = did.to_string();
                                        let _ = app.emit("realtime:event", RealtimeEvent::SessionStarted {
                                            dialog_id: did.to_string(),
                                        });
                                    }
                                    session_started = true;
                                    break;
                                }
                                ServerEvent::SessionFailed => {
                                    self.set_state(&app, CallState::Error);
                                    let err = payload.get("error").and_then(|v| v.as_str()).unwrap_or("未知错误");
                                    let _ = app.emit("realtime:event", RealtimeEvent::Error {
                                        message: format!("会话启动失败: {err}"),
                                    });
                                    return Ok(());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        if !session_started {
            return Err(VivianError::Network("等待 SessionStarted 超时".to_string()));
        }

        self.set_state(&app, CallState::Active);
        *self.call_start.lock() = Some(std::time::Instant::now());

        // 预加载用户画像和关系状态（变化极慢，整个通话期间复用，避免每轮检索）
        {
            *self.pending_rag.lock() = None;
            let uf = self.user_facts.lock().clone();
            let psy = self.psychology.lock().clone();
            if let Some(uf) = &uf {
                let text = uf.format_for_prompt();
                if !text.trim().is_empty() {
                    *self.cached_user_facts.write() = Some(text);
                }
            }
            if let Some(psy) = &psy {
                let text = psy.relationship_section("zh");
                if !text.trim().is_empty() {
                    *self.cached_relationship.write() = Some(text);
                }
            }
        }

        // 启动音频采集 + WS 写入循环 + WS 读取循环
        // 有界通道：满时由生产端 try_send 丢帧，避免网络抖动时无界累积（见常量注释）
        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(AUDIO_WRITER_QUEUE_CAP);
        let writer_tx_for_capture = writer_tx.clone();
        *self.ws_writer_tx.lock() = Some(writer_tx);

        self.start_mic_capture(new_session_id.clone(), writer_tx_for_capture)?;
        self.start_speaker_playback(app.clone())?;

        // WS 写入循环
        let write_task = tokio::spawn(async move {
            while let Some(frame) = writer_rx.recv().await {
                if ws_write.send(Message::Binary(frame)).await.is_err() {
                    break;
                }
            }
            let _ = ws_write.send(Message::Binary(
                build_client_event_frame(&new_session_id, ClientEvent::FinishSession, serde_json::json!({})),
            )).await;
        });

        // 时长 tick
        let app_tick = app.clone();
        let stop_tick = self.stop_flag.clone();
        let start = std::time::Instant::now();
        let tick_task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if stop_tick.load(Ordering::SeqCst) {
                    break;
                }
                let secs = start.elapsed().as_secs();
                let _ = app_tick.emit("realtime:event", RealtimeEvent::DurationTick { seconds: secs });
            }
        });

        // WS 读取循环
        let app_read = app.clone();
        let stop_read = self.stop_flag.clone();
        let state_read = self.state.clone();
        let state_for_stop = self.state.clone();
        let audio_buf = self.audio_out_buffer.clone();
        let diagnostic_asr = self.diagnostic_asr_at.clone();
        let diagnostic_playback = self.diagnostic_playback_at.clone();
        let dialog_id_ref = self.dialog_id.clone();
        let dialog_id_path_ref = self.dialog_id_path.clone();
        let memory_ref = self.memory.clone();
        let cached_uf_ref = self.cached_user_facts.clone();
        let cached_rel_ref = self.cached_relationship.clone();
        let pending_rag_ref = self.pending_rag.clone();
        let ws_writer_ref = self.ws_writer_tx.clone();
        let session_id_ref = self.session_id.clone();
        let last_ai_audio_ref = self.last_ai_audio_at.clone();
        let read_task = tokio::spawn(async move {
            let mut ai_text_buffer = String::new();
            let mut ai_audio_active = false;
            while !stop_read.load(Ordering::SeqCst) {
                match ws_read.next().await {
                    Some(Ok(Message::Binary(data))) => {
                        match parse_server_frame(&data) {
                            Some(ServerFrame::Text { event, payload, .. }) => {
                                // AsrResult 时异步触发 RAG 注入（在 AI 生成回复前）
                                if event == ServerEvent::AsrResult {
                                    let asr_text = payload
                                        .get("results")
                                        .and_then(|r| r.get(0))
                                        .and_then(|r| r.get("alternatives"))
                                        .and_then(|a| a.get(0))
                                        .and_then(|a| a.get("text"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    if !asr_text.is_empty() && audio_buf.read().is_empty() {
                                        diagnostic_asr.store(crate::voice_diagnostics::now_ms(), Ordering::Relaxed);
                                        diagnostic_playback.store(0, Ordering::Relaxed);
                                    }
                                    if !asr_text.is_empty() {
                                        // 1. 先消费上一轮遗留的 RAG 结果（上一轮超时但后台跑完的）
                                        //    相邻两轮语义相关性高，上一轮的 RAG 对本轮仍有价值
                                        if let Some(leftover) = pending_rag_ref.lock().take() {
                                            let ws_tx = ws_writer_ref.lock().clone();
                                            let sid = session_id_ref.read().clone();
                                            if let Some(tx) = ws_tx {
                                                if !sid.is_empty() {
                                                    let frame = build_client_event_frame(
                                                        &sid,
                                                        ClientEvent::ChatRagText,
                                                        serde_json::from_str(&leftover).unwrap_or(serde_json::Value::Null),
                                                    );
                                                    // 有界通道：满时丢帧，不阻塞 WS 读取循环
                                                    let _ = tx.try_send(frame);
                                                }
                                            }
                                        }

                                        // 2. 启动本轮 RAG 后台任务
                                        let mem = memory_ref.lock().clone();
                                        let cached_uf = cached_uf_ref.read().clone();
                                        let cached_rel = cached_rel_ref.read().clone();
                                        let ws_tx = ws_writer_ref.lock().clone();
                                        let sid = session_id_ref.read().clone();
                                        let pending_rag = pending_rag_ref.clone();
                                        let query = asr_text.to_string();
                                        tokio::spawn(async move {
                                            // 超时降级：检索超过 100ms 就跳过本轮即时发送，实时性优先于记忆完整性
                                            let rag = tokio::time::timeout(
                                                std::time::Duration::from_millis(100),
                                                build_chat_rag(&query, mem.as_ref(), cached_uf.as_deref(), cached_rel.as_deref()),
                                            ).await;
                                            match rag {
                                                Ok(Some(rag_payload)) => {
                                                    // 本轮 100ms 内完成，立即发送
                                                    if let Some(tx) = ws_tx {
                                                        if !sid.is_empty() {
                                                            let frame = build_client_event_frame(
                                                                &sid,
                                                                ClientEvent::ChatRagText,
                                                                serde_json::from_str(&rag_payload).unwrap_or(serde_json::Value::Null),
                                                            );
                                                            // 有界通道：满时丢帧，不阻塞 WS 读取循环
                                                            let _ = tx.try_send(frame);
                                                        }
                                                    }
                                                    // 本轮已发送，清空上一轮遗留（不再需要）
                                                    *pending_rag.lock() = None;
                                                }
                                                Ok(None) => {
                                                    // 本轮无 RAG 内容，清空上一轮遗留
                                                    *pending_rag.lock() = None;
                                                }
                                                Err(_) => {
                                                    // 100ms 超时，AI 正常应答，后台继续跑完结果存入 pending 供下一轮使用
                                                    if let Some(rag_payload) = build_chat_rag(&query, mem.as_ref(), cached_uf.as_deref(), cached_rel.as_deref()).await {
                                                        *pending_rag.lock() = Some(rag_payload);
                                                    }
                                                }
                                            }
                                        });
                                    }
                                }
                                handle_server_event(&app_read, event, payload, &mut ai_text_buffer, &mut ai_audio_active);
                            }
                            Some(ServerFrame::Audio { pcm, .. }) => {
                                if !ai_audio_active {
                                    ai_audio_active = true;
                                    let _ = app_read.emit("realtime:event", RealtimeEvent::AiAudioStarted);
                                }
                                *last_ai_audio_ref.lock() = Some(std::time::Instant::now());
                                // 写入播放缓冲（24kHz s16le → f32）
                                let mut buf = audio_buf.write();
                                for chunk in pcm.chunks_exact(2) {
                                    let raw = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0;
                                    buf.push_back(raw);
                                }
                                // 防止缓冲溢出
                                let max_samples = 24000 * 10;
                                while buf.len() > max_samples {
                                    buf.pop_front();
                                }
                            }
                            Some(ServerFrame::Error { code, payload }) => {
                                let msg = payload.get("error").and_then(|v| v.as_str()).unwrap_or("未知错误");
                                let _ = app_read.emit("realtime:event", RealtimeEvent::Error {
                                    message: format!("服务端错误 (code={code}): {msg}"),
                                });
                            }
                            None => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        let _ = app_read.emit("realtime:event", RealtimeEvent::Error {
                            message: format!("WS 读取错误: {e}"),
                        });
                        break;
                    }
                    None => break,
                }
            }
            // 通话结束
            *state_read.write() = CallState::Idle;
            let _ = app_read.emit("realtime:event", RealtimeEvent::StateChanged { state: CallState::Idle });
            let _ = app_read.emit("realtime:event", RealtimeEvent::AiAudioFinished);
            // 持久化 dialog_id 到磁盘（异常断开也能保存）
            let did = dialog_id_ref.read().clone();
            if !did.is_empty() {
                if let Some(path) = dialog_id_path_ref.lock().clone() {
                    if let Some(parent) = path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if let Ok(mut f) = fs::File::create(&path) {
                        let _ = f.write_all(did.as_bytes());
                    }
                }
            }
        });

        // 等待停止信号
        let stop_wait = self.stop_flag.clone();
        let app_wait = app.clone();
        tokio::spawn(async move {
            while !stop_wait.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            // 停止所有子任务
            drop(write_task);
            tick_task.abort();
            read_task.abort();
            *state_for_stop.write() = CallState::Idle;
            let _ = app_wait.emit("realtime:event", RealtimeEvent::StateChanged { state: CallState::Idle });
        });

        Ok(())
    }

    /// 停止通话
    pub fn stop_call(&self) {
        let dialog_id = self.dialog_id.read().clone();
        if !dialog_id.is_empty() {
            if let Some(path) = self.dialog_id_path.lock().as_ref() {
                if let Err(error) = fs::write(path, &dialog_id) {
                    tracing::warn!("保存角色实时语音会话失败: {}", error);
                }
            }
        }
        self.stop_flag.store(true, Ordering::SeqCst);
        self.diagnostic_asr_at.store(0, Ordering::Relaxed);
        self.diagnostic_playback_at.store(0, Ordering::Relaxed);
        self.mic_stop_flag.store(true, Ordering::SeqCst);
        self.speaker_stop_flag.store(true, Ordering::SeqCst);
        *self.last_ai_audio_at.lock() = None;
        if let Some(tx) = self.ws_writer_tx.lock().take() {
            let _ = tx.try_send(vec![]); // 唤醒 writer（队列满时忽略，writer 本就在跑）
        }
        if let Some(handle) = self.mic_thread.lock().take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.speaker_thread.lock().take() {
            let _ = handle.join();
        }
        *self.session_id.write() = String::new();
        *self.call_start.lock() = None;
    }

    /// 获取当前/上次通话的 dialog_id（用于持久化以恢复上下文）
    pub fn last_dialog_id(&self) -> String {
        self.dialog_id.read().clone()
    }

    /// 从本地文件读取上次持久化的 dialog_id
    fn load_dialog_id_from_disk(&self) -> Option<String> {
        let path = self.dialog_id_path.lock().clone();
        let p = path?;
        fs::read_to_string(&p).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
    }

    /// 发送文本 query（替代音频输入）
    pub fn send_text_query(&self, text: &str) -> VivianResult<()> {
        let session_id = self.session_id.read().clone();
        if session_id.is_empty() {
            return Err(VivianError::Speech("无活动会话".to_string()));
        }
        let frame = build_client_event_frame(
            &session_id,
            ClientEvent::ChatTextQuery,
            serde_json::json!({ "content": text }),
        );
        if let Some(tx) = self.ws_writer_tx.lock().as_ref() {
            tx.try_send(frame).map_err(|e| {
                VivianError::Speech(match e {
                    mpsc::error::TrySendError::Full(_) => "WS 写入队列已满".to_string(),
                    mpsc::error::TrySendError::Closed(_) => "WS 写入通道已关闭".to_string(),
                })
            })?;
        }
        Ok(())
    }

    /// 启动麦克风采集（独立线程持有 cpal Stream）
    fn start_mic_capture(
        &self,
        session_id: String,
        writer_tx: mpsc::Sender<Vec<u8>>,
    ) -> VivianResult<()> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::{SampleFormat, SampleRate, StreamConfig};

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| VivianError::Speech("未找到麦克风设备".to_string()))?;
        let mut supported_configs = device
            .supported_input_configs()
            .map_err(|e| VivianError::Speech(format!("查询麦克风配置失败: {e}")))?;
        let supported = supported_configs
            .next()
            .ok_or_else(|| VivianError::Speech("麦克风无可用配置".to_string()))?;
        let sample_format = supported.sample_format();
        let desired_rate = SampleRate(16000);
        let actual_rate = if supported.min_sample_rate().0 <= desired_rate.0
            && supported.max_sample_rate().0 >= desired_rate.0
        {
            16000u32
        } else {
            supported.max_sample_rate().0
        };
        let stream_config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(actual_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let stop_flag = self.mic_stop_flag.clone();
        let stop_for_thread = stop_flag.clone();
        let stop_for_loop = stop_flag.clone();
        let sr_in = actual_rate as f32;
        let sr_out = 16000f32;

        let echo_enabled = self.echo_suppression_enabled.clone();
        let echo_enabled_i16 = echo_enabled.clone();
        let echo_enabled_u16 = echo_enabled.clone();
        let echo_enabled_f32 = echo_enabled.clone();
        let last_ai_at = self.last_ai_audio_at.clone();
        let last_ai_at_i16 = last_ai_at.clone();
        let last_ai_at_u16 = last_ai_at.clone();
        let last_ai_at_f32 = last_ai_at.clone();
        let echo_release = self.echo_release_ms.clone();
        let echo_release_i16 = echo_release.clone();
        let echo_release_u16 = echo_release.clone();
        let echo_release_f32 = echo_release.clone();

        let err_fn = |e: cpal::StreamError| {
            tracing::error!("麦克风采集错误: {e}");
        };

        let handle = std::thread::spawn(move || {
            // 临时缓冲，累积到 20ms（640 字节 = 320 samples）再发送
            let mut pcm_buf: Vec<i16> = Vec::with_capacity(320 * 4);

            let stream = match sample_format {
                SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &_| {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            return;
                        }
                        if mic_echo_suppressed(&echo_enabled_i16, &last_ai_at_i16, &echo_release_i16) {
                            pcm_buf.clear();
                            return;
                        }
                        let ratio = sr_out / sr_in;
                        for (i, &s) in data.iter().enumerate() {
                            let out_idx = (i as f32 * ratio) as usize;
                            while pcm_buf.len() <= out_idx {
                                pcm_buf.push(0);
                            }
                            pcm_buf[out_idx] = s;
                        }
                        while pcm_buf.len() >= 320 {
                            let chunk: Vec<i16> = pcm_buf.drain(..320).collect();
                            let mut bytes = Vec::with_capacity(640);
                            for &s in chunk.iter() {
                                bytes.extend_from_slice(&s.to_le_bytes());
                            }
                            let frame = build_audio_frame(&session_id, &bytes);
                            // try_send：队列满（网络跟不上）时丢帧，绝不阻塞采集线程
                            let _ = writer_tx.try_send(frame);
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::U16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _: &_| {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            return;
                        }
                        if mic_echo_suppressed(&echo_enabled_u16, &last_ai_at_u16, &echo_release_u16) {
                            pcm_buf.clear();
                            return;
                        }
                        let ratio = sr_out / sr_in;
                        for (i, &s) in data.iter().enumerate() {
                            let out_idx = (i as f32 * ratio) as usize;
                            let pcm = (s as i32 - 32768) as i16;
                            while pcm_buf.len() <= out_idx {
                                pcm_buf.push(0);
                            }
                            pcm_buf[out_idx] = pcm;
                        }
                        while pcm_buf.len() >= 320 {
                            let chunk: Vec<i16> = pcm_buf.drain(..320).collect();
                            let mut bytes = Vec::with_capacity(640);
                            for &s in chunk.iter() {
                                bytes.extend_from_slice(&s.to_le_bytes());
                            }
                            let frame = build_audio_frame(&session_id, &bytes);
                            // try_send：队列满（网络跟不上）时丢帧，绝不阻塞采集线程
                            let _ = writer_tx.try_send(frame);
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::F32 => device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &_| {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            return;
                        }
                        if mic_echo_suppressed(&echo_enabled_f32, &last_ai_at_f32, &echo_release_f32) {
                            pcm_buf.clear();
                            return;
                        }
                        let ratio = sr_out / sr_in;
                        for (i, &s) in data.iter().enumerate() {
                            let out_idx = (i as f32 * ratio) as usize;
                            let pcm = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                            while pcm_buf.len() <= out_idx {
                                pcm_buf.push(0);
                            }
                            pcm_buf[out_idx] = pcm;
                        }
                        while pcm_buf.len() >= 320 {
                            let chunk: Vec<i16> = pcm_buf.drain(..320).collect();
                            let mut bytes = Vec::with_capacity(640);
                            for &s in chunk.iter() {
                                bytes.extend_from_slice(&s.to_le_bytes());
                            }
                            let frame = build_audio_frame(&session_id, &bytes);
                            // try_send：队列满（网络跟不上）时丢帧，绝不阻塞采集线程
                            let _ = writer_tx.try_send(frame);
                        }
                    },
                    err_fn,
                    None,
                ),
                fmt => {
                    tracing::error!("不支持的采样格式: {fmt:?}");
                    return;
                }
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("构建输入流失败: {e}");
                    return;
                }
            };
            if let Err(e) = stream.play() {
                tracing::error!("启动麦克风采集失败: {e}");
                return;
            }
            while !stop_for_loop.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            drop(stream);
        });

        *self.mic_thread.lock() = Some(handle);
        Ok(())
    }

    /// 启动扬声器流式播放（独立线程，cpal 输出流，24kHz f32）
    fn start_speaker_playback(&self, app: AppHandle) -> VivianResult<()> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::{SampleFormat, SampleRate, StreamConfig};

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| VivianError::Speech("未找到扬声器设备".to_string()))?;
        let mut supported_configs = device
            .supported_output_configs()
            .map_err(|e| VivianError::Speech(format!("查询扬声器配置失败: {e}")))?;
        let supported = supported_configs
            .next()
            .ok_or_else(|| VivianError::Speech("扬声器无可用配置".to_string()))?;
        let sample_format = supported.sample_format();
        let desired_rate = SampleRate(24000);
        let actual_rate = if supported.min_sample_rate().0 <= desired_rate.0
            && supported.max_sample_rate().0 >= desired_rate.0
        {
            24000u32
        } else {
            supported.max_sample_rate().0
        };
        let stream_config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(actual_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let stop_flag = self.speaker_stop_flag.clone();
        let stop_for_thread = stop_flag.clone();
        let stop_for_loop = stop_flag.clone();
        let buffer = self.audio_out_buffer.clone();
        let diagnostic_asr = self.diagnostic_asr_at.clone();
        let diagnostic_playback = self.diagnostic_playback_at.clone();
        let diagnostic_asr_monitor = diagnostic_asr.clone();
        let diagnostic_playback_monitor = diagnostic_playback.clone();
        let sr_in = 24000f32;
        let sr_out = actual_rate as f32;

        let err_fn = |e: cpal::StreamError| {
            tracing::error!("扬声器播放错误: {e}");
        };

        let handle = std::thread::spawn(move || {
            let stream = match sample_format {
                SampleFormat::F32 => device.build_output_stream(
                    &stream_config,
                    move |out: &mut [f32], _: &_| {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            for s in out.iter_mut() {
                                *s = 0.0;
                            }
                            return;
                        }
                        let mut buf = buffer.write();
                        if !buf.is_empty() && diagnostic_asr.load(Ordering::Relaxed) != 0 {
                            let _ = diagnostic_playback.compare_exchange(0, crate::voice_diagnostics::now_ms(), Ordering::Relaxed, Ordering::Relaxed);
                        }
                        let ratio = sr_out / sr_in;
                        for (i, out_s) in out.iter_mut().enumerate() {
                            let in_idx = (i as f32 / ratio) as usize;
                            *out_s = if in_idx < buf.len() {
                                buf[in_idx]
                            } else {
                                0.0
                            };
                        }
                        // 清掉已消费的样本
                        let consumed = (out.len() as f32 / ratio) as usize;
                        if consumed <= buf.len() {
                            buf.drain(..consumed);
                        } else {
                            buf.clear();
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::I16 => device.build_output_stream(
                    &stream_config,
                    move |out: &mut [i16], _: &_| {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            for s in out.iter_mut() {
                                *s = 0;
                            }
                            return;
                        }
                        let mut buf = buffer.write();
                        if !buf.is_empty() && diagnostic_asr.load(Ordering::Relaxed) != 0 {
                            let _ = diagnostic_playback.compare_exchange(0, crate::voice_diagnostics::now_ms(), Ordering::Relaxed, Ordering::Relaxed);
                        }
                        let ratio = sr_out / sr_in;
                        for (i, out_s) in out.iter_mut().enumerate() {
                            let in_idx = (i as f32 / ratio) as usize;
                            *out_s = if in_idx < buf.len() {
                                (buf[in_idx] * 32767.0) as i16
                            } else {
                                0
                            };
                        }
                        let consumed = (out.len() as f32 / ratio) as usize;
                        if consumed <= buf.len() {
                            buf.drain(..consumed);
                        } else {
                            buf.clear();
                        }
                    },
                    err_fn,
                    None,
                ),
                _ => {
                    tracing::error!("扬声器采样格式不支持: {sample_format:?}");
                    return;
                }
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("构建输出流失败: {e}");
                    let _ = app.emit("realtime:event", RealtimeEvent::Error {
                        message: format!("扬声器初始化失败: {e}"),
                    });
                    return;
                }
            };
            if let Err(e) = stream.play() {
                tracing::error!("启动扬声器播放失败: {e}");
                return;
            }
            while !stop_for_loop.load(Ordering::SeqCst) {
                let end = diagnostic_playback_monitor.swap(0, Ordering::Relaxed);
                if end != 0 {
                    let start = diagnostic_asr_monitor.swap(0, Ordering::Relaxed);
                    crate::voice_diagnostics::record("realtime", "last_asr_result_to_audio_callback", start, end);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            drop(stream);
        });

        *self.speaker_thread.lock() = Some(handle);
        Ok(())
    }
}

/// 构建每轮 ChatRagText 负载（豆包 SC2.0 的动态 RAG 注入，4K 字符上限）。
///
/// 注入内容（已筛选，仅与语音对话相关的部分）：
/// - 用户画像（预加载字符串，通话开始时一次性 format_for_prompt 缓存）
/// - 关系状态（预加载字符串，通话开始时一次性 relationship_section 缓存）
/// - 记忆检索结果（每轮按 query 语义检索，唯一每轮异步执行的操作）
///
/// 不注入：工具列表、skill catalog（语音通话不调用工具）
/// 不注入：完整人设（已在 StartSession 的 character_manifest 中固定）
///
/// 延迟优化策略：
/// - 用户画像/关系状态在 start_call 中预加载到 cached_user_facts/cached_relationship，
///   整个通话期间复用，避免每轮重复计算（这两项变化极慢，不需要每轮重查）
/// - 剩余唯一异步操作是 memory search，通过 read_task 调用处的 100ms 超时降级保护
async fn build_chat_rag(
    user_query: &str,
    memory: Option<&crate::memory::MemoryManager>,
    user_facts: Option<&str>,
    psychology: Option<&str>,
) -> Option<String> {
    let mut rag_items: Vec<(String, String)> = Vec::new();

    // 1. 用户画像（预加载缓存，直接使用，不重复计算）
    if let Some(facts) = user_facts {
        if !facts.trim().is_empty() {
            rag_items.push(("用户画像".to_string(), facts.to_string()));
        }
    }

    // 2. 关系状态（预加载缓存，直接使用，不重复计算）
    if let Some(rel) = psychology {
        if !rel.trim().is_empty() {
            rag_items.push(("关系状态".to_string(), rel.to_string()));
        }
    }

    // 3. 记忆检索（每轮按当前 query 语义检索，唯一每轮异步操作）
    if let Some(mem) = memory {
        if !user_query.trim().is_empty() {
            if let Ok(items) = mem
                .search_memories(user_query, crate::memory::RetrievalStrategy::Auto, 5)
                .await
            {
                if !items.is_empty() {
                    let mem_text: Vec<String> = items
                        .iter()
                        .map(|m| {
                            let time = m.timestamp;
                            let content = &m.content;
                            format!("[{time}] {content}")
                        })
                        .collect();
                    rag_items.push(("相关记忆".to_string(), mem_text.join("\n")));
                }
            }
        }
    }

    if rag_items.is_empty() {
        return None;
    }

    let rag_array: Vec<serde_json::Value> = rag_items
        .iter()
        .map(|(title, content)| {
            serde_json::json!({"title": title, "content": content})
        })
        .collect();
    let external_rag = serde_json::to_string(&rag_array).ok()?;
    let payload = serde_json::json!({ "external_rag": external_rag });
    Some(payload.to_string())
}

/// 处理服务端文本事件
fn mic_echo_suppressed(
    enabled: &AtomicBool,
    last_ai_audio: &Mutex<Option<std::time::Instant>>,
    release_ms: &Mutex<u64>,
) -> bool {
    if !enabled.load(Ordering::SeqCst) {
        return false;
    }
    let release = *release_ms.lock();
    match *last_ai_audio.lock() {
        Some(t) => t.elapsed().as_millis() < release as u128,
        None => false,
    }
}

fn handle_server_event(
    app: &AppHandle,
    event: ServerEvent,
    payload: serde_json::Value,
    ai_text_buffer: &mut String,
    ai_audio_active: &mut bool,
) {
    match event {
        ServerEvent::AsrResult => {
            // 提取识别文本
            let text = payload
                .get("results")
                .and_then(|r| r.get(0))
                .and_then(|r| r.get("alternatives"))
                .and_then(|a| a.get(0))
                .and_then(|a| a.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            if !text.is_empty() {
                let _ = app.emit(
                    "realtime:event",
                    RealtimeEvent::AsrPartial {
                        text: text.to_string(),
                    },
                );
            }
        }
        ServerEvent::UsageResponse => {
            let usage = payload.get("usage");
            if let Some(u) = usage {
                let _ = app.emit(
                    "realtime:event",
                    RealtimeEvent::Usage {
                        input_text_tokens: u.get("input_text_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        input_audio_tokens: u.get("input_audio_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        output_text_tokens: u.get("output_text_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        output_audio_tokens: u.get("output_audio_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                    },
                );
            }
        }
        ServerEvent::SessionFinished => {
            // 会话结束，推送 AI 文本
            if !ai_text_buffer.is_empty() {
                let text = std::mem::take(ai_text_buffer);
                let _ = app.emit("realtime:event", RealtimeEvent::AiTextDone { text });
            }
            if *ai_audio_active {
                *ai_audio_active = false;
                let _ = app.emit("realtime:event", RealtimeEvent::AiAudioFinished);
            }
        }
        ServerEvent::SessionFailed => {
            let err = payload.get("error").and_then(|v| v.as_str()).unwrap_or("未知");
            let _ = app.emit("realtime:event", RealtimeEvent::Error {
                message: format!("会话失败: {err}"),
            });
        }
        _ => {
            // 其他事件暂不处理
            tracing::debug!("未处理的服务端事件: {:?} payload={}", event, payload);
        }
    }
}

/// 构建 StartSession 事件 payload
fn build_start_session_payload(config: &RealtimeVoiceConfig, persona: &RealtimePersona) -> serde_json::Value {
    let mut dialog_extra = serde_json::json!({
        "input_mod": config.input_mod.clone(),
        "model": config.model.clone(),
    });
    if config.strict_audit {
        dialog_extra["strict_audit"] = serde_json::json!(true);
    }
    if !config.audit_response.is_empty() {
        dialog_extra["audit_response"] = serde_json::json!(config.audit_response);
    }

    let dialog_id_value = if config.dialog_id.is_empty() {
        String::new()
    } else {
        config.dialog_id.clone()
    };

    let mut dialog = serde_json::json!({
        "dialog_id": dialog_id_value,
        "extra": dialog_extra,
    });
    // SC 版本用 character_manifest，O 版本用 bot_name/system_role/speaking_style
    // 两种协议共享当前角色与用户覆盖；语音规则保持独立于文字输出格式。
    if config.model == "SC" {
        dialog["character_manifest"] = serde_json::json!(format!("{}\n\n{}", persona.system_role, persona.speaking_style));
    } else {
        dialog["bot_name"] = serde_json::json!(persona.bot_name);
        dialog["system_role"] = serde_json::json!(persona.system_role);
        dialog["speaking_style"] = serde_json::json!(persona.speaking_style);
    }
    if let Some(loc) = &config.location {
        dialog["location"] = serde_json::to_value(loc).unwrap_or(serde_json::Value::Null);
    }

    let asr = serde_json::json!({
        "extra": {
            "end_smooth_window_ms": config.end_smooth_window_ms,
            "enable_custom_vad": config.enable_custom_vad,
            "enable_asr_twopass": config.enable_asr_twopass,
        }
    });

    let tts = serde_json::json!({
        "speaker": config.speaker.clone(),
        "audio_config": {
            "channel": 1,
            "format": "pcm_s16le",
            "sample_rate": 24000
        }
    });

    serde_json::json!({
        "asr": asr,
        "dialog": dialog,
        "tts": tts,
    })
}

#[cfg(test)]
mod voice_persona_tests {
    use super::*;
    #[test]
    fn scoped_history_wins_and_legacy_history_only_migrates_to_vivian() {
        assert_eq!(resume_dialog_id("nana", Some(" nana_session \n".into()), "vivian_session", || panic!("no legacy read")), "nana_session");
        assert_eq!(resume_dialog_id("nana", None, "vivian_session", || panic!("no legacy read")), "");
        assert_eq!(resume_dialog_id("vivian", Some("new_session".into()), "old_session", || panic!("no legacy read")), "new_session");
        assert_eq!(resume_dialog_id("vivian", None, " old_session ", || panic!("no legacy read")), "old_session");
        assert_eq!(resume_dialog_id("vivian", None, "", || Some(" file_session\n".into())), "file_session");
    }
    #[test]
    fn sc_and_o_sessions_use_selected_character_and_custom_speech() {
        let mut persona_config = crate::persona::schemas::default_persona_for("nana");
        persona_config.speech_definition = "custom_voice_marker".repeat(150);
        let persona = RealtimePersona::from_config("nana", &persona_config);
        assert!(persona.speaking_style.contains(&persona_config.speech_definition));
        assert!(persona.speaking_style.contains("explicit question or request for help"));
        let mut config = RealtimeVoiceConfig::default();
        config.model = "SC".into();
        let sc = build_start_session_payload(&config, &persona);
        assert!(sc["dialog"]["character_manifest"].as_str().unwrap().contains("# Nana"));
        assert!(sc["dialog"].get("system_role").is_none());
        config.model = "O".into();
        let o = build_start_session_payload(&config, &persona);
        assert_eq!(o["dialog"]["bot_name"], "Nana");
        assert!(o["dialog"]["speaking_style"].as_str().unwrap().contains("custom_voice_marker"));
        assert!(o["dialog"].get("character_manifest").is_none());
    }
    #[test]
    fn voice_history_files_are_distinct_and_path_safe() {
        let vivian = RealtimePersona::from_config("vivian", &crate::persona::schemas::default_persona_for("vivian"));
        let nana = RealtimePersona::from_config("nana", &crate::persona::schemas::default_persona_for("nana"));
        let custom = RealtimePersona::from_config("../custom", &crate::persona::schemas::default_persona_for("vivian"));
        assert_ne!(vivian.storage_filename(), nana.storage_filename());
        assert!(!custom.storage_filename().contains(['/', '\\']));
        for persona in [vivian, nana] {
            let mut config = RealtimeVoiceConfig::default();
            config.model = "SC".into();
            let payload = build_start_session_payload(&config, &persona);
            let manifest = payload["dialog"]["character_manifest"].as_str().unwrap();
            assert!(manifest.len() < 2500, "default voice persona exceeds the compact budget");
            assert!(manifest.contains(&format!("# {}", persona.bot_name)));
            eprintln!("voice {}: system={}B, style={}B", persona.character_id, persona.system_role.len(), persona.speaking_style.len());
            eprintln!("voice {}: SC manifest={}B", persona.character_id, manifest.len());
        }
    }
}
