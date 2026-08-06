//! 语音模块 - 包含 ASR 语音识别与 TTS 语音合成
//!
//! - `asr`：后端抽象 trait、配置、事件、管理器
//! - `aliyun_backend`：阿里云 NLS 流式 ASR 后端（WebSocket + JSON-RPC，支持 partial）
//! - `winrt_backend`：Windows WinRT `SpeechRecognizer` 后端（默认，真实实现）
//! - `whisper_backend`：Whisper 识别后端（HTTP 客户端，调用外部 Whisper 服务）
//! - `whisper_service`：faster-whisper-server 子进程管理（一键启动/停止）
//! - `azure_backend`：Azure Speech 云端识别后端（REST API）
//! - `tts`：TTS 管理器与配置（多后端统一调度）
//! - `tts_backend`：TTS 后端抽象 trait + 公共类型 + factory
//! - `tts_edge`：Edge-TTS 后端（WebSocket + WordBoundary，默认在线后端）
//! - `tts_azure`：Azure 认知服务 TTS 后端（REST API + /voices/list）
//! - `tts_gpt_sovits`：GPT-SoVITS 自托管 TTS 后端（兼容 v1/v2）
//! - `gpt_sovits_service`：GPT-SoVITS api_v2.py 子进程管理（一键启动/停止）
//! - `tts_fish_speech`：Fish Speech 后端（fishaudio/fish-speech，/v1/tts）
//! - `tts_minimax`：MiniMax 语音合成后端（WebSocket 流式协议，云端）
//! - `tts_windows`：Windows 原生 WinRT TTS 后端（离线 fallback）
//! - `tts_audio`：MCI 进程内音频播放器
//! - `realtime_voice`：豆包端到端实时语音（SC2.0，独立全双工通话模式）
//! - `realtime_protocol`：实时语音二进制协议编解码
//! - `planner`：Speech Planner 言语调度层（Priority 仲裁 + 队列 + 多角色协调）
//! - `speech_memory`：言语记忆(记录最近说过的内容,避免短时间重复)

pub mod aliyun_backend;
pub mod asr;
pub mod azure_backend;
pub mod fish_speech_service;
pub mod gpt_sovits_service;
pub mod planner;
pub mod realtime_protocol;
pub mod realtime_voice;
pub mod speech_memory;
pub mod tts;
pub mod tts_cache;
pub mod tts_audio;
pub mod tts_azure;
pub mod tts_backend;
pub mod tts_doubao;
pub mod tts_edge;
pub mod tts_fish_speech;
pub mod tts_gpt_sovits;
pub mod tts_minimax;
pub mod tts_windows;
pub mod whisper_backend;
pub mod whisper_realtime;
pub mod whisper_service;
pub mod winrt_backend;

pub use aliyun_backend::{AliyunAsrConfig, AliyunBackend};
pub use asr::{
    create_asr_backend, AsrBackendType, AsrConfig, AsrEngine, AsrEvent, AsrManager,
    RecognitionResult, SpeechManager, VadConfig,
};
pub use azure_backend::{AzureBackend, AzureSpeechConfig};
pub use fish_speech_service::{
    service as fish_speech_service, FishSpeechServiceManager, FishSpeechServiceState,
    FishSpeechServiceStatus,
};
pub use gpt_sovits_service::{service as gpt_sovits_service, ServiceState, ServiceStatus};
pub use realtime_voice::{CallState, RealtimeEvent, RealtimeVoiceManager};
pub use tts::{
    EmotionVoiceEntry, MouthCallback, TtsConfig, TtsEngine, TtsEvent, TtsEventCallback,
    TtsManager, VoiceInfo,
};
pub use tts_backend::{
    char_to_mouth_open, word_to_mouth_open, AudioChunk, AudioFormat, StreamCallback, TtsBackend,
    TtsSynthesisResult, WordBoundary,
};
pub use whisper_backend::{
    WhisperApiFormat, WhisperBackend, WhisperConfig, WhisperStreamingMode,
};
pub use whisper_realtime::WhisperRealtimeBackend;
pub use whisper_service::{
    service as whisper_service, WhisperServiceManager, WhisperServiceState, WhisperServiceStatus,
};
pub use winrt_backend::{RecognitionMode, WinrtBackend};

pub use planner::{
    planner as get_planner, speak_intent, start_pump_loop, PlannerEvent, PlannerEventCallback,
    Presentation, SpeakIntent, SpeakIntentBuilder, SpeechContext, SpeechPlanner, SpeechPriority,
    SpeechScene, SubmitHandle, SubmitResult,
};
pub use speech_memory::SpeechMemory;
pub use tts::VoiceProfile;
