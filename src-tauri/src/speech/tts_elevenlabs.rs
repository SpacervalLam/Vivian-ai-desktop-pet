//! ElevenLabs TTS 后端（云端，多语种 + 音色库/克隆）
//!
//! ## 端点
//! - 纯音频：`POST {base}/v1/text-to-speech/{voice_id}` → 二进制音频
//! - 带时间轴：`POST {base}/v1/text-to-speech/{voice_id}/with-timestamps`
//!   → JSON（`audio_base64` + 字符级时间轴）
//! - 音色列表：`GET {base}/v1/voices`
//!
//! ## 鉴权
//! - 请求头 `xi-api-key: <api_key>`
//!
//! ## 音色
//! `voice_id` 在控制台 Voices / VoiceLab 页获取（预置音色或克隆音色均可）。
//! 与 GPT-SoVITS 的参考音频克隆不同，ElevenLabs 的音色是服务端资源，只需 ID。
//!
//! ## 韵律
//! - `voice_settings.speed` 范围为 0.7-1.2，与 `TtsConfig.rate`(0.5-3.0) 标度不同，需裁剪。
//! - `stability` / `similarity_boost` / `style` / `use_speaker_boost` 直接透传。
//!
//! ## 唇形同步
//! 优先走 `/with-timestamps`：一次请求同时拿到音频与字符级时间轴，换算成词/字级
//! `WordBoundary`。该端点不可用（网络异常、模型不支持、账户权限）时自动回退到纯音频
//! 端点，此时没有词边界，口型回落到音素映射。
//!
//! ## 代理
//! 复用全局 HTTP 客户端（跟随应用内代理配置）。ElevenLabs 为海外服务，
//! 国内网络通常需要配置代理，或把 `elevenlabs_endpoint` 指向自建反代。

use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;
use serde_json::{json, Value};

use crate::error::{VivianError, VivianResult};

use super::tts::{TtsConfig, VoiceInfo};
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult, WordBoundary};

/// 官方入口（可通过 `elevenlabs_endpoint` 覆盖为反代地址）
const DEFAULT_BASE_URL: &str = "https://api.elevenlabs.io";
/// 默认模型：多语种 v2，质量最稳，中文表现好
const DEFAULT_MODEL: &str = "eleven_multilingual_v2";
/// 默认输出格式：44.1kHz / 128kbps MP3（rodio 可直接从内存解码）
const DEFAULT_OUTPUT_FORMAT: &str = "mp3_44100_128";

/// `/with-timestamps` 熔断标记
///
/// 该端点不可用（账户权限/模型不支持）时置位，后续句子直接走纯音频端点。
/// 否则每句都要先失败一次再重试，白付一次往返延迟与额度。
static TIMESTAMPS_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

pub struct ElevenLabsBackend;

impl ElevenLabsBackend {
    pub fn new() -> Self {
        Self
    }

    /// 从配置解析 API Key
    fn api_key(config: &TtsConfig) -> VivianResult<&str> {
        config
            .elevenlabs_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("ElevenLabs API Key 未配置".to_string()))
    }

    /// 从配置解析音色 ID
    fn voice_id(config: &TtsConfig) -> VivianResult<&str> {
        let id = config
            .elevenlabs_voice_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                VivianError::Speech(
                    "ElevenLabs 音色 ID 未配置（控制台 Voices 页复制 voice_id）".to_string(),
                )
            })?;
        // 音色 ID 会拼进 URL path，限制字符集避免路径注入
        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(VivianError::Speech(format!(
                "ElevenLabs 音色 ID 非法: {id}"
            )));
        }
        Ok(id)
    }

    /// 解析服务入口（留空用官方地址，去掉结尾斜杠）
    fn base_url(config: &TtsConfig) -> String {
        config
            .elevenlabs_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
    }

    /// 解析模型
    fn model(config: &TtsConfig) -> String {
        config
            .elevenlabs_model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_MODEL)
            .to_string()
    }

    /// 解析输出格式，并映射到本地播放器认识的音频类型
    fn output_format(config: &TtsConfig) -> (String, AudioFormat) {
        let fmt = config
            .elevenlabs_output_format
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_OUTPUT_FORMAT)
            .to_string();
        let audio_format = if fmt.starts_with("pcm") || fmt.starts_with("ulaw") {
            AudioFormat::Pcm
        } else if fmt.starts_with("opus") || fmt.starts_with("ogg") {
            AudioFormat::Ogg
        } else {
            AudioFormat::Mp3
        };
        (fmt, audio_format)
    }

    /// 构建 `voice_settings`
    ///
    /// speed 由全局语速倍率裁剪而来（ElevenLabs 只接受 0.7-1.2）。
    fn voice_settings(config: &TtsConfig) -> Value {
        json!({
            "stability": config.elevenlabs_stability.unwrap_or(0.5).clamp(0.0, 1.0),
            "similarity_boost": config
                .elevenlabs_similarity_boost
                .unwrap_or(0.75)
                .clamp(0.0, 1.0),
            "style": config.elevenlabs_style.unwrap_or(0.0).clamp(0.0, 1.0),
            "use_speaker_boost": config.elevenlabs_speaker_boost.unwrap_or(true),
            "speed": config.rate.clamp(0.7, 1.2),
        })
    }

    /// 构建请求体
    ///
    /// `language_code` 只有 turbo / flash 系列模型接受，其他模型传了会报错，
    /// 因此按模型名判断后再附加。
    fn request_body(config: &TtsConfig, text: &str) -> Value {
        let model = Self::model(config);
        let mut body = json!({
            "text": text,
            "model_id": model,
            "voice_settings": Self::voice_settings(config),
        });

        let supports_language_code = model.contains("turbo") || model.contains("flash");
        if supports_language_code {
            let lang = config
                .tts_language
                .as_deref()
                .filter(|s| !s.is_empty())
                .or_else(|| config.display_language.as_deref().filter(|s| !s.is_empty()));
            if let Some(lang) = lang {
                body["language_code"] = json!(lang);
            }
        }

        body
    }

    /// 请求纯音频端点，返回二进制音频
    async fn request_audio(
        config: &TtsConfig,
        api_key: &str,
        voice_id: &str,
        text: &str,
    ) -> VivianResult<TtsSynthesisResult> {
        let (output_format, audio_format) = Self::output_format(config);
        let url = format!(
            "{}/v1/text-to-speech/{}",
            Self::base_url(config),
            voice_id
        );
        let accept = if matches!(audio_format, AudioFormat::Mp3) {
            "audio/mpeg"
        } else {
            "application/octet-stream"
        };

        let client = crate::network::http_client::get_global_client();
        let resp = client
            .post(&url)
            .query(&[("output_format", output_format.as_str())])
            .header("xi-api-key", api_key)
            .header("Accept", accept)
            .json(&Self::request_body(config, text))
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("ElevenLabs 合成请求失败: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let preview = error_preview(resp).await;
            return Err(VivianError::Speech(format!(
                "ElevenLabs 合成失败 ({status}): {preview}"
            )));
        }

        let audio = resp
            .bytes()
            .await
            .map_err(|e| VivianError::Speech(format!("读取 ElevenLabs 音频失败: {e}")))?
            .to_vec();
        if audio.is_empty() {
            return Err(VivianError::Speech("ElevenLabs 返回空音频".to_string()));
        }

        Ok(TtsSynthesisResult::new(audio, audio_format))
    }

    /// 请求带时间轴端点，返回音频 + 词/字级边界
    async fn request_with_timestamps(
        config: &TtsConfig,
        api_key: &str,
        voice_id: &str,
        text: &str,
    ) -> VivianResult<TtsSynthesisResult> {
        let (output_format, audio_format) = Self::output_format(config);
        let url = format!(
            "{}/v1/text-to-speech/{}/with-timestamps",
            Self::base_url(config),
            voice_id
        );

        let client = crate::network::http_client::get_global_client();
        let resp = client
            .post(&url)
            .query(&[("output_format", output_format.as_str())])
            .header("xi-api-key", api_key)
            .header("Accept", "application/json")
            .json(&Self::request_body(config, text))
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("ElevenLabs 请求失败: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let preview = error_preview(resp).await;
            return Err(VivianError::Speech(format!(
                "ElevenLabs /with-timestamps 失败 ({status}): {preview}"
            )));
        }

        let data: Value = resp
            .json()
            .await
            .map_err(|e| VivianError::Speech(format!("ElevenLabs 响应解析失败: {e}")))?;

        let audio_b64 = data
            .get("audio_base64")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                VivianError::Speech("ElevenLabs 响应缺少 audio_base64".to_string())
            })?;
        let audio = base64::engine::general_purpose::STANDARD
            .decode(audio_b64.trim())
            .map_err(|e| VivianError::Speech(format!("ElevenLabs 音频解码失败: {e}")))?;
        if audio.is_empty() {
            return Err(VivianError::Speech("ElevenLabs 返回空音频".to_string()));
        }

        let boundaries = parse_alignment(&data);
        Ok(TtsSynthesisResult::new(audio, audio_format).with_boundaries(boundaries))
    }
}

impl Default for ElevenLabsBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TtsBackend for ElevenLabsBackend {
    fn name(&self) -> &'static str {
        "elevenlabs"
    }

    fn requires_network(&self) -> bool {
        true
    }

    fn supports_word_boundary(&self) -> bool {
        true
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        let text = text.trim();
        if text.is_empty() {
            let (_, audio_format) = Self::output_format(config);
            return Ok(TtsSynthesisResult::new(Vec::new(), audio_format));
        }

        let api_key = Self::api_key(config)?;
        let voice_id = Self::voice_id(config)?;

        // 1) 优先带时间轴：一次请求同时拿到音频与口型数据
        if !TIMESTAMPS_UNAVAILABLE.load(Ordering::Relaxed) {
            match Self::request_with_timestamps(config, api_key, voice_id, text).await {
                Ok(result) if !result.audio.is_empty() => return Ok(result),
                Ok(_) => tracing::warn!(
                    "[TTS] ElevenLabs /with-timestamps 返回空音频，回退纯音频端点"
                ),
                Err(e) => {
                    tracing::warn!(
                        "[TTS] ElevenLabs /with-timestamps 不可用（{e}），本次及后续句子改用纯音频端点"
                    );
                    TIMESTAMPS_UNAVAILABLE.store(true, Ordering::Relaxed);
                }
            }
        }

        // 2) 回退纯音频端点（无词边界，口型走音素映射）
        Self::request_audio(config, api_key, voice_id, text).await
    }

    async fn list_voices(&self, config: &TtsConfig) -> VivianResult<Vec<VoiceInfo>> {
        let api_key = match config.elevenlabs_key.as_deref().map(str::trim) {
            Some(key) if !key.is_empty() => key,
            // 未配置 Key 时返回空列表，避免无意义的 401
            _ => return Ok(Vec::new()),
        };

        let url = format!("{}/v1/voices", Self::base_url(config));
        let client = crate::network::http_client::get_global_client();
        let resp = client
            .get(&url)
            .header("xi-api-key", api_key)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[TTS] ElevenLabs /v1/voices 请求失败: {e}");
                return Ok(Vec::new());
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            tracing::warn!(
                "[TTS] ElevenLabs /v1/voices 失败 [{status}]: {}",
                error_preview(resp).await
            );
            return Ok(Vec::new());
        }

        let data: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[TTS] 解析 ElevenLabs 音色列表失败: {e}");
                return Ok(Vec::new());
            }
        };

        let voices = data
            .get("voices")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let id = v.get("voice_id").and_then(|x| x.as_str())?;
                        let name = v
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or(id)
                            .to_string();
                        let category = v
                            .get("category")
                            .and_then(|x| x.as_str())
                            .unwrap_or("premade");
                        // labels.language 存在时（如 "zh"）用于展示，缺失则标记 multi
                        let language = v
                            .get("labels")
                            .and_then(|l| l.get("language"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("multi");
                        Some(VoiceInfo {
                            id: id.to_string(),
                            name: format!("{name} [{category}]"),
                            language: language.to_string(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(voices)
    }

    async fn health_check(&self, config: &TtsConfig) -> bool {
        let api_key = match Self::api_key(config) {
            Ok(k) => k,
            Err(_) => return false,
        };
        let url = format!("{}/v1/voices", Self::base_url(config));
        let client = crate::network::http_client::get_global_client();
        match client.get(&url).header("xi-api-key", api_key).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                tracing::debug!("[TTS] ElevenLabs health_check 失败: {e}");
                false
            }
        }
    }
}

/// 读取错误响应体前 300 字符，用于日志与报错提示
async fn error_preview(resp: reqwest::Response) -> String {
    resp.text()
        .await
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}

/// 从 `/with-timestamps` 响应解析词边界
///
/// 优先 `normalized_alignment`（按实际朗读文本归一化，如数字被念成读法），
/// 缺失时回退 `alignment`（原始字符时间轴）。
///
/// 分组规则：
/// - 拉丁字母/数字连续字母串合成一个词（`hello`）
/// - 空格与标点只作切分，不产生边界
/// - 中日韩等非拉丁字符逐字成段（唇形同步按字驱动）
fn parse_alignment(data: &Value) -> Vec<WordBoundary> {
    let alignment = data
        .get("normalized_alignment")
        .filter(|v| !v.is_null())
        .or_else(|| data.get("alignment"));
    let alignment = match alignment {
        Some(a) => a,
        None => return Vec::new(),
    };

    let chars = match alignment.get("characters").and_then(|v| v.as_array()) {
        Some(c) => c,
        None => return Vec::new(),
    };
    let starts = match alignment
        .get("character_start_times_seconds")
        .and_then(|v| v.as_array())
    {
        Some(s) => s,
        None => return Vec::new(),
    };
    let ends = match alignment
        .get("character_end_times_seconds")
        .and_then(|v| v.as_array())
    {
        Some(e) => e,
        None => return Vec::new(),
    };

    let count = chars.len().min(starts.len()).min(ends.len());
    let mut words: Vec<WordBoundary> = Vec::new();
    let mut buf = String::new();
    let mut buf_start = 0.0_f64;
    let mut buf_end = 0.0_f64;

    for i in 0..count {
        let piece = chars[i].as_str().unwrap_or("");
        let start = starts[i].as_f64().unwrap_or(0.0);
        let end = ends[i].as_f64().unwrap_or(start);

        for ch in piece.chars() {
            if ch.is_whitespace() || is_word_separator(ch) {
                flush_word(&mut words, &mut buf, buf_start, buf_end);
                continue;
            }
            if ch.is_ascii_alphanumeric() || ch == '\'' {
                if buf.is_empty() {
                    buf_start = start;
                }
                buf.push(ch);
                buf_end = end;
            } else {
                // 非拉丁字符（CJK 等）逐字成段
                flush_word(&mut words, &mut buf, buf_start, buf_end);
                words.push(WordBoundary {
                    text: ch.to_string(),
                    offset_ms: secs_to_ms(start),
                    duration_ms: secs_to_ms(end - start),
                });
            }
        }
    }
    flush_word(&mut words, &mut buf, buf_start, buf_end);
    words
}

/// 把累积的拉丁词写入边界列表
fn flush_word(words: &mut Vec<WordBoundary>, buf: &mut String, start: f64, end: f64) {
    if buf.is_empty() {
        return;
    }
    words.push(WordBoundary {
        text: std::mem::take(buf),
        offset_ms: secs_to_ms(start),
        duration_ms: secs_to_ms(end - start),
    });
}

fn secs_to_ms(secs: f64) -> u64 {
    // round 而非截断：0.2s 在 f64 里是 0.19999…，截断会把每段时长都少 1ms，
    // 句子越长口型漂移越明显
    if secs.is_finite() && secs > 0.0 {
        (secs * 1000.0).round() as u64
    } else {
        0
    }
}

/// 词分隔标点（不产生边界，仅切分）
fn is_word_separator(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.' | '!' | '?' | ';' | ':' | '-' | '—' | '"' | ')' | '(' | '[' | ']'
            | '、' | '。' | '，' | '！' | '？' | '；' | '：' | '「' | '」' | '『' | '』'
            | '“' | '”' | '‘' | '’' | '《' | '》' | '…' | '·'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_groups_latin_words_and_splits_cjk() {
        let data = json!({
            "normalized_alignment": {
                "characters": ["好", "的", " ", "h", "i", " ", "你", "好"],
                "character_start_times_seconds": [0.0, 0.2, 0.4, 0.5, 0.6, 0.7, 0.8, 1.0],
                "character_end_times_seconds": [0.2, 0.4, 0.5, 0.6, 0.7, 0.8, 1.0, 1.2]
            }
        });
        let words = parse_alignment(&data);
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, vec!["好", "的", "hi", "你", "好"]);
        assert_eq!(words[2].offset_ms, 500);
        assert_eq!(words[2].duration_ms, 200);
    }

    #[test]
    fn alignment_without_timeline_returns_empty() {
        let data = json!({ "audio_base64": "AAA" });
        assert!(parse_alignment(&data).is_empty());
    }

    #[test]
    fn voice_settings_clamp_speed_into_supported_range() {
        let mut config = TtsConfig::default();
        config.rate = 3.0;
        assert_eq!(ElevenLabsBackend::voice_settings(&config)["speed"], json!(1.2));

        config.rate = 0.4;
        assert_eq!(ElevenLabsBackend::voice_settings(&config)["speed"], json!(0.7));
    }

    #[test]
    fn language_code_only_sent_for_turbo_and_flash() {
        let mut config = TtsConfig::default();
        config.tts_language = Some("zh".to_string());

        config.elevenlabs_model = Some("eleven_multilingual_v2".to_string());
        assert!(ElevenLabsBackend::request_body(&config, "你好")
            .get("language_code")
            .is_none());

        config.elevenlabs_model = Some("eleven_turbo_v2_5".to_string());
        assert_eq!(
            ElevenLabsBackend::request_body(&config, "你好")["language_code"],
            json!("zh")
        );
    }

    #[test]
    fn voice_id_rejects_path_injection() {
        let mut config = TtsConfig::default();
        config.elevenlabs_voice_id = Some("../user".to_string());
        assert!(ElevenLabsBackend::voice_id(&config).is_err());
    }
}
