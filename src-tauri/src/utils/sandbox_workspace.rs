//! 工作区沙箱写授权 —— 让低完整性沙箱进程在工作区内可读可写
//!
//! 为什么需要这一层：受限令牌沙箱跑在低完整性级别（Low IL）。强制完整性控制
//! 下，Low IL 进程写不动 Medium IL 对象；而用户的工作区默认没有完整性标签，
//! 按 Medium 处理。结果是沙箱里 `cd` 到工作区能读不能写——持久终端、构建命令、
//! `git commit` 全部失效。
//!
//! 只靠降标签还不够：受限令牌的访问检查要走**两遍**，一遍用令牌里全部 SID，
//! 一遍只用 restricting SID（Everyone + BUILTIN\Users）。两遍都通过才放行。
//! 工作区的默认 DACL 里 Users 通常只有读取，第二遍会把写挡下来。所以授权必须
//! 同时处理两件事：
//! - **SACL**：打 Low IL 强制标签（`ML;OICI;NW;;;LW`）
//! - **DACL**：给 Everyone / Users 授予完全控制
//!
//! 本模块只服务**用户显式声明的工作区**（主工作区与附加工作区），不对用户磁盘
//! 上的其他位置动手。授权是一次性的、持久落在文件系统上的改动——同一目录只做
//! 一遍，之后按规范化路径命中缓存。
//!
//! 失败语义：单个条目失败不中断，记首条错误返回；调用方据此降级（工作区只读）
//! 而不是放弃整个命令执行。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 单次授权处理的条目上限。
///
/// 巨型工作区（node_modules 之类）逐个改安全描述符会拖住命令执行，超过上限
/// 就停下：目录根已经带继承标签，之后新建的对象会自动拿到正确标签，只有超限
/// 之前的旧文件仍保持不可写。
const MAX_ENTRIES: usize = 20_000;

/// 递归深度上限（防 junction / 意外深层结构）
const MAX_DEPTH: u32 = 12;

/// 目录：完全控制给 SYSTEM / Administrators / Users / Everyone + Low IL 标签（可继承）
const DIR_SDDL: &str = "D:AI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;BU)(A;OICI;FA;;;WD)S:AI(ML;OICI;NW;;;LW)";

/// 文件：同上但不带继承标志（文件没有子对象）
const FILE_SDDL: &str = "D:AI(A;FA;;;SY)(A;FA;;;BA)(A;FA;;;BU)(A;FA;;;WD)S:AI(ML;;NW;;;LW)";

/// 授权结果
#[derive(Debug, Clone, Default)]
pub struct GrantOutcome {
    /// 成功设置安全描述符的条目数
    pub tagged: usize,
    /// 命中上限后未处理的条目数（>0 表示授权不完整）
    pub skipped: usize,
    /// 首条失败原因（其余只记数，避免刷屏）
    pub error: Option<String>,
    /// 是否因上限提前停止
    pub truncated: bool,
}

impl GrantOutcome {
    /// 是否足以认为"工作区可写"
    pub fn usable(&self) -> bool {
        self.tagged > 0 && self.error.is_none()
    }
}

/// 已授权目录缓存（规范化后的小写路径）
fn granted() -> &'static Mutex<std::collections::HashSet<String>> {
    static G: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn cache_key(dir: &Path) -> String {
    let norm = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    norm.to_string_lossy().to_lowercase()
}

/// 该目录本次进程内是否已授权过
pub fn is_granted(dir: &Path) -> bool {
    granted()
        .lock()
        .map(|g| g.contains(&cache_key(dir)))
        .unwrap_or(false)
}

/// 给工作区目录授予沙箱写权限（幂等，同目录只做一次）
///
/// 未启用 / 非 Windows / 目录不存在都返回 `tagged == 0` 的结果，由调用方决定
/// 降级策略。
pub fn grant_workspace_write(dir: &Path) -> GrantOutcome {
    let mut out = GrantOutcome::default();
    if dir.as_os_str().is_empty() {
        out.error = Some("工作区路径为空".to_string());
        return out;
    }
    let meta = match std::fs::metadata(dir) {
        Ok(m) => m,
        Err(e) => {
            out.error = Some(format!("读取工作区失败: {e}"));
            return out;
        }
    };
    if !meta.is_dir() {
        out.error = Some(format!("工作区不是目录: {}", dir.display()));
        return out;
    }

    if is_granted(dir) {
        out.tagged = 1; // 已授权，视为可用
        return out;
    }

    let key = cache_key(dir);
    apply_recursive(dir, &mut out);
    if out.tagged > 0 && out.error.is_none() {
        if let Ok(mut g) = granted().lock() {
            g.insert(key);
        }
    }
    out
}

#[cfg(windows)]
fn apply_recursive(root: &Path, out: &mut GrantOutcome) {
    // 根目录先打标签（继承标志保证之后新建的对象自动拿到）
    match set_security(root, DIR_SDDL) {
        Ok(()) => out.tagged += 1,
        Err(e) => {
            out.error = Some(format!("设置工作区安全描述符失败: {e}"));
            return;
        }
    }

    let mut stack: Vec<(PathBuf, u32)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if out.tagged + out.skipped >= MAX_ENTRIES {
            out.truncated = true;
            break;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                if out.error.is_none() {
                    out.error = Some(format!("遍历 {} 失败: {e}", dir.display()));
                }
                continue;
            }
        };
        for entry in entries.flatten() {
            if out.tagged + out.skipped >= MAX_ENTRIES {
                out.truncated = true;
                out.skipped += 1;
                continue;
            }
            let path = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                // 不跟随 junction / 符号链接：授权不应该跑出用户声明的工作区
                if is_reparse_point(&meta) {
                    continue;
                }
                match set_security(&path, DIR_SDDL) {
                    Ok(()) => out.tagged += 1,
                    Err(e) => record_error(out, format!("{}: {e}", path.display())),
                }
                stack.push((path, depth + 1));
            } else if meta.is_file() {
                match set_security(&path, FILE_SDDL) {
                    Ok(()) => out.tagged += 1,
                    Err(e) => record_error(out, format!("{}: {e}", path.display())),
                }
            }
        }
    }
}

#[cfg(not(windows))]
fn apply_recursive(_root: &Path, out: &mut GrantOutcome) {
    out.error = Some("工作区写授权仅支持 Windows".to_string());
}

fn record_error(out: &mut GrantOutcome, msg: String) {
    if out.error.is_none() {
        out.error = Some(msg);
    }
    out.skipped += 1;
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// 用 SDDL 设置对象的安全描述符（DACL + 强制完整性标签）
#[cfg(windows)]
fn set_security(path: &Path, sddl: &str) -> Result<(), String> {
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
        let sddl_w: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl_w.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
        .map_err(|e| format!("构造安全描述符失败: {e}"))?;

        let mut dacl: *mut windows::Win32::Security::ACL = std::ptr::null_mut();
        let mut dacl_present = windows::core::BOOL(0);
        let mut dacl_defaulted = windows::core::BOOL(0);
        let _ = GetSecurityDescriptorDacl(sd, &mut dacl_present, &mut dacl, &mut dacl_defaulted);
        let mut sacl: *mut windows::Win32::Security::ACL = std::ptr::null_mut();
        let mut sacl_present = windows::core::BOOL(0);
        let mut sacl_defaulted = windows::core::BOOL(0);
        let _ = GetSecurityDescriptorSacl(sd, &mut sacl_present, &mut sacl, &mut sacl_defaulted);

        let path_w: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
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
            return Err(format!("WIN32_ERROR({:?})", err.0));
        }
        Ok(())
    }
}
