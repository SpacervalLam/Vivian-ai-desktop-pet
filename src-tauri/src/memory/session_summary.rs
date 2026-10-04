//! 可恢复的分段计划：每段从原始消息生成，覆盖范围由程序决定。
use super::conversations::ConversationRecord;
use serde::{Deserialize, Serialize};

const CHUNK_CHARS: usize = 12_000;
const SLICE_CHARS: usize = 4_000;

/// 稳定、文件名安全的摘要 ID（可用于可选明文镜像），会话关联仍存于 metadata。
pub fn summary_id(character: &str, conversation: &str) -> String {
    use sha2::{Digest, Sha256};
    let input = serde_json::to_vec(&[character, conversation]).unwrap();
    let digest = Sha256::digest(input);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("mem_session_{hex}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSlice {
    pub message_id: String,
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone)]
pub struct SummaryChunk {
    pub fingerprint: String,
    pub slices: Vec<SourceSlice>,
    pub transcript: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryPart {
    pub fingerprint: String,
    pub slices: Vec<SourceSlice>,
    pub title: String,
    pub summary: Option<String>,
    pub topics: Vec<String>,
    pub importance: f64,
    pub events: Vec<SharedEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventPhase {
    Planned,
    Started,
    Progressed,
    Completed,
    Cancelled,
}

/// Event links are selected from an explicit catalogue, never inferred from topic overlap.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventDraft {
    pub existing_event_id: Option<String>,
    pub title: String,
    pub phase: EventPhase,
    pub detail: String,
    pub source_message_id: String,
    pub source_quote: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedEvent {
    pub id: String,
    pub title: String,
    pub phase: EventPhase,
    pub detail: String,
    pub source_message_id: String,
    pub source_quote: String,
    /// Time of the original report, not an invented occurrence date.
    pub recorded_at: f64,
}

pub fn validate_events(
    drafts: Vec<EventDraft>,
    session: &ConversationRecord,
    chunk: &SummaryChunk,
    known: &[SharedEvent],
) -> Result<Vec<SharedEvent>, String> {
    if drafts.len() > 8 {
        return Err("单次事件进展数量超过上限".into());
    }
    let mut events = Vec::new();
    for draft in drafts {
        if !draft.confidence.is_finite() || !(0.0..=1.0).contains(&draft.confidence) {
            return Err("事件置信度无效".into());
        }
        if draft.confidence < 0.85 {
            continue;
        }
        let turn = session
            .turns
            .iter()
            .find(|t| t.id == draft.source_message_id && t.speaker == "user")
            .ok_or("事件必须引用本会话中用户的原话")?;
        let quote = draft.source_quote.trim();
        if quote.chars().count() < 4
            || quote.chars().count() > 500
            || !chunk
                .slices
                .iter()
                .filter(|s| s.message_id == turn.id)
                .any(|s| {
                    let slice: String = turn
                        .text
                        .chars()
                        .skip(s.start)
                        .take(s.end - s.start)
                        .collect();
                    slice.contains(quote)
                })
        {
            return Err("事件证据不在当前原文片段中".into());
        }
        if draft.title.trim().is_empty()
            || draft.title.chars().count() > 80
            || draft.detail.trim().is_empty()
            || draft.detail.chars().count() > 500
        {
            return Err("事件标题或进展内容无效".into());
        }
        let (id, title) = match draft.existing_event_id {
            Some(id) => {
                let event = known
                    .iter()
                    .find(|e| e.id == id)
                    .ok_or("引用了不存在的事件")?;
                (id, event.title.clone())
            }
            None => (
                summary_id("event", &format!("{}:{}", turn.id, draft.title.trim())),
                draft.title.trim().to_string(),
            ),
        };
        if events
            .iter()
            .any(|e: &SharedEvent| e.id == id && e.source_message_id == turn.id)
        {
            continue;
        }
        events.push(SharedEvent {
            id,
            title,
            phase: draft.phase,
            detail: draft.detail.trim().into(),
            source_message_id: turn.id.clone(),
            source_quote: quote.into(),
            recorded_at: turn.timestamp,
        });
    }
    Ok(events)
}

/// 固定 hash 算法用于持久化指纹，不能依赖随机 hash seed 或内容相似度。
fn fingerprint(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn plan_chunks(session: &ConversationRecord) -> Vec<SummaryChunk> {
    let mut chunks = Vec::new();
    let mut transcript = String::new();
    let mut slices = Vec::new();
    let finish =
        |chunks: &mut Vec<SummaryChunk>, transcript: &mut String, slices: &mut Vec<SourceSlice>| {
            if slices.is_empty() {
                return;
            }
            let signature = format!(
                "{}:{}",
                serde_json::to_string(slices).unwrap_or_default(),
                transcript
            );
            chunks.push(SummaryChunk {
                fingerprint: fingerprint(&signature),
                slices: std::mem::take(slices),
                transcript: std::mem::take(transcript),
            });
        };
    for turn in &session.turns {
        let chars: Vec<_> = turn.text.chars().collect();
        if let Some(sticker) = &turn.sticker {
            let line = format!(
                "[{} | {} → {} | {}] 发送贴纸：{}\n",
                turn.id, turn.speaker, turn.listener, turn.timestamp, sticker
            );
            if !slices.is_empty() && transcript.chars().count() + line.chars().count() > CHUNK_CHARS
            {
                finish(&mut chunks, &mut transcript, &mut slices);
            }
            transcript.push_str(&line);
            if chars.is_empty() {
                slices.push(SourceSlice {
                    message_id: turn.id.clone(),
                    start: 0,
                    end: 0,
                });
            }
        }
        for start in (0..chars.len()).step_by(SLICE_CHARS) {
            let end = (start + SLICE_CHARS).min(chars.len());
            let body: String = chars[start..end].iter().collect();
            let line = format!(
                "[{} | {} → {} | {} | 字符 {}..{}]\n{}\n",
                turn.id, turn.speaker, turn.listener, turn.timestamp, start, end, body
            );
            if !slices.is_empty() && transcript.chars().count() + line.chars().count() > CHUNK_CHARS
            {
                finish(&mut chunks, &mut transcript, &mut slices);
            }
            transcript.push_str(&line);
            slices.push(SourceSlice {
                message_id: turn.id.clone(),
                start,
                end,
            });
        }
    }
    finish(&mut chunks, &mut transcript, &mut slices);
    chunks
}

/// 仅复用内容及来源范围都未变化的分段，不把旧概要作为下一次输入。
pub fn reusable_parts(chunks: &[SummaryChunk], previous: &[SummaryPart]) -> Vec<SummaryPart> {
    chunks
        .iter()
        .filter_map(|chunk| {
            previous
                .iter()
                .find(|part| part.fingerprint == chunk.fingerprint && part.slices == chunk.slices)
                .cloned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::conversations::ConversationTurn;
    use super::*;
    fn event_draft(quote: &str, existing: Option<String>) -> EventDraft {
        EventDraft {
            existing_event_id: existing,
            title: "一起完成桌宠项目".into(),
            phase: EventPhase::Planned,
            detail: "约好周末一起完成桌宠项目".into(),
            source_message_id: "source".into(),
            source_quote: quote.into(),
            confidence: 0.95,
        }
    }
    #[test]
    fn event_progress_links_across_exchanges_only_with_original_user_evidence() {
        let first = session("周末我们一起完成桌宠项目吧".into());
        let chunk = &plan_chunks(&first)[0];
        let events = validate_events(
            vec![event_draft("一起完成桌宠项目", None)],
            &first,
            chunk,
            &[],
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].phase, EventPhase::Planned));
        let mut next = session("桌宠项目完成了，谢谢你陪我".into());
        next.id = "second-exchange".into();
        next.turns[0].id = "next-source".into();
        let mut draft = event_draft("桌宠项目完成了", Some(events[0].id.clone()));
        draft.source_message_id = "next-source".into();
        draft.phase = EventPhase::Completed;
        draft.detail = "用户确认桌宠项目完成".into();
        let result = validate_events(vec![draft], &next, &plan_chunks(&next)[0], &events).unwrap();
        assert_eq!(result[0].id, events[0].id);
        assert!(matches!(result[0].phase, EventPhase::Completed));
        assert!(validate_events(
            vec![event_draft("用户从未说过这句话", None)],
            &first,
            chunk,
            &[]
        )
        .is_err());
        assert!(validate_events(
            vec![event_draft("一起完成桌宠项目", Some("invented".into()))],
            &first,
            chunk,
            &[]
        )
        .is_err());
        let mut guessed = first.clone();
        guessed.turns[0].speaker = "nana".into();
        assert!(validate_events(
            vec![event_draft("一起完成桌宠项目", None)],
            &guessed,
            chunk,
            &[]
        )
        .is_err());
        let mut uncertain = event_draft("一起完成桌宠项目", None);
        uncertain.confidence = 0.7;
        assert!(validate_events(vec![uncertain], &first, chunk, &[])
            .unwrap()
            .is_empty());
        let other = validate_events(
            vec![event_draft("一起完成桌宠项目", None)],
            &first,
            chunk,
            &[],
        )
        .unwrap();
        assert_eq!(events[0].id, other[0].id);
    }
    #[test]
    fn stable_summary_ids_are_distinct_and_safe_as_windows_filenames() {
        let id = summary_id("vivian", "conversation:a,b:c");
        assert_eq!(id, summary_id("vivian", "conversation:a,b:c"));
        assert_ne!(id, summary_id("nana", "conversation:a,b:c"));
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }
    fn session(text: String) -> ConversationRecord {
        ConversationRecord {
            id: "one".into(),
            source_session_id: None,
            title: "一次对话".into(),
            participants: vec!["user".into(), "nana".into()],
            channels: vec!["direct".into()],
            started_at: 1.0,
            ended_at: 2.0,
            turns: vec![ConversationTurn {
                id: "source".into(),
                speaker: "user".into(),
                listener: "nana".into(),
                text,
                timestamp: 1.0,
                channel: "direct".into(),
                sticker: None,
            }],
        }
    }
    #[test]
    fn long_unicode_messages_are_fully_covered_without_truncation() {
        let session = session("中文🙂".repeat(6000));
        let chunks = plan_chunks(&session);
        assert!(chunks.len() > 1);
        let slices: Vec<_> = chunks.iter().flat_map(|c| &c.slices).collect();
        assert_eq!(slices.first().unwrap().start, 0);
        assert_eq!(
            slices.last().unwrap().end,
            session.turns[0].text.chars().count()
        );
        for pair in slices.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        assert!(chunks
            .iter()
            .all(|c| c.transcript.chars().count() <= CHUNK_CHARS));
    }
    #[test]
    fn edited_original_invalidates_old_checkpoint() {
        let mut session = session("明天见".into());
        let chunks = plan_chunks(&session);
        let old = SummaryPart {
            fingerprint: chunks[0].fingerprint.clone(),
            slices: chunks[0].slices.clone(),
            title: "约定".into(),
            summary: Some("明天再聊".into()),
            topics: vec![],
            importance: 0.7,
            events: vec![],
        };
        assert_eq!(reusable_parts(&chunks, &[old.clone()]).len(), 1);
        session.turns[0].text = "后天见".into();
        assert!(reusable_parts(&plan_chunks(&session), &[old]).is_empty());
    }
}
