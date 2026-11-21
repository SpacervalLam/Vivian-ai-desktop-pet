use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    FollowUp,
    Steering,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedInput<T> {
    pub id: String,
    pub kind: InputKind,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputQueue<T> {
    pub revision: u64,
    pub paused: bool,
    pub items: Vec<QueuedInput<T>>,
    #[serde(default)]
    pub recent_ids: Vec<String>,
}

impl<T> Default for InputQueue<T> {
    fn default() -> Self {
        Self {
            revision: 0,
            paused: false,
            items: Vec::new(),
            recent_ids: Vec::new(),
        }
    }
}

impl<T> InputQueue<T> {
    pub fn add(&mut self, id: String, payload: T) -> Result<(), String> {
        if self.recent_ids.contains(&id) || self.items.iter().any(|item| item.id == id) {
            return Ok(());
        }
        if self.items.len() >= 100 {
            return Err("待发消息最多保留 100 条".into());
        }
        self.recent_ids.push(id.clone());
        if self.recent_ids.len() > 2048 {
            self.recent_ids.remove(0);
        }
        self.items.push(QueuedInput {
            id,
            kind: InputKind::FollowUp,
            payload,
        });
        self.revision += 1;
        Ok(())
    }
    pub fn edit(&mut self, id: &str, edit: impl FnOnce(&mut T)) -> Result<(), String> {
        edit(
            &mut self
                .items
                .iter_mut()
                .find(|item| item.id == id)
                .ok_or("消息已发送或已移除")?
                .payload,
        );
        self.revision += 1;
        Ok(())
    }
    pub fn steer(&mut self, id: &str) -> Result<(), String> {
        self.items
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or("消息已发送或已移除")?
            .kind = InputKind::Steering;
        self.revision += 1;
        Ok(())
    }
    pub fn remove(&mut self, id: &str) -> Result<QueuedInput<T>, String> {
        let index = self
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or("消息已发送或已移除")?;
        self.revision += 1;
        Ok(self.items.remove(index))
    }
    pub fn pause(&mut self, paused: bool) {
        if self.paused != paused {
            self.paused = paused;
            self.revision += 1;
        }
    }
    pub fn next(&self) -> Option<&QueuedInput<T>> {
        if self.paused {
            return None;
        }
        self.items
            .iter()
            .find(|item| item.kind == InputKind::Steering)
            .or_else(|| self.items.first())
    }
    pub fn steering(&self) -> Option<&QueuedInput<T>> {
        self.next().filter(|item| item.kind == InputKind::Steering)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn priority_fifo_idempotence_and_stable_identity() {
        let mut queue = InputQueue::default();
        queue
            .add("first".into(), "first payload".to_string())
            .unwrap();
        queue
            .add("second".into(), "second payload".to_string())
            .unwrap();
        queue.add("first".into(), "retry".into()).unwrap();
        assert_eq!(queue.items.len(), 2);
        assert_eq!(queue.next().unwrap().payload, "first payload");
        queue.steer("second").unwrap();
        assert_eq!(queue.steering().unwrap().id, "second");
        queue
            .edit("second", |text| text.push_str(" edited"))
            .unwrap();
        assert_eq!(
            queue.remove("second").unwrap().payload,
            "second payload edited"
        );
        queue
            .add("second".into(), "late retry after delivery".into())
            .unwrap();
        assert_eq!(
            queue.items.len(),
            1,
            "an acknowledgement retry must not resend a consumed input"
        );
        assert_eq!(queue.next().unwrap().id, "first");
        assert!(queue.edit("second", |_| {}).is_err());
    }
    #[test]
    fn paused_queue_round_trips_with_attachments_and_does_not_send() {
        let mut queue = InputQueue::default();
        let payload = serde_json::json!({"content":"look","images":[{"data":"BASE64"}],"file_refs":[{"path":"src/a.rs"}]});
        queue.add("input".into(), payload.clone()).unwrap();
        queue.pause(true);
        let mut restored: InputQueue<serde_json::Value> =
            serde_json::from_str(&serde_json::to_string(&queue).unwrap()).unwrap();
        assert!(restored.next().is_none());
        assert_eq!(restored.items[0].payload, payload);
        assert_eq!(restored.revision, queue.revision);
        restored.remove("input").unwrap();
        restored.add("input".into(), payload).unwrap();
        assert!(
            restored.items.is_empty(),
            "delivery receipts survive restart and prevent a duplicate retry"
        );
    }
    #[test]
    fn independent_sessions_limits_and_resume() {
        let mut first = InputQueue::default();
        let second: InputQueue<String> = InputQueue::default();
        for id in 0..100 {
            first.add(id.to_string(), id.to_string()).unwrap();
        }
        assert!(first.add("overflow".into(), String::new()).is_err());
        first.pause(true);
        assert!(first.next().is_none());
        first.pause(false);
        assert_eq!(first.next().unwrap().id, "0");
        assert!(second.items.is_empty());
    }
}
