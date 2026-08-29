//! 多级对话存档：压缩后的对话经历伪常驻于上下文。
//!
//! 设计（多级 cache 思想）：
//! - L1：对话窗口溢出切割出的消息段，经 LLM 压缩为一条存档摘要
//! - L(n) 满 4 条时，最旧 3 条合并压缩为一条 L(n+1)，指数收敛
//! - 存档持久化为 JSONL 索引 + 明文 txt 镜像（人可读可编辑）
//! - 每轮对话把全部存档渲染为 `[CONVERSATION ARCHIVE]` 块注入历史头部，
//!   使"之前聊过什么"天然在场，不依赖记忆检索命中
//!
//! 正常对话轮次零 LLM 调用；压缩仅在阈值触发时发生（摊销成本）。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

/// 触发同级合并的条数阈值（达到即合并最旧 MERGE_COUNT 条）
const MERGE_THRESHOLD: usize = 4;
/// 每次合并消耗的同级条数
const MERGE_COUNT: usize = 3;
/// 最高压缩层级（L3 覆盖约 27 个原始消息段）
const MAX_LEVEL: u8 = 3;
/// 注入上下文的存档摘要条数上限（超出时保留最新 N-1 条 L1 + 全部高层）
const INJECT_MAX_ENTRIES: usize = 8;

/// 单条存档
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub id: String,
    /// 压缩层级（1 = 对话段直接压缩，2+ = 低层级存档再压缩）
    pub level: u8,
    pub summary: String,
    pub start_time: DateTime<Local>,
    pub end_time: DateTime<Local>,
    /// 创建时刻（Unix 秒）
    pub created_at: f64,
}

/// 多级对话存档
pub struct ConversationArchive {
    /// 全部存档条目（按 start_time 升序维持）
    entries: Vec<ArchiveEntry>,
    /// JSONL 索引路径
    index_path: PathBuf,
    /// 明文镜像目录
    plain_dir: PathBuf,
}

impl ConversationArchive {
    /// 按角色初始化：`characters/<char_id>/memory/conversation_archive.jsonl`
    pub fn new(char_id: &str) -> Self {
        let memory_dir = crate::utils::path::get_character_data_dir(char_id).join("memory");
        let _ = std::fs::create_dir_all(&memory_dir);
        let index_path = memory_dir.join("conversation_archive.jsonl");
        let plain_dir = memory_dir.join("archive_plain");
        let _ = std::fs::create_dir_all(&plain_dir);
        let mut archive = Self {
            entries: Vec::new(),
            index_path,
            plain_dir,
        };
        archive.load();
        archive
    }

    fn load(&mut self) {
        if let Ok(content) = std::fs::read_to_string(&self.index_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(e) = serde_json::from_str::<ArchiveEntry>(line) {
                    self.entries.push(e);
                }
            }
        }
        self.entries.sort_by(|a, b| a.start_time.cmp(&b.start_time));
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 新增一条 L1 存档（对话窗口溢出压缩产物），追加持久化
    pub fn add_l1(&mut self, summary: String, start: DateTime<Local>, end: DateTime<Local>) {
        self.insert_entry(1, summary, start, end);
    }

    /// 插入条目并持久化（追加 JSONL 行 + 明文镜像）
    ///
    /// 摘要源自原始对话的 LLM 压缩，可能携带凭据/个人标识；
    /// 存档每轮注入 prompt 且有明文镜像，落盘前统一脱敏。
    fn insert_entry(&mut self, level: u8, summary: String, start: DateTime<Local>, end: DateTime<Local>) {
        let (safe_summary, _, status) = crate::memory::redact::redact_content(&summary);
        if status == crate::memory::redact::RedactStatus::Redacted {
            tracing::info!("[ConversationArchive] 存档摘要含敏感信息，已脱敏（level={level}）");
        }
        let summary = safe_summary;
        let id = format!(
            "L{level}-{}-{}",
            start.format("%Y%m%d%H%M%S"),
            end.format("%Y%m%d%H%M%S")
        );
        let entry = ArchiveEntry {
            id: id.clone(),
            level,
            summary,
            start_time: start,
            end_time: end,
            created_at: crate::memory::types::current_timestamp(),
        };
        // 明文镜像（写一次永不再碰）
        let text = format!(
            "压缩级别：{}\n时间范围：{} 到 {}\n\n{}",
            entry.level,
            entry.start_time.format("%Y-%m-%d %H:%M"),
            entry.end_time.format("%Y-%m-%d %H:%M"),
            entry.summary,
        );
        let _ = std::fs::write(self.plain_dir.join(format!("{id}.txt")), text);
        // JSONL 追加
        if let Ok(line) = serde_json::to_string(&entry) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.index_path)
            {
                let _ = writeln!(f, "{line}");
            }
        }
        // 按时间插入保持有序
        let pos = self
            .entries
            .partition_point(|e| e.start_time <= entry.start_time);
        self.entries.insert(pos, entry);
    }

    /// 检查是否存在待合并的层级：某层条数 ≥ MERGE_THRESHOLD 且低于最高层
    /// 返回该层最旧 MERGE_COUNT 条（合并候选）
    pub fn pending_merge(&self) -> Option<Vec<ArchiveEntry>> {
        for level in 1..MAX_LEVEL {
            let group: Vec<&ArchiveEntry> =
                self.entries.iter().filter(|e| e.level == level).collect();
            if group.len() >= MERGE_THRESHOLD {
                return Some(group[..MERGE_COUNT].iter().map(|e| (*e).clone()).collect());
            }
        }
        None
    }

    /// 提交合并：移除被合并条目，插入高一层的合并产物；整文件重写（低频）
    pub fn commit_merge(&mut self, consumed: &[ArchiveEntry], merged_summary: String) {
        let consumed_ids: std::collections::HashSet<&str> =
            consumed.iter().map(|e| e.id.as_str()).collect();
        self.entries.retain(|e| !consumed_ids.contains(e.id.as_str()));
        // 移除对应明文镜像
        for e in consumed {
            let _ = std::fs::remove_file(self.plain_dir.join(format!("{}.txt", e.id)));
        }
        let start = consumed.first().map(|e| e.start_time).unwrap_or_else(Local::now);
        let end = consumed.last().map(|e| e.end_time).unwrap_or_else(Local::now);
        let new_level = consumed.first().map(|e| e.level + 1).unwrap_or(2);
        self.insert_entry(new_level, merged_summary, start, end);
        // 整体重写索引（合并是低频事件，条目数为个位数到十位数）
        self.rewrite_index();
    }

    fn rewrite_index(&mut self) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.index_path)
        {
            for e in &self.entries {
                if let Ok(line) = serde_json::to_string(e) {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
    }

    /// 渲染注入上下文的存档块（旧→新，高层级在前）
    pub fn render_block(&self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        // 数量超限时优先保留：全部 L2+（时间更早）+ 最新若干条 L1
        let selected: Vec<&ArchiveEntry> = if self.entries.len() > INJECT_MAX_ENTRIES {
            let high: Vec<&ArchiveEntry> = self
                .entries
                .iter()
                .filter(|e| e.level > 1)
                .collect();
            let l1s: Vec<&ArchiveEntry> =
                self.entries.iter().filter(|e| e.level == 1).collect();
            let keep_l1 = INJECT_MAX_ENTRIES.saturating_sub(high.len());
            let mut v = high;
            let start = l1s.len().saturating_sub(keep_l1);
            v.extend_from_slice(&l1s[start..]);
            v
        } else {
            self.entries.iter().collect()
        };

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        let mut block = match lang_norm {
            "en" => "[CONVERSATION ARCHIVE]\nSummaries of your earlier conversations with the user, oldest first:\n".to_string(),
            "ja" => "[CONVERSATION ARCHIVE]\n以前のユーザーとの会話の要約（古い順）:\n".to_string(),
            _ => "[CONVERSATION ARCHIVE]\n以下是你与用户更早对话的摘要存档（从旧到新）:\n".to_string(),
        };
        for e in selected {
            block.push_str(&format!(
                "\n[记忆存档 {} {} ~ {}]\n{}\n",
                e.level,
                e.start_time.format("%Y-%m-%d %H:%M"),
                e.end_time.format("%Y-%m-%d %H:%M"),
                e.summary.trim()
            ));
        }
        block.push_str("\n[END ARCHIVE]");
        Some(block)
    }

    /// 把存档块注入消息列表头部（无存档时不动）
    pub fn inject_into(&self, messages: &mut Vec<ChatMessage>) {
        if let Some(block) = self.render_block() {
            messages.insert(0, ChatMessage::system(block));
        }
    }

    /// 清空全部存档（记忆清空时联动）
    pub fn clear(&mut self) {
        self.entries.clear();
        let _ = std::fs::remove_file(&self.index_path);
        if self.plain_dir.exists() {
            if let Ok(files) = std::fs::read_dir(&self.plain_dir) {
                for f in files.filter_map(|e| e.ok()) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
    }
}

/// LLM 合并低层存档为高层存档：输入为若干条存档摘要，输出一条自包含摘要
pub async fn compress_merge_with_llm(
    router: &Arc<ModelRouter>,
    group: &[ArchiveEntry],
) -> String {
    let input = group
        .iter()
        .map(|e| {
            format!(
                "（{} 至 {}）{}",
                e.start_time.format("%Y-%m-%d"),
                e.end_time.format("%Y-%m-%d"),
                e.summary.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    let prompt = match lang_norm {
        "en" => format!(
            "Merge the following conversation summaries into one concise summary (within 200 words), keeping key facts, names, emotions, and the chronological order of events.\nOutput only the merged summary.\n\nSummaries:\n{input}\n\nMerged summary:"
        ),
        "ja" => format!(
            "以下の要約を1つの簡潔な要約に統合してください（200文字以内）。重要な事実、名前、感情、出来事の時系列を保持すること。\n統合後の要約のみを出力すること。\n\n要約:\n{input}\n\n統合要約:"
        ),
        _ => format!(
            "请将以下几段对话摘要合并为一段简洁的总结（200字以内），保留关键事实、人物名字、情绪和事件的先后顺序。\n只输出合并后的总结内容。\n\n摘要:\n{input}\n\n合并总结:"
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
                trimmed
            } else {
                fallback_merge(group)
            }
        }
        Err(e) => {
            tracing::warn!("[ConversationArchive] LLM 合并失败，降级为拼接: {}", e);
            fallback_merge(group)
        }
    }
}

/// 降级合并：直接拼接各段摘要
fn fallback_merge(group: &[ArchiveEntry]) -> String {
    group
        .iter()
        .map(|e| e.summary.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: u8, start_min: i64, summary: &str) -> ArchiveEntry {
        let base = Local::now();
        ArchiveEntry {
            id: format!("L{level}-{start_min}"),
            level,
            summary: summary.to_string(),
            start_time: base - chrono::Duration::minutes(start_min),
            end_time: base - chrono::Duration::minutes(start_min - 10),
            created_at: 0.0,
        }
    }

    #[test]
    fn test_pending_merge_threshold() {
        let mut archive = ConversationArchive {
            entries: vec![
                entry(1, 100, "a"),
                entry(1, 90, "b"),
                entry(1, 80, "c"),
                entry(1, 70, "d"),
            ],
            index_path: PathBuf::from("unused.jsonl"),
            plain_dir: PathBuf::from("unused_dir"),
        };
        // 4 条 L1 → 触发合并，取最旧 3 条
        let pending = archive.pending_merge().unwrap();
        assert_eq!(pending.len(), 3);
        assert_eq!(pending[0].summary, "a");
        // 3 条不触发
        archive.entries.pop();
        assert!(archive.pending_merge().is_none());
    }

    #[test]
    fn test_render_block_order() {
        let archive = ConversationArchive {
            entries: vec![
                entry(2, 200, "高层存档"),
                entry(1, 100, "近期存档"),
            ],
            index_path: PathBuf::from("unused.jsonl"),
            plain_dir: PathBuf::from("unused_dir"),
        };
        let block = archive.render_block().unwrap();
        let high_pos = block.find("高层存档").unwrap();
        let low_pos = block.find("近期存档").unwrap();
        assert!(high_pos < low_pos, "高层级（更早）应排在前面");
        assert!(block.contains("[CONVERSATION ARCHIVE]"));
    }

    #[test]
    fn test_empty_render() {
        let archive = ConversationArchive {
            entries: vec![],
            index_path: PathBuf::from("unused.jsonl"),
            plain_dir: PathBuf::from("unused_dir"),
        };
        assert!(archive.render_block().is_none());
        assert!(archive.is_empty());
    }
}
