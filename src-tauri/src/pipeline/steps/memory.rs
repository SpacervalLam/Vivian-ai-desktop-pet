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
use crate::memory::types::current_timestamp;
use crate::memory::{estimate_tokens, staleness_text, MemoryManager, MemoryType, RetrievalStrategy};
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::prompt_modules::MEMORY_CONTEXT_MAX_TOKENS;
use crate::pipeline::state::PipelineState;

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
}

impl MemoryRetrievalStep {
    pub fn new(memory: Arc<MemoryManager>) -> Self {
        Self {
            memory,
            memory_filter: None,
            strategy: None,
            mind: None,
            router: None,
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

    fn build_retrieval_query(state: &PipelineState) -> String {
        let mut parts: Vec<String> = Vec::new();

        if !state.resolved_user_input.is_empty() {
            parts.push(format!("Resolved query: {}", state.resolved_user_input));
        }
        if !state.user_input.is_empty() {
            parts.push(format!("Original query: {}", state.user_input));
        }
        if !state.intent.is_empty() {
            parts.push(format!("Intent: {}", state.intent));
        }

        let recent: Vec<String> = state
            .messages
            .iter()
            .rev()
            .take(4)
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect();
        if !recent.is_empty() {
            parts.push(format!("Recent conversation:\n{}", recent.join("\n")));
        }

        parts.join("\n\n")
    }
}

#[async_trait]
impl Runnable for MemoryRetrievalStep {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        if state.user_input.is_empty() {
            return Ok(state.to_json());
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
            return Ok(state.to_json());
        }

        let retrieval_query = Self::build_retrieval_query(&state);

        let strategy = self.strategy.unwrap_or(RetrievalStrategy::Auto);
        let strategy_name = format!("{:?}", strategy);
        // 检索上限放大至 16，再由 MemoryFilter 按权重截断到 5
        let limit = 16;
        let items = self
            .memory
            .search_memories(&retrieval_query, strategy, limit)
            .await?;

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
        // 过滤发生在 MemoryFilter 之前，避免无关记忆挤占 5 条配额。
        let char_id = self.memory.char_id().to_string();
        let items: Vec<_> = items
            .into_iter()
            .filter(|m| crate::memory::precision_filter::is_relevant_to_entity(m, &char_id))
            .collect();

        // 应用 MemoryFilter：跨会话过滤临时话题、保留长期偏好
        let (mut filtered_items, session_id, new_session) = if let Some(filter_arc) = &self.memory_filter {
            let mut filter = filter_arc.write();
            let new_session = filter.is_new_session(&state.user_input, &char_id);
            let scored = filter.get_filtered_memories(
                &state.user_input,
                items,
                5,
                &char_id,
            );
            let session_id = filter.last_session_id.clone();
            (
                scored.into_iter().map(|(m, _)| m).collect::<Vec<_>>(),
                session_id,
                new_session,
            )
        } else {
            // 未注入过滤器：截断到 5 条（保持原行为）
            (items.into_iter().take(5).collect(), None, false)
        };

        // ── 检索后验证（verifier）──
        // 若注入了 router，用小模型过滤掉与用户问题无关的检索结果，减少幻觉噪声。
        // 记忆数 ≤ 2 时 verifier 自动跳过（开销不值得），LLM 不可用时降级为全部保留。
        if let Some(router) = &self.router {
            if filtered_items.len() > 2 {
                let llm_ref: Arc<dyn crate::memory::verifier::VerifierLlmClient> = router.clone();
                let result = crate::memory::verifier::verify_retrieval(
                    &filtered_items,
                    &state.user_input,
                    Some(&llm_ref),
                )
                .await;
                if !result.skipped && !result.verified_indices.is_empty() {
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
                        let score = m.importance.max(0.01) * att * recency;
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

        // 组装记忆文本（含时间/标签/重要性，便于调试和语义理解）
        let now = current_timestamp();
        let mut memory_parts: Vec<String> = Vec::new();
        let mut memory_details: Vec<serde_json::Value> = Vec::new();
        for mem in &filtered_items {
            let content = &mem.content;
            // 如果内容已有 [X says to Y] 说话者前缀，则不再额外添加 "User: "/"AI: " 标签
            let (_, has_spk_prefix, _) = parse_any_speaker_prefix(content);
            let has_speaker_prefix = has_spk_prefix.is_some();
            // 遍历 tags 查找角色归属（兼容 LongTerm 的 [mem_type, subject] 与 ShortTerm 的 [short_term, user/assistant, emo]）
            let role_prefix = if !has_speaker_prefix
                && mem.tags.iter().any(|t| t.eq_ignore_ascii_case("user"))
                && !content.starts_with("User: ")
            {
                "User: "
            } else if !has_speaker_prefix
                && mem
                    .tags
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case("assistant") || t.eq_ignore_ascii_case("vivian"))
                && !content.starts_with("AI: ")
            {
                "AI: "
            } else {
                ""
            };
            let mood = mem.mood_tags();
            let mood_str = if mood.is_empty() {
                String::new()
            } else {
                format!(" | mood={}", mood.join(","))
            };
            let type_label = if mem
                .tags
                .iter()
                .any(|t| t.eq_ignore_ascii_case("long_term"))
                || mem.granularity.eq_ignore_ascii_case("LongTerm")
            {
                "长期"
            } else if mem
                .tags
                .iter()
                .any(|t| t.eq_ignore_ascii_case("short_term"))
                || mem.granularity.eq_ignore_ascii_case("ShortTerm")
            {
                "短期"
            } else {
                "对话"
            };
            let time = chrono::DateTime::from_timestamp(mem.timestamp as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let stale_hint = staleness_text(mem.timestamp, now)
                .map(|s| format!(" [{}]", s))
                .unwrap_or_default();
            // 置信度标记：combined_score 或 temporal_adjusted_score 低于阈值的标注 [需验证]
            let confidence_hint = {
                let score = mem
                    .metadata
                    .get("temporal_adjusted_score")
                    .and_then(|v| v.as_f64())
                    .or_else(|| mem.metadata.get("combined_score").and_then(|v| v.as_f64()))
                    .unwrap_or(1.0);
                if score < 0.3 {
                    " [需验证]"
                } else {
                    ""
                }
            };
            memory_parts.push(format!(
                "[{} | {} | imp={:.2}{}] {}{}{}{}",
                time, type_label, mem.importance, mood_str, role_prefix, content, stale_hint, confidence_hint
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
            let user_meta = serde_json::json!({
                "channel": channel,
                "speaker": "user",
                "listener": char_id,
                "perspective": "speaker",
                "knowledge_source": "direct",
            });
            let ai_meta = serde_json::json!({
                "channel": channel,
                "speaker": char_id,
                "listener": "user",
                "perspective": "speaker",
                "knowledge_source": "direct",
            });
            if let Err(e) = memory_manager
                .save_context_with_metadata(
                    Some(&storage_text),
                    &[],
                    None,
                    user_emotion_label,
                    None,
                    state.importance_user,
                    state.importance_ai,
                    Some(user_meta),
                    Some(ai_meta),
                )
                .await
            {
                tracing::warn!("[UserMemorySaving] 保存失败: {}", e);
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

    /// 收集 AI 回复（immediate_response_text + text，去重）
    fn collect_ai_responses(state: &PipelineState) -> Vec<String> {
        let mut responses: Vec<String> = Vec::new();
        let immediate = state.immediate_response_text.trim();
        if !immediate.is_empty() {
            responses.push(immediate.to_string());
        }
        let final_text = state.text.trim();
        if !final_text.is_empty() {
            if responses.last().map(|s| s.as_str()) != Some(final_text) {
                responses.push(final_text.to_string());
            }
        }
        responses
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
            let clean_responses: Vec<String> = ai_responses
                .iter()
                .map(|r| Self::strip_json_if_any(r))
                .collect();

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
                let um = serde_json::json!({
                    "channel": channel,
                    "speaker": "user",
                    "listener": char_id,
                    "perspective": "speaker",
                    "knowledge_source": "direct",
                });
                let am = serde_json::json!({
                    "channel": channel,
                    "speaker": char_id,
                    "listener": "user",
                    "perspective": "speaker",
                    "knowledge_source": "direct",
                });
                (Some(um), Some(am))
            };

            let ltm = state.long_term_memory.trim();
            let save_fut = memory_manager.save_context_with_metadata(
                None,
                &clean_responses,
                if ltm.is_empty() { None } else { Some(ltm) },
                "neutral",
                Some(ai_emotion_label),
                state.importance_user,
                state.importance_ai,
                user_meta,
                ai_meta,
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
    fn test_collect_ai_responses_distinct() {
        let mut state = PipelineState::default();
        state.immediate_response_text = "即时回复".to_string();
        state.text = "最终回复".to_string();
        let resp = MemorySavingRunnable::collect_ai_responses(&state);
        assert_eq!(resp.len(), 2);
    }
}
