//! Clipboard hints observe only the sequence number. Text is read on explicit user action.
#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(handle: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(handle: *mut c_void) -> i32;
        fn GlobalSize(handle: *mut c_void) -> usize;
    }
    struct Open;
    impl Drop for Open { fn drop(&mut self) { unsafe { CloseClipboard(); } } }
    fn open() -> Result<Open, String> {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 { return Err("剪贴板正在被其他程序使用，请重试。".into()); }
        Ok(Open)
    }
    pub fn sequence() -> u32 { unsafe { GetClipboardSequenceNumber() } }
    pub fn read() -> Result<(u32, String), String> {
        let _open = open()?;
        let sequence = sequence();
        let handle = unsafe { GetClipboardData(13) };
        if handle.is_null() { return Err("剪贴板没有可读取的文本。".into()); }
        let bytes = unsafe { GlobalSize(handle) };
        if bytes == 0 || bytes > 1_048_576 || bytes % 2 != 0 { return Err("剪贴板文本过大或格式无效。".into()); }
        let pointer = unsafe { GlobalLock(handle) } as *const u16;
        if pointer.is_null() { return Err("无法读取剪贴板文本。".into()); }
        let text = unsafe {
            let units = std::slice::from_raw_parts(pointer, bytes / 2);
            String::from_utf16_lossy(&units[..units.iter().position(|unit| *unit == 0).unwrap_or(units.len())])
        };
        unsafe { GlobalUnlock(handle); }
        Ok((sequence, text))
    }

}
#[cfg(windows)]
pub use native::{read, sequence};
#[cfg(not(windows))]
pub fn sequence() -> u32 { 0 }
#[cfg(not(windows))]
pub fn read() -> Result<(u32, String), String> { Err("当前平台尚未支持剪贴板互动。".into()) }

/// Publish a standard CF_DIB image so image editors and chat apps can paste it.
pub async fn copy_png(bytes: &[u8]) -> Result<(), String> {
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            use std::ffi::c_void;
            #[link(name = "user32")]
            extern "system" {
                fn GetDesktopWindow() -> *mut c_void;
                fn OpenClipboard(owner: *mut c_void) -> i32;
                fn CloseClipboard() -> i32;
                fn EmptyClipboard() -> i32;
                fn SetClipboardData(format: u32, memory: *mut c_void) -> *mut c_void;
            }
            #[link(name = "kernel32")]
            extern "system" {
                fn GlobalAlloc(flags: u32, size: usize) -> *mut c_void;
                fn GlobalLock(memory: *mut c_void) -> *mut c_void;
                fn GlobalUnlock(memory: *mut c_void) -> i32;
                fn GlobalFree(memory: *mut c_void) -> *mut c_void;
            }
            let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
            let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
            let mut pixels = vec![0; reader.output_buffer_size().ok_or("Invalid PNG size")?];
            let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
            if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
                return Err("Invalid screenshot pixel format".into());
            }
            pixels.truncate(info.buffer_size());
            for pixel in pixels.chunks_exact_mut(4) { pixel.swap(0, 2); }
            let mut dib = vec![0u8; 40];
            dib[0..4].copy_from_slice(&40u32.to_le_bytes());
            dib[4..8].copy_from_slice(&(info.width as i32).to_le_bytes());
            dib[8..12].copy_from_slice(&(-(info.height as i32)).to_le_bytes());
            dib[12..14].copy_from_slice(&1u16.to_le_bytes());
            dib[14..16].copy_from_slice(&32u16.to_le_bytes());
            dib[20..24].copy_from_slice(&(pixels.len() as u32).to_le_bytes());
            dib.extend_from_slice(&pixels);
            unsafe {
                let memory = GlobalAlloc(2, dib.len());
                if memory.is_null() { return Err("无法分配剪贴板图像内存".into()); }
                let pointer = GlobalLock(memory);
                if pointer.is_null() { GlobalFree(memory); return Err("无法写入剪贴板图像".into()); }
                std::ptr::copy_nonoverlapping(dib.as_ptr(), pointer.cast(), dib.len());
                GlobalUnlock(memory);
                let mut opened = false;
                for _ in 0..5 {
                    if OpenClipboard(GetDesktopWindow()) != 0 { opened = true; break; }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                if !opened { GlobalFree(memory); return Err("剪贴板正在被其他程序使用，请重试".into()); }
                let success = EmptyClipboard() != 0 && !SetClipboardData(8, memory).is_null();
                CloseClipboard();
                // Successful SetClipboardData transfers ownership to Windows.
                if !success { GlobalFree(memory); return Err("复制截图到剪贴板失败".into()); }
                Ok(())
            }
        }
        #[cfg(not(windows))]
        { let _ = bytes; Err("当前平台暂不支持复制截图".into()) }
    }).await.map_err(|e| e.to_string())?
}
