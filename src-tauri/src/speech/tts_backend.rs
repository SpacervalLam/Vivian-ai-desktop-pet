//! TTS 后端抽象 trait 与公共类型
//!
//! 所有 TTS 后端实现 `TtsBackend` trait，由 `TtsManager` 统一调度。
//! 后端分为两类:
//! - 流式合成(返回逐句音频 + WordBoundary): EdgeTts / Windows
//! - 批量合成(返回完整音频): Azure / GptSoVits / FishSpeech / MiniMax
//!
//! Phase 2 新增:
//! - `synthesize_stream`: 流式合成接口(默认 fallback 到批量)
//! - `prewarm`: 预热连接(LLM 首 token 时调用,提前建立 WSS/HTTP 连接)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::VivianResult;

use super::tts::{TtsConfig, VoiceInfo};

/// 音频格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Mp3,
    Wav,
    Pcm,
    Ogg,
    Aac,
}

/// 词边界信息(用于音素级唇形同步)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordBoundary {
    /// 该词的文本
    pub text: String,
    /// 相对于音频开始的偏移(毫秒)
    pub offset_ms: u64,
    /// 持续时间(毫秒)
    pub duration_ms: u64,
}

/// 合成结果
#[derive(Debug, Clone)]
pub struct TtsSynthesisResult {
    /// 音频字节
    pub audio: Vec<u8>,
    /// 音频格式
    pub format: AudioFormat,
    /// 词边界(可选,用于唇形同步)
    pub word_boundaries: Vec<WordBoundary>,
}

impl TtsSynthesisResult {
    pub fn new(audio: Vec<u8>, format: AudioFormat) -> Self {
        Self {
            audio,
            format,
            word_boundaries: Vec::new(),
        }
    }

    pub fn with_boundaries(mut self, boundaries: Vec<WordBoundary>) -> Self {
        self.word_boundaries = boundaries;
        self
    }
}

/// 流式合成产生的音频块
#[derive(Debug, Clone)]
pub enum AudioChunk {
    /// PCM/MP3 音频数据块(可立即播放)
    Audio {
        data: Vec<u8>,
        format: AudioFormat,
    },
    /// 词边界事件(用于唇形同步)
    WordBoundary(WordBoundary),
    /// 合成结束
    End,
}

/// 流式合成的回调
pub type StreamCallback = Box<dyn Fn(AudioChunk) + Send + Sync>;

/// TTS 后端 trait
#[async_trait]
pub trait TtsBackend: Send + Sync {
    /// 后端名称
    fn name(&self) -> &'static str;

    /// 是否支持词边界(WordBoundary)事件
    fn supports_word_boundary(&self) -> bool {
        false
    }

    /// 是否需要网络连接
    fn requires_network(&self) -> bool {
        true
    }

    /// 是否支持流式合成(PCM 级别)
    ///
    /// 默认 false,表示该后端只支持批量合成。
    /// Edge-TTS 等流式后端应覆盖为 true。
    fn supports_streaming(&self) -> bool {
        false
    }

    /// 合成文本为音频(批量)
    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult>;

    /// 流式合成文本为音频
    ///
    /// 默认实现:fallback 到批量合成,将完整音频作为单个 AudioChunk 返回。
    /// 支持流式的后端(如 Edge-TTS)应覆盖此方法,逐块产出音频。
    async fn synthesize_stream(
        &self,
        text: &str,
        config: &TtsConfig,
        on_chunk: StreamCallback,
    ) -> VivianResult<()> {
        let result = self.synthesize(text, config).await?;
        on_chunk(AudioChunk::Audio {
            data: result.audio,
            format: result.format,
        });
        for wb in result.word_boundaries {
            on_chunk(AudioChunk::WordBoundary(wb));
        }
        on_chunk(AudioChunk::End);
        Ok(())
    }

    /// 预热连接
    ///
    /// 在 LLM 首 token 到达时调用,提前建立 WSS/HTTP 连接。
    /// LLM 结束后调用 synthesize 时可直接复用已建立的连接,省去连接建立时间。
    ///
    /// 默认实现:空操作(不支持预热的后端忽略)。
    async fn prewarm(&self, _config: &TtsConfig) -> VivianResult<()> {
        Ok(())
    }

    /// 列出可用语音
    async fn list_voices(&self, config: &TtsConfig) -> VivianResult<Vec<VoiceInfo>> {
        let _ = config;
        Ok(Vec::new())
    }

    /// 健康检查(用于 fallback 决策)
    async fn health_check(&self, config: &TtsConfig) -> bool {
        let _ = config;
        true
    }
}

/// 根据配置创建后端实例
pub fn create_backend(engine: &super::tts::TtsEngine) -> VivianResult<Box<dyn TtsBackend>> {
    use super::tts::TtsEngine;
    Ok(match engine {
        TtsEngine::None => {
            return Err(crate::error::VivianError::Speech(
                "TTS 引擎为 None,无法创建后端".to_string(),
            ));
        }
        TtsEngine::Windows => Box::new(super::tts_windows::WindowsTtsBackend::new()),
        TtsEngine::EdgeTts => Box::new(super::tts_edge::EdgeTtsBackend::new()),
        TtsEngine::Azure => Box::new(super::tts_azure::AzureTtsBackend::new()),
        TtsEngine::GptSoVits => Box::new(super::tts_gpt_sovits::GptSoVitsBackend::new()),
        TtsEngine::FishSpeech => Box::new(super::tts_fish_speech::FishSpeechBackend::new()),
        // BertVits2 不应在工厂中出现:resolve() 已将其映射为 FishSpeech
        // 此分支仅为穷尽匹配保护,实际不会到达
        TtsEngine::BertVits2 => Box::new(super::tts_fish_speech::FishSpeechBackend::new()),
        TtsEngine::MiniMax => Box::new(super::tts_minimax::MiniMaxBackend::new()),
        TtsEngine::Doubao => Box::new(super::tts_doubao::DoubaoBackend::new()),
        TtsEngine::Mimo => Box::new(super::tts_mimo::MimoBackend::new()),
    })
}

/// 简单音素映射:将文本字符映射到 Live2D 嘴形开合度
///
/// 中文元音/英文元音 → 较大开合;辅音 → 较小开合;标点/空格 → 闭合
pub fn char_to_mouth_open(ch: char) -> f32 {
    match ch.to_ascii_lowercase() {
        'a' | 'e' | 'i' | 'o' | 'u' | 'y' => 0.7,
        '啊' | '哦' | '额' | '衣' | '乌' | '鱼' | '诶' | '奥' | '爱' | '安' => 0.75,
        c if c.is_ascii_alphabetic() => 0.35,
        c if c.is_ascii_punctuation() || c.is_whitespace() => 0.0,
        _ => 0.5,
    }
}

/// 从词边界文本推断嘴形开合度(取首字符)
pub fn word_to_mouth_open(text: &str) -> f32 {
    text.chars()
        .next()
        .map(char_to_mouth_open)
        .unwrap_or(0.3)
}
