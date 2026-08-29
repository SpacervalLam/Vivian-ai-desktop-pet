//! 豆包（火山引擎）TTS 后端
//!
//! 对齐官方 API:
//! - HTTP 一次性合成: https://www.volcengine.com/docs/6561/79820
//! - WebSocket 流式合成: https://www.volcengine.com/docs/6561/79821
//! - 参数说明: https://www.volcengine.com/docs/6561/79823
//! - 音色列表: https://www.volcengine.com/docs/6561/97465
//!
//! ## 端点
//! - HTTP:  https://openspeech.bytedance.com/api/v1/tts
//! - WS:    wss://openspeech.bytedance.com/api/v1/tts/ws_binary
//!
//! ## 认证
//! - Header: `Authorization: Bearer;{token}`(注意分号分隔,非空格)
//! - JSON body: `app.appid` / `app.token` / `app.cluster`
//!
//! ## 请求结构
//! ```json
//! {
//!   "app": {"appid":"...", "token":"access_token", "cluster":"volcano_tts"},
//!   "user": {"uid":"..."},
//!   "audio": {
//!     "voice_type":"BV700_streaming",
//!     "encoding":"mp3", "rate":24000,
//!     "speed_ratio":1.0, "volume_ratio":1.0, "pitch_ratio":1.0,
//!     "emotion":"happy", "language":"cn"
//!   },
//!   "request": {
//!     "reqid":"uuid", "text":"...", "text_type":"plain",
//!     "operation":"query"  // query=非流式, submit=流式
//!   }
//! }
//! ```
//!
//! ## pitch_ratio 说明
//! 豆包的 pitch_ratio 是倍率(0.1-3.0,1.0=正常),与 Edge/Azure 的半音偏移不同。
//! 这里将 config.pitch(半音)转换为倍率: ratio = 2^(semitones/12)。
//!
//! Vivian 集成策略:
//! - 默认使用 HTTP 一次性合成(与 TtsManager 的 synthesize→play 架构一致)
//! - 支持 WebSocket 流式合成(通过 synthesize_stream)
//! - emotion 字符串直接透传,由 EmotionMapper 统一映射

use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};

use crate::error::{VivianError, VivianResult};

use super::tts::TtsConfig;
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult};

/// HTTP 一次性合成端点
const HTTP_URL: &str = "https://openspeech.bytedance.com/api/v1/tts";

pub struct DoubaoBackend;

impl DoubaoBackend {
    pub fn new() -> Self {
        Self
    }

    /// 从配置解析 appid
    fn appid(config: &TtsConfig) -> VivianResult<&str> {
        config
            .doubao_appid
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("豆包 appid 未配置".to_string()))
    }

    /// 从配置解析 access_token
    fn access_token(config: &TtsConfig) -> VivianResult<&str> {
        config
            .doubao_access_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("豆包 access_token 未配置".to_string()))
    }

    /// 从配置解析 cluster(默认 volcano_tts)
    fn cluster(config: &TtsConfig) -> String {
        config
            .doubao_cluster
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "volcano_tts".to_string())
    }

    /// 从配置解析 voice_type
    fn voice_type(config: &TtsConfig) -> VivianResult<&str> {
        config
            .doubao_voice_type
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("豆包 voice_type 未配置".to_string()))
    }

    /// 解析音频格式(默认 mp3)
    fn format(config: &TtsConfig) -> (String, AudioFormat) {
        let fmt = config
            .doubao_format
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("mp3");
        let audio_fmt = match fmt {
            "wav" => AudioFormat::Wav,
            "pcm" => AudioFormat::Pcm,
            "ogg_opus" => AudioFormat::Ogg,
            _ => AudioFormat::Mp3,
        };
        (fmt.to_string(), audio_fmt)
    }

    /// 解析采样率(默认 24000)
    fn sample_rate(config: &TtsConfig) -> u32 {
        config.doubao_sample_rate.unwrap_or(24000)
    }

    /// 将 config.pitch(半音偏移)转换为豆包 pitch_ratio(倍率)
    ///
    /// 豆包 pitch_ratio 范围 [0.1, 3.0],1.0 为正常音高。
    /// 半音 → 倍率: ratio = 2^(semitones / 12)
    ///   +12 半音 → 2.0(高一个八度)
    ///   -12 半音 → 0.5(低一个八度)
    ///   0 半音 → 1.0
    fn pitch_ratio(config: &TtsConfig) -> f64 {
        let semitones = config.pitch.unwrap_or(0.0);
        let ratio = 2.0_f64.powf(semitones / 12.0);
        ratio.clamp(0.1, 3.0)
    }

    /// 构建 audio 配置块
    fn build_audio_config(config: &TtsConfig, emotion: Option<&str>) -> Value {
        let (format_str, _) = Self::format(config);
        let voice_type = Self::voice_type(config).unwrap_or("BV700_V2_streaming");
        let mut audio = json!({
            "voice_type": voice_type,
            "encoding": format_str,
            "rate": Self::sample_rate(config),
            "speed_ratio": config.rate,
            "volume_ratio": config.volume,
            "pitch_ratio": Self::pitch_ratio(config),
            "language": "cn",
        });

        // emotion 透传(豆包支持 happy/sad/angry/等,依赖音色)
        if let Some(em) = emotion.filter(|s| !s.is_empty()) {
            let mapped = map_emotion(em);
            if !mapped.is_empty() {
                audio["emotion"] = json!(mapped);
            }
        }

        audio
    }

    /// 构建 HTTP 请求 body
    fn build_http_body(config: &TtsConfig, text: &str, emotion: Option<&str>) -> VivianResult<Value> {
        let appid = Self::appid(config)?;
        let token = Self::access_token(config)?;
        let cluster = Self::cluster(config);

        Ok(json!({
            "app": {
                "appid": appid,
                "token": token,
                "cluster": cluster,
            },
            "user": {
                "uid": "vivian-rs",
            },
            "audio": Self::build_audio_config(config, emotion),
            "request": {
                "reqid": uuid::Uuid::new_v4().to_string(),
                "text": text,
                "text_type": "plain",
                "operation": "query",
                "with_frontend": 1,
                "frontend_type": "unitTson",
            },
        }))
    }
}

#[async_trait]
impl TtsBackend for DoubaoBackend {
    fn name(&self) -> &'static str {
        "doubao"
    }

    fn supports_word_boundary(&self) -> bool {
        true
    }

    fn supports_streaming(&self) -> bool {
        // WebSocket 流式合成待实现(需处理豆包二进制协议)
        false
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        if text.trim().is_empty() {
            let (_, fmt) = Self::format(config);
            return Ok(TtsSynthesisResult::new(Vec::new(), fmt));
        }

        let access_token = Self::access_token(config)?.to_string();
        // emotion 由 TtsManager 通过 with_emotion_prosody 设置到 current_emotion
        let emotion = config.current_emotion.as_deref();
        let body = Self::build_http_body(config, text, emotion)?;
        let (_, audio_format) = Self::format(config);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| VivianError::Speech(format!("创建 HTTP 客户端失败: {e}")))?;

        let resp = client
            .post(HTTP_URL)
            .header("Authorization", format!("Bearer;{access_token}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("豆包 HTTP 请求失败: {e}")))?;

        let status = resp.status();
        let resp_json: Value = resp
            .json()
            .await
            .map_err(|e| VivianError::Speech(format!("豆包响应解析失败: {e}")))?;

        if !status.is_success() {
            return Err(VivianError::Speech(format!(
                "豆包 HTTP 错误: status={} body={}",
                status,
                resp_json
            )));
        }

        // 检查业务码
        let code = resp_json
            .get("code")
            .and_then(|c| c.as_i64())
            .unwrap_or(-1);
        if code != 3000 {
            let message = resp_json
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("未知错误");
            return Err(VivianError::Speech(format!(
                "豆包合成失败: code={} message={}",
                code, message
            )));
        }

        // 提取音频数据(base64)
        let audio_b64 = resp_json
            .get("data")
            .and_then(|d| d.as_str())
            .ok_or_else(|| VivianError::Speech("豆包响应缺少 data 字段".to_string()))?;

        let audio = general_purpose::STANDARD
            .decode(audio_b64)
            .map_err(|e| VivianError::Speech(format!("豆包音频 base64 解码失败: {e}")))?;

        if audio.is_empty() {
            return Err(VivianError::Speech("豆包返回空音频".to_string()));
        }

        // 提取词边界(用于唇形同步)
        let boundaries = parse_word_boundaries(&resp_json);

        Ok(TtsSynthesisResult::new(audio, audio_format).with_boundaries(boundaries))
    }

    async fn health_check(&self, config: &TtsConfig) -> bool {
        Self::appid(config).is_ok() && Self::access_token(config).is_ok()
    }
}

/// 解析响应中的词边界时间戳(addition.frontend)
///
/// 豆包返回的 frontend 是 JSON 字符串,包含 words 和 phonemes 数组。
/// 只提取 words 用于唇形同步。
fn parse_word_boundaries(resp: &Value) -> Vec<super::tts_backend::WordBoundary> {
    let frontend_str = match resp
        .get("addition")
        .and_then(|a| a.get("frontend"))
        .and_then(|f| f.as_str())
    {
        Some(s) => s,
        None => return Vec::new(),
    };

    let frontend: Value = match serde_json::from_str(frontend_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let words = match frontend.get("words").and_then(|w| w.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    words
        .iter()
        .map(|w| super::tts_backend::WordBoundary {
            text: w
                .get("word")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            offset_ms: (w
                .get("start_time")
                .and_then(|t| t.as_f64())
                .unwrap_or(0.0)
                * 1000.0) as u64,
            duration_ms: ((w
                .get("end_time")
                .and_then(|t| t.as_f64())
                .unwrap_or(0.0)
                - w.get("start_time")
                    .and_then(|t| t.as_f64())
                    .unwrap_or(0.0))
                * 1000.0) as u64,
        })
        .collect()
}

/// Emotion 字符串映射
///
/// 豆包 V2 音色(如 BV700_V2_streaming)支持的官方 emotion 值:
/// pleased(愉悦)/sorry(抱歉)/annoyed(嗔怪)/happy(开心)/sad(悲伤)/angry(愤怒)/
/// scare(害怕)/hate(厌恶)/surprise(惊讶)/tear(哭腔)/customer_service(客服)/
/// professional(专业)/serious(严肃)/conniving(绿茶)/comfort(安慰鼓励)/
/// lovey-dovey(撒娇)/tsundere(傲娇)/charming(娇媚)/radio(情感电台)/yoga(瑜伽)/
/// storytelling(讲故事)/chat(自然对话)/narrator(旁白-舒缓)/narrator_immersive(旁白-沉浸)/
/// novel_dialog(平和)/advertising(广告)/assistant(助手)/energetic(可爱元气)
///
/// 将 Vivian 内部 emotion 映射到豆包官方英文 emotion 字符串。
fn map_emotion(emotion: &str) -> String {
    match emotion.to_lowercase().as_str() {
        "happy" | "开心" | "愉悦" | "pleased" => "happy".to_string(),
        "sad" | "悲伤" | "难过" => "sad".to_string(),
        "angry" | "愤怒" | "生气" => "angry".to_string(),
        "surprised" | "惊讶" | "惊奇" | "surprise" => "surprise".to_string(),
        "fearful" | "害怕" | "恐惧" | "scare" | "fear" => "scare".to_string(),
        "disgusted" | "厌恶" | "嫌弃" | "hate" => "hate".to_string(),
        "crying" | "哭腔" | "哭泣" | "tear" => "tear".to_string(),
        "shy" | "害羞" | "娇媚" | "charming" => "charming".to_string(),
        "proud" | "傲娇" | "tsundere" => "tsundere".to_string(),
        "gentle" | "温柔" | "安慰" | "安慰鼓励" | "comfort" => "comfort".to_string(),
        "love" | "撒娇" | "lovey-dovey" | "affectionate" => "lovey-dovey".to_string(),
        "professional" | "专业" => "professional".to_string(),
        "serious" | "严肃" => "serious".to_string(),
        "apologetic" | "抱歉" | "道歉" | "sorry" => "sorry".to_string(),
        "annoyed" | "嗔怪" => "annoyed".to_string(),
        "storytelling" | "讲故事" => "storytelling".to_string(),
        "customer_service" | "客服" => "customer_service".to_string(),
        "conniving" | "绿茶" => "conniving".to_string(),
        "radio" | "情感电台" => "radio".to_string(),
        "yoga" => "yoga".to_string(),
        "chat" | "自然对话" => "chat".to_string(),
        "narrator" | "旁白" => "narrator".to_string(),
        "advertising" | "广告" => "advertising".to_string(),
        "assistant" | "助手" => "assistant".to_string(),
        "energetic" | "元气" | "可爱" => "energetic".to_string(),
        "neutral" | "平静" | "平和" | "通用" | "novel_dialog" => "".to_string(),
        other => other.to_string(),
    }
}
