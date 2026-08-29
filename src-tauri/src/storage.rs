//! 存储抽象 —— 命名域 KV（domain KV）+ 变更事件。
//!
//! 领域数据（待办/设置/工作区等）以「域」为单位挂载：每个域是一张持久化 KV 表
//!（JSON 后端，原子写），读走内存权威态、写经串行化链路，变更后广播
//! `domain/changed` 事件供订阅者（前端刷新/联动逻辑）响应。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::Value;

/// 域变更事件。
#[derive(Debug, Clone, Serialize)]
pub struct DomainChanged {
    pub domain: String,
    pub key: String,
}

/// 单个域 facility：内存权威态 + 持久化文件。
struct DomainFacility {
    data: RwLock<BTreeMap<String, Value>>,
    path: PathBuf,
}

impl DomainFacility {
    fn load(path: PathBuf) -> Self {
        let data = crate::utils::fs::load_json_or_backup::<BTreeMap<String, Value>>(&path)
            .unwrap_or_default();
        Self {
            data: RwLock::new(data),
            path,
        }
    }

    fn persist(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&*self.data.read()) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// 域 KV 存储中枢：按域名打开 facility，写操作串行化并广播变更。
pub struct DomainStore {
    root: PathBuf,
    facilities: RwLock<BTreeMap<String, Arc<DomainFacility>>>,
    /// 串行化写链（保证同域写操作的顺序一致性）
    write_lock: Mutex<()>,
}

impl DomainStore {
    pub fn new() -> Arc<Self> {
        let root = crate::utils::path::get_user_data_dir().join("domains");
        let _ = std::fs::create_dir_all(&root);
        Arc::new(Self {
            root,
            facilities: RwLock::new(BTreeMap::new()),
            write_lock: Mutex::new(()),
        })
    }

    fn facility(&self, domain: &str) -> Arc<DomainFacility> {
        // 卫生：域名只允许字母数字与下划线，避免路径注入
        let safe: String = domain
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if safe.is_empty() {
            // 兜底域（不应发生）
            return self
                .facilities
                .write()
                .entry("_default".into())
                .or_insert_with(|| {
                    Arc::new(DomainFacility::load(self.root.join("_default.json")))
                })
                .clone();
        }
        self.facilities
            .write()
            .entry(safe.clone())
            .or_insert_with(|| Arc::new(DomainFacility::load(self.root.join(format!("{safe}.json")))))
            .clone()
    }

    /// 读取一个键。
    pub fn get(&self, domain: &str, key: &str) -> Option<Value> {
        self.facility(domain).data.read().get(key).cloned()
    }

    /// 列出一个域的全部键值。
    pub fn list(&self, domain: &str) -> BTreeMap<String, Value> {
        self.facility(domain).data.read().clone()
    }

    /// 写入一个键（串行化 + 持久化 + 变更事件）。
    pub fn put(&self, domain: &str, key: &str, value: Value) {
        let facility = self.facility(domain);
        {
            let _guard = self.write_lock.lock();
            facility.data.write().insert(key.to_string(), value);
            facility.persist();
        }
        tracing::debug!(domain, key, "[domain] 键已变更");
    }

    /// 删除一个键。
    pub fn delete(&self, domain: &str, key: &str) -> bool {
        let facility = self.facility(domain);
        let removed = {
            let _guard = self.write_lock.lock();
            let removed = facility.data.write().remove(key).is_some();
            if removed {
                facility.persist();
            }
            removed
        };
        removed
    }
}
