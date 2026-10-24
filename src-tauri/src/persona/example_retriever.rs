//! Bounded, character-specific few-shot retrieval. Example facts never become memory.
//! Context gates precede relevance ranking; conversation-local repetition only
//! breaks near ties. Selection is deterministic and never mutates session state.
//! Without a remote embedding, turns that miss every literal cue fall back to
//! bounded surface overlap — capped at one example per turn.
use std::{collections::BTreeSet, sync::Arc};
use super::embedding_snapshot::EmbeddingSnapshot;
use super::example_library::{self, VoiceExample as Example, ExampleOverride};
use parking_lot::RwLock;
use crate::memory::{embedding::MemoryEmbeddingProvider, vector_search::cosine_similarity};
use crate::types::response::ChatMessage;

const MAX_EXAMPLES: usize = 3;
const MAX_CHARS: usize = 1800;

struct RevisedCorpus {
    key: String,
    entries: Vec<Example>,
    vectors: EmbeddingSnapshot,
    revised_ids: BTreeSet<String>,
}

pub struct ExampleRetriever {
    entries: Vec<Example>,
    vectors: EmbeddingSnapshot,
    embedding: Arc<dyn MemoryEmbeddingProvider>,
    char_id: String,
    revised: RwLock<Option<Arc<RevisedCorpus>>>,
}

impl ExampleRetriever {
    pub fn new(char_id: &str, embedding: Arc<dyn MemoryEmbeddingProvider>) -> Self {
        let entries = example_library::base_examples(char_id);
        let texts = entries.iter().map(|e| format!("{}\n{}\n{}", e.cues.join(" "), e.situation, e.user)).collect();
        let vectors = EmbeddingSnapshot::new(embedding.clone(), format!("examples_{char_id}"), texts);
        Self { entries, vectors, embedding, char_id: char_id.into(), revised: RwLock::new(None) }
    }

    pub(crate) fn request_preload(&self) { self.vectors.request(); }
    #[cfg(test)]
    pub fn preload(&self) { self.vectors.preload(); }

    pub fn retrieve(&self, user: &str, messages: &[ChatMessage], query: Option<&[f32]>, learned: &[String], lang: &str) -> Option<String> {
        self.retrieve_from(user, messages, query, learned, &self.entries, &self.vectors, &BTreeSet::new(), lang)
    }

    pub fn retrieve_growing(&self, user: &str, messages: &[ChatMessage], query: Option<&[f32]>,
        learned: &[(String, String)], overrides: &[ExampleOverride], lang: &str) -> Option<String> {
        let scopes: Vec<_> = learned.iter().map(|(scope, _)| scope.clone()).collect();
        let active: Vec<_> = overrides.iter().filter(|update| update.applies(learned)).cloned().collect();
        if active.is_empty() { return self.retrieve(user, messages, query, &scopes, lang); }
        let key = example_library::fingerprint(&active);
        let cached = self.revised.read().as_ref().filter(|snapshot| snapshot.key == key).cloned();
        let snapshot = cached.unwrap_or_else(|| {
            let entries = example_library::materialize(&self.entries, &active, learned);
            let texts = entries.iter().map(|e| format!("{}\n{}\n{}", e.cues.join(" "), e.situation, e.user)).collect();
            let vectors = EmbeddingSnapshot::new(self.embedding.clone(), format!("examples_{}_growing", self.char_id), texts);
            let snapshot = Arc::new(RevisedCorpus { key, entries, vectors,
                revised_ids: active.iter().filter(|update| update.edit.example.is_some()).map(|update| update.edit.id.clone()).collect() });
            *self.revised.write() = Some(snapshot.clone());
            snapshot
        });
        self.retrieve_from(user, messages, query, &scopes, &snapshot.entries, &snapshot.vectors, &snapshot.revised_ids, lang)
    }

    fn retrieve_from(&self, user: &str, messages: &[ChatMessage], query: Option<&[f32]>, learned: &[String],
        entries: &[Example], snapshot: &EmbeddingSnapshot, revised_ids: &BTreeSet<String>, lang: &str) -> Option<String> {
        if user.trim().is_empty() || source_transformation(user) { return None; }
        snapshot.request();
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
        let vectors = snapshot.get();
        let has_vectors = vectors.as_ref().is_some_and(|v| !v.is_empty());
        let current = if self.embedding.is_remote() && has_vectors {
            query.filter(|v| valid(v)).map(|v| v.to_vec()).or_else(|| self.embedding.embed(user).ok().filter(|v| valid(v)))
        } else { None };
        let contextual = if current.is_some() && !history.is_empty() {
            self.embedding.embed(&format!("Recent dialogue:\n{history}\nCurrent user: {user}")).ok().filter(|v| valid(v))
        } else { None };
        let mut scored = Vec::new();
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        for (index, e) in entries.iter().enumerate() {
            if learned.contains(&e.scope) && !revised_ids.contains(&e.id) { continue; }
            // 语种门：三语示例混在同一库文件里，检索必须按界面语言选档。
            // 不过滤时，配了远程多语 embedding 的中文对话会召回 style-en/ja 示例
            // （跨语言相似度过准入线），而英文/日文界面更是一直在吃中文示例——
            // 模型会直接跟着示例的语言说话。2026-10-24 补上这道门。
            let e_lang = entry_lang(e);
            if e_lang != "any" && e_lang != lang_norm { continue; }
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
            let mut from_fallback = false;
            let score = match semantic {
                Some(now) => {
                    let context = contextual.as_ref().zip(vector).map(|(q,v)| cosine_similarity(q,v)).unwrap_or(now);
                    if !literal && ((!continuation && now < 0.55) || (continuation && (now < 0.45 || context < 0.65))) { continue; }
                    let mixed = if continuation { 0.35 * now + 0.65 * context } else { 0.85 * now + 0.15 * context };
                    if let Some(lexical) = literal_score { mixed.max(lexical) } else { mixed }
                }
                None if literal => literal_score.unwrap(),
                // Offline with no literal cue hit: bounded surface overlap stands in
                // for the missing semantic channel. Hash vectors are deliberately
                // never treated as similarity evidence; see lexical_fallback_score.
                None => match lexical_fallback_score(&normalized_user, e, continuation) {
                    Some(fuzzy) => { from_fallback = true; fuzzy }
                    None => continue,
                },
            };
            if score >= 0.65 {
                let terms = response_terms(e.response["text"].as_str().unwrap_or_default());
                let repeated = recent_replies.iter().enumerate().map(|(age, reply)|
                    term_similarity(&terms, reply) / (age + 1) as f64).fold(0.0_f64, f64::max);
                // A small penalty breaks near ties without admitting irrelevant examples.
                scored.push((index, score - 0.06 * repeated, terms, from_fallback));
            }
        }
        scored.sort_by(|a,b| b.1.total_cmp(&a.1).then_with(||
            tie_key(user, &history, &entries[a.0].id).cmp(&tie_key(user, &history, &entries[b.0].id))));
        let mut out = String::from("[RETRIEVED EXAMPLES — REFERENCE ONLY]\nThese are fictional examples of judgment and voice, never facts about this user or evidence of completed work. Infer the real scene from the current dialogue. Do not copy wording or assume example Context is true. Follow the current output schema and language.\n");
        let mut scopes = Vec::new();
        let mut ids = Vec::new();
        let mut selected_terms = Vec::new();
        let mut fallback_used = false;
        for (index, score, terms, from_fallback) in scored {
            let e = &entries[index];
            if scopes.contains(&e.scope) { continue; }
            // Different scopes can still contain essentially the same line.
            if selected_terms.iter().any(|chosen| term_similarity(&terms, chosen) >= 0.85) { continue; }
            // A fuzzy match justifies one voice anchor, not a trio. Uncapped, it
            // fills slots 2-3 and same-scope precision@3 drops ~26 points.
            if from_fallback && fallback_used { continue; }
            if from_fallback { fallback_used = true; }
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

/// Quoted source material and code are not the user's emotional self-report.
pub(crate) fn source_transformation(input: &str) -> bool {
    let input = input.to_lowercase();
    if ["把", "请把", "帮我把", "能把", "可以把"].iter().any(|cue| input.starts_with(cue))
        && ["翻译成", "翻译为", "译成", "翻译一下", "润色一下", "改写成"].iter().any(|cue| input.contains(cue)) { return true; }
    input.split(['，', ',', '。', ';', '；', '\n']).any(|clause| {
        let clause = clause.trim();
        // Talking about a rewrite is different from requesting one.
        if ["润色后", "润色完", "改写后", "翻译后", "翻訳後"].iter().any(|cue| clause.starts_with(cue)) { return false; }
        let directive = ["翻译", "译成", "改写", "润色", "把", "请", "帮我", "麻烦", "能把", "可以把", "这句", "怎么",
            "translate ", "rewrite ", "please ", "can you ", "could you ", "how to "]
            .iter().any(|cue| clause.starts_with(cue));
        let transform = ["翻译", "译成", "translate ", "改写", "润色", "rewrite "]
            .iter().any(|cue| clause.contains(cue));
        let japanese_request = ["翻訳して", "訳して", "翻訳してください", "訳してください"].iter().any(|cue| clause.contains(cue));
        let code = ["代码", "code", "コード"].iter().any(|cue| clause.contains(cue));
        let analysis = ["分析", "解释", "检查", "修复", "debug", "review", "analyze", "analyse", "explain"];
        let analysis_request = directive || analysis.iter().any(|cue| clause.starts_with(cue));
        (directive && transform) || japanese_request || (code && analysis_request && analysis.iter().any(|cue| clause.contains(cue)))
    })
}

/// 示例条目的语种：先看显式 id 前缀（style-en-* / style-ja-*），否则按文本判定
/// （假名→ja，汉字→zh，纯 ASCII→en）。
///
/// 与 persona 的 examples.md 不同，example_library.json 是三语混排的单文件，
/// 消费侧必须自己选档——`resolve_section` 那套按语言解析的逻辑到不了这里。
fn entry_lang(entry: &Example) -> &'static str {
    // 生产约定：跨语言示例一律带 style-en-* / style-ja-* 前缀（few-shot-library
    // 测试强制 ≥12 条），前缀即权威标记。
    if entry.id.starts_with("style-en-") { return "en"; }
    if entry.id.starts_with("style-ja-") { return "ja"; }
    let text = format!("{}{}{}", entry.user, entry.situation,
        entry.response["text"].as_str().unwrap_or_default());
    if text.chars().any(|c| matches!(c, '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}')) { return "ja"; }
    if text.chars().any(|c| matches!(c, '\u{4E00}'..='\u{9FFF}')) { return "zh"; }
    // 纯 ASCII 没有任何文字证据：判为「中立」，不参与语种门。
    // 生产库里的英文条目前缀已覆盖；这条只放过语言无关的测试夹具。
    "any"
}

/// 离线模式下的表面重叠回退打分（无远程 embedding 且字面 cue 未命中时）。
///
/// 与 `response_terms` 的分工：那里要原始表面（重复惩罚），这里要内容词重叠
/// （场景匹配），因此过滤功能字二元组——「了我你」不构成场景证据。
///
/// 约束（阈值经 `tmp/human_feel_probe.mjs` 全库实测标定）：
/// - 准入线与语义通道对齐（0.55，continuation 0.62）：模糊证据不得比语义证据
///   更易通关——早期 0.42 档把「好，我想聊新话题」误塞进 pause-conversation；
/// - 分数区间 [0.60, 0.78]，恒低于字面命中下限 0.80，真实 cue 永远优先；
/// - `retrieve` 另限制回退候选每轮最多贡献 1 个范例。
fn lexical_fallback_score(user: &str, entry: &Example, continuation: bool) -> Option<f64> {
    const OVERLAP_THRESHOLD: f64 = 0.55;
    const CONTINUATION_OVERLAP_THRESHOLD: f64 = 0.62;
    const BASE: f64 = 0.60;
    const SPAN: f64 = 0.18;
    let user_terms = content_terms(user);
    if user_terms.len() < 3 { return None; }
    let mut surface = String::new();
    for cue in &entry.cues { surface.push_str(cue); surface.push('\n'); }
    surface.push_str(&entry.situation);
    surface.push('\n');
    surface.push_str(&entry.user);
    let entry_terms = content_terms(&surface);
    if entry_terms.is_empty() { return None; }
    let inter = user_terms.intersection(&entry_terms).count();
    if inter < 2 { return None; }
    let overlap = inter as f64 / user_terms.len().min(entry_terms.len()) as f64;
    let threshold = if continuation { CONTINUATION_OVERLAP_THRESHOLD } else { OVERLAP_THRESHOLD };
    if overlap < threshold { return None; }
    Some(BASE + SPAN * overlap)
}

/// 内容词表面集合：ASCII 词（长 ≥2）+ 过滤功能字后的 CJK 二元组。
fn content_terms(text: &str) -> BTreeSet<String> {
    const FUNCTION_CHARS: &str = "的了是在我你他她它们吗呢吧啊嗯哦呀和跟也就都还又被把给对而且或因所以于不没别太很";
    let lower: String = text.chars().take(350).collect::<String>().to_lowercase();
    let mut terms: BTreeSet<_> = lower.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.len() > 1).map(str::to_owned).collect();
    let chars: Vec<_> = lower.chars().collect();
    for pair in chars.windows(2) {
        if pair.iter().all(|c| !c.is_ascii() && c.is_alphanumeric())
            && !pair.iter().any(|c| FUNCTION_CHARS.contains(*c))
        {
            terms.insert(pair.iter().collect());
        }
    }
    terms
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
    fn diary_samples_refresh_in_place_and_follow_their_persona_version() {
        use super::example_library::{ExampleEdit, ExampleOverride};
        let r = fixture(serde_json::json!([example("old", "comfort", "难过", "原来的回应")]));
        let mut changed: Example = serde_json::from_value(example("old", "comfort", "难过", "现在先听你说")).unwrap();
        let mut update = ExampleOverride { scope:"comfort".into(), understanding:Some("先倾听".into()), updated_at:1.0,
            edit:ExampleEdit {id:"old".into(), reference:None, reason:"修订表达".into(), example:Some(changed.clone())} };
        let learned = vec![("comfort".into(), "先倾听".into())];
        let first = r.retrieve_growing("难过", &[], None, &learned, &[update.clone()], "zh").unwrap();
        assert!(first.contains("现在先听你说")); assert!(!first.contains("原来的回应"));
        changed.response["text"] = "想说的时候我都在".into();
        update.edit.example = Some(changed);
        let second = r.retrieve_growing("难过", &[], None, &learned, &[update.clone()], "zh").unwrap();
        assert!(second.contains("想说的时候我都在")); assert!(!second.contains("现在先听你说"));
        assert!(r.retrieve_growing("难过", &[], None, &[("comfort".into(), "新的理解".into())], &[update.clone()], "zh").is_none());
        // Withdrawing the interpretation withdraws its samples and restores the seed.
        assert!(r.retrieve_growing("难过", &[], None, &[], &[update.clone()], "zh").unwrap().contains("原来的回应"));
        update.understanding = None; update.edit.example = None;
        assert!(r.retrieve_growing("难过", &[], None, &[], &[update], "zh").is_none());
    }
    #[test]
    fn restored_corpus_basis_and_evolved_samples_are_used_after_restart() {
        use super::example_library::{ExampleEdit, ExampleOverride};
        let char_id = format!("restored-{}", uuid::Uuid::new_v4());
        let path = crate::utils::path::get_character_data_dir(&char_id).join("persona").join(example_library::BASE_FILE);
        let original:Example = serde_json::from_value(example("portable-base", "comfort", "难过", "归档中的原始语气")).unwrap();
        example_library::save_base_at(&path, &[original]).unwrap();
        let restored = ExampleRetriever::new(&char_id, default_embedding());
        assert!(restored.retrieve("难过", &[], None, &[], "zh").unwrap().contains("归档中的原始语气"));
        let sample:Example = serde_json::from_value(example("grown-portable", "comfort", "难过", "进化之后先陪你歇会儿")).unwrap();
        let updates = vec![ExampleOverride {scope:"comfort".into(),understanding:Some("尊重沉默".into()),updated_at:2.0,
            edit:ExampleEdit {id:sample.id.clone(),reference:None,reason:"随备份继承的成长".into(),example:Some(sample)}}];
        let roundtrip:Vec<ExampleOverride> = serde_json::from_str(&serde_json::to_string(&updates).unwrap()).unwrap();
        let output = restored.retrieve_growing("难过", &[], None, &[("comfort".into(),"尊重沉默".into())], &roundtrip, "zh").unwrap();
        assert!(output.contains("进化之后先陪你歇会儿"));
        assert!(!output.contains("归档中的原始语气"));
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
            assert!(r.retrieve(input, &[], None, &[], "zh").is_none(), "false cue in {input}");
        }
        for (input, id) in [("HI!", "hi"), ("OK，可以", "ok"), ("6!", "six"), ("草", "grass")] {
            assert!(r.retrieve(input, &[], None, &[], "zh").unwrap().contains(&format!("Example {id}\n")));
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
        let out = r.retrieve("下雨还得出门", &history, None, &[], "zh").unwrap();
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
        let first = r.retrieve("猜猜", &[], None, &[], "zh").unwrap();
        assert_eq!(first, r.retrieve("猜猜", &[], None, &[], "zh").unwrap(), "retries stay stable");
        let (used, alternative) = if first.contains("Example a\n") {
            ("Maybe a new game", "b")
        } else { ("I bet it is a drawing", "a") };
        let history = vec![message("assistant", used)];
        let next = r.retrieve("猜猜", &history, None, &[], "zh").unwrap();
        assert!(next.contains(&format!("Example {alternative}\n")));
        assert_eq!(first, r.retrieve("猜猜", &[], None, &[], "zh").unwrap(), "another session is independent");
        assert_eq!(next, fixture(entries).retrieve("猜猜", &history, None, &[], "zh").unwrap(), "restart is reproducible");
    }
    #[test]
    fn repetition_never_removes_the_only_relevant_example_or_relaxes_gates() {
        let r = fixture(serde_json::json!([example("only", "care", "下雨", "Take an umbrella")]));
        let history = vec![message("assistant", "Take an umbrella")];
        assert!(r.retrieve("下雨", &history, None, &[], "zh").unwrap().contains("Example only\n"));
        assert!(r.retrieve("新话题", &history, None, &[], "zh").is_none());
        assert!(r.retrieve("下雨", &history, None, &["care".into()], "zh").is_none());
    }

    fn scenario(id: &str, scope: &str, cue: &str, situation: &str, user: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"id":id,"scope":scope,"cues":[cue],"situation":situation,
            "user":user,"response":{"text":text,"intent":"reply"}})
    }

    #[test]
    fn offline_surface_overlap_recovers_paraphrase_and_yields_to_literal_cues() {
        let r = fixture(serde_json::json!([
            scenario("interview", "comfort", "怯场", "用户周三要面试，很紧张", "周三要面试了好紧张", "别慌，你准备得比你以为的充分。"),
            scenario("rain", "care", "带伞", "用户下班时下雨，没带伞", "外面下雨了我没带伞", "那先找个地方躲一躲。")
        ]));
        // 与 situation 同措辞的 paraphrase：字面 cue 全部未命中，表面重叠接住
        let out = r.retrieve("周三要面试，特别紧张", &[], None, &[], "zh").unwrap();
        assert!(out.contains("Example interview\n"), "{out}");
        assert!(!out.contains("Example rain\n"), "{out}");
        // 字面 cue 命中时优先，且回退分恒低于字面分
        let out = r.retrieve("周三要面试，特别紧张，也没带伞", &[], None, &[], "zh").unwrap();
        assert!(out.find("Example rain\n") < out.find("Example interview\n"), "literal must outrank fallback: {out}");
        // 纯字面场景不受影响
        let out = r.retrieve("今天没带伞", &[], None, &[], "zh").unwrap();
        assert!(out.contains("Example rain\n") && !out.contains("Example interview\n"), "{out}");
    }

    #[test]
    fn offline_surface_overlap_never_fills_two_slots_or_matches_thin_input() {
        // 三条同表面、不同 scope：都能过回退线，但只允许入选一个
        let r = fixture(serde_json::json!([
            scenario("a", "comfort", "甲", "用户面试前紧张", "面试前紧张", "a"),
            scenario("b", "care", "乙", "用户面试前紧张", "面试前紧张", "b"),
            scenario("c", "daily", "丙", "用户面试前紧张", "面试前紧张", "c")
        ]));
        let out = r.retrieve("面试前紧张", &[], None, &[], "zh").unwrap();
        assert_eq!(out.lines().filter(|l| l.starts_with("Example ") && !l.starts_with("Example response:")).count(), 1, "{out}");
        assert!(out.contains("Example a\n"), "{out}");
        // 内容词不足或无重叠的输入不触发
        assert!(r.retrieve("嗯", &[], None, &[], "zh").is_none());
        assert!(r.retrieve("去吃饭", &[], None, &[], "zh").is_none());
    }

    #[test]
    fn lexical_fallback_score_stays_below_literal_floor_and_respects_continuation() {
        let entry: Example = serde_json::from_value(serde_json::json!({
            "id":"y","scope":"comfort","cues":[],"situation":"aa bb cc dd ee ff gg","user":"aa bb cc dd ee ff gg",
            "response":{"text":"ok","intent":"reply"}
        })).unwrap();
        // overlap = 4/7 ≈ 0.571：越过常规阈值、低于 continuation 阈值
        let plain = lexical_fallback_score("aa bb cc dd hh ii jj kk", &entry, false).unwrap();
        assert!((0.65..0.80).contains(&plain), "fallback must clear the 0.65 gate yet stay under the 0.80 literal floor: {plain}");
        assert!(lexical_fallback_score("aa bb cc dd hh ii jj kk", &entry, true).is_none(),
            "continuation inputs face the stricter threshold");
        assert!(lexical_fallback_score("嗯", &entry, false).is_none(), "thin inputs never match");
    }
    #[test]
    fn duplicate_lines_across_scopes_do_not_fill_all_slots() {
        let r = fixture(serde_json::json!([
            example("a", "first", "猜猜", "Maybe a new game"),
            example("b", "second", "猜猜", "Maybe a new game!"),
            example("c", "third", "猜猜", "I bet it is a drawing")
        ]));
        let out = r.retrieve("猜猜", &[], None, &[], "zh").unwrap();
        assert_eq!(out.lines().filter(|l| l.starts_with("Example response:")).count(), 2);
        assert!(out.contains("Example c\n"));
        assert!(out.chars().count() <= MAX_CHARS);
    }
    #[test]
    fn context_cues_also_respect_ascii_boundaries() {
        let mut e = example("duration", "task", "8分钟", "Eight minutes works");
        e["context_cues"] = serde_json::json!(["talk"]);
        let r = fixture(serde_json::json!([e]));
        assert!(r.retrieve("8分钟", &[message("assistant", "A stalk of celery")], None, &[], "zh").is_none());
        assert!(r.retrieve("8分钟", &[message("assistant", "How long is the talk?")], None, &[], "zh").is_some());
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
                let out = r.retrieve(input, &[], None, &[], "zh").unwrap();
                assert!(out.contains(id));
                assert!(out.chars().count() <= MAX_CHARS);
                assert!(out.contains("Fictional context (not user history)"));
                assert!(r.retrieve(input, &[], None, &[scope.into()], "zh").is_none());
            }
        }
    }
    #[test]
    fn duration_requires_current_presentation_context() {
        let r = ExampleRetriever::new("nana", default_embedding());
        assert!(r.retrieve("8分钟左右", &[], None, &[], "zh").is_none());
        let history = vec![message("assistant", "英语演讲要讲多久？"), message("user", "8分钟左右")];
        let out = r.retrieve("8分钟左右", &history, None, &[], "zh").unwrap();
        assert!(out.contains("presentation-duration"));
        assert!(out.chars().count() <= MAX_CHARS);
    }
    #[test]
    fn confirmation_does_not_end_an_active_task() {
        let r = ExampleRetriever::new("vivian", default_embedding());
        assert!(r.retrieve("嗯", &[message("assistant", "要用蓝色吗？")], None, &[], "zh").is_none());
        assert!(r.retrieve("嗯", &[message("assistant", "晚安")], None, &[], "zh").unwrap().contains("closed-confirmation"));
        assert!(r.retrieve("好，我想聊新话题", &[message("assistant", "晚安")], None, &[], "zh").is_none());
    }
    #[test]
    fn old_emotion_does_not_match_new_topic_and_learned_scope_wins() {
        let r = ExampleRetriever::new("nana", default_embedding());
        let history = vec![message("user", "委屈"), message("assistant", "发生什么了？")];
        assert!(r.retrieve("怎么安装软件", &history, None, &[], "zh").is_none());
        assert!(r.retrieve("方案明明是我写的", &[], None, &["comfort".into()], "zh").is_none());
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
                    let examples = r.retrieve(input, &[], None, &[], "zh").expect("offline cue retrieval");
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
                    let result = r.retrieve(input, &history, None, &[], "zh").unwrap();
                    assert!(result.contains(&format!("Example {id}\n")), "{name}: {input}");
                    assert!(result.chars().count() <= MAX_CHARS);
                }
                assert!(e.context_cues.is_empty());
            }
        }
    }
    #[test]
    fn supportive_examples_respect_user_needs_and_character_voice() {
        let mut voices = Vec::new();
        for name in ["vivian", "nana"] {
            let r = ExampleRetriever::new(name, default_embedding());
            for id in ["vent-no-advice", "tired-no-questions", "uncertain-motive", "repair-feeling"] {
                let e = r.entries.iter().find(|e| e.id == id).unwrap();
                let result = r.retrieve(&e.user, &[], None, &[], "zh").unwrap();
                assert!(result.contains(&format!("Example {id}\n")), "{name}/{id}");
                assert!(result.chars().count() <= MAX_CHARS);
                assert!(e.situation.contains("虚构"));
                assert!(e.attention.contains("不可借来当事实"));
                let learned = r.retrieve(&e.user, &[], None, &[e.scope.clone()], "zh").unwrap_or_default();
                assert!(!learned.contains(&format!("Example {id}\n")));
                voices.push(e.response["text"].as_str().unwrap().to_string());
            }
        }
        for i in 0..4 { assert_ne!(voices[i], voices[i + 4]); }
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
        let result = r.retrieve("名字太明显，会不会泄露故事", &[], Some(&[1.0, 0.0]), &[], "zh").unwrap();
        assert!(result.contains("creative-choice"));
        r.preload();
        assert_eq!(provider.0.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert!(result.lines().filter(|line| line.starts_with("Example ") && !line.starts_with("Example response:")).count() <= MAX_EXAMPLES);
        // A wrong-dimension shared query must be replaced, never silently compared.
        r.retrieve("标题", &[], Some(&[1.0]), &[], "zh");
        assert_eq!(provider.0.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
    #[test]
    fn revised_cues_rebuild_vectors_without_mixing_old_and_new_indices() {
        use super::example_library::{ExampleEdit, ExampleOverride};
        let provider = Arc::new(SemanticFixture(std::sync::atomic::AtomicUsize::new(0)));
        let r = ExampleRetriever::new("test", provider);
        let sample:Example = serde_json::from_value(example("grown-semantic", "daily", "离别", "第一种表达")).unwrap();
        let mut update = ExampleOverride {scope:"daily".into(),understanding:None,updated_at:1.0,
            edit:ExampleEdit{id:sample.id.clone(),reference:None,reason:"更新场景".into(),example:Some(sample)}};
        // An empty preview builds the immutable corpus but starts no embedding I/O.
        assert!(r.retrieve_growing("", &[], None, &[], &[update.clone()], "zh").is_none());
        let first = r.revised.read().as_ref().unwrap().clone();
        first.vectors.preload();
        assert_eq!(first.vectors.get().unwrap()[0], vec![1.0, 0.0]);
        let sample = update.edit.example.as_mut().unwrap();
        sample.cues = vec!["面试".into()]; sample.user = "面试".into();
        sample.response["text"] = "第二种表达".into();
        r.retrieve_growing("", &[], None, &[], &[update.clone()], "zh");
        let second = r.revised.read().as_ref().unwrap().clone();
        assert_ne!(first.key, second.key);
        second.vectors.preload();
        assert_eq!(second.vectors.get().unwrap()[0], vec![0.0, 1.0]);
        assert_eq!(first.vectors.get().unwrap()[0], vec![1.0, 0.0]);
        let output = r.retrieve_growing("准备迎接新的挑战", &[], Some(&[0.0, 1.0]), &[], &[update], "zh").unwrap();
        assert!(output.contains("第二种表达")); assert!(!output.contains("第一种表达"));
    }

    #[test]
    fn retrieval_selects_examples_by_interface_language() {
        // 三语示例混在同一个 example_library.json 里：检索必须按界面语言选档，
        // 否则中文对话会召回 style-en/ja 示例，英文对话会一直吃中文示例
        // （模型直接跟着示例的语言说话）。2026-10-24 补的语种门。
        let r = ExampleRetriever::new("vivian", default_embedding());
        // 离线哈希嵌入下只有字面/回退通道：用中文 cue 拿中文示例，用英文 cue 拿英文示例。
        let zh = r.retrieve("给你准备了个东西，猜猜？", &[], None, &[], "zh").unwrap();
        assert!(zh.contains("Example "));
        assert!(!zh.contains("style-en-") && !zh.contains("style-ja-"), "zh turn must not pull non-zh examples: {zh}");
        // 英文界面：同一个场景拿到的是 style-en-* 示例，不是中文条目。
        let en = r.retrieve("Guess what I made. Hint: it fits in a pocket.", &[], None, &[], "en").unwrap();
        assert!(en.contains("style-en-guess-game"), "en turn must resolve the en example: {en}");
        assert!(!en.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)), "en turn must not contain Chinese: {en}");
        // 日文界面同理。
        let ja = r.retrieve("何を作ったか当てて。手のひらに乗るものだよ。", &[], None, &[], "ja").unwrap();
        assert!(ja.contains("style-ja-guess-game"), "ja turn must resolve the ja example: {ja}");
        // Japanese legitimately contains Han characters (模型, 最初, etc.).
        // Check the selected entries' language, rather than banning kanji.
        for id in ja.lines().filter_map(|line| line.strip_prefix("Example ")).filter(|id| !id.starts_with("response:")) {
            let entry = r.entries.iter().find(|entry| entry.id == id).unwrap();
            assert!(matches!(entry_lang(entry), "ja" | "any"), "wrong-language entry: {id}");
        }
    }

    #[test]
    fn source_tasks_do_not_borrow_venting_or_emotion_repair_scripts() {
        let retriever = ExampleRetriever::new("vivian", default_embedding());
        assert!(retriever.retrieve("把“别给我建议”翻译成英文。", &[], None, &[], "zh").is_none());
        assert!(source_transformation("把“今天累死了，不想解释”翻译成英文。"));
        assert!(retriever.retrieve("别分析我，帮我分析这段代码。", &[], None, &[], "zh").is_none());
        assert!(retriever.retrieve("我只想吐槽，别给我建议。", &[], None, &[], "zh").is_some());
        assert!(retriever.retrieve("今天累死了，不想讲经过。", &[], None, &[], "zh").is_some());
        assert!(!source_transformation("润色后不像自己"));
    }
}
