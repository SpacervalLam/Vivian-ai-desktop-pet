//! work 侧值守 Loop：把「过一会儿再来一次」做成可持久、可停用、有配额的条目。
//!
//! 静默成本为零：tick 只比较时间戳，到期才唤醒一次主会话。唤醒走普通消息入口，
//! 因此权限、工具面、上下文压缩都与用户手动发消息完全一致，不需要另一套执行通道。
//!
//! 只在应用运行时值守；客户端退出后不保留后台进程。

use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 最短周期：低于一分钟就不叫值守，而是轮询。
pub const MIN_CADENCE_SECS: u64 = 60;
/// 值守条目上限：数量本身就是一种配额，避免把会话变成任务农场。
pub const MAX_LOOPS: usize = 32;
/// tick 间隔（秒）。到期判断本身不花钱。
pub const TICK_SECS: u64 = 30;
/// 上一轮还没跑完时的重试间隔（秒）。
const RETRY_DEFER_SECS: i64 = 120;
const MAX_TITLE_CHARS: usize = 40;
const MAX_PROMPT_CHARS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkLoop {
    pub id: String,
    pub session_id: String,
    pub title: String,
    /// 唤醒时交给模型这一轮要做什么。
    pub prompt: String,
    pub cadence_secs: u64,
    pub next_run_at: i64,
    /// 0 表示不限次数。
    #[serde(default)]
    pub max_runs: u32,
    #[serde(default)]
    pub runs: u32,
    /// 到这个时刻之后不再运行（Unix 秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 同一会话内的幂等键：相同键再次创建时更新原条目而不是新增。
    #[serde(default)]
    pub dedup_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
}

fn default_enabled() -> bool {
    true
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// 当前 Unix 秒（命令面与工具用它算「还有多久」）。
pub fn now_secs() -> i64 {
    unix_now()
}

impl WorkLoop {
    /// 配额是否还允许再跑一次。
    pub fn within_limits(&self, now: i64) -> bool {
        let runs_left = self.max_runs == 0 || self.runs < self.max_runs;
        runs_left && self.ends_at.is_none_or(|ends_at| now < ends_at)
    }

    pub fn is_due(&self, now: i64) -> bool {
        self.enabled && self.within_limits(now) && now >= self.next_run_at
    }

    /// 给用户看的一行摘要。
    pub fn cadence_label(&self) -> String {
        if self.cadence_secs % 3600 == 0 {
            format!("每 {} 小时", self.cadence_secs / 3600)
        } else {
            format!("每 {} 分钟", (self.cadence_secs / 60).max(1))
        }
    }

    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "session_id": self.session_id,
            "title": self.title,
            "prompt": self.prompt,
            "cadence_secs": self.cadence_secs,
            "cadence_label": self.cadence_label(),
            "next_run_at": self.next_run_at,
            "max_runs": self.max_runs,
            "runs": self.runs,
            "ends_at": self.ends_at,
            "enabled": self.enabled,
            "dedup_key": self.dedup_key,
            "last_run_at": self.last_run_at,
            "last_status": self.last_status,
        })
    }
}

/// 唤醒消息：明确告诉模型这是自己安排的值守，不是用户新说的话。
pub fn wake_text(loop_: &WorkLoop) -> String {
    format!(
        "[值守触发] 这是你自己先前安排的定时值守「{}」（{}）。\n值守内容：{}\n\
         只完成这一轮该做的事；确实没有需要处理的情况，就一句话说明并结束，不要为凑输出扩大范围。",
        loop_.title,
        loop_.cadence_label(),
        loop_.prompt
    )
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LoopSnapshot {
    #[serde(default)]
    next_id: u32,
    #[serde(default)]
    loops: Vec<WorkLoop>,
}

pub struct WorkLoopStore {
    path: Option<PathBuf>,
    snapshot: RwLock<LoopSnapshot>,
}

impl WorkLoopStore {
    pub fn new() -> Self {
        Self { path: None, snapshot: RwLock::new(LoopSnapshot::default()) }
    }

    pub fn persistent(path: PathBuf) -> Self {
        let loaded: LoopSnapshot = crate::utils::fs::load_json_or_backup(&path).unwrap_or_default();
        Self { path: Some(path), snapshot: RwLock::new(loaded) }
    }

    fn save_locked(&self, snapshot: &LoopSnapshot) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| error.to_string())?;
        crate::utils::fs::atomic_write(path, &bytes).map_err(|error| error.to_string())
    }

    pub fn list(&self) -> Vec<WorkLoop> {
        self.snapshot.read().loops.clone()
    }

    pub fn list_for(&self, session_id: &str) -> Vec<WorkLoop> {
        self.snapshot
            .read()
            .loops
            .iter()
            .filter(|loop_| loop_.session_id == session_id)
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<WorkLoop> {
        self.snapshot.read().loops.iter().find(|loop_| loop_.id == id).cloned()
    }

    /// 新建或更新一条值守。
    ///
    /// 同一个会话里 `dedup_key` 相同的条目视为同一件事：再次创建只更新内容与周期，
    /// 不会产生第二条并行值守。`id` 为空表示「按 dedup_key 幂等新建」。
    pub fn upsert(
        &self,
        session_id: &str,
        id: Option<&str>,
        title: &str,
        prompt: &str,
        cadence_secs: u64,
        max_runs: u32,
        ends_at: Option<i64>,
        dedup_key: &str,
    ) -> Result<WorkLoop, String> {
        if session_id.trim().is_empty() {
            return Err("缺少会话".to_string());
        }
        let prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        if prompt.is_empty() {
            return Err("值守内容不能为空".to_string());
        }
        let prompt: String = prompt.chars().take(MAX_PROMPT_CHARS).collect();
        if cadence_secs < MIN_CADENCE_SECS {
            return Err(format!("值守周期不能短于 {MIN_CADENCE_SECS} 秒"));
        }
        let title = if title.trim().is_empty() {
            prompt.chars().take(MAX_TITLE_CHARS).collect()
        } else {
            title.trim().chars().take(MAX_TITLE_CHARS).collect()
        };
        let dedup_key = dedup_key.trim().to_string();
        let now = unix_now();
        if let Some(ends_at) = ends_at {
            if ends_at <= now {
                return Err("结束时间必须晚于当前时间".to_string());
            }
        }

        let mut snapshot = self.snapshot.write();
        let existing = id
            .and_then(|id| snapshot.loops.iter().position(|loop_| loop_.id == id))
            .or_else(|| {
                (!dedup_key.is_empty()).then(|| {
                    snapshot.loops.iter().position(|loop_| {
                        loop_.session_id == session_id && loop_.dedup_key == dedup_key
                    })
                })?
            });

        if let Some(index) = existing {
            let loop_ = &mut snapshot.loops[index];
            loop_.title = title;
            loop_.prompt = prompt;
            loop_.cadence_secs = cadence_secs;
            loop_.max_runs = max_runs;
            loop_.ends_at = ends_at;
            loop_.dedup_key = dedup_key;
            loop_.enabled = true;
            // 改周期后从「现在」重新计时，避免沿用旧排期立刻触发。
            loop_.next_run_at = now.saturating_add(cadence_secs as i64);
            let updated = loop_.clone();
            self.save_locked(&snapshot)?;
            return Ok(updated);
        }

        if snapshot.loops.len() >= MAX_LOOPS {
            return Err(format!("值守条目已达上限（{MAX_LOOPS} 条）"));
        }
        snapshot.next_id = snapshot.next_id.saturating_add(1);
        let loop_ = WorkLoop {
            id: format!("loop_{}", snapshot.next_id),
            session_id: session_id.to_string(),
            title,
            prompt,
            cadence_secs,
            next_run_at: now.saturating_add(cadence_secs as i64),
            max_runs,
            runs: 0,
            ends_at,
            enabled: true,
            dedup_key,
            last_run_at: None,
            last_status: None,
        };
        snapshot.loops.push(loop_.clone());
        self.save_locked(&snapshot)?;
        Ok(loop_)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let mut snapshot = self.snapshot.write();
        let loop_ = snapshot
            .loops
            .iter_mut()
            .find(|loop_| loop_.id == id)
            .ok_or_else(|| format!("找不到值守 {id}"))?;
        if enabled && !loop_.within_limits(unix_now()) {
            return Err("配额已用尽或已过结束时间，无法重新启用".to_string());
        }
        loop_.enabled = enabled;
        if enabled {
            loop_.next_run_at = unix_now().saturating_add(loop_.cadence_secs as i64);
        }
        self.save_locked(&snapshot)
    }

    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut snapshot = self.snapshot.write();
        let before = snapshot.loops.len();
        snapshot.loops.retain(|loop_| loop_.id != id);
        if snapshot.loops.len() == before {
            return Ok(false);
        }
        self.save_locked(&snapshot)?;
        Ok(true)
    }

    /// 到期的条目（只读，不改变状态）。
    pub fn due(&self, now: i64) -> Vec<WorkLoop> {
        self.snapshot
            .read()
            .loops
            .iter()
            .filter(|loop_| loop_.is_due(now))
            .cloned()
            .collect()
    }

    /// 记一次成功派发：次数 +1，排下一次；配额用完则自动停用。
    pub fn record_run(&self, id: &str, now: i64, status: &str) {
        let mut snapshot = self.snapshot.write();
        let Some(loop_) = snapshot.loops.iter_mut().find(|loop_| loop_.id == id) else { return };
        loop_.runs = loop_.runs.saturating_add(1);
        loop_.last_run_at = Some(now);
        loop_.last_status = Some(status.to_string());
        loop_.next_run_at = now.saturating_add(loop_.cadence_secs as i64);
        if !loop_.within_limits(now) {
            loop_.enabled = false;
            loop_.last_status = Some("quota_exhausted".to_string());
        }
        let _ = self.save_locked(&snapshot);
    }

    /// 本轮没派出去（会话还在跑）：顺延重试，不计次数。
    pub fn defer(&self, id: &str, now: i64) {
        let mut snapshot = self.snapshot.write();
        let Some(loop_) = snapshot.loops.iter_mut().find(|loop_| loop_.id == id) else { return };
        loop_.next_run_at = now.saturating_add(RETRY_DEFER_SECS);
        let _ = self.save_locked(&snapshot);
    }
}

impl Default for WorkLoopStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(test))]
static WORK_LOOPS: once_cell::sync::Lazy<std::sync::Arc<WorkLoopStore>> =
    once_cell::sync::Lazy::new(|| {
        std::sync::Arc::new(WorkLoopStore::persistent(
            crate::utils::path::get_user_data_dir().join("work_loops.json"),
        ))
    });

#[cfg(test)]
static WORK_LOOPS: once_cell::sync::Lazy<std::sync::Arc<WorkLoopStore>> =
    once_cell::sync::Lazy::new(|| std::sync::Arc::new(WorkLoopStore::new()));

pub fn global_work_loops() -> std::sync::Arc<WorkLoopStore> {
    std::sync::Arc::clone(&WORK_LOOPS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(store: &WorkLoopStore, cadence: u64, max_runs: u32, ends_at: Option<i64>) -> WorkLoop {
        store
            .upsert("s1", None, "巡检", "看看有没有新反馈", cadence, max_runs, ends_at, "")
            .unwrap()
    }

    #[test]
    fn loops_are_idempotent_by_dedup_key_within_a_session() {
        let store = WorkLoopStore::new();
        let first = store
            .upsert("s1", None, "巡检", "看看有没有新反馈", 300, 0, None, "watch:inbox")
            .unwrap();
        let again = store
            .upsert("s1", None, "巡检", "只看未读消息", 600, 0, None, "watch:inbox")
            .unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(again.cadence_secs, 600);
        assert_eq!(store.list().len(), 1);
        // 不同会话的同名键互不影响。
        store
            .upsert("s2", None, "巡检", "看看有没有新反馈", 300, 0, None, "watch:inbox")
            .unwrap();
        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn validation_rejects_bad_input_and_caps_quantity() {
        let store = WorkLoopStore::new();
        assert!(store.upsert("", None, "t", "p", 300, 0, None, "").is_err());
        assert!(store.upsert("s1", None, "t", "   ", 300, 0, None, "").is_err());
        assert!(store.upsert("s1", None, "t", "p", 10, 0, None, "").is_err());
        assert!(store.upsert("s1", None, "t", "p", 300, 0, Some(1), "").is_err());
        for index in 0..MAX_LOOPS {
            store
                .upsert("s1", None, "t", &format!("任务 {index}"), 300, 0, None, "")
                .unwrap();
        }
        assert!(store.upsert("s1", None, "t", "超额", 300, 0, None, "").is_err());
    }

    #[test]
    fn due_respects_schedule_quota_and_end_time() {
        let store = WorkLoopStore::new();
        let loop_ = create(&store, 300, 2, None);
        let start = loop_.next_run_at;
        assert!(!loop_.is_due(start - 1));
        assert!(store.due(start).len() == 1, "到点应当到期");

        store.record_run(&loop_.id, start, "dispatched");
        let after = store.get(&loop_.id).unwrap();
        assert_eq!(after.runs, 1);
        assert_eq!(after.next_run_at, start + 300);
        assert!(!after.is_due(start));
        assert!(after.is_due(start + 300));

        store.record_run(&loop_.id, start + 300, "dispatched");
        let exhausted = store.get(&loop_.id).unwrap();
        assert_eq!(exhausted.runs, 2);
        assert!(!exhausted.enabled, "配额用完自动停用");
        assert_eq!(exhausted.last_status.as_deref(), Some("quota_exhausted"));
        assert!(store.due(start + 10_000).is_empty());
        assert!(store.set_enabled(&loop_.id, true).is_err(), "配额用尽不可重新启用");
        assert!(store.set_enabled(&loop_.id, false).is_ok(), "停用是幂等的");
    }

    #[test]
    fn expired_loops_stop_and_deferral_keeps_the_run_count() {
        let store = WorkLoopStore::new();
        let now = unix_now();
        let loop_ = store
            .upsert("s1", None, "限时", "到点前做完", 60, 0, Some(now + 3600), "")
            .unwrap();
        assert!(loop_.is_due(now + 60));
        let expired = WorkLoop { ends_at: Some(now + 120), ..loop_.clone() };
        assert!(!expired.is_due(now + 200), "过结束时间不再到期");

        store.defer(&loop_.id, now);
        let deferred = store.get(&loop_.id).unwrap();
        assert_eq!(deferred.runs, 0, "顺延不算一次运行");
        assert!(deferred.next_run_at > now);
    }

    #[test]
    fn wake_text_marks_the_trigger_as_self_scheduled() {
        let store = WorkLoopStore::new();
        let loop_ = create(&store, 3600, 0, None);
        let text = wake_text(&loop_);
        assert!(text.contains("[值守触发]"));
        assert!(text.contains("每 1 小时"));
        assert!(text.contains("看看有没有新反馈"));
        assert_eq!(store.get(&loop_.id).unwrap().cadence_label(), "每 1 小时");
        assert!(store.remove(&loop_.id).unwrap());
        assert!(!store.remove(&loop_.id).unwrap());
    }
}
