//! 交互终端（ConPTY）—— 持久 PTY 会话能力
//!
//! 基于 Windows 10 1809+ 内置的 ConPTY 伪控制台（CreatePseudoConsole），
//! 零新增第三方 crate（复用 windows crate 的 feature）。每个终端会话：
//! - 一对匿名管道（stdin 写入 / stdout 读取）
//! - 子进程：powershell.exe -NoLogo -NoExit（UTF-8 编码修正，避免 GBK 乱码）
//! - 独立读线程：阻塞 ReadFile → UTF-8 边界切分 → `terminal:data` 事件广播
//! - 前端（xterm.js）onData → `terminal_write` 回写；onResize → `terminal_resize`
//!
//! 会话上限 4 个，防止句柄泄漏；kill 时 Terminate → ClosePseudoConsole → 句柄清理。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

/// 最大并发终端会话数
const MAX_SESSIONS: usize = 4;

/// 单个 PTY 会话的资源集合（句柄裸值，进程全局有效）
struct PtySession {
    hpcon: isize,
    input_write: isize,
    process: isize,
    thread_handle: isize,
}

/// 全局会话表
static SESSIONS: Lazy<Mutex<HashMap<String, PtySession>>> = Lazy::new(|| Mutex::new(HashMap::new()));
/// 会话计数
static SESSION_COUNT: AtomicUsize = AtomicUsize::new(0);

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ============================================================================
// Tauri 命令
// ============================================================================

/// 创建交互终端会话（ConPTY + powershell）。返回 session_id。
#[tauri::command]
pub fn terminal_create(
    app: AppHandle,
    working_directory: String,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<String, String> {
    if SESSION_COUNT.load(Ordering::SeqCst) >= MAX_SESSIONS {
        return Err(format!("终端会话已达上限（{MAX_SESSIONS}），请先关闭其他终端"));
    }
    let id = format!("term-{}", uuid::Uuid::new_v4().simple());
    spawn_pty(&app, &id, &working_directory, cols.unwrap_or(80), rows.unwrap_or(24))?;
    SESSION_COUNT.fetch_add(1, Ordering::SeqCst);
    Ok(id)
}

/// 向终端写入输入（前端 xterm onData 转发；回显由 ConPTY/PowerShell 负责）
#[tauri::command]
pub fn terminal_write(session_id: String, data: String) -> Result<(), String> {
    let guard = SESSIONS.lock();
    let Some(s) = guard.get(&session_id) else {
        return Err("终端会话不存在".into());
    };
    pty_write(s.input_write, data.as_bytes())
}

/// 终端尺寸变更（前端 xterm onResize 转发）
#[tauri::command]
pub fn terminal_resize(session_id: String, cols: u16, rows: u16) -> Result<(), String> {
    let guard = SESSIONS.lock();
    let Some(s) = guard.get(&session_id) else {
        return Err("终端会话不存在".into());
    };
    unsafe {
        let size = COORD { X: cols as i16, Y: rows as i16 };
        ResizePseudoConsole(HPCON(s.hpcon), size)
            .map_err(|e| format!("resize 失败: {e}"))?;
    }
    Ok(())
}

/// 终止终端会话并释放全部资源
#[tauri::command]
pub fn terminal_kill(app: AppHandle, session_id: String) -> Result<(), String> {
    kill_session(&app, &session_id);
    Ok(())
}

/// 列出活跃终端会话 ID
#[tauri::command]
pub fn terminal_list() -> Vec<String> {
    SESSIONS.lock().keys().cloned().collect()
}

// ============================================================================
// ConPTY 核心
// ============================================================================

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    ClosePseudoConsole, COORD, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, PROCESS_INFORMATION,
};

fn spawn_pty(
    app: &AppHandle,
    id: &str,
    working_directory: &str,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    unsafe {
        // 1. 管道对：ConPTY 输入（本侧写 in_write）/ 输出（本侧读 out_read）
        let mut pty_in_read = HANDLE::default();
        let mut pty_in_write = HANDLE::default();
        let mut pty_out_read = HANDLE::default();
        let mut pty_out_write = HANDLE::default();
        CreatePipe(&mut pty_in_read, &mut pty_in_write, None, 0)
            .map_err(|e| format!("CreatePipe 失败: {e}"))?;
        CreatePipe(&mut pty_out_read, &mut pty_out_write, None, 0)
            .map_err(|e| format!("CreatePipe 失败: {e}"))?;

        // 2. 伪控制台（持有 in_read / out_write），随后关闭本侧副本让退出检测及时
        let size = COORD { X: cols as i16, Y: rows as i16 };
        let hpcon = CreatePseudoConsole(size, pty_in_read, pty_out_write, 0)
            .map_err(|e| format!("CreatePseudoConsole 失败: {e}"))?;
        let _ = CloseHandle(pty_in_read);
        let _ = CloseHandle(pty_out_write);

        // 3. 属性列表：把 HPCON 传给子进程
        let mut attr_size = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, Some(0), &mut attr_size);
        let mut attr_buf: Vec<u8> = vec![0u8; attr_size.max(1)];
        let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut core::ffi::c_void);
        InitializeProcThreadAttributeList(Some(attr_list), 1, Some(0), &mut attr_size)
            .map_err(|e| format!("InitializeProcThreadAttributeList 失败: {e}"))?;
        UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            Some(hpcon.0 as *const core::ffi::c_void),
            std::mem::size_of::<HPCON>(),
            None,
            None,
        )
        .map_err(|e| format!("UpdateProcThreadAttribute 失败: {e}"))?;

        // 4. 启动 powershell（NoExit 保持会话；UTF-8 修正避免中文乱码）
        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.lpAttributeList = attr_list;
        let cmdline = r#"powershell.exe -NoLogo -NoExit -Command "[Console]::InputEncoding=[Console]::OutputEncoding=[System.Text.Encoding]::UTF8""#;
        let mut cmdw = to_wide(cmdline);
        let cwdw = to_wide(if working_directory.is_empty() { "." } else { working_directory });
        let mut pi = PROCESS_INFORMATION::default();
        let result = CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(cmdw.as_mut_ptr())),
            None,
            None,
            false,
            EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR(cwdw.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(attr_list);
        result.map_err(|e| format!("CreateProcessW 失败: {e}"))?;

        // 5. 登记会话 + 读线程
        SESSIONS.lock().insert(
            id.to_string(),
            PtySession {
                hpcon: hpcon.0,
                input_write: pty_in_write.0 as isize,
                process: pi.hProcess.0 as isize,
                thread_handle: pi.hThread.0 as isize,
            },
        );

        let app_c = app.clone();
        let id_c = id.to_string();
        // HANDLE 含裸指针非 Send，读线程以 isize 传递后还原
        let out_read_raw = pty_out_read.0 as isize;
        std::thread::Builder::new()
            .name(format!("pty-reader-{id}"))
            .spawn(move || reader_loop(app_c, id_c, out_read_raw))
            .map_err(|e| format!("读线程启动失败: {e}"))?;
        Ok(())
    }
}

/// 读线程：持续读 ConPTY 输出并广播（UTF-8 多字节截断安全：残余字节留到下一批）
fn reader_loop(app: AppHandle, id: String, out_read_raw: isize) {
    let out_read = HANDLE(out_read_raw as *mut core::ffi::c_void);
    unsafe {
        let mut buf = [0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            let mut nread = 0u32;
            if ReadFile(out_read, Some(&mut buf), Some(&mut nread), None).is_err() || nread == 0 {
                break;
            }
            pending.extend_from_slice(&buf[..nread as usize]);
            match std::str::from_utf8(&pending) {
                Ok(s) => {
                    let _ = app.emit("terminal:data", serde_json::json!({ "session_id": id, "data": s }));
                    pending.clear();
                }
                Err(e) if e.error_len().is_none() => {
                    // 截断：输出合法前缀，残余留待下一批补齐
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        let s = std::str::from_utf8(&pending[..valid]).unwrap_or("");
                        let _ = app.emit("terminal:data", serde_json::json!({ "session_id": id, "data": s }));
                        pending.drain(..valid);
                    }
                }
                Err(_) => {
                    let s = String::from_utf8_lossy(&pending).into_owned();
                    let _ = app.emit("terminal:data", serde_json::json!({ "session_id": id, "data": s }));
                    pending.clear();
                }
            }
        }
        let _ = CloseHandle(out_read);
        let _ = app.emit("terminal:exit", serde_json::json!({ "session_id": id }));
    }
}

/// 写入 stdin 管道。
fn pty_write(input_write: isize, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let mut written = 0u32;
        WriteFile(HANDLE(input_write as *mut core::ffi::c_void), Some(bytes), Some(&mut written), None)
            .map_err(|e| format!("写入失败: {e}"))?;
    }
    Ok(())
}

/// 终止并清理会话资源（Terminate → ClosePseudoConsole → 句柄清理 → 计数递减）。
fn kill_session(app: &AppHandle, id: &str) {
    let Some(s) = SESSIONS.lock().remove(id) else {
        return;
    };
    unsafe {
        // 终止子进程后读线程的 ReadFile 随管道断开退出
        let _ = TerminateProcess(HANDLE(s.process as *mut core::ffi::c_void), 0);
        let _ = app.emit("terminal:exit", serde_json::json!({ "session_id": id }));
        ClosePseudoConsole(HPCON(s.hpcon));
        let _ = CloseHandle(HANDLE(s.input_write as *mut core::ffi::c_void));
        let _ = CloseHandle(HANDLE(s.process as *mut core::ffi::c_void));
        let _ = CloseHandle(HANDLE(s.thread_handle as *mut core::ffi::c_void));
    }
    SESSION_COUNT.fetch_sub(1, Ordering::SeqCst);
}
