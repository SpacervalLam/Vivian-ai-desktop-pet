//! 远程访问 HTTP 服务端
//!
//! 在应用后台启动一个轻量 axum HTTP 服务，暴露聊天接口，并托管手机端 Web 前端。
//! 配合 Tailscale 等组网工具，手机可通过 Tailscale IP 直接访问电脑上的智能体。
//!
//! # API 端点
//!
//! | 路径 | 方法 | 说明 |
//! |------|------|------|
//! | `/` | GET | 手机端 Web 前端 |
//! | `/api/health` | GET | 健康检查 |
//! | `/api/characters` | GET | 角色列表 |
//! | `/api/characters/:id/presence` | GET | 在场状态 |
//! | `/api/characters/:id/mood` | GET | 心情状态 |
//! | `/api/characters/:id/relationship` | GET | 关系状态 |
//! | `/api/characters/:id/environment` | GET | 环境信息 |
//! | `/api/characters/:id/mind` | GET | 心智快照 |
//! | `/api/characters/:id/history` | GET | 聊天历史 |
//! | `/api/characters/:id/memories` | GET | 记忆列表 |
//! | `/api/characters/:id/diary` | GET | 日记列表 |
//! | `/api/chat` | POST | 发送消息（非流式，含渠道参数） |
//! | `/api/asr` | POST | 语音转文字（base64 f32 PCM） |
//! | `/api/characters/:id/chat/image` | POST | 发送图片消息（base64 → 视觉回复） |
//! | `/api/characters/:id/stop` | POST | 停止生成 |
//! | `/api/characters/:id/presence` | POST | 设置在场状态 |
//! | `/api/config` | GET | 获取完整配置 |
//! | `/api/characters/:id/notes` | GET/POST | 笔记列表 / 创建 |
//! | `/api/characters/:id/notes/:note_id` | GET/PUT/DELETE | 笔记详情 / 更新 / 删除 |
//! | `/api/characters/:id/notes/:note_id/html` | GET | 渲染笔记为 HTML 页面 |
//! | `/api/todos` | GET/POST | 待办列表 / 添加 |
//! | `/api/todos/:id` | PUT/DELETE | 更新 / 删除待办 |
//! | `/api/todos/:id/complete` | POST | 完成待办 |
//! | `/api/tasks` | GET/POST | 定时任务列表 / 添加提醒 |
//! | `/api/tasks/:id` | DELETE | 取消定时任务 |
//! | `/api/tasks/:id/pause` `/resume` | POST | 暂停 / 恢复定时任务 |
//! | `/api/characters/:id/profile` | GET | 用户画像 |
//! | `/api/characters/:id/profile/types` | GET | 事实类型 |
//! | `/api/characters/:id/profile/:type` | PUT/DELETE | 设置 / 删除事实 |
//! | `/api/characters/:id/profile/:type/pin` | POST | 锁定 / 解锁事实 |
//! | `/api/toasts` | GET | 拉取通知（增量） |
//! | `/api/confirmations` | GET | pending 工具确认列表 |
//! | `/api/confirmations/:id` | POST | 解决工具确认 |

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tower_http::services::ServeDir;
use tracing::info;

use crate::conversation::{CONVERSATION_MANAGER, ResponseMode};
use crate::cross_character;
use crate::memory::types::MemoryType;
use crate::messages::MessageMeta;
use crate::providers::base::LLMRequest;
use crate::state::AppState;
use crate::types::response::{ChatMessage, MessageImage};

/// base64 编解码（与 commands/speech.rs 使用方式一致）
use base64::Engine as _;

// ── 请求类型 ──────────────────────────────────────────────────

/// 聊天请求
#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub character_id: Option<String>,
    /// 消息渠道：direct / wechat（默认 wechat）
    #[serde(default = "default_channel")]
    pub channel: String,
}

fn default_channel() -> String {
    "wechat".to_string()
}

/// 语音转文字请求（base64 f32 PCM，16kHz 单声道）
#[derive(Deserialize)]
pub struct AsrRequest {
    pub samples_b64: String,
}

/// 图片消息请求（base64 data URL）
#[derive(Deserialize)]
pub struct ImageChatRequest {
    /// 图片 data URL，形如 `data:image/png;base64,....`
    pub image_data: String,
    pub character_id: Option<String>,
    /// 消息渠道：direct / wechat（默认 wechat）
    #[serde(default = "default_channel")]
    pub channel: String,
    /// 可选的附加文字说明
    #[serde(default)]
    pub message: String,
}

/// 设置在场状态请求
#[derive(Deserialize)]
pub struct SetPresenceRequest {
    pub target: String,
}

/// 共享的应用状态
#[derive(Clone)]
pub struct RemoteAppState {
    pub app_state: Arc<AppState>,
}

// ── 远程 toast 通知队列 ─────────────────────────────────────────

/// 单条远程通知（供手机端轮询展示）
#[derive(Debug, Clone, Serialize)]
pub struct RemoteToast {
    pub id: u64,
    /// 类型：proactive（主动消息） / confirmation（工具确认） / system（系统提醒）
    pub kind: String,
    pub title: String,
    pub body: String,
    pub char_id: String,
    pub timestamp: f64,
    pub payload: serde_json::Value,
}

static NEXT_TOAST_ID: AtomicU64 = AtomicU64::new(1);
static TOAST_QUEUE: OnceLock<Mutex<Vec<RemoteToast>>> = OnceLock::new();
/// 队列容量：超出后丢弃最旧的
const TOAST_MAX: usize = 100;

fn toast_queue_lock() -> &'static Mutex<Vec<RemoteToast>> {
    TOAST_QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

/// 压入一条远程通知（供主动消息 / 提醒等 emit 点调用）
pub fn push_toast(
    kind: &str,
    title: &str,
    body: &str,
    char_id: &str,
    payload: serde_json::Value,
) -> u64 {
    let id = NEXT_TOAST_ID.fetch_add(1, Ordering::Relaxed);
    let toast = RemoteToast {
        id,
        kind: kind.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        char_id: char_id.to_string(),
        timestamp: chrono::Local::now().timestamp() as f64,
        payload,
    };
    let mut q = match toast_queue_lock().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    q.push(toast);
    if q.len() > TOAST_MAX {
        let excess = q.len() - TOAST_MAX;
        q.drain(..excess);
    }
    id
}

/// 列出自 `since_id` 之后的所有通知（用于增量拉取）
pub fn list_toasts(since_id: u64) -> Vec<RemoteToast> {
    let q = match toast_queue_lock().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let all: Vec<RemoteToast> = q.clone();
    all.into_iter().filter(|t| t.id > since_id).collect()
}

/// 清空全部通知（切换角色等场景调用）
pub fn clear_toasts() {
    if let Ok(mut q) = toast_queue_lock().lock() {
        q.clear();
    }
}

// ── 通用工具 ──────────────────────────────────────────────────

fn err_status(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// 检查主 LLM API 是否已配置
fn main_api_configured(state: &AppState) -> bool {
    if let Some(router) = state.model_router.read().as_ref() {
        return router.has_main_provider();
    }
    false
}

// ── Handler ───────────────────────────────────────────────────

/// 健康检查
async fn health_check(State(state): State<RemoteAppState>) -> Json<serde_json::Value> {
    let chars = state.app_state.characters.read();
    let character_list: Vec<serde_json::Value> = chars
        .values()
        .map(|c| {
            let presence = &c.brain.presence;
            let current = presence.current();
            serde_json::json!({
                "id": c.id,
                "name": c.name,
                "online": *c.online.read(),
                "presence": current.as_str(),
                "presence_display": current.display_zh(),
            })
        })
        .collect();
    let initialized = state.app_state.is_initialized();
    drop(chars);

    Json(serde_json::json!({
        "status": "ok",
        "initialized": initialized,
        "api_configured": main_api_configured(&state.app_state),
        "characters": character_list,
    }))
}

/// 角色列表
async fn list_characters(
    State(state): State<RemoteAppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chars = state.app_state.characters.read();
    let active_id = state.app_state.active_character_id.read().clone();
    let list: Vec<serde_json::Value> = chars
        .values()
        .map(|c| {
            let presence = &c.brain.presence;
            let current = presence.current();
            serde_json::json!({
                "id": c.id,
                "name": c.name,
                "online": *c.online.read(),
                "presence": current.as_str(),
                "presence_display": current.display_zh(),
                "can_direct": current.can_direct(),
                "is_in_presence": current.is_in_presence(),
                "since": presence.since(),
                "elapsed_seconds": presence.elapsed_seconds(),
            })
        })
        .collect();
    drop(chars);
    Ok(Json(serde_json::json!({
        "active_id": active_id,
        "characters": list,
    })))
}

/// 获取角色在场状态
async fn get_presence(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let presence = &instance.brain.presence;
    let current = presence.current();
    Ok(Json(serde_json::json!({
        "character_id": presence.char_id(),
        "state": current.as_str(),
        "display_zh": current.display_zh(),
        "can_direct": current.can_direct(),
        "is_in_presence": current.is_in_presence(),
        "since": presence.since(),
        "elapsed_seconds": presence.elapsed_seconds(),
    })))
}

/// 获取角色心情状态
async fn get_mood(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let mood = instance.brain.psychology.compute_mood();
    Ok(Json(serde_json::json!({
        "primary_emotion": mood.primary_emotion.as_str(),
        "secondary_emotion": mood.secondary_emotion.as_str(),
        "valence": mood.valence,
        "arousal": mood.arousal,
        "primary_intensity": mood.primary_intensity,
        "fatigue": mood.fatigue,
        "stress": mood.stress,
        "relationship_score": mood.relationship_score,
    })))
}

/// 获取角色关系状态
async fn get_relationship(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let rel = instance.brain.psychology.relationship();
    Ok(Json(serde_json::json!({
        "intimacy": rel.intimacy * 100.0,
        "trust": rel.trust * 100.0,
        "respect": rel.respect * 100.0,
        "dependency": rel.dependency * 100.0,
        "familiarity": rel.familiarity * 100.0,
        "interaction_count": rel.interaction_count,
        "consecutive_positive": rel.consecutive_positive,
        "consecutive_negative": rel.consecutive_negative,
        "permanent_stage": rel.permanent_stage.as_str(),
        "temporary_stage": rel.temporary_stage.as_ref().map(|t| t.as_str()),
    })))
}

/// 获取角色环境信息
async fn get_environment(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let info = instance.brain.environment.get_environment_info();
    Ok(Json(serde_json::json!({
        "current_window": info.current_window,
        "system_time": info.system_time,
        "cpu_usage": info.cpu_usage,
        "memory_usage": info.memory_usage,
        "battery_level": info.battery_level,
        "is_plugged_in": info.is_plugged_in,
        "network_status": info.network_status,
        "keyboard_idle_seconds": info.keyboard_idle_seconds,
    })))
}

/// 获取角色心智快照
async fn get_mind(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let brain = instance.brain.clone();

    let attention_top: Vec<serde_json::Value> = brain
        .mind
        .attention_top_n(5)
        .into_iter()
        .map(|(entity, weight)| {
            serde_json::json!({ "entity": entity, "weight": weight })
        })
        .collect();

    let goals: Vec<serde_json::Value> = brain
        .mind
        .goals
        .read()
        .active_top_n(3)
        .into_iter()
        .map(|g| {
            serde_json::json!({
                "id": g.id,
                "description": g.description,
                "priority": g.priority,
                "active": g.active,
            })
        })
        .collect();

    let focus = brain.focus_state.lock().await;
    let cognition_mode = focus.mode.as_str().to_string();
    let focus_charge = focus.charge;
    drop(focus);

    let current_thought = brain.mind.current_thought_snapshot().unwrap_or_default();
    let inner_monologue_enabled = brain.config.world.enable_inner_monologue;

    Ok(Json(serde_json::json!({
        "character_id": instance.id,
        "character_name": instance.name,
        "attention_top": attention_top,
        "goals": goals,
        "cognition_mode": cognition_mode,
        "focus_charge": focus_charge,
        "current_thought": current_thought,
        "inner_monologue_enabled": inner_monologue_enabled,
    })))
}

/// 获取聊天历史（可按渠道过滤）
#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<usize>,
    /// 渠道过滤：wechat / direct，缺省返回全部
    pub channel: Option<String>,
}

async fn get_history(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let dialogue = instance.brain.dialogue.clone();
    let limit = query.limit.unwrap_or(50);
    let channel_filter = query.channel.clone();
    let entries = tokio::task::spawn_blocking(move || dialogue.get_all_history())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("任务执行失败: {}", e)))?
        .map_err(err_status)?;
    let values: Vec<serde_json::Value> = entries
        .into_iter()
        .rev()
        .filter(|e| {
            // 按渠道过滤（取最新优先，先过滤再 take limit）
            match &channel_filter {
                Some(ch) => e
                    .metadata
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map_or(false, |c| c == ch),
                None => true,
            }
        })
        .take(limit)
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "role": e.role,
                "content": e.content,
                "timestamp": e.timestamp,
                "session_id": e.session_id,
                "channel": e.metadata.get("channel"),
                "kind": e.metadata.get("kind"),
                "image_path": e.metadata.get("image_path"),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "history": values })))
}

/// 获取记忆列表
#[derive(Deserialize)]
pub struct MemoriesQuery {
    pub limit: Option<usize>,
}

async fn get_memories(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
    Query(query): Query<MemoriesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let memory = &instance.brain.memory;
    let items = memory.get_all_memories().await.map_err(err_status)?;
    let limit = query.limit.unwrap_or(30);
    let values: Vec<serde_json::Value> = items
        .iter()
        .rev()
        .take(limit)
        .filter(|m| {
            m.metadata
                .get("source")
                .and_then(|v| v.as_str())
                .map_or(true, |s| s != "system_seed")
        })
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "content": m.content,
                "timestamp": m.timestamp,
                "memory_type": format!("{:?}", m.memory_type),
                "importance": m.importance,
                "tags": m.tags,
                "metadata": m.metadata,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "memories": values })))
}

/// 获取日记列表
#[derive(Deserialize)]
pub struct DiaryQuery {
    pub date: Option<String>,
}

async fn get_diary(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
    Query(query): Query<DiaryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let cid = instance.id.clone();
    let entries = tokio::task::spawn_blocking(move || {
        crate::diary::get_entries(&cid, query.date.as_deref())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("任务执行失败: {}", e)))?
    .map_err(err_status)?;
    let values: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|e| serde_json::to_value(&e).unwrap_or(serde_json::json!({})))
        .collect();
    Ok(Json(serde_json::json!({ "diaries": values })))
}

/// 设置在场状态
async fn set_presence(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
    Json(req): Json<SetPresenceRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let instance = state
        .app_state
        .get_character(Some(&char_id))
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let target_state = crate::presence::PresenceState::from_str(&req.target);
    let event = instance.brain.presence.transition(
        target_state,
        crate::presence::PresenceChangeReason::UserInteraction,
    );

    // 任务进行中切回 Online 时被延迟
    if event.is_none() && target_state == crate::presence::PresenceState::Online {
        return Ok(Json(serde_json::json!({
            "changed": false,
            "deferred": true,
            "current": instance.brain.presence.current().as_str(),
        })));
    }

    let current = instance.brain.presence.current();
    Ok(Json(serde_json::json!({
        "changed": event.is_some(),
        "current": current.as_str(),
        "display_zh": current.display_zh(),
    })))
}

/// 停止生成
async fn stop_generation(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state.app_state.set_generation_cancel(&char_id, true);
    Ok(Json(serde_json::json!({ "stopped": true })))
}

/// 发送消息（非流式，返回完整响应）
///
/// 完整复刻 send_message_stream 的核心流程：
/// - 会话生命周期（start_or_continue → update_after_round → 意图判断 close）
/// - 在场状态唤醒（Rest/Offline → Online）
/// - World Model 上下文抽取
/// - 焦点租约（屏蔽其他角色主动打断）
/// - Busy 状态延后处理
/// - URL 知识抓取入库
/// - 旁观记忆（direct 渠道）
/// - 人格卡片轮次递增
async fn chat_handler(
    State(state): State<RemoteAppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let char_id = req
        .character_id
        .clone()
        .unwrap_or_else(|| state.app_state.active_character_id.read().clone());

    if req.message.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "消息不能为空".to_string()));
    }

    if !main_api_configured(&state.app_state) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "主 LLM API 未配置".to_string(),
        ));
    }

    let instance = state
        .app_state
        .get_character(req.character_id.as_deref())
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let brain = instance.brain.clone();
    let channel_str = req.channel.clone();
    let message = req.message.clone();

    // ── 渠道限制：direct 在 Offline 状态拒绝 ──
    let current_presence = brain.presence.current();
    let is_busy = current_presence == crate::presence::PresenceState::Busy;

    if channel_str == "direct" && !current_presence.can_direct() {
        let hint = match current_presence {
            crate::presence::PresenceState::Offline => "对方不在，发微信留言",
            _ => "对方不在场，发微信吧",
        };
        return Err((
            StatusCode::FORBIDDEN,
            format!("{}: {}", hint, current_presence.display_zh()),
        ));
    }

    // ── Busy 状态下微信消息延后处理 ──
    if is_busy && channel_str == "wechat" {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            if brain.presence.current() != crate::presence::PresenceState::Busy {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    // ── 用户交互唤醒：从 Rest/Offline 回到 Online ──
    if let Some(event) = brain.presence.wake_on_user_interaction() {
        brain.proactive.signal_waking_up();
        let memory_text = brain.presence.memory_text(&event);
        let memory = brain.memory.clone();
        let text = memory_text;
        let char_id_for_mem = char_id.clone();
        tokio::spawn(async move {
            let meta = serde_json::json!({
                "channel": "presence",
                "speaker": char_id_for_mem,
                "listener": char_id_for_mem,
                "perspective": "speaker",
            });
            let _ = memory
                .add_memory_with_metadata(
                    &text,
                    MemoryType::ShortTerm,
                    0.4,
                    vec!["presence_log".to_string(), "assistant".to_string()],
                    meta,
                )
                .await;
        });
    }

    // ── 设置渠道 ──
    brain.dialogue.set_channel(&channel_str);

    // ── 串行化 brain.think ──
    state.app_state.session_coordinator.signal_user_input(&char_id);
    let _brain_lock = instance.think_lock.clone();
    let _brain_guard = _brain_lock.lock().await;
    state.app_state.reset_generation_cancel(&char_id);

    // ── 会话生命周期：获取或创建会话 ──
    let conv = CONVERSATION_MANAGER
        .start_or_continue("user", &char_id, &message)
        .unwrap_or_else(|| {
            CONVERSATION_MANAGER.force_new_session("user", &char_id, &message)
        });
    CONVERSATION_MANAGER.touch_user_message(&char_id);
    brain.presence.record_user_interaction();

    // ── World Model：抽取预期回归时间等 ──
    brain.world_state.ingest_dialogue(&message);

    // ── 焦点租约：think 期间屏蔽其他角色主动打断 ──
    let _focus_lease = crate::commands::proactive::FocusLeaseGuard::acquire(&char_id);

    // ── Busy 状态下 direct 渠道：注入忙碌被呼唤的语境 ──
    let think_input = if is_busy && channel_str == "direct" {
        format!("（你从忙碌状态下被用户呼唤）\n{}", message)
    } else {
        message.clone()
    };

    // 临时开启路由回退事件发送
    if let Some(router) = state.app_state.model_router.read().as_ref() {
        router.set_emit_enabled(true);
    }

    let result = brain.think(&think_input, false).await;
    drop(_focus_lease);

    if let Some(router) = state.app_state.model_router.read().as_ref() {
        router.set_emit_enabled(false);
    }

    // ── 会话生命周期：think 完成后更新会话状态 + 意图判断 ──
    {
        let response_mode = result.as_ref().ok().map(|r| r.response_mode.clone()).unwrap_or_else(|| "speak".to_string());
        let reply_text = result.as_ref().ok().map(|r| r.text.clone()).unwrap_or_default();
        let mode = ResponseMode::from_str(&response_mode);
        let _ = CONVERSATION_MANAGER.update_after_round(
            &conv.id,
            mode,
            if mode.needs_speech() { Some(&reply_text) } else { None },
            &message,
        );

        // 意图判断
        let history: Vec<String> = brain.dialogue.get_history().iter().map(|m| m.content.clone()).collect();
        let judge = crate::dialogue::intent_judge::IntentJudge::new(
            state.app_state.model_router.read().as_ref().map(|r| std::sync::Arc::new(r.clone())),
        );
        let user_close = judge.judge_close_reason(&message, &history).await;
        let agent_close = if user_close.is_none() {
            judge.judge_close_reason(&reply_text, &history).await
        } else {
            None
        };
        if let Some(reason) = user_close.or(agent_close) {
            let closed_conv = CONVERSATION_MANAGER.close_with_reason(&conv.id, reason);
            // Episode 封包
            if let Some(closed) = closed_conv {
                let memory_ids = CONVERSATION_MANAGER.get_session_memory_ids(&closed.id);
                if !memory_ids.is_empty() || closed.rounds > 0 {
                    if let Some(ep_store) = brain.memory.episode_store() {
                        let timestamps = vec![closed.created_at, closed.last_active_at];
                        let importances = vec![0.5];
                        let topic = if closed.topic.is_empty()
                            || closed.topic == "(无话题)"
                            || closed.topic == crate::conversation::manager::TOPIC_PENDING
                        {
                            None
                        } else {
                            Some(closed.topic.clone())
                        };
                        ep_store.seal_episode(memory_ids, &timestamps, &importances, topic, None, &[]);
                    }
                }
                brain.mind.clear_working_memory();
            }
            // 用户说"去忙了" → 角色也跟着去做自己的事
            if matches!(
                reason,
                crate::conversation::CloseReason::Interrupted | crate::conversation::CloseReason::GoodBye
            ) {
                let _ = brain.presence.transition(
                    crate::presence::PresenceState::Busy,
                    crate::presence::PresenceChangeReason::UserLeft,
                );
            }
        }
    }

    // 清理流式回调 + 重置渠道
    brain.set_stream_emitter(None);
    brain.dialogue.set_channel("wechat");
    drop(_brain_guard);

    // 检查取消
    if state.app_state.is_generation_cancelled(&char_id) {
        return Err((StatusCode::OK, "已取消".to_string()));
    }

    let response = result.map_err(err_status)?;

    // 递增人格卡片轮次计数器
    brain.persona.tick_card_turn();

    // ── URL 知识抓取入库（fire-and-forget）──
    {
        let memory = brain.memory.clone();
        let msg_for_kb = message.clone();
        tokio::spawn(async move {
            if let Some(url) = crate::network::url_fetcher::extract_first_url(&msg_for_kb) {
                tracing::info!("[Remote] 检测到用户分享链接，开始抓取: {}", url);
                match crate::network::url_fetcher::fetch_page(&url).await {
                    Ok(page) => {
                        let tags = vec!["user_link".to_string()];
                        let _ = memory
                            .add_knowledge_document(&page.title, &page.text, tags, "user_link", Some(-1))
                            .await;
                    }
                    Err(e) => tracing::warn!("[Remote] 抓取链接 {} 失败: {}", url, e),
                }
            }
        });
    }

    // ── 旁观记忆（direct 渠道，fire-and-forget）──
    if channel_str == "direct" {
        let speaker_id = char_id.clone();
        let user_msg = message.trim().to_string();
        let agent_reply = response.text.trim().to_string();
        let channel_clone = channel_str.clone();
        let base_importance = response.importance_user.max(response.importance_ai);
        let observer_importance = (base_importance * 0.6).clamp(0.05, 0.85);

        let observers: Vec<_> = {
            let chars = state.app_state.characters.read();
            chars
                .iter()
                .filter(|(id, _)| *id != &speaker_id)
                .filter(|(_, inst)| *inst.online.read())
                .map(|(id, inst)| {
                    (
                        id.clone(),
                        inst.brain.memory.clone(),
                        inst.name.clone(),
                    )
                })
                .collect()
        };

        if !observers.is_empty() {
            tokio::spawn(async move {
                for (other_id, observer_memory, _observer_name) in observers {
                    let user_prefix = cross_character::build_speaker_prefix("user", &speaker_id, &other_id);
                    let user_observation = format!("{} {}", user_prefix, user_msg);
                    let user_meta = serde_json::json!({
                        "channel": channel_clone,
                        "speaker": "user",
                        "listener": speaker_id,
                        "perspective": "observer",
                        "knowledge_source": "observed",
                        "observer_id": other_id,
                    });
                    let _ = observer_memory
                        .add_memory_with_metadata(
                            &user_observation,
                            MemoryType::CasualConversation,
                            observer_importance,
                            vec!["dialogue".to_string(), "observer".to_string(), "overheard".to_string()],
                            user_meta,
                        )
                        .await;

                    let agent_prefix = cross_character::build_speaker_prefix(&speaker_id, "user", &other_id);
                    let agent_observation = format!("{} {}", agent_prefix, agent_reply);
                    let agent_meta = serde_json::json!({
                        "channel": channel_clone,
                        "speaker": speaker_id,
                        "listener": "user",
                        "perspective": "observer",
                        "knowledge_source": "observed",
                        "observer_id": other_id,
                    });
                    let _ = observer_memory
                        .add_memory_with_metadata(
                            &agent_observation,
                            MemoryType::CasualConversation,
                            observer_importance,
                            vec!["dialogue".to_string(), "observer".to_string(), "overheard".to_string()],
                            agent_meta,
                        )
                        .await;
                }
            });
        }
    }

    Ok(Json(serde_json::json!({
        "reply": response.text,
        "character_id": char_id,
        "motion": response.motion,
        "expression": response.expression,
        "expression_duration_ms": response.expression_duration_ms,
        "emotion_score": response.emotion_score,
        "sticker": response.sticker,
        "user_emotion": response.user_emotion,
        "response_mode": response.response_mode,
        "voice_message": response.voice_message,
    })))
}

/// 语音转文字（手机端 ASR）
///
/// 接收 base64 编码的小端 f32 PCM（16kHz 单声道），委托给后端 ASR 识别并返回文本。
async fn asr_handler(
    State(state): State<RemoteAppState>,
    Json(req): Json<AsrRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(req.samples_b64.trim())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("base64 解码失败: {e}")))?;
    if bytes.len() % 4 != 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "音频数据长度不是 4 的倍数（f32 PCM）".to_string(),
        ));
    }
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if samples.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "音频为空".to_string()));
    }
    let text = state
        .app_state
        .asr
        .transcribe(&samples)
        .await
        .map_err(err_status)?;
    Ok(Json(serde_json::json!({ "text": text })))
}

/// 发送图片消息（手机端发图）
///
/// 接收 base64 data URL 图片，保存副本、写入对话历史、调用多模态 LLM 生成
/// 图片描述与回应，返回 `reply`（前端渲染 AI 文字气泡）与 `image_path`。
async fn image_chat_handler(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
    Json(req): Json<ImageChatRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let char_id = req
        .character_id
        .clone()
        .unwrap_or_else(|| char_id.clone());
    let channel_str = if req.channel.is_empty() {
        "wechat".to_string()
    } else {
        req.channel.clone()
    };

    if !main_api_configured(&state.app_state) {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "主 LLM API 未配置".to_string()));
    }
    if !state
        .app_state
        .config
        .read()
        .get_typed::<bool>("ai.enable_vision", false)
    {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "图片输入功能未启用（ai.enable_vision）".to_string()));
    }

    // 解析 data URL：`data:<mime>;base64,<data>`，也兼容纯 base64
    let image_data = req.image_data.trim().to_string();
    let (mime, raw_b64) = if let Some(rest) = image_data.strip_prefix("data:") {
        let comma = rest.find(',').ok_or((
            StatusCode::BAD_REQUEST,
            "无效的 data URL（缺少逗号分隔）".to_string(),
        ))?;
        let header = &rest[..comma];
        let mime = header
            .split(';')
            .next()
            .unwrap_or("image/png")
            .to_string();
        (mime, rest[comma + 1..].to_string())
    } else {
        // 无 data URL 前缀：按字节嗅探 MIME
        let raw = base64::engine::general_purpose::STANDARD
            .decode(image_data.trim())
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("base64 解码失败: {e}")))?;
        let mime = crate::commands::config::detect_image_mime(&raw).to_string();
        drop(raw);
        (mime, image_data.trim().to_string())
    };

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw_b64.as_bytes())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("base64 解码失败: {e}")))?;
    if bytes.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "图片数据为空".to_string()));
    }

    // 保存副本到用户数据目录 images/（阻塞操作移入 spawn_blocking）
    let (rel_path, b64) = tokio::task::spawn_blocking(move || -> Result<(String, String), String> {
        let data_dir = crate::utils::path::get_user_data_dir();
        let images_dir = data_dir.join("images");
        crate::utils::path::ensure_dir(&images_dir)
            .map_err(|e| format!("创建图片目录失败: {}", e))?;
        let saved_name = format!("{}.png", uuid::Uuid::new_v4());
        let saved_path = images_dir.join(&saved_name);
        std::fs::write(&saved_path, &bytes).map_err(|e| format!("保存图片失败: {}", e))?;
        let rel_path = format!("images/{}", saved_name);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok((rel_path, b64))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("图片处理任务失败: {}", e)))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("图片处理失败: {}", e)))?;

    let data_url = format!("data:{};base64,{}", mime, b64);
    let now_ts = chrono::Local::now().timestamp_millis() as f64 / 1000.0;

    // 写入对话历史：用户图片消息
    {
        let instance = state
            .app_state
            .get_character(Some(&char_id))
            .map_err(|e| (StatusCode::NOT_FOUND, e))?;
        let mut user_msg = ChatMessage::user("📷 [图片]");
        user_msg.meta = Some(MessageMeta::user().with_channel(&channel_str));
        instance.brain.dialogue.add_message_with_metadata(
            user_msg,
            serde_json::json!({
                "source": "chat",
                "kind": "image",
                "image_path": rel_path,
                "channel": channel_str,
            }),
        );
    }

    // 提取最近对话上下文，帮助理解图片意图
    let recent_context = {
        let instance = state
            .app_state
            .get_character(Some(&char_id))
            .map_err(|e| (StatusCode::NOT_FOUND, e))?;
        let history = instance.brain.dialogue.get_history();
        let recent: Vec<String> = history
            .iter()
            .rev()
            .take(6)
            .map(|m| {
                let role = if m.role == "user" { "User" } else { "AI" };
                format!("{}: {}", role, m.content)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if recent.is_empty() {
            String::new()
        } else {
            format!("\n## 最近对话上下文\n{}\n\n请结合以上对话理解用户发送这张图片的意图。", recent.join("\n"))
        }
    };

    let system_prompt = format!(
        "你是图片描述助手。请分析用户发送的图片，返回严格的 JSON：\n\
        {{\"description\": \"对图片内容的客观、详细的中文描述（用于记忆存档，50-150字）\", \
        \"reply\": \"以角色口吻对这张图片给出自然的中文回应（20-60字）\"}}\n\
        仅返回 JSON 对象，不要任何其他内容、不要 markdown 代码块。\
        {}",
        recent_context
    );

    let router = {
        let guard = state.app_state.model_router.read();
        guard
            .as_ref()
            .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "模型路由未初始化".to_string()))?
            .clone()
    };
    let image_detail = state
        .app_state
        .config
        .read()
        .get_typed::<String>("ai.image_detail", "auto".to_string());
    let image = MessageImage {
        media_type: mime.clone(),
        data: b64,
        url: None,
        detail: Some(image_detail),
    };
    let nonce = uuid::Uuid::new_v4().as_simple().to_string();
    let user_text = format!("请描述这张图片。[req:{}]", &nonce[..8]);
    let messages = vec![
        ChatMessage::system(&system_prompt),
        ChatMessage::user_with_images(user_text, vec![image]),
    ];

    let llm_result = router
        .generate(LLMRequest::new("vision_describe", messages))
        .await;

    let (description, reply) = match llm_result {
        Ok(text) => crate::commands::chat::parse_image_description_response(&text),
        Err(e) => return Err(err_status(e)),
    };

    // AI 回复写入对话历史
    {
        let instance = state
            .app_state
            .get_character(Some(&char_id))
            .map_err(|e| (StatusCode::NOT_FOUND, e))?;
        let mut ai_msg = ChatMessage::assistant(&reply);
        ai_msg.meta = Some(MessageMeta::assistant().with_channel(&channel_str));
        instance.brain.dialogue.add_message(ai_msg);
    }

    // 图片描述写入记忆系统（fire-and-forget）
    {
        let memory = state
            .app_state
            .get_character(Some(&char_id))
            .ok()
            .map(|inst| inst.brain.memory.clone());
        if let Some(memory_mgr) = memory {
            let char_id_for_mem = char_id.clone();
            let description_for_memory = if description.is_empty() {
                reply.clone()
            } else {
                description.clone()
            };
            let rel = rel_path.clone();
            let channel_clone = channel_str.clone();
            tokio::spawn(async move {
                let init_meta = serde_json::json!({
                    "channel": channel_clone,
                    "speaker": "user",
                    "listener": char_id_for_mem,
                    "perspective": "speaker",
                    "knowledge_source": "direct",
                });
                match memory_mgr
                    .add_memory_with_metadata(
                        &description_for_memory,
                        MemoryType::General,
                        0.5,
                        vec![
                            "image".to_string(),
                            "shared_memory".to_string(),
                            "user".to_string(),
                            "assistant".to_string(),
                        ],
                        init_meta,
                    )
                    .await
                {
                    Ok(item) => {
                        let _ = memory_mgr.patch_memory_metadata(
                            &item.id,
                            serde_json::json!({
                                "kind": "image",
                                "image_path": rel,
                                "source": "chat",
                                "role": "user",
                                "memory_type": "general",
                                "semantic_type": "shared_memory",
                            }),
                        );
                    }
                    Err(e) => {
                        tracing::warn!("[remote image] 图片记忆写入失败: {}", e);
                    }
                }
            });
        }
    }

    Ok(Json(serde_json::json!({
        "reply": reply,
        "description": description,
        "image_path": rel_path,
        "data_url": data_url,
        "timestamp": now_ts,
        "character_id": char_id,
    })))
}

/// 获取完整配置
async fn get_config(
    State(state): State<RemoteAppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = state.app_state.config.read();
    let all = config.get_all();
    Ok(Json(serde_json::to_value(&all).map_err(err_status)?))
}

// ── 笔记 API ─────────────────────────────────────────────────

/// 笔记创建请求
#[derive(Deserialize)]
pub struct NoteWriteRequest {
    pub title: String,
    #[serde(default)]
    pub blocks: serde_json::Value,
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub palette: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub cover: Option<serde_json::Value>,
}

/// 列出笔记摘要
async fn list_notes(
    State(_state): State<RemoteAppState>,
    Path(char_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let summaries = crate::notebook::storage::list(&char_id).map_err(err_status)?;
    Ok(Json(serde_json::json!({ "notes": summaries })))
}

/// 读取笔记详情
async fn get_note(
    State(_state): State<RemoteAppState>,
    Path((char_id, note_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let note = crate::notebook::storage::load(&char_id, &note_id).map_err(err_status)?;
    Ok(Json(serde_json::to_value(&note).map_err(err_status)?))
}

/// 渲染笔记为完整 HTML 页面
async fn get_note_html(
    State(_state): State<RemoteAppState>,
    Path((char_id, note_id)): Path<(String, String)>,
) -> Response {
    let note = match crate::notebook::storage::load(&char_id, &note_id) {
        Ok(n) => n,
        Err(e) => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("note not found: {}", e)))
                .unwrap();
        }
    };
    let html = crate::notebook::renderer::render_html(&note);
    Response::builder()
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap()
}

/// 创建笔记
async fn create_note(
    State(_state): State<RemoteAppState>,
    Path(char_id): Path<String>,
    Json(req): Json<NoteWriteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::tools::builtin::notebook_tools::{parse_blocks, parse_cover, parse_layout, parse_palette};
    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "标题不能为空".to_string()));
    }
    let blocks = parse_blocks(&req.blocks).map_err(err_status)?;
    if blocks.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "内容块不能为空".to_string()));
    }
    let now = chrono::Local::now().timestamp() as f64;
    let note = crate::notebook::NoteBook {
        id: crate::notebook::NoteBook::generate_id(),
        title: title.clone(),
        char_id: char_id.clone(),
        created_at: now,
        updated_at: now,
        tags: req.tags.unwrap_or_default(),
        layout: req.layout.as_deref().map(parse_layout).unwrap_or_default(),
        palette: req.palette.as_deref().map(parse_palette).unwrap_or_default(),
        cover: match req.cover {
            Some(c) if !c.is_null() => parse_cover(&c).map_err(err_status)?,
            _ => None,
        },
        blocks,
    };
    let note_id = note.id.clone();
    crate::notebook::storage::save(&note).map_err(err_status)?;
    Ok(Json(serde_json::json!({ "note_id": note_id, "char_id": char_id, "title": title })))
}

/// 更新笔记
async fn update_note(
    State(_state): State<RemoteAppState>,
    Path((char_id, note_id)): Path<(String, String)>,
    Json(req): Json<NoteWriteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::tools::builtin::notebook_tools::{parse_blocks, parse_cover, parse_layout, parse_palette};
    let mut note = crate::notebook::storage::load(&char_id, &note_id).map_err(err_status)?;
    let t = req.title.trim().to_string();
    if t.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "标题不能为空".to_string()));
    }
    note.title = t;
    if let Some(l) = req.layout {
        note.layout = parse_layout(&l);
    }
    if let Some(p) = req.palette {
        note.palette = parse_palette(&p);
    }
    if let Some(tags) = req.tags {
        note.tags = tags;
    }
    if let Some(c) = req.cover {
        note.cover = parse_cover(&c).map_err(err_status)?;
    }
    if !req.blocks.is_null() {
        let blocks = parse_blocks(&req.blocks).map_err(err_status)?;
        if blocks.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "内容块不能为空".to_string()));
        }
        note.blocks = blocks;
    }
    note.updated_at = chrono::Local::now().timestamp() as f64;
    crate::notebook::storage::save(&note).map_err(err_status)?;
    Ok(Json(serde_json::json!({ "note_id": note_id, "char_id": char_id, "title": note.title })))
}

/// 删除笔记
async fn delete_note(
    State(state): State<RemoteAppState>,
    Path((char_id, note_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // 清理知识库关联条目（与 commands/notebook.rs 保持一致）
    if let Ok(inst) = state.app_state.get_character(Some(&char_id)) {
        let memory = inst.brain.memory.clone();
        let ref_path = crate::notebook::storage::note_memory_ref_path(&char_id, &note_id);
        if let Ok(memory_id) = std::fs::read_to_string(&ref_path) {
            let memory_id = memory_id.trim();
            if !memory_id.is_empty() {
                if let Err(e) = memory.delete_knowledge_document(memory_id).await {
                    tracing::warn!("[Remote] 删除笔记 {} 的知识库条目失败: {}", note_id, e);
                }
            }
        }
    }
    crate::notebook::storage::delete(&char_id, &note_id).map_err(err_status)?;
    Ok(Json(serde_json::json!({ "deleted": true, "note_id": note_id })))
}

// ── 待办 API ─────────────────────────────────────────────────

/// 待办写入请求
#[derive(Deserialize)]
pub struct TodoWriteRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<u32>,
    #[serde(default)]
    pub due_date: Option<String>,
}

async fn list_todos(
    State(_state): State<RemoteAppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let include_completed = q.get("include_completed").map(|v| v == "true").unwrap_or(false);
    let items = crate::tools::builtin::todo_tools::list_todo_items(include_completed, None);
    Ok(Json(serde_json::json!({ "items": items, "total": items.len() })))
}

async fn add_todo(
    State(_state): State<RemoteAppState>,
    Json(req): Json<TodoWriteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let item = crate::tools::builtin::todo_tools::add_todo_item(
        &req.title,
        req.description.as_deref().unwrap_or(""),
        req.priority.unwrap_or(1),
        req.due_date.as_deref(),
    );
    Ok(Json(serde_json::json!({ "item": item })))
}

/// 待办更新请求（全部可选，仅更新提供字段）
#[derive(Deserialize)]
pub struct TodoUpdateRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<u32>,
    #[serde(default)]
    pub due_date: Option<String>,
}

async fn update_todo(
    State(_state): State<RemoteAppState>,
    Path(id): Path<String>,
    Json(req): Json<TodoUpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let item = crate::tools::builtin::todo_tools::update_todo_item(
        &id,
        req.title.as_deref(),
        req.description.as_deref(),
        req.priority,
        req.due_date.as_deref(),
    )
    .map_err(err_status)?;
    Ok(Json(serde_json::json!({ "item": item })))
}

async fn complete_todo(
    State(_state): State<RemoteAppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let item = crate::tools::builtin::todo_tools::complete_todo_item(&id).map_err(err_status)?;
    Ok(Json(serde_json::json!({ "item": item })))
}

async fn delete_todo(
    State(_state): State<RemoteAppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if crate::tools::builtin::todo_tools::delete_todo_item(&id) {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err((StatusCode::NOT_FOUND, "待办不存在".to_string()))
    }
}

// ── 定时任务 API ─────────────────────────────────────────────

/// 定时提醒创建请求
#[derive(Deserialize)]
pub struct TaskWriteRequest {
    pub message: String,
    pub scheduled_time: f64,
    #[serde(default)]
    pub repeat_interval: Option<u64>,
}

async fn list_tasks(
    State(state): State<RemoteAppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let tasks = state.app_state.scheduler.list_tasks();
    Ok(Json(serde_json::json!({ "tasks": tasks, "total": tasks.len() })))
}

async fn add_task(
    State(state): State<RemoteAppState>,
    Json(req): Json<TaskWriteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let id = if let Some(interval) = req.repeat_interval {
        let task = crate::brain::scheduler::ScheduledTask::new_reminder(&req.message, req.scheduled_time);
        state.app_state.scheduler.schedule_repeat(task, interval)
    } else {
        state.app_state.scheduler.schedule_reminder(&req.message, req.scheduled_time)
    };
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn cancel_task(
    State(state): State<RemoteAppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ok = state.app_state.scheduler.cancel_task(&id);
    Ok(Json(serde_json::json!({ "cancelled": ok })))
}

async fn pause_task(
    State(state): State<RemoteAppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ok = state.app_state.scheduler.pause_task(&id);
    Ok(Json(serde_json::json!({ "paused": ok })))
}

async fn resume_task(
    State(state): State<RemoteAppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ok = state.app_state.scheduler.resume_task(&id);
    Ok(Json(serde_json::json!({ "resumed": ok })))
}

// ── 用户画像 API ─────────────────────────────────────────────

/// 事实写入请求
#[derive(Deserialize)]
pub struct FactWriteRequest {
    pub content: String,
    #[serde(default)]
    pub pinned: Option<bool>,
}

/// 获取用户画像
async fn get_profile(
    State(state): State<RemoteAppState>,
    Path(char_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let characters = state.app_state.characters.read();
    let instance = characters
        .get(&char_id)
        .ok_or((StatusCode::NOT_FOUND, format!("角色不存在: {char_id}")))?;
    let chat_chain = instance
        .brain
        .chat_chain
        .as_ref()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "ChatChain 未初始化".to_string()))?;
    let store = &chat_chain.user_facts;
    let (basic_data, custom_facts) = store.get_all_facts();
    let recent_state = store.get_recent_state();
    let ordered_types = [
        crate::memory::user_facts::UserFactType::Name,
        crate::memory::user_facts::UserFactType::Age,
        crate::memory::user_facts::UserFactType::Gender,
        crate::memory::user_facts::UserFactType::Occupation,
        crate::memory::user_facts::UserFactType::Location,
        crate::memory::user_facts::UserFactType::Birthday,
        crate::memory::user_facts::UserFactType::SleepSchedule,
        crate::memory::user_facts::UserFactType::FavoriteWebsite,
        crate::memory::user_facts::UserFactType::FavoriteGame,
        crate::memory::user_facts::UserFactType::Hobby,
    ];
    let basic_facts: Vec<serde_json::Value> = ordered_types
        .iter()
        .filter_map(|t| basic_data.get(t).map(|f| fact_to_json(f, t.label_zh())))
        .collect();
    let custom_views: Vec<serde_json::Value> = custom_facts
        .iter()
        .map(|f| fact_to_json(f, crate::memory::user_facts::UserFactType::Custom.label_zh()))
        .collect();
    Ok(Json(serde_json::json!({
        "basic_facts": basic_facts,
        "recent_state": recent_state,
        "custom_facts": custom_views,
    })))
}

fn fact_to_json(f: &crate::memory::user_facts::UserFact, label: &str) -> serde_json::Value {
    serde_json::json!({
        "fact_type": f.fact_type.as_str(),
        "label": label,
        "content": f.content,
        "confidence": f.confidence,
        "timestamp": f.timestamp,
        "is_pinned": f.is_pinned,
        "is_manual": f.reasoning.as_deref() == Some("manual_edit"),
    })
}

/// 获取支持的事实类型
async fn get_profile_types() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let types = [
        crate::memory::user_facts::UserFactType::Name,
        crate::memory::user_facts::UserFactType::Age,
        crate::memory::user_facts::UserFactType::Gender,
        crate::memory::user_facts::UserFactType::Occupation,
        crate::memory::user_facts::UserFactType::Location,
        crate::memory::user_facts::UserFactType::Birthday,
        crate::memory::user_facts::UserFactType::SleepSchedule,
        crate::memory::user_facts::UserFactType::FavoriteWebsite,
        crate::memory::user_facts::UserFactType::FavoriteGame,
        crate::memory::user_facts::UserFactType::Hobby,
        crate::memory::user_facts::UserFactType::Custom,
    ];
    let list: Vec<serde_json::Value> = types
        .iter()
        .map(|t| serde_json::json!({ "value": t.as_str(), "label": t.label_zh() }))
        .collect();
    Ok(Json(serde_json::json!({ "types": list })))
}

/// 设置事实
async fn set_fact(
    State(state): State<RemoteAppState>,
    Path((char_id, fact_type)): Path<(String, String)>,
    Json(req): Json<FactWriteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let fact_type = crate::memory::user_facts::UserFactType::from_str(&fact_type)
        .ok_or((StatusCode::BAD_REQUEST, format!("未知的事实类型")))?;
    let characters = state.app_state.characters.read();
    let instance = characters
        .get(&char_id)
        .ok_or((StatusCode::NOT_FOUND, format!("角色不存在: {char_id}")))?;
    let chat_chain = instance
        .brain
        .chat_chain
        .as_ref()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "ChatChain 未初始化".to_string()))?;
    chat_chain
        .user_facts
        .set_fact(fact_type, &req.content, req.pinned.unwrap_or(false))
        .map_err(err_status)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 锁定/解锁事实
async fn pin_fact(
    State(state): State<RemoteAppState>,
    Path((char_id, fact_type)): Path<(String, String)>,
    Json(req): Json<FactWriteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let fact_type = crate::memory::user_facts::UserFactType::from_str(&fact_type)
        .ok_or((StatusCode::BAD_REQUEST, "未知的事实类型".to_string()))?;
    let pinned = req.pinned.unwrap_or(false);
    let characters = state.app_state.characters.read();
    let instance = characters
        .get(&char_id)
        .ok_or((StatusCode::NOT_FOUND, format!("角色不存在: {char_id}")))?;
    let chat_chain = instance
        .brain
        .chat_chain
        .as_ref()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "ChatChain 未初始化".to_string()))?;
    chat_chain
        .user_facts
        .set_pinned(fact_type, pinned)
        .map_err(err_status)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 删除事实（可选 content 参数用于删除自定义事实中的特定条目）
async fn delete_fact(
    State(state): State<RemoteAppState>,
    Path((char_id, fact_type)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let fact_type = crate::memory::user_facts::UserFactType::from_str(&fact_type)
        .ok_or((StatusCode::BAD_REQUEST, "未知的事实类型".to_string()))?;
    let content = q.get("content").cloned();
    let characters = state.app_state.characters.read();
    let instance = characters
        .get(&char_id)
        .ok_or((StatusCode::NOT_FOUND, format!("角色不存在: {char_id}")))?;
    let chat_chain = instance
        .brain
        .chat_chain
        .as_ref()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "ChatChain 未初始化".to_string()))?;
    chat_chain
        .user_facts
        .delete_fact(fact_type, content.as_deref())
        .map_err(err_status)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── 通知 / 确认 API ──────────────────────────────────────────

/// 获取新通知（增量拉取）
#[derive(Deserialize)]
pub struct ToastQuery {
    pub since: Option<u64>,
}

async fn get_toasts(
    Query(q): Query<ToastQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let toasts = crate::remote::list_toasts(q.since.unwrap_or(0));
    Ok(Json(serde_json::json!({ "toasts": toasts })))
}

/// 获取全部 pending 确认（用户确认 toast）
async fn get_confirmations(
    State(state): State<RemoteAppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let pending = state.app_state.tool_system.confirmation.list_pending();
    Ok(Json(serde_json::json!({ "confirmations": pending })))
}

/// 解决确认请求
#[derive(Deserialize)]
pub struct ConfirmResolveRequest {
    pub action: String,
}

async fn resolve_confirmation(
    State(state): State<RemoteAppState>,
    Path(request_id): Path<u64>,
    Json(req): Json<ConfirmResolveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::tools::confirmation::ConfirmationResponse;
    let response = match req.action.as_str() {
        "deny" => ConfirmationResponse::Deny,
        "allow_once" => ConfirmationResponse::AllowOnce,
        "allow_always" => ConfirmationResponse::AllowAlways,
        other => return Err((StatusCode::BAD_REQUEST, format!("无效的确认动作: {}", other))),
    };
    let resolved = state
        .app_state
        .tool_system
        .confirmation
        .resolve_request(request_id, response);
    Ok(Json(serde_json::json!({ "resolved": resolved })))
}

// ── 模型资源路由（供手机端 live2d 渲染加载）────────────────────────

/// 提供 live2d 模型资源文件
///
/// `path` 形如 `Vivian/Vivian.model3.json`。
/// - release：从加密 bundle 解密获取
/// - dev：从资源目录（public/）读取
async fn get_model_asset(Path(path): Path<String>) -> Response {
    // 防路径穿越
    if path.is_empty() || path.split('/').any(|s| s == "..") {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("invalid path"))
            .unwrap();
    }

    let content_type = crate::bundle_reader::content_type(&path);

    // release：bundle 已初始化则从解密资源读取
    if crate::bundle_reader::is_initialized() {
        if let Some(bytes) = crate::bundle_reader::get(&path) {
            return Response::builder()
                .header("Content-Type", content_type)
                .header("Cache-Control", "no-cache")
                .body(Body::from(bytes))
                .unwrap();
        }
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("asset not found in bundle"))
            .unwrap();
    }

    // dev：从资源目录读取
    let base = crate::utils::path::get_resource_dir();
    let file = base.join(&path);
    if file.is_file() {
        match std::fs::read(&file) {
            Ok(bytes) => {
                return Response::builder()
                    .header("Content-Type", content_type)
                    .header("Cache-Control", "no-cache")
                    .body(Body::from(bytes))
                    .unwrap();
            }
            Err(e) => {
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from(format!("read error: {}", e)))
                    .unwrap();
            }
        }
    }
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("asset not found"))
        .unwrap()
}

/// 读取用户数据目录 images/ 下的图片（供聊天历史中的图片气泡展示）
async fn get_user_image(Path(path): Path<String>) -> Response {
    // 仅允许 images/ 前缀，防路径穿越
    if !path.starts_with("images/") || path.split('/').any(|s| s == "..") {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("invalid path"))
            .unwrap();
    }
    let base = crate::utils::path::get_user_data_dir();
    let file = base.join(&path);
    if !file.is_file() {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("image not found"))
            .unwrap();
    }
    match std::fs::read(&file) {
        Ok(bytes) => {
            let mime = crate::commands::config::detect_image_mime(&bytes);
            Response::builder()
                .header("Content-Type", mime)
                .header("Cache-Control", "no-cache")
                .body(Body::from(bytes))
                .unwrap()
        }
        Err(e) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(format!("read error: {}", e)))
            .unwrap(),
    }
}

// ── 启动服务器 ────────────────────────────────────────────────

pub async fn start_server(app_state: Arc<AppState>, port: u16) {
    let state = RemoteAppState { app_state };
    let frontend_dir = get_frontend_dir();

    let app = Router::new()
        .route("/api/health", get(health_check))
        .route("/api/characters", get(list_characters))
        .route("/api/characters/:id/presence", get(get_presence))
        .route("/api/characters/:id/mood", get(get_mood))
        .route("/api/characters/:id/relationship", get(get_relationship))
        .route("/api/characters/:id/environment", get(get_environment))
        .route("/api/characters/:id/mind", get(get_mind))
        .route("/api/characters/:id/history", get(get_history))
        .route("/api/characters/:id/memories", get(get_memories))
        .route("/api/characters/:id/diary", get(get_diary))
        .route("/api/characters/:id/presence", post(set_presence))
        .route("/api/characters/:id/stop", post(stop_generation))
        .route("/api/chat", post(chat_handler))
        .route("/api/asr", post(asr_handler))
        .route("/api/characters/:id/chat/image", post(image_chat_handler))
        .route("/api/config", get(get_config))
        .route("/api/characters/:id/notes", get(list_notes))
        .route("/api/characters/:id/notes", post(create_note))
        .route("/api/characters/:id/notes/:note_id", get(get_note))
        .route("/api/characters/:id/notes/:note_id/html", get(get_note_html))
        .route("/api/characters/:id/notes/:note_id", axum::routing::put(update_note))
        .route("/api/characters/:id/notes/:note_id", axum::routing::delete(delete_note))
        .route("/api/todos", get(list_todos))
        .route("/api/todos", post(add_todo))
        .route("/api/todos/:id", axum::routing::put(update_todo))
        .route("/api/todos/:id", axum::routing::delete(delete_todo))
        .route("/api/todos/:id/complete", post(complete_todo))
        .route("/api/tasks", get(list_tasks))
        .route("/api/tasks", post(add_task))
        .route("/api/tasks/:id", axum::routing::delete(cancel_task))
        .route("/api/tasks/:id/pause", post(pause_task))
        .route("/api/tasks/:id/resume", post(resume_task))
        .route("/api/characters/:id/profile", get(get_profile))
        .route("/api/characters/:id/profile/types", get(get_profile_types))
        .route("/api/characters/:id/profile/:fact_type", axum::routing::put(set_fact))
        .route("/api/characters/:id/profile/:fact_type", axum::routing::delete(delete_fact))
        .route("/api/characters/:id/profile/:fact_type/pin", post(pin_fact))
        .route("/api/toasts", get(get_toasts))
        .route("/api/confirmations", get(get_confirmations))
        .route("/api/confirmations/:request_id", post(resolve_confirmation))
        .route("/remote/model/*path", get(get_model_asset))
        .route("/remote/image/*path", get(get_user_image))
        .nest_service(
            "/",
            ServeDir::new(&frontend_dir).not_found_service(
                ServeDir::new(&frontend_dir).append_index_html_on_directories(true),
            ),
        )
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("[Remote] 远程访问 HTTP 服务启动: http://{}", addr);

    // 端口变更重启时，旧监听器被 abort 后端口释放存在短暂延迟，
    // 此处带重试绑定，避免立即重启报 AddrInUse。
    let listener = {
        let mut attempt = 0;
        loop {
            match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => break l,
                Err(e) => {
                    attempt += 1;
                    if attempt >= 5 {
                        tracing::error!("[Remote] 监听 {} 失败: {}", addr, e);
                        return;
                    }
                    tracing::warn!(
                        "[Remote] 监听 {} 失败（第 {} 次重试）: {}",
                        addr,
                        attempt,
                        e
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("[Remote] HTTP 服务运行出错: {}", e);
    }
}

// ── 服务器生命周期管理（支持运行时改端口 / 启停）─────────────────

/// 正在运行的远程服务器句柄
struct RemoteServerHandle {
    port: u16,
    task: tauri::async_runtime::JoinHandle<()>,
}

static REMOTE_SERVER: OnceLock<Mutex<Option<RemoteServerHandle>>> = OnceLock::new();

fn remote_server_lock() -> &'static Mutex<Option<RemoteServerHandle>> {
    REMOTE_SERVER.get_or_init(|| Mutex::new(None))
}

/// 根据当前配置同步远程服务器状态（启动 / 停止 / 重启到新端口）。
///
/// 幂等：配置未变化时不重复操作。由配置保存（`save_config`）与启动流程调用，
/// 从而支持在运行时修改监听端口或启用开关，无需重启应用。
pub fn sync_remote_server(app_state: Arc<AppState>) {
    let enabled = app_state
        .config
        .read()
        .get_all()
        .network
        .remote_access
        .enabled;
    let port = app_state
        .config
        .read()
        .get_all()
        .network
        .remote_access
        .port;

    let mut guard = match remote_server_lock().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    let running_port = guard.as_ref().map(|h| h.port);

    match (enabled, running_port) {
        // 未启用：确保已停止
        (false, Some(_)) => {
            if let Some(h) = guard.take() {
                h.task.abort();
                tracing::info!("[Remote] 远程访问已关闭，HTTP 服务已停止");
            }
        }
        // 未启用且未运行：无操作
        (false, None) => {}
        // 已启用，端口未变且正在运行：无操作
        (true, Some(p)) if p == port => {}
        // 已启用：启动或重启到新端口
        (true, _) => {
            if let Some(h) = guard.take() {
                h.task.abort();
                tracing::info!("[Remote] 端口变更，重启 HTTP 服务 -> {}", port);
            }
            let state_for_server = app_state.clone();
            let task = tauri::async_runtime::spawn(async move {
                start_server(state_for_server, port).await;
            });
            *guard = Some(RemoteServerHandle { port, task });
            tracing::info!("[Remote] 远程访问已开启，HTTP 服务监听端口 {}", port);
        }
    }
}

fn get_frontend_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let prod_path = exe_dir.join("remote").join("frontend");
    if prod_path.exists() {
        return prod_path;
    }

    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("remote")
        .join("frontend");
    if dev_path.exists() {
        return dev_path;
    }

    PathBuf::from(".")
}