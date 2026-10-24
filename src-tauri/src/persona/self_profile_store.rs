use std::path::PathBuf;
use parking_lot::RwLock;
use super::self_profile::{ExperienceSource, SelfAspect, SelfEvidence, SelfProfile};

/// Private per-character file, deliberately absent from frontend commands and events.
pub struct SelfProfileStore { inner: RwLock<SelfProfile>, path: PathBuf }
impl SelfProfileStore {
    pub fn new(path: PathBuf) -> Self {
        let profile = crate::utils::fs::load_json_or_backup(&path).unwrap_or_default();
        Self { inner: RwLock::new(profile), path }
    }
    pub fn fallback() -> Self { Self::new(PathBuf::new()) }
    fn commit(&self, guard: &mut SelfProfile, next: SelfProfile) -> bool {
        if !self.path.as_os_str().is_empty() {
            let result = serde_json::to_string_pretty(&next).map_err(std::io::Error::other)
                .and_then(|data| crate::utils::fs::write_atomic(&self.path, &data));
            if let Err(error) = result {
                tracing::warn!("Private self-profile persistence failed: {error}");
                return false;
            }
        }
        *guard = next;
        true
    }
    pub fn observe(&self, kind: SelfAspect, name: &str, reason: &str, evidence: SelfEvidence) -> bool {
        let mut guard = self.inner.write();
        // A reset invalidates pending web reflections, including those already in flight.
        if evidence.source == ExperienceSource::Web && guard.last_web_reflection != evidence.observed_at { return false; }
        let mut next = guard.clone();
        next.observe(kind, name, reason, evidence) && self.commit(&mut guard, next)
    }
    pub fn render(&self, now: f64) -> String { self.inner.read().render(now) }
    pub fn flush_for_backup(&self) -> Result<(), String> {
        if self.path.as_os_str().is_empty() {
            return Err("私人画像处于内存模式，无法生成完整备份".into());
        }
        let profile = self.inner.read();
        let json = serde_json::to_string_pretty(&*profile).map_err(|e| e.to_string())?;
        crate::utils::fs::write_atomic(&self.path, &json).map_err(|e| e.to_string())
    }
    pub fn reconcile_chat(&self, valid: impl Fn(&SelfEvidence) -> bool) {
        let mut guard = self.inner.write();
        let mut next = guard.clone();
        if next.reconcile_chat(valid) { self.commit(&mut guard, next); }
    }
    /// Reserve before the LLM call, including failures, to bound concurrent background work.
    pub fn reserve_web_reflection(&self, now: f64) -> bool {
        let mut guard = self.inner.write();
        if !now.is_finite() || now <= 0.0
            || (guard.last_web_reflection > 0.0 && now - guard.last_web_reflection < 1800.0) { return false; }
        let mut next = guard.clone();
        next.last_web_reflection = now;
        self.commit(&mut guard, next)
    }
    pub fn reset(&self) {
        let mut guard = self.inner.write();
        self.commit(&mut guard, SelfProfile::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persists_by_character_path_and_reserves_cooldown_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vivian-self.json");
        let store = SelfProfileStore::new(path.clone());
        assert!(store.reserve_web_reflection(86400.0));
        assert!(store.observe(SelfAspect::Interest, "星空摄影", "读后产生好奇", SelfEvidence {
            source:ExperienceSource::Web, source_id:"page1".into(), fingerprint:"hash1".into(),
            quote:"相机可以记录星空中的细节".into(), observed_at:86400.0,
        }));
        let reloaded = SelfProfileStore::new(path);
        reloaded.flush_for_backup().unwrap();
        assert!(reloaded.render(86400.0).contains("星空摄影"));
        assert!(!reloaded.reserve_web_reflection(86460.0));
        assert!(SelfProfileStore::new(dir.path().join("nana-self.json")).render(86400.0).is_empty());
        reloaded.reset();
        assert!(reloaded.render(86400.0).is_empty());
        assert!(!reloaded.observe(SelfAspect::Interest, "星空摄影", "过期的异步结果", SelfEvidence {
            source:ExperienceSource::Web, source_id:"page2".into(), fingerprint:"hash2".into(),
            quote:"相机可以记录星空中的细节".into(), observed_at:86400.0,
        }));
    }
    #[test]
    fn failed_write_does_not_apply_unpersisted_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = SelfProfileStore::new(dir.path().to_path_buf());
        assert!(!store.reserve_web_reflection(86400.0));
        assert!(store.flush_for_backup().is_err());
        assert_eq!(store.inner.read().last_web_reflection, 0.0);
    }
}
