//! Physical desktop capture and PNG cropping. Pixel data never touches the filesystem.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionAction { Analyze, Save, SaveAndAnalyze, Copy, CopyAndAnalyze, Cancel }
impl SelectionAction {
    pub fn copies(self) -> bool { matches!(self, Self::Copy | Self::CopyAndAnalyze) }
    pub fn saves(self) -> bool { matches!(self, Self::Save | Self::SaveAndAnalyze) }
    pub fn analyzes(self) -> bool { matches!(self, Self::Analyze | Self::SaveAndAnalyze | Self::CopyAndAnalyze) }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct CaptureRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl CaptureRegion {
    pub fn validate(self, width: u32, height: u32) -> Result<Self, String> {
        if self.width < 2 || self.height < 2
            || self.x.checked_add(self.width).is_none_or(|right| right > width)
            || self.y.checked_add(self.height).is_none_or(|bottom| bottom > height) {
            return Err("请框选有效的屏幕区域".into());
        }
        Ok(self)
    }
}

pub struct DesktopFrame {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
impl DesktopFrame {
    pub fn png(&self) -> Result<Vec<u8>, String> {
        encode_png(self.width, self.height, &self.rgba)
    }
    pub fn crop_png(&self, region: CaptureRegion) -> Result<Vec<u8>, String> {
        let region = region.validate(self.width, self.height)?;
        let mut pixels = Vec::with_capacity(region.width as usize * region.height as usize * 4);
        let stride = self.width as usize * 4;
        for y in region.y..region.y + region.height {
            let start = y as usize * stride + region.x as usize * 4;
            pixels.extend_from_slice(&self.rgba[start..start + region.width as usize * 4]);
        }
        encode_png(region.width, region.height, &pixels)
    }
}
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

pub async fn capture_desktop() -> Result<DesktopFrame, String> {
    tokio::task::spawn_blocking(capture_desktop_sync).await.map_err(|e| e.to_string())?
}
fn capture_desktop_sync() -> Result<DesktopFrame, String> {
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Gdi::*;
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN};
        use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        struct DpiGuard(DPI_AWARENESS_CONTEXT);
        impl Drop for DpiGuard {
            fn drop(&mut self) { unsafe { SetThreadDpiAwarenessContext(self.0); } }
        }
        unsafe {
            let previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            if previous_dpi.0.is_null() { return Err("无法读取屏幕物理坐标".into()); }
            let _dpi = DpiGuard(previous_dpi);
            let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let length = (width as i64).checked_mul(height as i64).and_then(|v| v.checked_mul(4))
                .filter(|v| width > 0 && height > 0 && *v <= 256 * 1024 * 1024)
                .ok_or("屏幕尺寸无效或过大")? as usize;
            let screen = GetDC(None);
            if screen.is_invalid() { return Err("无法读取屏幕".into()); }
            let memory = CreateCompatibleDC(Some(screen));
            if memory.is_invalid() {
                ReleaseDC(None, screen);
                return Err("无法创建截图上下文".into());
            }
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            if bitmap.is_invalid() {
                let _ = DeleteDC(memory);
                ReleaseDC(None, screen);
                return Err("无法创建截图位图".into());
            }
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            let copied = BitBlt(memory, 0, 0, width, height, Some(screen), left, top, SRCCOPY | CAPTUREBLT);
            // GetDIBits requires that the bitmap is not selected into a DC.
            SelectObject(memory, previous);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            let mut rgba = vec![0u8; length];
            let rows = GetDIBits(memory, bitmap, 0, height as u32, Some(rgba.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            if copied.is_err() || rows != height { return Err("截取屏幕失败".into()); }
            for pixel in rgba.chunks_exact_mut(4) { pixel.swap(0, 2); pixel[3] = 255; }
            Ok(DesktopFrame { left, top, width: width as u32, height: height as u32, rgba })
        }
    }
    #[cfg(not(windows))]
    { Err("当前平台暂不支持屏幕框选".into()) }
}

/// Explicit save actions use Windows' redirected Screenshots known folder (including OneDrive).
pub async fn save_selected_png(bytes: &[u8]) -> Result<String, String> {
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            use std::io::Write;
            use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Screenshots, KF_FLAG_CREATE};
            use windows::Win32::System::Com::CoTaskMemFree;
            let folder = unsafe {
                let value = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_CREATE, None).map_err(|e| format!("无法访问系统截图目录：{e}"))?;
                let path = value.to_string().map_err(|e| e.to_string());
                CoTaskMemFree(Some(value.0.cast()));
                std::path::PathBuf::from(path?)
            };
            std::fs::create_dir_all(&folder).map_err(|e| format!("无法创建系统截图目录：{e}"))?;
            let path = folder.join(format!("Screenshot_{}_{}.png", chrono::Local::now().format("%Y%m%d_%H%M%S"), &uuid::Uuid::new_v4().simple().to_string()[..8]));
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| format!("保存截图失败：{e}"))?;
            if let Err(error) = file.write_all(&bytes).and_then(|_| file.flush()) {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(format!("保存截图失败：{error}"));
            }
            Ok(path.to_string_lossy().into_owned())
        }
        #[cfg(not(windows))]
        { let _ = bytes; Err("当前平台暂不支持系统截图目录".into()) }
    }).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_save_actions_write_files() {
        assert!(SelectionAction::Copy.copies());
        assert!(SelectionAction::CopyAndAnalyze.copies());
        assert!(!SelectionAction::Copy.saves());
        assert!(!SelectionAction::Copy.analyzes());
        assert!(!SelectionAction::CopyAndAnalyze.saves());
        assert!(SelectionAction::CopyAndAnalyze.analyzes());
        assert!(!SelectionAction::Analyze.copies());
        assert!(!SelectionAction::Cancel.copies());
        assert!(!SelectionAction::Analyze.saves());
        assert!(!SelectionAction::Cancel.saves());
        assert!(SelectionAction::Save.saves());
        assert!(SelectionAction::SaveAndAnalyze.saves());
        assert!(!SelectionAction::Save.analyzes());
        assert!(!SelectionAction::Cancel.analyzes());
        assert!(SelectionAction::Analyze.analyzes());
        assert!(SelectionAction::SaveAndAnalyze.analyzes());
    }
    #[test]
    fn validates_bounds_without_overflow() {
        assert!(CaptureRegion { x: 8, y: 9, width: 2, height: 2 }.validate(10, 11).is_ok());
        assert!(CaptureRegion { x: 9, y: 9, width: 2, height: 2 }.validate(10, 11).is_err());
        assert!(CaptureRegion { x: u32::MAX, y: 0, width: 2, height: 2 }.validate(10, 11).is_err());
        assert!(CaptureRegion { x: 0, y: 0, width: 1, height: 2 }.validate(10, 11).is_err());
    }
    #[test]
    fn crop_contains_only_selected_pixels() {
        let rgba: Vec<u8> = (0..16).flat_map(|v| [v, 0, 0, 255]).collect();
        let frame = DesktopFrame { left: -4, top: -4, width: 4, height: 4, rgba };
        let bytes = frame.crop_png(CaptureRegion { x: 1, y: 1, width: 2, height: 2 }).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(&pixels[..info.buffer_size()], &[5,0,0,255, 6,0,0,255, 9,0,0,255, 10,0,0,255]);
    }
}
