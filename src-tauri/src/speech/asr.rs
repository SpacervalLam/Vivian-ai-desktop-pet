//! 语音识别（ASR）- 后端抽象、配置、事件与管理器
//!
//! - `AsrEngine` trait 对应 `SpeechRecognitionBackend` ABC
//! - `AsrManager` 对应 `SpeechRecognitionManager`
//! - 当前默认后端为 Windows WinRT `SpeechRecognizer`

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

use crate::error::{VivianError, VivianResult};

use super::winrt_backend::WinrtBackend;
use super::whisper_backend::{WhisperBackend, WhisperConfig, WhisperStreamingMode};
use super::whisper_realtime::WhisperRealtimeBackend;
use super::azure_backend::{AzureBackend, AzureSpeechConfig};
use super::aliyun_backend::{AliyunBackend, AliyunAsrConfig};
use super::openai_whisper_backend::{OpenaiWhisperBackend, OpenaiWhisperConfig};

// ---------------------------------------------------------------------------
// 识别结果（保留原有类型，供现有代码使用）
// ---------------------------------------------------------------------------

/// 识别结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecognitionResult {
    pub text: String,
    pub is_final: bool,
    pub confidence: f64,
}

impl RecognitionResult {
    pub fn new(text: impl Into<String>, is_final: bool, confidence: f64) -> Self {
        Self {
            text: text.into(),
            is_final,
            confidence,
        }
    }

    pub fn partial(text: impl Into<String>) -> Self {
        Self::new(text, false, 0.0)
    }

    pub fn final_result(text: impl Into<String>, confidence: f64) -> Self {
        Self::new(text, true, confidence)
    }
}

// ---------------------------------------------------------------------------
// 后端类型枚举
// ---------------------------------------------------------------------------

/// ASR 后端类型
///
/// - Winrt：Windows 原生 WinRT/SAPI 语音识别（无需额外密钥/模型）
/// - Whisper：本地 Whisper 推理（需 `whisper` feature + ggml 模型文件）
/// - Azure：Azure Cognitive Services Speech-to-Text REST API（需订阅密钥）
/// - Aliyun：阿里云 NLS 实时语音识别（WebSocket 流式，支持 partial）
/// - OpenaiWhisper：OpenAI Whisper API 云端识别（需 API Key）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AsrBackendType {
    /// Windows 原生 WinRT/SAPI 语音识别
    Winrt,
    /// 本地 Whisper 语音识别（HTTP 客户端，调用本地 whisper.cpp / faster-whisper-server）
    Whisper,
    /// Azure Speech 云端识别（REST API）
    Azure,
    /// 阿里云 NLS 实时语音识别（WebSocket 流式）
    Aliyun,
    /// OpenAI Whisper API 云端识别（REST API）
    OpenaiWhisper,
}

impl Default for AsrBackendType {
    fn default() -> Self {
        AsrBackendType::Winrt
    }
}

impl AsrBackendType {
    pub fn from_engine_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "whisper" => AsrBackendType::Whisper,
            "azure" => AsrBackendType::Azure,
            "aliyun" => AsrBackendType::Aliyun,
            "openai_whisper" | "openaiwhisper" => AsrBackendType::OpenaiWhisper,
            _ => AsrBackendType::Winrt,
        }
    }
}

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// VAD（语音活动检测）参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadConfig {
    /// 能量阈值，超过则视为正在说话（默认 500.0）
    pub energy_threshold: f32,
    /// 最小语音持续时间（ms），短于此长度视为噪声
    pub min_speech_duration_ms: u64,
    /// 最大静默持续时间（ms），超过则判定用户说毕
    pub max_silence_duration_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            energy_threshold: 500.0,
            min_speech_duration_ms: 250,
            max_silence_duration_ms: 1500,
        }
    }
}

/// ASR 配置 + 音频采集参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrConfig {
    /// 后端引擎类型
    pub engine: AsrBackendType,
    /// 识别语言（BCP-47，如 zh-CN / en-US）
    pub language: String,
    /// 采样率（Hz），16kHz
    pub sample_rate: u32,
    /// 声道数（1 = 单声道）
    pub channels: u16,
    /// 位深（16 = 16-bit PCM）
    pub bits_per_sample: u16,
    /// 静默自动停止超时（ms）
    pub silence_timeout_ms: u64,
    /// VAD 参数
    pub vad: VadConfig,
    /// Whisper 后端子配置（仅 engine=Whisper 时使用）
    #[serde(default)]
    pub whisper: WhisperConfig,
    /// Azure 后端子配置（仅 engine=Azure 时使用）
    #[serde(default)]
    pub azure: AzureSpeechConfig,
    /// 阿里云 NLS 后端子配置（仅 engine=Aliyun 时使用）
    #[serde(default)]
    pub aliyun: AliyunAsrConfig,
    /// OpenAI Whisper API 后端子配置（仅 engine=OpenaiWhisper 时使用）
    #[serde(default)]
    pub openai_whisper: OpenaiWhisperConfig,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            engine: AsrBackendType::Winrt,
            language: "zh-CN".to_string(),
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
            silence_timeout_ms: 1500,
            vad: VadConfig::default(),
            whisper: WhisperConfig::default(),
            azure: AzureSpeechConfig::default(),
            aliyun: AliyunAsrConfig::default(),
            openai_whisper: OpenaiWhisperConfig::default(),
        }
    }
}

impl AsrConfig {
    /// 从 AppConfig.speech_recognition 构造 AsrConfig
    /// 音频采集参数（sample_rate / channels / bits_per_sample / vad）使用默认值
    pub fn from_speech_config(s: &crate::config::manager::SpeechRecognitionConfig) -> Self {
        Self {
            engine: AsrBackendType::from_engine_str(&s.engine),
            language: s.language.clone(),
            silence_timeout_ms: s.silence_timeout_ms,
            whisper: s.whisper.clone(),
            azure: s.azure.clone(),
            aliyun: s.aliyun.clone(),
            openai_whisper: s.openai_whisper.clone(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------

/// ASR 识别事件（partial / final / started / stopped）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AsrEvent {
    /// 识别已开始
    Started,
    /// 部分识别结果（流式）
    PartialResult { text: String },
    /// 最终识别结果
    FinalResult { text: String, confidence: f64 },
    /// 识别已停止
    Stopped,
    /// 识别错误
    Error { message: String },
}

impl AsrEvent {
    pub fn partial(text: impl Into<String>) -> Self {
        AsrEvent::PartialResult {
            text: text.into(),
        }
    }

    pub fn final_result(text: impl Into<String>, confidence: f64) -> Self {
        AsrEvent::FinalResult {
            text: text.into(),
            confidence,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        AsrEvent::Error {
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// AsrEngine trait
// ---------------------------------------------------------------------------

/// ASR 后端抽象 trait。
///
/// 所有方法为 async，使用 `async_trait` 提供对象安全 trait 对象，
/// 以便 `AsrManager` 在多后端间动态切换（`Box<dyn AsrEngine + Send + Sync>`）。
#[async_trait]
pub trait AsrEngine: Send + Sync {
    /// 初始化后端（加载模型 / 创建识别器），成功返回 true。
    async fn initialize(&mut self) -> VivianResult<bool>;

    /// 开始录音识别。
    async fn start_recording(&mut self) -> VivianResult<()>;

    /// 停止录音识别。
    async fn stop_recording(&mut self) -> VivianResult<()>;

    /// 对音频样本（f32，归一化到 [-1, 1]）进行转译，返回文本。
    async fn transcribe(&self, audio: &[f32]) -> VivianResult<String>;

    /// 检查后端是否可用。
    fn is_available(&self) -> bool;

    /// 是否内置静默检测。
    fn supports_silence_detection(&self) -> bool;

    /// 是否支持流式部分结果。
    fn supports_partial_results(&self) -> bool;

    /// 释放所有资源。
    fn dispose(&mut self);

    /// 返回后端类型。
    fn backend_type(&self) -> AsrBackendType;

    /// 注入事件广播通道。
    ///
    /// 后端可在 WinRT 事件回调中通过该通道向 [`AsrManager`] 广播
    /// `PartialResult` / `FinalResult` / `Started` / `Stopped` / `Error` 事件。
    /// 默认空实现，使非事件驱动后端无需关心。
    fn set_event_sender(&mut self, _sender: broadcast::Sender<AsrEvent>) {}
}

/// 按配置创建对应后端的 trait 对象。
pub fn create_asr_backend(config: &AsrConfig) -> Box<dyn AsrEngine + Send + Sync> {
    match config.engine {
        AsrBackendType::Winrt => Box::new(WinrtBackend::from_config(config.clone())),
        AsrBackendType::Whisper => {
            // realtime_ws 模式使用独立的 WebSocket 后端；其他模式用 HTTP 后端
            match config.whisper.streaming_mode {
                WhisperStreamingMode::RealtimeWs => Box::new(WhisperRealtimeBackend::from_config(
                    config.clone(),
                    config.whisper.clone(),
                )),
                WhisperStreamingMode::None | WhisperStreamingMode::Sse => Box::new(
                    WhisperBackend::from_config(config.clone(), config.whisper.clone()),
                ),
            }
        }
        AsrBackendType::Azure => {
            Box::new(AzureBackend::from_config(config.clone(), config.azure.clone()))
        }
        AsrBackendType::Aliyun => {
            Box::new(AliyunBackend::from_config(config.clone(), config.aliyun.clone()))
        }
        AsrBackendType::OpenaiWhisper => Box::new(OpenaiWhisperBackend::from_config(
            config.clone(),
            config.openai_whisper.clone(),
        )),
    }
}

// ---------------------------------------------------------------------------
// AsrManager
// ---------------------------------------------------------------------------

/// ASR 高层管理器。
///
/// 作为唯一入口，按配置创建对应后端并委托全部操作；
/// 支持配置热重载（`reconfigure`）与事件广播（`subscribe`）。
///
/// 内部状态置于 `Arc`，因此 `AsrManager` 是 `Clone` 的，
/// 可安全放入 `AppState` 并在命令中共享。
#[derive(Clone)]
pub struct AsrManager {
    inner: Arc<AsrManagerInner>,
}

struct AsrManagerInner {
    config: RwLock<AsrConfig>,
    backend: Mutex<Option<Box<dyn AsrEngine + Send + Sync>>>,
    is_recording: AtomicBool,
    available: AtomicBool,
    event_tx: broadcast::Sender<AsrEvent>,
}

impl AsrManager {
    /// 使用默认配置创建管理器。
    pub fn new() -> Self {
        Self::new_with_config(AsrConfig::default())
    }

    /// 使用指定配置创建管理器。
    pub fn new_with_config(config: AsrConfig) -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(AsrManagerInner {
                config: RwLock::new(config),
                backend: Mutex::new(None),
                is_recording: AtomicBool::new(false),
                available: AtomicBool::new(true),
                event_tx,
            }),
        }
    }

    /// 获取当前配置副本。
    pub fn get_config(&self) -> AsrConfig {
        self.inner.config.read().clone()
    }

    /// 更新配置并在下次使用时重建后端。
    pub async fn set_config(&self, config: AsrConfig) -> VivianResult<()> {
        self.reconfigure_with(|c| {
            *c = config;
        })
        .await
    }

    /// 惰性创建并初始化后端。
    ///
    /// 若之前初始化失败（available=false），会丢弃旧后端并重新尝试初始化，
    /// 允许用户在切换后端或修复环境后重试，而不是永久拒绝。
    async fn ensure_initialized(&self) -> VivianResult<()> {
        let mut backend_guard = self.inner.backend.lock().await;
        // 后端已存在且可用时直接返回
        if backend_guard.is_some() && self.inner.available.load(Ordering::SeqCst) {
            return Ok(());
        }
        // 之前初始化失败：丢弃旧后端（如果有），重新创建
        if let Some(mut old) = backend_guard.take() {
            old.dispose();
        }

        let config = self.inner.config.read().clone();
        let mut backend = create_asr_backend(&config);
        // 注入事件通道，使后端可在回调中广播识别事件
        backend.set_event_sender(self.inner.event_tx.clone());
        // 后端 initialize() 失败时返回 Err 携带详细诊断（如 WinRT 0x800455A0 的修复建议），
        // 这里直接透传，避免被 unwrap_or(false) 吞掉具体原因。
        let ok = match backend.initialize().await {
            Ok(v) => v,
            Err(e) => {
                self.inner.available.store(false, Ordering::SeqCst);
                return Err(e);
            }
        };
        if !ok {
            self.inner.available.store(false, Ordering::SeqCst);
            return Err(VivianError::Speech(
                "ASR 后端初始化失败（后端返回 false 但未提供详细原因）".to_string(),
            ));
        }
        self.inner.available.store(true, Ordering::SeqCst);
        *backend_guard = Some(backend);
        Ok(())
    }

    /// 开始语音识别。
    pub async fn start_recognition(&self) -> VivianResult<()> {
        if self.inner.is_recording.load(Ordering::SeqCst) {
            return Err(VivianError::Speech(
                "语音识别已在进行中".to_string(),
            ));
        }

        // ensure_initialized 会自行处理 available 标志：
        // - 后端未创建时尝试初始化，成功则 available=true，失败则 available=false 并返回 Err
        // - 之前初始化失败（available=false）时，这里仍会重新尝试初始化，允许用户切换后端后重试
        self.ensure_initialized().await?;

        let mut backend_guard = self.inner.backend.lock().await;
        if let Some(backend) = backend_guard.as_mut() {
            backend.start_recording().await?;
        }
        self.inner.is_recording.store(true, Ordering::SeqCst);
        let _ = self.inner.event_tx.send(AsrEvent::Started);
        tracing::info!("语音识别已启动");
        Ok(())
    }

    /// 停止语音识别（幂等：已停止时直接返回 Ok）。
    pub async fn stop_recognition(&self) -> VivianResult<()> {
        if !self.inner.is_recording.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut backend_guard = self.inner.backend.lock().await;
        if let Some(backend) = backend_guard.as_mut() {
            let _ = backend.stop_recording().await;
        }
        self.inner.is_recording.store(false, Ordering::SeqCst);
        let _ = self.inner.event_tx.send(AsrEvent::Stopped);
        tracing::info!("语音识别已停止");
        Ok(())
    }

    /// 对音频样本进行转译（委托给当前后端）。
    pub async fn transcribe(&self, audio: &[f32]) -> VivianResult<String> {
        self.ensure_initialized().await?;
        let backend_guard = self.inner.backend.lock().await;
        if let Some(backend) = backend_guard.as_ref() {
            backend.transcribe(audio).await
        } else {
            Err(VivianError::Speech(
                "ASR 后端未初始化".to_string(),
            ))
        }
    }

    /// 是否正在录音。
    pub fn is_recording(&self) -> bool {
        self.inner.is_recording.load(Ordering::SeqCst)
    }

    /// 后端是否可用。
    pub fn is_available(&self) -> bool {
        self.inner.available.load(Ordering::SeqCst)
    }

    /// 订阅识别事件流。
    pub fn subscribe(&self) -> broadcast::Receiver<AsrEvent> {
        self.inner.event_tx.subscribe()
    }

    /// 发送一个事件（供后端/外部回调使用）。
    pub fn emit_event(&self, event: AsrEvent) {
        let _ = self.inner.event_tx.send(event);
    }

    /// 标记识别已停止（供事件转发器在收到后端 Stopped 事件时同步状态）。
    pub fn mark_stopped(&self) {
        self.inner.is_recording.store(false, Ordering::SeqCst);
    }

    /// 释放当前后端并强制下次使用时重建。
    pub async fn reconfigure(&self) -> VivianResult<()> {
        self.reconfigure_with(|_| {}).await
    }

    async fn reconfigure_with<F: FnOnce(&mut AsrConfig)>(&self, f: F) -> VivianResult<()> {
        // 先释放后端（异步获取锁，避免在 async 上下文中 block_on）
        let backend_opt = {
            let mut backend_guard = self.inner.backend.lock().await;
            backend_guard.take()
        };
        if let Some(mut backend) = backend_opt {
            backend.dispose();
        }

        {
            let mut config = self.inner.config.write();
            f(&mut config);
        }
        self.inner.is_recording.store(false, Ordering::SeqCst);
        self.inner.available.store(true, Ordering::SeqCst);
        tracing::info!("语音识别管理器已重配置，下次使用将创建新后端");
        Ok(())
    }

    /// 释放所有资源。
    pub async fn dispose(&self) {
        let mut backend_guard = self.inner.backend.lock().await;
        if let Some(backend) = backend_guard.as_mut() {
            backend.dispose();
        }
        *backend_guard = None;
        self.inner.is_recording.store(false, Ordering::SeqCst);
        self.inner.available.store(false, Ordering::SeqCst);
    }
}

impl Default for AsrManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AsrManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsrManager")
            .field("is_recording", &self.inner.is_recording.load(Ordering::SeqCst))
            .field("available", &self.inner.available.load(Ordering::SeqCst))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// SpeechManager（保留，向后兼容；新代码应使用 AsrManager）
// ---------------------------------------------------------------------------

/// 语音识别管理器（旧版，仅管理录音状态）。
///
/// 保留以兼容现有导出；新代码请使用 [`AsrManager`]。
pub struct SpeechManager {
    is_recording: AtomicBool,
}

impl SpeechManager {
    pub fn new() -> Self {
        Self {
            is_recording: AtomicBool::new(false),
        }
    }

    pub async fn start_recognition(&mut self) -> VivianResult<()> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Err(VivianError::Speech("语音识别已在进行中".to_string()));
        }
        self.is_recording.store(true, Ordering::SeqCst);
        tracing::info!("语音识别已启动");
        Ok(())
    }

    pub async fn stop_recognition(&mut self) -> VivianResult<()> {
        if !self.is_recording.load(Ordering::SeqCst) {
            return Err(VivianError::Speech("语音识别未在进行中".to_string()));
        }
        self.is_recording.store(false, Ordering::SeqCst);
        tracing::info!("语音识别已停止");
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }
}

impl Default for SpeechManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asr_config_default() {
        let config = AsrConfig::default();
        assert_eq!(config.engine, AsrBackendType::Winrt);
        assert_eq!(config.language, "zh-CN");
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
        assert_eq!(config.bits_per_sample, 16);
        assert_eq!(config.silence_timeout_ms, 1500);
        assert_eq!(config.vad.energy_threshold, 500.0);
    }

    #[test]
    fn test_asr_backend_type_from_engine_str() {
        assert_eq!(
            AsrBackendType::from_engine_str("winrt"),
            AsrBackendType::Winrt
        );
        assert_eq!(
            AsrBackendType::from_engine_str("unknown"),
            AsrBackendType::Winrt
        );
    }

    #[test]
    fn test_asr_event_serialization() {
        let event = AsrEvent::final_result("你好", 0.95);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("final_result"));
        assert!(json.contains("你好"));

        let partial = AsrEvent::partial("你");
        let json2 = serde_json::to_string(&partial).unwrap();
        assert!(json2.contains("partial_result"));
    }

    #[test]
    fn test_recognition_result_constructors() {
        let partial = RecognitionResult::partial("hello");
        assert!(!partial.is_final);
        assert_eq!(partial.text, "hello");

        let final_res = RecognitionResult::final_result("你好", 0.9);
        assert!(final_res.is_final);
        assert_eq!(final_res.confidence, 0.9);
    }

    #[tokio::test]
    async fn test_asr_manager_start_stop_state() {
        let manager = AsrManager::new();
        assert!(!manager.is_recording());

        // 真实 WinRT 后端：start 可能因无麦克风/语音运行时而失败，也可能成功。
        // 此处仅验证状态一致性，不依赖硬件环境。
        let start_result = manager.start_recognition().await;
        if start_result.is_ok() {
            assert!(manager.is_recording());
            let stop_result = manager.stop_recognition().await;
            assert!(stop_result.is_ok());
            assert!(!manager.is_recording());
        } else {
            assert!(!manager.is_recording());
            // 未在录音时 stop 幂等返回 Ok
            let stop_result = manager.stop_recognition().await;
            assert!(stop_result.is_ok());
        }
        manager.dispose().await;
    }

    #[tokio::test]
    async fn test_asr_manager_transcribe_not_implemented() {
        let manager = AsrManager::new();
        // transcribe 会触发后端初始化（占位后端 initialize 返回 false），
        // 故应返回错误。
        let audio = vec![0.0f32; 1600];
        let result = manager.transcribe(&audio).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_asr_manager_clone_shares_state() {
        let manager = AsrManager::new();
        let cloned = manager.clone();
        // 克隆共享内部状态
        assert!(!cloned.is_recording());
        assert!(!manager.is_recording());
    }
}
