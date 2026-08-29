//! 小米 MiMo TTS 后端（语音克隆）
//!
//! 官方接口：POST https://api.xiaomimimo.com/v1/chat/completions
//!
//! ## 鉴权
//! - `api-key: <api_key>` 请求头
//!
//! ## 协议
//! - messages: 风格提示（可选，role=user）+ 待合成文本（role=assistant）
//! - `audio.voice`：克隆音频的 data URL（base64 内联）
//! - 响应 `choices[0].message.audio.data` 为 base64 WAV
//!
//! ## 模型
//! - mimo-v2.5-tts-voiceclone（默认）
//!
//! 每次合成需携带克隆音频文件（读文件 → base64 内联），无固定音色 ID。

use base64::Engine;
use serde_json::json;

use crate::error::{VivianError, VivianResult};

use super::tts::TtsConfig;
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult};

/// MiMo TTS 默认端点
const DEFAULT_ENDPOINT: &str = "https://api.xiaomimimo.com/v1/chat/completions";
/// MiMo TTS 默认模型（语音克隆）
const DEFAULT_MODEL: &str = "mimo-v2.5-tts-voiceclone";

pub struct MimoBackend;

impl MimoBackend {
    pub fn new() -> Self {
        Self
    }

    /// 从配置解析 API Key
    fn api_key(config: &TtsConfig) -> VivianResult<&str> {
        config
            .mimo_key
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("MiMo API Key 未配置".to_string()))
    }

    /// 从配置解析克隆音频路径
    fn voice_audio_path(config: &TtsConfig) -> VivianResult<&str> {
        config
            .mimo_voice_audio_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("MiMo 克隆音频未配置".to_string()))
    }

    /// 解析端点（默认官方入口）
    fn endpoint(config: &TtsConfig) -> String {
        config
            .mimo_endpoint
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
    }

    /// 解析模型（默认克隆模型）
    fn model(config: &TtsConfig) -> String {
        config
            .mimo_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())
    }

    /// 读取克隆音频并编码为 data URL
    fn build_voice_data_url(path: &str) -> VivianResult<String> {
        let audio = std::fs::read(path)
            .map_err(|e| VivianError::Speech(format!("读取 MiMo 克隆音频失败: {e}")))?;
        if audio.is_empty() {
            return Err(VivianError::Speech("MiMo 克隆音频为空".to_string()));
        }
        let mime = match path.rsplit('.').next().map(|s| s.to_lowercase()).as_deref() {
            Some("wav") => "audio/wav",
            Some("m4a") | Some("mp4") => "audio/mp4",
            Some("ogg") => "audio/ogg",
            Some("flac") => "audio/flac",
            _ => "audio/mpeg",
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&audio);
        Ok(format!("data:{mime};base64,{b64}"))
    }
}

impl Default for MimoBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TtsBackend for MimoBackend {
    fn name(&self) -> &'static str {
        "mimo"
    }

    fn requires_network(&self) -> bool {
        true
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        let api_key = Self::api_key(config)?;
        let voice_path = Self::voice_audio_path(config)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(VivianError::Speech("合成文本为空".to_string()));
        }

        let voice = Self::build_voice_data_url(voice_path)?;

        // messages：风格提示（可选）+ 待合成文本；克隆音频走 audio.voice
        let mut messages = Vec::new();
        if let Some(style) = config
            .mimo_style_prompt
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            messages.push(json!({ "role": "user", "content": style.trim() }));
        }
        messages.push(json!({ "role": "assistant", "content": text }));

        let body = json!({
            "model": Self::model(config),
            "messages": messages,
            "audio": {
                "format": "wav",
                "voice": voice,
            },
        });

        let client = crate::network::http_client::get_global_client();
        let response = client
            .post(Self::endpoint(config))
            .header("Content-Type", "application/json")
            .header("api-key", api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("MiMo TTS 请求失败: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let preview: String = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            return Err(VivianError::Speech(format!(
                "MiMo TTS 合成失败 ({status}): {preview}"
            )));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| VivianError::Speech(format!("MiMo TTS 响应解析失败: {e}")))?;

        let b64 = data["choices"][0]["message"]["audio"]["data"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| VivianError::Speech("MiMo TTS 响应缺少音频数据".to_string()))?;

        let audio = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| VivianError::Speech(format!("MiMo TTS 音频解码失败: {e}")))?;
        if audio.is_empty() {
            return Err(VivianError::Speech("MiMo TTS 返回空音频".to_string()));
        }

        Ok(TtsSynthesisResult::new(audio, AudioFormat::Wav))
    }

    async fn synthesize_stream(
        &self,
        text: &str,
        config: &TtsConfig,
        _callback: super::tts_backend::StreamCallback,
    ) -> VivianResult<()> {
        // MiMo 为一次性合成接口，流式路径回退到批量合成
        let _ = self.synthesize(text, config).await?;
        Ok(())
    }
}
