//! 对话记忆策略
//!
//! 对齐 LangChain `BaseChatMemory`：决定"从历史中取什么放入 LLM 上下文"
//! 与"如何把新一轮对话写回历史"。后端持久化由 `ChatMessageHistory` 负责，
//! 策略层只关注窗口/压缩/摘要逻辑。
//!
//! 三种策略：
//! - [`WindowStrategy`]：保留最近 N 条（对应 `ConversationBufferWindowMemory`）
//! - [`TokenStrategy`]：按 token 预算保留（对应 `ConversationTokenBufferMemory`）
//! - [`SummaryBufferStrategy`]：超阈值时 LLM 摘要前缀 + 保留近期（对应 `ConversationSummaryBufferMemory`）

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::dialogue::history::ChatMessageHistory;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;
use crate::utils::truncate_chars;

/// 对话记忆策略 trait
#[async_trait]
pub trait ChatMemoryStrategy: Send + Sync {
    /// 读取要注入 LLM 上下文的消息（顺序：旧 → 新）
    async fn load_context(&self) -> Vec<ChatMessage>;

    /// 写入一轮对话（user + assistant）
    async fn save_context(&self, user: &str, ai: &str) {
        self.history().add_user_message(user).await;
        self.history().add_ai_message(ai).await;
    }

    /// 获取底层 history 后端（供 save_context 默认实现使用）
    fn history(&self) -> &Arc<dyn ChatMessageHistory>;
}

// ============================================================================
// WindowStrategy：保留最近 N 条消息
// ============================================================================

/// 简单窗口策略：只保留最近 `window_size` 条消息
pub struct WindowStrategy {
    history: Arc<dyn ChatMessageHistory>,
    window_size: usize,
}

impl WindowStrategy {
    pub fn new(history: Arc<dyn ChatMessageHistory>, window_size: usize) -> Self {
        Self {
            history,
            window_size: window_size.max(1),
        }
    }
}

#[async_trait]
impl ChatMemoryStrategy for WindowStrategy {
    async fn load_context(&self) -> Vec<ChatMessage> {
        let msgs = self.history.messages().await;
        let start = msgs.len().saturating_sub(self.window_size);
        msgs[start..].to_vec()
    }

    fn history(&self) -> &Arc<dyn ChatMessageHistory> {
        &self.history
    }
}

// ============================================================================
// TokenStrategy：按 token 预算保留
// ============================================================================

/// Token 预算策略：从最新消息向前累加，直到 token 总量超过预算
pub struct TokenStrategy {
    history: Arc<dyn ChatMessageHistory>,
    max_tokens: usize,
}

impl TokenStrategy {
    pub fn new(history: Arc<dyn ChatMessageHistory>, max_tokens: usize) -> Self {
        Self {
            history,
            max_tokens: max_tokens.max(64),
        }
    }

    /// 粗略 token 估算（中文约 3 字符 = 1 token，英文约 4 字符 = 1 token）
    fn estimate_tokens(text: &str) -> usize {
        text.chars().count() / 3
    }

    fn estimate_message_tokens(msg: &ChatMessage) -> usize {
        Self::estimate_tokens(&msg.content) + 4
    }
}

#[async_trait]
impl ChatMemoryStrategy for TokenStrategy {
    async fn load_context(&self) -> Vec<ChatMessage> {
        let msgs = self.history.messages().await;
        let mut budget = 0usize;
        let mut start = msgs.len();
        for (i, msg) in msgs.iter().rev().enumerate() {
            let t = Self::estimate_message_tokens(msg);
            if budget + t > self.max_tokens {
                break;
            }
            budget += t;
            start = msgs.len() - i - 1;
        }
        msgs[start..].to_vec()
    }

    fn history(&self) -> &Arc<dyn ChatMessageHistory> {
        &self.history
    }
}

// ============================================================================
// SummaryBufferStrategy：LLM 摘要前缀 + 保留近期
// ============================================================================

/// 摘要缓冲策略：当消息数超过 `max_messages` 时，将较早的消息压缩为 LLM 摘要，
/// 保留最近 `retain_recent` 条 + 摘要作为 system 消息前缀。
pub struct SummaryBufferStrategy {
    history: Arc<dyn ChatMessageHistory>,
    /// 触发摘要的消息条数阈值
    max_messages: usize,
    /// 摘要后保留的近期消息条数
    retain_recent: usize,
    /// LLM 路由器（None 时降级为本地拼接摘要）
    router: Option<Arc<ModelRouter>>,
    /// 缓存的摘要文本（每次触发摘要时更新）
    summary: Mutex<Option<String>>,
}

impl SummaryBufferStrategy {
    pub fn new(
        history: Arc<dyn ChatMessageHistory>,
        max_messages: usize,
        retain_recent: usize,
        router: Option<Arc<ModelRouter>>,
    ) -> Self {
        Self {
            history,
            max_messages: max_messages.max(4),
            retain_recent: retain_recent.max(2),
            router,
            summary: Mutex::new(None),
        }
    }

    /// 本地兜底摘要（LLM 不可用时）
    fn local_summarize(messages: &[ChatMessage]) -> String {
        let topics: Vec<String> = messages
            .iter()
            .filter(|m| m.role == "user" && !m.content.trim().is_empty())
            .map(|m| truncate_chars(&m.content, 60))
            .collect();
        if topics.is_empty() {
            return String::new();
        }
        let count = topics.len();
        let first: String = truncate_chars(&topics[0], 30);
        format!("Earlier: {}... ({} turns)", first, count)
    }

    /// LLM 摘要
    async fn summarize_with_llm(router: &ModelRouter, messages: &[ChatMessage]) -> String {
        let conversation = messages
            .iter()
            .map(|m| {
                let speaker_tag = if m.role == "user" {
                    "[User says to me]"
                } else {
                    "[I say to User]"
                };
                format!("{} {}", speaker_tag, m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let lang_norm =
            crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        let prompt = match lang_norm {
            "en" => format!(
                "Summarize the following conversation concisely (under 200 chars).\n\
                 Keep key facts, preferences, and decisions. Drop pleasantries.\n\n{}",
                conversation
            ),
            "ja" => format!(
                "以下の会話を簡潔にまとめてください（200字以内）。\n\
                 重要な事実、好み、決定事項を残し、挨拶や雑談は省いてください。\n\n{}",
                conversation
            ),
            _ => format!(
                "请简洁总结以下对话（200字以内）。\n\
                 保留关键事实、偏好和决定，省略客套话。\n\n{}",
                conversation
            ),
        };
        let llm_messages = vec![ChatMessage::user(&prompt)];
        match router.generate(LLMRequest::new("chat", llm_messages)).await {
            Ok(text) => text.trim().to_string(),
            Err(e) => {
                tracing::warn!("[SummaryBufferStrategy] LLM 摘要失败，回退本地: {}", e);
                Self::local_summarize(messages)
            }
        }
    }
}

#[async_trait]
impl ChatMemoryStrategy for SummaryBufferStrategy {
    async fn load_context(&self) -> Vec<ChatMessage> {
        let msgs = self.history.messages().await;
        if msgs.len() <= self.max_messages {
            return msgs;
        }

        // 需要摘要：较早的消息压缩，保留最近 retain_recent 条
        let split_at = msgs.len().saturating_sub(self.retain_recent);
        if split_at == 0 {
            return msgs;
        }

        let old = &msgs[..split_at];
        let recent = &msgs[split_at..];

        // 组合指纹：消息数 + 总字符数 + 末尾消息内容，避免首条不变但后续变化时摘要陈旧
        let fingerprint = {
            let total_chars: usize = old.iter().map(|m| m.content.chars().count()).sum();
            let last_head = old
                .last()
                .map(|m| truncate_chars(&m.content, 32))
                .unwrap_or_default();
            format!("{}|{}|{}", old.len(), total_chars, last_head)
        };
        let mut summary_guard = self.summary.lock().await;
        let need_recompute = match &*summary_guard {
            Some(s) => !s.starts_with(&fingerprint),
            None => true,
        };

        if need_recompute {
            let new_summary = match &self.router {
                Some(router) => Self::summarize_with_llm(router, old).await,
                None => Self::local_summarize(old),
            };
            *summary_guard = Some(format!("{}\n{}", fingerprint, new_summary));
        }

        let summary_text = summary_guard
            .as_ref()
            .and_then(|s| s.split_once('\n').map(|(_, summary)| summary))
            .unwrap_or("")
            .to_string();

        let mut result = Vec::with_capacity(self.retain_recent + 1);
        if !summary_text.is_empty() {
            result.push(ChatMessage::system(format!(
                "[Context Summary] {}",
                summary_text
            )));
        }
        result.extend(recent.iter().cloned());
        result
    }

    fn history(&self) -> &Arc<dyn ChatMessageHistory> {
        &self.history
    }
}

// ============================================================================
// 工厂函数
// ============================================================================

/// 策略类型枚举（用于配置驱动选择）
#[derive(Debug, Clone)]
pub enum MemoryStrategyKind {
    Window(usize),
    Token(usize),
    SummaryBuffer { max_messages: usize, retain_recent: usize },
}

/// 根据类型创建策略实例
pub fn create_strategy(
    kind: MemoryStrategyKind,
    history: Arc<dyn ChatMessageHistory>,
    router: Option<Arc<ModelRouter>>,
) -> Box<dyn ChatMemoryStrategy> {
    match kind {
        MemoryStrategyKind::Window(n) => Box::new(WindowStrategy::new(history, n)),
        MemoryStrategyKind::Token(n) => Box::new(TokenStrategy::new(history, n)),
        MemoryStrategyKind::SummaryBuffer {
            max_messages,
            retain_recent,
        } => Box::new(SummaryBufferStrategy::new(
            history,
            max_messages,
            retain_recent,
            router,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogue::DialogueManager;

    fn make_history() -> Arc<dyn ChatMessageHistory> {
        Arc::new(DialogueManager::new(50, "test"))
    }

    async fn populate(history: &Arc<dyn ChatMessageHistory>, n: usize) {
        for i in 0..n {
            history
                .add_user_message(&format!("user msg {}", i))
                .await;
            history
                .add_ai_message(&format!("ai msg {}", i))
                .await;
        }
    }

    #[tokio::test]
    async fn test_window_strategy_truncates() {
        let history = make_history();
        populate(&history, 5).await; // 10 messages
        let strategy = WindowStrategy::new(history.clone(), 4);
        let ctx = strategy.load_context().await;
        assert_eq!(ctx.len(), 4);
        assert_eq!(ctx[0].content, "ai msg 3"); // 最近 4 条：ai3, user4, ai4, user5
    }

    #[tokio::test]
    async fn test_token_strategy_respects_budget() {
        let history = make_history();
        populate(&history, 3).await; // 6 messages
        // 每条 "user msg N" 约 4 tokens，预算 20 → 约 5 条
        let strategy = TokenStrategy::new(history.clone(), 20);
        let ctx = strategy.load_context().await;
        assert!(ctx.len() <= 6);
        assert!(!ctx.is_empty());
    }

    #[tokio::test]
    async fn test_summary_buffer_falls_back_to_local() {
        let history = make_history();
        populate(&history, 10).await; // 20 messages
        let strategy = SummaryBufferStrategy::new(history.clone(), 10, 4, None);
        let ctx = strategy.load_context().await;
        // 应包含 1 条 system summary + 4 条 recent = 5 条
        assert_eq!(ctx.len(), 5);
        assert_eq!(ctx[0].role, "system");
        assert!(ctx[0].content.contains("[Context Summary]"));
    }

    #[tokio::test]
    async fn test_save_context_writes_to_history() {
        let history = make_history();
        let strategy = WindowStrategy::new(history.clone(), 10);
        strategy.save_context("hello", "hi there").await;
        let msgs = history.messages().await;
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "hello");
        assert_eq!(msgs[1].content, "hi there");
    }

    #[tokio::test]
    async fn test_factory_creates_correct_strategy() {
        let history = make_history();
        let s1 = create_strategy(MemoryStrategyKind::Window(5), history.clone(), None);
        assert_eq!(s1.load_context().await.len(), 0); // 空 history

        populate(&history, 2).await;
        let s2 = create_strategy(
            MemoryStrategyKind::Token(100),
            history.clone(),
            None,
        );
        assert!(!s2.load_context().await.is_empty());

        let s3 = create_strategy(
            MemoryStrategyKind::SummaryBuffer {
                max_messages: 2,
                retain_recent: 2,
            },
            history.clone(),
            None,
        );
        let ctx = s3.load_context().await;
        assert!(ctx.iter().any(|m| m.role == "system"));
    }
}
