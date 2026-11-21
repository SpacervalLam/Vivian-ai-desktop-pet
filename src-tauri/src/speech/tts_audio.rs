//! 音频播放助手:使用 Windows MCI (Media Control Interface) 进程内播放
//!
//! - 替代旧的 PowerShell + SAPI.SpVoice 子进程方案
//! - 支持 MP3 / WAV 格式
//! - 优雅停止:发送 MCI stop 命令而非 taskkill
//! - 非阻塞播放:启动后立即返回,通过轮询状态检测完成

use std::path::Path;

use crate::error::{VivianError, VivianResult};

use super::tts_backend::AudioFormat;

pub struct MciPlayer {
    alias: String,
}

impl MciPlayer {
    pub fn new() -> Self {
        Self {
            alias: String::new(),
        }
    }

    pub fn play_file(&mut self, path: &Path, format: AudioFormat) -> VivianResult<()> {
        self.stop_and_close();

        let alias = format!("tts_{}", uuid::Uuid::new_v4().simple());
        let device_type = match format {
            AudioFormat::Mp3 => "MPEGVideo",
            AudioFormat::Wav => "waveaudio",
            AudioFormat::Pcm => "waveaudio",
            AudioFormat::Ogg | AudioFormat::Aac => "MPEGVideo",
        };

        let path_str = path.to_string_lossy();
        let open_cmd = format!("open \"{}\" type {} alias {}", path_str, device_type, alias);

        tracing::info!("[TTS] MCI open: device={} alias={} path={}", device_type, alias, path_str);
        mci_send_string(&open_cmd).map_err(|e| {
            tracing::error!("[TTS] MCI open 失败: {}", e);
            e
        })?;

        // 设置时间格式为毫秒（便于 position 查询）
        let time_cmd = format!("set {} time format ms", alias);
        if let Err(e) = mci_send_string(&time_cmd) {
            tracing::warn!("[TTS] MCI set time format 失败（非致命）: {}", e);
        }

        // seek 到开头
        let seek_cmd = format!("seek {} to start", alias);
        if let Err(e) = mci_send_string(&seek_cmd) {
            tracing::warn!("[TTS] MCI seek 失败（非致命）: {}", e);
        }

        let play_cmd = format!("play {}", alias);
        let play_start = std::time::Instant::now();
        if let Err(e) = mci_send_string(&play_cmd) {
            tracing::error!("[TTS] MCI play 失败: {} (耗时 {}ms)", e, play_start.elapsed().as_millis());
            let _ = mci_send_string(&format!("close {}", alias));
            return Err(e);
        }
        tracing::info!("[TTS] MCI play 已启动: {} (play命令耗时 {}ms)", alias, play_start.elapsed().as_millis());

        self.alias = alias;
        Ok(())
    }

    fn query_mode(&self) -> Option<String> {
        if self.alias.is_empty() {
            return None;
        }
        match mci_send_string(&format!("status {} mode", self.alias)) {
            Ok(mode) => Some(mode.trim().to_lowercase()),
            Err(e) => {
                tracing::debug!("[TTS] MCI status mode 查询失败: {}", e);
                None
            }
        }
    }

    fn query_position(&self) -> Option<u32> {
        if self.alias.is_empty() {
            return None;
        }
        match mci_send_string(&format!("status {} position", self.alias)) {
            Ok(pos) => pos.trim().parse::<u32>().ok(),
            Err(_) => None,
        }
    }

    fn query_length(&self) -> Option<u32> {
        if self.alias.is_empty() {
            return None;
        }
        match mci_send_string(&format!("status {} length", self.alias)) {
            Ok(len) => len.trim().parse::<u32>().ok(),
            Err(_) => None,
        }
    }

    pub fn is_still_playing(&self) -> bool {
        matches!(self.query_mode().as_deref(), Some("playing"))
    }

    pub fn alias_str(&self) -> &str {
        &self.alias
    }

    pub fn stop_and_close(&mut self) {
        if !self.alias.is_empty() {
            let _ = mci_send_string(&format!("stop {}", self.alias));
            let _ = mci_send_string(&format!("close {}", self.alias));
            tracing::debug!("[TTS] MCI 设备已关闭: {}", self.alias);
            self.alias.clear();
        }
    }

    pub fn wait_until_done(&mut self, cancel: &std::sync::atomic::AtomicBool) {
        let start = std::time::Instant::now();
        let audio_length = self.query_length().unwrap_or(0);
        tracing::info!("[TTS] wait_until_done 开始: alias={} audio_length={}ms", self.alias, audio_length);

        // 阶段1：等待设备进入 playing 状态（最长等待 2 秒）
        let mut entered_playing = false;
        for _ in 0..40 {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("[TTS] wait_until_done: 在启动阶段收到取消信号");
                self.stop_and_close();
                return;
            }
            if let Some(mode) = self.query_mode() {
                tracing::debug!("[TTS] MCI mode={} elapsed={}ms", mode, start.elapsed().as_millis());
                if mode == "playing" {
                    entered_playing = true;
                    tracing::info!("[TTS] 设备进入 playing 状态，耗时 {}ms", start.elapsed().as_millis());
                    break;
                }
                // 如果 mode 是 stopped 且 position > 0，说明已经播完了
                if mode == "stopped" {
                    if let Some(pos) = self.query_position() {
                        if pos > 0 {
                            tracing::info!("[TTS] 设备已停止且 position={}，播放已完成", pos);
                            self.stop_and_close();
                            return;
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        if !entered_playing {
            let final_mode = self.query_mode().unwrap_or_else(|| "unknown".to_string());
            let final_pos = self.query_position().unwrap_or(0);
            tracing::warn!(
                "[TTS] 设备在 2 秒内未进入 playing 状态: mode={} position={} elapsed={}ms",
                final_mode, final_pos, start.elapsed().as_millis()
            );
            // 设备未进入 playing 且 position=0：音频可能无效，快速失败
            if final_pos == 0 && audio_length == 0 {
                tracing::error!("[TTS] 设备无法播放且音频长度为 0，快速失败");
                self.stop_and_close();
                return;
            }
        }

        // 阶段2：等待播放完成（最长等待音频时长 + 5 秒余量）
        let max_wait = if audio_length > 0 {
            audio_length as u64 + 5_000
        } else {
            5_000 // 缩短默认超时，避免长时间阻塞
        };

        loop {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("[TTS] wait_until_done: 收到取消信号");
                self.stop_and_close();
                return;
            }
            if start.elapsed() > std::time::Duration::from_millis(max_wait) {
                tracing::warn!("[TTS] wait_until_done: 等待超时 ({}ms)，强制关闭", max_wait);
                break;
            }
            let mode = self.query_mode().unwrap_or_else(|| "unknown".to_string());
            if mode != "playing" {
                if entered_playing {
                    let pos = self.query_position().unwrap_or(0);
                    tracing::info!("[TTS] 播放结束: mode={} position={} total={}ms", mode, pos, start.elapsed().as_millis());
                    break;
                }
                if mode == "stopped" {
                    if let Some(pos) = self.query_position() {
                        if pos > 0 {
                            tracing::info!("[TTS] 播放结束(stopped): position={}ms", pos);
                            break;
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        tracing::info!("[TTS] wait_until_done 完成: 总耗时={}ms", start.elapsed().as_millis());
        self.stop_and_close();
    }
}

impl Default for MciPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MciPlayer {
    fn drop(&mut self) {
        self.stop_and_close();
    }
}

fn mci_send_string(cmd: &str) -> VivianResult<String> {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Media::Multimedia::mciSendStringW;

        let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buf = [0u16; 256];

        let result = unsafe {
            mciSendStringW(
                PCWSTR(wide.as_ptr()),
                Some(&mut buf),
                None,
            )
        };

        if result != 0 {
            let msg = String::from_utf16_lossy(&buf);
            let err = get_mci_error(result);
            return Err(VivianError::Speech(format!(
                "MCI 命令失败 [{}]: {} ({})",
                result, err, msg.trim()
            )));
        }

        // MCI 返回 null-terminated 字符串，只取 null 终止符之前的内容
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(String::from_utf16_lossy(&buf[..len]))
    }
    #[cfg(not(windows))]
    {
        Err(VivianError::Speech(format!(
            "MCI 仅支持 Windows 平台: {}",
            cmd
        )))
    }
}

#[cfg(windows)]
fn get_mci_error(code: u32) -> &'static str {
    match code {
        275 => "无法找到指定设备",
        301 => "设备未打开",
        305 => "MCI 设备驱动程序不正确",
        309 => "无法在此设备上使用指定命令",
        317 => "MCI 设备不支持此参数",
        _ => "未知 MCI 错误",
    }
}

pub fn save_to_temp_file(audio: &[u8], format: AudioFormat) -> VivianResult<std::path::PathBuf> {
    let ext = match format {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Wav => "wav",
        AudioFormat::Pcm => "pcm",
        AudioFormat::Ogg => "ogg",
        AudioFormat::Aac => "aac",
    };
    let filename = format!("vivian_tts_{}.{}", uuid::Uuid::new_v4().simple(), ext);
    let path = std::env::temp_dir().join(filename);
    std::fs::write(&path, audio)
        .map_err(|e| VivianError::Speech(format!("写入临时音频文件失败: {e}")))?;
    Ok(path)
}

pub fn cleanup_temp_file(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

// ─── MemoryPlayer: 使用 rodio 从内存直接播放 ───────────────────────────

use rodio::{Decoder, OutputStream, Sink};
use std::io::Cursor;

/// 内存音频播放器 — 使用 rodio 从 `Vec<u8>` 直接播放，无需临时文件
///
/// 支持 MP3 / WAV / OGG / FLAC 解码。AAC 格式不支持（回退到 MCI）。
/// `OutputStream` 必须与 `Sink` 同生命周期，否则音频设备会关闭。
pub struct MemoryPlayer {
    sink: Sink,
    _stream: OutputStream,
    // rodio 的 _stream_handle 需要保留（OutputStream 持有即可）
}

impl MemoryPlayer {
    /// 从内存字节创建播放器并立即开始播放
    pub fn play_from_memory(audio: Vec<u8>, format: AudioFormat) -> VivianResult<Self> {
        // AAC 不支持，返回错误让调用方回退 MCI
        if matches!(format, AudioFormat::Aac) {
            return Err(VivianError::Speech(
                "rodio 不支持 AAC 格式，需要回退 MCI".to_string(),
            ));
        }

        let (stream, stream_handle) = OutputStream::try_default()
            .map_err(|e| VivianError::Speech(format!("音频输出设备初始化失败: {e}")))?;

        let sink = Sink::try_new(&stream_handle)
            .map_err(|e| VivianError::Speech(format!("音频 Sink 创建失败: {e}")))?;

        let cursor = Cursor::new(audio);
        let source = Decoder::new(cursor)
            .map_err(|e| VivianError::Speech(format!("音频解码失败: {e}")))?;

        sink.append(source);
        Ok(Self {
            sink,
            _stream: stream,
        })
    }

    /// 设置音量（0.0 - 1.0），用于 ducking
    pub fn set_volume(&self, vol: f32) {
        self.sink.set_volume(vol.clamp(0.0, 1.0));
    }

    /// 获取 Sink 引用（供 ducking watcher 线程从外部控制音量）
    pub fn sink(&self) -> &Sink {
        &self.sink
    }

    /// 是否仍在播放
    pub fn is_playing(&self) -> bool {
        !self.sink.empty()
    }

    /// 停止播放
    pub fn stop(&self) {
        self.sink.stop();
    }

    /// 阻塞等待播放完成，支持 AtomicBool 取消
    pub fn wait_until_done(&self, cancel: &std::sync::atomic::AtomicBool) {
        let start = std::time::Instant::now();
        tracing::info!("[TTS] MemoryPlayer wait_until_done 开始");

        while !self.sink.empty() {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("[TTS] MemoryPlayer: 收到取消信号");
                self.stop();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        tracing::info!(
            "[TTS] MemoryPlayer wait_until_done 完成: 耗时={}ms",
            start.elapsed().as_millis()
        );
    }
}
