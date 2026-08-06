//! Whisper 本地/远程语音识别后端（HTTP 客户端）
//!
//! - 不内置 Whisper 推理，而是通过 HTTP 调用外部 Whisper 服务
//! - 兼容三种服务端：
//!   1. whisper.cpp 自带 server（examples/server，POST /inference）
//!   2. faster-whisper-server（OpenAI 兼容，POST /v1/audio/transcriptions）
//!   3. OpenAI Whisper API（POST /v1/audio/transcriptions）
//! - 通过 `cpal` 采集麦克风音频，重采样到 16kHz 单声道 16-bit PCM
//! - 停止录音时把 WAV 数据 POST 到服务端，返回完整识别结果
//! - 不支持流式 partial（HTTP 一次性返回）
//!
//! ## 配置要求
//! - 服务 URL（`whisper.server_url`，如 `http://localhost:8080`）
//! - API 格式（`whisper.api_format`：whisper_cpp / openai）
//! - 用户需自备一个 Whisper 服务（本地或远程），与 vivian-rs 解耦

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;
use parking_lot::RwLock;
use tokio::sync::broadcast;

use crate::error::{VivianError, VivianResult};

use super::asr::{AsrBackendType, AsrConfig, AsrEngine, AsrEvent};

/// Whisper HTTP 服务的 API 格式
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WhisperApiFormat {
    /// whisper.cpp examples/server（POST /inference，表单字段 response_format=text）
    WhisperCpp,
    /// OpenAI 兼容（faster-whisper-server、OpenAI 官方 API）
    /// POST /v1/audio/transcriptions，multipart 表单
    #[default]
    Openai,
}

/// Whisper 流式模式
///
/// - `none`：传统一次性 POST，录完返回完整结果（默认，兼容性最好）
/// - `sse`：POST /v1/audio/transcriptions?stream=true，SSE 流式返回转录片段
///         适合 push-to-talk 场景，前端可边出字边显示
/// - `realtime_ws`：WebSocket 连接 /v1/realtime?intent=transcription，
///                 边录边发音频 chunk，服务端 VAD + 实时转录 delta
///                 适合 always-on 场景，需 Speaches 服务端
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WhisperStreamingMode {
    /// 一次性 POST，无流式
    #[default]
    None,
    /// SSE 流式转录（push-to-talk）
    Sse,
    /// Realtime WebSocket（always-on）
    RealtimeWs,
}

impl WhisperStreamingMode {
    pub fn from_str_opt(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "sse" => Self::Sse,
            "realtime_ws" | "ws" | "realtime" => Self::RealtimeWs,
            _ => Self::None,
        }
    }
}

/// Whisper 后端专属配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperConfig {
    /// Whisper HTTP 服务地址（如 `http://localhost:8080`）
    pub server_url: String,
    /// API 格式
    pub api_format: WhisperApiFormat,
    /// OpenAI 兼容格式的可选 API Key（whisper.cpp server 可留空）
    pub api_key: String,
    /// 单次请求最大音频时长（秒），避免长录音阻塞
    pub max_audio_seconds: u32,

    // ── 一键启动 faster-whisper-server 子进程所需配置 ──
    /// faster-whisper-server 安装目录（git clone 的仓库根目录，可选）
    /// 若填写则作为子进程 cwd；留空则使用 pip 全局安装版本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_install_path: Option<String>,
    /// Python 可执行文件路径（如 `D:/Python/python.exe`，可选）
    /// 用于推导 faster-whisper-server 控制台脚本位置；留空则从 PATH 查找
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_python_path: Option<String>,
    /// Whisper 模型名（如 `small`、`medium`、`large-v3`，默认 `small`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_model: Option<String>,
    /// 推理设备：`auto` / `cpu` / `cuda`（默认 `auto`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_device: Option<String>,
    /// 计算精度：`auto` / `int8` / `int8_float16` / `float16` / `float32`（默认 `auto`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_compute_type: Option<String>,
    /// 服务监听端口（默认 8000）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_port: Option<u16>,
    /// 应用启动时是否自动拉起本地 Whisper 服务（仅在配置了 Python 路径或 PATH 可用时生效）
    #[serde(default)]
    pub service_auto_start: bool,

    // ── 流式转录配置 ──
    /// 流式模式：none / sse / realtime_ws（默认 none）
    #[serde(default)]
    pub streaming_mode: WhisperStreamingMode,
    /// Realtime WebSocket 使用的转录模型（如 `deepdml/faster-whisper-large-v3-turbo-ct2`）
    /// 仅 streaming_mode=realtime_ws 时使用；留空则复用 service_model
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_model: Option<String>,
    /// Realtime API 显式语言（ISO-639-1，如 `zh` / `en` / `ja`，留空则用 AsrConfig.language 推导）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_language: Option<String>,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            api_format: WhisperApiFormat::Openai,
            api_key: String::new(),
            max_audio_seconds: 30,
            service_install_path: None,
            service_python_path: None,
            service_model: None,
            service_device: None,
            service_compute_type: None,
            service_port: None,
            service_auto_start: false,
            streaming_mode: WhisperStreamingMode::None,
            realtime_model: None,
            realtime_language: None,
        }
    }
}

/// Whisper 后端实例
pub struct WhisperBackend {
    config: AsrConfig,
    whisper_cfg: WhisperConfig,
    available: bool,
    is_running: bool,
    event_tx: Option<broadcast::Sender<AsrEvent>>,
    // 采集线程句柄（线程内独占 cpal Stream，规避非 Send 限制）
    capture_thread: Option<std::thread::JoinHandle<()>>,
    stop_flag: Option<Arc<AtomicBool>>,
    // 16-bit PCM 样本缓冲（16kHz 单声道）
    buffer: Arc<RwLock<VecDeque<i16>>>,
}

impl WhisperBackend {
    pub fn from_config(config: AsrConfig, whisper_cfg: WhisperConfig) -> Self {
        Self {
            config,
            whisper_cfg,
            available: true,
            is_running: false,
            event_tx: None,
            capture_thread: None,
            stop_flag: None,
            buffer: Arc::new(RwLock::new(VecDeque::with_capacity(16000 * 30))),
        }
    }

    /// 启动 cpal 麦克风采集（在独立线程中持有 Stream，规避非 Send 限制）
    fn start_capture(&mut self) -> VivianResult<()> {
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
        let stop_flag = Arc::new(AtomicBool::new(false));
        let buffer = self.buffer.clone();
        let stop_for_thread = stop_flag.clone();
        let stop_for_loop = stop_flag.clone();
        let sr_in = actual_rate as f32;
        let sr_out = 16000f32;
        let max_samples = (self.whisper_cfg.max_audio_seconds * 16000) as usize;

        let err_fn = |e: cpal::StreamError| {
            tracing::error!("麦克风采集错误: {e}");
        };

        let handle = std::thread::spawn(move || {
            let stream = match sample_format {
                SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &_| {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            return;
                        }
                        let mut buf = buffer.write();
                        let ratio = sr_out / sr_in;
                        for (i, &s) in data.iter().enumerate() {
                            let out_idx = (i as f32 * ratio) as usize;
                            if out_idx < max_samples {
                                while buf.len() <= out_idx {
                                    buf.push_back(0);
                                }
                                buf[out_idx] = s;
                            }
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
                        let mut buf = buffer.write();
                        let ratio = sr_out / sr_in;
                        for (i, &s) in data.iter().enumerate() {
                            let out_idx = (i as f32 * ratio) as usize;
                            let pcm = (s as i32 - 32768) as i16;
                            if out_idx < max_samples {
                                while buf.len() <= out_idx {
                                    buf.push_back(0);
                                }
                                buf[out_idx] = pcm;
                            }
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
                        let mut buf = buffer.write();
                        let ratio = sr_out / sr_in;
                        for (i, &s) in data.iter().enumerate() {
                            let out_idx = (i as f32 * ratio) as usize;
                            let pcm = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                            if out_idx < max_samples {
                                while buf.len() <= out_idx {
                                    buf.push_back(0);
                                }
                                buf[out_idx] = pcm;
                            }
                        }
                    },
                    err_fn,
                    None,
                ),
                _ => {
                    tracing::error!("不支持的采样格式: {:?}", sample_format);
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
            // 阻塞线程直到 stop_flag 置位，Stream 在线程退出时 drop
            while !stop_for_loop.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            drop(stream);
        });

        self.stop_flag = Some(stop_flag);
        self.capture_thread = Some(handle);
        Ok(())
    }

    /// 把 i16 缓冲打包成 WAV 字节流（16kHz 单声道 16-bit）
    fn build_wav(samples: &[i16]) -> Vec<u8> {
        let num_channels: u16 = 1;
        let sample_rate: u32 = 16000;
        let bits_per_sample: u16 = 16;
        let data_len = samples.len() * 2;
        let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
        let block_align = num_channels * bits_per_sample / 8;
        let mut out = Vec::with_capacity(44 + data_len);
        // RIFF header
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        // fmt chunk
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&num_channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bits_per_sample.to_le_bytes());
        // data chunk
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data_len as u32).to_le_bytes());
        for &s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    /// 调用 Whisper HTTP 服务
    async fn recognize(&self, wav: Vec<u8>) -> VivianResult<String> {
        if self.whisper_cfg.server_url.is_empty() {
            return Err(VivianError::Speech(
                "Whisper 服务地址未配置".to_string(),
            ));
        }
        let base = self.whisper_cfg.server_url.trim_end_matches('/').to_string();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| VivianError::Network(format!("构建 HTTP 客户端失败: {e}")))?;

        // 语言代码：BCP-47 取主语言（zh-CN → zh, en-US → en, ja-JP → ja）
        let lang = self.config.language.split('-').next().unwrap_or("en").to_string();

        match self.whisper_cfg.api_format {
            WhisperApiFormat::WhisperCpp => {
                // whisper.cpp server: POST /inference
                // multipart: file=<wav>, response_format=json, language=<lang>
                let part = reqwest::multipart::Part::bytes(wav)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| VivianError::Network(format!("构造 multipart 失败: {e}")))?;
                let form = reqwest::multipart::Form::new()
                    .text("response_format", "json")
                    .text("language", lang)
                    .part("file", part);
                let url = format!("{}/inference", base);
                let mut req = client.post(&url).multipart(form);
                if !self.whisper_cfg.api_key.is_empty() {
                    req = req.header("Authorization", format!("Bearer {}", self.whisper_cfg.api_key));
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| VivianError::Network(format!("Whisper 请求失败: {e}")))?;
                let status = resp.status();
                let body = resp
                    .text()
                    .await
                    .map_err(|e| VivianError::Network(format!("读取响应失败: {e}")))?;
                if !status.is_success() {
                    return Err(VivianError::Speech(format!(
                        "Whisper 识别失败 ({}): {}",
                        status, body
                    )));
                }
                // whisper.cpp server 返回 {"text": "..."}
                let v: serde_json::Value = serde_json::from_str(&body)
                    .map_err(|e| VivianError::Serialization(format!("解析 Whisper 响应失败: {e}")))?;
                let text = v
                    .get("text")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                Ok(text)
            }
            WhisperApiFormat::Openai => {
                // OpenAI 兼容: POST /v1/audio/transcriptions
                // multipart: file=<wav>, model=<任意>, response_format=json, language=<lang>
                let part = reqwest::multipart::Part::bytes(wav)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| VivianError::Network(format!("构造 multipart 失败: {e}")))?;
                let form = reqwest::multipart::Form::new()
                    .text("model", "whisper-1")
                    .text("response_format", "json")
                    .text("language", lang)
                    .part("file", part);
                let url = format!("{}/v1/audio/transcriptions", base);
                let mut req = client.post(&url).multipart(form);
                if !self.whisper_cfg.api_key.is_empty() {
                    req = req.header("Authorization", format!("Bearer {}", self.whisper_cfg.api_key));
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| VivianError::Network(format!("Whisper 请求失败: {e}")))?;
                let status = resp.status();
                let body = resp
                    .text()
                    .await
                    .map_err(|e| VivianError::Network(format!("读取响应失败: {e}")))?;
                if !status.is_success() {
                    return Err(VivianError::Speech(format!(
                        "Whisper 识别失败 ({}): {}",
                        status, body
                    )));
                }
                // OpenAI 格式返回 {"text": "..."}
                let v: serde_json::Value = serde_json::from_str(&body)
                    .map_err(|e| VivianError::Serialization(format!("解析 Whisper 响应失败: {e}")))?;
                let text = v
                    .get("text")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                Ok(text)
            }
        }
    }

    /// SSE 流式识别：POST /v1/audio/transcriptions?stream=true
    ///
    /// 响应为 `text/event-stream`，每行 `data: {"text": "..."}` 或 `data: [DONE]`。
    /// 兼容累积 `text` 与增量 `delta` 两种字段；每段到达即 emit `PartialResult`。
    /// 若服务端不支持流式（返回普通 JSON），自动降级为一次性返回。
    async fn recognize_stream(
        &self,
        wav: Vec<u8>,
        event_tx: &broadcast::Sender<AsrEvent>,
    ) -> VivianResult<String> {
        use futures::StreamExt;

        if self.whisper_cfg.server_url.is_empty() {
            return Err(VivianError::Speech("Whisper 服务地址未配置".to_string()));
        }
        let base = self.whisper_cfg.server_url.trim_end_matches('/').to_string();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| VivianError::Network(format!("构建 HTTP 客户端失败: {e}")))?;

        let lang = self.config.language.split('-').next().unwrap_or("en").to_string();
        let model = self
            .whisper_cfg
            .service_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("whisper-1")
            .to_string();

        // SSE 流式仅支持 OpenAI 兼容端点
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| VivianError::Network(format!("构造 multipart 失败: {e}")))?;
        let form = reqwest::multipart::Form::new()
            .text("model", model)
            .text("response_format", "json")
            .text("language", lang)
            .text("stream", "true")
            .part("file", part);
        let url = format!("{}/v1/audio/transcriptions", base);
        let mut req = client.post(&url).multipart(form);
        if !self.whisper_cfg.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.whisper_cfg.api_key));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| VivianError::Network(format!("Whisper 流式请求失败: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(VivianError::Speech(format!(
                "Whisper 流式识别失败 ({}): {}",
                status, body
            )));
        }

        // 检查是否真的是 SSE 流
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !content_type.contains("text/event-stream") {
            // 服务端不支持流式，降级为普通 JSON
            tracing::info!("[Whisper] 服务端未返回 text/event-stream，降级为一次性 JSON");
            let body = resp
                .text()
                .await
                .map_err(|e| VivianError::Network(format!("读取响应失败: {e}")))?;
            let v: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| VivianError::Serialization(format!("解析 Whisper 响应失败: {e}")))?;
            let text = v
                .get("text")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            return Ok(text);
        }

        // SSE 流式解析：按 \n\n 分割 event，每 event 含 data: 行
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut full_text = String::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result
                .map_err(|e| VivianError::Network(format!("读取 SSE chunk 失败: {e}")))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(idx) = buf.find("\n\n") {
                let event_str = buf[..idx].to_string();
                buf = buf[idx + 2..].to_string();
                for line in event_str.lines() {
                    let data = match line.strip_prefix("data: ") {
                        Some(d) => d,
                        None => continue,
                    };
                    if data.trim() == "[DONE]" {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                        // 兼容累积 text 与增量 delta 两种字段
                        if let Some(text) = v.get("text").and_then(|s| s.as_str()) {
                            if text != full_text.as_str() {
                                full_text = text.to_string();
                                let _ = event_tx.send(AsrEvent::partial(&full_text));
                            }
                        } else if let Some(delta) = v.get("delta").and_then(|s| s.as_str()) {
                            full_text.push_str(delta);
                            let _ = event_tx.send(AsrEvent::partial(&full_text));
                        }
                    }
                }
            }
        }

        Ok(full_text.trim().to_string())
    }
}

#[async_trait]
impl AsrEngine for WhisperBackend {
    async fn initialize(&mut self) -> VivianResult<bool> {
        // Whisper 后端无需预加载；仅校验服务地址非空
        if self.whisper_cfg.server_url.is_empty() {
            tracing::warn!("Whisper 服务地址未配置，后端标记为不可用");
            self.available = false;
            return Ok(false);
        }
        tracing::info!(
            "Whisper HTTP 后端初始化完成: url={} format={:?}",
            self.whisper_cfg.server_url,
            self.whisper_cfg.api_format
        );
        Ok(true)
    }

    async fn start_recording(&mut self) -> VivianResult<()> {
        if !self.available {
            return Err(VivianError::NotImplemented("Whisper 后端不可用".to_string()));
        }
        if self.is_running {
            return Ok(());
        }
        self.start_capture()?;
        self.is_running = true;
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AsrEvent::Started);
        }
        tracing::info!("Whisper 录音已启动");
        Ok(())
    }

    async fn stop_recording(&mut self) -> VivianResult<()> {
        if let Some(flag) = &self.stop_flag {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
        self.stop_flag = None;
        self.is_running = false;

        // 取出采集到的音频
        let samples: Vec<i16> = {
            let mut buf = self.buffer.write();
            buf.drain(..).collect()
        };
        if samples.is_empty() {
            if let Some(tx) = &self.event_tx {
                let _ = tx.send(AsrEvent::Stopped);
            }
            return Ok(());
        }
        let wav = Self::build_wav(&samples);

        // 根据流式模式分流
        let result = match self.whisper_cfg.streaming_mode {
            WhisperStreamingMode::Sse => {
                if let Some(tx) = &self.event_tx {
                    self.recognize_stream(wav, tx).await
                } else {
                    self.recognize(wav).await
                }
            }
            WhisperStreamingMode::None | WhisperStreamingMode::RealtimeWs => {
                // realtime_ws 模式由 WhisperRealtimeBackend 处理；此处兜底用普通 recognize
                self.recognize(wav).await
            }
        };

        match result {
            Ok(text) => {
                if !text.is_empty() {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(AsrEvent::final_result(text, 0.8));
                    }
                }
            }
            Err(e) => {
                tracing::error!("Whisper 识别失败: {e}");
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(AsrEvent::error(format!("{e}")));
                }
            }
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AsrEvent::Stopped);
        }
        tracing::info!("Whisper 录音已停止");
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> VivianResult<String> {
        // 把 f32 转 i16
        let samples: Vec<i16> = audio
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        let wav = Self::build_wav(&samples);
        self.recognize(wav).await
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn supports_silence_detection(&self) -> bool {
        true
    }

    fn supports_partial_results(&self) -> bool {
        matches!(
            self.whisper_cfg.streaming_mode,
            WhisperStreamingMode::Sse | WhisperStreamingMode::RealtimeWs
        )
    }

    fn dispose(&mut self) {
        if let Some(flag) = &self.stop_flag {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
        self.stop_flag = None;
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

impl std::fmt::Debug for WhisperBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperBackend")
            .field("server_url", &self.whisper_cfg.server_url)
            .field("api_format", &self.whisper_cfg.api_format)
            .field("available", &self.available)
            .field("is_running", &self.is_running)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whisper_backend_from_config() {
        let backend = WhisperBackend::from_config(AsrConfig::default(), WhisperConfig::default());
        assert_eq!(backend.backend_type(), AsrBackendType::Whisper);
        assert!(!backend.supports_partial_results());
        assert!(backend.supports_silence_detection());
    }

    #[test]
    fn test_build_wav_header() {
        let samples = vec![0i16; 100];
        let wav = WhisperBackend::build_wav(&samples);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_len, 200);
    }

    #[tokio::test]
    async fn test_whisper_initialize_no_url() {
        let mut backend = WhisperBackend::from_config(AsrConfig::default(), WhisperConfig::default());
        let ok = backend.initialize().await.unwrap_or(false);
        assert!(!ok);
    }

    #[test]
    fn test_whisper_api_format_default() {
        assert_eq!(WhisperApiFormat::default(), WhisperApiFormat::Openai);
    }
}
