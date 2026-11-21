//! 应用信任列表 - open_application 工具的持久化白名单
//!
//! 用户在确认 toast 中选择"信任此应用"后，应用标识写入此列表，
//! 后续启动同一应用不再询问。
//!
//! 持久化走统一存储网关 [`DomainStore`]：域 `trust`，**每个应用标识一个键**，
//! 新增信任只写入对应键，不重写整份列表。首次运行会把旧版
//! `<用户数据目录>/trusted_apps.json`（JSON 数组）迁移进存储域并改名保留现场。

use std::collections::HashSet;
use std::path::PathBuf;

use once_cell::sync::Lazy;
use parking_lot::RwLock;

use crate::storage::{DomainMutation, DomainStore};
use crate::utils::path::get_user_data_dir;

/// 信任列表存储域名。
const DOMAIN: &str = "trust";

/// 信任列表内存缓存（首次访问时从存储域加载，写穿到网关）。
static TRUSTED_APPS: Lazy<RwLock<HashSet<String>>> = Lazy::new(|| RwLock::new(load_set()));

/// 旧版 JSON 数组文件路径。
fn legacy_path() -> PathBuf {
    get_user_data_dir().join("trusted_apps.json")
}

/// 把旧版 `trusted_apps.json` 迁移进存储域（幂等：迁移后改名，不重复执行）。
fn migrate_legacy(store: &DomainStore) {
    let legacy = legacy_path();
    if !legacy.is_file() {
        return;
    }
    let apps: Vec<String> = match std::fs::read_to_string(&legacy)
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
    {
        Some(apps) => apps,
        None => {
            tracing::warn!("[Trust] 旧版信任列表解析失败，保留原文件待人工处理");
            return;
        }
    };
    let batch: Vec<DomainMutation> = apps
        .iter()
        .map(|app| normalize_app(app))
        .filter(|id| !id.is_empty())
        .map(|id| DomainMutation::Put(id, serde_json::Value::Bool(true)))
        .collect();
    if let Err(e) = store.transaction(DOMAIN, &batch) {
        tracing::warn!("[Trust] 旧版信任列表迁移失败，保留原文件: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&legacy, legacy.with_extension("json.migrated")) {
        tracing::warn!("[Trust] 迁移后改名旧文件失败（下次会重复迁移）: {e}");
    } else {
        tracing::info!("[Trust] 已迁移旧版信任列表 {} 项", batch.len());
    }
}

/// 从存储域加载信任集合；网关不可用时退化为空集（不阻断启动）。
fn load_set() -> HashSet<String> {
    let Ok(store) = DomainStore::shared() else {
        tracing::warn!("[Trust] 存储网关不可用，信任列表按空集处理");
        return HashSet::new();
    };
    migrate_legacy(&store);
    match store.list(DOMAIN) {
        Ok(data) => {
            let set: HashSet<String> = data.into_keys().collect();
            tracing::info!("[Trust] 加载应用信任列表: {} 项", set.len());
            set
        }
        Err(e) => {
            tracing::warn!("[Trust] 读取信任列表失败: {e}");
            HashSet::new()
        }
    }
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

/// 将应用加入信任列表并持久化（单键写入，不重写整份列表）
pub fn add_trusted_app(app: &str) {
    let normalized = normalize_app(app);
    if normalized.is_empty() {
        return;
    }

    let inserted = TRUSTED_APPS.write().insert(normalized.clone());
    if !inserted {
        return;
    }

    match DomainStore::shared() {
        Ok(store) => match store.put(DOMAIN, &normalized, serde_json::Value::Bool(true)) {
            Ok(_) => tracing::info!("[Trust] 应用「{}」已加入信任列表", normalized),
            Err(e) => tracing::warn!("[Trust] 信任列表写盘失败: {e}"),
        },
        Err(e) => tracing::warn!("[Trust] 存储网关不可用，未持久化: {e}"),
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
