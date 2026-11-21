use std::collections::HashMap;
use std::time::Instant;

use parking_lot::Mutex;

const TOPIC_STABILITY_THRESHOLD: usize = 3;
const TOPIC_FLUSH_INTERVAL_SECS: u64 = 300;

struct TopicBufferInner {
    last_topics: Vec<String>,
    stable_count: usize,
    flushed_topics: Vec<String>,
    last_flush_at: Instant,
}

impl Default for TopicBufferInner {
    fn default() -> Self {
        Self {
            last_topics: Vec::new(),
            stable_count: 0,
            flushed_topics: Vec::new(),
            last_flush_at: Instant::now(),
        }
    }
}

pub struct TopicSignalBuffer {
    inner: Mutex<HashMap<String, TopicBufferInner>>,
}

impl TopicSignalBuffer {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// 记录本轮的 topic 信号（pipeline 后处理调用）。
    pub fn record_topics(&self, char_id: &str, topics: Vec<String>) {
        let mut inner = self.inner.lock();
        let buffer = inner
            .entry(char_id.to_string())
            .or_insert_with(TopicBufferInner::default);

        let same = topics.len() == buffer.last_topics.len()
            && topics
                .iter()
                .zip(buffer.last_topics.iter())
                .all(|(a, b)| a == b);

        if same {
            buffer.stable_count += 1;
        } else {
            buffer.stable_count = 1;
            buffer.last_topics = topics;
        }
    }

    /// 检查是否应该将当前稳定 topic 持久化到记忆。
    ///
    /// 返回 Some(topics) 表示需要写入；写入后内部标记为已刷新。
    pub fn should_flush(&self, char_id: &str) -> Option<Vec<String>> {
        let mut inner = self.inner.lock();
        let buffer = inner.get_mut(char_id)?;

        let stabilized = buffer.stable_count >= TOPIC_STABILITY_THRESHOLD
            && buffer.last_topics != buffer.flushed_topics;
        let timed = buffer.last_flush_at.elapsed().as_secs() >= TOPIC_FLUSH_INTERVAL_SECS
            && !buffer.last_topics.is_empty()
            && buffer.last_topics != buffer.flushed_topics;

        if stabilized || timed {
            buffer.flushed_topics = buffer.last_topics.clone();
            buffer.last_flush_at = Instant::now();
            Some(buffer.last_topics.clone())
        } else {
            None
        }
    }
}

impl Default for TopicSignalBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::types::response::ChatMessage;

    fn bounded_text(text: &str) -> String {
        let chars: Vec<_> = text.chars().collect();
        if chars.len() <= 400 {
            return text.into();
        }
        format!(
            "{}…{}",
            chars[..260].iter().collect::<String>(),
            chars[chars.len() - 139..].iter().collect::<String>()
        )
    }

    fn topic_context(
        history: &[ChatMessage],
        input: &str,
    ) -> serde_json::Value {
        let previous: Vec<_> = history
            .iter()
            .rev()
            .filter(|message| {
                matches!(message.role.as_str(), "user" | "assistant")
                    && !message.content.trim().is_empty()
                    && message.tool_calls.is_none()
                    && !message.meta.as_ref().is_some_and(|m| m.is_memory_disabled)
                    && !message
                        .meta
                        .as_ref()
                        .and_then(|m| m.kind.as_deref())
                        .is_some_and(|kind| {
                            matches!(
                                kind,
                                "internal_directive" | "response_status" | "tool_result"
                            )
                        })
            })
            .take(6)
            .map(|m| serde_json::json!({"role":m.role,"text":bounded_text(&m.content)}))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        serde_json::json!({"previous_messages":previous,"latest_message":bounded_text(input)})
    }

    #[test]
    fn judgment_uses_bounded_original_speech_in_order() {
        let mut hidden = ChatMessage::user("程序内部通知");
        hidden.meta = Some(crate::messages::MessageMeta {
            kind: Some("internal_directive".into()),
            ..Default::default()
        });
        let context = topic_context(
            &[
                ChatMessage::system("角色人设"),
                ChatMessage::assistant("你希望约在周几？"),
                hidden,
                ChatMessage::user("让我想想"),
                ChatMessage::tool_result("工具结果", "call"),
            ],
            "周五",
        );
        assert_eq!(context["previous_messages"].as_array().unwrap().len(), 2);
        assert_eq!(context["previous_messages"][0]["text"], "你希望约在周几？");
        assert_eq!(context["latest_message"], "周五");
        let text = format!("{}句末决定", "前言".repeat(1000));
        assert_eq!(bounded_text(&text).chars().count(), 400);
        assert!(bounded_text(&text).ends_with("句末决定"));
    }
}
