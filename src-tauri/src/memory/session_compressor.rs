//! 会话记忆压缩层
//!
//! DialogueManager 维护一个固定窗口（默认 10 条消息），超出部分被静默丢弃。
//! TimeStampedMemory 已有 LLM 摘要能力，但摘要仅注入工具上下文，不进入主对话窗口。
//!
//! 本模块桥接两者：从 TimeStampedMemory 提取摘要，注入为会话回顾消息，
//! 让 LLM 在 10 条近期消息之外也能感知"之前聊过什么"。

use crate::memory::time_stamped::TimeStampedMemory;
use crate::types::response::ChatMessage;

/// 从 TimeStampedMemory 提取摘要并构造会话回顾消息
///
/// 返回 `Some(ChatMessage)` 如果存在有效摘要，否则 `None`。
/// 摘要以 system 消息格式注入，放在对话历史最前面。
pub fn build_conversation_recap(tsm: &TimeStampedMemory) -> Option<ChatMessage> {
    let summary_text = tsm.recent_summary();
    if summary_text.trim().is_empty() {
        return None;
    }

    // 避免重复：如果摘要内容和最近的消息高度重叠，跳过注入
    // （TimeStampedMemory 的 recent_summary 基于被压缩的旧消息，理论上不会与当前窗口重叠）
    Some(ChatMessage::system(format!(
        "[CONVERSATION RECAP]\n{}\n[END RECAP]\n\n注意：以上是此前对话的摘要。你的近期对话历史紧随其后。请自然延续话题，不要重复摘要中的内容。",
        summary_text.trim()
    )))
}

/// 将会话回顾消息注入到消息列表头部
///
/// 在 PipelineState.messages 加载后、Pipeline 执行前调用。
/// 如果存在有效摘要，将其作为第一条消息插入；否则不做修改。
pub fn inject_recap_if_available(
    messages: &mut Vec<ChatMessage>,
    tsm: &TimeStampedMemory,
) {
    if let Some(recap) = build_conversation_recap(tsm) {
        messages.insert(0, recap);
    }
}
