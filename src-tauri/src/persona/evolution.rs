//! Evidence-backed, scoped growth. Factory identity is never rewritten.
//! Legacy entries remain inspectable but cannot become active without provenance.
use std::path::PathBuf;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use crate::error::{VivianError, VivianResult};
use crate::utils::path;

const MIN_INTERVAL: f64 = 6.0 * 3600.0;
const MAX_CANDIDATES: usize = 12;
const MAX_EVIDENCE: usize = 8;
pub const SCOPES: &[&str] = &["comfort", "care", "praise", "humor", "daily", "disagreement"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthEvidence {
    pub memory_id: String,
    pub quote: String,
    pub timestamp: f64,
    /// Hash of original user content, not a model-generated reflection ID.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionEntry {
    pub timestamp: f64,
    pub kind: String,
    pub text: String,
    pub reason: String,
    #[serde(default)]
    pub support: u32,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub evidence: Vec<GrowthEvidence>,
    /// Explicit user feedback can immediately change this local interaction habit.
    #[serde(default)]
    pub explicit_feedback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionCandidate {
    pub kind: String,
    pub text: String,
    pub reason: String,
    pub first_seen: f64,
    pub support: u32,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub evidence: Vec<GrowthEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaEvolution {
    #[serde(default)]
    pub entries: Vec<EvolutionEntry>,
    #[serde(default)]
    pub candidates: Vec<EvolutionCandidate>,
    #[serde(default)]
    pub updated_at: f64,
    /// Replaced versions are inspectable, never automatically resurrected.
    #[serde(default)]
    pub history: Vec<EvolutionEntry>,
}

fn clean(s: &str, limit: usize) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(limit).collect()
}
fn independent_support(evidence: &[GrowthEvidence]) -> u32 {
    evidence.iter().map(|e| (e.timestamp / 86400.0).floor() as i64)
        .collect::<std::collections::HashSet<_>>().len() as u32
}
impl EvolutionEntry {
    pub fn active(&self) -> bool {
        SCOPES.contains(&self.scope.as_str()) && !self.evidence.is_empty()
            && (self.explicit_feedback || independent_support(&self.evidence) >= 2)
    }
}
impl EvolutionCandidate {
    /// Snapshot reference lets the model refine wording without fragmenting support.
    pub fn reference(&self) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(format!("{}\0{}\0{}\0{}",
            self.kind, self.scope, self.first_seen, self.text).as_bytes()))[..16].to_string()
    }
}
impl PersonaEvolution {
    pub fn empty() -> Self {
        Self { entries: vec![], candidates: vec![], history: vec![], updated_at: 0.0 }
    }
    pub fn is_empty(&self) -> bool { !self.entries.iter().any(EvolutionEntry::active) }
    fn touch(&mut self, now: f64) { self.updated_at = now.max(self.updated_at + 0.000001); }

    /// Evidence must be verified against a saved user message by the caller.
    /// Same source/content never counts twice; ordinary growth requires different days.
    pub fn propose(&mut self, kind: &str, scope: &str, text: &str, reason: &str,
        evidence: GrowthEvidence, explicit_feedback: bool, now: f64) -> bool {
        self.propose_revision(kind, scope, text, reason, evidence, explicit_feedback, now, None)
    }

    pub fn propose_revision(&mut self, kind: &str, scope: &str, text: &str, reason: &str,
        evidence: GrowthEvidence, explicit_feedback: bool, now: f64, revises: Option<&str>) -> bool {
        let text = clean(text, 180);
        if !matches!(kind, "tone" | "personality") || !SCOPES.contains(&scope)
            || text.is_empty() || evidence.memory_id.is_empty() || evidence.quote.trim().is_empty()
            || evidence.fingerprint.is_empty() || !evidence.timestamp.is_finite() { return false; }
        // A single event may not be recycled to support another interpretation in this scope.
        let seen = |items: &[GrowthEvidence]| items.iter().any(|e|
            e.memory_id == evidence.memory_id || e.fingerprint == evidence.fingerprint);
        if self.entries.iter().chain(self.history.iter()).any(|e| e.scope == scope && seen(&e.evidence))
            || self.candidates.iter().any(|c| c.scope == scope && seen(&c.evidence)) { return false; }
        let reason = clean(reason, 240);
        let idx = if let Some(reference) = revises.filter(|r| !r.is_empty()) {
            // Never merge across scopes/kinds or accept references from an old snapshot.
            let Some(idx) = self.candidates.iter().position(|c|
                c.scope == scope && c.kind == kind && c.reference() == reference) else { return false; };
            Some(idx)
        } else {
            self.candidates.iter().position(|c| c.scope == scope && c.kind == kind && c.text == text)
        };
        let idx = idx.unwrap_or_else(|| {
            self.candidates.push(EvolutionCandidate { kind: kind.into(), scope: scope.into(),
                text: text.clone(), reason: reason.clone(), first_seen: now, support: 0, evidence: vec![] });
            self.candidates.len() - 1
        });
        let c = &mut self.candidates[idx];
        c.text = text;
        c.evidence.push(evidence);
        if c.evidence.len() > MAX_EVIDENCE { c.evidence.remove(0); }
        c.support = independent_support(&c.evidence);
        c.reason = reason;
        let last_promotion = self.entries.iter().filter(|e| e.active())
            .map(|e| e.timestamp).fold(0.0, f64::max);
        let promote = explicit_feedback || (c.support >= 2
            && (last_promotion == 0.0 || now - last_promotion >= MIN_INTERVAL));
        if promote {
            let c = self.candidates.remove(idx);
            let mut retained = vec![];
            for e in self.entries.drain(..) {
                if e.scope == scope { self.history.push(e); } else { retained.push(e); }
            }
            self.entries = retained;
            self.entries.push(EvolutionEntry { timestamp: now, kind: c.kind, scope: c.scope,
                text: c.text, reason: c.reason, support: c.support, evidence: c.evidence, explicit_feedback });
            if self.history.len() > 20 { self.history.drain(..self.history.len() - 20); }
        }
        // Keep strongest/newest candidates; do not accidentally retain the weakest ones.
        self.candidates.sort_by(|a,b| b.support.cmp(&a.support).then(b.first_seen.total_cmp(&a.first_seen)));
        self.candidates.truncate(MAX_CANDIDATES);
        self.touch(now);
        promote
    }

    /// Deletion/correction invalidates derived claims and removes copied source quotes.
    /// Missing evidence is conservative: fall back to the seed, not a fabricated biography.
    pub fn reconcile(&mut self, valid: impl Fn(&GrowthEvidence) -> bool, now: f64) -> bool {
        let before = (self.entries.len(), self.candidates.len(), self.history.len());
        // Withdraw the whole interpretation on source edits, including copied reason text.
        // Re-deriving it from surviving evidence is safer than retaining a stale explanation.
        self.entries.retain(|e| e.evidence.is_empty() || e.evidence.iter().all(&valid));
        self.history.retain(|e| e.evidence.is_empty() || e.evidence.iter().all(&valid));
        self.candidates.retain(|c| c.evidence.is_empty() || c.evidence.iter().all(&valid));
        let changed = before != (self.entries.len(), self.candidates.len(), self.history.len());
        if changed { self.touch(now); }
        changed
    }
    pub fn render(&self, _lang: &str) -> Option<String> {
        let active: Vec<_> = self.entries.iter().filter(|e| e.active()).map(|e|
            serde_json::json!({"scope":e.scope,"understanding":e.text})).collect();
        if active.is_empty() { return None; }
        Some(format!("[LEARNED_SELF]\n{}\nThese are scoped, revisable interpretations of real interactions. Apply only in the named situation; within it they replace factory behavioral examples. Preserve identity, temperament, safety and explicit user settings. Do not recite these records or manufacture experiences.\n[/LEARNED_SELF]", serde_json::to_string(&active).unwrap_or_default()))
    }
}

pub struct PersonaEvolutionStore { inner: RwLock<PersonaEvolution>, path: PathBuf }
impl PersonaEvolutionStore {
    pub fn new(char_id: &str) -> VivianResult<Self> {
        let dir = path::get_character_data_dir(char_id).join("persona");
        std::fs::create_dir_all(&dir).map_err(|e| VivianError::Memory(e.to_string()))?;
        let path = dir.join("evolution.json");
        let value = crate::utils::fs::load_json_or_backup::<PersonaEvolution>(&path).unwrap_or_else(PersonaEvolution::empty);
        Ok(Self { inner: RwLock::new(value), path })
    }
    pub fn fallback() -> Self { Self { inner: RwLock::new(PersonaEvolution::empty()), path: PathBuf::new() } }
    pub fn is_empty(&self) -> bool { self.inner.read().is_empty() }
    pub fn last_update(&self) -> f64 { self.inner.read().updated_at }
    pub fn entries(&self) -> Vec<EvolutionEntry> { self.inner.read().entries.clone() }
    pub fn candidates(&self) -> Vec<EvolutionCandidate> { self.inner.read().candidates.clone() }
    pub fn history(&self) -> Vec<EvolutionEntry> { self.inner.read().history.clone() }
    fn persist(&self, value: &PersonaEvolution) -> VivianResult<()> {
        if self.path.as_os_str().is_empty() { return Ok(()); }
        let json = serde_json::to_string_pretty(value).map_err(|e| VivianError::Memory(e.to_string()))?;
        crate::utils::fs::write_atomic(&self.path, &json).map_err(|e| VivianError::Memory(e.to_string()))
    }
    /// Persist candidates as well as promotions; serialize writes under the same lock.
    pub fn propose(&self, kind: &str, scope: &str, text: &str, reason: &str,
        evidence: GrowthEvidence, explicit_feedback: bool) -> bool {
        self.propose_revision(kind, scope, text, reason, evidence, explicit_feedback, None)
    }
    pub fn propose_revision(&self, kind: &str, scope: &str, text: &str, reason: &str,
        evidence: GrowthEvidence, explicit_feedback: bool, revises: Option<&str>) -> bool {
        let mut guard = self.inner.write();
        let mut next = guard.clone();
        let promoted = next.propose_revision(kind, scope, text, reason, evidence, explicit_feedback,
            crate::memory::types::current_timestamp(), revises);
        if next.updated_at == guard.updated_at { return false; }
        if let Err(e) = self.persist(&next) { tracing::warn!("Growth persistence failed: {e}"); return false; }
        *guard = next;
        promoted
    }
    pub fn reconcile(&self, valid: impl Fn(&GrowthEvidence) -> bool) {
        let mut guard = self.inner.write();
        if guard.reconcile(valid, crate::memory::types::current_timestamp()) {
            if let Err(e) = self.persist(&guard) { tracing::warn!("Growth invalidation persistence failed: {e}"); }
        }
    }
    pub fn reset(&self) {
        let mut guard = self.inner.write();
        *guard = PersonaEvolution::empty();
        if let Err(e) = self.persist(&guard) { tracing::warn!("Growth reset persistence failed: {e}"); }
    }
    pub fn render(&self, lang: &str) -> Option<String> { self.inner.read().render(lang) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence(id: &str, day: f64) -> GrowthEvidence {
        GrowthEvidence { memory_id: id.into(), quote: format!("quote-{id}"), timestamp: day * 86400.0,
            fingerprint: id.into() }
    }
    fn add(ev: &mut PersonaEvolution, id: &str, day: f64, text: &str, explicit: bool) -> bool {
        ev.propose("tone", "comfort", text, "reason", evidence(id, day), explicit, day * 86400.0)
    }
    #[test]
    fn retries_and_same_day_are_not_independent_experiences() {
        let mut ev = PersonaEvolution::empty();
        assert!(!add(&mut ev,"a",1.0,"listen first",false));
        assert!(!add(&mut ev,"a",2.0,"listen first",false));
        assert!(!add(&mut ev,"b",1.0,"listen first",false));
        assert_eq!(ev.candidates[0].support,1);
        assert!(add(&mut ev,"c",2.0,"listen first",false));
    }
    #[test]
    fn boundary_replaces_only_its_scope_and_invalidated_evidence_falls_back() {
        let mut ev = PersonaEvolution::empty();
        assert!(add(&mut ev,"a",1.0,"old",true));
        assert!(add(&mut ev,"b",1.0,"new",true));
        assert_eq!(ev.entries.len(),1);
        assert_eq!(ev.history[0].text,"old");
        ev.reconcile(|e| e.memory_id != "b", 100000.0);
        assert!(ev.is_empty());
        assert!(ev.render("zh").is_none());
    }
    #[test]
    fn legacy_records_are_readable_but_not_evidence() {
        let ev: PersonaEvolution = serde_json::from_str(r#"{"entries":[{"timestamp":1,"kind":"tone","text":"old","reason":"reason","support":99}]}"#).unwrap();
        assert_eq!(ev.entries.len(),1);
        assert!(ev.is_empty());
    }
    #[test]
    fn candidate_survives_store_reload() {
        let dir = std::env::temp_dir().join(format!("growth-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("evolution.json");
        let store = PersonaEvolutionStore { inner: RwLock::new(PersonaEvolution::empty()), path: path.clone() };
        assert!(!store.propose("tone","comfort","listen","reason",evidence("a",1.0),false));
        let saved: PersonaEvolution = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.candidates[0].evidence[0].memory_id,"a");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn loss_of_one_independent_day_demotes_growth() {
        let mut ev=PersonaEvolution::empty();
        add(&mut ev,"a",1.0,"listen",false);
        add(&mut ev,"b",2.0,"listen",false);
        assert!(!ev.is_empty());
        ev.reconcile(|e| e.memory_id != "a",200000.0);
        assert!(ev.is_empty());
    }
    #[test]
    fn refined_candidate_keeps_independent_evidence_and_rewrites_prompt() {
        let mut ev = PersonaEvolution::empty();
        assert!(!add(&mut ev, "a", 1.0, "listen first", false));
        let reference = ev.candidates[0].reference();
        assert!(ev.propose_revision("tone", "comfort", "先听具体困扰，再给建议", "new evidence",
            evidence("b", 2.0), false, 2.0 * 86400.0, Some(&reference)));
        assert!(ev.candidates.is_empty());
        assert_eq!(ev.entries[0].evidence.len(), 2);
        assert_eq!(ev.entries[0].support, 2);
        let prompt = ev.render("zh").unwrap();
        assert!(prompt.contains("先听具体困扰，再给建议"));
        assert!(!prompt.contains("listen first"));
        ev.reconcile(|e| e.memory_id != "a", 3.0 * 86400.0);
        assert!(ev.render("zh").is_none(), "revised wording still depends on original evidence");
    }
    #[test]
    fn references_cannot_merge_other_scopes_kinds_or_recycled_sources() {
        let mut ev = PersonaEvolution::empty();
        add(&mut ev, "a", 1.0, "listen first", false);
        let reference = ev.candidates[0].reference();
        for (kind, scope, id, target) in [
            ("tone", "daily", "b", reference.as_str()),
            ("personality", "comfort", "b", reference.as_str()),
            ("tone", "comfort", "b", "stale-reference"),
            ("tone", "comfort", "a", reference.as_str()),
        ] {
            assert!(!ev.propose_revision(kind, scope, "rewritten", "reason", evidence(id, 2.0),
                false, 2.0 * 86400.0, Some(target)));
        }
        assert_eq!(ev.candidates.len(), 1);
        assert_eq!(ev.candidates[0].text, "listen first");
        assert_eq!(ev.candidates[0].support, 1);
    }
    #[test]
    fn active_revision_preserves_history_and_explicit_correction_wins() {
        let mut ev = PersonaEvolution::empty();
        add(&mut ev, "a", 1.0, "old", false);
        add(&mut ev, "b", 2.0, "old", false);
        assert!(!add(&mut ev, "c", 3.0, "new", false));
        assert_eq!(ev.entries[0].text, "old", "one new event cannot rewrite active behavior");
        assert!(add(&mut ev, "d", 4.0, "new", false));
        assert_eq!(ev.history[0].text, "old");
        assert!(add(&mut ev, "e", 4.0, "user correction", true));
        assert_eq!(ev.entries[0].text, "user correction");
        assert_eq!(ev.history.len(), 2);
    }
}
