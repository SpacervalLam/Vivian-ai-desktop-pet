//! PID 文件持久化 + 孤儿进程清理
//!
//! 每个外部子进程服务在启动时将自身 PID 写入 `<user_data_dir>/pids/<name>.pid`，
//! 启动前检查并清理上次崩溃残留的孤儿进程。
//!
//! 这是 Job Object 兜底机制的补充：Job Object 在进程强杀/segfault 时由 OS
//! 自动关闭句柄杀子进程，但若子进程在 Job Object 初始化前已启动（极端时序），
//! 或 Job Object 初始化失败，PID 文件提供最后一道防线。

use std::path::PathBuf;

/// PID 文件根目录：`<user_data_dir>/pids/`
fn pid_dir() -> PathBuf {
    let dir = crate::utils::path::get_user_data_dir().join("pids");
    let _ = crate::utils::path::ensure_dir(&dir);
    dir
}

/// PID 文件路径：`<user_data_dir>/pids/<name>.pid`
fn pid_file_path(name: &str) -> PathBuf {
    pid_dir().join(format!("{name}.pid"))
}

/// 写入 PID 文件
///
/// 在子进程 spawn 成功后立即调用。文件内容为纯数字 PID，附加换行符便于人工查看。
pub fn write_pid(name: &str, pid: u32) {
    let path = pid_file_path(name);
    match std::fs::write(&path, pid.to_string()) {
        Ok(()) => tracing::debug!("[PidFile] 已写入 {name}.pid = {pid}"),
        Err(e) => tracing::warn!("[PidFile] 写入 {name}.pid 失败: {e}"),
    }
}

/// 删除 PID 文件
///
/// 在子进程正常停止后调用。崩溃时文件会残留，由下次启动的 `cleanup_orphan` 处理。
pub fn remove_pid(name: &str) {
    let path = pid_file_path(name);
    let _ = std::fs::remove_file(&path);
}

/// 检查并清理孤儿进程
///
/// 在子进程启动前调用。读取 PID 文件，若存在且进程仍存活，则强杀该进程及其子进程树。
/// 清理后删除 PID 文件，无论进程是否被成功杀掉。
///
/// `name` 为服务标识（如 "ollama" / "gpt_sovits" / "fish_speech" / "whisper"）。
pub fn cleanup_orphan(name: &str) {
    let path = pid_file_path(name);
    let pid_str = match std::fs::read_to_string(&path) {
        Ok(s) => s.trim().to_string(),
        Err(_) => return, // 文件不存在，无孤儿
    };
    let pid: u32 = match pid_str.parse() {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!("[PidFile] {name}.pid 内容无效: {pid_str:?}，删除文件");
            let _ = std::fs::remove_file(&path);
            return;
        }
    };

    // 检查进程是否存活
    if !is_process_alive(pid) {
        tracing::info!("[PidFile] 孤儿进程 {name} pid={pid} 已不存在，删除 PID 文件");
        let _ = std::fs::remove_file(&path);
        return;
    }

    tracing::warn!("[PidFile] 检测到上次崩溃残留的孤儿进程 {name} pid={pid}，尝试清理");
    kill_process_tree(pid);
    // 等待进程退出（最多 3s）
    for _ in 0..30 {
        if !is_process_alive(pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if is_process_alive(pid) {
        tracing::error!("[PidFile] 孤儿进程 {name} pid={pid} 清理失败，仍存活");
    } else {
        tracing::info!("[PidFile] 孤儿进程 {name} pid={pid} 已清理");
    }
    let _ = std::fs::remove_file(&path);
}

/// 检查进程是否存活
#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// 强杀进程及其子进程树
#[cfg(windows)]
fn kill_process_tree(pid: u32) {
    // taskkill /F /T /PID 强杀进程树
    let output = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output();
    match output {
        Ok(o) => {
            if !o.status.success() {
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::warn!("[PidFile] taskkill pid={pid} 失败: {stderr}");
            }
        }
        Err(e) => tracing::warn!("[PidFile] taskkill pid={pid} 启动失败: {e}"),
    }
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(not(windows))]
fn kill_process_tree(pid: u32) {
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
}
