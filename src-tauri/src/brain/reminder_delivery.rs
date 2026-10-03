//! Delivery facts and receipts are independent of the character's wording.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeliveryState {
    #[serde(default)]
    pub confirmed_count: u32,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub next_attempt_at: Option<f64>,
    #[serde(default)]
    pub last_delivered_at: Option<f64>,
    #[serde(default)]
    pub last_text: Option<String>,
}
impl DeliveryState {
    pub fn failed(&mut self, now: f64) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let delay = 15.0 * 2f64.powi(self.consecutive_failures.saturating_sub(1).min(6) as i32);
        self.next_attempt_at = Some(now + delay.min(900.0));
    }
    pub fn confirmed(&mut self, now: f64, text: String) {
        self.confirmed_count = self.confirmed_count.saturating_add(1);
        self.last_delivered_at = Some(now);
        self.last_text = Some(text);
        self.next_attempt_at = None;
        self.consecutive_failures = 0;
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ReminderContext {
    pub scheduled_time: f64,
    pub current_time: f64,
    pub confirmed_delivery_count: u32,
    pub last_delivered_at: Option<f64>,
    pub last_delivered_text: Option<String>,
    pub user_read_status: &'static str,
}
impl ReminderContext {
    pub fn new(scheduled_time: f64, current_time: f64, delivery: &DeliveryState) -> Self {
        Self {
            scheduled_time,
            current_time,
            confirmed_delivery_count: delivery.confirmed_count,
            last_delivered_at: delivery.last_delivered_at,
            last_delivered_text: delivery.last_text.clone(),
            user_read_status: "unknown",
        }
    }
    /// Vary only confirmed deliveries, never transport attempts or inferred user attitude.
    pub fn wording(&self, message: &str, character: &str, language: &str) -> String {
        let index = self.confirmed_delivery_count as usize % 3;
        let text = if language.starts_with("en") {
            [
                format!("It's time: {message}"),
                format!("A reminder for you: {message}"),
                format!("Your scheduled reminder: {message}"),
            ][index]
                .clone()
        } else if language.starts_with("ja") {
            [
                format!("時間だよ：{message}"),
                format!("予定のリマインダー：{message}"),
                format!("今の予定は、{message}だよ。"),
            ][index]
                .clone()
        } else if character == "nana" {
            [
                format!("嗯，到时间了：{message}"),
                format!("提醒你一下，{message}"),
                format!("现在的安排是：{message}"),
            ][index]
                .clone()
        } else {
            [
                format!("喂，到时间啦：{message}"),
                format!("提醒一下：{message}"),
                format!("这会儿该是：{message}"),
            ][index]
                .clone()
        };
        if self.current_time - self.scheduled_time < 60.0 {
            return text;
        }
        use chrono::TimeZone;
        let time = chrono::Local
            .timestamp_opt(self.scheduled_time as i64, 0)
            .single()
            .map(|t| t.format("%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let late = if language.starts_with("en") {
            format!(" (scheduled for {time})")
        } else if language.starts_with("ja") {
            format!("（予定時刻：{time}）")
        } else {
            format!("（原定 {time}）")
        };
        text + &late
    }
}
static RECEIPTS: LazyLock<Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>> =
    LazyLock::new(Default::default);
pub fn register_receipt() -> (String, tokio::sync::oneshot::Receiver<()>) {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    RECEIPTS.lock().insert(id.clone(), tx);
    (id, rx)
}
pub fn acknowledge(id: &str) -> bool {
    RECEIPTS
        .lock()
        .remove(id)
        .is_some_and(|tx| tx.send(()).is_ok())
}
pub fn discard_receipt(id: &str) {
    RECEIPTS.lock().remove(id);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_preserve_delivery_facts_and_wording() {
        let mut state = DeliveryState::default();
        let first = ReminderContext::new(100.0, 100.0, &state).wording("开会", "nana", "zh");
        for attempt in 1..=3 {
            state.attempts = attempt;
            state.failed(110.0);
        }
        assert_eq!(state.confirmed_count, 0);
        assert!(state.last_delivered_at.is_none());
        assert_eq!(
            ReminderContext::new(100.0, 100.0, &state).wording("开会", "nana", "zh"),
            first
        );
        state.confirmed(120.0, first.clone());
        assert_ne!(
            ReminderContext::new(200.0, 200.0, &state).wording("开会", "nana", "zh"),
            first
        );
        let context = ReminderContext::new(100.0, 300.0, &state);
        assert_eq!(context.scheduled_time, 100.0);
        assert_eq!(context.current_time, 300.0);
        assert_eq!(context.confirmed_delivery_count, 1);
        assert_eq!(context.user_read_status, "unknown");
        assert!(!serde_json::to_string(&context)
            .unwrap()
            .contains("attempts"));
    }
    #[tokio::test]
    async fn receipt_confirms_once_and_expired_receipts_do_not_count() {
        let (id, rx) = register_receipt();
        assert!(acknowledge(&id));
        assert!(!acknowledge(&id));
        rx.await.unwrap();
        let (id, rx) = register_receipt();
        discard_receipt(&id);
        assert!(!acknowledge(&id));
        assert!(rx.await.is_err());
    }
}
