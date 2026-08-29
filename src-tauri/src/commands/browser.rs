//! 浏览器桥面板命令 —— 设置页「浏览器平台」分区的前端接口。
//!
//! 三个命令：
//! - [`get_browser_platforms`]：桥连接状态 + 各平台登录态 + 扩展目录路径
//! - [`open_extension_folder`]：在系统文件管理器中打开扩展目录（引导加载）
//! - [`open_chrome_extensions`]：打开 Chrome 扩展管理页（chrome://extensions/）

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::state::AppState;

/// 前端视图：单个平台登录态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformStatusView {
    pub platform: String,
    pub logged_in: bool,
}

/// 前端视图：浏览器桥面板状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserBridgeStatus {
    /// 扩展是否已连接桥
    pub connected: bool,
    /// 各平台登录态（扩展未上报时为空数组）
    pub platforms: Vec<PlatformStatusView>,
    /// 最近一次上报时间（Unix 毫秒；未上报为 0）
    pub reported_at_ms: u64,
    /// 扩展目录（供引导安装时打开）
    pub extension_dir: String,
}

/// 解析扩展目录：打包版优先资源目录，回退开发目录（源码仓 browser-extension/）
fn resolve_extension_dir(app: &AppHandle) -> Option<String> {
    // 打包版：resource 目录下的 browser-extension
    if let Ok(res_dir) = app.path().resource_dir() {
        let packaged = res_dir.join("browser-extension");
        if packaged.join("manifest.json").exists() {
            return Some(packaged.to_string_lossy().to_string());
        }
    }
    // 开发版：src-tauri 的上级目录（源码仓根）
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("browser-extension");
    if dev.join("manifest.json").exists() {
        return Some(dev.to_string_lossy().to_string());
    }
    None
}

/// 获取浏览器桥面板状态
#[tauri::command]
pub fn get_browser_platforms(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<BrowserBridgeStatus, String> {
    let bridge = state.browser_bridge.clone();
    let (connected, platforms, reported_at_ms) = match bridge.platform_status() {
        Some((ts, status)) => {
            let mut views: Vec<PlatformStatusView> = status
                .into_iter()
                .map(|(platform, logged_in)| PlatformStatusView { platform, logged_in })
                .collect();
            views.sort_by(|a, b| a.platform.cmp(&b.platform));
            (bridge.is_connected(), views, ts)
        }
        None => (bridge.is_connected(), Vec::new(), 0),
    };
    Ok(BrowserBridgeStatus {
        connected,
        platforms,
        reported_at_ms,
        extension_dir: resolve_extension_dir(&app).unwrap_or_default(),
    })
}

/// 在系统文件管理器中打开扩展目录（引导用户加载未打包扩展）
#[tauri::command]
pub fn open_extension_folder(app: AppHandle) -> Result<(), String> {
    let Some(dir) = resolve_extension_dir(&app) else {
        return Err("未找到扩展目录".to_string());
    };
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("打开目录失败: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("打开目录失败: {e}"))?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("打开目录失败: {e}"))?;
    }
    Ok(())
}

/// Windows：通过 App Paths 注册表定位 chrome.exe（找不到再查标准安装目录）。
#[cfg(target_os = "windows")]
fn find_chrome_exe() -> Option<std::path::PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    const APP_PATH_SUBKEYS: [&str; 2] = [
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe",
    ];
    let roots = [RegKey::predef(HKEY_CURRENT_USER), RegKey::predef(HKEY_LOCAL_MACHINE)];
    for root in &roots {
        for subkey in APP_PATH_SUBKEYS {
            if let Ok(key) = root.open_subkey(subkey) {
                // 默认值即 exe 完整路径
                if let Ok(path) = key.get_value::<String, _>("") {
                    let p = std::path::PathBuf::from(path.trim());
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    // 标准安装目录兜底（App Paths 未注册的绿色版/便携版场景）
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(pf) = std::env::var("ProgramFiles") {
        candidates.push(std::path::PathBuf::from(pf).join(r"Google\Chrome\Application\chrome.exe"));
    }
    if let Ok(pf) = std::env::var("ProgramFiles(x86)") {
        candidates.push(std::path::PathBuf::from(pf).join(r"Google\Chrome\Application\chrome.exe"));
    }
    if let Ok(lad) = std::env::var("LOCALAPPDATA") {
        candidates.push(std::path::PathBuf::from(lad).join(r"Google\Chrome\Application\chrome.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// 用 Chrome 打开指定 URL（跨平台）。
///
/// 与系统默认浏览器无关：桥扩展运行在 Chrome 里，登录等操作必须发生
/// 在 Chrome 中，登录态（Cookie）才能被扩展的 Cookie 哨兵探测到。
/// `chrome://` 内部 scheme 也只能经此路径打开（非 OS 注册协议）。
/// Chrome 自身的单实例机制会把调用转交给已运行的浏览器（新开标签页）。
fn launch_chrome(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let exe = find_chrome_exe()
            .ok_or_else(|| "未找到 Chrome，请手动在 Chrome 中打开该页面".to_string())?;
        std::process::Command::new(exe)
            .arg(url)
            .spawn()
            .map_err(|e| format!("打开 Chrome 失败: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        // `open -a` 指定应用打开内部 URL；Chrome 未安装时报错而非静默
        std::process::Command::new("open")
            .args(["-a", "Google Chrome", url])
            .spawn()
            .map_err(|e| format!("打开 Chrome 失败: {e}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        const BROWSERS: [&str; 4] =
            ["google-chrome", "google-chrome-stable", "chromium-browser", "chromium"];
        let mut launched = false;
        for b in BROWSERS {
            if std::process::Command::new(b).arg(url).spawn().is_ok() {
                launched = true;
                break;
            }
        }
        if !launched {
            return Err("未找到 Chrome/Chromium，请手动在浏览器中打开该页面".to_string());
        }
    }
    Ok(())
}

/// 打开 Chrome 扩展管理页（chrome://extensions/）。
///
/// `chrome://` 是 Chrome 内部 scheme，不是操作系统注册的协议，经
/// ShellExecute / xdg-open 的通用「打开 URL」必然静默失败，故必须
/// 直接定位 Chrome 可执行文件带参启动。
#[tauri::command]
pub fn open_chrome_extensions() -> Result<(), String> {
    launch_chrome("chrome://extensions/")
}

/// 用 Chrome 打开 http(s) 登录页。
///
/// 平台登录态由桥扩展在 Chrome 内探测（Cookie 哨兵）；若按系统默认
/// 浏览器打开（可能是 Edge 等），登录发生在别的浏览器，Chrome 侧
/// Cookie 不变，面板会一直显示未登录。故登录页强制经 Chrome 打开。
#[tauri::command]
pub fn open_url_in_chrome(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("仅支持 http(s) 链接".to_string());
    }
    launch_chrome(trimmed)
}
