//! Shared icon resolver for saved shortcuts and embedded browser sites.
//!
//! 网站图标按成本从低到高尝试三条来源：
//!   1. 站点根目录的 `/favicon.ico`（绝大多数站点都有，一次请求）
//!   2. 首页 HTML 里声明的 `<link rel="icon">`（含 apple-touch-icon / SVG 图标）
//!   3. 约定俗成的 `/favicon.png`、`/favicon.svg`、`/apple-touch-icon.png`
//!
//! 每条来源都会同时用「应用配置的代理」和「直连」两条路由去取，谁先成功用谁。
//! 这不是过度设计：用户机器的环境变量里长期存着 `HTTP_PROXY`/`HTTPS_PROXY`，
//! 而代理软件（Clash 等）关掉时那个端口是死的——reqwest 默认会吃这两个环境变量，
//! 于是裸客户端把所有图标请求都送进死端口，全部站点退化成首字母。
//! 直连一路同时兜底，代理开着的时候又能正常走代理。
use std::collections::HashSet;
use std::time::{Duration, Instant};

use base64::Engine;
use futures::future::select_ok;
use reqwest::{Client, Url};
use scraper::{Html, Selector};

/// 单个图标的体积上限
const MAX_ICON_BYTES: usize = 512 * 1024;
/// 首页只读到这么多字节（图标声明都在 `<head>` 里）
const MAX_PAGE_BYTES: usize = 512 * 1024;
/// 首页里最多采纳几个候选图标
const MAX_PAGE_ICONS: usize = 5;
/// 单次请求超时
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
/// 一次取图标的总时间预算，防止「站点没图标 + 代理半死不活」时把界面卡住
const TOTAL_BUDGET: Duration = Duration::from_secs(15);
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const ICON_ACCEPT: &str = "image/avif,image/webp,image/png,image/svg+xml,image/*,*/*;q=0.8";
const FALLBACK_PATHS: [&str; 3] = ["/favicon.png", "/favicon.svg", "/apple-touch-icon.png"];

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// 补全协议头：用户经常只填 `bilibili.com` 或 `www.bilibili.com`。
pub fn normalize_target(target: &str) -> String {
    let trimmed = target.trim();
    match trimmed.find("://") {
        Some(at) if at > 0 && at + 3 < trimmed.len() => trimmed.to_string(),
        _ => format!("https://{trimmed}"),
    }
}

fn parse_website(target: &str) -> Result<Url, String> {
    let url = Url::parse(&normalize_target(target)).map_err(|e| format!("网址无效: {e}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Invalid website URL".into());
    }
    Ok(url)
}

fn base_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(4))
        .user_agent(USER_AGENT)
}

/// 代理路由 + 直连路由。直连那一路显式 `no_proxy()`，否则会被环境变量带回死端口。
fn build_clients(proxy_url: Option<&str>) -> Vec<(String, Client)> {
    let mut clients = Vec::new();
    if let Some(url) = proxy_url {
        match reqwest::Proxy::all(url) {
            Ok(proxy) => match base_builder().proxy(proxy).build() {
                Ok(client) => clients.push((format!("proxy {url}"), client)),
                Err(e) => tracing::warn!(target: "shortcut_icon", error = %e, "proxy_client_build_failed"),
            },
            Err(e) => tracing::warn!(target: "shortcut_icon", proxy = %url, error = %e, "proxy_config_invalid"),
        }
    }
    match base_builder().no_proxy().build() {
        Ok(client) => clients.push(("direct".to_string(), client)),
        Err(e) => tracing::warn!(target: "shortcut_icon", error = %e, "direct_client_build_failed"),
    }
    clients
}

fn root_path(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// 只认图片格式；SVG / WebP 也算，现代站点越来越常用。
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if looks_like_svg(bytes) {
        Some("image/svg+xml")
    } else {
        None
    }
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg"))
}

async fn read_all(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() + chunk.len() > cap {
            return Err(format!("响应超过 {} KB", cap / 1024));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_head(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    while bytes.len() < cap {
        match response.chunk().await.map_err(|e| e.to_string())? {
            Some(chunk) => bytes.extend_from_slice(&chunk),
            None => break,
        }
    }
    Ok(bytes)
}

async fn fetch_icon(client: &Client, url: Url) -> Result<String, String> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, ICON_ACCEPT)
        // 未启用 gzip 特性，明确要 identity，免得拿到压缩字节误判成「不是图片」
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let bytes = read_all(response, MAX_ICON_BYTES).await?;
    if bytes.is_empty() {
        return Err("响应为空".into());
    }
    let mime = sniff_mime(&bytes).ok_or_else(|| "不是可识别的图片格式".to_string())?;
    Ok(data_url(mime, &bytes))
}

/// 抓首页 `<head>`，按声明顺序取出图标地址（普通 icon 优先于 apple-touch-icon）。
async fn page_icon_urls(client: &Client, target: &Url) -> Result<Vec<Url>, String> {
    let response = client
        .get(target.clone())
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let final_url = response.url().clone();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = read_head(response, MAX_PAGE_BYTES).await?;
    if !content_type.contains("html") && !bytes.starts_with(b"<") {
        return Err("首页不是 HTML".into());
    }
    let html = String::from_utf8_lossy(&bytes);
    Ok(extract_icon_urls(&html, &final_url))
}

fn extract_icon_urls(html: &str, base: &Url) -> Vec<Url> {
    let Ok(selector) = Selector::parse("link[rel]") else {
        return Vec::new();
    };
    let document = Html::parse_document(html);
    let (mut primary, mut secondary) = (Vec::new(), Vec::new());
    let mut seen = HashSet::new();
    for element in document.select(&selector) {
        let rel = element.value().attr("rel").unwrap_or("").to_ascii_lowercase();
        if !rel.contains("icon") {
            continue;
        }
        let Some(href) = element.value().attr("href").map(str::trim).filter(|h| !h.is_empty()) else {
            continue;
        };
        let Ok(resolved) = base.join(href) else { continue };
        if !matches!(resolved.scheme(), "http" | "https") || !seen.insert(resolved.as_str().to_string()) {
            continue;
        }
        if rel.contains("apple-touch") || rel.contains("mask") {
            secondary.push(resolved);
        } else {
            primary.push(resolved);
        }
    }
    primary.extend(secondary);
    primary.truncate(MAX_PAGE_ICONS);
    primary
}

async fn try_candidate(client: &Client, url: Url, tried: &mut Vec<String>) -> Option<String> {
    match fetch_icon(client, url.clone()).await {
        Ok(icon) => Some(icon),
        Err(e) => {
            tried.push(format!("{url} -> {e}"));
            None
        }
    }
}

async fn best_icon(client: &Client, target: &Url, deadline: Instant) -> Result<String, String> {
    let mut tried: Vec<String> = Vec::new();

    if let Some(icon) = try_candidate(client, root_path(target, "/favicon.ico"), &mut tried).await {
        return Ok(icon);
    }

    let mut queue: Vec<Url> = Vec::new();
    if Instant::now() < deadline {
        match page_icon_urls(client, target).await {
            Ok(candidates) if !candidates.is_empty() => queue.extend(candidates),
            Ok(_) => tried.push(format!("{target} 首页没有声明图标")),
            Err(e) => tried.push(format!("{target} 首页读取失败: {e}")),
        }
    }
    for path in FALLBACK_PATHS {
        queue.push(root_path(target, path));
    }

    for candidate in queue {
        if Instant::now() >= deadline {
            tried.push("时间预算耗尽".into());
            break;
        }
        if let Some(icon) = try_candidate(client, candidate, &mut tried).await {
            return Ok(icon);
        }
    }
    Err(tried.join("; "))
}

pub async fn website(target: &str, proxy_url: Option<&str>) -> Result<String, String> {
    let url = parse_website(target)?;
    let clients = build_clients(proxy_url);
    if clients.is_empty() {
        return Err("无法创建网络客户端".into());
    }
    let deadline = Instant::now() + TOTAL_BUDGET;
    // 两条路由并发跑，谁先取到用谁：代理关掉时直连立刻返回，代理开着时也不吃亏。
    // select_ok 要求每路都是 Result，所以把路由名一起塞进 Ok/Err 里；Client 内部是 Arc，
    // 直接 move 进 future，免得 future 去借用外面这个 Vec。
    let routes: Vec<_> = clients
        .into_iter()
        .map(|(label, client)| {
            let url = url.clone();
            Box::pin(async move {
                match best_icon(&client, &url, deadline).await {
                    Ok(icon) => Ok((label, icon)),
                    Err(error) => Err((label, error)),
                }
            })
        })
        .collect();
    match select_ok(routes).await {
        Ok(((route, icon), _)) => {
            tracing::debug!(target: "shortcut_icon", %url, route = %route, "website_icon_resolved");
            Ok(icon)
        }
        Err((route, error)) => {
            let summary = format!("{route}: {error}");
            tracing::warn!(target: "shortcut_icon", %url, error = %summary, "website_icon_unavailable");
            Err(summary)
        }
    }
}

#[cfg(windows)]
pub fn application(target: &str) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::PCWSTR, Win32::{Graphics::Gdi::*, Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
        UI::{Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON}, WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL}}}};
    let path = std::path::Path::new(target);
    if !path.is_absolute() || !path.is_file() { return Err("Application does not exist".into()); }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut pixels = vec![0u8; 32 * 32 * 4];
    unsafe {
        let mut file = SHFILEINFOW::default();
        if SHGetFileInfoW(PCWSTR(wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut file), std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON) == 0 {
            return Err("Cannot read application icon".into());
        }
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, 32, 32);
        if screen.is_invalid() || dc.is_invalid() || bitmap.is_invalid() {
            if !bitmap.is_invalid() { let _ = DeleteObject(HGDIOBJ(bitmap.0)); }
            if !dc.is_invalid() { let _ = DeleteDC(dc); }
            if !screen.is_invalid() { ReleaseDC(None, screen); }
            let _ = DestroyIcon(file.hIcon);
            return Err("Cannot create icon bitmap".into());
        }
        let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
        // A neutral white tile also supports legacy icons that have no alpha channel.
        let _ = PatBlt(dc, 0, 0, 32, 32, WHITENESS);
        let drawn = DrawIconEx(dc, 0, 0, file.hIcon, 32, 32, 0, None, DI_NORMAL);
        SelectObject(dc, previous);
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = 32;
        info.bmiHeader.biHeight = -32;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let rows = GetDIBits(dc, bitmap, 0, 32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        let _ = DestroyIcon(file.hIcon);
        if drawn.is_err() || rows != 32 { return Err("Cannot render icon".into()); }
    }
    for pixel in pixels.chunks_exact_mut(4) { pixel.swap(0, 2); pixel[3] = 255; }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 32, 32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&pixels).map_err(|e| e.to_string())?;
    }
    Ok(data_url("image/png", &bytes))
}

#[cfg(not(windows))]
pub fn application(_target: &str) -> Result<String, String> { Err("Application icons are only available on Windows".into()) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_target_adds_scheme() {
        assert_eq!(normalize_target("bilibili.com"), "https://bilibili.com");
        assert_eq!(normalize_target(" www.bilibili.com "), "https://www.bilibili.com");
        assert_eq!(normalize_target("https://github.com"), "https://github.com");
        assert_eq!(normalize_target("http://127.0.0.1:8080/app"), "http://127.0.0.1:8080/app");
    }

    #[test]
    fn parse_website_accepts_scheme_less_and_rejects_credentials() {
        assert_eq!(parse_website("bilibili.com").unwrap().as_str(), "https://bilibili.com/");
        assert!(parse_website("https://user:pass@example.com").is_err());
        assert!(parse_website("ftp://example.com").is_err());
    }

    #[test]
    fn sniff_mime_recognises_common_favicon_formats() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\n\0\0"), Some("image/png"));
        assert_eq!(sniff_mime(&[0, 0, 1, 0, 1, 0, 32, 32]), Some("image/x-icon"));
        assert_eq!(sniff_mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(sniff_mime(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        let webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert_eq!(sniff_mime(webp), Some("image/webp"));
        assert_eq!(sniff_mime(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"), Some("image/svg+xml"));
        assert_eq!(sniff_mime(b"<?xml version=\"1.0\"?>\n<svg></svg>"), Some("image/svg+xml"));
        assert_eq!(sniff_mime(b"<!DOCTYPE html><html>404</html>"), None);
        assert_eq!(sniff_mime(b""), None);
    }

    #[test]
    fn extract_icon_urls_orders_and_resolves() {
        let base = Url::parse("https://www.bilibili.com/").unwrap();
        let html = r#"<html><head>
            <link rel="apple-touch-icon" href="/apple.png">
            <link rel="icon" type="image/svg+xml" href="/favicon.svg">
            <link rel="shortcut icon" href="https://cdn.example.com/favicon.ico">
            <link rel="stylesheet" href="/style.css">
            <link rel="icon" href="/favicon.svg">
            </head></html>"#;
        let urls = extract_icon_urls(html, &base);
        let rendered: Vec<&str> = urls.iter().map(|u| u.as_str()).collect();
        assert_eq!(
            rendered,
            vec![
                "https://www.bilibili.com/favicon.svg",
                "https://cdn.example.com/favicon.ico",
                "https://www.bilibili.com/apple.png",
            ],
            "普通 icon 排在 apple-touch-icon 之前，相对地址要按首页解析，重复项要去掉"
        );
    }

    #[test]
    fn root_path_strips_path_and_query() {
        let base = Url::parse("https://example.com/app/index.html?a=1#x").unwrap();
        assert_eq!(root_path(&base, "/favicon.ico").as_str(), "https://example.com/favicon.ico");
    }
}
