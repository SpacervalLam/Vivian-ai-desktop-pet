//! 日记驱动的自省：审视有效人设与模仿语料，原子提交人格成长及样例增改。
//!
//! 人格成长只有当天已保存的用户原话能当证据。模型自己的总结、
//! 它给出的 memory_id 一律不采信：宿主只按 `source_quote` 在当天记录里逐字反查。
//! 晋升门槛（≥2 个独立自然日、同证据同 scope 去重、候选上限）全部沿用
//! `persona::evolution`；语料维护可独立进行，两者共用同一个版本化状态文件。

use std::time::Duration;

use serde_json::Value;

use crate::brain::Brain;
use crate::error::{VivianError, VivianResult};
use crate::persona::evolution::{GrowthEvidence, DiaryRevision, DiaryReviewOutcome, PersonaEvolution};
use crate::persona::example_library;
use crate::persona::evolution_agent::PersonaEvolutionAgent;
pub use crate::persona::evolution_agent::EvolutionProposal as GrowthProposal;
#[cfg(test)]
use crate::persona::evolution_agent::parse_review as parse_growth_review;
use crate::types::response::ChatMessage;

use super::intelligent_generator::{DailyContext, InteractionRecord};

/// 归纳是后台任务：超时即放弃，不让日记链路堆积。
pub const GROWTH_TIMEOUT: Duration = Duration::from_secs(90);

/// 单轮最多几条提案：候选池只有 12 格，一次灌满会挤掉更接近晋升的候选。
const MAX_PROPOSALS: usize = 2;
const MIN_QUOTE_CHARS: usize = 6;
const MAX_QUOTE_CHARS: usize = 500;
/// 当天用户原话摘录的总预算（按整条截断，不改写内容，避免模型抄不回逐字片段）。
const MAX_DIGEST_CHARS: usize = 4000;

fn fingerprint(content: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(content.trim().as_bytes()))
}

/// 只按原话反查证据：quote 必须逐字出现在**当天已保存的用户发言**里。
///
/// 助手发言、摘录、模型自己编的句子一律不构成证据。返回的证据 ID 来自存储条目本身，
/// 因此与 `PersonaEngine::reconcile_evolution` 的失效校验严格对齐。
pub fn resolve_evidence(quote: &str, records: &[InteractionRecord]) -> Option<GrowthEvidence> {
    let quote = quote.trim();
    let chars = quote.chars().count();
    if chars < MIN_QUOTE_CHARS || chars > MAX_QUOTE_CHARS {
        return None;
    }
    let record = records
        .iter()
        .find(|r| r.role == "user" && r.is_user_dialogue() && !r.memory_id.is_empty() && r.content.contains(quote))?;
    Some(GrowthEvidence {
        memory_id: record.memory_id.clone(),
        quote: quote.to_string(),
        timestamp: record.timestamp,
        fingerprint: fingerprint(&record.content),
    })
}

/// 提案 → 可提交项：解析不到逐字证据、或理解为空的直接丢弃，并限制单轮条数。
fn acceptable_proposals<'a>(
    proposals: &'a [GrowthProposal],
    records: &[InteractionRecord],
) -> Vec<(&'a GrowthProposal, GrowthEvidence)> {
    proposals
        .iter()
        .take(MAX_PROPOSALS)
        .filter(|proposal| !proposal.understanding.trim().is_empty())
        .filter_map(|proposal| {
            resolve_evidence(&proposal.source_quote, records).map(|evidence| (proposal, evidence))
        })
        .collect()
}

/// 当天用户原话摘录：只取真实用户发言，整条纳入直到预算用完。
fn user_digest(records: &[InteractionRecord]) -> String {
    let mut out = String::new();
    for record in records
        .iter()
        .filter(|r| r.role == "user" && r.is_user_dialogue() && !r.content.trim().is_empty())
    {
        let line = format!("- {}\n", record.content.trim());
        if out.chars().count() + line.chars().count() > MAX_DIGEST_CHARS {
            break;
        }
        out.push_str(&line);
    }
    out
}

/// 已生效理解与待验证候选：让模型优先改写而不是重复新增。
fn learned_context(evolution: &PersonaEvolution) -> String {
    let active: Vec<Value> = evolution.entries.iter()
        .filter(|entry| entry.active())
        .map(|entry| serde_json::json!({"scope": entry.scope, "kind":entry.kind, "understanding": entry.text}))
        .collect();
    let candidates: Vec<Value> = evolution.candidates.iter()
        .map(|candidate| {
            serde_json::json!({
                "reference": candidate.reference(),
                "kind": candidate.kind,
                "scope": candidate.scope,
                "understanding": candidate.text,
                "support_days": candidate.support,
                "pending_examples": candidate.examples,
            })
        })
        .collect();
    format!(
        "{}\n{}\n",
        serde_json::to_string(&active).unwrap_or_default(),
        serde_json::to_string(&candidates).unwrap_or_default()
    )
}

/// Rotate through the whole corpus over diaries, with extra coverage for scenes
/// encountered today and the targets of pending persona/example revisions.
fn corpus_context(ctx: &DailyContext, evolution: &PersonaEvolution) -> String {
    let learned = evolution.learned();
    let entries = example_library::materialize(&example_library::base_examples(&ctx.char_id),
        &evolution.example_overrides, &learned);
    if entries.is_empty() { return "[]".into(); }
    let mut indices = std::collections::BTreeSet::new();
    for offset in 0..example_library::AUDIT_WINDOW.min(entries.len()) {
        indices.insert((evolution.review_cursor + offset) % entries.len());
    }
    let today = user_digest(&ctx.interactions).to_lowercase();
    for (index, _) in entries.iter().enumerate().filter(|(_, entry)|
        entry.cues.iter().any(|cue| !cue.is_empty() && today.contains(&cue.to_lowercase()))).take(8) {
        indices.insert(index);
    }
    for edit in evolution.candidates.iter().flat_map(|candidate| &candidate.examples) {
        if let Some(index) = entries.iter().position(|entry| entry.id == edit.id) { indices.insert(index); }
    }
    let audit: Vec<_> = indices.into_iter().map(|index| {
        let entry = &entries[index];
        let overridden = evolution.example_overrides.iter().any(|o| o.edit.id == entry.id && o.applies(&learned));
        serde_json::json!({"reference":example_library::fingerprint(entry), "example":entry,
            "retrievable": overridden || !learned.iter().any(|(scope, _)| scope == &entry.scope)})
    }).collect();
    serde_json::to_string(&audit).unwrap_or_default()
}

fn interaction_digest(records: &[InteractionRecord]) -> String {
    let mut lines = Vec::new();
    let mut budget = 5000;
    for record in records.iter().rev().filter(|r| r.is_user_dialogue() && matches!(r.role.as_str(), "user" | "assistant")) {
        let line = format!("{}: {}", record.role, record.content);
        let size = line.chars().count();
        if size > budget { continue; }
        budget -= size;
        lines.push(line);
        if lines.len() == 20 { break; }
    }
    lines.reverse();
    lines.join("\n")
}

/// 构造归纳消息：当天用户原话 + 刚写完的日记 + 已有理解/候选。
pub fn build_growth_messages(
    ctx: &DailyContext,
    diary: &str,
    persona: &crate::persona::PersonaEngine,
    lang: &str,
) -> Vec<ChatMessage> {
    PersonaEvolutionAgent::messages(build_review_context(ctx, diary, persona, lang, &persona.evolution_snapshot()))
}

fn build_review_context(ctx: &DailyContext, diary: &str, persona: &crate::persona::PersonaEngine,
    lang: &str, evolution: &PersonaEvolution) -> String {
    let (user_lines_h, diary_h, learned_h) = match crate::pipeline::prompt_modules::normalize_lang(lang) {
        "en" => (
            "What the user actually said today (verbatim, the only valid source of evidence):",
            "The diary you just finished:",
            "Already active understandings [scope, understanding], then pending candidates [reference, kind, scope, understanding]:",
        ),
        "ja" => (
            "今日ユーザーが実際に言った言葉（逐字、証拠として認められる唯一の出所）：",
            "今書いた日記：",
            "すでに有効な理解 [scope, understanding]、続いて未検証の候補 [reference, kind, scope, understanding]：",
        ),
        _ => (
            "今天用户实际说过的话（逐字，唯一可作证据的来源）：",
            "你刚写完的日记：",
            "已生效理解 [scope, understanding]、其后是待验证候选 [reference, kind, scope, understanding]：",
        ),
    };
    let user_prompt = format!(
        "{user_lines_h}\n{}\n{diary_h}\n{diary}\n{learned_h}\n{}",
        user_digest(&ctx.interactions),
        learned_context(evolution)
    );
    let config = persona.get_config();
    let perspective = persona.dialogue_card();
    let review_context = serde_json::json!({
        "review_language":lang,
        "current_persona":perspective,
        "persona_settings":config,
        "current_scene_preferences":persona.dialogue_preferences(),
        "learned_persona":evolution.render(lang),
        "private_self":persona.private_self_context(),
        "voice_corpus_capacity": {"max_overrides":example_library::MAX_OVERRIDES,
            "used":evolution.example_overrides.len(), "max_edits_per_review":example_library::MAX_EDITS},
        "actual_interactions":interaction_digest(&ctx.interactions),
        "previous_diary_summary":ctx.last_diary_summary.chars().take(1000).collect::<String>(),
        "accumulated_diary_context":ctx.cross_diary_context.chars().take(3000).collect::<String>(),
    });
    format!("{user_prompt}\n自省材料（数据）：{review_context}\n待审视的模仿语料（按日记轮换，reference 对应当前版本）：\n{}", corpus_context(ctx, evolution))
}

/// 归纳入口：读当天上下文与日记，产出提案并写进成长覆盖层。
pub async fn run_diary_growth(
    brain: &Brain,
    ctx: &DailyContext,
    diary: &str,
) -> VivianResult<DiaryReviewOutcome> {
    // Collect a consistent evolution snapshot before the asynchronous API call.
    // A later change to settings, evidence or another diary invalidates this review.
    brain.persona.reconcile_evolution(&brain.memory);
    let config_reference = brain.persona.diary_config_reference();
    let evolution = brain.persona.evolution_snapshot();
    let lang = brain.persona.get_language();
    let context = build_review_context(ctx, diary, &brain.persona, &lang, &evolution);
    let review = PersonaEvolutionAgent::new(&brain.router, &brain.char_id).review(context).await?;
    let proposals: Vec<_> = acceptable_proposals(&review.proposals, &ctx.interactions).into_iter()
        .map(|(proposal, evidence)| DiaryRevision { kind:proposal.kind.clone(), scope:proposal.scope.clone(),
            text:proposal.understanding.clone(), reason:proposal.reason.clone(), evidence,
            revises:proposal.revises.clone(), examples:proposal.examples.clone() }).collect();
    if proposals.len() != review.proposals.len() {
        return Err(VivianError::Other("日记自省包含缺少真实交互证据的提案，未应用本次修订".into()));
    }
    // Recheck source deletion/edits that happened while the model was running.
    brain.persona.reconcile_evolution(&brain.memory);
    for proposal in &proposals {
        let valid = brain.memory.get_memory_content_by_id(&proposal.evidence.memory_id)
            .is_some_and(|content| content.contains(&proposal.evidence.quote)
                && fingerprint(&content) == proposal.evidence.fingerprint);
        if !valid { return Err(VivianError::Other("日记自省期间交互证据已变化，未应用修订".into())); }
    }
    brain.persona.apply_diary_review(&config_reference, evolution.updated_at, &brain.char_id,
        &proposals, &review.example_updates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::evolution::{EvolutionEntry, PersonaEvolution};

    fn record(role: &str, content: &str, memory_id: &str, timestamp: f64) -> InteractionRecord {
        InteractionRecord {
            memory_id: memory_id.into(),
            role: role.into(),
            content: content.into(),
            timestamp,
            channel: "direct".into(),
            speaker: role.into(),
            listener: if role == "assistant" { "user".into() } else { String::new() },
        }
    }

    fn proposal(scope: &str, text: &str, quote: &str) -> GrowthProposal {
        GrowthProposal {
            scope: scope.into(),
            kind: "tone".into(),
            understanding: text.into(),
            reason: "日记归纳".into(),
            source_quote: quote.into(),
            revises: None,
            examples: vec![],
        }
    }

    #[test]
    fn proposals_without_verbatim_evidence_are_dropped_and_capped() {
        let records = vec![
            record("user", "今天累死了，不想讲经过了", "m1", 1.0),
            record("user", "方案明明是我写的", "m2", 2.0),
            record("assistant", "那就先别讲了。", "m3", 3.0),
        ];
        // 助手发言不能作证据：该条被丢弃，但仍占用单轮名额。
        let noisy = [
            proposal("daily", "少讲道理", "那就先别讲了"),
            proposal("comfort", "先听完再回应", "不想讲经过了"),
            proposal("praise", "承认具体贡献", "方案明明是我写的"),
        ];
        let mixed = acceptable_proposals(&noisy, &records);
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].1.memory_id, "m1");

        // 全部可用时按上限截断到两条。
        let plenty = [
            proposal("comfort", "先听完再回应", "不想讲经过了"),
            proposal("praise", "承认具体贡献", "方案明明是我写的"),
            proposal("daily", "第三条被上限截掉", "今天累死了，不想讲经过了"),
        ];
        let capped = acceptable_proposals(&plenty, &records);
        assert_eq!(capped.len(), MAX_PROPOSALS);
        assert_eq!(capped[1].0.scope, "praise");

        // 理解为空时丢弃。
        let blank = [proposal("daily", "   ", "方案明明是我写的")];
        assert!(acceptable_proposals(&blank, &records).is_empty());
    }

    #[test]
    fn only_verbatim_user_speech_can_become_evidence() {
        let records = vec![
            record("assistant", "那我陪你待会儿。", "m-assistant", 1.0),
            record("user", "今天累死了，不想讲经过了", "m-user", 2.0),
        ];
        let evidence = resolve_evidence("不想讲经过了", &records).unwrap();
        assert_eq!(evidence.memory_id, "m-user");
        assert_eq!(evidence.timestamp, 2.0);
        assert_eq!(evidence.fingerprint, fingerprint("今天累死了，不想讲经过了"));

        // 助手发言、过短摘录、编造的句子、缺 memory_id 的记录都不构成证据。
        assert!(resolve_evidence("那我陪你待会儿", &records).is_none());
        assert!(resolve_evidence("太短", &records).is_none());
        assert!(resolve_evidence("你昨天说过想休息", &records).is_none());
        assert!(resolve_evidence("不想讲经过了", &[record("user", "今天累死了，不想讲经过了", "", 2.0)]).is_none());
        assert!(resolve_evidence(&"长".repeat(MAX_QUOTE_CHARS + 1), &records).is_none());
    }

    #[test]
    fn digest_keeps_user_dialogue_verbatim_and_bounded() {
        let mut records = vec![
            record("user", "今天累死了，不想讲经过了", "m1", 1.0),
            record("assistant", "那就别讲了。", "m2", 2.0),
            record("user", "cross", "m3", 3.0),
        ];
        records[2].channel = "cross_character".into();
        let digest = user_digest(&records);
        assert!(digest.contains("今天累死了，不想讲经过了"));
        assert!(!digest.contains("那就别讲了"));
        assert!(!digest.contains("cross"));

        let long = vec![record("user", &"很".repeat(MAX_DIGEST_CHARS + 10), "m4", 1.0)];
        assert!(user_digest(&long).is_empty(), "整条超出预算时不截断内容，直接不放");
    }

    #[test]
    fn corpus_review_rotates_and_sees_actual_dialogue_without_treating_it_as_new_facts() {
        let mut ctx = DailyContext {char_id:"vivian".into(), ..Default::default()};
        ctx.interactions = vec![record("user", "今天累死了，不想讲经过了", "user", 1.0),
            record("assistant", "那就先休息一会儿。", "assistant", 2.0)];
        let mut evolution = PersonaEvolution::empty();
        let first:Value = serde_json::from_str(&corpus_context(&ctx, &evolution)).unwrap();
        evolution.review_cursor = example_library::AUDIT_WINDOW;
        let second:Value = serde_json::from_str(&corpus_context(&ctx, &evolution)).unwrap();
        assert_ne!(first, second);
        assert!(first.as_array().unwrap().iter().all(|entry| entry["reference"].as_str().unwrap().len() == 64));
        let digest = interaction_digest(&ctx.interactions);
        assert!(digest.contains("user: 今天累死了")); assert!(digest.contains("assistant: 那就先休息"));
        ctx.interactions[0].channel = "cross_character".into();
        assert!(resolve_evidence("不想讲经过了", &ctx.interactions).is_none());
    }

    #[test]
    fn malformed_sample_edits_cannot_be_silently_applied_as_a_partial_review() {
        assert!(parse_growth_review(r#"{"proposals":[],"example_updates":[{"id":"missing-reason"}]}"#).is_err());
        assert!(parse_growth_review(r#"{"proposals":[],"example_updates":[{"id":"old","reference":"ref","reason":"missing explicit example or null"}]}"#).is_err());
        let voice_only = parse_growth_review(r#"{"proposals":[],"example_updates":[]}"#).unwrap();
        assert!(voice_only.proposals.is_empty());
    }

    #[test]
    fn parsing_accepts_fences_and_rejects_incomplete_reviews() {
        let fenced = "```json\n{\"proposals\":[{\"scope\":\"comfort\",\"kind\":\"tone\",\"understanding\":\"先听完再说\",\"source_quote\":\"不想讲经过了\"}]}\n```";
        assert_eq!(parse_growth_review(fenced).unwrap().proposals.len(), 1);

        let bare = "[{\"scope\":\"daily\",\"kind\":\"tone\",\"understanding\":\"短一点\",\"source_quote\":\"上班去了\"}]";
        assert!(parse_growth_review(bare).is_err());

        // 一条缺字段就丢弃整批，避免只提交人格或样例的一部分。
        let mixed = "{\"proposals\":[{\"scope\":\"comfort\",\"kind\":\"tone\"},{\"scope\":\"care\",\"kind\":\"personality\",\"understanding\":\"先问一句\",\"source_quote\":\"今天有点撑不住\"}]}";
        assert!(parse_growth_review(mixed).is_err());
        assert!(parse_growth_review("模型没有按协议输出").is_err());
    }

    #[test]
    fn same_day_records_count_as_one_independent_day() {
        // 同一天两条被引用的用户消息：只累积一条独立支持，不晋升。
        let mut evolution = PersonaEvolution::empty();
        let evidence = resolve_evidence(
            "不想讲经过了",
            &[
                record("user", "今天累死了，不想讲经过了", "m1", 100.0),
                record("user", "还是不想讲经过了", "m2", 101.0),
            ],
        )
        .unwrap();
        assert!(!evolution.propose_revision(
            "tone", "comfort", "先听完再回应", "日记归纳", evidence, false, 100.0, None
        ));
        let evidence = resolve_evidence(
            "还是不想讲经过了",
            &[record("user", "还是不想讲经过了", "m2", 101.0)],
        )
        .unwrap();
        assert!(!evolution.propose_revision(
            "tone", "comfort", "先听完再回应", "日记归纳", evidence, false, 101.0, None
        ));
        assert_eq!(evolution.candidates[0].support, 1);
        assert!(evolution.is_empty());

        // 第二天再出现，才跨过独立日的门槛。
        let evidence = resolve_evidence(
            "先别给建议了",
            &[record("user", "先别给建议了", "m3", 2.0 * 86400.0 + 100.0)],
        )
        .unwrap();
        assert!(evolution.propose_revision(
            "tone", "comfort", "先听完再回应", "日记归纳", evidence, false, 2.0 * 86400.0, None
        ));
        assert_eq!(evolution.entries.len(), 1);
        assert!(evolution.render("zh").unwrap().contains("先听完再回应"));
    }

    #[test]
    fn one_event_cannot_support_the_same_scope_twice() {
        let mut evolution = PersonaEvolution::empty();
        let records = vec![record("user", "今天累死了，不想讲经过了", "m1", 1.0)];
        let first = resolve_evidence("不想讲经过了", &records).unwrap();
        assert!(!evolution.propose_revision("tone", "comfort", "版本一", "r", first, false, 1.0, None));
        let second = resolve_evidence("不想讲经过了", &records).unwrap();
        assert!(!evolution.propose_revision("tone", "comfort", "版本二", "r", second, false, 1.0, None));
        assert_eq!(evolution.candidates.len(), 1);
        assert_eq!(evolution.candidates[0].text, "版本一");
    }

    #[test]
    fn entries_stay_serializable_for_the_growth_store() {
        let entry = EvolutionEntry {
            timestamp: 1.0,
            kind: "tone".into(),
            scope: "daily".into(),
            text: "短一点".into(),
            reason: "r".into(),
            support: 2,
            evidence: vec![],
            explicit_feedback: false,
        };
        assert_eq!(entry.kind, "tone");
        assert!(!entry.active(), "无证据的条目永不生效");
    }
}
