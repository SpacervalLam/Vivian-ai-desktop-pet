//! User-specific matching phrases grounded in successful host tool receipts.
//! Corpus text is retrieval data only; it never grants execution permission.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use super::ToolSystem;
use crate::memory::embedding::MemoryEmbeddingProvider;

const MAX_OBSERVATIONS: usize = 64;
const MAX_ENTRIES: usize = 256;
const MAX_PER_TOOL: usize = 16;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolObservation {
    pub id: String,
    pub text: String,
    pub tools: HashMap<String, u64>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct UsageCandidate {
    pub tool_name: String,
    pub utterance: String,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageEntry {
    pub tool_name: String,
    pub utterance: String,
    pub language: String,
    pub tool_fingerprint: u64,
    pub evidence_ids: Vec<String>,
    pub model_id: String,
    pub embedding: Vec<f32>,
}
#[derive(Default, Serialize, Deserialize)]
struct CorpusData {
    observations: Vec<ToolObservation>,
    entries: Vec<UsageEntry>,
    pending_turns: usize,
}
#[derive(Clone)]
pub struct EvolutionBatch {
    pub observations: Vec<ToolObservation>,
    pending_turns: usize,
}
pub struct ToolUsageCorpus {
    path: Option<PathBuf>,
    data: Mutex<CorpusData>,
    evolving: AtomicBool,
}
pub fn fingerprint(tool: &dyn super::types::Tool) -> u64 {
    crate::utils::fnv1a_64(&format!("{}\n{}", tool.description(), tool.parameters_schema()))
}
fn generic_ack(text: &str) -> bool {
    let normalized = text.trim().trim_end_matches(|c:char| c.is_ascii_punctuation() || "。，！？".contains(c)).to_lowercase();
    matches!(normalized.as_str(), "yes" | "no" | "ok" | "okay" | "thanks" | "好的" | "可以" | "好" | "嗯" | "お願い" | "いいよ")
}
fn valid_vector(v: &[f32], dim: usize) -> bool {
    v.len() == dim && !v.is_empty() && v.iter().all(|x| x.is_finite()) && v.iter().any(|x| *x != 0.0)
}
impl ToolUsageCorpus {
    /// Character chains and hot reloads share one owner per file, avoiding lost updates.
    pub fn shared(path: PathBuf) -> std::sync::Arc<Self> {
        static STORES: once_cell::sync::Lazy<Mutex<HashMap<PathBuf, std::sync::Weak<ToolUsageCorpus>>>>
            = once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));
        let mut stores = STORES.lock();
        stores.retain(|_, store| store.strong_count() > 0);
        if let Some(store) = stores.get(&path).and_then(std::sync::Weak::upgrade) { return store; }
        let store = std::sync::Arc::new(Self::new(Some(path.clone())));
        stores.insert(path, std::sync::Arc::downgrade(&store));
        store
    }
    pub fn new(path: Option<PathBuf>) -> Self {
        let data = path.as_ref().and_then(|p| crate::utils::fs::load_json_or_backup(p)).unwrap_or_default();
        Self { path, data: Mutex::new(data), evolving: AtomicBool::new(false) }
    }
    fn save(&self, data: &CorpusData) {
        if let Some(path) = &self.path {
            match serde_json::to_vec(data).map_err(|e| e.to_string()).and_then(|bytes|
                crate::utils::fs::atomic_write(path, &bytes).map_err(|e| e.to_string())) {
                Ok(()) => {}, Err(e) => tracing::warn!("Tool usage corpus persistence failed: {e}"),
            }
        }
    }
    /// Each call is one real user turn, not one tool invocation. Never learn failures or drafts.
    pub fn observe(&self, text: &str, names: &[String], system: &ToolSystem) {
        let spans = crate::memory::redact::detect_pii(text);
        if generic_ack(text) || !spans.is_empty() || text.trim().chars().count() < 3 { return; }
        let tools: HashMap<_, _> = names.iter().filter(|n| n.as_str() != "tool_search")
            .filter_map(|name| system.find_tool(name).map(|t| (t.name().to_owned(), fingerprint(t.as_ref())))).collect();
        if tools.is_empty() { return; }
        let mut data = self.data.lock();
        data.observations.push(ToolObservation { id: uuid::Uuid::new_v4().to_string(),
            text: crate::utils::truncate_chars(text.trim(), 300), tools });
        if data.observations.len() > MAX_OBSERVATIONS { data.observations.remove(0); }
        data.pending_turns = (data.pending_turns + 1).min(MAX_OBSERVATIONS);
        self.save(&data);
    }
    /// A bounded background summary every four successful turns, with at least one repeated tool.
    pub fn begin_evolution(&self) -> Option<EvolutionBatch> {
        let data = self.data.lock();
        if data.pending_turns < 4 { return None; }
        let mut counts = HashMap::new();
        for observation in data.observations.iter().rev().take(24) {
            for tool in observation.tools.keys() { *counts.entry(tool).or_insert(0) += 1; }
        }
        if !counts.values().any(|count| *count >= 2)
            || self.evolving.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() { return None; }
        Some(EvolutionBatch { observations: data.observations.iter().rev().take(24).cloned().collect(), pending_turns: data.pending_turns })
    }
    pub fn finish_evolution(&self, batch: &EvolutionBatch) {
        let mut data = self.data.lock();
        data.pending_turns = data.pending_turns.saturating_sub(batch.pending_turns);
        self.save(&data);
        self.evolving.store(false, Ordering::SeqCst);
    }
    pub fn learn(&self, candidates: Vec<UsageCandidate>, batch: &EvolutionBatch,
        provider: &dyn MemoryEmbeddingProvider, system: &ToolSystem, language: &str) -> usize {
        let observations: HashMap<_, _> = batch.observations.iter().map(|o| (o.id.as_str(), o)).collect();
        let mut accepted = Vec::new();
        for candidate in candidates.into_iter().take(16) {
            let Some(tool) = system.find_tool(&candidate.tool_name) else { continue; };
            if tool.name() == "tool_search" { continue; }
            let text = candidate.utterance.trim().to_owned();
            if generic_ack(&text) || !(3..=160).contains(&text.chars().count()) || !crate::memory::redact::detect_pii(&text).is_empty() { continue; }
            let version = fingerprint(tool.as_ref());
            let ids: HashSet<_> = candidate.evidence_ids.iter().collect();
            if ids.len() < 2 || ids.iter().any(|id| observations.get(id.as_str())
                .is_none_or(|o| o.tools.get(tool.name()) != Some(&version))) { continue; }
            if self.data.lock().entries.iter().any(|e| e.tool_name == tool.name()
                && e.language == language && e.utterance == text && e.tool_fingerprint == version) { continue; }
            let texts = std::iter::once(text.clone()).chain(ids.iter().map(|id|
                observations[id.as_str()].text.clone())).collect::<Vec<_>>();
            let Ok(vectors) = provider.embed_batch(&texts) else { continue; };
            if vectors.len() != texts.len() { continue; }
            let embedding = vectors[0].clone();
            if !valid_vector(&embedding, provider.dimension()) { continue; }
            // Generalized wording must still match two observed user turns.
            let grounded = vectors[1..].iter().filter(|v| valid_vector(v, provider.dimension())
                && super::semantic_filter::cosine_similarity(&embedding, v) >= 0.45).count();
            if grounded < 2 { continue; }
            accepted.push(UsageEntry { tool_name: tool.name().into(), utterance: text, language: language.into(),
                tool_fingerprint: version, evidence_ids: ids.into_iter().cloned().collect(),
                model_id: provider.index_identity(), embedding });
        }
        let mut data = self.data.lock();
        let mut added = 0;
        for entry in accepted {
            if data.entries.iter().any(|e| e.tool_name == entry.tool_name && e.utterance == entry.utterance
                && e.language == entry.language && e.tool_fingerprint == entry.tool_fingerprint) { continue; }
            while data.entries.iter().filter(|e| e.tool_name == entry.tool_name).count() >= MAX_PER_TOOL {
                let index = data.entries.iter().position(|e| e.tool_name == entry.tool_name).unwrap(); data.entries.remove(index);
            }
            data.entries.push(entry); added += 1;
            if data.entries.len() > MAX_ENTRIES { data.entries.remove(0); }
        }
        if added > 0 { self.save(&data); }
        added
    }
    /// The escape-hatch keyword index uses the same learned phrases, without embedding I/O.
    pub fn search_hints(&self, tool: &dyn super::types::Tool) -> String {
        let version = fingerprint(tool);
        self.data.lock().entries.iter().filter(|entry| entry.tool_name == tool.name()
            && entry.tool_fingerprint == version).map(|entry| entry.utterance.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Rebuild learned vectors after an embedding-model switch, preserving supported text.
    /// All network I/O runs on a blocking worker, outside the corpus lock.
    pub fn embeddings(&self, provider: &dyn MemoryEmbeddingProvider, system: &ToolSystem, language: &str) -> Vec<UsageEntry> {
        let entries = self.data.lock().entries.clone();
        let mut updates = Vec::new();
        let mut result = Vec::new();
        let mut pending = Vec::new();
        let identity = provider.index_identity();
        for entry in entries {
            if entry.language != language { continue; }
            let Some(tool) = system.find_tool(&entry.tool_name) else { continue; };
            if fingerprint(tool.as_ref()) != entry.tool_fingerprint { continue; }
            if entry.model_id != identity || !valid_vector(&entry.embedding, provider.dimension()) {
                pending.push(entry);
            } else { result.push(entry); }
        }
        if !pending.is_empty() {
            let texts = pending.iter().map(|entry|entry.utterance.clone()).collect::<Vec<_>>();
            if let Ok(vectors) = provider.embed_batch(&texts) {
                if vectors.len() == pending.len() {
                    for (mut entry, vector) in pending.into_iter().zip(vectors) {
                        if !valid_vector(&vector, provider.dimension()) { continue; }
                        entry.embedding = vector; entry.model_id = identity.clone();
                        updates.push(entry.clone()); result.push(entry);
                    }
                }
            }
        }
        if !updates.is_empty() {
            let mut data = self.data.lock();
            for update in updates {
                if let Some(entry) = data.entries.iter_mut().find(|e| e.tool_name == update.tool_name
                    && e.utterance == update.utterance && e.language == update.language
                    && e.tool_fingerprint == update.tool_fingerprint) { *entry = update; }
            }
            self.save(&data);
        }
        result
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::super::semantic_filter::tests::{fixture_system, FixtureEmbedding, FixtureTool};
    use std::sync::Arc;
    fn setup(corpus: &ToolUsageCorpus) -> (ToolSystem, EvolutionBatch) {
        let system = fixture_system();
        for _ in 0..4 { corpus.observe("nickname", &["photo_tool".into()], &system); }
        let batch = corpus.begin_evolution().unwrap();
        (system, batch)
    }
    fn candidate(batch: &EvolutionBatch) -> UsageCandidate {
        UsageCandidate { tool_name:"photo_tool".into(), utterance:"nickname".into(),
            evidence_ids:batch.observations.iter().take(2).map(|o|o.id.clone()).collect() }
    }
    #[test]
    fn unsupported_repeated_ids_unrelated_phrases_and_secrets_are_rejected() {
        let corpus = ToolUsageCorpus::new(None); let (system,batch) = setup(&corpus);
        let provider = FixtureEmbedding {model:"one"};
        let mut invalid = candidate(&batch); invalid.evidence_ids[1] = invalid.evidence_ids[0].clone();
        assert_eq!(corpus.learn(vec![invalid], &batch, &provider, &system, "en"),0);
        let mut invalid = candidate(&batch); invalid.tool_name = "code_tool".into();
        assert_eq!(corpus.learn(vec![invalid], &batch, &provider, &system, "en"),0);
        let mut invalid = candidate(&batch); invalid.evidence_ids[0] = "invented".into();
        assert_eq!(corpus.learn(vec![invalid], &batch, &provider, &system, "en"),0);
        let mut invalid = candidate(&batch); invalid.utterance = "code".into();
        assert_eq!(corpus.learn(vec![invalid], &batch, &provider, &system, "en"),0);
        let mut invalid = candidate(&batch); invalid.utterance = "alice@example.com".into();
        assert_eq!(corpus.learn(vec![invalid], &batch, &provider, &system, "en"),0);
        assert!(corpus.begin_evolution().is_none()); // one learner at a time
        corpus.finish_evolution(&batch);
        assert!(corpus.begin_evolution().is_none()); // bounded retries, no loop on old evidence
    }
    #[test]
    fn corpus_survives_restart_deduplicates_rebuilds_models_and_invalidates_changed_tools() {
        let directory = std::env::temp_dir().join(format!("vivian-corpus-test-{}",uuid::Uuid::new_v4()));
        let path = directory.join("corpus.json");
        let corpus = ToolUsageCorpus::new(Some(path.clone())); let (system,batch) = setup(&corpus);
        let provider = FixtureEmbedding {model:"one"};
        assert_eq!(corpus.learn(vec![candidate(&batch),candidate(&batch)], &batch, &provider, &system, "en"),1);
        corpus.finish_evolution(&batch);
        let restored = ToolUsageCorpus::new(Some(path.clone()));
        assert_eq!(restored.embeddings(&provider,&system,"en").len(),1);
        assert!(restored.embeddings(&provider,&system,"ja").is_empty());
        let switched = FixtureEmbedding {model:"two"};
        assert_eq!(restored.embeddings(&switched,&system,"en")[0].model_id,"two:3:false");
        assert_eq!(ToolUsageCorpus::new(Some(path.clone())).embeddings(&switched,&system,"en")[0].model_id,"two:3:false");
        let changed = ToolSystem::new();
        changed.register_tool(Arc::new(FixtureTool {name:"photo_tool",description:"changed code behavior"}));
        assert!(restored.embeddings(&switched,&changed,"en").is_empty());
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn multiple_character_chains_share_one_corpus_owner() {
        let directory = std::env::temp_dir().join(format!("vivian-shared-corpus-{}",uuid::Uuid::new_v4()));
        let path = directory.join("corpus.json");
        let first = ToolUsageCorpus::shared(path.clone()); let second = ToolUsageCorpus::shared(path.clone());
        assert!(Arc::ptr_eq(&first,&second));
        let system = fixture_system();
        for _ in 0..2 {
            first.observe("nickname", &["photo_tool".into()], &system);
            second.observe("nickname", &["photo_tool".into()], &system);
        }
        let batch = second.begin_evolution().unwrap(); assert_eq!(batch.observations.len(),4);
        assert!(first.begin_evolution().is_none()); second.finish_evolution(&batch);
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn acknowledgments_unknown_tools_and_private_text_do_not_create_observations() {
        let corpus = ToolUsageCorpus::new(None); let system = fixture_system();
        for text in ["yes", "okay!", "alice@example.com"] {
            corpus.observe(text, &["photo_tool".into()], &system);
        }
        corpus.observe("nickname", &["missing".into()], &system);
        assert!(corpus.data.lock().observations.is_empty());
        assert!(corpus.begin_evolution().is_none());
    }
}
