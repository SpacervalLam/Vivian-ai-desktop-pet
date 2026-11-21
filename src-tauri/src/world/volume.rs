//! 系统音量感知 —— 通过 Core Audio API 获取主音量与静音状态。
//!
//! 事件驱动：注册 IAudioEndpointVolumeCallback，音量/静音变化时立即回调。
//! 30s 兜底刷新防止事件丢失。

use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VolumeSnapshot {
    pub level: u8,
    pub muted: bool,
    pub device_name: Option<String>,
}

pub fn get_volume() -> VolumeSnapshot {
    #[cfg(target_os = "windows")]
    {
        match try_get_volume_windows() {
            Some(v) => v,
            None => VolumeSnapshot {
                level: 50,
                muted: false,
                device_name: None,
            },
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        VolumeSnapshot {
            level: 50,
            muted: false,
            device_name: None,
        }
    }
}

#[cfg(target_os = "windows")]
fn try_get_volume_windows() -> Option<VolumeSnapshot> {
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        if hr.is_err() {
            tracing::debug!("[Volume] CoInitializeEx 失败: {:?}", hr);
        }

        let enumerator: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("[Volume] CoCreateInstance 失败: {}", e);
                return None;
            }
        };
        let device = match enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("[Volume] GetDefaultAudioEndpoint 失败: {}", e);
                return None;
            }
        };
        let endpoint_volume: IAudioEndpointVolume = match device.Activate(CLSCTX_ALL, None) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[Volume] Activate IAudioEndpointVolume 失败: {}", e);
                return None;
            }
        };

        let scalar = endpoint_volume.GetMasterVolumeLevelScalar().ok()?;
        let muted = endpoint_volume.GetMute().ok()?;

        let level = (scalar * 100.0).round().clamp(0.0, 100.0) as u8;
        tracing::debug!("[Volume] Core Audio 读取成功: {}% (muted={})", level, muted.as_bool());

        Some(VolumeSnapshot {
            level,
            muted: muted.as_bool(),
            device_name: None,
        })
    }
}

// ─── 音量变化事件订阅 ─────────────────────────────────────────────────────────

/// 音量事件守卫 —— 持有 endpoint volume 引用，Drop 时取消回调注册。
#[cfg(windows)]
pub struct VolumeEventGuard {
    endpoint_volume: windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
    callback_ptr: *mut std::ffi::c_void,
}

#[cfg(windows)]
unsafe impl Send for VolumeEventGuard {}

#[cfg(windows)]
impl Drop for VolumeEventGuard {
    fn drop(&mut self) {
        unsafe {
            use windows::core::Interface;
            let cb = windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolumeCallback::from_raw(
                self.callback_ptr,
            );
            let _ = self.endpoint_volume.UnregisterControlChangeNotify(&cb);
        }
    }
}

// ─── 手动 COM vtable：IAudioEndpointVolumeCallback ────────────────────────────

#[cfg(windows)]
#[allow(non_snake_case)]
mod com_volume_cb {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use windows::core::GUID;

    const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_C000_000000000046);
    // IID_IAudioEndpointVolumeCallback: {657804FA-D6AD-4496-8A60-352752AF4F89}
    pub const IID_VOLUME_CB: GUID = GUID::from_u128(0x657804FA_D6AD_4496_8A60_352752AF4F89);

    #[repr(C)]
    pub struct VolumeCbVtbl {
        pub QueryInterface:
            unsafe extern "system" fn(*mut std::ffi::c_void, *const GUID, *mut *mut std::ffi::c_void) -> i32,
        pub AddRef: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
        pub Release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
        pub OnNotify: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32,
    }

    #[repr(C)]
    pub struct VolumeCbSink {
        pub vtbl: *const VolumeCbVtbl,
        pub ref_count: AtomicU32,
        pub notify: Arc<tokio::sync::Notify>,
    }

    pub static VTBL: VolumeCbVtbl = VolumeCbVtbl {
        QueryInterface,
        AddRef,
        Release,
        OnNotify,
    };

    unsafe extern "system" fn QueryInterface(
        this: *mut std::ffi::c_void,
        riid: *const GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32 {
        let riid = &*riid;
        if *riid == IID_IUNKNOWN || *riid == IID_VOLUME_CB {
            *ppv = this;
            AddRef(this);
            0
        } else {
            *ppv = std::ptr::null_mut();
            0x8000_4002u32 as i32
        }
    }

    unsafe extern "system" fn AddRef(this: *mut std::ffi::c_void) -> u32 {
        let sink = &*(this as *const VolumeCbSink);
        sink.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn Release(this: *mut std::ffi::c_void) -> u32 {
        let sink = &*(this as *const VolumeCbSink);
        let count = sink.ref_count.fetch_sub(1, Ordering::Release) - 1;
        if count == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this as *mut VolumeCbSink));
        }
        count
    }

    unsafe extern "system" fn OnNotify(
        this: *mut std::ffi::c_void,
        _notify_data: *mut std::ffi::c_void,
    ) -> i32 {
        let sink = &*(this as *const VolumeCbSink);
        sink.notify.notify_one();
        0
    }
}

/// 订阅音量变化事件（阻塞式，需在 spawn_blocking 中调用）。
///
/// 注册 IAudioEndpointVolumeCallback，音量/静音变化时触发 Notify。
/// 返回守卫结构，Drop 时自动取消注册。
#[cfg(windows)]
pub fn subscribe_volume_events(
    notify: Arc<tokio::sync::Notify>,
) -> Option<VolumeEventGuard> {
    use std::sync::atomic::AtomicU32;
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let endpoint_volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;

        // 构建手动 COM 回调对象
        let sink = Box::new(com_volume_cb::VolumeCbSink {
            vtbl: &com_volume_cb::VTBL,
            ref_count: AtomicU32::new(1),
            notify,
        });
        let sink_ptr = Box::into_raw(sink) as *mut std::ffi::c_void;

        let callback: IAudioEndpointVolumeCallback = Interface::from_raw(sink_ptr);
        endpoint_volume.RegisterControlChangeNotify(&callback).ok()?;

        // 保留原始指针用于 Drop 时 Unregister（from_raw 转移了所有权给 callback，
        // 但 RegisterControlChangeNotify 内部会 AddRef，所以 sink 仍存活）
        let callback_ptr = callback.as_raw();
        std::mem::forget(callback); // 不让 callback drop 时 Release（由 guard 管理）

        Some(VolumeEventGuard {
            endpoint_volume,
            callback_ptr,
        })
    }
}

#[cfg(not(windows))]
pub fn subscribe_volume_events(
    _notify: Arc<tokio::sync::Notify>,
) -> Option<()> {
    None
}
