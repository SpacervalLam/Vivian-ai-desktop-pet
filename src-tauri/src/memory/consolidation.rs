//! 记忆巩固 —— 夜间/空闲时整理记忆，模拟"睡眠巩固"
//!
//! 在深夜（2-5 点）或用户长时间离场时触发：
//! 1. 调用现有 ConsolidationPipeline 跑完整三阶段（ShortTerm→MidTerm→LongTerm→Insight）
//! 2. 强化近期重要记忆（提升 importance）
//! 3. 衰减无关的临时记忆
//! 4. Belief/Goal 生成（Stage 4）：从 Insight + LongTerm 提炼信念写入 Mind
//! 5. memory.md 整理（Stage 5）：由 [`MemoryConsolidator::tidy_memory_md`] 按 [`TidyNeed`] 分派增量整理或全量压缩
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

/// memory.md **全量压缩** system prompt：机械合并去重，不需要人设（用 memory 路由便宜模型）。
///
/// 这是兜底路径（文件逼近预算上限时触发），输入是全文、输出是按主题重排的精简正文。
/// 增量整理路径由 [`MEMORY_MD_INCREMENTAL_SYSTEM_PROMPT`] 与 `memory_md::merge_entries` 约定。
const MEMORY_MD_TIDY_SYSTEM_PROMPT: &str = "你是角色长期记忆笔记的整理器。把下面的笔记合并去重、删除过时或一次性内容、按主题分节组织（如 相处约定 / 承诺 / 教训 / 梗；`## 近期补充` 分节里的条目要归入合适的主题）。保留所有仍然有效的信息，每条一行、简洁。只输出重写后的 markdown 正文，不要输出文件标题、前言或总结。重写后须精简（控制在 1500 字符内）。";

/// memory.md **增量整理** system prompt。
///
/// 输入只有「新增沉淀」（几百字符），已整理正文仅用于去重比对，
/// 因此成本远低于全量压缩，可高频触发。与 `memory_md::merge_entries` 协议一致：
/// 普通行 = 新增条目，`REPLACE: 旧 => 新` = 就地改写正文里的某条。
const MEMORY_MD_INCREMENTAL_SYSTEM_PROMPT: &str = "你是角色长期记忆笔记的整理器。输入分两部分：「已整理的正文」（仅供你去重参考）和「新增沉淀」（本次要处理的原始素材）。\n\
只处理新增沉淀，输出需要并入正文的条目：\n\
- 每条一行，以 `- ` 开头，简洁陈述（如 `- 主人不喜欢香菜`），不带日期、不加引号、不解释。\n\
- 合并新增沉淀里重复或同类的内容。\n\
- 丢弃一次性的、已失效的、没有长期价值的内容——这类不必输出。\n\
- 已整理正文里已经有的信息不要重复输出。\n\
- 若新增沉淀明确更新或推翻了正文里的某一条，输出一行 `REPLACE: 正文里的原条目 => 新条目`，其中原条目必须与正文逐字一致。\n\
不要输出标题、前言、总结或任何解释性文字。";

/// 健康状态持久化路径（按角色隔离，避免多角色互相覆盖）
fn health_path(char_id: &str) -> PathBuf {
    crate::utils::path::get_companion_shared_dir().join(format!("consolidation_health_{char_id}.json"))
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

        // Stage 5: memory.md 整理
        // 按需触发（`memory_md::tidy_need` 判定）：待整理沉淀攒够行数走**增量整理**
        // （LLM 只读新增部分，机械并入正文），文件逼近预算上限才走**全量压缩**。
        // 两者都不满足则完全不动，一次 LLM 都不烧。
        // 机械合并去重，用 memory 路由（不需人设）；失败熔断沿用现有机制
        if !self.char_id.is_empty() {
            if let Some(reason) = self.health.is_paused("memory_md") {
                tracing::warn!("[MemoryConsolidator] memory.md 整理处于熔断暂停，跳过：{}", reason);
                all_ok = false;
            } else {
                let need = crate::memory::memory_md::tidy_need(&self.char_id);
                if need != crate::memory::memory_md::TidyNeed::None {
                    match self.tidy_memory_md(need).await {
                        Ok(true) => {
                            tracing::info!("[MemoryConsolidator] memory.md 已整理合并（{need:?}）");
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
        }

        // 成功烧满冷却；失败只烧短冷却（快速重试自愈）
        *self.last_consolidation.lock() = now;
        if !all_ok {
            tracing::info!("巩固部分失败，{:.0} 分钟后自动重试", FAILURE_RETRY_SEC / 60.0);
        }
        true
    }

    /// 整理角色长期记忆笔记（memory.md）。
    ///
    /// `need` 由 `memory_md::tidy_need` 判定，两条路径：
    /// - [`TidyNeed::Incremental`]：LLM 只读「待整理区」（新增沉淀），产出条目后由
    ///   `memory_md::merge_entries` 机械并入「已整理区」。输入规模从「全文」降到
    ///   「新增部分」；若并入后反而超预算，自动回落全量压缩。
    /// - [`TidyNeed::FullCompaction`]：LLM 读全文，按主题重排并精简（兜底路径）。
    ///
    /// 返回 `Ok(true)` 表示已重写；`Ok(false)` 表示无需整理（文件为空 / 没有待整理内容）；
    /// `Err` 表示整理失败（LLM 调用失败 / 返回空 / 重写后超预算被 write_memory_md 拒绝），
    /// 失败时保留原笔记不动。
    async fn tidy_memory_md(
        &self,
        need: crate::memory::memory_md::TidyNeed,
    ) -> Result<bool, String> {
        use crate::memory::memory_md;

        let existing = match memory_md::read_memory_md_raw(&self.char_id) {
            Some(t) => t,
            None => return Ok(false),
        };
        let router = self.pipeline.router();

        // ── 增量路径：只把新增沉淀交给 LLM，机械并入已整理正文 ──────────────
        if need == memory_md::TidyNeed::Incremental {
            let regions = memory_md::split_regions(&existing);
            if regions.pending_body.trim().is_empty() {
                return Ok(false);
            }
            let pending_lines = regions
                .pending_body
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();
            let entries = router
                .generate(LLMRequest::new(
                    "memory",
                    vec![
                        ChatMessage::system(MEMORY_MD_INCREMENTAL_SYSTEM_PROMPT),
                        ChatMessage::user(format!(
                            "（已整理的正文，仅供你去重参考，不要复述）\n{}\n\n（新增沉淀，需并入）\n{}",
                            regions.consolidated_body, regions.pending_body
                        )),
                    ],
                )
                .with_character_id(self.char_id.clone()))
                .await
                .map_err(|e| format!("增量整理 LLM 调用失败：{e}"))?;

            let entries = entries.trim();
            if !entries.is_empty() {
                let merged = memory_md::merge_entries(&regions.consolidated_body, entries);
                match memory_md::write_memory_md(&self.char_id, &merged) {
                    Ok(()) => {
                        tracing::info!(
                            "[MemoryConsolidator] memory.md 增量整理完成：已并入 {pending_lines} 行待整理沉淀"
                        );
                        return Ok(true);
                    }
                    // 并入后超预算 → 回落全量压缩（它才有权精简正文）
                    Err(e) => {
                        tracing::info!("[MemoryConsolidator] 增量并入后超预算（{e}），转全量压缩");
                    }
                }
            } else {
                tracing::info!(
                    "[MemoryConsolidator] 增量整理无新增条目，转全量压缩以清空待整理区"
                );
            }
        }

        // ── 全量压缩（兜底 / 文件逼近上限）──────────────────────────────────
        let rewritten = router
            .generate(LLMRequest::new(
                "memory",
                vec![
                    ChatMessage::system(MEMORY_MD_TIDY_SYSTEM_PROMPT),
                    ChatMessage::user(format!("（当前 memory.md 全文，需整理合并）\n{existing}")),
                ],
            )
            .with_character_id(self.char_id.clone()))
            .await
            .map_err(|e| format!("整理 LLM 调用失败：{e}"))?;
        let rewritten = rewritten.trim().to_string();
        if rewritten.is_empty() {
            return Err("整理 LLM 返回空内容（已保留原笔记）".into());
        }
        // write_memory_md 内部校验预算上限，超限返回 Err → 保留原文件不写
        memory_md::write_memory_md(&self.char_id, &rewritten)?;
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
