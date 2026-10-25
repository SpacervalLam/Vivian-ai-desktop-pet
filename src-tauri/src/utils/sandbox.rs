//! 受限令牌子进程沙箱 —— PowerShell 工具脚本执行专用（方案 C 第一阶段）
//!
//! DynamicTool 执行的是插件/智能体提供的不可信脚本。此前子进程是全权限用户
//! 进程（Job Object 只做生命周期遏制）。本模块把子进程放进受限执行环境：
//! - **特权全剥**：`CreateRestrictedToken(DISABLE_MAX_PRIVILEGE)`——调试、
//!   备份、加载驱动等特权一概不可用
//! - **写入受限令牌**：Everyone + BUILTIN\Users + 当前登录会话作为 restricting SID，
//!   写访问做二次检查；读取沿用用户权限，避免阻断 PowerShell/.NET 的启动读取
//! - **低完整性级别**（S-1-16-4096）：强制完整性控制下不可写 Medium/High
//!   IL 对象——用户文件与 HKCU 常规注册表键均拒绝写入
//! - **可写面收窄**：scratch 目录和显式授权的工作区带 Low IL 标签 + 当前登录会话写
//!   DACL；子进程 TMP/TEMP 指向 `%TEMP%\vivian-sandbox-low` 下的 scratch
//! - **网络策略**：通过环境变量与本地代理提供协作式约束，不是防火墙级隔离
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

/// 策略类型（网络模式、执行请求描述）放在零依赖的 `sandbox_policy` 里：
/// 隔离层不该反向依赖配置层，且那部分要能在隔离 harness 里单独编译验证。
///
/// 单路输出上限的默认 4 MiB 见 `SandboxRequest::cap`。
pub use crate::utils::sandbox_policy::{SandboxNetworkMode, SandboxRequest};

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

    fn into_file(self) -> std::fs::File {
        use std::os::windows::io::FromRawHandle;
        let raw = self.0 .0;
        std::mem::forget(self);
        unsafe { std::fs::File::from_raw_handle(raw) }
    }
}

impl Drop for HandleWrap {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct AttributeList(windows::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST);
impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Threading::DeleteProcThreadAttributeList(self.0);
        }
    }
}

struct ScriptFile(PathBuf);
impl Drop for ScriptFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Per-execution lifetime, nested inside the application's crash-recovery job.
/// Handles are stored as integers so a persistent shell can cross async tasks.
struct ProcessJob(usize);
impl ProcessJob {
    fn start(process: HANDLE, thread: HANDLE) -> Result<Self, String> {
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows::Win32::System::Threading::{ResumeThread, TerminateProcess};
        unsafe {
            let setup = || -> Result<Self, String> {
                let raw = CreateJobObjectW(None, windows::core::PCWSTR::null())
                    .map_err(|e| format!("创建沙箱进程 Job 失败: {e}"))?;
                let job = Self(raw.0 as usize);
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    raw,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of_val(&info) as u32,
                )
                .map_err(|e| format!("设置沙箱进程 Job 失败: {e}"))?;
                crate::utils::job_object::assign_process(process.0 as isize);
                AssignProcessToJobObject(raw, process)
                    .map_err(|e| format!("绑定沙箱进程 Job 失败: {e}"))?;
                if ResumeThread(thread) == u32::MAX {
                    return Err("恢复沙箱进程线程失败".into());
                }
                Ok(job)
            };
            let result = setup();
            if result.is_err() {
                let _ = TerminateProcess(process, 1);
            }
            result
        }
    }
    fn kill(&self) {
        unsafe {
            let _ =
                windows::Win32::System::JobObjects::TerminateJobObject(HANDLE(self.0 as *mut _), 1);
        }
    }
}
impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(HANDLE(self.0 as *mut _));
        }
    }
}

// ============================================================================
// scratch 目录（显式授权工作区以外的临时可写面）
// ============================================================================

/// scratch 目录（懒初始化，进程内复用）：`%TEMP%\vivian-sandbox-low`
///
/// 安全属性：DACL 授当前登录会话与 SYSTEM 完全访问（restricted token 的允许判定走
/// restricting SID，当前登录 SID 同时存在于允许与 restricting 列表），强制标签 Low——低箱进程可写，
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

/// 按会话隔离的 scratch 目录：`%TEMP%\vivian-sandbox-low\s-<会话标识>`
///
/// 分目录避免临时文件混用和名称碰撞。各沙箱仍使用同一 Windows 登录 SID，
/// 因此目录划分不提供会话之间的 OS 级读写隔离。
fn scratch_dir_for(session_id: &str) -> Result<PathBuf, String> {
    let base = scratch_dir()?;
    if session_id.trim().is_empty() {
        return Ok(base);
    }
    // 会话 ID 可能含路径分隔符，只保留安全字符
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut hash);
    let dir = base.join(format!(
        "s-{}-{:016x}",
        if safe.is_empty() { "default" } else { &safe },
        hash.finish()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建会话 scratch 目录失败: {e}"))?;
    // 修复旧版本创建的 Medium IL 会话目录；仅依赖父目录继承无法修复已有目录。
    apply_scratch_security(&dir)?;
    Ok(dir)
}

/// 为请求里声明的工作区授予沙箱写权限
///
/// 授权是一次性的持久改动，失败只降级（工作区变只读），不阻断命令执行。
/// 返回 (成功数, 首条错误)。
fn grant_workspaces(req: &SandboxRequest) -> (usize, Option<String>) {
    let mut ok = 0usize;
    let mut first_err: Option<String> = None;
    for dir in &req.workspace_dirs {
        if dir.trim().is_empty() {
            continue;
        }
        let path = PathBuf::from(dir);
        if !path.exists() {
            continue;
        }
        let outcome = crate::utils::sandbox_workspace::grant_workspace_write(&path);
        if outcome.usable() {
            ok += 1;
        } else if let Some(e) = outcome.error {
            if first_err.is_none() {
                first_err = Some(format!("{}: {}", dir, e));
            }
        } else if first_err.is_none() {
            first_err = Some(format!(
                "工作区授权不完整: {dir}（已处理 {} 项）",
                outcome.tagged
            ));
        }
    }
    if ok > 0 || !req.workspace_dirs.is_empty() {
        tracing::info!(
            "[sandbox] 工作区授权: 成功 {}/{}，错误 {:?}",
            ok,
            req.workspace_dirs.len(),
            first_err
        );
    }
    (ok, first_err)
}

/// 给 scratch 目录打 Low IL 标签 + 当前登录会话 DACL
fn apply_scratch_security(dir: &std::path::Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, GetSecurityDescriptorSacl, DACL_SECURITY_INFORMATION,
        LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    unsafe {
        // 当前登录会话访问 + 强制标签 Low（NW：完整性低于对象者不可写）
        let logon = current_logon_sid_sddl()?;
        let sddl = wide(&format!(
            "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;{logon})S:PAI(ML;OICI;NW;;;LW)"
        ));
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
            return Err(format!(
                "设置 scratch 目录安全属性失败: WIN32_ERROR({:?})",
                err.0
            ));
        }
        Ok(())
    }
}

// ============================================================================
// 完整性级别（宿主自检）
// ============================================================================

/// 进程完整性级别（Windows 强制完整性控制，SID 前缀 `S-1-16-*`）。
///
/// 这里只做**观测**：一旦宿主主令牌被降成 Low，进程自己升不回去
/// （no-write-up 同样作用于令牌对象），唯一的出路是重启进程换一份干净令牌。
/// 所以调用方拿到 [`IntegrityLevel::Low`] 时该做的是告警并引导用户重启，
/// 而不是尝试修正。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityLevel {
    Untrusted,
    Low,
    Medium,
    High,
    System,
    /// 读不到（无权限或非预期 SID）。按"未知"处理，不当成异常。
    Unknown,
}

impl IntegrityLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::System => "system",
            Self::Unknown => "unknown",
        }
    }

    fn from_rid(rid: u32) -> Self {
        match rid & 0x0000_F000 {
            0x0000_0000 => Self::Untrusted,
            0x0000_1000 => Self::Low,
            0x0000_2000 => Self::Medium,
            0x0000_3000 => Self::High,
            0x0000_4000 => Self::System,
            _ => Self::Unknown,
        }
    }

    /// 宿主写自己的用户数据目录（`%APPDATA%\vivian`）至少需要 Medium。
    ///
    /// Low 及以下在 no-write-up 规则下对所有 Medium 对象 `ACCESS_DENIED`，
    /// 表现为配置保存、记忆、日志、心理状态全线失败——而读取一切正常，
    /// 因此很容易被误判成"某个按钮坏了"。
    pub fn can_write_user_data(self) -> bool {
        !matches!(self, Self::Untrusted | Self::Low)
    }
}

/// 读某个令牌对象的完整性级别。
///
/// `TOKEN_MANDATORY_LABEL` 尾部的 SID 就是 IL：`S-1-16-<rid>`，
/// 最后一个 sub-authority 决定档位（0x1000 = Low，0x2000 = Medium...）。
fn token_integrity_level(token: HANDLE) -> IntegrityLevel {
    use windows::Win32::Security::{
        GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL,
    };

    unsafe {
        let mut needed = 0u32;
        // 第一次只为拿长度，失败即视为读不到
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
        if needed == 0 {
            return IntegrityLevel::Unknown;
        }
        let mut buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            needed,
            &mut needed,
        )
        .is_err()
        {
            return IntegrityLevel::Unknown;
        }
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        if sid.0.is_null() {
            return IntegrityLevel::Unknown;
        }
        // SID 布局：Revision(1) + SubAuthorityCount(1) + IdentifierAuthority(6) + u32 * count
        let count = *(sid.0 as *const u8).add(1) as usize;
        if count == 0 {
            return IntegrityLevel::Unknown;
        }
        let last =
            std::ptr::read_unaligned((sid.0 as *const u8).add(8 + 4 * (count - 1)) as *const u32);
        IntegrityLevel::from_rid(last)
    }
}

/// 当前进程的完整性级别。
pub fn current_integrity_level() -> IntegrityLevel {
    use windows::Win32::Security::TOKEN_QUERY;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return IntegrityLevel::Unknown;
        }
        let token = HandleWrap::new(token);
        token_integrity_level(token.get())
    }
}

// ============================================================================
// 受限令牌
// ============================================================================

/// 当前进程令牌 → 写入受限主令牌（全特权剥离 + restricting SID + Low IL）
///
/// **绝不改动当前进程的令牌。** `OpenProcessToken(GetCurrentProcess())` 打开的是本进程
/// 那个主令牌对象本身，不是副本；在它上面写 `TokenIntegrityLevel` 会把整个应用永久降成
/// Low IL（降得下去、升不回来）。后果是宿主此后写自己的配置 / 记忆 / 日志全部
/// `ACCESS_DENIED`，工作区授权也拿不到 `WRITE_DAC`——从调用方看像是「沙箱坏了」，
/// 实际是宿主被自己拖下了水。
///
/// 正确的派生顺序（每一步都在副本上）：
/// 1. 用未改动的进程令牌 `CreateRestrictedToken` —— 必须是原令牌，这样内核才认它
///    是「调用方自己主令牌的受限版本」，`CreateProcessAsUser` 不需要
///    `SeAssignPrimaryTokenPrivilege`（标准用户没有这个特权）
/// 2. `DuplicateTokenEx` 把受限令牌复制成独立的主令牌副本
/// 3. 只在副本上降 IL。**必须先复制再降**：直接在受限令牌上降 IL 会让第 1 步的
///    豁免失效，`CreateProcessAsUser` 以 `ERROR_ACCESS_DENIED(5)` 失败
fn build_restricted_token() -> Result<HandleWrap, String> {
    use windows::Win32::Security::{
        CreateRestrictedToken, CreateWellKnownSid, DuplicateTokenEx, SecurityImpersonation,
        SetTokenInformation, TokenIntegrityLevel, TokenPrimary, WinBuiltinUsersSid, WinWorldSid,
        DISABLE_MAX_PRIVILEGE, SID_AND_ATTRIBUTES, TOKEN_ALL_ACCESS, TOKEN_DUPLICATE,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, WRITE_RESTRICTED,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut base = HANDLE::default();
        // 只要读 + 派生权限：这里**不申请** TOKEN_ADJUST_DEFAULT，从权限层面就杜绝
        // 误改进程自身令牌的可能
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut base,
        )
        .map_err(|e| format!("打开进程令牌失败: {e}"))?;
        let base = HandleWrap::new(base);

        // restricting SIDs：Everyone + BUILTIN\Users + 当前登录会话。
        // 登录 SID 让会话系统对象及下方默认 DACL 同时通过两轮访问检查。
        let mut restrict_sids: Vec<SID_AND_ATTRIBUTES> = Vec::new();
        let mut sid_buffers: Vec<Vec<u8>> = Vec::new();
        for kind in [WinWorldSid, WinBuiltinUsersSid] {
            let mut buf = vec![0u8; 68]; // SECURITY_MAX_SID_SIZE
            let mut size = buf.len() as u32;
            CreateWellKnownSid(
                kind,
                None,
                Some(windows::Win32::Security::PSID(buf.as_mut_ptr() as *mut _)),
                &mut size,
            )
            .map_err(|e| format!("构造 well-known SID 失败: {e}"))?;
            sid_buffers.push(buf);
        }
        let logon = logon_sid(base.get())?;
        sid_buffers.push(logon.clone());
        for buf in &sid_buffers {
            restrict_sids.push(SID_AND_ATTRIBUTES {
                Sid: windows::Win32::Security::PSID(buf.as_ptr() as *mut _),
                Attributes: 0,
            });
        }

        // 第 1 步：从**未改动的**进程令牌派生受限令牌
        let mut restricted = HANDLE::default();
        CreateRestrictedToken(
            base.get(),
            // 完整的读取限制会阻断 PowerShell/.NET 启动（8007000e）。只对写入做
            // SID 二次检查；Low IL 仍阻止写用户数据、注册表等非授权的 Medium 对象。
            DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED,
            None,
            None,
            Some(&restrict_sids),
            &mut restricted,
        )
        .map_err(|e| format!("构造受限令牌失败: {e}"))?;
        let restricted = HandleWrap::new(restricted);

        // 第 2 步：复制成独立的主令牌副本——只有副本可以随便改
        let mut primary = HANDLE::default();
        DuplicateTokenEx(
            restricted.get(),
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
        .map_err(|e| format!("复制受限令牌失败: {e}"))?;
        let primary = HandleWrap::new(primary);
        grant_token_default_dacl(primary.get(), &logon)?;

        // 第 3 步：在副本上降完整性（SetTokenInformation 需要 TOKEN_ADJUST_DEFAULT，
        // 副本带 TOKEN_ALL_ACCESS，够用）
        let low_sid_buf = string_to_sid(LOW_IL_SID)?;
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: windows::Win32::Security::PSID(low_sid_buf.as_ptr() as *mut _),
                Attributes: 0,
            },
        };
        let label_size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() + low_sid_buf.len();
        SetTokenInformation(
            primary.get(),
            TokenIntegrityLevel,
            &label as *const _ as *const core::ffi::c_void,
            label_size as u32,
        )
        .map_err(|e| format!("设置完整性级别失败: {e}"))?;

        // 护栏：派生只该影响副本。宿主被降权过就再也升不回去，宁可在这里
        // 大声报错，也不要让用户几天后从"某个按钮点了没反应"反推。
        let host = current_integrity_level();
        if !host.can_write_user_data() {
            tracing::error!(
                "[sandbox] 派生受限令牌后宿主完整性级别降为 {}：主令牌被改动，\
                 此后配置/记忆/日志写入将全部 ACCESS_DENIED（需重启进程）",
                host.as_str()
            );
        }

        Ok(primary)
    }
}

/// 复制当前令牌的登录 SID（不是用户 SID），供受限进程访问会话内的系统对象。
fn logon_sid(token: HANDLE) -> Result<Vec<u8>, String> {
    use windows::Win32::Security::{
        GetLengthSid, GetTokenInformation, TokenLogonSid, TOKEN_GROUPS,
    };

    unsafe {
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenLogonSid, None, 0, &mut needed);
        if needed < std::mem::size_of::<TOKEN_GROUPS>() as u32 {
            return Err("查询登录 SID 大小失败".into());
        }
        let mut buf = vec![0u8; needed as usize];
        GetTokenInformation(
            token,
            TokenLogonSid,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .map_err(|e| format!("读取登录 SID 失败: {e}"))?;
        let groups = std::ptr::read_unaligned(buf.as_ptr().cast::<TOKEN_GROUPS>());
        if groups.GroupCount != 1 || groups.Groups[0].Sid.0.is_null() {
            return Err("令牌没有唯一登录 SID".into());
        }
        let sid = groups.Groups[0].Sid;
        let len = GetLengthSid(sid) as usize;
        Ok(std::slice::from_raw_parts(sid.0.cast::<u8>(), len).to_vec())
    }
}

pub(super) fn current_logon_sid_sddl() -> Result<String, String> {
    static SID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(sid) = SID.get() {
        return Ok(sid.clone());
    }
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{PSID, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut handle = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle)
            .map_err(|e| e.to_string())?;
        let handle = HandleWrap::new(handle);
        let sid = logon_sid(handle.get())?;
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(PSID(sid.as_ptr() as *mut _), &mut text)
            .map_err(|e| e.to_string())?;
        let result = text.to_string().map_err(|e| e.to_string());
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        let result = result?;
        let _ = SID.set(result.clone());
        Ok(result)
    }
}

/// 子进程创建的内核对象需要允许自身访问；原默认 DACL 常常只授权用户/SYSTEM，
/// 无法通过 restricting SID 的第二轮检查，KERNELBASE 初始化便会失败。
/// 仅合并当前登录 SID 的权限，保留原 ACL，且只修改派生令牌。
fn grant_token_default_dacl(token: HANDLE, logon: &[u8]) -> Result<(), String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{LocalFree, GENERIC_ALL, HLOCAL};
    use windows::Win32::Security::Authorization::{
        SetEntriesInAclW, EXPLICIT_ACCESS_W, GRANT_ACCESS, TRUSTEE_IS_SID, TRUSTEE_W,
    };
    use windows::Win32::Security::{
        GetTokenInformation, SetTokenInformation, TokenDefaultDacl, TOKEN_DEFAULT_DACL,
    };

    unsafe {
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenDefaultDacl, None, 0, &mut needed);
        if needed < std::mem::size_of::<TOKEN_DEFAULT_DACL>() as u32 {
            return Err("查询沙箱默认 DACL 大小失败".into());
        }
        let mut buf = vec![0u8; needed as usize];
        GetTokenInformation(
            token,
            TokenDefaultDacl,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .map_err(|e| format!("读取沙箱默认 DACL 失败: {e}"))?;
        let current = std::ptr::read_unaligned(buf.as_ptr().cast::<TOKEN_DEFAULT_DACL>());
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_ALL.0,
            grfAccessMode: GRANT_ACCESS,
            Trustee: TRUSTEE_W {
                TrusteeForm: TRUSTEE_IS_SID,
                ptstrName: PWSTR(logon.as_ptr() as *mut u16),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut acl = std::ptr::null_mut();
        SetEntriesInAclW(Some(&[entry]), Some(current.DefaultDacl), &mut acl)
            .ok()
            .map_err(|e| format!("合并沙箱默认 DACL 失败: {e}"))?;
        let updated = TOKEN_DEFAULT_DACL { DefaultDacl: acl };
        let result = SetTokenInformation(
            token,
            TokenDefaultDacl,
            &updated as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<TOKEN_DEFAULT_DACL>() as u32,
        );
        let _ = LocalFree(Some(HLOCAL(acl.cast())));
        result.map_err(|e| format!("设置沙箱默认 DACL 失败: {e}"))
    }
}

/// SDDL 字符串 → 自有缓冲的 SID 字节
fn string_to_sid(sddl: &str) -> Result<Vec<u8>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertStringSidToSidW;

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
fn build_env_block(
    token: HANDLE,
    scratch: &std::path::Path,
    req: &SandboxRequest,
) -> Result<Vec<u16>, String> {
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};

    unsafe {
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        let have_env =
            CreateEnvironmentBlock(&mut raw, Some(token), false).is_ok() && !raw.is_null();

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

        apply_network_env(&mut vars, req);

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

/// 按网络模式注入代理环境变量
///
/// **这是协作式约束，不是内核级边界。** 走环境变量意味着子进程里的程序只要
/// 不理这几个变量（直接 socket、显式 `-NoProxy`、自己构造 WebRequest）就能绕开。
/// 在受限令牌 + 低完整性这条路线上，Windows 没有轻量手段做按进程的网络过滤
/// （Job Object 只能限速不能断网，WFP 要驱动）——要真正的强制出口，得换成
/// 独立账户 + 按账户的防火墙规则，那是另一套架构。这里的目标是：让**按约定
/// 行事**的程序（curl / PowerShell / npm / git / .NET）按策略走，而不是假装
/// 挡得住有意绕行的代码。
fn apply_network_env(vars: &mut Vec<(String, String)>, req: &SandboxRequest) {
    if req.network_mode == SandboxNetworkMode::Allow {
        return;
    }
    let proxy_keys = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    vars.retain(|(k, _)| !proxy_keys.iter().any(|p| k.eq_ignore_ascii_case(p)));

    let target: Option<String> = match req.network_mode {
        SandboxNetworkMode::Allow => None,
        SandboxNetworkMode::Proxy => req
            .proxy_addr
            .as_ref()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .map(|a| format!("http://{a}"))
            .or_else(|| Some("http://127.0.0.1:0".into())),
        // 指向必然拒绝连接的回环端口：不依赖代理进程是否活着，
        // 未被白名单放行的请求连不上就是连不上
        SandboxNetworkMode::Blocked => Some("http://127.0.0.1:0".to_string()),
    };

    if let Some(url) = target {
        vars.retain(|(key, _)| !key.eq_ignore_ascii_case("NO_PROXY"));
        vars.push(("NO_PROXY".into(), String::new()));
        for k in proxy_keys {
            vars.push((k.to_string(), url.clone()));
        }
        tracing::info!(
            "[sandbox] 网络模式 {:?}，代理出口 {}",
            req.network_mode,
            url
        );
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
    run_powershell_sandboxed_with(
        &SandboxRequest::default(),
        script,
        stdin_payload,
        timeout_secs,
        None,
    )
}

/// 带完整请求描述执行 PowerShell 脚本（插件脚本路径）
///
/// `cwd` 为 None 时子进程工作目录落在 scratch；传工作区时要求该目录已经过
/// `grant_workspaces` 授权，否则进程能进目录但写不进去。
pub fn run_powershell_sandboxed_with(
    req: &SandboxRequest,
    script: &str,
    stdin_payload: &str,
    timeout_secs: u64,
    cwd: Option<&std::path::Path>,
) -> Result<SandboxOutput, String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, GetExitCodeProcess, InitializeProcThreadAttributeList,
        UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED,
        CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES,
        STARTUPINFOEXW,
    };

    let scratch = scratch_dir_for(&req.session_id)?;
    let token = build_restricted_token()?;
    let env_block = build_env_block(token.get(), &scratch, req)?;
    let _ = grant_workspaces(req);

    // 脚本落盘（BOM），用后删除
    let script_path = scratch.join(format!(
        "vivian-script-{}.ps1",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&script_path, format!("\u{feff}{script}\n"))
        .map_err(|e| format!("写沙箱脚本文件失败: {e}"))?;
    let _script_file = ScriptFile(script_path.clone());

    // PowerShell 绝对路径（%SystemRoot% 定位，不依赖子进程 PATH）
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let ps_exe = format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    // 内部生成的临时脚本只在本进程跳过签名检查，不修改系统/用户执行策略。
    let mut cmdline = wide(&format!(
        "\"{ps_exe}\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
        script_path.display()
    ));
    let appname = wide(&ps_exe);
    let cwd = wide(&cwd.unwrap_or(scratch.as_path()).to_string_lossy());

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
        let attr_list =
            LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut core::ffi::c_void);
        InitializeProcThreadAttributeList(Some(attr_list), 1, None, &mut attr_size)
            .map_err(|e| format!("初始化属性列表失败: {e}"))?;
        let _attributes = AttributeList(attr_list);
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
            // build_env_block 返回 UTF-16；W 版 API 仍要求显式声明环境块编码。
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW
                | CREATE_SUSPENDED,
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

        let process = HandleWrap::new(pi.hProcess);
        let _thread = HandleWrap::new(pi.hThread);

        // Job Object 兜底回收（应用崩溃/强杀时 OS 回收子进程）
        let job = ProcessJob::start(process.get(), _thread.get())?;

        // stdout/stderr 后台读取（限幅）；File 接管父侧读句柄，读取完成后 drop 关闭
        let cap = req.cap();
        let read_pipe = |handle: HANDLE| -> std::thread::JoinHandle<Vec<u8>> {
            let raw = handle.0 as usize; // HANDLE 非 Send，线程内按裸值重建
            std::thread::spawn(move || {
                use std::io::Read;
                use std::os::windows::io::FromRawHandle;
                let mut file = std::fs::File::from_raw_handle(raw as *mut _);
                let mut out = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match file.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let keep = n.min(cap.saturating_sub(out.len()));
                            out.extend_from_slice(&chunk[..keep]);
                        }
                    }
                }
                out
            })
        };
        let out_thread = read_pipe(stdout_read.get());
        let err_thread = read_pipe(stderr_read.get());
        std::mem::forget(stdout_read);
        std::mem::forget(stderr_read);

        // Readers and writer run concurrently: neither a full stdout pipe nor a
        // script that never reads stdin may hold up the timeout clock.
        let mut input = stdin_write.into_file();
        let payload = stdin_payload.as_bytes().to_vec();
        let input_thread = std::thread::spawn(move || {
            use std::io::Write;
            let _ = input.write_all(&payload);
        });
        // 带超时等待
        let wait_ms = timeout_secs.saturating_mul(1000).min(u32::MAX as u64) as u32;
        let mut timed_out = false;
        if WaitForSingleObject(process.get(), wait_ms) == WAIT_TIMEOUT {
            timed_out = true;
            job.kill();
            let _ = WaitForSingleObject(process.get(), 5000);
        }

        let mut exit_code: u32 = 0;
        let _ = GetExitCodeProcess(process.get(), &mut exit_code);
        job.kill();
        let _ = input_thread.join();
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

// ============================================================================
// 通用命令执行（one-shot）
// ============================================================================

/// 在沙箱里执行一条 PowerShell 命令（one-shot，跑完即走）
///
/// 命令走脚本文件而不是 `-Command`：PowerShell 5.1 的命令行引号转义是个已知的
/// 坑，`-File` 把原文整段交给解释器，语义与在持久终端里敲进去的一致。
///
/// 末尾补 `exit $LASTEXITCODE` 是为了把外部程序的退出码带出来——`-File` 结束
/// 默认返回 0，不加这句的话构建/测试的失败会被吞成成功。
pub fn run_command_sandboxed(
    req: &SandboxRequest,
    command: &str,
    timeout_secs: u64,
    cwd: Option<&std::path::Path>,
) -> Result<SandboxOutput, String> {
    let script = format!("{command}\nexit $LASTEXITCODE\n");
    run_powershell_sandboxed_with(req, &script, "", timeout_secs, cwd)
}

// ============================================================================
// 持久 shell（终端会话）
// ============================================================================

/// 沙箱内的持久 PowerShell 进程
///
/// 三个 stdio 交给 `std::fs::File` 持有（drop 即 CloseHandle），进程句柄按裸值
/// 保存——`HANDLE` 不是 `Send`，而终端句柄要跨 await 存活。
///
/// 与 one-shot 路径的区别：stdin **不关闭**，进程一直活着等下一条命令。
pub struct SandboxedShell {
    process: usize,
    job: ProcessJob,
    scratch: PathBuf,
    pub stdin: std::fs::File,
    pub stdout: std::fs::File,
    pub stderr: std::fs::File,
}

impl SandboxedShell {
    pub fn scratch_dir(&self) -> &std::path::Path {
        &self.scratch
    }
    /// 进程句柄裸值（供 Job Object 归属与等待）
    pub fn process_raw(&self) -> usize {
        self.process
    }

    /// 进程是否已退出；返回退出码
    pub fn try_wait(&self) -> Option<i32> {
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        let h = HANDLE(self.process as *mut core::ffi::c_void);
        let _ = unsafe { WaitForSingleObject(h, 0) };
        let mut code: u32 = 0;
        if unsafe { GetExitCodeProcess(h, &mut code) }.is_ok() {
            // STILL_ACTIVE (259) 表示还在跑
            if code != 259 {
                return Some(code as i32);
            }
        }
        None
    }

    /// 终止进程（终端关闭 / 超时兜底）
    pub fn kill(&self) {
        self.job.kill();
    }
}

impl Drop for SandboxedShell {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(HANDLE(self.process as *mut core::ffi::c_void)) };
    }
}

/// 在沙箱里起一个长期活着的 PowerShell（`terminal_open` 用）
///
/// 与 one-shot 同一套受限令牌 / 低完整性 / 环境块，只是 stdin 保持打开，进程
/// 常驻。`cwd` 传工作区时该目录需要先经过授权，否则 `cd` 过去只能读。
pub fn spawn_sandboxed_shell(
    req: &SandboxRequest,
    cwd: Option<&std::path::Path>,
) -> Result<SandboxedShell, String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, InitializeProcThreadAttributeList, UpdateProcThreadAttribute,
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
        EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    let scratch = scratch_dir_for(&req.session_id)?;
    let token = build_restricted_token()?;
    let env_block = build_env_block(token.get(), &scratch, req)?;
    let _ = grant_workspaces(req);

    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let ps_exe = format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let mut cmdline = wide(&format!(
        "\"{ps_exe}\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command -"
    ));
    let appname = wide(&ps_exe);
    let cwd_w = wide(&cwd.unwrap_or(scratch.as_path()).to_string_lossy());

    unsafe {
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

        // 父侧句柄清继承位：子进程只能拿到白名单里的三个子侧句柄
        for h in [stdin_write.get(), stdout_read.get(), stderr_read.get()] {
            SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))
                .map_err(|e| format!("清除父侧句柄继承失败: {e}"))?;
        }

        let inherit_list: Vec<HANDLE> =
            vec![stdin_read.get(), stdout_write.get(), stderr_write.get()];

        let mut attr_size: usize = 0;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut attr_size);
        let mut attr_buf = vec![0u8; attr_size];
        let attr_list =
            LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut core::ffi::c_void);
        InitializeProcThreadAttributeList(Some(attr_list), 1, None, &mut attr_size)
            .map_err(|e| format!("初始化属性列表失败: {e}"))?;
        let _attributes = AttributeList(attr_list);
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
        siex.StartupInfo.hStdInput = stdin_read.get();
        siex.StartupInfo.hStdOutput = stdout_write.get();
        siex.StartupInfo.hStdError = stderr_write.get();
        siex.lpAttributeList = attr_list;

        let mut pi = PROCESS_INFORMATION::default();
        CreateProcessAsUserW(
            Some(token.get()),
            PCWSTR(appname.as_ptr()),
            Some(PWSTR(cmdline.as_mut_ptr())),
            None,
            None,
            true,
            // 与一次性命令相同：显式环境块编码，并避免桌面上弹出控制台。
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW
                | CREATE_SUSPENDED,
            Some(env_block.as_ptr() as *const core::ffi::c_void),
            PCWSTR(cwd_w.as_ptr()),
            &siex.StartupInfo,
            &mut pi,
        )
        .map_err(|e| format!("沙箱启动持久终端失败: {e}"))?;

        drop(stdin_read);
        drop(stdout_write);
        drop(stderr_write);

        // Assign the suspended process before it can create descendants.
        let job = match ProcessJob::start(pi.hProcess, pi.hThread) {
            Ok(job) => job,
            Err(error) => {
                let _ = CloseHandle(pi.hThread);
                let _ = CloseHandle(pi.hProcess);
                return Err(error);
            }
        };
        // 主线程句柄不需要：只留进程句柄做等待与终止
        if !pi.hThread.is_invalid() {
            let _ = CloseHandle(pi.hThread);
        }

        // Job Object 兜底：宿主崩溃时 OS 回收进程树

        use std::os::windows::io::FromRawHandle;
        let stdin = std::fs::File::from_raw_handle(stdin_write.get().0 as *mut _);
        let stdout = std::fs::File::from_raw_handle(stdout_read.get().0 as *mut _);
        let stderr = std::fs::File::from_raw_handle(stderr_read.get().0 as *mut _);
        std::mem::forget(stdin_write);
        std::mem::forget(stdout_read);
        std::mem::forget(stderr_read);

        tracing::info!(
            "[sandbox] 持久终端已在受限令牌下启动，cwd={:?}",
            cwd.map(|p| p.display().to_string())
        );

        Ok(SandboxedShell {
            process: pi.hProcess.0 as usize,
            job,
            scratch,
            stdin,
            stdout,
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_token_never_downgrades_the_host() {
        // 这是本模块最贵的一条教训：`OpenProcessToken(GetCurrentProcess())` 拿到的是
        // 进程主令牌本身，在它上面写 `TokenIntegrityLevel` 会把整个应用永久降成
        // Low IL（降得下去、升不回来），此后宿主写自己的配置/记忆/日志全部
        // ACCESS_DENIED——对外表现为"侧边栏某个按钮点了没反应"。
        // 旧实现正是这么干的，这条测试就是那次回归的围栏。
        let before = current_integrity_level();
        assert!(
            before.can_write_user_data(),
            "测试进程本身已是 {} 完整性，无法验证派生行为（请从普通终端启动测试）",
            before.as_str()
        );

        let token = build_restricted_token().expect("从宿主令牌派生受限令牌应成功");

        let after = current_integrity_level();
        assert_eq!(
            before,
            after,
            "派生沙箱令牌把宿主降成了 {}——降下去升不回来",
            after.as_str()
        );
        // 反过来也要守住：交给子进程的那份令牌必须真的是 Low，否则沙箱形同虚设。
        assert_eq!(
            token_integrity_level(token.get()),
            IntegrityLevel::Low,
            "沙箱子令牌必须是 Low IL"
        );
    }

    #[test]
    #[ignore = "requires Windows restricted-token process privileges; run test:windows-integration in a supported environment"]
    fn sandbox_command_keeps_host_integrity() {
        // 端到端版：真的跑一次沙箱命令，宿主完整性级别必须原样。
        // 派生令牌是否干净由 restricted_token_never_downgrades_the_host 守住，
        // 这条守的是 spawn 路径（环境块 / 句柄继承 / CreateProcessAsUser）。
        let before = current_integrity_level();
        assert!(
            before.can_write_user_data(),
            "宿主需以 Medium 及以上完整性运行才能验证这件事，实测 {}",
            before.as_str()
        );
        let out = run("Write-Output 'sandbox-ok'", "");
        assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
        assert!(out.stdout.contains("sandbox-ok"), "stdout: {}", out.stdout);
        let after = current_integrity_level();
        assert_eq!(
            after,
            before,
            "沙箱命令改动了宿主完整性级别（{} -> {}），宿主被自己降级",
            before.as_str(),
            after.as_str()
        );
    }

    fn run(script: &str, payload: &str) -> SandboxOutput {
        run_powershell_sandboxed(script, payload, 60).expect("沙箱执行应成功")
    }

    #[test]
    #[ignore = "requires Windows restricted-token process privileges; run test:windows-integration in a supported environment"]
    fn benign_script_runs_with_stdin_payload() {
        let out = run(
            "$p = [Console]::In.ReadToEnd() | ConvertFrom-Json; Write-Output \"hello-$($p.name)\"",
            r#"{"name":"sandbox"}"#,
        );
        assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
        assert!(
            out.stdout.contains("hello-sandbox"),
            "stdout: {}",
            out.stdout
        );
        assert!(!out.timed_out);
    }

    #[test]
    #[ignore = "requires Windows restricted-token process privileges; run test:windows-integration in a supported environment"]
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
    #[ignore = "requires Windows restricted-token process privileges; run test:windows-integration in a supported environment"]
    fn user_profile_write_denied() {
        // 低完整性 + Users restricting SID：用户资料目录写不进去
        let filename = format!("vivian-sandbox-denied-{}.txt", uuid::Uuid::new_v4());
        let out = run(
            &format!("try {{ Set-Content -LiteralPath (Join-Path $env:USERPROFILE '{filename}') -Value 'x' -ErrorAction Stop; Write-Output 'WROTE' }} catch {{ Write-Output 'DENIED' }}"),
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
        let _ = std::fs::remove_file(home.join(filename));
    }

    #[test]
    #[ignore = "requires Windows restricted-token process privileges; run test:windows-integration in a supported environment"]
    fn exit_code_passthrough() {
        let out = run("exit 3", "");
        assert_eq!(out.exit_code, 3);
        assert!(!out.timed_out);
    }

    struct TestWorkspace {
        root: PathBuf,
        dir: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("vivian-sandbox-test-{}", uuid::Uuid::new_v4()));
            let dir = root.join("工作区 with spaces");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(root.join("outside.txt"), "unchanged").unwrap();
            Self { root, dir }
        }

        fn request(&self) -> SandboxRequest {
            SandboxRequest {
                session_id: uuid::Uuid::new_v4().simple().to_string(),
                workspace_dirs: vec![self.dir.to_string_lossy().into_owned()],
                ..Default::default()
            }
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    #[ignore = "launches Windows restricted-token processes; run test:windows-integration"]
    fn command_writes_unicode_workspace_and_temp_but_denies_outside_write() {
        let workspace = TestWorkspace::new();
        let req = workspace.request();
        // 模拟宿主在会话 TEMP 下创建文件；Low 标签必须可继承，子进程才可修改。
        let scratch = scratch_dir_for(&req.session_id).unwrap();
        std::fs::write(scratch.join("host-created.txt"), "before").unwrap();
        let out = run_command_sandboxed(
            &req,
            r#"$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[System.Text.Encoding]::UTF8
Set-Content -LiteralPath 'result.txt' -Value '42'
Set-Content -LiteralPath (Join-Path $env:TEMP 'host-created.txt') -Value 'scratch-ok'
try { Set-Content -LiteralPath '..\outside.txt' -Value 'bad'; Write-Output 'WROTE-OUTSIDE' } catch { Write-Output 'DENIED-OUTSIDE' }
Write-Output (Get-Location).Path
Write-Output (Get-Content -LiteralPath (Join-Path $env:TEMP 'host-created.txt'))
& $env:ComSpec /d /c 'exit 7'"#,
            15,
            Some(&workspace.dir),
        ).expect("命令应能启动并执行");
        assert_eq!(out.exit_code, 7, "{out:?}");
        assert!(!out.timed_out);
        assert!(out.stdout.contains("工作区 with spaces"), "{out:?}");
        assert!(out.stdout.contains("scratch-ok"), "{out:?}");
        assert!(out.stdout.contains("DENIED-OUTSIDE"), "{out:?}");
        assert!(!out.stdout.contains("WROTE-OUTSIDE"), "{out:?}");
        assert_eq!(
            std::fs::read_to_string(workspace.dir.join("result.txt"))
                .unwrap()
                .trim(),
            "42"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.root.join("outside.txt")).unwrap(),
            "unchanged"
        );
        let _ = std::fs::remove_dir_all(scratch);
    }

    #[test]
    #[ignore = "launches Windows restricted-token processes; run test:windows-integration"]
    fn persistent_shell_preserves_state_and_can_spawn_piped_children() {
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};

        let workspace = TestWorkspace::new();
        let req = workspace.request();
        let mut shell =
            spawn_sandboxed_shell(&req, Some(&workspace.dir)).expect("持久沙箱应能启动");
        let read = |mut file: std::fs::File| {
            std::thread::spawn(move || {
                let mut text = String::new();
                file.read_to_string(&mut text).unwrap();
                text
            })
        };
        let stdout = read(shell.stdout.try_clone().unwrap());
        let stderr = read(shell.stderr.try_clone().unwrap());
        shell
            .stdin
            .write_all(
                br#"$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[System.Text.Encoding]::UTF8
$n=40
$n+=2; Set-Content -LiteralPath 'persistent.txt' -Value $n
$p=New-Object System.Diagnostics.Process
$p.StartInfo.FileName=$env:ComSpec
$p.StartInfo.Arguments='/d /c echo piped-child-ok'
$p.StartInfo.UseShellExecute=$false
$p.StartInfo.RedirectStandardOutput=$true
$null=$p.Start(); Write-Output $p.StandardOutput.ReadToEnd(); $p.WaitForExit()
Write-Output "persistent=$n"
exit 0
"#,
            )
            .unwrap();
        shell.stdin.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let code = loop {
            if let Some(code) = shell.try_wait() {
                break Some(code);
            }
            if Instant::now() >= deadline {
                shell.kill();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let stdout = stdout.join().unwrap();
        let stderr = stderr.join().unwrap();
        assert_eq!(code, Some(0), "stdout={stdout} stderr={stderr}");
        assert!(
            stdout.contains("persistent=42"),
            "stdout={stdout} stderr={stderr}"
        );
        assert!(
            stdout.contains("piped-child-ok"),
            "stdout={stdout} stderr={stderr}"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.dir.join("persistent.txt"))
                .unwrap()
                .trim(),
            "42"
        );
        let _ = std::fs::remove_dir_all(scratch_dir_for(&req.session_id).unwrap());
    }

    #[test]
    #[ignore = "launches Windows restricted-token processes; run test:windows-integration"]
    fn sandbox_timeout_terminates_the_process() {
        let out = run_powershell_sandboxed("Start-Sleep -Seconds 30", "", 1).unwrap();
        assert!(out.timed_out, "{out:?}");
        assert_ne!(out.exit_code, 0, "{out:?}");
    }

    #[test]
    #[ignore = "launches Windows restricted-token processes; run test:windows-integration"]
    fn bounded_output_drains_while_large_stdin_is_written() {
        let req = SandboxRequest {
            output_cap: 64,
            ..Default::default()
        };
        let input = "x".repeat(1024 * 1024);
        let out = run_powershell_sandboxed_with(&req,
            "[Console]::Write(('x' * 262144)); $p=[Console]::In.ReadToEnd(); [Console]::Error.Write($p.Length); exit 0",
            &input, 15, None).unwrap();
        assert_eq!(out.exit_code, 0, "{out:?}");
        assert_eq!(out.stdout.len(), 64);
        assert!(out.stderr.contains("1048576"), "{out:?}");
        assert!(!out.timed_out);
    }

    #[test]
    #[ignore = "launches Windows restricted-token processes; run test:windows-integration"]
    fn unread_stdin_does_not_prevent_timeout() {
        let started = std::time::Instant::now();
        let out = run_powershell_sandboxed("Start-Sleep -Seconds 30", &"x".repeat(1024 * 1024), 1)
            .unwrap();
        assert!(out.timed_out);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    #[ignore = "launches Windows restricted-token processes; run test:windows-integration"]
    fn completed_command_reaps_its_descendants() {
        let out = run("$p=New-Object System.Diagnostics.Process; $p.StartInfo.FileName=\"$PSHOME\\powershell.exe\"; $p.StartInfo.Arguments='-NoProfile -NonInteractive -Command Start-Sleep -Seconds 30'; $p.StartInfo.UseShellExecute=$false; $p.StartInfo.CreateNoWindow=$true; $null=$p.Start(); Write-Output $p.Id; exit 0", "");
        assert_eq!(out.exit_code, 0, "{out:?}");
        let pid: u32 = out.stdout.trim().parse().unwrap();
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let alive = unsafe {
                match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    Ok(handle) => {
                        let handle = HandleWrap::new(handle);
                        let mut code = 0;
                        GetExitCodeProcess(handle.get(), &mut code).is_ok() && code == 259
                    }
                    Err(_) => false,
                }
            };
            if !alive {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "descendant {pid} survived completion"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn missing_proxy_and_no_proxy_bypass_fail_closed() {
        let req = SandboxRequest {
            network_mode: SandboxNetworkMode::Proxy,
            ..Default::default()
        };
        let mut vars = vec![("NO_PROXY".into(), "*".into())];
        apply_network_env(&mut vars, &req);
        assert!(vars
            .iter()
            .any(|(key, value)| key == "HTTP_PROXY" && value == "http://127.0.0.1:0"));
        assert!(!vars
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("NO_PROXY") && !value.is_empty()));
    }
}
