//! Pure boundary rules shared by the chat-only request path and contract tests.
use serde_json::{json, Value};

pub async fn acquire_turn(
    lock: &tokio::sync::Mutex<()>,
    wait: std::time::Duration,
) -> Result<tokio::sync::MutexGuard<'_, ()>, String> {
    tokio::time::timeout(wait, lock.lock())
        .await
        .map_err(|_| "聊天排队超时".into())
}

/// Cancellation must work while a provider is connecting, before its first event.
pub async fn wait_for_model<T>(
    future: impl std::future::Future<Output = T>,
    cancelled: impl Fn() -> bool,
    timeout: std::time::Duration,
) -> Result<Option<T>, String> {
    let pending = tokio::time::timeout(timeout, future);
    tokio::pin!(pending);
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(50));
    loop {
        if cancelled() {
            return Ok(None);
        }
        tokio::select! {
            _ = poll.tick() => {},
            result = &mut pending => return result.map(Some).map_err(|_| "聊天模型连接超时".into()),
        }
    }
}

pub fn validate_origin(window: &str, channel: &str) -> Result<(), String> {
    if window != "chat" || (channel != "wechat" && !crate::sns_store::is_group(channel)) {
        return Err("聊天专属角色仅支持聊天窗口内的私聊和群聊".into());
    }
    Ok(())
}

pub fn validate_channel(channel: &str) -> Result<(), String> {
    if channel == "wechat" || crate::sns_store::is_group(channel) {
        Ok(())
    } else {
        Err("聊天专属角色不支持此消息渠道".into())
    }
}

/// Keep display metadata, never accept a caller's channel/source/speaker override.
pub fn input_metadata(input: Option<&Value>, channel: &str) -> Result<Value, String> {
    validate_channel(channel)?;
    let mut result = json!({"source":"chat_partner", "channel":channel});
    if let Some(input) = input {
        let keys: &[&str] = match input.get("kind").and_then(Value::as_str) {
            Some("file") => &[
                "kind",
                "file_name",
                "file_type",
                "file_path",
                "file_size",
                "truncated",
                "original_char_count",
            ],
            Some("voice") => &["kind", "audio_path", "duration"],
            Some("image") => &["kind", "image_path", "visual_description"],
            _ => {
                return Err(
                    "聊天专属角色只接收文字、文件文字和语音转写，不支持此附件类型".into(),
                )
            }
        };
        for key in keys {
            if let Some(value) = input.get(*key) {
                result[*key] = value.clone();
            }
        }
    }
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct ContextLine {
    pub source_id: String,
    pub speaker: String,
    pub role: String,
    pub content: String,
    pub timestamp: f64,
    pub channel: Option<String>,
}

/// Defense in depth: filter again even when the storage query was scoped.
pub fn scoped_context(
    mut lines: Vec<ContextLine>,
    channel: &str,
    limit: usize,
) -> Vec<ContextLine> {
    lines.retain(|line| {
        matches!(line.role.as_str(), "user" | "assistant")
            && (line.channel.as_deref() == Some(channel)
                || (channel == "wechat" && line.channel.is_none()))
    });
    lines.sort_by(|a, b| {
        a.timestamp
            .total_cmp(&b.timestamp)
            .then(a.speaker.cmp(&b.speaker))
    });
    // Group broadcasts are stored once in each recipient's history. Collapse only
    // copies from different recipients, preserving repeated messages to one recipient.
    let mut unique: Vec<ContextLine> = Vec::new();
    for line in lines {
        if crate::sns_store::is_group(channel)
            && line.role == "user"
            && unique.iter().rev().any(|previous| {
                previous.role == "user"
                    && previous.speaker != line.speaker
                    && previous.content == line.content
                    && (previous.timestamp - line.timestamp).abs() < 1.0
            })
        {
            continue;
        }
        unique.push(line);
    }
    unique.drain(..unique.len().saturating_sub(limit));
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn silent_model_can_be_cancelled_or_timed_out() {
        use std::{
            sync::{
                atomic::{AtomicBool, Ordering},
                Arc,
            },
            time::Duration,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let outcome = tokio::time::timeout(
            Duration::from_millis(500),
            wait_for_model(
                std::future::pending::<()>(),
                || cancelled.load(Ordering::SeqCst),
                Duration::from_secs(10),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(outcome, None);
        assert!(wait_for_model(
            std::future::pending::<()>(),
            || false,
            Duration::from_millis(10)
        )
        .await
        .is_err());
        assert_eq!(
            wait_for_model(async { 42 }, || false, Duration::from_secs(1))
                .await
                .unwrap(),
            Some(42)
        );
    }
    fn line(
        speaker: &str,
        role: &str,
        content: &str,
        channel: Option<&str>,
        timestamp: f64,
    ) -> ContextLine {
        ContextLine {
            source_id: format!("{speaker}:{timestamp}"),
            speaker: speaker.into(),
            role: role.into(),
            content: content.into(),
            channel: channel.map(str::to_owned),
            timestamp,
        }
    }
    #[test]
    fn only_chat_window_and_two_chat_channels_are_allowed() {
        for channel in ["wechat", "wechat_group"] {
            assert!(validate_origin("chat", channel).is_ok());
        }
        for window in ["vivian", "main", "deepseek", "remote"] {
            assert!(validate_origin(window, "wechat").is_err());
        }
        for channel in ["direct", "broadcast", "proactive", "", "wechat_exact"] {
            assert!(validate_origin("chat", channel).is_err());
        }
    }
    #[test]
    fn private_and_group_context_cannot_cross() {
        let lines = vec![
            line("fish", "user", "私密", Some("wechat"), 1.0),
            line("fish", "user", "旧私聊", None, 2.0),
            line("vivian", "assistant", "群聊", Some("wechat_group"), 3.0),
            line("fish", "system", "内部", Some("wechat_group"), 4.0),
        ];
        let private = scoped_context(lines.clone(), "wechat", 16);
        assert_eq!(
            private
                .iter()
                .map(|x| x.content.as_str())
                .collect::<Vec<_>>(),
            ["私密", "旧私聊"]
        );
        let group = scoped_context(lines, "wechat_group", 16);
        assert_eq!(group.len(), 1);
        assert_eq!(group[0].content, "群聊");
    }
    #[test]
    fn group_keeps_speakers_and_repeated_user_turns_but_collapses_broadcast_copies() {
        let result = scoped_context(
            vec![
                line("fish", "user", "你好", Some("wechat_group"), 1.0),
                line("vivian", "user", "你好", Some("wechat_group"), 1.1),
                line("fish", "user", "你好", Some("wechat_group"), 1.2),
                line("nana", "assistant", "喵", Some("wechat_group"), 2.0),
            ],
            "wechat_group",
            3,
        );
        assert_eq!(result.len(), 3);
        assert_eq!(result[2].speaker, "nana");
        assert_eq!(scoped_context(result, "wechat_group", 1)[0].content, "喵");
    }
    #[test]
    fn media_metadata_survives_without_overriding_boundaries() {
        let meta = input_metadata(Some(&json!({"kind":"voice","audio_path":"a.webm","duration":3,"channel":"direct","source":"system","speaker":"admin"})),"wechat").unwrap();
        assert_eq!(meta["audio_path"], "a.webm");
        assert_eq!(meta["channel"], "wechat");
        assert_eq!(meta["source"], "chat_partner");
        assert!(meta.get("speaker").is_none());
        assert_eq!(
            input_metadata(
                Some(&json!({"kind":"file","file_name":"a.txt","file_path":"a.txt"})),
                "wechat"
            )
            .unwrap()["file_name"],
            "a.txt"
        );
        assert!(input_metadata(Some(&json!({"kind":"image","visual_description":"a whale"})), "wechat").is_ok());
    }

    #[tokio::test]
    async fn concurrent_channels_wait_for_the_entire_previous_turn() {
        use std::{sync::Arc, time::Duration};
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let entries = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let private = acquire_turn(&gate, Duration::from_secs(1)).await.unwrap();
        entries.lock().await.push("private user");
        let next_gate = gate.clone();
        let next_entries = entries.clone();
        let next = tokio::spawn(async move {
            let _group = acquire_turn(&next_gate, Duration::from_secs(1))
                .await
                .unwrap();
            next_entries
                .lock()
                .await
                .extend(["group user", "group assistant"]);
        });
        assert!(acquire_turn(&gate, Duration::from_millis(10))
            .await
            .is_err());
        assert_eq!(*entries.lock().await, ["private user"]);
        entries.lock().await.push("private assistant");
        drop(private);
        next.await.unwrap();
        assert_eq!(
            *entries.lock().await,
            [
                "private user",
                "private assistant",
                "group user",
                "group assistant"
            ]
        );
        // A timed-out waiter has not poisoned the queue.
        assert!(acquire_turn(&gate, Duration::from_millis(10)).await.is_ok());
    }
}
