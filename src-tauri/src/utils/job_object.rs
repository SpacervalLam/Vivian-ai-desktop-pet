//! Windows Job Object 兜底机制
//!
//! 创建一个 Job Object 并设置 `KILL_ON_JOB_CLOSE` 标志，所有子进程启动时
//! 绑定到该 Job。无论应用是正常退出、panic、还是被强杀，Job Object 句柄
//! 被 OS 自动关闭时会触发 `KILL_ON_JOB_CLOSE`，杀掉所有绑定的子进程及其
//! 子孙进程（Windows 默认行为：未设置 `BREAKAWAY_OK` 时子进程自动加入父 Job）。
//!
//! 这解决了三个问题：
//! 1. `kill_on_drop(true)` 在服务管理器被 `Arc<OnceCell>` 长期持有时永远不触发
//! 2. panic hook 无法清理子进程（panic 后进程立即退出，Arc 不会 drop）
//! 3. 进程被强杀/segfault 时无任何应用层清理机会
//!
//! 非 Windows 平台为 noop。

use std::sync::OnceLock;

/// 全局 Job Object 句柄持有者（应用生命周期内不释放，确保 KILL_ON_JOB_CLOSE 不触发）
struct JobObjectHandle {
    #[cfg(windows)]
    handle: isize,
}

static JOB_OBJECT: OnceLock<JobObjectHandle> = OnceLock::new();

/// 初始化全局 Job Object
///
/// 在应用 setup 阶段调用一次。失败时应用仍可运行，但失去 OS 级兜底保护，
/// 此时退化为依赖 ExitRequested + panic hook 的应用层清理。
pub fn init() -> bool {
    if JOB_OBJECT.get().is_some() {
        return true;
    }
    #[cfg(windows)]
    {
        match JobObjectHandle::create() {
            Some(h) => {
                let _ = JOB_OBJECT.set(h);
                tracing::info!("[JobObject] 全局 Job Object 已初始化，子进程将绑定到 Job");
                true
            }
            None => {
                tracing::warn!("[JobObject] 初始化失败，子进程将失去 OS 级兜底保护");
                false
            }
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 将已 spawn 的子进程绑定到全局 Job Object
///
/// 在 `Command::spawn()` 成功返回后立即调用。即使进程已开始执行，assign 也会
/// 立即生效；进程后续 fork 的子进程会自动加入同一个 Job。
///
/// 参数 `raw_handle` 为子进程的 HANDLE 值（通过 `AsRawHandle::as_raw_handle()` 获取）。
/// 返回是否成功绑定。失败时记录警告但不影响子进程运行。
#[cfg(windows)]
pub fn assign_process(raw_handle: isize) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;

    let Some(job) = JOB_OBJECT.get() else {
        tracing::debug!("[JobObject] 未初始化，跳过子进程绑定");
        return false;
    };
    let result = unsafe {
        AssignProcessToJobObject(
            HANDLE(job.handle as *mut core::ffi::c_void),
            HANDLE(raw_handle as *mut core::ffi::c_void),
        )
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("[JobObject] 绑定子进程到 Job 失败: {e}");
            false
        }
    }
}

#[cfg(not(windows))]
pub fn assign_process(_raw_handle: isize) -> bool {
    false
}

#[cfg(windows)]
impl JobObjectHandle {
    fn create() -> Option<Self> {
        use windows::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            SetInformationJobObject,
        };

        unsafe {
            let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let result = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if result.is_err() {
                let _ = windows::Win32::Foundation::CloseHandle(job);
                return None;
            }

            Some(Self {
                handle: job.0 as isize,
            })
        }
    }
}
