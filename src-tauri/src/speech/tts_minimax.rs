//! MiniMax TTS 后端
//!
//! 对齐真实 API: https://platform.minimaxi.com/document
//!
//! ## 端点
//! - WebSocket: wss://api.minimaxi.com/ws/v1/t2a_v2 — 文本转语音（流式合成）
//!
//! ## 鉴权
//! - Authorization: Bearer <api_key>（WebSocket 握手头）
//!
//! ## 协议流程
//! 1. 连接成功（connected_success）→ 发送 task_start（模型 + 音色 + 音频设置）
//! 2. task_started → 发送 task_continue（待合成文本）
//! 3. 收到 data.audio 块（hex 编码）→ 解码拼接
//! 4. is_final=true → 发送 task_finish，合成完成
//!
//! ## 模型
//! - speech-01-turbo（极速，默认）
//! - speech-01-hd（高保真）
//!
//! ## 音频设置
//! - 格式: mp3 / wav / pcm（默认 mp3）
//! - 采样率: 16000 / 24000 / 32000（默认 32000）
//! - 比特率: 128000
//! - 声道: 1（单声道）
//!
//! Vivian 集成策略：使用 WebSocket 一次性合成完整音频（与批量合成架构一致），
//! 不实现逐句流式播放（Vivian 的 TtsManager 是 synthesize → play 模式）

use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

use crate::error::{VivianError, VivianResult};

use super::tts::TtsConfig;
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult};

/// MiniMax WebSocket 端点
const WS_URL: &str = "wss://api.minimaxi.com/ws/v1/t2a_v2";

pub struct MiniMaxBackend;

impl MiniMaxBackend {
    pub fn new() -> Self {
        Self
    }

    /// 从配置解析 API Key
    fn api_key(config: &TtsConfig) -> VivianResult<&str> {
        config
            .minimax_key
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("MiniMax API Key 未配置".to_string()))
    }

    /// 从配置解析音色 ID
    fn voice_id(config: &TtsConfig) -> VivianResult<&str> {
        config
            .minimax_voice_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("MiniMax 音色 ID 未配置".to_string()))
    }

    /// 解析模型（默认极速）
    fn model(config: &TtsConfig) -> String {
        config
            .minimax_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "speech-01-turbo".to_string())
    }

    /// 解析音频格式（默认 mp3）
    fn format(config: &TtsConfig) -> (String, AudioFormat) {
        let fmt = config
            .minimax_format
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("mp3");
        let audio_fmt = match fmt {
            "wav" => AudioFormat::Wav,
            "pcm" => AudioFormat::Pcm,
            _ => AudioFormat::Mp3,
        };
        (fmt.to_string(), audio_fmt)
    }

    /// 解析采样率（默认 32000）
    fn sample_rate(config: &TtsConfig) -> u32 {
        config.minimax_sample_rate.unwrap_or(32000)
    }
}

#[async_trait]
impl TtsBackend for MiniMaxBackend {
    fn name(&self) -> &'static str {
        "minimax"
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        if text.trim().is_empty() {
            let (_, fmt) = Self::format(config);
            return Ok(TtsSynthesisResult::new(Vec::new(), fmt));
        }

        let api_key = Self::api_key(config)?.to_string();
        let voice_id = Self::voice_id(config)?.to_string();
        let model = Self::model(config);
        let (format_str, audio_format) = Self::format(config);
        let sample_rate = Self::sample_rate(config);

        // 构建 WebSocket 握手请求（带 Authorization 头）
        let mut request = WS_URL
            .into_client_request()
            .map_err(|e| VivianError::Speech(format!("解析 MiniMax URL 失败: {e}")))?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {api_key}").parse().unwrap(),
        );

        // 连接 WebSocket（30 秒超时）
        let connect_fut = connect_async(request);
        let (ws_stream, _) = timeout(Duration::from_secs(30), connect_fut)
            .await
            .map_err(|_| VivianError::Speech("MiniMax WebSocket 连接超时".to_string()))?
            .map_err(|e| VivianError::Speech(format!("MiniMax WebSocket 连接失败: {e}")))?;

        let (mut write, mut read) = ws_stream.split();

        // 等待 connected_success 事件
        let connected = wait_event(&mut read, Duration::from_secs(15)).await?;
        if connected.get("event").and_then(|v| v.as_str()) != Some("connected_success") {
            return Err(VivianError::Speech(format!(
                "MiniMax 握手失败: {}",
                connected
            )));
        }

        // 发送 task_start
        let task_start = json!({
            "event": "task_start",
            "model": model,
            "voice_setting": {
                "voice_id": voice_id,
                "speed": config.rate,
                "vol": config.volume,
                "pitch": 0,
                "english_normalization": false,
            },
            "audio_setting": {
                "sample_rate": sample_rate,
                "bitrate": 128000,
                "format": format_str,
                "channel": 1,
            },
        });
        write
            .send(Message::Text(task_start.to_string()))
            .await
            .map_err(|e| VivianError::Speech(format!("发送 task_start 失败: {e}")))?;

        // 等待 task_started 事件
        let started = wait_event(&mut read, Duration::from_secs(15)).await?;
        if started.get("event").and_then(|v| v.as_str()) != Some("task_started") {
            // 检查错误
            if let Some(msg) = error_from_message(&started) {
                return Err(VivianError::Speech(format!("MiniMax task_start 失败: {msg}")));
            }
            return Err(VivianError::Speech(format!(
                "MiniMax 未返回 task_started: {}",
                started
            )));
        }

        // 发送 task_continue（文本）
        let task_continue = json!({
            "event": "task_continue",
            "text": text,
        });
        write
            .send(Message::Text(task_continue.to_string()))
            .await
            .map_err(|e| VivianError::Speech(format!("发送 task_continue 失败: {e}")))?;

        // 接收音频块直到 is_final
        let mut audio_chunks: Vec<Vec<u8>> = Vec::new();
        let synth_deadline = Duration::from_secs(60);
        let synth_start = std::time::Instant::now();

        loop {
            let remaining = synth_deadline
                .checked_sub(synth_start.elapsed())
                .unwrap_or(Duration::from_millis(100));
            if remaining.is_zero() {
                return Err(VivianError::Speech(
                    "MiniMax 合成超时（60 秒）".to_string(),
                ));
            }

            let msg = match timeout(remaining, read.next()).await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    return Err(VivianError::Speech(
                        "MiniMax WebSocket 连接关闭".to_string(),
                    ));
                }
                Err(_) => {
                    return Err(VivianError::Speech(
                        "MiniMax 等待音频块超时".to_string(),
                    ));
                }
            };

            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    return Err(VivianError::Speech(format!(
                        "MiniMax WebSocket 读取错误: {e}"
                    )));
                }
            };

            if msg.is_close() {
                break;
            }

            let data: Value = match msg {
                Message::Text(t) => serde_json::from_str(&t).map_err(|e| {
                    VivianError::Speech(format!("MiniMax JSON 解析失败: {e}"))
                })?,
                Message::Binary(b) => serde_json::from_slice(&b).map_err(|e| {
                    VivianError::Speech(format!("MiniMax JSON 解析失败: {e}"))
                })?,
                _ => continue,
            };

            // 检查错误
            if let Some(err_msg) = error_from_message(&data) {
                return Err(VivianError::Speech(format!(
                    "MiniMax 合成失败: {err_msg}"
                )));
            }

            // 收集音频块（hex 编码）
            if let Some(audio_hex) = data
                .get("data")
                .and_then(|d| d.get("audio"))
                .and_then(|a| a.as_str())
            {
                if !audio_hex.is_empty() {
                    let bytes = hex_decode(audio_hex)?;
                    audio_chunks.push(bytes);
                }
            }

            // 合成完成
            if data.get("is_final").and_then(|v| v.as_bool()) == Some(true) {
                // 发送 task_finish
                let _ = write
                    .send(Message::Text(json!({"event": "task_finish"}).to_string()))
                    .await;
                let _ = write.close().await;
                break;
            }
        }

        let audio: Vec<u8> = audio_chunks.into_iter().flatten().collect();

        if audio.is_empty() {
            return Err(VivianError::Speech(
                "MiniMax 合成返回空音频".to_string(),
            ));
        }

        Ok(TtsSynthesisResult::new(audio, audio_format))
    }

    async fn health_check(&self, config: &TtsConfig) -> bool {
        // API Key 和音色 ID 都配置即视为健康
        Self::api_key(config).is_ok() && Self::voice_id(config).is_ok()
    }
}

/// 等待下一条 WebSocket 文本消息并解析为 JSON
async fn wait_event(
    read: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    >,
    deadline: Duration,
) -> VivianResult<Value> {
    let msg = timeout(deadline, read.next())
        .await
        .map_err(|_| VivianError::Speech("MiniMax 等待事件超时".to_string()))?
        .ok_or_else(|| VivianError::Speech("MiniMax WebSocket 连接关闭".to_string()))?
        .map_err(|e| VivianError::Speech(format!("MiniMax WebSocket 读取错误: {e}")))?;

    match msg {
        Message::Text(t) => serde_json::from_str(&t)
            .map_err(|e| VivianError::Speech(format!("MiniMax JSON 解析失败: {e}"))),
        Message::Binary(b) => serde_json::from_slice(&b)
            .map_err(|e| VivianError::Speech(format!("MiniMax JSON 解析失败: {e}"))),
        Message::Close(_) => Err(VivianError::Speech(
            "MiniMax WebSocket 连接已关闭".to_string(),
        )),
        _ => Err(VivianError::Speech(
            "MiniMax 收到非文本消息".to_string(),
        )),
    }
}

/// 从 JSON 消息中提取错误信息（base_resp.status_code != 0）
fn error_from_message(data: &Value) -> Option<String> {
    let status_code = data
        .get("base_resp")
        .and_then(|b| b.get("status_code"))
        .and_then(|c| c.as_i64())?;
    if status_code == 0 {
        return None;
    }
    let status_msg = data
        .get("base_resp")
        .and_then(|b| b.get("status_msg"))
        .and_then(|s| s.as_str())
        .unwrap_or("未知错误");
    Some(format!("{} (code: {})", status_msg, status_code))
}

/// hex 字符串解码为字节
fn hex_decode(hex: &str) -> VivianResult<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return Err(VivianError::Speech(format!(
            "MiniMax 音频 hex 长度异常: {}",
            hex.len()
        )));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(c: u8) -> VivianResult<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(VivianError::Speech(format!(
            "MiniMax 音频 hex 字符非法: {c}"
        ))),
    }
}
