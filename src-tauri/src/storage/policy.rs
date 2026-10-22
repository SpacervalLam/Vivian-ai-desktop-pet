//! Backup/restore conservation policy: unknown top-level data stays durable.
//!
//! 顶层分层契约：[`DataLayer`] 区分「权威 / 派生 / 运行期 / 未知」。备份与恢复
//! 只跳过派生层与运行期层，未知层按权威层处理，避免误删用户数据。
//! [`browser_cache`] 是更细粒度的规则：浏览器引擎内部缓存属派生数据，同样跳过。
use std::path::Path;

/// 用户数据目录顶层条目的数据分层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataLayer {
    /// 用户数据的唯一副本：必须备份。
    Authoritative,
    /// 可从权威数据重建（缓存、嵌入索引）：不备份，可丢失。
    Derived,
    /// 运行期产物（进程、日志、临时文件）：不备份。
    Ephemeral,
    /// 未归类的条目：按权威层处理，默认保留。
    Unknown,
}

/// 已知派生层顶层目录（可从权威数据重建）。
const DERIVED_TOP_LEVEL: &[&str] = &["cache"];
/// 已知运行期层顶层条目（无迁移价值）。
const EPHEMERAL_TOP_LEVEL: &[&str] = &[
    // 内嵌 Python 运行时（约 280MB，新设备安装时自行释放）
    "python-libs",
    "logs",
    "pids",
    "spill",
    "tmp",
    ".runtime.lease",
    ".backup.pending.json",
    ".backup.result.json",
    ".backup.queue.lock",
    ".restore_pending",
    ".restore_staging",
    ".factory_reset_pending",
];

/// 顶层条目分层归类。
///
/// `skills` / `plugins` / `mcp` 等自进化内容是用户定制数据，未被列出故归入
/// 未知层（按权威层处理），随备份打包并在恢复时回填。
pub fn classify_top_level(name: &str) -> DataLayer {
    if DERIVED_TOP_LEVEL.contains(&name) {
        return DataLayer::Derived;
    }
    if EPHEMERAL_TOP_LEVEL.contains(&name) || name.starts_with("config.yaml.clipboard-hint-backup-")
    {
        return DataLayer::Ephemeral;
    }
    DataLayer::Unknown
}

/// 备份与恢复是否跳过该顶层条目：派生层与运行期层不收录，权威层与未知层保留。
pub fn backup_skip_top_level(name: &str) -> bool {
    matches!(
        classify_top_level(name),
        DataLayer::Derived | DataLayer::Ephemeral
    )
}

pub fn browser_cache(relative: &Path) -> bool {
    let parts: Vec<_> = relative
        .components()
        .map(|p| p.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    if parts.len() < 4 || parts[..3] != ["browser", "engine", "ebwebview"] {
        return false;
    }
    let shared = matches!(
        parts[3].as_str(),
        "shadercache"
            | "grshadercache"
            | "dawncache"
            | "gpucache"
            | "crashpad"
            | "browsermetrics"
            | "component_crx_cache"
    );
    let profile = parts[3] == "default"
        || parts[3].starts_with("wv2profile_")
        || parts[3].starts_with("profile ");
    shared
        || (profile
            && parts.get(4).is_some_and(|p| {
                matches!(
                    p.as_str(),
                    "cache"
                        | "code cache"
                        | "gpucache"
                        | "shadercache"
                        | "grshadercache"
                        | "dawncache"
                        | "media cache"
                )
            }))
}

/// 可整体清理的浏览器缓存目录；崩溃报告与运行指标属于诊断数据，不在此列。
pub fn clearable_browser_cache(relative: &Path) -> bool {
    if !browser_cache(relative) {
        return false;
    }
    let name = relative
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    !matches!(name.as_str(), "crashpad" | "browsermetrics")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_layers_exclude_only_derived_and_ephemeral() {
        // 派生层与运行期层不进入备份/恢复
        for name in [
            "cache",
            "python-libs",
            "logs",
            "pids",
            "spill",
            ".runtime.lease",
            ".restore_staging",
            ".factory_reset_pending",
            "config.yaml.clipboard-hint-backup-20261010185633",
        ] {
            assert!(
                backup_skip_top_level(name),
                "{name} 应被跳过: {:?}",
                classify_top_level(name)
            );
        }
        // 权威层与未知层（含用户定制内容）必须保留
        for name in [
            "characters",
            "companion-v2",
            "config.yaml",
            "config",
            "persona",
            "shared",
            "sound",
            "plugins",
            "skills",
            "mcp",
            "browser",
            "images",
            "coding_workflow",
        ] {
            assert!(
                !backup_skip_top_level(name),
                "{name} 不应被跳过: {:?}",
                classify_top_level(name)
            );
        }
    }

    #[test]
    fn browser_policy_keeps_login_offline_data_and_unknown_directories() {
        for rel in [
            "browser/state.json",
            "browser/engine/EBWebView/Local State",
            "browser/engine/EBWebView/WV2Profile_default/Network/Cookies",
            "browser/engine/EBWebView/WV2Profile_default/Local Storage/leveldb/data",
            "browser/engine/EBWebView/WV2Profile_default/IndexedDB/data",
            "browser/engine/EBWebView/WV2Profile_default/Sessions/session",
            "browser/engine/EBWebView/WV2Profile_default/Service Worker/CacheStorage/offline",
            "browser/engine/EBWebView/WV2Profile_default/Extensions/id/data",
            "browser/unknown/Cache/user-data",
        ] {
            assert!(!browser_cache(Path::new(rel)), "{rel}");
        }
        for rel in [
            "browser/engine/EBWebView/WV2Profile_default/Cache/data",
            "browser/engine/EBWebView/Default/Code Cache/js",
            "browser/engine/EBWebView/GrShaderCache/data",
            "browser/engine/EBWebView/Crashpad/reports/crash",
            "browser/engine/EBWebView/component_crx_cache/package",
        ] {
            assert!(browser_cache(Path::new(rel)), "{rel}");
        }
    }

    #[test]
    fn only_cache_directories_are_clearable() {
        for rel in [
            "browser/engine/EBWebView/WV2Profile_default/Cache",
            "browser/engine/EBWebView/Default/Code Cache",
            "browser/engine/EBWebView/GrShaderCache",
            "browser/engine/EBWebView/component_crx_cache",
        ] {
            assert!(clearable_browser_cache(Path::new(rel)), "{rel}");
        }
        for rel in [
            "browser/engine/EBWebView/Crashpad",
            "browser/engine/EBWebView/BrowserMetrics",
            "browser/engine/EBWebView/WV2Profile_default/Network",
            "browser/engine/EBWebView/WV2Profile_default/Network/Cookies",
            "browser/engine/EBWebView/WV2Profile_default/Local Storage/leveldb",
        ] {
            assert!(!clearable_browser_cache(Path::new(rel)), "{rel}");
        }
    }
}
