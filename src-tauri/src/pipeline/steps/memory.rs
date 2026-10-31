//! 记忆流水线步骤：检索、用户消息保存、AI 回复+长期记忆保存。
//!
//! - [`MemoryRetrievalStep`]：检索步骤（带 MemoryFilter 跨会话过滤）
//! - [`UserMemorySavingRunnable`]：用户消息早期保存
//! - [`MemorySavingRunnable`]：AI 回复 + 长期记忆保存

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::{json, Value};

use crate::cross_character::{build_speaker_prefix, parse_any_speaker_prefix};
use crate::error::VivianResult;
use crate::memory::filter::MemoryFilter;
use crate::memory::user_model::UserModelManager;
use crate::memory::types::current_timestamp;
use crate::memory::{estimate_tokens, staleness_text, MemoryManager, MemoryType, RetrievalStrategy};
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::prompt_modules::MEMORY_CONTEXT_MAX_TOKENS;
use crate::pipeline::state::PipelineState;

/// 记忆重要度引导阈值：importance 不低于该值的关键记忆在注入时打上 [重点] 标记。
const IMPORTANCE_EMPHASIS_THRESHOLD: f64 = 0.7;
/// 基础召回保留条数；关联扩展会在此基础上追加，最终仍受 `MEMORY_CONTEXT_MAX_TOKENS` 约束。
const RECALL_TOP_K: usize = 6;

fn is_authored_context(memory: &crate::memory::MemoryItem) -> bool {
    memory.id.starts_with("seed_") || matches!(
        memory.metadata.get("source").and_then(Value::as_str),
        Some("system_seed" | "environment_preset")
    )
}

fn memory_time_label(memory: &crate::memory::MemoryItem) -> String {
    if memory.metadata["source"] == "system_seed" && memory.metadata["fictional_backstory"] == true {
        format!("前史设定时间：{}", crate::utils::prompt_time::format_prompt_time(memory.timestamp))
    } else if is_authored_context(memory) {
        "预置背景".into()
    } else {
        crate::utils::prompt_time::format_prompt_time(memory.timestamp)
    }
}

// ============================================================================
// MemoryRetrievalStep：原有检索步骤（保留，扩展 MemoryFilter 接入点）
// ============================================================================

/// 记忆检索步骤 —— 从记忆系统检索与用户输入相关的记忆。
///
/// 当注入 `memory_filter` 时，会进行跨会话过滤：
/// - 新会话（问候语或 >1 小时间隔）只保留长期偏好记忆
/// - 老会话保留临时话题 + 长期偏好（按时间衰减权重排序）
pub struct MemoryRetrievalStep {
    pub memory: Arc<MemoryManager>,
    /// 可选的记忆过滤器：跨会话过滤临时话题、保留长期偏好
    pub memory_filter: Option<Arc<RwLock<MemoryFilter>>>,
    /// 检索策略（由配置注入；None 时回退到 Auto）
    pub strategy: Option<RetrievalStrategy>,
    /// Mind 句柄（用于 Attention-weighted 重排序）
    ///
    /// 注入后，检索结果会按 `Score = base × Attention(entity) × Recency × Importance`
    /// 重排序，让当前注意力聚焦的实体相关记忆优先保留。
    /// None 时退回原行为（MemoryFilter 排序）。
    pub mind: Option<Arc<crate::mind::Mind>>,
    /// 可选的 LLM 路由器：注入后启用检索后验证（verifier），
    /// 用小模型过滤掉与问题无关的检索结果，减少幻觉噪声。
    pub router: Option<Arc<crate::providers::ModelRouter>>,
    /// 用户认知模型：注入后启用多跳关联检索。
    ///
    /// 基础向量检索命中表面相似的记忆后，通过 UserModel 的特征/项目展开关联话题，
    /// 再召回"概念相关但字面不相似"的旧记忆并合并，模拟"想起很久以前说过但能联系起来的内容"。
    pub user_model: Option<Arc<UserModelManager>>,
}

impl MemoryRetrievalStep {
    pub fn new(memory: Arc<MemoryManager>) -> Self {
        Self {
            memory,
            memory_filter: None,
            strategy: None,
            mind: None,
            router: None,
            user_model: None,
        }
    }

    /// 构造带 MemoryFilter 的检索步骤。
    pub fn with_filter(
        memory: Arc<MemoryManager>,
        memory_filter: Arc<RwLock<MemoryFilter>>,
    ) -> Self {
        Self {
            memory,
            memory_filter: Some(memory_filter),
            strategy: None,
            mind: None,
            router: None,
            user_model: None,
        }
    }

    /// 构造带检索策略和 MemoryFilter 的检索步骤。
    pub fn with_filter_and_strategy(
        memory: Arc<MemoryManager>,
        memory_filter: Arc<RwLock<MemoryFilter>>,
        strategy: RetrievalStrategy,
    ) -> Self {
        Self {
            memory,
            memory_filter: Some(memory_filter),
            strategy: Some(strategy),
            mind: None,
            router: None,
            user_model: None,
        }
    }

    /// 注入 Mind 句柄，启用 Attention-weighted 重排序
    pub fn with_mind(mut self, mind: Arc<crate::mind::Mind>) -> Self {
        self.mind = Some(mind);
        self
    }

    /// 注入 LLM 路由器，启用检索后验证（verifier）
    pub fn with_router(mut self, router: Arc<crate::providers::ModelRouter>) -> Self {
        self.router = Some(router);
        self
    }

    /// 注入用户认知模型，启用多跳关联检索
    pub fn with_user_model(mut self, user_model: Arc<UserModelManager>) -> Self {
        self.user_model = Some(user_model);
        self
    }

    fn build_retrieval_query(state: &PipelineState) -> crate::memory::recall_policy::RecallQuery {
        let resolved = state.resolved_user_input.trim();
        let has_resolution = !resolved.is_empty() && resolved != state.user_input.trim();
        let focus = if has_resolution { resolved } else { &state.user_input };
        let context = if !has_resolution
            && crate::pipeline::message_context::retrieval_needs_context(&state.user_input) {
            crate::pipeline::message_context::recent_retrieval_topic(&state.messages, &state.user_input)
        } else { String::new() };
        let mut query = crate::memory::recall_policy::RecallQuery::new(focus, &context);
        query.continuation = !has_resolution && crate::pipeline::message_context::is_short_followup(&state.user_input);
        query
    }

    /// 收集多跳关联检索的查询词。
    ///
    /// 优先使用 FastSemantic 产出的话题标签（语义分类，最贴近"用户此刻在聊什么"），
    /// 不足时回退到对用户输入做轻量分词。返回的小写词用于在 UserModel 中匹配特征/项目。
    fn collect_query_terms(state: &PipelineState) -> Vec<String> {
        let mut terms: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Some(fp) = &state.fast_perception {
            for t in fp.topics.iter().take(4) {
                let label = t.label.trim().to_lowercase();
                if !label.is_empty() && seen.insert(label.clone()) {
                    terms.push(label);
                }
            }
        }

        // 话题标签不足时，从用户输入提取关键词补充信号
        if terms.len() < 2 && !state.user_input.is_empty() {
            for tok in crate::memory::tokenize::tokenize(&state.user_input) {
                let w = tok.to_lowercase();
                if w.len() >= 2 && seen.insert(w.clone()) && terms.len() < 6 {
                    terms.push(w);
                }
            }
        }

        terms
    }
}

/// Typed, request-local base recall; never serialized into the prompt or shared globally.
pub(super) struct PrefetchedMemory {
    query: crate::memory::recall_policy::RecallQuery,
    items: Vec<crate::memory::MemoryItem>,
    budget: crate::memory::retrieval_budget::Budget,
    ready_at: std::time::Instant,
}

#[async_trait]
impl Runnable for MemoryRetrievalStep {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);
        let base = self.prefetch(&mut state).await;
        self.finish_recall(state, base).await
    }
}

impl MemoryRetrievalStep {
    pub(super) async fn prefetch(&self, state: &mut PipelineState) -> Option<PrefetchedMemory> {
        if state.user_input.is_empty() {
            return None;
        }

        // FLARE 式按需检索：上游 QueryRewriteStep 判断为无需检索时直接跳过
        if state
            .metadata
            .get("skip_memory_retrieval")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let reason = state
                .metadata
                .get("skip_retrieval_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::debug!(
                "[MemoryRetrievalStep] 按需检索：跳过检索（原因：{}）",
                reason
            );
            state.metadata["memory_retrieval_skipped"] = json!(true);
            return None;
        }

        let mut retrieval_budget = crate::memory::retrieval_budget::Budget::new(std::time::Duration::from_millis(1800));
        let retrieval_query = Self::build_retrieval_query(&state);

        let strategy = self.strategy.unwrap_or(RetrievalStrategy::Auto);
        // 兼容旧库：原先的 OS/话题/工具记录可能占据候选位，先多取再按陪伴记忆边界过滤。
        let limit = 24;
        let items = match retrieval_budget.run("base", std::time::Duration::from_millis(900),
            || self.memory.search_turn_memories(&retrieval_query, strategy, limit)).await {
            Some(Ok(items)) => items,
            result => { tracing::warn!(?result, "[MemoryRetrieval] base arm unavailable; continuing without fabricated recall"); Vec::new() }
        };

        Some(PrefetchedMemory { query: retrieval_query, items, budget: retrieval_budget,
            ready_at: std::time::Instant::now() })
    }

    pub(super) async fn finish_recall(&self, mut state: PipelineState, base: Option<PrefetchedMemory>) -> VivianResult<Value> {
        let Some(PrefetchedMemory { query: retrieval_query, items, mut budget, ready_at }) = base else {
            return Ok(state.to_json());
        };
        state.metadata["memory_query_components"] = json!(retrieval_query);
        let retrieval_query = retrieval_query.effective_text().to_owned();
        budget.exclude_dependency_wait(ready_at);
        let mut retrieval_budget = budget;
        let strategy = self.strategy.unwrap_or(RetrievalStrategy::Auto);
        let strategy_name = format!("{:?}", strategy);
        let limit = 24;

        // 过滤掉与当前用户输入相同的记忆条目：
        // UserMemorySaving（步骤2）刚将当前输入写入 ShortTerm，
        // 向量检索必然命中，造成冗余上下文。剥掉 [X says to Y] 前缀后精确比较。
        let items: Vec<_> = items
            .into_iter()
            .filter(|m| {
                let stripped = parse_any_speaker_prefix(&m.content).0;
                let trimmed = stripped.trim();
                trimmed != state.user_input.trim()
                    && (state.resolved_user_input.is_empty()
                        || trimmed != state.resolved_user_input.trim())
            })
            .collect();

        // 应用实体相关性过滤（修复 M6）：
        // 过滤掉"其他角色之间"的记忆，避免 LLM 把室友间对话误记为用户对话。
        // 保留：无 metadata 的普通记忆、用户直接对话、自己参与的跨角色对话、自己旁观的记忆。
        // 过滤发生在 MemoryFilter 之前，避免无关记忆挤占基础召回配额。
        let char_id = self.memory.char_id().to_string();
        let items: Vec<_> = items
            .into_iter()
            .filter(|m| crate::memory::precision_filter::is_relevant_to_entity(m, &char_id)
                && crate::memory::companion_policy::is_recallable(m, current_timestamp()))
            .collect();
        let items = crate::memory::companion_policy::dedup_recall(items, current_timestamp(), limit);

        // 应用 MemoryFilter：跨会话过滤临时话题、保留长期偏好
        let (mut filtered_items, session_id, new_session) = if let Some(filter_arc) = &self.memory_filter {
            let mut filter = filter_arc.write();
            let new_session = filter.is_new_session(&state.user_input, &char_id);
            let scored = filter.get_filtered_memories(
                &state.user_input,
                items,
                RECALL_TOP_K,
                &char_id,
            );
            let session_id = filter.last_session_id.clone();
            (
                scored.into_iter().map(|(m, _)| m).collect::<Vec<_>>(),
                session_id,
                new_session,
            )
        } else {
            // 未注入过滤器：保留基础召回条数
            (items.into_iter().take(RECALL_TOP_K).collect(), None, false)
        };

        // ── 多跳关联检索 ──
        // 注入用户认知模型时启用：基础检索命中表面相似的记忆后，用当前话题展开
        // UserModel 特征/项目的关联话题，再召回"概念相关但字面不相似"的旧记忆并合并。
        // 这模拟"想起很久以前说过但能联系起来的内容"，是普通向量检索无法做到的。
        let mut multi_hop_applied = false;
        if let Some(um) = &self.user_model {
            let terms = Self::collect_query_terms(&state);
            if !terms.is_empty() {
                let expanded = um.expand_related_topics(&terms);
                if !expanded.is_empty() {
                    multi_hop_applied = true;
                    let query = format!("关联话题：{}", expanded.join("、"));
                    // 控制补充量：多跳结果追加在基础结果之后，限制条数避免挤占 token 预算，
                    // 且能让这些关联记忆自然进入后续截断窗口（总条数 ≤ 5 + 4）。
                    let extra = retrieval_budget.run("query_expansion", std::time::Duration::from_millis(200),
                        || self.memory.search_memories(&query, strategy, 4)).await.and_then(Result::ok).unwrap_or_default();
                    // 按 id 去重合并，避免与基础检索结果重复
                    let existing: std::collections::HashSet<&str> =
                        filtered_items.iter().map(|m| m.id.as_str()).collect();
                    let to_add: Vec<_> = extra
                        .into_iter()
                        .filter(|m| !existing.contains(m.id.as_str()))
                        .collect();
                    filtered_items.extend(to_add);
                    tracing::debug!(
                        "[MemoryRetrievalStep] 多跳关联检索：话题 {:?} 展开为 {:?}，补入 {} 条关联记忆",
                        terms,
                        expanded,
                        filtered_items.len()
                    );
                }
            }
        }

        // ── 图谱概念路（①）──
        // 概念层由 ConsolidationStage3.5 写入 KnowledgeGraph 后，query 话题词可通过
        // 图谱 Concept 实体展开到其关联的 evidence 记忆，实现"概念相关但字面不相似"的召回。
        let mut graph_concept_applied = false;
        let concept_seeds = Self::collect_query_terms(&state);
        if !concept_seeds.is_empty() {
            let terms: Vec<&str> = concept_seeds.iter().map(|s| s.as_str()).collect();
            let concept_ids = self
                .memory
                .knowledge_graph()
                .find_concept_memories(&terms, 4);
            if !concept_ids.is_empty() {
                let existing: std::collections::HashSet<&str> =
                    filtered_items.iter().map(|m| m.id.as_str()).collect();
                let concept_memories = self.memory.get_memories_by_ids(&concept_ids);
                let to_add: Vec<_> = concept_memories
                    .into_iter()
                    .filter(|m| !existing.contains(m.id.as_str()))
                    .collect();
                if !to_add.is_empty() {
                    graph_concept_applied = true;
                    let add_count = to_add.len();
                    filtered_items.extend(to_add);
                    tracing::debug!(
                        "[MemoryRetrievalStep] 图谱概念路：话题 {:?} 命中概念，补入 {} 条概念记忆",
                        concept_seeds,
                        add_count
                    );
                }
            }
        }

        // ── 结果侧回跳（②）──
        // 基础检索命中后，用命中记忆携带的主题标签（topic:xxx / concept:xxx）作为第二跳种子，
        // 再检索"概念相关但第一轮未命中"的记忆并合并。与查询侧多跳互补：
        // 查询侧从当前话题展开，结果侧从"已命中记忆"往回跳，形成迭代关联。
        let mut result_reexpand_applied = false;
        let reexpand_seeds: Vec<String> = filtered_items
            .iter()
            .flat_map(|m| {
                let mut tags: Vec<String> = Vec::new();
                for t in &m.tags {
                    if let Some(rest) = t.strip_prefix("topic:") {
                        tags.push(rest.to_string());
                    } else if let Some(rest) = t.strip_prefix("concept:") {
                        tags.push(rest.to_string());
                    }
                }
                tags
            })
            .collect();
        if !reexpand_seeds.is_empty() {
            let query = format!("关联主题：{}", reexpand_seeds.join("、"));
            let extra = retrieval_budget.run("result_expansion", std::time::Duration::from_millis(200),
                || self.memory.search_memories(&query, strategy, 4)).await.and_then(Result::ok).unwrap_or_default();
            let existing: std::collections::HashSet<&str> =
                filtered_items.iter().map(|m| m.id.as_str()).collect();
            let to_add: Vec<_> = extra
                .into_iter()
                .filter(|m| !existing.contains(m.id.as_str()))
                .filter(|m| {
                    let stripped = parse_any_speaker_prefix(&m.content).0;
                    let trimmed = stripped.trim();
                    trimmed != state.user_input.trim()
                        && (state.resolved_user_input.is_empty()
                            || trimmed != state.resolved_user_input.trim())
                })
                .collect();
            if !to_add.is_empty() {
                result_reexpand_applied = true;
                let add_count = to_add.len();
                filtered_items.extend(to_add);
                tracing::debug!(
                    "[MemoryRetrievalStep] 结果侧回跳：由命中记忆标签 {:?} 补入 {} 条关联记忆",
                    reexpand_seeds,
                    add_count
                );
            }
        }

        // Expansion arms must obey the same entity/current-turn/session constraints
        // as the base arm. Applying the boundary here prevents graph or multi-hop
        // results from bypassing access filters.
        filtered_items.retain(|m| {
            if !crate::memory::precision_filter::is_relevant_to_entity(m, &char_id) {
                return false;
            }
            if !crate::memory::companion_policy::is_recallable(m, current_timestamp()) {
                return false;
            }
            let stripped = parse_any_speaker_prefix(&m.content).0;
            let trimmed = stripped.trim();
            trimmed != state.user_input.trim()
                && (state.resolved_user_input.is_empty()
                    || trimmed != state.resolved_user_input.trim())
        });
        if let Some(filter_arc) = &self.memory_filter {
            let mut filter = filter_arc.write();
            filtered_items = filter.filter_for_session(filtered_items, new_session);
            filtered_items.sort_by(|a, b| {
                crate::memory::filter::MemoryFilter::calculate_memory_weight(b)
                    .partial_cmp(&crate::memory::filter::MemoryFilter::calculate_memory_weight(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        let before_admission = filtered_items.len();
        filtered_items = crate::memory::recall_policy::filter_for_query(filtered_items, &retrieval_query);
        state.metadata["memory_admission"] = json!({"considered":before_admission,
            "admitted":filtered_items.len(),"rejected":before_admission-filtered_items.len()});
        // 融合路径可能返回同一事实的原话、摘要和重复副本；运行时想法不能成为对话证据。
        filtered_items = crate::memory::companion_policy::dedup_recall(
            filtered_items, current_timestamp(), 5,
        );

        // ── 检索后验证（verifier）──
        // 若注入了 router，用小模型过滤掉与用户问题无关的检索结果，减少幻觉噪声。
        // 记忆数 ≤ 2 时 verifier 自动跳过（开销不值得），LLM 不可用时降级为全部保留。
        if let Some(router) = &self.router {
            if filtered_items.len() > 2 {
                let llm_ref: Arc<dyn crate::memory::verifier::VerifierLlmClient> = router.clone();
                let result = retrieval_budget.run("verifier", std::time::Duration::from_millis(400), ||
                    crate::memory::verifier::verify_retrieval(&filtered_items, &state.user_input, Some(&llm_ref))).await;
                if let Some(result) = result.filter(|result| !result.skipped) {
                    tracing::debug!(
                        "[MemoryRetrievalStep] verifier 过滤：{} → {} 条",
                        filtered_items.len(),
                        result.verified_indices.len()
                    );
                    filtered_items = result
                        .verified_indices
                        .iter()
                        .filter_map(|&i| filtered_items.get(i).cloned())
                        .collect();
                }
            }
        }

        // ── Attention-weighted 重排序 ──
        // 若注入了 Mind，按 `Score = base × Attention(entity) × Recency × Importance` 重排序。
        // base 来自 MemoryFilter 已计算的权重（若未注入 filter 则用 1.0）。
        // Attention(entity) 从记忆的 tags/content 中提取实体，查 attention 权重。
        // 这样当前注意力聚焦的实体相关记忆优先保留。
        let attention_applied = if let Some(mind) = &self.mind {
            let attention = mind.attention_snapshot();
            if attention.focus.is_empty() {
                false
            } else {
                let now_ts = current_timestamp();
                let mut scored: Vec<(crate::memory::types::MemoryItem, f64)> = filtered_items
                    .into_iter()
                    .map(|m| {
                        let att = attention_weight_of_memory(&m, &attention);
                        let recency = recency_factor(m.timestamp, now_ts);
                        let base = m
                            .metadata
                            .get("fused_score")
                            .or_else(|| m.metadata.get("combined_score"))
                            .and_then(|v| v.as_f64())
                            .unwrap_or(1.0)
                            .max(0.01);
                        let score = base * m.importance.max(0.01) * att * recency;
                        (m, score)
                    })
                    .collect();
                scored.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                });
                filtered_items = scored.into_iter().map(|(m, _)| m).collect();
                true
            }
        } else {
            false
        };

        // 组装记忆文本（自然语言化：时间 + 类型 + 说话者 + 内容；数值元数据不进 prompt）
        let now = current_timestamp();
        let mut memory_parts: Vec<String> = Vec::new();
        let mut memory_details: Vec<serde_json::Value> = Vec::new();
        for (index, mem) in filtered_items.iter().enumerate() {
            // Give the strongest two episodes more room for context; the remaining entries stay
            // compact so the expanded total still fits the memory budget.
            let content = crate::utils::truncate_chars(&mem.content, if index < 2 { 1200 } else { 240 });
            let authored_context = is_authored_context(mem);
            let type_label = if mem.metadata["fictional_backstory"] == true {
                format!("{}；仅在用户问及或当前话题确有需要时回应，不主动以此开启或延续话题，也不当作与当前用户的共同经历",
                    crate::memory::provenance::recall_label(&mem.metadata))
            } else if authored_context { "角色预置背景；非运行期对话".to_string() }
                else { crate::memory::provenance::recall_label(&mem.metadata).to_string() };
            let mut attribution = crate::memory::provenance::attribution(&mem.metadata);
            if let Some(timestamp) = mem.metadata["source_timestamp"].as_f64().filter(|t| t.is_finite() && *t > 0.0) {
                attribution.push_str(&format!("；原话时间：{}", crate::utils::prompt_time::format_prompt_time(timestamp)));
            }
            let content = if mem.metadata["utterance_format"] == "plain" { content } else {
                parse_any_speaker_prefix(&content).0
            };
            let role_prefix = if attribution.is_empty() { String::new() } else { format!("{}：", attribution) };
            // Escape retrieved text; it remains evidence inside the retrieval data boundary.
            let content = content.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            let time = memory_time_label(mem);
            let stale_hint = staleness_text(mem.timestamp, now)
                .map(|s| format!(" [{}]", s))
                .unwrap_or_default();
            // Vector-only relevance is not proof of a remembered detail. Batch
            // rank and age never become confidence labels; disputes stay separate.
            let confidence_hint = {
                if mem.metadata["recall_admission"]["kind"] == "semantic" {
                    " [语义关联，细节需核对]"
                } else {
                    ""
                }
            };
            // 证据可信度标记：来自 `memory::evidence` 的 reinforcement / disputation 双时钟。
            //
            // 这套评分一直只活在存储层，prompt 层看不到——于是模型可能拿一条
            // 已经被用户反驳过的记忆去说「我记得你说过……」，用户回一句
            // 「我什么时候说过这个？」就出戏了。
            //
            // 只标**负面**证据（正面不加标记）：绝大多数记忆都处于 Pending（待定），
            // 给它们都打标记等于没打；真正需要模型改变行为的只有"被否认过"这一种。
            let evidence_hint = {
                use crate::memory::evidence::{derive_status, EvidenceStatus};
                match derive_status(mem, now) {
                    // score ≤ ARCHIVE_THRESHOLD(-2.0)：净负面证据压过了正面证据
                    EvidenceStatus::ArchiveCandidate => " [存疑]",
                    // 刚被反驳、正面证据还在宽限期内恢复 → 同样不该主动引用
                    _ if mem.rebuttal_grace_remaining > 0 => " [刚被否认]",
                    _ => "",
                }
            };
            // 重要度引导：高重要度的关键记忆打上 [重点] 标记，引导 LLM 作答时更重视
            let emphasis_mark = if mem.importance >= IMPORTANCE_EMPHASIS_THRESHOLD {
                " [重点]"
            } else {
                ""
            };
            // 自然语言化格式：时间 + 类型标签 + 说话者 + 内容。
            // 不再输出 imp=x.xx / mood=... 等数据库元数据——重要性由 [重点] 标记表达，
            // 数值符号只会把"自然回忆"变成"查表结果"。
            memory_parts.push(format!(
                "[{} | {}] {}{}{}{}{}{}",
                time, type_label, role_prefix, content, emphasis_mark, stale_hint,
                confidence_hint, evidence_hint
            ));
            memory_details.push(json!({
                "id": mem.id,
                "content_snippet": mem.content.chars().take(120).collect::<String>(),
                "importance": mem.importance,
                "timestamp": mem.timestamp,
                "tags": mem.tags,
                "granularity": mem.granularity,
                "age_hours": MemoryFilter::memory_age_hours(mem),
            }));
        }

        // 按 MEMORY_CONTEXT_MAX_TOKENS 截断记忆上下文
        // 前面的更重要（MemoryFilter 已按权重排序 + Attention 重排序），从前往后保留
        let mut kept_count = 0usize;
        let mut used_tokens = 0usize;
        for part in &memory_parts {
            let t = estimate_tokens(part);
            if used_tokens + t > MEMORY_CONTEXT_MAX_TOKENS && kept_count > 0 {
                break;
            }
            used_tokens += t;
            kept_count += 1;
        }
        if kept_count < memory_parts.len() {
            tracing::debug!(
                "[MemoryRetrievalStep] 记忆上下文按 token 截断：{} → {} 条（{} tokens）",
                memory_parts.len(),
                kept_count,
                used_tokens
            );
            memory_parts.truncate(kept_count);
            memory_details.truncate(kept_count);
            filtered_items.truncate(kept_count);
        }
        state.memory_text = memory_parts.join("\n");
        if filtered_items.iter().any(is_authored_context) {
            state.memory_text = format!("Authored character/background entries are fictional context, not evidence of this user's identity, words, past interactions or current environment. Their timestamps mark initialization, not an event. Use real dialogue and observations for shared history; never tell the user they said or did something based only on a preset.\n{}", state.memory_text);
        }

        // 仅真实记忆可触发强命中；预置角色剧情不能抑制事实核验。
        // 命中时向主 prompt 注入"优先用记忆回答"引导，并标记 memory_strong_hit，
        // 供 prompt 构建阶段抑制本轮无谓的 web_search —— 避免角色为了"查询"去外部
        // 搜索自己本该记得的内容（如用户、创造者等）。
        let strong_hit = filtered_items.iter().any(|m| {
            if is_authored_context(m) { return false; }
            let relevance = m
                .metadata
                .get("fused_score")
                .or_else(|| m.metadata.get("combined_score"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            relevance >= 0.45 && m.importance >= 0.75
        });
        if strong_hit {
            state.metadata["memory_strong_hit"] = json!(true);
            // 规则类提示统一英文
            let note = "(Note: the memories above directly relate to the user's question — answer from them first; do NOT call web_search or other lookup tools for this.)";
            state.memory_text = format!("{}\n{}", note, state.memory_text);
        }

        // 检索命中后更新热度（best-effort，失败不影响主流程）
        let hit_ids: Vec<String> = filtered_items.iter().map(|m| m.id.clone()).collect();
        if let Err(e) = self.memory.bump_visits(&hit_ids) {
            tracing::warn!("[MemoryRetrievalStep] 热度更新失败: {}", e);
        }

        state.memories = filtered_items.iter().map(|m| m.content.clone()).collect();
        state.raw_semantic_memory = filtered_items
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "content": m.content,
                    "importance": m.importance,
                    "timestamp": m.timestamp,
                    "tags": m.tags,
                    "granularity": m.granularity,
                    "source": m.metadata.get("source").and_then(Value::as_str).unwrap_or_default(),
                })
            })
            .collect();
        state.memory_vars["_memory_search_query"] = json!(retrieval_query);
        state.memory_vars["_memory_search_strategy"] = json!(strategy_name);
        state.memory_vars["_memory_retrieval_results"] = json!(memory_details);
        state.metadata["memory_count"] = json!(state.memories.len());
        state.metadata["memory_filter_enabled"] = json!(self.memory_filter.is_some());
        state.metadata["memory_filter_session_id"] = json!(session_id);
        state.metadata["memory_filter_new_session"] = json!(new_session);
        state.metadata["memory_attention_reranked"] = json!(attention_applied);
        state.metadata["memory_multi_hop"] = json!(multi_hop_applied);
        state.metadata["memory_graph_concept"] = json!(graph_concept_applied);
        state.metadata["memory_result_reexpand"] = json!(result_reexpand_applied);

        // Separate external user evidence: never feed these into the character's
        // episodic memories, attention history, visit counters or fact extraction.
        let embedding = self.memory.embedding();
        let query = retrieval_query.clone();
        match retrieval_budget.run("quick_notes", std::time::Duration::from_millis(150), ||
            tokio::task::spawn_blocking(move || crate::user_quick_notes::search(&query, 3, Some(embedding)))).await {
            Some(Ok(Ok(notes))) => {
                state.memory_text.push_str(&crate::user_quick_notes::context(&notes));
                state.memory_vars["_user_quick_notes"] = json!(notes);
            }
            result => tracing::debug!(?result, "用户随手记检索跳过或失败，继续角色记忆检索结果"),
        }

        state.metadata["memory_retrieval_timing"] = retrieval_budget.report();
        tracing::debug!(timing=?state.metadata["memory_retrieval_timing"], "[MemoryRetrieval] stage timing");
        Ok(state.to_json())
    }
}

// ============================================================================
// Attention-weighted 重排序辅助函数
// ============================================================================

/// 计算记忆条目的 Attention 权重（0.1-2.0）
///
/// 从记忆的 tags（user/assistant/角色ID）和 content（jieba 分词实体）中
/// 提取实体，取所有匹配实体的最大 attention 权重，然后映射到 [0.1, 2.0] 区间：
/// - 无匹配：0.5（中性，不惩罚也不奖励）
/// - 权重 0.0：0.1（强降权）
/// - 权重 1.0：2.0（强升权）
fn attention_weight_of_memory(
    m: &crate::memory::types::MemoryItem,
    attention: &crate::mind::attention::Attention,
) -> f64 {
    let mut max_weight: f32 = 0.0;
    let mut any_match = false;

    // 1. 从 tags 提取实体（user/assistant/角色ID）
    for tag in &m.tags {
        let entity = match tag.as_str() {
            "user" => "user",
            "assistant" | "vivian" | "nana" => tag.as_str(),
            _ => continue,
        };
        let w = attention.weight_of(entity);
        if w > max_weight {
            max_weight = w;
        }
        any_match = true;
    }

    // 2. 从 content 用 jieba 分词提取实体
    let tokens = crate::memory::tokenize::tokenize(&m.content);
    for token in tokens.iter() {
        let token_lower = token.to_lowercase();
        if token_lower.len() < 2 {
            continue;
        }
        let w = attention.weight_of(&token_lower);
        if w > max_weight {
            max_weight = w;
            any_match = true;
        }
    }

    // 3. 检查 metadata 中的 speaker/listener（跨角色对话记忆）
    if let Some(meta) = m.metadata.as_object() {
        for key in &["speaker", "listener"] {
            if let Some(s) = meta.get(*key).and_then(|v| v.as_str()) {
                let w = attention.weight_of(s);
                if w > max_weight {
                    max_weight = w;
                    any_match = true;
                }
            }
        }
    }

    if !any_match {
        return 0.5; // 中性
    }
    // 映射：weight 0.0 → 0.1，weight 1.0 → 2.0
    0.1 + (max_weight as f64) * 1.9
}

/// 时间衰减因子（0.3-1.0）
///
/// 1 小时内 → 1.0，1 天内 → 0.7，1 周内 → 0.5，更久 → 0.3
fn recency_factor(timestamp: f64, now: f64) -> f64 {
    if timestamp <= 0.0 || now <= 0.0 {
        return 0.3;
    }
    let age_secs = (now - timestamp).max(0.0);
    let hour = 3600.0;
    let day = 24.0 * hour;
    let week = 7.0 * day;
    if age_secs < hour {
        1.0
    } else if age_secs < day {
        0.7
    } else if age_secs < week {
        0.5
    } else {
        0.3
    }
}

// ============================================================================
// UserMemorySavingRunnable：用户消息早期保存
// ============================================================================

/// 用户消息早期保存 Runnable —— 在后台将用户消息保存到记忆系统。
///
/// 使用共指消解后的输入作为存储文本，便于后续向量/BM25 检索。
/// 通过 `with_memory` 注入 `MemoryManager`，未注入时跳过保存。
pub struct UserMemorySavingRunnable {
    pub memory_manager: Option<Arc<MemoryManager>>,
}

impl UserMemorySavingRunnable {
    pub fn new() -> Self {
        Self {
            memory_manager: None,
        }
    }

    pub fn with_memory(memory_manager: Arc<MemoryManager>) -> Self {
        Self {
            memory_manager: Some(memory_manager),
        }
    }
}

impl Default for UserMemorySavingRunnable {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for UserMemorySavingRunnable {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 启动问候等内部指令跳过保存：避免把合成的问候指令当作"用户消息"写入记忆。
        if state
            .metadata
            .get("skip_memory_save")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(state.to_json());
        }

        if !state.should_respond {
            return Ok(state.to_json());
        }

        let memory_manager = match &self.memory_manager {
            Some(m) => m,
            None => {
                tracing::debug!("[UserMemorySaving] memory_manager 未注入，跳过保存");
                return Ok(state.to_json());
            }
        };

        // 解析 [X says to me] 前缀：跨角色消息需剥离前缀并记录真实说话者。
        // 检测基于原始输入（共指消解结果可能已不含前缀）；普通用户消息仍优先用消解后文本。
        let (stripped_input, speaker_id) =
            crate::cross_character::parse_speaker_prefix(&state.user_input);
        let is_cross_character = speaker_id != "user";

        // 优先使用共指消解后的输入作为存储文本（跨角色消息用剥离前缀后的原文）
        let storage_text = if is_cross_character {
            stripped_input
        } else if !state.resolved_user_input.is_empty() {
            state.resolved_user_input.clone()
        } else {
            state.user_input.clone()
        };

        if storage_text.trim().is_empty() {
            return Ok(state.to_json());
        }

        // user_emotion 由 LLM 在 JSON 中返回，写入 tags 伴随记忆持久化
        // 注意：此处 user_emotion 尚未被 ResponseParsingRunnable 设置（步骤顺序），
        // 后续 MemorySavingRunnable 保存 AI 回复时才使用真实的 ai_emotion。
        // 用户消息保存只用 importance_user，emotion 标签用 neutral 兜底。
        let user_emotion_label = if state.user_emotion.is_empty() {
            "neutral"
        } else {
            state.user_emotion.as_str()
        };

        if is_cross_character {
            // 跨角色消息：以真实说话者元数据 + cross_character 标签写入 ShortTerm，
            // 图谱按 metadata.speaker 归类着色，避免误判为用户发言（绿色节点）。
            let char_id = memory_manager.char_id().to_string();
            let tags = vec![
                "short_term".to_string(),
                "cross_character".to_string(),
                "dialogue_turn".to_string(),
                user_emotion_label.to_string(),
            ];
            let metadata = serde_json::json!({
                "channel": "cross_character",
                "speaker": speaker_id,
                "listener": char_id,
                "perspective": "speaker",
                "knowledge_source": "heard",
                "conversation_id": state.conversation_id,
            });
            // 检查是否已有前缀（防御性），没有则添加
            let (_, existing_spk, _) = parse_any_speaker_prefix(&storage_text);
            let content_to_store = if existing_spk.is_some() {
                storage_text.clone()
            } else {
                let prefix = build_speaker_prefix(&speaker_id, &char_id, &char_id);
                format!("{} {}", prefix, storage_text)
            };
            if let Err(e) = memory_manager
                .add_memory_with_metadata(
                    &content_to_store,
                    MemoryType::ShortTerm,
                    state.importance_user,
                    tags,
                    metadata,
                )
                .await
            {
                tracing::warn!("[UserMemorySaving] 跨角色消息写入失败: {}", e);
            }
        } else {
            // 非跨角色消息：显式传入 current_channel，避免 save_context 硬编码 'direct'
            let char_id = memory_manager.char_id().to_string();
            let channel = if state.current_channel.is_empty() {
                "direct"
            } else {
                state.current_channel.as_str()
            };
            // 广播：listener 记 "all"，落成 "[User says to everyone] …"。
            // 若沿用 listener=char_id，每个角色都会把这句话存成"他单独对我说了一遍"，
            // 而广播的真实语义是"他当众说了一次，我们都在场"。
            let is_broadcast = channel == "broadcast";
            let user_meta = serde_json::json!({
                "channel": channel,
                "speaker": "user",
                "listener": if is_broadcast { "all" } else { char_id.as_str() },
                "perspective": "speaker",
                "knowledge_source": if is_broadcast { "broadcast" } else { "direct" },
            });
            // Keep the actual user wording for evidence; resolved references are supplementary.
            let mut user_meta = user_meta;
            user_meta["conversation_id"] = json!(state.conversation_id);
            let prefix = build_speaker_prefix("user", if is_broadcast { "all" } else { &char_id }, &char_id);
            let content = format!("{} {}", prefix, state.user_input.trim());
            match memory_manager.add_memory_with_metadata(
                &content, MemoryType::ShortTerm, state.importance_user,
                vec!["short_term".into(), "user".into(), "dialogue_turn".into(), user_emotion_label.into()],
                user_meta,
            ).await {
                Ok(item) => {
                    state.metadata["growth_source_memory_id"] = json!(item.id);
                    state.metadata["growth_source_timestamp"] = json!(item.timestamp);
                    state.metadata["growth_source_content"] = json!(item.content);
                }
                Err(e) => tracing::warn!("[UserMemorySaving] 保存失败: {}", e),
            }
        }
        state.metadata["user_memory_saved"] = json!(true);

        Ok(state.to_json())
    }
}

// ============================================================================
// MemorySavingRunnable：AI 回复 + 长期记忆保存
// ============================================================================

/// 记忆保存 Runnable —— 保存 AI 回复与 LLM 生成的长期记忆。
///
/// 设置字段：`memory_saved` / `generation_status`。
/// 通过 `with_memory` 注入 `MemoryManager` 调用记忆系统保存；`dialogue_manager`
/// 字段已声明但当前未注入（保留待后续接入对话历史持久化）。
pub struct MemorySavingRunnable {
    pub dialogue_manager: Option<Arc<crate::dialogue::DialogueManager>>,
    pub memory_manager: Option<Arc<MemoryManager>>,
}

impl MemorySavingRunnable {
    pub fn new() -> Self {
        Self {
            dialogue_manager: None,
            memory_manager: None,
        }
    }

    pub fn with_memory(memory_manager: Arc<MemoryManager>) -> Self {
        Self {
            dialogue_manager: None,
            memory_manager: Some(memory_manager),
        }
    }

    /// 防御性清理：若 text 看起来是 JSON 对象，尝试提取其中 text 字段。
    ///
    /// 作为 ResponseParsingRunnable 之后的二次防御：当上游因为异常路径
    /// 把原始 JSON 串透传到 memory_saving 时，这里确保不会把
    /// `{"text":"晚安","motion":"idle",...}` 这种字符串写进记忆库。
    pub fn strip_json_if_any(text: &str) -> String {
        let s = text.trim();
        if !(s.starts_with('{') && s.ends_with('}')) {
            return text.to_string();
        }
        if let Ok(obj) = serde_json::from_str::<Value>(s) {
            if let Some(obj_map) = obj.as_object() {
                for key in ["text", "reply", "content", "output"] {
                    if let Some(Value::String(inner)) = obj_map.get(key) {
                        if !inner.trim().is_empty() {
                            return inner.trim().to_string();
                        }
                    }
                }
            }
        }
        text.to_string()
    }

    /// Only the finalized character turn is a spoken memory source.
    fn collect_ai_responses(state: &PipelineState) -> Vec<String> {
        let final_text = crate::pipeline::spoken_reply::text(&state.text);
        if final_text.is_empty() { vec![] } else { vec![final_text] }
    }
}

impl Default for MemorySavingRunnable {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for MemorySavingRunnable {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 启动问候等内部指令跳过保存：问候有独立的对话/记忆后处理，避免重复写入。
        if state
            .metadata
            .get("skip_memory_save")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(state.to_json());
        }

        if !state.should_respond || state.is_command {
            return Ok(state.to_json());
        }

        // 收集 AI 回复
        let ai_responses = Self::collect_ai_responses(&state);

        // 1. 保存到对话管理器（保留原始用户表述）
        // TODO: 注入 dialogue_manager 后启用（dialogue_manager 字段已声明但当前始终为 None）
        // if let Some(dm) = &self.dialogue_manager {
        //     dm.add_message(ChatMessage::user(&state.user_input));
        //     if let Some(last) = ai_responses.last() {
        //         let clean = Self::strip_json_if_any(last);
        //         dm.add_message(ChatMessage::assistant(&clean));
        //     }
        // }

        // 2. 保存到记忆系统（使用 save_context 统一入口）
        if let Some(memory_manager) = &self.memory_manager {
            // ai_emotion 由 LLM 在 JSON 中返回，写入 tags 伴随记忆持久化
            let ai_emotion_label = state
                .emotion
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("neutral");

            // 防御性清理：避免原始 JSON 串写入长期记忆
            let mut clean_responses: Vec<String> = ai_responses
                .iter()
                .map(|r| Self::strip_json_if_any(r))
                .collect();
            if let Some(sticker) = &state.sticker {
                let metadata = json!({"sticker":sticker});
                if let Some(last) = clean_responses.last_mut() {
                    *last = crate::memory::extraction_queue::content_with_sticker(last, &metadata);
                } else {
                    clean_responses.push(crate::memory::extraction_queue::content_with_sticker("", &metadata));
                }
            }

            // 根据 current_channel / 输入前缀构造正确的 metadata
            // 跨角色对话时，AI 回复的 speaker 是自己，listener 是对方角色
            let char_id = memory_manager.char_id().to_string();
            let (user_meta, ai_meta) = if state.current_channel == "cross_character" {
                let (_, other_speaker_id) =
                    crate::cross_character::parse_speaker_prefix(&state.user_input);
                let um = serde_json::json!({
                    "channel": "cross_character",
                    "speaker": other_speaker_id,
                    "listener": char_id,
                    "perspective": "speaker",
                    "knowledge_source": "heard",
                });
                let am = serde_json::json!({
                    "channel": "cross_character",
                    "speaker": char_id,
                    "listener": other_speaker_id,
                    "perspective": "speaker",
                    "knowledge_source": "direct",
                });
                (Some(um), Some(am))
            } else {
                // 非跨角色：传入实际 channel（wechat / direct / proactive 等），
                // 避免 save_context_with_metadata 兜底硬编码 'direct'
                let channel = if state.current_channel.is_empty() {
                    "direct"
                } else {
                    state.current_channel.as_str()
                };
                // 广播：用户当众说、自己也当众回，两边 listener 都记 "all"，
                // 落成 "[User says to everyone] …" / "[I say to everyone] …"。
                // 避免每个角色把这句话存成"他单独对我说了一遍"。
                let is_broadcast = channel == "broadcast";
                let um = serde_json::json!({
                    "channel": channel,
                    "speaker": "user",
                    "listener": if is_broadcast { "all" } else { char_id.as_str() },
                    "perspective": "speaker",
                    "knowledge_source": if is_broadcast { "broadcast" } else { "direct" },
                });
                let am = serde_json::json!({
                    "channel": channel,
                    "speaker": char_id,
                    "listener": if is_broadcast { "all" } else { "user" },
                    "perspective": "speaker",
                    "knowledge_source": if is_broadcast { "broadcast" } else { "direct" },
                });
                (Some(um), Some(am))
            };

            let mut ai_meta = ai_meta;
            if let Some(meta) = ai_meta.as_mut() {
                meta["conversation_id"] = json!(state.conversation_id);
                if let Some(sticker)=&state.sticker {meta["sticker"]=json!(sticker);}
            }
            let save_fut = memory_manager.save_context_with_metadata(
                None,
                &clean_responses,
                None,
                "neutral",
                Some(ai_emotion_label),
                state.importance_user,
                state.importance_ai,
                user_meta,
                ai_meta,
                None,
            );
            // 2 秒超时：save_context_with_metadata 已不再触发 LLM enrich
            // （LTM 走 add_memory_with_metadata 路径，主调 LLM 已完成语义抽取），
            // 正常毫秒级完成；超时只记日志不阻塞链后处理
            match tokio::time::timeout(std::time::Duration::from_secs(2), save_fut).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!("[MemorySaving] save_context 失败: {}", e);
                }
                Err(_) => {
                    tracing::warn!("[MemorySaving] save_context 2s 超时，链后处理可能读到旧记忆");
                }
            }
        }

        state.memory_saved = true;
        state.generation_status = "memory_saving_complete".to_string();
        Ok(state.to_json())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_query_preserves_focus_and_only_borrows_history_for_unresolved_followups() {
        let mut state = PipelineState { user_input:"展开讲讲".into(),
            messages:vec![crate::types::response::ChatMessage::user("我最近在准备考研"),
                crate::types::response::ChatMessage::assistant("无关产品介绍".repeat(1000))],
            ..Default::default() };
        let query = MemoryRetrievalStep::build_retrieval_query(&state);
        assert_eq!(query.effective_text(),"我最近在准备考研");
        assert!(query.continuation);
        state.user_input = "iPhone 17 怎么样".into();
        let query = MemoryRetrievalStep::build_retrieval_query(&state);
        assert_eq!(query.focus,"iPhone 17 怎么样"); assert!(query.context.is_empty());
        state.user_input = "它呢".into(); state.resolved_user_input = "考研进展".into();
        let query = MemoryRetrievalStep::build_retrieval_query(&state);
        assert_eq!(query.focus,"考研进展"); assert!(query.context.is_empty());
    }

    #[tokio::test]
    async fn persisted_reply_is_the_presented_text_without_unpublished_drafts_or_templates() {
        let owner = format!("authored-reply-{}", uuid::Uuid::new_v4());
        let memory = Arc::new(MemoryManager::new(&crate::config::AppConfig::default(), &owner).unwrap());
        let state = PipelineState {
            text:"**Master**，换好了。".into(),
            immediate_response_text:"PRIVATE_REASONING_DRAFT".into(),
            ai_response:Some(crate::types::response::AiResponse::new("**Master**，换好了。".into())),
            ..Default::default()
        };
        let validated = super::super::validation::ValidationRunnable::new().ainvoke(state.to_json(), None).await.unwrap();
        let state = PipelineState::from_json(validated.clone());
        assert_eq!(state.text, "Master，换好了。");
        assert_eq!(state.ai_response.unwrap().text, state.text);
        MemorySavingRunnable::with_memory(memory.clone()).ainvoke(validated, None).await.unwrap();
        let memories = memory.get_all_memories().await.unwrap();
        assert_eq!(memories.iter().filter(|m| m.content == "Master，换好了。").count(), 1);
        assert!(memories.iter().all(|m| !m.content.contains("PRIVATE_REASONING_DRAFT")));
    }

    #[tokio::test]
    async fn sent_stickers_are_searchable_memory_with_original_reference() {
        let owner = format!("sticker-memory-{}", uuid::Uuid::new_v4());
        let memory = Arc::new(MemoryManager::new(&crate::config::AppConfig::default(), &owner).unwrap());
        let saver = MemorySavingRunnable::with_memory(memory.clone());
        for text in ["", "谢谢你帮忙"] {
            let mut state = PipelineState::default();
            state.should_respond = true;
            state.current_channel = "wechat".into();
            state.text = text.into();
            state.sticker = Some(crate::stickers::StickerRef {
                id:"vivian_thanks_01".into(),character_id:"vivian".into(),version:"2".into(),
                label:"谢谢".into(),meaning:"感谢对方的用心".into(),
            });
            saver.ainvoke(state.to_json(), None).await.unwrap();
        }
        let memories = memory.get_all_memories().await.unwrap();
        let sent: Vec<_> = memories.iter().filter(|m|m.metadata["sticker"]["id"]=="vivian_thanks_01").collect();
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|m| m.content.contains("发送贴纸") && m.content.contains("感谢对方的用心") && m.metadata["sticker"]["version"]=="2"));
        assert!(sent.iter().any(|m|m.content.starts_with("谢谢你帮忙\n")));
        let mut suppressed = PipelineState::default();
        suppressed.should_respond = true;
        suppressed.sticker_id = Some("vivian_happy_01".into());
        saver.ainvoke(suppressed.to_json(), None).await.unwrap();
        assert_eq!(memory.get_all_memories().await.unwrap().len(), memories.len());
    }

    #[test]
    fn authored_context_is_distinct_from_user_history() {
        let mut memory = crate::memory::MemoryItem::new(
            "用户说慢慢来".into(), crate::memory::types::Granularity::Turn, 0.9,
        );
        assert!(!is_authored_context(&memory));
        memory.id = "seed_legacy".into();
        assert!(is_authored_context(&memory));
        memory.id = "imported".into();
        for source in ["system_seed", "environment_preset"] {
            memory.metadata = json!({"source":source});
            assert!(is_authored_context(&memory));
        }
        memory.metadata = json!({"source":"direct", "speaker":"user"});
        assert!(!is_authored_context(&memory));
    }

    #[test]
    fn dated_backstory_retains_its_authored_time_in_recall() {
        let bundle = crate::memory::seed::load("vivian").unwrap();
        let memory = &bundle.memories[0];
        let label = memory_time_label(memory);
        assert!(label.starts_with("前史设定时间："));
        assert!(label.contains(&crate::utils::prompt_time::format_prompt_time(memory.timestamp)));
        let mut environment = memory.clone();
        environment.metadata = json!({"source":"environment_preset"});
        assert_eq!(memory_time_label(&environment), "预置背景");
    }

    #[test]
    fn test_strip_json_extracts_text() {
        let s = r#"{"text":"晚安","motion":"idle"}"#;
        assert_eq!(MemorySavingRunnable::strip_json_if_any(s), "晚安");
    }

    #[test]
    fn test_strip_json_reply_field() {
        let s = r#"{"reply":"你好"}"#;
        assert_eq!(MemorySavingRunnable::strip_json_if_any(s), "你好");
    }

    #[test]
    fn test_strip_json_passthrough_non_json() {
        let s = "普通的纯文本回复";
        assert_eq!(MemorySavingRunnable::strip_json_if_any(s), s);
    }

    #[test]
    fn test_strip_json_empty_text_falls_back() {
        let s = r#"{"motion":"idle"}"#;
        // 没有可用字段时返回原文
        assert_eq!(MemorySavingRunnable::strip_json_if_any(s), s);
    }

    #[test]
    fn test_collect_ai_responses_dedup() {
        let mut state = PipelineState::default();
        state.immediate_response_text = "你好".to_string();
        state.text = "你好".to_string(); // 与 immediate 相同，应被去重
        let resp = MemorySavingRunnable::collect_ai_responses(&state);
        assert_eq!(resp.len(), 1);
        assert_eq!(resp[0], "你好");
    }

    #[test]
    fn test_collect_ai_responses_excludes_unpublished_drafts() {
        let mut state = PipelineState::default();
        state.immediate_response_text = "即时回复".to_string();
        state.text = "最终回复".to_string();
        let resp = MemorySavingRunnable::collect_ai_responses(&state);
        assert_eq!(resp, vec!["最终回复".to_string()]);
    }
}
