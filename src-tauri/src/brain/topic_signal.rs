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

    /// 基于用户输入与上一轮 topic 标签的关键词重叠检测话题切换。
    ///
    /// 重叠率低于 1/3 视为话题切换；首轮或无基线时返回 false。
    pub fn detect_topic_change(&self, char_id: &str, user_input: &str) -> bool {
        let inner = self.inner.lock();
        let Some(buffer) = inner.get(char_id) else {
            return false;
        };
        if buffer.last_topics.is_empty() {
            return false;
        }
        let input_lower = user_input.to_lowercase();
        let overlap = buffer
            .last_topics
            .iter()
            .filter(|t| {
                let t_lower = t.to_lowercase();
                t_lower.len() >= 2 && input_lower.contains(&t_lower)
            })
            .count();
        overlap * 3 < buffer.last_topics.len()
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
