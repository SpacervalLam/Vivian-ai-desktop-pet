//! Durable receipt and processing are separate: a busy brain can still receive.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, Weak},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    User,
    PeerRequest,
    PeerReply,
    WorkResult,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    Queued,
    Processing,
    Completed,
    Interrupted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxMessage {
    pub id: String,
    pub sender: String,
    pub recipient: String,
    pub channel: String,
    pub thread_id: String,
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub concern_id: Option<String>,
    pub kind: MessageKind,
    pub text: String,
    pub received_at: i64,
    pub state: MessageState,
    pub attempts: u8,
    pub not_before: i64,
    pub error: Option<String>,
}
impl InboxMessage {
    pub fn new(
        id: String,
        sender: String,
        recipient: String,
        channel: String,
        thread_id: String,
        kind: MessageKind,
        text: String,
        now: i64,
    ) -> Self {
        Self {
            id,
            sender,
            recipient,
            channel,
            thread_id,
            kind,
            text,
            received_at: now,
            in_reply_to: None,
            concern_id: None,
            state: MessageState::Queued,
            attempts: 0,
            not_before: now,
            error: None,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Snapshot {
    messages: Vec<InboxMessage>,
}
pub struct Inbox {
    path: PathBuf,
    state: Mutex<Snapshot>,
}
impl Inbox {
    /// Brain rebuilds share the same file owner while old requests are settling.
    /// Weak entries do not retain a character after shutdown.
    pub fn shared(path: PathBuf) -> Arc<Self> {
        static OWNERS: OnceLock<Mutex<HashMap<PathBuf, Weak<Inbox>>>> = OnceLock::new();
        let mut owners = OWNERS.get_or_init(Default::default).lock();
        owners.retain(|_, owner| owner.strong_count() != 0);
        if let Some(existing) = owners.get(&path).and_then(Weak::upgrade) {
            return existing;
        }
        let inbox = Arc::new(Self::load(path.clone()));
        owners.insert(path, Arc::downgrade(&inbox));
        inbox
    }
    pub fn load(path: PathBuf) -> Self {
        let mut state: Snapshot = crate::utils::fs::load_json_or_backup(&path).unwrap_or_default();
        // There is no caller left for a queued user IPC after restart. Retain the
        // input and expose interruption; never silently replay tools or old requests.
        for message in &mut state.messages {
            if message.state == MessageState::Processing
                || (message.kind == MessageKind::User && message.state == MessageState::Queued)
            {
                message.state = MessageState::Interrupted;
                message.error =
                    Some("host restarted before completion; explicit retry required".into());
            }
        }
        Self {
            path,
            state: Mutex::new(state),
        }
    }
    fn update<T>(
        &self,
        edit: impl FnOnce(&mut Snapshot) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.state.lock();
        let mut next = state.clone();
        let result = edit(&mut next)?;
        if next == *state {
            return Ok(result);
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        crate::utils::fs::write_atomic(
            &self.path,
            &serde_json::to_string(&next).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        *state = next;
        Ok(result)
    }
    /// Idempotent by identity and envelope: a retry cannot change an accepted input.
    pub fn receive(&self, message: InboxMessage) -> Result<bool, String> {
        if message.id.is_empty() || message.recipient.is_empty() || message.text.trim().is_empty() {
            return Err("无效的收件消息".into());
        }
        self.update(|state| {
            if let Some(old) = state.messages.iter().find(|old| old.id == message.id) {
                if old.sender != message.sender
                    || old.recipient != message.recipient
                    || old.channel != message.channel
                    || old.thread_id != message.thread_id
                    || old.kind != message.kind
                    || old.text != message.text
                    || old.in_reply_to != message.in_reply_to
                    || old.concern_id != message.concern_id
                {
                    return Err("消息 ID 已用于另一份内容".into());
                }
                return Ok(false);
            }
            if state
                .messages
                .iter()
                .filter(|message| {
                    matches!(
                        message.state,
                        MessageState::Queued | MessageState::Processing
                    )
                })
                .count()
                >= 64
            {
                return Err("收件箱待处理消息已满，请稍后重试".into());
            }
            // Keep pending and interrupted inputs; compact only completed receipts.
            while state.messages.len() >= 256 {
                let Some(index) = state
                    .messages
                    .iter()
                    .position(|message| message.state == MessageState::Completed)
                else {
                    return Err("收件箱保留记录已满，需要处理未完成消息".into());
                };
                state.messages.remove(index);
            }
            state.messages.push(message);
            Ok(true)
        })
    }
    pub fn get(&self, id: &str) -> Option<InboxMessage> {
        self.state
            .lock()
            .messages
            .iter()
            .find(|message| message.id == id)
            .cloned()
    }
    pub fn pending(&self, kind: MessageKind) -> Vec<InboxMessage> {
        self.state
            .lock()
            .messages
            .iter()
            .filter(|message| message.kind == kind && message.state == MessageState::Queued)
            .cloned()
            .collect()
    }
    pub fn claim(&self, id: &str, now: i64) -> Result<bool, String> {
        self.update(|state| {
            let Some(message) = state.messages.iter_mut().find(|message| message.id == id) else {
                return Ok(false);
            };
            if message.state != MessageState::Queued || message.not_before > now {
                return Ok(false);
            }
            message.state = MessageState::Processing;
            message.attempts = message.attempts.saturating_add(1);
            Ok(true)
        })
    }
    pub fn settle(
        &self,
        id: &str,
        outcome: MessageState,
        error: Option<String>,
    ) -> Result<(), String> {
        self.update(|state| {
            if let Some(message) = state.messages.iter_mut().find(|message| message.id == id) {
                if message.state == MessageState::Completed {
                    return Ok(());
                }
                message.state = outcome;
                message.error = error;
            }
            Ok(())
        })
    }
    pub fn defer(&self, id: &str, now: i64, retry: bool, error: &str) -> Result<(), String> {
        self.update(|state| {
            if let Some(message) = state
                .messages
                .iter_mut()
                .find(|message| message.id == id && message.state == MessageState::Processing)
            {
                if !retry {
                    message.attempts = message.attempts.saturating_sub(1);
                }
                message.state = if message.attempts >= 3 {
                    MessageState::Interrupted
                } else {
                    MessageState::Queued
                };
                message.not_before = now.saturating_add(if retry {
                    30 * i64::from(message.attempts)
                } else {
                    5
                });
                message.error = Some(error.into());
            }
            Ok(())
        })
    }
    pub fn clear(&self) -> Result<(), String> {
        self.update(|state| {
            state.messages.clear();
            Ok(())
        })
    }
    pub fn retry_interrupted_peer(&self, id: &str, now: i64) -> Result<bool, String> {
        self.update(|state| {
            let Some(message) = state.messages.iter_mut().find(|message| {
                message.id == id
                    && message.kind == MessageKind::PeerRequest
                    && message.state == MessageState::Interrupted
            }) else {
                return Ok(false);
            };
            message.state = MessageState::Queued;
            message.not_before = now;
            message.attempts = 0;
            message.error = None;
            Ok(true)
        })
    }
    pub fn interrupt_peer_processing(&self) -> Result<(), String> {
        self.update(|state| {
            for message in &mut state.messages {
                if message.kind == MessageKind::PeerRequest
                    && message.state == MessageState::Processing
                {
                    message.state = MessageState::Interrupted;
                    message.error = Some("peer worker restarted; explicit retry required".into());
                }
            }
            Ok(())
        })
    }
    pub fn status(&self) -> serde_json::Value {
        let state = self.state.lock();
        serde_json::json!({"queued":state.messages.iter().filter(|m| m.state == MessageState::Queued).count(),
            "processing":state.messages.iter().filter(|m| m.state == MessageState::Processing).count(),
            "interrupted":state.messages.iter().filter(|m| m.state == MessageState::Interrupted).count(),
            "recent":state.messages.iter().rev().take(12).map(|m| serde_json::json!({
                "id":m.id,"sender":m.sender,"recipient":m.recipient,"channel":m.channel,"thread_id":m.thread_id,
                "in_reply_to":m.in_reply_to,"kind":m.kind,"state":m.state,"received_at":m.received_at,"attempts":m.attempts,"error":m.error,
            })).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rebuilds_share_one_writer_and_clearing_cannot_be_undone_by_an_old_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inbox.json");
        let first = Inbox::shared(path.clone());
        let rebuilt = Inbox::shared(path.clone());
        assert!(Arc::ptr_eq(&first, &rebuilt));
        first
            .receive(message("old", MessageKind::PeerRequest))
            .unwrap();
        rebuilt.clear().unwrap();
        first.settle("old", MessageState::Completed, None).unwrap();
        assert!(Inbox::load(path).get("old").is_none());
    }
    #[test]
    fn only_an_explicit_retry_rearms_interrupted_peer_processing() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::load(dir.path().join("inbox.json"));
        inbox
            .receive(message("peer", MessageKind::PeerRequest))
            .unwrap();
        inbox.claim("peer", 100).unwrap();
        inbox.interrupt_peer_processing().unwrap();
        assert!(!inbox.claim("peer", 200).unwrap());
        assert!(inbox.retry_interrupted_peer("peer", 200).unwrap());
        assert!(inbox.claim("peer", 200).unwrap());
        inbox.receive(message("user", MessageKind::User)).unwrap();
        inbox
            .settle("user", MessageState::Interrupted, None)
            .unwrap();
        assert!(!inbox.retry_interrupted_peer("user", 200).unwrap());
        inbox.settle("peer", MessageState::Completed, None).unwrap();
        assert!(!inbox.retry_interrupted_peer("peer", 300).unwrap());
    }
    fn message(id: &str, kind: MessageKind) -> InboxMessage {
        InboxMessage::new(
            id.into(),
            "nana".into(),
            "vivian".into(),
            "cross_character".into(),
            "peer:vivian:nana".into(),
            kind,
            "有一个真实的问题".into(),
            100,
        )
    }
    #[test]
    fn receiving_while_another_lane_is_processing_is_durable_and_does_not_change_its_route() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inbox.json");
        let inbox = Inbox::load(path.clone());
        inbox.receive(message("user", MessageKind::User)).unwrap();
        assert!(inbox.claim("user", 100).unwrap());
        inbox
            .receive(message("peer", MessageKind::PeerRequest))
            .unwrap();
        inbox
            .receive(message("work", MessageKind::WorkResult))
            .unwrap();
        let restored = Inbox::load(path);
        assert_eq!(
            restored.get("user").unwrap().state,
            MessageState::Interrupted
        );
        assert_eq!(restored.get("peer").unwrap().state, MessageState::Queued);
        assert_eq!(restored.get("work").unwrap().state, MessageState::Queued);
        assert_eq!(restored.get("peer").unwrap().thread_id, "peer:vivian:nana");
    }
    #[test]
    fn receipts_are_idempotent_and_duplicate_ids_cannot_replace_message_contents() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::load(dir.path().join("inbox.json"));
        assert!(inbox
            .receive(message("a", MessageKind::PeerRequest))
            .unwrap());
        assert!(!inbox
            .receive(message("a", MessageKind::PeerRequest))
            .unwrap());
        let mut changed = message("a", MessageKind::PeerRequest);
        changed.recipient = "nana".into();
        assert!(inbox.receive(changed).is_err());
        assert!(inbox.claim("a", 100).unwrap());
        assert!(!inbox.claim("a", 101).unwrap());
        inbox.settle("a", MessageState::Completed, None).unwrap();
        inbox.settle("a", MessageState::Interrupted, None).unwrap();
        assert_eq!(inbox.get("a").unwrap().state, MessageState::Completed);
    }
    #[test]
    fn save_failure_does_not_acknowledge_or_claim_and_interrupted_execution_is_not_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "file").unwrap();
        let inbox = Inbox::load(blocked.join("inbox.json"));
        assert!(inbox
            .receive(message("a", MessageKind::PeerRequest))
            .is_err());
        assert!(inbox.get("a").is_none());
        let path = dir.path().join("inbox.json");
        let inbox = Inbox::load(path.clone());
        inbox
            .receive(message("a", MessageKind::PeerRequest))
            .unwrap();
        inbox.claim("a", 100).unwrap();
        assert_eq!(
            Inbox::load(path).get("a").unwrap().state,
            MessageState::Interrupted
        );
    }
    #[test]
    fn busy_deferrals_do_not_consume_model_retries_but_provider_failures_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::load(dir.path().join("inbox.json"));
        inbox
            .receive(message("a", MessageKind::PeerRequest))
            .unwrap();
        inbox.claim("a", 100).unwrap();
        inbox.defer("a", 100, false, "busy").unwrap();
        assert_eq!(inbox.get("a").unwrap().attempts, 0);
        assert!(!inbox.claim("a", 104).unwrap());
        for now in [110, 210, 310] {
            assert!(inbox.claim("a", now).unwrap());
            inbox.defer("a", now, true, "provider failed").unwrap();
        }
        assert_eq!(inbox.get("a").unwrap().state, MessageState::Interrupted);
    }
}
