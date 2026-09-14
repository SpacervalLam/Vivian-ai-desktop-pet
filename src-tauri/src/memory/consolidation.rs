//! 记忆巩固 —— 夜间/空闲时整理记忆，模拟"睡眠巩固"
//!
//! 在深夜（2-5 点）或用户长时间离场时触发：
//! 1. 调用现有 ConsolidationPipeline 跑完整三阶段（ShortTerm→MidTerm→LongTerm→Insight）
//! 2. 强化近期重要记忆（提升 importance）
//! 3. 衰减无关的临时记忆
//! 4. Belief/Goal 生成（Stage 4）：从 Insight + LongTerm 提炼信念写入 Mind
//!
//! 设计：复用现有 MemoryManager 与 ConsolidationPipeline，不重复造轮子。
//!
//! 工程韧性：
//! - **失败不烧冷却**：巩固失败只回退短冷却（30 分钟后重试），凭证恢复后
//!   自愈；成功才烧满 6 小时冷却。
//! - **步骤健康跟踪**：每步（pipeline / belief）独立记录成败与连续失败计数，
//!   同根因错误只打一次 error（防刷屏），恢复时打恢复日志。
//!   健康快照持久化到 `<用户数据目录>/consolidation_health_<char>.json`，供 UI/诊断读取。

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::memory::manager::MemoryManager;
use crate::memory::pipeline::ConsolidationPipeline;
use crate::memory::step_health::StepHealthTracker;
use crate::mind::{BeliefGenerator, Mind};
use crate::providers::base::LLMRequest;
use crate::types::response::ChatMessage;

/// 成功后的完整冷却（6 小时）
const SUCCESS_COOLDOWN_SEC: f64 = 6.0 * 3600.0;
/// 失败后的快速重试冷却（30 分钟）——LLM/凭证类故障恢复后尽快自愈
const FAILURE_RETRY_SEC: f64 = 30.0 * 60.0;
/// 熔断暂停后的半开重试等待（1 小时）——暂停期间完全跳过该步骤，不烧 LLM
const PAUSE_COOLDOWN_SEC: f64 = 3600.0;

/// memory.md 整理 system prompt：机械合并去重，不需要人设（用 memory 路由便宜模型）。
const MEMORY_MD_TIDY_SYSTEM_PROMPT: &str = "你是角色长期记忆笔记的整理器。把下面的笔记合并去重、删除过时或一次性内容、按主题分节组织（如 相处约定 / 承诺 / 教训 / 梗）。保留所有仍然有效的信息，每条一行、简洁。只输出重写后的 markdown 正文，不要输出文件标题、前言或总结。重写后须精简（控制在 1500 字符内）。";

/// 健康状态持久化路径（按角色隔离，避免多角色互相覆盖）
fn health_path(char_id: &str) -> PathBuf {
    crate::utils::path::get_user_data_dir().join(format!("consolidation_health_{char_id}.json"))
}

/// 记忆巩固器
pub struct MemoryConsolidator {
    memory: Arc<MemoryManager>,
    pipeline: Arc<ConsolidationPipeline>,
    /// Belief/Goal 生成器（可选 —— 未注入 Mind 时不执行 Stage 4）
    belief_generator: Option<Arc<BeliefGenerator>>,
    /// 关联的 Mind（可选 —— 未注入时跳过 Belief 生成）
    mind: Option<Arc<Mind>>,
    /// 角色长期记忆笔记整理：上次整理时间戳，避免每日空跑
    memory_md_last_tidy: Mutex<f64>,
    /// 关联角色 ID（用于 memory.md 整理路径解析）
    char_id: String,
    /// 上次巩固时间戳
    last_consolidation: Mutex<f64>,
    /// 步骤健康跟踪（pipeline / belief / memory_md 三步）
    health: StepHealthTracker,
}

impl MemoryConsolidator {
    pub fn new(memory: Arc<MemoryManager>, pipeline: Arc<ConsolidationPipeline>) -> Self {
        Self {
            memory,
            pipeline,
            belief_generator: None,
            mind: None,
            memory_md_last_tidy: Mutex::new(0.0),
            char_id: String::new(),
            last_consolidation: Mutex::new(0.0),
            health: StepHealthTracker::load(None),
        }
    }

    /// 注入 Mind 与 BeliefGenerator，启用 Stage 4（Belief/Goal 生成）
    ///
    /// 由 Brain 在初始化 Mind 后调用。注入后每次巩固末尾会额外生成 Belief/Goal。
    pub fn with_mind(mut self, mind: Arc<Mind>) -> Self {
        let router = self.pipeline.router();
        self.belief_generator = Some(Arc::new(BeliefGenerator::new(router)));
        self.mind = Some(mind);
        self
    }

    /// 绑定角色 ID（切换到按角色隔离的健康持久化文件，并启用 memory.md 整理路径）
    ///
    /// 由 Brain 构造时调用；多角色各持一份健康状态，互不覆盖。
    pub fn with_char_id(mut self, char_id: &str) -> Self {
        self.health = StepHealthTracker::load(Some(health_path(char_id)));
        self.char_id = char_id.to_string();
        self
    }

    /// 是否冷却已过（成功后 6 小时 / 失败后 30 分钟）
    pub fn should_run(&self) -> bool {
        let now = chrono::Utc::now().timestamp() as f64;
        let last = *self.last_consolidation.lock();
        let cooldown = if self.health.is_healthy() {
            SUCCESS_COOLDOWN_SEC
        } else {
            FAILURE_RETRY_SEC
        };
        now - last >= cooldown
    }

    /// 执行一次记忆巩固
    ///
    /// 返回是否实际执行了巩固（冷却未到则跳过）。
    /// 失败不烧满冷却：30 分钟后自动重试，凭证/网络恢复即可自愈。
    /// 连续失败达阈值的步骤熔断暂停：完全跳过，1 小时后半开重试。
    pub async fn consolidate(&self) -> bool {
        // 半开恢复：暂停超过 1 小时的步骤解除暂停，允许本轮重试
        self.health.try_resume(PAUSE_COOLDOWN_SEC);

        let now = chrono::Utc::now().timestamp() as f64;
        if !self.should_run() {
            return false;
        }

        tracing::info!("开始夜间记忆巩固...");
        let mut all_ok = true;

        // 跑完整巩固流水线（Stage 1/2/3：ShortTerm→MidTerm→LongTerm→Insight）
        // ConsolidationPipeline::run 会处理 ShortTerm 摘要、画像抽取、Insight 生成
        if let Some(reason) = self.health.is_paused("pipeline") {
            tracing::warn!("[MemoryConsolidator] 巩固流水线处于熔断暂停，跳过：{}", reason);
            all_ok = false;
        } else {
            match self.pipeline.run(&self.memory).await {
                Ok(report) => {
                    tracing::info!("记忆巩固完成: {:?}", report);
                    self.health.mark_success("pipeline");
                }
                Err(e) => {
                    tracing::warn!("记忆巩固流水线失败: {}", e);
                    self.health.mark_failure("pipeline", &e.to_string());
                    all_ok = false;
                }
            }
        }

        // Stage 4: Belief/Goal 生成（仅在注入 Mind 时执行）
        if let (Some(gen), Some(mind)) = (&self.belief_generator, &self.mind) {
            if let Some(reason) = self.health.is_paused("belief") {
                tracing::warn!("[MemoryConsolidator] Belief 生成处于熔断暂停，跳过：{}", reason);
                all_ok = false;
            } else {
                match gen.generate(&self.memory, mind).await {
                    Ok(report) => {
                        tracing::info!("Belief 生成完成: {:?}", report);
                        self.health.mark_success("belief");
                    }
                    Err(e) => {
                        tracing::warn!("Belief 生成失败: {}", e);
                        self.health.mark_failure("belief", &e.to_string());
                        all_ok = false;
                    }
                }
            }
        }

        // Stage 5: memory.md 整理（仅在行数超阈值时触发，避免每日空跑浪费 LLM）
        // 机械合并去重，用 memory 路由（不需人设）；失败熔断沿用现有机制
        if !self.char_id.is_empty() {
            if let Some(reason) = self.health.is_paused("memory_md") {
                tracing::warn!("[MemoryConsolidator] memory.md 整理处于熔断暂停，跳过：{}", reason);
                all_ok = false;
            } else if crate::memory::memory_md::needs_tidy(&self.char_id) {
                match self.tidy_memory_md().await {
                    Ok(true) => {
                        tracing::info!("[MemoryConsolidator] memory.md 已整理合并");
                        self.health.mark_success("memory_md");
                        *self.memory_md_last_tidy.lock() = now;
                    }
                    Ok(false) => {
                        // 不需要整理（文件已被写侧驱逐压在阈值内 / 或为空）
                        self.health.mark_success("memory_md");
                    }
                    Err(e) => {
                        tracing::warn!("[MemoryConsolidator] memory.md 整理失败: {}", e);
                        self.health.mark_failure("memory_md", &e.to_string());
                        all_ok = false;
                    }
                }
            }
        }

        // 成功烧满冷却；失败只烧短冷却（快速重试自愈）
        *self.last_consolidation.lock() = now;
        if !all_ok {
            tracing::info!("巩固部分失败，{:.0} 分钟后自动重试", FAILURE_RETRY_SEC / 60.0);
        }
        true
    }

    /// 整理角色长期记忆笔记（memory.md）：LLM 合并去重、删过时、按主题分节。
    ///
    /// 返回 `Ok(true)` 表示已重写；`Ok(false)` 表示无需整理（文件为空）；
    /// `Err` 表示整理失败（LLM 调用失败 / 返回空 / 重写后超预算被 write_memory_md 拒绝），
    /// 失败时保留原笔记不动。
    async fn tidy_memory_md(&self) -> Result<bool, String> {
        let existing = match crate::memory::memory_md::read_memory_md_raw(&self.char_id) {
            Some(t) => t,
            None => return Ok(false),
        };
        let router = self.pipeline.router();
        let rewritten = router
            .generate(LLMRequest::new(
                "memory",
                vec![
                    ChatMessage::system(MEMORY_MD_TIDY_SYSTEM_PROMPT),
                    ChatMessage::user(format!("（当前 memory.md 全文，需整理合并）\n{existing}")),
                ],
            ))
            .await
            .map_err(|e| format!("整理 LLM 调用失败：{e}"))?;
        let rewritten = rewritten.trim().to_string();
        if rewritten.is_empty() {
            return Err("整理 LLM 返回空内容（已保留原笔记）".into());
        }
        // write_memory_md 内部校验预算上限，超限返回 Err → 保留原文件不写
        crate::memory::memory_md::write_memory_md(&self.char_id, &rewritten)?;
        Ok(true)
    }

    /// 启动恢复补偿
    ///
    /// 进程崩溃/退出时，后台可能留下未摘要的 ShortTerm 记忆（尚未达到条数阈值
    /// 或空闲阈值，Stage 1 未触发）。本方法在启动后无条件跑一遍流水线：
    /// - Stage 1 的空闲触发（最新 ShortTerm 距今 ≥ idle_timeout）天然捕获崩溃残留
    /// - Stage 2/3 各自的条件门控不满足则跳过，不浪费 LLM 调用
    /// - 成功后正常烧满冷却，失败只烧短冷却（复用 consolidate 的自愈语义）
    ///
    /// 返回值：本次恢复是否补跑了摘要（Stage 1 产出 > 0）。
    pub async fn recover(&self) -> bool {
        tracing::info!("[MemoryConsolidator] 启动恢复检查：扫描崩溃前未巩固的短期记忆...");
        let before_short_term = self.count_pending_short_term().await;
        let ran = self.consolidate().await;
        let recovered = ran && before_short_term > 0;
        if recovered {
            tracing::info!(
                "[MemoryConsolidator] 恢复完成：崩溃前遗留 {} 条短期记忆已进入巩固流水线",
                before_short_term
            );
        }
        recovered
    }

    /// 统计待摘要的 ShortTerm 记忆条数（与 Stage 1 相同的筛选口径，只读不写）
    async fn count_pending_short_term(&self) -> usize {        let Ok(all) = self.memory.get_all_memories().await else {
            return 0;
        };
        all.iter()
            .filter(|m| {
                let is_short_term = m.tags.iter().any(|t| t == "short_term")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "short_term")
                        .unwrap_or(false);
                let is_inner = m.tags.iter().any(|t| t == "inner_monologue")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "inner_monologue")
                        .unwrap_or(false);
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
                is_short_term && !is_inner && !is_observation
            })
            .count()
    }

    /// 健康快照（每步的成败/连续失败计数，供 UI / 诊断接口读取）
    pub fn health_status(&self) -> std::collections::HashMap<String, crate::memory::step_health::StepHealth> {
        self.health.snapshot()
    }
}
