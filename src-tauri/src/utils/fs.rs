//! 状态文件 JSON 加载 —— 损坏时保留现场 + 大声报错 + 空态继续
//!
//! 统一处理磁盘 JSON 状态文件的损坏场景：
//! - 解析失败 → 原文件改名保留现场（`<name>.corrupt-<ts>`），error 级日志
//! - 返回 None 由调用方走默认值，不阻断启动
//!
//! 与原子写（tmp + rename）配合：崩溃留下的半截文件会在下次加载时被发现并备份。

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

/// 备份损坏的状态文件：改名保留现场，不删除。
///
/// 返回备份路径；文件不存在或改名失败时返回 None（调用方继续走空态）。
pub fn backup_corrupted_file(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let backup_path = path.with_file_name(format!("{file_name}.corrupt-{timestamp}"));
    match std::fs::rename(path, &backup_path) {
        Ok(()) => {
            tracing::warn!("[fs] 损坏文件已备份到: {}", backup_path.display());
            Some(backup_path)
        }
        Err(e) => {
            tracing::error!("[fs] 备份损坏文件失败 {}: {}", path.display(), e);
            None
        }
    }
}

/// 读取并解析 JSON 状态文件。
///
/// - 文件缺失 / 读取失败 / 内容为空 → None（调用方用默认值）
/// - 解析失败 → error 级报错 + 备份现场 + None
pub fn load_json_or_backup<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::error!("[fs] 读取状态文件失败 {}: {}", path.display(), e);
            return None;
        }
    };
    if text.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<T>(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::error!(
                "[fs] 状态文件 JSON 解析失败，已备份现场并按空态处理 {}: {}",
                path.display(),
                e
            );
            backup_corrupted_file(path);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
    struct Sample {
        count: u32,
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = std::env::temp_dir().join(format!("vivian-fs-{}", uuid::Uuid::new_v4()));
        assert!(load_json_or_backup::<Sample>(&dir.join("a.json")).is_none());
    }

    #[test]
    fn load_valid_file() {
        let dir = std::env::temp_dir().join(format!("vivian-fs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.json");
        std::fs::write(&path, r#"{"count": 3}"#).unwrap();
        assert_eq!(load_json_or_backup::<Sample>(&path), Some(Sample { count: 3 }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_file_backed_up() {
        let dir = std::env::temp_dir().join(format!("vivian-fs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load_json_or_backup::<Sample>(&path), None);
        // 原文件已被移走，现场保留为 .corrupt-<ts>
        assert!(!path.exists());
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(leftovers.iter().any(|n| n.starts_with("c.json.corrupt-")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
