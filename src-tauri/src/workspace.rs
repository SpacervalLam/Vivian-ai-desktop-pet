//! 工作区注册表 —— 管理工作目录实体及其与会话的挂接。
//!
//! 工作区是「一个项目目录」的一等实体：创建时按 realpath 规范化（每个 canonical
//! path 至多一条），可排序（insertBefore 语义）、可删除；工作/编程会话创建时挂接
//! 到某个工作区，之后按工作区聚合查询会话。持久化到 `<用户数据目录>/workspaces.json`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 工作区实体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    /// 规范化后的绝对路径（canonical path，每工作区唯一）
    pub path: String,
    /// 可选标题（默认取目录名）
    pub title: String,
    pub created_at: i64,
    /// 挂接到此工作区的会话 ID（按挂接顺序）
    pub session_ids: Vec<String>,
}

/// 工作区注册表。
#[derive(Clone)]
pub struct WorkspaceRegistry {
    inner: Arc<RwLock<BTreeMap<String, Workspace>>>,
}

fn store_path() -> PathBuf {
    crate::utils::path::get_user_data_dir().join("workspaces.json")
}

impl WorkspaceRegistry {
    pub fn new() -> Arc<Self> {
        let inner = crate::utils::fs::load_json_or_backup::<Vec<Workspace>>(&store_path())
            .map(|list| {
                list.into_iter()
                    .map(|w| (w.id.clone(), w))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        Arc::new(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    fn persist(&self) {
        let list: Vec<Workspace> = self.inner.read().values().cloned().collect();
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            let _ = std::fs::write(store_path(), json);
        }
    }

    /// 创建（或返回已存在的）工作区。同一路径重复创建返回既有条目。
    pub fn create(&self, path: &str, title: Option<&str>) -> Workspace {
        let canonical = normalize_path(path);
        {
            let inner = self.inner.read();
            if let Some(existing) = inner.values().find(|w| w.path == canonical) {
                return existing.clone();
            }
        }
        let dir_name = Path::new(&canonical)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| canonical.clone());
        let ws = Workspace {
            id: format!("ws-{}", uuid::Uuid::new_v4().simple()),
            path: canonical.clone(),
            title: title
                .filter(|t| !t.is_empty())
                .map(|t| t.to_string())
                .unwrap_or(dir_name),
            created_at: chrono::Local::now().timestamp(),
            session_ids: Vec::new(),
        };
        self.inner.write().insert(ws.id.clone(), ws.clone());
        self.persist();
        ws
    }

    /// 按路径查找工作区。
    pub fn resolve_by_path(&self, path: &str) -> Option<Workspace> {
        let canonical = normalize_path(path);
        self.inner
            .read()
            .values()
            .find(|w| w.path == canonical)
            .cloned()
    }

    /// 按 ID 查找。
    pub fn get(&self, id: &str) -> Option<Workspace> {
        self.inner.read().get(id).cloned()
    }

    /// 列出全部工作区（按创建顺序）。
    pub fn list(&self) -> Vec<Workspace> {
        let mut list: Vec<Workspace> = self.inner.read().values().cloned().collect();
        list.sort_by_key(|w| w.created_at);
        list
    }

    /// 把会话挂接到工作区（按路径解析；不存在则自动创建）。
    pub fn attach_session(&self, path: &str, session_id: &str) -> Option<Workspace> {
        let ws = self.create(path, None);
        let mut inner = self.inner.write();
        if let Some(w) = inner.get_mut(&ws.id) {
            if !w.session_ids.iter().any(|s| s == session_id) {
                w.session_ids.push(session_id.to_string());
            }
            let updated = w.clone();
            drop(inner);
            self.persist();
            Some(updated)
        } else {
            None
        }
    }

    /// 解除会话挂接。
    pub fn detach_session(&self, workspace_id: &str, session_id: &str) -> bool {
        let mut inner = self.inner.write();
        if let Some(w) = inner.get_mut(workspace_id) {
            let before = w.session_ids.len();
            w.session_ids.retain(|s| s != session_id);
            let changed = before != w.session_ids.len();
            drop(inner);
            if changed {
                self.persist();
            }
            changed
        } else {
            false
        }
    }

    /// 删除工作区（会话本身不受影响，仅解除归属）。
    pub fn delete(&self, id: &str) -> bool {
        let removed = self.inner.write().remove(id).is_some();
        if removed {
            self.persist();
        }
        removed
    }
}

/// 路径规范化（展开 `.`、移除 `..`，统一分隔符），不要求路径存在。
fn normalize_path(p: &str) -> String {
    let path = Path::new(p);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().replace('/', "\\")
}
