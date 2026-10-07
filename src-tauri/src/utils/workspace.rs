//! Shared workspace selection for office sessions and companion delegation.
use std::path::{Path, PathBuf};

pub fn default_workspace(configured: Option<&str>) -> Result<PathBuf, String> {
    if let Some(path) = configured.map(str::trim).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    #[cfg(test)]
    if let Some(root) = std::env::var_os("VIVIAN_TEST_DATA_DIR") {
        return Ok(PathBuf::from(root).join("Documents").join("Vivian"));
    }
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Documents, KF_FLAG_DEFAULT};
        use windows::Win32::System::Com::CoTaskMemFree;
        // Resolve the real Documents known folder, including OneDrive or user redirection.
        let pointer = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) }
            .map_err(|error| format!("无法获取系统文档目录：{error}"))?;
        let directory = unsafe { pointer.to_string() };
        unsafe { CoTaskMemFree(Some(pointer.0.cast())); }
        return directory.map(|path| PathBuf::from(path).join("Vivian"))
            .map_err(|error| format!("无法读取系统文档目录：{error}"));
    }
    #[cfg(not(windows))]
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Documents").join("Vivian"))
        .ok_or_else(|| "无法获取用户文档目录".into())
}

pub fn resolve_workspace(explicit: &str, configured: Option<&str>) -> Result<String, String> {
    let explicit = explicit.trim();
    let is_default = explicit.is_empty();
    let path = if is_default { default_workspace(configured)? } else { PathBuf::from(explicit) };
    prepare_workspace(&path, is_default)
}

fn prepare_workspace(path: &Path, create: bool) -> Result<String, String> {
    if !path.is_absolute() { return Err(format!("工作目录必须是绝对路径：{}", path.display())); }
    if create {
        std::fs::create_dir_all(path).map_err(|error| format!("无法创建默认工作区 {}：{error}", path.display()))?;
    }
    if !path.is_dir() { return Err(format!("工作目录不存在：{}", path.display())); }
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_default_is_created_and_explicit_directory_wins() {
        let root = std::env::temp_dir().join(format!("vivian-workspace-{}", uuid::Uuid::new_v4()));
        let custom = root.join("custom");
        let actual = resolve_workspace("  ", Some(custom.to_str().unwrap())).unwrap();
        assert_eq!(PathBuf::from(actual), custom);
        assert!(custom.is_dir());
        let explicit = root.join("explicit");
        std::fs::create_dir_all(&explicit).unwrap();
        assert_eq!(PathBuf::from(resolve_workspace(explicit.to_str().unwrap(), Some(custom.to_str().unwrap())).unwrap()), explicit);
        assert!(resolve_workspace(root.join("missing").to_str().unwrap(), None).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn default_cannot_overwrite_a_file() {
        let file = std::env::temp_dir().join(format!("vivian-workspace-{}", uuid::Uuid::new_v4()));
        std::fs::write(&file, "keep").unwrap();
        assert!(resolve_workspace("", Some(file.to_str().unwrap())).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep");
        std::fs::remove_file(file).unwrap();
    }
    #[test]
    fn blank_setting_uses_documents_vivian() {
        let default = default_workspace(None).unwrap();
        assert_eq!(default_workspace(Some(" ")).unwrap(), default);
        assert_eq!(default.file_name().unwrap(), "Vivian");
        assert!(default.is_absolute());
    }
}
