//! 记忆巩固流水线：把"短期原话"沉淀为"中期摘要"。
//!
//! 与 `retention.rs`（仅做过期清理+字面去重）互补，这里实现的是需要 LLM 参与
//! 的深度巩固链路：
//!
//! - **Stage 1（summarize）**：ShortTerm 满/会话空闲 → LLM 多主题摘要 → MidTerm SessionSummary
//!
//! 长期事实（画像/事实/关系信号/行为模式）**不再**由摘要二次抽取，统一交给
//! AutoExtractor 从原始对话提取：同一用户表述若在摘要层与原始层各存一份，
//! 召回时会被重复命中并重复呈现。原先的 Stage 2 / Stage 3 已随之移除。
//!
//! 所有 LLM 调用走 `routing_matrix["consolidation"]`（需强推理模型）。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::embedding::MemoryEmbeddingProvider;
use super::manager::MemoryManager;
use super::types::{current_timestamp, MemoryItem, MemoryType};
use super::vector_search::cosine_similarity;
use crate::config::manager::ConsolidationConfig;
use crate::error::VivianResult;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;
use crate::memory::user_model::UserModelManager;


/// Stage 1 主题连续性阈值：新摘要与近期 SessionSummary 的余弦相似度 ≥ 此值时合并，
/// 否则新建独立 SessionSummary。
///
/// 取值 0.6：高于此值通常为同主题延续，低于此值视为话题切换。
const SESSION_TOPIC_SIMILARITY_THRESHOLD: f64 = 0.6;

/// Stage 1 主题连续性检测时，参与比对的近期 SessionSummary 数量上限。
///
/// 取最近 5 条避免 embedding 计算开销过大（每条 1 次 embed 调用）。
const RECENT_SESSION_SUMMARY_LIMIT: usize = 5;

/// Stage 1 冷却时间（秒）：两次 Stage 1 执行之间的最小间隔。
/// 防止 proactive tick（约 10s 一次）串行重入导致同批 ShortTerm 被反复摘要。
const STAGE1_COOLDOWN_SEC: f64 = 120.0;

/// 巩固流水线
pub struct ConsolidationPipeline {
    /// LLM 路由（使用 reflection 路由）
    router: Arc<ModelRouter>,
    /// 配置
    config: ConsolidationConfig,
    /// 锁定核心文本（来自 PersonaConfig::locked_core_summary）。
    ///
    /// 原为 Stage 2 反思注入「不可修改人设边界」而存。Stage 2 已摘除（长期事实
    /// 改由 AutoExtractor 统一提取），此处暂无读取方，但 `set_locked_core` 仍是
    /// 活跃 pub API（BrainChatChain 启动时注入），暂予保留。
    locked_core_text: parking_lot::RwLock<String>,
    /// 上次 Stage 1 执行的时间戳（秒）；0 表示从未执行过。
    /// 两次 Stage 1 之间至少间隔 STAGE1_COOLDOWN_SEC，防止 tick 串行重入。
    last_stage1_at: std::sync::atomic::AtomicU64,
    /// 运行锁：防止对话路径（post_process_memory_async）与 tick 路径
    /// （proactive_tick 日常巩固检查）并发执行 run() 产生竞态。
    /// 用 try_lock 而非 lock，冲突时直接跳过本次（下一个 tick 会重试）。
    run_lock: tokio::sync::Mutex<()>,
    /// 用户认知模型（可选）。原供 Stage 3.5 概念归并使用，Stage 3.5 已随
    /// Stage 2/3 一并摘除；`set_user_model` 仍为活跃 pub API，字段暂予保留。
    user_model: parking_lot::RwLock<Option<Arc<UserModelManager>>>,
    /// Stage 1 断点续跑上下文（角色 ID + 进度文件路径；未绑定角色时不启用）
    progress: parking_lot::RwLock<Option<ProgressCtx>>,
}

/// 断点续跑上下文
struct ProgressCtx {
    char_id: String,
    path: PathBuf,
}

/// Stage 1 断点续跑持久化状态
///
/// 上下文键 = char_id + 逻辑日：跨天或换角色时整体作废（防止误恢复陈旧水位）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConsolidationProgress {
    context_key: String,
    /// 摘要已落库（或正在落库）但尚未 mark_summarized 的 ShortTerm 源 ID
    stage1_pending_ids: Vec<String>,
    updated_at: String,
}

impl ConsolidationPipeline {
    pub fn new(router: Arc<ModelRouter>, config: ConsolidationConfig) -> Self {
        Self {
            router,
            config,
            last_stage1_at: std::sync::atomic::AtomicU64::new(0),
            locked_core_text: parking_lot::RwLock::new(String::new()),
            run_lock: tokio::sync::Mutex::new(()),
            user_model: parking_lot::RwLock::new(None),
            progress: parking_lot::RwLock::new(None),
        }
    }

    /// 绑定角色 ID，启用 Stage 1 断点续跑（由 BrainChatChain 构造后调用）
    pub fn set_progress_char_id(&self, char_id: &str) {
        let path = crate::utils::path::get_companion_shared_dir()
            .join(format!("consolidation_progress_{char_id}.json"));
        *self.progress.write() = Some(ProgressCtx {
            char_id: char_id.to_string(),
            path,
        });
    }

    /// 断点续跑上下文键：char_id + 逻辑日（本地时区）
    fn progress_context_key(char_id: &str) -> String {
        let today = chrono::Local::now().format("%Y-%m-%d");
        format!("{char_id}|{today}")
    }

    /// 写入 Stage 1 断点水位（摘要写入前调用，记录本批源 ID）
    fn save_progress(&self, source_ids: &[String]) {
        let guard = self.progress.read();
        let Some(ctx) = guard.as_ref() else { return };
        let state = ConsolidationProgress {
            context_key: Self::progress_context_key(&ctx.char_id),
            stage1_pending_ids: source_ids.to_vec(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let Ok(text) = serde_json::to_string_pretty(&state) else { return };
        if let Some(parent) = ctx.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = super::step_health::atomic_write(&ctx.path, &text) {
            tracing::warn!("[ConsolidationPipeline] 断点水位写入失败: {e}");
        }
    }

    /// 清除断点水位（mark_summarized 全部完成后调用）
    fn clear_progress(&self) {
        let guard = self.progress.read();
        let Some(ctx) = guard.as_ref() else { return };
        let _ = std::fs::remove_file(&ctx.path);
    }

    /// 断点续跑：补完崩溃时中断的 Stage 1 事务
    ///
    /// 崩溃窗口 = SessionSummary 已写入 → ShortTerm 尚未 mark_summarized。
    /// 恢复语义（双现场区分）：
    /// - 源 ID 出现在某条 SessionSummary 的 promoted_from 中 → 摘要已落库，补标记
    /// - 未出现 → 摘要未落库（LLM 输出丢失），不标记，走正常重摘要
    async fn complete_interrupted_stage1(&self, memory: &MemoryManager) {
        let (char_id, path) = {
            let guard = self.progress.read();
            let Some(ctx) = guard.as_ref() else { return };
            (ctx.char_id.clone(), ctx.path.clone())
        };
        let Some(state) =
            crate::utils::fs::load_json_or_backup::<ConsolidationProgress>(&path)
        else {
            return;
        };
        if state.context_key != Self::progress_context_key(&char_id) {
            // 上下文键变化（跨天）：整体作废，防止误恢复陈旧水位
            tracing::debug!("[ConsolidationPipeline] 断点水位上下文键不匹配，作废");
            let _ = std::fs::remove_file(&path);
            return;
        }
        if state.stage1_pending_ids.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }

        let Ok(all) = memory.get_all_memories().await else { return };
        let mut promoted_ids: HashSet<&str> = HashSet::new();
        for m in &all {
            let is_summary = m.tags.iter().any(|t| t == "session_summary")
                || m.metadata
                    .get("memory_type")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "session_summary")
                    .unwrap_or(false);
            if !is_summary {
                continue;
            }
            if let Some(arr) = m.metadata.get("promoted_from").and_then(|v| v.as_array()) {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        promoted_ids.insert(s);
                    }
                }
            }
        }

        let mut completed = 0usize;
        for id in &state.stage1_pending_ids {
            if promoted_ids.contains(id.as_str()) {
                let _ = memory.mark_summarized(id);
                completed += 1;
            }
        }
        if completed > 0 {
            tracing::info!(
                "[ConsolidationPipeline] 断点续跑：补完 {} 条崩溃前已摘要未标记的 ShortTerm",
                completed
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    /// 注入用户认知模型，启用 Stage 3 末尾的概念归并（由 BrainChatChain 初始化后调用）。
    ///
    /// 注入后，每次 Stage 3 生成 Insight 时，会额外把洞察归纳为高层概念，
    /// 归并进 UserModel（merge_concept）并写入知识图谱（ingest_concepts），
    /// 从而让"用户长期在乎什么"沉淀为可检索的概念层。
    pub fn set_user_model(&self, user_model: Arc<UserModelManager>) {
        *self.user_model.write() = Some(user_model);
    }

    /// 注入锁定核心文本（由 BrainChatChain 在初始化后调用）。
    ///
    /// 传入空字符串等价于清除锁定核心。当前 Stage 2 已摘除，此值暂无读取方，
    /// 保留 setter 以维持既有构造链不变。
    pub fn set_locked_core(&self, text: String) {
        *self.locked_core_text.write() = text;
    }

    /// LLM 路由访问器（供外部高层阶段如 Belief 生成复用 reflection 路由）。
    pub fn router(&self) -> Arc<ModelRouter> {
        Arc::clone(&self.router)
    }

    /// 执行完整的巩固检查（每轮对话后或 proactive tick 调用）。
    ///
    /// 只执行 Stage 1 的主题摘要。长期事实由原话提取器负责。
    ///
    /// 运行锁：try_lock 失败时说明另一条路径（对话/tick）正在执行，直接跳过本次。
    /// 不阻塞等待，避免 proactive tick 被长 LLM 调用卡住；下一个 tick 会重试。
    pub async fn run(&self, memory: &MemoryManager) -> VivianResult<ConsolidationReport> {
        let _run_guard = match self.run_lock.try_lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::debug!(
                    "[ConsolidationPipeline] 已有巩固流水线在执行，跳过本次检查"
                );
                return Ok(ConsolidationReport::default());
            }
        };

        let mut report = ConsolidationReport::default();

        // 断点续跑：补完上次崩溃时中断的 Stage 1 事务（在 Stage 1 之前执行，
        // 避免已摘要的 ShortTerm 被重复摘要）
        self.complete_interrupted_stage1(memory).await;

        // Stage 1: ShortTerm → MidTerm SessionSummary
        if let Some(count) = self.stage1_summarize(memory).await? {
            report.stage1_summaries = count;
        }

        // 长期事实只由 AutoExtractor 从原始对话提取。摘要不再反复提取事实或
        // 生成画像/洞察；否则同一用户表述会在不同层级中被重复保存和召回。

        // 索引漂移检测：长期增删后向量索引可能与记忆条目脱节，必要时全量重建。
        if let Some(n) = memory.check_index_drift_and_rebuild() {
            tracing::info!(
                "[ConsolidationPipeline] 索引漂移检测触发全量重建，重新嵌入 {} 条向量",
                n
            );
        }

        Ok(report)
    }

    /// Stage 1: 短期记忆摘要
    ///
    /// 触发条件（满足任一即触发）：
    /// 1. ShortTerm 条数 ≥ `stage1_short_term_threshold`（计数触发）
    /// 2. ShortTerm 非空且距最新一条 ≥ `stage1_idle_timeout_sec`（空闲触发）
    ///
    /// 动作：LLM 把所有 ShortTerm 摘要成 1-3 条 SessionSummary，删除原 ShortTerm
    async fn stage1_summarize(&self, memory: &MemoryManager) -> VivianResult<Option<usize>> {
        let all = memory.get_all_memories().await?;
        let now = current_timestamp();

        // 冷却检查：防止 tick 串行重入导致同批 ShortTerm 被反复摘要
        let last_s1 = self.last_stage1_at.load(std::sync::atomic::Ordering::Relaxed) as f64;
        if last_s1 > 0.0 && now - last_s1 < STAGE1_COOLDOWN_SEC {
            return Ok(None);
        }

        // 筛选 ShortTerm 记忆（排除 InnerMonologue / ObservationNote，避免与对话事实混合摘要）
        // - InnerMonologue 是角色主观内心独白，与对话事实语义性质不同，混合摘要会失真
        // - ObservationNote 是旁观记忆，不含原文，不应参与对话摘要
        let mut short_term: Vec<&MemoryItem> = all
            .iter()
            .filter(|m| {
                let is_short_term = m.tags.iter().any(|t| t == "short_term")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "short_term")
                        .unwrap_or(false);
                if m.consolidated || !is_short_term || m.tags.iter().any(|t| t == "topic_signal")
                    || m.metadata.get("topic_signal").and_then(|v| v.as_bool()).unwrap_or(false) {
                    return false;
                }
                // 排除内心独白（带 inner_monologue tag 或 memory_type=inner_monologue）
                let is_inner_monologue = m.tags.iter().any(|t| t == "inner_monologue")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "inner_monologue")
                        .unwrap_or(false);
                if is_inner_monologue {
                    return false;
                }
                // 排除旁观记忆
                let is_observation = m.tags.iter().any(|t| t == "observation_note")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "observation_note")
                        .unwrap_or(false)
                    || m.metadata
                        .get("perspective")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "observer")
                        .unwrap_or(false);
                !is_observation
            })
            .collect();
        short_term.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
        short_term.truncate(40);

        if short_term.is_empty() {
            return Ok(None);
        }

        // 触发条件 1：计数触发
        let count_triggered = short_term.len() >= self.config.stage1_short_term_threshold;

        // 触发条件 2：空闲触发 —— 最新一条 ShortTerm 距今 ≥ idle_timeout_sec
        let last_short_term_ts = short_term
            .iter()
            .map(|m| m.timestamp)
            .fold(f64::MIN, f64::max);
        let idle_secs = now - last_short_term_ts;
        let idle_triggered =
            idle_secs >= self.config.stage1_idle_timeout_sec && idle_secs >= 0.0;

        if !count_triggered && !idle_triggered {
            return Ok(None);
        }

        let trigger_reason = if count_triggered { "count" } else { "idle" };
        tracing::debug!(
            "[ConsolidationPipeline] Stage 1 触发: {} (count={}, threshold={}, idle_secs={:.0}, idle_timeout={:.0})",
            trigger_reason,
            short_term.len(),
            self.config.stage1_short_term_threshold,
            idle_secs,
            self.config.stage1_idle_timeout_sec
        );

        // 拼接内容：含时间标签和情绪余温的原始对话
        let conversation_text = short_term
            .iter()
            .map(|m| {
                let mood = m.mood_tags();
                let mood_line = if mood.is_empty() {
                    String::new()
                } else {
                    format!(" [情绪余温: {}]", mood.join(","))
                };
                let date_line = m
                    .date_label()
                    .map(|d| format!(" [日期: {}]", d))
                    .unwrap_or_default();
                let tod_line = m
                    .time_of_day()
                    .map(|t| format!(" [时段: {}]", t))
                    .unwrap_or_default();
                format!("[{}]{}{}{} {}", m.id, date_line, tod_line, mood_line,
                    crate::memory::companion_policy::compact_excerpt(&m.content, 240))
            })
            .collect::<Vec<_>>()
            .join("\n");

        // 摘要是事实索引，不由人设补全场景或情绪；角色口吻留给回复阶段。
        let persona_section = "";

        // 注入前次阶段摘要作为参考（防幻觉连续性约束）
        let recent_summaries_for_ref = self.get_recent_session_summaries(memory, &all).await;
        let reference_section = if recent_summaries_for_ref.is_empty() {
            String::new()
        } else {
            let ref_text = recent_summaries_for_ref
                .iter().take(3)
                .map(|m| {
                    let mood = m.mood_tags();
                    let mood_line = if mood.is_empty() {
                        String::new()
                    } else {
                        format!(" [情绪余温: {}]", mood.join(","))
                    };
                    format!("- {}{}", crate::utils::truncate_chars(&m.content, 160), mood_line)
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "\n[可参考的既有阶段摘要]\n\
                 只用于保持人物关系、项目脉络、时间线和记忆口吻一致；\n\
                 不要把参考摘要里出现、但本段原始对话没有出现的内容写成这段的新事实。\n{}\n\n",
                ref_text
            )
        };

        let prompt = format!(
            "{persona_section}\
             请将以下多条真实对话压缩为0-3条主题级会话摘要。只保留用户明确表达的事实、正在进行的事、双方明确作出的约定和有后续价值的情绪背景。\n\
             不要把角色自己的客套回复、猜测、内心感受或人设写成用户事实；不要推断亲密度变化。没有可延续信息就输出 {{\"items\":[]}}。每条只写一个主题，标明是谁说的；避免重复已有摘要。\n\
             {reference_section}\
             输出JSON对象，字段 items 为数组，每项含：\n\
             - \"summary\"(string)：会话摘要\n\
             - \"importance\"(0.0-1.0)，评分标准（统一适用）：\n\
               - 0.9-1.0：硬性约束、核心身份属性、健康/过敏信息、重大关系里程碑\n\
               - 0.6-0.8：长期偏好、项目背景、关键决策、关系事件、共同经历\n\
               - 0.3-0.5：一般事实、上下文信息、解释性内容\n\
               - 0.0-0.2：闲聊、寒暄、临时性问题、一次性话题\n\
             - \"mood_tags\"(string[])：本段摘要的情感余温，0-3 个标签，从以下 16 个中选：\n\
               calm, warm, affectionate, happy, playful, curious, thoughtful,\n\
               touched, proud, worried, lonely, sad, embarrassed, tense, annoyed, determined\n\
             - \"date_labels\"(string[])：本段摘要覆盖的日期（YYYY-MM-DD），可多天，从原始对话的时间标签提取\n\
             - \"time_of_days\"(string[])：本段摘要覆盖的时段，可多选，从 morning/afternoon/evening/night 中选\n\
             仅输出JSON，无其他文本。\n\n\
             短期记忆：\n{}",
            conversation_text
        );

        let response = self.router.generate(
            LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                .with_json_schema(consolidation_array_schema::<SummaryListSchema>())
                .with_character_id(memory.char_id().to_string()),
        ).await?;
        let summaries = parse_summaries(&response);

        if summaries.is_empty() {
            let valid_empty = serde_json::from_str::<serde_json::Value>(&extract_json_from_response(&response))
                .ok()
                .map(|value| value.as_array().is_some_and(Vec::is_empty)
                    || value.get("items").and_then(|items| items.as_array()).is_some_and(Vec::is_empty))
                .unwrap_or(false);
            if !valid_empty {
                tracing::warn!("[ConsolidationPipeline] Stage 1 摘要解析失败，保留原话稍后重试");
                return Ok(None);
            }
            // 空摘要是有效的「没有值得沉淀的内容」判定；消费原始片段，
            // 否则每次后台巩固都会为同一段寒暄重新付费。
            for item in &short_term {
                memory.mark_summarized(&item.id)?;
            }
            self.last_stage1_at.store(now as u64, std::sync::atomic::Ordering::Relaxed);
            tracing::debug!("[ConsolidationPipeline] Stage 1 无可沉淀信息，已消费 {} 条原话", short_term.len());
            return Ok(Some(0));
        }

        // 写入 SessionSummary，设置 promoted_from 元数据
        let source_ids: Vec<String> = short_term.iter().map(|m| m.id.clone()).collect();

        // 断点水位：摘要落库前记录本批源 ID（崩溃后据此补完/重做，见 complete_interrupted_stage1）
        self.save_progress(&source_ids);

        // 主题连续性检测：对每条新摘要计算 embedding，与近期 SessionSummary 比对，
        // 相似度 ≥ 阈值则合并到既有 SessionSummary（追加 source_memory_ids + 取较大 importance），
        // 否则新建独立 SessionSummary。避免将不同话题强行合并到同一条摘要。
        let recent_summaries = self.get_recent_session_summaries(memory, &all).await;
        let embedding_provider = memory.embedding();
        let mut created = 0usize;
        let mut merged = 0usize;
        // 收集新建的 SessionSummary ID（用于 Episode 封包）
        let mut new_summary_ids: Vec<String> = Vec::new();
        for s in &summaries {
            let new_emb = match embedding_provider.embed(&s.summary) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        "[ConsolidationPipeline] summary embedding 生成失败，降级到关键词合并: {}", e
                    );
                    None
                }
            };
            let merge_target = new_emb
                .as_ref()
                .and_then(|emb| {
                    self.find_merge_target(emb, &recent_summaries, &embedding_provider)
                })
                .or_else(|| {
                    // embedding 不可用或无匹配 → 降级到关键词重叠方案
                    self.find_merge_target_by_keywords(&s.summary, &recent_summaries)
                });

            match merge_target {
                Some(target_id) => {
                    // 合并到既有 SessionSummary：追加 source_memory_ids、
                    // 取较大 importance、内容用换行拼接（保留多段主题脉络）
                    if let Err(e) = self
                        .merge_into_session_summary(
                            memory,
                            &target_id,
                            &s.summary,
                            s.importance,
                            &source_ids,
                            now,
                            &s.mood_tags,
                            &s.date_labels,
                            &s.time_of_days,
                        )
                        .await
                    {
                        tracing::warn!(
                            "[ConsolidationPipeline] Stage 1 合并到 SessionSummary {} 失败，回退新建: {}",
                            target_id,
                            e
                        );
                        let sid = self.create_new_session_summary(
                            memory,
                            &s.summary,
                            s.importance,
                            &source_ids,
                            now,
                            &s.mood_tags,
                            &s.date_labels,
                            &s.time_of_days,
                        )
                        .await?;
                        new_summary_ids.push(sid);
                        created += 1;
                    } else {
                        merged += 1;
                        tracing::debug!(
                            "[ConsolidationPipeline] Stage 1 主题连续：合并到既有 SessionSummary {}（相似度 ≥ {:.2}）",
                            target_id,
                            SESSION_TOPIC_SIMILARITY_THRESHOLD
                        );
                    }
                }
                None => {
                    // 无相似既有摘要或 embedding 不可用 → 新建独立 SessionSummary
                    let sid = self.create_new_session_summary(
                        memory,
                        &s.summary,
                        s.importance,
                        &source_ids,
                        now,
                        &s.mood_tags,
                        &s.date_labels,
                        &s.time_of_days,
                    )
                    .await?;
                    new_summary_ids.push(sid);
                    created += 1;
                }
            }
        }

        // ── Episode 封包 ──────────────────────────────────────────────
        // 当至少有一条新建 SessionSummary 时，将本轮 ShortTerm + 新 SessionSummary
        // 封为一个 Episode（一段经历）。ShortTerm 即将被删除，但 Episode 的元数据
        // （时间跨度、情绪曲线、importance）已从它们提取；SessionSummary 保留
        // episode_id 用于后续检索 boost。
        if !new_summary_ids.is_empty() {
            if let Some(episode_store) = memory.episode_store() {
                let timestamps: Vec<f64> = short_term.iter().map(|m| m.timestamp).collect();
                let importances: Vec<f64> = short_term.iter().map(|m| m.importance).collect();

                // 情绪曲线：从 ShortTerm 的 mood_tags 提取 (timestamp, tag) 对
                let emotion_curve: Vec<(f64, String)> = short_term
                    .iter()
                    .flat_map(|m| {
                        m.mood_tags()
                            .into_iter()
                            .map(|tag| (m.timestamp, tag))
                            .collect::<Vec<_>>()
                    })
                    .collect();

                // topic 取首条新 SessionSummary 的摘要前 50 字符
                let topic = summaries.first().map(|s| {
                    let t = &s.summary;
                    if t.chars().count() > 50 {
                        format!("{}...", t.chars().take(50).collect::<String>())
                    } else {
                        t.clone()
                    }
                });

                // 摘要取所有新 SessionSummary 的内容拼接
                let summary_text = summaries
                    .iter()
                    .map(|s| s.summary.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");

                // 封包 memory_ids = ShortTerm IDs + 新 SessionSummary IDs
                let mut episode_memory_ids = source_ids.clone();
                episode_memory_ids.extend(new_summary_ids.iter().cloned());

                let episode = episode_store.seal_episode(
                    episode_memory_ids,
                    &timestamps,
                    &importances,
                    topic,
                    Some(summary_text),
                    &emotion_curve,
                );

                // 回填 episode_id 到 ShortTerm（即将删除，但保持一致性）和新 SessionSummary
                let _ = memory.backfill_episode_id(&source_ids, &episode.episode_id);
                let _ = memory.backfill_episode_id(&new_summary_ids, &episode.episode_id);

                tracing::info!(
                    "[ConsolidationPipeline] Episode 封包: {} ({} 条 ShortTerm + {} 条 SessionSummary)",
                    episode.episode_id,
                    source_ids.len(),
                    new_summary_ids.len()
                );
            }
        }

        // 标记原始 ShortTerm 为"已摘要"（保留向量索引，前端图谱可展开显示）
        for m in &short_term {
            memory.mark_summarized(&m.id)?;
        }

        // 事务完成，清除断点水位
        self.clear_progress();

        tracing::info!(
            "[ConsolidationPipeline] Stage 1: {} 条 ShortTerm → {} 条新建 + {} 条合并 SessionSummary",
            short_term.len(),
            created,
            merged
        );
        self.last_stage1_at.store(now as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(Some(created + merged))
    }

    /// 获取近期 SessionSummary 列表（按时间倒序，最多 `RECENT_SESSION_SUMMARY_LIMIT` 条）
    ///
    /// 用于 Stage 1 主题连续性检测的比对源。传入 `all` 避免重复查询。
    async fn get_recent_session_summaries(
        &self,
        _memory: &MemoryManager,
        all: &[MemoryItem],
    ) -> Vec<MemoryItem> {
        let mut summaries: Vec<MemoryItem> = all
            .iter()
            .filter(|m| {
                !m.consolidated && (m.tags.iter().any(|t| t == "session_summary")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "session_summary")
                        .unwrap_or(false))
            })
            .cloned()
            .collect();
        // 按时间倒序，取最近 N 条
        summaries.sort_by(|a, b| {
            b.timestamp
                .partial_cmp(&a.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        summaries.truncate(RECENT_SESSION_SUMMARY_LIMIT);
        summaries
    }

    /// 在近期 SessionSummary 中查找与新摘要 embedding 最相似且超过阈值的条目。
    ///
    /// 返回最相似条目的 id（若存在）。embedding 计算失败时跳过该条目并记录警告。
    fn find_merge_target(
        &self,
        new_emb: &[f32],
        recent: &[MemoryItem],
        embedding_provider: &Arc<dyn MemoryEmbeddingProvider>,
    ) -> Option<String> {
        let mut best_id: Option<String> = None;
        let mut best_sim = SESSION_TOPIC_SIMILARITY_THRESHOLD;
        for m in recent {
            let emb = match embedding_provider.embed(&m.content) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "[ConsolidationPipeline] 既有 SessionSummary {} embedding 失败，跳过比对: {}",
                        m.id,
                        e
                    );
                    continue;
                }
            };
            let sim = cosine_similarity(new_emb, &emb);
            if sim > best_sim {
                best_sim = sim;
                best_id = Some(m.id.clone());
            }
        }
        if best_id.is_some() {
            tracing::debug!(
                "[ConsolidationPipeline] Stage 1 主题连续：最佳相似度 = {:.3}",
                best_sim
            );
        }
        best_id
    }

    /// 关键词重叠降级方案：当 embedding 不可用时，用 Jaccard 关键词相似度判断主题连续性。
    ///
    /// 提取两段文本中的关键词（Unicode 字母/数字 token，≥2 字符），
    /// 计算 Jaccard 系数 = |A∩B| / |A∪B|。超过阈值时返回匹配 ID。
    fn find_merge_target_by_keywords(
        &self,
        new_text: &str,
        recent: &[MemoryItem],
    ) -> Option<String> {
        let new_kw = extract_keywords(new_text);
        if new_kw.is_empty() {
            return None;
        }

        let mut best_id: Option<String> = None;
        let mut best_sim = SESSION_TOPIC_SIMILARITY_THRESHOLD;

        for m in recent {
            let existing_kw = extract_keywords(&m.content);
            if existing_kw.is_empty() {
                continue;
            }

            let intersection_count = new_kw.intersection(&existing_kw).count();
            let union_count = new_kw.union(&existing_kw).count();
            let sim = intersection_count as f64 / union_count as f64;

            if sim > best_sim {
                best_sim = sim;
                best_id = Some(m.id.clone());
            }
        }

        if best_id.is_some() {
            tracing::debug!(
                "[ConsolidationPipeline] Stage 1 主题连续（关键词降级）：最佳 Jaccard = {:.3}",
                best_sim
            );
        }
        best_id
    }

    /// 合并新摘要到既有 SessionSummary：
    /// - 内容用换行拼接（保留多段主题脉络）
    /// - importance 取较大值
    /// - promoted_from 追加 source_ids（去重）
    /// - mood_tags / date_labels / time_of_days 并集去重后写回；主标量取首项
    /// - promoted_at 更新为当前时间
    async fn merge_into_session_summary(
        &self,
        memory: &MemoryManager,
        target_id: &str,
        new_summary: &str,
        new_importance: f64,
        source_ids: &[String],
        now: f64,
        new_mood_tags: &[String],
        new_date_labels: &[String],
        new_time_of_days: &[String],
    ) -> VivianResult<()> {
        let all = memory.get_all_memories().await?;
        let target = all
            .iter()
            .find(|m| m.id == target_id)
            .ok_or_else(|| crate::error::VivianError::Memory(format!("目标 SessionSummary 不存在: {target_id}")))?;

        // 内容冗余检测：新摘要核心句子已存在于旧内容时跳过文本追加，仅更新元数据
        let merged_content = if is_content_redundant(&target.content, new_summary) {
            tracing::debug!(
                "[ConsolidationPipeline] Stage 1 合并：新摘要与既有内容冗余，跳过文本追加 (target={})",
                target_id
            );
            target.content.clone()
        } else {
            format!("{}\n{}", target.content, new_summary)
        };
        // importance 取较大值
        let merged_importance = target.importance.max(new_importance);

        // 合并 promoted_from（去重）
        let mut merged_source_ids: Vec<String> = Vec::new();
        if let Some(existing) = target.metadata.get("promoted_from").and_then(|v| v.as_array()) {
            for v in existing {
                if let Some(s) = v.as_str() {
                    if !merged_source_ids.iter().any(|x| x == s) {
                        merged_source_ids.push(s.to_string());
                    }
                }
            }
        }
        for s in source_ids {
            if !merged_source_ids.iter().any(|x| x == s) {
                merged_source_ids.push(s.clone());
            }
        }

        // 合并 mood_tags / date_labels / time_of_days（并集去重，保留顺序）
        let existing_mood = target.mood_tags();
        let existing_dates: Vec<String> = target
            .metadata
            .get("date_labels")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let existing_tods: Vec<String> = target
            .metadata
            .get("time_of_days")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let merged_mood = union_strings(&existing_mood, new_mood_tags);
        let merged_dates = union_strings(&existing_dates, new_date_labels);
        let merged_tods = union_strings(&existing_tods, new_time_of_days);

        // 主标量：date_label 取并集中字典序最小（最早）的日期；
        // time_of_day 取并集首项（保持与原逻辑一致）。
        let date_label_primary = merged_dates.iter().min().cloned();
        let time_of_day_primary = merged_tods.first().cloned();

        // 写回内容 + importance（通过 delete + add 重建，因为 MemoryManager 没有直接更新内容的接口）
        // 保留原 tags / metadata（合并 promoted_from）
        let mut merged_tags = target.tags.clone();
        if !merged_tags.iter().any(|t| t == "session_summary") {
            merged_tags.push("session_summary".to_string());
        }
        let char_id_for_mem = memory.char_id().to_string();
        let init_meta = json!({
            "channel": "inner",
            "speaker": char_id_for_mem,
            "listener": char_id_for_mem,
            "perspective": "speaker",
            "knowledge_source": "extracted",
        });
        let new_item = memory
            .add_memory_with_metadata(&merged_content, MemoryType::SessionSummary, merged_importance, merged_tags, init_meta)
            .await?;

        // 合并 metadata
        let mut merged_metadata = target.metadata.clone();
        if let Some(obj) = merged_metadata.as_object_mut() {
            obj.insert("promoted_from".to_string(), serde_json::Value::Array(
                merged_source_ids.iter().map(|s| serde_json::Value::String(s.clone())).collect()
            ));
            obj.insert("promoted_at".to_string(), serde_json::json!(now));
            obj.insert("consolidation_stage".to_string(), serde_json::json!("stage1_merge"));
            obj.insert("merged_from".to_string(), serde_json::json!(target_id));
            if !merged_mood.is_empty() {
                obj.insert(
                    "mood_tags".to_string(),
                    serde_json::Value::Array(
                        merged_mood.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                    ),
                );
            }
            if !merged_dates.is_empty() {
                obj.insert(
                    "date_labels".to_string(),
                    serde_json::Value::Array(
                        merged_dates.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                    ),
                );
                if let Some(primary) = &date_label_primary {
                    obj.insert("date_label".to_string(), serde_json::Value::String(primary.clone()));
                }
            }
            if !merged_tods.is_empty() {
                obj.insert(
                    "time_of_days".to_string(),
                    serde_json::Value::Array(
                        merged_tods.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                    ),
                );
                if let Some(primary) = &time_of_day_primary {
                    obj.insert("time_of_day".to_string(), serde_json::Value::String(primary.clone()));
                }
            }
        } else {
            let mut fallback = serde_json::json!({
                "promoted_from": merged_source_ids,
                "promoted_at": now,
                "consolidation_stage": "stage1_merge",
                "merged_from": target_id,
            });
            if let Some(obj) = fallback.as_object_mut() {
                if !merged_mood.is_empty() {
                    obj.insert(
                        "mood_tags".to_string(),
                        serde_json::Value::Array(
                            merged_mood.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                        ),
                    );
                }
                if !merged_dates.is_empty() {
                    obj.insert(
                        "date_labels".to_string(),
                        serde_json::Value::Array(
                            merged_dates.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                        ),
                    );
                }
                if !merged_tods.is_empty() {
                    obj.insert(
                        "time_of_days".to_string(),
                        serde_json::Value::Array(
                            merged_tods.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                        ),
                    );
                }
            }
            merged_metadata = fallback;
        }
        memory.patch_memory_metadata(&new_item.id, merged_metadata)?;

        // 标记原目标 SessionSummary 为已摘要（已被合并版本替代，保留向量索引和磁盘条目）
        memory.mark_summarized(target_id)?;

        Ok(())
    }

    /// 新建独立 SessionSummary 并注入 promoted_from 元数据。
    /// 返回创建的 SessionSummary 的 ID（供 Episode 封包使用）。
    async fn create_new_session_summary(
        &self,
        memory: &MemoryManager,
        summary: &str,
        importance: f64,
        source_ids: &[String],
        now: f64,
        mood_tags: &[String],
        date_labels: &[String],
        time_of_days: &[String],
    ) -> VivianResult<String> {
        let char_id_for_mem = memory.char_id().to_string();
        let init_meta = json!({
            "channel": "inner",
            "speaker": char_id_for_mem,
            "listener": char_id_for_mem,
            "perspective": "speaker",
            "knowledge_source": "extracted",
        });
        let item = memory
            .add_memory_with_metadata(
                summary,
                MemoryType::SessionSummary,
                importance,
                vec!["session_summary".to_string()],
                init_meta,
            )
            .await?;

        let date_label_primary = date_labels.first().cloned();
        let time_of_day_primary = time_of_days.first().cloned();
        let mut patch = json!({
            "promoted_from": source_ids,
            "promoted_at": now,
            "consolidation_stage": "stage1",
        });
        if let Some(obj) = patch.as_object_mut() {
            if !mood_tags.is_empty() {
                obj.insert(
                    "mood_tags".to_string(),
                    serde_json::Value::Array(
                        mood_tags.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                    ),
                );
            }
            if !date_labels.is_empty() {
                obj.insert(
                    "date_labels".to_string(),
                    serde_json::Value::Array(
                        date_labels.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                    ),
                );
                if let Some(primary) = &date_label_primary {
                    obj.insert("date_label".to_string(), serde_json::Value::String(primary.clone()));
                }
            }
            if !time_of_days.is_empty() {
                obj.insert(
                    "time_of_days".to_string(),
                    serde_json::Value::Array(
                        time_of_days.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                    ),
                );
                if let Some(primary) = &time_of_day_primary {
                    obj.insert("time_of_day".to_string(), serde_json::Value::String(primary.clone()));
                }
            }
        }
        memory.patch_memory_metadata(&item.id, patch)?;
        Ok(item.id)
    }

}

/// 巩固报告
///
/// 目前只有 Stage 1 有效：长期事实统一由 AutoExtractor 从原始对话提取，
/// 不再由摘要二次抽取（否则同一表述会在多层重复保存与召回）。
/// Stage 2 / Stage 3 相关字段已随之移除。
#[derive(Debug, Default)]
pub struct ConsolidationReport {
    pub stage1_summaries: usize,
}

// ===== LLM 响应解析 =====

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SummaryItem {
    summary: String,
    importance: f64,
    #[serde(default)]
    mood_tags: Vec<String>,
    #[serde(default)]
    date_labels: Vec<String>,
    #[serde(default)]
    time_of_days: Vec<String>,
}

// ===== JSON Schema 定义（用于 consolidation 任务的 schema 级约束） =====

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct SummaryListSchema {
    /// 摘要列表
    items: Vec<SummaryItem>,
}

fn consolidation_array_schema<T: schemars::JsonSchema>() -> serde_json::Value {
    let root = schemars::schema_for!(T);
    serde_json::to_value(&root.schema).unwrap_or_else(|_| {
        serde_json::json!({"type": "object"})
    })
}

fn parse_summaries(response: &str) -> Vec<SummaryItem> {
    parse_json_array(response)
}

fn parse_json_array<T: serde::de::DeserializeOwned>(response: &str) -> Vec<T> {
    let json_str = extract_json_from_response(response);
    // schema 返回 {"items":[...]}；旧模型仍可能返回裸数组。
    let value = serde_json::from_str::<serde_json::Value>(&json_str).ok().or_else(|| {
        let start = json_str.find('[')?;
        let end = json_str.rfind(']')?;
        serde_json::from_str::<serde_json::Value>(&json_str[start..=end]).ok()
    });
    let items = match value {
        Some(serde_json::Value::Array(items)) => Some(items),
        Some(serde_json::Value::Object(mut object)) => object.remove("items").and_then(|v| v.as_array().cloned()),
        _ => None,
    };
    match items.and_then(|items| serde_json::from_value::<Vec<T>>(serde_json::Value::Array(items)).ok()) {
        Some(items) => items,
        None => {
            tracing::warn!("[ConsolidationPipeline] JSON 数组解析失败，响应前缀: {}", &json_str[..json_str.len().min(200)]);
            Vec::new()
        }
    }
}

fn extract_json_from_response(response: &str) -> String {
    let trimmed = response.trim();
    // 尝试提取 ```json ... ``` 块
    if let Some(start) = trimmed.find("```json") {
        let after_start = &trimmed[start + 7..];
        if let Some(end) = after_start.find("```") {
            return after_start[..end].trim().to_string();
        }
    }
    // 尝试提取 ``` ... ``` 块
    if let Some(start) = trimmed.find("```") {
        let after_start = &trimmed[start + 3..];
        if let Some(end) = after_start.find("```") {
            return after_start[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

/// 两个字符串切片的并集，保持 `a` 顺序在前、`b` 中新元素追加在后；按值去重。
fn union_strings(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = a.to_vec();
    for s in b {
        if !out.iter().any(|x| x == s) {
            out.push(s.clone());
        }
    }
    out
}

/// 从文本中提取关键词集合（用于 Jaccard 相似度降级方案）。
///
/// 策略：按非字母数字字符分词，转小写，过滤掉长度 < 2 的 token。
/// 中英文混合文本中，中文按连续字符提取（CJK 字符独立成词）。
fn extract_keywords(text: &str) -> std::collections::HashSet<String> {
    let mut kw = std::collections::HashSet::new();
    // 按非字母数字分词
    for token in text.split(|c: char| !c.is_alphanumeric()) {
        let lower = token.to_lowercase();
        if lower.len() >= 2 {
            kw.insert(lower);
        }
    }
    // 额外：CJK 字符每字独立成词（中文无空格分隔）
    for ch in text.chars() {
        if ch.is_alphanumeric() && ch as u32 >= 0x4E00 && ch as u32 <= 0x9FFF {
            kw.insert(ch.to_string());
        }
    }
    kw
}

/// 检测新摘要相对于既有内容是否冗余（核心句子已存在）。
///
/// 按中英文句号/分号分句，逐句做子串包含检测（去除首尾空白后 ≥ 6 字符的句子参与比对）。
/// 若超过 70% 的有效句子已存在于旧内容中，判定为冗余，合并时应跳过文本追加。
fn is_content_redundant(existing: &str, new_summary: &str) -> bool {
    let sentences: Vec<&str> = new_summary
        .split(|c: char| c == '。' || c == '；' || c == '.' || c == ';')
        .map(|s| s.trim())
        .filter(|s| s.chars().count() >= 6)
        .collect();

    if sentences.is_empty() {
        // 无有效句子（太短），用整体子串检测兜底
        let trimmed = new_summary.trim();
        return trimmed.len() >= 6 && existing.contains(trimmed);
    }

    let contained_count = sentences
        .iter()
        .filter(|s| existing.contains(*s))
        .count();

    (contained_count as f64) / (sentences.len() as f64) > 0.7
}

#[cfg(test)]
mod companion_memory_tests {
    use super::*;

    #[test]
    fn schema_wrapped_summaries_keep_nested_arrays() {
        let response = r#"{"items":[{"summary":"用户明天面试","importance":0.7,"mood_tags":["worried"],"date_labels":[],"time_of_days":[]}]}"#;
        let items = parse_summaries(response);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].mood_tags, vec!["worried"]);
        assert!(parse_summaries(r#"{"items":[]}"#).is_empty());
    }
}
