//! Edge-TTS 后端:通过 WebSocket 调用微软 Edge 浏览器朗读服务
//!
//! - 免费、高质量、无需 API Key
//! - 支持 WordBoundary 事件(音素级唇形同步)
//! - 输出 MP3 音频(24kHz 48kbps mono)
//! - 协议: WSS → 发送 speech.config + SSML → 接收二进制音频 + 文本词边界

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{handshake::client::generate_key, http::Uri},
    MaybeTlsStream, WebSocketStream,
};
use uuid::Uuid;

use crate::error::{VivianError, VivianResult};

use super::tts::{TtsConfig, VoiceInfo};
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult, WordBoundary};

const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const EDGE_TTS_URL: &str = "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
const EDGE_ORIGIN: &str = "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold";
const CHROMIUM_FULL_VERSION: &str = "143.0.3650.75";
const CHROMIUM_MAJOR_VERSION: &str = "143";

/// Edge-TTS 语音列表（中文、英文、日语，不含方言）
const EDGE_VOICES: &[(&str, &str, &str)] = &[
    ("zh-CN-XiaoxiaoNeural", "晓晓 (女)", "zh-CN"),
    ("zh-CN-XiaoyiNeural", "晓伊 (女)", "zh-CN"),
    ("zh-CN-XiaohanNeural", "晓涵 (女)", "zh-CN"),
    ("zh-CN-XiaolinNeural", "晓琳 (女)", "zh-CN"),
    ("zh-CN-XiaomoNeural", "晓墨 (女)", "zh-CN"),
    ("zh-CN-XiaoxuanNeural", "晓轩 (女)", "zh-CN"),
    ("zh-CN-YunjianNeural", "云健 (男)", "zh-CN"),
    ("zh-CN-YunxiNeural", "云希 (男)", "zh-CN"),
    ("zh-CN-YunxiaNeural", "云夏 (男)", "zh-CN"),
    ("zh-CN-YunyangNeural", "云扬 (男)", "zh-CN"),
    ("zh-CN-XiaozhiNeural", "晓志 (男)", "zh-CN"),
    ("en-US-AriaNeural", "Aria (女)", "en-US"),
    ("en-US-AnaNeural", "Ana (女)", "en-US"),
    ("en-US-EmmaNeural", "Emma (女)", "en-US"),
    ("en-US-JennyNeural", "Jenny (女)", "en-US"),
    ("en-US-MichelleNeural", "Michelle (女)", "en-US"),
    ("en-US-GabrielaNeural", "Gabriela (女)", "en-US"),
    ("en-US-GuyNeural", "Guy (男)", "en-US"),
    ("en-US-AndrewNeural", "Andrew (男)", "en-US"),
    ("en-US-BrianNeural", "Brian (男)", "en-US"),
    ("en-US-EricNeural", "Eric (男)", "en-US"),
    ("en-US-JamesNeural", "James (男)", "en-US"),
    ("en-US-RyanNeural", "Ryan (男)", "en-US"),
    ("en-US-DavisNeural", "Davis (男)", "en-US"),
    ("ja-JP-NanamiNeural", "七海 (女)", "ja-JP"),
    ("ja-JP-KeitaNeural", "圭太 (男)", "ja-JP"),
];

pub struct EdgeTtsBackend {
    /// 不再持有独立 client，统一走全局 client（共享代理配置）
    cached_ws:
        tokio::sync::Mutex<Option<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>>,
}

impl EdgeTtsBackend {
    pub fn new() -> Self {
        Self {
            cached_ws: tokio::sync::Mutex::new(None),
        }
    }

    fn generate_sec_ms_gec() -> String {
        const WIN_EPOCH: u64 = 11_644_473_600;
        const S_TO_100NS: u64 = 10_000_000;

        let unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut ticks = unix_secs + WIN_EPOCH;
        ticks -= ticks % 300;
        ticks *= S_TO_100NS;

        let str_to_hash = format!("{ticks}{TRUSTED_CLIENT_TOKEN}");
        let hash = Sha256::digest(str_to_hash.as_bytes());
        hash.iter().map(|b| format!("{:02X}", b)).collect::<String>()
    }

    fn generate_muid() -> String {
        Uuid::new_v4().simple().to_string().to_uppercase()
    }

    /// 生成 Javascript 风格的 GMT 时间戳（UTC），对应 Python 版 date_to_string()
    fn date_to_string() -> String {
        let now = Utc::now();
        format!(
            "{} GMT+0000 (Coordinated Universal Time)",
            now.format("%a %b %d %Y %H:%M:%S")
        )
    }

    fn build_ssml(text: &str, config: &TtsConfig) -> String {
        let voice = Self::resolve_voice(config);
        let rate_pct = ((config.rate - 1.0) * 100.0).round() as i32;
        let vol_pct = (config.volume * 100.0).round() as i32;
        let rate_str = format!("{:+}%", rate_pct);
        let vol_str = format!("{:+}%", vol_pct);
        // Emotion Prosody: pitch 以半音为单位,SSML 使用 "+Xst" / "-Xst" 格式
        let pitch_str = config
            .pitch
            .map(|p| {
                let pct = p.round() as i32;
                format!("pitch=\"{:+}st\" ", pct)
            })
            .unwrap_or_default();
        let escaped = escape_xml(text);
        // xml:lang 从 voice 名提取（如 "ja-JP-NanamiNeural" → "ja-JP"）
        let voice_lang = voice
            .split('-')
            .take(2)
            .collect::<Vec<_>>()
            .join("-");
        format!(
            "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" xml:lang=\"{}\">\
             <voice name=\"{}\">\
             <prosody {}rate=\"{}\" volume=\"{}\">\
             {}\
             </prosody>\
             </voice>\
             </speak>",
            voice_lang, voice, pitch_str, rate_str, vol_str, escaped
        )
    }

    /// 解析实际使用的 voice：根据 tts_language 校验 voice_id 语言匹配度
    ///
    /// 跨语言 TTS 场景（display_language=zh + tts_language=ja）下，用户配置的
    /// voice_id 通常是显示语言音色（如 zh-CN-XiaoxiaoNeural）。翻译后文本为
    /// 目标语言，若仍用原音色会出现音色/语言错位。此函数自动切换为目标语言默认音色。
    ///
    /// 同时校验配置的 voice 是否在 EDGE_VOICES 列表中存在，避免使用已下架/无效的
    /// 音色名导致 Edge 服务静默关闭连接（turn.start 后无音频返回）。
    fn resolve_voice(config: &TtsConfig) -> String {
        let configured = config.voice_id.as_deref().filter(|s| !s.is_empty());

        let target_lang = match config.tts_language.as_deref() {
            Some(l) if !l.is_empty() => l,
            _ => return configured.unwrap_or("zh-CN-XiaoxiaoNeural").to_string(),
        };

        let target_main = target_lang.split('-').next().unwrap_or(target_lang);

        if let Some(v) = configured {
            let voice_main = v.split('-').next().unwrap_or("");
            if voice_main == target_main && Self::is_valid_voice(v) {
                return v.to_string();
            }
        }

        let fallback = Self::default_voice_for_language(target_lang);
        tracing::info!(
            "[TTS] Edge voice 自动切换: configured={:?} tts_language={} -> {}",
            configured, target_lang, fallback
        );
        fallback.to_string()
    }

    /// 检查 voice 名称是否在 EDGE_VOICES 列表中
    fn is_valid_voice(voice: &str) -> bool {
        EDGE_VOICES.iter().any(|(id, _, _)| *id == voice)
    }

    /// 根据语言代码返回该语言的默认 EdgeTTS 音色
    fn default_voice_for_language(lang: &str) -> &'static str {
        let main = lang.split('-').next().unwrap_or(lang);
        match main {
            "ja" => "ja-JP-NanamiNeural",
            "en" => "en-US-AriaNeural",
            "zh" => "zh-CN-XiaoxiaoNeural",
            _ => "zh-CN-XiaoxiaoNeural",
        }
    }

    async fn connect(&self) -> VivianResult<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>> {
        let connection_id = Uuid::new_v4().simple().to_string();
        let sec_ms_gec = Self::generate_sec_ms_gec();
        let sec_ms_gec_version = format!("1-{CHROMIUM_FULL_VERSION}");
        let full_url = format!(
            "{EDGE_TTS_URL}?TrustedClientToken={TRUSTED_CLIENT_TOKEN}&ConnectionId={connection_id}&Sec-MS-GEC={sec_ms_gec}&Sec-MS-GEC-Version={sec_ms_gec_version}"
        );

        let uri: Uri = full_url.parse().map_err(|e| {
            VivianError::Speech(format!("解析 Edge-TTS URL 失败: {e}"))
        })?;

        let host = uri.host().unwrap_or("speech.platform.bing.com");

        let muid = Self::generate_muid();
        let user_agent = format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/{CHROMIUM_MAJOR_VERSION}.0.0.0 Safari/537.36 \
             Edg/{CHROMIUM_MAJOR_VERSION}.0.0.0"
        );

        let request = tokio_tungstenite::tungstenite::handshake::client::Request::builder()
            .method("GET")
            .uri(&full_url)
            .header("Host", host)
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Key", generate_key())
            .header("Sec-WebSocket-Version", "13")
            .header("Origin", EDGE_ORIGIN)
            .header("User-Agent", &user_agent)
            .header("Accept-Encoding", "gzip, deflate, br, zstd")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Pragma", "no-cache")
            .header("Cache-Control", "no-cache")
            .header("Cookie", format!("muid={muid};"))
            .body(())
            .map_err(|e| VivianError::Speech(format!("构建 WS 请求失败: {e}")))?;

        let (ws_stream, response) = tokio::time::timeout(
            Duration::from_secs(15),
            connect_async(request),
        )
        .await
        .map_err(|_| VivianError::Speech("Edge-TTS WSS 握手超时".to_string()))?
        .map_err(|e| VivianError::Speech(format!("Edge-TTS WSS 握手失败: {e}")))?;

        // 检查 403 响应，尝试时钟偏差校正
        if response.status().as_u16() == 403 {
            if let Some(server_date) = response.headers().get("Date") {
                if let Ok(date_str) = server_date.to_str() {
                    tracing::warn!("[TTS] Edge-TTS 返回 403, 服务器日期: {}", date_str);
                }
            }
        }

        Ok(ws_stream)
    }

    /// 发送 speech.config 消息（对应 Python 版 send_command_request）
    async fn send_config(&self, ws: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>) -> VivianResult<()> {
        let timestamp = Self::date_to_string();
        let config_msg = format!(
            "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{}\r\n",
            timestamp,
            json!({
                "context": {
                    "synthesis": {
                        "audio": {
                            "metadataoptions": {
                                "sentenceBoundaryEnabled": "false",
                                "wordBoundaryEnabled": "true"
                            },
                            "outputFormat": "audio-24khz-48kbitrate-mono-mp3"
                        }
                    }
                }
            })
        );
        ws.send(tokio_tungstenite::tungstenite::Message::Text(config_msg))
            .await
            .map_err(|e| VivianError::Speech(format!("发送 config 失败: {e}")))?;
        Ok(())
    }

    /// 发送 SSML（对应 Python 版 send_ssml_request / ssml_headers_plus_data）
    async fn send_ssml(
        &self,
        ws: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        ssml: &str,
    ) -> VivianResult<()> {
        let request_id = Uuid::new_v4().simple().to_string();
        let timestamp = Self::date_to_string();
        let ssml_msg = format!(
            "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}Z\r\nPath:ssml\r\n\r\n{}",
            request_id, timestamp, ssml
        );
        ws.send(tokio_tungstenite::tungstenite::Message::Text(ssml_msg))
            .await
            .map_err(|e| VivianError::Speech(format!("发送 SSML 失败: {e}")))?;
        Ok(())
    }

    /// 接收并解析响应:二进制音频 + 词边界（严格对应 Python 版 __stream 解析逻辑）
    async fn receive_response(
        &self,
        ws: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    ) -> VivianResult<(Vec<u8>, Vec<WordBoundary>)> {
        let mut audio_buf: Vec<u8> = Vec::new();
        let mut boundaries: Vec<WordBoundary> = Vec::new();
        let mut binary_count = 0u32;
        let mut text_count = 0u32;
        let mut audio_received = false;

        loop {
            let msg = tokio::time::timeout(Duration::from_secs(30), ws.next())
                .await
                .map_err(|_| VivianError::Speech("Edge-TTS 接收超时".to_string()))?
                .ok_or_else(|| VivianError::Speech("Edge-TTS 连接关闭".to_string()))?
                .map_err(|e| VivianError::Speech(format!("Edge-TTS 消息错误: {e}")))?;

            match msg {
                tokio_tungstenite::tungstenite::Message::Binary(bin) => {
                    binary_count += 1;
                    if bin.len() < 2 {
                        tracing::warn!("[TTS] Edge 二进制消息过短: {} 字节", bin.len());
                        continue;
                    }
                    // 前 2 字节是 header 长度（大端 u16）
                    let header_len = u16::from_be_bytes([bin[0], bin[1]]) as usize;
                    if header_len + 2 > bin.len() {
                        tracing::warn!("[TTS] Edge 二进制消息 header 长度异常: header_len={} bin_len={}", header_len, bin.len());
                        continue;
                    }
                    // 解析 header（键值对，\r\n 分隔）
                    let header_bytes = &bin[2..2 + header_len];
                    let header_str = String::from_utf8_lossy(header_bytes);
                    let mut path_is_audio = false;
                    let mut content_type: Option<String> = None;
                    for line in header_str.split("\r\n") {
                        if let Some((key, value)) = line.split_once(':') {
                            let key = key.trim();
                            let value = value.trim();
                            if key.eq_ignore_ascii_case("Path") {
                                path_is_audio = value.eq_ignore_ascii_case("audio");
                            } else if key.eq_ignore_ascii_case("Content-Type") {
                                content_type = Some(value.to_string());
                            }
                        }
                    }
                    let audio_data = &bin[2 + header_len..];
                    // 终止消息：Content-Type 为空且无音频数据，跳过
                    if content_type.is_none() && audio_data.is_empty() {
                        continue;
                    }
                    if !path_is_audio {
                        tracing::warn!("[TTS] Edge 二进制消息 Path 不是 audio: {}", header_str.lines().next().unwrap_or(""));
                        continue;
                    }
                    if !audio_data.is_empty() {
                        audio_received = true;
                        audio_buf.extend_from_slice(audio_data);
                    }
                }
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    text_count += 1;
                    tracing::debug!("[TTS] Edge 文本消息 #{}: {}", text_count, text);
                    // 解析 Path
                    let mut path = "";
                    for line in text.split("\r\n") {
                        if let Some((key, value)) = line.split_once(':') {
                            if key.trim().eq_ignore_ascii_case("Path") {
                                path = value.trim();
                                break;
                            }
                        }
                    }
                    match path {
                        "turn.end" => break,
                        "response" | "turn.start" => {}
                        "audio.metadata" => {
                            if let Some(wb_list) = parse_audio_metadata(&text) {
                                boundaries.extend(wb_list);
                            }
                        }
                        _ => {
                            tracing::warn!("[TTS] Edge 未知文本消息路径: {}", path);
                        }
                    }
                }
                tokio_tungstenite::tungstenite::Message::Close(_) => break,
                _ => {}
            }
        }

        tracing::info!(
            "[TTS] Edge 接收完成: binary={} text={} audio={}字节 boundaries={}",
            binary_count, text_count, audio_buf.len(), boundaries.len()
        );

        if !audio_received {
            return Err(VivianError::Speech(
                "Edge-TTS 未收到音频数据，请检查语音名称和网络连接".to_string(),
            ));
        }

        Ok((audio_buf, boundaries))
    }

    async fn fetch_voices_from_api(client: &reqwest::Client) -> VivianResult<Vec<VoiceInfo>> {
        let url = format!(
            "https://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1/voices/list?TrustedClientToken={TRUSTED_CLIENT_TOKEN}"
        );

        let response = client
            .get(&url)
            .header("Origin", EDGE_ORIGIN)
            .header("User-Agent", format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/{CHROMIUM_MAJOR_VERSION}.0.0.0 Safari/537.36 \
                 Edg/{CHROMIUM_MAJOR_VERSION}.0.0.0"
            ))
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("获取语音列表失败: {e}")))?;

        if !response.status().is_success() {
            return Err(VivianError::Speech(format!(
                "获取语音列表返回错误状态: {}",
                response.status()
            )));
        }

        let voices: Vec<serde_json::Value> = response.json()
            .await
            .map_err(|e| VivianError::Speech(format!("解析语音列表失败: {e}")))?;

        let mut result = Vec::new();
        for voice in voices {
            let id = voice.get("ShortName").and_then(|v| v.as_str()).ok_or_else(
                || VivianError::Speech("语音列表缺少 ShortName 字段".to_string())
            )?.to_string();
            
            let name = voice.get("LocalName").and_then(|v| v.as_str())
                .or_else(|| voice.get("DisplayName").and_then(|v| v.as_str()))
                .unwrap_or(&id)
                .to_string();
            
            let language = voice.get("Locale").and_then(|v| v.as_str()).ok_or_else(
                || VivianError::Speech("语音列表缺少 Locale 字段".to_string())
            )?.to_string();

            if language.starts_with("zh-CN") && !language.starts_with("zh-CN-") {
                result.push(VoiceInfo { id, name, language });
            } else if language == "en-US" || language == "ja-JP" {
                result.push(VoiceInfo { id, name, language });
            }
        }

        result.sort_by(|a, b| {
            a.language.cmp(&b.language).then(a.name.cmp(&b.name))
        });

        tracing::info!("[TTS] 从 API 获取到 {} 个语音", result.len());
        Ok(result)
    }
}

#[async_trait]
impl TtsBackend for EdgeTtsBackend {
    fn name(&self) -> &'static str {
        "edge-tts"
    }

    fn supports_word_boundary(&self) -> bool {
        true
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        if text.trim().is_empty() {
            return Ok(TtsSynthesisResult::new(Vec::new(), AudioFormat::Mp3));
        }

        let ssml = Self::build_ssml(text, config);

        // 尝试复用缓存的 WSS 连接（prewarm 建立的）
        let ws = {
            let mut guard = self.cached_ws.lock().await;
            guard.take()
        };

        // 无缓存连接则新建 + 发送 config
        let mut ws = match ws {
            Some(w) => {
                tracing::info!("[TTS] Edge 复用缓存 WSS 连接");
                w
            }
            None => {
                tracing::debug!("[TTS] Edge 新建 WSS 连接");
                let mut w = self.connect().await?;
                self.send_config(&mut w).await?;
                w
            }
        };

        self.send_ssml(&mut ws, &ssml).await?;
        let (audio, boundaries) = self.receive_response(&mut ws).await?;

        if audio.is_empty() {
            return Err(VivianError::Speech(
                "Edge-TTS 返回空音频".to_string(),
            ));
        }

        Ok(TtsSynthesisResult::new(audio, AudioFormat::Mp3).with_boundaries(boundaries))
    }

    /// 预热 WSS 连接：提前建立并发送 speech.config，缓存供后续 synthesize 复用
    async fn prewarm(&self, _config: &TtsConfig) -> VivianResult<()> {
        let mut guard = self.cached_ws.lock().await;
        if guard.is_some() {
            tracing::debug!("[TTS] Edge prewarm: 已有缓存连接，跳过");
            return Ok(());
        }
        tracing::info!("[TTS] Edge prewarm: 建立 WSS 连接");
        let mut ws = self.connect().await?;
        self.send_config(&mut ws).await?;
        *guard = Some(ws);
        tracing::info!("[TTS] Edge prewarm: WSS 连接已缓存");
        Ok(())
    }

    async fn list_voices(&self, _config: &TtsConfig) -> VivianResult<Vec<VoiceInfo>> {
        let client = crate::network::http_client::get_global_client();
        match EdgeTtsBackend::fetch_voices_from_api(&client).await {
            Ok(voices) => Ok(voices),
            Err(e) => {
                tracing::warn!("[TTS] 从 API 获取语音列表失败，使用内置列表: {}", e);
                Ok(EDGE_VOICES
                    .iter()
                    .map(|(id, name, lang)| VoiceInfo {
                        id: id.to_string(),
                        name: name.to_string(),
                        language: lang.to_string(),
                    })
                    .collect())
            }
        }
    }

    async fn health_check(&self, _config: &TtsConfig) -> bool {
        crate::network::http_client::get_global_client()
            .head("https://speech.platform.bing.com")
            .send()
            .await
            .is_ok()
    }
}

/// 解析 audio.metadata 消息中的 WordBoundary 列表
///
/// Python 版在 __parse_metadata 中从 Metadata 数组提取，
/// 每个元素的 Type 为 "WordBoundary"，Data 中包含 Offset/Duration/text.Text
fn parse_audio_metadata(text: &str) -> Option<Vec<WordBoundary>> {
    let body = text.split("\r\n\r\n").nth(1)?;
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let metadata_arr = json.get("Metadata")?.as_array()?;
    let mut results = Vec::new();
    for meta in metadata_arr {
        let meta_type = meta.get("Type")?.as_str()?;
        if meta_type != "WordBoundary" {
            continue;
        }
        let data = meta.get("Data")?;
        let text_val = data.get("text")?.get("Text")?.as_str()?.to_string();
        let offset = data.get("Offset")?.as_u64()?;
        let duration = data.get("Duration")?.as_u64()?;
        results.push(WordBoundary {
            text: text_val,
            offset_ms: offset / 10_000,
            duration_ms: duration / 10_000,
        });
    }
    Some(results)
}

/// XML 转义（双引号属性 + 元素内容场景）
fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
