//! Icons are read only for shortcuts the user has already saved.
use base64::Engine;

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub async fn website(target: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(target).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err("Invalid website URL".into());
    }
    url.set_path("/favicon.ico");
    url.set_query(None);
    url.set_fragment(None);
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(6))
        .redirect(reqwest::redirect::Policy::limited(3)).build().map_err(|e| e.to_string())?;
    let mut response = client.get(url).send().await.map_err(|e| e.to_string())?
        .error_for_status().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() + chunk.len() > 512 * 1024 { return Err("Icon is too large".into()); }
        bytes.extend_from_slice(&chunk);
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { "image/png" }
        else if bytes.starts_with(&[0, 0, 1, 0]) { "image/x-icon" }
        else if bytes.starts_with(b"GIF8") { "image/gif" }
        else if bytes.starts_with(&[0xff, 0xd8, 0xff]) { "image/jpeg" }
        else { return Err("Unsupported icon format".into()); };
    Ok(data_url(mime, &bytes))
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
