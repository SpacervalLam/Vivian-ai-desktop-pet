//! 系统音乐感知 —— 通过 Windows SMTC（SystemMediaTransportControls）读取当前播放。
//!
//! 让 Vivian 知道用户正在听什么歌，可在对话中自然提及（"这首歌挺好听的"）。
//!
//! 设计原则：
//! - 只读：仅读取当前播放信息，不控制播放
//! - 失败即"不知道"：任何 SMTC 调用失败均返回 None
//! - 非 Windows 平台为空实现

use serde::{Deserialize, Serialize};

use crate::error::VivianResult;

/// 播放状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
    Changing,
    Closed,
}

impl PlaybackStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Changing => "changing",
            Self::Closed => "closed",
        }
    }
}

/// 音乐快照 —— 某一时刻系统正在播放的曲目信息
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicSnapshot {
    /// 曲名
    pub title: String,
    /// 艺术家
    pub artist: String,
    /// 专辑（可能为空）
    pub album: String,
    /// 播放状态
    pub status: PlaybackStatus,
    /// 播放来源应用（如 "Spotify"/"网易云音乐"，可能为空）
    pub source_app: String,
}

/// 音乐数据源 —— 通过 Windows SMTC 读取系统当前播放
pub struct MusicSource;

impl MusicSource {
    pub fn new() -> Self {
        Self
    }

    /// 获取当前系统播放的音乐信息。
    ///
    /// 无播放会话或任何错误均返回 `Ok(None)`（"不知道"）。
    /// 加 3 秒超时防止 SMTC 阻塞。
    #[cfg(windows)]
    pub async fn fetch(&self) -> VivianResult<Option<MusicSnapshot>> {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::task::spawn_blocking(fetch_music_blocking),
        )
        .await;

        match result {
            Ok(Ok(snap)) => Ok(snap),
            Ok(Err(e)) => {
                tracing::debug!("[MusicSource] spawn_blocking panic: {}", e);
                Ok(None)
            }
            Err(_) => {
                tracing::debug!("[MusicSource] SMTC 读取超时（3s）");
                Ok(None)
            }
        }
    }

    #[cfg(not(windows))]
    pub async fn fetch(&self) -> VivianResult<Option<MusicSnapshot>> {
        Ok(None)
    }
}

/// 阻塞式读取系统当前播放（需在 spawn_blocking 中调用）。
///
/// spawn_blocking 线程无 COM 初始化，需手动调用 CoInitializeEx。
/// windows 0.58 的 IAsyncOperation 未实现 Future，用阻塞 `.get()` 等待完成。
#[cfg(windows)]
fn fetch_music_blocking() -> Option<MusicSnapshot> {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let manager_op = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok()?;
    let manager = manager_op.get().ok()?;

    let session = manager.GetCurrentSession().ok()?;

    // 读取播放信息
    let status = session
        .GetPlaybackInfo()
        .ok()
        .and_then(|info| info.PlaybackStatus().ok())
        .map(|s| match s {
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing => PlaybackStatus::Playing,
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused => PlaybackStatus::Paused,
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped => PlaybackStatus::Stopped,
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Changing => PlaybackStatus::Changing,
            _ => PlaybackStatus::Closed,
        })
        .unwrap_or(PlaybackStatus::Closed);

    // 若状态为 Closed/Stopped，视为无有效播放
    if matches!(status, PlaybackStatus::Closed | PlaybackStatus::Stopped) {
        return None;
    }

    // 读取媒体属性（曲名/艺术家/专辑）
    let props_op = session.TryGetMediaPropertiesAsync().ok()?;
    let props = props_op.get().ok()?;
    let title = props.Title().unwrap_or_default().to_string();
    let artist = props.Artist().unwrap_or_default().to_string();
    let album = props.AlbumTitle().unwrap_or_default().to_string();

    // 读取来源应用名
    let source_app = session
        .SourceAppUserModelId()
        .unwrap_or_default()
        .to_string();

    Some(MusicSnapshot {
        title,
        artist,
        album,
        status,
        source_app,
    })
}

// ─── SMTC 事件订阅 ───────────────────────────────────────────────────────────

/// SMTC 事件守卫 —— 持有事件注册，Drop 时自动取消订阅。
#[cfg(windows)]
pub struct SmcEventGuard {
    manager: windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager,
    manager_token: i64,
    session: Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession>,
    session_status_token: Option<i64>,
    session_props_token: Option<i64>,
}

#[cfg(windows)]
impl Drop for SmcEventGuard {
    fn drop(&mut self) {
        let _ = self.manager.RemoveCurrentSessionChanged(self.manager_token);
        if let Some(session) = &self.session {
            if let Some(token) = self.session_status_token {
                let _ = session.RemovePlaybackInfoChanged(token);
            }
            if let Some(token) = self.session_props_token {
                let _ = session.RemoveMediaPropertiesChanged(token);
            }
        }
    }
}

/// 订阅 SMTC 事件（阻塞式，需在 spawn_blocking 中调用）。
///
/// 注册三类事件：会话切换、播放状态变化、媒体属性变化。
/// 任一事件触发时通过 Notify 通知异步侧。
#[cfg(windows)]
pub fn subscribe_smtc_events(
    notify: std::sync::Arc<tokio::sync::Notify>,
) -> Option<SmcEventGuard> {
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let manager_op = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok()?;
    let manager = manager_op.get().ok()?;

    let n = notify.clone();
    let manager_token = manager
        .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            n.notify_one();
            Ok(())
        }))
        .ok()?;

    let session = manager.GetCurrentSession().ok();
    let (session_status_token, session_props_token) = if let Some(s) = &session {
        let n1 = notify.clone();
        let n2 = notify.clone();
        let status_token = s
            .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                n1.notify_one();
                Ok(())
            }))
            .ok();
        let props_token = s
            .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                n2.notify_one();
                Ok(())
            }))
            .ok();
        (status_token, props_token)
    } else {
        (None, None)
    };

    Some(SmcEventGuard {
        manager,
        manager_token,
        session,
        session_status_token,
        session_props_token,
    })
}

#[cfg(not(windows))]
pub fn subscribe_smtc_events(
    _notify: std::sync::Arc<tokio::sync::Notify>,
) -> Option<()> {
    None
}

impl Default for MusicSource {
    fn default() -> Self {
        Self::new()
    }
}
