//! Azure 认知服务 TTS 后端
//!
//! 完整对齐 Azure TTS REST API 官方文档:
//! - 端点: `https://{region}.tts.speech.microsoft.com/cognitiveservices/v1`
//! - 语音列表: `GET https://{region}.tts.speech.microsoft.com/cognitiveservices/voices/list`
//! - 认证: `Ocp-Apim-Subscription-Key: {key}`(推荐) 或 `Authorization: Bearer {token}`
//! - Content-Type: `application/ssml+xml`
//! - X-Microsoft-OutputFormat: 控制音频格式(wav/mp3/ogg/webm)
//!
//! SSML 支持完整特性:
//! - `xmlns:mstts='https://www.w3.org/2001/mstts'` 命名空间
//! - `<mstts:express-as style='...' styledegree='...' role='...'>` 情感表达
//! - `<prosody rate='...' pitch='...' volume='...'>` 韵律控制
//! - 动态 xml:lang 从 voice 短名推断(如 zh-CN-XiaoxiaoNeural → zh-CN)
//!
//! 参考文档:
//! - https://learn.microsoft.com/azure/ai-services/speech-service/rest-text-to-speech
//! - https://learn.microsoft.com/azure/ai-services/speech-service/speech-synthesis-markup
//! - https://learn.microsoft.com/azure/ai-services/speech-service/speech-synthesis-markup-voice

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::{VivianError, VivianResult};

use super::tts::{TtsConfig, VoiceInfo};
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult};

/// Azure /voices/list 返回的语音项(完整字段)
///
/// 参考官方响应示例:
/// ```json
/// {
///   "Name": "Microsoft Server Speech Text to Speech Voice (zh-CN, XiaoxiaoNeural)",
///   "DisplayName": "Xiaoxiao",
///   "LocalName": "晓晓",
///   "ShortName": "zh-CN-XiaoxiaoNeural",
///   "Gender": "Female",
///   "Locale": "zh-CN",
///   "LocaleName": "Chinese (Mandarin, Simplified)",
///   "StyleList": ["assistant", "chat", "cheerful", "sad", ...],
///   "RolePlayList": ["YoungAdultFemale", "YoungAdultMale", ...],
///   "SampleRateHertz": "24000",
///   "VoiceType": "Neural",
///   "Status": "GA",
///   "WordsPerMinute": "342",
///   "SecondaryLocaleList": [{"Locale": "en-US", "LocaleName": "English (United States)"}]
/// }
/// ```
#[derive(Debug, Deserialize)]
struct AzureVoiceItem {
    /// 短名(如 zh-CN-XiaoxiaoNeural),作为 voice id
    #[serde(rename = "ShortName")]
    short_name: String,
    /// 显示名(如 Xiaoxiao)
    #[serde(rename = "DisplayName")]
    #[serde(default)]
    display_name: String,
    /// 本地化名(如 晓晓)
    #[serde(rename = "LocalName")]
    #[serde(default)]
    local_name: String,
    /// 语言区域(如 zh-CN)
    #[serde(rename = "Locale")]
    locale: String,
    /// 性别(Female/Male)
    #[serde(rename = "Gender")]
    #[serde(default)]
    gender: String,
    /// 支持的风格列表(如 ["cheerful", "sad", "excited"])
    #[serde(rename = "StyleList", default)]
    style_list: Vec<String>,
    /// 支持的角色扮演列表(如 ["YoungAdultFemale", "OlderAdultMale"])
    #[serde(rename = "RolePlayList", default)]
    role_play_list: Vec<String>,
    /// 采样率(如 "24000")
    #[serde(rename = "SampleRateHertz", default)]
    sample_rate_hertz: String,
    /// 语音类型(如 "Neural", "NeuralHD", "DragonHD")
    #[serde(rename = "VoiceType", default)]
    voice_type: String,
    /// 每分钟字数(用于估算输出时长)
    #[serde(rename = "WordsPerMinute", default)]
    words_per_minute: String,
}

pub struct AzureTtsBackend {
    client: reqwest::Client,
}

impl AzureTtsBackend {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// 从 voice 短名推断 xml:lang(如 "zh-CN-XiaoxiaoNeural" → "zh-CN")
    ///
    /// Azure voice 短名格式: `{locale}-{VoiceName}`,locale 为 `xx-XX` 形式
    /// 若解析失败,回退到 "en-US"
    fn infer_lang_from_voice(voice: &str) -> String {
        // 取前两段:xx-XX
        let parts: Vec<&str> = voice.splitn(3, '-').collect();
        if parts.len() >= 2 {
            format!("{}-{}", parts[0], parts[1])
        } else {
            "en-US".to_string()
        }
    }

    /// 转义 SSML 文本中的 XML 特殊字符
    fn escape_xml(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\'', "&apos;")
            .replace('"', "&quot;")
    }

    /// 构建 SSML 文档
    ///
    /// 完整结构:
    /// ```xml
    /// <speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis'
    ///        xmlns:mstts='https://www.w3.org/2001/mstts' xml:lang='{locale}'>
    ///   <voice name='{voice}'>
    ///     [<mstts:express-as style='{style}' styledegree='{degree}' role='{role}'>]
    ///       <prosody rate='{rate}%' pitch='{pitch}%' volume='{vol}%'>
    ///         {text}
    ///       </prosody>
    ///     [</mstts:express-as>]
    ///   </voice>
    /// </speak>
    /// ```
    ///
    /// - style 为空时不输出 express-as 元素
    /// - styledegree 范围 0.5-2.0(默认 1.0)
    /// - rate/pitch 为相对百分比(0% 表示正常)
    /// - volume 为百分比(0-100)
    fn build_ssml(text: &str, config: &TtsConfig) -> String {
        let voice = config
            .voice_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("zh-CN-XiaoxiaoNeural");
        let lang = Self::infer_lang_from_voice(voice);
        let rate_pct = ((config.rate - 1.0) * 100.0).round() as i32;
        let vol_pct = (config.volume * 100.0).round().clamp(0.0, 100.0) as i32;
        // pitch: 优先 azure_pitch(百分比格式,向后兼容),其次 config.pitch(半音格式)
        // Azure SSML 同时支持 "+X%" 和 "+Xst" 两种格式
        let pitch_attr = if let Some(ap) = config.azure_pitch {
            format!("pitch='{:+}%'", ap.round() as i32)
        } else if let Some(p) = config.pitch {
            format!("pitch='{:+}st'", p.round() as i32)
        } else {
            "pitch='+0%'".to_string()
        };
        let escaped = Self::escape_xml(text);

        // 可选的 mstts:express-as 元素
        let style = config.azure_style.as_deref().unwrap_or("").trim();
        let style_degree = config
            .azure_style_degree
            .map(|d| format!("{:.1}", d))
            .unwrap_or_else(|| "1.0".to_string());
        let role = config.azure_role.as_deref().unwrap_or("").trim();

        let (open_express, close_express) = if !style.is_empty() {
            // 仅当 style 非空时才输出 express-as
            // role 可选,为空时不输出 role 属性
            let role_attr = if !role.is_empty() {
                format!(" role='{}'", Self::escape_xml(role))
            } else {
                String::new()
            };
            (
                format!(
                    "<mstts:express-as style='{}' styledegree='{}'{}>",
                    Self::escape_xml(style),
                    style_degree,
                    role_attr
                ),
                "</mstts:express-as>".to_string(),
            )
        } else {
            (String::new(), String::new())
        };

        format!(
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' \
             xmlns:mstts='https://www.w3.org/2001/mstts' xml:lang='{}'>\
             <voice name='{}'>\
             {}\
             <prosody rate='{}%' {} volume='{}%'>\
             {}\
             </prosody>\
             {}\
             </voice>\
             </speak>",
            lang, voice, open_express, rate_pct, pitch_attr, vol_pct, escaped, close_express
        )
    }

    /// TTS 合成端点(区域端点格式)
    ///
    /// 格式: `https://{region}.tts.speech.microsoft.com/cognitiveservices/v1`
    fn tts_endpoint(region: &str) -> String {
        format!(
            "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
            region
        )
    }

    /// 语音列表端点
    ///
    /// 格式: `https://{region}.tts.speech.microsoft.com/cognitiveservices/voices/list`
    fn voices_endpoint(region: &str) -> String {
        format!(
            "https://{}.tts.speech.microsoft.com/cognitiveservices/voices/list",
            region
        )
    }

    /// 解析用户配置的输出格式字符串为 (X-Microsoft-OutputFormat 值, AudioFormat 枚举)
    ///
    /// 支持的格式(参考官方文档):
    /// - `riff-24khz-16bit-mono-pcm` → Wav (24kHz, 推荐)
    /// - `riff-16khz-16bit-mono-pcm` → Wav (16kHz, 低采样率)
    /// - `riff-48khz-16bit-mono-pcm` → Wav (48kHz, HD)
    /// - `audio-24khz-48kbitrate-mono-mp3` → Mp3 (24kHz 48kbps, 默认)
    /// - `audio-24khz-160kbitrate-mono-mp3` → Mp3 (24kHz 160kbps, 高码率)
    /// - `audio-48khz-192kbitrate-mono-mp3` → Mp3 (48kHz 192kbps, HD)
    /// - `ogg-24khz-16bit-mono-opus` → Ogg
    /// - `webm-24khz-16bit-mono-opus` → Ogg (WebM 容器)
    fn parse_output_format(config: &TtsConfig) -> (&'static str, AudioFormat) {
        let fmt = config.azure_output_format.as_deref().unwrap_or("").trim();
        match fmt {
            // WAV 格式
            "riff-24khz-16bit-mono-pcm" | "wav" | "wav-24khz" => {
                ("riff-24khz-16bit-mono-pcm", AudioFormat::Wav)
            }
            "riff-16khz-16bit-mono-pcm" | "wav-16khz" => {
                ("riff-16khz-16bit-mono-pcm", AudioFormat::Wav)
            }
            "riff-48khz-16bit-mono-pcm" | "wav-48khz" => {
                ("riff-48khz-16bit-mono-pcm", AudioFormat::Wav)
            }
            // MP3 格式
            "audio-24khz-48kbitrate-mono-mp3" | "mp3" | "mp3-24khz-48k" | "" => {
                // 默认格式
                ("audio-24khz-48kbitrate-mono-mp3", AudioFormat::Mp3)
            }
            "audio-24khz-160kbitrate-mono-mp3" | "mp3-24khz-160k" => {
                ("audio-24khz-160kbitrate-mono-mp3", AudioFormat::Mp3)
            }
            "audio-48khz-192kbitrate-mono-mp3" | "mp3-48khz-192k" => {
                ("audio-48khz-192kbitrate-mono-mp3", AudioFormat::Mp3)
            }
            // OGG/WebM 格式
            "ogg-24khz-16bit-mono-opus" | "ogg" => {
                ("ogg-24khz-16bit-mono-opus", AudioFormat::Ogg)
            }
            "webm-24khz-16bit-mono-opus" | "webm" => {
                ("webm-24khz-16bit-mono-opus", AudioFormat::Ogg)
            }
            // PCM 裸数据(无 WAV 头)
            "raw-24khz-16bit-mono-pcm" | "pcm" => {
                ("raw-24khz-16bit-mono-pcm", AudioFormat::Pcm)
            }
            // 未知格式: 回退到默认 MP3
            other => {
                tracing::warn!(
                    "[TTS] Azure 未知的输出格式 '{}',回退到默认 audio-24khz-48kbitrate-mono-mp3",
                    other
                );
                ("audio-24khz-48kbitrate-mono-mp3", AudioFormat::Mp3)
            }
        }
    }
}

#[async_trait]
impl TtsBackend for AzureTtsBackend {
    fn name(&self) -> &'static str {
        "azure"
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        let key = config.azure_key.as_deref().unwrap_or("");
        let region = config.azure_region.as_deref().unwrap_or("eastus");
        if key.is_empty() {
            return Err(VivianError::Speech(
                "Azure TTS 未配置 API Key".to_string(),
            ));
        }

        let ssml = Self::build_ssml(text, config);
        let url = Self::tts_endpoint(region);
        let (output_format, audio_format) = Self::parse_output_format(config);

        tracing::debug!(
            "[TTS] Azure 请求: region={}, voice={}, style={}, format={}",
            region,
            config.voice_id.as_deref().unwrap_or("zh-CN-XiaoxiaoNeural"),
            config.azure_style.as_deref().unwrap_or(""),
            output_format
        );

        let resp = self
            .client
            .post(&url)
            .header("Ocp-Apim-Subscription-Key", key)
            .header("Content-Type", "application/ssml+xml")
            .header("X-Microsoft-OutputFormat", output_format)
            .header("User-Agent", "VivianDesktopPet/1.0")
            .body(ssml)
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("Azure TTS 请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // 401 Unauthorized: API Key 无效或 token 过期
            // 403 Forbidden: 区域不匹配或配额耗尽
            // 429 Too Many Requests: 限流
            let hint = match status.as_u16() {
                401 => " (检查 API Key 是否正确)",
                403 => " (检查区域是否匹配,或配额是否耗尽)",
                429 => " (请求被限流,请稍后重试)",
                _ => "",
            };
            return Err(VivianError::Speech(format!(
                "Azure TTS 失败 [{}]{}: {}",
                status, hint, body
            )));
        }

        let audio = resp
            .bytes()
            .await
            .map_err(|e| VivianError::Speech(format!("读取 Azure 音频失败: {e}")))?
            .to_vec();

        Ok(TtsSynthesisResult::new(audio, audio_format))
    }

    async fn list_voices(&self, config: &TtsConfig) -> VivianResult<Vec<VoiceInfo>> {
        let key = config.azure_key.as_deref().unwrap_or("");
        let region = config.azure_region.as_deref().unwrap_or("eastus");

        // 未配置 API Key 时返回空列表(避免触发 401)
        if key.is_empty() {
            return Ok(Vec::new());
        }

        let url = Self::voices_endpoint(region);
        let resp = self
            .client
            .get(&url)
            .header("Ocp-Apim-Subscription-Key", key)
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("Azure /voices/list 请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!("[TTS] Azure /voices/list 失败 [{}]: {}", status, body);
            return Ok(Vec::new());
        }

        let items: Vec<AzureVoiceItem> = resp
            .json()
            .await
            .map_err(|e| VivianError::Speech(format!("解析 Azure 语音列表失败: {e}")))?;

        // 优先显示 LocalName(如"晓晓"),fallback DisplayName
        // 在 name 中附加 StyleList 和 VoiceType 信息,方便用户选择
        Ok(items
            .into_iter()
            .map(|v| {
                let base_name = if !v.local_name.is_empty() {
                    v.local_name.clone()
                } else {
                    v.display_name.clone()
                };

                let mut name = format!("{} ({})", base_name, v.gender);

                // 附加 VoiceType(如 Neural / NeuralHD / DragonHD)
                if !v.voice_type.is_empty() {
                    name = format!("{} [{}]", name, v.voice_type);
                }

                // 附加支持的 style 数量(如果有)
                if !v.style_list.is_empty() {
                    name = format!("{} {{{} styles}}", name, v.style_list.len());
                }

                // 附加 RolePlay 数量(如果有,表示该 voice 支持角色扮演)
                if !v.role_play_list.is_empty() {
                    name = format!("{} <{} roles>", name, v.role_play_list.len());
                }

                // 附加采样率(非 24kHz 时显式标注)
                if !v.sample_rate_hertz.is_empty() && v.sample_rate_hertz != "24000" {
                    name = format!("{} {}Hz", name, v.sample_rate_hertz);
                }

                // 附加每分钟字数(用于估算输出时长)
                if !v.words_per_minute.is_empty() {
                    name = format!("{} {}wpm", name, v.words_per_minute);
                }

                VoiceInfo {
                    id: v.short_name,
                    name,
                    language: v.locale,
                }
            })
            .collect())
    }

    /// 健康检查:实际调用 /voices/list 验证 API Key 和区域是否有效
    ///
    /// 返回 true 表示 API Key 和区域配置正确,服务可用
    async fn health_check(&self, config: &TtsConfig) -> bool {
        let key = config.azure_key.as_deref().unwrap_or("");
        let region = config.azure_region.as_deref().unwrap_or("eastus");
        if key.is_empty() {
            return false;
        }

        let url = Self::voices_endpoint(region);
        match self
            .client
            .get(&url)
            .header("Ocp-Apim-Subscription-Key", key)
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                tracing::debug!("[TTS] Azure health_check 失败: {}", e);
                false
            }
        }
    }
}
