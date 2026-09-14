//! 受限令牌子进程沙箱 —— PowerShell 工具脚本执行专用（方案 C 第一阶段）
//!
//! DynamicTool 执行的是插件/智能体提供的不可信脚本。此前子进程是全权限用户
//! 进程（Job Object 只做生命周期遏制）。本模块把子进程放进受限执行环境：
//! - **特权全剥**：`CreateRestrictedToken(DISABLE_MAX_PRIVILEGE)`——调试、
//!   备份、加载驱动等特权一概不可用
//! - **restricting SID**：Everyone + BUILTIN\Users——访问检查按"标准用户"
//!   二次评审，显式授予当前用户 SID 的路径（用户资料目录）被拒之门外
//! - **低完整性级别**（S-1-16-4096）：强制完整性控制下不可写 Medium/High
//!   IL 对象——用户文件与 HKCU 常规注册表键均拒绝写入
//! - **可写面收窄**：唯一写入出口是 scratch 目录（`%TEMP%\vivian-sandbox-low`，
//!   打 Low IL 标签 + Everyone RW DACL），子进程 TMP/TEMP 指向它
//! - **网络不拦**（本阶段取舍）：fetch 型脚本需要网络，且命令清单已经
//!   AST 审计呈现给用户
//!
//! spawn 走 `CreateProcessAsUserW`（`std::process::Command` 不支持指定令牌）；
//! 管道句柄经 STARTUPINFO 标准句柄 + `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`
//! 白名单继承。与审计同一原则：沙箱建立失败 = 拒绝执行，不回退全权限。

use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};

/// 沙箱执行结果
#[derive(Debug, Clone)]
pub struct SandboxOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// true = 超时被强杀
    pub timed_out: bool,
}

const LOW_IL_SID: &str = "S-1-16-4096";

/// 单路输出读取上限（字节）；与 DynamicTool 的字符截断互补
const OUTPUT_CAP: usize = 4 * 1024 * 1024;

// ============================================================================
// 辅助
// ============================================================================

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 拥有式 HANDLE 包装（Drop 关闭）
struct HandleWrap(HANDLE);

impl HandleWrap {
    fn new(h: HANDLE) -> Self {
        Self(h)
    }
    fn get(&self) -> HANDLE {
        self.0
    }
}

impl Drop for HandleWrap {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// ============================================================================
// scratch 目录（子进程唯一可写面）
// ============================================================================

/// scratch 目录（懒初始化，进程内复用）：`%TEMP%\vivian-sandbox-low`
///
/// 安全属性：DACL 授 Everyone 完全访问（restricted token 的允许判定走
/// restricting SID，授予当前用户 SID 无效），强制标签 Low——低箱进程可写，
/// 宿主（Medium）写入向下不受限。
fn scratch_dir() -> Result<PathBuf, String> {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(dir) = DIR.get() {
        return Ok(dir.clone());
    }
    let dir = std::env::temp_dir().join("vivian-sandbox-low");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 scratch 目录失败: {e}"))?;
    apply_scratch_security(&dir)?;
    let _ = DIR.set(dir.clone());
    Ok(dir)
}

/// 给 scratch 目录打 Low IL 标签 + Everyone 完全访问 DACL
fn apply_scratch_security(dir: &std::path::Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, GetSecurityDescriptorSacl, DACL_SECURITY_INFORMATION,
        LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    unsafe {
        // Everyone 完全访问 + 强制标签 Low（NW：完整性低于对象者不可写）
        let sddl = wide("D:PAI(A;OICI;FA;;;WD)S:PAI(ML;;NW;;;LW)");
        let mut sd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
        .map_err(|e| format!("构造 scratch 安全描述符失败: {e}"))?;

        let mut dacl: *mut windows::Win32::Security::ACL = std::ptr::null_mut();
        let mut dacl_present = windows::core::BOOL(0);
        let mut dacl_defaulted = windows::core::BOOL(0);
        let _ = GetSecurityDescriptorDacl(sd, &mut dacl_present, &mut dacl, &mut dacl_defaulted);
        let mut sacl: *mut windows::Win32::Security::ACL = std::ptr::null_mut();
        let mut sacl_present = windows::core::BOOL(0);
        let mut sacl_defaulted = windows::core::BOOL(0);
        let _ = GetSecurityDescriptorSacl(sd, &mut sacl_present, &mut sacl, &mut sacl_defaulted);

        let path_w = wide(&dir.to_string_lossy());
        let err = SetNamedSecurityInfoW(
            PCWSTR(path_w.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
            None,
            None,
            Some(dacl),
            Some(sacl),
        );
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if err != windows::Win32::Foundation::WIN32_ERROR(0) {
            return Err(format!("设置 scratch 目录安全属性失败: WIN32_ERROR({:?})", err.0));
        }
        Ok(())
    }
}

// ============================================================================
// 受限令牌
// ============================================================================

/// 当前进程令牌 → 受限主令牌（全特权剥离 + Everyone/Users restricting SID + Low IL）
fn build_restricted_token() -> Result<HandleWrap, String> {
    use windows::Win32::Security::{
        CreateRestrictedToken, CreateWellKnownSid, SetTokenInformation, DISABLE_MAX_PRIVILEGE,
        SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY, TokenIntegrityLevel, WinBuiltinUsersSid, WinWorldSid,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut base = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT,
            &mut base,
        )
            .map_err(|e| format!("打开进程令牌失败: {e}"))?;
        let base = HandleWrap::new(base);

        // restricting SIDs：Everyone + BUILTIN\Users
        let mut restrict_sids: Vec<SID_AND_ATTRIBUTES> = Vec::new();
        let mut sid_buffers: Vec<Vec<u8>> = Vec::new();
        for kind in [WinWorldSid, WinBuiltinUsersSid] {
            let mut buf = vec![0u8; 68]; // SECURITY_MAX_SID_SIZE
            let mut size = buf.len() as u32;
            CreateWellKnownSid(kind, None, Some(windows::Win32::Security::PSID(buf.as_mut_ptr() as *mut _)), &mut size)
                .map_err(|e| format!("构造 well-known SID 失败: {e}"))?;
            sid_buffers.push(buf);
        }
        for buf in &sid_buffers {
            restrict_sids.push(SID_AND_ATTRIBUTES {
                Sid: windows::Win32::Security::PSID(buf.as_ptr() as *mut _),
                Attributes: 0,
            });
        }

        // 先在主令牌副本上降完整性（SetTokenInformation 需要 TOKEN_ADJUST_DEFAULT，
        // 且必须发生在 CreateRestrictedToken 之前——IL 随令牌派生，受限令牌上
        // 直接改 IL 会被拒绝）
        let low_sid_buf = string_to_sid(LOW_IL_SID)?;
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: windows::Win32::Security::PSID(low_sid_buf.as_ptr() as *mut _),
                Attributes: 0,
            },
        };
        let label_size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() + low_sid_buf.len();
        SetTokenInformation(
            base.get(),
            TokenIntegrityLevel,
            &label as *const _ as *const core::ffi::c_void,
            label_size as u32,
        )
        .map_err(|e| format!("设置完整性级别失败: {e}"))?;

        // 再从低完整性主令牌派生受限令牌（本身就是 primary，可直接 spawn）
        let mut restricted = HANDLE::default();
        CreateRestrictedToken(
            base.get(),
            DISABLE_MAX_PRIVILEGE,
            None,
            None,
            Some(&restrict_sids),
            &mut restricted,
        )
        .map_err(|e| format!("构造受限令牌失败: {e}"))?;
        Ok(HandleWrap::new(restricted))
    }
}

/// SDDL 字符串 → 自有缓冲的 SID 字节
fn string_to_sid(sddl: &str) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows::core::PCWSTR;

    unsafe {
        let w = wide(sddl);
        let mut psid = windows::Win32::Security::PSID(std::ptr::null_mut());
        ConvertStringSidToSidW(PCWSTR(w.as_ptr()), &mut psid)
            .map_err(|e| format!("解析 SID 字符串失败: {e}"))?;
        if psid.0.is_null() {
            return Err("SID 解析为空".into());
        }
        // SID 总长 = 1(Revision) + 1(SubAuthorityCount) + 6(IdentifierAuthority) + 4*count
        let count = *(psid.0 as *const u8).add(1) as usize;
        let len = 8 + 4 * count;
        let mut buf = vec![0u8; len];
        std::ptr::copy_nonoverlapping(psid.0 as *const u8, buf.as_mut_ptr(), len);
        let _ = LocalFree(Some(HLOCAL(psid.0)));
        Ok(buf)
    }
}

// ============================================================================
// 环境块
// ============================================================================

/// 用受限令牌构造环境块，TMP/TEMP 重定向到 scratch
fn build_env_block(token: HANDLE, scratch: &std::path::Path) -> Result<Vec<u16>, String> {
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};

    unsafe {
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        let have_env = CreateEnvironmentBlock(&mut raw, Some(token), false).is_ok() && !raw.is_null();

        let mut vars: Vec<(String, String)> = if have_env {
            parse_env_block(raw)
        } else {
            std::env::vars().collect()
        };
        if have_env {
            let _ = DestroyEnvironmentBlock(raw);
        }

        let scratch_str = scratch.to_string_lossy().into_owned();
        vars.retain(|(k, _)| !k.eq_ignore_ascii_case("TMP") && !k.eq_ignore_ascii_case("TEMP"));
        vars.push(("TMP".into(), scratch_str.clone()));
        vars.push(("TEMP".into(), scratch_str));

        // 双 null 结尾的 UTF-16 块：NAME=VALUE\0 ... \0\0
        let mut block: Vec<u16> = Vec::new();
        for (k, v) in &vars {
            block.extend(format!("{k}={v}").encode_utf16());
            block.push(0);
        }
        if block.is_empty() {
            block.push(0);
        }
        block.push(0);
        Ok(block)
    }
}

/// 解析 CreateEnvironmentBlock 返回的双 null 结尾 UTF-16 块
unsafe fn parse_env_block(raw: *mut core::ffi::c_void) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut p = raw as *const u16;
    if p.is_null() {
        return out;
    }
    loop {
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        if len == 0 {
            break; // 结束的双 null
        }
        let entry = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        if let Some((k, v)) = entry.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        }
        p = p.add(len + 1);
    }
    out
}

// ============================================================================
// 沙箱执行入口
// ============================================================================

/// 在受限令牌下执行 PowerShell 脚本（阻塞；调用方应置于 spawn_blocking）。
///
/// 脚本落盘到 scratch 目录（UTF-8 BOM——PS 5.1 对无 BOM 文件按 ANSI 解码），
/// 经 `-File` 执行（规避 `-Command` 的命令行引号转义面）；stdin 写入
/// `stdin_payload` 后关闭（脚本 `[Console]::In` 读到 EOF）。
pub fn run_powershell_sandboxed(
    script: &str,
    stdin_payload: &str,
    timeout_secs: u64,
) -> Result<SandboxOutput, String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT, HANDLE_FLAGS};
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, TerminateProcess, UpdateProcThreadAttribute,
        WaitForSingleObject, EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROCESS_INFORMATION,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    let scratch = scratch_dir()?;
    let token = build_restricted_token()?;
    let env_block = build_env_block(token.get(), &scratch)?;

    // 脚本落盘（BOM），用后删除
    let script_path =
        scratch.join(format!("vivian-script-{}.ps1", uuid::Uuid::new_v4().simple()));
    std::fs::write(&script_path, format!("\u{feff}{script}\n"))
        .map_err(|e| format!("写沙箱脚本文件失败: {e}"))?;

    // PowerShell 绝对路径（%SystemRoot% 定位，不依赖子进程 PATH）
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let ps_exe = format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let mut cmdline = wide(&format!(
        "\"{ps_exe}\" -NoProfile -NonInteractive -File \"{}\"",
        script_path.display()
    ));
    let appname = wide(&ps_exe);
    let cwd = wide(&scratch.to_string_lossy());

    unsafe {
        // 三对匿名管道；子进程侧句柄可继承，父进程侧关闭继承位
        let make_pipe = |name: &str| -> Result<(HandleWrap, HandleWrap), String> {
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: true.into(),
            };
            let mut read = HANDLE::default();
            let mut write = HANDLE::default();
            CreatePipe(&mut read, &mut write, Some(&sa), 0)
                .map_err(|e| format!("创建{name}管道失败: {e}"))?;
            Ok((HandleWrap::new(read), HandleWrap::new(write)))
        };
        let (stdin_read, stdin_write) = make_pipe("stdin")?;
        let (stdout_read, stdout_write) = make_pipe("stdout")?;
        let (stderr_read, stderr_write) = make_pipe("stderr")?;

        // 父进程侧句柄清除继承位（只有子进程侧三句柄进入继承白名单）
        for h in [stdin_write.get(), stdout_read.get(), stderr_read.get()] {
            SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))
                .map_err(|e| format!("清除父侧句柄继承失败: {e}"))?;
        }

        let child_stdin = stdin_read.get();
        let child_stdout = stdout_write.get();
        let child_stderr = stderr_write.get();
        let inherit_list: Vec<HANDLE> = vec![child_stdin, child_stdout, child_stderr];

        // 属性列表：句柄继承白名单（1 条）
        let mut attr_size: usize = 0;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut attr_size);
        let mut attr_buf = vec![0u8; attr_size];
        let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut core::ffi::c_void);
        InitializeProcThreadAttributeList(Some(attr_list), 1, None, &mut attr_size)
            .map_err(|e| format!("初始化属性列表失败: {e}"))?;
        UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(inherit_list.as_ptr() as *const _ as *mut core::ffi::c_void),
            std::mem::size_of::<HANDLE>() * inherit_list.len(),
            None,
            None,
        )
        .map_err(|e| format!("设置句柄白名单失败: {e}"))?;

        let mut siex = STARTUPINFOEXW::default();
        siex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        siex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        siex.StartupInfo.hStdInput = child_stdin;
        siex.StartupInfo.hStdOutput = child_stdout;
        siex.StartupInfo.hStdError = child_stderr;
        siex.lpAttributeList = attr_list;

        let mut pi = PROCESS_INFORMATION::default();
        CreateProcessAsUserW(
            Some(token.get()),
            PCWSTR(appname.as_ptr()),
            Some(PWSTR(cmdline.as_mut_ptr())),
            None,
            None,
            true,
            EXTENDED_STARTUPINFO_PRESENT,
            Some(env_block.as_ptr() as *const core::ffi::c_void),
            PCWSTR(cwd.as_ptr()),
            &siex.StartupInfo,
            &mut pi,
        )
        .map_err(|e| {
            let _ = std::fs::remove_file(&script_path);
            format!("沙箱启动 PowerShell 失败: {e}")
        })?;

        // 子进程侧句柄在父进程里立即关闭
        drop(stdin_read);
        drop(stdout_write);
        drop(stderr_write);
        DeleteProcThreadAttributeList(attr_list);

        let process = HandleWrap::new(pi.hProcess);
        let _thread = HandleWrap::new(pi.hThread);

        // Job Object 兜底回收（应用崩溃/强杀时 OS 回收子进程）
        crate::utils::job_object::assign_process(process.get().0 as isize);

        // stdin 写入后关闭（EOF）；from_raw_handle 接管句柄，File drop 即 CloseHandle
        {
            use std::io::Write;
            use std::os::windows::io::FromRawHandle;
            let mut file = std::fs::File::from_raw_handle(stdin_write.get().0 as *mut _);
            file.write_all(stdin_payload.as_bytes())
                .map_err(|e| format!("写沙箱 stdin 失败: {e}"))?;
            drop(file);
        }
        // stdin_write 包装与上面的 File 会对同一句柄 double close——
        // File 已 drop 关闭，这里让包装静默过期（不要 CloseHandle）
        std::mem::forget(stdin_write);

        // stdout/stderr 后台读取（限幅）；File 接管父侧读句柄，读取完成后 drop 关闭
        let read_pipe = |handle: HANDLE| -> std::thread::JoinHandle<Vec<u8>> {
            let raw = handle.0 as usize; // HANDLE 非 Send，线程内按裸值重建
            std::thread::spawn(move || {
                use std::io::Read;
                use std::os::windows::io::FromRawHandle;
                let mut file = std::fs::File::from_raw_handle(raw as *mut _);
                let mut out = Vec::new();
                let mut chunk = [0u8; 8192];
                while out.len() < OUTPUT_CAP {
                    match file.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => out.extend_from_slice(&chunk[..n]),
                    }
                }
                out
            })
        };
        let out_thread = read_pipe(stdout_read.get());
        let err_thread = read_pipe(stderr_read.get());
        std::mem::forget(stdout_read);
        std::mem::forget(stderr_read);

        // 带超时等待
        let wait_ms = timeout_secs.saturating_mul(1000).min(u32::MAX as u64) as u32;
        let mut timed_out = false;
        if WaitForSingleObject(process.get(), wait_ms) == WAIT_TIMEOUT {
            timed_out = true;
            let _ = TerminateProcess(process.get(), 1);
            let _ = WaitForSingleObject(process.get(), 5000);
        }

        let mut exit_code: u32 = 0;
        let _ = GetExitCodeProcess(process.get(), &mut exit_code);
        let stdout = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
        let stderr = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();

        // 清理脚本文件（best effort）
        let _ = std::fs::remove_file(&script_path);

        Ok(SandboxOutput {
            exit_code: exit_code as i32,
            stdout,
            stderr,
            timed_out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(script: &str, payload: &str) -> SandboxOutput {
        run_powershell_sandboxed(script, payload, 60).expect("沙箱执行应成功")
    }

    #[test]
    fn benign_script_runs_with_stdin_payload() {
        let out = run(
            "$p = [Console]::In.ReadToEnd() | ConvertFrom-Json; Write-Output \"hello-$($p.name)\"",
            r#"{"name":"sandbox"}"#,
        );
        assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
        assert!(out.stdout.contains("hello-sandbox"), "stdout: {}", out.stdout);
        assert!(!out.timed_out);
    }

    #[test]
    fn temp_redirects_to_scratch() {
        let out = run("Write-Output $env:TEMP", "");
        assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
        assert!(
            out.stdout.contains("vivian-sandbox-low"),
            "TEMP 应指向 scratch，实际: {}",
            out.stdout
        );
    }

    #[test]
    fn user_profile_write_denied() {
        // 低完整性 + Users restricting SID：用户资料目录写不进去
        let out = run(
            r#"try { Set-Content -Path (Join-Path $env:USERPROFILE 'vivian-sandbox-denied.txt') -Value 'x' -ErrorAction Stop; Write-Output 'WROTE' } catch { Write-Output 'DENIED' }"#,
            "",
        );
        assert!(
            out.stdout.contains("DENIED"),
            "写用户资料应被拒绝，stdout: {} stderr: {}",
            out.stdout,
            out.stderr
        );
        // 兜底清理（不应存在，但防测试残留）
        let home = std::env::var("USERPROFILE").map(PathBuf::from).unwrap();
        let _ = std::fs::remove_file(home.join("vivian-sandbox-denied.txt"));
    }

    #[test]
    fn exit_code_passthrough() {
        let out = run("exit 3", "");
        assert_eq!(out.exit_code, 3);
        assert!(!out.timed_out);
    }
}
