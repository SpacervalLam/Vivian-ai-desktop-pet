//! 日记写完后的归纳总结：把当天的互动沉淀成「场景化自我理解」，交给人格自进化覆盖层。
//!
//! 安全口径与逐轮反思一致——**只有当天已保存的用户原话能当证据**。模型自己的总结、
//! 它给出的 memory_id 一律不采信：宿主只按 `source_quote` 在当天记录里逐字反查。
//! 晋升门槛（≥2 个独立自然日、同证据同 scope 去重、候选上限）全部沿用
//! `persona::evolution`，本模块不新增存储。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::brain::Brain;
use crate::error::{VivianError, VivianResult};
use crate::persona::evolution::GrowthEvidence;
use crate::types::response::ChatMessage;

use super::intelligent_generator::{strip_markdown_code_fence, DailyContext, InteractionRecord};

/// 归纳是后台任务：超时即放弃，不让日记链路堆积。
pub const GROWTH_TIMEOUT: Duration = Duration::from_secs(90);

/// 单轮最多几条提案：候选池只有 12 格，一次灌满会挤掉更接近晋升的候选。
const MAX_PROPOSALS: usize = 2;
const MIN_QUOTE_CHARS: usize = 6;
const MAX_QUOTE_CHARS: usize = 500;
/// 当天用户原话摘录的总预算（按整条截断，不改写内容，避免模型抄不回逐字片段）。
const MAX_DIGEST_CHARS: usize = 4000;

const GROWTH_DIRECTIVE: &str = include_str!("../../prompts/framework/diary_growth.md");

/// 归纳出的一条自我理解提案
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GrowthProposal {
    /// 六个允许场景之一：comfort / care / praise / humor / daily / disagreement
    pub scope: String,
    /// tone | personality
    pub kind: String,
    pub understanding: String,
    #[serde(default)]
    pub reason: String,
    /// 必须是当天用户原话的逐字片段
    pub source_quote: String,
    /// 与已有候选高度重合时，用候选的 reference 改写而不是新增
    #[serde(default)]
    pub revises: Option<String>,
}

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
        .find(|r| r.role == "user" && !r.memory_id.is_empty() && r.content.contains(quote))?;
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

/// 应用提案：解析不到证据的直接丢弃，其余交给成长覆盖层做全部校验。
///
/// 返回 `(提交条数, 其中当场晋升条数)`；非晋升的提案仍会作为候选留存。
pub fn apply_proposals(
    persona: &crate::persona::PersonaEngine,
    proposals: &[GrowthProposal],
    records: &[InteractionRecord],
) -> (usize, usize) {
    let mut promoted = 0;
    let acceptable = acceptable_proposals(proposals, records);
    for (proposal, evidence) in &acceptable {
        if persona.revise_evolution(
            &proposal.kind,
            &proposal.scope,
            proposal.understanding.trim(),
            &proposal.reason,
            evidence.clone(),
            false,
            proposal.revises.as_deref(),
        ) {
            promoted += 1;
        }
    }
    (acceptable.len(), promoted)
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
fn learned_context(persona: &crate::persona::PersonaEngine) -> String {
    let active: Vec<Value> = persona
        .evolution_entries()
        .into_iter()
        .filter(|entry| entry.active())
        .map(|entry| serde_json::json!({"scope": entry.scope, "understanding": entry.text}))
        .collect();
    let candidates: Vec<Value> = persona
        .evolution_candidates()
        .into_iter()
        .map(|candidate| {
            serde_json::json!({
                "reference": candidate.reference(),
                "kind": candidate.kind,
                "scope": candidate.scope,
                "understanding": candidate.text,
            })
        })
        .collect();
    format!(
        "{}\n{}\n",
        serde_json::to_string(&active).unwrap_or_default(),
        serde_json::to_string(&candidates).unwrap_or_default()
    )
}

/// 构造归纳消息：当天用户原话 + 刚写完的日记 + 已有理解/候选。
pub fn build_growth_messages(
    ctx: &DailyContext,
    diary: &str,
    persona: &crate::persona::PersonaEngine,
    lang: &str,
) -> Vec<ChatMessage> {
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
        learned_context(persona)
    );
    vec![
        ChatMessage::system(GROWTH_DIRECTIVE),
        ChatMessage::user(user_prompt),
    ]
}

/// 归纳输出 JSON Schema（仅用于激活 response_format，结构由提示词约束）
fn growth_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {"proposals": {"type": "array", "items": {"type": "object"}}},
        "required": ["proposals"]
    })
}

/// 容错解析：接受围栏包裹、裸对象或裸数组；单条不合规就丢掉该条。
pub fn parse_growth_proposals(raw: &str) -> Vec<GrowthProposal> {
    let text = strip_markdown_code_fence(raw).trim();
    let value: Option<Value> = serde_json::from_str(text).ok().or_else(|| {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        (start < end).then(|| serde_json::from_str(&text[start..=end]).ok()).flatten()
    });
    let Some(value) = value else {
        return Vec::new();
    };
    let items = value
        .get("proposals")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned())
        .unwrap_or_default();
    items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect()
}

/// 归纳入口：读当天上下文与日记，产出提案并写进成长覆盖层。
pub async fn run_diary_growth(
    brain: &Brain,
    ctx: &DailyContext,
    diary: &str,
) -> VivianResult<(usize, usize)> {
    let lang = brain.persona.get_language();
    let messages = build_growth_messages(ctx, diary, &brain.persona, &lang);
    let response = brain
        .router
        .generate(
            crate::providers::base::LLMRequest::new("diary_growth", messages)
                .with_json_schema(growth_schema())
                .with_character_id(brain.char_id.clone()),
        )
        .await
        .map_err(|e| VivianError::Other(format!("日记归纳调用失败: {e}")))?;
    let proposals = parse_growth_proposals(&response);
    Ok(apply_proposals(
        &brain.persona,
        &proposals,
        &ctx.interactions,
    ))
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
            listener: String::new(),
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
        }
    }

    #[test]
    fn proposals_without_verbatim_evidence_are_dropped_and_capped() {
        let records = vec![
            record("user", "今天累死了，不想讲经过", "m1", 1.0),
            record("user", "方案明明是我写的", "m2", 2.0),
            record("assistant", "那就先别讲了。", "m3", 3.0),
        ];
        // 助手发言不能作证据：该条被丢弃，但仍占用单轮名额。
        let noisy = [
            proposal("daily", "少讲道理", "那就先别讲了"),
            proposal("comfort", "先听完再回应", "不想讲经过"),
            proposal("praise", "承认具体贡献", "方案明明是我写的"),
        ];
        let mixed = acceptable_proposals(&noisy, &records);
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].1.memory_id, "m1");

        // 全部可用时按上限截断到两条。
        let plenty = [
            proposal("comfort", "先听完再回应", "不想讲经过"),
            proposal("praise", "承认具体贡献", "方案明明是我写的"),
            proposal("daily", "第三条被上限截掉", "今天累死了，不想讲经过"),
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
            record("user", "今天累死了，不想讲经过", "m-user", 2.0),
        ];
        let evidence = resolve_evidence("不想讲经过", &records).unwrap();
        assert_eq!(evidence.memory_id, "m-user");
        assert_eq!(evidence.timestamp, 2.0);
        assert_eq!(evidence.fingerprint, fingerprint("今天累死了，不想讲经过"));

        // 助手发言、过短摘录、编造的句子、缺 memory_id 的记录都不构成证据。
        assert!(resolve_evidence("那我陪你待会儿", &records).is_none());
        assert!(resolve_evidence("太短", &records).is_none());
        assert!(resolve_evidence("你昨天说过想休息", &records).is_none());
        assert!(resolve_evidence("不想讲经过", &[record("user", "今天累死了，不想讲经过", "", 2.0)]).is_none());
        assert!(resolve_evidence(&"长".repeat(MAX_QUOTE_CHARS + 1), &records).is_none());
    }

    #[test]
    fn digest_keeps_user_dialogue_verbatim_and_bounded() {
        let mut records = vec![
            record("user", "今天累死了，不想讲经过", "m1", 1.0),
            record("assistant", "那就别讲了。", "m2", 2.0),
            record("user", "cross", "m3", 3.0),
        ];
        records[2].channel = "cross_character".into();
        let digest = user_digest(&records);
        assert!(digest.contains("今天累死了，不想讲经过"));
        assert!(!digest.contains("那就别讲了"));
        assert!(!digest.contains("cross"));

        let long = vec![record("user", &"很".repeat(MAX_DIGEST_CHARS + 10), "m4", 1.0)];
        assert!(user_digest(&long).is_empty(), "整条超出预算时不截断内容，直接不放");
    }

    #[test]
    fn parsing_tolerates_fences_bare_arrays_and_broken_items() {
        let fenced = "```json\n{\"proposals\":[{\"scope\":\"comfort\",\"kind\":\"tone\",\"understanding\":\"先听完再说\",\"source_quote\":\"不想讲经过\"}]}\n```";
        assert_eq!(parse_growth_proposals(fenced).len(), 1);

        let bare = "[{\"scope\":\"daily\",\"kind\":\"tone\",\"understanding\":\"短一点\",\"source_quote\":\"上班去了\"}]";
        assert_eq!(parse_growth_proposals(bare).len(), 1);

        // 缺字段（无 understanding）与非法 scope 由下游丢弃，这里只丢解析不了的条目。
        let mixed = "{\"proposals\":[{\"scope\":\"comfort\",\"kind\":\"tone\"},{\"scope\":\"care\",\"kind\":\"personality\",\"understanding\":\"先问一句\",\"source_quote\":\"今天有点撑不住\"}]}";
        assert_eq!(parse_growth_proposals(mixed).len(), 1);
        assert!(parse_growth_proposals("模型没有按协议输出").is_empty());
    }

    #[test]
    fn same_day_records_count_as_one_independent_day() {
        // 同一天两条被引用的用户消息：只累积一条独立支持，不晋升。
        let mut evolution = PersonaEvolution::empty();
        let evidence = resolve_evidence(
            "不想讲经过",
            &[
                record("user", "今天累死了，不想讲经过", "m1", 100.0),
                record("user", "还是不想讲经过", "m2", 101.0),
            ],
        )
        .unwrap();
        assert!(!evolution.propose_revision(
            "tone", "comfort", "先听完再回应", "日记归纳", evidence, false, 100.0, None
        ));
        let evidence = resolve_evidence(
            "还是不想讲经过",
            &[record("user", "还是不想讲经过", "m2", 101.0)],
        )
        .unwrap();
        assert!(!evolution.propose_revision(
            "tone", "comfort", "先听完再回应", "日记归纳", evidence, false, 101.0, None
        ));
        assert_eq!(evolution.candidates[0].support, 1);
        assert!(evolution.is_empty());

        // 第二天再出现，才跨过独立日的门槛。
        let evidence = resolve_evidence(
            "先别给建议",
            &[record("user", "先别给建议", "m3", 2.0 * 86400.0 + 100.0)],
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
        let records = vec![record("user", "今天累死了，不想讲经过", "m1", 1.0)];
        let first = resolve_evidence("不想讲经过", &records).unwrap();
        assert!(!evolution.propose_revision("tone", "comfort", "版本一", "r", first, false, 1.0, None));
        let second = resolve_evidence("不想讲经过", &records).unwrap();
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
