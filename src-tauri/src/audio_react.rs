//! 系统音频回环捕获 — 音乐驱动 Live2D 律动
//!
//! 以 WASAPI 回环模式捕获系统默认输出设备（扬声器正在播放的所有声音），
//! 切块后通过 `audio:pcm` 事件推给前端（Vec<f32> 单声道波形），
//! 前端做 FFT / 节拍检测后驱动角色随音乐摆动。
//!
//! 移植自 Petra 的 audio.rs（MIT），适配 vivian-rs 的 windows crate 版本与配置体系。
//! 通过 `set_music_reactive` 幂等启停，全局单实例（系统音频是全局的）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};
use windows::core::GUID;
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, EDataFlow, ERole, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};

const WAVE_TAG_FLOAT: u16 = 0x0003; // WAVE_FORMAT_IEEE_FLOAT
const WAVE_TAG_EXTENSIBLE: u16 = 0xFFFE; // WAVE_FORMAT_EXTENSIBLE

/// KSDATAFORMAT_SUBTYPE_PCM = {00000001-0000-0010-8000-00AA00389B71}
const SUBTYPE_PCM: GUID = GUID::from_u128(0x0000_0001_0000_0010_8000_00AA_0038_9B71);
/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT = {00000003-0000-0010-8000-00AA00389B73}
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x0000_0003_0000_0010_8000_00AA_0038_9B73);
/// CLSID_MMDeviceEnumerator = {BCDE0395-E52F-467C-8E3D-C4579291692E}
const CLSID_MM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xBCDE_0395_E52F_467C_8E3D_C457_9291_692E);

const CHUNK: usize = 1024;

/// 正在运行的捕获线程句柄（持有停止标志；线程随 stop 标志在 ≤50ms 内退出）
struct CaptureHandle {
    stop: Arc<AtomicBool>,
}

/// 全局单实例：系统音频捕获与角色无关，所有角色窗口共享同一路 PCM 流
static CAPTURE: Lazy<Mutex<Option<CaptureHandle>>> = Lazy::new(|| Mutex::new(None));

/// 当前是否已启用音乐驱动
pub fn is_enabled() -> bool {
    CAPTURE.lock().is_some()
}

/// 设置音乐驱动动画开关（幂等启停 WASAPI 回环捕获）
///
/// 状态持久化到配置 `music_reactive.enabled`，重启后自动恢复。
#[tauri::command]
pub fn set_music_reactive(
    app: tauri::AppHandle,
    state: tauri::State<'_, std::sync::Arc<crate::state::AppState>>,
    enabled: bool,
) -> Result<(), String> {
    set_enabled(app, enabled)?;
    // 仅在值变化时写盘，避免 config:saved 事件循环里频繁落盘
    let cfg = state.config.read();
    if cfg.get_all().music_reactive.enabled != enabled {
        let _ = cfg.set("music_reactive.enabled", serde_json::json!(enabled));
    }
    Ok(())
}

/// 查询音乐驱动动画当前是否启用
#[tauri::command]
pub fn get_music_reactive(
    state: tauri::State<'_, std::sync::Arc<crate::state::AppState>>,
) -> Result<bool, String> {
    Ok(state.config.read().get_all().music_reactive.enabled)
}

/// 幂等启停回环捕获。
///
/// - 启用：启动 WASAPI 回环捕获线程，持续 emit `audio:pcm`（Vec<f32> 单声道）
/// - 停用：置停止标志，线程在下一轮循环（≤50ms）内退出
pub fn set_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut guard = CAPTURE.lock();
    if enabled == guard.is_some() {
        return Ok(());
    }
    if enabled {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_closure = Arc::clone(&stop);
        // spawn 的 JoinHandle 立即 drop（分离线程），停止由 stop 标志驱动 ≤50ms 内退出
        let _ = std::thread::Builder::new()
            .name("music-react-wasapi".to_string())
            .spawn(move || {
                let _ = run_capture(app, stop_closure);
            })
            .map_err(|e| format!("启动音频回环线程失败: {e}"))?;
        *guard = Some(CaptureHandle { stop });
        tracing::info!("[audio_react] WASAPI 回环捕获已启动");
    } else {
        if let Some(handle) = guard.take() {
            handle.stop.store(true, Ordering::SeqCst);
            // drop JoinHandle 即分离线程；停止标志会在 ≤50ms 内使线程退出
        }
        tracing::info!("[audio_react] WASAPI 回环捕获已停止");
    }
    Ok(())
}

fn run_capture(app: AppHandle, stop: Arc<AtomicBool>) -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("COM 初始化失败: {e}"))?;
    }
    let result = unsafe { capture_loop(&app, &stop) };
    unsafe { CoUninitialize() };
    if let Err(e) = &result {
        tracing::warn!("[audio_react] 回环捕获异常: {e}");
        let _ = app.emit("audio:error", format!("音乐驱动音频捕获不可用：{e}"));
    }
    result
}

unsafe fn capture_loop(app: &AppHandle, stop: &AtomicBool) -> Result<(), String> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, None, CLSCTX_ALL)
            .map_err(|e| format!("无法创建设备枚举器：{e}"))?;

    let device = enumerator
        .GetDefaultAudioEndpoint(EDataFlow(eRender.0), ERole(eConsole.0))
        .map_err(|e| format!("无法获取默认输出设备：{e}"))?;

    let client: IAudioClient = device
        .Activate::<IAudioClient>(CLSCTX_ALL, None)
        .map_err(|e| format!("激活音频客户端失败：{e}"))?;

    let format_ptr = client.GetMixFormat().map_err(|e| e.to_string())?;
    if format_ptr.is_null() {
        return Err("GetMixFormat 返回空".into());
    }
    let fmt = &*format_ptr;
    let channels = fmt.nChannels as usize;
    let bits = fmt.wBitsPerSample as usize;
    let is_float = is_float_format(fmt.wFormatTag, format_ptr);

    client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            0,
            0,
            format_ptr,
            None,
        )
        .map_err(|e| format!("回环捕获初始化失败 (0x{:08X})：{e}", e.code().0))?;

    let capture: IAudioCaptureClient = client
        .GetService::<IAudioCaptureClient>()
        .map_err(|e| format!("获取捕获端点失败：{e}"))?;

    client.Start().map_err(|e| e.to_string())?;

    let mut mono: Vec<f32> = Vec::with_capacity(CHUNK);
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let packet_count = capture.GetNextPacketSize().map_err(|e| e.to_string())?;
        if packet_count == 0 {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        for _ in 0..packet_count {
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                .map_err(|e| e.to_string())?;
            if frames > 0 && !data.is_null() {
                if is_float && bits == 32 {
                    let vals = std::slice::from_raw_parts(data as *const f32, frames as usize * channels);
                    for f in vals.chunks_exact(channels) {
                        // 多声道取平均（保留正负波形），不用 RMS（RMS 丢失正负信息导致 FFT 失效）
                        mono.push(f.iter().sum::<f32>() / channels as f32);
                    }
                } else {
                    let bps = bits / 8;
                    let bytes = std::slice::from_raw_parts(data, frames as usize * channels * bps);
                    for frm in bytes.chunks_exact(channels * bps) {
                        let avg: f64 = frm
                            .chunks_exact(bps)
                            .map(|ch| decode_int_sample(ch, bps))
                            .sum::<f64>()
                            / channels as f64;
                        mono.push(avg as f32);
                    }
                }
                capture.ReleaseBuffer(frames).map_err(|e| e.to_string())?;
            }
        }
        while mono.len() >= CHUNK {
            let chunk: Vec<f32> = mono.drain(..CHUNK).collect();
            let _ = app.emit("audio:pcm", chunk);
        }
    }
    Ok(())
}

/// WAVEFORMATEXTENSIBLE 是 packed（1 字节对齐），字段必须用 read_unaligned 读取，
/// 不能直接解引用（会触发 E0793 unaligned 错误）。
fn is_float_format(tag: u16, format_ptr: *mut WAVEFORMATEX) -> bool {
    if tag == WAVE_TAG_FLOAT {
        return true;
    }
    if tag == WAVE_TAG_EXTENSIBLE {
        let ext = format_ptr as *const WAVEFORMATEXTENSIBLE;
        unsafe { std::ptr::addr_of!((*ext).SubFormat).read_unaligned() == SUBTYPE_FLOAT }
    } else {
        false
    }
}

fn decode_int_sample(bytes: &[u8], bps: usize) -> f64 {
    match bps {
        1 => (i32::from(bytes[0]) - 128) as f64 / 128.0,
        2 => i16::from_le_bytes([bytes[0], bytes[1]]) as f64 / 32768.0,
        3 => {
            let mut v = i32::from(bytes[0]) | (i32::from(bytes[1]) << 8);
            v |= (bytes[2] as i8 as i32) << 16;
            v as f64 / 8388608.0
        }
        4 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64 / 2147483648.0,
        _ => 0.0,
    }
}

#[allow(dead_code)]
const _SUBTYPE_PCM_GUARD: GUID = SUBTYPE_PCM;
