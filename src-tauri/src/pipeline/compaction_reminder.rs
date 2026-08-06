//! 压缩后 Reminder 注入
//!
//! 当上下文压缩丢弃中段消息后，LLM 可能丢失对"之前在执行什么任务"的感知。
//! 本模块从被丢弃的消息中提取关键信息（活跃工具名、最后用户话题），
//! 生成一条简短的系统提醒消息，注入到对话中。
//!
//! 特点：
//! - 不调用 LLM，纯规则提取，零额外延迟
//! - 生成的提醒不超过 ~200 token
//! - 仅在确实丢弃了有意义的内容时才生成

use crate::types::response::ChatMessage;

/// 从消息列表中提取活跃工具名（去重、排序）
///
/// 扫描 assistant 消息的 tool_calls，收集所有工具名。
pub fn extract_active_tools(messages: &[ChatMessage]) -> Vec<String> {
    let mut tools: Vec<String> = messages
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flat_map(|tcs| tcs.iter().map(|tc| tc.name.clone()))
        .collect();
    tools.sort();
    tools.dedup();
    tools
}

/// 提取丢弃消息中最后一条 user 消息的内容摘要（截取前 100 字符）
pub fn extract_last_user_topic(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| {
            let content = m.content.trim();
            // 按字符（而非字节）截断：char_indices 返回的 byte_idx 保证在字符边界上，
            // 避免 &content[..100] 在多字节 UTF-8 字符（如中文/全角符号）中间切片导致 panic。
            match content.char_indices().nth(100) {
                Some((byte_idx, _)) => format!("{}…", &content[..byte_idx]),
                None => content.to_string(),
            }
        })
        .filter(|s| !s.is_empty())
}

/// 构建压缩后的 Reminder 系统消息
///
/// - `dropped_count`：被丢弃的消息/组数量
/// - `active_tools`：被丢弃消息中涉及的工具名
/// - `last_topic`：被丢弃消息中最后一条用户消息的内容摘要
///
/// 返回 `Some(ChatMessage)` 如果确实有需要提醒的内容，`None` 否则。
pub fn build_reminder(
    dropped_count: usize,
    active_tools: &[String],
    last_topic: Option<&str>,
) -> Option<ChatMessage> {
    if dropped_count == 0 {
        return None;
    }

    let mut parts = Vec::new();

    parts.push(format!(
        "此前对话已压缩（压缩了 {} 段历史）。",
        dropped_count
    ));

    if !active_tools.is_empty() {
        parts.push(format!("涉及工具：{}。", active_tools.join(", ")));
    }

    if let Some(topic) = last_topic {
        parts.push(format!("最后讨论话题：「{}」", topic));
    }

    if parts.is_empty() {
        return None;
    }

    let content = format!("[上下文提醒] {}", parts.join(" "));
    Some(ChatMessage::system(content))
}

/// 一体化辅助函数：从原始中段消息中提取 Reminder 所需信息
///
/// 在压缩前调用，保存中段消息的关键信息快照。
/// 压缩后调用 `build_reminder()` 生成提醒。
pub struct CompactionSnapshot {
    pub active_tools: Vec<String>,
    pub last_topic: Option<String>,
    pub message_count: usize,
}

impl CompactionSnapshot {
    /// 从消息列表的中段 [start..end) 创建快照
    pub fn from_mid_section(messages: &[ChatMessage], start: usize, end: usize) -> Self {
        let mid = if start < end && end <= messages.len() {
            &messages[start..end]
        } else {
            &[]
        };

        Self {
            active_tools: extract_active_tools(mid),
            last_topic: extract_last_user_topic(mid),
            message_count: mid.len(),
        }
    }

    /// 根据实际压缩的组数，生成 Reminder 消息
    pub fn build_reminder(&self, dropped_groups: usize) -> Option<ChatMessage> {
        build_reminder(
            dropped_groups,
            &self.active_tools,
            self.last_topic.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(content: &str) -> ChatMessage {
        ChatMessage::user(content)
    }

    fn assistant(content: &str) -> ChatMessage {
        ChatMessage::assistant(content)
    }

    #[test]
    fn extract_tools_from_tool_calls() {
        use crate::types::response::MessageToolCall;

        let msgs = vec![
            ChatMessage::assistant_with_tool_calls(
                String::new(),
                vec![
                    MessageToolCall {
                        id: "c1".to_string(),
                        name: "read_file".to_string(),
                        arguments: serde_json::json!({}),
                    },
                    MessageToolCall {
                        id: "c2".to_string(),
                        name: "web_search".to_string(),
                        arguments: serde_json::json!({}),
                    },
                ],
            ),
            ChatMessage::assistant_with_tool_calls(
                String::new(),
                vec![MessageToolCall {
                    id: "c3".to_string(),
                    name: "read_file".to_string(), // 重复
                    arguments: serde_json::json!({}),
                }],
            ),
        ];

        let tools = extract_active_tools(&msgs);
        assert_eq!(tools, vec!["read_file", "web_search"]); // 去重、排序
    }

    #[test]
    fn extract_last_user_topic_basic() {
        let msgs = vec![
            user("first question"),
            assistant("response"),
            user("second question about Rust"),
        ];
        let topic = extract_last_user_topic(&msgs);
        assert_eq!(topic, Some("second question about Rust".to_string()));
    }

    #[test]
    fn extract_last_user_topic_truncates_long() {
        let long = "a".repeat(200);
        let msgs = vec![user(&long)];
        let topic = extract_last_user_topic(&msgs);
        assert!(topic.is_some());
        assert!(topic.unwrap().len() <= 102); // 100 + "…"
    }

    #[test]
    fn extract_last_user_topic_truncates_multibyte() {
        // 回归测试：多字节 UTF-8 字符（中文/全角符号）不应在字节边界切片时 panic。
        // 旧实现 &content[..100] 在第 100 字节落在多字节字符中间时 panic。
        let long = "你好吗？".repeat(50); // 每字 3 字节，共 750 字节，150 字符
        let msgs = vec![user(&long)];
        let topic = extract_last_user_topic(&msgs).unwrap();
        assert!(topic.ends_with('…'));
        // 截断后应为 100 个字符 + "…"
        assert_eq!(topic.chars().count(), 101);
    }

    #[test]
    fn extract_last_user_topic_none_when_no_user() {
        let msgs = vec![assistant("only assistant")];
        let topic = extract_last_user_topic(&msgs);
        assert!(topic.is_none());
    }

    #[test]
    fn build_reminder_with_all_info() {
        let msg = build_reminder(
            5,
            &["read_file".to_string(), "web_search".to_string()],
            Some("如何学习 Rust"),
        );
        assert!(msg.is_some());
        let m = msg.unwrap();
        assert_eq!(m.role, "system");
        assert!(m.content.contains("5 段历史"));
        assert!(m.content.contains("read_file"));
        assert!(m.content.contains("如何学习 Rust"));
    }

    #[test]
    fn build_reminder_none_when_no_drops() {
        let msg = build_reminder(0, &[], None);
        assert!(msg.is_none());
    }

    #[test]
    fn build_reminder_minimal_info() {
        let msg = build_reminder(3, &[], None);
        assert!(msg.is_some());
        let m = msg.unwrap();
        assert!(m.content.contains("3 段历史"));
    }
}
