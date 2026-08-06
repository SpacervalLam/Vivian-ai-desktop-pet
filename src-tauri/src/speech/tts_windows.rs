//! Windows 原生 TTS 后端 (WinRT SpeechSynthesizer)
//!
//! - 使用 Windows 10+ 内置的 WinRT 语音合成
//! - 无需网络、无需 API Key、无需额外安装
//! - 输出 WAV 格式
//! - 支持枚举系统已安装的语音

use async_trait::async_trait;

use crate::error::{VivianError, VivianResult};

use super::tts::{TtsConfig, VoiceInfo};
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult};

pub struct WindowsTtsBackend;

impl WindowsTtsBackend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TtsBackend for WindowsTtsBackend {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn requires_network(&self) -> bool {
        false
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        if text.trim().is_empty() {
            return Ok(TtsSynthesisResult::new(Vec::new(), AudioFormat::Wav));
        }

        tracing::info!("[TTS] WinRT synthesize 开始: voice_id={:?} rate={} vol={}", config.voice_id, config.rate, config.volume);
        let text = text.to_string();
        let voice_id = config.voice_id.clone();
        // config.rate 是倍率(1.0=正常); WinRT SpeakingRate 是相对偏移(0=默认)
        let speaking_rate = (config.rate - 1.0) * 150.0;
        let volume = config.volume as f64;

        let result = tokio::task::spawn_blocking(move || -> VivianResult<TtsSynthesisResult> {
            // 初始化 COM (WinRT 需要)
            #[cfg(windows)]
            {
                use windows::Win32::System::Com::{
                    CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED,
                };
                let co_init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let co_init_ok = co_init.is_ok();

                let result = (|| -> VivianResult<TtsSynthesisResult> {
                    use windows::Media::SpeechSynthesis::SpeechSynthesizer;
                    use windows::Storage::Streams::DataReader;
                    use windows::core::HSTRING;

                    let synth = SpeechSynthesizer::new().map_err(|e| {
                        VivianError::Speech(format!("创建 SpeechSynthesizer 失败: {e}"))
                    })?;

                    // 选择语音
                    if let Some(vid) = &voice_id {
                        if !vid.is_empty() {
                            let voices = SpeechSynthesizer::AllVoices().map_err(|e| {
                                VivianError::Speech(format!("获取语音列表失败: {e}"))
                            })?;
                            let count = voices.Size().map_err(|e| {
                                VivianError::Speech(format!("获取语音数量失败: {e}"))
                            })?;
                            for i in 0..count {
                                let voice = voices.GetAt(i).map_err(|e| {
                                    VivianError::Speech(format!("获取语音失败: {e}"))
                                })?;
                                let display = voice.DisplayName().unwrap_or_default().to_string();
                                let id = voice.Id().unwrap_or_default().to_string();
                                if display.contains(vid.as_str()) || id.contains(vid.as_str()) {
                                    synth.SetVoice(&voice).map_err(|e| {
                                        VivianError::Speech(format!("设置语音失败: {e}"))
                                    })?;
                                    break;
                                }
                            }
                        }
                    }

                    // 设置语速和音量
                    let options = synth.Options().map_err(|e| {
                        VivianError::Speech(format!("获取合成选项失败: {e}"))
                    })?;
                    let _ = options.SetSpeakingRate(speaking_rate);
                    let _ = options.SetAudioVolume(volume);

                    // 合成
                    let htext = HSTRING::from(&text);
                    let async_op = synth.SynthesizeTextToStreamAsync(&htext).map_err(|e| {
                        VivianError::Speech(format!("启动合成失败: {e}"))
                    })?;
                    let stream = async_op.get().map_err(|e| {
                        VivianError::Speech(format!("合成等待失败: {e}"))
                    })?;

                    // 读取流到字节
                    let size = stream.Size().map_err(|e| {
                        VivianError::Speech(format!("获取流大小失败: {e}"))
                    })? as u32;
                    let input_stream = stream.GetInputStreamAt(0).map_err(|e| {
                        VivianError::Speech(format!("获取输入流失败: {e}"))
                    })?;
                    let reader = DataReader::CreateDataReader(&input_stream).map_err(|e| {
                        VivianError::Speech(format!("创建 DataReader 失败: {e}"))
                    })?;
                    let load_op = reader.LoadAsync(size).map_err(|e| {
                        VivianError::Speech(format!("启动加载失败: {e}"))
                    })?;
                    let _ = load_op.get().map_err(|e| {
                        VivianError::Speech(format!("加载数据失败: {e}"))
                    })?;

                    let mut bytes = vec![0u8; size as usize];
                    reader.ReadBytes(&mut bytes).map_err(|e| {
                        VivianError::Speech(format!("读取字节失败: {e}"))
                    })?;

                    tracing::info!("[TTS] WinRT synthesize 成功: {} 字节", bytes.len());
                    Ok(TtsSynthesisResult::new(bytes, AudioFormat::Wav))
                })();

                if co_init_ok {
                    unsafe { CoUninitialize() };
                }
                result
            }
            #[cfg(not(windows))]
            {
                Err(VivianError::Speech(
                    "Windows TTS 仅支持 Windows 平台".to_string(),
                ))
            }
        })
        .await
        .map_err(|e| VivianError::Speech(format!("WinRT TTS 任务失败: {e}")))??;

        Ok(result)
    }

    async fn list_voices(&self, _config: &TtsConfig) -> VivianResult<Vec<VoiceInfo>> {
        let voices = tokio::task::spawn_blocking(|| -> VivianResult<Vec<VoiceInfo>> {
            #[cfg(windows)]
            {
                use windows::Win32::System::Com::{
                    CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED,
                };
                use windows::Media::SpeechSynthesis::SpeechSynthesizer;

                let co_init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let co_init_ok = co_init.is_ok();

                let result = (|| -> VivianResult<Vec<VoiceInfo>> {
                    let all_voices = SpeechSynthesizer::AllVoices()
                        .map_err(|e| VivianError::Speech(format!("获取语音列表失败: {e}")))?;
                    let count = all_voices
                        .Size()
                        .map_err(|e| VivianError::Speech(format!("获取语音数量失败: {e}")))?;
                    let mut result = Vec::new();
                    for i in 0..count {
                        let voice = all_voices
                            .GetAt(i)
                            .map_err(|e| VivianError::Speech(format!("获取语音失败: {e}")))?;
                        let display = voice.DisplayName().unwrap_or_default().to_string();
                        let id = voice.Id().unwrap_or_default().to_string();
                        let lang = voice.Language().unwrap_or_default().to_string();
                        result.push(VoiceInfo {
                            id,
                            name: display,
                            language: lang,
                        });
                    }
                    Ok(result)
                })();

                if co_init_ok {
                    unsafe { CoUninitialize() };
                }
                result
            }
            #[cfg(not(windows))]
            {
                Ok(Vec::new())
            }
        })
        .await
        .map_err(|e| VivianError::Speech(format!("列出语音失败: {e}")))??;

        Ok(voices)
    }

    async fn health_check(&self, _config: &TtsConfig) -> bool {
        true
    }
}
