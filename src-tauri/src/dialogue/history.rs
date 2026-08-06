//! 对话历史后端抽象
//!
//! 将"对话消息如何持久化"与"对话消息如何被消费"解耦：
//! - 后端层（`ChatMessageHistory` trait）：定义 `add_message` / `messages` / `clear` 等标准接口
//! - 实现层：`DialogueManager`（JSON 文件 + 内存窗口）
//!
//! 主路径（`BrainChatChain`）通过 trait 持有后端，未来可替换为 SQLite / Redis 等实现，
//! 而无需改动消费侧。生命周期管理（load_history / start_background_flush / force_flush）
//! 仍由具体后端实现独有，不进入 trait。

use async_trait::async_trait;

use crate::types::response::ChatMessage;

/// 对话历史后端 trait
///
/// 对齐 LangChain `BaseChatMessageHistory`：仅关注消息的增删读，
/// 不涉及窗口压缩、摘要等策略（那些属于 `ChatMemoryStrategy` 层）。
#[async_trait]
pub trait ChatMessageHistory: Send + Sync {
    /// 追加一条消息
    async fn add_message(&self, message: ChatMessage);

    /// 追加 user 消息（便捷方法）
    async fn add_user_message(&self, content: &str) {
        self.add_message(ChatMessage::user(content)).await;
    }

    /// 追加 assistant 消息（便捷方法）
    async fn add_ai_message(&self, content: &str) {
        self.add_message(ChatMessage::assistant(content)).await;
    }

    /// 批量追加消息（默认实现逐条调用 `add_message`）
    async fn add_messages(&self, messages: Vec<ChatMessage>) {
        for m in messages {
            self.add_message(m).await;
        }
    }

    /// 读取当前内存窗口内的全部消息（顺序：旧 → 新）
    async fn messages(&self) -> Vec<ChatMessage>;

    /// 清空内存窗口（不影响已落盘的历史）
    async fn clear(&self);
}
