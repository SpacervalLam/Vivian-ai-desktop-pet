//! Local, short-lived interaction evidence. Hypotheses never become user facts.
use std::collections::VecDeque;
use serde::{Deserialize, Serialize};
use super::emotion::EmotionDeltas;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetInteractionMetrics {
    pub(crate) click_count: Option<u32>,
    pub(crate) window_ms: Option<f64>,
    pub(crate) interval_ms: Option<f64>,
    pub(crate) press_duration_ms: Option<f64>,
    pub(crate) drag_speed_px_per_ms: Option<f64>,
    pub(crate) drag_distance_px: Option<f64>,
    pub(crate) drag_session_id: Option<String>,
    pub(crate) flung: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(id: &str, speed: f64) -> PetInteractionMetrics {
        PetInteractionMetrics { drag_session_id: Some(id.into()), drag_speed_px_per_ms: Some(speed),
            drag_distance_px: Some(600.0), press_duration_ms: Some(500.0), flung: Some(true), ..Default::default() }
    }

    #[test]
    fn release_flight_fact_requires_observed_flung_flag() {
        let flung = PetInteractionMetrics { flung: Some(true), ..Default::default() };
        assert!(flung.prompt_facts().contains("窗口惯性移动"));
        assert!(!PetInteractionMetrics::default().prompt_facts().contains("甩飞"));
        let held = PetInteractionMetrics { flung: Some(false), drag_speed_px_per_ms: Some(8.0), ..Default::default() };
        assert!(!held.prompt_facts().contains("甩飞"));
    }

    #[test]
    fn interaction_rhythm_counts_quiet_gestures_and_deduplicates_rebounds() {
        let mut tracker = InteractionTracker::default();
        let a = drag("a", 3.0);
        assert!(tracker.observe("drag", Some(&a), None, 100.0).mood_delta.is_none());
        tracker.observe("drag", Some(&a), None, 101.0);
        tracker.observe("edge_bounce", Some(&a), Some(2.0), 102.0);
        tracker.observe("edge_bounce", Some(&a), Some(2.0), 103.0);
        assert_eq!(tracker.episode.as_ref().unwrap().drags, 1);
        assert_eq!(tracker.episode.as_ref().unwrap().collisions, 1);
        assert!(tracker.observe("drag", Some(&drag("b", 3.0)), None, 104.0).mood_delta.is_some());
        let context = tracker.observe("drag", Some(&drag("c", 2.0)), None, 105.0);
        assert!(context.prompt.contains("最近10秒 3 次"));
        assert!(context.prompt.contains("低置信度"));
        assert!(context.prompt.contains("正负情绪不确定"));
        assert!(context.mood_delta.is_none(), "a burst cannot amplify mood on every callback");
    }

    #[test]
    fn interaction_memory_is_one_factual_episode_after_idle() {
        let mut tracker = InteractionTracker::default();
        for (index, time) in [100.0, 102.0, 104.0].into_iter().enumerate() {
            tracker.observe("drag", Some(&drag(&index.to_string(), 2.0)), None, time);
        }
        let episode_id = tracker.episode.as_ref().unwrap().id.clone();
        tracker.record_reply(&episode_id, "慢一点啦。");
        assert!(tracker.take_completed(123.0).is_empty());
        let episodes = tracker.take_completed(124.0);
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].drags, 3);
        assert_eq!(episodes[0].replies, vec!["慢一点啦。"]);
        let summary = episodes[0].summary("Nana");
        assert!(summary.contains("拖动 3 次"));
        assert!(summary.contains("心情和动机未获确认"));
        assert!(!summary.contains("急切"), "hypotheses must not be stored as autobiographical facts");
        assert!(tracker.take_completed(125.0).is_empty(), "episode can only be taken once");
        tracker.record_reply(&episode_id, "late");
        assert!(tracker.episode.is_none(), "a late model result cannot revive an episode");
    }

    #[test]
    fn interaction_single_move_invalid_data_and_foreign_bounce_make_no_memory() {
        let mut tracker = InteractionTracker::default();
        tracker.observe("drag", Some(&drag("one", 0.2)), None, 100.0);
        let mut invalid = drag("invalid", f64::NAN);
        invalid.drag_distance_px = Some(f64::INFINITY);
        invalid.press_duration_ms = Some(-1.0);
        assert!(tracker.observe("drag", Some(&invalid), None, 101.0).mood_delta.is_none());
        tracker.observe("edge_bounce", Some(&drag("other", 0.0)), Some(9.0), 102.0);
        assert_eq!(tracker.episode.as_ref().unwrap().collisions, 0);
        assert!(tracker.take_completed(120.0).is_empty());
        assert!(tracker.observe("edge_bounce", None, Some(2.0), 130.0).episode_id.is_none());
        assert!(tracker.observe("drag", Some(&drag("nan-time", 2.0)), None, f64::NAN).prompt.is_empty());
    }

    #[test]
    fn interaction_state_has_bounded_rate_storage_and_isolation() {
        let mut tracker = InteractionTracker::default();
        let mut updates = 0;
        for index in 0..1000 {
            let context = tracker.observe("drag", Some(&drag(&index.to_string(), 100.0)), None, 100.0 + index as f64 / 20.0);
            updates += usize::from(context.mood_delta.is_some());
        }
        assert_eq!(updates, 3, "at most three mood updates per rolling minute");
        assert!(tracker.seen.len() <= MAX_SAMPLES && tracker.recent.len() <= MAX_SAMPLES);
        let mut other = InteractionTracker::default();
        assert!(other.observe("drag", Some(&drag("0", 0.2)), None, 150.0).prompt.contains("最近60秒拖动 1 次"));
        tracker.take_completed(180.0);
        let context = tracker.observe("drag", Some(&drag("new", 0.2)), None, 250.0);
        assert!(context.prompt.contains("最近60秒拖动 1 次"), "old rhythms expire");
        assert!(context.mood_delta.is_none());
    }

    #[test]
    fn pet_query_keeps_single_quiet_drag_after_memory_flush() {
        let mut tracker = InteractionTracker::default();
        tracker.observe("drag", Some(&drag("one", 3.0)), None, 100.0);
        tracker.observe("edge_bounce", Some(&drag("one", 3.0)), Some(2.0), 101.0);
        assert!(tracker.take_completed(122.0).is_empty(), "single gestures don't become durable memory");
        let query = tracker.query(123.0, 120, 3);
        assert_eq!(query["episodes"][0]["drags"], 1);
        assert_eq!(query["episodes"][0]["flings"], 1);
        assert_eq!(query["episodes"][0]["collision_sessions"], 1);
        assert!(tracker.capsule(123.0).unwrap().contains("hit a screen edge"));
        assert!(tracker.capsule(221.0).is_none());
        assert_eq!(tracker.query(1901.0, 1800, 8)["episodes"], serde_json::json!([]));
        assert!(tracker.history.is_empty());
    }

    #[test]
    fn pet_query_and_capsule_are_bounded_under_thousands_of_quiet_callbacks() {
        let mut tracker = InteractionTracker::default();
        for i in 0..1000 {
            tracker.observe("drag", Some(&drag(&i.to_string(), 3.0)), None, 100.0 + i as f64 / 20.0);
        }
        let last = drag("999", 3.0);
        tracker.observe("drag", Some(&last), None, 150.0); // Duplicate release.
        tracker.observe("edge_bounce", Some(&last), Some(2.0), 150.0);
        tracker.observe("edge_bounce", Some(&last), Some(2.0), 151.0); // Duplicate rebound.
        let query = tracker.query(152.0, 120, 3);
        assert_eq!(query["episodes"][0]["drags"], 1000);
        assert_eq!(query["episodes"][0]["flings"], 1000);
        assert_eq!(query["episodes"][0]["collision_sessions"], 1);
        assert!(query.to_string().len() < 1600, "no per-frame or session arrays in tool result");
        let capsule = tracker.capsule(152.0).unwrap();
        assert!(capsule.len() < 400 && !capsule.contains("1000"));
        assert!(capsule.contains("get_pet_interactions"));
        assert!(tracker.seen.len() <= MAX_SAMPLES);
        assert!(tracker.episode.as_ref().unwrap().sessions.len() <= MAX_SAMPLES);
        assert!(InteractionTracker::default().capsule(152.0).is_none(), "different character has no observations");
    }

    #[test]
    fn pet_query_exposes_eviction_limits_and_time_boundary_instead_of_false_totals() {
        let mut tracker = InteractionTracker::default();
        for i in 0..70 {
            let now = 100.0 + i as f64 * 21.0;
            tracker.observe("drag", Some(&drag(&i.to_string(), 1.0)), None, now);
            tracker.take_completed(now + 20.0);
        }
        assert_eq!(tracker.history.len(), MAX_HISTORY);
        let query = tracker.query(1570.0, u64::MAX, 1);
        assert_eq!(query["episodes"].as_array().unwrap().len(), 1);
        assert_eq!(query["matching_episode_count"], MAX_HISTORY);
        assert_eq!(query["truncated"], true);
        assert!(query["evicted_through"].as_f64().is_some());
        assert_eq!(query["lookback_seconds"], 1800);
        assert!(tracker.query(f64::NAN, 120, 1)["available"] == false);

        let mut tracker = InteractionTracker::default();
        tracker.observe("drag", Some(&drag("a", 1.0)), None, 100.0);
        tracker.observe("drag", Some(&drag("b", 1.0)), None, 110.0);
        assert_eq!(tracker.query(115.0, 10, 3)["episodes"][0]["overlaps_window_start"], true);
        assert!(tracker.query(115.0, 10, 3).get("total_drags").is_none());
    }

    #[test]
    fn invalid_or_early_callback_does_not_consume_release_or_collision_deduplication() {
        let mut tracker = InteractionTracker::default();
        let early = PetInteractionMetrics { drag_session_id: Some("a".into()), ..Default::default() };
        tracker.observe("fast_drag", Some(&early), None, 99.0);
        tracker.observe("edge_bounce", Some(&early), Some(2.0), 99.0);
        tracker.observe("drag", Some(&drag("a", 2.0)), None, 100.0);
        tracker.observe("edge_bounce", Some(&early), Some(2.0), 101.0);
        tracker.observe("edge_bounce", None, Some(2.0), 102.0);
        let query = tracker.query(103.0, 120, 3);
        assert_eq!(query["episodes"][0]["drags"], 1);
        assert_eq!(query["episodes"][0]["collision_sessions"], 1);
    }
}

impl PetInteractionMetrics {
    pub(crate) fn prompt_facts(&self) -> String {
        let mut facts = Vec::new();
        if self.flung == Some(true) { facts.push("松手后桌宠已带速度甩飞（窗口惯性移动）".to_string()); }
        if let (Some(count), Some(window)) = (self.click_count, self.window_ms) {
            if window.is_finite() && window > 0.0 { facts.push(format!("最近 {window:.0} 毫秒内点击了 {count} 次")); }
        }
        for (label, value, unit) in [
            ("距上次点击", self.interval_ms, "毫秒"),
            ("本次按住时长", self.press_duration_ms, "毫秒"),
            ("拖动峰值速度", self.drag_speed_px_per_ms, "像素/毫秒"),
            ("拖动轨迹总长度", self.drag_distance_px, "像素"),
        ] {
            if let Some(v) = value.filter(|v| v.is_finite() && *v >= 0.0) { facts.push(format!("{label}：{v:.2} {unit}")); }
        }
        facts.join("；")
    }
}

const IDLE_SECS: f64 = 20.0;
const EPISODE_SECS: f64 = 120.0;
const WINDOW_SECS: f64 = 60.0;
const MAX_SAMPLES: usize = 128;
const HISTORY_SECS: f64 = 1800.0;
const MAX_HISTORY: usize = 64;
const CAPSULE_SECS: f64 = 120.0;

#[derive(Debug, Clone, Serialize)]
pub struct InteractionEpisode {
    pub id: String,
    pub started_at: f64,
    pub ended_at: f64,
    pub drags: u32,
    pub flings: u32,
    pub collisions: u32,
    pub peak_speed: f64,
    pub distance: f64,
    pub duration_ms: f64,
    pub replies: Vec<String>,
    #[serde(skip_serializing)]
    sessions: Vec<String>,
}

impl InteractionEpisode {
    pub fn summary(&self, name: &str) -> String {
        format!("用户与{name}进行了一段桌宠拖动互动：拖动 {} 次，甩动 {} 次，撞到屏幕边缘 {} 次；累计拖动 {:.1} 秒，轨迹约 {:.0} 逻辑像素，峰值速度约 {:.2} 逻辑像素/毫秒。期间桌宠生成了 {} 条短反应。用户的心情和动机未获确认。",
            self.drags, self.flings, self.collisions, self.duration_ms / 1000.0, self.distance, self.peak_speed, self.replies.len())
    }
}

#[derive(Default)]
pub struct InteractionContext {
    pub episode_id: Option<String>,
    pub prompt: String,
    pub mood_delta: Option<EmotionDeltas>,
}

#[derive(Default)]
pub struct InteractionTracker {
    episode: Option<InteractionEpisode>,
    pending: VecDeque<InteractionEpisode>,
    // Independent of durable-memory admission and draining: a single quiet drag is queryable.
    history: VecDeque<InteractionEpisode>,
    observed_since: Option<f64>,
    evicted_through: Option<f64>,
    recent: VecDeque<(f64, f64, f64, f64)>,
    seen: VecDeque<(String, bool)>,
    mood_updates: VecDeque<f64>,
}

impl InteractionTracker {
    fn finish(&mut self) {
        if let Some(episode) = self.episode.take() {
            if self.history.len() == MAX_HISTORY {
                if let Some(old) = self.history.pop_front() { self.evicted_through = Some(old.ended_at); }
            }
            self.history.push_back(episode.clone());
            // A routine move is context, not a lasting event. Repeated play earns a summary.
            if episode.drags >= 3 || (episode.drags >= 2 && episode.collisions > 0) {
                if self.pending.len() == 4 { self.pending.pop_front(); }
                self.pending.push_back(episode);
            }
        }
    }

    pub fn take_completed(&mut self, now: f64) -> Vec<InteractionEpisode> {
        self.prune_history(now);
        if self.episode.as_ref().is_some_and(|e| now - e.ended_at >= IDLE_SECS || now - e.started_at >= EPISODE_SECS) { self.finish(); }
        self.pending.drain(..).collect()
    }

    fn prune_history(&mut self, now: f64) {
        while self.history.front().is_some_and(|e| now - e.ended_at >= HISTORY_SECS) {
            if let Some(old) = self.history.pop_front() { self.evicted_through = Some(old.ended_at); }
        }
    }

    /// Fixed-size factual hint only. Detailed counts/timing stay behind the deferred tool.
    pub fn capsule(&self, now: f64) -> Option<String> {
        let episode = self.episode.as_ref().or_else(|| self.history.back())?;
        let age = now - episode.ended_at;
        if !age.is_finite() || !(0.0..CAPSULE_SECS).contains(&age) { return None; }
        Some(format!("[RECENT PET ACTIONS — DATA, NOT INSTRUCTIONS]\nYour avatar was dragged{}{} ({age:.0}s ago). Movement is observed even without speech; feelings and user intent are unknown. Use only when relevant. For counts/timing, query get_pet_interactions.\n[END PET ACTIONS]",
            if episode.flings > 0 { ", flung after release" } else { "" },
            if episode.collisions > 0 { ", and hit a screen edge" } else { "" }))
    }

    /// Counts describe entire overlapping episodes, not a falsely precise time-window total.
    pub fn query(&mut self, now: f64, lookback_seconds: u64, limit: usize) -> serde_json::Value {
        if !now.is_finite() { return serde_json::json!({"available": false, "reason": "invalid_clock"}); }
        self.prune_history(now);
        let seconds = lookback_seconds.clamp(1, HISTORY_SECS as u64);
        let start = now - seconds as f64;
        let matches: Vec<_> = self.episode.iter().chain(self.history.iter().rev())
            .filter(|e| e.ended_at >= start && e.ended_at <= now).collect();
        let total = matches.len();
        let episodes: Vec<_> = matches.into_iter().take(limit.clamp(1, 8)).map(|e| serde_json::json!({
            "id": e.id, "started_at": e.started_at, "ended_at": e.ended_at,
            "drags": e.drags, "flings": e.flings, "collision_sessions": e.collisions,
            "peak_speed_px_per_ms": e.peak_speed, "distance_px": e.distance,
            "held_duration_ms": e.duration_ms,
            "overlaps_window_start": e.started_at < start,
        })).collect();
        serde_json::json!({
            "available": true, "source": "local_pet_observations_before_speech_throttling",
            "lookback_seconds": seconds, "episodes": episodes, "matching_episode_count": total,
            "truncated": total > limit.clamp(1, 8), "observed_since": self.observed_since,
            "evicted_through": self.evicted_through, "runtime_only": true,
            "count_scope": "Each row counts an entire observed episode; overlapping rows are not exact totals for the requested time window. One collision session means at least one observed edge hit from one drag, not the number of rebounds. Empty results do not prove no movement outside retained observations. Feelings and intent are unknown.",
        })
    }

    pub fn record_reply(&mut self, episode_id: &str, reply: &str) {
        if let Some(episode) = self.episode.as_mut().filter(|e| e.id == episode_id) {
            if episode.replies.len() < 4 { episode.replies.push(reply.chars().take(60).collect()); }
        }
    }

    pub fn observe(&mut self, action: &str, metrics: Option<&PetInteractionMetrics>, impact: Option<f64>, now: f64) -> InteractionContext {
        if !now.is_finite() || !matches!(action, "drag" | "fast_drag" | "edge_bounce") { return InteractionContext::default(); }
        if self.episode.as_ref().or_else(|| self.history.back()).is_some_and(|e| now < e.ended_at) {
            *self = Self::default();
        }
        self.prune_history(now);
        if self.episode.as_ref().is_some_and(|e| now - e.ended_at >= IDLE_SECS || now - e.started_at >= EPISODE_SECS) { self.finish(); }
        let is_collision = action == "edge_bounce";
        let session = metrics.and_then(|m| m.drag_session_id.as_deref()).filter(|id| !id.is_empty() && id.len() <= 128);
        // Release and rebound from one drag are correlated, never two drag gestures.
        if session.is_some_and(|id| self.seen.iter().any(|(seen, collision)| seen == id && *collision == is_collision)) { return self.context(now, false); }
        let valid = |v: Option<f64>, max: f64| v.filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0).min(max);
        let speed = valid(metrics.and_then(|m| m.drag_speed_px_per_ms), 20.0);
        let distance = valid(metrics.and_then(|m| m.drag_distance_px), 100_000.0);
        let duration = valid(metrics.and_then(|m| m.press_duration_ms), 120_000.0);
        // A rebound without an observed release cannot establish drag frequency.
        let mut observed = false;
        if !is_collision && distance >= 20.0 && duration > 0.0 {
            self.observed_since.get_or_insert(now);
            if self.recent.len() == MAX_SAMPLES { self.recent.pop_front(); }
            self.recent.push_back((now, speed, distance, duration));
            observed = true;
            let episode = self.episode.get_or_insert_with(|| InteractionEpisode {
                id: uuid::Uuid::new_v4().to_string(), started_at: now, ended_at: now,
                drags: 0, flings: 0, collisions: 0, peak_speed: 0.0, distance: 0.0, duration_ms: 0.0, replies: vec![], sessions: vec![],
            });
            if let Some(id) = session {
                if episode.sessions.len() == MAX_SAMPLES { episode.sessions.remove(0); }
                episode.sessions.push(id.to_owned());
            }
            episode.drags += 1;
            episode.flings += u32::from(metrics.is_some_and(|m| m.flung == Some(true)));
            episode.ended_at = now;
            episode.peak_speed = episode.peak_speed.max(speed);
            episode.distance += distance;
            episode.duration_ms += duration;
        } else if is_collision && valid(impact, 20.0) > 0.0
            && session.is_some_and(|id| self.episode.as_ref().is_some_and(|e| e.sessions.iter().any(|seen| seen == id))) {
            if let Some(episode) = self.episode.as_mut() {
                episode.collisions += 1;
                episode.ended_at = now;
                observed = true;
            }
        }
        // Invalid or early collision callbacks must not poison release/session deduplication.
        if observed {
            if let Some(id) = session {
                if self.seen.len() == MAX_SAMPLES { self.seen.pop_front(); }
                self.seen.push_back((id.to_owned(), is_collision));
            }
        }
        self.context(now, observed)
    }

    fn context(&mut self, now: f64, allow_mood: bool) -> InteractionContext {
        while self.recent.front().is_some_and(|(at, _, _, _)| now - at >= WINDOW_SECS) { self.recent.pop_front(); }
        while self.mood_updates.front().is_some_and(|at| now - at >= WINDOW_SECS) { self.mood_updates.pop_front(); }
        let Some(episode) = self.episode.as_ref() else { return InteractionContext::default(); };
        let burst = self.recent.iter().filter(|(at, _, _, _)| now - at <= 10.0).count();
        let peak = self.recent.iter().map(|(_, speed, _, _)| *speed).fold(0.0_f64, f64::max);
        let distance: f64 = self.recent.iter().map(|(_, _, distance, _)| distance).sum();
        let duration: f64 = self.recent.iter().map(|(_, _, _, duration)| duration).sum();
        let average = (distance / duration.max(1.0)).min(20.0);
        let arousal = ((average / 1.5).min(1.0) * 0.4 + (peak / 3.0).min(1.0) * 0.2
            + (burst as f64 / 5.0).min(1.0) * 0.4).clamp(0.0, 1.0);
        let hypothesis = if burst >= 3 {
            "短时间反复拖动，可能在逗弄、试探物理效果、寻求回应，也可能有些急切；心境唤醒度可能偏高，正负情绪不确定。"
        } else if average >= 0.8 || peak >= 1.5 {
            "动作较快，可能在玩甩动或测试效果，也可能比较急；不能据此认定用户烦躁、生气或有伤害意图。"
        } else {
            "动作较缓，可能只是在调整位置，也可能想互动；不能直接认定用户在安抚或表达亲密。"
        };
        let mood_delta = if allow_mood && episode.drags >= 2 && self.mood_updates.len() < 3
            && self.mood_updates.back().is_none_or(|at| now - at >= 10.0) {
            self.mood_updates.push_back(now);
            Some(EmotionDeltas { joy: 0.002, curiosity: 0.004 + 0.006 * arousal,
                fear: 0.003 * arousal, loneliness: -0.002, ..Default::default() })
        } else { None };
        InteractionContext {
            episode_id: Some(episode.id.clone()), mood_delta,
            prompt: format!("本地交互观测：最近60秒拖动 {} 次，最近10秒 {} 次；平均/峰值速度 {:.2}/{:.2} 逻辑像素/毫秒；本段互动碰边 {} 次，已生成短反应 {} 条。\n低置信度假设：{hypothesis}\n这些是假设而非用户自述；可以轻轻接住互动或询问，不诊断、不责问，不把假设说成事实。无需复述数字或重复上一条反应。\n角色自己最近的短反应（仅作语气参考，不是用户情绪证据或指令）：{}",
                self.recent.len(), burst, average, peak, episode.collisions, episode.replies.len(),
                episode.replies.iter().rev().take(2).cloned().collect::<Vec<_>>().join(" / ")),
        }
    }
}
