//! Azure Speech 云端语音识别后端
//!
//! - 使用 Azure Cognitive Services Speech-to-Text REST API（短音频识别 v3.1）
//! - 通过 `cpal` 采集麦克风音频，重采样到 16kHz 单声道 16-bit PCM
//! - 停止录音时把 WAV 数据 POST 到 Azure，返回完整识别结果
//! - 不支持流式 partial（短音频 API 一次性返回）
//!
//! ## 配置要求
//! - Azure 订阅密钥（`azure.speech_key`）
//! - 服务区域（`azure.speech_region`，如 `eastasia`、`southeastasia`）
//! - 获取：https://portal.azure.com → 创建 "Speech" 资源

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;
use parking_lot::RwLock;
use tokio::sync::broadcast;

use crate::error::{VivianError, VivianResult};

use super::asr::{AsrBackendType, AsrConfig, AsrEngine, AsrEvent};

/// Azure Speech 后端专属配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureSpeechConfig {
    /// Azure Speech 订阅密钥
    pub speech_key: String,
    /// 服务区域（如 eastasia、southeastasia、westus）
    pub speech_region: String,
    /// 识别模式：true=对话模式（更准但慢），false=听写模式
    pub conversation_mode: bool,
    /// 单次请求最大音频时长（秒），Azure 短音频 API 上限 60 秒
    pub max_audio_seconds: u32,
}

impl Default for AzureSpeechConfig {
    fn default() -> Self {
        Self {
            speech_key: String::new(),
            speech_region: "eastasia".to_string(),
            conversation_mode: true,
            max_audio_seconds: 30,
        }
    }
}

// ===========================================================================
// 真实实现（依赖 reqwest + cpal）
// ===========================================================================

mod imp {
    use super::*;

    /// Azure Speech 后端实例
    pub struct AzureBackend {
        pub(crate) config: AsrConfig,
        pub(crate) azure_cfg: AzureSpeechConfig,
        pub(crate) available: bool,
        pub(crate) is_running: bool,
        event_tx: Option<broadcast::Sender<AsrEvent>>,
        // 采集线程句柄（线程内独占 cpal Stream，避免 Send/Sync 限制）
        capture_thread: Option<std::thread::JoinHandle<()>>,
        stop_flag: Option<Arc<AtomicBool>>,
        // 16-bit PCM 样本缓冲（16kHz 单声道）
        buffer: Arc<RwLock<VecDeque<i16>>>,
    }

    impl AzureBackend {
        pub(crate) fn new_inner(config: AsrConfig, azure_cfg: AzureSpeechConfig) -> Self {
            Self {
                config,
                azure_cfg,
                available: true,
                is_running: false,
                event_tx: None,
                capture_thread: None,
                stop_flag: None,
                buffer: Arc::new(RwLock::new(VecDeque::with_capacity(16000 * 30))),
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
            let max_samples = (self.azure_cfg.max_audio_seconds * 16000) as usize;
            let stop_for_thread = stop_flag.clone();
            let stop_for_loop = stop_flag.clone();

            // 把 device move 到线程内（cpal Device 不要求 Send，但在同一线程内使用即可）
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
                        tracing::error!(
                            "不支持的采样格式: {:?}",
                            sample_format
                        );
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
                // 阻塞线程直到 stop_flag 置位，Stream 在线程退出时 drop
                while !stop_for_loop.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                drop(stream);
            });

            self.stop_flag = Some(stop_flag);
            self.capture_thread = Some(handle);
            Ok(())
        }

        /// 把 i16 缓冲打包成 WAV 字节流（16kHz 单声道 16-bit）
        pub(crate) fn build_wav(samples: &[i16]) -> Vec<u8> {
            let num_channels: u16 = 1;
            let sample_rate: u32 = 16000;
            let bits_per_sample: u16 = 16;
            let data_len = samples.len() * 2;
            let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
            let block_align = num_channels * bits_per_sample / 8;
            let mut out = Vec::with_capacity(44 + data_len);
            // RIFF header
            out.extend_from_slice(b"RIFF");
            out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
            out.extend_from_slice(b"WAVE");
            // fmt chunk
            out.extend_from_slice(b"fmt ");
            out.extend_from_slice(&16u32.to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes()); // PCM
            out.extend_from_slice(&num_channels.to_le_bytes());
            out.extend_from_slice(&sample_rate.to_le_bytes());
            out.extend_from_slice(&byte_rate.to_le_bytes());
            out.extend_from_slice(&block_align.to_le_bytes());
            out.extend_from_slice(&bits_per_sample.to_le_bytes());
            // data chunk
            out.extend_from_slice(b"data");
            out.extend_from_slice(&(data_len as u32).to_le_bytes());
            for &s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
            out
        }

        /// 调用 Azure Speech-to-Text 短音频 REST API
        async fn recognize(&self, wav: Vec<u8>) -> VivianResult<String> {
            if self.azure_cfg.speech_key.is_empty() {
                return Err(VivianError::Speech(
                    "Azure Speech 订阅密钥未配置".to_string(),
                ));
            }
            if self.azure_cfg.speech_region.is_empty() {
                return Err(VivianError::Speech(
                    "Azure Speech 服务区域未配置".to_string(),
                ));
            }
            let url = format!(
                "https://{}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1?language={}",
                self.azure_cfg.speech_region,
                self.config.language
            );
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| VivianError::Network(format!("构建 HTTP 客户端失败: {e}")))?;
            let resp = client
                .post(&url)
                .header("Ocp-Apim-Subscription-Key", &self.azure_cfg.speech_key)
                .header("Content-Type", "audio/wav; codecs=audio/pcm; samplerate=16000")
                .header("Accept", "application/json")
                .body(wav)
                .send()
                .await
                .map_err(|e| VivianError::Network(format!("Azure 请求失败: {e}")))?;
            let status = resp.status();
            let body = resp
                .text()
                .await
                .map_err(|e| VivianError::Network(format!("读取响应失败: {e}")))?;
            if !status.is_success() {
                return Err(VivianError::Speech(format!(
                    "Azure 识别失败 ({}): {}",
                    status, body
                )));
            }
            // 解析 JSON: { "RecognitionStatus": "Success", "DisplayText": "你好" }
            let v: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| VivianError::Serialization(format!("解析 Azure 响应失败: {e}")))?;
            let status = v
                .get("RecognitionStatus")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if status != "Success" {
                let err = v
                    .get("DisplayText")
                    .and_then(|s| s.as_str())
                    .unwrap_or(status);
                return Err(VivianError::Speech(format!("Azure 识别状态: {}", err)));
            }
            let text = v
                .get("DisplayText")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            Ok(text)
        }
    }

    #[async_trait]
    impl AsrEngine for AzureBackend {
        async fn initialize(&mut self) -> VivianResult<bool> {
            // Azure 后端无需预加载模型；仅校验配置
            if self.azure_cfg.speech_key.is_empty() {
                tracing::warn!("Azure Speech 密钥未配置，后端标记为不可用");
                self.available = false;
                return Ok(false);
            }
            tracing::info!("Azure Speech 后端初始化完成: region={}", self.azure_cfg.speech_region);
            Ok(true)
        }

        async fn start_recording(&mut self) -> VivianResult<()> {
            if !self.available {
                return Err(VivianError::NotImplemented("Azure 后端不可用".to_string()));
            }
            if self.is_running {
                return Ok(());
            }
            self.start_capture()?;
            self.is_running = true;
            if let Some(tx) = &self.event_tx {
                let _ = tx.send(AsrEvent::Started);
            }
            tracing::info!("Azure 录音已启动");
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

            // 取出采集到的音频
            let samples: Vec<i16> = {
                let mut buf = self.buffer.write();
                buf.drain(..).collect()
            };
            if samples.is_empty() {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(AsrEvent::Stopped);
                }
                return Ok(());
            }
            let wav = Self::build_wav(&samples);
            // 调用 Azure REST API
            match self.recognize(wav).await {
                Ok(text) => {
                    if !text.is_empty() {
                        if let Some(tx) = &self.event_tx {
                            let _ = tx.send(AsrEvent::final_result(text, 0.9));
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Azure 识别失败: {e}");
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(AsrEvent::error(format!("{e}")));
                    }
                }
            }
            if let Some(tx) = &self.event_tx {
                let _ = tx.send(AsrEvent::Stopped);
            }
            tracing::info!("Azure 录音已停止");
            Ok(())
        }

        async fn transcribe(&self, audio: &[f32]) -> VivianResult<String> {
            // 把 f32 转 i16
            let samples: Vec<i16> = audio
                .iter()
                .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                .collect();
            let wav = Self::build_wav(&samples);
            self.recognize(wav).await
        }

        fn is_available(&self) -> bool {
            self.available
        }

        fn supports_silence_detection(&self) -> bool {
            true
        }

        fn supports_partial_results(&self) -> bool {
            false
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
            AsrBackendType::Azure
        }

        fn set_event_sender(&mut self, sender: broadcast::Sender<AsrEvent>) {
            self.event_tx = Some(sender);
        }
    }
}

pub use imp::AzureBackend;

impl AzureBackend {
    pub fn from_config(config: AsrConfig, azure_cfg: AzureSpeechConfig) -> Self {
        Self::new_inner(config, azure_cfg)
    }
}

impl std::fmt::Debug for AzureBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureBackend")
            .field("region", &self.azure_cfg.speech_region)
            .field("conversation_mode", &self.azure_cfg.conversation_mode)
            .field("available", &self.available)
            .field("is_running", &self.is_running)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_azure_backend_from_config() {
        let backend = AzureBackend::from_config(AsrConfig::default(), AzureSpeechConfig::default());
        assert_eq!(backend.backend_type(), AsrBackendType::Azure);
        assert!(!backend.supports_partial_results());
        assert!(backend.supports_silence_detection());
    }

    #[test]
    fn test_build_wav_header() {
        let samples = vec![0i16; 100];
        let wav = AzureBackend::build_wav(&samples);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // data 长度 = 100 * 2 = 200
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_len, 200);
    }

    #[tokio::test]
    async fn test_azure_initialize_no_key() {
        let mut backend = AzureBackend::from_config(AsrConfig::default(), AzureSpeechConfig::default());
        let ok = backend.initialize().await.unwrap_or(false);
        assert!(!ok);
    }
}
