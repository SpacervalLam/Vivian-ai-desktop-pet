//! Native browser surfaces have their own WebView2 environment and no Tauri IPC bridge.
pub mod model;
pub(crate) mod input;
pub(crate) mod plan;
#[cfg(windows)]
pub(crate) mod native;
use model::{BrowserBounds, BrowserRequest, BrowserSnapshot};

fn owner(webview: &tauri::Webview) -> Result<String, String> {
    if webview.label() != "memory" {
        return Err("Browser controls belong to the workbench".into());
    }
    Ok(webview.window().label().to_string())
}

#[tauri::command]
pub async fn embedded_browser_request(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    request: BrowserRequest,
) -> Result<BrowserSnapshot, String> {
    let owner = owner(&webview)?;
    #[cfg(windows)]
    {
        native::request(app, owner, request).await
    }
    #[cfg(not(windows))]
    {
        let _ = (app, owner, request);
        Err("Native browser currently requires Windows WebView2".into())
    }
}

#[tauri::command]
pub async fn embedded_browser_layout(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    bounds: BrowserBounds,
    visible: bool,
    sequence: u64,
    scrollbar_css: Option<String>,
    clip: Option<model::BrowserClip>,
) -> Result<(), String> {
    bounds.validate()?;
    if let Some(clip) = &clip { clip.validate()?; }
    let owner = owner(&webview)?;
    #[cfg(windows)]
    {
        if let Some(css) = scrollbar_css { native::set_scrollbar_style(app.clone(), owner.clone(), css).await?; }
        native::layout_clipped(app, owner, bounds, visible, sequence, clip).await
    }
    #[cfg(not(windows))]
    {
        let _ = (app, owner, bounds, visible, sequence, scrollbar_css, clip);
        Ok(())
    }
}

/// A temporary, local-only viewport image for HTML overlays above the native surface.
#[tauri::command]
pub async fn embedded_browser_capture(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    id: String,
) -> Result<String, String> {
    owner(&webview)?;
    #[cfg(windows)]
    { native::automation::capture_viewport(&app, &id).await }
    #[cfg(not(windows))]
    { let _ = (app, id); Err("Native browser currently requires Windows WebView2".into()) }
}

pub fn window_destroyed(label: &str) {
    #[cfg(windows)]
    native::park(label);
    #[cfg(not(windows))]
    let _ = label;
}
