//! 应用信任列表 - open_application 工具的持久化白名单
//!
//! 用户在确认 toast 中选择"信任此应用"后，应用标识写入此列表，
//! 后续启动同一应用不再询问。列表持久化到用户数据目录的
//! `trusted_apps.json`，应用重启后依然生效。

use std::collections::HashSet;
use std::path::PathBuf;

use once_cell::sync::Lazy;
use parking_lot::RwLock;

use crate::utils::path::get_user_data_dir;

/// 信任列表内存缓存（首次访问时从磁盘加载）
static TRUSTED_APPS: Lazy<RwLock<HashSet<String>>> = Lazy::new(|| {
    let path = trust_file_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let apps: Vec<String> = serde_json::from_str(&content).unwrap_or_default();
            let set: HashSet<String> = apps.into_iter().map(|a| normalize_app(&a)).collect();
            tracing::info!("[Trust] 加载应用信任列表: {} 项 ({})", set.len(), path.display());
            RwLock::new(set)
        }
        Err(_) => RwLock::new(HashSet::new()),
    }
});

/// 信任列表文件路径
fn trust_file_path() -> PathBuf {
    get_user_data_dir().join("trusted_apps.json")
}

/// 应用标识归一化：小写 + trim + 去除 .exe 后缀
///
/// LLM 可能传 "Chrome" / "chrome.exe" / " chrome "，归一化后视为同一应用。
fn normalize_app(app: &str) -> String {
    let trimmed = app.trim().to_ascii_lowercase();
    trimmed
        .strip_suffix(".exe")
        .unwrap_or(&trimmed)
        .to_string()
}

/// 检查应用是否在信任列表中
pub fn is_trusted_app(app: &str) -> bool {
    TRUSTED_APPS.read().contains(&normalize_app(app))
}

/// 将应用加入信任列表并持久化
pub fn add_trusted_app(app: &str) {
    let normalized = normalize_app(app);
    if normalized.is_empty() {
        return;
    }

    let inserted = TRUSTED_APPS.write().insert(normalized.clone());
    if !inserted {
        return;
    }

    let apps: Vec<String> = {
        let mut sorted: Vec<String> = TRUSTED_APPS.read().iter().cloned().collect();
        sorted.sort();
        sorted
    };

    let path = trust_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&apps) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("[Trust] 信任列表写盘失败: {}", e);
            } else {
                tracing::info!("[Trust] 应用「{}」已加入信任列表", normalized);
            }
        }
        Err(e) => tracing::warn!("[Trust] 信任列表序列化失败: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_app() {
        assert_eq!(normalize_app("Chrome"), "chrome");
        assert_eq!(normalize_app("notepad.exe"), "notepad");
        assert_eq!(normalize_app("  Steam  "), "steam");
        assert_eq!(normalize_app("CODE.EXE"), "code");
    }
}
