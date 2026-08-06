use std::path::PathBuf;

/// 获取用户数据目录
///
/// 平台默认路径（Windows: %APPDATA%\Vivian，macOS: ~/Library/Application Support/Vivian，Linux: ~/.local/share/Vivian），
/// 失败时回退到当前目录下的 `vivian_data`。
pub fn get_user_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("vivian");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Vivian");
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local").join("share").join("Vivian");
        }
    }
    PathBuf::from("vivian_data")
}

/// 获取角色数据目录（按角色 ID 隔离）
///
/// 路径：`<user_data_dir>/characters/<char_id>/`
/// 每个角色拥有独立的 memory / persona / psychology / history 子目录。
pub fn get_character_data_dir(char_id: &str) -> PathBuf {
    let dir = get_user_data_dir().join("characters").join(char_id);
    let _ = ensure_dir(&dir);
    dir
}

/// 获取共享数据目录（跨角色共享的数据，如世界知识、统一事件账本）
///
/// 路径：`<user_data_dir>/shared/`
/// 注意：user_facts 已按角色隔离存储，不在此目录下。
pub fn get_shared_data_dir() -> PathBuf {
    let dir = get_user_data_dir().join("shared");
    let _ = ensure_dir(&dir);
    dir
}

/// 获取共同记忆目录（两个角色共享的记忆，如世界设定、共同经历）
///
/// 路径：`<user_data_dir>/common/memory/`
pub fn get_common_memory_dir() -> PathBuf {
    let dir = get_user_data_dir().join("common").join("memory");
    let _ = ensure_dir(&dir);
    dir
}

/// 获取共同日记目录（两个角色共享的日记，如一起做的事）
///
/// 路径：`<user_data_dir>/common/diary/`
pub fn get_common_diary_dir() -> PathBuf {
    let dir = get_user_data_dir().join("common").join("diary");
    let _ = ensure_dir(&dir);
    dir
}

pub fn get_resource_dir() -> PathBuf {
    // release 模式优先：从 exe 目录查找打包资源，避免 cwd 指向错误位置
    if let Some(exe) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        // 直接位于 exe 同级：bundle.resources 映射到 <模型名>/
        if has_live2d_model(&exe) {
            return exe;
        }
        // 新配置：resources/<模型名>/
        let resources = exe.join("resources");
        if resources.exists() && has_live2d_model(&resources) {
            return resources;
        }
        // 旧配置兼容：_up_/public/<模型名>/
        let old = exe.join("_up_").join("public");
        if old.exists() && has_live2d_model(&old) {
            return old;
        }
    }

    // dev 模式兜底：cwd 通常是 src-tauri，向上查找 public/
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = cwd.clone();
    for _ in 0..5 {
        let public = dir.join("public");
        if public.exists() && has_live2d_model(&public) {
            return public;
        }
        if !dir.pop() {
            break;
        }
    }

    // release 模式下文件系统无资源（已打包为 vivian.bundle.enc）
    // 返回虚拟路径，ResourceLoader 会检测 model_dir 不存在时走 scan_embedded 路径
    PathBuf::from("embedded")
}

/// 获取 vivian.bundle.enc 路径（release 模式）
///
/// 查找顺序：
/// 1. exe 同级（bundle.resources 直接映射）
/// 2. exe/resources/ 子目录
/// 3. exe/_up_/（旧配置兼容）
pub fn get_bundle_path() -> PathBuf {
    if let Some(exe) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        // 直接位于 exe 同级
        let bundle = exe.join("vivian.bundle.enc");
        if bundle.exists() {
            return bundle;
        }
        // resources/ 子目录
        let bundle = exe.join("resources").join("vivian.bundle.enc");
        if bundle.exists() {
            return bundle;
        }
        // _up_/ 兼容
        let bundle = exe.join("_up_").join("vivian.bundle.enc");
        if bundle.exists() {
            return bundle;
        }
    }
    PathBuf::from("vivian.bundle.enc")
}

/// 检查目录下是否有任意 Live2D 模型子目录（包含 .model3.json 或 model_manifest.json）
fn has_live2d_model(dir: &std::path::Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(sub_entries) = std::fs::read_dir(&path) {
                for sub in sub_entries.flatten() {
                    let name = sub.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.ends_with(".model3.json") || name_str == "model_manifest.json" {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub fn ensure_dir(path: &std::path::Path) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}
