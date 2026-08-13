//! World —— 真实世界感知层。
//!
//! 让 Vivian 感知真实世界：时间、节气、节日、日出日落、天气、季节。
//! 即使用户未交互，Vivian 也"知道外面在下雨""今天是中秋""太阳快落山了"。
//!
//! 设计原则：
//! - 本地计算优先（时间/节气/节日/日出日落 纯本地，无需网络）
//! - 天气失败即"不知道"，不做时间推断兜底
//! - WorldSnapshot 是轻量可克隆快照，由 WorldStateProvider 缓存与产出

pub mod activity_classifier;
pub mod activity_corpus;
pub mod entity_state;
pub mod events;
pub mod foreground_window;
pub mod geolocation;
pub mod music;
pub mod network_status;
pub mod network_watch;
pub mod state;
pub mod system_metrics;
pub mod time_perception;
pub mod user_behavior;
pub mod volume;
pub mod weather;

pub use entity_state::{
    ExpectationEngine, ExpectationSource, ExpectedReturn, ReturnClassification, ReturnEvent,
    UserActivity, UserEntitySnapshot, UserEntityState, UserPresence,
};
pub use state::WorldState;
pub use user_behavior::{
    BehaviorEndReason, SharedUserBehaviorLog, UserBehaviorEntry, UserBehaviorLog,
};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::config::WorldConfig;

pub use events::{WorldEvent, WorldEventDetector, WorldEventKind};
pub use foreground_window::{get_foreground_window, subscribe_foreground_events, ForegroundWindowSnapshot};
pub use music::{MusicSnapshot, MusicSource, PlaybackStatus};
pub use network_status::{get_network_status, NetworkStatusSnapshot};
pub use network_watch::subscribe_network_events;
pub use system_metrics::{SystemMetrics, SystemMetricsCollector};
pub use time_perception::{Festival, Season, SolarTerm, TimePerception};
pub use volume::{get_volume, subscribe_volume_events, VolumeSnapshot};
pub use weather::{WeatherSnapshot, WeatherSource};

/// 世界快照 —— 某一时刻 Vivian 感知到的真实世界
///
/// 所有字段都可序列化，便于注入 prompt 与记忆系统。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldSnapshot {
    /// UTC 时间戳（秒）
    pub timestamp: i64,
    /// 本地时间人类可读串（"2026-07-08 周三 14:30"）
    pub local_time: String,
    /// 本地小时（0-23）
    pub hour: u32,
    /// 星期几（0=周一 ... 6=周日）
    pub weekday: u32,
    pub is_weekend: bool,
    pub season: Season,
    /// 当前节气（若当日处于某节气期间）
    pub solar_term: Option<SolarTerm>,
    /// 今日节日
    pub festival: Option<Festival>,
    /// 日出日落（若配置了经纬度）
    pub sunrise_sunset: Option<SunriseSunset>,
    /// 地理位置（城市/坐标，由定位服务检测或手动配置）
    pub location: Option<LocationSnapshot>,
    /// 天气快照（可能因未配置/获取失败而为 None）
    pub weather: Option<WeatherSnapshot>,
    /// 当前播放的音乐（可能因无播放/获取失败而为 None）
    pub music: Option<MusicSnapshot>,
    /// 系统硬件指标（CPU/内存/网速）
    pub system: Option<SystemMetrics>,
    /// 系统主音量与静音状态
    pub volume: Option<VolumeSnapshot>,
    /// 网络连接状态
    pub network_status: Option<NetworkStatusSnapshot>,
    /// 前台窗口（用户当前正在使用的应用）
    pub foreground_window: Option<ForegroundWindowSnapshot>,
    /// 用户实体在场状态（Present/Away）
    pub user_presence: Option<UserEntitySnapshot>,
    /// 距上次用户交互的秒数（None 表示未知）
    pub seconds_since_last_interaction: Option<f64>,
}

/// 地理位置快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationSnapshot {
    pub latitude: f64,
    pub longitude: f64,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
}

/// 日出日落
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SunriseSunset {
    /// 日出本地小时（如 5.7 表示 5:42）
    pub sunrise_hour: f64,
    /// 日落本地小时
    pub sunset_hour: f64,
    /// 当前是否为白天
    pub is_daytime: bool,
}

impl SunriseSunset {
    pub fn sunrise_str(&self) -> String {
        format_hhmm(self.sunrise_hour)
    }
    pub fn sunset_str(&self) -> String {
        format_hhmm(self.sunset_hour)
    }
}

fn format_hhmm(hour: f64) -> String {
    let h = hour.floor() as u32;
    let m = ((hour - h as f64) * 60.0).round() as u32;
    format!("{:02}:{:02}", h, m)
}

/// 世界状态提供者 —— 组合时间感知 + 天气源，产出 WorldSnapshot
///
/// 内部缓存天气（TTL 由配置决定，默认 1 小时），时间感知纯本地实时计算。
pub struct WorldStateProvider {
    config: RwLock<WorldConfig>,
    weather_source: RwLock<Option<Arc<WeatherSource>>>,
    /// 缓存的世界快照（每次 snapshot() 时若过期则刷新天气）
    cached_weather: RwLock<Option<WeatherSnapshot>>,
    /// 音乐数据源（SMTC 读取系统当前播放）
    music_source: RwLock<Option<Arc<MusicSource>>>,
    /// 缓存的音乐快照
    cached_music: RwLock<Option<MusicSnapshot>>,
    /// 音乐轮询循环是否已启动（幂等保护）
    music_polling_started: AtomicBool,
    /// 系统指标采集器（Mutex 保护可变 sysinfo 句柄）
    system_collector: parking_lot::Mutex<SystemMetricsCollector>,
    /// 缓存的系统指标快照
    cached_system: RwLock<Option<SystemMetrics>>,
    /// 系统指标轮询循环是否已启动（幂等保护）
    system_polling_started: AtomicBool,
    /// 缓存的地理位置快照
    cached_location: RwLock<Option<LocationSnapshot>>,
    /// 缓存的音量快照
    cached_volume: RwLock<Option<VolumeSnapshot>>,
    /// 缓存的网络连接状态快照
    cached_network_status: RwLock<Option<NetworkStatusSnapshot>>,
    /// 缓存的前台窗口快照
    cached_foreground_window: RwLock<Option<ForegroundWindowSnapshot>>,
    /// 前台窗口切换监听器列表（由各角色 ActivityJournal 注入，仅当 title 变化时回调全部）
    foreground_listeners: RwLock<Vec<ForegroundListener>>,
    /// 缓存的用户在场状态快照
    cached_user_presence: RwLock<Option<UserEntitySnapshot>>,
}

/// 前台窗口切换监听器回调签名
pub type ForegroundListener = Arc<dyn Fn(&ForegroundWindowSnapshot) + Send + Sync>;

impl WorldStateProvider {
    pub fn new(config: WorldConfig) -> Self {
        Self {
            config: RwLock::new(config),
            weather_source: RwLock::new(None),
            cached_weather: RwLock::new(None),
            music_source: RwLock::new(None),
            cached_music: RwLock::new(None),
            music_polling_started: AtomicBool::new(false),
            system_collector: parking_lot::Mutex::new(SystemMetricsCollector::new()),
            cached_system: RwLock::new(None),
            system_polling_started: AtomicBool::new(false),
            cached_location: RwLock::new(None),
            cached_volume: RwLock::new(None),
            cached_network_status: RwLock::new(None),
            cached_foreground_window: RwLock::new(None),
            foreground_listeners: RwLock::new(Vec::new()),
            cached_user_presence: RwLock::new(None),
        }
    }

    /// 注入天气源（由 lib.rs 初始化时调用）
    pub fn set_weather_source(&self, source: Arc<WeatherSource>) {
        *self.weather_source.write() = Some(source);
    }

    /// 检查是否已注入天气源
    pub fn has_weather_source(&self) -> bool {
        self.weather_source.read().is_some()
    }

    /// 注入音乐源（由 lib.rs 初始化时调用）
    pub fn set_music_source(&self, source: Arc<MusicSource>) {
        *self.music_source.write() = Some(source);
    }

    /// 检查是否已注入音乐源
    pub fn has_music_source(&self) -> bool {
        self.music_source.read().is_some()
    }

    /// 追加前台窗口切换监听器（由各角色 Brain 初始化时调用）
    ///
    /// 全局共享的 WorldStateProvider 支持多个监听器，每个角色的 ActivityJournal
    /// 都会收到 title 变化事件，独立记录到各自的 user_behaviors.json。
    /// 回调应保持轻量（避免阻塞事件循环）。
    pub fn add_foreground_listener(&self, listener: ForegroundListener) {
        self.foreground_listeners.write().push(listener);
    }

    /// 更新配置（设置窗口保存后调用）
    pub fn update_config(&self, config: WorldConfig) {
        *self.config.write() = config;
    }

    /// 读取当前配置快照
    pub fn config(&self) -> WorldConfig {
        self.config.read().clone()
    }

    /// 同步产出快照（不刷新天气，使用缓存或 None）
    pub fn snapshot(&self, seconds_since_last_interaction: Option<f64>) -> WorldSnapshot {
        let now = chrono::Local::now();
        let config = self.config.read().clone();

        let tp = TimePerception::at(&now, &config);

        let sunrise_sunset = if config.latitude.is_some() && config.longitude.is_some() {
            let lat = config.latitude.unwrap();
            let lon = config.longitude.unwrap();
            time_perception::compute_sunrise_sunset(lat, lon, &now)
        } else {
            None
        };

        let weather = if config.enable_weather {
            self.cached_weather.read().clone()
        } else {
            None
        };

        let music = self.cached_music.read().clone();
        let system = self.cached_system.read().clone();
        let location = self.cached_location.read().clone().or_else(|| {
            if config.latitude.is_some() && config.longitude.is_some() {
                Some(LocationSnapshot {
                    latitude: config.latitude.unwrap(),
                    longitude: config.longitude.unwrap(),
                    city: config.city.clone(),
                    region: config.region.clone(),
                    country: config.country.clone(),
                })
            } else {
                None
            }
        });
        let volume = self.cached_volume.read().clone();
        let network_status = self.cached_network_status.read().clone();
        let foreground_window = self.cached_foreground_window.read().clone();
        let user_presence = self.cached_user_presence.read().clone();

        WorldSnapshot {
            timestamp: now.timestamp(),
            local_time: tp.local_time_str(),
            hour: tp.hour(),
            weekday: tp.weekday(),
            is_weekend: tp.is_weekend(),
            season: tp.season(),
            solar_term: tp.solar_term(),
            festival: tp.festival(),
            sunrise_sunset,
            location,
            weather,
            music,
            system,
            volume,
            network_status,
            foreground_window,
            user_presence,
            seconds_since_last_interaction,
        }
    }

    /// 异步刷新天气缓存（由后台定时调用）
    pub async fn refresh_weather(&self) {
        let config = self.config.read().clone();
        if !config.enable_weather {
            tracing::debug!("[WorldStateProvider] 天气功能未启用，跳过刷新");
            return;
        }
        let lat = match config.latitude {
            Some(v) => v,
            None => {
                tracing::warn!("[WorldStateProvider] 纬度未配置，跳过天气刷新");
                return;
            }
        };
        let lon = match config.longitude {
            Some(v) => v,
            None => {
                tracing::warn!("[WorldStateProvider] 经度未配置，跳过天气刷新");
                return;
            }
        };

        // 检查缓存是否仍在有效期内
        {
            let cached = self.cached_weather.read();
            if let Some(w) = cached.as_ref() {
                let age = chrono::Utc::now().timestamp() - w.cached_at;
                if age < config.weather_cache_ttl_secs as i64 {
                    tracing::debug!(
                        "[WorldStateProvider] 天气缓存仍有效（年龄 {}s < TTL {}s），跳过刷新",
                        age,
                        config.weather_cache_ttl_secs
                    );
                    return;
                }
            }
        }

        let source = self.weather_source.read().clone();
        if let Some(src) = source {
            tracing::info!(
                "[WorldStateProvider] 开始刷新天气缓存（经纬度: {}, {}）",
                lat,
                lon
            );
            match src.fetch(lat, lon).await {
                Ok(w) => {
                    tracing::info!(
                        "[WorldStateProvider] 天气刷新成功: {} {}°C",
                        w.description,
                        w.temperature
                    );
                    *self.cached_weather.write() = Some(w);
                }
                Err(e) => {
                    tracing::warn!("天气获取失败，保持未知状态: {}", e);
                    // 失败即"不知道"，不更新缓存（保留旧数据或 None）
                    // 但若旧数据过旧，清空以免一直用过期数据
                    let stale = {
                        let cached = self.cached_weather.read();
                        cached.as_ref().map_or(false, |w| {
                            chrono::Utc::now().timestamp() - w.cached_at
                                > (config.weather_cache_ttl_secs as i64) * 3
                        })
                    };
                    if stale {
                        tracing::warn!("[WorldStateProvider] 旧天气缓存已过期 3 倍 TTL，清空");
                        *self.cached_weather.write() = None;
                    }
                }
            }
        } else {
            tracing::warn!("[WorldStateProvider] 天气源未注入，无法刷新天气");
        }
    }

    /// 强制清空天气缓存（用户关闭天气功能时调用）
    pub fn clear_weather(&self) {
        *self.cached_weather.write() = None;
    }

    /// 异步刷新音乐缓存（由后台定时调用，读取系统当前播放）
    ///
    /// 无播放或读取失败时缓存为 None（"不知道"）。
    pub async fn refresh_music(&self) {
        let source = self.music_source.read().clone();
        if let Some(src) = source {
            match src.fetch().await {
                Ok(snap) => *self.cached_music.write() = snap,
                Err(e) => {
                    tracing::debug!("[WorldStateProvider] 音乐感知失败，保持未知: {}", e);
                }
            }
        }
    }

    /// 启动后台音乐感知循环（事件驱动 + 30s 兜底）。
    ///
    /// 订阅 Windows SMTC 事件（会话切换/播放状态/媒体属性变化），
    /// 事件触发时立即刷新；无事件时 30s 兜底刷新一次。
    /// 事件订阅失败则退化为 10s 轮询。
    /// 由 `AtomicBool` 保证幂等：多次调用只启动一个循环。
    pub fn start_music_polling(self: &Arc<Self>) {
        if self
            .music_polling_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let source = self.music_source.read().clone();
        if source.is_none() {
            self.music_polling_started
                .store(false, Ordering::SeqCst);
            return;
        }

        let provider = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tracing::info!("[MusicWatch] 后台音乐事件监听已启动");
            let cancel = crate::utils::cancel_token::cancel_token();
            loop {
                if cancel.is_cancelled() {
                    tracing::info!("[MusicWatch] 收到取消信号，退出");
                    return;
                }
                let notify = std::sync::Arc::new(tokio::sync::Notify::new());

                let guard = tokio::task::spawn_blocking({
                    let n = notify.clone();
                    move || crate::world::music::subscribe_smtc_events(n)
                })
                .await
                .unwrap_or(None);

                if guard.is_some() {
                    tokio::select! {
                        _ = notify.notified() => {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                        _ = cancel.cancelled() => return,
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {}
                        _ = cancel.cancelled() => return,
                    }
                }

                provider.refresh_music().await;
            }
        });
    }

    /// 刷新系统指标缓存（同步，内部持锁采集）
    pub fn refresh_system(&self) {
        let metrics = self.system_collector.lock().refresh();
        *self.cached_system.write() = Some(metrics);
    }

    /// 设置地理位置缓存（由定位命令调用）
    pub fn set_location(&self, loc: LocationSnapshot) {
        *self.cached_location.write() = Some(loc);
    }

    /// 设置用户在场状态缓存（由 Brain 的 proactive tick 调用）
    pub fn set_user_presence(&self, presence: UserEntitySnapshot) {
        *self.cached_user_presence.write() = Some(presence);
    }

    /// 启动后台系统指标轮询循环（CPU/内存/网速，每 10 秒）。
    ///
    /// 感知层（音量/前台窗口/网络状态）已改为事件驱动，
    /// 分别由 start_volume_events / start_foreground_events / start_network_events 管理。
    /// 由 `AtomicBool` 保证幂等：多次调用只启动一个循环。
    pub fn start_system_polling(self: &Arc<Self>) {
        if self
            .system_polling_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let provider = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tracing::info!("[SystemPolling] 后台系统指标轮询已启动（10s）");
            provider.refresh_system();
            let cancel = crate::utils::cancel_token::cancel_token();
            loop {
                if cancel.is_cancelled() {
                    tracing::info!("[SystemPolling] 收到取消信号，退出");
                    return;
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {}
                    _ = cancel.cancelled() => return,
                }
                provider.refresh_system();
            }
        });
    }

    /// 启动音量事件监听循环（IAudioEndpointVolumeCallback + 30s 兜底）。
    pub fn start_volume_events(self: &Arc<Self>) {
        let provider = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tracing::info!("[VolumeWatch] 音量事件监听已启动");
            // 初始刷新
            let vol = tokio::task::spawn_blocking(get_volume).await.unwrap_or_default();
            *provider.cached_volume.write() = Some(vol);

            let cancel = crate::utils::cancel_token::cancel_token();
            loop {
                if cancel.is_cancelled() {
                    tracing::info!("[VolumeWatch] 收到取消信号，退出");
                    return;
                }
                let notify = std::sync::Arc::new(tokio::sync::Notify::new());
                let guard = tokio::task::spawn_blocking({
                    let n = notify.clone();
                    move || subscribe_volume_events(n)
                })
                .await
                .unwrap_or(None);

                if guard.is_some() {
                    tokio::select! {
                        _ = notify.notified() => {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                        _ = cancel.cancelled() => return,
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {}
                        _ = cancel.cancelled() => return,
                    }
                }

                let vol = tokio::task::spawn_blocking(get_volume).await.unwrap_or_default();
                *provider.cached_volume.write() = Some(vol);
            }
        });
    }

    /// 启动前台窗口事件监听循环（SetWinEventHook + 10s 兜底）。
    pub fn start_foreground_events(self: &Arc<Self>) {
        let provider = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tracing::info!("[ForegroundWatch] 前台窗口事件监听已启动");
            // 初始刷新
            let fw = get_foreground_window();
            provider.update_foreground(fw);

            let cancel = crate::utils::cancel_token::cancel_token();
            loop {
                if cancel.is_cancelled() {
                    tracing::info!("[ForegroundWatch] 收到取消信号，退出");
                    return;
                }
                let notify = std::sync::Arc::new(tokio::sync::Notify::new());
                let guard = tokio::task::spawn_blocking({
                    let n = notify.clone();
                    move || subscribe_foreground_events(n)
                })
                .await
                .unwrap_or(None);

                if guard.is_some() {
                    tokio::select! {
                        _ = notify.notified() => {
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {}
                        _ = cancel.cancelled() => return,
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                        _ = cancel.cancelled() => return,
                    }
                }

                let fw = get_foreground_window();
                provider.update_foreground(fw);
            }
        });
    }

    /// 更新缓存的前台窗口快照，并在 title 变化时通知监听器。
    ///
    /// - pid 为 0（无前台窗口/获取失败）时跳过
    /// - 与缓存 title 不同时回调 foreground_listener（驱动 ActivityJournal 记录）
    fn update_foreground(&self, fw: ForegroundWindowSnapshot) {
        if fw.pid == 0 {
            return;
        }
        let title_changed = {
            let cached = self.cached_foreground_window.read();
            match cached.as_ref() {
                Some(prev) => prev.title != fw.title,
                None => true,
            }
        };
        *self.cached_foreground_window.write() = Some(fw.clone());
        if title_changed {
            let listeners = self.foreground_listeners.read().clone();
            for listener in &listeners {
                listener(&fw);
            }
        }
    }

    /// 启动网络状态事件监听循环（INetworkListManagerEvents + 60s 兜底）。
    pub fn start_network_events(self: &Arc<Self>) {
        let provider = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tracing::info!("[NetworkWatch] 网络状态事件监听已启动");
            // 初始刷新
            let ns = tokio::task::spawn_blocking(get_network_status).await.unwrap_or_default();
            *provider.cached_network_status.write() = Some(ns);

            let cancel = crate::utils::cancel_token::cancel_token();
            loop {
                if cancel.is_cancelled() {
                    tracing::info!("[NetworkWatch] 收到取消信号，退出");
                    return;
                }
                let notify = std::sync::Arc::new(tokio::sync::Notify::new());
                let guard = tokio::task::spawn_blocking({
                    let n = notify.clone();
                    move || subscribe_network_events(n)
                })
                .await
                .unwrap_or(None);

                if guard.is_some() {
                    tokio::select! {
                        _ = notify.notified() => {
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
                        _ = cancel.cancelled() => return,
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                        _ = cancel.cancelled() => return,
                    }
                }

                let ns = tokio::task::spawn_blocking(get_network_status).await.unwrap_or_default();
                *provider.cached_network_status.write() = Some(ns);
            }
        });
    }
}

impl Default for WorldStateProvider {
    fn default() -> Self {
        Self::new(WorldConfig::default())
    }
}
