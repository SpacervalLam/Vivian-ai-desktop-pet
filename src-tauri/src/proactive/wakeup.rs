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
    #[serde(default)]
    pub retry_after: f64,
    #[serde(default)]
    pub attempts: u32,
}

/// 单角色的唤醒队列（内存 + JSON 持久化）
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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
        let dir = crate::utils::path::get_companion_data_dir(char_id)
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
        if !in_seconds.is_finite() { return Err(VivianError::Other("唤醒时间必须为有限数值".into())); }
        let in_seconds = in_seconds.clamp(60.0, MAX_HORIZON_SECS);
        let purpose = purpose.trim();
        if purpose.is_empty() {
            return Err(VivianError::Other("唤醒目的不能为空".into()));
        }
        let now = crate::memory::types::current_timestamp();
        let mut data = self.data.write();
        let previous = data.clone();
        // 去重：同 purpose 未到期的任务刷新到期时间，避免堆积
        if let Some(existing) = data
            .items
            .iter_mut()
            .find(|w| w.purpose == purpose)
        {
            existing.due_at = now + in_seconds;
            existing.retry_after = 0.0;
            existing.attempts = 0;
            let id = existing.id.clone();
            if let Err(error) = self.persist_data(&data) { *data = previous; return Err(error); }
            return Ok(id);
        }
        if data.items.len() >= 32 { return Err(VivianError::Other("待兑现的唤醒任务已达32条，请先完成已有安排".into())); }
        data.next_seq = data.next_seq.saturating_add(1);
        let id = format!("wakeup-{}-{}", self.char_id, data.next_seq);
        data.items.push(ScheduledWakeup {
            id: id.clone(),
            due_at: now + in_seconds,
            purpose: purpose.to_string(),
            created_at: now,
            retry_after: 0.0,
            attempts: 0,
        });
        if let Err(error) = self.persist_data(&data) { *data = previous; return Err(error); }
        Ok(id)
    }

    /// Observe the earliest eligible promise without consuming it before delivery.
    pub fn next_due(&self, now: f64) -> Option<ScheduledWakeup> {
        self.data.read().items.iter().filter(|w| w.due_at <= now && w.retry_after <= now)
            .min_by(|a,b| a.due_at.total_cmp(&b.due_at)).cloned()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.data.read().items.iter().any(|w| w.id == id)
    }

    pub fn is_due(&self, id: &str, now: f64) -> bool {
        self.data.read().items.iter().any(|w| w.id == id && w.due_at <= now)
    }

    /// Failed generation or deferred delivery must not spin once every tick.
    pub fn defer(&self, id: &str, now: f64) {
        let mut data = self.data.write();
        if let Some(item) = data.items.iter_mut().find(|w| w.id == id) {
            item.attempts = item.attempts.saturating_add(1);
            item.retry_after = now + (60.0 * 2_f64.powi(item.attempts.min(6) as i32 - 1)).min(1800.0);
            if let Err(e) = self.persist_data(&data) { tracing::warn!("[WakeupScheduler] 持久化失败: {e}"); }
        }
    }

    pub fn acknowledge(&self, id: &str) {
        let mut data = self.data.write();
        data.items.retain(|w| w.id != id);
        if let Err(e) = self.persist_data(&data) { tracing::warn!("[WakeupScheduler] 持久化失败: {e}"); }
    }

    /// 待处理任务数（UI/调试用）
    pub fn pending_count(&self) -> usize {
        self.data.read().items.len()
    }

    /// 清空全部唤醒
    pub fn clear(&self) {
        let mut data = self.data.write();
        data.items.clear();
        let _ = self.persist_data(&data);
    }

    fn persist_data(&self, data: &WakeupData) -> VivianResult<()> {
        let json = serde_json::to_string(data)
            .map_err(|e| VivianError::Memory(format!("序列化唤醒队列失败: {e}")))?;
        crate::utils::fs::atomic_write(&self.path, json.as_bytes())
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

#[cfg(test)]
mod tests {
    use super::*;
    fn scheduler() -> WakeupScheduler {
        WakeupScheduler { char_id:"test".into(), path:std::env::temp_dir().join(format!("vivian-wakeup-{}.json", uuid::Uuid::new_v4())), data:RwLock::new(WakeupData::default()) }
    }
    #[test]
    fn due_promises_keep_identity_until_delivery_and_back_off_without_starving_others() {
        let scheduler = scheduler();
        let now = crate::memory::types::current_timestamp();
        let first = scheduler.schedule(60.0, "看看用户").unwrap();
        let second = scheduler.schedule(120.0, "分享发现").unwrap();
        assert_eq!(scheduler.next_due(now+121.0).unwrap().id, first);
        assert_eq!(scheduler.pending_count(), 2);
        scheduler.defer(&first, now+121.0);
        assert_eq!(scheduler.next_due(now+121.0).unwrap().id, second);
        scheduler.acknowledge(&second);
        assert!(scheduler.contains(&first));
        assert!(scheduler.next_due(now+150.0).is_none());
        assert_eq!(scheduler.next_due(now+182.0).unwrap().id, first);
        let saved: WakeupData = serde_json::from_slice(&std::fs::read(&scheduler.path).unwrap()).unwrap();
        assert_eq!(saved.items.len(),1);
        assert_eq!(saved.items[0].id,first);
        assert_eq!(saved.items[0].attempts,1);
        scheduler.clear();
        assert_eq!(scheduler.pending_count(),0);
        let _ = std::fs::remove_file(&scheduler.path);
    }
    #[test]
    fn scheduling_deduplicates_pending_promises_and_rejects_invalid_times() {
        let scheduler = scheduler();
        let id = scheduler.schedule(60.0,"回来看看").unwrap();
        assert_eq!(scheduler.schedule(180.0,"回来看看").unwrap(),id);
        assert_eq!(scheduler.pending_count(),1);
        assert!(scheduler.schedule(f64::NAN,"invalid").is_err());
        assert!(scheduler.schedule(f64::INFINITY,"invalid").is_err());
        assert!(scheduler.schedule(60.0," ").is_err());
        let _ = std::fs::remove_file(&scheduler.path);
    }

    #[test]
    fn failed_persistence_does_not_leave_an_unacknowledged_schedule_in_memory() {
        let mut scheduler = scheduler();
        // An ordinary file cannot be used as the schedule's parent directory.
        std::fs::write(&scheduler.path,b"blocked").unwrap();
        let blocker = scheduler.path.clone();
        scheduler.path = blocker.join("wakeups.json");
        assert!(scheduler.schedule(60.0,"should not run").is_err());
        assert_eq!(scheduler.pending_count(),0);
        let _ = std::fs::remove_file(blocker);
    }
}
