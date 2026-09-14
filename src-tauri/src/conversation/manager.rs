//! 会话管理器（全局单例）

use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;

use super::evaluator;
use super::session::{CloseReason, Conversation, ConversationState, ResponseMode};
use crate::providers::base::LLMRequest;
use crate::types::response::ChatMessage;

/// 全局会话管理器单例
pub static CONVERSATION_MANAGER: Lazy<Arc<ConversationManager>> = Lazy::new(|| {
    Arc::new(ConversationManager {
        inner: RwLock::new(ManagerInner {
            active: std::collections::HashMap::new(),
            seq: 0,
        }),
    })
});

pub struct ConversationManager {
    inner: RwLock<ManagerInner>,
}

struct ManagerInner {
    /// 活跃会话表：key = "owner|participant"（已排序，A↔B 与 B↔A 共享同一 key）
    active: std::collections::HashMap<String, Conversation>,
    /// 会话 ID 自增序号
    seq: u64,
}

impl ConversationManager {
    /// 获取或创建两个角色之间的会话
    ///
    /// - 若已有 Active 会话：返回它
    /// - 若已有 Cooling 会话：检查超时与抢救规则
    ///   - 超时 → 关闭旧会话，创建新会话
    ///   - 未超时 + Continuation Score ≥ RESCUE_THRESHOLD → 抢救回 Active
    ///   - 未超时 + 低分 → 关闭旧会话，创建新会话
    /// - 若已有 Closed 会话或无会话：创建新会话
    ///
    /// `first_message` 用于提取话题。
    /// 获取或创建两个角色之间的会话
    ///
    /// 返回 `Some(Conversation)` 表示可以继续对话，`None` 表示不允许继续：
    /// - **Cooling 未超时 + 抢救失败** → None（等 Cooling 超时或对方发高分消息再说）
    /// - **Closed + 在创建冷却窗口内** → None（防止 A 反复创建新会话）
    /// - **Closed + 超出创建冷却** → 创建新会话返回 Some
    ///
    /// `first_message` 用于提取话题。
    pub fn start_or_continue(
        &self,
        source_id: &str,
        target_id: &str,
        first_message: &str,
    ) -> Option<Conversation> {
        let key = pair_key(source_id, target_id);
        let now = chrono::Local::now().timestamp() as f64;
        let mut inner = self.inner.write();

        if let Some(existing) = inner.active.get(&key).cloned() {
            match existing.state {
                ConversationState::Active | ConversationState::Created => {
                    return Some(existing);
                }
                ConversationState::Cooling => {
                    // 检查 Cooling 超时
                    let cooling_elapsed = existing
                        .cooling_since
                        .map(|s| now - s)
                        .unwrap_or(f64::MAX);

                    if cooling_elapsed > Conversation::COOLING_WINDOW_SECS {
                        // 超时 → 关闭旧会话，检查创建冷却
                        let mut closed = existing.clone();
                        closed.state = ConversationState::Closed;
                        closed.closed_at = Some(now);
                        inner.active.insert(key.clone(), closed);
                        // 超时关闭后立即检查创建冷却：刚关闭的在冷却窗口内，不允许创建
                        // 但 Cooling 超时场景下，会话已经经历了 COOLING_WINDOW_SECS 的冷却，
                        // 如果 COOLING_WINDOW_SECS >= CLOSED_COOLDOWN_SECS 则可以直接创建新会话
                        if cooling_elapsed >= Conversation::CLOSED_COOLDOWN_SECS {
                            return Some(self.create_new(&mut inner, source_id, target_id, first_message));
                        }
                        return None;
                    }

                    // 未超时：检查抢救条件
                    if existing.continuation_score >= Conversation::RESCUE_THRESHOLD {
                        let mut rescued = existing.clone();
                        rescued.state = ConversationState::Active;
                        rescued.cooling_since = None;
                        rescued.last_active_at = now;
                        inner.active.insert(key.clone(), rescued.clone());
                        return Some(rescued);
                    }

                    // 抢救失败：不创建新会话，返回 None（等 Cooling 超时再说）
                    return None;
                }
                ConversationState::Closed => {
                    // 检查创建冷却
                    let closed_elapsed = existing
                        .closed_at
                        .map(|t| now - t)
                        .unwrap_or(f64::MAX);

                    if closed_elapsed < Conversation::CLOSED_COOLDOWN_SECS {
                        // 在创建冷却窗口内：不允许创建新会话
                        return None;
                    }

                    // 超出创建冷却：创建新会话
                    return Some(self.create_new(&mut inner, source_id, target_id, first_message));
                }
            }
        }

        // 无既有会话
        Some(self.create_new(&mut inner, source_id, target_id, first_message))
    }

    /// 强制创建新会话（用于用户主动发消息场景）
    ///
    /// 用户主动发消息时，若 `start_or_continue` 返回 None（旧会话在创建冷却内），
    /// 应忽略创建冷却强制开新会话——用户主动行为本身就是最高优先级信号。
    /// 旧会话会被标记为 Closed(Natural)。
    pub fn force_new_session(
        &self,
        source_id: &str,
        target_id: &str,
        first_message: &str,
    ) -> Conversation {
        let mut inner = self.inner.write();
        let key = pair_key(source_id, target_id);
        let now = chrono::Local::now().timestamp() as f64;
        if let Some(existing) = inner.active.get_mut(&key) {
            existing.state = ConversationState::Closed;
            existing.closed_at = Some(now);
            if existing.close_reason.is_none() {
                existing.close_reason = Some(CloseReason::Natural);
            }
        }
        self.create_new(&mut inner, source_id, target_id, first_message)
    }

    fn create_new(
        &self,
        inner: &mut ManagerInner,
        source_id: &str,
        target_id: &str,
        first_message: &str,
    ) -> Conversation {
        inner.seq += 1;
        let id = format!(
            "conv-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            inner.seq
        );
        let topic = extract_topic(first_message);
        let mut conv = Conversation::new(id, source_id, target_id, &topic);
        conv.state = ConversationState::Active;
        conv.rounds = 1;
        let key = pair_key(source_id, target_id);
        inner.active.insert(key, conv.clone());
        conv
    }

    /// 一轮结束后更新会话状态
    ///
    /// - `response_mode`：本轮 B 的响应模式
    /// - `reply_text`：B 的回复文本（Speak 模式才有）
    /// - `user_input`：A 的输入（用于计算 Novelty）
    pub fn update_after_round(
        &self,
        conv_id: &str,
        response_mode: ResponseMode,
        reply_text: Option<&str>,
        user_input: &str,
    ) -> Option<Conversation> {
        let mut inner = self.inner.write();
        let key = inner
            .active
            .iter()
            .find(|(_, c)| c.id == conv_id)
            .map(|(k, _)| k.clone())?;
        let conv = inner.active.get_mut(&key)?;
        let now = chrono::Local::now().timestamp() as f64;

        conv.rounds += 1;
        conv.last_active_at = now;
        conv.last_response_mode = response_mode;

        // 话题补全：首条消息信息量不足时 topic 为 TOPIC_PENDING，
        // 后续若有信息量的消息到达，用它替换占位符。
        if conv.topic == TOPIC_PENDING {
            let trimmed_input = user_input.trim();
            if !trimmed_input.is_empty() && !is_low_information(trimmed_input) {
                conv.topic = extract_topic(trimmed_input);
            }
        }

        // 计算 Energy 和 Novelty
        let new_novelty = evaluator::compute_novelty(user_input, reply_text);
        let energy_delta = evaluator::compute_energy_delta(response_mode, new_novelty, conv.novelty);
        conv.energy = (conv.energy + energy_delta).clamp(0.0, 1.0);
        conv.novelty = new_novelty;

        // 计算 Continuation Score
        conv.continuation_score = evaluator::compute_continuation_score(conv);
        // 记录到历史（保留最近 5 轮，用于 Open Loop 检测）
        conv.record_continuation_score(conv.continuation_score);

        // 轮次硬上限：达到 MAX_ROUNDS 后强制进入 Cooling，避免 token 过度消耗
        let force_end = conv.rounds >= Conversation::MAX_ROUNDS;

        // 根据得分决定状态转换
        if response_mode == ResponseMode::Ignore {
            // Ignore 直接进入 Cooling
            conv.state = ConversationState::Cooling;
            conv.cooling_since = Some(now);
        } else if force_end {
            // 轮次达上限：强制进入 Cooling 结束话题
            conv.state = ConversationState::Cooling;
            conv.cooling_since = Some(now);
            tracing::info!(
                "[conversation] 会话 {} 达到 {} 轮上限，强制进入 Cooling",
                conv.id, Conversation::MAX_ROUNDS
            );
        } else if conv.continuation_score < Conversation::CONTINUATION_THRESHOLD
            || conv.energy < Conversation::ENERGY_THRESHOLD
            || conv.novelty < Conversation::NOVELTY_THRESHOLD
        {
            // 倾向结束：进入 Cooling
            conv.state = ConversationState::Cooling;
            conv.cooling_since = Some(now);
        } else {
            conv.state = ConversationState::Active;
            conv.cooling_since = None;
        }

        Some(conv.clone())
    }

    /// 手动标记会话为 Cooling（LLM 返回 conversation_action="close" 时调用）
    pub fn mark_cooling(&self, conv_id: &str) -> Option<Conversation> {
        let mut inner = self.inner.write();
        let key = inner
            .active
            .iter()
            .find(|(_, c)| c.id == conv_id)
            .map(|(k, _)| k.clone())?;
        let conv = inner.active.get_mut(&key)?;
        let now = chrono::Local::now().timestamp() as f64;
        conv.state = ConversationState::Cooling;
        conv.cooling_since = Some(now);
        Some(conv.clone())
    }

    /// 关闭会话
    pub fn close(&self, conv_id: &str) -> Option<Conversation> {
        let mut inner = self.inner.write();
        let key = inner
            .active
            .iter()
            .find(|(_, c)| c.id == conv_id)
            .map(|(k, _)| k.clone())?;
        let conv = inner.active.get_mut(&key)?;
        conv.state = ConversationState::Closed;
        Some(conv.clone())
    }

    /// 关闭会话并记录原因
    ///
    /// 由关键词检测（GoodNight/GoodBye/Interrupted）或超时规则（NoResponse/Timeout）
    /// 触发。close_reason 会持久化在 Conversation 上，供后续 Episode 封包、
    /// Relationship 联动、proactive 决策使用。
    pub fn close_with_reason(
        &self,
        conv_id: &str,
        reason: CloseReason,
    ) -> Option<Conversation> {
        let mut inner = self.inner.write();
        let key = inner
            .active
            .iter()
            .find(|(_, c)| c.id == conv_id)
            .map(|(k, _)| k.clone())?;
        let now = chrono::Local::now().timestamp() as f64;
        let conv = inner.active.get_mut(&key)?;
        conv.state = ConversationState::Closed;
        conv.closed_at = Some(now);
        conv.close_reason = Some(reason);
        Some(conv.clone())
    }

    /// 关闭两个角色（或 user↔char）之间的活跃会话
    ///
    /// 便捷方法：不需要 conv_id，直接按参与者 pair 关闭。
    /// 用于关键词检测命中后立即关闭当前会话。
    pub fn close_pair_with_reason(
        &self,
        a_id: &str,
        b_id: &str,
        reason: CloseReason,
    ) -> Option<Conversation> {
        let key = pair_key(a_id, b_id);
        let mut inner = self.inner.write();
        let conv = inner.active.get_mut(&key)?;
        let now = chrono::Local::now().timestamp() as f64;
        conv.state = ConversationState::Closed;
        conv.closed_at = Some(now);
        conv.close_reason = Some(reason);
        Some(conv.clone())
    }

    /// 记录用户发言时间戳（仅 User↔Agent 会话使用）
    ///
    /// 在 start_or_continue 之后调用，用于后续 NoResponse/Timeout 判定。
    pub fn touch_user_message(&self, char_id: &str) {
        let key = pair_key("user", char_id);
        let now = chrono::Local::now().timestamp() as f64;
        let mut inner = self.inner.write();
        if let Some(conv) = inner.active.get_mut(&key) {
            conv.last_user_message_at = Some(now);
        }
    }

    /// 检查 User↔Agent 会话是否应当因超时关闭
    ///
    /// 在 proactive_tick 中调用：
    /// - 会话 Active 但用户超过 `user_timeout_secs` 未发言 → close(Timeout)
    /// - 会话 Active 但 Agent 主动聊天被忽略（last_response_mode=Ignore 且超过阈值）→ close(NoResponse)
    ///
    /// 返回被关闭的 (char_id, CloseReason, Conversation) 列表，调用方可据此调整 proactive 行为
    /// 并检查 Open Loop（通过 Conversation.recent_continuation_scores）。
    pub fn sweep_user_session_timeouts(&self, user_timeout_secs: f64) -> Vec<(String, CloseReason, Conversation)> {
        let mut inner = self.inner.write();
        let now = chrono::Local::now().timestamp() as f64;
        let mut closed: Vec<(String, CloseReason, Conversation)> = Vec::new();
        let mut to_close: Vec<(String, CloseReason)> = Vec::new();

        for (key, conv) in inner.active.iter() {
            // 只处理 user↔char 会话（key 以 "user|" 开头）
            if !key.starts_with("user|") {
                continue;
            }
            if conv.state == ConversationState::Closed {
                continue;
            }

            let char_id = key
                .strip_prefix("user|")
                .or_else(|| key.strip_suffix("|user"))
                .unwrap_or("")
                .to_string();

            // 用户超时：Active/Cooling 状态下，用户长时间未发言
            if let Some(last_user_ts) = conv.last_user_message_at {
                let elapsed = now - last_user_ts;
                if elapsed >= user_timeout_secs {
                    to_close.push((char_id.clone(), CloseReason::Timeout));
                    continue;
                }
            } else if conv.state == ConversationState::Active && conv.rounds > 0 {
                // 有轮次但从未记录用户发言（理论不应发生，兜底）
                let elapsed = now - conv.created_at;
                if elapsed >= user_timeout_secs {
                    to_close.push((char_id.clone(), CloseReason::Timeout));
                    continue;
                }
            }
        }

        for (char_id, reason) in to_close {
            let key = pair_key("user", &char_id);
            if let Some(conv) = inner.active.get_mut(&key) {
                conv.state = ConversationState::Closed;
                conv.closed_at = Some(now);
                conv.close_reason = Some(reason);
                closed.push((char_id, reason, conv.clone()));
            }
        }
        closed
    }

    /// 查询某角色与用户的会话是否已关闭（或不存在）
    ///
    /// 用于 proactive 决策：Closed 状态下不主动搭话，除非有新 Trigger。
    /// `is_user_session_closed` 返回 true 时，proactive 应跳过主动消息。
    pub fn is_user_session_closed(&self, char_id: &str) -> bool {
        let key = pair_key("user", char_id);
        let inner = self.inner.read();
        match inner.active.get(&key) {
            None => true,
            Some(conv) => conv.state == ConversationState::Closed,
        }
    }

    /// 查询是否有任何 User↔Agent 会话处于 Active 状态
    ///
    /// 用于 proactive 决策：用户正在与任意角色聊天时，抑制打断性行为
    /// （TopicExtension / CrossCharacterReply 等），避免主动消息打断正在进行的对话。
    pub fn is_any_user_session_active(&self) -> bool {
        let inner = self.inner.read();
        inner.active.iter().any(|(key, conv)| {
            key.starts_with("user|") && conv.state == ConversationState::Active
        })
    }

    /// 查询某角色与用户的会话关闭原因
    pub fn user_session_close_reason(&self, char_id: &str) -> Option<CloseReason> {
        let key = pair_key("user", char_id);
        let inner = self.inner.read();
        inner.active.get(&key).and_then(|c| c.close_reason)
    }

    /// 向会话追加记忆 ID（用于 close 时触发 seal_episode）
    ///
    /// 由 memory_saving step 在写入记忆后调用。
    /// close 时调用方据此触发 `EpisodeStore::seal_episode`。
    pub fn add_memory_to_session(&self, conv_id: &str, memory_id: String) {
        let mut inner = self.inner.write();
        for (_, conv) in inner.active.iter_mut() {
            if conv.id == conv_id {
                conv.memory_ids.push(memory_id);
                return;
            }
        }
    }

    /// 获取会话的记忆 ID 列表（close 时调用方用于 seal_episode）
    pub fn get_session_memory_ids(&self, conv_id: &str) -> Vec<String> {
        let inner = self.inner.read();
        inner
            .active
            .values()
            .find(|c| c.id == conv_id)
            .map(|c| c.memory_ids.clone())
            .unwrap_or_default()
    }

    /// Cooling 超时清理：所有 Cooling 状态超过 COOLING_WINDOW_SECS 的会话自动关闭
    ///
    /// 返回被关闭的 Conversation 列表（可挂在 proactive_tick 上定期调用）。
    /// 调用方可据此做 Open Loop 检测（检查 continuation_score 历史）。
    pub fn sweep_cooling(&self) -> Vec<Conversation> {
        let mut inner = self.inner.write();
        let now = chrono::Local::now().timestamp() as f64;
        let mut closed_convs = Vec::new();
        let mut to_close: Vec<String> = Vec::new();

        for (key, conv) in inner.active.iter() {
            if conv.state == ConversationState::Cooling {
                if let Some(since) = conv.cooling_since {
                    if now - since > Conversation::COOLING_WINDOW_SECS {
                        to_close.push(key.clone());
                    }
                }
            }
        }

        for key in to_close {
            if let Some(conv) = inner.active.get_mut(&key) {
                conv.state = ConversationState::Closed;
                conv.closed_at = Some(now);
                if conv.close_reason.is_none() {
                    conv.close_reason = Some(CloseReason::Natural);
                }
                closed_convs.push(conv.clone());
            }
        }
        closed_convs
    }

    /// 查询两个角色之间的活跃会话
    pub fn get(&self, source_id: &str, target_id: &str) -> Option<Conversation> {
        let key = pair_key(source_id, target_id);
        self.inner.read().active.get(&key).cloned()
    }

    /// 列出所有会话（含已关闭但尚未被新会话替换的）
    ///
    /// 供 Mind Inspector 的 `get_sessions` 命令使用：返回所有 pair 的会话快照，
    /// 调用方可按 `character_id` 过滤、按 `last_active_at` 排序。
    pub fn list_all(&self) -> Vec<Conversation> {
        self.inner.read().active.values().cloned().collect()
    }

    /// 会话是否已关闭
    pub fn is_closed(&self, conv_id: &str) -> bool {
        let inner = self.inner.read();
        inner
            .active
            .values()
            .find(|c| c.id == conv_id)
            .map(|c| c.state == ConversationState::Closed)
            .unwrap_or(true)
    }

    /// 清理已关闭超过 1 小时的非用户会话，防止 HashMap 无限膨胀
    ///
    /// 在 sweep_cooling 之后调用。user↔char 会话保留（供 Mind Inspector 查看）。
    pub fn purge_stale(&self) {
        let mut inner = self.inner.write();
        let now = chrono::Local::now().timestamp() as f64;
        const STALE_THRESHOLD_SECS: f64 = 3600.0;

        inner.active.retain(|key, conv| {
            if key.starts_with("user|") {
                return true;
            }
            match (conv.state, conv.closed_at) {
                (ConversationState::Closed, Some(closed_at)) => {
                    now - closed_at < STALE_THRESHOLD_SECS
                }
                _ => true,
            }
        });
    }
}

/// 生成已排序的会话键（A↔B 与 B↔A 共享同一 key，"user" 作为固定标识也兼容）
fn pair_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{}|{}", a, b)
    } else {
        format!("{}|{}", b, a)
    }
}

/// 话题占位符：首条消息信息量不足时使用，等待后续消息补全
pub const TOPIC_PENDING: &str = "(待识别)";

/// 从首条消息提取话题（简短化处理，最长 20 字）
///
/// 信息量不足（寒暄/表情/短词）时返回 `TOPIC_PENDING`，由 `update_after_round`
/// 在后续有信息量的消息到达时补全。
fn extract_topic(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return "(无话题)".to_string();
    }
    if is_low_information(trimmed) {
        return TOPIC_PENDING.to_string();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 20 {
        trimmed.to_string()
    } else {
        let head: String = chars.iter().take(20).collect();
        format!("{}…", head)
    }
}

/// 判断文本是否信息量不足（寒暄、表情、短词、纯标点）
fn is_low_information(text: &str) -> bool {
    let stripped: String = text
        .chars()
        .filter(|c| {
            !c.is_whitespace()
                && !is_punctuation(*c)
                && !is_emoji(*c)
        })
        .collect();
    if stripped.is_empty() {
        return true;
    }
    // 短词（≤2 字符）一律视为低信息量，覆盖"诶/嗯/哦/嗨/在吗/那个"等
    if stripped.chars().count() <= 2 {
        return true;
    }
    let lower = stripped.to_lowercase();
    const LOW_INFO_WORDS: &[&str] = &[
        "在吗", "在么", "你好", "您好", "哈喽", "hello", "hi", "hey",
        "晚安", "早安", "早上好", "晚上好", "午安", "中午好",
        "嗯嗯", "哈哈", "呵呵", "嘿嘿", "哎呀", "哎",
        "好的", "好吧", "ok", "okay", "嗯哼",
        "干嘛", "干啥", "咋了", "怎么了",
    ];
    LOW_INFO_WORDS.iter().any(|w| lower == *w)
}

fn is_punctuation(c: char) -> bool {
    matches!(c, '，'|'。'|'、'|'；'|'：'|'！'|'？'|','|'.'|';'|':'|'!'|'?'|'~'|'—'|'…'|'‘'|'’'|'“'|'”'|'（'|'）'|'('|')'|'【'|'】'|'['|']'|'{'|'}')
}

fn is_emoji(c: char) -> bool {
    // 常见 emoji 区段：杂项符号、表情符号、运输符号、补充符号
    let code = c as u32;
    (0x1F300..=0x1FAFF).contains(&code)
        || (0x2600..=0x27BF).contains(&code)
        || (0x1F1E6..=0x1F1FF).contains(&code)
}

/// Open Loop 检测：会话关闭时若话题仍有生命力，在最后一条记忆上附加 follow_up hook
///
/// 话题字段优先用 LLM 总结会话期间产生的记忆内容生成（≤20 字），
/// 失败时回退到 `conv.topic`。LLM 调用是异步的，因此本函数为 async。
///
/// 触发条件：
/// 1. 关闭原因不是用户主动告别（GoodNight/GoodBye）或冲突（Conflict）
/// 2. 最近 3 轮平均 continuity > 0.5（话题还没聊完）
/// 3. 会话有记忆产出（memory_ids 非空）
pub async fn maybe_mark_open_loop(
    conv: &super::Conversation,
    memory: &Arc<crate::memory::MemoryManager>,
    router: &Arc<crate::providers::router::ModelRouter>,
) {
    use crate::memory::types::OpenHook;

    if let Some(reason) = conv.close_reason {
        if reason.is_user_farewell() || matches!(reason, super::CloseReason::Conflict) {
            return;
        }
    }

    let avg = match conv.recent_score_avg(3) {
        Some(v) if v > 0.5 => v,
        _ => return,
    };

    let Some(last_id) = conv.memory_ids.last() else {
        return;
    };

    // 话题总结：从会话产生的记忆中取最后若干条内容，让 LLM 提炼一句话主题
    let topic = summarize_conversation_topic(conv, memory, router).await;

    let hook = OpenHook::new(
        "follow_up",
        format!(
            "上次聊到「{}」还没聊完（连续性 {:.0}%），下次提到相关话题时自然续接",
            topic, avg * 100.0
        ),
    );
    memory.attach_open_hook(last_id, hook);

    tracing::debug!(
        "[OpenLoop] 会话 {} 标记为待续话题「{}」（avg_score={:.2}）",
        conv.id,
        topic,
        avg
    );
}

/// 用 LLM 总结会话话题（≤20 字），失败回退到 `conv.topic`
async fn summarize_conversation_topic(
    conv: &super::Conversation,
    memory: &Arc<crate::memory::MemoryManager>,
    router: &Arc<crate::providers::router::ModelRouter>,
) -> String {
    // 取会话最后若干条记忆内容拼接
    let tail_ids: Vec<&String> = conv.memory_ids.iter().rev().take(5).collect();
    let mut snippets: Vec<String> = Vec::with_capacity(tail_ids.len());
    for id in &tail_ids {
        if let Some(content) = memory.get_memory_content_by_id(id) {
            let trimmed: String = content.chars().take(120).collect();
            if !trimmed.trim().is_empty() {
                snippets.push(trimmed);
            }
        }
    }
    if snippets.is_empty() {
        return conv.topic.clone();
    }

    let conversation = snippets
        .iter()
        .map(|s| format!("- {}", s))
        .collect::<Vec<_>>()
        .join("\n");

    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    let prompt = match lang_norm {
        "en" => format!(
            "Summarize the topic of the following conversation snippets in ONE short phrase (max 20 chars, no quotes).\n\n{}",
            conversation
        ),
        "ja" => format!(
            "以下の会話の話題を1つの短いフレーズで表してください（20字以内、引用符なし）。\n\n{}",
            conversation
        ),
        _ => format!(
            "用一句话概括以下对话片段的话题（20字以内，不要加引号）：\n\n{}",
            conversation
        ),
    };

    let messages = vec![ChatMessage::user(&prompt)];
    match router.generate(LLMRequest::new("chat", messages)).await {
        Ok(text) => {
            let cleaned = text.trim().trim_matches(|c: char| c == '"' || c == '「' || c == '」').to_string();
            if cleaned.is_empty() {
                conv.topic.clone()
            } else {
                let chars: Vec<char> = cleaned.chars().collect();
                if chars.len() <= 20 {
                    cleaned
                } else {
                    format!("{}…", chars.iter().take(20).collect::<String>())
                }
            }
        }
        Err(e) => {
            tracing::warn!("[OpenLoop] LLM 话题总结失败，回退 conv.topic: {}", e);
            conv.topic.clone()
        }
    }
}
