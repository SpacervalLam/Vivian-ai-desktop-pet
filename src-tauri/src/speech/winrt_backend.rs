//! WinRT 语音识别后端
//!
//! - 通过 Windows Runtime `SpeechRecognizer` 提供持续识别
//! - 可配置语言选择（BCP-47，如 `zh-CN`）+ 静默自动停止（默认 1500ms）
//! - 非用户停止时尝试自动重启
//! - `supports_silence_detection = true`，`supports_partial_results = true`
//!
//! ## 事件流
//! 后端通过 [`AsrEngine::set_event_sender`] 接收 [`AsrManager`] 的广播通道，
//! 在 WinRT 事件回调（`ResultGenerated` / `HypothesisGenerated` / `Completed`）
//! 中向管理器发送 `AsrEvent::PartialResult` / `FinalResult` / `Started` / `Stopped` / `Error`。
//!
//! ## 异步说明
//! windows 0.58 的 `IAsyncAction` / `IAsyncOperation<T>` 在当前 feature 配置下未实现
//! `Future`/`IntoFuture`，因此使用阻塞式 `.get()` 等待 COM 异步操作完成。
//! 这些操作（编译约束 / 启停会话）均为短时操作，阻塞可接受；音频结果通过事件回调异步到达。
//!
//! ## 平台支持
//! - Windows：使用 `windows` crate 调用 `Windows.Media.SpeechRecognition` API（真实实现）
//! - 非 Windows：占位实现，所有方法返回 `NotImplemented`，保持跨平台可编译

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::broadcast;

use crate::error::{VivianError, VivianResult};

use super::asr::{AsrBackendType, AsrConfig, AsrEngine, AsrEvent};

/// 识别模式：持续识别 / 关键词唤醒两种用法。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RecognitionMode {
    /// 持续识别（ContinuousRecognitionSession）
    Continuous,
    /// 关键词唤醒（KeywordRecognition），当前回退到持续识别 + 自由听写
    Keyword,
}

impl Default for RecognitionMode {
    fn default() -> Self {
        RecognitionMode::Continuous
    }
}

// ===========================================================================
// Windows 真实实现
// ===========================================================================

#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use windows::core::HSTRING;
    use windows::Foundation::TypedEventHandler;

    /// WinRT 事件注销令牌（windows 0.61 中 EventRegistrationToken 即 i64）
    type EventRegistrationToken = i64;
    use windows::Globalization::Language;
    use windows::Media::SpeechRecognition::{
        SpeechContinuousRecognitionCompletedEventArgs,
        SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechRecognitionConfidence,
        SpeechRecognitionHypothesisGeneratedEventArgs, SpeechRecognitionScenario,
        SpeechRecognitionResultStatus, SpeechRecognitionTopicConstraint, SpeechRecognizer,
        SpeechContinuousRecognitionSession,
    };

    /// 事件回调与监视任务共享的状态（Arc 包裹，跨线程访问）。
    struct WinrtShared {
        event_tx: broadcast::Sender<AsrEvent>,
        last_speech_ms: AtomicU64,
        user_stop: AtomicBool,
        needs_restart: AtomicBool,
        is_running: AtomicBool,
        stop_watcher: AtomicBool,
    }

    impl WinrtShared {
        fn touch(&self) {
            self.last_speech_ms.store(now_ms(), Ordering::SeqCst);
        }
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// 将 WinRT 置信度枚举映射为 [0.0, 1.0] 浮点。
    fn map_confidence(c: SpeechRecognitionConfidence) -> f64 {
        match c {
            SpeechRecognitionConfidence::High => 0.9,
            // windows 0.58 中 `Normal` 已重命名为 `Medium`
            SpeechRecognitionConfidence::Medium => 0.7,
            SpeechRecognitionConfidence::Low => 0.4,
            _ => 0.0,
        }
    }

    /// Windows WinRT 语音识别后端。
    pub struct WinrtBackend {
        pub(crate) config: AsrConfig,
        pub(crate) mode: RecognitionMode,
        pub(crate) available: bool,
        pub(crate) is_running: bool,
        event_tx: Option<broadcast::Sender<AsrEvent>>,
        recognizer: Option<SpeechRecognizer>,
        session: Option<SpeechContinuousRecognitionSession>,
        shared: Option<Arc<WinrtShared>>,
        result_token: Option<EventRegistrationToken>,
        completed_token: Option<EventRegistrationToken>,
        hypothesis_token: Option<EventRegistrationToken>,
        watcher_handle: Option<tokio::task::JoinHandle<()>>,
    }

    impl WinrtBackend {
        pub(crate) fn new_inner(config: AsrConfig, mode: RecognitionMode) -> Self {
            Self {
                config,
                mode,
                available: true,
                is_running: false,
                event_tx: None,
                recognizer: None,
                session: None,
                shared: None,
                result_token: None,
                completed_token: None,
                hypothesis_token: None,
                watcher_handle: None,
            }
        }

        /// 创建识别器，语言不可用时回退到系统默认。
        ///
        /// 返回原始 `windows::core::Error`，调用方负责通过 [`diagnose_winrt_error`] 转换为
        /// 带可执行修复建议的 `VivianError`。
        fn create_recognizer(&self) -> windows::core::Result<SpeechRecognizer> {
            match Language::CreateLanguage(&HSTRING::from(self.config.language.as_str())) {
                Ok(lang) => match SpeechRecognizer::Create(&lang) {
                    Ok(r) => Ok(r),
                    Err(e) => {
                        tracing::warn!(
                            "Create({}) 失败: {e}，回退到系统默认",
                            self.config.language
                        );
                        SpeechRecognizer::new()
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "Language({}) 创建失败: {e}，回退到系统默认",
                        self.config.language
                    );
                    SpeechRecognizer::new()
                }
            }
        }

        /// 加载自由听写约束并编译。
        ///
        /// 使用 `.get()` 阻塞等待编译完成——一次性初始化操作，可接受。
        fn compile_constraints(
            &self,
            recognizer: &SpeechRecognizer,
        ) -> windows::core::Result<()> {
            // windows 0.58: Create 只接受 2 个参数（scenario, topichint）
            let dictation = SpeechRecognitionTopicConstraint::Create(
                SpeechRecognitionScenario::Dictation,
                &HSTRING::new(),
            )?;
            recognizer.Constraints()?.Append(&dictation)?;
            let op = recognizer.CompileConstraintsAsync()?;
            // 阻塞等待编译完成（windows 0.58 IAsyncOperation 未实现 Future，用 .get()）
            op.get()?;
            Ok(())
        }
    }

    /// 将 `windows::core::Error` 转换为带可执行修复建议的 `VivianError`。
    ///
    /// 根据 HRESULT 错误码映射出对应的修复指引，便于前端 toast 直接向用户展示。
    /// 特别针对 `0x800455A0`（SPERR_WINRT_INTERNAL_ERROR）：该错误在 unpackaged
    /// 桌面应用（如 Tauri）调用 WinRT `SpeechRecognizer` 时高发，通常是 WinRT 公共
    /// 语音 API 在无包身份上下文中无法初始化的信号。
    fn diagnose_winrt_error(err: &windows::core::Error, context: &str) -> VivianError {
        let hr = err.code();
        // HRESULT 是 i32，转 u32 以便用十六进制匹配 Win32 错误码
        let hr_u = hr.0 as u32;
        let hr_hex = format!("0x{:08X}", hr_u);
        let msg = err.message().to_string();

        let hint: String = match hr_u {
            // SPERR_WINRT_INTERNAL_ERROR —— Tauri unpackaged 应用调用 WinRT SpeechRecognizer 高发
            0x800455A0 => format!(
                "WinRT 公共语音 API 在当前应用上下文中初始化失败。\n\
                 排查建议：\n\
                 1) 设置 → 隐私和安全性 → 麦克风，确认「麦克风访问」、「允许应用访问麦克风」、「允许桌面应用访问麦克风」三项全部为开；\n\
                 2) 确认 Windows 语音识别语言包已正确安装；\n\
                 3) 若以上已开仍报此错，可能是 Tauri 作为非 MSIX 桌面应用调用 WinRT SpeechRecognizer 受限。"
            ),
            // SPERR_AUDIO_NOT_FOUND —— 无音频输入设备
            0x800455A1 => "未检测到音频输入设备，请检查麦克风是否已连接并启用。".to_string(),
            // E_ACCESSDENIED —— 麦克风权限被拒
            0x80070005 => "麦克风访问被拒绝，请在「设置 → 隐私和安全性 → 麦克风」中允许本应用访问麦克风。".to_string(),
            // ERROR_FILE_NOT_FOUND —— 语言包缺失
            0x80070002 => "未找到所选语言的语音识别语言包，请在「Windows 设置 → 时间和语言 → 语言」中安装对应语言的语音包。".to_string(),
            // REGDB_E_CLASSNOTREGISTERED —— Speech 运行时未注册
            0x80040154 => "Windows Speech 运行时未注册，请检查 Windows Speech 服务是否正常运行（services.msc）。".to_string(),
            // SPERR_AUDIO_ALREADY_STARTED —— 会话已在运行，非致命
            0x80045509 => "音频会话已在运行（非致命，可忽略）。".to_string(),
            _ => format!("未知 WinRT 语音错误。HRESULT={}。", hr_hex),
        };

        VivianError::Speech(format!(
            "WinRT {} 失败 [{}]：{}\n{}",
            context, hr_hex, msg, hint
        ))
    }

    #[async_trait]
    impl AsrEngine for WinrtBackend {
        async fn initialize(&mut self) -> VivianResult<bool> {
            if self.recognizer.is_some() {
                return Ok(true);
            }

            // 1. 创建识别器
            let recognizer = match self.create_recognizer() {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("WinRT 识别器创建失败: {e}");
                    self.available = false;
                    return Err(diagnose_winrt_error(&e, "识别器创建"));
                }
            };

            // 2. 编译约束
            if let Err(e) = self.compile_constraints(&recognizer) {
                tracing::error!("WinRT 编译约束失败: {e}");
                self.available = false;
                let _ = recognizer.Close();
                return Err(diagnose_winrt_error(&e, "约束编译"));
            }

            // 3. 获取持续识别会话
            let session = match recognizer.ContinuousRecognitionSession() {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("获取 ContinuousRecognitionSession 失败: {e}");
                    self.available = false;
                    let _ = recognizer.Close();
                    return Err(diagnose_winrt_error(&e, "获取持续识别会话"));
                }
            };

            // 4. 共享状态（事件发送器若未注入则用丢弃通道）
            let event_tx = self
                .event_tx
                .clone()
                .unwrap_or_else(|| broadcast::channel(1).0);
            let shared = Arc::new(WinrtShared {
                event_tx,
                last_speech_ms: AtomicU64::new(0),
                user_stop: AtomicBool::new(false),
                needs_restart: AtomicBool::new(false),
                is_running: AtomicBool::new(false),
                stop_watcher: AtomicBool::new(false),
            });

            // 5. 注册 ResultGenerated → 最终结果
            let shared_r = shared.clone();
            let result_token = session.ResultGenerated(&TypedEventHandler::<
                SpeechContinuousRecognitionSession,
                SpeechContinuousRecognitionResultGeneratedEventArgs,
            >::new(move |_sender, args| {
                // windows 0.61 闭包签名：args: Ref<'_, U>（Deref 到 U）
                let args = match args.as_ref() {
                    Some(a) => a,
                    None => return Ok(()),
                };
                shared_r.touch();
                let result = args.Result()?;
                if result.Status()? == SpeechRecognitionResultStatus::Success {
                    let text = result.Text()?.to_string();
                    if !text.is_empty() {
                        let conf = result.Confidence().map(map_confidence).unwrap_or(0.5);
                        let _ = shared_r
                            .event_tx
                            .send(AsrEvent::final_result(text, conf));
                    }
                }
                Ok(())
            }));
            let _result_token = match result_token {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::error!("注册 ResultGenerated 失败: {e}");
                    self.available = false;
                    let _ = recognizer.Close();
                    return Err(diagnose_winrt_error(&e, "注册 ResultGenerated 事件"));
                }
            };

            // 6. 注册 Completed → 自动重启 / 停止
            let shared_c = shared.clone();
            let completed_token = session.Completed(&TypedEventHandler::<
                SpeechContinuousRecognitionSession,
                SpeechContinuousRecognitionCompletedEventArgs,
            >::new(move |_sender, _args| {
                tracing::info!("WinRT 会话结束");
                shared_c.is_running.store(false, Ordering::SeqCst);
                if !shared_c.user_stop.load(Ordering::SeqCst) {
                    tracing::warn!("检测到非正常结束，标记需要自动重启");
                    shared_c.needs_restart.store(true, Ordering::SeqCst);
                }
                let _ = shared_c.event_tx.send(AsrEvent::Stopped);
                Ok(())
            }));
            let _completed_token = match completed_token {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::error!("注册 Completed 失败: {e}");
                    self.available = false;
                    let _ = recognizer.Close();
                    return Err(diagnose_winrt_error(&e, "注册 Completed 事件"));
                }
            };

            // 7. 注册 HypothesisGenerated → 部分结果
            let shared_h = shared.clone();
            let hypothesis_token = recognizer.HypothesisGenerated(&TypedEventHandler::<
                SpeechRecognizer,
                SpeechRecognitionHypothesisGeneratedEventArgs,
            >::new(move |_sender, args| {
                let args = match args.as_ref() {
                    Some(a) => a,
                    None => return Ok(()),
                };
                shared_h.touch();
                let hyp = args.Hypothesis()?;
                let text = hyp.Text()?.to_string();
                if !text.is_empty() {
                    let _ = shared_h.event_tx.send(AsrEvent::partial(text));
                }
                Ok(())
            }));
            let _hypothesis_token = match hypothesis_token {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::error!("注册 HypothesisGenerated 失败: {e}");
                    self.available = false;
                    let _ = recognizer.Close();
                    return Err(diagnose_winrt_error(&e, "注册 HypothesisGenerated 事件"));
                }
            };

            // 8. 启动静默监视 + 自动重启后台任务
            let watcher_session = session.clone();
            let watcher_shared = shared.clone();
            let silence_timeout_ms = self.config.silence_timeout_ms;
            self.watcher_handle = Some(tokio::spawn(async move {
                winrt_watcher_loop(watcher_session, watcher_shared, silence_timeout_ms).await;
            }));

            self.recognizer = Some(recognizer);
            self.session = Some(session);
            self.shared = Some(shared);
            tracing::info!("WinRT 语音引擎初始化完成: 语言={}", self.config.language);
            Ok(true)
        }

        async fn start_recording(&mut self) -> VivianResult<()> {
            if !self.available {
                return Err(VivianError::NotImplemented(
                    "WinRT 后端不可用".to_string(),
                ));
            }
            if self.is_running {
                return Ok(());
            }
            let shared = self.shared.as_ref().ok_or_else(|| {
                VivianError::Speech("WinRT 后端未初始化".to_string())
            })?;
            shared.user_stop.store(false, Ordering::SeqCst);
            shared.needs_restart.store(false, Ordering::SeqCst);
            shared.last_speech_ms.store(0, Ordering::SeqCst);

            let session = self.session.clone().ok_or_else(|| {
                VivianError::Speech("WinRT 会话未初始化".to_string())
            })?;
            match session.StartAsync() {
                Ok(action) => {
                    if let Err(e) = action.get() {
                        // 0x80045509 (SPERR_AUDIO_ALREADY_STARTED) 表示会话已运行，视为成功
                        let msg = format!("{e}");
                        if msg.contains("0x80045509") {
                            tracing::warn!("WinRT 会话已在运行: {e}");
                        } else {
                            return Err(VivianError::Speech(format!(
                                "StartAsync 失败: {e}"
                            )));
                        }
                    }
                    self.is_running = true;
                    shared.is_running.store(true, Ordering::SeqCst);
                    let _ = shared.event_tx.send(AsrEvent::Started);
                    tracing::info!("WinRT 持续识别已启动");
                }
                Err(e) => {
                    return Err(VivianError::Speech(format!(
                        "StartAsync 创建失败: {e}"
                    )));
                }
            }
            Ok(())
        }

        async fn stop_recording(&mut self) -> VivianResult<()> {
            let shared = match self.shared.as_ref() {
                Some(s) => s,
                None => {
                    self.is_running = false;
                    return Ok(());
                }
            };
            shared.user_stop.store(true, Ordering::SeqCst);
            shared.is_running.store(false, Ordering::SeqCst);
            self.is_running = false;

            if let Some(session) = self.session.as_ref() {
                if let Ok(action) = session.StopAsync() {
                    let _ = action.get();
                } else {
                    tracing::warn!("StopAsync 调用失败（可忽略）");
                }
            }
            let _ = shared.event_tx.send(AsrEvent::Stopped);
            tracing::info!("WinRT 持续识别已停止");
            Ok(())
        }

        async fn transcribe(&self, audio: &[f32]) -> VivianResult<String> {
            let _ = audio;
            Err(VivianError::NotImplemented(
                "WinRT 持续识别模式不直接转译音频缓冲（请通过事件回调获取结果）".to_string(),
            ))
        }

        fn is_available(&self) -> bool {
            self.available
        }

        fn supports_silence_detection(&self) -> bool {
            true
        }

        fn supports_partial_results(&self) -> bool {
            true
        }

        fn dispose(&mut self) {
            if let Some(shared) = self.shared.as_ref() {
                shared.stop_watcher.store(true, Ordering::SeqCst);
                shared.user_stop.store(true, Ordering::SeqCst);
                shared.is_running.store(false, Ordering::SeqCst);
            }
            if let Some(handle) = self.watcher_handle.take() {
                handle.abort();
            }
            // 注销事件回调
            if let Some(session) = self.session.as_ref() {
                if let Some(t) = self.result_token.take() {
                    let _ = session.RemoveResultGenerated(t);
                }
                if let Some(t) = self.completed_token.take() {
                    let _ = session.RemoveCompleted(t);
                }
            }
            if let Some(rec) = self.recognizer.as_ref() {
                if let Some(t) = self.hypothesis_token.take() {
                    let _ = rec.RemoveHypothesisGenerated(t);
                }
                let _ = rec.Close();
            }
            self.session = None;
            self.recognizer = None;
            self.shared = None;
            self.is_running = false;
            self.available = false;
        }

        fn backend_type(&self) -> AsrBackendType {
            AsrBackendType::Winrt
        }

        fn set_event_sender(&mut self, sender: broadcast::Sender<AsrEvent>) {
            self.event_tx = Some(sender);
        }
    }

    /// 静默监视 + 自动重启后台循环。
    ///
    /// 每 200ms 轮询：
    /// - 若运行中且超过 `silence_timeout_ms` 未收到语音 → 停止会话并广播 `Stopped`
    /// - 若 `needs_restart` 被置位（非用户停止的会话结束）→ 重新 `StartAsync`
    ///
    /// 内部 COM 调用使用 `.get()` 阻塞（短时操作，可接受）。
    async fn winrt_watcher_loop(
        session: SpeechContinuousRecognitionSession,
        shared: Arc<WinrtShared>,
        silence_timeout_ms: u64,
    ) {
        while !shared.stop_watcher.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(200)).await;

            // 静默自动停止
            if shared.is_running.load(Ordering::SeqCst)
                && !shared.user_stop.load(Ordering::SeqCst)
            {
                let last = shared.last_speech_ms.load(Ordering::SeqCst);
                if last > 0 {
                    let elapsed = now_ms().saturating_sub(last);
                    if elapsed > silence_timeout_ms {
                        tracing::info!("检测到静默 {}ms，自动停止识别", elapsed);
                        shared.user_stop.store(true, Ordering::SeqCst);
                        shared.is_running.store(false, Ordering::SeqCst);
                        if let Ok(action) = session.StopAsync() {
                            let _ = action.get();
                        }
                        let _ = shared.event_tx.send(AsrEvent::Stopped);
                    }
                }
            }

            // 非正常结束后自动重启
            if shared.needs_restart.swap(false, Ordering::SeqCst)
                && !shared.user_stop.load(Ordering::SeqCst)
            {
                tracing::info!("尝试自动重启 WinRT 会话");
                match session.StartAsync() {
                    Ok(action) => match action.get() {
                        Ok(()) => {
                            shared.is_running.store(true, Ordering::SeqCst);
                            let _ = shared.event_tx.send(AsrEvent::Started);
                            tracing::info!("WinRT 会话已自动重启");
                        }
                        Err(e) => {
                            let _ = shared
                                .event_tx
                                .send(AsrEvent::error(format!("自动重启失败: {e}")));
                        }
                    },
                    Err(e) => {
                        let _ = shared
                            .event_tx
                            .send(AsrEvent::error(format!("StartAsync 创建失败: {e}")));
                    }
                }
            }
        }
    }
}

// ===========================================================================
// 非 Windows 占位实现
// ===========================================================================

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct WinrtBackend {
        pub(crate) config: AsrConfig,
        pub(crate) mode: RecognitionMode,
        pub(crate) available: bool,
        pub(crate) is_running: bool,
    }

    impl WinrtBackend {
        pub(crate) fn new_inner(config: AsrConfig, mode: RecognitionMode) -> Self {
            Self {
                config,
                mode,
                available: true,
                is_running: false,
            }
        }
    }

    #[async_trait]
    impl AsrEngine for WinrtBackend {
        async fn initialize(&mut self) -> VivianResult<bool> {
            tracing::warn!("WinrtBackend 仅在 Windows 平台可用（当前为占位实现）");
            self.available = false;
            Ok(false)
        }

        async fn start_recording(&mut self) -> VivianResult<()> {
            Err(VivianError::NotImplemented(
                "WinRT 语音识别仅在 Windows 平台可用".to_string(),
            ))
        }

        async fn stop_recording(&mut self) -> VivianResult<()> {
            self.is_running = false;
            Ok(())
        }

        async fn transcribe(&self, audio: &[f32]) -> VivianResult<String> {
            let _ = audio;
            Err(VivianError::NotImplemented(
                "WinRT 语音识别仅在 Windows 平台可用".to_string(),
            ))
        }

        fn is_available(&self) -> bool {
            self.available
        }

        fn supports_silence_detection(&self) -> bool {
            true
        }

        fn supports_partial_results(&self) -> bool {
            true
        }

        fn dispose(&mut self) {
            self.is_running = false;
            self.available = false;
        }

        fn backend_type(&self) -> AsrBackendType {
            AsrBackendType::Winrt
        }

        fn set_event_sender(&mut self, _sender: broadcast::Sender<AsrEvent>) {}
    }
}

// ===========================================================================
// 公共 API（平台无关）
// ===========================================================================

pub use imp::WinrtBackend;

impl WinrtBackend {
    /// 从配置构造后端实例（默认持续识别模式）。
    pub fn from_config(config: AsrConfig) -> Self {
        Self::new_inner(config, RecognitionMode::Continuous)
    }

    /// 指定识别模式构造。
    pub fn with_mode(config: AsrConfig, mode: RecognitionMode) -> Self {
        Self::new_inner(config, mode)
    }

    /// 当前识别模式
    pub fn mode(&self) -> RecognitionMode {
        self.mode
    }

    /// 识别语言
    pub fn language(&self) -> &str {
        &self.config.language
    }

    /// 静默自动停止超时（ms）
    pub fn silence_timeout_ms(&self) -> u64 {
        self.config.silence_timeout_ms
    }
}

impl std::fmt::Debug for WinrtBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WinrtBackend")
            .field("language", &self.config.language)
            .field("mode", &self.mode)
            .field("silence_timeout_ms", &self.config.silence_timeout_ms)
            .field("available", &self.available)
            .field("is_running", &self.is_running)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_winrt_backend_from_config() {
        let backend = WinrtBackend::from_config(AsrConfig::default());
        assert_eq!(backend.mode(), RecognitionMode::Continuous);
        assert_eq!(backend.language(), "zh-CN");
        assert_eq!(backend.silence_timeout_ms(), 1500);
        assert!(backend.is_available());
        assert_eq!(backend.backend_type(), AsrBackendType::Winrt);
        assert!(backend.supports_silence_detection());
        assert!(backend.supports_partial_results());
    }

    #[test]
    fn test_winrt_backend_with_keyword_mode() {
        let backend = WinrtBackend::with_mode(AsrConfig::default(), RecognitionMode::Keyword);
        assert_eq!(backend.mode(), RecognitionMode::Keyword);
    }

    #[tokio::test]
    async fn test_winrt_backend_initialize_runs() {
        // 真实 Windows 环境可能成功（语音运行时可用）；CI/无麦克风或非 Windows 失败。
        // 仅验证状态一致性，不依赖硬件。
        let mut backend = WinrtBackend::from_config(AsrConfig::default());
        let result =
            tokio::time::timeout(Duration::from_secs(10), backend.initialize()).await;
        let ok = result.unwrap_or(Ok(false)).unwrap_or(false);
        assert_eq!(ok, backend.is_available());
        backend.dispose();
    }

    #[tokio::test]
    async fn test_winrt_backend_transcribe_not_implemented() {
        let backend = WinrtBackend::from_config(AsrConfig::default());
        let result = backend.transcribe(&[0.0; 100]).await;
        assert!(matches!(result, Err(VivianError::NotImplemented(_))));
    }
}
