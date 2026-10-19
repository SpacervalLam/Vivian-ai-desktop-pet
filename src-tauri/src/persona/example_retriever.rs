//! Bounded, character-specific few-shot retrieval. Example facts never become memory.
//! Context gates precede relevance ranking; conversation-local repetition only
//! breaks near ties. Selection is deterministic and never mutates session state.
use std::{collections::BTreeSet, sync::Arc};
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
        let normalized_user = user.trim().to_lowercase();
        let normalized_history = history.to_lowercase();
        // Derive recency from this conversation, not a shared mutable counter:
        // previews, retries and other sessions must not consume one another's examples.
        let recent_replies: Vec<_> = messages.iter().rev()
            .filter(|m| m.role == "assistant" && !m.content.trim().is_empty())
            .take(4).map(|m| response_terms(&m.content)).collect();
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
            if !e.context_cues.is_empty() && !e.context_cues.iter().any(|cue| cue_matches(&normalized_history, &cue.trim().to_lowercase())) { continue; }
            let literal_score = e.cues.iter().filter_map(|cue| {
                let cue = cue.trim().to_lowercase();
                if !cue_matches(&normalized_user, &cue) || (e.scope == "closure" && normalized_user != cue) { return None; }
                // Prefer a specific phrase over a generic substring. Mere cue count
                // would reward entries that happen to have more aliases.
                let coverage = cue.chars().count() as f64 / normalized_user.chars().count().max(1) as f64;
                Some(0.80 + 0.16 * coverage.min(1.0))
            }).max_by(f64::total_cmp);
            let literal = literal_score.is_some();
            // Closing a conversation is a decision, not a style similarity match.
            if e.scope == "closure" && !literal { continue; }
            let vector = vectors.as_ref().and_then(|v| v.get(index)).filter(|v| valid(v));
            let semantic = current.as_ref().zip(vector).map(|(q,v)| cosine_similarity(q,v));
            let score = match semantic {
                Some(now) => {
                    let context = contextual.as_ref().zip(vector).map(|(q,v)| cosine_similarity(q,v)).unwrap_or(now);
                    if !literal && ((!continuation && now < 0.55) || (continuation && (now < 0.45 || context < 0.65))) { continue; }
                    let mixed = if continuation { 0.35 * now + 0.65 * context } else { 0.85 * now + 0.15 * context };
                    if let Some(lexical) = literal_score { mixed.max(lexical) } else { mixed }
                }
                None if literal => literal_score.unwrap(),
                _ => continue,
            };
            if score >= 0.65 {
                let terms = response_terms(e.response["text"].as_str().unwrap_or_default());
                let repeated = recent_replies.iter().enumerate().map(|(age, reply)|
                    term_similarity(&terms, reply) / (age + 1) as f64).fold(0.0_f64, f64::max);
                // A small penalty breaks near ties without admitting irrelevant examples.
                scored.push((index, score - 0.06 * repeated, terms));
            }
        }
        scored.sort_by(|a,b| b.1.total_cmp(&a.1).then_with(||
            tie_key(user, &history, &self.entries[a.0].id).cmp(&tie_key(user, &history, &self.entries[b.0].id))));
        let mut out = String::from("[RETRIEVED EXAMPLES — REFERENCE ONLY]\nThese are fictional examples of judgment and voice, never facts about this user or evidence of completed work. Infer the real scene from the current dialogue. Do not copy wording or assume example Context is true. Follow the current output schema and language.\n");
        let mut scopes = Vec::new();
        let mut ids = Vec::new();
        let mut selected_terms = Vec::new();
        for (index, score, terms) in scored {
            let e = &self.entries[index];
            if scopes.contains(&e.scope) { continue; }
            // Different scopes can still contain essentially the same line.
            if selected_terms.iter().any(|chosen| term_similarity(&terms, chosen) >= 0.85) { continue; }
            let attention = if e.attention.is_empty() { String::new() } else {
                format!("Attention (creative direction, not a reasoning transcript): {}\n", e.attention)
            };
            let block = format!("\nExample {}\nFictional context (not user history): {}\n{}Fictional user: {}\nExample response: {}\n", e.id, e.situation, attention, e.user, e.response);
            if out.chars().count() + block.chars().count() + 40 > MAX_CHARS { continue; }
            out.push_str(&block);
            selected_terms.push(terms);
            scopes.push(e.scope.clone()); ids.push(format!("{}:{score:.2}", e.id));
            if ids.len() == MAX_EXAMPLES { break; }
        }
        if ids.is_empty() { return None; }
        out.push_str("[END RETRIEVED EXAMPLES]");
        tracing::debug!(examples = %ids.join(","), chars = out.chars().count(), continuation, "Persona example retrieval");
        Some(out)
    }
}

/// ASCII words/numbers require ASCII token boundaries ("hi" != "this").
/// Chinese/Japanese phrases can occur within a sentence; a single non-ASCII
/// character is too ambiguous unless it is the entire utterance.
fn cue_matches(text: &str, cue: &str) -> bool {
    if cue.is_empty() { return false; }
    if text.trim() == cue { return true; }
    if cue.chars().count() == 1 && !cue.is_ascii() { return false; }
    let token = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(cue).any(|(start, _)| {
        let end = start + cue.len();
        let left = !cue.chars().next().is_some_and(token)
            || !text[..start].chars().next_back().is_some_and(token);
        let right = !cue.chars().next_back().is_some_and(token)
            || !text[end..].chars().next().is_some_and(token);
        left && right
    })
}

/// Bounded surface overlap for repetition, never used as semantic evidence.
fn response_terms(text: &str) -> BTreeSet<String> {
    let lower: String = text.chars().take(350).collect::<String>().to_lowercase();
    let mut terms: BTreeSet<_> = lower.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty()).map(str::to_owned).collect();
    let chars: Vec<_> = lower.chars().collect();
    for pair in chars.windows(2) {
        if pair.iter().all(|c| !c.is_ascii() && c.is_alphanumeric()) {
            terms.insert(pair.iter().collect());
        }
    }
    terms
}

fn term_similarity(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 { 0.0 } else { a.intersection(b).count() as f64 / union as f64 }
}

fn tie_key(user: &str, history: &str, id: &str) -> u64 {
    // Stable FNV-1a: reproducible within a turn, varied with the conversation.
    [user, history, id].into_iter().fold(0xcbf29ce484222325_u64, |hash, part|
        part.bytes().chain(std::iter::once(0)).fold(hash, |h, byte|
            (h ^ byte as u64).wrapping_mul(0x100000001b3)))
}

fn recent_context(user: &str, messages: &[ChatMessage]) -> String {
    let mut items: Vec<_> = messages.iter().rev().filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty()).collect();
    if items.first().is_some_and(|m| m.role == "user" && m.content.trim() == user.trim()) { items.remove(0); }
    items.into_iter().take(2).rev().map(|m| format!("{}: {}", m.role, m.content.chars().take(350).collect::<String>())).collect::<Vec<_>>().join("\n")
}

fn is_continuation(user: &str) -> bool {
    crate::pipeline::message_context::is_short_followup(user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding::default_embedding;
    fn message(role: &str, content: &str) -> ChatMessage { serde_json::from_value(serde_json::json!({"role":role,"content":content})).unwrap() }
    fn fixture(entries: serde_json::Value) -> ExampleRetriever {
        let mut r = ExampleRetriever::new("test", default_embedding());
        r.entries = serde_json::from_value(entries).unwrap();
        r
    }
    fn example(id: &str, scope: &str, cue: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"id":id,"scope":scope,"cues":[cue],"situation":"Fictional scene",
            "user":cue,"response":{"text":text,"intent":"reply"}})
    }
    #[test]
    fn cues_respect_word_boundaries_and_single_character_ambiguity() {
        let r = fixture(serde_json::json!([
            example("hi", "hello", "hi", "Hello there"),
            example("ok", "confirm", "ok", "All right"),
            example("six", "slang", "6", "Nice work"),
            example("grass", "slang", "草", "What a surprise")
        ]));
        for input in ["this is a book", "token lookup", "16分钟", "草率决定"] {
            assert!(r.retrieve(input, &[], None, &[]).is_none(), "false cue in {input}");
        }
        for (input, id) in [("HI!", "hi"), ("OK，可以", "ok"), ("6!", "six"), ("草", "grass")] {
            assert!(r.retrieve(input, &[], None, &[]).unwrap().contains(&format!("Example {id}\n")));
        }
        assert!(!cue_matches("anything", ""));
        assert!(cue_matches("晚上下雨还得出门呢", "下雨还得出门"));
    }
    #[test]
    fn specific_cue_beats_generic_cue_even_after_repetition() {
        let r = fixture(serde_json::json!([
            example("generic", "care", "出门", "Take care out there"),
            example("rain", "care", "下雨还得出门", "Bring your umbrella")
        ]));
        let history = vec![message("assistant", "Bring your umbrella")];
        let out = r.retrieve("下雨还得出门", &history, None, &[]).unwrap();
        assert!(out.contains("Example rain\n"));
        assert!(!out.contains("Example generic\n"));
    }
    #[test]
    fn recent_reply_promotes_equally_relevant_alternative_without_shared_state() {
        let entries = serde_json::json!([
            example("a", "play", "猜猜", "Maybe a new game"),
            example("b", "play", "猜猜", "I bet it is a drawing")
        ]);
        let r = fixture(entries.clone());
        let first = r.retrieve("猜猜", &[], None, &[]).unwrap();
        assert_eq!(first, r.retrieve("猜猜", &[], None, &[]).unwrap(), "retries stay stable");
        let (used, alternative) = if first.contains("Example a\n") {
            ("Maybe a new game", "b")
        } else { ("I bet it is a drawing", "a") };
        let history = vec![message("assistant", used)];
        let next = r.retrieve("猜猜", &history, None, &[]).unwrap();
        assert!(next.contains(&format!("Example {alternative}\n")));
        assert_eq!(first, r.retrieve("猜猜", &[], None, &[]).unwrap(), "another session is independent");
        assert_eq!(next, fixture(entries).retrieve("猜猜", &history, None, &[]).unwrap(), "restart is reproducible");
    }
    #[test]
    fn repetition_never_removes_the_only_relevant_example_or_relaxes_gates() {
        let r = fixture(serde_json::json!([example("only", "care", "下雨", "Take an umbrella")]));
        let history = vec![message("assistant", "Take an umbrella")];
        assert!(r.retrieve("下雨", &history, None, &[]).unwrap().contains("Example only\n"));
        assert!(r.retrieve("新话题", &history, None, &[]).is_none());
        assert!(r.retrieve("下雨", &history, None, &["care".into()]).is_none());
    }
    #[test]
    fn duplicate_lines_across_scopes_do_not_fill_all_slots() {
        let r = fixture(serde_json::json!([
            example("a", "first", "猜猜", "Maybe a new game"),
            example("b", "second", "猜猜", "Maybe a new game!"),
            example("c", "third", "猜猜", "I bet it is a drawing")
        ]));
        let out = r.retrieve("猜猜", &[], None, &[]).unwrap();
        assert_eq!(out.lines().filter(|l| l.starts_with("Example response:")).count(), 2);
        assert!(out.contains("Example c\n"));
        assert!(out.chars().count() <= MAX_CHARS);
    }
    #[test]
    fn context_cues_also_respect_ascii_boundaries() {
        let mut e = example("duration", "task", "8分钟", "Eight minutes works");
        e["context_cues"] = serde_json::json!(["talk"]);
        let r = fixture(serde_json::json!([e]));
        assert!(r.retrieve("8分钟", &[message("assistant", "A stalk of celery")], None, &[]).is_none());
        assert!(r.retrieve("8分钟", &[message("assistant", "How long is the talk?")], None, &[]).is_some());
    }
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
    /// 网络口语语料（`net_*` scope）不变量。
    ///
    /// 这批语料在所有渠道生效，检索层不做 channel 门控，因此以下三点必须成立：
    /// 1. 短 cue 会经裸 `contains` 误命中（`6` 撞「16分钟」、`草` 撞「草率」），
    ///    所以每条 cue 至少 2 字。
    /// 2. 同一 scope 每轮只取一条，scope 太少会让这批语料互相挤掉。
    /// 3. `no_reply` 必须配空文本，否则模型会照着念出来。
    ///
    /// 注意：条数**不设等式约束**。语料库是持续增长的资产，写死 `assert_eq!(net.len(), N)`
    /// 会在每次扩充时失败，把「规模变化」误报成「质量回归」。真正该守的是
    /// 每条的 cue 质量与 scope 采样面，故下面用下限表达。
    #[test]
    fn net_corpus_cues_are_unambiguous_and_scopes_stay_distinct() {
        for name in ["nana", "vivian"] {
            let r = ExampleRetriever::new(name, default_embedding());
            let net: Vec<_> = r.entries.iter().filter(|e| e.scope.starts_with("net_")).collect();
            assert!(net.len() >= 36, "{name} 网络口语语料被删减到 {} 条", net.len());
            for e in &net {
                for cue in &e.cues {
                    assert!(cue.chars().count() >= 2, "{name}/{} 的 cue 过短: {cue}", e.id);
                }
                if e.response["intent"] == "no_reply" {
                    assert_eq!(e.response["text"], "", "{name}/{} 是 no_reply 但带文本", e.id);
                }
            }
            // 同 scope 每轮只取一条，所以采样面由 scope 数量决定而非条数。
            let scopes: std::collections::HashSet<_> = net.iter().map(|e| e.scope.as_str()).collect();
            assert!(
                scopes.len() >= 20,
                "{name} 的 net_ scope 只有 {} 个，采样面不足",
                scopes.len()
            );
        }
        // 同一 cue 下的两种人格必须给出不同反应，否则语料会把角色拉向同一个腔调。
        let vivian = ExampleRetriever::new("vivian", default_embedding());
        let nana = ExampleRetriever::new("nana", default_embedding());
        for (a, b) in vivian.entries.iter().zip(nana.entries.iter()) {
            assert_eq!(a.id, b.id);
            if a.id.starts_with("net-") && a.response["intent"] == "reply" {
                assert_ne!(a.response["text"], b.response["text"], "{} 两人格给了同一句回复", a.id);
            }
        }
    }
    /// 语料规模护栏。
    ///
    /// `MAX_EXAMPLES = 3` + `MAX_CHARS = 1800` 意味着单条回复必须够短，
    /// 否则三条并排注入时会挤掉第三条。语料条数本身不设上限——
    /// 有效采样面由 scope 数量决定（同 scope 每轮只取一条）。
    #[test]
    fn example_responses_stay_short_enough_for_the_char_budget() {
        for name in ["nana", "vivian"] {
            for e in ExampleRetriever::new(name, default_embedding()).entries {
                let n = e.response["text"].as_str().map(|s| s.chars().count()).unwrap_or(0);
                assert!(
                    n <= 120,
                    "{name}/{} 回复 {n} 字，过长会挤掉同轮其他示例（预算 {}）",
                    e.id,
                    MAX_CHARS
                );
            }
        }
    }
    #[test]
    fn modern_net_examples_reach_companion_prompt_without_remote_embeddings() {
        for name in ["nana", "vivian"] {
            let r = ExampleRetriever::new(name, default_embedding());
            let modern: Vec<_> = r.entries.iter().filter(|e| e.id.starts_with("net-modern-")).collect();
            assert_eq!(modern.len(), 24);
            for e in modern {
                assert!(e.context_cues.is_empty(), "{} must work without group history", e.id);
                for input in std::iter::once(&e.user).chain(e.cues.iter()) {
                    let examples = r.retrieve(input, &[], None, &[]).expect("offline cue retrieval");
                    assert!(examples.contains(&format!("Example {}\n", e.id)), "{name}/{} missing for {input}", e.id);
                    assert!(examples.chars().count() <= MAX_CHARS);
                    for channel in ["", "desktop", "private", "group"] {
                        let parts = crate::pipeline::prompt_modules::PromptParts {
                            char_id: name.into(), channel: channel.into(),
                            examples_block: Some(examples.clone()), ..Default::default()
                        };
                        let prompt = crate::pipeline::companion_prompt::CompanionPrompt::build(&parts, &[]);
                        let messages = prompt.messages(&[], input, false, None);
                        assert!(messages.iter().any(|m| m.role == "system" && m.content.contains(&format!("Example {}\n", e.id))), "{name}/{} missing in {channel} prompt", e.id);
                    }
                }
            }
        }
    }
    #[test]
    fn conversation_turn_examples_are_retrievable_without_remote_embeddings() {
        for name in ["vivian", "nana"] {
            let r = ExampleRetriever::new(name, default_embedding());
            for id in ["topic-hard-turn", "playful-babble", "scrambled-message", "serious-after-babble", "task-incomplete", "mixed-language-clear"] {
                let e = r.entries.iter().find(|e| e.id == id).unwrap();
                let history = vec![message("user", "明天工作好多"), message("assistant", "先做最急的那件吧")];
                for input in std::iter::once(&e.user).chain(e.cues.iter()) {
                    let result = r.retrieve(input, &history, None, &[]).unwrap();
                    assert!(result.contains(&format!("Example {id}\n")), "{name}: {input}");
                    assert!(result.chars().count() <= MAX_CHARS);
                }
                assert!(e.context_cues.is_empty());
            }
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
        assert!(result.lines().filter(|line| line.starts_with("Example ") && !line.starts_with("Example response:")).count() <= MAX_EXAMPLES);
        // A wrong-dimension shared query must be replaced, never silently compared.
        r.retrieve("标题", &[], Some(&[1.0]), &[]);
        assert_eq!(provider.0.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
