//! 对话管理 - 维护聊天历史并提供持久化能力
//!
//! - 缓冲写入（2 秒定时器 + max_buffer_size=10 立即刷新）
//! - 重复检测（UUID + 内容+时间戳+角色，仅比对已落盘尾部 20 条）
//! - 分页查询（offset/limit + has_more）
//! - 持久化：JSONL 追加写（每行一条 HistoryEntry），刷新仅追加新消息，
//!   不再全量重写整个文件；旧版 full_chat_history.json 首次访问时自动迁移

pub mod history;
pub mod intent_judge;
pub mod strategy;
pub mod topic_tracker;

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::error::{VivianError, VivianResult};
use crate::types::response::ChatMessage;
use crate::utils::path::get_character_data_dir;

use self::history::ChatMessageHistory;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct HistoryFile {
    version: String,
    messages: Vec<HistoryEntry>,
}

impl Default for HistoryFile {
    fn default() -> Self {
        Self {
            version: "1.0".to_string(),
            messages: Vec::new(),
        }
    }
}

/// 对话缓冲区 flush 间隔（2 秒），平衡 IO 频率和实时性
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);

pub struct DialogueManager {
    /// 角色 ID（用于持久化路径分桶）
    char_id: String,
    /// 内存中的近期消息（用于上下文构建，限制为 max_history_len）
    messages: Mutex<Vec<ChatMessage>>,
    max_history_len: usize,
    /// 写入缓冲区（待持久化的消息），使用 Mutex 实现内部可变性
    buffer: Mutex<Vec<HistoryEntry>>,
    /// 缓冲区最大大小（超过则立即刷新）
    max_buffer_size: usize,
    /// 后台定时刷新间隔（2 秒）
    flush_interval: Duration,
    /// AppHandle 注入后，add_message 会 emit `dialogue:changed` 事件通知前端刷新
    app_handle: Mutex<Option<AppHandle>>,
    /// 当前消息渠道标记（"wechat" / "direct"），由 send_message_stream 在调用 brain.think 前设置
    current_channel: Mutex<String>,
    /// 当前会话 ID（来自 ConversationManager），由 send_message_stream 在 think 前设置
    ///
    /// 写入 HistoryEntry.session_id，实现对话历史按会话切分。
    /// None 表示未启用会话管理（向后兼容旧消息）。
    current_session_id: Mutex<Option<String>>,
    /// 已落盘消息的尾部缓存（最近 20 条），供 flush 时重复检测，
    /// 避免每次刷新全量读文件
    written_tail: Mutex<Vec<HistoryEntry>>,
    /// JSONL 就绪标志：首次 flush 前完成旧格式迁移与尾部缓存恢复
    jsonl_ready: Mutex<bool>,
}

impl DialogueManager {
    pub fn new(max_history_len: usize, char_id: &str) -> Self {
        Self {
            char_id: char_id.to_string(),
            messages: Mutex::new(Vec::new()),
            max_history_len,
            buffer: Mutex::new(Vec::new()),
            max_buffer_size: 10,
            flush_interval: FLUSH_INTERVAL,
            app_handle: Mutex::new(None),
            current_channel: Mutex::new("wechat".to_string()),
            current_session_id: Mutex::new(None),
            written_tail: Mutex::new(Vec::new()),
            jsonl_ready: Mutex::new(false),
        }
    }

    /// 创建 Arc 实例并启动后台定时刷新任务（生产环境使用）
    pub fn spawn(max_history_len: usize, char_id: &str) -> Arc<Self> {
        let mgr = Arc::new(Self::new(max_history_len, char_id));
        mgr.start_background_flush();
        mgr
    }

    /// 启动后台定时刷新任务（2 秒间隔，需在 tokio 运行时中调用）
    pub fn start_background_flush(self: &Arc<Self>) {
        let weak: Weak<Self> = Arc::downgrade(self);
        let interval = self.flush_interval;
        let loop_name = format!("dialogue_flush:{}", self.char_id);
        tokio::spawn(async move {
            crate::utils::watchdog::register(
                &loop_name,
                interval.as_secs_f64(),
                None,
            );
            // 跳过第一次立即触发
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                crate::utils::watchdog::beat(&loop_name);
                match weak.upgrade() {
                    Some(mgr) => {
                        if let Err(e) = mgr.flush_buffer() {
                            tracing::error!("后台刷新缓冲区失败: {}", e);
                        }
                    }
                    None => {
                        crate::utils::watchdog::unregister(&loop_name);
                        break;
                    }
                }
            }
        });
    }

    /// 注入 AppHandle，启用 add_message 时的 `dialogue:changed` 事件通知
    pub fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.lock() = Some(handle);
    }

    /// 设置当前消息渠道（"wechat" / "direct"），影响后续 add_message 写入的 metadata.channel
    pub fn set_channel(&self, channel: &str) {
        *self.current_channel.lock() = channel.to_string();
    }

    /// 设置当前会话 ID（由 send_message_stream 在 think 前调用）
    ///
    /// 传 None 清除会话标记（如 think 结束后重置）。
    pub fn set_session_id(&self, session_id: Option<String>) {
        *self.current_session_id.lock() = session_id;
    }

    /// 获取当前会话 ID
    pub fn get_session_id(&self) -> Option<String> {
        self.current_session_id.lock().clone()
    }

    /// 添加一条消息：同时加入内存近期列表与写入缓冲区
    pub fn add_message(&self, msg: ChatMessage) {
        // 1. 注入当前渠道标记到 ChatMessage.meta（用于工作记忆通道隔离过滤）
        let default_channel = self.current_channel.lock().clone();
        let session_id = self.current_session_id.lock().clone();
        let mut msg_with_channel = msg.clone();
        let meta = msg_with_channel.meta.get_or_insert_with(Default::default);
        if meta.channel.is_none() {
            meta.channel = Some(default_channel.clone());
        }
        // 最终生效的 channel：优先用 msg 自带的，否则用 current_channel
        let effective_channel = meta.channel.clone().unwrap_or_else(|| default_channel.clone());

        // 2. 加入内存近期消息（截断到 max_history_len）
        let mut msgs = self.messages.lock();
        msgs.push(msg_with_channel.clone());
        if msgs.len() > self.max_history_len {
            let drop_count = msgs.len() - self.max_history_len;
            msgs.drain(0..drop_count);
        }
        drop(msgs);

        // 3. 构造 HistoryEntry 并注入最终渠道标记 + 会话 ID
        // 注意：优先用 msg.meta.channel（允许调用方显式指定 proactive/direct 等渠道），
        // 避免主动消息等被默认 wechat 污染导致微信界面错误显示
        let mut entry = Self::message_to_entry(&msg_with_channel, &session_id);
        if let Some(meta) = entry.metadata.as_object_mut() {
            meta.insert("channel".to_string(), serde_json::json!(effective_channel));
        }
        let need_flush = {
            let mut buf = self.buffer.lock();
            buf.push(entry);
            buf.len() >= self.max_buffer_size
        };

        // 4. 缓冲区满则立即刷新
        if need_flush {
            if let Err(e) = self.flush_buffer() {
                tracing::error!("缓冲区满触发刷新失败: {}", e);
            }
        }

        // 5. 通知前端对话历史已变更
        if let Some(handle) = self.app_handle.lock().as_ref() {
            let _ = handle.emit(
                "dialogue:changed",
                serde_json::json!({ "character_id": self.char_id }),
            );
        }

        // 6. 注册为统一事件账本的 UserMessage/AgentMessage 事件
        // 对话消息天然是 Event，不仅限于带 metadata 的版本。
        // 根据 role 映射为 sender/receiver，让双角色都能感知到对话发生。
        let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let (sender, receiver) = match msg_with_channel.role.as_str() {
            "user" => ("user".to_string(), self.char_id.clone()),
            "assistant" => (self.char_id.clone(), "user".to_string()),
            _ => ("system".to_string(), "all".to_string()),
        };
        let event_type = match msg_with_channel.role.as_str() {
            "user" => "user_message",
            "assistant" => "agent_message",
            _ => "system_message",
        };
        crate::memory::unified_event_ledger::register_event(
            crate::memory::unified_event_ledger::UnifiedEvent {
                id: format!("evt-{}-{}", now as u64, rand::random::<u32>()),
                timestamp: now,
                sender,
                receiver,
                event_type: event_type.to_string(),
                content_preview: msg_with_channel.content.chars().take(80).collect(),
                context_tags: vec![effective_channel.clone(), "dialogue".to_string()],
                visibility: crate::memory::unified_event_ledger::EventVisibility::Participants,
                associated_char_id: None,
            },
        );
    }

    /// 添加一条带自定义 metadata 的消息
    ///
    /// 用于图片消息等需要在 HistoryEntry.metadata 中附加 `kind`/`image_path` 的场景。
    /// 自定义字段会合并到默认 `{"source":"chat"}` 之上（同名键覆盖）。
    pub fn add_message_with_metadata(&self, msg: ChatMessage, metadata: serde_json::Value) {
        // 1. 注入当前渠道标记到 ChatMessage.meta（用于工作记忆通道隔离过滤）
        let default_channel = self.current_channel.lock().clone();
        let session_id = self.current_session_id.lock().clone();
        let mut msg_with_channel = msg.clone();
        let meta = msg_with_channel.meta.get_or_insert_with(Default::default);
        if meta.channel.is_none() {
            meta.channel = Some(default_channel.clone());
        }
        // 从 metadata 提取 kind 注入 msg.meta（与磁盘 HistoryEntry.metadata.kind 对称），
        // 使内存中的 ChatMessage 也携带结构化类型标记，供 prompt 构建/前端渲染区分文件消息。
        if let Some(kind) = metadata.get("kind").and_then(|v| v.as_str()) {
            if meta.kind.is_none() {
                meta.kind = Some(kind.to_string());
            }
        }
        // 最终生效的 channel：优先用 msg 自带的，否则用 current_channel
        let effective_channel = meta.channel.clone().unwrap_or_else(|| default_channel.clone());

        // 2. 加入内存近期消息
        let mut msgs = self.messages.lock();
        msgs.push(msg_with_channel.clone());
        if msgs.len() > self.max_history_len {
            let drop_count = msgs.len() - self.max_history_len;
            msgs.drain(0..drop_count);
        }
        drop(msgs);

        // 3. 构造 HistoryEntry 并合并自定义 metadata + 最终渠道标记 + 会话 ID
        let mut entry = Self::message_to_entry(&msg_with_channel, &session_id);
        if let (Some(target), Some(patch)) =
            (entry.metadata.as_object_mut(), metadata.as_object())
        {
            for (k, v) in patch {
                target.insert(k.clone(), v.clone());
            }
        }
        if let Some(meta) = entry.metadata.as_object_mut() {
            meta.insert("channel".to_string(), serde_json::json!(effective_channel));
        }
        let need_flush = {
            let mut buf = self.buffer.lock();
            buf.push(entry);
            buf.len() >= self.max_buffer_size
        };
        if need_flush {
            if let Err(e) = self.flush_buffer() {
                tracing::error!("缓冲区满触发刷新失败: {}", e);
            }
        }

        // 4. 通知前端对话历史已变更
        if let Some(handle) = self.app_handle.lock().as_ref() {
            let _ = handle.emit(
                "dialogue:changed",
                serde_json::json!({ "character_id": self.char_id }),
            );
        }

        // 5. 直接注册到统一事件账本（独立于角色记忆）
        // 对话消息在写入对话管理器时即成为"事件"，不依赖 MemoryManager 写入。
        // 仅当 metadata 包含 speaker/listener/perspective 时注册。
        let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        crate::memory::unified_event_ledger::register_event_from_dialogue(
            &self.char_id,
            &msg_with_channel.content,
            &metadata,
            now,
        );
    }

    /// 将 ChatMessage 转换为可持久化的 HistoryEntry
    ///
    /// `session_id`：当前会话 ID（来自 ConversationManager），None 表示未启用会话管理。
    pub fn message_to_entry(msg: &ChatMessage, session_id: &Option<String>) -> HistoryEntry {
        let timestamp = msg
            .timestamp
            .map(|t| t.timestamp_millis() as f64 / 1000.0)
            .unwrap_or_else(|| {
                use std::time::{SystemTime, UNIX_EPOCH};
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0)
            });
        HistoryEntry {
            id: format!("{}", uuid::Uuid::new_v4()),
            role: msg.role.clone(),
            content: msg.content.clone(),
            timestamp,
            session_id: session_id.clone(),
            metadata: serde_json::json!({"source": "chat"}),
        }
    }

    pub fn get_history(&self) -> Vec<ChatMessage> {
        let mut history = self.messages.lock().clone();
        // 修复孤立的 tool_call（中断导致 tool_call 无对应 tool_result）
        let repairs = crate::conversation::integrity::ConversationIntegrity::repair(&mut history);
        if !repairs.is_empty() {
            tracing::info!(
                "[DialogueManager] 对话完整性修复：修复了 {} 个孤立的 tool_call",
                repairs.len()
            );
            // 将修复后的消息同步回内存（避免下次 get_history 重复修复）
            *self.messages.lock() = history.clone();
        }
        history
    }

    /// 获取按 channel 过滤的近期消息（用于工作记忆通道隔离）
    ///
    /// 过滤规则：
    /// - 包含 channel 匹配的消息（通过 msg.meta.channel 判断）
    /// - 包含没有 channel 标记的旧消息（向后兼容）
    /// - 排除其他 channel 的消息（如跨角色对话不污染用户对话上下文）
    ///
    /// 当 `channel` 为 None 或 "all" 时返回全部（等价于 get_history）。
    pub fn get_history_filtered_by_channel(&self, channel: Option<&str>) -> Vec<ChatMessage> {
        let history = self.messages.lock().clone();
        match channel {
            None | Some("all") => history,
            Some(target_ch) => history
                .into_iter()
                .filter(|msg| {
                    // 没有 meta 或 channel 字段的旧消息默认保留
                    match msg.meta.as_ref().and_then(|m| m.channel.as_deref()) {
                        None => true,
                        Some(ch) => ch == target_ch,
                    }
                })
                .collect(),
        }
    }

    /// 获取用户可见渠道的近期消息（direct/wechat/proactive 及无标记旧消息）
    ///
    /// 仅排除 `cross_character` 渠道消息，使同一用户在不同入口
    /// （Chat window、侧边栏、微信）切换时共享完整上下文。
    pub fn get_user_visible_history(&self) -> Vec<ChatMessage> {
        self.messages
            .lock()
            .clone()
            .into_iter()
            .filter(|msg| {
                match msg.meta.as_ref().and_then(|m| m.channel.as_deref()) {
                    None => true,
                    Some("cross_character") => false,
                    Some(_) => true,
                }
            })
            .collect()
    }

    /// 获取当前消息渠道（"wechat" / "direct"）
    pub fn get_channel(&self) -> String {
        self.current_channel.lock().clone()
    }

    pub fn clear(&self) {
        self.messages.lock().clear();
    }

    /// 刷新缓冲区到磁盘（JSONL 追加写，带尾部重复检测）
    pub fn flush_buffer(&self) -> VivianResult<()> {
        // 1. 取出缓冲区内容（不在持锁期间进行文件 I/O）
        let messages_to_write = {
            let mut buf = self.buffer.lock();
            if buf.is_empty() {
                return Ok(());
            }
            let taken = buf.clone();
            buf.clear();
            taken
        };

        // 2. 首次刷新：完成旧格式迁移并恢复尾部缓存
        self.ensure_jsonl_ready();

        // 3. 重复检测（仅比对已落盘尾部 20 条）后逐行序列化
        let mut lines: Vec<String> = Vec::with_capacity(messages_to_write.len());
        {
            let mut tail = self.written_tail.lock();
            for msg in &messages_to_write {
                if Self::is_duplicate(msg, tail.as_slice()) {
                    tracing::debug!("检测到重复消息，跳过: {}", msg.id);
                    continue;
                }
                let line = match serde_json::to_string(msg) {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::error!("消息序列化失败，跳过该条: {}", e);
                        continue;
                    }
                };
                lines.push(line);
                tail.push(msg.clone());
                while tail.len() > 20 {
                    tail.remove(0);
                }
            }
        }
        if lines.is_empty() {
            return Ok(());
        }

        // 4. 追加写：单次 write_all 写入全部新行
        let path = self.history_jsonl_file();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let mut buf = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
        for line in lines {
            buf.push_str(&line);
            buf.push('\n');
        }
        if let Err(e) = file.write_all(buf.as_bytes()) {
            tracing::error!("刷新缓冲区失败: {}", e);
            // 写入失败，将消息放回缓冲区等待重试
            let mut b = self.buffer.lock();
            b.extend(messages_to_write);
            return Err(VivianError::Io(e));
        }
        tracing::debug!("已追加 {} 条消息到磁盘", messages_to_write.len());
        Ok(())
    }

    /// 强制刷新缓冲区（程序退出时调用）
    pub fn force_flush(&self) -> VivianResult<()> {
        let result = self.flush_buffer();
        tracing::info!("已强制刷新缓冲区");
        result
    }

    /// 检查消息是否重复（UUID + 内容+时间戳+角色三元组）
    /// 仅检查最后 20 条以优化性能
    fn is_duplicate(new_msg: &HistoryEntry, existing: &[HistoryEntry]) -> bool {
        let start = existing.len().saturating_sub(20);
        let recent = &existing[start..];

        // 1. 检查 UUID 重复
        for msg in recent {
            if msg.id == new_msg.id {
                return true;
            }
        }

        // 2. 检查内容+时间戳（精确到秒）+角色 三元组重复
        let new_ts_key = new_msg.timestamp as i64;
        let new_content_key = new_msg.content.trim();
        for msg in recent {
            let existing_ts = msg.timestamp as i64;
            let existing_content = msg.content.trim();
            if existing_ts == new_ts_key
                && existing_content == new_content_key
                && msg.role == new_msg.role
            {
                return true;
            }
        }

        false
    }

    fn history_dir(&self) -> PathBuf {
        let dir = get_character_data_dir(&self.char_id).join("history");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    /// 旧版全量 JSON 历史文件（仅用于迁移读取）
    fn history_file(&self) -> PathBuf {
        self.history_dir().join("full_chat_history.json")
    }

    /// 当前历史存储：JSONL（每行一条 HistoryEntry，追加写）
    fn history_jsonl_file(&self) -> PathBuf {
        self.history_dir().join("chat_history.jsonl")
    }

    /// JSONL 就绪：首次访问时把旧版 JSON 历史迁移为 JSONL，并恢复尾部缓存
    fn ensure_jsonl_ready(&self) {
        let mut ready = self.jsonl_ready.lock();
        if *ready {
            return;
        }
        let jsonl = self.history_jsonl_file();
        if !jsonl.exists() {
            if let Some(entries) = self.read_legacy_json() {
                if !entries.is_empty() {
                    let mut buf = String::new();
                    for e in &entries {
                        if let Ok(line) = serde_json::to_string(e) {
                            buf.push_str(&line);
                            buf.push('\n');
                        }
                    }
                    if fs::write(&jsonl, &buf).is_ok() {
                        // 旧文件重命名保留为迁移备份
                        let legacy = self.history_file();
                        if legacy.exists() {
                            let _ =
                                fs::rename(&legacy, legacy.with_extension("json.migrated"));
                        }
                        tracing::info!(
                            "[DialogueManager] 历史已迁移为 JSONL（{} 条），旧文件保留为 .migrated",
                            entries.len()
                        );
                    }
                }
            }
        }
        *self.written_tail.lock() = self.read_jsonl_tail(20);
        *ready = true;
    }

    /// 读取旧版 JSON 历史文件（支持 {version,messages} 与裸数组两种格式）
    fn read_legacy_json(&self) -> Option<Vec<HistoryEntry>> {
        let path = self.history_file();
        if !path.exists() {
            return None;
        }
        let content = fs::read_to_string(&path).ok()?;
        let trimmed = content.trim_start();
        if trimmed.starts_with('{') {
            serde_json::from_str::<HistoryFile>(&content).ok().map(|f| f.messages)
        } else if trimmed.starts_with('[') {
            serde_json::from_str::<Vec<HistoryEntry>>(&content).ok()
        } else {
            None
        }
    }

    /// 从 JSONL 尾部读取最近 n 条完整记录（倒序块读取，O(tail)）
    fn read_jsonl_tail(&self, n: usize) -> Vec<HistoryEntry> {
        let path = self.history_jsonl_file();
        if !path.exists() {
            return Vec::new();
        }
        let Ok(mut file) = fs::File::open(&path) else {
            return Vec::new();
        };
        let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if file_len == 0 {
            return Vec::new();
        }
        // 从文件末尾向前分块读取，凑齐 n 个换行或到达文件头为止
        const CHUNK: u64 = 16 * 1024;
        let mut pending: Vec<u8> = Vec::new();
        let mut pos = file_len;
        let mut newlines = 0usize;
        while pos > 0 && newlines <= n {
            let read_len = pos.min(CHUNK) as usize;
            pos -= read_len as u64;
            let mut chunk = vec![0u8; read_len];
            if file.seek(SeekFrom::Start(pos)).is_err()
                || file.read_exact(&mut chunk).is_err()
            {
                return Vec::new();
            }
            newlines += chunk.iter().filter(|&&b| b == b'\n').count();
            chunk.extend_from_slice(&pending);
            pending = chunk;
        }
        let text = String::from_utf8_lossy(&pending);
        let mut entries: Vec<HistoryEntry> = Vec::with_capacity(n);
        // 逐行从后往前解析，跳过不完整的首行（块边界截断）
        for line in text.lines().rev() {
            if entries.len() >= n {
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(e) = serde_json::from_str::<HistoryEntry>(line) {
                entries.push(e);
            }
        }
        // 解析顺序为新→旧，反转为时间升序以配合尾部淘汰
        entries.reverse();
        entries
    }

    /// 读取所有消息：JSONL 逐行解析（跳过损坏行），无 JSONL 时回退旧版 JSON
    fn read_all_messages(&self) -> Vec<HistoryEntry> {
        self.ensure_jsonl_ready();
        let path = self.history_jsonl_file();
        let mut messages: Vec<HistoryEntry> = Vec::new();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => {
                    for (idx, line) in content.lines().enumerate() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<HistoryEntry>(line) {
                            Ok(e) => messages.push(e),
                            Err(e) => {
                                tracing::warn!("历史第 {} 行解析失败，跳过: {}", idx + 1, e);
                            }
                        }
                    }
                }
                Err(e) => tracing::error!("读取历史文件错误: {}", e),
            }
        }
        // 合并缓冲区中尚未落盘的消息，确保读取到最近 2 秒内的最新消息
        let buffered = self.buffer.lock().clone();
        if !buffered.is_empty() {
            let existing_ids: std::collections::HashSet<String> =
                messages.iter().map(|e| e.id.clone()).collect();
            for entry in buffered {
                if !existing_ids.contains(&entry.id) {
                    messages.push(entry);
                }
            }
        }
        messages
    }

    /// 清空历史文件（供命令层调用）：截断 JSONL 并重置尾部缓存
    pub fn clear_history_file(&self) -> VivianResult<()> {
        self.ensure_jsonl_ready();
        let path = self.history_jsonl_file();
        fs::write(&path, "")?;
        *self.written_tail.lock() = Vec::new();
        Ok(())
    }

    /// 保存当前内存中的对话历史（委托给缓冲区刷新）
    pub fn save_history(&self) -> VivianResult<()> {
        self.flush_buffer()
    }

    /// 从磁盘加载历史到内存（用于上下文构建，截断到 max_history_len）
    pub fn load_history(&mut self) -> VivianResult<()> {
        let entries = self.read_all_messages();
        let chat_messages: Vec<ChatMessage> = entries
            .into_iter()
            .map(|e| {
                let ts = chrono::DateTime::from_timestamp(e.timestamp as i64, 0)
                    .map(|dt| dt.with_timezone(&chrono::Local));
                // 从 HistoryEntry.metadata 恢复消息标记（channel + kind）
                let channel = e
                    .metadata
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let kind = e
                    .metadata
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let meta = match (channel.as_ref(), kind.as_ref()) {
                    (Some(ch), Some(k)) => {
                        Some(crate::messages::MessageMeta::user().with_channel(ch).with_kind(k))
                    }
                    (Some(ch), None) => Some(crate::messages::MessageMeta::user().with_channel(ch)),
                    (None, Some(k)) => Some(crate::messages::MessageMeta::user().with_kind(k)),
                    (None, None) => None,
                };
                ChatMessage {
                    role: e.role,
                    content: e.content,
                    timestamp: ts,
                    tool_calls: None,
                    tool_call_id: None,
                    images: None,
                    reasoning: None,
                    meta,
                }
            })
            .collect();

        let len = chat_messages.len();
        let mut msgs = self.messages.lock();
        if len > self.max_history_len {
            *msgs = chat_messages[len - self.max_history_len..].to_vec();
        } else {
            *msgs = chat_messages;
        }
        // 修复从磁盘加载的对话中可能存在的孤立 tool_call
        let repairs = crate::conversation::integrity::ConversationIntegrity::repair(&mut msgs);
        if !repairs.is_empty() {
            tracing::info!(
                "[DialogueManager] 加载历史时对话完整性修复：修复了 {} 个孤立的 tool_call",
                repairs.len()
            );
        }
        tracing::debug!("已加载 {} 条对话历史", msgs.len());
        Ok(())
    }

    /// 获取所有历史消息（供命令层调用）
    ///
    /// 合并"磁盘已持久化" + "内存 buffer 未落盘"两份数据，去重后按时间升序返回。
    /// 仅读磁盘会导致主页会话预览显示陈旧消息：add_message 走 2s 定时缓冲落盘，
    /// 而前端 dialogue:changed + 500ms debounce 后调用 get_chat_history_all 时，
    /// AI 回复可能仍留在 buffer 中尚未 flush，预览被回退到更早的用户消息。
    pub fn get_all_history(&self) -> VivianResult<Vec<HistoryEntry>> {
        // 1. 快照 buffer（快速释放锁，避免阻塞后续磁盘 I/O）
        let buffer_snapshot: Vec<HistoryEntry> = {
            let buf = self.buffer.lock();
            if buf.is_empty() {
                // 必须先释放锁再调用 read_all_messages：它内部会再次获取 buffer 锁
                // 来合并未落盘条目，parking_lot::Mutex 不可重入，持锁状态下调用会死锁。
                drop(buf);
                return Ok(self.read_all_messages());
            }
            buf.clone()
        };

        // 2. 读磁盘持久化数据
        let mut merged = self.read_all_messages();

        // 3. 合并 buffer 中的未落盘条目（ID + 内容/时间戳(秒)/角色 三元组去重，避免旧 flush 残留重复）
        for entry in buffer_snapshot {
            if merged.iter().any(|m| m.id == entry.id) {
                continue;
            }
            let entry_ts = entry.timestamp as i64;
            let entry_content = entry.content.trim();
            let entry_role = entry.role.as_str();
            let dup = merged.iter().any(|m| {
                m.content.trim() == entry_content
                    && (m.timestamp as i64) == entry_ts
                    && m.role == entry_role
            });
            if !dup {
                merged.push(entry);
            }
        }

        merged.sort_by(|a, b| {
            a.timestamp
                .partial_cmp(&b.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(merged)
    }

    /// 分页获取历史消息
    /// - offset: 跳过的消息数（从开头算）
    /// - limit:  返回的最大消息数
    /// - 返回: (消息列表, 是否还有更多)
    ///
    /// 数据源与 get_all_history 一致：磁盘 + 未落盘 buffer 合并去重，
    /// 避免 2s 缓冲窗口内主页/会话视图预览落后于真实最新一条。
    pub fn get_messages_paginated(&self, offset: usize, limit: usize) -> (Vec<HistoryEntry>, bool) {
        let all = self.get_all_history().unwrap_or_default();
        let total = all.len();
        if offset >= total {
            return (Vec::new(), false);
        }
        let end = (offset + limit).min(total);
        let result = all[offset..end].to_vec();
        let has_more = end < total;
        (result, has_more)
    }

    /// 检查指定偏移之后是否还有更多消息
    pub fn has_more_messages(&self, offset: usize) -> bool {
        let all = self.get_all_history().unwrap_or_default();
        offset < all.len()
    }

    /// 获取最近 N 条消息（窗口打开时加载）
    pub fn get_recent_messages(&self, n: usize) -> Vec<HistoryEntry> {
        let all = self.get_all_history().unwrap_or_default();
        if all.len() <= n {
            return all;
        }
        all[all.len() - n..].to_vec()
    }

    /// 获取历史消息总数
    pub fn get_total_count(&self) -> usize {
        self.get_all_history().unwrap_or_default().len()
    }

    /// 获取对话上下文（格式化后的历史消息字符串）
    pub fn get_context(&self, user_input: &str, max_len: Option<usize>) -> String {
        let history = self.get_history();
        let limit = max_len.unwrap_or(history.len());
        let recent = if history.len() > limit {
            &history[history.len() - limit..]
        } else {
            &history
        };

        let mut parts: Vec<String> = recent
            .iter()
            .map(|msg| format!("{}: {}", msg.role, msg.content))
            .collect();

        // 追加当前用户输入
        parts.push(format!("user: {}", user_input));
        parts.join("\n")
    }

    /// 获取历史消息作为字典列表
    pub fn get_history_as_messages(&self, max_len: Option<usize>) -> Vec<serde_json::Value> {
        let history = self.get_history();
        let limit = max_len.unwrap_or(history.len());
        let recent = if history.len() > limit {
            &history[history.len() - limit..]
        } else {
            &history
        };

        recent
            .iter()
            .map(|msg| {
                serde_json::json!({
                    "role": msg.role,
                    "content": msg.content,
                })
            })
            .collect()
    }

    /// 获取历史长度
    pub fn get_history_length(&self) -> usize {
        self.messages.lock().len()
    }

    /// 获取最后一条消息
    pub fn get_last_message(&self) -> Option<ChatMessage> {
        self.messages.lock().last().cloned()
    }

    /// 用前端渲染时刻回传的时间戳覆盖最后一条 assistant 消息的 timestamp。
    ///
    /// 后端 `add_message` 持久化时用的是后端构造消息时刻（T1），而前端 `chat:done`
    /// 渲染时用的是 `Date.now()`（T2）。T1 < T2 会导致 `refreshHistory` 合并时
    /// 按时间戳过滤保留流式消息造成重复。此方法让存储的时间戳与前端渲染时刻对齐。
    ///
    /// 同时更新内存 `messages`、缓冲区 `buffer`（未 flush 时）和磁盘文件（已 flush 时）。
    pub fn patch_last_assistant_timestamp(&self, timestamp_ms: i64) {
        let timestamp_secs = timestamp_ms as f64 / 1000.0;
        let new_dt = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
            .map(|dt| dt.with_timezone(&chrono::Local));

        // 1. 更新内存近期消息中的最后一条 assistant 消息
        {
            let mut msgs = self.messages.lock();
            for m in msgs.iter_mut().rev() {
                if m.role == "assistant" {
                    if let Some(dt) = new_dt {
                        m.timestamp = Some(dt);
                    }
                    break;
                }
            }
        }

        // 2. 先在 buffer 中查找并更新
        {
            let mut buffer = self.buffer.lock();
            for entry in buffer.iter_mut().rev() {
                if entry.role == "assistant" {
                    entry.timestamp = timestamp_secs;
                    return;
                }
            }
        }

        // 3. buffer 中找不到（已 flush），回退到磁盘 JSONL
        self.patch_last_on_disk("assistant", |e| e.timestamp = timestamp_secs);
    }

    /// Patch 最后一条用户消息的 timestamp。
    ///
    /// 用于修正并发发送场景下的消息顺序：用户消息在 brain.think 完成后才写入 dialogue，
    /// timestamp 是 think 完成时刻而非用户实际发送时刻。调用此方法用排队时刻（send_message_stream
    /// 获取 brain_lock 之前记录的时间）覆盖，确保 refreshHistory 按正确时间戳排序。
    pub fn patch_last_user_timestamp(&self, queued_at: chrono::DateTime<chrono::Local>) {
        let timestamp_secs = queued_at.timestamp_millis() as f64 / 1000.0;

        // 1. 更新内存近期消息中的最后一条 user 消息
        {
            let mut msgs = self.messages.lock();
            for m in msgs.iter_mut().rev() {
                if m.role == "user" {
                    m.timestamp = Some(queued_at);
                    break;
                }
            }
        }

        // 2. 先在 buffer 中查找并更新
        {
            let mut buffer = self.buffer.lock();
            for entry in buffer.iter_mut().rev() {
                if entry.role == "user" {
                    entry.timestamp = timestamp_secs;
                    return;
                }
            }
        }

        // 3. buffer 中找不到（已 flush），回退到磁盘 JSONL
        self.patch_last_on_disk("user", |e| e.timestamp = timestamp_secs);
    }

    /// Patch 最后一条用户消息的 HistoryEntry metadata。
    ///
    /// 在 brain.think 返回后调用，用于追加 `kind=file` 等标记到刚写入的用户消息。
    /// 先在 buffer 中查找（未 flush），找不到则回退到磁盘文件。
    pub fn patch_last_user_entry_metadata(&self, patch: serde_json::Value) {
        // 1. 先在 buffer 中查找
        let mut buffer = self.buffer.lock();
        for entry in buffer.iter_mut().rev() {
            if entry.role == "user" {
                if let (Some(target), Some(src)) =
                    (entry.metadata.as_object_mut(), patch.as_object())
                {
                    for (k, v) in src {
                        target.insert(k.clone(), v.clone());
                    }
                }
                return;
            }
        }
        drop(buffer);

        // 2. buffer 中找不到（已 flush），回退到磁盘 JSONL
        self.patch_last_on_disk("user", |e| {
            if let (Some(target), Some(src)) =
                (e.metadata.as_object_mut(), patch.as_object())
            {
                for (k, v) in src {
                    target.insert(k.clone(), v.clone());
                }
            }
        });
    }

    /// Patch 最后一条 assistant 消息的 metadata
    ///
    /// 用于在 TTS 合成完成后回写 voice 消息的 audio_path / duration，
    /// 使历史刷新时能恢复语音气泡。先在 buffer 中查找（未 flush），找不到则回退到磁盘文件。
    pub fn patch_last_assistant_entry_metadata(&self, patch: serde_json::Value) {
        // 1. 先在 buffer 中查找
        let mut buffer = self.buffer.lock();
        for entry in buffer.iter_mut().rev() {
            if entry.role == "assistant" {
                if let (Some(target), Some(src)) =
                    (entry.metadata.as_object_mut(), patch.as_object())
                {
                    for (k, v) in src {
                        target.insert(k.clone(), v.clone());
                    }
                }
                return;
            }
        }
        drop(buffer);

        // 2. buffer 中找不到（已 flush），回退到磁盘 JSONL
        self.patch_last_on_disk("assistant", |e| {
            if let (Some(target), Some(src)) =
                (e.metadata.as_object_mut(), patch.as_object())
            {
                for (k, v) in src {
                    target.insert(k.clone(), v.clone());
                }
            }
        });
    }

    /// 在磁盘 JSONL 中 patch 最后一条指定角色的 HistoryEntry（低频操作，整文件重写）
    ///
    /// 同时同步尾部缓存中对应条目，保持重复检测视图一致。
    fn patch_last_on_disk(&self, role: &str, patch_fn: impl FnOnce(&mut HistoryEntry)) {
        self.ensure_jsonl_ready();
        let path = self.history_jsonl_file();
        if !path.exists() {
            return;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
        for line in lines.iter_mut().rev() {
            let Ok(mut entry) = serde_json::from_str::<HistoryEntry>(line) else {
                continue;
            };
            if entry.role != role {
                continue;
            }
            patch_fn(&mut entry);
            // 同步尾部缓存（按 id 匹配最后一条）
            {
                let mut tail = self.written_tail.lock();
                if let Some(t) = tail.iter_mut().rev().find(|t| t.id == entry.id) {
                    *t = entry.clone();
                }
            }
            if let Ok(json) = serde_json::to_string(&entry) {
                *line = json;
            }
            let mut out = lines.join("\n");
            out.push('\n');
            let _ = std::fs::write(&path, out);
            return;
        }
    }

    /// 清空历史并删除文件
    pub fn clear_history_and_file(&mut self) -> VivianResult<()> {
        self.clear();
        self.clear_history_file()
    }
}

impl Default for DialogueManager {
    fn default() -> Self {
        Self::new(10, "default")
    }
}

// ===== ChatMessageHistory trait 实现 =====

#[async_trait]
impl ChatMessageHistory for DialogueManager {
    async fn add_message(&self, message: ChatMessage) {
        // 委托给现有同步方法（内部已处理内存窗口截断 + 缓冲区写入）
        DialogueManager::add_message(self, message);
    }

    async fn messages(&self) -> Vec<ChatMessage> {
        self.get_history()
    }

    async fn clear(&self) {
        DialogueManager::clear(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_truncate() {
        let mut mgr = DialogueManager::new(3, "test");
        for i in 0..5 {
            mgr.add_message(ChatMessage::user(format!("msg {}", i)));
        }
        assert_eq!(mgr.get_history().len(), 3);
        assert_eq!(mgr.get_history()[0].content, "msg 2");
    }

    #[test]
    fn test_clear() {
        let mut mgr = DialogueManager::new(10, "test");
        mgr.add_message(ChatMessage::user("hello"));
        mgr.clear();
        assert!(mgr.get_history().is_empty());
    }

    #[test]
    fn test_is_duplicate_uuid() {
        let entry = HistoryEntry {
            id: "abc-123".to_string(),
            role: "user".to_string(),
            content: "hello".to_string(),
            timestamp: 1000.5,
            session_id: None,
            metadata: serde_json::json!({}),
        };
        let existing = vec![entry.clone()];
        // 相同 UUID → 重复
        assert!(DialogueManager::is_duplicate(&entry, &existing));
    }

    #[test]
    fn test_is_duplicate_triple() {
        let entry = HistoryEntry {
            id: "new-id".to_string(),
            role: "user".to_string(),
            content: "  hello  ".to_string(),
            timestamp: 1000.7,
            session_id: None,
            metadata: serde_json::json!({}),
        };
        let existing_entry = HistoryEntry {
            id: "old-id".to_string(),
            role: "user".to_string(),
            content: "hello".to_string(),
            timestamp: 1000.9, // 同一秒
            session_id: None,
            metadata: serde_json::json!({}),
        };
        let existing = vec![existing_entry];
        // UUID 不同但 内容+时间戳(秒)+角色 相同 → 重复
        assert!(DialogueManager::is_duplicate(&entry, &existing));
    }

    #[test]
    fn test_not_duplicate() {
        let entry = HistoryEntry {
            id: "new-id".to_string(),
            role: "assistant".to_string(),
            content: "world".to_string(),
            timestamp: 2000.0,
            session_id: None,
            metadata: serde_json::json!({}),
        };
        let existing_entry = HistoryEntry {
            id: "old-id".to_string(),
            role: "user".to_string(),
            content: "hello".to_string(),
            timestamp: 1000.0,
            session_id: None,
            metadata: serde_json::json!({}),
        };
        let existing = vec![existing_entry];
        // 完全不同 → 不重复
        assert!(!DialogueManager::is_duplicate(&entry, &existing));
    }

    #[test]
    fn test_buffer_accumulates() {
        // 缓冲区在 add_message 后应有累积（未满 10 条不触发刷新）
        let mut mgr = DialogueManager::new(50, "test");
        mgr.add_message(ChatMessage::user("buffer test"));
        let buf = mgr.buffer.lock();
        assert_eq!(buf.len(), 1);
    }
}
