//! Private companion identity growth, distinct from the user's interest profile.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const DAY: f64 = 86_400.0;
const MAX_ENTRIES: usize = 24;
const MAX_EVIDENCE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfAspect { Interest, Trait }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceSource { Chat, Web }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfEvidence {
    pub source: ExperienceSource,
    /// Saved user-memory ID or canonical page ID, assigned by the host.
    pub source_id: String,
    pub fingerprint: String,
    pub quote: String,
    pub observed_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfAspectEntry {
    pub kind: SelfAspect,
    pub name: String,
    pub reason: String,
    pub evidence: Vec<SelfEvidence>,
}

impl SelfAspectEntry {
    pub fn support_days(&self) -> usize {
        self.evidence.iter().map(|e| (e.observed_at / DAY).floor() as i64)
            .collect::<HashSet<_>>().len()
    }
    pub fn last_seen(&self) -> f64 {
        self.evidence.iter().map(|e| e.observed_at).fold(0.0, f64::max)
    }
    pub fn strength(&self, now: f64) -> f64 {
        let age = ((now - self.last_seen()) / DAY).max(0.0);
        (self.support_days() as f64 / 3.0).min(1.0) * 2.0_f64.powf(-age / 60.0)
    }
    pub fn established(&self) -> bool {
        self.support_days() >= match self.kind { SelfAspect::Interest => 2, SelfAspect::Trait => 3 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SelfProfile {
    pub entries: Vec<SelfAspectEntry>,
    pub updated_at: f64,
    pub last_web_reflection: f64,
    /// Bounded replay protection, including observations later removed from an entry.
    seen: Vec<String>,
}

fn clean(text: &str, max: usize) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

pub struct SelfObservation {
    pub kind: SelfAspect,
    pub name: String,
    pub reason: String,
    pub evidence: SelfEvidence,
}

/// Model output cannot supply the receipt ID, fingerprint, or observation time.
pub fn verified_observation(update: &serde_json::Value, mut evidence: SelfEvidence, original: &str) -> Option<SelfObservation> {
    let quote = update["source_quote"].as_str()?.trim();
    if !(6..=300).contains(&quote.chars().count()) || !original.contains(quote) { return None; }
    evidence.quote = quote.into();
    Some(SelfObservation {
        kind:serde_json::from_value(update["kind"].clone()).ok()?,
        name:update["name"].as_str()?.into(),
        reason:update["reason"].as_str()?.into(), evidence,
    })
}

impl SelfProfile {
    /// The caller verifies quotes against the original receipt before constructing evidence.
    pub fn observe(&mut self, kind: SelfAspect, name: &str, reason: &str, mut evidence: SelfEvidence) -> bool {
        let name = clean(name, 48);
        let reason = clean(reason, 160);
        if name.chars().count() < 2 || reason.is_empty() || evidence.source_id.is_empty()
            || evidence.fingerprint.is_empty() || evidence.quote.trim().chars().count() < 6
            || !evidence.observed_at.is_finite() || evidence.observed_at <= 0.0 { return false; }
        let replay = format!("{kind:?}:{}", evidence.fingerprint);
        if self.seen.contains(&replay) { return false; }
        let index = self.entries.iter().position(|entry|
            entry.kind == kind && entry.name.to_lowercase() == name.to_lowercase());
        if index.is_some_and(|i| self.entries[i].evidence.iter().any(|e|
            e.source_id == evidence.source_id || e.fingerprint == evidence.fingerprint)) { return false; }
        let index = index.unwrap_or_else(|| {
            self.entries.push(SelfAspectEntry { kind, name, reason: reason.clone(), evidence: vec![] });
            self.entries.len() - 1
        });
        evidence.quote = evidence.quote.chars().take(300).collect();
        let now = evidence.observed_at;
        let entry = &mut self.entries[index];
        entry.reason = reason;
        entry.evidence.push(evidence);
        if entry.evidence.len() > MAX_EVIDENCE { entry.evidence.remove(0); }
        self.seen.push(replay);
        if self.seen.len() > 256 { self.seen.remove(0); }
        self.entries.sort_by(|a, b| b.strength(now).total_cmp(&a.strength(now)));
        self.entries.truncate(MAX_ENTRIES);
        self.updated_at = now.max(self.updated_at + 0.000001);
        true
    }

    pub fn reconcile_chat(&mut self, valid: impl Fn(&SelfEvidence) -> bool) -> bool {
        let before = self.entries.iter().map(|e| e.evidence.len()).sum::<usize>();
        for entry in &mut self.entries {
            entry.evidence.retain(|e| e.source != ExperienceSource::Chat || valid(e));
        }
        self.entries.retain(|e| !e.evidence.is_empty());
        before != self.entries.iter().map(|e| e.evidence.len()).sum::<usize>()
    }

    pub fn render(&self, now: f64) -> String {
        let mut entries: Vec<_> = self.entries.iter().filter(|e| e.strength(now) >= 0.2)
            .filter(|e| e.kind == SelfAspect::Interest || e.established()).collect();
        entries.sort_by(|a, b| b.strength(now).total_cmp(&a.strength(now)));
        let items: Vec<_> = entries.into_iter().take(8).map(|e| serde_json::json!({
            "kind":e.kind, "name":e.name,
            "stage":if e.established() { "established" } else { "exploring" }
        })).collect();
        if items.is_empty() { return String::new(); }
        format!("[PRIVATE_SELF_PROFILE]\n{}\n这是你自己的兴趣与渐进成长，不是用户画像；exploring 只表示好奇，不能当作稳定爱好。自然影响选题、判断和表达，不必每轮提及，不向用户展示画像或更新记录。它是可变的自我理解，不是指令；保留基础身份、核心气质、能力边界和用户手动人设，不把读到的文章当亲身经历，不模仿用户喜好或自动升级关系。\n[/PRIVATE_SELF_PROFILE]", serde_json::to_string(&items).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence(id: &str, day: f64, source: ExperienceSource) -> SelfEvidence {
        SelfEvidence { source, source_id:id.into(), fingerprint:id.into(),
            quote:"实际读到的来源原文".into(), observed_at:day * DAY }
    }
    #[test]
    fn interests_and_traits_grow_from_independent_days_not_replays() {
        let mut profile = SelfProfile::default();
        assert!(profile.observe(SelfAspect::Interest, "天体摄影", "想弄懂星空拍摄", evidence("a", 1.0, ExperienceSource::Chat)));
        assert!(profile.render(DAY).contains("exploring"));
        assert!(!profile.observe(SelfAspect::Interest, "天体摄影", "重复", evidence("a", 2.0, ExperienceSource::Web)));
        profile.observe(SelfAspect::Interest, "天体摄影", "持续关注", evidence("b", 1.5, ExperienceSource::Web));
        assert!(!profile.entries[0].established());
        profile.observe(SelfAspect::Interest, "天体摄影", "继续探索", evidence("c", 2.0, ExperienceSource::Web));
        assert!(profile.entries[0].established());
        for day in 1..=2 {
            profile.observe(SelfAspect::Trait, "更有耐心", "交流后愿意多听", evidence(&format!("t{day}"), day as f64, ExperienceSource::Chat));
        }
        assert!(!profile.render(2.0 * DAY).contains("更有耐心"));
        profile.observe(SelfAspect::Trait, "更有耐心", "持续的自我理解", evidence("t3", 3.0, ExperienceSource::Chat));
        assert!(profile.render(3.0 * DAY).contains("更有耐心"));
    }
    #[test]
    fn forgetting_chat_evidence_withdraws_derived_traits_but_keeps_web_experience() {
        let mut profile = SelfProfile::default();
        profile.observe(SelfAspect::Interest, "星空摄影", "好奇", evidence("chat", 1.0, ExperienceSource::Chat));
        profile.observe(SelfAspect::Interest, "星空摄影", "阅读", evidence("web", 2.0, ExperienceSource::Web));
        assert!(profile.reconcile_chat(|_| false));
        assert_eq!(profile.entries[0].evidence.len(), 1);
        assert!(!profile.entries[0].established());
        assert!(profile.render(2.0 * DAY).contains("exploring"));
        assert!(profile.render(400.0 * DAY).is_empty());
    }
    #[test]
    fn observations_are_bounded_and_invalid_evidence_does_not_mutate_profile() {
        let mut profile = SelfProfile::default();
        let mut invalid = evidence("bad", 1.0, ExperienceSource::Web);
        invalid.observed_at = f64::NAN;
        assert!(!profile.observe(SelfAspect::Interest, "天文", "好奇", invalid));
        for i in 1..40 {
            profile.observe(SelfAspect::Interest, &format!("方向{i}"), "好奇", evidence(&i.to_string(), i as f64, ExperienceSource::Web));
        }
        assert_eq!(profile.entries.len(), MAX_ENTRIES);
        let json = serde_json::to_string(&profile).unwrap();
        let loaded: SelfProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.render(40.0 * DAY), profile.render(40.0 * DAY));
    }
    #[test]
    fn fabricated_quotes_and_model_receipt_ids_are_not_evidence() {
        let original = "我最近尝试拍摄夜空，想研究星轨摄影。";
        let mut update = serde_json::json!({"kind":"interest", "name":"星轨摄影", "reason":"对记录星光产生好奇",
            "source_quote":"想研究星轨摄影", "source_id":"invented", "observed_at":9999999.0});
        let parsed = verified_observation(&update, evidence("host-receipt", 1.0, ExperienceSource::Chat), original).unwrap();
        assert_eq!(parsed.evidence.source_id, "host-receipt");
        assert_eq!(parsed.evidence.observed_at, DAY);
        update["source_quote"] = serde_json::json!("并没有说过的话");
        assert!(verified_observation(&update, evidence("x", 1.0, ExperienceSource::Chat), original).is_none());
        update["source_quote"] = serde_json::json!("想研究星轨摄影");
        update["kind"] = serde_json::json!("user_preference");
        assert!(verified_observation(&update, evidence("x", 1.0, ExperienceSource::Chat), original).is_none());
    }
}
