//! Settings entry points for the embedded browser. No external browser or extension is launched.
use crate::state::AppState;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Fetch public site artwork through the same resolver as quick-launch shortcuts.
/// Only the origin is used: page paths, query strings and browser cookies stay local.
#[tauri::command]
pub async fn browser_site_icon(
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<String, String> {
    let parsed = tauri::Url::parse(&url).map_err(|_| "网址无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("仅支持不含账号密码的 HTTP(S) 网址".into());
    }
    let proxy_url = {
        let config = state.config.read();
        crate::network::proxy::ProxyConfig::from_app_config(&config.get_all()).effective_proxy_url()
    };
    super::desktop_assistant::shortcut_icons::website(
        &parsed.origin().ascii_serialization(),
        proxy_url.as_deref(),
    )
    .await
}

#[derive(Serialize)]
pub struct PlatformStatusView {
    pub platform: String,
    pub logged_in: bool,
}
#[derive(Serialize)]
pub struct BrowserStatus {
    pub connected: bool,
    pub platforms: Vec<PlatformStatusView>,
    pub reported_at_ms: u64,
    pub error: Option<String>,
    pub credential_error: Option<String>,
}

#[tauri::command]
pub fn get_browser_platforms(state: State<'_, Arc<AppState>>) -> BrowserStatus {
    let bridge = &state.browser_bridge;
    let (reported_at_ms, platforms) = bridge
        .platform_status()
        .map(|(ts, status)| {
            let mut platforms = status
                .into_iter()
                .map(|(platform, logged_in)| PlatformStatusView {
                    platform,
                    logged_in,
                })
                .collect::<Vec<_>>();
            platforms.sort_by(|a, b| a.platform.cmp(&b.platform));
            (ts, platforms)
        })
        .unwrap_or_default();
    BrowserStatus {
        connected: bridge.is_connected(),
        platforms,
        reported_at_ms,
        error: bridge.error(),
        credential_error: bridge.credential_error(),
    }
}

#[tauri::command]
pub async fn open_embedded_browser(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    url: Option<String>,
) -> Result<(), String> {
    open_browser_page(app, &state, url).await
}

pub(crate) fn browser_page_url(input: &str) -> Result<String, String> {
    let parsed = tauri::Url::parse(input.trim()).map_err(|_| "网址无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none()
        || !parsed.username().is_empty() || parsed.password().is_some()
    {
        return Err("仅支持不含账号密码的 HTTP(S) 网址".into());
    }
    Ok(parsed.into())
}

/// Shared by desktop actions and settings: reveal the workspace, open/select its tab,
/// then repeat the navigation event so a freshly mounted inspector also sees it.
pub(crate) async fn open_browser_page(app: AppHandle, state: &AppState, url: Option<String>) -> Result<(), String> {
    let url = url.as_deref().map(browser_page_url).transpose()?;
    reveal_browser(&app).await?;
    if let Some(url) = url {
        state.browser_bridge.request_tool(
            "browser_open_tab", &serde_json::json!({"url":url}),
            std::time::Duration::from_secs(20), Some("desktop-open"),
        ).await?;
    }
    reveal_browser(&app).await
}

async fn reveal_browser(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window("memory").is_none() {
        super::window::open_child_window(
            app.clone(), "memory".into(), "index.html?view=memory&nav=code&browser=1".into(),
            "Vivian".into(), 1280, 850,
        ).await?;
    }
    if let Some(window) = app.get_webview_window("memory") {
        super::window::thaw_webview(&window);
        window.unminimize().map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        // tao posts show/restore to its UI queue. Focusing before those flags update
        // silently does nothing, particularly when restoring a minimized inspector.
        for _ in 0..12 {
            if window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(true) { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        window.set_focus().map_err(|e| e.to_string())?;
    }
    app.emit_to("memory", "memory:navigate", serde_json::json!({
        "page":"code", "browserRequest":chrono::Utc::now().timestamp_millis()
    })).map_err(|e| e.to_string())
}

#[cfg(test)]
mod browser_page_tests {
    use super::browser_page_url;
    #[test]
    fn page_urls_are_normalized_before_opening_either_browser() {
        assert_eq!(browser_page_url(" HTTPS://EXAMPLE.COM/path?q=1#part ").unwrap(), "https://example.com/path?q=1#part");
        for url in ["file:///C:/secret.txt", "javascript:alert(1)", "https://user:password@example.com", "not a url", "https://"] {
            assert!(browser_page_url(url).is_err(), "{url}");
        }
    }
}

#[tauri::command]
pub async fn verify_browser_page(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let text = state
        .browser_bridge
        .request_tool(
            "browser_snapshot",
            &serde_json::json!({}),
            std::time::Duration::from_secs(15),
            Some("settings-probe"),
        )
        .await?;
    Ok(text.chars().take(2400).collect())
}
