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
    error: Option<String>,
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
        state.error = None;
        self.ready.store(true, Ordering::Relaxed);
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
    pub fn error(&self) -> Option<String> {
        self.credentials.read().error.clone()
    }
    pub async fn refresh_credentials(&self) {
        let Ok(_refresh) = self.refresh.try_lock() else {
            return;
        };
        let Some(app) = self.app.read().clone() else {
            return;
        };
        #[cfg(windows)]
        let result = crate::embedded_browser::native::automation::credentials(app).await;
        #[cfg(not(windows))]
        let result: Result<(), String> = {
            let _ = app;
            Err("内嵌浏览器需要 Windows WebView2".into())
        };
        #[cfg(windows)]
        match result {
            Ok(snapshot) => {
                let now = chrono::Utc::now().timestamp_millis() as u64;
                self.record(snapshot, now);
            }
            Err(error) => {
                let mut state = self.credentials.write();
                state.error = Some(error);
                state.platforms = None;
                state.x = None;
                state.reddit = None;
                self.ready.store(false, Ordering::Relaxed);
            }
        }
        #[cfg(not(windows))]
        if let Err(error) = result {
            self.credentials.write().error = Some(error);
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
        };
        bridge.record(snapshot(), 100);
        assert_eq!(
            bridge.reddit_cookie(),
            Some((100, "synthetic-reddit".into()))
        );
        bridge.record(snapshot(), 200);
        assert_eq!(bridge.reddit_cookie().unwrap().0, 100);
        bridge.record(
            Credentials {
                status: HashMap::from([("twitter".into(), false), ("reddit".into(), false)]),
                ..Default::default()
            },
            300,
        );
        assert!(bridge.x_cookie().is_none() && bridge.reddit_cookie().is_none());
        assert_eq!(bridge.platform_status().unwrap().0, 300);
    }
}
