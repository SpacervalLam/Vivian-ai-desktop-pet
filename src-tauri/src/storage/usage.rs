//! File sizes within Vivian's data directory. Never follows links.
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct StorageCategory {
    pub id: &'static str,
    pub bytes: u64,
    pub subcategories: Vec<StorageCategory>,
}

#[derive(Debug, Serialize)]
pub struct StorageUsage {
    pub path: String,
    pub total_bytes: u64,
    pub categories: Vec<StorageCategory>,
    pub skipped_entries: u64,
}

fn bucket(id: &'static str) -> StorageCategory {
    let children: &[&str] = match id {
        "appCache" => &["fontCache", "embeddingCache", "speechCache", "otherAppCache"],
        "browserCache" => &["webCache", "graphicsCache", "componentCache"],
        _ => &[],
    };
    StorageCategory { id, bytes: 0, subcategories: children.iter().map(|id| bucket(id)).collect() }
}

// Match known directory roots before examining character-owned data. In particular,
// browser History and extension directories must never become chats or app plugins.
fn category(relative: &Path) -> (&'static str, Option<&'static str>) {
    let parts: Vec<_> = relative.iter().map(|p| p.to_string_lossy().to_ascii_lowercase()).collect();
    let first = parts.first().map(String::as_str).unwrap_or("");
    if first == "browser" {
        if super::policy::browser_cache(relative) {
            let component = if parts.get(3).is_some_and(|p| p == "default" || p.starts_with("wv2profile_") || p.starts_with("profile ")) {
                parts.get(4)
            } else { parts.get(3) }.map(String::as_str).unwrap_or("");
            return match component {
                "crashpad" | "browsermetrics" => ("logs", None),
                "gpucache" | "shadercache" | "grshadercache" | "dawncache" => ("browserCache", Some("graphicsCache")),
                "component_crx_cache" => ("browserCache", Some("componentCache")),
                _ => ("browserCache", Some("webCache")),
            };
        }
        return ("browserData", None);
    }
    if first == "cache" {
        let sub = match parts.get(1).map(String::as_str) {
            Some("corpus_embeddings") => "embeddingCache",
            Some("assets") if parts.get(2).is_some_and(|p| p == "notebook-fonts") => "fontCache",
            _ => "otherAppCache",
        };
        return ("appCache", Some(sub));
    }
    // 笔记共享字体（不随缓存清理），属于可重建的字体资源。
    if first == "fonts" { return ("appCache", Some("fontCache")); }
    if matches!(first, "coding_sessions" | "coding_sessions.json" | "coding_workflow" | "coding_events" | "coding_memory_pending" | "web_evidence") { return ("work", None); }
    if matches!(first, "plugins" | "skills" | "mcp") { return ("extensions", None); }
    if first == "logs" { return ("logs", None); }
    if matches!(first, "python-libs" | "pids" | "spill" | "tmp" | ".runtime.lease") { return ("runtime", None); }
    if first == "sns" { return ("social", None); }
    if first == "todo" { return ("tasks", None); }
    if first == "user" && parts.get(1).is_some_and(|p| p == "quick_notes") { return ("notes", None); }
    if first == "config" || first.starts_with("config.yaml") || matches!(first, "token_usage.json" | "trusted_apps.json" | "trusted_origins.json" | "gpt_sovits_tts_infer.yaml") { return ("settings", None); }

    let owned = if first == "characters" && parts.len() >= 3 { &parts[2..] } else { &parts[..] };
    let mut owned = owned;
    while owned.first().is_some_and(|p| matches!(p.as_str(), "companion-v2" | "common" | "shared")) {
        owned = &owned[1..];
    }
    let section = owned.first().map(String::as_str).unwrap_or("");
    if section == "sound" && owned.get(1).is_some_and(|p| p == "cache") { return ("appCache", Some("speechCache")); }
    match section {
        "history" => ("chats", None),
        "memory" => ("memory", None),
        "diary" => ("diary", None),
        "notebook" => {
            // 知识采集自动归档的笔记属于知识资料，不计入用户可见的笔记与随手记。
            if owned.get(1).is_some_and(|id| id.starts_with(crate::notebook::collected::COLLECTED_PREFIX)) {
                ("memory", None)
            } else {
                ("notes", None)
            }
        }
        "images" | "screenshots" | "audio" | "attachments" | "media" => ("media", None),
        "sound" if owned.get(1).is_some_and(|p| p == "config.json") => ("settings", None),
        "sound" => ("media", None),
        "persona" | "psychology" | "presence" | "psychology.json" | "mind" | "proactive" | "meme_acquisition_state.json" => ("companion", None),
        _ if section.starts_with("consolidation_health_") && first == "companion-v2" => ("memory", None),
        _ => ("other", None),
    }
}

/// 硬链接共享同一份数据块，按路径累加会重复计算（笔记字体曾因此虚高百兆）。
/// 扫描时用文件身份去重：同一 inode / FileId 只计一次。
#[derive(Default)]
struct SeenFiles(HashSet<(u64, u64)>);

impl SeenFiles {
    fn already_counted(&mut self, path: &Path, meta: &std::fs::Metadata) -> bool {
        match file_key(path, meta) {
            Some(key) => !self.0.insert(key),
            None => false,
        }
    }
}

#[cfg(unix)]
fn file_key(_path: &Path, meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn file_key(path: &Path, _meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

    let file = std::fs::File::open(path).ok()?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
    let index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    Some((info.dwVolumeSerialNumber as u64, index))
}

#[cfg(not(any(unix, windows)))]
fn file_key(_path: &Path, _meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

pub fn scan(root: &Path) -> Result<StorageUsage, String> {
    let mut usage = StorageUsage {
        path: root.to_string_lossy().into_owned(), total_bytes: 0, skipped_entries: 0,
        categories: ["chats", "work", "memory", "diary", "notes", "tasks", "social", "companion", "media",
            "appCache", "browserCache", "browserData", "extensions", "runtime", "logs", "settings", "other"].into_iter()
            .map(bucket).collect(),
    };
    match std::fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(usage),
        Err(e) => return Err(e.to_string()),
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Err("无效的数据目录".into()),
        _ => {}
    }
    let mut pending = vec![root.to_path_buf()];
    let mut seen_files = SeenFiles::default();
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
                if meta.len() > 0 && seen_files.already_counted(&path, &meta) { continue; }
                let (id, subcategory) = category(path.strip_prefix(root).map_err(|e| e.to_string())?);
                usage.total_bytes = usage.total_bytes.saturating_add(meta.len());
                if let Some(bucket) = usage.categories.iter_mut().find(|bucket| bucket.id == id) {
                    bucket.bytes = bucket.bytes.saturating_add(meta.len());
                    if let Some(child) = bucket.subcategories.iter_mut().find(|child| Some(child.id) == subcategory) {
                        child.bytes = child.bytes.saturating_add(meta.len());
                    }
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

/// 浏览器缓存清理结果：已释放的逻辑字节数，以及被占用/跳过而未删除的条目数。
#[derive(Debug, Default, Serialize)]
pub struct BrowserCacheClear {
    pub freed_bytes: u64,
    pub failed_entries: u64,
}

/// 只清理 WebView2 已识别的缓存目录（网页资源、脚本、GPU、组件下载）。
///
/// 登录信息、Cookies、Local Storage、IndexedDB、会话与扩展属于站点数据，
/// 一律不碰；崩溃报告与运行指标也不属于缓存。目录本身保留，运行中的浏览器
/// 会按需重建文件；被占用的文件跳过并计数，供界面提示稍后重试。
pub fn clear_browser_cache(root: &Path) -> Result<BrowserCacheClear, String> {
    for ancestor in root.ancestors() {
        if std::fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?.file_type().is_symlink() {
            return Err("不能清理符号链接中的缓存目录".into());
        }
    }
    let browser = root.join("browser");
    let mut result = BrowserCacheClear::default();
    if !browser.is_dir() { return Ok(result); }

    let mut pending = vec![browser];
    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => { result.failed_entries += 1; continue; }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => { result.failed_entries += 1; continue; }
            };
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => { result.failed_entries += 1; continue; }
            };
            if meta.file_type().is_symlink() { result.failed_entries += 1; continue; }
            if !meta.is_dir() { continue; }
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
            if super::policy::clearable_browser_cache(relative) {
                clear_cache_dir_files(&path, &mut result);
            } else {
                pending.push(path);
            }
        }
    }
    Ok(result)
}

fn clear_cache_dir_files(dir: &Path, result: &mut BrowserCacheClear) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => { result.failed_entries += 1; return; }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => { result.failed_entries += 1; continue; }
        };
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => { result.failed_entries += 1; continue; }
        };
        if meta.file_type().is_symlink() { result.failed_entries += 1; continue; }
        if meta.is_dir() {
            // 保留目录结构，避免与运行中的浏览器抢占目录。
            clear_cache_dir_files(&path, result);
        } else if meta.is_file() {
            match std::fs::remove_file(&path) {
                Ok(()) => result.freed_bytes = result.freed_bytes.saturating_add(meta.len()),
                Err(_) => result.failed_entries += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detailed_categories_respect_roots_and_cache_boundaries() {
        let cases = [
            ("cache/assets/notebook-fonts/font.woff2", "appCache", Some("fontCache")),
            ("fonts/ma-shan-zheng.woff2", "appCache", Some("fontCache")),
            ("cache/corpus_embeddings/emotions.bin", "appCache", Some("embeddingCache")),
            ("cache/unknown.bin", "appCache", Some("otherAppCache")),
            ("characters/nana/sound/cache/speech.wav", "appCache", Some("speechCache")),
            ("characters/nana/companion-v2/history/messages.db", "chats", None),
            ("characters/nana/companion-v2/memory/embeddings.db-wal", "memory", None),
            ("companion-v2/common/memory/knowledge.db", "memory", None),
            ("characters/nana/diary/day.json", "diary", None),
            ("characters/nana/notebook/attachments/image.png", "notes", None),
            ("characters/nana/notebook/note_collected_mem_123/note.html", "memory", None),
            ("characters/nana/notebook/note_ui_sample/note.html", "notes", None),
            ("user/quick_notes/note.json", "notes", None),
            ("characters/nana/companion-v2/psychology.json", "companion", None),
            ("characters/nana/companion-v2/mind/state.json", "companion", None),
            ("characters/nana/sound/config.json", "settings", None),
            ("characters/nana/sound/voice.wav", "media", None),
            ("coding_events/task/history/events.jsonl", "work", None),
            ("web_evidence/task/evidence.json", "work", None),
            ("browser/engine/EBWebView/Default/History", "browserData", None),
            ("browser/engine/EBWebView/Default/Network/Cookies", "browserData", None),
            ("browser/engine/EBWebView/Default/Service Worker/CacheStorage/offline", "browserData", None),
            ("browser/engine/EBWebView/WV2Profile_default/Code Cache/js/data", "browserCache", Some("webCache")),
            ("browser/engine/EBWebView/Profile 1/GPUCache/data", "browserCache", Some("graphicsCache")),
            ("browser/engine/EBWebView/GrShaderCache/data", "browserCache", Some("graphicsCache")),
            ("browser/engine/EBWebView/component_crx_cache/package", "browserCache", Some("componentCache")),
            ("browser/engine/EBWebView/Crashpad/reports/dump", "logs", None),
            ("browser/engine/EBWebView/BrowserMetrics/report", "logs", None),
            ("plugins/example/history/data", "extensions", None),
            ("python-libs/package/cache/data", "runtime", None),
            ("logs/app.log", "logs", None),
            ("sns/contacts/person/media/image.png", "social", None),
            ("todo/tasks.json", "tasks", None),
            ("config.yaml", "settings", None),
            ("unknown/history/private.data", "other", None),
            ("characters/nana/unknown/private.data", "other", None),
        ];
        let dir = tempfile::tempdir().unwrap();
        for (name, expected, child) in cases {
            assert_eq!(category(Path::new(name)), (expected, child), "{name}");
            let file = dir.path().join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, [0; 7]).unwrap();
        }
        let usage = scan(dir.path()).unwrap();
        assert_eq!(usage.total_bytes, cases.len() as u64 * 7);
        assert_eq!(usage.categories.iter().map(|c| c.bytes).sum::<u64>(), usage.total_bytes);
        for bucket in usage.categories.iter().filter(|c| !c.subcategories.is_empty()) {
            assert_eq!(bucket.subcategories.iter().map(|c| c.bytes).sum::<u64>(), bucket.bytes, "{}", bucket.id);
        }
    }

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
        assert_eq!(usage.categories.iter().find(|c| c.id == "appCache").unwrap().bytes, 10);
        assert_eq!(usage.categories.iter().find(|c| c.id == "browserCache").unwrap().bytes, 10);
        clear_cache(dir.path()).unwrap();
        assert_eq!(scan(dir.path()).unwrap().total_bytes, 60);
        assert!(dir.path().join("unknown/user.data").exists());
        assert!(dir.path().join("browser/engine/EBWebView/Default/Cache/cache.bin").exists());
    }

    #[test]
    fn hard_links_count_once() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("characters/nana/notebook/a/fonts/ma-shan-zheng.woff2");
        let second = dir.path().join("characters/vivian/notebook/b/fonts/ma-shan-zheng.woff2");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::write(&first, [0; 11]).unwrap();
        std::fs::hard_link(&first, &second).unwrap();
        let usage = scan(dir.path()).unwrap();
        assert_eq!(usage.total_bytes, 11);
        assert_eq!(usage.categories.iter().find(|c| c.id == "notes").unwrap().bytes, 11);
    }

    #[test]
    fn browser_cache_clear_keeps_site_data_and_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "browser/engine/EBWebView/WV2Profile_default/Cache/cache.bin",
            "browser/engine/EBWebView/WV2Profile_default/Code Cache/js.bin",
            "browser/engine/EBWebView/component_crx_cache/package.bin",
            "browser/engine/EBWebView/WV2Profile_default/Network/Cookies",
            "browser/engine/EBWebView/Crashpad/report.bin",
        ] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, [0; 5]).unwrap();
        }
        let result = clear_browser_cache(dir.path()).unwrap();
        assert_eq!(result.freed_bytes, 15);
        assert_eq!(result.failed_entries, 0);
        for name in [
            "browser/engine/EBWebView/WV2Profile_default/Cache/cache.bin",
            "browser/engine/EBWebView/WV2Profile_default/Code Cache/js.bin",
            "browser/engine/EBWebView/component_crx_cache/package.bin",
        ] {
            assert!(!dir.path().join(name).exists(), "{name}");
        }
        assert!(dir.path().join("browser/engine/EBWebView/WV2Profile_default/Network/Cookies").exists());
        assert!(dir.path().join("browser/engine/EBWebView/Crashpad/report.bin").exists());
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
