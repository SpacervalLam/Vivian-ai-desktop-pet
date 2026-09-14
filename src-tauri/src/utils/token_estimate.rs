//! Token 估算模块
//!
//! 提供快速 token 估算（`bytes / 4`），用于上下文压缩阈值判定、Prompt 预算检查和对话历史管理。

use crate::types::response::ChatMessage;

/// 每 token 的近似字节数
const BYTES_PER_TOKEN: usize = 4;

/// 每条消息的格式开销（role + 分隔符等）
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

/// 图片固定 token 估算
pub const IMAGE_TOKEN_ESTIMATE: usize = 765;

/// 快速 token 估算：bytes / 4（向上取整）
///
/// 适用于 GPT-4、Claude、DeepSeek 等主流模型的分词近似。
/// 对中文内容会略低估（中文 1 token ≈ 3-6 bytes，bytes/4 偏向低估端）。
///
/// # 示例
/// ```
/// assert_eq!(estimate_tokens("hello"), 2);     // 5 bytes → 2 tokens
/// assert_eq!(estimate_tokens("你好"), 2);       // 6 bytes → 2 tokens
/// assert_eq!(estimate_tokens(""), 0);           // 空字符串
/// ```
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    (text.len() + BYTES_PER_TOKEN - 1) / BYTES_PER_TOKEN
}

/// 估算单条消息的 token 数
///
/// 包含：
/// - content 文本的 token
/// - reasoning（思维链）的 token（如有）
/// - tool_calls 的 token（名称 + 序列化参数）
/// - 每条消息的格式开销
pub fn estimate_message_tokens(msg: &ChatMessage) -> usize {
    let mut tokens = estimate_tokens(&msg.content) + MESSAGE_OVERHEAD_TOKENS;

    // reasoning 内容（思维链）
    if let Some(reasoning) = &msg.reasoning {
        tokens += estimate_tokens(reasoning);
    }

    // tool_calls（assistant 消息）
    if let Some(tool_calls) = &msg.tool_calls {
        for tc in tool_calls {
            tokens += estimate_tokens(&tc.name);
            let args_str =
                serde_json::to_string(&tc.arguments).unwrap_or_default();
            tokens += estimate_tokens(&args_str);
            tokens += 2; // tool_call_id + 结构开销
        }
    }

    tokens
}

/// 估算整组消息的总 token 数
pub fn estimate_messages_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// 估算工具定义的 token 开销
///
/// 工具定义（name + description + parameters schema）在每次 API 调用中
/// 都会计入 token 用量。
pub fn estimate_tool_definitions_tokens(
    tools: &[(String, String, String)], // (name, description, params_json)
) -> usize {
    tools
        .iter()
        .map(|(name, desc, params)| {
            estimate_tokens(name) + estimate_tokens(desc) + estimate_tokens(params) + 6
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::response::MessageToolCall;

    #[test]
    fn empty_string() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn ascii_text() {
        // "hello" = 5 bytes → ceil(5/4) = 2
        assert_eq!(estimate_tokens("hello"), 2);
        // "hello world" = 11 bytes → ceil(11/4) = 3
        assert_eq!(estimate_tokens("hello world"), 3);
    }

    #[test]
    fn chinese_text() {
        // "你好" = 6 bytes (UTF-8) → ceil(6/4) = 2
        assert_eq!(estimate_tokens("你好"), 2);
    }

    #[test]
    fn long_text() {
        let text = "a".repeat(4000);
        assert_eq!(estimate_tokens(&text), 1000);
    }

    #[test]
    fn message_tokens_basic() {
        let msg = ChatMessage::user("hello");
        let tokens = estimate_message_tokens(&msg);
        // "hello" = 2 tokens + 4 overhead = 6
        assert_eq!(tokens, 6);
    }

    #[test]
    fn message_tokens_with_tool_calls() {
        let msg = ChatMessage::assistant_with_tool_calls(
            String::new(),
            vec![MessageToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/tmp/test.txt"}),
            }],
        );
        let tokens = estimate_message_tokens(&msg);
        // 0 (empty content) + 4 (overhead) + tokens for tool call
        assert!(tokens > 4);
    }

    #[test]
    fn message_tokens_with_reasoning() {
        let msg = ChatMessage {
            role: "assistant".to_string(),
            content: "answer".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning: Some("thinking about this carefully".to_string()),
            images: None,
            meta: None,
            timestamp: None,
        };
        let tokens_with = estimate_message_tokens(&msg);
        let msg_no_reasoning = ChatMessage {
            reasoning: None,
            ..msg.clone()
        };
        let tokens_without = estimate_message_tokens(&msg_no_reasoning);
        assert!(tokens_with > tokens_without);
    }

    #[test]
    fn conversation_tokens() {
        let msgs = vec![
            ChatMessage::user("hi"),
            ChatMessage::user("how are you"),
        ];
        let total = estimate_messages_tokens(&msgs);
        let sum: usize = msgs.iter().map(estimate_message_tokens).sum();
        assert_eq!(total, sum);
    }

    #[test]
    fn tool_definitions_tokens() {
        let tools = vec![
            ("read_file".to_string(), "Read a file".to_string(), "{}".to_string()),
        ];
        let tokens = estimate_tool_definitions_tokens(&tools);
        assert!(tokens > 0);
    }
}
