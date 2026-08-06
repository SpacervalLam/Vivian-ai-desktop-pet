//! 在场状态系统（Presence System）
//!
//! 四态状态机管理桌宠的"在场/不在场"语义：
//! - `Online`：在线，可面对面对话（direct）+ 能听见他人对话 + 能微信
//! - `Busy`：忙碌，在场但不主动说话，用户可发起但不一定立即回
//! - `Rest`：休息，不可面对面但能收微信（类似午睡轻度可唤醒）
//! - `Offline`：离线，窗口隐藏但仍保留主动上线能力
//!   （离线满一定时长且孤独感累积达标时，主动回归 Online ——「想念用户」）
//!
//! 触发方式：
//! - LLM 通过 `set_presence_state` 工具主动调用
//! - 程序端自主触发（心情驱动 / 被忽略次数 / 两角色协调 / 想念用户）
//!
//! 持久化：`characters/<char_id>/presence/state.json`（按角色隔离）
//! 记忆写入：状态切换时写入 ShortTerm 记忆（行为日志），含 from→to 与原因

pub mod background_tasks;
pub mod config;

pub use background_tasks::{
    spawn_knowledge_acquisition, spawn_memory_consolidation, spawn_user_cognition_consolidation,
};
pub use config::PresenceConfig;

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::{ensure_dir, get_character_data_dir};

/// 在场状态四态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PresenceState {
    /// 在线：可面对面 + 能听见他人 + 能微信
    Online,
    /// 忙碌：在场但不主动说话，用户可发起但不一定立即回
    Busy,
    /// 休息：不可面对面但能收微信
    Rest,
    /// 离线：窗口隐藏，仅微信留消息；但保留主动上线能力（想念用户）
    Offline,
}

impl PresenceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PresenceState::Online => "online",
            PresenceState::Busy => "busy",
            PresenceState::Rest => "rest",
            PresenceState::Offline => "offline",
        }
    }

    pub fn display_zh(&self) -> &'static str {
        match self {
            PresenceState::Online => "在线",
            PresenceState::Busy => "忙碌",
            PresenceState::Rest => "休息",
            PresenceState::Offline => "离线",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "online" => PresenceState::Online,
            "busy" => PresenceState::Busy,
            "rest" => PresenceState::Rest,
            "offline" => PresenceState::Offline,
            _ => PresenceState::Online,
        }
    }

    /// 是否在场（能听见他人对话）
    pub fn is_in_presence(&self) -> bool {
        matches!(self, PresenceState::Online | PresenceState::Busy)
    }

    /// 是否允许面对面对话（direct 渠道）
    /// Online 正常对话；Rest 允许被叫醒迷糊应答；Busy/Offline 拒绝
    pub fn can_direct(&self) -> bool {
        matches!(self, PresenceState::Online | PresenceState::Rest)
    }

    /// 是否允许微信（wechat 渠道）
    pub fn can_wechat(&self) -> bool {
        true
    }
}

impl Default for PresenceState {
    fn default() -> Self {
        PresenceState::Online
    }
}

impl std::fmt::Display for PresenceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 状态切换触发原因
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresenceChangeReason {
    /// LLM 通过 set_presence_state 工具触发
    LlmTrigger,
    /// 心情驱动（疲劳/孤独等）
    MoodDriven,
    /// 想念用户：离线状态下孤独感持续累积，主动回归 Online
    MissedUser,
    /// 休息够了：Rest 持续满阈值后自动醒来
    RestedEnough,
    /// 后台任务完成：Busy 知识采集任务自然结束后自动回到 Online
    TaskCompleted,
    /// 被忽略次数过多
    Ignored,
    /// 两角色协调（都在线过久）
    Coordination,
    /// 用户交互唤醒
    UserInteraction,
    /// 用户去忙了，角色也跟着去做自己的事
    UserLeft,
    /// 系统初始化
    SystemInit,
}

impl PresenceChangeReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            PresenceChangeReason::LlmTrigger => "llm_trigger",
            PresenceChangeReason::MoodDriven => "mood_driven",
            PresenceChangeReason::MissedUser => "missed_user",
            PresenceChangeReason::RestedEnough => "rested_enough",
            PresenceChangeReason::TaskCompleted => "task_completed",
            PresenceChangeReason::Ignored => "ignored",
            PresenceChangeReason::Coordination => "coordination",
            PresenceChangeReason::UserInteraction => "user_interaction",
            PresenceChangeReason::UserLeft => "user_left",
            PresenceChangeReason::SystemInit => "system_init",
        }
    }

    pub fn display_zh(&self) -> &'static str {
        match self {
            PresenceChangeReason::LlmTrigger => "主动声明",
            PresenceChangeReason::MoodDriven => "心情驱动",
            PresenceChangeReason::MissedUser => "想念用户",
            PresenceChangeReason::RestedEnough => "休息够了",
            PresenceChangeReason::TaskCompleted => "忙完了",
            PresenceChangeReason::Ignored => "被忽略太久",
            PresenceChangeReason::Coordination => "与另一个我协调",
            PresenceChangeReason::UserInteraction => "用户唤醒",
            PresenceChangeReason::UserLeft => "用户去忙了",
            PresenceChangeReason::SystemInit => "系统初始化",
        }
    }
}

/// 状态切换事件（用于持久化历史 + 记忆写入）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceEvent {
    pub from: String,
    pub to: String,
    pub timestamp: f64,
    pub reason: String,
}

/// 持久化状态
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PresencePersistState {
    /// 当前状态（字符串形式便于序列化）
    pub current: String,
    /// 进入当前状态的时间戳（Unix 秒）
    pub since: f64,
    /// 上次在线时间戳
    pub last_online: f64,
    /// 今日总在线时长（秒）
    pub total_online_today: f64,
    /// 今日总休息时长（秒）
    pub total_rest_today: f64,
    /// 今日计数重置时间戳（每天 0 点重置）
    pub day_reset_at: f64,
    /// 上次用户互动时间戳（Unix 秒，用于在线空闲→Busy 判定）
    #[serde(default)]
    pub last_user_interaction: f64,
    /// 近期状态切换历史（最多 50 条）
    pub history: Vec<PresenceEvent>,
}

/// 任务分发钩子：transition 切到 Busy/Rest 时同步调用
///
/// 由 Brain 初始化时通过 `set_task_spawner` 注入，闭包内部负责：
/// - 同步调 `presence.begin_task()`（消除 race window）
/// - `tokio::spawn` 异步任务（任务体只调 `finish_task()` 收尾）
pub type TaskSpawner = Arc<dyn Fn() + Send + Sync>;

/// 在场状态管理器（按角色隔离）
///
/// 线程安全：内部用 `RwLock<PresencePersistState>` 保护，
/// `transition` 方法在写锁内完成状态切换 + 持久化 + 历史追加。
///
/// 后台任务延迟退出机制：
/// - `task_in_progress` 标记当前是否有 Busy 知识采集 / Rest 记忆沉淀在跑
/// - 期间任何 `transition(Online, ...)` 不会立即切换，只标记 `pending_exit_to_online`
/// - 后台任务结束时调 `finish_task()`，若标记了延迟退出则自动 `transition(Online)`
pub struct PresenceManager {
    state: Arc<RwLock<PresencePersistState>>,
    persistence_path: PathBuf,
    char_id: String,
    /// 后台任务进行中标记（Busy 知识采集 / Rest 记忆沉淀）
    task_in_progress: Arc<parking_lot::Mutex<bool>>,
    /// 用户已请求唤醒但被延迟，等任务结束后切回 Online
    pending_exit_to_online: Arc<parking_lot::Mutex<bool>>,
    /// 后台任务已自然完成，但当前状态持续不足 min_state_duration，
    /// 延迟到满足最小时长后再切回 Online（避免"去歇会 → 9 秒后忙完了"的突兀感）
    task_completed_pending: Arc<parking_lot::Mutex<bool>>,
    /// Busy 状态进入时调用的任务分发钩子（spawn 知识采集任务）
    busy_task_spawner: parking_lot::RwLock<Option<TaskSpawner>>,
    /// Rest 状态进入时调用的任务分发钩子（spawn 记忆沉淀任务）
    rest_task_spawner: parking_lot::RwLock<Option<TaskSpawner>>,
}

impl PresenceManager {
    /// 创建指定角色的在场状态管理器
    pub fn new(char_id: &str) -> VivianResult<Self> {
        let dir = get_character_data_dir(char_id).join("presence");
        ensure_dir(&dir).map_err(|e| VivianError::Memory(format!("创建在场状态目录失败: {e}")))?;
        let persistence_path = dir.join("state.json");

        let now_ts = chrono::Local::now().timestamp() as f64;
        let mut state = if persistence_path.exists() {
            Self::load_from(&persistence_path)
        } else {
            PresencePersistState {
                current: PresenceState::Online.as_str().to_string(),
                since: now_ts,
                last_online: now_ts,
                day_reset_at: now_ts,
                last_user_interaction: now_ts,
                ..Default::default()
            }
        };

        // 启动时始终重置 last_user_interaction 为当前时间：
        // 持久化的值是上次运行时的时间戳，若距上次运行已超过阈值，
        // 启动后 user_idle_seconds 会立即满足 Online→Busy 条件，导致刚启动就进入忙碌。
        // 重置后空闲计时从启动时刻重新开始，避免启动即 Busy。
        state.last_user_interaction = now_ts;

        // 启动重置：若上次退出时停留在非 Online 状态，应用重启后直接恢复 Online。
        // 原因：Rest/Offline/Busy 状态通常由自动触发产生（疲劳/忽略/协调），
        // 没有用户主动交互就无法唤醒，导致一打开界面就看不到角色，体验违和。
        // 后台任务随进程退出已中断，task_in_progress 也已重置为 false，可安全恢复。
        let prev_state = PresenceState::from_str(&state.current);
        if prev_state != PresenceState::Online {
            let prev_since = state.since;
            let elapsed = (now_ts - prev_since).max(0.0);
            // 累加旧状态时长到对应计数器
            match prev_state {
                PresenceState::Rest => state.total_rest_today += elapsed,
                PresenceState::Offline | PresenceState::Busy => {
                    // Offline/Busy 不累加到 total_online_today（不算在线时长）
                }
                PresenceState::Online => unreachable!(),
            }
            state.history.push(PresenceEvent {
                from: state.current.clone(),
                to: PresenceState::Online.as_str().to_string(),
                timestamp: now_ts,
                reason: PresenceChangeReason::SystemInit.as_str().to_string(),
            });
            if state.history.len() > 50 {
                state.history.drain(0..state.history.len() - 50);
            }
            state.current = PresenceState::Online.as_str().to_string();
            tracing::info!(
                "[Presence:{}] 启动重置：{} → Online（上次状态持续 {:.0}s）",
                char_id,
                prev_state.as_str(),
                elapsed
            );
        }

        // 无论持久化状态如何，启动时 since 必须重置为当前时间。
        // 否则上次运行时 Online 状态的旧 since 会让 both_online_duration 计算出巨大值
        // （now - old_since 可能是数小时），启动后第一个 tick 立即满足
        // coordination_threshold（1 小时）→ 启动即触发 Coordination → Rest。
        state.since = now_ts;

        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            persistence_path,
            char_id: char_id.to_string(),
            task_in_progress: Arc::new(parking_lot::Mutex::new(false)),
            pending_exit_to_online: Arc::new(parking_lot::Mutex::new(false)),
            task_completed_pending: Arc::new(parking_lot::Mutex::new(false)),
            busy_task_spawner: parking_lot::RwLock::new(None),
            rest_task_spawner: parking_lot::RwLock::new(None),
        })
    }

    /// 创建使用临时目录持久化的降级实例
    ///
    /// 当角色数据目录不可写时使用：状态在临时目录中持久化，
    /// 重启后丢失（不会与正常状态混淆），但进程内功能完整可用。
    pub fn new_with_temp_dir(char_id: &str) -> VivianResult<Self> {
        let dir = std::env::temp_dir().join(format!("vivian-presence-{}", char_id));
        ensure_dir(&dir)
            .map_err(|e| VivianError::Memory(format!("创建临时在场状态目录失败: {e}")))?;
        let persistence_path = dir.join("state.json");

        let now_ts = chrono::Local::now().timestamp() as f64;
        let state = PresencePersistState {
            current: PresenceState::Online.as_str().to_string(),
            since: now_ts,
            last_online: now_ts,
            day_reset_at: now_ts,
            last_user_interaction: now_ts,
            ..Default::default()
        };

        tracing::warn!(
            "[Presence:{}] 使用临时目录降级: {}",
            char_id,
            dir.display()
        );

        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            persistence_path,
            char_id: char_id.to_string(),
            task_in_progress: Arc::new(parking_lot::Mutex::new(false)),
            pending_exit_to_online: Arc::new(parking_lot::Mutex::new(false)),
            task_completed_pending: Arc::new(parking_lot::Mutex::new(false)),
            busy_task_spawner: parking_lot::RwLock::new(None),
            rest_task_spawner: parking_lot::RwLock::new(None),
        })
    }

    fn load_from(path: &std::path::Path) -> PresencePersistState {
        match std::fs::read_to_string(path) {
            Ok(content) if !content.trim().is_empty() => {
                serde_json::from_str::<PresencePersistState>(&content).unwrap_or_default()
            }
            _ => PresencePersistState::default(),
        }
    }

    fn save_to(&self) -> VivianResult<()> {
        let state = self.state.read().clone();
        let json = serde_json::to_string_pretty(&state)
            .map_err(|e| VivianError::Memory(format!("序列化在场状态失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| VivianError::Memory(format!("写入在场状态临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换在场状态文件失败: {e}")))?;
        Ok(())
    }

    /// 获取当前状态
    pub fn current(&self) -> PresenceState {
        PresenceState::from_str(&self.state.read().current)
    }

    /// 进入当前状态的时间戳
    pub fn since(&self) -> f64 {
        self.state.read().since
    }

    /// 当前状态已持续的秒数
    pub fn elapsed_seconds(&self) -> f64 {
        let now = chrono::Local::now().timestamp() as f64;
        (now - self.state.read().since).max(0.0)
    }

    /// 是否允许面对面对话（direct 渠道）
    pub fn can_direct(&self) -> bool {
        self.current().can_direct()
    }

    /// 是否在场（能听见他人对话）
    pub fn is_in_presence(&self) -> bool {
        self.current().is_in_presence()
    }

    /// 角色 ID
    pub fn char_id(&self) -> &str {
        &self.char_id
    }

    pub fn recent_history(&self, n: usize) -> Vec<PresenceEvent> {
        let state = self.state.read();
        let len = state.history.len();
        let start = len.saturating_sub(n);
        state.history[start..].to_vec()
    }

    /// 状态切换
    ///
    /// 如果 `to` 与当前状态相同则返回 None（无切换）。
    ///
    /// 延迟退出语义：若当前有后台任务在跑（`task_in_progress == true`）且目标为 `Online`，
    /// 则不立即切换，仅置 `pending_exit_to_online = true`，由后台任务结束时统一收尾。
    /// 这种情况下返回 `None`，调用方应通过 `has_pending_exit()` 区分「未切换」与「不需要切换」，
    /// 以便给用户提示「等我把手上事做完」。
    ///
    /// 切换时：
    /// 1. 累计旧状态的在线/休息时长
    /// 2. 更新 current / since / last_online
    /// 3. 追加到 history（最多 50 条）
    /// 4. 持久化
    /// 5. 返回 PresenceEvent（供调用方写入记忆）
    pub fn transition(
        &self,
        to: PresenceState,
        reason: PresenceChangeReason,
    ) -> Option<PresenceEvent> {
        // 延迟退出：后台任务进行中切回 Online 时，标记延迟并直接返回 None
        if to == PresenceState::Online && self.is_task_in_progress() {
            *self.pending_exit_to_online.lock() = true;
            tracing::info!(
                "[Presence:{}] 任务进行中，已标记延迟退出 → Online（reason={}）",
                self.char_id,
                reason.as_str()
            );
            return None;
        }

        let now = chrono::Local::now().timestamp() as f64;
        let mut state = self.state.write();

        let from = PresenceState::from_str(&state.current);
        if from == to {
            return None;
        }

        // 累计旧状态时长
        let duration = (now - state.since).max(0.0);
        match from {
            PresenceState::Online | PresenceState::Busy => {
                state.total_online_today += duration;
                state.last_online = now;
            }
            PresenceState::Rest => {
                state.total_rest_today += duration;
            }
            _ => {}
        }

        // 日计数重置（每天 0 点）
        let today_start = {
            let now_local = chrono::Local::now();
            now_local
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp() as f64
        };
        if state.day_reset_at < today_start {
            state.total_online_today = 0.0;
            state.total_rest_today = 0.0;
            state.day_reset_at = today_start;
        }

        let event = PresenceEvent {
            from: from.as_str().to_string(),
            to: to.as_str().to_string(),
            timestamp: now,
            reason: reason.as_str().to_string(),
        };

        state.current = to.as_str().to_string();
        state.since = now;
        if to == PresenceState::Online {
            state.last_online = now;
        }

        // 追加历史（最多 50 条）
        state.history.push(event.clone());
        let len = state.history.len();
        if len > 50 {
            state.history.drain(0..len - 50);
        }

        drop(state);
        let _ = self.save_to();

        tracing::info!(
            "[Presence:{}] {} → {} ({})",
            self.char_id,
            event.from,
            event.to,
            reason.display_zh()
        );

        // 切到 Busy/Rest 时触发对应任务分发钩子（spawn 后台任务）
        // 钩子内部负责 begin_task + tokio::spawn
        let spawner = match to {
            PresenceState::Busy => self.busy_task_spawner.read().clone(),
            PresenceState::Rest => self.rest_task_spawner.read().clone(),
            _ => None,
        };
        if let Some(spawner) = spawner {
            // 同步标记任务进行中，避免在 spawn 任务体调 begin_task 之前出现 race window
            self.begin_task();
            spawner();
        }

        Some(event)
    }

    /// 用户交互唤醒：用户发起对话时，从 Rest/Offline 回到 Online
    ///
    /// 返回是否发生了切换（用于决定是否写记忆）。
    /// 注意：若后台任务进行中（task_in_progress），返回 None 但会标记延迟退出，
    /// 调用方应通过 `has_pending_exit()` 区分「未切换但已延迟」与「状态本来就不需要切」。
    pub fn wake_on_user_interaction(&self) -> Option<PresenceEvent> {
        let current = self.current();
        match current {
            PresenceState::Rest | PresenceState::Offline => {
                self.transition(PresenceState::Online, PresenceChangeReason::UserInteraction)
            }
            _ => None,
        }
    }

    /// 记录用户互动时间戳
    ///
    /// 每次用户发消息时调用，用于在线空闲→Busy 的自动判定。
    pub fn record_user_interaction(&self) {
        let now = chrono::Local::now().timestamp() as f64;
        let mut state = self.state.write();
        state.last_user_interaction = now;
        drop(state);
        let _ = self.save_to();
    }

    /// 获取自上次用户互动以来经过的秒数
    pub fn user_idle_seconds(&self) -> f64 {
        let now = chrono::Local::now().timestamp() as f64;
        let state = self.state.read();
        (now - state.last_user_interaction).max(0.0)
    }

    /// 后台任务开始：标记 task_in_progress，阻止任何 transition(Online) 立即生效
    ///
    /// 由 Busy 知识采集 / Rest 记忆沉淀任务在 spawn 后立即调用。
    /// 已有任务在跑时再次调用是 no-op（防止并发任务叠加）。
    pub fn begin_task(&self) {
        let mut flag = self.task_in_progress.lock();
        if *flag {
            tracing::warn!(
                "[Presence:{}] begin_task 被重复调用，已忽略（任务已在进行中）",
                self.char_id
            );
            return;
        }
        *flag = true;
        tracing::info!("[Presence:{}] 后台任务开始", self.char_id);
    }

    /// 后台任务结束：清除 task_in_progress 标记。
    ///
    /// 若期间用户请求过唤醒（pending_exit_to_online == true），
    /// 则自动 `transition(Online, UserInteraction)` 并返回该事件，调用方负责写记忆 + emit 事件。
    /// 否则返回 None。
    pub fn finish_task(&self, config: &PresenceConfig) -> Option<PresenceEvent> {
        {
            let mut flag = self.task_in_progress.lock();
            *flag = false;
        }
        tracing::info!("[Presence:{}] 后台任务结束", self.char_id);

        let pending = {
            let mut p = self.pending_exit_to_online.lock();
            let v = *p;
            *p = false;
            v
        };
        if pending {
            tracing::info!(
                "[Presence:{}] 检测到延迟退出标记，自动切回 Online",
                self.char_id
            );
            return self.transition(PresenceState::Online, PresenceChangeReason::UserInteraction);
        }

        // 任务自然结束（无用户唤醒请求）：若当前仍在 Busy/Rest，自动切回 Online
        // 避免 Busy/Rest 状态在任务结束后无出口而"卡死"
        let current = self.current();
        if matches!(current, PresenceState::Busy | PresenceState::Rest) {
            // 最短状态持续保护：若当前状态时长不足 min_state_duration，
            // 不立即切回 Online，而是标记 task_completed_pending，
            // 由 check_auto_triggers 在满足最小时长后触发 TaskCompleted 切换。
            // 避免"去歇会 → 9 秒后忙完了"的突兀感。
            let elapsed = self.elapsed_seconds();
            let min_duration = config.min_state_duration;
            if elapsed < min_duration {
                *self.task_completed_pending.lock() = true;
                tracing::info!(
                    "[Presence:{}] 后台任务自然结束，但状态仅持续 {:.0}s < min_state_duration({:.0}s)，延迟切回 Online",
                    self.char_id,
                    elapsed,
                    min_duration
                );
                return None;
            }
            tracing::info!(
                "[Presence:{}] 后台任务自然结束，自动切回 Online（TaskCompleted）",
                self.char_id
            );
            return self.transition(PresenceState::Online, PresenceChangeReason::TaskCompleted);
        }
        None
    }

    /// 当前是否有后台任务在跑
    pub fn is_task_in_progress(&self) -> bool {
        *self.task_in_progress.lock()
    }

    /// 是否有用户已请求但被延迟的唤醒
    pub fn has_pending_exit(&self) -> bool {
        *self.pending_exit_to_online.lock()
    }

    /// 注册 Busy 状态进入时的任务分发钩子（spawn 知识采集任务）
    ///
    /// 由 Brain 初始化时调用。钩子闭包内部应：
    /// 1. 准备好 `router` / `memory` / `app` 等依赖
    /// 2. 调 `spawn_knowledge_acquisition(...)`（内部 tokio::spawn 异步任务）
    ///
    /// `begin_task()` 已在 transition 内同步调用，闭包无需再调。
    pub fn set_busy_task_spawner(&self, spawner: TaskSpawner) {
        *self.busy_task_spawner.write() = Some(spawner);
    }

    /// 注册 Rest 状态进入时的任务分发钩子（spawn 记忆沉淀任务）
    ///
    /// 由 Brain 初始化时调用。同 `set_busy_task_spawner`。
    pub fn set_rest_task_spawner(&self, spawner: TaskSpawner) {
        *self.rest_task_spawner.write() = Some(spawner);
    }

    /// 检查程序端自主触发条件
    ///
    /// 五个条件（任一满足即返回建议的目标状态 + 原因）：
    /// 1. 心情驱动：疲劳度高 → Rest；孤独感持续高 + 被忽略 → Offline
    /// 2. 被忽略次数：连续被忽略 N 次 → Offline
    /// 3. 两角色协调：两角色都在线超过阈值 → 其中一个 Rest
    /// 4. 在线空闲：用户长时间未互动 → Busy（去做知识采集）
    /// 5. 想念用户：离线满一定时长且孤独感累积达标 → 主动回归 Online
    ///
    /// 参数：
    /// - `fatigue`: 疲劳度 (0-100)，来自 MoodSnapshot.fatigue
    /// - `loneliness`: 孤独感 (0-1)，来自 EmotionState.loneliness
    /// - `ignored_count`: 连续被忽略次数
    /// - `other_in_presence`: 另一个角色是否在场（Online/Busy）
    /// - `both_online_duration`: 两角色同时在线的持续秒数
    /// - `user_idle_seconds`: 用户未互动的秒数
    pub fn check_auto_triggers(
        &self,
        config: &PresenceConfig,
        fatigue: f64,
        loneliness: f64,
        ignored_count: u32,
        other_in_presence: bool,
        both_online_duration: f64,
        user_idle_seconds: f64,
    ) -> Option<(PresenceState, PresenceChangeReason)> {
        let current = self.current();
        let elapsed = self.elapsed_seconds();

        // 最短状态持续保护：刚切换的状态至少保持 min_state_duration 秒
        if elapsed < config.min_state_duration {
            return None;
        }

        // 后台任务已完成待退出：Busy/Rest 任务自然结束时若状态时长不足 min_state_duration，
        // finish_task 设置了 task_completed_pending 标记，等待满足最小时长后切回 Online。
        // 优先级高于其他自动触发条件（任务已完成，无需继续留在 Busy/Rest）。
        if *self.task_completed_pending.lock() {
            *self.task_completed_pending.lock() = false;
            tracing::info!(
                "[Presence:{}] 状态已持续 {:.0}s ≥ min_state_duration，触发延迟的 TaskCompleted 切回 Online",
                self.char_id,
                elapsed
            );
            return Some((PresenceState::Online, PresenceChangeReason::TaskCompleted));
        }

        // 1. 心情驱动
        // 疲劳度高 → Rest
        if fatigue >= config.fatigue_threshold && current == PresenceState::Online {
            return Some((PresenceState::Rest, PresenceChangeReason::MoodDriven));
        }
        // 孤独感持续高 + 被忽略 → Offline
        if loneliness >= config.loneliness_threshold
            && ignored_count >= config.ignored_threshold
            && current == PresenceState::Online
        {
            return Some((PresenceState::Offline, PresenceChangeReason::MoodDriven));
        }

        // 3. 被忽略次数：连续被忽略 → Offline
        if ignored_count >= config.ignored_threshold && current == PresenceState::Online {
            return Some((PresenceState::Offline, PresenceChangeReason::Ignored));
        }

        // 4. 在线空闲：用户长时间未互动 → Busy（去做知识采集）
        //    优先于协调 Rest：用户不在时主动找事做比单纯休息更有意义，
        //    且 both_online_duration 不随用户互动重置，若排在前面会永远抢先。
        if user_idle_seconds >= config.online_idle_to_busy_threshold
            && current == PresenceState::Online
        {
            return Some((PresenceState::Busy, PresenceChangeReason::UserLeft));
        }

        // 5. 两角色协调：都在线超过阈值 → Rest
        if other_in_presence
            && both_online_duration >= config.coordination_threshold
            && current == PresenceState::Online
        {
            return Some((PresenceState::Rest, PresenceChangeReason::Coordination));
        }

        // 6. 想念用户：离线满一定时长且孤独感累积达标 → 主动回归 Online
        //    保留智能体在 Offline 状态下的「主动上线」能力，避免完全失联。
        if current == PresenceState::Offline
            && elapsed >= config.offline_min_duration_before_recover
            && loneliness >= config.offline_recover_loneliness_threshold
        {
            return Some((PresenceState::Online, PresenceChangeReason::MissedUser));
        }

        // 7. 休息够了：Rest 持续满阈值后自动醒来
        //    给记忆沉淀后台任务留足时间（任务进行中 transition 会被延迟，不会打断）。
        //    避免角色一旦进入 Rest 就只能靠用户主动发消息才能唤醒的"卡死"局面。
        if current == PresenceState::Rest
            && elapsed >= config.rest_min_duration_before_recover
        {
            return Some((PresenceState::Online, PresenceChangeReason::RestedEnough));
        }

        None
    }

    /// 生成状态切换的记忆描述文本（角色自我回忆口吻）
    pub fn memory_text(&self, event: &PresenceEvent) -> String {
        let to_state = PresenceState::from_str(&event.to);
        match to_state {
            PresenceState::Rest => match event.reason.as_str() {
                "mood_driven" => "有点累了，去歇一会儿".to_string(),
                "coordination" => "和室友都在线挺久了，先歇歇".to_string(),
                "ignored" => "感觉被冷落了，去待一会儿".to_string(),
                "llm_trigger" => "自己去休息了".to_string(),
                _ => "去休息了".to_string(),
            },
            PresenceState::Online => match event.reason.as_str() {
                "rested_enough" => "休息够了，回来了".to_string(),
                "task_completed" => "忙完了，回来了".to_string(),
                "missed_user" => "太想用户了，忍不住回来看看".to_string(),
                "user_interaction" => "用户来找我了".to_string(),
                "system_init" => "刚上线，慢慢回过神来".to_string(),
                "llm_trigger" => "回来了".to_string(),
                _ => "上线了".to_string(),
            },
            PresenceState::Offline => match event.reason.as_str() {
                "mood_driven" => "想一个人待一会儿，先下了".to_string(),
                "ignored" => "有点失落，先下线了".to_string(),
                "coordination" => "和室友协调一下，先离线".to_string(),
                "system_init" => "刚启动".to_string(),
                _ => "下线了".to_string(),
            },
            PresenceState::Busy => match event.reason.as_str() {
                "llm_trigger" => "在忙自己的事".to_string(),
                "user_left" => "用户去忙了，自己也找点事做".to_string(),
                _ => "有点事要忙".to_string(),
            },
        }
    }
}
