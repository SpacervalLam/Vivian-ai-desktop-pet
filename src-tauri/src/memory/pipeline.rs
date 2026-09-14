//! 记忆巩固流水线：三阶段触发器实现"短期→长期"的真正巩固。
//!
//! 与 `retention.rs`（仅做过期清理+字面去重）互补，这里实现的是需要 LLM 参与
//! 的深度巩固链路：
//!
//! - **Stage 1（summarize）**：ShortTerm 满/会话空闲 → LLM 多主题摘要 → MidTerm SessionSummary
//! - **Stage 2（reflect）**：MidTerm 热度 ≥ 阈值 → LLM 抽取画像/事实 → LongTerm
//! - **Stage 3（insight）**：新增 LongTerm ≥ N 或累计 importance ≥ 阈值 → LLM 聚类生成洞察 → Insight
//!
//! 触发条件：
//! - `H_segment = α·N_visit+β·L_interaction+γ·e^(-Δt/24h)` ≥ 5.0 触发三路 LLM
//! - 新增观察 ≥ 3 触发 reflection
//! - importance 累计 ≥ 150 触发 reflection
//!
//! 所有 LLM 调用走 `routing_matrix["consolidation"]`（需强推理模型）。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::age::compute_heat_score;
use super::embedding::MemoryEmbeddingProvider;
use super::manager::MemoryManager;
use super::types::{current_timestamp, MemoryItem, MemoryType};
use super::vector_search::cosine_similarity;
use crate::config::manager::ConsolidationConfig;
use crate::error::VivianResult;
use crate::persona::AcquiredBehavior;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;
use crate::memory::user_model::UserModelManager;

/// Stage 2 触发热度阈值：MidTerm SessionSummary 的 H_segment ≥ 此值时触发画像/事实抽取
///
/// 取值 2.5：新摘要 H≈1.05（visit=0, R_recency=1.0），需被检索 3 次即可触发；
/// 配合下方的 24h 兜底触发，确保未被检索的旧摘要有机会沉淀为 LongTerm，
/// 避免大量 SessionSummary 永远停留在 MidTerm 导致长期记忆稀薄。
const STAGE2_HEAT_THRESHOLD: f64 = 2.5;
/// Stage 2 兜底触发：SessionSummary 创建满 24h 且未被检索（visit_count=0）时触发
/// 避免低频访问的摘要永远无法沉淀
const STAGE2_FALLBACK_AGE_HOURS: f64 = 24.0;
/// Stage 3 触发新 LongTerm 条数阈值
const STAGE3_NEW_LTM_THRESHOLD: usize = 5;
/// Stage 3 触发累计 importance 阈值
const STAGE3_IMPORTANCE_SUM: f64 = 8.0;

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
    /// 自上次 Stage 3 反思以来新增的 LongTerm 条数
    new_ltm_count: std::sync::atomic::AtomicUsize,
    /// 自上次 Stage 3 反思以来新增的 LongTerm 累计 importance
    new_ltm_importance_sum: std::sync::atomic::AtomicU64,
    /// 上次 Stage 3 反思的时间戳（秒）；0 表示从未执行过。
    /// Stage 3 只聚类此时间戳之后新增的 LongTerm，避免重复 Insight。
    last_reflection_at: std::sync::atomic::AtomicU64,
    /// 锁定核心文本（来自 PersonaConfig::locked_core_summary）。
    ///
    /// Stage 2 反思抽取动态行为时，将此文本作为「不可修改的人设边界」注入 prompt，
    /// 让 LLM 明确哪些字段是只读的，避免污染角色身份核心。
    /// 空字符串表示未设置（不影响现有逻辑）。
    locked_core_text: parking_lot::RwLock<String>,
    /// 上次 Stage 1 执行的时间戳（秒）；0 表示从未执行过。
    /// 两次 Stage 1 之间至少间隔 STAGE1_COOLDOWN_SEC，防止 tick 串行重入。
    last_stage1_at: std::sync::atomic::AtomicU64,
    /// 运行锁：防止对话路径（post_process_memory_async）与 tick 路径
    /// （proactive_tick 日常巩固检查）并发执行 run() 产生竞态。
    /// 用 try_lock 而非 lock，冲突时直接跳过本次（下一个 tick 会重试）。
    run_lock: tokio::sync::Mutex<()>,
    /// 反思退避表：key 为全部源 ID 排序后的指纹，value 为 (失败次数, 下次重试时间戳)。
    /// 失败时指数退避，避免持续失败时每 tick 浪费 LLM 配额。
    reflection_backoff: parking_lot::Mutex<HashMap<String, (u32, f64)>>,
    /// 已成功反思的源指纹集合：同批源 ID 已处理过则 short-circuit 跳过。
    reflection_done: parking_lot::Mutex<HashSet<String>>,
    /// 用户认知模型（可选；注入后 Stage 3 末尾执行概念归并，把 Insight 沉淀为概念层）。
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
            new_ltm_count: std::sync::atomic::AtomicUsize::new(0),
            new_ltm_importance_sum: std::sync::atomic::AtomicU64::new(0),
            last_reflection_at: std::sync::atomic::AtomicU64::new(0),
            last_stage1_at: std::sync::atomic::AtomicU64::new(0),
            locked_core_text: parking_lot::RwLock::new(String::new()),
            run_lock: tokio::sync::Mutex::new(()),
            reflection_backoff: parking_lot::Mutex::new(HashMap::new()),
            reflection_done: parking_lot::Mutex::new(HashSet::new()),
            user_model: parking_lot::RwLock::new(None),
            progress: parking_lot::RwLock::new(None),
        }
    }

    /// 绑定角色 ID，启用 Stage 1 断点续跑（由 BrainChatChain 构造后调用）
    pub fn set_progress_char_id(&self, char_id: &str) {
        let path = crate::utils::path::get_user_data_dir()
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
    /// 传入空字符串等价于清除锁定核心，Stage 2 反思将不引用任何不可修改边界。
    pub fn set_locked_core(&self, text: String) {
        *self.locked_core_text.write() = text;
    }

    /// 计算源 ID 集合的确定性指纹（排序后 SHA-256 截断）。
    /// 同批源 ID 产生相同指纹，用于幂等 short-circuit 和退避 key。
    fn source_fingerprint(source_ids: &[String]) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut sorted: Vec<&String> = source_ids.iter().collect();
        sorted.sort();
        let mut hasher = DefaultHasher::new();
        for id in &sorted {
            id.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    /// 检查该批源是否已在退避期内（失败退避）或已成功处理（幂等跳过）。
    /// 返回 Some(reason) 表示应跳过，None 表示可以执行。
    fn check_reflection_gate(&self, fingerprint: &str, now: f64) -> Option<&'static str> {
        if self.reflection_done.lock().contains(fingerprint) {
            return Some("already_done");
        }
        let backoff = self.reflection_backoff.lock();
        if let Some(&(fail_count, next_retry)) = backoff.get(fingerprint) {
            if now < next_retry {
                return Some("backoff");
            }
            let _ = fail_count;
        }
        None
    }

    /// 记录反思成功：标记指纹为已完成，清除退避记录。
    fn record_reflection_success(&self, fingerprint: &str) {
        self.reflection_done.lock().insert(fingerprint.to_string());
        self.reflection_backoff.lock().remove(fingerprint);
        if self.reflection_done.lock().len() > 200 {
            let mut done = self.reflection_done.lock();
            let remove_count = done.len().saturating_sub(100);
            let keys: Vec<String> = done.iter().take(remove_count).cloned().collect();
            for k in keys {
                done.remove(&k);
            }
        }
    }

    /// 记录反思失败：递增失败次数，按指数退避设置下次重试时间。
    fn record_reflection_failure(&self, fingerprint: &str, now: f64) {
        let mut backoff = self.reflection_backoff.lock();
        let entry = backoff.entry(fingerprint.to_string()).or_insert((0, now));
        entry.0 = entry.0.saturating_add(1);
        let delay = 60.0 * 2f64.powi(entry.0 as i32).min(8.0);
        entry.1 = now + delay;
    }

    /// LLM 路由访问器（供外部高层阶段如 Belief 生成复用 reflection 路由）。
    pub fn router(&self) -> Arc<ModelRouter> {
        Arc::clone(&self.router)
    }

    /// 读取锁定核心文本的快照（用于 Stage 2 prompt 注入）。
    fn get_locked_core(&self) -> String {
        self.locked_core_text.read().clone()
    }

    /// 记录新增 LongTerm（由 AutoExtractor 或其他路径写入时调用）
    pub fn notify_new_ltm(&self, importance: f64) {
        self.new_ltm_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // importance × 100 转为 u64 以原子累加
        let imp_u64 = (importance * 100.0).round() as u64;
        self.new_ltm_importance_sum.fetch_add(imp_u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// 执行完整的巩固检查（每轮对话后或 proactive tick 调用）。
    ///
    /// 按顺序检查 Stage 1 → Stage 2 → Stage 3 触发条件，满足则执行。
    /// 所有 LLM 调用均使用 `reflection` 路由。
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

        // Stage 2: MidTerm → LongTerm (画像/事实抽取 + 语义级行为画像 + 关系信号 + L1近期状态)
        let stage2_result = self.stage2_reflect(memory).await?;
        if let Some((count, behaviors, signals, recent_state)) = stage2_result {
            report.stage2_facts = count;
            report.stage2_acquired_behaviors = behaviors;
            report.stage2_relationship_signals = signals;
            report.stage2_recent_state = recent_state;
        }

        // Stage 3: LongTerm → Insight (聚类洞察)
        if let Some((count, insights)) = self.stage3_insight(memory).await? {
            report.stage3_insights = count;
            // Stage 3.5: Insight → 概念归并（UserModel + 图谱），把洞察沉淀为概念层
            if let Some(n) = self.stage3_concept(memory, &insights).await? {
                report.stage3_concepts = n;
            }
        }

        // 索引漂移检测：长期增删后向量索引可能与记忆条目脱节，必要时全量重建
        // 在巩固流水线末尾执行，避免与 Stage 1-3 的向量写入冲突
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
        let short_term: Vec<&MemoryItem> = all
            .iter()
            .filter(|m| {
                let is_short_term = m.tags.iter().any(|t| t == "short_term")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "short_term")
                        .unwrap_or(false);
                if !is_short_term {
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
                format!("[{}]{}{}{} {}", m.id, date_line, tod_line, mood_line, m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        // 注入 persona（locked_core）让模型以角色身份整理记忆
        let locked_core = self.get_locked_core();
        let persona_section = if locked_core.is_empty() {
            String::new()
        } else {
            format!(
                "\n[角色身份]\n下面是你此刻的角色身份；整理记忆时就按这个身份记。\n\
                 角色设定只决定你的记忆口吻、在意点和情感余温，不是这段对话发生过的事实。\n{}\n\n",
                locked_core
            )
        };

        // 注入前次阶段摘要作为参考（防幻觉连续性约束）
        let recent_summaries_for_ref = self.get_recent_session_summaries(memory, &all).await;
        let reference_section = if recent_summaries_for_ref.is_empty() {
            String::new()
        } else {
            let ref_text = recent_summaries_for_ref
                .iter()
                .map(|m| {
                    let mood = m.mood_tags();
                    let mood_line = if mood.is_empty() {
                        String::new()
                    } else {
                        format!(" [情绪余温: {}]", mood.join(","))
                    };
                    format!("- {}{}", m.content, mood_line)
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
             请将以下多条短期记忆摘要为1-3条主题级会话摘要。每条摘要应保留关键事实、情感和关系信息，去除冗余细节。\n\
             {reference_section}\
             输出JSON数组，每项含：\n\
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
                .with_json_schema(consolidation_array_schema::<SummaryListSchema>()),
        ).await?;
        let summaries = parse_summaries(&response);

        if summaries.is_empty() {
            tracing::warn!("[ConsolidationPipeline] Stage 1 LLM 返回空摘要，跳过");
            return Ok(None);
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
                m.tags.iter().any(|t| t == "session_summary")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "session_summary")
                        .unwrap_or(false)
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

    /// Stage 2: 中期记忆反思 → 长期事实/画像 + 语义级行为画像
    ///
    /// 触发条件：MidTerm SessionSummary 的 H_segment ≥ `stage2_heat_threshold`
    /// 动作：并行四路 LLM 抽取（用户画像 / 关系事件 / 行为模式 / 语义级行为画像），
    ///       前三路合并写入 LongTerm，第四路返回 AcquiredBehavior 列表
    ///
    /// 四路并行：独立视角的并行分析比单次混合抽取更精准，
    /// 且 tokio::join! 让四路 LLM 调用并发执行，总延迟 ≈ 最慢一路而非四路之和。
    async fn stage2_reflect(
        &self,
        memory: &MemoryManager,
    ) -> VivianResult<Option<(usize, Vec<AcquiredBehavior>, Vec<RelationshipSignalItem>, Option<L1RecentStateUpdate>)>> {
        let all = memory.get_all_memories().await?;
        let now = current_timestamp();

        // 筛选 SessionSummary 并计算热度
        let hot_summaries: Vec<&MemoryItem> = all
            .iter()
            .filter(|m| {
                let is_session_summary = m.tags.iter().any(|t| t == "session_summary")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "session_summary")
                        .unwrap_or(false);
                if !is_session_summary {
                    return false;
                }
                // 热度计算：H = α·visit_count + β·interaction_len + γ·R_recency
                let heat = compute_heat_score(
                    m.visit_count,
                    m.content.chars().count(),
                    m.last_visit_at,
                    now,
                );
                if heat >= STAGE2_HEAT_THRESHOLD {
                    return true;
                }
                // 兜底触发：创建满 24h 且未被检索（visit_count=0）的摘要也参与沉淀
                // 避免低频访问的摘要永远停留在 MidTerm 导致长期记忆稀薄
                let age_hours = ((now - m.timestamp).max(0.0)) / 3600.0;
                m.visit_count == 0 && age_hours >= STAGE2_FALLBACK_AGE_HOURS
            })
            .collect();

        if hot_summaries.is_empty() {
            return Ok(None);
        }

        let source_ids: Vec<String> = hot_summaries.iter().map(|m| m.id.clone()).collect();
        let fingerprint = Self::source_fingerprint(&source_ids);

        if let Some(reason) = self.check_reflection_gate(&fingerprint, now) {
            tracing::debug!(
                "[ConsolidationPipeline] Stage 2 跳过（{}）：{} 条源摘要",
                reason,
                hot_summaries.len()
            );
            return Ok(None);
        }

        let content_text = hot_summaries
            .iter()
            .map(|m| format!("[{}] {}", m.id, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        // 并行六路 LLM 分析（tokio::join! 并发轮询，总延迟 ≈ 最慢一路）
        let (profile_res, relationship_res, behavior_res, persona_res, signal_res, recent_state_res) = tokio::join!(
            self.stage2_extract_profile(&content_text),
            self.stage2_extract_relationships(&content_text),
            self.stage2_extract_behaviors(&content_text),
            self.stage2_extract_dynamic_persona(&content_text),
            self.stage2_extract_relationship_signal(&content_text),
            self.stage2_extract_recent_state(&content_text),
        );

        // 合并三路事实结果（各路失败仅 warn，不影响其他路）
        let mut facts = Vec::new();
        match profile_res {
            Ok(f) => facts.extend(f),
            Err(e) => tracing::warn!("[ConsolidationPipeline] Stage 2 用户画像抽取失败: {}", e),
        }
        match relationship_res {
            Ok(f) => facts.extend(f),
            Err(e) => tracing::warn!("[ConsolidationPipeline] Stage 2 关系事件抽取失败: {}", e),
        }
        match behavior_res {
            Ok(f) => facts.extend(f),
            Err(e) => tracing::warn!("[ConsolidationPipeline] Stage 2 行为模式抽取失败: {}", e),
        }

        // 第四路：语义级行为画像（失败仅 warn，不影响前三路）
        let acquired_behaviors = match persona_res {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("[ConsolidationPipeline] Stage 2 语义级行为画像抽取失败: {}", e);
                Vec::new()
            }
        };

        // 第五路：关系信号（失败仅 warn，不影响其他路）
        let relationship_signals = match signal_res {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("[ConsolidationPipeline] Stage 2 关系信号抽取失败: {}", e);
                Vec::new()
            }
        };

        // 第六路：L1 近期状态（失败仅 warn，不影响其他路）
        let recent_state = match recent_state_res {
            Ok(s) if !s.recent_goals.is_empty() || !s.current_projects.is_empty() || !s.recent_preferences.is_empty() => Some(s),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!("[ConsolidationPipeline] Stage 2 L1 近期状态抽取失败: {}", e);
                None
            }
        };

        if facts.is_empty() && acquired_behaviors.is_empty() && relationship_signals.is_empty() && recent_state.is_none() {
            tracing::warn!("[ConsolidationPipeline] Stage 2 六路 LLM 均返回空结果，跳过");
            self.record_reflection_failure(&fingerprint, now);
            return Ok(None);
        }

        let mut created = 0usize;

        for f in &facts {
            let mem_type = match f.fact_type.as_str() {
                "user" => MemoryType::User,
                "preference" => MemoryType::Preference,
                "relationship" => MemoryType::ImportantEvent,
                "feedback" => MemoryType::Feedback,
                _ => MemoryType::LongTerm,
            };
            // topic 作为 tag 写入，便于后续按主题聚类检索
            // assistant 标记当前角色对该反思产物的归属（巩固产物由该角色的"大脑"反思生成）
            let topic_tag = format!("topic:{}", f.topic);
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
                    &f.fact,
                    mem_type,
                    f.importance,
                    vec![f.fact_type.clone(), topic_tag, "assistant".to_string()],
                    init_meta,
                )
                .await?;

            let patch = json!({
                "promoted_from": source_ids,
                "promoted_at": now,
                "consolidation_stage": "stage2",
                "topic": f.topic,
            });
            memory.patch_memory_metadata(&item.id, patch)?;
            created += 1;

            // 证据主动再评估：用新 fact 检索相似旧 LongTerm，
            // 若词法矛盾则对旧记忆应用 Negates 信号，使其 evidence_score 下降
            // 长期累积触发归档（解决"用户改主意后旧记忆仍存留"问题）
            self.reassess_evidence_for_new_fact(memory, &f.fact).await;

            // 通知 Stage 3 计数器
            self.notify_new_ltm(f.importance);
        }

        // 标记已抽取的源 SessionSummary 为已摘要（保留向量索引，避免反复触发 Stage 2 重复抽取）
        for m in &hot_summaries {
            if let Err(e) = memory.mark_summarized(&m.id) {
                tracing::warn!(
                    "[ConsolidationPipeline] Stage 2 标记源 SessionSummary {} 失败: {}",
                    m.id,
                    e
                );
            }
        }

        self.record_reflection_success(&fingerprint);

        tracing::info!(
            "[ConsolidationPipeline] Stage 2: {} 条 SessionSummary → {} 条 LongTerm + {} 条语义行为 + {} 条关系信号 + L1更新={}（六路并行，源已清理）",
            hot_summaries.len(),
            created,
            acquired_behaviors.len(),
            relationship_signals.len(),
            recent_state.is_some()
        );
        Ok(Some((created, acquired_behaviors, relationship_signals, recent_state)))
    }

    /// 证据主动再评估：对新写入的 LongTerm fact，检索语义相似的旧持久型记忆，
    /// 若检测到词法矛盾（如"喜欢 X" vs "不喜欢 X"），则对旧记忆应用 Negates 信号。
    ///
    /// 解决"用户改主意后旧记忆仍存留"问题：旧记忆的 disputation 累积，
    /// evidence_score 下降至 -2.0 后进入 sub_zero 倒计时，14 天后归档。
    async fn reassess_evidence_for_new_fact(
        &self,
        memory: &MemoryManager,
        new_fact: &str,
    ) {
        // 检索 top-3 相似的持久型记忆
        let similar = memory.find_similar_persistent_memories(new_fact, 3);
        if similar.is_empty() {
            return;
        }

        let mut disputed = 0usize;
        for (old_id, old_content, _sim) in similar {
            // 词法矛盾检测（本地规则，零 LLM 调用）
            if super::conflict::detect_local_contradiction(new_fact, &old_content).is_some() {
                if let Err(e) = memory.apply_evidence_to_memory(
                    &old_id,
                    super::evidence::EvidenceSource::UserFact,
                    super::evidence::SignalKind::Negates,
                ) {
                    tracing::warn!(
                        "[ConsolidationPipeline] Stage 2 证据再评估失败 (old_id={}): {}",
                        old_id,
                        e
                    );
                } else {
                    disputed += 1;
                    tracing::info!(
                        "[ConsolidationPipeline] Stage 2 证据再评估：新 fact 与旧记忆 {} 词法矛盾，应用 Negates 信号",
                        old_id
                    );
                }
            }
        }
        if disputed > 0 {
            tracing::info!(
                "[ConsolidationPipeline] Stage 2 证据再评估：本次共对 {} 条旧记忆应用 Negates",
                disputed
            );
        }
    }

    /// Stage 2 路径 1：用户画像抽取（身份 + 偏好）
    async fn stage2_extract_profile(&self, content_text: &str) -> VivianResult<Vec<FactItem>> {
        let prompt = format!(
            "请从以下会话摘要中抽取用户的身份事实和偏好。\n\n\
             关注：姓名、年龄、职业、所在地、技能、喜好（食物/音乐/活动等）、\n\
             持续性偏好（非一次性提及）。\n\n\
             输出JSON数组，每项含：\n\
             - \"fact\"(string): 一条简洁事实陈述\n\
             - \"type\"(string): \"user\" 或 \"preference\"\n\
             - \"importance\"(0.0-1.0)，评分标准：\n\
               0.9-1.0 硬性约束/核心身份/健康信息；0.6-0.8 长期偏好/关键决策；\n\
               0.3-0.5 一般事实/上下文；0.0-0.2 闲聊/临时话题\n\
             - \"topic\"(string): 主题标签（如\"identity\"/\"food\"/\"hobby\"，\n\
               无明确主题时填\"general\"）\n\
             仅输出JSON，无其他文本。\n\n\
             会话摘要：\n{}",
            content_text
        );
        let response = self.router.generate(
            LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                .with_json_schema(consolidation_array_schema::<FactListSchema>()),
        ).await?;
        Ok(parse_facts(&response))
    }

    /// Stage 2 路径 2：关系事件抽取（里程碑 + 情感变化）
    async fn stage2_extract_relationships(&self, content_text: &str) -> VivianResult<Vec<FactItem>> {
        let prompt = format!(
            "请从以下会话摘要中抽取用户与 Vivian 之间的关系事件和情感变化。\n\n\
             关注：约定/承诺、共同经历、关系里程碑、情感转折点、冲突与和解、\n\
             信任变化、亲密度变化。\n\n\
             输出JSON数组，每项含：\n\
             - \"fact\"(string): 一条简洁关系事实陈述\n\
             - \"type\"(string): \"relationship\"\n\
             - \"importance\"(0.0-1.0)，评分标准：\n\
               0.9-1.0 重大关系里程碑；0.6-0.8 关系事件/共同经历；\n\
               0.3-0.5 一般关系上下文；0.0-0.2 闲聊/临时话题\n\
             - \"topic\"(string): 主题标签（如\"milestone\"/\"conflict\"/\"promise\"，\n\
               无明确主题时填\"relationship\"）\n\
             仅输出JSON，无其他文本。\n\n\
             会话摘要：\n{}",
            content_text
        );
        let response = self.router.generate(
            LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                .with_json_schema(consolidation_array_schema::<FactListSchema>()),
        ).await?;
        Ok(parse_facts(&response))
    }

    /// Stage 2 路径 3：行为模式抽取（反馈 + 习惯 + 主题归并）
    async fn stage2_extract_behaviors(&self, content_text: &str) -> VivianResult<Vec<FactItem>> {
        let prompt = format!(
            "请从以下会话摘要中抽取用户的行为模式、反馈和习惯，并跨摘要归并同一主题。\n\n\
             ## 任务\n\
             1. 识别跨多个摘要的同一主题（如「项目 X」、「健康习惯」、「学习计划」），\n\
                将相关片段归并为主题级事实，避免碎片化重复。\n\
             2. 抽取用户对 Vivian 的反馈（喜欢/不喜欢的回复方式、话题偏好）。\n\
             3. 抽取用户的行为习惯（作息、工作模式、互动节奏）。\n\n\
             输出JSON数组，每项含：\n\
             - \"fact\"(string): 一条简洁事实陈述（已归并的主题用一句话概括）\n\
             - \"type\"(string): \"feedback\"\n\
             - \"importance\"(0.0-1.0)，评分标准：\n\
               0.9-1.0 硬性约束/核心偏好；0.6-0.8 长期反馈/习惯；\n\
               0.3-0.5 一般行为模式；0.0-0.2 闲聊/临时话题\n\
             - \"topic\"(string): 主题标签（如\"project_x\"/\"health\"/\"work_pattern\"，\n\
               无明确主题时填\"general\"）\n\
             仅输出JSON，无其他文本。\n\n\
             会话摘要：\n{}",
            content_text
        );
        let response = self.router.generate(
            LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                .with_json_schema(consolidation_array_schema::<FactListSchema>()),
        ).await?;
        Ok(parse_facts(&response))
    }

    /// Stage 2 路径 4：语义级行为画像抽取（动态人设演化）
    ///
    /// 从会话摘要中归纳 Vivian 已表现出的稳定行为模式，作为可演化的动态人设层。
    /// 与路径 3（行为模式抽取）的区别：路径 3 抽取的是**用户**的行为模式，
    /// 本路径抽取的是**Vivian 自己**已表现出的语言风格、行为举止、互动方式、习得能力。
    ///
    /// 从历史交互中归纳智能体的行为特征，作为 prompt 注入的动态信号源
    /// （与 PersonaConfig 的锁定核心相对）。
    async fn stage2_extract_dynamic_persona(
        &self,
        content_text: &str,
    ) -> VivianResult<Vec<AcquiredBehavior>> {
        let locked_core = self.get_locked_core();
        let locked_section = if locked_core.is_empty() {
            String::new()
        } else {
            format!(
                "\n{}\n（上述为不可修改的人设核心，抽取的动态行为不得与之冲突）\n",
                locked_core
            )
        };

        let prompt = format!(
            "请从以下会话摘要中归纳 **Vivian 自己** 已表现出的稳定行为模式。\n\n\
             ## 任务\n\
             抽取四类语义级行为：\n\
             1. **language_style**（语言风格）：句式、词汇偏好、语气特征\n\
                如「常用短句+语气词」「偶尔中英混用」「喜欢用反问句」\n\
             2. **behavior**（行为举止）：Vivian 主动做出的稳定行为\n\
                如「主动追问项目进度」「用户疲惫时主动安慰」「记得用户提到的人名」\n\
             3. **interaction**（互动方式）：沟通节奏、提问方式\n\
                如「不直接否定用户」「用问句开启新话题」「吐槽后补一句软话」\n\
             4. **skill**（习得能力）：新学会的工具或技能\n\
                如「学会了查询天气」「能识别用户工作日程」「知道用日历工具」\n\
             \
             ## 抽取原则\n\
             - 只抽取 Vivian **已经表现出**的行为，不抽取用户的行为\n\
             - 只抽取 **稳定** 的模式（多次出现或一次明确表现），不抽取一次性偶发行为\n\
             - 描述要具体可操作，避免空泛（如「对用户友好」过于空泛）\n\
             - 每类最多抽取 2 条，避免噪音{}\n\
             \
             ## 输出格式\n\
             JSON 数组，每项含：\n\
             - \"category\"(string): \"language_style\" / \"behavior\" / \"interaction\" / \"skill\"\n\
             - \"description\"(string): 一句话描述\n\
             - \"confidence\"(0.0-1.0): 置信度（多次出现≥0.7，单次明确≥0.5）\n\
             仅输出JSON，无其他文本。\n\n\
             会话摘要：\n{}",
            locked_section,
            content_text
        );
        let response = self
            .router
            .generate(
                LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                    .with_json_schema(consolidation_array_schema::<AcquiredBehaviorListSchema>()),
            )
            .await?;
        Ok(parse_acquired_behaviors(&response))
    }

    /// Stage 2 路径 5：关系信号抽取（写入关系日志）
    ///
    /// 从会话摘要中识别用户情绪状态、关系信号、重要时刻和下次回应提示。
    /// 与路径 2（关系事件抽取）的区别：路径 2 抽取的是具体关系事实（写入 LongTerm），
    /// 本路径抽取的是每轮的情绪信号和回应线索（写入关系日志，影响下次回应方式）。
    async fn stage2_extract_relationship_signal(
        &self,
        content_text: &str,
    ) -> VivianResult<Vec<RelationshipSignalItem>> {
        let prompt = format!(
            "请从以下会话摘要中识别用户与 Vivian 互动时的关系信号和情绪状态。\n\n\
             ## 任务\n\
             为摘要中每个可识别的互动片段抽取一组关系信号：\n\
             1. **user_mood**(string): 用户当时的情绪状态\n\
                如「疲惫」「焦虑」「低落」「开心」「平静」「烦躁」「兴奋」「失落」\n\
             2. **relationship_signal**(string): 用户对 Vivian 的态度信号\n\
                如「亲近」「疏远」「信任」「试探」「依赖」「敷衍」「真诚」「回避」\n\
             3. **important_moment**(string, 可选): 值得记住的关系瞬间\n\
                如里程碑、第一次某行为、情感转折点；无则留空\n\
             4. **next_care_cue**(string): 基于 Vivian 的视角，下次该如何回应\n\
                如「用户疲惫时少打扰」「主动关心项目进度」「避免追问」\n\
             \
             ## 抽取原则\n\
             - 基于用户在对话中表现出的情绪和态度，不臆测\n\
             - next_care_cue 要具体可操作，避免空泛（如「对用户好」过于空泛）\n\
             - 每个摘要最多抽取 2 组信号，避免噪音\n\
             - 没有明确信号时返回空数组\n\
             \
             ## 输出格式\n\
             JSON 数组，每项含上述四个字段。仅输出 JSON，无其他文本。\n\n\
             会话摘要：\n{}",
            content_text
        );
        let response = self
            .router
            .generate(
                LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                    .with_json_schema(consolidation_array_schema::<RelationshipSignalListSchema>()),
            )
            .await?;
        Ok(parse_json_array(&response))
    }

    /// Stage 2 路径 6：L1 近期状态抽取（更新到 UserFactStore）
    ///
    /// 从会话摘要中识别用户最近的目标、正在做的项目、近期偏好。
    /// 与路径 1（用户画像）的区别：路径 1 抽取的是长期稳定事实（写入 L0/L2），
    /// 本路径抽取的是"最近在忙什么"（写入 L1，会被新状态覆盖）。
    async fn stage2_extract_recent_state(
        &self,
        content_text: &str,
    ) -> VivianResult<L1RecentStateUpdate> {
        let prompt = format!(
            "请从以下会话摘要中识别用户最近的动态状态。\n\n\
             ## 任务\n\
             抽取三类近期状态：\n\
             1. **recent_goals**(string[]): 用户最近的目标或想做的事\n\
                如「准备考研」「找工作」「想学吉他」「计划减肥」\n\
             2. **current_projects**(string[]): 用户当前正在进行的项目或任务\n\
                如「在开发一个网站」「在写毕业论文」「在做XX项目」\n\
             3. **recent_preferences**(string[]): 用户最近表现出的偏好或兴趣\n\
                如「最近在听后摇」「最近迷上原神」「最近喜欢熬夜」\n\
             \
             ## 抽取原则\n\
             - 只抽取「最近」的状态，不抽取长期稳定的身份信息（姓名/职业等）\n\
             - 基于用户在对话中明确表达的内容，不臆测\n\
             - 每类最多 3 条，避免噪音\n\
             - 没有明确信号时对应字段返回空数组\n\
             - 如果三类都没有明确信号，整体返回空 JSON 对象\n\
             \
             ## 输出格式\n\
             JSON 对象，含上述三个字段。仅输出 JSON，无其他文本。\n\n\
             会话摘要：\n{}",
            content_text
        );
        let response = self
            .router
            .generate(
                LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                    .with_json_schema({
                        let root = schemars::schema_for!(L1RecentStateUpdate);
                        serde_json::to_value(&root.schema).unwrap_or_else(|_| serde_json::json!({"type": "object"}))
                    }),
            )
            .await?;
        let cleaned = extract_json_from_response(&response);
        let update: L1RecentStateUpdate = serde_json::from_str(&cleaned).unwrap_or_default();
        Ok(update)
    }

    /// Stage 3: 长期记忆聚类 → 洞察生成
    ///
    /// 触发条件：新增 LongTerm ≥ `stage3_new_ltm_threshold` 或
    ///           累计 importance ≥ `stage3_importance_sum`
    /// 动作：LLM 把**自上次反思以来新增**的 LongTerm 聚类生成高层洞察，写入 Insight。
    /// 用 `last_reflection_at` 时间戳去重，避免对相同来源反复生成相似 Insight。
    async fn stage3_insight(&self, memory: &MemoryManager) -> VivianResult<Option<(usize, Vec<InsightItem>)>> {
        let new_count = self.new_ltm_count.load(std::sync::atomic::Ordering::Relaxed);
        let imp_sum_raw = self.new_ltm_importance_sum.load(std::sync::atomic::Ordering::Relaxed);
        let imp_sum = imp_sum_raw as f64 / 100.0;

        if new_count < STAGE3_NEW_LTM_THRESHOLD
            && imp_sum < STAGE3_IMPORTANCE_SUM
        {
            return Ok(None);
        }

        // 只聚类自上次 Stage 3 反思以来新增的 LongTerm（去重核心）
        let last_reflection = self.last_reflection_at.load(std::sync::atomic::Ordering::Relaxed);
        let all = memory.get_all_memories().await?;
        let now = current_timestamp();

        let recent_ltm: Vec<&MemoryItem> = all
            .iter()
            .filter(|m| {
                // 时间过滤：只取上次反思之后创建的 LTM
                if last_reflection > 0 && m.timestamp < last_reflection as f64 {
                    return false;
                }
                m.tags.iter().any(|t| t == "long_term" || t == "user" || t == "preference" || t == "important_event" || t == "feedback" || t == "knowledge")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "long_term" || s == "user" || s == "preference" || s == "important_event" || s == "feedback" || s == "knowledge")
                        .unwrap_or(false)
            })
            .collect();

        if recent_ltm.len() < 3 {
            // 新增 LTM 不足以聚类，重置计数器避免反复触发
            self.new_ltm_count.store(0, std::sync::atomic::Ordering::Relaxed);
            self.new_ltm_importance_sum.store(0, std::sync::atomic::Ordering::Relaxed);
            return Ok(None);
        }

        let stage3_source_ids: Vec<String> = recent_ltm.iter().map(|m| m.id.clone()).collect();
        let stage3_fp = Self::source_fingerprint(&stage3_source_ids);
        if let Some(reason) = self.check_reflection_gate(&stage3_fp, now) {
            tracing::debug!(
                "[ConsolidationPipeline] Stage 3 跳过（{}）：{} 条源 LTM",
                reason,
                recent_ltm.len()
            );
            return Ok(None);
        }

        let content_text = recent_ltm
            .iter()
            .map(|m| format!("[{}] {}", m.id, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "请从以下长期记忆中发现模式、规律或高层洞察。生成1-3条洞察。\n\
             洞察应是超越单条事实的抽象结论，如行为模式、关系变化趋势、深层偏好等。\n\
             输出JSON数组，每项含：\n\
             - \"insight\"(string): 一条洞察陈述\n\
             - \"importance\"(0.0-1.0)，评分标准：\n\
               0.9-1.0 核心性格洞察/重大关系趋势；0.6-0.8 重要行为模式/偏好规律；\n\
               0.3-0.5 一般观察；0.0-0.2 浅层/临时观察\n\
             - \"source_ids\"(string[]): 支持此洞察的记忆ID列表\n\
             仅输出JSON，无其他文本。\n\n\
             长期记忆：\n{}",
            content_text
        );

        let response = self.router.generate(
            LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                .with_json_schema(consolidation_array_schema::<InsightListSchema>()),
        ).await?;
        let insights = parse_insights(&response);

        if insights.is_empty() {
            tracing::warn!("[ConsolidationPipeline] Stage 3 LLM 返回空洞察，跳过");
            self.record_reflection_failure(&stage3_fp, now);
            return Ok(None);
        }

        // Semantic Reinforcement：新洞察生成前先找重叠旧洞察，
        // 重叠分（共享 source_ids 数）≥ SEMANTIC_REINFORCEMENT_OVERLAP 则合并而非新建。
        let existing_insights: Vec<&MemoryItem> = all
            .iter()
            .filter(|m| {
                m.tags.iter().any(|t| t == "insight")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "insight")
                        .unwrap_or(false)
            })
            .collect();

        let mut created = 0usize;
        let mut reinforced = 0usize;
        // 本次产出的洞察（供 Stage 3.5 概念归并复用，避免重复 LLM）
        let mut produced: Vec<InsightItem> = Vec::new();
        for ins in &insights {
            produced.push((*ins).clone());
            // 查找重叠旧洞察
            let reinforce_target = self.find_reinforce_target(ins, &existing_insights);

            match reinforce_target {
                Some(target_id) => {
                    // 合并到既有洞察（强化而非新建）
                    if let Err(e) = self
                        .reinforce_insight(memory, &target_id, &ins.insight, ins.importance, &ins.source_ids, now)
                        .await
                    {
                        tracing::warn!(
                            "[ConsolidationPipeline] Stage 3 强化洞察 {} 失败，回退新建: {}",
                            target_id,
                            e
                        );
                        self.create_new_insight(memory, &ins.insight, ins.importance, &ins.source_ids, now)
                            .await?;
                        created += 1;
                    } else {
                        reinforced += 1;
                        tracing::debug!(
                            "[ConsolidationPipeline] Stage 3 语义强化：合并到既有 Insight {}（共享 source_ids ≥ {}）",
                            target_id,
                            Self::SEMANTIC_REINFORCEMENT_OVERLAP
                        );
                    }
                }
                None => {
                    self.create_new_insight(memory, &ins.insight, ins.importance, &ins.source_ids, now)
                        .await?;
                    created += 1;
                }
            }
        }

        // 重置计数器，并记录本次反思时间（后续只聚类此时间之后新增的 LTM）
        self.new_ltm_count.store(0, std::sync::atomic::Ordering::Relaxed);
        self.new_ltm_importance_sum.store(0, std::sync::atomic::Ordering::Relaxed);
        self.last_reflection_at.store(now as u64, std::sync::atomic::Ordering::Relaxed);
        self.record_reflection_success(&stage3_fp);

        tracing::info!(
            "[ConsolidationPipeline] Stage 3: {} 条 LongTerm → {} 条新建 + {} 条强化 Insight（下次从 ts={} 起聚类）",
            recent_ltm.len(),
            created,
            reinforced,
            now as u64
        );
        Ok(Some((created + reinforced, produced)))
    }

    /// Stage 3.5: Insight → 概念归并（概念层）
    ///
    /// 在 Stage 3 生成 Insight 后执行（需注入 `user_model`）：
    /// - 用 LLM 把本次洞察归纳为高层概念（key/meaning/related_topics/strength）
    /// - 归并进 UserModel（`merge_concept`：同名强化、异名新建）
    /// - 写入知识图谱（`ingest_concepts`：概念作为 Concept 实体 + related_topics 边）
    ///
    /// 让"用户长期在乎什么"沉淀为可检索的概念层，支撑跨主题的主题联想。
    async fn stage3_concept(
        &self,
        memory: &MemoryManager,
        insights: &[InsightItem],
    ) -> VivianResult<Option<usize>> {
        let user_model = match self.user_model.read().as_ref() {
            Some(um) => um.clone(),
            None => return Ok(None),
        };
        if insights.is_empty() {
            return Ok(None);
        }

        // 已有概念（供 LLM 判断归并到已有概念 vs 新建）
        let existing_text = {
            let model = user_model.read();
            if model.traits.is_empty() {
                String::new()
            } else {
                model
                    .traits
                    .iter()
                    .map(|t| {
                        let meaning = if t.meaning.is_empty() {
                            t.value.clone()
                        } else {
                            t.meaning.clone()
                        };
                        let rel = if t.related_topics.is_empty() {
                            "无".to_string()
                        } else {
                            t.related_topics.join(" / ")
                        };
                        format!("- {}（{}）相关主题：{}", t.key, meaning, rel)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };

        let insight_text = insights
            .iter()
            .map(|i| format!("- [{}] {}", i.source_ids.join(","), i.insight))
            .collect::<Vec<_>>()
            .join("\n");

        let existing_section = if existing_text.is_empty() {
            "（暂无）".to_string()
        } else {
            existing_text
        };

        let prompt = format!(
            "请从以下高层洞察中归纳用户的长期概念（\"用户长期在乎什么\"）。\n\n\
             ## 任务\n\
             识别跨主题的稳定概念，如自主性、可靠性、UI 审美、工作效率等。\n\
             每个概念应：\n\
             - key：概念名（英文小写蛇形，如 agent_autonomy / ui_aesthetic）\n\
             - value：简要状态（如 high / prefers_soft_ui）\n\
             - meaning：一句话说明\"用户为什么在乎\"\n\
             - related_topics：与之关联的主题标签（跨主题关联，如 proactive、inner_monologue、observation）\n\
             - evidence_ids：支持该概念的洞察来源 ID（直接引用输入的 [id]）\n\
             - strength：0.0-1.0，该概念对用户的重要程度\n\
             若某洞察与【已有概念】语义相同，应归并到已有概念而非新建（related_topics 合并）。\n\n\
             ## 已有概念\n\
             {}\n\n\
             ## 新洞察\n\
             {}\n\n\
             输出JSON数组，仅输出JSON，无其他文本。",
            existing_section, insight_text
        );

        let response = self.router.generate(
            LLMRequest::new("consolidation", vec![ChatMessage::user(prompt)])
                .with_json_schema(consolidation_array_schema::<ConceptListSchema>()),
        ).await?;
        let concepts = parse_concepts(&response);
        if concepts.is_empty() {
            tracing::warn!("[ConsolidationPipeline] Stage 3.5 LLM 返回空概念，跳过");
            return Ok(None);
        }

        let now = current_timestamp();
        let mut merged = 0usize;
        for c in &concepts {
            let key = c.key.trim();
            if key.is_empty() {
                continue;
            }
            user_model.merge_concept(
                key,
                &c.value,
                &c.meaning,
                &c.related_topics,
                &c.evidence_ids,
                c.strength,
            );
            // 概念写入知识图谱（Concept 实体 + related_topics 边），供图谱概念检索
            let (e_count, edge_count) = memory
                .knowledge_graph()
                .ingest_concepts(key, &c.related_topics, &c.evidence_ids, now);
            if e_count > 0 || edge_count > 0 {
                tracing::debug!(
                    "[ConsolidationPipeline] Stage 3.5 概念 {} 写入图谱：{} 实体 / {} 边",
                    key,
                    e_count,
                    edge_count
                );
            }
            merged += 1;
        }
        let _ = memory.knowledge_graph().save_to_disk();

        tracing::info!(
            "[ConsolidationPipeline] Stage 3.5: {} 条 Insight → {} 条概念（归并进 UserModel + 图谱）",
            insights.len(),
            merged
        );
        Ok(Some(merged))
    }

    /// Semantic Reinforcement 阈值：新洞察与既有洞察共享 source_ids ≥ 此值时合并。
    ///
    /// 通常意味着同一主题的高层抽象，应强化而非新建独立洞察。
    const SEMANTIC_REINFORCEMENT_OVERLAP: usize = 2;

    /// 在既有洞察中查找与新洞察共享 source_ids ≥ 阈值的条目。
    fn find_reinforce_target(
        &self,
        new_insight: &InsightItem,
        existing: &[&MemoryItem],
    ) -> Option<String> {
        let new_sources: std::collections::HashSet<&str> = new_insight
            .source_ids
            .iter()
            .map(|s| s.as_str())
            .collect();
        if new_sources.is_empty() {
            return None;
        }
        let mut best_id: Option<String> = None;
        let mut best_overlap = 0usize;
        for m in existing {
            let existing_sources: std::collections::HashSet<&str> = m
                .metadata
                .get("promoted_from")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect()
                })
                .unwrap_or_default();
            let overlap = new_sources.intersection(&existing_sources).count();
            if overlap >= Self::SEMANTIC_REINFORCEMENT_OVERLAP && overlap > best_overlap {
                best_overlap = overlap;
                best_id = Some(m.id.clone());
            }
        }
        best_id
    }

    /// 强化既有洞察：合并内容、取较大 importance、追加 source_ids、递增 reinforcement_count。
    async fn reinforce_insight(
        &self,
        memory: &MemoryManager,
        target_id: &str,
        new_insight: &str,
        new_importance: f64,
        new_source_ids: &[String],
        now: f64,
    ) -> VivianResult<()> {
        let all = memory.get_all_memories().await?;
        let target = all
            .iter()
            .find(|m| m.id == target_id)
            .ok_or_else(|| crate::error::VivianError::Memory(format!("目标 Insight 不存在: {target_id}")))?;

        let merged_content = format!("{}\n{}", target.content, new_insight);
        let merged_importance = target.importance.max(new_importance);

        // 合并 source_ids（去重）
        let mut merged_sources: Vec<String> = Vec::new();
        if let Some(existing) = target.metadata.get("promoted_from").and_then(|v| v.as_array()) {
            for v in existing {
                if let Some(s) = v.as_str() {
                    if !merged_sources.iter().any(|x| x == s) {
                        merged_sources.push(s.to_string());
                    }
                }
            }
        }
        for s in new_source_ids {
            if !merged_sources.iter().any(|x| x == s) {
                merged_sources.push(s.clone());
            }
        }

        // 递增 reinforcement_count
        let prev_count = target
            .metadata
            .get("reinforcement_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let new_count = prev_count + 1;

        let char_id_for_mem = memory.char_id().to_string();
        let init_meta = json!({
            "channel": "inner",
            "speaker": char_id_for_mem,
            "listener": char_id_for_mem,
            "perspective": "speaker",
            "knowledge_source": "extracted",
        });
        let new_item = memory
            .add_memory_with_metadata(
                &merged_content,
                MemoryType::Insight,
                merged_importance,
                vec!["insight".to_string()],
                init_meta,
            )
            .await?;

        let mut merged_metadata = target.metadata.clone();
        if let Some(obj) = merged_metadata.as_object_mut() {
            obj.insert(
                "promoted_from".to_string(),
                serde_json::Value::Array(
                    merged_sources.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                ),
            );
            obj.insert("promoted_at".to_string(), serde_json::json!(now));
            obj.insert("consolidation_stage".to_string(), serde_json::json!("stage3_reinforce"));
            obj.insert("reinforced_from".to_string(), serde_json::json!(target_id));
            obj.insert("reinforcement_count".to_string(), serde_json::json!(new_count));
        } else {
            merged_metadata = serde_json::json!({
                "promoted_from": merged_sources,
                "promoted_at": now,
                "consolidation_stage": "stage3_reinforce",
                "reinforced_from": target_id,
                "reinforcement_count": new_count,
            });
        }
        memory.patch_memory_metadata(&new_item.id, merged_metadata)?;
        memory.mark_summarized(target_id)?;

        Ok(())
    }

    /// 新建独立 Insight 并注入 promoted_from 元数据
    async fn create_new_insight(
        &self,
        memory: &MemoryManager,
        insight: &str,
        importance: f64,
        source_ids: &[String],
        now: f64,
    ) -> VivianResult<()> {
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
                insight,
                MemoryType::Insight,
                importance,
                vec!["insight".to_string()],
                init_meta,
            )
            .await?;
        let patch = json!({
            "promoted_from": source_ids,
            "promoted_at": now,
            "consolidation_stage": "stage3",
        });
        memory.patch_memory_metadata(&item.id, patch)?;
        Ok(())
    }
}

/// 巩固报告
#[derive(Debug, Default)]
pub struct ConsolidationReport {
    pub stage1_summaries: usize,
    pub stage2_facts: usize,
    pub stage3_insights: usize,
    /// Stage 3.5 归并进 UserModel + 图谱的概念数
    pub stage3_concepts: usize,
    /// Stage 2 第四路抽取的语义级行为画像（由 BrainChatChain 合并到 DynamicBehaviorProfile）
    pub stage2_acquired_behaviors: Vec<AcquiredBehavior>,
    /// Stage 2 第五路抽取的关系信号（由 BrainChatChain 写入关系日志）
    pub stage2_relationship_signals: Vec<RelationshipSignalItem>,
    /// Stage 2 第六路抽取的 L1 近期状态（由 BrainChatChain 更新到 UserFactStore）
    pub stage2_recent_state: Option<L1RecentStateUpdate>,
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct FactItem {
    fact: String,
    #[serde(rename = "type")]
    fact_type: String,
    importance: f64,
    /// 主题标签（LLM 跨摘要归并输出，默认 general）
    #[serde(default = "default_topic")]
    topic: String,
}

/// Stage 2 第五路：关系信号抽取的 LLM 输出项
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct RelationshipSignalItem {
    /// 用户情绪（疲惫/焦虑/低落/开心/平静/烦躁等）
    pub user_mood: String,
    /// 关系信号（亲近/疏远/信任/试探/依赖等）
    pub relationship_signal: String,
    /// 重要时刻（关系里程碑或值得记住的瞬间，可为空）
    #[serde(default)]
    pub important_moment: Option<String>,
    /// 下次回应提示（Vivian 下次该如何回应）
    #[serde(default)]
    pub next_care_cue: String,
}

/// Stage 2 第六路：L1 近期状态抽取的 LLM 输出项
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
pub struct L1RecentStateUpdate {
    /// 最近目标（如"准备考研""找工作"）
    #[serde(default)]
    pub recent_goals: Vec<String>,
    /// 当前项目（如"在开发一个网站""在写毕业论文"）
    #[serde(default)]
    pub current_projects: Vec<String>,
    /// 近期偏好（如"最近在听后摇""最近迷上原神"）
    #[serde(default)]
    pub recent_preferences: Vec<String>,
}

fn default_topic() -> String {
    "general".to_string()
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct InsightItem {
    insight: String,
    importance: f64,
    #[serde(default)]
    source_ids: Vec<String>,
}

/// Stage 3.5：概念归纳的 LLM 输出项
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ConceptItem {
    /// 概念名（英文小写蛇形，如 agent_autonomy）
    key: String,
    /// 简要状态（如 high / prefers_soft_ui）
    #[serde(default)]
    value: String,
    /// 一句话说明"用户为什么在乎"
    #[serde(default)]
    meaning: String,
    /// 关联主题（跨主题关联）
    #[serde(default)]
    related_topics: Vec<String>,
    /// 支持该概念的洞察来源 ID
    #[serde(default)]
    evidence_ids: Vec<String>,
    /// 重要程度 0.0-1.0
    #[serde(default = "default_concept_strength")]
    strength: f64,
}

fn default_concept_strength() -> f64 {
    0.5
}

/// LLM 返回的语义级行为画像中间结构（用于反序列化）
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AcquiredBehaviorItem {
    category: String,
    description: String,
    #[serde(default = "default_acquired_confidence")]
    confidence: f64,
}

fn default_acquired_confidence() -> f64 {
    0.5
}

// ===== JSON Schema 定义（用于 consolidation 任务的 schema 级约束） =====

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct SummaryListSchema {
    /// 摘要列表
    items: Vec<SummaryItem>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct FactListSchema {
    /// 事实列表
    items: Vec<FactItem>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct InsightListSchema {
    /// 洞察列表
    items: Vec<InsightItem>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct ConceptListSchema {
    /// 概念列表
    items: Vec<ConceptItem>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct AcquiredBehaviorListSchema {
    /// 习得行为列表
    items: Vec<AcquiredBehaviorItem>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct RelationshipSignalListSchema {
    /// 关系信号列表
    items: Vec<RelationshipSignalItem>,
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

fn parse_facts(response: &str) -> Vec<FactItem> {
    parse_json_array(response)
}

fn parse_insights(response: &str) -> Vec<InsightItem> {
    parse_json_array(response)
}

fn parse_concepts(response: &str) -> Vec<ConceptItem> {
    parse_json_array(response)
}

/// 解析 LLM 返回的语义级行为画像，转换为 `AcquiredBehavior`（带时间戳）
fn parse_acquired_behaviors(response: &str) -> Vec<AcquiredBehavior> {
    let items: Vec<AcquiredBehaviorItem> = parse_json_array(response);
    let now = current_timestamp();
    items
        .into_iter()
        .map(|item| AcquiredBehavior {
            category: crate::persona::AcquiredBehaviorCategory::from_str_lossy(&item.category),
            description: item.description,
            evidence: Vec::new(),
            confidence: item.confidence.clamp(0.0, 1.0),
            acquired_at: now,
        })
        .collect()
}

fn parse_json_array<T: serde::de::DeserializeOwned>(response: &str) -> Vec<T> {
    // 尝试从 markdown 代码块中提取 JSON
    let json_str = extract_json_from_response(response);
    match serde_json::from_str::<Vec<T>>(&json_str) {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!("[ConsolidationPipeline] JSON 解析失败: {}, 原始响应: {}", e, &json_str[..json_str.len().min(200)]);
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
    // 尝试提取 [...] 数组
    if let Some(start) = trimmed.find('[') {
        if let Some(end) = trimmed.rfind(']') {
            return trimmed[start..=end].to_string();
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
