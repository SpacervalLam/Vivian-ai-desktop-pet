//! Fish Speech TTS 后端
//!
//! 对齐真实 API: https://github.com/fishaudio/fish-speech
//!
//! ## 端点
//! - POST /v1/tts          — 文本转语音
//! - GET  /v1/health       — 健康检查
//! - GET  /v1/references/list — 列出本地参考音频 ID
//!
//! ## 请求格式
//! - Content-Type: application/json（也支持 msgpack，这里用 json 简化）
//! - 鉴权: Authorization: Bearer <token>（云端必需；本地部署若 --api-key 未设置则可省略）
//!
//! ## 字段（严格对齐 ServeTTSRequest schema）
//! - text: 必需
//! - chunk_length: 100-1000，默认 200
//! - format: wav/pcm/mp3/opus
//! - latency: normal/balanced
//! - references: [{audio: base64, text: str}] — in-context learning 参考音频
//! - reference_id: str — 引用预上传的参考音频目录名（与 references 二选一）
//! - normalize: bool — 文本归一化（数字稳定性）
//! - streaming: bool — 流式返回（仅 wav）
//! - seed: int — 随机种子
//!
//! ## 不存在的字段（修复臆想）
//! - prosody.speed：schema 中不存在，移除
//! - model：服务器启动时决定，无 per-request model 字段
//!
//! ## 音色指定方式（二选一）
//! 1. reference_id：服务器端 references/<id>/ 目录下的预上传参考音频
//!    - 通过 /v1/references/add 上传
//!    - 通过 /v1/references/list 列出
//! 2. references：直接在请求中传 base64 音频 + 文本（in-context learning）
//!    - 适合零样本克隆（无需预上传）
//!
//! Vivian 集成策略：优先使用 reference_id（性能更好，服务端有缓存）
//! 若用户配置了 fish_speech_ref_audio 本地路径，则读取并 base64 编码后通过 references 字段传递

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::fs;

use crate::error::{VivianError, VivianResult};

use super::tts::{TtsConfig, VoiceInfo};
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult};

/// Fish Speech 云端默认端点
const FISH_CLOUD_URL: &str = "https://api.fish.audio";

/// /v1/references/list 响应
#[derive(Debug, Deserialize)]
struct ListReferencesResponse {
    success: bool,
    #[serde(default)]
    reference_ids: Vec<String>,
    #[serde(default)]
    message: String,
}

pub struct FishSpeechBackend {
    client: reqwest::Client,
}

impl FishSpeechBackend {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// 解析服务地址：未配置时默认使用云端 api.fish.audio
    fn base_url(config: &TtsConfig) -> String {
        let configured = config
            .fish_speech_url
            .as_deref()
            .filter(|s| !s.is_empty());
        match configured {
            Some(u) => u.trim_end_matches('/').to_string(),
            None => FISH_CLOUD_URL.to_string(),
        }
    }

    /// 是否为云端（api.fish.audio）
    fn is_cloud(base: &str) -> bool {
        base.eq_ignore_ascii_case(FISH_CLOUD_URL)
    }

    /// 从 Content-Type 推断音频格式
    fn format_from_content_type(content_type: &str) -> AudioFormat {
        if content_type.contains("wav") {
            AudioFormat::Wav
        } else if content_type.contains("mpeg") || content_type.contains("mp3") {
            AudioFormat::Mp3
        } else if content_type.contains("pcm") {
            AudioFormat::Pcm
        } else if content_type.contains("ogg") || content_type.contains("opus") {
            AudioFormat::Ogg
        } else {
            // Fish Speech 默认输出 wav
            AudioFormat::Wav
        }
    }

    /// 读取本地参考音频文件并 base64 编码
    /// 用于零样本克隆（in-context learning）
    async fn read_ref_audio_base64(path: &str) -> VivianResult<String> {
        if !Path::new(path).exists() {
            return Err(VivianError::Speech(format!(
                "参考音频文件不存在: {path}"
            )));
        }
        let bytes = fs::read(path)
            .await
            .map_err(|e| VivianError::Speech(format!("读取参考音频失败: {e}")))?;
        Ok(base64_encode(&bytes))
    }
}

/// 简单的 base64 编码（避免引入新依赖）
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[async_trait]
impl TtsBackend for FishSpeechBackend {
    fn name(&self) -> &'static str {
        "fish-speech"
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        if text.trim().is_empty() {
            return Ok(TtsSynthesisResult::new(Vec::new(), AudioFormat::Wav));
        }

        let base = Self::base_url(config);
        let url = format!("{}/v1/tts", base);

        // 云端必须鉴权；本地部署无 --api-key 时允许空 token
        let token = config.fish_speech_key.as_deref().unwrap_or("");
        if Self::is_cloud(&base) && token.is_empty() {
            return Err(VivianError::Speech(
                "Fish Speech 云端服务未配置 API Key".to_string(),
            ));
        }

        // 音频格式：默认 wav（Fish Speech 服务器原生格式，无编码开销）
        let format_str = config
            .fish_speech_format
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("wav");

        // chunk_length: schema 限制 100-1000，默认 200
        // 不暴露给用户配置，使用默认值
        let chunk_length: u32 = 200;

        // 构建请求体：严格对齐 ServeTTSRequest schema
        let mut body = json!({
            "text": text,
            "format": format_str,
            "latency": "normal",
            "normalize": true,
            "chunk_length": chunk_length,
            "streaming": false,
        });

        // 音色指定策略（二选一）：
        // 1. reference_id（优先）：服务器端预上传的参考音频目录名
        // 2. references（零样本）：本地音频文件 base64 编码后传入
        let reference_id = config
            .fish_speech_character
            .as_deref()
            .filter(|s| !s.is_empty());

        if let Some(id) = reference_id {
            body["reference_id"] = json!(id);
        } else if let Some(ref_audio_path) = config.fish_speech_ref_audio.as_deref() {
            if !ref_audio_path.is_empty() {
                // 零样本克隆：读取本地参考音频，base64 编码后通过 references 字段传递
                let audio_b64 = Self::read_ref_audio_base64(ref_audio_path).await?;
                let ref_text = config
                    .fish_speech_ref_text
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("");
                body["references"] = json!([{
                    "audio": audio_b64,
                    "text": ref_text,
                }]);
            }
        }

        // 构建请求
        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body);

        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("Fish Speech 请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(VivianError::Speech(format!(
                "Fish Speech 失败 [{}]: {}",
                status, body
            )));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let audio = resp
            .bytes()
            .await
            .map_err(|e| VivianError::Speech(format!("读取 Fish Speech 音频失败: {e}")))?
            .to_vec();

        let format = Self::format_from_content_type(&content_type);

        Ok(TtsSynthesisResult::new(audio, format))
    }

    async fn list_voices(&self, config: &TtsConfig) -> VivianResult<Vec<VoiceInfo>> {
        // 调用 /v1/references/list 拉取服务器端预上传的参考音频 ID
        let base = Self::base_url(config);
        let url = format!("{}/v1/references/list", base);
        let token = config.fish_speech_key.as_deref().unwrap_or("");

        let mut req = self.client.get(&url);
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[TTS] Fish Speech /v1/references/list 请求失败: {e}");
                return Ok(Vec::new());
            }
        };

        if !resp.status().is_success() {
            tracing::warn!(
                "[TTS] Fish Speech /v1/references/list 失败 [{}]",
                resp.status()
            );
            return Ok(Vec::new());
        }

        let list_resp: ListReferencesResponse = match resp.json().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[TTS] Fish Speech /v1/references/list 解析失败: {e}");
                return Ok(Vec::new());
            }
        };

        if !list_resp.success {
            tracing::warn!(
                "[TTS] Fish Speech /v1/references/list 返回失败: {}",
                list_resp.message
            );
            return Ok(Vec::new());
        }

        Ok(list_resp
            .reference_ids
            .into_iter()
            .map(|id| VoiceInfo {
                id: id.clone(),
                name: id,
                language: "zh".to_string(),
            })
            .collect())
    }

    async fn health_check(&self, config: &TtsConfig) -> bool {
        let base = Self::base_url(config);
        let url = format!("{}/v1/health", base);
        let token = config.fish_speech_key.as_deref().unwrap_or("");

        let mut req = self.client.get(&url);
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        match req.send().await {
            Ok(r) => r.status().is_success(),
            Err(_) => false,
        }
    }
}
