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

/// 原子写文件：同目录临时文件 + 全量落盘（sync）+ rename 替换。
///
/// 写入中断（崩溃 / 断电）最多留下一个 `.tmp` 残骸，目标文件要么是旧内容
/// 要么是完整新内容，不会出现半截 JSON。配合 [`load_json_or_backup`]：
/// 极端情况下仍损坏的文件会在下次加载时被备份并按空态处理。
pub fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let tmp = path.with_file_name(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content)?;
        f.sync_all()?;
    }
    let result = replace_file(&tmp, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    atomic_write(path, content.as_bytes())
}

/// 原子替换目录：把已经完全写好的临时目录 `staging` 整体替换为 `target`。
///
/// 与单文件 [`atomic_write`] 同构，只是交换单位是整棵目录树：
/// 1. 交换前 [`fsync_dir_tree`] 把 `staging` 内所有文件落盘，避免"半截目录"；
/// 2. 若 `target` 已存在，先改名备份为 `.{target名}.{uuid}.bak`
///    （UUID 临时名，避免固定名并发互盖）；
/// 3. `staging` → `target` 走与 [`atomic_write`] 相同的平台原子 rename
///    （Unix `rename` + 父目录 sync；Windows `MoveFileExW` + `MOVEFILE_WRITE_THROUGH`）；
/// 4. 成功则删除备份，失败则把备份回滚成 `target` 并返回错误。
///
/// `staging` 在成功后不再存在（它已成为 `target`）。调用方负责在调用前把
/// `staging` 完全写好；本函数不读取 `staging` 的内容语义。
pub fn write_atomic_dir(target: &Path, staging: &Path) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 交换前把整棵 staging 树完全落盘，保证不是"半截目录"。
    fsync_dir_tree(staging)?;

    let target_name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "target".to_string());
    let backup = target.with_file_name(format!(".{target_name}.{}.bak", uuid::Uuid::new_v4()));

    let had_target = target.exists();
    if had_target {
        // 清掉可能残留的旧备份，再备份当前 target
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(target, &backup)?;
    }

    // 原子交换；失败则回滚备份（若有）
    if let Err(e) = replace_file(staging, target) {
        if had_target {
            let _ = replace_file(&backup, target);
        }
        return Err(e);
    }

    if had_target {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(())
}

/// 递归把目录树完全落盘：先同步每个文件的数据，再自底向上同步每个目录项。
fn fsync_dir_tree(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            fsync_dir_tree(&path)?;
            fsync_dir(&path)?;
        } else {
            // 以读写方式打开：Windows 下 `sync_all` → FlushFileBuffers 在只读句柄上会
            // 返回 ERROR_ACCESS_DENIED(5)，需写访问权才能刷盘。
            let f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)?;
            f.sync_all()?;
        }
    }
    fsync_dir(dir)?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(tmp: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::rename(tmp, path)?;
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file(tmp: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let from: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| std::io::Error::last_os_error())
    }
}

/// 同步一个目录项到磁盘（Unix 下有效；Windows 下目录无 fsync 等价物，
/// 关键交换的持久性由 `MoveFileExW` 的 `MOVEFILE_WRITE_THROUGH` 保证）。
#[cfg(not(windows))]
fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(windows)]
fn fsync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
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

    #[test]
    fn write_atomic_dir_creates_target() {
        let dir = std::env::temp_dir().join(format!("vivian-fs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let staging = dir.join(".staging.tmp");
        std::fs::create_dir_all(staging.join("sub")).unwrap();
        std::fs::write(staging.join("sub").join("a.txt"), b"hello").unwrap();

        let target = dir.join("final");
        write_atomic_dir(&target, &staging).unwrap();

        assert!(target.join("sub").join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(target.join("sub").join("a.txt")).unwrap(),
            "hello"
        );
        // 成功后 staging 被消费（已成为 target）
        assert!(!staging.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_dir_replaces_existing_and_cleans_backup() {
        let dir = std::env::temp_dir().join(format!("vivian-fs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // 预置一个已存在的 target
        std::fs::create_dir_all(dir.join("final")).unwrap();
        std::fs::write(dir.join("final").join("old.txt"), b"old").unwrap();

        let staging = dir.join(".staging.tmp");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("new.txt"), b"new").unwrap();

        write_atomic_dir(&dir.join("final"), &staging).unwrap();

        assert!(dir.join("final").join("new.txt").exists());
        assert!(!dir.join("final").join("old.txt").exists());
        // 不应残留 .bak 备份
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !leftovers.iter().any(|n| n.contains(".bak")),
            "leftover backup: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
