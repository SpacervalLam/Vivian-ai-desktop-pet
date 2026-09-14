//! Whisper Realtime WebSocket 后端（OpenAI Realtime API 兼容，transcription-only 模式）
//!
//! 通过 WebSocket 连接 Speaches / OpenAI 的 `/v1/realtime?intent=transcription`，
//! 边录音边发送音频 chunk，服务端 VAD 自动检测语音起止，实时返回转录 delta。
//!
//! 协议：
//! - 客户端 → 服务端：`input_audio_buffer.append`（base64 PCM16 24kHz mono）
//! - 服务端 → 客户端：
//!   - `input_audio_buffer.speech_started` / `speech_stopped`（VAD 事件）
//!   - `conversation.item.input_audio_transcription.delta`（增量转录）
//!   - `conversation.item.input_audio_transcription.completed`（最终转录）
//!
//! 适合 always-on 实时监听场景。push-to-talk 场景建议用 SSE 流式。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use parking_lot::RwLock;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::error::{VivianError, VivianResult};

use super::asr::{AsrBackendType, AsrConfig, AsrEngine, AsrEvent};
use super::whisper_backend::{WhisperConfig, WhisperStreamingMode};

/// 目标采样率：OpenAI Realtime API 要求 24kHz
const TARGET_SAMPLE_RATE: u32 = 24000;
/// 每个 audio chunk 的样本数（100ms @ 24kHz = 2400 samples）
const CHUNK_SAMPLES: usize = 2400;

/// WS 任务指令
enum WsCmd {
    /// 音频 chunk（PCM16 24kHz mono）
    Audio(Vec<i16>),
    /// 提交音频缓冲（停止录音时）
    Commit,
    /// 清空音频缓冲
    Clear,
}

/// Whisper Realtime WebSocket 后端
pub struct WhisperRealtimeBackend {
    config: AsrConfig,
    whisper_cfg: WhisperConfig,
    available: bool,
    is_running: bool,
    event_tx: Option<broadcast::Sender<AsrEvent>>,
    /// cpal 采集线程（线程内独占 Stream，规避非 Send 限制）
    capture_thread: Option<std::thread::JoinHandle<()>>,
    stop_flag: Option<Arc<AtomicBool>>,
    /// WS 主任务（连接 + 收发循环）
    ws_task: Option<JoinHandle<()>>,
    /// 向 WS 任务发送指令的通道
    cmd_tx: Option<mpsc::UnboundedSender<WsCmd>>,
}

impl WhisperRealtimeBackend {
    pub fn from_config(config: AsrConfig, whisper_cfg: WhisperConfig) -> Self {
        Self {
            config,
            whisper_cfg,
            available: true,
            is_running: false,
            event_tx: None,
            capture_thread: None,
            stop_flag: None,
            ws_task: None,
            cmd_tx: None,
        }
    }

    /// 构造 Realtime WS URL
    fn build_ws_url(&self) -> VivianResult<String> {
        if self.whisper_cfg.server_url.is_empty() {
            return Err(VivianError::Speech("Whisper 服务地址未配置".to_string()));
        }
        let base = self.whisper_cfg.server_url.trim_end_matches('/');
        // http(s):// → ws(s)://
        let ws_base = if base.starts_with("https://") {
            format!("wss://{}", &base[8..])
        } else if base.starts_with("http://") {
            format!("ws://{}", &base[7..])
        } else {
            base.to_string()
        };
        let model = self
            .whisper_cfg
            .realtime_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.whisper_cfg.service_model.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or("whisper-1");
        let lang = self
            .whisper_cfg
            .realtime_language
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                self.config
                    .language
                    .split('-')
                    .next()
                    .unwrap_or("en")
                    .to_string()
            });
        let mut url = format!(
            "{}/v1/realtime?intent=transcription&model={}&language={}",
            ws_base,
            url_encode(model),
            url_encode(&lang)
        );
        if !self.whisper_cfg.api_key.is_empty() {
            url.push_str(&format!("&api_key={}", url_encode(&self.whisper_cfg.api_key)));
        }
        Ok(url)
    }

    /// 启动 cpal 麦克风采集（独立线程，重采样到 24kHz，每 CHUNK_SAMPLES 发一个 chunk）
    fn start_capture(&mut self, cmd_tx: mpsc::UnboundedSender<WsCmd>) -> VivianResult<()> {
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
        let desired_rate = SampleRate(TARGET_SAMPLE_RATE);
        let actual_rate = if supported.min_sample_rate().0 <= desired_rate.0
            && supported.max_sample_rate().0 >= desired_rate.0
        {
            TARGET_SAMPLE_RATE
        } else {
            supported.max_sample_rate().0
        };
        let stream_config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(actual_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop_flag.clone();
        let stop_for_loop = stop_flag.clone();
        let sr_in = actual_rate as f32;
        let sr_out = TARGET_SAMPLE_RATE as f32;

        let err_fn = |e: cpal::StreamError| {
            tracing::error!("[Whisper-RT] 麦克风采集错误: {e}");
        };

        // 重采样后的样本缓冲（24kHz），攒够 CHUNK_SAMPLES 发一次
        let resampled_buf: Arc<RwLock<Vec<i16>>> =
            Arc::new(RwLock::new(Vec::with_capacity(CHUNK_SAMPLES * 2)));

        let handle = std::thread::spawn(move || {
            let stream = match sample_format {
                SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &_| {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            return;
                        }
                        process_pcm_chunk(data, sr_in, sr_out, &resampled_buf, &cmd_tx, |s| s);
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
                        process_pcm_chunk(data, sr_in, sr_out, &resampled_buf, &cmd_tx, |s| {
                            (s as i32 - 32768) as i16
                        });
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
                        process_pcm_chunk(data, sr_in, sr_out, &resampled_buf, &cmd_tx, |s| {
                            (s.clamp(-1.0, 1.0) * 32767.0) as i16
                        });
                    },
                    err_fn,
                    None,
                ),
                _ => {
                    tracing::error!("[Whisper-RT] 不支持的采样格式: {:?}", sample_format);
                    return;
                }
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("[Whisper-RT] 构建输入流失败: {e}");
                    return;
                }
            };
            if let Err(e) = stream.play() {
                tracing::error!("[Whisper-RT] 启动麦克风采集失败: {e}");
                return;
            }
            while !stop_for_loop.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            drop(stream);
        });

        self.stop_flag = Some(stop_flag);
        self.capture_thread = Some(handle);
        Ok(())
    }

    /// 启动 WS 主任务
    fn start_ws_task(
        &mut self,
        ws_url: String,
        cmd_rx: mpsc::UnboundedReceiver<WsCmd>,
        event_tx: broadcast::Sender<AsrEvent>,
    ) -> VivianResult<()> {
        let api_key = self.whisper_cfg.api_key.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = run_ws_loop(ws_url, api_key, cmd_rx, event_tx).await {
                tracing::error!("[Whisper-RT] WS 主任务异常退出: {e}");
            }
        });
        self.ws_task = Some(handle);
        Ok(())
    }
}

/// 处理 cpal 回调中的 PCM chunk：重采样到 24kHz，攒够 CHUNK_SAMPLES 发送
fn process_pcm_chunk<T: Copy>(
    data: &[T],
    sr_in: f32,
    sr_out: f32,
    resampled_buf: &Arc<RwLock<Vec<i16>>>,
    cmd_tx: &mpsc::UnboundedSender<WsCmd>,
    convert: impl Fn(T) -> i16,
) {
    let mut buf = resampled_buf.write();
    if (sr_in - sr_out).abs() < 1.0 {
        // 同采样率，直接转换
        for &s in data.iter() {
            buf.push(convert(s));
        }
    } else {
        // 线性插值重采样：sr_in → sr_out
        let ratio = sr_in / sr_out;
        let out_len = (data.len() as f32 / ratio) as usize;
        for out_idx in 0..out_len {
            let in_idx = out_idx as f32 * ratio;
            let in_i = in_idx.floor() as usize;
            let frac = in_idx - in_i as f32;
            if in_i + 1 < data.len() {
                let s0 = convert(data[in_i]) as f32;
                let s1 = convert(data[in_i + 1]) as f32;
                let s = s0 * (1.0 - frac) + s1 * frac;
                buf.push(s as i16);
            } else if in_i < data.len() {
                buf.push(convert(data[in_i]));
            }
        }
    }
    // 攒够 CHUNK_SAMPLES 就发一次
    while buf.len() >= CHUNK_SAMPLES {
        let chunk: Vec<i16> = buf.drain(..CHUNK_SAMPLES).collect();
        let _ = cmd_tx.send(WsCmd::Audio(chunk));
    }
}

/// WS 主循环：连接 → 收发 → 关闭
async fn run_ws_loop(
    ws_url: String,
    api_key: String,
    mut cmd_rx: mpsc::UnboundedReceiver<WsCmd>,
    event_tx: broadcast::Sender<AsrEvent>,
) -> VivianResult<()> {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;

    tracing::info!("[Whisper-RT] 连接 Realtime WS: {}", ws_url);
    let mut request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&ws_url)
        .method("GET")
        .header("Host", extract_host(&ws_url))
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header(
            "Sec-WebSocket-Version",
            "13",
        )
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        );
    if !api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", api_key));
    }
    let request = request
        .body(())
        .map_err(|e| VivianError::Speech(format!("构建 WS 请求失败: {e}")))?;

    let (ws_stream, _resp) = connect_async(request)
        .await
        .map_err(|e| VivianError::Network(format!("连接 Realtime WS 失败: {e}")))?;
    tracing::info!("[Whisper-RT] WS 连接已建立");

    let (mut ws_write, mut ws_read) = ws_stream.split();

    // 发送初始 session.update（可选，Speaches 默认即 transcription 模式）
    // 这里不发送，依赖 URL 参数 intent=transcription

    let mut accumulated_text = String::new();
    let mut speech_active = false;

    loop {
        tokio::select! {
            msg = ws_read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(e) = handle_ws_message(
                            &text,
                            &event_tx,
                            &mut accumulated_text,
                            &mut speech_active,
                        ) {
                            tracing::warn!("[Whisper-RT] 处理 WS 消息失败: {e}");
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        // transcription 模式一般不发二进制
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::info!("[Whisper-RT] WS 连接关闭");
                        let _ = event_tx.send(AsrEvent::Stopped);
                        return Ok(());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        let msg = format!("WS 读取错误: {e}");
                        tracing::error!("[Whisper-RT] {msg}");
                        let _ = event_tx.send(AsrEvent::error(msg));
                        return Ok(());
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(WsCmd::Audio(samples)) => {
                        // PCM16 → base64
                        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let payload = serde_json::json!({
                            "type": "input_audio_buffer.append",
                            "audio": b64,
                        });
                        let text = payload.to_string();
                        if let Err(e) = ws_write.send(Message::Text(text)).await {
                            let msg = format!("发送 audio chunk 失败: {e}");
                            tracing::error!("[Whisper-RT] {msg}");
                            let _ = event_tx.send(AsrEvent::error(msg));
                            return Ok(());
                        }
                    }
                    Some(WsCmd::Commit) => {
                        let payload = serde_json::json!({"type": "input_audio_buffer.commit"});
                        let _ = ws_write.send(Message::Text(payload.to_string())).await;
                        // commit 后等服务端 completed 事件，不主动关闭
                    }
                    Some(WsCmd::Clear) => {
                        let payload = serde_json::json!({"type": "input_audio_buffer.clear"});
                        let _ = ws_write.send(Message::Text(payload.to_string())).await;
                        accumulated_text.clear();
                    }
                    None => {
                        // cmd_tx 关闭，退出
                        let _ = ws_write.send(Message::Close(None)).await;
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// 处理服务端 WS 消息
fn handle_ws_message(
    text: &str,
    event_tx: &broadcast::Sender<AsrEvent>,
    accumulated_text: &mut String,
    speech_active: &mut bool,
) -> VivianResult<()> {
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| VivianError::Serialization(format!("解析 WS 消息失败: {e}")))?;
    let msg_type = v.get("type").and_then(|s| s.as_str()).unwrap_or("");
    match msg_type {
        "input_audio_buffer.speech_started" => {
            *speech_active = true;
            let _ = event_tx.send(AsrEvent::Started);
            tracing::debug!("[Whisper-RT] speech_started");
        }
        "input_audio_buffer.speech_stopped" => {
            *speech_active = false;
            tracing::debug!("[Whisper-RT] speech_stopped");
        }
        "input_audio_buffer.committed" => {
            tracing::debug!("[Whisper-RT] audio buffer committed");
        }
        "conversation.item.input_audio_transcription.delta" => {
            if let Some(delta) = v.get("delta").and_then(|s| s.as_str()) {
                accumulated_text.push_str(delta);
                let _ = event_tx.send(AsrEvent::partial(accumulated_text.clone()));
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            let transcript = v
                .get("transcript")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if !transcript.is_empty() {
                let _ = event_tx.send(AsrEvent::final_result(transcript.clone(), 0.9));
            }
            accumulated_text.clear();
            tracing::debug!("[Whisper-RT] transcription completed");
        }
        "error" => {
            let err_msg = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|s| s.as_str())
                .unwrap_or("未知错误");
            let _ = event_tx.send(AsrEvent::error(format!("Realtime 服务错误: {err_msg}")));
            tracing::warn!("[Whisper-RT] 服务端 error: {err_msg}");
        }
        _ => {
            tracing::trace!("[Whisper-RT] 未处理的 WS 消息类型: {msg_type}");
        }
    }
    Ok(())
}

/// 简单 URL 编码（仅编码 model/language/api_key 中的特殊字符）
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

/// 从 ws(s)://host:port/path 提取 host:port
fn extract_host(url: &str) -> String {
    let stripped = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))
        .unwrap_or(url);
    if let Some(idx) = stripped.find('/') {
        stripped[..idx].to_string()
    } else {
        stripped.to_string()
    }
}

#[async_trait]
impl AsrEngine for WhisperRealtimeBackend {
    async fn initialize(&mut self) -> VivianResult<bool> {
        if self.whisper_cfg.server_url.is_empty() {
            tracing::warn!("[Whisper-RT] 服务地址未配置，后端标记为不可用");
            self.available = false;
            return Ok(false);
        }
        tracing::info!(
            "[Whisper-RT] 后端初始化完成: url={} mode={:?}",
            self.whisper_cfg.server_url,
            WhisperStreamingMode::RealtimeWs
        );
        Ok(true)
    }

    async fn start_recording(&mut self) -> VivianResult<()> {
        if !self.available {
            return Err(VivianError::NotImplemented(
                "Whisper Realtime 后端不可用".to_string(),
            ));
        }
        if self.is_running {
            return Ok(());
        }

        let ws_url = self.build_ws_url()?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<WsCmd>();
        let event_tx = self
            .event_tx
            .clone()
            .ok_or_else(|| VivianError::Speech("事件通道未注入".to_string()))?;

        // 启动 WS 主任务
        self.start_ws_task(ws_url, cmd_rx, event_tx)?;

        // 启动 cpal 采集
        self.start_capture(cmd_tx.clone())?;

        self.cmd_tx = Some(cmd_tx);
        self.is_running = true;
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AsrEvent::Started);
        }
        tracing::info!("[Whisper-RT] 录音已启动");
        Ok(())
    }

    async fn stop_recording(&mut self) -> VivianResult<()> {
        // 停止 cpal 采集
        if let Some(flag) = &self.stop_flag {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
        self.stop_flag = None;

        // 发送 commit 指令，让 WS 任务发 input_audio_buffer.commit
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(WsCmd::Commit);
            // tx drop 后 WS 任务的 cmd_rx 会返回 None，触发关闭
        }

        // 等待 WS 任务结束（带超时）
        if let Some(handle) = self.ws_task.take() {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                handle,
            )
            .await;
        }

        self.is_running = false;
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AsrEvent::Stopped);
        }
        tracing::info!("[Whisper-RT] 录音已停止");
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> VivianResult<String> {
        // Realtime 模式不支持一次性 transcribe，降级为不支持
        let _ = audio;
        Err(VivianError::NotImplemented(
            "Realtime WebSocket 后端不支持 transcribe，请用 start_recording/stop_recording".into(),
        ))
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn supports_silence_detection(&self) -> bool {
        true
    }

    fn supports_partial_results(&self) -> bool {
        true
    }

    fn dispose(&mut self) {
        if let Some(flag) = &self.stop_flag {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
        self.stop_flag = None;
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(WsCmd::Clear);
        }
        if let Some(handle) = self.ws_task.take() {
            handle.abort();
        }
        self.is_running = false;
        self.available = false;
    }

    fn backend_type(&self) -> AsrBackendType {
        AsrBackendType::Whisper
    }

    fn set_event_sender(&mut self, sender: broadcast::Sender<AsrEvent>) {
        self.event_tx = Some(sender);
    }
}

impl std::fmt::Debug for WhisperRealtimeBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperRealtimeBackend")
            .field("server_url", &self.whisper_cfg.server_url)
            .field("realtime_model", &self.whisper_cfg.realtime_model)
            .field("available", &self.available)
            .field("is_running", &self.is_running)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ws_url_http() {
        let mut cfg = WhisperConfig::default();
        cfg.server_url = "http://localhost:8000".to_string();
        cfg.service_model = Some("whisper-1".into());
        let backend = WhisperRealtimeBackend::from_config(AsrConfig::default(), cfg);
        let url = backend.build_ws_url().unwrap();
        assert!(url.starts_with("ws://localhost:8000/v1/realtime"));
        assert!(url.contains("intent=transcription"));
        assert!(url.contains("model=whisper-1"));
    }

    #[test]
    fn test_build_ws_url_https() {
        let mut cfg = WhisperConfig::default();
        cfg.server_url = "https://api.example.com".to_string();
        cfg.realtime_model = Some("deepdml/faster-whisper-large-v3-turbo-ct2".into());
        let backend = WhisperRealtimeBackend::from_config(AsrConfig::default(), cfg);
        let url = backend.build_ws_url().unwrap();
        assert!(url.starts_with("wss://api.example.com/v1/realtime"));
        assert!(url.contains("model=deepdml%2Ffaster-whisper-large-v3-turbo-ct2"));
    }

    #[test]
    fn test_url_encode() {
        assert_eq!(url_encode("whisper-1"), "whisper-1");
        assert_eq!(url_encode("a/b"), "a%2Fb");
        assert_eq!(url_encode("a b"), "a%20b");
    }

    #[test]
    fn test_extract_host() {
        assert_eq!(extract_host("ws://localhost:8000/path"), "localhost:8000");
        assert_eq!(extract_host("wss://api.example.com/v1/realtime"), "api.example.com");
    }
}
