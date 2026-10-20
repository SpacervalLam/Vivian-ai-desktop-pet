//! File sizes within Vivian's data directory. Never follows links.
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct StorageCategory {
    pub id: &'static str,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct StorageUsage {
    pub path: String,
    pub total_bytes: u64,
    pub categories: Vec<StorageCategory>,
    pub skipped_entries: u64,
}

fn category(relative: &Path) -> &'static str {
    let parts: Vec<_> = relative.iter().map(|p| p.to_string_lossy().to_ascii_lowercase()).collect();
    let first = parts.first().map(String::as_str).unwrap_or("");
    if first == "cache" || super::policy::browser_cache(relative) { return "cache"; }
    if matches!(first, "coding_sessions" | "coding_sessions.json" | "coding_workflow") { return "work"; }
    if parts.iter().any(|p| p == "history") { return "chats"; }
    if matches!(first, "images" | "screenshots" | "sound" | "audio")
        || parts.iter().any(|p| matches!(p.as_str(), "attachments" | "media")) { return "media"; }
    if matches!(first, "characters" | "companion-v2" | "memory" | "shared" | "common" | "notebook" | "diary") {
        return "memory";
    }
    "other"
}

pub fn scan(root: &Path) -> Result<StorageUsage, String> {
    let mut usage = StorageUsage {
        path: root.to_string_lossy().into_owned(), total_bytes: 0, skipped_entries: 0,
        categories: ["chats", "work", "memory", "media", "cache", "other"].into_iter()
            .map(|id| StorageCategory { id, bytes: 0 }).collect(),
    };
    match std::fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(usage),
        Err(e) => return Err(e.to_string()),
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Err("无效的数据目录".into()),
        _ => {}
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => { usage.skipped_entries += 1; continue; }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => { usage.skipped_entries += 1; continue; }
            };
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => { usage.skipped_entries += 1; continue; }
            };
            if meta.file_type().is_symlink() { usage.skipped_entries += 1; continue; }
            if meta.is_dir() { pending.push(path); }
            else if meta.is_file() {
                let id = category(path.strip_prefix(root).map_err(|e| e.to_string())?);
                usage.total_bytes = usage.total_bytes.saturating_add(meta.len());
                if let Some(bucket) = usage.categories.iter_mut().find(|bucket| bucket.id == id) {
                    bucket.bytes = bucket.bytes.saturating_add(meta.len());
                }
            }
        }
    }
    Ok(usage)
}

/// Only the app's disposable cache directory is eligible. Browser data stays open.
pub fn clear_cache(root: &Path) -> Result<(), String> {
    // Reject a linked root or ancestor before touching any descendant.
    for ancestor in root.ancestors() {
        if std::fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?.file_type().is_symlink() {
            return Err("不能清理符号链接中的缓存目录".into());
        }
    }
    let cache = root.join("cache");
    fn remove(path: &Path) -> std::io::Result<()> {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() { return Ok(()); }
        if meta.is_file() { return std::fs::remove_file(path); }
        if meta.is_dir() {
            for entry in std::fs::read_dir(path)? { remove(&entry?.path())?; }
            // Keep directories: caches can be concurrently used by the running app.
        }
        Ok(())
    }
    remove(&cache).map_err(|e| format!("部分缓存未能清理，请稍后重试：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_each_file_once_and_preserves_unknown_data() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["characters/nana/companion-v2/history/messages.db", "coding_sessions/task.json",
            "characters/nana/companion-v2/memory/events.db", "images/example.png", "cache/thumb.bin",
            "browser/engine/EBWebView/Default/Cache/cache.bin", "unknown/user.data"] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, [0; 10]).unwrap();
        }
        let usage = scan(dir.path()).unwrap();
        assert_eq!(usage.total_bytes, 70);
        assert_eq!(usage.categories.iter().map(|c| c.bytes).sum::<u64>(), 70);
        assert_eq!(usage.categories.iter().find(|c| c.id == "cache").unwrap().bytes, 20);
        clear_cache(dir.path()).unwrap();
        assert_eq!(scan(dir.path()).unwrap().total_bytes, 60);
        assert!(dir.path().join("unknown/user.data").exists());
        assert!(dir.path().join("browser/engine/EBWebView/Default/Cache/cache.bin").exists());
    }

    #[test]
    fn missing_directory_reports_zero() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(scan(&dir.path().join("missing")).unwrap().total_bytes, 0);
    }

    #[cfg(unix)]
    #[test]
    fn links_never_count_or_remove_external_files() {
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("private"), "keep").unwrap();
        std::os::unix::fs::symlink(external.path(), dir.path().join("cache")).unwrap();
        assert_eq!(scan(dir.path()).unwrap().total_bytes, 0);
        clear_cache(dir.path()).unwrap();
        assert!(external.path().join("private").exists());
    }
}
