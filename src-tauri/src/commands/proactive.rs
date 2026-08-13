//! 主动交互命令 - 状态查询、手动触发与消息消费

use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{Emitter, Manager, State};

use crate::cross_character::{build_speaker_prefix, generate_cross_stream_id, CrossCharacterRequest, CROSS_CHARACTER_BUS};
use crate::messages::{MessageMeta, MessageSource};
use crate::memory::types::MemoryType;
use crate::proactive::{OnlineCompanion, TickContext};
use crate::state::AppState;
use crate::types::response::ChatMessage;

/// 跨角色发言协调：记录每个角色最近一次主动发言的时间戳。
/// proactive_tick 触发前检查是否有其他角色在冷却窗口内发言过，
/// 若有则跳过本次，避免两个角色同时发言。
static LAST_SPOKEN: Lazy<RwLock<std::collections::HashMap<String, f64>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 查询某角色距上次发言已过多少秒（用于 Public State 暴露）
///
/// 返回 `Some(ago_secs)` 表示该角色曾经发言过；`None` 表示从未发言。
pub fn last_spoken_ago(char_id: &str) -> Option<f64> {
    let spoken = LAST_SPOKEN.read();
    let ts = *spoken.get(char_id)?;
    let now = chrono::Local::now().timestamp() as f64;
    Some((now - ts).max(0.0))
}

/// 记录跨角色对话发言时间戳与文本快照。
///
/// 跨角色对话完成后调用，更新双方的 `LAST_SPOKEN` 和 `LAST_SPOKEN_TEXT`，
/// 让 `CrossCharacterReply` 触发器能感知到室友最近和谁聊过天、聊了什么。
/// 否则非 leader 角色（只发跨角色消息、不发用户消息）的 `LAST_SPOKEN` 永远为空，
/// 导致 leader 切换后对方永远无法触发跨角色回复，形成死锁。
pub fn record_cross_character_spoken(char_id: &str, text: &str) {
    let now_ts = chrono::Local::now().timestamp() as f64;
    {
        let mut last_spoken = LAST_SPOKEN.write();
        last_spoken.insert(char_id.to_string(), now_ts);
    }
    {
        let mut last_spoken_text = LAST_SPOKEN_TEXT.write();
        let truncated: String = text.chars().take(80).collect();
        last_spoken_text.insert(char_id.to_string(), truncated);
    }
    record_speak_history(char_id);
}

/// 仅更新角色的 `LAST_SPOKEN` 时间戳，不覆盖 `LAST_SPOKEN_TEXT`。
///
/// 用于跨角色对话中目标角色非 speak 模式（NonVerbal/Internal/Ignore）：
/// 角色参与了交流（应被 `CrossCharacterReply` 触发器感知），但没有说话（不应覆盖文本快照）。
pub fn touch_last_spoken(char_id: &str) {
    let now_ts = chrono::Local::now().timestamp() as f64;
    let mut last_spoken = LAST_SPOKEN.write();
    last_spoken.insert(char_id.to_string(), now_ts);
    drop(last_spoken);
    record_speak_history(char_id);
}

/// 跨角色发言文本记录：与 `LAST_SPOKEN` 同步写入，保留最近一次发言的文本快照。
/// 供 `CrossCharacterReply` 触发器构造室友最近说了什么的上下文。
static LAST_SPOKEN_TEXT: Lazy<RwLock<std::collections::HashMap<String, String>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 软频率调整值：每角色维护一个 [0.1, 5.0] 浮点乘数，作为发言概率的软调节。
/// 刚发言后调低，随时间回升到基础值；情绪状态可联动调整。
/// 相比硬冷却（CROSS_ROLE_COOLDOWN_SECS），软调整更平滑，避免"冷却一到突然又能说话"的突兀感。
/// value = (调整值, 上次更新时间戳)
static TALK_FREQUENCY_ADJUST: Lazy<RwLock<std::collections::HashMap<String, (f64, f64)>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 软频率调整值上下限
const TALK_FREQ_MIN: f64 = 0.1;
const TALK_FREQ_MAX: f64 = 5.0;

/// 软频率调整值：发言后的即时回落数值
const TALK_FREQ_AFTER_SPOKEN: f64 = 0.3;

/// 软频率调整值：每秒回升速率（约 60s 从 0.3 回升到 1.0）
const TALK_FREQ_RECOVER_PER_SEC: f64 = 0.012;

/// 软频率调整值：默认基础值（未发言过的角色）
const TALK_FREQ_DEFAULT: f64 = 1.0;

/// 获取角色的当前软频率调整值（含时间回升）
pub fn get_talk_frequency_adjust(char_id: &str) -> f64 {
    let now = chrono::Local::now().timestamp() as f64;
    let mut map = TALK_FREQUENCY_ADJUST.write();
    let entry = map.entry(char_id.to_string()).or_insert((
        TALK_FREQ_DEFAULT,
        now,
    ));
    let (value, last_ts) = *entry;
    let elapsed = (now - last_ts).max(0.0);
    let recovered = value + elapsed * TALK_FREQ_RECOVER_PER_SEC;
    let clamped = recovered.clamp(TALK_FREQ_MIN, TALK_FREQ_MAX);
    *entry = (clamped, now);
    clamped
}

/// 设置角色的软频率调整值（立即生效，重置时间戳）
pub fn set_talk_frequency_adjust(char_id: &str, value: f64) {
    let now = chrono::Local::now().timestamp() as f64;
    let clamped = value.clamp(TALK_FREQ_MIN, TALK_FREQ_MAX);
    let mut map = TALK_FREQUENCY_ADJUST.write();
    map.insert(char_id.to_string(), (clamped, now));
}

/// 标记角色刚发言：把软频率调整值压低到 TALK_FREQ_AFTER_SPOKEN
pub fn mark_spoken_frequency(char_id: &str) {
    set_talk_frequency_adjust(char_id, TALK_FREQ_AFTER_SPOKEN);
}

/// 近期发言历史：每角色保留最近 20 条发言时间戳，用于计算发言比例
static RECENT_SPEAK_HISTORY: Lazy<RwLock<std::collections::HashMap<String, Vec<f64>>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 发言历史保留窗口（秒）：超过此时间的记录被清理
const SPEAK_HISTORY_WINDOW_SECS: f64 = 600.0;
/// 每角色保留的最大记录数
const SPEAK_HISTORY_MAX_ENTRIES: usize = 20;

/// 记录一次发言到历史（在 record_cross_character_spoken / touch_last_spoken 中调用）
fn record_speak_history(char_id: &str) {
    let now_ts = chrono::Local::now().timestamp() as f64;
    let mut map = RECENT_SPEAK_HISTORY.write();
    let entry = map.entry(char_id.to_string()).or_default();
    entry.push(now_ts);
    // 清理过期记录
    entry.retain(|&ts| now_ts - ts <= SPEAK_HISTORY_WINDOW_SECS);
    // 限制条数
    if entry.len() > SPEAK_HISTORY_MAX_ENTRIES {
        let drain_count = entry.len() - SPEAK_HISTORY_MAX_ENTRIES;
        entry.drain(..drain_count);
    }
}

/// 计算角色在近 SPEAK_HISTORY_WINDOW_SECS 内的发言占比 [0, 1]
///
/// 返回 0.5 表示双方发言次数均等；> 0.5 表示本角色说得更多。
/// 用于动态调整 reluctance / yield_delay：说得多的一方更克制，说得少的一方更积极。
pub fn compute_speak_ratio(char_id: &str) -> f64 {
    let now_ts = chrono::Local::now().timestamp() as f64;
    let map = RECENT_SPEAK_HISTORY.read();
    let my_count = map
        .get(char_id)
        .map(|v| v.iter().filter(|&&ts| now_ts - ts <= SPEAK_HISTORY_WINDOW_SECS).count())
        .unwrap_or(0);
    let other_count: usize = map
        .iter()
        .filter(|(k, _)| *k != char_id)
        .flat_map(|(_, v)| v.iter())
        .filter(|&&ts| now_ts - ts <= SPEAK_HISTORY_WINDOW_SECS)
        .count();
    let total = my_count + other_count;
    if total == 0 {
        0.5
    } else {
        my_count as f64 / total as f64
    }
}

/// 策略 D：发言优先级仲裁 —— 记录每个角色最近一次"成功产出主动消息"的时间戳。
/// 当两个角色在极短窗口内（5s）同时产出消息时，低优先级角色（priority 数值大）
/// 的消息被抑制，避免用户感知为"同时开口"。
static SPEECH_RESERVATION: Lazy<RwLock<std::collections::HashMap<String, f64>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 策略 D：同时产出的判定窗口（秒）
const SPEECH_COLLISION_WINDOW_SECS: f64 = 5.0;

/// 策略 D：仲裁让步抑制表 —— 记录被仲裁抑制的角色及其抑制起始时间戳。
/// 在 yield_delay_secs 到期前，该角色的所有主动发言请求被直接跳过。
static YIELD_SUPPRESSION: Lazy<RwLock<std::collections::HashMap<String, f64>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 跨角色发言冷却（秒）：一个角色发言后，其他角色在此窗口内不主动发言
const CROSS_ROLE_COOLDOWN_SECS: f64 = 15.0;

/// 行为日志事件节流表：key = "{char_id}_{event_kind}"，value = 上次写入时间戳。
/// 每类事件至少间隔 1 小时再写一次，避免记忆被反复刷屏。
static LAST_BEHAVIOR_LOG: Lazy<RwLock<std::collections::HashMap<String, f64>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 行为日志事件最小间隔（秒）：1 小时
const BEHAVIOR_LOG_INTERVAL_SECS: f64 = 3600.0;

/// 仲裁状态持久化文件名
const ARBITRATION_STATE_FILE: &str = "arbitration_state.json";

/// 仲裁状态有效窗口（秒）：超过此时间的记录视为过期，不恢复
const ARBITRATION_STATE_TTL: f64 = 600.0;

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ArbitrationState {
    last_spoken: std::collections::HashMap<String, f64>,
    last_spoken_text: std::collections::HashMap<String, String>,
    yield_suppression: std::collections::HashMap<String, f64>,
}

fn arbitration_state_path() -> std::path::PathBuf {
    crate::utils::path::get_shared_data_dir().join(ARBITRATION_STATE_FILE)
}

/// 从磁盘恢复仲裁状态。应用启动时调用，过期记录自动丢弃。
pub fn load_arbitration_state() {
    let path = arbitration_state_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let state: ArbitrationState = match serde_json::from_str(&data) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("仲裁状态文件解析失败: {}", e);
            return;
        }
    };
    let now = chrono::Local::now().timestamp() as f64;
    let mut last_spoken = LAST_SPOKEN.write();
    for (id, ts) in &state.last_spoken {
        if now - ts <= ARBITRATION_STATE_TTL {
            last_spoken.insert(id.clone(), *ts);
        }
    }
    drop(last_spoken);
    let mut last_spoken_text = LAST_SPOKEN_TEXT.write();
    for (id, text) in &state.last_spoken_text {
        if state.last_spoken.get(id).map_or(false, |ts| now - ts <= ARBITRATION_STATE_TTL) {
            last_spoken_text.insert(id.clone(), text.clone());
        }
    }
    drop(last_spoken_text);
    let mut yield_sup = YIELD_SUPPRESSION.write();
    for (id, ts) in &state.yield_suppression {
        if now - ts <= ARBITRATION_STATE_TTL {
            yield_sup.insert(id.clone(), *ts);
        }
    }
    tracing::info!("已恢复跨角色仲裁状态");
}

/// 将当前仲裁状态持久化到磁盘。应用关闭或周期性调用。
pub fn persist_arbitration_state() {
    let state = ArbitrationState {
        last_spoken: LAST_SPOKEN.read().clone(),
        last_spoken_text: LAST_SPOKEN_TEXT.read().clone(),
        yield_suppression: YIELD_SUPPRESSION.read().clone(),
    };
    let path = arbitration_state_path();
    match serde_json::to_string_pretty(&state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("仲裁状态持久化失败: {}", e);
            }
        }
        Err(e) => tracing::warn!("仲裁状态序列化失败: {}", e),
    }
}

/// 多角色轮流触发：每角色的 tick 计数（char_id → 累计调用次数）
///
/// 两角色都在线时，每角色隔一次 tick 跳过（奇数次运行、偶数次跳过），
/// 等效每角色每 20s 触发一次（原 10s），总系统 tick 频率从 1/5s 降至 1/10s。
/// 单角色在线时不跳过，保持原 10s 频率。
static CHAR_TICK_COUNT: Lazy<RwLock<std::collections::HashMap<String, u64>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

// ============================================================================
// 焦点租约（Focus Lease）：15s TTL + 检查机制
// ============================================================================
//
// 焦点租约用于在角色正在生成回复 / 处理用户消息 / 跨角色对话期间，
// 暂时屏蔽其他角色的主动打断（主动消息、旁观者插话）。
//
// 与 LAST_SPOKEN / SPEECH_RESERVATION 的区别：
// - LAST_SPOKEN：记录"已说完"的时间戳，用于冷却窗口
// - SPEECH_RESERVATION：声明"打算说"，5s 碰撞窗口
// - FOCUS_LEASE：声明"正在处理 / 正在说"，15s 持有窗口，可主动释放或续期
//
// 三者关系：FOCUS_LEASE 是最外层保护，SPEECH_RESERVATION 是中层意图，
// LAST_SPOKEN 是事后冷却。任一角色在持租约期间，其他角色跳过主动消息。

/// 焦点租约默认 TTL（秒）
const FOCUS_LEASE_TTL_SECS: f64 = 15.0;

/// 焦点租约条目：holder_id → (获取时间戳, TTL)
static FOCUS_LEASE: Lazy<RwLock<std::collections::HashMap<String, (f64, f64)>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

/// 获取某角色当前持有的焦点租约剩余时间（秒）。
///
/// 返回 `Some(remaining)` 表示该角色持有效租约；
/// `None` 表示未持有或已过期。
pub fn focus_lease_remaining(char_id: &str) -> Option<f64> {
    let now = chrono::Local::now().timestamp() as f64;
    let lease = FOCUS_LEASE.read();
    let &(acquired_at, ttl) = lease.get(char_id)?;
    let remaining = ttl - (now - acquired_at);
    if remaining > 0.0 {
        Some(remaining)
    } else {
        None
    }
}

/// 查询当前焦点租约的持有者（任一有效租约）。
///
/// 若多个角色同时持有有效租约（理论上不应出现，但并发场景可能发生），
/// 返回最近一次获取的持有者。
pub fn focus_lease_holder() -> Option<String> {
    let now = chrono::Local::now().timestamp() as f64;
    let lease = FOCUS_LEASE.read();
    let mut latest: Option<(String, f64)> = None;
    for (id, &(acquired_at, ttl)) in lease.iter() {
        if now - acquired_at < ttl {
            match &latest {
                None => latest = Some((id.clone(), acquired_at)),
                Some((_, prev_ts)) if acquired_at > *prev_ts => {
                    latest = Some((id.clone(), acquired_at));
                }
                _ => {}
            }
        }
    }
    latest.map(|(id, _)| id)
}

/// 检查指定角色是否被其他角色的焦点租约阻塞。
///
/// 返回 `Some(other_id)` 表示被另一角色阻塞；
/// `None` 表示无阻塞，可以发起主动消息 / 插话。
pub fn focus_lease_blocked_by_other(char_id: &str) -> Option<String> {
    let now = chrono::Local::now().timestamp() as f64;
    let lease = FOCUS_LEASE.read();
    for (id, &(acquired_at, ttl)) in lease.iter() {
        if id != char_id && now - acquired_at < ttl {
            return Some(id.clone());
        }
    }
    None
}

/// 获取或续期焦点租约。
///
/// 调用时机：角色开始 think / 跨角色回复 / 流式生成开始。
/// 续期场景：长响应（流式 chunk > TTL）周期性续期，避免被误判为释放。
pub fn acquire_focus_lease(char_id: &str) {
    let now = chrono::Local::now().timestamp() as f64;
    FOCUS_LEASE
        .write()
        .insert(char_id.to_string(), (now, FOCUS_LEASE_TTL_SECS));
}

/// 续期焦点租约（重置 TTL），仅在已持有时生效。
pub fn renew_focus_lease(char_id: &str) {
    let now = chrono::Local::now().timestamp() as f64;
    let mut lease = FOCUS_LEASE.write();
    if lease.contains_key(char_id) {
        lease.insert(char_id.to_string(), (now, FOCUS_LEASE_TTL_SECS));
    }
}

/// 主动释放焦点租约。
///
/// 调用时机：think 完成 / 流式结束 / 异常退出。
/// 若未持有则无操作。
pub fn release_focus_lease(char_id: &str) {
    FOCUS_LEASE.write().remove(char_id);
}

/// 清理所有过期的焦点租约条目（周期性调用，避免 HashMap 无限膨胀）。
pub fn sweep_expired_focus_leases() {
    let now = chrono::Local::now().timestamp() as f64;
    FOCUS_LEASE
        .write()
        .retain(|_, (acquired_at, ttl)| (now - *acquired_at) < *ttl);
}

/// 焦点租约守卫：RAII 模式，drop 时自动释放租约。
pub struct FocusLeaseGuard {
    char_id: Option<String>,
}

impl FocusLeaseGuard {
    /// 获取焦点租约并返回守卫。drop 时自动释放。
    pub fn acquire(char_id: impl Into<String>) -> Self {
        let id = char_id.into();
        acquire_focus_lease(&id);
        Self { char_id: Some(id) }
    }

    /// 提前释放租约（等同于 drop，但语义更清晰）。
    pub fn release(mut self) {
        if let Some(id) = self.char_id.take() {
            release_focus_lease(&id);
        }
    }
}

impl Drop for FocusLeaseGuard {
    fn drop(&mut self) {
        if let Some(id) = self.char_id.take() {
            release_focus_lease(&id);
        }
    }
}

/// 检查某类事件是否应当写入（节流），命中则登记时间戳。
fn should_log_behavior(char_id: &str, kind: &str, now: f64) -> bool {
    let key = format!("{}_{}", char_id, kind);
    let can_log = {
        let m = LAST_BEHAVIOR_LOG.read();
        !m.contains_key(&key) || now - m[&key] >= BEHAVIOR_LOG_INTERVAL_SECS
    };
    if can_log {
        LAST_BEHAVIOR_LOG.write().insert(key, now);
    }
    can_log
}

/// 获取主动交互状态
#[tauri::command]
pub fn get_proactive_status(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    Ok(brain.proactive.get_status())
}

/// 启动主动交互
#[tauri::command]
pub fn start_proactive(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain.start_proactive();
    Ok(())
}

/// 停止主动交互
#[tauri::command]
pub fn stop_proactive(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain.stop_proactive();
    Ok(())
}

/// 单次主动交互 tick（前端每 10 秒调用一次）
#[tauri::command]
pub async fn proactive_tick(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
    app: tauri::AppHandle,
    context: Value,
) -> Result<Value, String> {
    // 恢复出厂设置 / 记忆向量重建进行中：立即跳过，避免产生新数据干扰
    if state.is_factory_reset_in_progress() || state.is_rebuild_in_progress() {
        return Ok(json!({
            "produced": false,
            "messages": [],
            "skipped": true,
            "reason": "busy",
        }));
    }

    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let instance = state.get_character(character_id.as_deref())?;
    let brain = instance.brain.clone();
    let state_arc = state.inner().clone();

    // 多角色轮流触发：两角色都在线时每角色隔一次 tick 跳过（奇数次运行、偶数次跳过）。
    // 等效每角色每 20s 触发一次（原 10s），与前端错峰结合，总系统 tick 频率从 1/5s 降至 1/10s。
    // 单角色在线时不跳过，保持原 10s 频率。
    {
        let other_online = state
            .characters
            .read()
            .iter()
            .any(|(id, inst)| id.as_str() != char_id.as_str() && inst.brain.presence.is_in_presence());
        if other_online {
            let mut counts = CHAR_TICK_COUNT.write();
            let count = counts.entry(char_id.clone()).or_insert(0);
            *count += 1;
            if *count % 2 == 0 {
                tracing::debug!(
                    "[Proactive] {} 跳过：多角色轮流触发（count={}）",
                    char_id,
                    count
                );
                return Ok(json!({
                    "produced": false,
                    "messages": [],
                    "skipped": true,
                }));
            }
        }
    }

    // 优先使用系统级空闲时间（跨应用，权威）；失败时回退到前端 webview 信号
    let system_idle_seconds = crate::utils::get_system_idle_seconds().unwrap_or_else(|| {
        context
            .get("idle_seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    });
    let mut ctx = TickContext {
        now: chrono::Local::now().timestamp() as f64,
        idle_seconds: system_idle_seconds,
        away_seconds: context
            .get("away_seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        // 在场状态以系统级空闲为准（< 300s 视为在场），失败时回退到前端判定
        user_present: crate::utils::get_system_idle_seconds()
            .map(|idle| idle < 300.0)
            .unwrap_or_else(|| {
                context
                    .get("user_present")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
            }),
        interaction_count_today: context
            .get("interaction_count_today")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        active_window: context
            .get("active_window")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        window_changed: context
            .get("window_changed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        last_topic_relevant: context
            .get("last_topic_relevant")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        has_relevant_memory: context
            .get("has_relevant_memory")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        drag_distance: context
            .get("drag_distance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        user_emotion: context
            .get("user_emotion")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        // SelfState 防打扰决策：quiet_mode/今日主动次数上限/被忽略接近阈值/Rest/Offline 任一成立则跳过主动消息
        lay_low: brain.self_state.snapshot().should_lay_low(),
        // 用户是否正在与任意角色活跃对话：抑制打断性主动消息
        // 判定标准：会话 Active 且用户最近 90s 内有真实活跃（系统级空闲 < 90s）。
        // 之前仅判断 is_any_user_session_active()，但会话 Active 状态会持续到超时关闭（默认 30 分钟），
        // 导致用户只是发了一条消息就离开后，长达 30 分钟内 CrossCharacterReply 被误判为"用户正在聊天"而跳过，
        // 两个角色之间的跨角色交流几乎不发生。
        is_user_chatting: crate::conversation::CONVERSATION_MANAGER.is_any_user_session_active()
            && system_idle_seconds < 90.0,
        is_speaking_leader: false,
    };

    // ── World Entity State 桥接：把 idle_seconds 翻译为用户在场/离开信号 ──
    {
        let idle = ctx.idle_seconds;
        let away_threshold = brain.config.proactive.away_threshold_seconds as f64;
        if idle < 60.0 {
            let _ = brain.world_state.mark_user_present();
        } else if idle > away_threshold {
            brain.world_state.mark_user_away();
        }
    }

    // 跨角色发言冷却（策略 D：按被阻塞角色的 reluctance 乘数差异化）：
    // 检查其他角色是否在冷却窗口内发言过。
    // 冷却时长 = CROSS_ROLE_COOLDOWN_SECS × 动态 reluctance。
    // 动态调整：近 10 分钟发言比例 > 0.5（说得多）时 reluctance 上浮，让对方有更多机会接话；
    // 比例 < 0.5（说得少）时 reluctance 下调，让自己更容易开口。
    {
        let now_ts = chrono::Local::now().timestamp() as f64;
        let last_spoken = LAST_SPOKEN.read();
        let behavior = crate::character_behavior::get_behavior(&char_id);
        let speak_ratio = compute_speak_ratio(&char_id);
        // ratio 0.5 → ×1.0（基准），ratio 1.0 → ×1.5（更克制），ratio 0.0 → ×0.6（更积极）
        let ratio_mult = 0.6 + (speak_ratio - 0.5).clamp(-0.5, 0.5) * 1.8;
        let effective_cooldown = CROSS_ROLE_COOLDOWN_SECS * behavior.arbitration.reluctance * ratio_mult;
        let blocked_by = last_spoken.iter().find(|(lbl, &ts)| {
            *lbl != &char_id && (now_ts - ts) < effective_cooldown
        });
        if let Some((other_lbl, ts)) = blocked_by {
            tracing::debug!(
                "[Proactive] {} 跳过：{} 在 {:.1}s 前刚发言（冷却窗口 {:.0}s, ratio={:.2}）",
                char_id,
                other_lbl,
                now_ts - ts,
                effective_cooldown,
                speak_ratio
            );
            return Ok(json!({
                "produced": false,
                "messages": [],
                "skipped": true,
            }));
        }
    }

    // 策略 D：仲裁让步延迟 —— 被仲裁抑制的角色在 yield_delay_secs 内不得发言
    // 动态调整：发言比例高的一方让步延迟更长，给对方更多发言空间
    {
        let now_ts = chrono::Local::now().timestamp() as f64;
        let suppression = YIELD_SUPPRESSION.read();
        if let Some(&suppressed_at) = suppression.get(&char_id) {
            let behavior = crate::character_behavior::get_behavior(&char_id);
            let speak_ratio = compute_speak_ratio(&char_id);
            // ratio 0.5 → ×1.0，ratio 1.0 → ×1.5，ratio 0.0 → ×0.5
            let ratio_mult = 0.5 + (speak_ratio - 0.5).clamp(-0.5, 0.5) * 2.0;
            let delay = behavior.arbitration.yield_delay_secs * ratio_mult;
            if delay > 0.0 && (now_ts - suppressed_at) < delay {
                tracing::debug!(
                    "[Proactive] {} 跳过：仲裁让步延迟中（{:.0}s / {:.0}s, ratio={:.2}）",
                    char_id,
                    now_ts - suppressed_at,
                    delay,
                    speak_ratio
                );
                return Ok(json!({
                    "produced": false,
                    "messages": [],
                    "skipped": true,
                }));
            }
        }
    }

    // 焦点租约检查：其他角色正在 think / 流式生成 / 跨角色对话期间，跳过主动消息。
    // 这是比 LAST_SPOKEN 更即时的保护层，覆盖"正在说"而非"已说完"。
    if let Some(other_id) = focus_lease_blocked_by_other(&char_id) {
        tracing::debug!(
            "[Proactive] {} 跳过：{} 正持有焦点租约",
            char_id,
            other_id
        );
        return Ok(json!({
            "produced": false,
            "messages": [],
            "skipped": true,
            "reason": "focus_lease_held",
        }));
    }

    // 清理超时的会话（Cooling 状态超过 30 秒的自动关闭）
    // 每 10 秒跑一次，轻量操作
    {
        let closed_convs = crate::conversation::CONVERSATION_MANAGER.sweep_cooling();
        if !closed_convs.is_empty() {
            let ids: Vec<&str> = closed_convs.iter().map(|c| c.id.as_str()).collect();
            tracing::debug!(
                "[Proactive] 清理了 {} 个超时的跨角色会话: {:?}",
                closed_convs.len(),
                ids
            );
            // Open Loop 检测：冷却关闭的会话如果最近连续性分数高，标记为待续话题
            for conv in &closed_convs {
                maybe_mark_open_loop(conv, &brain).await;
            }
        }
        // 清理已关闭超过 1 小时的非用户会话，防止 HashMap 无限膨胀
        crate::conversation::CONVERSATION_MANAGER.purge_stale();
    }

    // 清理超时的 User↔Agent 会话（用户长时间未发言 → close(Timeout)）
    // 阈值 30 分钟：与 long_idle 行为日志阈值对齐
    {
        let closed = crate::conversation::CONVERSATION_MANAGER
            .sweep_user_session_timeouts(1800.0);
        if !closed.is_empty() {
            for (cid, reason, conv) in &closed {
                tracing::debug!(
                    "[Proactive] User↔{} 会话因 {} 关闭",
                    cid,
                    reason.as_str()
                );
                // Open Loop 检测：超时关闭的会话如果最近连续性分数高，标记为待续话题
                // 用户主动告别（GoodNight/GoodBye）不触发——但此处都是 Timeout/NoResponse
                maybe_mark_open_loop(conv, &brain).await;
            }
        }
    }

    // 会话状态检查：若 User↔Agent 会话已关闭（GoodNight/NoResponse/Timeout 等），
    // 跳过本次主动搭话。只有会话 Active/Cooling 或无会话时才允许主动。
    // 注意：close_reason 为 GoodNight 时，整个睡眠时段都不应主动搭话。
    if crate::conversation::CONVERSATION_MANAGER.is_user_session_closed(&char_id) {
        let reason = crate::conversation::CONVERSATION_MANAGER
            .user_session_close_reason(&char_id);
        // GoodNight/NoResponse/Timeout → 跳过主动搭话
        // 其余原因（Natural/SwitchTopic）允许主动开新话题
        let skip = matches!(
            reason,
            Some(crate::conversation::CloseReason::GoodNight)
                | Some(crate::conversation::CloseReason::NoResponse)
                | Some(crate::conversation::CloseReason::Timeout)
        );
        if skip {
            tracing::debug!(
                "[Proactive] {} 跳过主动搭话：User 会话已关闭（{:?}）",
                char_id,
                reason
            );
            return Ok(json!({
                "produced": false,
                "messages": [],
                "skipped": true,
            }));
        }
    }

    // 注入流式推送回调：LLM 每产生一个 text 增量即 emit `proactive:chunk` 事件
    // 前端订阅 proactive:chunk 即可实时流式显示气泡，无需等完整响应
    let app_for_emitter = app.clone();
    let cid_for_emitter = char_id.clone();
    let emitter: crate::pipeline::steps::generation::StreamEmitter = Arc::new(move |chunk: &str| {
        let _ = app_for_emitter.emit("proactive:chunk", json!({ "text": chunk, "character_id": &cid_for_emitter }));
    });
    brain.proactive.set_stream_emitter(Some(emitter));

    // ── 在场状态自动触发检查 ──
    // 每次 proactive tick 检查四个条件（心情/被忽略/两角色协调/想念用户），
    // 满足则自动切换状态并写入行为日志记忆。
    {
        let presence_config = crate::presence::PresenceConfig::default();
        let mood = brain.psychology.compute_mood();
        let emotion = brain.psychology.emotion();
        let fatigue = mood.fatigue;
        let loneliness = emotion.loneliness;

        // 从 ProactiveOrchestrator 读取被忽略次数
        let ignored_count = brain.proactive.get_ignored_count();

        // 检查其他角色是否在场 + 两角色同时在线时长
        let characters = state.characters.read().clone();
        let mut other_in_presence = false;
        let mut other_online_since = f64::MAX;
        for (other_id, other_instance) in characters.iter() {
            if other_id == &char_id {
                continue;
            }
            if other_instance.brain.presence.is_in_presence() {
                other_in_presence = true;
                let other_since = other_instance.brain.presence.since();
                if other_since < other_online_since {
                    other_online_since = other_since;
                }
            }
        }
        // 两角色同时在线时长 = min(自己的 since, 对方的 since) 到现在的差
        let both_online_duration = if other_in_presence {
            let now = chrono::Local::now().timestamp() as f64;
            let my_since = brain.presence.since();
            (now - my_since.min(other_online_since)).max(0.0)
        } else {
            0.0
        };

        // 用户空闲时长（用于 Online→Busy 自动触发）
        let user_idle_seconds = brain.presence.user_idle_seconds();

        if let Some((target, reason)) = brain.presence.check_auto_triggers(
            &presence_config,
            fatigue,
            loneliness,
            ignored_count,
            other_in_presence,
            both_online_duration,
            user_idle_seconds,
        ) {
            // 切换到 Rest/Offline 前，先生成告别语告知用户
            let farewell_text = if matches!(
                target,
                crate::presence::PresenceState::Rest | crate::presence::PresenceState::Offline
            ) {
                brain.generate_farewell_greeting(target, reason.clone()).await
            } else {
                None
            };

            // 告别语写入对话历史 + MemoryManager（作为 dialogue 节点出现在时间轴右侧）
            if let Some(ref farewell) = farewell_text {
                let mut m = ChatMessage::assistant(farewell);
                m.meta = Some(MessageMeta::new(MessageSource::Assistant).with_channel("proactive"));
                brain.dialogue.add_message(m);
                let memory = brain.memory.clone();
                let cid = char_id.clone();
                let text = farewell.clone();
                tokio::spawn(async move {
                    let meta = serde_json::json!({
                        "channel": "proactive",
                        "speaker": cid,
                        "listener": "user",
                        "perspective": "speaker",
                        "knowledge_source": "direct",
                    });
                    let _ = memory
                        .add_memory_with_metadata(
                            &text,
                            crate::memory::types::MemoryType::CasualConversation,
                            0.3,
                            vec![
                                "assistant".to_string(),
                                "proactive".to_string(),
                                "dialogue_turn".to_string(),
                                "farewell".to_string(),
                            ],
                            meta,
                        )
                        .await;
                });
            }

            if let Some(event) = brain.presence.transition(target, reason) {
                // 内心独白信号
                let to_state = crate::presence::PresenceState::from_str(&event.to);
                if matches!(to_state, crate::presence::PresenceState::Rest | crate::presence::PresenceState::Offline) {
                    brain.proactive.signal_going_to_rest(match to_state {
                        crate::presence::PresenceState::Rest => "累了想休息一下",
                        _ => "有点想离线独处一会儿",
                    });
                    state.leader_coordinator.resign(&char_id);
                }

                let memory_text = brain.presence.memory_text(&event);
                let eid = char_id.clone();
                // 在场状态切换是世界事件，注册到统一事件账本，双角色各自感知
                let now_ts = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
                crate::memory::unified_event_ledger::register_world_event(
                    "presence_change",
                    &memory_text,
                    vec!["behavior".to_string(), "presence_log".to_string(), format!("from:{}", event.from), format!("to:{}", event.to)],
                    now_ts,
                    Some(&char_id),
                );

                // 状态切换事件写入 MemoryManager（带 presence_log 标签，时间轴左侧显示为 episode 节点）
                {
                    let memory = brain.memory.clone();
                    let cid = char_id.clone();
                    let mem_text = memory_text.clone();
                    let to_state = event.to.clone();
                    let from_state = event.from.clone();
                    tokio::spawn(async move {
                        let meta = serde_json::json!({
                            "kind": "presence_log",
                            "character_id": cid,
                            "from": from_state,
                            "to": to_state,
                        });
                        let _ = memory
                            .add_memory_with_metadata(
                                &mem_text,
                                crate::memory::types::MemoryType::ShortTerm,
                                0.4,
                                vec![
                                    "presence_log".to_string(),
                                    "behavior".to_string(),
                                    format!("from:{}", from_state),
                                    format!("to:{}", to_state),
                                ],
                                meta,
                            )
                            .await;
                    });
                }

                let _ = app.emit(
                    "presence:changed",
                    json!({
                        "character_id": &char_id,
                        "from": event.from,
                        "to": event.to,
                        "reason": event.reason,
                        "farewell_text": farewell_text,
                    }),
                );

                // 后端联动 Live2D 窗口可见性
                // 有告别语时跳过后端 hide（让前端先显示气泡再延迟隐藏）
                if farewell_text.is_none() {
                    if let Some(win) = app.get_webview_window(&char_id) {
                        let to_state = crate::presence::PresenceState::from_str(&event.to);
                        let from_state = crate::presence::PresenceState::from_str(&event.from);
                        if matches!(to_state, crate::presence::PresenceState::Offline) {
                            let _ = win.hide();
                            tracing::info!(
                                "[Presence:{}] 后端联动 hide 窗口（Offline）",
                                eid
                            );
                        } else if matches!(from_state, crate::presence::PresenceState::Offline) {
                            let _ = win.show();
                            let _ = win.set_focus();
                            tracing::info!(
                                "[Presence:{}] 后端联动 show 窗口（从 Offline 恢复）",
                                eid
                            );
                        }
                    }
                } else if let Some(win) = app.get_webview_window(&char_id) {
                    // 有告别语：仅处理从 Offline 恢复的 show，hide 交给前端延迟执行
                    let from_state = crate::presence::PresenceState::from_str(&event.from);
                    if matches!(from_state, crate::presence::PresenceState::Offline) {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }

                tracing::info!("[Presence:{}] 自动触发: {} → {}", eid, event.from, event.to);
            }
        }
    }

    // ── 行为日志事件：心情显著变化 / 长时间无互动 / 被忽略 ──
    // 这些是程序确定的事实（World Event），注册到统一事件账本，双角色各自感知。
    // 不再写入 MemoryManager —— Memory 层只保留 AI 主观认为值得记的事。
    // 每类事件按 char_id 节流，至少间隔 1 小时再写一次。
    {
        let now = chrono::Local::now().timestamp() as f64;
        let snap = brain.psychology.snapshot();
        let idle_secs = snap.secs_since_last_interaction();
        let ignored_count = brain.proactive.get_ignored_count();

        // 1. 心情显著变化：取 events 末尾两笔做对比
        if snap.events.len() >= 2 {
            let prev = &snap.events[snap.events.len() - 2].emotion_after;
            let curr = &snap.events[snap.events.len() - 1].emotion_after;
            let mood_shift_text: Option<String> = if prev.joy >= 0.6 && curr.joy <= 0.2 {
                Some(format!("心情事件：从开心变得低落（joy {:.2}→{:.2}）", prev.joy, curr.joy))
            } else if prev.sadness <= 0.3 && curr.sadness >= 0.7 {
                Some(format!("心情事件：突然感到难过（sadness {:.2}→{:.2}）", prev.sadness, curr.sadness))
            } else if prev.anger <= 0.3 && curr.anger >= 0.7 {
                Some(format!("心情事件：变得生气（anger {:.2}→{:.2}）", prev.anger, curr.anger))
            } else {
                None
            };
            if let Some(text) = mood_shift_text {
                if should_log_behavior(&char_id, "mood_event", now) {
                    crate::memory::unified_event_ledger::register_world_event(
                        "mood_shift",
                        &text,
                        vec!["behavior".to_string(), "mood_event".to_string()],
                        now,
                        Some(&char_id),
                    );
                }
            }
        }

        // 2. 长时间无互动：距上次互动 > 30 分钟
        if idle_secs >= 1800.0 && should_log_behavior(&char_id, "long_idle", now) {
            let minutes = (idle_secs / 60.0).round() as u32;
            let text = format!("独自等待：已经 {} 分钟没和用户说话了", minutes);
            crate::memory::unified_event_ledger::register_world_event(
                "idle_timeout",
                &text,
                vec!["behavior".to_string(), "long_idle".to_string(), format!("duration:{}", minutes)],
                now,
                Some(&char_id),
            );
        }

        // 3. 被忽略：连续主动搭话被忽略 >= 3 次
        if ignored_count >= 3 && should_log_behavior(&char_id, "quiet_mode", now) {
            let text = format!("被忽略了：连续 {} 次主动搭话都没得到回应", ignored_count);
            crate::memory::unified_event_ledger::register_world_event(
                "ignored_message",
                &text,
                vec!["behavior".to_string(), "quiet_mode".to_string(), "ignored".to_string(), format!("count:{}", ignored_count)],
                now,
                Some(&char_id),
            );
        }
    }

    // 休息/离线/忙碌状态：不主动发话
    // - Rest = 午睡不主动；Offline = 完全失联；Busy = 在场但不主动（在做知识采集等后台任务）
    // 但前面的自动触发检查与行为日志仍正常执行，状态不会被冻结。
    let current_presence = brain.presence.current();
    if matches!(
        current_presence,
        crate::presence::PresenceState::Rest
            | crate::presence::PresenceState::Offline
            | crate::presence::PresenceState::Busy
    ) {
        return Ok(json!({
            "produced": false,
            "messages": [],
            "presence": current_presence.as_str(),
        }));
    }

    // 刷新室友快照：让 ProactiveOrchestrator 在触发器决策和 prompt 构造时
    // 能感知到室友在线状态 + 最近发言内容。
    // CrossCharacterReply 触发器会基于此快照判断是否回应。
    {
        let now_ts = chrono::Local::now().timestamp() as f64;
        let characters = state.characters.read();
        let last_spoken_guard = LAST_SPOKEN.read();
        let last_spoken_text_guard = LAST_SPOKEN_TEXT.read();
        // 只有一个室友：找除自己外第一个在线角色
        let companion = characters
            .iter()
            .find(|(id, inst)| *id != &char_id && *inst.online.read())
            .map(|(other_id, other_instance)| {
                let last_spoke_secs_ago = last_spoken_guard
                    .get(other_id)
                    .map(|ts| (now_ts - ts).max(0.0));
                let last_spoke_text = last_spoken_text_guard
                    .get(other_id)
                    .cloned();
                OnlineCompanion {
                    id: other_id.clone(),
                    name: other_instance.name.clone(),
                    last_spoke_secs_ago,
                    last_spoke_text,
                }
            });
        drop(characters);
        brain.proactive.update_companions_snapshot(companion);
    }

    // Leader 选举：只有持有发言权的角色才执行触发器评估与消息投递。
    // 非 leader 仍跑 brain.proactive_tick 做状态维护（homeostasis / 内心独白 / 世界事件），
    // 但 tick 内部跳过 evaluate_and_fire_triggers。
    let is_speaking_leader = {
        let is_online = *instance.online.read();
        let is_present = !matches!(
            brain.presence.current(),
            crate::presence::PresenceState::Rest
                | crate::presence::PresenceState::Offline
                | crate::presence::PresenceState::Busy
        );
        let is_active = *state.active_character_id.read() == char_id;
        if !is_present {
            state.leader_coordinator.resign(&char_id);
        }
        state.leader_coordinator.try_acquire_or_renew(
            &char_id,
            is_online,
            is_present,
            is_active,
        )
    };
    ctx.is_speaking_leader = is_speaking_leader;

    // 非 leader：跳过发言预占与 turn 登记，直接执行状态维护 tick
    // 但仍处理 CrossCharacterReply 产出的跨角色消息（发给室友，不发给用户）
    if !is_speaking_leader {
        let _ = brain
            .proactive_tick(&ctx)
            .await
            .map_err(|e| e.to_string())?;
        brain.proactive.set_stream_emitter(None);
        let all_messages = brain.drain_proactive_messages();
        // 分离跨角色消息（非 leader 也允许产出 CrossCharacterReply）
        let cross_messages: Vec<_> = all_messages
            .into_iter()
            .filter(|m| m.trigger == "cross_character_reply")
            .collect();
        if !cross_messages.is_empty() {
            let app_clone = app.clone();
            let char_id_clone = char_id.clone();
            tokio::spawn(async move {
                deliver_cross_character_messages(&app_clone, &state_arc, &char_id_clone, cross_messages).await;
            });
        }
        return Ok(json!({
            "produced": false,
            "messages": [],
            "skipped": true,
            "reason": "not_leader",
        }));
    }

    // 策略 D 预占：在昂贵的 LLM tick 之前先声明发言意图，防止两角色并行 tick 同时通过仲裁。
    // 若对方已预占且优先级更高，本次直接跳过；否则写入自己的预占时间戳。
    {
        let now_ts = chrono::Local::now().timestamp() as f64;
        let my_behavior = crate::character_behavior::get_behavior(&char_id);
        let reservation = SPEECH_RESERVATION.read();
        let conflict = reservation.iter().find(|(other_id, &ts)| {
            *other_id != &char_id && (now_ts - ts) < SPEECH_COLLISION_WINDOW_SECS
        });
        if let Some((other_id, _)) = conflict {
            let other_behavior = crate::character_behavior::get_behavior(other_id);
            if other_behavior.arbitration.priority < my_behavior.arbitration.priority {
                tracing::debug!(
                    "[Proactive:{}] 预占冲突：{} 已声明发言意图且优先级更高，跳过",
                    char_id,
                    other_id
                );
                return Ok(json!({
                    "produced": false,
                    "messages": [],
                    "skipped": true,
                }));
            }
        }
        drop(reservation);
        SPEECH_RESERVATION.write().insert(char_id.clone(), now_ts);
    }

    let _proactive_guard = match state.session_coordinator.try_enter_proactive_turn(
        &char_id,
        &brain.memory,
        &brain.dialogue,
    ) {
        Some(g) => g,
        None => {
            SPEECH_RESERVATION.write().remove(&char_id);
            return Ok(json!({
                "produced": false,
                "messages": [],
                "skipped": true,
                "reason": "user_input_pending",
            }));
        }
    };

    let produced = brain
        .proactive_tick(&ctx)
        .await
        .map_err(|e| e.to_string())?;

    // 清理流式回调
    brain.proactive.set_stream_emitter(None);

    let all_messages = brain.drain_proactive_messages();
    drop(_proactive_guard);

    // 分离跨角色消息（不发给用户，而是通过 CrossCharacterBus 发给室友）
    // 和普通主动消息（走原有路径：对话历史/记忆/LAST_SPOKEN）
    let trigger_name_cross = "cross_character_reply";
    let mut user_messages: Vec<crate::proactive::ProactiveAction> = Vec::new();
    let mut cross_messages: Vec<crate::proactive::ProactiveAction> = Vec::new();
    for msg in all_messages {
        if msg.trigger == trigger_name_cross {
            cross_messages.push(msg);
        } else {
            user_messages.push(msg);
        }
    }

    // 策略 D：优先级仲裁（后校验）—— 两角色在极短窗口内同时产出时，
    // 预占时间戳更晚且优先级更低的角色让位，并记录 yield 抑制。
    let mut produced = produced;
    if produced && !user_messages.is_empty() {
        let now_ts = chrono::Local::now().timestamp() as f64;
        let my_behavior = crate::character_behavior::get_behavior(&char_id);
        let reservation = SPEECH_RESERVATION.read();
        let collision = reservation.iter().find(|(other_id, &ts)| {
            *other_id != &char_id && (now_ts - ts) < SPEECH_COLLISION_WINDOW_SECS
        });
        if let Some((other_id, &other_ts)) = collision {
            let other_behavior = crate::character_behavior::get_behavior(other_id);
            // 对方优先级更高，或优先级相同但对方预占更早 → 本角色让位
            let should_yield = other_behavior.arbitration.priority < my_behavior.arbitration.priority
                || (other_behavior.arbitration.priority == my_behavior.arbitration.priority
                    && other_ts < now_ts);
            if should_yield {
                tracing::info!(
                    "[Proactive:{}] 策略D仲裁让位：{} 优先级更高或预占更早，抑制本次发言",
                    char_id,
                    other_id
                );
                user_messages.clear();
                produced = false;
                // 记录让步抑制时间戳，yield_delay_secs 内不再尝试
                if my_behavior.arbitration.yield_delay_secs > 0.0 {
                    YIELD_SUPPRESSION.write().insert(char_id.clone(), now_ts);
                }
            }
        }
        drop(reservation);
    }

    // 未产出消息：释放预占，避免无谓阻塞对方
    if !produced || user_messages.is_empty() {
        SPEECH_RESERVATION.write().remove(&char_id);
    }

    // 发言成功：更新跨角色发言时间戳，广播事件让其他角色感知
    // 跨角色对话的 LAST_SPOKEN 更新在 CROSS_CHARACTER_BUS.send 成功后单独处理
    if produced && !user_messages.is_empty() {
        let now_ts = chrono::Local::now().timestamp() as f64;
        {
            let mut last_spoken = LAST_SPOKEN.write();
            last_spoken.insert(char_id.clone(), now_ts);
        }
        {
            let mut last_spoken_text = LAST_SPOKEN_TEXT.write();
            let text: String = user_messages
                .iter()
                .map(|m| m.content.clone())
                .collect::<Vec<_>>()
                .join(" | ");
            let truncated: String = text.chars().take(80).collect();
            last_spoken_text.insert(char_id.clone(), truncated);
        }
        mark_spoken_frequency(&char_id);
        // 记录今日主动发起次数 + 持久化（防打扰决策依据）
        brain.self_state.record_proactive_initiative();
        if let Err(e) = brain.self_state.persist() {
            tracing::warn!("[SelfState:{}] 持久化失败: {}", char_id, e);
        }
        let _ = app.emit(
            "proactive:spoken",
            json!({ "character_id": &char_id, "timestamp": now_ts }),
        );
        persist_arbitration_state();
    }

    // 主动行为按 delivery_channel 派发到对应渠道
    // - Bubble：写入 dialogue channel="proactive"（桌宠气泡路径，被 wechat 过滤排除）
    // - ChatWindow：写入 dialogue channel="wechat" + emit chat:assistant_message（前端 ChatWindow 立即追加气泡）
    // 用户不在场时，全部强制改为 ChatWindow（见下方覆盖逻辑），避免气泡无人看到
    // Share 类强制 value_score 门槛，未达标跳过发送（待分享池后续阶段实现）

    // 播放边界感知：TTS 正在播放时跳过本轮投递，避免音频冲突
    // 消息已在 pending_messages 中，下个 tick 会重新 drain
    if produced && !user_messages.is_empty() && state.playback_gate.is_playing() {
        tracing::info!(
            "[Proactive:{}] TTS 播放中，推迟主动消息投递（{} 条）",
            char_id,
            user_messages.len()
        );
        brain.proactive.requeue_messages(user_messages.clone());
        return Ok(json!({
            "produced": false,
            "messages": [],
            "skipped": true,
            "reason": "tts_playing",
        }));
    }
    // 用户不在场时，主动消息全部强制走 ChatWindow（微信面板），
    // 避免桌宠气泡无人看到。覆盖写入 user_messages 让下游投递、记忆、旁观过滤、
    // 返回前端字段统一使用生效渠道。
    if !ctx.user_present {
        for action in &mut user_messages {
            action.delivery_channel = crate::proactive::DeliveryChannel::ChatWindow;
        }
    }

    for action in &user_messages {
        if matches!(action.content_type, crate::proactive::ContentType::Share) {
            let score = action.value_score.unwrap_or(0.0);
            if score < crate::proactive::SHARE_VALUE_THRESHOLD {
                tracing::info!(
                    "[Proactive:{}] Share 类消息 value_score={:.2} < {:.2}, 跳过发送: trigger={}",
                    char_id,
                    score,
                    crate::proactive::SHARE_VALUE_THRESHOLD,
                    action.trigger
                );
                continue;
            }
        }

        if action.content.trim().is_empty() {
            tracing::info!(
                "[Proactive:{}] 空文本消息，跳过发送: trigger={}",
                char_id,
                action.trigger
            );
            continue;
        }

        let clean_content = crate::utils::strip_markdown_syntax(&action.content);
        let channel_str = match action.delivery_channel {
            crate::proactive::DeliveryChannel::Bubble => "proactive",
            crate::proactive::DeliveryChannel::ChatWindow => "wechat",
        };
        let mut m = ChatMessage::assistant(&clean_content);
        m.meta = Some(MessageMeta::new(MessageSource::Assistant).with_channel(channel_str));
        brain.dialogue.add_message(m);

        // ChatWindow 渠道额外 emit chat:assistant_message，让前端 ChatWindow 立即追加气泡
        // 复用现有事件（todo_tools.rs 已用作 Scheduler 主动推消息到 ChatWindow 的入口）
        if matches!(action.delivery_channel, crate::proactive::DeliveryChannel::ChatWindow) {
            let _ = app.emit(
                "chat:assistant_message",
                json!({
                    "character_id": &char_id,
                    "content": &clean_content,
                    "channel": "wechat",
                }),
            );

            // 若 chat 窗口（微信主界面）未可见，emit 消息横幅提示用户
            let need_banner = match app.get_webview_window("chat") {
                Some(win) => !win.is_visible().ok().unwrap_or(false),
                None => true,
            };
            if need_banner {
                let preview: String = clean_content.chars().take(60).collect();
                let _ = app.emit(
                    "wechat:message_banner",
                    json!({
                        "character_id": &char_id,
                        "preview": preview,
                        "kind": "proactive",
                        "timestamp": chrono::Local::now().timestamp() as f64,
                    }),
                );
                // 同步压入远程通知队列，供手机端 toast 轮询展示
                crate::remote::push_toast(
                    "proactive",
                    "智能体消息",
                    &preview,
                    &char_id,
                    json!({ "kind": "proactive" }),
                );
            }
        }
    }

    // 主动消息存入记忆系统（fire-and-forget，不阻塞 tick 返回）
    // 让记忆系统对主动对话有感知，后续 RAG 检索可召回
    if !user_messages.is_empty() {
        let memory = brain.memory.clone();
        let msgs_clone = user_messages.clone();
        let char_id_for_mem = char_id.clone();
        tokio::spawn(async move {
            for action in &msgs_clone {
                // Share 类未达发送阈值：直接跳过（不再入待分享池）
                // 设计变更：知识采集时已直接通过微信面板发送分享类链接，
                // Spontaneous/MoodDriven 触发器产出的 Share 若未达阈值则丢弃，
                // 让 LLM 在下次触发器中自主决定是否再次尝试分享。
                if matches!(action.content_type, crate::proactive::ContentType::Share) {
                    let score = action.value_score.unwrap_or(0.0);
                    if score < crate::proactive::SHARE_VALUE_THRESHOLD {
                        tracing::info!(
                            "[Proactive:{}] Share 未达阈值，丢弃: score={:.2}",
                            char_id_for_mem,
                            score
                        );
                        continue;
                    }
                }

                let channel_str = match action.delivery_channel {
                    crate::proactive::DeliveryChannel::Bubble => "proactive",
                    crate::proactive::DeliveryChannel::ChatWindow => "wechat",
                };
                let meta = serde_json::json!({
                    "channel": channel_str,
                    "speaker": char_id_for_mem,
                    "listener": "user",
                    "perspective": "speaker",
                    "knowledge_source": "direct",
                    "content_type": format!("{:?}", action.content_type).to_lowercase(),
                });
                let clean_for_mem = crate::utils::strip_markdown_syntax(&action.content);
                let _ = memory
                    .add_memory_with_metadata(
                        &clean_for_mem,
                        crate::memory::types::MemoryType::CasualConversation,
                        action.importance as f64,
                        vec![
                            "assistant".to_string(),
                            "proactive".to_string(),
                            "dialogue_turn".to_string(),
                            action.trigger.clone(),
                        ],
                        meta,
                    )
                    .await;
            }
        });

        // 第三者旁观记忆：主动消息也应被在线室友旁观记录
        // 仅桌宠气泡（Bubble）渠道的主动消息被旁观——这是角色在桌面上公开发出的声音。
        // 微信窗口（ChatWindow）是角色私聊通道，其他角色不应旁观。
        let speaker_id = char_id.clone();
        let user_msgs_for_observer: Vec<_> = user_messages
            .iter()
            .filter(|m| m.delivery_channel == crate::proactive::DeliveryChannel::Bubble)
            .cloned()
            .collect();
        if !user_msgs_for_observer.is_empty() {
            let observers: Vec<_> = {
                let chars = state.characters.read();
                chars
                    .iter()
                    .filter(|(id, _)| *id != &speaker_id)
                    .map(|(id, inst)| {
                        (id.clone(), inst.online.clone(), inst.brain.memory.clone())
                    })
                    .collect()
            };
            tokio::spawn(async move {
                for (other_id, online_lock, observer_memory) in observers {
                    if !*online_lock.read() {
                        continue;
                    }
                    for msg in &user_msgs_for_observer {
                        let agent_prefix = build_speaker_prefix(&speaker_id, "user", &other_id);
                        let agent_observation = format!("{} {}", agent_prefix, msg.content);
                        let agent_meta = json!({
                            "channel": "proactive",
                            "speaker": speaker_id,
                            "listener": "user",
                            "perspective": "observer",
                            "knowledge_source": "observed",
                            "reliability": "second_hand",
                            "observer_id": other_id,
                        });
                        if let Err(e) = observer_memory
                            .add_memory_with_metadata(
                                &agent_observation,
                                MemoryType::CasualConversation,
                                0.28,
                                vec!["dialogue".to_string(), "observer".to_string(), "overheard".to_string(), "bystander".to_string(), "proactive".to_string()],
                                agent_meta,
                            )
                            .await
                        {
                            tracing::warn!(
                                "[Proactive] 旁观者 {} 写入主动消息旁观记忆失败: {}",
                                other_id,
                                e
                            );
                        }
                    }
                }
            });
        }
    }

    // 跨角色消息：通过 CrossCharacterBus 发送给第一个在线室友
    // 这些消息是 LLM 为室友量身生成的（"对她说的话"），不走对话历史/记忆/LAST_SPOKEN
    if !cross_messages.is_empty() {
        deliver_cross_character_messages(&app, &state_arc, &char_id, cross_messages).await;
    }

    // 自动日记检查（后台异步，不阻塞 tick 返回）
    crate::diary::spawn_auto_diary_check(&brain, instance.name.clone(), app);

    // 计算推荐的下次 tick 间隔（毫秒）
    // adaptive_tick_enabled=true 时根据空闲时间动态调整；否则使用配置的固定间隔
    // 策略 A：传入 char_id 让每个角色获得独立的随机抖动因子，tick 相位自然漂移
    let recommended_next_interval_ms = if brain.config.proactive.adaptive_tick_enabled {
        crate::proactive::compute_adaptive_tick_ms(ctx.idle_seconds, &char_id)
    } else {
        (brain.config.proactive.tick_interval * 1000) as u64
    };

    // 策略 D：告知前端本角色的跨角色冷却时长（对方发言后本角色应等待的毫秒数）
    // 使用与仲裁逻辑一致的动态 reluctance
    let behavior = crate::character_behavior::get_behavior(&char_id);
    let speak_ratio = compute_speak_ratio(&char_id);
    let ratio_mult = 0.6 + (speak_ratio - 0.5).clamp(-0.5, 0.5) * 1.8;
    let effective_cross_cooldown_ms =
        (CROSS_ROLE_COOLDOWN_SECS * behavior.arbitration.reluctance * ratio_mult * 1000.0) as u64;

    Ok(json!({
        "produced": produced,
        "messages": user_messages,
        "recommended_next_interval_ms": recommended_next_interval_ms,
        "effective_cross_cooldown_ms": effective_cross_cooldown_ms,
    }))
}

/// 投递跨角色消息给第一个在线室友
///
/// 从 `proactive_tick` 的 leader 和非 leader 路径抽取的公共逻辑。
/// 跨角色消息通过 `CROSS_CHARACTER_BUS` 发送，不写入对话历史/记忆/LAST_SPOKEN。
async fn deliver_cross_character_messages(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    char_id: &str,
    cross_messages: Vec<crate::proactive::ProactiveAction>,
) {
    if cross_messages.is_empty() {
        return;
    }
    let target = {
        let characters = state.characters.read();
        characters
            .iter()
            .find(|(id, inst)| *id != char_id && *inst.online.read())
            .map(|(id, inst)| (id.clone(), inst.name.clone()))
    };
    if let Some((target_id, _)) = target {
        for msg in &cross_messages {
            if msg.content.trim().is_empty() {
                tracing::info!(
                    "[Proactive:{}] 跨角色空文本消息，跳过发送: trigger={}",
                    char_id,
                    msg.trigger
                );
                continue;
            }
            let stream_id = generate_cross_stream_id();
            let req = CrossCharacterRequest {
                source_id: char_id.to_string(),
                target_id: target_id.clone(),
                message: msg.content.clone(),
                stream_id,
            };
            match CROSS_CHARACTER_BUS.send(app, state, req).await {
                Ok(reply) => {
                    tracing::info!(
                        "[Proactive:{}] 跨角色对话 → {}: {:?} | 回复: {:?}",
                        char_id,
                        target_id,
                        msg.content,
                        reply
                    );
                    // Path B 续聊：目标回复有价值且建议继续时，spawn 一次反向续聊
                    // （目标→源），让对话能自然延续一轮。限制最多 1 次避免循环。
                    if reply.should_continue && reply.response_mode == "speak" && !reply.reply.is_empty() {
                        let app_clone = app.clone();
                        let state_clone = state.clone();
                        let source_id_for_followup = target_id.clone();
                        let target_id_for_followup = char_id.to_string();
                        let reply_text = reply.reply.clone();
                        tokio::spawn(async move {
                            let followup_stream_id = generate_cross_stream_id();
                            let followup_req = CrossCharacterRequest {
                                source_id: source_id_for_followup,
                                target_id: target_id_for_followup,
                                message: reply_text,
                                stream_id: followup_stream_id,
                            };
                            // 续聊不关心回复，仅投递
                            let _ = CROSS_CHARACTER_BUS.send(&app_clone, &state_clone, followup_req).await;
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[Proactive:{}] 跨角色对话失败 → {}: {}",
                        char_id,
                        target_id,
                        e
                    );
                }
            }
        }
    } else {
        tracing::debug!(
            "[Proactive:{}] 产生了跨角色消息但无在线室友，丢弃",
            char_id
        );
    }
}

/// 消费所有待发送的主动行为
#[tauri::command]
pub async fn drain_proactive_messages(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    let messages = brain.drain_proactive_messages();

    // 按 delivery_channel 写入对话历史
    // - Bubble → channel="proactive"（被 wechat 过滤排除，桌宠气泡路径）
    // - ChatWindow → channel="wechat"（前端 ChatWindow refreshHistory 拉取时显示）
    for action in &messages {
        if action.content.trim().is_empty() {
            tracing::info!(
                "[Proactive:{}] drain 空文本消息，跳过: trigger={}",
                brain.char_id,
                action.trigger
            );
            continue;
        }
        let channel_str = match action.delivery_channel {
            crate::proactive::DeliveryChannel::Bubble => "proactive",
            crate::proactive::DeliveryChannel::ChatWindow => "wechat",
        };
        let clean_content = crate::utils::strip_markdown_syntax(&action.content);
        let mut m = ChatMessage::assistant(&clean_content);
        m.meta = Some(MessageMeta::new(MessageSource::Assistant).with_channel(channel_str));
        brain.dialogue.add_message(m);
    }

    // 主动消息存入记忆系统
    if !messages.is_empty() {
        let char_id_for_mem = brain.char_id.clone();
        for action in &messages {
            let channel_str = match action.delivery_channel {
                crate::proactive::DeliveryChannel::Bubble => "proactive",
                crate::proactive::DeliveryChannel::ChatWindow => "wechat",
            };
            let meta = serde_json::json!({
                "channel": channel_str,
                "speaker": char_id_for_mem,
                "listener": "user",
                "perspective": "speaker",
                "knowledge_source": "direct",
                "content_type": format!("{:?}", action.content_type).to_lowercase(),
            });
            let clean_for_mem = crate::utils::strip_markdown_syntax(&action.content);
            let _ = brain
                .memory
                .add_memory_with_metadata(
                    &clean_for_mem,
                    crate::memory::types::MemoryType::CasualConversation,
                    action.importance as f64,
                    vec![
                        "assistant".to_string(),
                        "proactive".to_string(),
                        "dialogue_turn".to_string(),
                        action.trigger.clone(),
                    ],
                    meta,
                )
                .await;
        }
    }

    Ok(json!({ "messages": messages }))
}

/// 标记本次主动消息被忽略
#[tauri::command]
pub async fn mark_proactive_ignored(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let brain = state.get_character(character_id.as_deref())?.brain;
    brain
        .proactive
        .on_ignored()
        .map_err(|e| e.to_string())
}

/// 即时更新主动对话运行时配置（设置面板保存后调用，无需 reinitialize）
///
/// 从当前 AppConfig 读取 proactive 段并注入 ProactiveOrchestrator，
/// 让 enabled / tick_interval / idle_threshold / min_trigger_interval /
/// proactivity / enable_idle_trigger / enable_window_change_trigger /
/// enable_away_reminder 8 个字段立即影响后续 tick。
#[tauri::command]
pub fn update_proactive_config(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let cfg = state.config.read().get_all().proactive.clone();
    if let Some(cid) = character_id {
        let brain = state.get_character(Some(&cid))?.brain;
        brain.proactive.set_config(cfg);
    } else {
        // 共享窗口（ConfigWindow）不指定角色时，更新所有角色的 proactive 配置
        let characters = state.characters.read();
        for instance in characters.values() {
            instance.brain.proactive.set_config(cfg.clone());
        }
    }
    Ok(())
}

/// 即时更新世界感知配置（设置面板保存后调用，无需 reinitialize）
///
/// 从当前 AppConfig 读取 world 段并注入 WorldStateProvider，
/// 让 enable / enable_weather / enable_inner_monologue /
/// latitude / longitude / sleep_start_hour / sleep_end_hour
/// 等字段立即生效。
/// 注意：`enable_memory_consolidation` 不在此列——它只在 Brain 构造时读取一次决定是否创建
/// MemoryConsolidator，实际靠 handleSave 中的 `reinitialize` 重建 Brain 生效。
/// - 关闭天气功能时同步清空天气缓存
/// - 总开关切换时启停活动日志后台线程
#[tauri::command]
pub fn update_world_config(
    character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let cfg = state.config.read().get_all().world.clone();

    // 天气功能关闭时清空缓存，避免继续使用旧天气数据
    if !cfg.enable_weather {
        state.world_provider.clear_weather();
    }
    // 天气功能启用但未注入天气源时（如曾经关闭后重新打开），自动注入
    if cfg.enable_weather && !state.world_provider.has_weather_source() {
        state.world_provider.set_weather_source(std::sync::Arc::new(crate::world::WeatherSource::new()));
    }
    // 更新全局共享的 WorldStateProvider 配置（跨角色只更新一次）
    state.world_provider.update_config(cfg.clone());

    // 各角色的 ActivityJournal 仍按角色独立启停（每角色独立的行为日志）
    // 同时：内心独白开关关闭时立即清空 current_thought 缓存（与当前想法共享同一滑块）
    let apply_journal = |brain: &crate::brain::brain::Brain| {
        let journal = brain.proactive.activity_journal().clone();
        if cfg.enable {
            journal.start();
        } else {
            journal.stop();
        }
        if !cfg.enable_inner_monologue {
            brain.mind.clear_current_thought();
        }
    };
    if let Some(cid) = character_id {
        let brain = state.get_character(Some(&cid))?.brain;
        apply_journal(&brain);
    } else {
        let characters = state.characters.read();
        for instance in characters.values() {
            apply_journal(&instance.brain);
        }
    }
    Ok(())
}

/// 自动检测设备地理位置（Windows 系统定位优先，IP 定位兜底）。
///
/// 检测成功后写入配置并更新 WorldStateProvider，返回 (纬度, 经度)。
/// 失败返回 None，用户可手动填写。
#[tauri::command]
pub async fn auto_detect_location(
    _character_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<(f64, f64)>, String> {
    let location = crate::world::geolocation::detect_location().await;

    if let Some(ref info) = location {
        // 写入配置文件
        let cm = state.config.read();
        cm.set("world.latitude", json!(info.latitude))
            .map_err(|e| format!("保存纬度失败: {e}"))?;
        cm.set("world.longitude", json!(info.longitude))
            .map_err(|e| format!("保存经度失败: {e}"))?;
        cm.set("world.city", json!(info.city))
            .map_err(|e| format!("保存城市失败: {e}"))?;
        cm.set("world.region", json!(info.region))
            .map_err(|e| format!("保存地区失败: {e}"))?;
        cm.set("world.country", json!(info.country))
            .map_err(|e| format!("保存国家失败: {e}"))?;
        drop(cm);

        // 更新全局共享的 WorldStateProvider（地理位置为全局信息，跨角色只更新一次）
        let cfg = state.config.read().get_all().world.clone();
        state.world_provider.update_config(cfg);
        state.world_provider.set_location(crate::world::LocationSnapshot {
            latitude: info.latitude,
            longitude: info.longitude,
            city: info.city.clone(),
            region: info.region.clone(),
            country: info.country.clone(),
        });
    }

    Ok(location.map(|i| (i.latitude, i.longitude)))
}

/// Open Loop 检测的调用包装：从 brain 取出 router 后转发到 conversation 模块
async fn maybe_mark_open_loop(
    conv: &crate::conversation::Conversation,
    brain: &crate::brain::Brain,
) {
    crate::conversation::maybe_mark_open_loop(conv, &brain.memory, &brain.router).await;
}
