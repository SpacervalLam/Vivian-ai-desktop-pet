//! 自主定点唤醒 — 角色给自己安排的"稍后再来"日程。
//!
//! 由 LLM 在对话中调用 `schedule_wakeup` 工具注册（如"我 20 分钟后再来看看"
//! "明早等用户起床我去打招呼"），到期后由 ProactiveOrchestrator::tick 消费，
//! 以主动消息形式兑现承诺。持久化到 `characters/<char_id>/proactive/wakeups.json`，
//! 重启后未到期项自动恢复。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};

/// 单条唤醒任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledWakeup {
    pub id: String,
    /// 到期时间戳（Unix 秒）
    pub due_at: f64,
    /// 唤醒目的（LLM 填写，生成主动消息时的上下文）
    pub purpose: String,
    /// 创建时间戳
    pub created_at: f64,
}

/// 单角色的唤醒队列（内存 + JSON 持久化）
#[derive(Debug, Default, Serialize, Deserialize)]
struct WakeupData {
    #[serde(default)]
    items: Vec<ScheduledWakeup>,
    /// 自增序号（id 生成）
    #[serde(default)]
    next_seq: u64,
}

/// 全局注册表：char_id → WakeupScheduler（工具层按角色路由）
static WAKEUP_SCHEDULERS: Lazy<RwLock<HashMap<String, Arc<WakeupScheduler>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

pub struct WakeupScheduler {
    char_id: String,
    path: PathBuf,
    data: RwLock<WakeupData>,
}

impl WakeupScheduler {
    fn new(char_id: &str) -> Self {
        let dir = crate::utils::path::get_character_data_dir(char_id)
            .join("proactive");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("wakeups.json");
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<WakeupData>(&s).ok())
            .unwrap_or_default();
        Self {
            char_id: char_id.to_string(),
            path,
            data: RwLock::new(data),
        }
    }

    /// 注册一条唤醒（相对秒数 + 目的）
    ///
    /// 返回任务 id。同 purpose 的未到期重复注册会被去重（刷新到期时间）。
    pub fn schedule(&self, in_seconds: f64, purpose: &str) -> VivianResult<String> {
        const MAX_HORIZON_SECS: f64 = 48.0 * 3600.0;
        let in_seconds = in_seconds.clamp(60.0, MAX_HORIZON_SECS);
        let purpose = purpose.trim();
        if purpose.is_empty() {
            return Err(VivianError::Other("唤醒目的不能为空".into()));
        }
        let now = crate::memory::types::current_timestamp();
        let mut data = self.data.write();
        // 去重：同 purpose 未到期的任务刷新到期时间，避免堆积
        if let Some(existing) = data
            .items
            .iter_mut()
            .find(|w| w.purpose == purpose && w.due_at > now)
        {
            existing.due_at = now + in_seconds;
            let id = existing.id.clone();
            drop(data);
            self.persist()?;
            return Ok(id);
        }
        data.next_seq += 1;
        let id = format!("wakeup-{}-{}", self.char_id, data.next_seq);
        data.items.push(ScheduledWakeup {
            id: id.clone(),
            due_at: now + in_seconds,
            purpose: purpose.to_string(),
            created_at: now,
        });
        drop(data);
        self.persist()?;
        Ok(id)
    }

    /// 取出到期的唤醒任务（并从队列移除）
    pub fn drain_due(&self, now: f64) -> Vec<ScheduledWakeup> {
        let mut data = self.data.write();
        let (due, pending): (Vec<_>, Vec<_>) = data
            .items
            .drain(..)
            .partition(|w| w.due_at <= now);
        data.items = pending;
        if !due.is_empty() {
            if let Err(e) = self.persist() {
                tracing::warn!("[WakeupScheduler] 持久化失败: {e}");
            }
        }
        due
    }

    /// 待处理任务数（UI/调试用）
    pub fn pending_count(&self) -> usize {
        self.data.read().items.len()
    }

    /// 清空全部唤醒
    pub fn clear(&self) {
        self.data.write().items.clear();
        let _ = self.persist();
    }

    fn persist(&self) -> VivianResult<()> {
        let json = serde_json::to_string(&*self.data.read())
            .map_err(|e| VivianError::Memory(format!("序列化唤醒队列失败: {e}")))?;
        std::fs::write(&self.path, json)
            .map_err(|e| VivianError::Memory(format!("写入唤醒队列失败: {e}")))
    }
}

/// 获取（或创建）角色的唤醒调度器
pub fn get_scheduler(char_id: &str) -> Arc<WakeupScheduler> {
    let mut map = WAKEUP_SCHEDULERS.write();
    map.entry(char_id.to_string())
        .or_insert_with(|| Arc::new(WakeupScheduler::new(char_id)))
        .clone()
}
