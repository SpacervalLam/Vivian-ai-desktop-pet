//! 宿主侧配对回放：用**运行时真实的 BM25 选择函数**给候选打分。
//!
//! `use_skill(query)` 走的是 `SkillService::search_skills` → `ToolSearchIndex`。
//! 评估必须用同一条路径，否则测的只是代理指标。为此 `skill_descriptors` 被抽成
//! 公共映射，运行时与回放共用；本模块只负责把「现行版本」与「候选版本」分别放进
//! 同一批干扰技能里，对每条选择集样本各回放一次 top-1 判定，再把逐样本结果交给
//! `evaluation::evaluate_paired` 做严格改善判定。

use crate::skills::evaluation::{evaluate_paired, Evaluation, SelectionSet};
use crate::skills::{skill_descriptors, Skill};
use crate::tools::discovery::ToolSearchIndex;

/// 影响技能召回的全部字段。正文不参与 BM25，因此回放只需这三个字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillVariant {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
}

/// 干扰技能上限，避免超大技能表让每次发布都做昂贵回放。
pub const MAX_DISTRACTORS: usize = 40;

impl SkillVariant {
    /// 从实际要落盘的 SKILL.md 文本解析，保证评估对象 = 发布对象。
    pub fn from_content(content: &str, fallback_name: &str) -> Self {
        let parsed = crate::skills::parse_skill_doc(content, fallback_name);
        Self {
            name: parsed.name,
            description: parsed.description,
            keywords: parsed.keywords,
        }
    }

    pub fn from_skill(skill: &Skill) -> Self {
        Self {
            name: skill.name.clone(),
            description: skill.description.clone(),
            keywords: skill.keywords.clone(),
        }
    }
}

/// 回放用的可见技能池：排除目标技能本身（由调用方按版本注入）与 scoped 技能。
pub fn visible_distractors(skills: &[Skill], name: &str) -> Vec<SkillVariant> {
    skills
        .iter()
        .filter(|s| s.scope.is_none() && s.name != name)
        .take(MAX_DISTRACTORS)
        .map(SkillVariant::from_skill)
        .collect()
}

fn build_index(variants: &[SkillVariant]) -> ToolSearchIndex {
    let skills: Vec<Skill> = variants
        .iter()
        .map(|v| {
            Skill::global(v.name.clone(), v.description.clone(), String::new())
                .with_keywords(v.keywords.clone())
        })
        .collect();
    ToolSearchIndex::build(skill_descriptors(&skills))
}

/// 配对回放。
///
/// - `baseline = None` 表示「现行还没有这个技能」：正样本在基线侧永远判错，
///   因此新建技能同样需要证明带来了可测的召回改善。
/// - 两侧使用完全相同的干扰技能集合与同一批样本，唯一差异是目标技能的版本。
/// - top-1 命中判定与 `search_skills(query, 1)` 一致：只看是否召回目标技能。
pub fn replay(
    name: &str,
    baseline: Option<&SkillVariant>,
    candidate: &SkillVariant,
    distractors: &[SkillVariant],
    set: &SelectionSet,
) -> Evaluation {
    let mut base_pool = distractors.to_vec();
    if let Some(base) = baseline {
        base_pool.push(base.clone());
    }
    let mut cand_pool = distractors.to_vec();
    cand_pool.push(candidate.clone());

    let base_index = build_index(&base_pool);
    let cand_index = build_index(&cand_pool);

    evaluate_paired(set, |case, is_candidate| {
        let index = if is_candidate {
            &cand_index
        } else {
            &base_index
        };
        index
            .search(&case.query, 1)
            .first()
            .map(|(descriptor, _)| descriptor.name == name)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::evaluation::EvalCase;

    fn set(cases: Vec<EvalCase>) -> SelectionSet {
        SelectionSet {
            skill: "deploy".into(),
            delta_min: 1.0,
            cases,
        }
    }

    fn case(id: &str, query: &str, should_hit: bool) -> EvalCase {
        EvalCase {
            id: id.into(),
            query: query.into(),
            should_hit,
            protected: false,
            weight: 1.0,
        }
    }

    #[test]
    fn replay_uses_real_bm25_and_detects_recall_improvement() {
        let candidate = SkillVariant {
            name: "deploy".into(),
            description: "部署上线流程".into(),
            keywords: vec!["上线".into(), "发布".into(), "deploy".into()],
        };
        // 候选把「帮我上线」写成关键词，基线只写「部署流程」。
        let baseline = SkillVariant {
            name: "deploy".into(),
            description: "部署流程".into(),
            keywords: vec!["部署".into()],
        };
        let distractors = vec![SkillVariant {
            name: "joke".into(),
            description: "讲笑话逗人开心".into(),
            keywords: vec!["笑话".into(), "讲个笑话".into()],
        }];
        let set = set(vec![
            case("p1", "帮我上线", true),
            case("p2", "deploy 发布", true),
            case("p3", "部署流程", true),
            case("p4", "发布版本", true),
            case("n1", "讲个笑话", false),
            case("n2", "今天天气", false),
        ]);

        let result = replay("deploy", Some(&baseline), &candidate, &distractors, &set);
        assert_eq!(result.case_count, 6);
        assert!(result.improved > 0, "真实 BM25 应当能观察到关键词带来的召回改善：{:?}", result.reasons);
        assert!(result.delta > 0.0, "{:?}", result.outcomes);
        // 负样本不应被候选误召回。
        assert!(result.new_false_positives.is_empty(), "{:?}", result.new_false_positives);

        // 同一批样本、同一版本：确定性，且没有任何改善 → 必然被拒。
        let identical = replay("deploy", Some(&baseline), &baseline, &distractors, &set);
        assert_eq!(identical.delta, 0.0);
        assert_eq!(identical.verdict, crate::skills::evaluation::Verdict::Rejected);
    }

    #[test]
    fn new_skill_baseline_has_no_target_skill() {
        let candidate = SkillVariant {
            name: "deploy".into(),
            description: "部署上线流程".into(),
            keywords: vec!["上线".into(), "deploy".into()],
        };
        let distractors = vec![SkillVariant {
            name: "joke".into(),
            description: "讲笑话".into(),
            keywords: vec!["笑话".into()],
        }];
        let set = set(vec![
            case("p1", "帮我上线", true),
            case("p2", "deploy 一下", true),
            case("p3", "发布版本", true),
            case("p4", "部署服务", true),
            case("n1", "讲个笑话", false),
            case("n2", "今天天气", false),
        ]);
        let result = replay("deploy", None, &candidate, &distractors, &set);
        assert_eq!(result.baseline_score, 2.0, "基线只应判对两条负样本");
        assert!(result.delta >= 1.0, "{:?}", result.reasons);
        assert_eq!(result.improved, 4);
    }
}
