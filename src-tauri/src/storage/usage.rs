//! File sizes within Vivian's data directory. Never follows links.
use serde::Serialize;
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
        "notebook" => ("notes", None),
        "images" | "screenshots" | "audio" | "attachments" | "media" => ("media", None),
        "sound" if owned.get(1).is_some_and(|p| p == "config.json") => ("settings", None),
        "sound" => ("media", None),
        "persona" | "psychology" | "presence" | "psychology.json" | "mind" | "proactive" | "meme_acquisition_state.json" => ("companion", None),
        _ if section.starts_with("consolidation_health_") && first == "companion-v2" => ("memory", None),
        _ => ("other", None),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detailed_categories_respect_roots_and_cache_boundaries() {
        let cases = [
            ("cache/assets/notebook-fonts/font.woff2", "appCache", Some("fontCache")),
            ("cache/corpus_embeddings/emotions.bin", "appCache", Some("embeddingCache")),
            ("cache/unknown.bin", "appCache", Some("otherAppCache")),
            ("characters/nana/sound/cache/speech.wav", "appCache", Some("speechCache")),
            ("characters/nana/companion-v2/history/messages.db", "chats", None),
            ("characters/nana/companion-v2/memory/embeddings.db-wal", "memory", None),
            ("companion-v2/common/memory/knowledge.db", "memory", None),
            ("characters/nana/diary/day.json", "diary", None),
            ("characters/nana/notebook/attachments/image.png", "notes", None),
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
