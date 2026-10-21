//! 聊天窗口专属角色（chat-only partner）
//!
//! 这类角色仅通过聊天窗口交互：没有本机桌宠窗口、Brain 或心理状态，
//! 只有独立对话历史、轻量片段记忆和人格提示词。它们不是 `CharacterInstance`，
//! 因此不参与 `state.characters` 的角色初始化、窗口创建、主动行为与调度。
//!
//! 与聊天窗口的接线点只有三处：
//!   1. `list_chat_partners` 命令 → 前端首页联系人列表多一项
//!   2. `send_message_stream` 的旁支分支 → 群聊/私聊发送时按角色 ID 分流到这里
//!   3. 历史类命令的兜底 → 用本模块的 DialogueManager 顶替 `brain.dialogue`
//!
//! 流式事件与普通角色完全一致（chat:start / chat:chunk / chat:done），
//! 所以前端聊天窗口不需要为它写任何特例分支。

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::chat_partner_memory::{self as memory, SemanticMemory, Snippet};
use crate::chat_partner_policy::{self as policy, ContextLine};
use crate::dialogue::DialogueManager;
use crate::memory::embedding::{build_embedding, MemoryEmbeddingProvider};
use crate::providers::base::{LLMRequest, StreamEvent};
use crate::state::AppState;

/// 聊天专属角色的展示名。三语言界面统一用这一个值（i18n `partner_name_deepseek`
/// 同值），seed 与开机改名同步都以它为准。
pub const DEEPSEEK_NAME: &str = "ディープシークちゃん";
use crate::types::response::ChatMessage;

/// 内存近期消息上限；模型上下文另按渠道从持久历史分页读取。
const HISTORY_MAX_LEN: usize = 200;
/// 每轮送进模型的最近历史条数
const CONTEXT_TURNS: usize = 16;

/// 聊天专属角色独占的路由键。
///
/// 它在 `routing_matrix` 里是一格完整配置，但不在设置窗口的路由列表里出现
/// （catalog 标了 `hidden`）——配置入口在聊天窗口内她的模型设置面板。
/// 未配置时沿 fallback 链退回 `companion`，与两位桌宠同一档模型。
pub const MODEL_ROUTE: &str = "chat_partner";

/// 一个聊天专属角色
pub struct ChatPartner {
    pub id: String,
    pub profile: RwLock<crate::sns_store::Contact>,
    pub deleted: std::sync::atomic::AtomicBool,
    pub(crate) memory_write_lock: parking_lot::Mutex<()>,
    router_cache: parking_lot::Mutex<Option<(u64, crate::providers::ModelRouter)>>,
    /// 人格提示词（编译期嵌入）
    /// 独立对话历史。与角色无关，落盘在 companion 数据目录下的同名分桶
    dialogue: Arc<DialogueManager>,
    /// Hold across the entire turn: history, provider stream and final persistence.
    turn_lock: tokio::sync::Mutex<()>,
    memory: SemanticMemory,
    memory_seeded: parking_lot::Mutex<HashSet<String>>,
    embedding: parking_lot::Mutex<Option<(u64, Arc<dyn MemoryEmbeddingProvider>)>>,
    /// A stalled remote embedder cannot accumulate unbounded blocking jobs.
    embedding_slot: Arc<tokio::sync::Semaphore>,
}

struct PreparedContext {
    messages: Vec<ChatMessage>,
    excluded: Vec<String>,
    recalled: Option<String>,
}

impl ChatPartner {
    pub fn new(profile: crate::sns_store::Contact) -> Self {
        let id = profile.id.clone();
        Self {
            id: id.clone(),
            profile: RwLock::new(profile),
            deleted: std::sync::atomic::AtomicBool::new(false),
            dialogue: DialogueManager::spawn(HISTORY_MAX_LEN, &id),
            turn_lock: tokio::sync::Mutex::new(()),
            memory: SemanticMemory::new(
                crate::utils::path::get_companion_data_dir(&id).join("chat_partner_memory.sqlite3"),
            ),
            memory_seeded: parking_lot::Mutex::new(HashSet::new()),
            embedding: parking_lot::Mutex::new(None),
            embedding_slot: Arc::new(tokio::sync::Semaphore::new(1)),
            memory_write_lock: parking_lot::Mutex::new(()),
            router_cache: parking_lot::Mutex::new(None),
        }
    }
    pub fn is_deleted(&self) -> bool {
        self.deleted.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn router_for(
        &self,
        state: &AppState,
        app: &AppHandle,
    ) -> Result<crate::providers::ModelRouter, String> {
        let mut config = state.config.read().get_all();
        let model = self.profile.read().model.clone();
        let route = if let Some(mut value) = model {
            if value.get("api_key").is_none() {
                value["api_key"] = json!("");
            }
            if value.get("provider_type").is_none() {
                value["provider_type"] = json!("openai");
            }
            serde_json::from_value(value).map_err(|e| format!("好友模型配置无效: {e}"))?
        } else {
            crate::config::manager::TaskRouteConfig::default()
        };
        config.routing_matrix.insert(MODEL_ROUTE.into(), route);
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(&config)
            .unwrap_or_default()
            .hash(&mut hash);
        let key = hash.finish();
        let mut cached = self.router_cache.lock();
        if let Some((previous, router)) = cached.as_ref() {
            if *previous == key {
                return Ok(router.clone());
            }
        }
        let router = crate::providers::ModelRouter::reload(&config).map_err(|e| e.to_string())?;
        router.set_app_handle(app.clone());
        *cached = Some((key, router.clone()));
        Ok(router)
    }
    pub async fn delete_owned(self: &Arc<Self>) -> Result<(), String> {
        self.deleted
            .store(true, std::sync::atomic::Ordering::Release);
        let _turn = policy::acquire_turn(&self.turn_lock, Duration::from_secs(95)).await?;
        let _memory = self.memory_write_lock.lock();
        self.dialogue
            .clear_history_file()
            .map_err(|e| e.to_string())?;
        let root = crate::utils::path::get_user_data_dir();
        let path = crate::sns_store::owned_directory(&root, &self.id, false)?;
        if path.exists() {
            crate::sns_store::verify_removal(&root, &path)?;
            std::fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// 组装本轮请求的消息列表：人格 system + 最近若干条历史。
    ///
    /// 历史按渠道从落盘记录里读（分页会合并尚未刷盘的缓冲），
    /// 因此重启后上下文不会丢。
    fn build_messages(
        &self,
        state: &AppState,
        channel: &str,
        query: &str,
    ) -> Result<PreparedContext, String> {
        let mut messages = vec![ChatMessage::system(self.system_prompt(channel))];
        let seed = !self.memory_seeded.lock().contains(channel);
        let sources = if crate::sns_store::group_id(channel).is_some() {
            Vec::new()
        } else if channel == "wechat_group" {
            // Include visible public turns even from a pet that has since gone offline.
            let mut sources: Vec<_> = state
                .characters
                .read()
                .values()
                .map(|c| (c.brain.dialogue.clone(), c.id.clone()))
                .collect();
            sources.push((self.dialogue.clone(), self.id.clone()));
            sources
        } else {
            vec![(self.dialogue.clone(), self.id.clone())]
        };
        let mut lines = Vec::new();
        let mut snippets = Vec::new();
        if let Some(group) = crate::sns_store::group_id(channel) {
            for entry in crate::sns_runtime::store().history(
                group,
                None,
                if seed { 200 } else { CONTEXT_TURNS },
            )? {
                let content = if let Some(description) = entry
                    .metadata
                    .get("visual_description")
                    .and_then(|v| v.as_str())
                {
                    format!("{}\n[图片识别] {description}", entry.content)
                } else {
                    entry.content.clone()
                };
                if entry.role == "user" || entry.speaker != self.id {
                    snippets.push(Snippet {
                        source_id: entry.id.clone(),
                        channel: channel.into(),
                        speaker: entry.speaker.clone(),
                        content: content.clone(),
                        timestamp: entry.timestamp,
                    });
                }
                lines.push(ContextLine {
                    source_id: entry.id,
                    speaker: entry.speaker,
                    role: entry.role,
                    content,
                    timestamp: entry.timestamp,
                    channel: Some(channel.into()),
                });
            }
        }

        for (dialogue, speaker) in sources {
            let (rows, _) = dialogue
                .history_page_before(None, if seed { 200 } else { CONTEXT_TURNS }, Some(channel))
                .map_err(|e| format!("读取聊天上下文失败: {e}"))?;
            for entry in &rows {
                let source_id = format!("{speaker}:{}", entry.id);
                // Preserve received speech, not our own generated backstory or file payloads.
                if (entry.role == "user"
                    || (channel == "wechat_group"
                        && entry.role == "assistant"
                        && speaker != self.id))
                    && entry.metadata.get("kind").and_then(|v| v.as_str()) != Some("file")
                {
                    snippets.push(Snippet {
                        source_id,
                        channel: channel.into(),
                        speaker: if entry.role == "user" {
                            "user".into()
                        } else {
                            speaker.clone()
                        },
                        content: entry
                            .metadata
                            .get("visual_description")
                            .and_then(|v| v.as_str())
                            .map(|v| format!("用户分享图片：{v}"))
                            .unwrap_or_else(|| entry.content.clone()),
                        timestamp: entry.timestamp,
                    });
                }
            }
            lines.extend(rows.into_iter().map(|entry| {
                ContextLine {
                    source_id: format!("{speaker}:{}", entry.id),
                    speaker: speaker.clone(),
                    role: entry.role,
                    content: entry
                        .metadata
                        .get("visual_description")
                        .and_then(|v| v.as_str())
                        .map(|v| format!("{}\n[图片识别] {v}", entry.content))
                        .unwrap_or(entry.content),
                    timestamp: entry.timestamp,
                    channel: entry
                        .metadata
                        .get("channel")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                }
            }));
        }
        let mut excluded = Vec::new();
        for line in policy::scoped_context(lines, channel, CONTEXT_TURNS) {
            excluded.push(line.source_id.clone());
            if line.role == "assistant" && line.speaker == self.id {
                messages.push(ChatMessage::assistant(line.content));
            } else if crate::sns_store::is_group(channel) {
                // Other characters' speech is quoted evidence, never this model's assistant role.
                messages.push(ChatMessage::user(format!(
                    "[群聊消息] {}",
                    json!({
                        "speaker":if line.role == "user" { "user" } else { &line.speaker },
                        "text":line.content,
                    })
                )));
            } else {
                messages.push(ChatMessage::user(line.content));
            }
        }
        if let Err(error) = self.memory.remember(channel, &snippets) {
            tracing::warn!("[ChatPartner] 记忆保存失败，继续聊天: {error}");
        } else {
            self.memory_seeded.lock().insert(channel.into());
        }
        let recalled = self
            .memory
            .recall(channel, query, &excluded, None)
            .ok()
            .and_then(|snippets| memory::prompt(&snippets));
        Ok(PreparedContext {
            messages,
            excluded,
            recalled,
        })
    }

    fn embedding_provider(&self, state: &AppState) -> Arc<dyn MemoryEmbeddingProvider> {
        let config = state.config.read().get_all();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(&config.memory.embedding)
            .unwrap_or_default()
            .hash(&mut hasher);
        let key = hasher.finish();
        let mut cached = self.embedding.lock();
        if let Some((previous, provider)) = cached.as_ref() {
            if *previous == key {
                return provider.clone();
            }
        }
        let provider = build_embedding(&config);
        *cached = Some((key, provider.clone()));
        provider
    }

    async fn recall_semantic(
        self: &Arc<Self>,
        state: &Arc<AppState>,
        channel: String,
        query: String,
        excluded: Vec<String>,
    ) -> Option<String> {
        let permit = self.embedding_slot.clone().try_acquire_owned().ok()?;
        let partner = self.clone();
        let state = state.clone();
        let job = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
            let _permit = permit;
            let provider = partner.embedding_provider(&state);
            let model = provider.index_identity();
            let pending = {
                let _write = partner.memory_write_lock.lock();
                if partner.is_deleted() {
                    return Ok(None);
                }
                partner.memory.pending(&channel, &model)?
            };
            let mut texts = vec![query.clone()];
            texts.extend(pending.iter().map(|snippet| snippet.content.clone()));
            let mut vectors = provider.embed_batch(&texts).map_err(|e| e.to_string())?;
            if vectors.len() != texts.len() {
                return Err("聊天记忆嵌入数量不匹配".into());
            }
            if vectors
                .iter()
                .any(|vector| vector.len() != provider.dimension())
            {
                return Err("聊天记忆嵌入维度不匹配".into());
            }
            let query_vector = vectors.remove(0);
            let _write = partner.memory_write_lock.lock();
            if partner.is_deleted() || !crate::sns_runtime::scope_alive(&partner.id, &channel) {
                return Ok(None);
            }
            partner.memory.index(&channel, &model, &pending, &vectors)?;
            let snippets = partner.memory.recall(
                &channel,
                &query,
                &excluded,
                Some((&model, &query_vector)),
            )?;
            Ok(memory::prompt(&snippets))
        });
        match tokio::time::timeout(Duration::from_millis(1200), job).await {
            Ok(Ok(Ok(prompt))) => prompt,
            Ok(Ok(Err(error))) => {
                tracing::warn!("[ChatPartner] 语义召回不可用，使用本地检索: {error}");
                None
            }
            _ => None,
        }
    }

    pub async fn clear_history(self: &Arc<Self>) -> Result<(), String> {
        let _turn = policy::acquire_turn(&self.turn_lock, Duration::from_secs(90)).await?;
        let partner = self.clone();
        tokio::task::spawn_blocking(move || {
            partner.memory.clear()?;
            partner
                .dialogue
                .clear_history_file()
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// 人格提示词 + SNS 会话语境。
    fn system_prompt(&self, channel: &str) -> String {
        let profile = self.profile.read();
        let scene = if let Some(id) = crate::sns_store::group_id(channel) {
            crate::sns_runtime::store().group(id).map(|group|format!("你正在 ChatWindow SNS 的群「{}」里聊天。成员：{}。你是 {}，只以自己的身份接话。带 speaker 标记的记录是实际收到的群消息；已删除好友的旧消息只是历史记录。",group.name,group.members.join("、"),profile.name)).unwrap_or_default()
        } else if channel == "wechat_group" {
            "这是 ChatWindow SNS 的群聊。根据实际收到的群消息接话。".into()
        } else {
            "这是你通过 ChatWindow SNS 和用户的一对一私聊。你在自己的电脑那头发消息。只根据这段私聊提供的记录和回忆交流。".into()
        };
        format!("{}\n\n## 当前会话\n{}", profile.persona, scene)
    }
}

/// 全局注册表（ID → 角色）。启动时一次性建立，之后只读。
static PARTNERS: Lazy<RwLock<HashMap<String, Arc<ChatPartner>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// 注册聊天专属角色。幂等，`AppState::initialize` 每次都会调用。
///
/// 必须在 tokio 运行时内调用：`DialogueManager::spawn` 会起后台刷盘任务。
pub fn init() -> Result<(), String> {
    let store = crate::sns_runtime::store();
    store.seed(&crate::sns_store::Contact {
        id: "deepseek".into(),
        name: DEEPSEEK_NAME.into(),
        persona: include_str!("../prompts/chat_partners/deepseek_chan.md").into(),
        model: None,
        avatar: "/icons/charaters/deepseek.webp".into(),
    })?;
    // seed 带 contacts_seeded 标记、只跑一次；展示名后来统一过，
    // 老装机里的旧名要靠这里幂等同步过去（名字以外字段一律不动）。
    store.rename_contact("deepseek", DEEPSEEK_NAME)?;
    for profile in store.contacts()? {
        register(profile);
    }
    Ok(())
}

pub fn register(profile: crate::sns_store::Contact) -> Arc<ChatPartner> {
    if let Some(partner) = get(&profile.id) {
        *partner.profile.write() = profile;
        return partner;
    }
    let partner = Arc::new(ChatPartner::new(profile));
    PARTNERS.write().insert(partner.id.clone(), partner.clone());
    partner
}
pub fn unregister(id: &str) -> Option<Arc<ChatPartner>> {
    PARTNERS.write().remove(id)
}

/// 按 ID 取角色；不是聊天专属角色时返回 None
pub fn get(char_id: &str) -> Option<Arc<ChatPartner>> {
    PARTNERS.read().get(char_id).cloned()
}

/// 全部聊天专属角色（顺序稳定：按 ID 排序，避免 HashMap 迭代顺序抖动影响列表）
pub fn all() -> Vec<Arc<ChatPartner>> {
    let mut list: Vec<Arc<ChatPartner>> = PARTNERS.read().values().cloned().collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    list
}

/// 按角色 ID 解析对话历史：普通角色取 `brain.dialogue`，聊天专属角色取自己的会话。
///
/// 返回 `(对应 DialogueManager, id, name)`。找不到角色时报错（与原有命令语义一致）。
pub fn resolve_dialogue(
    state: &AppState,
    character_id: Option<&str>,
) -> Result<(Arc<DialogueManager>, String, String), String> {
    let char_id = match character_id {
        Some(id) => id.to_string(),
        None => state.active_character_id.read().clone(),
    };
    if let Some(instance) = state.characters.read().get(&char_id) {
        return Ok((
            instance.brain.dialogue.clone(),
            instance.id.clone(),
            instance.name.clone(),
        ));
    }
    if let Some(partner) = get(&char_id) {
        return Ok((
            partner.dialogue.clone(),
            partner.id.clone(),
            partner.profile.read().name.clone(),
        ));
    }
    Err(format!("角色未找到: {}", char_id))
}

/// 历史聚合命令要遍历的会话清单：所有在线角色 + 全部聊天专属角色。
pub fn online_dialogues(state: &AppState) -> Vec<(Arc<DialogueManager>, String, String)> {
    let mut list: Vec<(Arc<DialogueManager>, String, String)> = state
        .characters
        .read()
        .values()
        .filter(|c| *c.online.read())
        .map(|c| (c.brain.dialogue.clone(), c.id.clone(), c.name.clone()))
        .collect();
    for partner in all() {
        list.push((
            partner.dialogue.clone(),
            partner.id.clone(),
            partner.profile.read().name.clone(),
        ));
    }
    list
}

/// `list_chat_partners` 的序列化形态
pub fn list_json() -> Vec<serde_json::Value> {
    all()
        .into_iter()
        .map(|p| {
            let id = p.id.clone();
            let profile = p.profile.read().clone();
            let name = profile.name;
            json!({
                "id": id,
                "name": name,
                // 聊天专属角色始终可用：它没有上下线概念
                "online": true,
                "kind": "chat_partner", "avatar":profile.avatar,
            })
        })
        .collect()
}

/// 生成一轮流式回复。
///
/// 与 `send_message_stream` 的普通角色分支共享同一套前端事件契约：
/// `chat:start` → 若干 `chat:chunk` → `chat:done`（失败走 `chat:error`）。
pub async fn stream_reply(
    state: &Arc<AppState>,
    app: &AppHandle,
    partner: Arc<ChatPartner>,
    message: String,
    stream_id: String,
    channel: String,
    file_metadata: Option<serde_json::Value>,
) -> Result<Option<crate::types::AiResponse>, String> {
    let char_id = partner.id.clone();
    let cancel_id = if crate::sns_store::group_id(&channel).is_some() {
        format!("sns:{channel}:{char_id}")
    } else {
        char_id.clone()
    };
    if !crate::sns_runtime::scope_alive(&char_id, &channel) {
        return Err("好友不在此群或群已解散".into());
    }
    let mut metadata = policy::input_metadata(file_metadata.as_ref(), &channel)?;
    if crate::sns_store::group_id(&channel).is_none() {
        let _files = partner.memory_write_lock.lock();
        if partner.is_deleted() {
            return Err("好友已删除".into());
        }
        metadata = crate::sns_runtime::own_media(&partner.id, metadata)?;
    }
    if partner.is_deleted() {
        return Err("好友已删除".into());
    }
    let _turn = match policy::acquire_turn(&partner.turn_lock, Duration::from_secs(90)).await {
        Ok(guard) => guard,
        Err(error) => {
            emit_error(
                app,
                &stream_id,
                &char_id,
                &channel,
                "上一轮聊天仍未结束，请稍后重试",
            );
            return Err(error);
        }
    };
    if partner.is_deleted() {
        return Err("好友已删除".into());
    }
    if crate::companion_quiet::active() {
        let _ = app.emit(
            "chat:cancelled",
            json!({"stream_id":stream_id,"character_id":char_id,"channel":channel}),
        );
        return Ok(None);
    }
    // 清掉上一轮可能残留的取消标志，否则本轮一开流就被判定为取消
    state.set_generation_cancel(&cancel_id, false);

    // 前端据此初始化流式缓冲（私聊视图靠这一步才认这条流）
    let _ = app.emit(
        "chat:start",
        json!({
            "message": &message,
            "stream_id": &stream_id,
            "character_id": &char_id,
            "channel": &channel,
            "source": "chat_partner",
        }),
    );

    // 用户消息先落账：即使模型这一轮失败，聊天窗口刷新后仍能看到自己说过的话
    partner.dialogue.set_app_handle(app.clone());
    let recall_query: String = message.chars().take(1200).collect();
    if crate::sns_store::group_id(&channel).is_none() {
        partner.dialogue.add_message_with_metadata(
            ChatMessage::user(message).with_meta(
                crate::messages::MessageMeta::new(crate::messages::MessageSource::User)
                    .with_channel(&channel),
            ),
            metadata,
        );
    }
    let context_partner = partner.clone();
    let context_state = state.clone();
    let context_channel = channel.clone();
    let context_query = recall_query.clone();
    let mut prepared = match tokio::task::spawn_blocking(move || {
        context_partner.build_messages(&context_state, &context_channel, &context_query)
    })
    .await
    {
        Ok(Ok(messages)) => messages,
        result => {
            let error = match result {
                Ok(Err(e)) => e,
                Err(e) => e.to_string(),
                _ => unreachable!(),
            };
            emit_error(app, &stream_id, &char_id, &channel, &error);
            return Err(error);
        }
    };

    let recalled = partner
        .recall_semantic(state, channel.clone(), recall_query, prepared.excluded)
        .await
        .or(prepared.recalled);
    if let Some(recalled) = recalled {
        prepared.messages.insert(1, ChatMessage::system(recalled));
    }
    let messages = prepared.messages;

    let router = partner.router_for(state, app)?;
    // task_type 用她的独占路由：未单独配置时按 catalog 的 fallback 链退回 companion，
    // 配置了就用她自己那条，互不影响
    let request = LLMRequest::new(MODEL_ROUTE, messages)
        .with_stream(true)
        .with_usage_tag("chat_partner");

    let mut rx = match policy::wait_for_model(
        router.generate_stream(request),
        || {
            state.is_generation_cancelled(&cancel_id)
                || crate::companion_quiet::active()
                || partner.is_deleted()
                || !crate::sns_runtime::scope_alive(&char_id, &channel)
        },
        Duration::from_secs(90),
    )
    .await
    {
        Ok(Some(Ok(rx))) => rx,
        Ok(Some(Err(e))) => {
            let text = e.to_string();
            emit_error(app, &stream_id, &char_id, &channel, &text);
            return Err(text);
        }
        Ok(None) => {
            state.set_generation_cancel(&cancel_id, false);
            let _ = app.emit(
                "chat:cancelled",
                json!({"stream_id":stream_id,"character_id":char_id,"channel":channel}),
            );
            return Ok(None);
        }
        Err(error) => {
            emit_error(app, &stream_id, &char_id, &channel, &error);
            return Err(error);
        }
    };

    let mut full = String::new();
    let mut poll = tokio::time::interval(Duration::from_millis(100));
    let mut last_event = Instant::now();
    loop {
        if state.is_generation_cancelled(&cancel_id)
            || crate::companion_quiet::active()
            || partner.is_deleted()
            || !crate::sns_runtime::scope_alive(&char_id, &channel)
        {
            state.set_generation_cancel(&cancel_id, false);
            let _ = app.emit(
                "chat:cancelled",
                json!({"stream_id":stream_id,"character_id":char_id,"channel":channel}),
            );
            return Ok(None);
        }
        if last_event.elapsed() > Duration::from_secs(90) {
            emit_error(app, &stream_id, &char_id, &channel, "聊天模型响应超时");
            return Err("聊天模型响应超时".into());
        }
        let event = tokio::select! {
            _ = poll.tick() => continue,
            event = rx.recv() => match event { Some(event) => event, None => break },
        };
        last_event = Instant::now();
        match event {
            StreamEvent::Text { content } => {
                if content.is_empty() {
                    continue;
                }
                full.push_str(&content);
                let _ = app.emit(
                    "chat:chunk",
                    json!({
                        "text": content,
                        "stream_id": &stream_id,
                        "character_id": &char_id,
                        "channel": &channel,
                    }),
                );
            }
            StreamEvent::Error { message } => {
                emit_error(app, &stream_id, &char_id, &channel, &message);
                return Err(message);
            }
            // 思维链 / 工具增量 / 用量统计：聊天专属角色不需要，直接丢弃
            _ => {}
        }
        if state.is_generation_cancelled(&cancel_id) {
            state.set_generation_cancel(&cancel_id, false);
            let _ = app.emit(
                "chat:cancelled",
                json!({ "stream_id": &stream_id, "character_id": &char_id, "channel": &channel }),
            );
            return Ok(None);
        }
    }

    let text = full.trim().to_string();
    if text.is_empty() {
        // 空回复也要结算流，否则前端「对方正在输入」会一直挂着
        let _ = app.emit(
            "chat:cancelled",
            json!({ "stream_id": &stream_id, "character_id": &char_id, "channel": &channel }),
        );
        return Ok(None);
    }

    if partner.is_deleted() || !crate::sns_runtime::scope_alive(&char_id, &channel) {
        return Ok(None);
    }
    if let Some(group) = crate::sns_store::group_id(&channel) {
        crate::sns_runtime::store().append(group,&char_id,"assistant",&text,json!({"speaker_name":partner.profile.read().name,"speaker_avatar":partner.profile.read().avatar}))?;
        let _ = app.emit("sns:changed", json!({"group_id":group}));
    } else {
        partner.dialogue.add_message_with_metadata(
            ChatMessage::assistant(text.clone()).with_meta(
                crate::messages::MessageMeta::new(crate::messages::MessageSource::Assistant)
                    .with_channel(&channel),
            ),
            json!({"source":"chat_partner","channel":channel}),
        );
    }
    let response = crate::types::AiResponse::new(text.clone());
    let _ = app.emit(
        "chat:done",
        json!({
            "text": text,
            "stream_id": &stream_id,
            "character_id": &char_id,
            "channel": &channel,
            // 没有桌宠，表情/动作字段给中性值，前端按普通文本气泡渲染
            "motion": "idle",
            "expression": "",
            "emotion_score": 0.0,
            "source": "chat_partner",
        }),
    );
    Ok(Some(response))
}

fn emit_error(app: &AppHandle, stream_id: &str, char_id: &str, channel: &str, error: &str) {
    tracing::warn!("[ChatPartner:{}] 生成失败: {}", char_id, error);
    let _ = app.emit(
        "chat:error",
        json!({
            "error": error,
            "stream_id": stream_id,
            "character_id": char_id,
            "channel": channel,
        }),
    );
}
