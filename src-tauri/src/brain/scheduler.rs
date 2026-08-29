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

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::utils::path::get_user_data_dir;

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
}

impl ScheduledTask {
    /// 生成 8 字符随机 ID（取 uuid 前 8 字符）。
    fn generate_id() -> String {
        uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("00000000").to_string()
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
    shutdown: Arc<tokio::sync::Notify>,
}

struct SchedulerInner {
    tasks: HashMap<String, ScheduledTask>,
    callback: Option<SchedulerCallback>,
    /// 预触发回调：在 `scheduled_time - 5s` 到 `scheduled_time` 之间触发一次，
    /// 让主 LLM 提前决定如何进行该定时任务。
    pre_trigger_callback: Option<SchedulerCallback>,
}

impl Scheduler {
    /// 创建调度器。`persist=true` 时持久化到用户数据目录。
    pub fn new(persist: bool) -> Self {
        let persistence_path = if persist {
            Some(get_user_data_dir().join("scheduled_tasks.json"))
        } else {
            None
        };
        let mut scheduler = Self {
            inner: Arc::new(Mutex::new(SchedulerInner {
                tasks: HashMap::new(),
                callback: None,
                pre_trigger_callback: None,
            })),
            persistence_path,
            shutdown: Arc::new(tokio::sync::Notify::new()),
        };
        if let Some(path) = scheduler.persistence_path.clone() {
            scheduler.load_tasks_from(&path);
        }
        scheduler
    }

    /// 设置任务触发回调。
    pub fn set_callback(&self, callback: SchedulerCallback) {
        self.inner.lock().callback = Some(callback);
    }

    /// 设置预触发回调。
    ///
    /// 在 `scheduled_time - 5s` 到 `scheduled_time` 之间触发一次，
    /// 用于发起主 LLM 调用，让智能体提前决定如何进行定时任务。
    /// 与正常 `set_callback` 分离，避免影响到期触发的原有行为。
    pub fn set_pre_trigger_callback(&self, callback: SchedulerCallback) {
        self.inner.lock().pre_trigger_callback = Some(callback);
    }

    /// 调度一个提醒任务，返回任务 ID。
    pub fn schedule_reminder(
        &self,
        message: impl Into<String>,
        scheduled_time: f64,
    ) -> String {
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
    pub fn schedule_repeat(
        &self,
        task: ScheduledTask,
        interval_seconds: u64,
    ) -> String {
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
                    tracing::info!(task_id, new_time = task.scheduled_time, "[Scheduler] 恢复时已顺延过期任务");
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
        let mut tasks: Vec<ScheduledTask> =
            self.inner.lock().tasks.values().cloned().collect();
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
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.scheduled_time.partial_cmp(&b.scheduled_time).unwrap_or(std::cmp::Ordering::Equal))
        });
        tasks
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
        // ── 预触发检测：在 scheduled_time - 5s 到 scheduled_time 之间
        // 发起一次主 LLM 调用，让智能体提前决定如何进行定时任务 ──
        const PRE_TRIGGER_LEAD_SECS: f64 = 5.0;
        let now = now_ts();
        let pre_trigger_tasks: Vec<ScheduledTask> = {
            let mut inner = self.inner.lock();
            let callback = inner.pre_trigger_callback.clone();
            if callback.is_none() {
                // 没有注册预触发回调：跳过检测，避免无谓遍历
                Vec::new()
            } else {
                inner
                    .tasks
                    .values_mut()
                    .filter(|t| {
                        // 仅 Reminder 类型参与预触发（ToolCall 类型本身即智能体自执行，无需预触发）
                        t.status == TaskStatus::Pending
                            && !t.pre_triggered
                            && t.task_type == TaskType::Reminder
                            && t.scheduled_time > now
                            && t.scheduled_time - now <= PRE_TRIGGER_LEAD_SECS
                    })
                    .map(|t| {
                        t.pre_triggered = true;
                        t.clone()
                    })
                    .collect()
            }
        };

        if !pre_trigger_tasks.is_empty() {
            self.persist();
            let callback = self.inner.lock().pre_trigger_callback.clone();
            if let Some(cb) = callback {
                for task in pre_trigger_tasks {
                    let cb = cb.clone();
                    tokio::spawn(async move {
                        cb(task);
                    });
                }
            }
        }

        let due_tasks: Vec<ScheduledTask> = {
            let inner = self.inner.lock();
            inner
                .tasks
                .values()
                .filter(|t| {
                    t.status == TaskStatus::Pending && t.scheduled_time <= now_ts()
                })
                .cloned()
                .collect()
        };

        for task in due_tasks {
            // 打扰检查：将任务优先级映射到 InterruptPriority
            let interrupt_priority = match task.priority {
                Priority::Urgent => crate::brain::interruption_controller::InterruptPriority::Urgent,
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
            // 标记为 Running
            {
                let mut inner = self.inner.lock();
                if let Some(t) = inner.tasks.get_mut(&task.id) {
                    t.status = TaskStatus::Running;
                }
            }

            // 触发回调
            let callback = self.inner.lock().callback.clone();
            if let Some(cb) = callback {
                let task_clone = task.clone();
                let inner_arc = self.inner.clone();
                let persistence_path = self.persistence_path.clone();
                // 异步执行，避免阻塞调度循环
                tokio::spawn(async move {
                    let cb = cb.clone();
                    // 在 try_into 之前先 clone 任务用于回调
                    cb(task_clone.clone());

                    // 处理重复任务或清理
                    {
                        let mut guard = inner_arc.lock();
                        if let Some(t) = guard.tasks.get_mut(&task_clone.id) {
                            if let Some(interval) = t.repeat_interval {
                                // 基于原定时间 + interval 计算下次触发，避免时间漂移
                                // 若已过去则顺延到未来（避免立即触发或累积延迟）
                                let interval_f = interval as f64;
                                let mut next = t.scheduled_time + interval_f;
                                let now = now_ts();
                                while next <= now {
                                    next += interval_f;
                                }
                                t.scheduled_time = next;
                                t.status = TaskStatus::Pending;
                                // 重置预触发标志：新一轮等待窗口可再次发起 LLM 预调用
                                t.pre_triggered = false;
                            } else {
                                t.status = TaskStatus::Completed;
                                t.completed_at = Some(now_ts());
                            }
                        }
                    }
                    if let Some(path) = persistence_path {
                        let tasks = inner_arc.lock().tasks.clone();
                        if let Err(e) = save_tasks_to(&path, &tasks) {
                            tracing::warn!(error = %e, "[scheduler] 定时任务保存失败，重启后可能丢失已设置的提醒");
                        }
                    }
                });
            }
        }

        // tick 末尾滚动清理：drain 已完成且超过 SCHEDULER_COMPLETED_RETENTION_SECS 的任务
        // 避免单次运行期内 Completed/Cancelled/Failed 任务持续累积导致内存增长
        self.cleanup_terminal_tasks();
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
        }
    }

    /// 关闭调度器。
    pub fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// 清空所有定时任务（内存 + 持久化文件）
    /// 供 factory_reset 调用：清空内存 HashMap 并删除磁盘文件，避免重启后任务"复活"
    pub fn clear_all_tasks(&self) {
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

    fn persist(&self) {
        if let Some(path) = &self.persistence_path {
            let tasks = self.inner.lock().tasks.clone();
            if let Err(e) = save_tasks_to(path, &tasks) {
                tracing::warn!(error = %e, "[scheduler] 定时任务保存失败，重启后可能丢失已设置的提醒");
            }
        }
    }

    fn load_tasks_from(&mut self, path: &PathBuf) {
        if let Ok(content) = std::fs::read_to_string(path) {
            #[derive(serde::Deserialize)]
            struct Persisted {
                tasks: Vec<ScheduledTask>,
            }
            if let Ok(data) = serde_json::from_str::<Persisted>(&content) {
                let mut inner = self.inner.lock();
                for task in data.tasks {
                    // 加载未完成的 Pending 任务（过期也保留，tick 会立即触发）
                    // 和 Paused 任务（暂停状态需保留，等待用户恢复）
                    if task.status == TaskStatus::Pending || task.status == TaskStatus::Paused {
                        inner.tasks.insert(task.id.clone(), task);
                    }
                }
                tracing::info!(
                    loaded = inner.tasks.len(),
                    "[Scheduler] 已加载持久化任务"
                );
            }
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
    use regex::Regex;
    use once_cell::sync::Lazy;

    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$")
            .unwrap()
    });

    let caps = RE
        .captures(spec)
        .ok_or_else(|| format!("无效的 ISO 8601 持续时间: {}", spec))?;

    let years = caps.get(1).map(|m| m.as_str().parse::<u64>().unwrap_or(0)).unwrap_or(0);
    let months = caps.get(2).map(|m| m.as_str().parse::<u64>().unwrap_or(0)).unwrap_or(0);
    let days = caps.get(3).map(|m| m.as_str().parse::<u64>().unwrap_or(0)).unwrap_or(0);
    let hours = caps.get(4).map(|m| m.as_str().parse::<u64>().unwrap_or(0)).unwrap_or(0);
    let minutes = caps.get(5).map(|m| m.as_str().parse::<u64>().unwrap_or(0)).unwrap_or(0);
    let seconds = caps.get(6).map(|m| m.as_str().parse::<u64>().unwrap_or(0)).unwrap_or(0);

    let total_days = days + years * 365 + months * 30;
    let total_seconds =
        total_days * 86400 + hours * 3600 + minutes * 60 + seconds;
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
    std::fs::write(path, serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

// 引入 chrono Local 转换
use chrono::TimeZone;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_reminder() {
        let mut scheduler = Scheduler::new(false);
        let id = scheduler.schedule_reminder("test", now_ts() + 60.0);
        assert!(scheduler.get_task(&id).is_some());
    }

    #[test]
    fn test_cancel_task() {
        let mut scheduler = Scheduler::new(false);
        let id = scheduler.schedule_reminder("test", now_ts() + 60.0);
        assert!(scheduler.cancel_task(&id));
        let task = scheduler.get_task(&id).unwrap();
        assert_eq!(task.status, TaskStatus::Cancelled);
    }

    #[test]
    fn test_list_pending_sorted_by_priority() {
        let mut scheduler = Scheduler::new(false);
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
    async fn test_tick_triggers_callback() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        let scheduler = Scheduler::new(false);
        scheduler.set_callback(Arc::new(move |_task| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        }));

        // 调度一个立即到期的任务
        let mut scheduler = scheduler;
        let _id = scheduler.schedule_reminder("test", now_ts() - 1.0);
        scheduler.tick().await;
        // 给异步任务一点时间
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(counter.load(Ordering::SeqCst) >= 1);
    }
}
