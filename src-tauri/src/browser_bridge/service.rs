//! Compatibility facade for browser tools and discovery, backed by the in-process WebView2 profile.
use parking_lot::RwLock;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub struct BridgeState {
    app: RwLock<Option<tauri::AppHandle>>,
    ready: AtomicBool,
    credentials: RwLock<SessionState>,
    refresh: tokio::sync::Mutex<()>,
}
#[derive(Default)]
struct SessionState {
    platforms: Option<(u64, HashMap<String, bool>)>,
    x: Option<String>,
    reddit: Option<(u64, String)>,
    bilibili: Option<(u64, String)>,
    error: Option<String>,
    credential_error: Option<String>,
}
impl BridgeState {
    #[cfg(windows)]
    fn record(&self, snapshot: crate::embedded_browser::native::automation::Credentials, now: u64) {
        let mut state = self.credentials.write();
        state.platforms = Some((now, snapshot.status));
        state.x = snapshot.x;
        if state.reddit.as_ref().map(|(_, cookie)| cookie) != snapshot.reddit.as_ref() {
            state.reddit = snapshot.reddit.map(|cookie| (now, cookie));
        }
        if state
            .bilibili
            .as_ref()
            .map(|(_, cookie)| cookie)
            != snapshot.bilibili.as_ref()
        {
            state.bilibili = snapshot.bilibili.map(|cookie| (now, cookie));
        }
        state.credential_error = None;
    }
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            app: RwLock::new(None),
            ready: AtomicBool::new(false),
            credentials: RwLock::new(SessionState::default()),
            refresh: tokio::sync::Mutex::new(()),
        })
    }
    pub fn start(self: &Arc<Self>, app: tauri::AppHandle) {
        *self.app.write() = Some(app);
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                service.refresh_credentials().await;
            }
        });
    }
    pub fn is_connected(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }
    pub fn platform_status(&self) -> Option<(u64, HashMap<String, bool>)> {
        self.credentials.read().platforms.clone()
    }
    pub fn x_cookie(&self) -> Option<String> {
        self.credentials.read().x.clone()
    }
    pub fn reddit_cookie(&self) -> Option<(u64, String)> {
        self.credentials.read().reddit.clone()
    }
    pub fn bilibili_cookie(&self) -> Option<(u64, String)> {
        self.credentials.read().bilibili.clone()
    }
    pub fn error(&self) -> Option<String> {
        self.credentials.read().error.clone()
    }
    pub fn credential_error(&self) -> Option<String> {
        self.credentials.read().credential_error.clone()
    }
    /// Keep the last display snapshot, but revoke credentials consumed by background jobs.
    fn mark_credentials_failed(&self, error: String) {
        let mut state = self.credentials.write();
        state.x = None;
        state.reddit = None;
        state.bilibili = None;
        state.credential_error = Some(error);
    }
    /// 浏览器不可用时回到未连接状态：不保留陈旧的登录态或错误信息。
    fn mark_disconnected(&self, error: Option<String>) {
        let mut state = self.credentials.write();
        state.platforms = None;
        state.x = None;
        state.reddit = None;
        state.bilibili = None;
        state.error = error;
        state.credential_error = None;
        self.ready.store(false, Ordering::Relaxed);
    }
    pub async fn refresh_credentials(&self) {
        let Ok(_refresh) = self.refresh.try_lock() else {
            return;
        };
        let Some(app) = self.app.read().clone() else {
            return;
        };
        #[cfg(windows)]
        match crate::embedded_browser::native::automation::engine_ready(&app).await {
            Ok(true) => {
                self.ready.store(true, Ordering::Relaxed);
                self.credentials.write().error = None;
            }
            Ok(false) => { self.mark_disconnected(None); return; }
            Err(error) => { self.mark_disconnected(Some(error)); return; }
        }
        #[cfg(windows)]
        let result = crate::embedded_browser::native::automation::credentials(app.clone()).await;
        #[cfg(not(windows))]
        let result: Result<Option<()>, String> = {
            let _ = app;
            Err("内嵌浏览器需要 Windows WebView2".into())
        };
        #[cfg(windows)]
        match result {
            Ok(Some(snapshot)) => {
                let now = chrono::Utc::now().timestamp_millis() as u64;
                self.record(snapshot, now);
            }
            result => {
                // A controller may close while credentials are read. Recheck the engine
                // before deciding whether this is a credential failure or a disconnection.
                match crate::embedded_browser::native::automation::engine_ready(&app).await {
                    Ok(true) => self.mark_credentials_failed(match result {
                        Err(error) => error,
                        _ => "普通浏览配置尚未就绪，正在重试凭据检测".into(),
                    }),
                    Ok(false) => self.mark_disconnected(None),
                    Err(error) => self.mark_disconnected(Some(error)),
                }
            }
        }
        #[cfg(not(windows))]
        if let Err(error) = result {
            self.mark_disconnected(Some(error));
        }
    }
    pub async fn request_tool(
        &self,
        name: &str,
        args: &Value,
        timeout: Duration,
        session: Option<&str>,
    ) -> Result<String, String> {
        let app = self.app.read().clone().ok_or("内嵌浏览器尚未初始化")?;
        #[cfg(windows)]
        {
            crate::embedded_browser::native::automation::tool(
                app,
                name.into(),
                args.clone(),
                timeout,
                session.unwrap_or("default").into(),
            )
            .await
        }
        #[cfg(not(windows))]
        {
            let _ = (app, name, args, timeout, session);
            Err("内嵌浏览器需要 Windows WebView2".into())
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::embedded_browser::native::automation::Credentials;
    #[test]
    fn credentials_refresh_without_rewriting_unchanged_cli_cookie_and_logout_revokes_memory() {
        let bridge = BridgeState::new();
        let snapshot = || Credentials {
            status: HashMap::from([("twitter".into(), true), ("reddit".into(), true)]),
            x: Some("synthetic-x".into()),
            reddit: Some("synthetic-reddit".into()),
            bilibili: Some("SESSDATA=synthetic".into()),
        };
        bridge.record(snapshot(), 100);
        assert!(!bridge.is_connected(), "credential reads do not establish engine readiness");
        assert_eq!(
            bridge.reddit_cookie(),
            Some((100, "synthetic-reddit".into()))
        );
        assert_eq!(
            bridge.bilibili_cookie(),
            Some((100, "SESSDATA=synthetic".into()))
        );
        bridge.record(snapshot(), 200);
        assert_eq!(bridge.reddit_cookie().unwrap().0, 100);
        assert_eq!(bridge.bilibili_cookie().unwrap().0, 100);
        bridge.record(
            Credentials {
                status: HashMap::from([("twitter".into(), false), ("reddit".into(), false)]),
                ..Default::default()
            },
            300,
        );
        assert!(bridge.x_cookie().is_none() && bridge.reddit_cookie().is_none());
        assert!(bridge.bilibili_cookie().is_none());
        assert_eq!(bridge.platform_status().unwrap().0, 300);
    }
    #[test]
    fn credential_failure_preserves_display_but_revokes_background_credentials() {
        let bridge = BridgeState::new();
        bridge.ready.store(true, Ordering::Relaxed);
        bridge.record(Credentials {
            status: HashMap::from([("bilibili".into(), true)]),
            x: Some("synthetic-x".into()),
            reddit: Some("synthetic-reddit".into()),
            bilibili: Some("synthetic-bilibili".into()),
        }, 100);
        bridge.mark_credentials_failed("timeout".into());
        assert!(bridge.is_connected());
        assert_eq!(bridge.platform_status().unwrap().0, 100);
        assert_eq!(bridge.credential_error().as_deref(), Some("timeout"));
        assert!(bridge.error().is_none());
        assert!(bridge.x_cookie().is_none() && bridge.reddit_cookie().is_none());
        assert!(bridge.bilibili_cookie().is_none());
        bridge.record(Credentials::default(), 200);
        assert!(bridge.credential_error().is_none());
        assert_eq!(bridge.platform_status().unwrap().0, 200);
    }
    #[test]
    fn unavailable_browser_reports_disconnected_without_error_or_stale_credentials() {
        let bridge = BridgeState::new();
        bridge.record(
            Credentials {
                status: HashMap::from([("bilibili".into(), true)]),
                x: Some("synthetic-x".into()),
                reddit: Some("synthetic-reddit".into()),
                bilibili: Some("SESSDATA=synthetic".into()),
            },
            100,
        );
        bridge.ready.store(true, Ordering::Relaxed);
        assert!(bridge.is_connected());
        bridge.mark_disconnected(None);
        assert!(!bridge.is_connected());
        assert!(bridge.platform_status().is_none());
        assert!(bridge.error().is_none());
        assert!(bridge.x_cookie().is_none() && bridge.reddit_cookie().is_none());
        assert!(bridge.bilibili_cookie().is_none());
    }
}
