//! 研究任务 JSON 持久化。

use std::collections::HashMap;
use std::path::PathBuf;

use super::task::ResearchTask;

/// JSON 持久化格式
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct PersistedStore {
    tasks: HashMap<String, ResearchTask>,
}

/// 从文件加载研究任务（不存在则返回空集合）
pub fn load(path: &PathBuf) -> HashMap<String, ResearchTask> {
    if !path.exists() {
        tracing::info!("[Research] 无历史数据，从空存储开始: {:?}", path);
        return HashMap::new();
    }
    match std::fs::read_to_string(path) {
        Ok(json) => match serde_json::from_str::<PersistedStore>(&json) {
            Ok(store) => {
                tracing::info!(
                    "[Research] 加载 {} 条研究任务 from {:?}",
                    store.tasks.len(),
                    path
                );
                store.tasks
            }
            Err(e) => {
                tracing::warn!("[Research] JSON 解析失败，使用空存储: {}", e);
                HashMap::new()
            }
        },
        Err(e) => {
            tracing::warn!("[Research] 文件读取失败: {}", e);
            HashMap::new()
        }
    }
}

/// 持久化研究任务到文件
pub fn save(path: &PathBuf, tasks: &HashMap<String, ResearchTask>) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::error!("[Research] 创建目录失败 {:?}: {}", parent, e);
            return;
        }
    }
    let store = PersistedStore {
        tasks: tasks.clone(),
    };
    match serde_json::to_string_pretty(&store) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                tracing::error!("[Research] 写入失败 {:?}: {}", path, e);
            }
        }
        Err(e) => {
            tracing::error!("[Research] 序列化失败: {}", e);
        }
    }
}
