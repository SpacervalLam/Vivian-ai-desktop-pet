//! 工作区沙箱写授权 —— 让低完整性沙箱进程在工作区内可读可写
//!
//! Low IL 工作者需要工作区的低完整性标签，以及任务策略专属 SID 的写 ACL。
//! SID 随权限版本改变，后来的只读工作者不能复用旧工作者的写权限。
//! 只读子目录追加同一任务 SID 的拒绝写 ACL；不修改宿主用户的访问权限。
//! ACL 缓存包含规范化路径、文件 ID 和任务 SID。ACL 遍历不完整时拒绝启动，
//! 不能把受限任务降级成不受限进程。授权不会自动清除既有标签和旧 SID ACE。

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

/// 目录的 Low IL 标签（可继承）；写 ACL 在设置时合并，保留用户原有权限
const DIR_SDDL: &str = "S:AI(ML;OICI;NW;;;LW)";

/// 文件的 Low IL 标签（文件没有子对象）
const FILE_SDDL: &str = "S:AI(ML;;NW;;;LW)";

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
        self.tagged > 0 && self.error.is_none() && !self.truncated && self.skipped == 0
    }
}

/// 已授权目录缓存（规范化路径 + 卷与文件 ID，防删除重建后误命中）
fn granted() -> &'static Mutex<std::collections::HashSet<String>> {
    static G: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn cache_key(dir: &Path) -> Option<String> {
    let norm = std::fs::canonicalize(dir).ok()?;
    #[cfg(windows)]
    let identity = crate::utils::sandbox::active_write_sid().unwrap_or_default();
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(&norm)
            .ok()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe {
            GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info).ok()?;
        }
        return Some(format!(
            "{}|{}:{}:{}|{}",
            norm.to_string_lossy().to_lowercase(),
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow, identity
        ));
    }
    #[cfg(not(windows))]
    Some(norm.to_string_lossy().into_owned())
}

/// 该目录本次进程内是否已授权过
pub fn is_granted(dir: &Path) -> bool {
    let Some(key) = cache_key(dir) else {
        return false;
    };
    granted().lock().map(|g| g.contains(&format!("{key}|deny=false"))).unwrap_or(false)
}

/// 给工作区目录授予沙箱写权限（幂等，同目录只做一次）
///
/// 未启用 / 非 Windows / 目录不存在都返回 `tagged == 0` 的结果，由调用方决定
/// 降级策略。
pub fn grant_workspace_write(dir: &Path) -> GrantOutcome { grant_workspace_access(dir, false) }
pub fn deny_workspace_write(dir: &Path) -> GrantOutcome {
    #[cfg(windows)]
    if crate::utils::sandbox::active_write_sid().is_none() {
        return GrantOutcome { error: Some("只读覆盖缺少任务专属身份，拒绝修改宿主 ACL".into()), ..Default::default() };
    }
    grant_workspace_access(dir, true)
}
fn grant_workspace_access(dir: &Path, deny: bool) -> GrantOutcome {
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

    let key = cache_key(dir).map(|key|format!("{key}|deny={deny}"));
    if key
        .as_ref()
        .is_some_and(|key| granted().lock().map(|g| g.contains(key)).unwrap_or(false))
    {
        out.tagged = 1; // 已授权，视为可用
        return out;
    }

    apply_recursive(dir, &mut out, deny);
    if out.usable() && key.is_some() {
        if let Ok(mut g) = granted().lock() {
            g.insert(key.unwrap());
        }
    }
    out
}

#[cfg(windows)]
fn apply_recursive(root: &Path, out: &mut GrantOutcome, deny: bool) {
    // 根目录先打标签（继承标志保证之后新建的对象自动拿到）
    match set_security(root, DIR_SDDL, deny) {
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
            out.truncated = true;
            out.skipped += 1;
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
        for entry in entries {
            if out.tagged + out.skipped >= MAX_ENTRIES {
                out.truncated = true;
                out.skipped += 1;
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    record_error(out, format!("遍历 {} 条目失败: {e}", dir.display()));
                    continue;
                }
            };
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    record_error(out, format!("读取 {} 属性失败: {e}", path.display()));
                    continue;
                }
            };
            if is_reparse_point(&meta) {
                continue;
            }
            if meta.is_dir() {
                match set_security(&path, DIR_SDDL, deny) {
                    Ok(()) => out.tagged += 1,
                    Err(e) => record_error(out, format!("{}: {e}", path.display())),
                }
                stack.push((path, depth + 1));
            } else if meta.is_file() {
                match set_security(&path, FILE_SDDL, deny) {
                    Ok(()) => out.tagged += 1,
                    Err(e) => record_error(out, format!("{}: {e}", path.display())),
                }
            }
        }
    }
}

#[cfg(not(windows))]
fn apply_recursive(_root: &Path, out: &mut GrantOutcome, _deny: bool) {
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
fn set_security(path: &Path, sddl: &str, deny: bool) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
        GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
        GRANT_ACCESS, DENY_ACCESS, SDDL_REVISION_1, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_W,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorSacl, ACE_FLAGS, DACL_SECURITY_INFORMATION,
        LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    };

    let sid_text = match crate::utils::sandbox::active_write_sid() {
        Some(sid) => sid,
        None => crate::utils::sandbox::current_logon_sid_sddl()?,
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

        let path_w: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut original = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        let mut old_acl = std::ptr::null_mut();
        let read = GetNamedSecurityInfoW(
            PCWSTR(path_w.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_acl),
            None,
            &mut original,
        )
        .ok();
        if let Err(error) = read {
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            return Err(format!("读取原有工作区 ACL 失败: {error}"));
        }
        // Merge a session logon Modify ACE. Do not replace private ACLs or grant
        // Everyone/Users full control of the user's files.
        let sid_w: Vec<u16> = sid_text.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sid = PSID::default();
        let converted = ConvertStringSidToSidW(PCWSTR(sid_w.as_ptr()), &mut sid);
        if let Err(error) = converted {
            let _ = LocalFree(Some(HLOCAL(original.0)));
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            return Err(error.to_string());
        }
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: 0x0013_01bf, // file/directory Modify, no WRITE_DAC/WRITE_OWNER
            grfAccessMode: if deny { DENY_ACCESS } else { GRANT_ACCESS },
            grfInheritance: ACE_FLAGS(if sddl == DIR_SDDL { 3 } else { 0 }),
            Trustee: TRUSTEE_W {
                TrusteeForm: TRUSTEE_IS_SID,
                ptstrName: windows::core::PWSTR(sid.0.cast()),
                ..Default::default()
            },
        };
        let mut dacl = std::ptr::null_mut();
        let merged = SetEntriesInAclW(Some(&[entry]), Some(old_acl), &mut dacl).ok();
        let _ = LocalFree(Some(HLOCAL(sid.0)));
        let _ = LocalFree(Some(HLOCAL(original.0)));
        if let Err(error) = merged {
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            return Err(error.to_string());
        }
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
        let _ = LocalFree(Some(HLOCAL(dacl.cast())));
        if err != windows::Win32::Foundation::WIN32_ERROR(0) {
            return Err(format!("WIN32_ERROR({:?})", err.0));
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn depth_limited_grants_are_not_cached_as_complete() {
        let fixture =
            std::env::temp_dir().join(format!("vivian-acl-depth-{}", uuid::Uuid::new_v4()));
        let mut leaf = fixture.clone();
        for _ in 0..=MAX_DEPTH {
            leaf.push("child");
        }
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(leaf.join("existing.txt"), "deep").unwrap();
        let outcome = grant_workspace_write(&fixture);
        assert!(outcome.truncated, "{outcome:?}");
        assert!(!outcome.usable(), "{outcome:?}");
        assert!(!is_granted(&fixture));
        std::fs::remove_dir_all(fixture).unwrap();
    }

    fn descriptor(path: &Path) -> String {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::Foundation::{LocalFree, HLOCAL};
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
            SDDL_REVISION_1, SE_FILE_OBJECT,
        };
        use windows::Win32::Security::{
            DACL_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        };
        unsafe {
            let name: Vec<u16> = path
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let info = DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION;
            let mut sd = PSECURITY_DESCRIPTOR::default();
            GetNamedSecurityInfoW(
                PCWSTR(name.as_ptr()),
                SE_FILE_OBJECT,
                info,
                None,
                None,
                None,
                None,
                &mut sd,
            )
            .ok()
            .unwrap();
            let mut text = PWSTR::null();
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                info,
                &mut text,
                None,
            )
            .unwrap();
            let result = text.to_string().unwrap();
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            result
        }
    }

    #[test]
    fn grants_preserve_acl_skip_junction_targets_and_do_not_cache_replaced_directories() {
        let fixture =
            std::env::temp_dir().join(format!("vivian-acl-test-{}", uuid::Uuid::new_v4()));
        let root = fixture.join("workspace");
        let outside = fixture.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let external = outside.join("private.txt");
        std::fs::write(&external, "unchanged").unwrap();
        let original = descriptor(&root);
        let external_before = descriptor(&external);
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(root.join("link"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success());
        let outcome = grant_workspace_write(&root);
        assert!(outcome.usable(), "{outcome:?}");
        let after = descriptor(&root);
        for ace in original
            .split('(')
            .skip(1)
            .filter_map(|rest| rest.split_once(')').map(|(ace, _)| ace))
        {
            assert!(
                after.contains(ace),
                "original ACE disappeared: {ace}; after={after}"
            );
        }
        assert_eq!(descriptor(&external), external_before);
        assert!(is_granted(&root));
        std::fs::rename(&root, fixture.join("previous-workspace")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        assert!(!is_granted(&root));
        assert!(grant_workspace_write(&root).usable());
        std::fs::remove_dir_all(fixture).unwrap();
    }
}
