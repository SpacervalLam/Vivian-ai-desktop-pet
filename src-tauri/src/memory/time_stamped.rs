//! 带时间戳的记忆系统：支持时间感知的记忆管理、摘要与长期偏好。
//!
//! - 40 条摘要阈值
//! - 保留最近 8 条
//! - `summarize()`：LLM 窗口压缩（注入 ModelRouter 时启用，否则降级为拼接）
//! - `TimeStampedMessage` / `TimeStampedSummary` 模型
//! - 名字检测（"我叫 XX" 等模式）

use std::sync::Arc;

use chrono::{DateTime, Local};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tiktoken_rs::cl100k_base;

use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

/// 摘要触发的 token 阈值（30000 × 0.7 = 21000）
const SUMMARY_TOKEN_THRESHOLD: usize = 21000;
/// 摘要触发的消息数硬上限（防止 token 计数异常时无限增长）
const SUMMARY_MESSAGE_FALLBACK: usize = 150;
/// 消息数达标时的最低 token 门槛，短消息场景不急于压缩，保留更多原始上下文
const SUMMARY_MIN_TOKENS_FOR_LEN_TRIGGER: usize = 5250;
/// 摘要后保留的最近消息条数
const RETAIN_RECENT: usize = 8;

/// 全局 cl100k_base tokenizer 单例（线程安全，启动时一次性加载）
///
/// 加载失败时降级到 None，token_count 会回退到字符数估算（中文 1 字 ≈ 1.5 token）。
static TOKENIZER: Lazy<Option<tiktoken_rs::CoreBPE>> =
    Lazy::new(|| match cl100k_base() {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!(
                "[TimeStampedMemory] cl100k_base tokenizer 加载失败，降级到字符数估算: {}", e
            );
            None
        }
    });

/// 估算文本的 token 数（公共入口，供记忆上下文截断等场景复用）
///
/// tokenizer 不可用时回退到字符数估算（中文 1 字 ≈ 1.5 token，英文 4 字符 ≈ 1 token）。
pub fn estimate_tokens(text: &str) -> usize {
    if let Some(tok) = TOKENIZER.as_ref() {
        tok.encode_with_special_tokens(text).len()
    } else {
        text.chars()
            .map(|c| if c.is_ascii() { 0.25 } else { 1.5 })
            .sum::<f64>()
            .ceil() as usize
    }
}

/// 名字检测正则集合（12 个模式）。
/// 命中任意一个即视为含名字信息。
///
/// 注意：Rust `regex` crate 不支持 lookahead，因此省略
/// `(?![谁什么哪几])` 负向预查。`{2,}` 的最小长度要求已能过滤 "我是谁？" 这类
/// 单字符疑问句。
static NAME_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    // 中文姓名模式（2 个以上汉字）
    let cn = r"[\u4e00-\u9fa5]{2,}";
    // 英文姓名模式（2 个以上字母）
    let en = r"[A-Za-z]{2,}";
    let prefixes: &[&str] = &[
        "我是", "我叫", "我的名字是", "叫我", "名字是", "称呼我",
    ];
    let mut patterns = Vec::new();
    for p in prefixes {
        patterns.push(Regex::new(&format!("{p}{cn}")).expect("名字检测正则编译失败"));
        patterns.push(Regex::new(&format!("{p}{en}")).expect("名字检测正则编译失败"));
    }
    patterns
});

/// 带时间戳的消息模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeStampedMessage {
    pub content: String,
    /// 消息类型："human" 或 "ai"
    pub message_type: String,
    pub timestamp: DateTime<Local>,
    #[serde(default)]
    pub is_summarized: bool,
    #[serde(default = "default_importance")]
    pub importance: f64,
}

fn default_importance() -> f64 {
    0.5
}

impl TimeStampedMessage {
    pub fn new(content: impl Into<String>, message_type: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            message_type: message_type.into(),
            timestamp: Local::now(),
            is_summarized: false,
            importance: 0.5,
        }
    }

    pub fn with_importance(mut self, importance: f64) -> Self {
        self.importance = importance;
        self
    }
}

/// 带时间戳的摘要模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeStampedSummary {
    pub content: String,
    pub start_time: DateTime<Local>,
    pub end_time: DateTime<Local>,
}

/// 带时间戳的记忆系统。
pub struct TimeStampedMemory {
    messages: Vec<ChatMessage>,
    timestamps: Vec<DateTime<Local>>,
    summaries: Vec<TimeStampedSummary>,
    retain_recent: usize,
    last_interaction_time: Option<DateTime<Local>>,
}

impl TimeStampedMemory {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            timestamps: Vec::new(),
            summaries: Vec::new(),
            retain_recent: RETAIN_RECENT,
            last_interaction_time: None,
        }
    }

    /// 检测内容中是否包含名字信息（"我叫 XX" / "我是 XX" 等模式）。
    pub fn detect_name_in_content(content: &str) -> bool {
        NAME_PATTERNS.iter().any(|re| re.is_match(content))
    }

    pub fn add_message(&mut self, message: ChatMessage) {
        let ts = message.timestamp.unwrap_or_else(Local::now);
        self.timestamps.push(ts);
        self.last_interaction_time = Some(ts);
        self.messages.push(message);
    }

    pub fn get_messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn get_summaries(&self) -> &[TimeStampedSummary] {
        &self.summaries
    }

    pub fn should_summarize(&self) -> bool {
        let tokens = self.total_tokens();
        let len = self.messages.len();
        // 主阈值：token 数达标，任何时候都触发
        if tokens > SUMMARY_TOKEN_THRESHOLD {
            return true;
        }
        // 辅助上限：消息数过多且 token 达到最低门槛时触发
        // 短消息场景（token 很少）不急于压缩，避免频繁压缩导致精度丢失
        if len > SUMMARY_MESSAGE_FALLBACK && tokens > SUMMARY_MIN_TOKENS_FOR_LEN_TRIGGER {
            return true;
        }
        false
    }

    /// 计算单条消息的 token 数
    ///
    /// tokenizer 不可用时回退到字符数估算（中文 1 字 ≈ 1.5 token，英文 4 字符 ≈ 1 token）。
    fn token_count(msg: &ChatMessage) -> usize {
        estimate_tokens(&msg.content)
    }

    /// 当前所有消息的总 token 数
    pub fn total_tokens(&self) -> usize {
        self.messages.iter().map(Self::token_count).sum()
    }

    /// 触发摘要：保留最近 `retain_recent` 条，其余移除并返回待压缩消息。
    ///
    /// 注意：本方法只做"切割"，不调用 LLM。调用方拿到 `removed` 后，
    /// 在锁外调用 `compress_with_llm` 生成摘要，再调用 `commit_summary` 写回。
    /// 这样避免在持有 RwLock 期间 await LLM（导致 future 不是 Send）。
    pub fn summarize(&mut self) -> Vec<ChatMessage> {
        if !self.should_summarize() {
            return Vec::new();
        }
        let retain = self.retain_recent.min(self.messages.len());
        let split = self.messages.len() - retain;
        let removed: Vec<ChatMessage> = self.messages.drain(..split).collect();
        self.timestamps.drain(..split);
        removed
    }

    /// 把压缩后的摘要写入 summaries 列表（配合 `summarize` 使用）。
    pub fn commit_summary(&mut self, content: String, removed: &[ChatMessage]) {
        if removed.is_empty() {
            return;
        }
        let start_time = removed
            .first()
            .and_then(|m| m.timestamp)
            .unwrap_or_else(Local::now);
        let end_time = removed
            .last()
            .and_then(|m| m.timestamp)
            .unwrap_or_else(Local::now);
        self.summaries.push(TimeStampedSummary {
            content,
            start_time,
            end_time,
        });
    }

    /// LLM 窗口压缩：把待压缩消息喂给 LLM，输出自包含的语义摘要。
    ///
    /// LLM 看到完整上下文，会自然把"他"消解为"张三"，把碎片对话整理为连贯陈述。
    /// 失败时降级为拼接摘要。
    pub async fn compress_with_llm(
        router: &Arc<ModelRouter>,
        messages: &[ChatMessage],
    ) -> String {
        let conversation = messages
            .iter()
            .map(|m| {
                // 优先从 meta 中识别 speaker；ChatMessage 暂无 speaker 字段，按 role 兜底
                // 不再硬编码 "Vivian"，避免 Nana 等多角色场景下错误标注
                // 使用第一人称标签，与项目记忆存储前缀格式对齐
                let speaker_tag = if m.role == "user" {
                    "[User says to me]"
                } else {
                    "[I say to User]"
                };
                // 时间戳前缀：从 m.timestamp（Option<DateTime<Local>>）格式化为 [HH:MM]
                let time = if let Some(ts) = m.timestamp {
                    format!("[{}] ", ts.format("%H:%M"))
                } else {
                    String::new()
                };
                format!("{}{} {}", time, speaker_tag, m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        let prompt = match lang_norm {
            "en" => format!(
                "Compress the following conversation into a concise summary (within 150 words), preserving key facts, entity names mentioned by the user, emotions, and important events.\n\
                 Requirements:\n\
                 1. Third-person narration, naturally resolving pronouns (e.g., replace 'he' with the specific person's name)\n\
                 2. Preserve temporal order (what happened first, what happened later)\n\
                 3. Do not add explanations or comments; output only the summary content\n\n\
                 Conversation:\n{conversation}\n\n\
                 Summary:"
            ),
            "ja" => format!(
                "以下の会話を簡潔な要約に圧縮してください（150文字以内）。重要な事実、ユーザーが言及したエンティティ名、感情、重要なイベントを保持すること。\n\
                 要件：\n\
                 1. 三人称で述べ、代名詞を自然に解消する（例：「彼」を具体的な名前に置き換える）\n\
                 2. 時系列を保持する（先に何が起きたか、後に何が起きたか）\n\
                 3. 説明やコメントを追加せず、要約内容のみを出力する\n\n\
                 会話内容:\n{conversation}\n\n\
                 要約:"
            ),
            _ => format!(
                "请将以下对话压缩为一段简洁的摘要（150字以内），保留关键事实、用户提到的实体名、情绪和重要事件。\n\
                 要求：\n\
                 1. 第三人称陈述，自然消解代词（如'他'替换为具体人名）\n\
                 2. 保留时序（先发生了什么，后发生了什么）\n\
                 3. 不要添加解释或评论，只输出摘要内容\n\n\
                 对话内容:\n{conversation}\n\n\
                 摘要:"
            ),
        };

        match router
            .generate(LLMRequest::new(
                "memory",
                vec![ChatMessage::user(&prompt)],
            ))
            .await
        {
            Ok(text) => {
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() {
                    return trimmed;
                }
                Self::fallback_summary(messages)
            }
            Err(e) => {
                tracing::warn!("[TimeStampedMemory] LLM 摘要失败，降级为拼接: {}", e);
                Self::fallback_summary(messages)
            }
        }
    }

    /// 降级摘要：LLM 不可用时按 "role: content" 拼接，截断到 200 字符。
    fn fallback_summary(messages: &[ChatMessage]) -> String {
        let parts: Vec<String> = messages
            .iter()
            .map(|m| {
                let role = if m.role == "user" { "User" } else { "AI" };
                format!("{}: {}", role, m.content)
            })
            .collect();
        let joined = parts.join("\n");
        let chars: Vec<char> = joined.chars().collect();
        if chars.len() > 200 {
            format!(
                "Conversation Summary: {}...",
                chars.into_iter().take(200).collect::<String>()
            )
        } else {
            format!("Conversation Summary: {}", joined)
        }
    }

    /// 获取最近一条摘要内容（无则空字符串）。
    pub fn recent_summary(&self) -> String {
        self.summaries
            .last()
            .map(|s| s.content.clone())
            .unwrap_or_default()
    }

    /// 获取最近交互时间。
    pub fn last_interaction_time(&self) -> Option<DateTime<Local>> {
        self.last_interaction_time
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.timestamps.clear();
        self.summaries.clear();
        self.last_interaction_time = None;
    }
}

impl Default for TimeStampedMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_name_chinese() {
        assert!(TimeStampedMemory::detect_name_in_content("我叫张三"));
        assert!(TimeStampedMemory::detect_name_in_content("我是李四"));
        assert!(TimeStampedMemory::detect_name_in_content("我的名字是王五"));
        assert!(TimeStampedMemory::detect_name_in_content("叫我赵六"));
        // 不应匹配疑问句
        assert!(!TimeStampedMemory::detect_name_in_content("我是谁？"));
        assert!(!TimeStampedMemory::detect_name_in_content("你叫什么"));
    }

    #[test]
    fn test_detect_name_english() {
        assert!(TimeStampedMemory::detect_name_in_content("My name is Vivian"));
        assert!(TimeStampedMemory::detect_name_in_content("我叫 Alice"));
    }

    #[test]
    fn test_should_summarize_by_token() {
        let mut mem = TimeStampedMemory::new();
        // 单条消息塞大量文本，让 token 数超过 21000 阈值
        let big = "这是一段很长的文本用于测试 token 阈值触发摘要。".repeat(500);
        mem.add_message(ChatMessage::user(big));
        assert!(mem.total_tokens() > SUMMARY_TOKEN_THRESHOLD);
        assert!(mem.should_summarize());
    }

    #[test]
    fn test_should_summarize_by_message_fallback() {
        let mut mem = TimeStampedMemory::new();
        // 80 条短消息（token 不会超阈值），触发消息数辅助上限
        for i in 0..SUMMARY_MESSAGE_FALLBACK {
            mem.add_message(ChatMessage::user(format!("msg {}", i)));
        }
        assert!(mem.should_summarize());
    }

    #[test]
    fn test_should_not_summarize_small() {
        let mut mem = TimeStampedMemory::new();
        for i in 0..10 {
            mem.add_message(ChatMessage::user(format!("msg {}", i)));
        }
        assert!(!mem.should_summarize());
    }

    #[test]
    fn test_summarize_keeps_recent() {
        let mut mem = TimeStampedMemory::new();
        // 用大消息触发摘要（避免依赖消息数 fallback）
        let big = "x".repeat(100000);
        mem.add_message(ChatMessage::user(big));
        for i in 0..10 {
            mem.add_message(ChatMessage::user(format!("msg {}", i)));
        }
        let removed = mem.summarize();
        // 11 - 8 = 3 条被移除
        assert_eq!(removed.len(), 3);
        // 保留最近 8 条
        assert_eq!(mem.get_messages().len(), 8);
        // 摘要列表此时为空（未调用 commit_summary）
        assert_eq!(mem.get_summaries().len(), 0);
        // 手动 commit 摘要
        mem.commit_summary("test summary".to_string(), &removed);
        assert_eq!(mem.get_summaries().len(), 1);
        assert_eq!(mem.recent_summary(), "test summary");
    }
}
