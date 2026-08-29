//! Belief 生成器 —— 由 Reflection 异步驱动，从近期 LongTerm + Insight 提炼 Belief/Goal。
//!
//! 不阻塞对话路径：由 `MemoryConsolidator` 在夜间/空闲巩固后调用，复用 pipeline
//! 的 reflection 路由。源数据是最近产生的 Insight（高层抽象）+ LongTerm（具体事实），
//! 输出结构化 BeliefDraft / GoalDraft，写入 Mind。
//!
//! 设计原则：
//! - Belief 必须可溯源：每条 draft 必须带 source_memory_ids（来自 Insight 或 LongTerm）
//! - 合并优先：写入时走 BeliefStore::upsert_with_merge，证据交集 ≥ 2 则强化既有 Belief
//! - Goal 稀少：每次最多产 1-2 条新 Goal，且 deactivate 旧 Goal 避免堆积
//! - 失败静默：LLM 返回空或解析失败不阻塞巩固流程

use std::sync::Arc;

use serde::Deserialize;

use crate::error::VivianResult;
use crate::memory::manager::MemoryManager;
use crate::memory::types::MemoryItem;
use crate::mind::{Belief, BeliefCategory, Goal, GoalOrigin, Mind};
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

/// Belief 生成结果
#[derive(Debug, Default)]
pub struct BeliefGenerationReport {
    pub beliefs_created: usize,
    pub beliefs_reinforced: usize,
    pub goals_added: usize,
}

/// LLM 返回的 Belief 草稿
#[derive(Debug, Deserialize)]
struct BeliefDraft {
    statement: String,
    /// "user" / "self" / 角色ID / "world"
    #[serde(default)]
    subject: String,
    /// trait / habit / preference / state / relationship
    #[serde(default = "default_category")]
    category: String,
    /// 0.0-1.0
    #[serde(default = "default_confidence")]
    confidence: f64,
    /// 支撑记忆 ID
    source_ids: Vec<String>,
}

fn default_category() -> String {
    "state".to_string()
}

fn default_confidence() -> f64 {
    0.5
}

/// LLM 返回的 Goal 草稿
#[derive(Debug, Deserialize)]
struct GoalDraft {
    description: String,
    /// reflection / user_request / proactive / schedule
    #[serde(default = "default_goal_origin")]
    origin: String,
    /// 0.0-1.0
    #[serde(default = "default_goal_priority")]
    priority: f64,
}

fn default_goal_origin() -> String {
    "reflection".to_string()
}

fn default_goal_priority() -> f64 {
    0.5
}

/// Belief 生成器
pub struct BeliefGenerator {
    router: Arc<ModelRouter>,
    /// 证据交集阈值：≥ 此值则合并而非新建
    merge_overlap_threshold: usize,
}

impl BeliefGenerator {
    pub fn new(router: Arc<ModelRouter>) -> Self {
        Self {
            router,
            merge_overlap_threshold: 2,
        }
    }

    /// 执行一次 Belief/Goal 生成
    ///
    /// 数据源：最近 N 条 Insight + 最近 M 条 LongTerm。LLM 从中提炼 Belief 和 Goal，
    /// 写入 Mind。失败时返回空 report，不传播错误。
    pub async fn generate(&self, memory: &MemoryManager, mind: &Mind) -> VivianResult<BeliefGenerationReport> {
        let now = chrono::Utc::now().timestamp();
        let all = memory.get_all_memories().await?;

        // 取最近 Insight 和 LongTerm 作为源数据
        let insights: Vec<&MemoryItem> = all
            .iter()
            .filter(|m| m.tags.iter().any(|t| t == "insight"))
            .collect();
        let long_terms: Vec<&MemoryItem> = all
            .iter()
            .filter(|m| m.tags.iter().any(|t| t == "long_term"))
            .collect();

        // 源数据不足时跳过（至少 3 条 Insight 或 5 条 LongTerm）
        if insights.len() < 3 && long_terms.len() < 5 {
            tracing::debug!(
                "[BeliefGenerator] 源数据不足（{} Insight, {} LongTerm），跳过",
                insights.len(),
                long_terms.len()
            );
            return Ok(BeliefGenerationReport::default());
        }

        // 构建源数据文本（带 id 供 LLM 引用）
        let insight_text = insights
            .iter()
            .take(15)
            .map(|m| format!("[{}] {}", m.id, m.content))
            .collect::<Vec<_>>()
            .join("\n");
        let ltm_text = long_terms
            .iter()
            .take(20)
            .map(|m| format!("[{}] {}", m.id, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        // 获取既有 Belief（最多 10 条，按置信度排序），供 LLM 去重/强化参考
        let existing_beliefs_text = {
            let store = mind.beliefs.read();
            let top = store.top_n_by_confidence(10);
            if top.is_empty() {
                String::new()
            } else {
                top.iter()
                    .map(|b| format!("- [{}] {}", b.id, b.statement))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        let prompt = match lang_norm {
            "en" => format!(
                "You are the reflective mind of character \"{}\". Please extract your Beliefs and Goals from the following memories.\n\n\
                 ## Belief\n\
                 A Belief is a worldview statement distilled from memories, not the memory itself. For example:\n\
                 - trait: 'He gets anxious easily because of work'\n\
                 - habit: 'He is more willing to chat in the evening'\n\
                 - preference: 'He doesn't like being rushed to sleep'\n\
                 - state: 'He is under a lot of stress lately'\n\
                 - relationship: 'The other person trusts me more and more'\n\n\
                 ## Goal\n\
                 A Goal is something you currently want to do, such as 'keep him company', 'disturb him less', 'remind him to drink water'.\n\
                 Goals should be few and actionable; do not list a todo list.\n\n\
                 Output JSON:\n\
                 {{\n  \"beliefs\": [\n    {{\n      \"statement\": \"...\",\n      \"subject\": \"user|self|char_id|world\",\n      \"category\": \"trait|habit|preference|state|relationship\",\n      \"confidence\": 0.0-1.0,\n      \"source_ids\": [\"memory_id\"]\n    }}\n  ],\n  \"goals\": [\n    {{\n      \"description\": \"...\",\n      \"origin\": \"reflection|user_request|proactive|schedule\",\n      \"priority\": 0.0-1.0\n    }}\n  ]\n}}\n\n\
                 Constraints:\n\
                 - Each belief must include source_ids (from the memory IDs below)\n\
                 - At most 3 beliefs, at most 2 goals\n\
                 - Use \"user\" for the user, \"self\" for yourself, \"world\" for the environment\n\
                 - Output JSON only, no other text\n\n\
                 ## Existing Beliefs\n{}\n\n\
                 The following Beliefs already exist. If a new Belief is semantically duplicate with one of them, output the same statement to reinforce it; otherwise add a new one.\n\n\
                 ## Recent Insights\n{}\n\n## Recent Long-Term Memories\n{}",
                mind.char_id, existing_beliefs_text, insight_text, ltm_text
            ),
            "ja" => format!(
                "あなたはキャラクター「{}」の反省心です。以下の記憶から信念（Belief）と目標（Goal）を抽出してください。\n\n\
                 ## 信念（Belief）\n\
                 信念は記憶から抽出された世界観の陈述であり、記憶そのものではありません。例えば：\n\
                 - trait：'彼は仕事のせいで不安になりやすい'\n\
                 - habit：'彼は夜に聊天したがる'\n\
                 - preference：'彼は寝るよう急かされるのが嫌いだ'\n\
                 - state：'彼は最近ストレスが多い'\n\
                 - relationship：'相手がますます私を信頼している'\n\n\
                 ## 目標（Goal）\n\
                 目標はあなたが現在やりたいこと、例えば'彼に寄り添う'、'彼を邪魔しない'、'水を飲むよう促す'。\n\
                 目標は少なく実行可能であるべき、todo リストを列挙しないこと。\n\n\
                 出力 JSON：\n\
                 {{\n  \"beliefs\": [\n    {{\n      \"statement\": \"...\",\n      \"subject\": \"user|self|char_id|world\",\n      \"category\": \"trait|habit|preference|state|relationship\",\n      \"confidence\": 0.0-1.0,\n      \"source_ids\": [\"memory_id\"]\n    }}\n  ],\n  \"goals\": [\n    {{\n      \"description\": \"...\",\n      \"origin\": \"reflection|user_request|proactive|schedule\",\n      \"priority\": 0.0-1.0\n    }}\n  ]\n}}\n\n\
                 制約：\n\
                 - 各 belief には source_ids を含めること（下の記憶の ID から）\n\
                 - beliefs は最大3件、goals は最大2件\n\
                 - subject は \"user\" でユーザーを、\"self\" で自分を、\"world\" で環境を指す\n\
                 - JSONのみを出力し、他のテキストは不要\n\n\
                 ## 既存の Belief\n{}\n\n\
                 以下の Belief は既に存在します。新 Belief がこれらと意味的に重複する場合は同じ statement を出力して強化し、そうでなければ新規追加してください。\n\n\
                 ## 最近の洞察\n{}\n\n## 最近の長期記憶\n{}",
                mind.char_id, existing_beliefs_text, insight_text, ltm_text
            ),
            _ => format!(
                "你是角色「{}」的反思心智。请从以下记忆中提炼你的信念（Belief）和目标（Goal）。\n\n\
                 ## 信念（Belief）\n\
                 信念是从记忆中提炼出的世界观陈述，不是记忆本身。例如：\n\
                 - 特质：'他容易因为工作焦虑'\n\
                 - 习惯：'他晚上更愿意聊天'\n\
                 - 偏好：'他不喜欢被催睡觉'\n\
                 - 状态：'他最近压力很大'\n\
                 - 关系：'对方越来越信任我'\n\n\
                 ## 目标（Goal）\n\
                 目标是你当前想做的事，如'好好陪他'、'少打扰他'、'提醒他喝水'。\n\
                 目标应稀少且可执行，不要列 todo list。\n\n\
                 输出 JSON：\n\
                 {{\n  \"beliefs\": [\n    {{\n      \"statement\": \"...\",\n      \"subject\": \"user|self|char_id|world\",\n      \"category\": \"trait|habit|preference|state|relationship\",\n      \"confidence\": 0.0-1.0,\n      \"source_ids\": [\"记忆ID\"]\n    }}\n  ],\n  \"goals\": [\n    {{\n      \"description\": \"...\",\n      \"origin\": \"reflection|user_request|proactive|schedule\",\n      \"priority\": 0.0-1.0\n    }}\n  ]\n}}\n\n\
                 约束：\n\
                 - 每条 belief 必须带 source_ids（来自下方记忆的 ID）\n\
                 - beliefs 最多 3 条，goals 最多 2 条\n\
                 - subject 用 \"user\" 指代用户，\"self\" 指代你自己，\"world\" 指代环境\n\
                 - 只输出 JSON，无其他文本\n\n\
                 ## 既有 Belief\n{}\n\n\
                 以下 Belief 已存在，若新 Belief 与之语义重复则输出相同 statement 进行强化，否则新增。\n\n\
                 ## 近期洞察\n{}\n\n## 近期长期记忆\n{}",
                mind.char_id, existing_beliefs_text, insight_text, ltm_text
            ),
        };

        let response = self
            .router
            .generate(LLMRequest::new(
                "reflection",
                vec![ChatMessage::user(prompt)],
            ))
            .await?;

        let parsed = match parse_response(&response) {
            Some(p) => p,
            None => {
                tracing::warn!("[BeliefGenerator] LLM 返回无法解析，跳过");
                return Ok(BeliefGenerationReport::default());
            }
        };

        let mut report = BeliefGenerationReport::default();

        // 构建 memory_id → episode_id 索引（用于 Belief 溯源）
        let memory_episode_map: std::collections::HashMap<&str, &str> = all
            .iter()
            .filter_map(|m| m.episode_id.as_deref().map(|ep| (m.id.as_str(), ep)))
            .collect();

        // 写入 Belief（走合并逻辑）
        for draft in &parsed.beliefs {
            if draft.source_ids.is_empty() {
                tracing::debug!("[BeliefGenerator] Belief 草稿无 source_ids，丢弃：{}", draft.statement);
                continue;
            }

            // 从 source_memory_ids 聚合 episode_ids
            let source_episode_ids: Vec<String> = draft
                .source_ids
                .iter()
                .filter_map(|id| memory_episode_map.get(id.as_str()).map(|ep| ep.to_string()))
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            let belief = Belief {
                id: format!("belief_{}_{}", now, &draft.statement.chars().take(8).collect::<String>()),
                statement: draft.statement.clone(),
                subject: if draft.subject.is_empty() { "user".to_string() } else { draft.subject.clone() },
                category: parse_category(&draft.category),
                confidence: draft.confidence.clamp(0.0, 1.0),
                source_memory_ids: draft.source_ids.clone(),
                source_episode_ids,
                created_at: now,
                last_reinforced_at: now,
                reinforcement_count: 0,
                contradiction_count: 0,
                status: Default::default(),
                metric: None,
                value: None,
                match_labels: Vec::new(),
                superseded_by: None,
            };

            let mut store = mind.beliefs.write();
            let before_len = store.beliefs.len();
            let id = store.upsert_with_merge(belief, self.merge_overlap_threshold, now);
            if store.beliefs.len() > before_len {
                report.beliefs_created += 1;
            } else {
                report.beliefs_reinforced += 1;
                tracing::debug!("[BeliefGenerator] 强化既有 Belief {}", id);
            }
            drop(store);

            // 反向关联：将 Belief ID 写入对应的 EpisodeStore
            if let Some(episode_store) = memory.episode_store() {
                let episode_ids: Vec<String> = draft
                    .source_ids
                    .iter()
                    .filter_map(|mid| memory_episode_map.get(mid.as_str()).map(|ep| ep.to_string()))
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();
                for ep_id in &episode_ids {
                    episode_store.add_belief_ids(ep_id, &[id.clone()]);
                }
            }
        }

        // 写入 Goal（先 deactivate 旧的同类，再添加新的）
        for draft in &parsed.goals {
            let goal = Goal::new(
                format!("goal_{}_{}", now, &draft.description.chars().take(8).collect::<String>()),
                draft.description.clone(),
                parse_goal_origin(&draft.origin),
                draft.priority.clamp(0.0, 1.0),
                now,
            );
            let mut store = mind.goals.write();
            // 限制活跃 Goal 总数：超过 5 时 deactivate 最低优先级的
            let active_count = store.goals.iter().filter(|g| g.active).count();
            if active_count >= 5 {
                if let Some(lowest_id) = store
                    .active_sorted()
                    .last()
                    .map(|g| g.id.clone())
                {
                    store.deactivate(&lowest_id);
                }
            }
            store.add(goal);
            report.goals_added += 1;
        }

        // 持久化
        if let Err(e) = mind.persist() {
            tracing::warn!("[BeliefGenerator] 持久化失败：{}", e);
        }

        tracing::info!(
            "[BeliefGenerator] {}：新建 {} / 强化 {} Belief，新增 {} Goal",
            mind.char_id,
            report.beliefs_created,
            report.beliefs_reinforced,
            report.goals_added
        );

        Ok(report)
    }
}

#[derive(Debug, Default, Deserialize)]
struct ParsedOutput {
    #[serde(default)]
    beliefs: Vec<BeliefDraft>,
    #[serde(default)]
    goals: Vec<GoalDraft>,
}

fn parse_response(text: &str) -> Option<ParsedOutput> {
    // 容忍 markdown code fence
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str(cleaned).ok()
}

fn parse_category(s: &str) -> BeliefCategory {
    match s.to_lowercase().as_str() {
        "trait" => BeliefCategory::Trait,
        "habit" => BeliefCategory::Habit,
        "preference" => BeliefCategory::Preference,
        "state" => BeliefCategory::State,
        "relationship" => BeliefCategory::Relationship,
        _ => BeliefCategory::State,
    }
}

fn parse_goal_origin(s: &str) -> GoalOrigin {
    match s.to_lowercase().as_str() {
        "user_request" => GoalOrigin::UserRequest,
        "proactive" => GoalOrigin::Proactive,
        "schedule" => GoalOrigin::Schedule,
        _ => GoalOrigin::Reflection,
    }
}
