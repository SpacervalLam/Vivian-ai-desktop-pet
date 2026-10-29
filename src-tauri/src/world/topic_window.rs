//! Runtime-only topic continuity, grounded in completed public exchanges.
//! Reads and writes on the reply path never wait for background analysis.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

const WINDOW_SECONDS: f64 = 1800.0;
const MAX_CHARACTERS: usize = 32;
const MAX_EXCHANGES: usize = 6;

#[derive(Clone, Debug, Serialize)]
pub struct TopicExchange {
    pub turn_id: String,
    pub user_text: String,
    pub assistant_text: String,
    pub at: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TopicRelation {
    Continue,
    Switch,
    Pause,
    Close,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TopicAssessment {
    pub turn_id: String,
    pub relation: TopicRelation,
    pub subject: String,
    /// An exact unresolved question/detail from the supplied dialogue, not a new task.
    #[serde(default)]
    pub open_question: Option<String>,
    pub evidence: String,
    pub confidence: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TopicSnapshot {
    pub character_id: String,
    pub session_id: String,
    pub recent_exchanges: VecDeque<TopicExchange>,
    /// An inference about earlier dialogue; latest speech always takes precedence.
    pub assessment: Option<TopicAssessment>,
}

impl TopicSnapshot {
    fn fresh(&self, now: f64) -> bool {
        now.is_finite()
            && self
                .recent_exchanges
                .back()
                .is_some_and(|turn| turn.at <= now + 1.0 && now - turn.at < WINDOW_SECONDS)
    }

    pub fn render(&self) -> Option<String> {
        // Raw dialogue already travels in API messages. Only an up-to-date
        // semantic state adds information; never inject a second history copy.
        let assessment = self.assessment.as_ref()?;
        let latest = self.recent_exchanges.back()?;
        if assessment.turn_id != latest.turn_id {
            return None;
        }
        Some(format!("[CURRENT CONVERSATION TOPIC]\nShort-lived inference about your last completed exchange, not user testimony or a request. Actual messages and the latest user input take precedence. Resolve references naturally; do not repeat answered questions or reopen paused/closed topics without the user's lead. Never mention this state.\n{}\n[END CURRENT CONVERSATION TOPIC]",
            serde_json::json!({"subject":assessment.subject,"relation":assessment.relation,
                "open_question":assessment.open_question})))
    }

    fn prune(&mut self, now: f64) {
        self.recent_exchanges
            .retain(|turn| turn.at <= now + 1.0 && now - turn.at < WINDOW_SECONDS);
        if self.assessment.as_ref().is_some_and(|assessment| {
            !self
                .recent_exchanges
                .iter()
                .any(|turn| turn.turn_id == assessment.turn_id)
        }) {
            self.assessment = None;
        }
    }
}

#[derive(Default)]
struct Windows {
    sequence: u64,
    characters: HashMap<String, TopicSnapshot>,
}

#[derive(Default)]
pub struct TopicWindows {
    inner: Mutex<Windows>,
    missed_input: AtomicBool,
}

impl TopicWindows {
    pub fn record(&self, character: &str, session: &str, user: &str, assistant: &str, now: f64) {
        if character.is_empty() || user.trim().is_empty() || !now.is_finite() {
            return;
        }
        let Some(mut windows) = self.inner.try_lock() else {
            self.missed_input.store(true, Ordering::Release);
            return;
        };
        // Losing optional input must not make an older subject look current.
        if self.missed_input.swap(false, Ordering::AcqRel) {
            windows.characters.clear();
        }
        windows.characters.retain(|_, window| window.fresh(now));
        if !windows.characters.contains_key(character) && windows.characters.len() >= MAX_CHARACTERS
        {
            let oldest = windows
                .characters
                .iter()
                .min_by(|(_, a), (_, b)| {
                    a.recent_exchanges
                        .back()
                        .unwrap()
                        .at
                        .total_cmp(&b.recent_exchanges.back().unwrap().at)
                })
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                windows.characters.remove(&oldest);
            }
        }
        windows.sequence = windows.sequence.wrapping_add(1);
        let turn_id = format!("topic_turn:{}", windows.sequence);
        let window = windows
            .characters
            .entry(character.into())
            .or_insert_with(|| TopicSnapshot {
                character_id: character.into(),
                session_id: session.into(),
                recent_exchanges: VecDeque::new(),
                assessment: None,
            });
        if window.session_id != session {
            window.session_id = session.into();
            window.recent_exchanges.clear();
            window.assessment = None;
        }
        window.prune(now);
        window.recent_exchanges.push_back(TopicExchange {
            turn_id,
            user_text: bounded_text(user, 480),
            assistant_text: bounded_text(assistant, 320),
            at: now,
        });
        while window.recent_exchanges.len() > MAX_EXCHANGES {
            window.recent_exchanges.pop_front();
        }
        // A label whose cited evidence has left the window is no longer grounded.
        if window.assessment.as_ref().is_some_and(|assessment| {
            !window
                .recent_exchanges
                .iter()
                .any(|turn| turn.turn_id == assessment.turn_id)
        }) {
            window.assessment = None;
        }
    }

    pub fn snapshot(
        &self,
        character: &str,
        session: Option<&str>,
        now: f64,
    ) -> Option<TopicSnapshot> {
        let windows = self.inner.try_lock()?;
        if self.missed_input.load(Ordering::Acquire) {
            return None;
        }
        let mut snapshot = windows
            .characters
            .get(character)
            .filter(|window| {
                window.fresh(now) && session.is_none_or(|session| session == window.session_id)
            })?
            .clone();
        snapshot.prune(now);
        Some(snapshot)
    }

    /// A slow or fabricated model result cannot overwrite a newer topic or another role.
    pub fn apply(
        &self,
        character: &str,
        session: &str,
        assessment: TopicAssessment,
        now: f64,
    ) -> bool {
        let Some(mut windows) = self.inner.try_lock() else {
            return false;
        };
        if self.missed_input.load(Ordering::Acquire) {
            return false;
        }
        let Some(window) = windows
            .characters
            .get_mut(character)
            .filter(|window| window.session_id == session && window.fresh(now))
        else {
            return false;
        };
        window.prune(now);
        let latest = window.recent_exchanges.back().unwrap();
        let quote = assessment.evidence.trim();
        if assessment.turn_id != latest.turn_id
            || quote.is_empty()
            || quote.chars().count() > 200
            || !latest.user_text.contains(quote)
            || assessment.subject.trim().is_empty()
            || assessment.subject.chars().count() > 80
            || !assessment.confidence.is_finite()
            || !(0.75..=1.0).contains(&assessment.confidence)
            || window
                .assessment
                .as_ref()
                .is_some_and(|old| old.turn_id == assessment.turn_id)
        {
            return false;
        }
        if let Some(question) = &assessment.open_question {
            if assessment.relation == TopicRelation::Close
                || question.trim().is_empty()
                || question.chars().count() > 160
                || !window.recent_exchanges.iter().any(|turn| {
                    turn.user_text.contains(question) || turn.assistant_text.contains(question)
                })
            {
                return false;
            }
            // A new topic cannot inherit an unfinished question from the old one.
            if assessment.relation == TopicRelation::Switch
                && !latest.user_text.contains(question)
                && !latest.assistant_text.contains(question)
            {
                return false;
            }
        }
        if matches!(
            assessment.relation,
            TopicRelation::Switch | TopicRelation::Close
        ) {
            let latest = window.recent_exchanges.pop_back().unwrap();
            window.recent_exchanges.clear();
            window.recent_exchanges.push_back(latest);
        }
        window.assessment = Some(assessment);
        true
    }

    pub fn clear_character(&self, character: &str) {
        self.inner.lock().characters.remove(character);
    }
    pub fn clear(&self) {
        self.inner.lock().characters.clear();
    }
}

fn bounded_text(text: &str, limit: usize) -> String {
    let mut chars = text.trim().chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_none() {
        return head;
    }
    let head: String = head.chars().take(limit * 2 / 3).collect();
    let tail: String = text
        .trim()
        .chars()
        .rev()
        .take(limit - head.chars().count() - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assessment(
        window: &TopicSnapshot,
        relation: TopicRelation,
        subject: &str,
    ) -> TopicAssessment {
        let turn = window.recent_exchanges.back().unwrap();
        TopicAssessment {
            turn_id: turn.turn_id.clone(),
            relation,
            subject: subject.into(),
            open_question: None,
            evidence: turn.user_text.clone(),
            confidence: 0.9,
        }
    }

    #[test]
    fn a_short_answer_keeps_its_question_and_roles_and_sessions_are_isolated() {
        let windows = TopicWindows::default();
        windows.record(
            "vivian",
            "s1",
            "我们周末看电影吧",
            "你希望约在周几？",
            1000.0,
        );
        let first = windows.snapshot("vivian", Some("s1"), 1001.0).unwrap();
        let mut topic = assessment(&first, TopicRelation::Continue, "周末看电影的安排");
        topic.open_question = Some("你希望约在周几？".into());
        assert!(windows.apply("vivian", "s1", topic, 1001.0));
        windows.record("vivian", "s1", "周五", "好，那就周五。", 1002.0);
        let next = windows.snapshot("vivian", Some("s1"), 1003.0).unwrap();
        assert_eq!(next.recent_exchanges.len(), 2);
        assert_eq!(
            next.assessment.as_ref().unwrap().subject,
            "周末看电影的安排"
        );
        assert!(
            next.render().is_none(),
            "a newer exchange retires an old assessment until rechecked"
        );
        let topic = assessment(&next, TopicRelation::Continue, "周五看电影");
        assert!(!windows.apply("nana", "s1", topic.clone(), 1003.0));
        assert!(!windows.apply("vivian", "other", topic.clone(), 1003.0));
        assert!(windows.apply("vivian", "s1", topic, 1003.0));
        let rendered = windows
            .snapshot("vivian", Some("s1"), 1003.0)
            .unwrap()
            .render()
            .unwrap();
        assert!(rendered.contains("周五看电影"));
        assert!(
            !rendered.contains("好，那就周五。"),
            "raw history stays in API messages"
        );
        assert!(!rendered.contains("我们周末看电影吧"));
        assert!(windows.snapshot("nana", None, 1003.0).is_none());
        assert!(windows.snapshot("vivian", Some("other"), 1003.0).is_none());
        windows.record("vivian", "s2", "午饭吃什么", "面条？", 1004.0);
        let fresh = windows.snapshot("vivian", Some("s2"), 1004.0).unwrap();
        assert_eq!(fresh.recent_exchanges.len(), 1);
        assert!(fresh.assessment.is_none());
    }

    #[test]
    fn switching_and_closing_retire_old_questions_without_inventing_new_ones() {
        let windows = TopicWindows::default();
        windows.record("vivian", "s", "一起看电影吗", "你想看哪部？", 1000.0);
        windows.record(
            "vivian",
            "s",
            "先不聊电影了，晚饭吃什么",
            "想吃面条吗？",
            1001.0,
        );
        let snapshot = windows.snapshot("vivian", None, 1001.0).unwrap();
        let mut topic = assessment(&snapshot, TopicRelation::Switch, "晚饭");
        topic.open_question = Some("你想看哪部？".into());
        assert!(!windows.apply("vivian", "s", topic.clone(), 1001.0));
        topic.open_question = Some("想吃面条吗？".into());
        assert!(windows.apply("vivian", "s", topic, 1001.0));
        let snapshot = windows.snapshot("vivian", None, 1001.0).unwrap();
        assert_eq!(snapshot.recent_exchanges.len(), 1);
        assert!(!snapshot.render().unwrap().contains("你想看哪部"));
        windows.record("vivian", "s", "吃过了，这个不用了", "好。", 1002.0);
        let snapshot = windows.snapshot("vivian", None, 1002.0).unwrap();
        let mut topic = assessment(&snapshot, TopicRelation::Close, "晚饭已解决");
        topic.open_question = Some("想吃面条吗？".into());
        assert!(!windows.apply("vivian", "s", topic.clone(), 1002.0));
        topic.open_question = None;
        assert!(windows.apply("vivian", "s", topic, 1002.0));
        let snapshot = windows.snapshot("vivian", None, 1002.0).unwrap();
        assert_eq!(snapshot.assessment.unwrap().relation, TopicRelation::Close);
        assert_eq!(snapshot.recent_exchanges.len(), 1);
    }

    #[test]
    fn stale_fabricated_low_confidence_and_duplicate_assessments_are_discarded() {
        let windows = TopicWindows::default();
        windows.record("vivian", "s", "周五", "好", 1000.0);
        let first = windows.snapshot("vivian", None, 1000.0).unwrap();
        let old = assessment(&first, TopicRelation::Continue, "安排");
        windows.record("vivian", "s", "下次再说吧", "好，下次再聊。", 1001.0);
        assert!(!windows.apply("vivian", "s", old, 1001.0));
        let next = windows.snapshot("vivian", None, 1001.0).unwrap();
        let valid = assessment(&next, TopicRelation::Pause, "暂缓安排");
        let mut bad = valid.clone();
        bad.evidence = "用户从没说过".into();
        assert!(!windows.apply("vivian", "s", bad, 1001.0));
        let mut bad = valid.clone();
        bad.confidence = f64::NAN;
        assert!(!windows.apply("vivian", "s", bad, 1001.0));
        let mut bad = valid.clone();
        bad.confidence = 0.4;
        assert!(!windows.apply("vivian", "s", bad, 1001.0));
        let mut bad = valid.clone();
        bad.open_question = Some("明天记得提醒我".into());
        assert!(!windows.apply("vivian", "s", bad, 1001.0));
        assert!(windows.apply("vivian", "s", valid.clone(), 1001.0));
        assert!(!windows.apply("vivian", "s", valid, 1001.0));
        assert!(windows.snapshot("vivian", None, 2801.0).is_none());
        assert!(windows.snapshot("vivian", None, f64::NAN).is_none());
    }

    #[test]
    fn bounded_window_preserves_message_endings_and_forgets_expired_evidence() {
        let windows = TopicWindows::default();
        windows.record("vivian", "s", "电影", "哪部？", 1000.0);
        let first = windows.snapshot("vivian", None, 1000.0).unwrap();
        assert!(windows.apply(
            "vivian",
            "s",
            assessment(&first, TopicRelation::Continue, "电影"),
            1000.0
        ));
        for i in 1..10 {
            windows.record(
                "vivian",
                "s",
                &format!("{}最终决定周五", "前文".repeat(1000)),
                &format!("{}已确定", "回复".repeat(1000)),
                1000.0 + i as f64,
            );
        }
        let snapshot = windows.snapshot("vivian", None, 1010.0).unwrap();
        assert_eq!(snapshot.recent_exchanges.len(), MAX_EXCHANGES);
        assert!(snapshot.assessment.is_none());
        for turn in snapshot.recent_exchanges {
            assert_eq!(turn.user_text.chars().count(), 480);
            assert_eq!(turn.assistant_text.chars().count(), 320);
            assert!(turn.user_text.ends_with("最终决定周五"));
            assert!(turn.assistant_text.ends_with("已确定"));
        }
        windows.record("vivian", "s", "新一天的话题", "你好", 4000.0);
        assert_eq!(
            windows
                .snapshot("vivian", None, 4000.0)
                .unwrap()
                .recent_exchanges
                .len(),
            1
        );
        windows.clear_character("vivian");
        assert!(windows.snapshot("vivian", None, 4000.0).is_none());
        windows.record("nana", "s", "你好", "你好", 4001.0);
        windows.clear();
        assert!(windows.snapshot("nana", None, 4001.0).is_none());
    }

    #[test]
    fn optional_analysis_never_blocks_reply_handoff_or_prompt_reads() {
        let windows = TopicWindows::default();
        windows.record("vivian", "s", "你好", "你好", 1000.0);
        let busy = windows.inner.lock();
        windows.record("vivian", "s", "新消息", "回复", 1001.0);
        assert!(windows.snapshot("vivian", None, 1001.0).is_none());
        drop(busy);
        assert!(
            windows.snapshot("vivian", None, 1001.0).is_none(),
            "missed input retires the old topic"
        );
        windows.record("vivian", "s", "重新开始", "好的", 1002.0);
        let fresh = windows.snapshot("vivian", None, 1002.0).unwrap();
        assert_eq!(fresh.recent_exchanges.len(), 1);
        assert_eq!(fresh.recent_exchanges[0].user_text, "重新开始");
    }

    #[test]
    fn continuous_conversation_does_not_keep_expired_quotes_alive() {
        let windows = TopicWindows::default();
        windows.record("vivian", "s", "早先的话题", "早先的问题？", 1000.0);
        let first = windows.snapshot("vivian", None, 1000.0).unwrap();
        let mut topic = assessment(&first, TopicRelation::Continue, "早先的话题");
        topic.open_question = Some("早先的问题？".into());
        assert!(windows.apply("vivian", "s", topic, 1000.0));
        windows.record("vivian", "s", "新的进展", "收到", 2000.0);
        let snapshot = windows.snapshot("vivian", None, 2800.0).unwrap();
        assert_eq!(snapshot.recent_exchanges.len(), 1);
        assert!(snapshot.assessment.is_none());
        let mut invalid = assessment(&snapshot, TopicRelation::Continue, "新的进展");
        invalid.open_question = Some("早先的问题？".into());
        assert!(!windows.apply("vivian", "s", invalid, 2800.0));
    }
}
