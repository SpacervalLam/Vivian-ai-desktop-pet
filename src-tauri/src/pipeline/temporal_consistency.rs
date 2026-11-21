//! Post-generation temporal wording guard.
//!
//! Prompt-side anchors are the primary fix. This module adds a narrow, conservative
//! safeguard for unsolicited proactive speech, where there is no current user turn
//! that "刚才" could legitimately refer to: if the latest real utterance is older
//! than five minutes, replace recency adverbs outside quoted text.

use crate::types::response::ChatMessage;

pub const JUST_NOW_WINDOW_SECS: f64 = 300.0;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemporalRewrite {
    pub replacements: usize,
    /// Age of the latest user/assistant utterance used for the decision.
    pub reference_age_secs: Option<u64>,
}

/// Soften `刚才/刚刚/just now` when the referenced conversation is already old.
///
/// Quoted material is preserved verbatim. Returns the original text when no
/// safe rewrite applies.
pub fn soften_relative_wording(
    text: &str,
    history: &[ChatMessage],
    now_secs: f64,
) -> (String, TemporalRewrite) {
    if text.trim().is_empty() || !now_secs.is_finite() {
        return (text.to_owned(), TemporalRewrite::default());
    }
    let latest = history.iter()
        .filter(|message| matches!(message.role.as_str(), "user" | "assistant"))
        .filter_map(|message| message.timestamp.map(|ts| ts.timestamp() as f64))
        .filter(|ts| ts.is_finite() && *ts > 0.0)
        .fold(f64::NEG_INFINITY, f64::max);
    if !latest.is_finite() {
        return (text.to_owned(), TemporalRewrite::default());
    }
    let age = (now_secs - latest).max(0.0);
    let audit = TemporalRewrite {
        replacements: 0,
        reference_age_secs: Some(age.round().max(0.0) as u64),
    };
    if age <= JUST_NOW_WINDOW_SECS {
        return (text.to_owned(), audit);
    }
    let replacements: &[(&str, &str)] = &[
        ("刚才", "之前"),
        ("刚刚", "之前"),
        ("just now", "earlier"),
        ("Just now", "Earlier"),
        ("a moment ago", "earlier"),
        ("さっき", "以前"),
    ];
    let (rewritten, count) = replace_outside_quotes(text, replacements);
    (rewritten, TemporalRewrite {
        replacements: count,
        reference_age_secs: audit.reference_age_secs,
    })
}

fn replace_outside_quotes(text: &str, replacements: &[(&str, &str)]) -> (String, usize) {
    let mut output = String::with_capacity(text.len());
    let mut outside = String::new();
    let mut close_quote: Option<char> = None;
    let mut count = 0usize;
    for ch in text.chars() {
        if let Some(close) = close_quote {
            output.push(ch);
            if ch == close {
                close_quote = None;
            }
            continue;
        }
        if matches!(ch, '"' | '“' | '「' | '『') {
            if !outside.is_empty() {
                let (rewritten, replaced) = replace_in_segment(&outside, replacements);
                output.push_str(&rewritten);
                count += replaced;
                outside.clear();
            }
            output.push(ch);
            close_quote = Some(match ch {
                '“' => '”',
                '「' => '」',
                '『' => '』',
                _ => '"',
            });
            continue;
        }
        outside.push(ch);
    }
    if !outside.is_empty() {
        let (rewritten, replaced) = replace_in_segment(&outside, replacements);
        output.push_str(&rewritten);
        count += replaced;
    }
    (output, count)
}

fn replace_in_segment(segment: &str, replacements: &[(&str, &str)]) -> (String, usize) {
    let mut result = segment.to_owned();
    let mut count = 0usize;
    for (from, to) in replacements {
        count += result.matches(from).count();
        result = result.replace(from, to);
    }
    (result, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn history_at(now: f64, age_secs: i64) -> Vec<ChatMessage> {
        let ts = chrono::Local.timestamp_opt((now as i64) - age_secs, 0).single().unwrap();
        let mut message = ChatMessage::user("old utterance");
        message.timestamp = Some(ts);
        vec![message]
    }

    #[test]
    fn old_conversation_cannot_be_called_just_now() {
        let now = Local::now().timestamp() as f64;
        let (text, audit) = soften_relative_wording("你刚才说的那个", &history_at(now, 600), now);
        assert_eq!(text, "你之前说的那个");
        assert_eq!(audit.replacements, 1);
        assert_eq!(audit.reference_age_secs, Some(600));
    }

    #[test]
    fn fresh_conversation_and_quotes_are_preserved() {
        let now = Local::now().timestamp() as f64;
        let (fresh, _) = soften_relative_wording("你刚才说的那个", &history_at(now, 60), now);
        assert_eq!(fresh, "你刚才说的那个");
        let (quoted, audit) = soften_relative_wording(
            "你说「刚才」的那句话", &history_at(now, 600), now);
        assert_eq!(quoted, "你说「刚才」的那句话");
        assert_eq!(audit.replacements, 0);
    }

    #[test]
    fn english_and_japanese_replacements_are_localized() {
        let now = Local::now().timestamp() as f64;
        let history = history_at(now, 3600);
        let (english, _) = soften_relative_wording("I said that just now.", &history, now);
        assert_eq!(english, "I said that earlier.");
        let (japanese, _) = soften_relative_wording("さっき言ったよ。", &history, now);
        assert_eq!(japanese, "以前言ったよ。");
    }
}
