//! Actual private learning episodes, never a command to share or simulate a feeling.
use super::learning_interest::PlannedSearch;
use crate::memory::{
    event_time::{EventAppraisal, EventTimeline},
    unified_event_ledger::register_character_episode,
};

pub struct LearningEpisode {
    owner: String,
    id: String,
    source: String,
    topic: String,
    started: f64,
    finished: bool,
    sequence: std::cell::Cell<u64>,
}
fn now() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64 / 1000.0
}
impl LearningEpisode {
    pub fn start(owner: &str, plan: &PlannedSearch) -> Self {
        let activity = Self {
            owner: owner.into(),
            id: format!("learning:{owner}:{}", uuid::Uuid::new_v4()),
            source: plan.anchor.id.clone(),
            topic: plan.query.clone(),
            started: now(),
            finished: false,
            sequence: std::cell::Cell::new(0),
        };
        activity.emit(
            "learning_started",
            "started",
            &format!(
                "开始围绕「{}」查阅资料；问题：{}",
                plan.query, plan.question
            ),
            None,
            false,
        );
        activity
    }
    fn emit(&self, kind: &str, phase: &str, fact: &str, memory_id: Option<&str>, ended: bool) {
        let at = now();
        self.sequence.set(self.sequence.get() + 1);
        register_character_episode(
            &self.owner,
            memory_id
                .map(|id| format!("knowledge:{id}"))
                .unwrap_or_else(|| format!("{}:{phase}", self.id)),
            kind,
            fact,
            at,
            vec!["private_learning".into()],
            EventTimeline {
                recorded_at: None,
                started_at: Some(self.started),
                ended_at: ended.then_some(at),
                episode_id: Some(self.id.clone()),
                caused_by: Some(self.source.clone()),
                memory_id: memory_id.map(str::to_owned),
                phase: Some(phase.into()),
                sequence: self.sequence.get(),
                appraisal: EventAppraisal {
                    personal_relevance: Some(0.8),
                    novelty: Some(if matches!(phase, "learned" | "completed") {
                        0.65
                    } else {
                        0.2
                    }),
                    agency: Some(0.8),
                    ..Default::default()
                },
            },
        );
    }
    pub fn read(&self, sources: usize) {
        self.emit(
            "learning_read",
            "read",
            &format!(
                "围绕「{}」实际取得了 {sources} 份可读原文，仍在理解整理",
                self.topic
            ),
            None,
            false,
        );
    }
    pub fn saved(&mut self, memory_id: &str, title: &str) {
        self.emit(
            "knowledge_acquired",
            "learned",
            &format!(
                "围绕「{}」阅读后保存了「{title}」；尚未向用户分享",
                self.topic
            ),
            Some(memory_id),
            true,
        );
        self.finished = true;
    }
    pub fn note(&mut self, note: &str) {
        self.emit(
            "learning_completed",
            "completed",
            &format!("围绕「{}」形成了私人阅读札记：{note}", self.topic),
            None,
            true,
        );
        self.finished = true;
    }
    pub fn defer(&mut self, reason: &str) {
        self.emit(
            "learning_deferred",
            "deferred",
            &format!("围绕「{}」的阅读暂告一段落：{reason}", self.topic),
            None,
            true,
        );
        self.finished = true;
    }
}
impl Drop for LearningEpisode {
    fn drop(&mut self) {
        if !self.finished {
            self.emit(
                "learning_interrupted",
                "interrupted",
                &format!("围绕「{}」的阅读已中断，没有完成内化的凭证", self.topic),
                None,
                true,
            );
        }
    }
}
