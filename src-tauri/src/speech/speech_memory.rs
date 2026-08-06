//! Speech Memory — 言语记忆
//!
//! 记录智能体最近说过的文本,支持:
//! - 查询某文本是否在指定时间窗口内说过(避免短时间重复)
//! - 统计高频短语(n-gram),识别口头禅
//! - 提供最近 N 条发言记录(供 Brain 参考)
//!
//! 设计原则:轻量、无锁竞争(单角色单实例,RwLock 足够)。
//! 数据不持久化,进程重启后清空(语音记忆是短期的,长期记忆归 MemoryManager)。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

/// 单条言语记录
#[derive(Debug, Clone)]
struct UtteranceRecord {
    /// 记录时刻
    at: Instant,
    /// 文本(已 trim)
    text: String,
}

/// 言语记忆 — 记录最近说过的内容
///
/// 每个 TtsManager 持有一个实例,角色间独立。
/// 保留最近 100 条记录(约 5-10 分钟对话量),超出自动淘汰。
pub struct SpeechMemory {
    records: Arc<RwLock<VecDeque<UtteranceRecord>>>,
    /// 最大保留条数
    max_records: usize,
}

impl SpeechMemory {
    pub fn new() -> Self {
        Self {
            records: Arc::new(RwLock::new(VecDeque::with_capacity(128))),
            max_records: 100,
        }
    }

    /// 记录一条发言
    ///
    /// 自动 trim 并跳过空文本。超出 max_records 时淘汰最旧的。
    pub fn record(&self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut records = self.records.write();
        records.push_back(UtteranceRecord {
            at: Instant::now(),
            text: trimmed.to_string(),
        });
        while records.len() > self.max_records {
            records.pop_front();
        }
    }

    /// 查询某文本是否在指定时间窗口内说过(精确匹配)
    ///
    /// `within` 为时间窗口,如 Duration::from_secs(300) 表示最近 5 分钟。
    /// 返回 true 表示最近说过,Brain 应避免重复。
    pub fn recently_spoken(&self, text: &str, within: Duration) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return false;
        }
        let now = Instant::now();
        let records = self.records.read();
        records
            .iter()
            .rev()
            .take(20) // 只检查最近 20 条,避免全量扫描
            .any(|r| now.duration_since(r.at) <= within && r.text == trimmed)
    }

    /// 查询最近 N 条发言文本(按时间倒序,最新的在前)
    pub fn recent_texts(&self, n: usize) -> Vec<String> {
        let records = self.records.read();
        records
            .iter()
            .rev()
            .take(n)
            .map(|r| r.text.clone())
            .collect()
    }

    /// 统计高频短语(2-6 字的 n-gram)
    ///
    /// 返回 (短语, 出现次数) 列表,按次数降序。
    /// 用于识别口头禅——出现频率异常高的短语。
    ///
    /// 注意:中文 n-gram 按字符切分,不依赖分词,结果粗略但足够识别明显口头禅。
    pub fn frequent_phrases(&self, top_n: usize) -> Vec<(String, usize)> {
        use std::collections::HashMap;
        let records = self.records.read();
        let mut counts: HashMap<String, usize> = HashMap::new();

        for r in records.iter() {
            let chars: Vec<char> = r.text.chars().collect();
            if chars.len() < 2 {
                continue;
            }
            // 提取 2-6 字的滑动窗口 n-gram
            for n in 2..=6.min(chars.len()) {
                for window in chars.windows(n) {
                    let phrase: String = window.iter().collect();
                    // 跳过含标点/空格的片段
                    if phrase.chars().any(|c| c.is_whitespace() || is_punctuation(c)) {
                        continue;
                    }
                    *counts.entry(phrase).or_insert(0) += 1;
                }
            }
        }

        let mut sorted: Vec<(String, usize)> = counts.into_iter().filter(|(_, c)| *c >= 3).collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.len().cmp(&b.0.len())));
        sorted.truncate(top_n);
        sorted
    }

    /// 清空记录
    pub fn clear(&self) {
        self.records.write().clear();
    }
}

impl Default for SpeechMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// 判断字符是否为标点(中英文)
fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '。' | '，' | '、' | '；' | '：' | '？' | '！' | '「' | '」' | '『' | '』' | '（' | '）' |
        '《' | '》' | '…' | '—' | '.' | ',' | ';' | ':' | '?' | '!' | '"' | '\'' | '(' | ')' | '[' | ']'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_recent() {
        let mem = SpeechMemory::new();
        mem.record("你好呀");
        mem.record("今天天气不错");
        assert!(mem.recently_spoken("你好呀", Duration::from_secs(60)));
        assert!(!mem.recently_spoken("没说过的话", Duration::from_secs(60)));
    }

    #[test]
    fn test_recent_texts() {
        let mem = SpeechMemory::new();
        mem.record("第一句");
        mem.record("第二句");
        let recent = mem.recent_texts(5);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0], "第二句"); // 最新的在前
    }

    #[test]
    fn test_frequent_phrases() {
        let mem = SpeechMemory::new();
        for _ in 0..5 {
            mem.record("那个那个我觉得吧");
        }
        let phrases = mem.frequent_phrases(10);
        // "那个" 应该出现至少 5 次
        assert!(phrases.iter().any(|(p, c)| p == "那个" && *c >= 5));
    }

    #[test]
    fn test_max_records() {
        let mem = SpeechMemory::new();
        for i in 0..150 {
            mem.record(&format!("文本{}", i));
        }
        let records = mem.records.read();
        assert_eq!(records.len(), 100); // 不超过 max_records
    }
}
