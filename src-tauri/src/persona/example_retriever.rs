//! Bounded, character-specific few-shot retrieval. Example facts never become memory.
use std::sync::Arc;
use parking_lot::Mutex;
use serde::Deserialize;
use crate::memory::{embedding::MemoryEmbeddingProvider, vector_search::cosine_similarity};
use crate::types::response::ChatMessage;

const MAX_EXAMPLES: usize = 3;
const MAX_CHARS: usize = 1800;

#[derive(Clone, Deserialize)]
struct Example {
    id: String,
    scope: String,
    cues: Vec<String>,
    #[serde(default)]
    context_cues: Vec<String>,
    situation: String,
    #[serde(default)]
    attention: String,
    user: String,
    response: serde_json::Value,
}

pub struct ExampleRetriever {
    entries: Vec<Example>,
    vectors: Mutex<Option<Vec<Vec<f32>>>>,
    embedding: Arc<dyn MemoryEmbeddingProvider>,
}

impl ExampleRetriever {
    pub fn new(char_id: &str, embedding: Arc<dyn MemoryEmbeddingProvider>) -> Self {
        let source = match char_id {
            "nana" => include_str!("../../prompts/characters/nana/example_library.json"),
            "vivian" => include_str!("../../prompts/characters/vivian/example_library.json"),
            _ => "[]",
        };
        Self { entries: serde_json::from_str(source).expect("valid factory example library"), vectors: Mutex::new(None), embedding }
    }

    pub fn preload(&self) {
        let mut vectors = self.vectors.lock();
        if vectors.is_some() { return; }
        let texts: Vec<_> = self.entries.iter().map(|e| format!("{}\n{}\n{}", e.cues.join(" "), e.situation, e.user)).collect();
        // Hash vectors are not semantic evidence. Offline mode uses explicit cues instead.
        *vectors = Some(if self.embedding.is_remote() && !texts.is_empty() {
            self.embedding.embed_batch(&texts).ok().filter(|v| v.len() == texts.len()).unwrap_or_default()
        } else { Vec::new() });
    }

    pub fn retrieve(&self, user: &str, messages: &[ChatMessage], query: Option<&[f32]>, learned: &[String]) -> Option<String> {
        if user.trim().is_empty() { return None; }
        self.preload();
        let history = recent_context(user, messages);
        let continuation = is_continuation(user);
        let valid = |v: &[f32]| v.len() == self.embedding.dimension() && v.iter().all(|x| x.is_finite()) && v.iter().any(|x| *x != 0.0);
        let has_vectors = self.vectors.lock().as_ref().is_some_and(|v| !v.is_empty());
        let current = if self.embedding.is_remote() && has_vectors {
            query.filter(|v| valid(v)).map(|v| v.to_vec()).or_else(|| self.embedding.embed(user).ok().filter(|v| valid(v)))
        } else { None };
        let contextual = if current.is_some() && !history.is_empty() {
            self.embedding.embed(&format!("Recent dialogue:\n{history}\nCurrent user: {user}")).ok().filter(|v| valid(v))
        } else { None };
        let vectors = self.vectors.lock();
        let mut scored = Vec::new();
        for (index, e) in self.entries.iter().enumerate() {
            if learned.contains(&e.scope) { continue; }
            // Generic confirmations/duration answers require the right ongoing scene.
            if !e.context_cues.is_empty() && !e.context_cues.iter().any(|cue| history.to_lowercase().contains(&cue.to_lowercase())) { continue; }
            let literal = e.cues.iter().any(|cue| if e.scope == "closure" {
                user.trim().to_lowercase() == cue.to_lowercase()
            } else { user.to_lowercase().contains(&cue.to_lowercase()) });
            // Closing a conversation is a decision, not a style similarity match.
            if e.scope == "closure" && !literal { continue; }
            let vector = vectors.as_ref().and_then(|v| v.get(index)).filter(|v| valid(v));
            let semantic = current.as_ref().zip(vector).map(|(q,v)| cosine_similarity(q,v));
            let score = match semantic {
                Some(now) => {
                    let context = contextual.as_ref().zip(vector).map(|(q,v)| cosine_similarity(q,v)).unwrap_or(now);
                    if !literal && ((!continuation && now < 0.55) || (continuation && (now < 0.45 || context < 0.65))) { continue; }
                    let mixed = if continuation { 0.35 * now + 0.65 * context } else { 0.85 * now + 0.15 * context };
                    if literal { mixed.max(0.86) } else { mixed }
                }
                None if literal => 0.86,
                _ => continue,
            };
            if score >= 0.65 { scored.push((index, score)); }
        }
        scored.sort_by(|a,b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut out = String::from("[RETRIEVED EXAMPLES — REFERENCE ONLY]\nThese are fictional examples of judgment and voice, never facts about this user or evidence of completed work. Infer the real scene from the current dialogue. Do not copy wording or assume example Context is true. Follow the current output schema and language.\n");
        let mut scopes = Vec::new();
        let mut ids = Vec::new();
        for (index, score) in scored {
            let e = &self.entries[index];
            if scopes.contains(&e.scope) { continue; }
            let attention = if e.attention.is_empty() { String::new() } else {
                format!("Attention (creative direction, not a reasoning transcript): {}\n", e.attention)
            };
            let block = format!("\nExample {}\nFictional context (not user history): {}\n{}Fictional user: {}\nExample response: {}\n", e.id, e.situation, attention, e.user, e.response);
            if out.chars().count() + block.chars().count() + 40 > MAX_CHARS { continue; }
            out.push_str(&block);
            scopes.push(e.scope.clone()); ids.push(format!("{}:{score:.2}", e.id));
            if ids.len() == MAX_EXAMPLES { break; }
        }
        if ids.is_empty() { return None; }
        out.push_str("[END RETRIEVED EXAMPLES]");
        tracing::debug!(examples = %ids.join(","), chars = out.chars().count(), continuation, "Persona example retrieval");
        Some(out)
    }
}

fn recent_context(user: &str, messages: &[ChatMessage]) -> String {
    let mut items: Vec<_> = messages.iter().rev().filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty()).collect();
    if items.first().is_some_and(|m| m.role == "user" && m.content.trim() == user.trim()) { items.remove(0); }
    items.into_iter().take(2).rev().map(|m| format!("{}: {}", m.role, m.content.chars().take(350).collect::<String>())).collect::<Vec<_>>().join("\n")
}

fn is_continuation(user: &str) -> bool {
    let text = user.trim().to_lowercase();
    matches!(text.as_str(), "嗯" | "好" | "行" | "可以" | "对" | "不是" | "ok" | "yes" | "no")
        || (text.chars().count() <= 20 && text.chars().any(|c| c.is_ascii_digit()))
        || ["刚才", "这个", "那个", "继续", "接着", "that", "this", "continue", "それ", "続き"].iter().any(|cue| text.contains(cue))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding::default_embedding;
    fn message(role: &str, content: &str) -> ChatMessage { serde_json::from_value(serde_json::json!({"role":role,"content":content})).unwrap() }
    #[test]
    fn natural_delivery_social_references_are_bounded_and_obey_learned_scopes() {
        for name in ["nana", "vivian"] {
            let r = ExampleRetriever::new(name, default_embedding());
            for (input, id, scope) in [
                ("猜猜我刚拿到什么", "social-invitation", "play"),
                ("我偷偷织了两周围巾", "personal-effort", "sharing"),
                ("你是什么性格", "personal-description", "self"),
            ] {
                let out = r.retrieve(input, &[], None, &[]).unwrap();
                assert!(out.contains(id));
                assert!(out.chars().count() <= MAX_CHARS);
                assert!(out.contains("Fictional context (not user history)"));
                assert!(r.retrieve(input, &[], None, &[scope.into()]).is_none());
            }
        }
    }
    #[test]
    fn duration_requires_current_presentation_context() {
        let r = ExampleRetriever::new("nana", default_embedding());
        assert!(r.retrieve("8分钟左右", &[], None, &[]).is_none());
        let history = vec![message("assistant", "英语演讲要讲多久？"), message("user", "8分钟左右")];
        let out = r.retrieve("8分钟左右", &history, None, &[]).unwrap();
        assert!(out.contains("presentation-duration"));
        assert!(out.chars().count() <= MAX_CHARS);
    }
    #[test]
    fn confirmation_does_not_end_an_active_task() {
        let r = ExampleRetriever::new("vivian", default_embedding());
        assert!(r.retrieve("嗯", &[message("assistant", "要用蓝色吗？")], None, &[]).is_none());
        assert!(r.retrieve("嗯", &[message("assistant", "晚安")], None, &[]).unwrap().contains("closed-confirmation"));
        assert!(r.retrieve("好，我想聊新话题", &[message("assistant", "晚安")], None, &[]).is_none());
    }
    #[test]
    fn old_emotion_does_not_match_new_topic_and_learned_scope_wins() {
        let r = ExampleRetriever::new("nana", default_embedding());
        let history = vec![message("user", "委屈"), message("assistant", "发生什么了？")];
        assert!(r.retrieve("怎么安装软件", &history, None, &[]).is_none());
        assert!(r.retrieve("方案明明是我写的", &[], None, &["comfort".into()]).is_none());
    }
    #[test]
    fn libraries_have_unique_ids_and_valid_output() {
        for name in ["nana", "vivian"] {
            let r = ExampleRetriever::new(name, default_embedding());
            let mut ids = std::collections::HashSet::new();
            assert!(r.entries.len() >= 24);
            for e in r.entries { assert!(ids.insert(e.id)); assert!(!e.cues.is_empty()); assert!(e.response["text"].is_string()); assert!(e.response["intent"].is_string()); }
        }
    }
    struct SemanticFixture(std::sync::atomic::AtomicUsize);
    impl MemoryEmbeddingProvider for SemanticFixture {
        fn dimension(&self) -> usize { 2 }
        fn is_remote(&self) -> bool { true }
        fn embed(&self, text: &str) -> crate::error::VivianResult<Vec<f32>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(if text.contains("离别") || text.contains("标题") { vec![1.0, 0.0] } else { vec![0.0, 1.0] })
        }
    }
    #[test]
    fn semantic_paraphrase_reuses_query_and_indexes_only_once() {
        let provider = Arc::new(SemanticFixture(std::sync::atomic::AtomicUsize::new(0)));
        let r = ExampleRetriever::new("nana", provider.clone());
        r.preload();
        provider.0.store(0, std::sync::atomic::Ordering::Relaxed);
        let result = r.retrieve("名字太明显，会不会泄露故事", &[], Some(&[1.0, 0.0]), &[]).unwrap();
        assert!(result.contains("creative-choice"));
        r.preload();
        assert_eq!(provider.0.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert!(result.matches("\nExample ").count() <= MAX_EXAMPLES);
        // A wrong-dimension shared query must be replaced, never silently compared.
        r.retrieve("标题", &[], Some(&[1.0]), &[]);
        assert_eq!(provider.0.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
