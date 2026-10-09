//! 定时任务调度器。
//!
//! - 支持 REMINDER / TOOL_CALL 两种任务类型
//! - 任务状态机：Pending / Running / Completed / Cancelled / Failed
//! - 优先级队列（按 scheduled_time 排序）
//! - tokio::spawn 异步执行
//! - ISO 8601 时间解析（datetime + duration）
//! - 持久化（可选，简化版：内存 + JSON 文件）

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::utils::path::get_user_data_dir;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// 已完成/取消/失败任务的保留期（秒）
///
/// 超过此保留期的终态任务会在 tick() 末尾被 drain，避免单次运行期内持续累积。
const SCHEDULER_COMPLETED_RETENTION_SECS: u64 = 30 * 60;

/// 任务类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    /// 消息提醒
    Reminder,
    /// 工具调用
    ToolCall,
}

impl Default for TaskType {
    fn default() -> Self {
        Self::Reminder
    }
}

/// 任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 等待执行
    Pending,
    /// 执行中
    Running,
    /// 已完成
    Completed,
    /// 已取消
    Cancelled,
    /// 执行失败
    Failed,
    /// 已暂停（可恢复）
    Paused,
}

impl Default for TaskStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// 优先级（数字越大越优先）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low = 0,
    Normal = 1,
    High = 2,
    Urgent = 3,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Normal
    }
}

/// 定时任务数据模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// 任务 ID（8 字符）
    pub id: String,
    /// 任务类型
    #[serde(default)]
    pub task_type: TaskType,
    /// 计划执行时间戳（Unix 秒）
    pub scheduled_time: f64,
    /// 提醒消息（REMINDER 类型使用）
    #[serde(default)]
    pub message: Option<String>,
    /// 工具名称（TOOL_CALL 类型使用）
    #[serde(default)]
    pub tool_name: Option<String>,
    /// 工具参数
    #[serde(default)]
    pub tool_arguments: serde_json::Value,
    /// 重复间隔（秒），None = 单次
    #[serde(default)]
    pub repeat_interval: Option<u64>,
    /// 任务状态
    #[serde(default)]
    pub status: TaskStatus,
    /// 创建时间戳
    pub created_at: f64,
    /// 优先级
    #[serde(default)]
    pub priority: Priority,
    /// 元数据
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// 完成/取消/失败时间戳（Unix 秒）
    ///
    /// 仅在 status 转为 Completed/Cancelled/Failed 时设置；
    /// 用于 tick() 末尾的滚动清理，避免已完成任务永驻内存。
    #[serde(default)]
    pub completed_at: Option<f64>,
    /// 创建该任务的角色 ID
    ///
    /// 任务触发执行工具时，用此 char_id 构造 ToolUseContext，
    /// 使工具调用能路由回正确的角色窗口并正常使用记忆等依赖 char_id 的能力。
    /// 旧任务文件无此字段时默认为空串。
    #[serde(default)]
    pub char_id: String,
    /// 是否已发起"预触发"LLM 调用
    ///
    /// 在 `scheduled_time - 5s` 到 `scheduled_time` 之间发起一次主 LLM 调用，
    /// 把定时任务内容说明作为 user_input 注入完整提示词，
    /// 让 LLM 提前决定如何进行（回复/工具调用/提醒用户）。
    /// 此字段防止每秒 tick 重复发起预触发。
    #[serde(default)]
    pub pre_triggered: bool,
    #[serde(default)]
    pub delivery: super::reminder_delivery::DeliveryState,
    #[serde(default)]
    pub notices: Vec<super::reminder_delivery::ReminderNotice>,
}

impl ScheduledTask {
    /// 生成 8 字符随机 ID（取 uuid 前 8 字符）。
    fn generate_id() -> String {
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("00000000")
            .to_string()
    }

    pub fn new_reminder(message: impl Into<String>, scheduled_time: f64) -> Self {
        Self {
            id: Self::generate_id(),
            task_type: TaskType::Reminder,
            scheduled_time,
            message: Some(message.into()),
            tool_name: None,
            tool_arguments: serde_json::Value::Null,
            repeat_interval: None,
            status: TaskStatus::Pending,
            created_at: now_ts(),
            priority: Priority::Normal,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
            completed_at: None,
            char_id: String::new(),
            pre_triggered: false,
            delivery: Default::default(),
            notices: Vec::new(),
        }
    }

    pub fn new_tool_call(
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
        scheduled_time: f64,
    ) -> Self {
        Self {
            id: Self::generate_id(),
            task_type: TaskType::ToolCall,
            scheduled_time,
            message: None,
            tool_name: Some(tool_name.into()),
            tool_arguments: arguments,
            repeat_interval: None,
            status: TaskStatus::Pending,
            created_at: now_ts(),
            priority: Priority::Normal,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
            completed_at: None,
            char_id: String::new(),
            pre_triggered: false,
            delivery: Default::default(),
            notices: Vec::new(),
        }
    }

    /// 剩余时间（秒）。
    pub fn remaining_seconds(&self) -> f64 {
        self.scheduled_time - now_ts()
    }

    /// 可读的剩余时间字符串。
    pub fn remaining_str(&self) -> String {
        let remaining = self.remaining_seconds();
        if remaining <= 0.0 {
            return "即将执行".to_string();
        }
        let total = remaining as u64;
        let hours = total / 3600;
        let minutes = (total % 3600) / 60;
        let seconds = total % 60;
        if hours > 0 {
            format!("{}小时{}分钟{}秒", hours, minutes, seconds)
        } else if minutes > 0 {
            format!("{}分钟{}秒", minutes, seconds)
        } else {
            format!("{}秒", seconds)
        }
    }
}

/// 调度器回调函数类型。
pub type SchedulerCallback = Arc<dyn Fn(ScheduledTask) + Send + Sync>;

/// 定时任务调度器。
///
/// 内存优先级队列 + tokio::spawn 异步执行 +
pub struct Scheduler {
    inner: Arc<Mutex<SchedulerInner>>,
    /// 持久化文件路径（None = 不持久化）。
    persistence_path: Option<PathBuf>,
    persistence_gate: Arc<Mutex<()>>,
    shutdown: Arc<tokio::sync::Notify>,
    heartbeat: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    dispatch_gate: Mutex<Option<Arc<dyn Fn(&ScheduledTask) -> bool + Send + Sync>>>,
}

struct SchedulerInner {
    tasks: HashMap<String, ScheduledTask>,
    callback: Option<SchedulerCallback>,
}

impl Scheduler {
    /// 创建调度器。`persist=true` 时持久化到用户数据目录。
    pub fn new(persist: bool) -> Self {
        let persistence_path = if persist {
            Some(get_user_data_dir().join("scheduled_tasks.json"))
        } else {
            None
        };
        Self::with_persistence_path(persistence_path)
    }

    fn with_persistence_path(persistence_path: Option<PathBuf>) -> Self {
        let mut scheduler = Self {
            inner: Arc::new(Mutex::new(SchedulerInner {
                tasks: HashMap::new(),
                callback: None,
            })),
            persistence_path,
            persistence_gate: Arc::new(Mutex::new(())),
            shutdown: Arc::new(tokio::sync::Notify::new()),
            heartbeat: Mutex::new(None),
            dispatch_gate: Mutex::new(None),
        };
        if let Some(path) = scheduler.persistence_path.clone() {
            scheduler.load_tasks_from(&path);
        }
        scheduler
    }

    /// Register dispatch; completion must be reported with complete_execution.
    pub fn set_callback(&self, callback: SchedulerCallback) {
        self.inner.lock().callback = Some(callback);
    }

    /// 调度一个提醒任务，返回任务 ID。
    pub fn schedule_reminder(&self, message: impl Into<String>, scheduled_time: f64) -> String {
        let task = ScheduledTask::new_reminder(message, scheduled_time);
        let id = task.id.clone();
        self.insert_task(task);
        id
    }

    /// 调度一个工具调用任务，返回任务 ID。
    pub fn schedule_tool_call(
        &self,
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
        scheduled_time: f64,
    ) -> String {
        let task = ScheduledTask::new_tool_call(tool_name, arguments, scheduled_time);
        let id = task.id.clone();
        self.insert_task(task);
        id
    }

    /// 调度一个重复任务，返回任务 ID。
    pub fn schedule_repeat(&self, task: ScheduledTask, interval_seconds: u64) -> String {
        let mut task = task;
        task.repeat_interval = Some(interval_seconds);
        let id = task.id.clone();
        self.insert_task(task);
        id
    }

    /// 更新任务的计划执行时间（仅 Pending 状态可更新）。
    pub fn update_task_scheduled_time(&self, task_id: &str, new_time: f64) -> bool {
        let mut inner = self.inner.lock();
        if let Some(task) = inner.tasks.get_mut(task_id) {
            if task.status == TaskStatus::Pending {
                task.scheduled_time = new_time;
                task.delivery.next_attempt_at = None;
                drop(inner);
                self.persist();
                tracing::info!(task_id, new_time, "[Scheduler] 任务时间已更新");
                return true;
            }
        }
        false
    }

    /// 移除任务（完全删除，区别于 cancel）。
    pub fn remove_task(&self, task_id: &str) -> bool {
        let mut inner = self.inner.lock();
        let removed = inner.tasks.remove(task_id).is_some();
        drop(inner);
        if removed {
            self.persist();
            tracing::info!(task_id, "[Scheduler] 任务已移除");
        }
        removed
    }

    fn insert_task(&self, task: ScheduledTask) {
        tracing::info!(
            task_id = %task.id,
            task_type = ?task.task_type,
            scheduled_at = task.scheduled_time,
            "[Scheduler] 任务已创建"
        );
        self.inner.lock().tasks.insert(task.id.clone(), task);
        self.persist();
    }

    /// 插入任务（公开接口，供工具层直接插入已构造的 ScheduledTask）。
    pub fn insert_task_public(&self, task: ScheduledTask) {
        self.insert_task(task);
    }

    /// 取消任务。
    pub fn cancel_task(&self, task_id: &str) -> bool {
        let mut inner = self.inner.lock();
        if let Some(task) = inner.tasks.get_mut(task_id) {
            task.status = TaskStatus::Cancelled;
            task.completed_at = Some(now_ts());
            drop(inner);
            self.persist();
            tracing::info!(task_id, "[Scheduler] 任务已取消");
            true
        } else {
            false
        }
    }

    /// 暂停任务（仅 Pending 状态可暂停）。
    pub fn pause_task(&self, task_id: &str) -> bool {
        let mut inner = self.inner.lock();
        if let Some(task) = inner.tasks.get_mut(task_id) {
            if task.status == TaskStatus::Pending {
                task.status = TaskStatus::Paused;
                drop(inner);
                self.persist();
                tracing::info!(task_id, "[Scheduler] 任务已暂停");
                return true;
            }
        }
        false
    }

    /// 恢复任务（仅 Paused 状态可恢复）。
    ///
    /// 若任务已过期，自动顺延到当前时间之后 60 秒，避免恢复即立即触发。
    pub fn resume_task(&self, task_id: &str) -> bool {
        let mut inner = self.inner.lock();
        if let Some(task) = inner.tasks.get_mut(task_id) {
            if task.status == TaskStatus::Paused {
                task.status = TaskStatus::Pending;
                let now = now_ts();
                if task.scheduled_time <= now {
                    task.scheduled_time = now + 60.0;
                    tracing::info!(
                        task_id,
                        new_time = task.scheduled_time,
                        "[Scheduler] 恢复时已顺延过期任务"
                    );
                }
                drop(inner);
                self.persist();
                tracing::info!(task_id, "[Scheduler] 任务已恢复");
                return true;
            }
        }
        false
    }

    /// 获取任务。
    pub fn get_task(&self, task_id: &str) -> Option<ScheduledTask> {
        self.inner.lock().tasks.get(task_id).cloned()
    }

    /// 列出所有任务（按计划时间升序）。
    pub fn list_tasks(&self) -> Vec<ScheduledTask> {
        let mut tasks: Vec<ScheduledTask> = self.inner.lock().tasks.values().cloned().collect();
        tasks.sort_by(|a, b| {
            a.scheduled_time
                .partial_cmp(&b.scheduled_time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        tasks
    }

    /// 列出待执行任务（按优先级 + 计划时间排序）。
    ///
    /// 对齐任务描述"优先级队列"。
    pub fn list_pending(&self) -> Vec<ScheduledTask> {
        let mut tasks: Vec<ScheduledTask> = self
            .inner
            .lock()
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Pending)
            .cloned()
            .collect();
        // 先按优先级降序，再按计划时间升序
        tasks.sort_by(|a, b| {
            b.priority.cmp(&a.priority).then_with(|| {
                a.scheduled_time
                    .partial_cmp(&b.scheduled_time)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        tasks
    }

    pub fn set_heartbeat(&self, callback: Arc<dyn Fn() + Send + Sync>) { *self.heartbeat.lock() = Some(callback); }
    pub fn set_dispatch_gate(&self, callback: Arc<dyn Fn(&ScheduledTask) -> bool + Send + Sync>) { *self.dispatch_gate.lock() = Some(callback); }

    fn update_persisted<T>(&self, update: impl FnOnce(&mut HashMap<String, ScheduledTask>) -> Result<T, String>) -> Result<T, String> {
        let _write = self.persistence_gate.lock();
        let mut inner = self.inner.lock();
        let previous = inner.tasks.clone();
        let result = match update(&mut inner.tasks) {
            Ok(result) => result,
            Err(error) => { inner.tasks = previous; return Err(error); }
        };
        if let Some(path) = &self.persistence_path {
            if let Err(error) = save_tasks_to(path, &inner.tasks) {
                inner.tasks = previous;
                return Err(error.to_string());
            }
        }
        Ok(result)
    }

    pub fn record_reminder_notice(&self, task: &ScheduledTask, character_id: String, content: String, delivery_id: String)
        -> Result<super::reminder_delivery::ReminderNotice, String> {
        self.update_persisted(|tasks| {
            let stored = tasks.get_mut(&task.id).ok_or("Reminder no longer exists")?;
            if stored.status != TaskStatus::Running || stored.delivery.attempts != task.delivery.attempts {
                return Err("Stale reminder attempt".into());
            }
            if let Some(notice) = stored.notices.iter_mut().find(|n| n.scheduled_time == task.scheduled_time) {
                notice.delivery_id = delivery_id;
                return Ok(notice.clone());
            }
            // Discard old acknowledged occurrences, never unread ones.
            stored.notices.retain(|n| n.acknowledged_at.is_none());
            let notice = super::reminder_delivery::ReminderNotice {
                id: uuid::Uuid::new_v4().to_string(), task_id: task.id.clone(),
                scheduled_time: task.scheduled_time, character_id, content, delivery_id,
                important: task.priority == Priority::Urgent, created_at: now_ts(),
                acknowledged_at: None, snoozed_task_id: None,
            };
            stored.notices.push(notice.clone());
            Ok(notice)
        })
    }

    pub fn pending_reminder_notices(&self) -> Vec<super::reminder_delivery::ReminderNotice> {
        let mut notices: Vec<_> = self.inner.lock().tasks.values().flat_map(|t| t.notices.iter())
            .filter(|n| n.acknowledged_at.is_none()).cloned().collect();
        notices.sort_by(|a,b| a.created_at.total_cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        notices
    }

    pub fn acknowledge_reminder_notice(&self, notice_id: &str, snooze: bool) -> Result<Option<String>, String> {
        self.update_persisted(|tasks| {
            let task = tasks.values_mut().find(|t| t.notices.iter().any(|n| n.id == notice_id))
                .ok_or("Reminder notice no longer exists")?;
            let mut next = ScheduledTask::new_reminder(task.message.clone().unwrap_or_default(), now_ts() + 300.0);
            next.char_id = task.char_id.clone();
            next.priority = task.priority;
            let notice = task.notices.iter_mut().find(|n| n.id == notice_id).unwrap();
            if notice.acknowledged_at.is_some() { return Ok(notice.snoozed_task_id.clone()); }
            next.char_id = notice.character_id.clone();
            notice.acknowledged_at = Some(now_ts());
            let id = if snooze { notice.snoozed_task_id = Some(next.id.clone()); Some(next.id.clone()) } else { None };
            if snooze { tasks.insert(next.id.clone(), next); }
            Ok(id)
        })
    }

    pub fn insert_reminder_checked(&self, task: ScheduledTask) -> Result<String, String> {
        let id = task.id.clone();
        self.update_persisted(|tasks| { tasks.insert(id.clone(), task); Ok(id) })
    }

    /// 启动后台调度循环（每秒检查一次）。
    ///
    /// 返回一个 `JoinHandle`，调用方可用于取消。
    pub async fn run(self: Arc<Self>) {
        tracing::info!("[Scheduler] 调度循环已启动");
        crate::utils::watchdog::register("scheduler", 1.0, None);
        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    tracing::info!("[Scheduler] 调度循环已停止");
                    crate::utils::watchdog::unregister("scheduler");
                    return;
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    crate::utils::watchdog::beat("scheduler");
                    let heartbeat = self.heartbeat.lock().clone();
                    if let Some(heartbeat) = heartbeat { heartbeat(); }
                    self.tick().await;
                }
            }
        }
    }

    /// 单次 tick：检查所有到期任务并触发回调。
    ///
    /// 集成 InterruptionController：执行前检查是否适合打扰，
    /// 避免在用户专注/静默期触发提醒。
    pub async fn tick(&self) {
        // Reminders are delivered only at their due time; preparation is not delivery.
        let now = now_ts();
        let mut due_tasks: Vec<ScheduledTask> = {
            let inner = self.inner.lock();
            inner
                .tasks
                .values()
                .filter(|t| {
                    t.status == TaskStatus::Pending
                        && t.scheduled_time <= now
                        && t.delivery
                            .next_attempt_at
                            .map_or(true, |retry| retry <= now)
                })
                .cloned()
                .collect()
        };
        due_tasks.sort_by(|a, b| b.priority.cmp(&a.priority)
            .then_with(|| a.scheduled_time.total_cmp(&b.scheduled_time))
            .then_with(|| a.id.cmp(&b.id)));

        for task in due_tasks {
            let gate = self.dispatch_gate.lock().clone();
            if gate.is_some_and(|gate| !gate(&task)) { continue; }
            // 打扰检查：将任务优先级映射到 InterruptPriority
            let interrupt_priority = match task.priority {
                Priority::Urgent => {
                    crate::brain::interruption_controller::InterruptPriority::Urgent
                }
                Priority::High => crate::brain::interruption_controller::InterruptPriority::High,
                Priority::Low | Priority::Normal => {
                    crate::brain::interruption_controller::InterruptPriority::Normal
                }
            };
            if let Some(ctrl) =
                crate::brain::interruption_controller::try_get_interruption_controller()
            {
                let decision = ctrl.should_interrupt(interrupt_priority);
                if !decision.allowed {
                    tracing::debug!(
                        "[Scheduler] 任务 {} 因打扰控制被跳过: {}",
                        task.id,
                        decision.reason
                    );
                    continue;
                }
            }
            let callback = self.inner.lock().callback.clone();
            let Some(cb) = callback else {
                continue;
            };
            let reserved = {
                let mut inner = self.inner.lock();
                match inner.tasks.get_mut(&task.id) {
                    Some(t) if t.status == TaskStatus::Pending
                        && t.scheduled_time <= now
                        && t.delivery.next_attempt_at.map_or(true, |retry| retry <= now) => {
                        t.status = TaskStatus::Running;
                        t.delivery.attempts = t.delivery.attempts.saturating_add(1);
                        Some(t.clone())
                    }
                    _ => None,
                }
            };
            if let Some(task) = reserved {
                // Persist the reservation before allowing an external side effect.
                if !self.persist() {
                    let mut inner = self.inner.lock();
                    if let Some(t) = inner.tasks.get_mut(&task.id) {
                        if t.status == TaskStatus::Running {
                            t.status = TaskStatus::Pending;
                            t.delivery.attempts = t.delivery.attempts.saturating_sub(1);
                        }
                    }
                    continue;
                }
                tokio::spawn(async move {
                    cb(task);
                });
            }
        }

        // tick 末尾滚动清理：drain 已完成且超过 SCHEDULER_COMPLETED_RETENTION_SECS 的任务
        // 避免单次运行期内 Completed/Cancelled/Failed 任务持续累积导致内存增长
        self.cleanup_terminal_tasks();
    }

    /// Callback completion is distinct from dispatch. Failures never count as reminders.
    pub fn complete_execution(&self, task_id: &str, result: Result<String, String>) {
        let Some(attempt) = self.get_task(task_id).map(|task|task.delivery.attempts) else {return;};
        self.complete_attempt(task_id,attempt,result);
    }

    /// A receipt belongs to one reservation; delayed receipts cannot complete a later repetition.
    pub fn complete_attempt(&self, task_id: &str, attempt: u32, result: Result<String, String>) {
        let now = now_ts();
        {
            let mut inner = self.inner.lock();
            let Some(task) = inner.tasks.get_mut(task_id) else {
                return;
            };
            if task.status != TaskStatus::Running || task.delivery.attempts != attempt {
                return;
            }
            match result {
                Ok(text) => {
                    if task.task_type == TaskType::Reminder {
                        task.delivery.confirmed(now, text);
                    }
                    if let Some(interval) = task.repeat_interval.filter(|i| *i > 0) {
                        let step = interval as f64;
                        let skipped = ((now - task.scheduled_time) / step).floor().max(0.0) + 1.0;
                        task.scheduled_time += skipped * step;
                        task.status = TaskStatus::Pending;
                        task.pre_triggered = false;
                    } else {
                        task.status = TaskStatus::Completed;
                        task.completed_at = Some(now);
                    }
                }
                Err(error) => {
                    tracing::warn!(task_id, error, "[scheduler] execution/delivery failed");
                    if task.task_type == TaskType::Reminder {
                        task.delivery.failed(now);
                        task.status = TaskStatus::Pending;
                    } else {
                        // Never automatically repeat an operation with possible side effects.
                        task.status = TaskStatus::Failed;
                        task.completed_at = Some(now);
                    }
                }
            }
        }
        self.persist();
    }

    /// 清理已完成/取消/失败且超过保留期的任务
    ///
    /// 保留期内的终态任务仍保留在内存中（供前端历史查看），超过后从 tasks 移除。
    fn cleanup_terminal_tasks(&self) {
        let now = now_ts();
        let retention = SCHEDULER_COMPLETED_RETENTION_SECS as f64;
        let mut inner = self.inner.lock();
        let before = inner.tasks.len();
        inner.tasks.retain(|_, t| {
            if t.notices.iter().any(|n| n.acknowledged_at.is_none()) { return true; }
            match t.status {
                TaskStatus::Completed | TaskStatus::Cancelled | TaskStatus::Failed => {
                    // 有 completed_at 时按保留期判断；无 completed_at（旧数据）也清理
                    t.completed_at
                        .map(|ts| now - ts < retention)
                        .unwrap_or(false)
                }
                _ => true, // Pending/Running/Paused 保留
            }
        });
        let removed = before - inner.tasks.len();
        if removed > 0 {
            tracing::debug!(
                "[Scheduler] 清理 {} 个已完成/取消/失败任务（保留期={}s）",
                removed,
                SCHEDULER_COMPLETED_RETENTION_SECS
            );
            drop(inner);
            self.persist();
        }
    }

    /// 关闭调度器。
    pub fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// 清空所有定时任务（内存 + 持久化文件）
    /// 供 factory_reset 调用：清空内存 HashMap 并删除磁盘文件，避免重启后任务"复活"
    pub fn clear_all_tasks(&self) {
        let _write = self.persistence_gate.lock();
        {
            let mut inner = self.inner.lock();
            inner.tasks.clear();
        }
        if let Some(path) = &self.persistence_path {
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }
        tracing::info!("[scheduler] 已清空所有定时任务（factory_reset）");
    }

    fn persist(&self) -> bool {
        if let Some(path) = &self.persistence_path {
            let _write = self.persistence_gate.lock();
            let tasks = self.inner.lock().tasks.clone();
            if let Err(e) = save_tasks_to(path, &tasks) {
                tracing::warn!(error = %e, "[scheduler] 定时任务保存失败，重启后可能丢失已设置的提醒");
                return false;
            }
        }
        true
    }

    fn load_tasks_from(&mut self, path: &PathBuf) {
        #[derive(serde::Deserialize)]
        struct Persisted {
            tasks: Vec<ScheduledTask>,
        }
        if let Some(data) = crate::utils::fs::load_json_or_backup::<Persisted>(path) {
                let mut inner = self.inner.lock();
                for mut task in data.tasks {
                    if task.status == TaskStatus::Running {
                        if task.task_type == TaskType::Reminder {
                            task.status = TaskStatus::Pending;
                        } else {
                            task.status = TaskStatus::Failed;
                            task.completed_at = Some(now_ts());
                            if !task.metadata.is_object() { task.metadata = serde_json::json!({}); }
                            task.metadata["recovery_error"] = serde_json::json!(
                                "Execution was interrupted; its outcome is unknown. Review the result before retrying."
                            );
                        }
                    }
                    if matches!(task.status, TaskStatus::Pending | TaskStatus::Paused)
                        || task.notices.iter().any(|n| n.acknowledged_at.is_none())
                        || task.completed_at.is_some_and(|at| now_ts() - at < SCHEDULER_COMPLETED_RETENTION_SECS as f64) {
                        inner.tasks.insert(task.id.clone(), task);
                    }
                }
                tracing::info!(loaded = inner.tasks.len(), "[Scheduler] 已加载持久化任务");
                drop(inner);
                self.persist();
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new(true)
    }
}

// ── 时间解析 ──

/// 解析时间规格，返回 Unix 时间戳。
///
/// - ISO 8601 日期时间: "2024-01-15T10:30:00", "2024-01-15T10:30"
/// - 纯日期: "2024-01-15"（默认当日 09:00）
/// - ISO 8601 持续时间: "PT2M", "PT2H30M", "P1DT2H", "PT30S"
pub fn parse_time_spec(time_spec: &str) -> Result<f64, String> {
    let trimmed = time_spec.trim();

    // 1. 尝试 ISO 8601 日期时间（含具体时间）
    let formats = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"];
    for fmt in formats {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(trimmed, fmt) {
            let local = chrono::Local.from_local_datetime(&dt).unwrap();
            return Ok(local.timestamp() as f64);
        }
    }

    // 2. 尝试纯日期 YYYY-MM-DD（默认当日 09:00）
    if let Ok(date) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        let dt = date.and_hms_opt(9, 0, 0).unwrap();
        let local = chrono::Local.from_local_datetime(&dt).unwrap();
        return Ok(local.timestamp() as f64);
    }

    // 3. 尝试 ISO 8601 持续时间
    if let Ok(duration) = parse_iso_duration(trimmed) {
        let now = chrono::Local::now();
        return Ok((now + duration).timestamp() as f64);
    }

    Err(format!(
        "不支持的时间格式: {}。请使用 ISO 8601，例如 PT2M（2分钟后）、2024-01-15T10:30:00 或 2024-01-15",
        time_spec
    ))
}

/// 解析 ISO 8601 持续时间 (PnYnMnDTnHnMnS)。
fn parse_iso_duration(spec: &str) -> Result<chrono::Duration, String> {
    use once_cell::sync::Lazy;
    use regex::Regex;

    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$")
            .unwrap()
    });

    let caps = RE
        .captures(spec)
        .ok_or_else(|| format!("无效的 ISO 8601 持续时间: {}", spec))?;

    let years = caps
        .get(1)
        .map(|m| m.as_str().parse::<u64>().unwrap_or(0))
        .unwrap_or(0);
    let months = caps
        .get(2)
        .map(|m| m.as_str().parse::<u64>().unwrap_or(0))
        .unwrap_or(0);
    let days = caps
        .get(3)
        .map(|m| m.as_str().parse::<u64>().unwrap_or(0))
        .unwrap_or(0);
    let hours = caps
        .get(4)
        .map(|m| m.as_str().parse::<u64>().unwrap_or(0))
        .unwrap_or(0);
    let minutes = caps
        .get(5)
        .map(|m| m.as_str().parse::<u64>().unwrap_or(0))
        .unwrap_or(0);
    let seconds = caps
        .get(6)
        .map(|m| m.as_str().parse::<u64>().unwrap_or(0))
        .unwrap_or(0);

    let total_days = days + years * 365 + months * 30;
    let total_seconds = total_days * 86400 + hours * 3600 + minutes * 60 + seconds;
    Ok(chrono::Duration::seconds(total_seconds as i64))
}

// ── 工具函数 ──

/// 当前 Unix 时间戳（秒）。公开供工具层使用。
pub fn now_ts_public() -> f64 {
    now_ts()
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn save_tasks_to(path: &PathBuf, tasks: &HashMap<String, ScheduledTask>) -> Result<(), String> {
    let tasks_vec: Vec<&ScheduledTask> = tasks.values().collect();
    let data = serde_json::json!({
        "tasks": tasks_vec,
        "saved_at": now_ts(),
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::utils::fs::write_atomic(
        path,
        &serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

// 引入 chrono Local 转换
use chrono::TimeZone;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reminder_inbox_survives_restart_and_snoozes_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let scheduler = Scheduler::with_persistence_path(Some(path.clone()));
        scheduler.set_callback(Arc::new(|_| {}));
        let id = scheduler.schedule_reminder("meeting", now_ts() - 1.0);
        scheduler.tick().await;
        let task = scheduler.get_task(&id).unwrap();
        let notice = scheduler.record_reminder_notice(&task, "nana".into(), "meeting".into(), "receipt1".into()).unwrap();
        let duplicate = scheduler.record_reminder_notice(&task, "nana".into(), "meeting".into(), "receipt2".into()).unwrap();
        assert_eq!(notice.id, duplicate.id);
        scheduler.complete_attempt(&id, task.delivery.attempts, Ok("meeting".into()));
        // History retention must never remove an unread notice, even across restart.
        scheduler.inner.lock().tasks.get_mut(&id).unwrap().completed_at = Some(now_ts() - 86400.0);
        scheduler.persist();
        scheduler.cleanup_terminal_tasks();
        drop(scheduler);
        let restored = Scheduler::with_persistence_path(Some(path.clone()));
        assert_eq!(restored.pending_reminder_notices().len(), 1);
        assert_eq!(restored.pending_reminder_notices()[0].delivery_id, "receipt2");
        let snoozed = restored.acknowledge_reminder_notice(&notice.id, true).unwrap().unwrap();
        assert_eq!(restored.acknowledge_reminder_notice(&notice.id, true).unwrap(), Some(snoozed.clone()));
        assert!(restored.pending_reminder_notices().is_empty());
        assert_eq!(restored.list_tasks().len(), 2);
        let next = restored.get_task(&snoozed).unwrap();
        assert!((next.scheduled_time - now_ts() - 300.0).abs() < 5.0);
        assert_eq!(next.repeat_interval, None);
        assert_eq!(next.char_id, "nana");
        let restarted = Scheduler::with_persistence_path(Some(path));
        assert!(restarted.pending_reminder_notices().is_empty());
        assert!(restarted.get_task(&snoozed).is_some());
    }

    #[test]
    fn reminder_ack_failure_does_not_drop_unread_card() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let scheduler = Scheduler::with_persistence_path(Some(path.clone()));
        let mut task = ScheduledTask::new_reminder("test", now_ts());
        task.status = TaskStatus::Running;
        scheduler.insert_reminder_checked(task.clone()).unwrap();
        let notice = scheduler.record_reminder_notice(&task, "vivian".into(), "test".into(), "receipt".into()).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(scheduler.acknowledge_reminder_notice(&notice.id, true).is_err());
        assert_eq!(scheduler.pending_reminder_notices().len(), 1);
        assert_eq!(scheduler.list_tasks().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_gate_defers_and_releases_due_tasks() {
        let scheduler = Scheduler::new(false);
        scheduler.set_callback(Arc::new(|_| {}));
        scheduler.set_dispatch_gate(Arc::new(|_| false));
        let id = scheduler.schedule_reminder("quiet", now_ts() - 1.0);
        scheduler.tick().await;
        assert_eq!(scheduler.get_task(&id).unwrap().status, TaskStatus::Pending);
        assert_eq!(scheduler.get_task(&id).unwrap().delivery.attempts, 0);
        scheduler.set_dispatch_gate(Arc::new(|_| true));
        scheduler.tick().await;
        assert_eq!(scheduler.get_task(&id).unwrap().status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn delayed_receipt_cannot_complete_the_next_repeat_attempt() {
        let scheduler=Scheduler::new(false);
        scheduler.set_callback(Arc::new(|_|{}));
        let id=scheduler.schedule_repeat(ScheduledTask::new_reminder("repeat",now_ts()-1.0),60);
        scheduler.tick().await;
        scheduler.complete_attempt(&id,1,Ok("first".into()));
        scheduler.update_task_scheduled_time(&id,now_ts()-1.0);
        scheduler.tick().await;
        scheduler.complete_attempt(&id,1,Ok("late duplicate".into()));
        assert_eq!(scheduler.get_task(&id).unwrap().status,TaskStatus::Running);
        assert_eq!(scheduler.get_task(&id).unwrap().delivery.confirmed_count,1);
        scheduler.complete_attempt(&id,2,Ok("second".into()));
        assert_eq!(scheduler.get_task(&id).unwrap().delivery.confirmed_count,2);
        assert_eq!(scheduler.get_task(&id).unwrap().delivery.last_text.as_deref(),Some("second"));
    }

    #[tokio::test]
    async fn restart_recovers_reminders_but_preserves_unknown_tool_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let scheduler = Scheduler::with_persistence_path(Some(path.clone()));
        let reminder = scheduler.schedule_reminder("remember", now_ts() - 1.0);
        let tool = scheduler.schedule_tool_call("write_file", serde_json::json!({}), now_ts() - 1.0);
        scheduler.set_callback(Arc::new(|_| {}));
        scheduler.tick().await;
        tokio::task::yield_now().await;
        drop(scheduler);
        let restored = Scheduler::with_persistence_path(Some(path.clone()));
        assert_eq!(restored.get_task(&reminder).unwrap().status, TaskStatus::Pending);
        let interrupted = restored.get_task(&tool).unwrap();
        assert_eq!(interrupted.status, TaskStatus::Failed);
        assert!(interrupted.metadata["recovery_error"].as_str().unwrap().contains("unknown"));
        let second_restart = Scheduler::with_persistence_path(Some(path));
        assert_eq!(second_restart.get_task(&tool).unwrap().status, TaskStatus::Failed);
    }

    #[tokio::test]
    async fn failed_reservation_persistence_never_dispatches_a_tool() {
        let dir = tempfile::tempdir().unwrap();
        // A directory at the target path makes atomic file replacement fail.
        let path = dir.path().join("blocked");
        std::fs::create_dir(&path).unwrap();
        let scheduler = Scheduler::with_persistence_path(Some(path));
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = calls.clone();
        scheduler.set_callback(Arc::new(move |_| { counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst); }));
        let id = scheduler.schedule_tool_call("write_file",serde_json::json!({}),now_ts()-1.0);
        scheduler.tick().await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),0);
        let task = scheduler.get_task(&id).unwrap();
        assert_eq!(task.status,TaskStatus::Pending);
        assert_eq!(task.delivery.attempts,0);
    }

    #[test]
    fn corrupted_state_is_preserved_for_diagnosis() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("tasks.json");
        std::fs::write(&path,"{partial").unwrap();
        let scheduler=Scheduler::with_persistence_path(Some(path.clone()));
        assert!(scheduler.list_tasks().is_empty());
        assert!(!path.exists());
        assert!(std::fs::read_dir(dir.path()).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().contains(".corrupt-")));
    }

    #[test]
    fn test_schedule_reminder() {
        let scheduler = Scheduler::new(false);
        let id = scheduler.schedule_reminder("test", now_ts() + 60.0);
        assert!(scheduler.get_task(&id).is_some());
    }

    #[test]
    fn test_cancel_task() {
        let scheduler = Scheduler::new(false);
        let id = scheduler.schedule_reminder("test", now_ts() + 60.0);
        assert!(scheduler.cancel_task(&id));
        let task = scheduler.get_task(&id).unwrap();
        assert_eq!(task.status, TaskStatus::Cancelled);
    }

    #[test]
    fn test_list_pending_sorted_by_priority() {
        let scheduler = Scheduler::new(false);
        let now = now_ts();
        let _ = scheduler.schedule_reminder("low", now + 60.0);
        // 手动插入高优先级
        let mut task = ScheduledTask::new_reminder("urgent", now + 60.0);
        task.priority = Priority::Urgent;
        scheduler.insert_task(task);

        let pending = scheduler.list_pending();
        assert_eq!(pending.len(), 2);
        // 紧急任务应排在前面
        assert_eq!(pending[0].priority, Priority::Urgent);
    }

    #[test]
    fn test_parse_iso_duration() {
        let result = parse_time_spec("PT2M").unwrap();
        let now = now_ts();
        assert!((result - now - 120.0).abs() < 5.0);

        let result2 = parse_time_spec("PT1H").unwrap();
        assert!((result2 - now - 3600.0).abs() < 5.0);
    }

    #[test]
    fn test_parse_invalid_format() {
        assert!(parse_time_spec("invalid").is_err());
    }

    #[tokio::test]
    async fn reminder_waits_for_completion_and_retries_without_counting_attempts() {
        let scheduler = Scheduler::new(false);
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let calls = counter.clone();
        scheduler.set_callback(Arc::new(move |_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let original = now_ts() - 100.0;
        let id = scheduler.schedule_reminder("test", original);
        scheduler.tick().await;
        tokio::task::yield_now().await;
        assert_eq!(
            scheduler.inner.lock().tasks[&id].status,
            TaskStatus::Running
        );
        assert_eq!(
            scheduler.inner.lock().tasks[&id].delivery.confirmed_count,
            0
        );
        scheduler.complete_execution(&id, Err("transport failed".into()));
        assert_eq!(scheduler.inner.lock().tasks[&id].scheduled_time, original);
        scheduler.tick().await;
        tokio::task::yield_now().await;
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        scheduler
            .inner
            .lock()
            .tasks
            .get_mut(&id)
            .unwrap()
            .delivery
            .next_attempt_at = None;
        scheduler.tick().await;
        tokio::task::yield_now().await;
        scheduler.complete_execution(&id, Ok("delivered".into()));
        scheduler.complete_execution(&id, Ok("duplicate acknowledgement".into()));
        let inner = scheduler.inner.lock();
        let task = &inner.tasks[&id];
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.delivery.attempts, 2);
        assert_eq!(task.delivery.confirmed_count, 1);
        assert_eq!(task.delivery.last_text.as_deref(), Some("delivered"));
    }
    #[tokio::test]
    async fn failed_tool_calls_are_not_repeated() {
        let scheduler = Scheduler::new(false);
        scheduler.set_callback(Arc::new(|_| {}));
        let id = scheduler.schedule_tool_call("operation", serde_json::json!({}), now_ts() - 1.0);
        scheduler.tick().await;
        tokio::task::yield_now().await;
        scheduler.complete_execution(&id, Err("operation failed".into()));
        assert_eq!(scheduler.inner.lock().tasks[&id].status, TaskStatus::Failed);
        assert_eq!(
            scheduler.inner.lock().tasks[&id].delivery.confirmed_count,
            0
        );
    }

    #[tokio::test]
    async fn test_tick_triggers_callback() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        let scheduler = Scheduler::new(false);
        scheduler.set_callback(Arc::new(move |_task| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        }));

        // 调度一个立即到期的任务
        let scheduler = scheduler;
        let _id = scheduler.schedule_reminder("test", now_ts() - 1.0);
        scheduler.tick().await;
        // 给异步任务一点时间
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(counter.load(Ordering::SeqCst) >= 1);
    }
}
