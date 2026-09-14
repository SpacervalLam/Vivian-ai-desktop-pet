//! 隐藏窗口的子进程启动封装
//!
//! Windows 上 `std::process::Command::spawn()` 默认会为子进程创建控制台窗口，
//! 工具频繁调用（powershell / cmd / reg / wallpaper64.exe / ffmpeg / python 等）
//! 会导致屏幕不停闪黑框。本模块统一封装 `CREATE_NO_WINDOW` 标志，所有工具
//! 启动外部进程都应通过这里的 `silent_command` / `silent_command_async`，
//! 避免新增工具时遗漏隐藏窗口配置。
//!
//! 非Windows 平台为 noop。

use std::ffi::OsStr;
use std::process::Command as StdCommand;
use tokio::process::Command as TokioCommand;

/// Windows `CREATE_NO_WINDOW` 标志：阻止系统为子进程创建控制台窗口。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 创建已预设 `CREATE_NO_WINDOW` 的同步 `std::process::Command`。
///
/// `program` 接受 `&str` / `&String` / `&PathBuf` / `&Path` 等任意 `AsRef<OsStr>` 类型，
/// 与原 `Command::new` 行为一致。调用方可继续链式配置 `.args()` / `.arg()` /
/// `.env()` / `.current_dir()` 等方法。
pub fn silent_command<S: AsRef<OsStr>>(program: S) -> StdCommand {
    let mut cmd = StdCommand::new(program);
    apply_no_window(&mut cmd);
    cmd
}

/// 创建已预设 `CREATE_NO_WINDOW` 的异步 `tokio::process::Command`。
pub fn silent_command_async<S: AsRef<OsStr>>(program: S) -> TokioCommand {
    let mut cmd = TokioCommand::new(program);
    apply_no_window_async(&mut cmd);
    cmd
}

#[cfg(windows)]
fn apply_no_window(cmd: &mut StdCommand) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn apply_no_window(_cmd: &mut StdCommand) {}

#[cfg(windows)]
fn apply_no_window_async(cmd: &mut TokioCommand) {
    // tokio::process::Command 在 Windows 上提供 creation_flags inherent method。
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn apply_no_window_async(_cmd: &mut TokioCommand) {}

/// 将 tokio 子进程绑定到全局 Job Object
///
/// 在 `Command::spawn()` 成功返回后立即调用，使子进程及其子孙进程
/// 在应用退出（含 panic/强杀）时被 OS 自动终止。
pub fn assign_child_to_job(child: &tokio::process::Child) -> bool {
    #[cfg(windows)]
    {
        match child.raw_handle() {
            Some(handle) => crate::utils::job_object::assign_process(handle as isize),
            None => false,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        false
    }
}
