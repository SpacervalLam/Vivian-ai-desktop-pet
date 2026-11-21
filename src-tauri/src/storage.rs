//! Durable domain transactions and committed change notifications.
//! Reads always observe disk state. Per-domain OS locks serialize independent
//! store instances and processes; unrelated domains do not share a write lock.
#[path = "storage/backup.rs"]
pub(crate) mod backup;
#[path = "storage/export.rs"]
pub mod export;
#[path = "storage/policy.rs"]
pub mod policy;
#[path = "storage/usage.rs"]
pub mod usage;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{mpsc, Arc},
};

#[derive(Debug, Clone, Serialize)]
pub struct DomainChanged {
    pub domain: String,
    pub key: String,
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub enum DomainMutation {
    Put(String, Value),
    Delete(String),
}

/// 存储域 schema 迁移的一步：把已解析的键值快照从 `version - 1` 升级到 `version`。
///
/// `upgrade` 只处理数据形态，不触碰磁盘；迁移步骤必须幂等，因为旧版本文件在
/// 首次写入前可能被重复读取。
pub struct DomainMigration {
    pub version: u32,
    pub description: &'static str,
    pub upgrade: fn(&mut BTreeMap<String, Value>),
}

#[derive(Default, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    revision: u64,
    data: BTreeMap<String, Value>,
}

pub struct DomainStore {
    root: PathBuf,
    subscribers: Mutex<Vec<mpsc::Sender<DomainChanged>>>,
    /// 按 version 升序排列的迁移步骤。
    migrations: Vec<DomainMigration>,
}

/// 进程级共享存储网关：域根固定为 `<用户数据目录>/domains`。
static SHARED: once_cell::sync::OnceCell<Arc<DomainStore>> = once_cell::sync::OnceCell::new();

impl DomainStore {
    pub fn new() -> Result<Arc<Self>, String> {
        Self::open(crate::utils::path::get_user_data_dir().join("domains"))
    }

    /// 进程级共享实例。作为各模块的统一持久化网关，避免各自散落读写 JSON 文件。
    pub fn shared() -> Result<Arc<Self>, String> {
        SHARED.get_or_try_init(DomainStore::new).map(Arc::clone)
    }

    pub fn open(root: PathBuf) -> Result<Arc<Self>, String> {
        Self::open_with_migrations(root, Vec::new())
    }

    /// 打开存储域根，并声明其 schema 迁移链。
    pub fn open_with_migrations(
        root: PathBuf,
        mut migrations: Vec<DomainMigration>,
    ) -> Result<Arc<Self>, String> {
        migrations.sort_by_key(|migration| migration.version);
        let mut seen = std::collections::HashSet::new();
        if migrations.iter().any(|m| !seen.insert(m.version)) {
            return Err("存储域迁移版本重复".into());
        }
        std::fs::create_dir_all(root.join(".locks")).map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            root,
            subscribers: Mutex::new(Vec::new()),
            migrations,
        }))
    }

    /// 当前支持的存储域版本；无迁移链时为 1。
    fn current_version(&self) -> u32 {
        self.migrations.last().map(|m| m.version).unwrap_or(1)
    }

    fn validate(domain: &str) -> Result<(), String> {
        let reserved = matches!(
            domain.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        );
        if domain.is_empty()
            || domain.chars().count() > 64
            || reserved
            || !domain.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            return Err("无效的存储域名".into());
        }
        Ok(())
    }

    fn locked<T>(
        &self,
        domain: &str,
        run: impl FnOnce(Snapshot) -> Result<T, String>,
    ) -> Result<T, String> {
        Self::validate(domain)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(".locks").join(format!("{domain}.lock")))
            .map_err(|e| e.to_string())?;
        let started = std::time::Instant::now();
        loop {
            match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    if started.elapsed() >= std::time::Duration::from_secs(3) {
                        return Err("等待存储域事务锁超时".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        let path = self.root.join(format!("{domain}.json"));
        let mut snapshot = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("存储域损坏，未覆盖: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Snapshot {
                version: self.current_version(),
                ..Default::default()
            },
            Err(e) => return Err(e.to_string()),
        };
        let current = self.current_version();
        if snapshot.version > current {
            return Err(format!(
                "存储域版本 {} 高于当前支持的 {}，未覆盖",
                snapshot.version, current
            ));
        }
        // 按版本顺序升级旧数据；升级结果就地落盘，避免旧文件被当成空态反复读取。
        let mut upgraded = false;
        for migration in &self.migrations {
            if migration.version > snapshot.version {
                (migration.upgrade)(&mut snapshot.data);
                snapshot.version = migration.version;
                upgraded = true;
            }
        }
        if upgraded {
            let bytes = serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?;
            crate::utils::fs::atomic_write(&path, &bytes).map_err(|e| e.to_string())?;
        }
        run(snapshot)
    }

    /// Local subscribers receive notifications only after successful durable commit.
    pub fn subscribe(&self) -> mpsc::Receiver<DomainChanged> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.lock().push(tx);
        rx
    }

    pub fn get(&self, domain: &str, key: &str) -> Result<Option<Value>, String> {
        self.locked(domain, |state| Ok(state.data.get(key).cloned()))
    }

    pub fn list(&self, domain: &str) -> Result<BTreeMap<String, Value>, String> {
        self.locked(domain, |state| Ok(state.data))
    }

    /// All keys in a batch share one atomic file replacement and revision.
    pub fn transaction(
        &self,
        domain: &str,
        mutations: &[DomainMutation],
    ) -> Result<Vec<DomainChanged>, String> {
        self.locked(domain, |mut state| {
            let original = state.data.clone();
            for mutation in mutations {
                match mutation {
                    DomainMutation::Put(key, value) => {
                        state.data.insert(key.clone(), value.clone());
                    }
                    DomainMutation::Delete(key) => {
                        state.data.remove(key);
                    }
                }
            }
            let keys: std::collections::BTreeSet<_> =
                original.keys().chain(state.data.keys()).cloned().collect();
            let changed: Vec<_> = keys
                .into_iter()
                .filter(|k| original.get(k) != state.data.get(k))
                .collect();
            if changed.is_empty() {
                return Ok(Vec::new());
            }
            state.revision = state.revision.checked_add(1).ok_or("存储版本溢出")?;
            let bytes = serde_json::to_vec(&state).map_err(|e| e.to_string())?;
            crate::utils::fs::atomic_write(&self.root.join(format!("{domain}.json")), &bytes)
                .map_err(|e| e.to_string())?;
            let events: Vec<_> = changed
                .into_iter()
                .map(|key| DomainChanged {
                    domain: domain.into(),
                    key,
                    revision: state.revision,
                })
                .collect();
            let mut subscribers = self.subscribers.lock();
            subscribers.retain(|subscriber| {
                events
                    .iter()
                    .all(|event| subscriber.send(event.clone()).is_ok())
            });
            Ok(events)
        })
    }

    pub fn put(
        &self,
        domain: &str,
        key: &str,
        value: Value,
    ) -> Result<Option<DomainChanged>, String> {
        Ok(self
            .transaction(domain, &[DomainMutation::Put(key.into(), value)])?
            .into_iter()
            .next())
    }

    pub fn delete(&self, domain: &str, key: &str) -> Result<bool, String> {
        Ok(!self
            .transaction(domain, &[DomainMutation::Delete(key.into())])?
            .is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domain_migrations_upgrade_legacy_files_and_reject_newer_versions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("prefs.json"),
            r#"{"version":1,"revision":3,"data":{"theme":"dark"}}"#,
        )
        .unwrap();
        let store = DomainStore::open_with_migrations(
            dir.path().into(),
            vec![DomainMigration {
                version: 2,
                description: "wrap scalar values",
                upgrade: |data| {
                    for value in data.values_mut() {
                        if let Some(text) = value.as_str() {
                            *value = serde_json::json!({ "value": text });
                        }
                    }
                },
            }],
        )
        .unwrap();
        assert_eq!(
            store.get("prefs", "theme").unwrap(),
            Some(serde_json::json!({ "value": "dark" }))
        );
        // 升级结果已就地落盘，避免旧文件被反复当成旧版读取
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("prefs.json")).unwrap()).unwrap();
        assert_eq!(raw["version"], 2);
        // 更高版本的文件被拒绝，且不写入缓存/磁盘
        std::fs::write(
            dir.path().join("future.json"),
            r#"{"version":9,"revision":0,"data":{}}"#,
        )
        .unwrap();
        assert!(store.get("future", "x").is_err());
    }
    #[test]
    fn domain_commits_are_atomic_and_observable_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = DomainStore::open(dir.path().into()).unwrap();
        let rx = store.subscribe();
        let events = store
            .transaction(
                "settings",
                &[
                    DomainMutation::Put("a".into(), 1.into()),
                    DomainMutation::Put("b".into(), 2.into()),
                ],
            )
            .unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.revision == 1));
        assert_eq!(rx.try_iter().count(), 2);
        assert!(store.put("settings", "a", 1.into()).unwrap().is_none());
        assert!(rx.try_recv().is_err());
        let reopened = DomainStore::open(dir.path().into()).unwrap();
        assert_eq!(reopened.get("settings", "b").unwrap(), Some(2.into()));
        assert!(reopened.delete("settings", "b").unwrap());
        assert!(store.get("settings", "b").unwrap().is_none());
        assert!(store.put("../settings", "a", 3.into()).is_err());
        assert!(store.put("set-tings", "a", 3.into()).is_err());
    }
    #[test]
    fn domain_independent_instances_merge_concurrent_writes() {
        let dir = tempfile::tempdir().unwrap();
        let handles: Vec<_> = (0..4)
            .map(|worker| {
                let store = DomainStore::open(dir.path().into()).unwrap();
                std::thread::spawn(move || {
                    for i in 0..40 {
                        store
                            .put("shared", &format!("{worker}_{i}"), i.into())
                            .unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            DomainStore::open(dir.path().into())
                .unwrap()
                .list("shared")
                .unwrap()
                .len(),
            160
        );
    }
    #[test]
    fn domain_corruption_does_not_become_empty_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = DomainStore::open(dir.path().into()).unwrap();
        let rx = store.subscribe();
        std::fs::write(dir.path().join("data.json"), "broken original").unwrap();
        assert!(store.get("data", "a").is_err());
        assert!(store.put("data", "a", 1.into()).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("data.json")).unwrap(),
            "broken original"
        );
        assert!(rx.try_recv().is_err());
    }
    #[cfg(windows)]
    #[test]
    fn domain_disk_failure_never_publishes_uncommitted_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = DomainStore::open(dir.path().into()).unwrap();
        store.put("data", "a", 1.into()).unwrap();
        let rx = store.subscribe();
        let path = dir.path().join("data.json");
        let original = std::fs::read(&path).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions.clone()).unwrap();
        let failed = store.put("data", "a", 2.into()).is_err();
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(failed);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(store.get("data", "a").unwrap(), Some(1.into()));
        assert!(rx.try_recv().is_err());
    }
}
