//! 凭据服务与匿名身份。
//!
//! 凭据：配置只携带**引用**（环境变量名，如 `DEEPSEEK_API_KEY`）而非明文值；
//! 值按层解析（进程环境 → `<用户数据目录>/.credentials.json`），每次操作即时解析、
//! 从不缓存。`describe` 只报告配置状态，永不返回值本身。
//!
//! 匿名身份：持久化一个与本安装关联的随机 id（`identity.json`），供遥测/反馈/外部
//! 请求关联使用，非认证账号。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::Serialize;
use serde_json::{json, Value};

/// 凭据来源层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    /// 进程环境变量
    Env,
    /// 用户凭据文件（.credentials.json）
    File,
}

/// 凭据服务的配置状态描述（不含值）。
#[derive(Debug, Clone, Serialize)]
pub struct CredentialInfo {
    /// 引用名（环境变量名）
    pub reference: String,
    pub configured: bool,
    pub source: Option<CredentialSource>,
    pub writable: bool,
}

/// 凭据服务。
pub struct CredentialsService {
    file_path: PathBuf,
    /// 凭据文件层缓存（写穿；读时若文件 mtime 变化则重载）
    file_layer: RwLock<BTreeMap<String, String>>,
    file_mtime: RwLock<Option<std::time::SystemTime>>,
}

impl CredentialsService {
    pub fn new() -> Arc<Self> {
        let file_path = crate::utils::path::get_user_data_dir().join(".credentials.json");
        let svc = Arc::new(Self {
            file_path,
            file_layer: RwLock::new(BTreeMap::new()),
            file_mtime: RwLock::new(None),
        });
        svc.reload_file_layer();
        svc
    }

    /// 解析一个凭据引用，返回值。环境层优先，其次文件层。
    pub fn resolve(&self, reference: &str) -> Option<String> {
        if let Ok(v) = std::env::var(reference) {
            if !v.is_empty() {
                return Some(v);
            }
        }
        self.reload_if_changed();
        self.file_layer.read().get(reference).cloned()
    }

    /// 描述一个凭据引用的配置状态（永不返回值）。
    pub fn describe(&self, reference: &str) -> CredentialInfo {
        let env_ok = std::env::var(reference).map(|v| !v.is_empty()).unwrap_or(false);
        let file_ok = {
            self.reload_if_changed();
            self.file_layer.read().contains_key(reference)
        };
        CredentialInfo {
            reference: reference.to_string(),
            configured: env_ok || file_ok,
            source: if env_ok {
                Some(CredentialSource::Env)
            } else if file_ok {
                Some(CredentialSource::File)
            } else {
                None
            },
            writable: !env_ok, // 环境层只读，不可经服务覆盖
        }
    }

    /// 写入/更新文件层的凭据值。
    pub fn set(&self, reference: &str, value: &str) -> Result<(), String> {
        if std::env::var(reference).map(|v| !v.is_empty()).unwrap_or(false) {
            return Err(format!("环境变量 {reference} 已设置（只读来源），不能经服务覆盖"));
        }
        self.reload_if_changed();
        {
            let mut layer = self.file_layer.write();
            layer.insert(reference.to_string(), value.to_string());
        }
        self.persist()
    }

    /// 删除文件层的凭据。
    pub fn unset(&self, reference: &str) -> Result<bool, String> {
        self.reload_if_changed();
        let removed = {
            let mut layer = self.file_layer.write();
            layer.remove(reference).is_some()
        };
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    /// 列出文件层全部引用（不含值）。
    pub fn list_references(&self) -> Vec<CredentialInfo> {
        self.reload_if_changed();
        self.file_layer
            .read()
            .keys()
            .map(|r| self.describe(r))
            .collect()
    }

    fn reload_if_changed(&self) {
        let current = std::fs::metadata(&self.file_path)
            .and_then(|m| m.modified())
            .ok();
        let cached = *self.file_mtime.read();
        if current != cached {
            self.reload_file_layer();
        }
    }

    fn reload_file_layer(&self) {
        let mtime = std::fs::metadata(&self.file_path)
            .and_then(|m| m.modified())
            .ok();
        let map = crate::utils::fs::load_json_or_backup::<BTreeMap<String, String>>(&self.file_path)
            .unwrap_or_default();
        *self.file_layer.write() = map;
        *self.file_mtime.write() = mtime;
    }

    fn persist(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&*self.file_layer.read())
            .map_err(|e| format!("序列化凭据失败: {e}"))?;
        std::fs::write(&self.file_path, json).map_err(|e| format!("写入凭据文件失败: {e}"))
    }
}

// ============================================================================
// 匿名身份
// ============================================================================

/// 读取（首次生成并持久化）本安装的匿名 id。
pub fn anonymous_id() -> String {
    let path = crate::utils::path::get_user_data_dir().join("identity.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(id) = v.get("anonymous_id").and_then(Value::as_str) {
                if !id.is_empty() {
                    return id.to_string();
                }
            }
        }
    }
    let id = format!("anon-{}", uuid::Uuid::new_v4().simple());
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({ "anonymous_id": id })).unwrap_or_default(),
    );
    id
}

/// 进程级缓存（避免每次读盘）。
pub fn cached_anonymous_id() -> String {
    use once_cell::sync::Lazy;
    static ID: Lazy<String> = Lazy::new(anonymous_id);
    ID.clone()
}
