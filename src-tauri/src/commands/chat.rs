//! 聊天命令 - 消息发送、流式响应与生成控制

use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Timelike;
use serde_json::json;
use tauri::{Emitter, Manager, State};

use crate::cross_character::build_speaker_prefix;
use crate::error::VivianResult;
use crate::memory::types::MemoryType;
use crate::providers::base::LLMRequest;
use crate::resilience::classify_llm_error_from_str;
use crate::state::AppState;
use crate::types::response::{AiResponse, ChatMessage, MessageImage};

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 会话关闭时触发 Episode 封包，让经历边界对齐会话边界
///
/// 从 Conversation 提取 topic/timestamps/memory_ids，调 EpisodeStore::seal_episode。
/// 若 EpisodeStore 未初始化或会话无记忆，静默跳过。
fn seal_episode_on_close(
    brain: &crate::brain::Brain,
    conv: &crate::conversation::Conversation,
) {
    let Some(ep_store) = brain.memory.episode_store() else {
        return;
    };
    let memory_ids = crate::conversation::CONVERSATION_MANAGER.get_session_memory_ids(&conv.id);
    if memory_ids.is_empty() && conv.rounds == 0 {
        return;
    }
    // timestamps 至少包含会话起止时间，importances 用默认值
    let timestamps = vec![conv.created_at, conv.last_active_at];
    let importances = vec![0.5];
    let topic = if conv.topic.is_empty()
        || conv.topic == "(无话题)"
        || conv.topic == crate::conversation::manager::TOPIC_PENDING
    {
        None
    } else {
        Some(conv.topic.clone())
    };
    let episode = ep_store.seal_episode(
        memory_ids,
        &timestamps,
        &importances,
        topic,
        None,
        &[],
    );
    tracing::debug!(
        "[Episode] 会话 {} 封包为 Episode {}",
        conv.id,
        episode.episode_id
    );

    // 会话关闭时清空工作记忆：本会话的临时想法已封包为 Episode，
    // 不应残留到下一会话的 prompt 中污染上下文。
    brain.mind.clear_working_memory();
}

/// 校验主 LLM API 是否已配置
///
/// 主 LLM API（`config.ai` 的 api_key / endpoint / model）是必须配置的；
/// 未配置时返回 false，调用方应发送 `chat:config_error` 事件并终止流程。
fn main_api_configured(state: &State<'_, Arc<AppState>>) -> bool {
    if let Some(router) = state.model_router.read().as_ref() {
        return router.has_main_provider();
    }
    false
}

/// 查询主 LLM API 是否已配置（前端启动门控与配置引导用）
#[tauri::command]
pub fn is_main_api_configured(state: State<'_, Arc<AppState>>) -> bool {
    main_api_configured(&state)
}

/// 发送消息并获取完整响应
#[tauri::command]
pub async fn send_message(
    state: State<'_, Arc<AppState>>,
    message: String,
    character_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());

    if message.trim().is_empty() {
        return Err("消息不能为空".to_string());
    }

    // 主 LLM API 必须配置，否则发 `chat:config_error` 并终止
    if !main_api_configured(&state) {
        let _ = app.emit(
            "chat:config_error",
            json!({ "reason": "no_main_api", "character_id": &char_id }),
        );
        return Err("MAIN_API_NOT_CONFIGURED".to_string());
    }

    let instance = state.get_character(character_id.as_deref())?;
    let brain = instance.brain.clone();

    state.session_coordinator.signal_user_input(&char_id);

    // 串行化 brain.think，与流式路径共用 brain_lock
    let _brain_lock = instance.think_lock.clone();
    let _brain_guard = _brain_lock.lock().await;
    state.reset_generation_cancel(&char_id);

    // 临时开启路由回退事件发送
    if let Some(router) = state.model_router.read().as_ref() {
        router.set_emit_enabled(true);
    }

    // ── 会话生命周期：获取或创建 User↔Agent 会话 ──
    let conv = crate::conversation::CONVERSATION_MANAGER
        .start_or_continue("user", &char_id, &message)
        .unwrap_or_else(|| {
            crate::conversation::CONVERSATION_MANAGER.force_new_session("user", &char_id, &message)
        });
    crate::conversation::CONVERSATION_MANAGER.touch_user_message(&char_id);
    brain.presence.record_user_interaction();
    let _session_guard = state.session_coordinator.enter_user_turn(
        &char_id,
        &conv.id,
        &brain.memory,
        &brain.dialogue,
    );

    // ── World Model：从用户消息抽取预期回归时间 ──
    brain.world_state.ingest_dialogue(&message);
    // 获取焦点租约：think 期间屏蔽其他角色的主动打断
    let _focus_lease = crate::commands::proactive::FocusLeaseGuard::acquire(&char_id);
    let result: VivianResult<AiResponse> = brain.think(&message, false).await;
    drop(_focus_lease);
    if let Some(router) = state.model_router.read().as_ref() {
        router.set_emit_enabled(false);
    }

    // ── 会话生命周期：think 完成后更新会话状态 + 意图判断 close 检测 ──
    {
        let response_mode = result.as_ref().ok().map(|r| r.response_mode.clone()).unwrap_or_else(|| "speak".to_string());
        let reply_text = result.as_ref().ok().map(|r| r.text.clone()).unwrap_or_default();
        let mode = crate::conversation::ResponseMode::from_str(&response_mode);
        let _ = crate::conversation::CONVERSATION_MANAGER.update_after_round(
            &conv.id,
            mode,
            if mode.needs_speech() { Some(&reply_text) } else { None },
            &message,
        );
        // 意图判断：规则预检 + LLM 判断，优先检查用户输入，再检查 Agent 回复
        let history: Vec<String> = brain.dialogue.get_history().iter().map(|m| m.content.clone()).collect();
        let judge = crate::dialogue::intent_judge::IntentJudge::new(
            state.model_router.read().as_ref().map(|r| std::sync::Arc::new(r.clone())),
        );
        let user_close = judge.judge_close_reason(&message, &history).await;
        let agent_close = if user_close.is_none() {
            judge.judge_close_reason(&reply_text, &history).await
        } else {
            None
        };
        if let Some(reason) = user_close.or(agent_close) {
            let closed_conv = crate::conversation::CONVERSATION_MANAGER.close_with_reason(&conv.id, reason);
            seal_episode_on_close(&brain, &conv);
            // Open Loop 检测：关闭的会话也检查是否有未聊完的话题
            if let Some(closed) = closed_conv {
                maybe_mark_open_loop(&closed, &brain).await;
            }
            // 用户说"去忙了"/"我先走了" → 角色也跟着去做自己的事（Online→Busy）
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
    drop(_session_guard);

    drop(_brain_guard);

    let response = result.map_err(err_str)?;
    // 递增人格卡片轮次计数器（驱动冷却机制）
    brain.persona.tick_card_turn();
    serde_json::to_value(&response).map_err(err_str)
}

/// 流式发送消息，通过事件推送响应片段
///
/// `stream_id` 由前端生成，贯穿全链路，用于在多消息并发场景下区分
/// 不同请求的流式输出。后端通过 `brain_lock` 串行化 `brain.think` 调用，
/// 多条消息按发送顺序排队执行，但流式事件通过 `stream_id` 路由到各自气泡。
#[tauri::command]
pub async fn send_message_stream(
    state: State<'_, Arc<AppState>>,
    message: String,
    stream_id: String,
    character_id: Option<String>,
    channel: Option<String>,
    whisper: Option<bool>,
    file_metadata: Option<serde_json::Value>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let channel_str = channel.clone().unwrap_or_else(|| "wechat".to_string());
    let is_whisper = whisper.unwrap_or(false);

    if message.trim().is_empty() {
        return Err("消息不能为空".to_string());
    }

    // 主 LLM API 必须配置，否则发 `chat:config_error` 并终止
    if !main_api_configured(&state) {
        let _ = app.emit(
            "chat:config_error",
            json!({ "reason": "no_main_api", "stream_id": &stream_id, "character_id": &char_id, "channel": &channel_str }),
        );
        return Ok(());
    }

    let instance = match state.get_character(character_id.as_deref()) {
        Ok(inst) => inst,
        Err(_) => {
            let _ = app.emit(
                "chat:error",
                json!({ "error": "Brain 未初始化", "stream_id": &stream_id, "character_id": &char_id, "channel": &channel_str }),
            );
            return Err("Brain 未初始化".to_string());
        }
    };
    let brain = instance.brain.clone();

    // 渠道限制：direct（面对面）在 Offline 状态拒绝；Busy 正常发送但注入忙碌语境
    let current_presence = brain.presence.current();
    let is_busy = current_presence == crate::presence::PresenceState::Busy;

    if channel_str == "direct" && !brain.presence.can_direct() {
        let hint = match current_presence {
            crate::presence::PresenceState::Offline => "对方不在，发微信留言",
            _ => "对方不在场，发微信吧",
        };
        let _ = app.emit(
            "chat:presence_blocked",
            json!({
                "stream_id": &stream_id,
                "character_id": &char_id,
                "presence": current_presence.as_str(),
                "hint": hint,
                "channel": &channel_str,
            }),
        );
        return Ok(());
    }

    // Busy 状态下微信消息延后处理：角色正在忙，不会立即看到微信
    if is_busy && channel_str == "wechat" {
        let _ = app.emit(
            "chat:busy_deferred",
            json!({
                "stream_id": &stream_id,
                "character_id": &char_id,
                "channel": &channel_str,
            }),
        );
        // 等待忙碌任务结束（最多 120 秒），期间每 2 秒检查一次状态
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

    let _ = app.emit(
        "chat:start",
        json!({ "message": &message, "stream_id": &stream_id, "character_id": &char_id, "channel": &channel_str }),
    );

    // ── 用户交互唤醒：从 Rest/Offline 回到 Online ──
    // 用户发起对话视为唤醒行为，角色恢复在线。
    // 注意：wechat 渠道也会唤醒（用户发微信也算交互），但 Rest→Online 只在用户主动发起时触发。
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
                .add_memory_with_metadata(&text, MemoryType::ShortTerm, 0.4, vec!["presence_log".to_string(), "assistant".to_string()], meta)
                .await;
        });
        // 通知前端状态变化
        let _ = app.emit(
            "presence:changed",
            json!({
                "character_id": &char_id,
                "from": event.from,
                "to": event.to,
                "reason": event.reason,
            }),
        );

        // 后端联动 Live2D 窗口可见性：
        // 从 Offline 唤醒时 show 窗口（与 proactive.rs / set_presence_state 保持一致）
        let from_state = crate::presence::PresenceState::from_str(&event.from);
        if matches!(from_state, crate::presence::PresenceState::Offline) {
            if let Some(win) = app.get_webview_window(&char_id) {
                let _ = win.show();
                let _ = win.set_focus();
                tracing::info!(
                    "[Presence:{}] 用户交互唤醒，后端联动 show 窗口（从 Offline 恢复）",
                    char_id
                );
            }
        }
    }

    // ── 跨角色旁观记忆：见下方 brain.think 返回后处理 ──
    // 旁观记忆需要同时包含用户消息和 AI 回复，必须在 think 完成后才能写入。

    let app_for_emitter = app.clone();
    let sid_for_emitter = stream_id.clone();
    let cid_for_emitter = char_id.clone();
    let channel_for_emitter = channel_str.clone();

    let is_direct_channel = channel_str == "direct";

    // 内联表情/动作标签扫描器：当 inline_expression 启用时，
    // 拦截流式文本 chunk，剥离 <e>/<m>/<s> 标签并 emit chat:inline_meta 事件，
    // 让表情/动作在文字流式输出过程中即时切换，无需等待 ExpressionMotionRunnable 的第二次 LLM 调用。
    let inline_enabled = brain.config.inline_expression.enabled;
    let mut inline_scanner: Option<Arc<parking_lot::Mutex<crate::pipeline::inline_tag_scanner::InlineTagScanner>>> = None;
    let mut paren_buffer: Option<Arc<parking_lot::Mutex<String>>> = None;
    let emitter: crate::pipeline::steps::generation::StreamEmitter = if inline_enabled {
        let app_for_meta = app.clone();
        let sid_for_meta = stream_id.clone();
        let cid_for_meta = char_id.clone();
        let ch_for_meta = channel_str.clone();
        let tag_callback: crate::pipeline::inline_tag_scanner::TagCallback =
            Box::new(move |tag| {
                let (tag_type, name, dur) = match &tag {
                    crate::pipeline::inline_tag_scanner::InlineTag::Expression { name, duration_ms } => {
                        ("expression".to_string(), name.clone(), *duration_ms)
                    }
                    crate::pipeline::inline_tag_scanner::InlineTag::Motion { name } => {
                        ("motion".to_string(), name.clone(), None)
                    }
                    crate::pipeline::inline_tag_scanner::InlineTag::Sticker { name } => {
                        ("sticker".to_string(), name.clone(), None)
                    }
                };
                let _ = app_for_meta.emit(
                    "chat:inline_meta",
                    json!({
                        "type": tag_type,
                        "name": name,
                        "duration_ms": dur,
                        "stream_id": &sid_for_meta,
                        "character_id": &cid_for_meta,
                        "channel": &ch_for_meta,
                    }),
                );
            });
        let scanner = Arc::new(parking_lot::Mutex::new(
            crate::pipeline::inline_tag_scanner::InlineTagScanner::new(tag_callback),
        ));
        let scanner_for_chunk = scanner.clone();
        inline_scanner = Some(scanner);

        if is_direct_channel {
            let buffer = Arc::new(parking_lot::Mutex::new(String::new()));
            let buf_for_chunk = buffer.clone();
            paren_buffer = Some(buffer);
            Arc::new(move |chunk: &str| {
                let mut s = scanner_for_chunk.lock();
                let after_inline = s.feed(chunk);
                if !after_inline.is_empty() {
                    let mut buf = buf_for_chunk.lock();
                    let (clean, remaining) = crate::utils::filter_parentheses(&after_inline, &buf);
                    *buf = remaining;
                    if !clean.is_empty() {
                        let _ = app_for_emitter.emit(
                            "chat:chunk",
                            json!({ "text": clean, "stream_id": sid_for_emitter, "character_id": &cid_for_emitter, "channel": &channel_for_emitter }),
                        );
                    }
                }
            })
        } else {
            Arc::new(move |chunk: &str| {
                let mut s = scanner_for_chunk.lock();
                let clean = s.feed(chunk);
                if !clean.is_empty() {
                    let _ = app_for_emitter.emit(
                        "chat:chunk",
                        json!({ "text": clean, "stream_id": sid_for_emitter, "character_id": &cid_for_emitter, "channel": &channel_for_emitter }),
                    );
                }
            })
        }
    } else if is_direct_channel {
        let buffer = Arc::new(parking_lot::Mutex::new(String::new()));
        let buf_for_chunk = buffer.clone();
        paren_buffer = Some(buffer);
        Arc::new(move |chunk: &str| {
            let mut buf = buf_for_chunk.lock();
            let (clean, remaining) = crate::utils::filter_parentheses(chunk, &buf);
            *buf = remaining;
            if !clean.is_empty() {
                let _ = app_for_emitter.emit(
                    "chat:chunk",
                    json!({ "text": clean, "stream_id": sid_for_emitter, "character_id": &cid_for_emitter, "channel": &channel_for_emitter }),
                );
            }
        })
    } else {
        Arc::new(move |chunk: &str| {
            let _ = app_for_emitter.emit(
                "chat:chunk",
                json!({ "text": chunk, "stream_id": sid_for_emitter, "character_id": &cid_for_emitter, "channel": &channel_for_emitter }),
            );
        })
    };

    // 串行化 brain.think：同一时刻只处理一个请求，避免对话历史/记忆/心理系统并发写入污染。
    // 排队中的请求在此等待；获取锁后重置取消标志（为当前请求清空上次残留）。
    // 记录排队时刻：brain.think 完成后用此时刻修正用户消息的 timestamp，
    // 确保并发发送时用户消息按实际发送顺序排列（而非 think 完成顺序）。
    let queued_at = chrono::Local::now();
    state.session_coordinator.signal_user_input(&char_id);
    let _brain_lock = instance.think_lock.clone();
    let _brain_guard = _brain_lock.lock().await;
    state.reset_generation_cancel(&char_id);

    // 设置消息渠道标记（影响 dialogue 写入的 metadata.channel）
    brain.dialogue.set_channel(&channel_str);

    // ── 会话生命周期：获取或创建 User↔Agent 会话 ──
    let conv = crate::conversation::CONVERSATION_MANAGER
        .start_or_continue("user", &char_id, &message)
        .unwrap_or_else(|| {
            crate::conversation::CONVERSATION_MANAGER.force_new_session("user", &char_id, &message)
        });
    crate::conversation::CONVERSATION_MANAGER.touch_user_message(&char_id);
    brain.presence.record_user_interaction();
    let _session_guard = state.session_coordinator.enter_user_turn(
        &char_id,
        &conv.id,
        &brain.memory,
        &brain.dialogue,
    );

    brain.set_stream_emitter(Some(emitter));

    // 临时开启路由回退事件发送（仅本次用户消息期间）
    if let Some(router) = state.model_router.read().as_ref() {
        router.set_emit_enabled(true);
    }
    // ── World Model：从用户消息抽取预期回归时间 ──
    // 用户说"我去洗澡，20分钟" → ExpectationEngine 抽取 20min 预期，存入 UserEntityState。
    // 后续 proactive tick 检测到用户离开时，用此预期判断是否超时。
    brain.world_state.ingest_dialogue(&message);
    // 获取焦点租约：流式 think 期间屏蔽其他角色的主动打断
    let _focus_lease = crate::commands::proactive::FocusLeaseGuard::acquire(&char_id);
    // Busy 状态下 direct 渠道：注入忙碌被呼唤的语境
    let think_input = if is_busy && channel_str == "direct" {
        format!("（你从忙碌状态下被用户呼唤）\n{}", message)
    } else {
        message.clone()
    };
    let result: VivianResult<AiResponse> = brain.think(&think_input, true).await;
    drop(_focus_lease);
    // 修正用户消息 timestamp 为排队时刻：brain.think 在完成后才写入用户消息，
    // 默认 timestamp 是 think 完成时刻，并发发送时会导致顺序错乱
    // （q2 的 timestamp 会大于 a1 的 timestamp）。用排队时刻覆盖确保正确排序。
    brain.dialogue.patch_last_user_timestamp(queued_at);
    // 文件消息结构化标记：brain.think 已在对话历史写入用户消息，
    // 若本次为文件消息，将 kind=file 等元信息追加到刚写入的最后一条用户消息。
    // patch_last_user_entry_metadata 在 buffer 未命中时会做全文件读写（read_to_string + write），
    // 移入 spawn_blocking 避免卡住 async 运行时。
    if let Some(meta) = file_metadata.clone() {
        let dialogue = brain.dialogue.clone();
        let _ = tokio::task::spawn_blocking(move || {
            dialogue.patch_last_user_entry_metadata(meta);
        })
        .await;
    }

    // ── 用户内容入向量知识库 ──
    // 1. 文件消息：把文件提取的文本内容入库（source="user_file"，永不过期）
    // 2. 网页链接：检测消息中的 URL，异步抓取页面正文入库（source="user_link"）
    // 两者均为 fire-and-forget，不阻塞主对话流程，失败只记日志。
    {
        let memory = brain.memory.clone();
        let msg_for_kb = message.clone();
        let char_id_for_kb = char_id.clone();
        let file_meta_for_kb = file_metadata.clone();
        tokio::spawn(async move {
            // 文件入库
            if let Some(meta) = file_meta_for_kb {
                if let Some(file_name) = meta.get("file_name").and_then(|v| v.as_str()) {
                    // 消息格式为 "[文件：filename]\n<文本内容>"，剥离前缀拿到纯文本
                    let prefix = format!("[文件：{}]\n", file_name);
                    let content = if msg_for_kb.starts_with(&prefix) {
                        msg_for_kb[prefix.len()..].to_string()
                    } else {
                        msg_for_kb.clone()
                    };
                    if !content.trim().is_empty() {
                        let title = format!("文件：{}", file_name);
                        match memory
                            .add_knowledge_document(
                                &title,
                                &content,
                                vec!["user_file".to_string()],
                                "user_file",
                                Some(-1),
                            )
                            .await
                        {
                            Ok(item) => tracing::info!(
                                "[Knowledge] 用户文件「{}」已入库，memory_id={}",
                                file_name,
                                item.id
                            ),
                            Err(e) => tracing::warn!("[Knowledge] 文件入库失败: {}", e),
                        }
                    }
                }
            } else {
                // URL 抓取入库（仅非文件消息检测，避免与文件消息冲突）
                if let Some(url) = crate::network::url_fetcher::extract_first_url(&msg_for_kb) {
                    tracing::info!("[Knowledge] 检测到用户分享链接，开始抓取: {}", url);
                    match crate::network::url_fetcher::fetch_page(&url).await {
                        Ok(page) => {
                            let tags = vec!["user_link".to_string()];
                            match memory
                                .add_knowledge_document(
                                    &page.title,
                                    &page.text,
                                    tags,
                                    "user_link",
                                    Some(-1),
                                )
                                .await
                            {
                                Ok(item) => tracing::info!(
                                    "[Knowledge] 用户链接「{}」已入库，memory_id={}",
                                    page.title,
                                    item.id
                                ),
                                Err(e) => tracing::warn!("[Knowledge] 链接入库失败: {}", e),
                            }
                        }
                        Err(e) => tracing::warn!(
                            "[Knowledge] 抓取链接 {} 失败: {}",
                            url,
                            e
                        ),
                    }
                }
            }
            let _ = char_id_for_kb;
        });
    }

    if let Some(router) = state.model_router.read().as_ref() {
        router.set_emit_enabled(false);
    }

    // ── 内联扫描器 flush：将跨 chunk 缓冲区的残余文本（不完整的标签前缀）输出 ──
    if let Some(ref scanner) = inline_scanner {
        let mut s = scanner.lock();
        let remaining = s.flush();
        if !remaining.is_empty() {
            let _ = app.emit(
                "chat:chunk",
                json!({ "text": remaining, "stream_id": &stream_id, "character_id": &char_id, "channel": &channel_str }),
            );
        }
    }

    // ── direct 模式括号缓冲区 flush：将未闭合括号的残余文本输出 ──
    if let Some(ref buffer) = paren_buffer {
        let mut buf = buffer.lock();
        let remaining = std::mem::take(&mut *buf);
        if !remaining.is_empty() {
            let _ = app.emit(
                "chat:chunk",
                json!({ "text": remaining, "stream_id": &stream_id, "character_id": &char_id, "channel": &channel_str }),
            );
        }
    }

    // ── 会话生命周期：think 完成后更新会话状态 ──
    // 根据本轮 response_mode（speak/non_verbal/internal/ignore）更新 Energy/Novelty/Continuation。
    // 同时做意图判断：若用户或 Agent 输出命中关闭意图 → 立即 close_with_reason。
    {
        let response_mode = result.as_ref().ok().map(|r| r.response_mode.clone()).unwrap_or_else(|| "speak".to_string());
        let reply_text = result.as_ref().ok().map(|r| r.text.clone()).unwrap_or_default();
        let mode = crate::conversation::ResponseMode::from_str(&response_mode);
        let _ = crate::conversation::CONVERSATION_MANAGER.update_after_round(
            &conv.id,
            mode,
            if mode.needs_speech() { Some(&reply_text) } else { None },
            &message,
        );

        // 意图判断：规则预检 + LLM 判断，优先检查用户输入，再检查 Agent 回复
        let history: Vec<String> = brain.dialogue.get_history().iter().map(|m| m.content.clone()).collect();
        let judge = crate::dialogue::intent_judge::IntentJudge::new(
            state.model_router.read().as_ref().map(|r| std::sync::Arc::new(r.clone())),
        );
        let user_close = judge.judge_close_reason(&message, &history).await;
        let agent_close = if user_close.is_none() {
            judge.judge_close_reason(&reply_text, &history).await
        } else {
            None
        };
        if let Some(reason) = user_close.or(agent_close) {
            let closed_conv = crate::conversation::CONVERSATION_MANAGER.close_with_reason(&conv.id, reason);
            tracing::debug!(
                "[Conversation] 会话 {} 因意图判断关闭，原因: {}",
                conv.id,
                reason.as_str()
            );
            // 触发 Episode 封包：让经历边界对齐会话边界
            seal_episode_on_close(&brain, &conv);
            // Open Loop 检测：关闭的会话也检查是否有未聊完的话题
            if let Some(closed) = closed_conv {
                maybe_mark_open_loop(&closed, &brain).await;
            }
            // 用户说"去忙了"/"我先走了" → 角色也跟着去做自己的事（Online→Busy）
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

    // 清理流式回调（避免非流式调用误触发）
    brain.set_stream_emitter(None);
    // 重置消息渠道标记为默认值
    brain.dialogue.set_channel("wechat");
    drop(_session_guard);
    // 释放串行化锁，让排队中的下一个请求开始执行
    drop(_brain_guard);

    if state.is_generation_cancelled(&char_id) {
        let _ = app.emit("chat:cancelled", json!({ "stream_id": &stream_id, "character_id": &char_id, "channel": &channel_str }));
        return Ok(());
    }

    match result {
        Ok(response) => {
            // 空文本或 response_mode=ignore：跳过 chat:meta/chat:done 发射，避免向微信界面发送空消息
            if response.text.trim().is_empty() || response.response_mode == "ignore" {
                tracing::info!(
                    "[Chat:{}] 空文本或 ignore 模式，跳过 chat:done 发射: stream_id={}",
                    char_id,
                    stream_id
                );
                return Ok(());
            }
            // 推送 meta 事件：expression/motion/sticker（前端提前播放 Live2D 动画 + 表情包弹窗）
            if !response.expression.is_empty() || response.motion != "idle" || !response.sticker.is_empty() {
                let _ = app.emit(
                    "chat:meta",
                    json!({
                        "expression": response.expression,
                        "expression_duration_ms": response.expression_duration_ms,
                        "motion": response.motion,
                        "sticker": response.sticker,
                        "source": "formal",
                        "stream_id": &stream_id,
                        "character_id": &char_id,
                        "channel": &channel_str,
                    }),
                );
            }
            let display_text = if is_direct_channel {
                crate::utils::filter_parentheses_sync(&response.text)
            } else {
                response.text.clone()
            };

            // 微信渠道语音消息：LLM 返回 voice_message=true 时，合成 TTS 音频文件，
            // 前端以微信风格语音气泡展示（不显示文本），点击可播放。
            // direct 渠道已有实时 TTS 播放，不需要语音消息模式。
            let (voice_audio_path, voice_duration) = if response.voice_message
                && !is_direct_channel
                && brain.tts.is_enabled()
            {
                match brain
                    .tts
                    .synthesize_to_file(&display_text, None)
                    .await
                {
                    Ok((path, dur)) => {
                        // 回写 dialogue 历史 metadata，使刷新历史时能恢复语音气泡
                        brain.dialogue.patch_last_assistant_entry_metadata(json!({
                            "kind": "voice",
                            "audio_path": &path,
                            "duration": dur,
                        }));
                        (Some(path), Some(dur))
                    }
                    Err(e) => {
                        tracing::warn!("[Chat:{}] 语音消息合成失败，回退为文本: {}", char_id, e);
                        (None, None)
                    }
                }
            } else {
                (None, None)
            };

            let _ = app.emit(
                "chat:done",
                json!({
                    "text": display_text,
                    "motion": response.motion,
                    "expression": response.expression,
                    "expression_duration_ms": response.expression_duration_ms,
                    "emotion_score": response.emotion_score,
                    "sticker": response.sticker,
                    "user_emotion": response.user_emotion,
                    "voice_message": response.voice_message && voice_audio_path.is_some(),
                    "voice_audio_path": voice_audio_path,
                    "voice_duration": voice_duration,
                    "source": "formal",
                    "stream_id": &stream_id,
                    "character_id": &char_id,
                    "channel": &channel_str,
                }),
            );
            // 递增人格卡片轮次计数器（驱动冷却机制）
            brain.persona.tick_card_turn();

            // ── 第三者旁观记忆（逐条写入带第三人称前缀的独立记忆条目）──
            // 用户与角色 A 对话时，如果角色 B 在线，B 以旁观者视角记录对话。
            // 设计原则：
            // - 用户消息和角色回复分别写入独立记忆条目，而非机械拼接
            // - 每条记忆带第三人称前缀（如 "[User says to Vivian]", "[Vivian says to User]"）
            // - importance = 原对话 importance × 0.6（旁观价值低于参与）
            // - metadata 标记 perspective=observer + speaker + listener
            // - 前端通过 perspective=observer 识别为旁观对话，用说话者主题色半透明节点显示
            // 悄悄话模式（is_whisper=true）跳过旁观记忆，其他在线角色不会听到此对话
            // 群聊渠道（wechat_group）跳过旁观记忆，每个角色已通过各自的 send_message_stream 直接记录
            // 微信私聊（wechat）是角色私有对话，其他角色不应旁观
            // 仅 direct（桌宠直接对话）才被其他在线角色旁观
            if !is_whisper && channel_str == "direct" {
                let speaker_id = char_id.clone();
                let user_msg_full = message.trim().to_string();
                let agent_reply_full = response.text.trim().to_string();
                let channel_clone = channel_str.clone();
                let app_clone = app.clone();
                // 原对话 importance（取 user/ai 较大值）× 0.6 折扣
                let base_importance = response.importance_user.max(response.importance_ai);
                let observer_importance = (base_importance * 0.6).clamp(0.05, 0.85);
                // 说话角色的名字（用于 roommate_cue 的 from_name 参数）
                let speaker_name = {
                    let chars = state.characters.read();
                    chars
                        .get(&speaker_id)
                        .map(|inst| inst.name.clone())
                        .unwrap_or_else(|| speaker_id.clone())
                };
                // 仅提取旁观所需字段，避免克隆整个 HashMap
                // 主动旁观插话需要额外访问 brain/think_lock/psychology/persona/dialogue
                let observers: Vec<_> = {
                    let chars = state.characters.read();
                    chars
                        .iter()
                        .filter(|(id, _)| *id != &speaker_id)
                        .map(|(id, inst)| {
                            (
                                id.clone(),
                                inst.online.clone(),
                                inst.brain.memory.clone(),
                                inst.brain.proactive.clone(),
                                inst.brain.psychology.clone(),
                                inst.brain.persona.clone(),
                                inst.brain.dialogue.clone(),
                                inst.name.clone(),
                                inst.brain.clone(),
                                inst.think_lock.clone(),
                            )
                        })
                        .collect()
                };
                let playback_gate = state.playback_gate.clone();
                tokio::spawn(async move {
                    for (other_id, online_lock, observer_memory, observer_proactive, observer_psychology, observer_persona, observer_dialogue, observer_name, observer_brain, observer_think_lock) in observers {
                        // 仅在线角色能旁观（Online 状态；Busy/Rest/Offline 不旁观）
                        if !*online_lock.read() {
                            continue;
                        }
                        // 旁观者视角：用户对说话角色说的话
                        let user_prefix = build_speaker_prefix("user", &speaker_id, &other_id);
                        let user_observation = format!("{} {}", user_prefix, user_msg_full);
                        let user_meta = json!({
                            "channel": channel_clone,
                            "speaker": "user",
                            "listener": speaker_id,
                            "perspective": "observer",
                            "knowledge_source": "observed",
                            "reliability": "second_hand",
                            "observer_id": other_id,
                        });
                        if let Err(e) = observer_memory
                            .add_memory_with_metadata(
                                &user_observation,
                                MemoryType::CasualConversation,
                                observer_importance,
                                vec!["dialogue".to_string(), "observer".to_string(), "overheard".to_string(), "bystander".to_string()],
                                user_meta,
                            )
                            .await
                        {
                            tracing::warn!(
                                "[Chat] 旁观者 {} 写入用户发言旁观记忆失败: {}",
                                other_id,
                                e
                            );
                        }
                        // 旁观者视角：说话角色回复用户的话
                        let agent_prefix = build_speaker_prefix(&speaker_id, "user", &other_id);
                        let agent_observation = format!("{} {}", agent_prefix, agent_reply_full);
                        let agent_meta = json!({
                            "channel": channel_clone,
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
                                observer_importance,
                                vec!["dialogue".to_string(), "observer".to_string(), "overheard".to_string(), "bystander".to_string()],
                                agent_meta,
                            )
                            .await
                        {
                            tracing::warn!(
                                "[Chat] 旁观者 {} 写入角色回复旁观记忆失败: {}",
                                other_id,
                                e
                            );
                        }

                        // 通知前端：在线角色"旁观"了对话
                        let _ = app_clone.emit(
                            "presence:overheard",
                            json!({
                                "speaker_id": &speaker_id,
                                "listener_id": other_id,
                            }),
                        );

                        // 三人共处一室语义：以低概率 cue 旁观者，让它有机会插话加入对话
                        // 不需要说话角色在文本里真的提到旁观者，系统层 cue 即可触发旁观者的插话意愿
                        if crate::proactive::roll_with_probability(0.08) {
                            let topic_brief: String = user_msg_full.chars().take(20).collect();
                            observer_proactive.seed_roommate_cue(&speaker_name, &topic_brief);
                        }

                        // 主动旁观插话评估：用轻量 LLM 判断 B 是否有动机插话
                        // 绕过概率 roll，每次用户普通消息都进行 LLM 判断，提升插话的合理性
                        // 轻量调用只判断是否插话，不生成内容——插话内容由主对话流程生成
                        let mood = observer_psychology.compute_mood();
                        let mood_hint = format!(
                            "主导情绪：{}，疲劳度：{:.0}",
                            mood.primary_emotion.as_str(),
                            mood.fatigue
                        );
                        let intimacy = observer_psychology.relationship().intimacy;
                        let hour = chrono::Local::now().hour();
                        let lang = observer_persona.get_language();
                        // 修复情绪参数注入：使用 build_style_prompt_ex 传入主导情绪，让场景选择感知内心
                        let system_prompt = observer_persona.build_style_prompt_ex(
                            intimacy,
                            hour,
                            Some(mood.primary_emotion.as_str().to_string()),
                            None,
                        );

                        // 旁观记忆作为最近对话参考（不含本轮刚写入的，避免时序竞争）
                        let bystander_memos =
                            observer_memory.recent_by_tags(&["bystander", "overheard"], 3);
                        let dialogue_history = bystander_memos
                            .iter()
                            .map(|m| m.content.as_str())
                            .collect::<Vec<_>>()
                            .join("\n");

                        if let Some(interjection_directive) = observer_proactive
                            .evaluate_active_bystander_interjection(
                                &user_msg_full,
                                &agent_reply_full,
                                &speaker_name,
                                &mood_hint,
                                &dialogue_history,
                                &system_prompt,
                                intimacy,
                                &lang,
                            )
                            .await
                        {
                            // 决定插话：spawn 延迟投递任务，让 A 先说完再插话
                            let app_clone2 = app_clone.clone();
                            let playback_gate_clone = playback_gate.clone();
                            let other_id_clone = other_id.clone();
                            let other_name_clone = observer_name.clone();
                            let observer_memory_clone = observer_memory.clone();
                            let observer_dialogue_clone = observer_dialogue.clone();
                            let observer_brain_clone = observer_brain.clone();
                            let observer_think_lock_clone = observer_think_lock.clone();
                            tokio::spawn(async move {
                                // 基础延迟：让 A 的气泡/TTS 先显示
                                tokio::time::sleep(Duration::from_secs(3)).await;

                                // 等待 TTS 播放完成（最多再等 12 秒），避免音频冲突
                                for _ in 0..24 {
                                    if !playback_gate_clone.is_playing() {
                                        break;
                                    }
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                }

                                // 调用主对话流程生成插话内容
                                // 串行化 brain.think，与 send_message_stream 共用 brain_lock
                                let _brain_lock = observer_think_lock_clone.clone();
                                let _brain_guard = _brain_lock.lock().await;

                                // 设置渠道为 proactive，让 dialogue 写入标记为主动气泡路径
                                let prev_channel = observer_dialogue_clone.get_channel();
                                observer_dialogue_clone.set_channel("proactive");

                                // 非流式调用主对话流程，插话指令作为 user_input
                                // 完整 prompt 会包含人设/记忆/情绪/工具等，插话指令出现在末尾
                                let result = observer_brain_clone
                                    .think(&interjection_directive, false)
                                    .await;

                                // 恢复渠道
                                observer_dialogue_clone.set_channel(&prev_channel);
                                drop(_brain_guard);

                                let response = match result {
                                    Ok(r) => r,
                                    Err(e) => {
                                        tracing::warn!(
                                            "[Chat] 旁观者 {}({}) 插话生成失败: {}",
                                            other_name_clone,
                                            other_id_clone,
                                            e
                                        );
                                        return;
                                    }
                                };

                                let text = response.text;
                                if text.trim().is_empty() {
                                    tracing::debug!(
                                        "[Chat] 旁观者 {}({}) 插话生成为空，跳过",
                                        other_name_clone,
                                        other_id_clone
                                    );
                                    return;
                                }
                                let expression = response.expression;

                                // 写入记忆系统
                                let meta = json!({
                                    "channel": "proactive",
                                    "speaker": &other_id_clone,
                                    "listener": "user",
                                    "perspective": "speaker",
                                    "knowledge_source": "direct",
                                    "content_type": "bystander_interjection",
                                });
                                let _ = observer_memory_clone
                                    .add_memory_with_metadata(
                                        &text,
                                        MemoryType::CasualConversation,
                                        0.3,
                                        vec![
                                            "assistant".to_string(),
                                            "proactive".to_string(),
                                            "dialogue_turn".to_string(),
                                            "bystander_interjection".to_string(),
                                        ],
                                        meta,
                                    )
                                    .await;

                                // 更新 LAST_SPOKEN，参与跨角色冷却仲裁
                                crate::commands::proactive::touch_last_spoken(&other_id_clone);

                                // emit proactive:bubble 事件，前端监听后 showBubble + TTS
                                let _ = app_clone2.emit(
                                    "proactive:bubble",
                                    json!({
                                        "character_id": &other_id_clone,
                                        "content": &text,
                                        "expression": &expression,
                                    }),
                                );

                                tracing::info!(
                                    "[Chat] 旁观者 {}({}) 主动插话: {}",
                                    other_name_clone,
                                    other_id_clone,
                                    text
                                );
                            });
                        }
                    }
                });
            }

            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            let error_kind = classify_llm_error_from_str(&msg);
            let error_type = match &error_kind {
                crate::resilience::LlmErrorKind::InvalidApiKey => "invalid_api_key",
                crate::resilience::LlmErrorKind::InsufficientBalance => "insufficient_balance",
                crate::resilience::LlmErrorKind::QuotaExceeded => "api_quota_exceeded",
                crate::resilience::LlmErrorKind::RateLimited => "rate_limited",
                crate::resilience::LlmErrorKind::Timeout => "timeout",
                crate::resilience::LlmErrorKind::NetworkError => "network_error",
                crate::resilience::LlmErrorKind::ModelNotFound => "model_not_found",
                crate::resilience::LlmErrorKind::ContextLengthExceeded => "context_length",
                crate::resilience::LlmErrorKind::ContentPolicy => "content_policy",
                crate::resilience::LlmErrorKind::ServerError | crate::resilience::LlmErrorKind::Overloaded => "server_error",
                crate::resilience::LlmErrorKind::CircuitBreakerOpen => "circuit_breaker",
                _ if msg.contains("MAIN_API_NOT_CONFIGURED") => "no_main_api",
                _ => "unknown",
            };
            let character_text = crate::engine::feedback::system_error_to_character_text(error_type);
            let _ = app.emit(
                "chat:error",
                json!({
                    "error": &msg,
                    "character_text": &character_text,
                    "error_type": error_type,
                    "error_kind": error_kind,
                    "stream_id": &stream_id,
                    "character_id": &char_id,
                    "channel": &channel_str,
                }),
            );
            Err(msg)
        }
    }
}

/// 停止指定角色的生成
#[tauri::command]
pub async fn stop_generation(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let char_id = character_id
        .unwrap_or_else(|| state.active_character_id.read().clone());
    state.set_generation_cancel(&char_id, true);
    tracing::info!("已请求停止角色 {} 的生成", char_id);
    Ok(())
}

/// 用前端渲染时刻的时间戳覆盖最后一条 assistant 消息的 timestamp
///
/// 后端持久化 AI 消息时用的是后端构造消息时刻，前端 `chat:done` 渲染时用的是
/// `Date.now()`。两者不一致会导致 `refreshHistory` 合并时按时间戳过滤保留
/// 流式消息造成重复。此命令让存储的时间戳与前端渲染时刻对齐。
#[tauri::command]
pub async fn update_last_assistant_timestamp(
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
    timestamp_ms: i64,
) -> Result<(), String> {
    let instance = state.get_character(character_id.as_deref())?;
    let dialogue = instance.brain.dialogue.clone();
    tokio::task::spawn_blocking(move || dialogue.patch_last_assistant_timestamp(timestamp_ms))
        .await
        .map_err(|e| format!("任务执行失败: {}", e))?;
    Ok(())
}

/// 从休息/忙碌状态唤醒角色
///
/// 用户通过连续点击 Live2D 模型触发：
/// 1. 切换 presence 到 Online + 写 presence_log 记忆 + emit presence:changed
/// 2. 触发一次主交互（brain.think 走完整 chat_chain：心情/表情/记忆/对话历史）
/// 3. 流式 emit chat:meta / chat:chunk / chat:done（前端 ChatController 接收并展示气泡+TTS）
///
/// 与 send_message_stream 的区别：不写用户消息到前端 store，唤醒语境由后端构造注入。
#[tauri::command]
pub async fn wake_from_presence(
    state: State<'_, Arc<AppState>>,
    app: tauri::AppHandle,
    character_id: Option<String>,
    stream_id: String,
) -> Result<(), String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let channel_str = "direct";

    if !main_api_configured(&state) {
        let _ = app.emit(
            "chat:config_error",
            json!({ "reason": "no_main_api", "stream_id": &stream_id, "character_id": &char_id, "channel": channel_str }),
        );
        return Ok(());
    }

    let instance = match state.get_character(character_id.as_deref()) {
        Ok(inst) => inst,
        Err(_) => {
            let _ = app.emit(
                "chat:error",
                json!({ "error": "Brain 未初始化", "stream_id": &stream_id, "character_id": &char_id, "channel": channel_str }),
            );
            return Err("Brain 未初始化".to_string());
        }
    };
    let brain = instance.brain.clone();

    // 切换 presence 状态 + 写 presence_log 记忆 + emit presence:changed
    let from_state = brain.presence.current();
    let transition_event = brain.presence.transition(
        crate::presence::PresenceState::Online,
        crate::presence::PresenceChangeReason::UserInteraction,
    );

    // 任务进行中：transition(Online) 被延迟（仅标记 pending_exit_to_online）。
    // 此时给前端 toast 提示，不进入对话流程（任务结束后会自动切回 Online）。
    if transition_event.is_none() && brain.presence.has_pending_exit() {
        let task_kind = if from_state == crate::presence::PresenceState::Busy {
            "knowledge_acquisition"
        } else {
            "memory_consolidation"
        };
        let hint = match from_state {
            crate::presence::PresenceState::Busy => "我手上的事还没做完，等我一下",
            crate::presence::PresenceState::Rest => "我正在整理记忆，等我做完就好",
            _ => "等我把手上的事做完",
        };
        let _ = app.emit(
            "presence:wake_deferred",
            json!({
                "character_id": &char_id,
                "stream_id": &stream_id,
                "from_state": from_state.as_str(),
                "task": task_kind,
                "hint": hint,
            }),
        );
        return Ok(());
    }

    if let Some(ref ev) = transition_event {
        let from_ps = crate::presence::PresenceState::from_str(&ev.from);
        let to_ps = crate::presence::PresenceState::from_str(&ev.to);
        if matches!(to_ps, crate::presence::PresenceState::Online)
            && matches!(from_ps, crate::presence::PresenceState::Rest | crate::presence::PresenceState::Offline)
        {
            brain.proactive.signal_waking_up();
        }

        let memory_text = brain.presence.memory_text(ev);
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
                .add_memory_with_metadata(&text, MemoryType::ShortTerm, 0.4, vec!["presence_log".to_string(), "assistant".to_string()], meta)
                .await;
        });
        let _ = app.emit(
            "presence:changed",
            json!({
                "character_id": &char_id,
                "from": ev.from,
                "to": ev.to,
                "reason": ev.reason,
            }),
        );

        // 后端联动 Live2D 窗口可见性：
        // 从 Offline 唤醒时 show 窗口（与 send_message_stream 路径保持一致）
        if matches!(from_ps, crate::presence::PresenceState::Offline) {
            if let Some(win) = app.get_webview_window(&char_id) {
                let _ = win.show();
                let _ = win.set_focus();
                tracing::info!(
                    "[Presence:{}] 连续点击唤醒，后端联动 show 窗口（从 Offline 恢复）",
                    char_id
                );
            }
        }
    }

    // 构造唤醒语境消息（由后端注入，前端不显示用户消息）
    let message = match from_state {
        crate::presence::PresenceState::Rest => "（用户通过连续点击把你从休息中唤醒）".to_string(),
        crate::presence::PresenceState::Busy => "（用户轻轻打断了你正在忙碌的事）".to_string(),
        _ => "（用户唤回了你的注意）".to_string(),
    };

    // 创建流式 emitter
    let app_for_emitter = app.clone();
    let sid_for_emitter = stream_id.clone();
    let cid_for_emitter = char_id.clone();
    let channel_for_emitter = channel_str.to_string();
    let emitter: crate::pipeline::steps::generation::StreamEmitter =
        Arc::new(move |chunk: &str| {
            let _ = app_for_emitter.emit(
                "chat:chunk",
                json!({ "text": chunk, "stream_id": sid_for_emitter, "character_id": &cid_for_emitter, "channel": &channel_for_emitter }),
            );
        });

    // 串行化 brain.think
    let _brain_lock = instance.think_lock.clone();
    let _brain_guard = _brain_lock.lock().await;
    state.reset_generation_cancel(&char_id);

    brain.dialogue.set_channel("direct");
    brain.set_stream_emitter(Some(emitter));

    if let Some(router) = state.model_router.read().as_ref() {
        router.set_emit_enabled(true);
    }
    // 获取焦点租约：唤醒 think 期间屏蔽其他角色的主动打断
    let _focus_lease = crate::commands::proactive::FocusLeaseGuard::acquire(&char_id);
    let result: VivianResult<AiResponse> = brain.think(&message, true).await;
    drop(_focus_lease);
    if let Some(router) = state.model_router.read().as_ref() {
        router.set_emit_enabled(false);
    }

    brain.set_stream_emitter(None);
    brain.dialogue.set_channel("wechat");
    drop(_brain_guard);

    if state.is_generation_cancelled(&char_id) {
        let _ = app.emit("chat:cancelled", json!({ "stream_id": &stream_id, "character_id": &char_id, "channel": channel_str }));
        return Ok(());
    }

    match result {
        Ok(response) => {
            // 空文本或 response_mode=ignore：跳过 chat:meta/chat:done 发射，避免向微信界面发送空消息
            if response.text.trim().is_empty() || response.response_mode == "ignore" {
                tracing::info!(
                    "[Chat:{}] 空文本或 ignore 模式，跳过 chat:done 发射: stream_id={}",
                    char_id,
                    stream_id
                );
                return Ok(());
            }
            if !response.expression.is_empty() || response.motion != "idle" || !response.sticker.is_empty() {
                let _ = app.emit(
                    "chat:meta",
                    json!({
                        "expression": response.expression,
                        "expression_duration_ms": response.expression_duration_ms,
                        "motion": response.motion,
                        "sticker": response.sticker,
                        "source": "formal",
                        "stream_id": &stream_id,
                        "character_id": &char_id,
                        "channel": channel_str,
                    }),
                );
            }
            let _ = app.emit(
                "chat:done",
                json!({
                    "text": response.text,
                    "motion": response.motion,
                    "expression": response.expression,
                    "expression_duration_ms": response.expression_duration_ms,
                    "emotion_score": response.emotion_score,
                    "sticker": response.sticker,
                    "user_emotion": response.user_emotion,
                    "voice_message": false,
                    "voice_audio_path": null,
                    "voice_duration": null,
                    "source": "formal",
                    "stream_id": &stream_id,
                    "character_id": &char_id,
                    "channel": channel_str,
                }),
            );
            brain.persona.tick_card_turn();
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = app.emit(
                "chat:error",
                json!({ "error": &msg, "stream_id": &stream_id, "character_id": &char_id, "channel": channel_str }),
            );
            Err(msg)
        }
    }
}

/// 从 LLM 返回中解析图片描述 JSON
///
/// 约定 LLM 返回 `{"description":"...","reply":"..."}`。
/// 解析失败时退化为：description 与 reply 均使用原始文本。
pub(crate) fn parse_image_description_response(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    // 尝试剥离 markdown 代码块围栏
    let body = if trimmed.starts_with("```") {
        let inner = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        inner
    } else {
        trimmed
    };
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
        let description = val
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let reply = val
            .get("reply")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !description.is_empty() || !reply.is_empty() {
            return (description, reply);
        }
    }
    let fallback = raw.trim().to_string();
    (fallback.clone(), fallback)
}

/// 发送本地图片消息
///
/// 流程：
/// 1. 读取图片 → base64 data URL，副本保存到 `<user_data_dir>/images/`。
/// 2. 立即 emit `chat:user_image`，前端渲染用户图片气泡。
/// 3. 把用户图片消息写入对话历史（metadata 标记 kind=image + image_path）。
/// 4. 调用多模态 LLM（主 API）生成图片描述 + 对用户的回应。
/// 5. emit `chat:start` / `chat:done`，前端渲染 AI 文字回复气泡。
/// 6. 把图片描述写入记忆系统（content=description，metadata 携带 image_path）。
#[tauri::command]
pub async fn send_image_message(
    state: State<'_, Arc<AppState>>,
    source_path: String,
    character_id: Option<String>,
    channel: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let channel_str = channel.unwrap_or_else(|| "wechat".to_string());

    // 主 LLM API 必须配置
    if !main_api_configured(&state) {
        let _ = app.emit("chat:config_error", json!({ "reason": "no_main_api", "character_id": &char_id, "channel": &channel_str }));
        return Err("MAIN_API_NOT_CONFIGURED".to_string());
    }

    // 图片输入功能须启用
    if !state.config.read().get_typed::<bool>("ai.enable_vision", false) {
        let _ = app.emit("chat:error", json!({ "reason": "vision_disabled", "character_id": &char_id, "channel": &channel_str }));
        return Err("VISION_NOT_ENABLED".to_string());
    }

    let router = {
        let guard = state.model_router.read();
        guard
            .as_ref()
            .ok_or_else(|| "模型路由未初始化".to_string())?
            .clone()
    };

    let src = std::path::PathBuf::from(&source_path);
    // 图片读取/复制 + base64 编码均为阻塞操作，移入 spawn_blocking 避免卡住 tokio 工作线程
    // （大图可达数 MB，base64 编码 CPU 密集）
    let (mime, b64, rel_path) = tokio::task::spawn_blocking(
        move || -> Result<(String, String, String), String> {
            let bytes = match std::fs::read(&src) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err("图片文件不存在".to_string());
                }
                Err(e) => return Err(format!("读取图片失败: {}", e)),
            };
            let mime = crate::commands::config::detect_image_mime(&bytes).to_string();
            let b64 = STANDARD.encode(&bytes);

            // 保存副本到用户数据目录 images/ 下
            let data_dir = crate::utils::path::get_user_data_dir();
            let images_dir = data_dir.join("images");
            crate::utils::path::ensure_dir(&images_dir)
                .map_err(|e| format!("创建图片目录失败: {}", e))?;
            let ext = src
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
                .to_lowercase();
            let saved_name = format!("{}.{}", uuid::Uuid::new_v4(), ext);
            let saved_path = images_dir.join(&saved_name);
            std::fs::copy(&src, &saved_path).map_err(|e| format!("保存图片失败: {}", e))?;
            let rel_path = format!("images/{}", saved_name);
            Ok((mime, b64, rel_path))
        },
    )
    .await
    .map_err(|e| format!("图片处理任务失败: {}", e))??;
    let data_url = format!("data:{};base64,{}", mime, b64);

    let now_ts = chrono::Local::now().timestamp_millis() as f64 / 1000.0;

    // 1. 立即推送用户图片消息（前端渲染图片气泡）
    let _ = app.emit(
        "chat:user_image",
        json!({
            "data_url": data_url,
            "image_path": rel_path,
            "timestamp": now_ts,
            "character_id": &char_id,
            "channel": &channel_str,
        }),
    );

    // 2. 写入对话历史：用户图片消息（content 占位，metadata 标记图片）
    {
        let brain = state.get_character(character_id.as_deref())?.brain;
        let mut user_msg = ChatMessage::user("📷 [图片]");
        user_msg.meta = Some(crate::messages::MessageMeta::user().with_channel(&channel_str));
        brain.dialogue.add_message_with_metadata(
            user_msg,
            json!({
                "source": "chat",
                "kind": "image",
                "image_path": rel_path,
                "channel": channel_str,
            }),
        );
    }

    // 3. 调用多模态 LLM 生成图片描述 + 回应
    let stream_id = uuid::Uuid::new_v4().to_string();
    let _ = app.emit(
        "chat:start",
        json!({ "message": "[图片]", "stream_id": &stream_id, "character_id": &char_id, "channel": &channel_str }),
    );

    // 注意：此处不启用 emit，避免 LLM 原始 JSON 输出泄露到前端。
    // 仅通过 chat:done 发送解析后的 reply 文本。

    // 提取最近对话历史，注入到 vision_describe 的 system prompt 中，
    // 让 LLM 能结合上下文理解图片（例如用户说"给你们拍照片"后发了一张角色截图）。
    let recent_context = {
        let brain = state.get_character(character_id.as_deref())?.brain;
        let history = brain.dialogue.get_history();
        let recent: Vec<String> = history
            .iter()
            .rev()
            .take(6) // 最近 3 轮对话（user + assistant 各 3 条）
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
    let image_detail = state.config.read().get_typed::<String>("ai.image_detail", "auto".to_string());
    let image = MessageImage {
        media_type: mime.clone(),
        data: b64,
        url: None,
        detail: Some(image_detail),
    };
    // 每次请求附加唯一 nonce，防止豆包 Responses API 服务端缓存命中
    // （其缓存 key 不区分 input_image 内容，相同文本会返回旧结果）
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
        Ok(text) => parse_image_description_response(&text),
        Err(e) => {
            let msg = e.to_string();
            let _ = app.emit(
                "chat:error",
                json!({ "error": &msg, "stream_id": &stream_id, "character_id": &char_id, "channel": &channel_str }),
            );
            return Err(msg);
        }
    };

    // 4. 推送 AI 回复完成事件（前端渲染 AI 文字气泡）
    let _ = app.emit(
        "chat:done",
        json!({
            "text": reply,
            "motion": "idle",
            "expression": "",
            "emotion_score": 0,
            "voice_message": false,
            "voice_audio_path": null,
            "voice_duration": null,
            "stream_id": &stream_id,
            "character_id": &char_id,
            "channel": &channel_str,
        }),
    );

    // 5. AI 回复写入对话历史
    {
        let brain = match state.get_character(character_id.as_deref()) {
            Ok(inst) => inst.brain,
            Err(_) => return Ok(()),
        };
        let mut ai_msg = ChatMessage::assistant(&reply);
        ai_msg.meta = Some(crate::messages::MessageMeta::assistant().with_channel(&channel_str));
        brain.dialogue.add_message(ai_msg);
    }

    // 6. 图片描述写入记忆系统（content=description，metadata 携带 image_path）
    let description_for_memory = if description.is_empty() {
        reply.clone()
    } else {
        description
    };
    let memory_mgr = state
        .get_character(character_id.as_deref())
        .ok()
        .map(|inst| inst.brain.memory.clone());
    if let Some(memory_mgr) = memory_mgr {
        let char_id_for_mem = memory_mgr.char_id().to_string();
        let init_meta = json!({
            "channel": "direct",
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
                if let Err(e) = memory_mgr.patch_memory_metadata(
                    &item.id,
                    json!({
                        "kind": "image",
                        "image_path": rel_path,
                        "source": "chat",
                        "role": "user",
                        "memory_type": "general",
                        "semantic_type": "shared_memory",
                    }),
                ) {
                    tracing::warn!("[send_image_message] 写入图片记忆 metadata 失败: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("[send_image_message] 图片记忆写入失败: {}", e);
            }
        }
    }

    Ok(())
}

/// Open Loop 检测的调用包装：从 brain 取出 router 后转发到 conversation 模块
async fn maybe_mark_open_loop(
    conv: &crate::conversation::Conversation,
    brain: &crate::brain::Brain,
) {
    crate::conversation::maybe_mark_open_loop(conv, &brain.memory, &brain.router).await;
}

// ============================================================================
// 文件拖放：提取文件文本内容
// ============================================================================

/// 文件提取结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileTextResult {
    /// 文件名（含扩展名）
    pub filename: String,
    /// 提取的文本内容（已截断到 MAX_FILE_TEXT_CHARS）
    pub text: String,
    /// 文件类型分类：image / text / pdf / unsupported
    pub file_type: String,
    /// 是否因过长被截断
    pub truncated: bool,
    /// 原始字符数（截断前）
    pub original_char_count: usize,
}

/// 文件文本最大字符数（约 4000 tokens，避免 prompt 过长）
const MAX_FILE_TEXT_CHARS: usize = 12000;

/// 判断文件扩展名属于哪种类型
fn classify_file_extension(ext: &str) -> &'static str {
    let ext_lower = ext.to_lowercase();
    // 图片
    const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
    if IMAGE_EXTS.contains(&ext_lower.as_str()) {
        return "image";
    }
    // PDF
    if ext_lower == "pdf" {
        return "pdf";
    }
    // 纯文本 / 代码 / 数据格式
    const TEXT_EXTS: &[&str] = &[
        "txt", "md", "markdown", "log", "csv", "tsv", "rtf",
        // 代码
        "rs", "py", "js", "ts", "tsx", "jsx", "mjs", "cjs",
        "go", "java", "c", "cpp", "cc", "cxx", "h", "hpp", "hxx",
        "cs", "rb", "php", "swift", "kt", "kts", "scala", "sc",
        "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
        "sql", "r", "lua", "pl", "vim", "el", "clj", "cljs",
        "ex", "exs", "erl", "hs", "ml", "mli", "fs", "fsx", "vb",
        "dart", "groovy", "gradle", "makefile", "mk", "cmake",
        "dockerfile", "gitignore", "env",
        // 数据
        "json", "yaml", "yml", "xml", "toml", "ini", "conf", "cfg",
        "properties", "gradle", "lock",
        // 网页
        "html", "htm", "css", "scss", "sass", "less", "svg",
    ];
    if TEXT_EXTS.contains(&ext_lower.as_str()) {
        return "text";
    }
    "unsupported"
}

/// 用编码检测读取文本文件（兼容 GBK / Shift-JIS 等非 UTF-8 编码）
pub(crate) fn read_text_with_encoding_detection(path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取文件失败: {}", e))?;
    // 尝试 UTF-8 直接解析
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.to_string());
    }
    // 非 UTF-8：用 chardetng 检测编码后解码
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(&bytes, true);
    let encoding = detector.guess(None, true);
    let (decoded, _, had_errors) = encoding.decode(&bytes);
    if had_errors {
        tracing::warn!("[extract_file_text] 编码检测可能有误: {:?}", encoding.name());
    }
    Ok(decoded.into_owned())
}

/// 保存前端拍摄得到的 base64 图片为临时文件，返回临时文件路径。
///
/// 前端通过 getUserMedia + canvas 拍照后，将图片以 data URL（base64）形式传入，
/// 此命令解码后写入系统临时目录，返回完整路径供前端转交给 `send_image_message`。
///
/// - `base64_data`：不含 `data:<mime>;base64,` 前缀的纯 base64 字符串
/// - `mime`：图片 MIME，如 `image/png` / `image/jpeg`，用于决定文件扩展名
#[tauri::command]
pub async fn save_temp_image(
    base64_data: String,
    mime: String,
) -> Result<String, String> {
    let bytes = STANDARD
        .decode(base64_data.trim())
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    let ext = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/gif" => "gif",
        _ => "png",
    };

    let filename = format!("vivian_capture_{}.{}", uuid::Uuid::new_v4(), ext);
    let temp_path = std::env::temp_dir().join(filename);

    std::fs::write(&temp_path, &bytes)
        .map_err(|e| format!("写入临时文件失败: {}", e))?;

    Ok(temp_path.to_string_lossy().to_string())
}

/// 保存前端录制得到的 base64 语音为持久化文件，返回相对用户数据目录的路径。
///
/// 前端通过 `MediaRecorder` 录制音频（webm/ogg 等格式），以 data URL（base64）形式传入，
/// 此命令解码后写入 `<user_data_dir>/audio/` 目录，返回相对路径（如 `audio/<uuid>.webm`）。
/// 相对路径会写入对话历史的 metadata.audio_path，供后续播放和历史回看使用。
///
/// - `base64_data`：不含 `data:<mime>;base64,` 前缀的纯 base64 字符串
/// - `mime`：音频 MIME，如 `audio/webm`，用于决定文件扩展名
#[tauri::command]
pub async fn save_voice_audio(
    base64_data: String,
    mime: String,
) -> Result<String, String> {
    let bytes = STANDARD
        .decode(base64_data.trim())
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    let ext = match mime.as_str() {
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mp4" | "audio/m4a" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        _ => "webm",
    };

    let data_dir = crate::utils::path::get_user_data_dir();
    let audio_dir = data_dir.join("audio");
    crate::utils::path::ensure_dir(&audio_dir)
        .map_err(|e| format!("创建音频目录失败: {}", e))?;

    let saved_name = format!("{}.{}", uuid::Uuid::new_v4(), ext);
    let saved_path = audio_dir.join(&saved_name);
    std::fs::write(&saved_path, &bytes)
        .map_err(|e| format!("保存音频文件失败: {}", e))?;

    let rel_path = format!("audio/{}", saved_name);
    Ok(rel_path)
}

/// 提取文件文本内容。
///
/// 支持的文件类型：
/// - **图片**（png/jpg/jpeg/gif/webp/bmp）：返回 `file_type="image"`，前端转走 `send_image_message`
/// - **PDF**：用 pdf-extract 提取文本
/// - **纯文本/代码/数据文件**：按编码检测读取（兼容 GBK 等）
/// - **其他**：返回 `file_type="unsupported"`
///
/// 文本内容截断到 `MAX_FILE_TEXT_CHARS`（12000 字符，约 4000 tokens）。
#[tauri::command]
pub async fn extract_file_text(source_path: String) -> Result<FileTextResult, String> {
    let path = std::path::PathBuf::from(&source_path);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    if !path.exists() {
        return Err("文件不存在".to_string());
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let file_type = classify_file_extension(ext);

    match file_type {
        "image" => {
            // 图片：仅返回分类，前端转走 send_image_message
            Ok(FileTextResult {
                filename,
                text: String::new(),
                file_type: "image".to_string(),
                truncated: false,
                original_char_count: 0,
            })
        }
        "pdf" => {
            // PDF：用 pdf-extract 提取文本（CPU 密集，放到 blocking 线程）
            let path_clone = path.clone();
            let raw_text = tokio::task::spawn_blocking(move || {
                pdf_extract::extract_text(&path_clone)
            })
            .await
            .map_err(|e| format!("PDF 提取任务失败: {}", e))?
            .map_err(|e| format!("PDF 文本提取失败: {}", e))?;

            let char_count = raw_text.chars().count();
            let (text, truncated) = if char_count > MAX_FILE_TEXT_CHARS {
                let truncated_text: String = raw_text.chars().take(MAX_FILE_TEXT_CHARS).collect();
                (truncated_text, true)
            } else {
                (raw_text, false)
            };

            Ok(FileTextResult {
                filename,
                text,
                file_type: "pdf".to_string(),
                truncated,
                original_char_count: char_count,
            })
        }
        "text" => {
            let raw_text = read_text_with_encoding_detection(&path)?;
            let char_count = raw_text.chars().count();
            let (text, truncated) = if char_count > MAX_FILE_TEXT_CHARS {
                let truncated_text: String = raw_text.chars().take(MAX_FILE_TEXT_CHARS).collect();
                (truncated_text, true)
            } else {
                (raw_text, false)
            };

            Ok(FileTextResult {
                filename,
                text,
                file_type: "text".to_string(),
                truncated,
                original_char_count: char_count,
            })
        }
        _ => {
            Ok(FileTextResult {
                filename,
                text: String::new(),
                file_type: "unsupported".to_string(),
                truncated: false,
                original_char_count: 0,
            })
        }
    }
}
