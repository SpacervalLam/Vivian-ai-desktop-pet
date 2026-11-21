//! 网络连通性监听 —— 基于 INetworkListManagerEvents COM 回调。
//!
//! 当系统网络状态变化（切换 WiFi、断线重连、IP 变更等）时触发通知，
//! 上层据此重新执行地理定位，实现"换网即刷新城市"。
//!
//! 注意：不使用 `#[implement]` 宏（与 tauri 依赖的 windows-core 0.54 存在版本冲突），
//! 改为手动构建 COM vtable。事件注册走 IConnectionPointContainer / IConnectionPoint。

use std::sync::Arc;

/// 网络事件守卫 —— 持有 COM 连接点注册，Drop 时自动取消订阅。
///
/// 内部 COM 指针在 MTA 模式下创建，可安全跨线程使用。
#[cfg(windows)]
pub struct NetworkEventGuard {
    inner: NetworkEventGuardInner,
}

#[cfg(windows)]
struct NetworkEventGuardInner {
    connection_point: windows::Win32::System::Com::IConnectionPoint,
    cookie: u32,
}

// MTA 下创建的 COM 对象可自由跨线程，标记为 Send 以便从 spawn_blocking 返回到 async 上下文。
#[cfg(windows)]
unsafe impl Send for NetworkEventGuard {}

#[cfg(windows)]
impl Drop for NetworkEventGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = self.inner.connection_point.Unadvise(self.inner.cookie);
        }
    }
}

// ─── 手动 COM vtable 实现 ─────────────────────────────────────────────────────

#[cfg(windows)]
#[allow(non_snake_case)]
mod com_sink {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use windows::core::GUID;

    // IID_IUnknown: {00000000-0000-0000-C000-000000000046}
    const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_C000_000000000046);
    // IID_INetworkListManagerEvents: {DCB00005-570F-4A9B-8D69-199FDBA5723B}
    pub const IID_EVENTS: GUID = GUID::from_u128(0xDCB00005_570F_4A9B_8D69_199FDBA5723B);

    #[repr(C)]
    pub struct SinkVtbl {
        pub QueryInterface:
            unsafe extern "system" fn(*mut std::ffi::c_void, *const GUID, *mut *mut std::ffi::c_void) -> i32,
        pub AddRef: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
        pub Release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
        pub ConnectivityChanged: unsafe extern "system" fn(*mut std::ffi::c_void, i32) -> i32,
    }

    #[repr(C)]
    pub struct Sink {
        pub vtbl: *const SinkVtbl,
        pub ref_count: AtomicU32,
        pub notify: Arc<tokio::sync::Notify>,
    }

    pub static VTBL: SinkVtbl = SinkVtbl {
        QueryInterface,
        AddRef,
        Release,
        ConnectivityChanged,
    };

    unsafe extern "system" fn QueryInterface(
        this: *mut std::ffi::c_void,
        riid: *const GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32 {
        let riid = &*riid;
        if *riid == IID_IUNKNOWN || *riid == IID_EVENTS {
            *ppv = this;
            AddRef(this);
            0 // S_OK
        } else {
            *ppv = std::ptr::null_mut();
            0x8000_4002u32 as i32 // E_NOINTERFACE
        }
    }

    unsafe extern "system" fn AddRef(this: *mut std::ffi::c_void) -> u32 {
        let sink = &*(this as *const Sink);
        sink.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn Release(this: *mut std::ffi::c_void) -> u32 {
        let sink = &*(this as *const Sink);
        let count = sink.ref_count.fetch_sub(1, Ordering::Release) - 1;
        if count == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this as *mut Sink));
        }
        count
    }

    unsafe extern "system" fn ConnectivityChanged(
        this: *mut std::ffi::c_void,
        _connectivity: i32,
    ) -> i32 {
        let sink = &*(this as *const Sink);
        sink.notify.notify_one();
        0 // S_OK
    }
}

/// 订阅网络连通性事件（阻塞式，需在 spawn_blocking 中调用）。
///
/// 通过 IConnectionPointContainer → IConnectionPoint::Advise 注册
/// ConnectivityChanged 回调。返回守卫结构，Drop 时自动 Unadvise。
#[cfg(windows)]
pub fn subscribe_network_events(
    notify: Arc<tokio::sync::Notify>,
) -> Option<NetworkEventGuard> {
    use std::sync::atomic::AtomicU32;
    use windows::core::Interface;
    use windows::Win32::Networking::NetworkListManager::{
        INetworkListManager, NetworkListManager,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IConnectionPoint, IConnectionPointContainer,
        CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let manager: INetworkListManager =
            CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;

        // 获取连接点容器，找到 INetworkListManagerEvents 对应的连接点
        let container: IConnectionPointContainer = manager.cast().ok()?;
        let connection_point: IConnectionPoint = container
            .FindConnectionPoint(&com_sink::IID_EVENTS)
            .ok()?;

        // 构建手动 COM 回调对象
        let sink = Box::new(com_sink::Sink {
            vtbl: &com_sink::VTBL,
            ref_count: AtomicU32::new(1),
            notify,
        });
        let sink_ptr = Box::into_raw(sink) as *mut std::ffi::c_void;

        // Advise 需要 &IUnknown；from_raw 接管原始引用，Advise 内部会 AddRef
        let unknown: windows::core::IUnknown = Interface::from_raw(sink_ptr);
        let cookie = connection_point.Advise(&unknown).ok()?;

        Some(NetworkEventGuard {
            inner: NetworkEventGuardInner {
                connection_point,
                cookie,
            },
        })
    }
}

#[cfg(not(windows))]
pub fn subscribe_network_events(
    _notify: Arc<tokio::sync::Notify>,
) -> Option<()> {
    None
}
