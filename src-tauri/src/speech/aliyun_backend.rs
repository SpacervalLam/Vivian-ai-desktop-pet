//! 阿里云 NLS 实时语音识别后端
//!
//! - 使用阿里云 NLS WebSocket 流式 ASR（一句话识别 / 实时转写）
//! - 通过 `cpal` 采集麦克风音频，重采样到 16kHz 单声道 16-bit PCM
//! - 采集到的 PCM 按帧（200ms = 6400 字节）通过 WebSocket 二进制帧发送
//! - 支持流式 partial 结果（TranscriptionResultChanged 事件）
//! - 停止录音时发送剩余音频 + StopTranscription 指令
//!
//! ## 协议
//! - URL：`wss://nls-gateway.cn-shanghai.aliyuncs.com/ws/v1?token=<token>`
//! - 鉴权：先用 AccessKeyId + AccessKeySecret 调用 CreateToken API 获取临时 token
//! - JSON 文本帧：StartTranscription / StopTranscription
//! - 二进制帧：PCM 音频（16kHz/16bit/mono）
//! - 事件：TranscriptionStarted / TranscriptionResultChanged / SentenceEnd / TranscriptionCompleted
//!
//! ## 配置要求
//! - app_key：阿里云 NLS 项目 AppKey
//! - access_key_id / access_key_secret：RAM 用户密钥（需 NLS 访问权限）
//! - 获取：https://nls-portal.console.aliyun.com/

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use parking_lot::RwLock;
use tokio::sync::{broadcast, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::error::{VivianError, VivianResult};

use super::asr::{AsrBackendType, AsrConfig, AsrEngine, AsrEvent};

/// 阿里云 NLS 后端专属配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliyunAsrConfig {
    /// NLS 项目 AppKey
    pub app_key: String,
    /// RAM AccessKeyId
    pub access_key_id: String,
    /// RAM AccessKeySecret
    pub access_key_secret: String,
    /// 单次请求最大音频时长（秒），避免长录音阻塞
    pub max_audio_seconds: u32,
}

impl Default for AliyunAsrConfig {
    fn default() -> Self {
        Self {
            app_key: String::new(),
            access_key_id: String::new(),
            access_key_secret: String::new(),
            max_audio_seconds: 60,
        }
    }
}

const NLS_GATEWAY: &str = "wss://nls-gateway.cn-shanghai.aliyuncs.com/ws/v1";
const NLS_TOKEN_API: &str = "https://nls-meta.cn-shanghai.aliyuncs.com/";

/// 阿里云 NLS 后端实例
pub struct AliyunBackend {
    config: AsrConfig,
    aliyun_cfg: AliyunAsrConfig,
    available: bool,
    is_running: bool,
    event_tx: Option<broadcast::Sender<AsrEvent>>,
    /// 麦克风采集线程句柄（线程内独占 cpal Stream，规避非 Send 限制）
    capture_thread: Option<std::thread::JoinHandle<()>>,
    stop_flag: Option<Arc<AtomicBool>>,
    /// 16-bit PCM 样本缓冲（16kHz 单声道）
    buffer: Arc<RwLock<VecDeque<i16>>>,
    /// WebSocket 写入端（用于发送音频帧和停止指令）
    ws_writer: Arc<Mutex<Option<futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Message>>>>,
    /// WebSocket 读取任务句柄
    read_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// 复用的 HTTP 客户端（直连，绕过代理；阿里云 NLS 服务国内可直连）
    client: reqwest::Client,
}

impl AliyunBackend {
    pub fn from_config(config: AsrConfig, aliyun_cfg: AliyunAsrConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config,
            aliyun_cfg,
            available: true,
            is_running: false,
            event_tx: None,
            capture_thread: None,
            stop_flag: None,
            buffer: Arc::new(RwLock::new(VecDeque::with_capacity(16000 * 60))),
            ws_writer: Arc::new(Mutex::new(None)),
            read_task: Arc::new(Mutex::new(None)),
            client,
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
        let sr_in = actual_rate as f32;
        let sr_out = 16000f32;
        let max_samples = (self.aliyun_cfg.max_audio_seconds * 16000) as usize;
        let stop_for_thread = stop_flag.clone();
        let stop_for_loop = stop_flag.clone();

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
            while !stop_for_loop.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            drop(stream);
        });

        self.stop_flag = Some(stop_flag);
        self.capture_thread = Some(handle);
        Ok(())
    }

    /// 用 AccessKeyId + AccessKeySecret 获取阿里云 NLS 临时 token
    async fn get_token(&self) -> VivianResult<String> {
        let access_key_id = &self.aliyun_cfg.access_key_id;
        let access_key_secret = &self.aliyun_cfg.access_key_secret;
        let params: Vec<(String, String)> = vec![
            ("AccessKeyId".into(), access_key_id.into()),
            ("Action".into(), "CreateToken".into()),
            ("Format".into(), "JSON".into()),
            ("RegionId".into(), "cn-shanghai".into()),
            ("SignatureMethod".into(), "HMAC-SHA256".into()),
            ("SignatureNonce".into(), uuid_str()),
            ("SignatureVersion".into(), "1.0".into()),
            ("Timestamp".into(), iso8601_now()),
            ("Version".into(), "2019-02-28".into()),
        ];

        // 按字母序排列参数
        let mut sorted = params.clone();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let canonical_query = sorted
            .iter()
            .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // 构建签名字符串
        let string_to_sign = format!("GET&%2F&{}", percent_encode(&canonical_query));

        // HMAC-SHA256 签名（阿里云签名附加 &）
        let key = format!("{}&", access_key_secret);
        let signature = hmac_sha256_base64(key.as_bytes(), string_to_sign.as_bytes());

        let url = format!(
            "{}?{}&Signature={}",
            NLS_TOKEN_API,
            canonical_query,
            percent_encode(&signature)
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| VivianError::Network(format!("获取 token 请求失败: {e}")))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| VivianError::Network(format!("读取 token 响应失败: {e}")))?;
        if !status.is_success() {
            // Mask 错误响应体，避免泄露敏感信息或过长内容到日志
            let masked = truncate_for_log(&body, 100);
            return Err(VivianError::Speech(format!(
                "获取阿里云 token 失败 ({}): {}",
                status, masked
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| VivianError::Serialization(format!("解析 token 响应失败: {e}")))?;
        let token = v
            .get("Token")
            .and_then(|t| t.get("Id"))
            .and_then(|s| s.as_str())
            .ok_or_else(|| {
                // Mask 响应体，避免可能包含的敏感信息泄露
                let masked = truncate_for_log(&body, 100);
                VivianError::Speech(format!("token 响应中未找到 Token.Id: {}", masked))
            })?;
        Ok(token.to_string())
    }

    /// 启动 WebSocket 连接并发送 StartTranscription
    async fn start_ws_session(&self, token: &str) -> VivianResult<()> {
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        // token 经 URL 传递（NLS 协议限制），错误处理需 mask
        let url = format!(
            "{}?token={}",
            NLS_GATEWAY,
            percent_encode(token)
        );
        let request = url
            .into_client_request()
            .map_err(|e| {
                // mask URL 中可能残留的 token
                let msg = format!("{e}").replace(token, "***");
                VivianError::Speech(format!("解析 NLS URL 失败: {}", msg))
            })?;

        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| {
                // mask 错误信息中可能残留的 token
                let msg = format!("{e}").replace(token, "***");
                VivianError::Network(format!("连接阿里云 NLS WebSocket 失败: {}", msg))
            })?;

        let (mut write, mut read) = ws_stream.split();
        let _ = write.send(Message::Text(start_transcription_json(
            &self.aliyun_cfg.app_key,
            &self.config.language,
        ))).await;

        // 启动读取任务，解析服务端事件并广播
        let event_tx = self.event_tx.clone();
        let read_task = tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                let msg = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Binary(_)) => continue,
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => continue,
                };
                if let Some(event) = parse_nls_message(&msg) {
                    if let Some(tx) = &event_tx {
                        let _ = tx.send(event);
                    }
                }
            }
        });

        *self.ws_writer.lock().await = Some(write);
        *self.read_task.lock().await = Some(read_task);
        Ok(())
    }

    /// 发送剩余 PCM 音频 + StopTranscription 指令并关闭连接
    async fn stop_ws_session(&self) {
        // 发送剩余音频
        let remaining: Vec<i16> = {
            let mut buf = self.buffer.write();
            buf.drain(..).collect()
        };
        if !remaining.is_empty() {
            let mut bytes = Vec::with_capacity(remaining.len() * 2);
            for &s in &remaining {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            let mut writer = self.ws_writer.lock().await;
            if let Some(w) = writer.as_mut() {
                let _ = w.send(Message::Binary(bytes.into())).await;
            }
        }

        // 发送 StopTranscription
        let mut writer = self.ws_writer.lock().await;
        if let Some(w) = writer.as_mut() {
            let _ = w.send(Message::Text(stop_transcription_json(&self.aliyun_cfg.app_key))).await;
            let _ = w.close().await;
        }
        drop(writer);

        // 等待读取任务结束（最多 2 秒）
        let mut read_guard = self.read_task.lock().await;
        if let Some(handle) = read_guard.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
    }
}

/// 生成 StartTranscription JSON 指令
fn start_transcription_json(app_key: &str, language: &str) -> String {
    let lang_map = match language {
        "zh-CN" | "zh" => "zh-CN",
        "en-US" | "en" => "en-US",
        _ => "zh-CN",
    };
    let msg = serde_json::json!({
        "header": {
            "message_id": uuid_str(),
            "task_id": uuid_str(),
            "namespace": "SpeechTranscriber",
            "name": "StartTranscription",
            "appkey": app_key,
        },
        "payload": {
            "format": "pcm",
            "sample_rate": 16000,
            "enable_intermediate_result": true,
            "enable_punctuation_prediction": true,
            "enable_inverse_text_normalization": true,
            "max_sentence_silence": 800,
            "language": lang_map,
        }
    });
    msg.to_string()
}

/// 生成 StopTranscription JSON 指令
fn stop_transcription_json(app_key: &str) -> String {
    let msg = serde_json::json!({
        "header": {
            "message_id": uuid_str(),
            "task_id": uuid_str(),
            "namespace": "SpeechTranscriber",
            "name": "StopTranscription",
            "appkey": app_key,
        }
    });
    msg.to_string()
}

/// 解析阿里云 NLS 服务端 JSON 消息为 AsrEvent
fn parse_nls_message(raw: &str) -> Option<AsrEvent> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let header = v.get("header")?;
    let status = header.get("status").and_then(|s| s.as_i64()).unwrap_or(20000000);
    if status != 20000000 {
        let status_text = header.get("status_text").and_then(|s| s.as_str()).unwrap_or("");
        return Some(AsrEvent::error(format!(
            "阿里云 ASR 错误: status={}, msg={}",
            status, status_text
        )));
    }
    let event_name = header.get("name").and_then(|s| s.as_str())?;
    let payload = v.get("payload").cloned().unwrap_or_default();
    match event_name {
        "TranscriptionStarted" => None,
        "TranscriptionResultChanged" => {
            let text = payload.get("result").and_then(|s| s.as_str()).unwrap_or("");
            if text.is_empty() {
                None
            } else {
                Some(AsrEvent::partial(text))
            }
        }
        "SentenceEnd" => {
            let text = payload.get("result").and_then(|s| s.as_str()).unwrap_or("");
            let confidence = payload
                .get("confidence")
                .and_then(|s| s.as_f64())
                .unwrap_or(0.9);
            if text.is_empty() {
                None
            } else {
                Some(AsrEvent::final_result(text, confidence))
            }
        }
        "TranscriptionCompleted" => None,
        _ => None,
    }
}

/// 生成不带连字符的 UUID 字符串
fn uuid_str() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 生成 ISO 8601 时间戳（UTC）
fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // 简化的 ISO 8601 格式：YYYY-MM-DDThh:mm:ssZ
    let (y, mo, d, h, mi, s) = epoch_to_ymdhms(secs);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s)
}

/// Unix 时间戳转 UTC 年月日时分秒（简化算法）
fn epoch_to_ymdhms(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    // 从 1970-01-01 起算
    let mut year = 1970u64;
    let mut remaining_days = days;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }
    let month_lengths = if is_leap(year) {
        [31u64, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for &ml in &month_lengths {
        if remaining_days < ml {
            break;
        }
        remaining_days -= ml;
        month += 1;
    }
    let day = remaining_days + 1;
    (year, month, day, h, m, s)
}

fn is_leap(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// 阿里云签名用 percent-encoding（RFC 3986）
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

/// 截断字符串用于日志/错误输出，避免泄露敏感信息或过长内容。
/// 按 UTF-8 字符边界安全截断到 max_bytes 字节以内。
fn truncate_for_log(s: &str, max_bytes: usize) -> String {
    if s.len() > max_bytes {
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

/// HMAC-SHA256 → Base64
fn hmac_sha256_base64(key: &[u8], data: &[u8]) -> String {
    use sha2::digest::Mac;
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key 长度错误");
    mac.update(data);
    let bytes = mac.finalize().into_bytes();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

#[async_trait]
impl AsrEngine for AliyunBackend {
    async fn initialize(&mut self) -> VivianResult<bool> {
        if self.aliyun_cfg.app_key.is_empty() {
            tracing::warn!("阿里云 NLS AppKey 未配置，后端标记为不可用");
            self.available = false;
            return Ok(false);
        }
        if self.aliyun_cfg.access_key_id.is_empty()
            || self.aliyun_cfg.access_key_secret.is_empty()
        {
            tracing::warn!("阿里云 AccessKeyId/Secret 未配置，后端标记为不可用");
            self.available = false;
            return Ok(false);
        }
        tracing::info!("阿里云 NLS 后端初始化完成: app_key={}", self.aliyun_cfg.app_key);
        Ok(true)
    }

    async fn start_recording(&mut self) -> VivianResult<()> {
        if !self.available {
            return Err(VivianError::Speech("阿里云后端不可用".to_string()));
        }
        if self.is_running {
            return Ok(());
        }

        // 1. 获取 token
        let token = self.get_token().await?;

        // 2. 启动 WebSocket 会话
        self.start_ws_session(&token).await?;

        // 3. 启动麦克风采集
        self.start_capture()?;
        self.is_running = true;
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AsrEvent::Started);
        }
        tracing::info!("阿里云 NLS 录音已启动");
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

        // 发送剩余音频 + StopTranscription，等待服务端返回最终结果
        self.stop_ws_session().await;

        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AsrEvent::Stopped);
        }
        tracing::info!("阿里云 NLS 录音已停止");
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32]) -> VivianResult<String> {
        // 一次性文件转写：建连 → 发 StartTranscription → 分帧发 PCM → 发 StopTranscription → 累积 SentenceEnd 结果
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let token = self.get_token().await?;
        let url = format!("{}?token={}", NLS_GATEWAY, percent_encode(&token));
        let request = url
            .into_client_request()
            .map_err(|e| {
                let msg = format!("{e}").replace(&token, "***");
                VivianError::Speech(format!("解析 NLS URL 失败: {}", msg))
            })?;
        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| {
                let msg = format!("{e}").replace(&token, "***");
                VivianError::Network(format!("连接阿里云 NLS WebSocket 失败: {}", msg))
            })?;
        let (mut write, mut read) = ws_stream.split();

        // 发 StartTranscription
        write
            .send(Message::Text(start_transcription_json(
                &self.aliyun_cfg.app_key,
                &self.config.language,
            )))
            .await
            .map_err(|e| VivianError::Network(format!("发送 StartTranscription 失败: {e}")))?;

        // 分帧发送 PCM（200ms = 3200 samples @ 16kHz）
        let i16_samples: Vec<i16> = audio
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        for chunk in i16_samples.chunks(3200) {
            let mut bytes = Vec::with_capacity(chunk.len() * 2);
            for &s in chunk {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            write
                .send(Message::Binary(bytes.into()))
                .await
                .map_err(|e| VivianError::Network(format!("发送音频帧失败: {e}")))?;
        }

        // 发 StopTranscription
        write
            .send(Message::Text(stop_transcription_json(
                &self.aliyun_cfg.app_key,
            )))
            .await
            .map_err(|e| VivianError::Network(format!("发送 StopTranscription 失败: {e}")))?;

        // 读取消息，累积 SentenceEnd 的 result，直到 TranscriptionCompleted 或连接关闭
        let mut result = String::new();
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(t)) => {
                    let v: serde_json::Value = match serde_json::from_str(&t) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let name = v
                        .get("header")
                        .and_then(|h| h.get("name"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    match name {
                        "SentenceEnd" => {
                            let text = v
                                .get("payload")
                                .and_then(|p| p.get("result"))
                                .and_then(|s| s.as_str())
                                .unwrap_or("");
                            if !text.is_empty() {
                                if !result.is_empty() {
                                    result.push(' ');
                                }
                                result.push_str(text);
                            }
                        }
                        "TranscriptionCompleted" => break,
                        _ => {}
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        let _ = write.close().await;
        Ok(result.trim().to_string())
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
        self.is_running = false;
        self.available = false;
    }

    fn backend_type(&self) -> AsrBackendType {
        AsrBackendType::Aliyun
    }

    fn set_event_sender(&mut self, sender: broadcast::Sender<AsrEvent>) {
        self.event_tx = Some(sender);
    }
}

impl std::fmt::Debug for AliyunBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AliyunBackend")
            .field("app_key", &self.aliyun_cfg.app_key)
            .field("available", &self.available)
            .field("is_running", &self.is_running)
            .finish()
    }
}
