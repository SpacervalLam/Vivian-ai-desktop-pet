//! 系统音乐感知与控制 —— 通过 Windows SMTC（SystemMediaTransportControls）读写当前播放。
//!
//! 读取失败时返回 `Ok(None)`（无法确定播放则视为无）。
//! 控制时若指定来源应用且找不到匹配会话，返回 `Err`（不退化到系统当前会话）。
//! 非 Windows 平台为空实现。

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

/// 播放控制动作（经 SMTC 定向下发）
///
/// 可定向到具体来源应用的会话（媒体键只能作用于系统当前会话）；
/// 下发后读回快照做校验（如回报"已切到《xxx》"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackAction {
    /// 开始播放（对已暂停的会话恢复）
    Play,
    /// 暂停
    Pause,
    /// 在播放/暂停之间切换（先读当前状态再决定）
    PlayPause,
    /// 下一首
    Next,
    /// 上一首
    Previous,
}

impl PlaybackAction {
    /// 从模型传入的字符串解析（与 `media_control` 的 action 命名对齐）
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "play" => Some(Self::Play),
            "pause" => Some(Self::Pause),
            "play_pause" => Some(Self::PlayPause),
            "next_track" | "next" => Some(Self::Next),
            "previous_track" | "previous" | "prev" => Some(Self::Previous),
            _ => None,
        }
    }
}

impl MusicSource {
    pub fn new() -> Self {
        Self
    }

    /// 获取当前系统播放的音乐信息。
    ///
    /// 无播放会话或任何错误均返回 `Ok(None)`。加 3 秒超时防止 SMTC 阻塞。
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

    /// 向 SMTC 会话下发播放控制指令。
    ///
    /// `target_app` 为 `None` 时作用于系统当前会话；给定时按
    /// `SourceAppUserModelId` 子串匹配（大小写不敏感）选定会话，
    /// 匹配不到则返回 `Err`（不退化到系统当前会话）。
    ///
    /// 返回动作**之后**重新读取的快照，供调用方校验（如回报"已切到《xxx》"）。
    #[cfg(windows)]
    pub async fn control(
        &self,
        action: PlaybackAction,
        target_app: Option<String>,
    ) -> Result<Option<MusicSnapshot>, String> {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::task::spawn_blocking(move || control_blocking(action, target_app)),
        )
        .await;

        match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => Err(format!("控制任务执行失败: {}", e)),
            Err(_) => Err("SMTC 控制超时（5s）".to_string()),
        }
    }

    #[cfg(not(windows))]
    pub async fn control(
        &self,
        _action: PlaybackAction,
        _target_app: Option<String>,
    ) -> Result<Option<MusicSnapshot>, String> {
        Err("当前平台不支持媒体控制".to_string())
    }
}

/// 把 SMTC 播放状态映射为本地枚举
#[cfg(windows)]
fn map_status(
    s: windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus,
) -> PlaybackStatus {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as S;
    match s {
        S::Playing => PlaybackStatus::Playing,
        S::Paused => PlaybackStatus::Paused,
        S::Stopped => PlaybackStatus::Stopped,
        S::Changing => PlaybackStatus::Changing,
        _ => PlaybackStatus::Closed,
    }
}

/// 读取某个会话当前的播放信息；无有效播放（Closed/Stopped）返回 None
#[cfg(windows)]
fn snapshot_of(
    session: &windows::Media::Control::GlobalSystemMediaTransportControlsSession,
) -> Option<MusicSnapshot> {
    let status = session
        .GetPlaybackInfo()
        .ok()
        .and_then(|info| info.PlaybackStatus().ok())
        .map(map_status)
        .unwrap_or(PlaybackStatus::Closed);

    if matches!(status, PlaybackStatus::Closed | PlaybackStatus::Stopped) {
        return None;
    }

    let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    Some(MusicSnapshot {
        title: props.Title().unwrap_or_default().to_string(),
        artist: props.Artist().unwrap_or_default().to_string(),
        album: props.AlbumTitle().unwrap_or_default().to_string(),
        status,
        source_app: session
            .SourceAppUserModelId()
            .unwrap_or_default()
            .to_string(),
    })
}

/// 打开 SMTC 会话管理器（需在 spawn_blocking 中调用）
#[cfg(windows)]
fn open_manager() -> Option<windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager>
{
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let op = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok()?;
    op.get().ok()
}

/// 按来源应用名（子串、大小写不敏感）在全部会话中查找
#[cfg(windows)]
fn find_session(
    manager: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager,
    target: &str,
) -> Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession> {
    let sessions = manager.GetSessions().ok()?;
    let count = sessions.Size().ok()?;
    let needle = target.to_lowercase();
    for i in 0..count {
        let Ok(session) = sessions.GetAt(i) else {
            continue;
        };
        let app = session
            .SourceAppUserModelId()
            .unwrap_or_default()
            .to_string();
        if app.to_lowercase().contains(&needle) {
            return Some(session);
        }
    }
    None
}

/// 阻塞式读取系统当前播放（需在 spawn_blocking 中调用）。
///
/// spawn_blocking 线程无 COM 初始化，需手动调用 CoInitializeEx。
/// windows 0.58 的 IAsyncOperation 未实现 Future，用阻塞 `.get()` 等待完成。
#[cfg(windows)]
fn fetch_music_blocking() -> Option<MusicSnapshot> {
    let manager = open_manager()?;
    let session = manager.GetCurrentSession().ok()?;
    snapshot_of(&session)
}

/// 阻塞式下发播放控制（需在 spawn_blocking 中调用）。
///
/// 返回动作后重新读取的快照（可能为 None，如暂停后状态为 Paused 仍有效、
/// 但切歌瞬间会短暂读到 Closed——故 None 不算失败，成功与否只看 `Try*` 的回执）。
#[cfg(windows)]
fn control_blocking(
    action: PlaybackAction,
    target_app: Option<String>,
) -> Result<Option<MusicSnapshot>, String> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as S;

    let manager = open_manager().ok_or("无法获取 SMTC 会话管理器")?;

    let session = match &target_app {
        Some(app) => find_session(&manager, app).ok_or_else(|| {
            format!(
                "没有找到来源为「{}」的播放会话（该应用可能未在播放，或未注册到系统媒体控制）",
                app
            )
        })?,
        None => manager
            .GetCurrentSession()
            .map_err(|_| "当前没有活跃的播放会话".to_string())?,
    };

    let is_playing = || {
        session
            .GetPlaybackInfo()
            .ok()
            .and_then(|i| i.PlaybackStatus().ok())
            .map(|s| s == S::Playing)
            .unwrap_or(false)
    };

    let op = match action {
        PlaybackAction::Play => session.TryPlayAsync(),
        PlaybackAction::Pause => session.TryPauseAsync(),
        PlaybackAction::Next => session.TrySkipNextAsync(),
        PlaybackAction::Previous => session.TrySkipPreviousAsync(),
        PlaybackAction::PlayPause => {
            if is_playing() {
                session.TryPauseAsync()
            } else {
                session.TryPlayAsync()
            }
        }
    };

    let accepted = op
        .map_err(|e| format!("下发控制指令失败: {}", e))?
        .get()
        .map_err(|e| format!("等待控制结果失败: {}", e))?;

    if !accepted {
        return Err("播放器拒绝了该控制指令（可能不支持或当前状态不允许）".to_string());
    }

    Ok(snapshot_of(&session))
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
