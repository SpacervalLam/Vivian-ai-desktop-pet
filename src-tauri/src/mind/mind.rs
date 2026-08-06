//! Mind —— 角色认知聚合句柄。
//!
//! Mind 不替代 PsychologyManager，只聚合它。已有心理状态（emotion/needs/persona/
//! relationship）继续由 PsychologyManager 管理，Mind 只在其上增加 Belief/Goal/
//! Attention 三个一等公民，并统一提供序列化与检索接口。
//!
//! 使用方式：
//! - prompt 序列化：`mind.serialize_for_prompt()` → 结构化段落，替代散落 with_xxx
//! - 检索预过滤：`mind.attention_snapshot()` → 传入 PrecisionFilterCriteria
//! - Reflection 写回：`mind.apply_reflection(...)` → 由 ConsolidationPipeline 调用

use std::sync::Arc;

use parking_lot::RwLock;

use crate::psychology::PsychologyManager;
use crate::utils::path::get_character_data_dir;

use super::attention::Attention;
use super::belief::{BeliefStore, SharedBeliefStore};
use super::current_activity::CurrentActivityTracker;
use super::goal::{GoalStore, SharedGoalStore};
use super::user_goals::UserGoalLedger;
use super::working_memory::WorkingMemory;

/// 内心 OS 累积缓冲区上限。
///
/// 60s 节流的 current_thought 合成下约等于 22 分钟滑动窗口。
/// 超出时丢弃最旧条目，防止 long-idle 场景下 token 爆炸。
const MAX_ACCUMULATED_THOUGHTS: usize = 22;

/// 累积条目过期时间（秒）。drain 时早于此窗口的条目会被丢弃。
///
/// 5 小时：保证"深夜→清晨"这种跨段场景下，昨晚的思绪不会混入今早的独白。
const ACCUMULATED_THOUGHT_TTL_SECS: i64 = 5 * 3600;

/// 带时间戳的 current_thought 快照（内心 OS 提示词累积注入用）
#[derive(Debug, Clone)]
pub struct ThoughtSnapshot {
    /// Unix 秒（本地时区，仅用于展示格式化）
    pub timestamp: i64,
    /// 当时的 current_thought 内容
    pub text: String,
}

/// 角色认知聚合句柄
pub struct Mind {
    pub char_id: String,
    /// 已有心理架构（不持有所有权，Brain 仍是 owner，Mind 持有 Arc 引用）
    pub psychology: Arc<PsychologyManager>,
    /// 信念存储（Knowledge 层）
    pub beliefs: SharedBeliefStore,
    /// 目标集合（角色自身目标，影响 Attention 分配）
    pub goals: SharedGoalStore,
    /// 用户长期目标账本（用户的周~月级目标，带 deadline）
    ///
    /// 与 `goals` 的区别：`goals` 是角色自己的运行时目标（"陪伴主人"），
    /// `user_goals` 是用户的人生阶段目标（"准备考研"），用于 prompt 注入
    /// 让 LLM 有"用户当前处于什么阶段"的上下文。
    pub user_goals: Arc<UserGoalLedger>,
    /// 注意力（重启后从 attention.json 恢复，mind_tick 持续衰减）
    pub attention: Arc<RwLock<Attention>>,
    /// 工作记忆：30 秒级"正在想什么"缓冲区（重启后从 working_memory.json 恢复）
    pub working_memory: Arc<RwLock<WorkingMemory>>,
    /// 当前活动状态机：跨分钟级"正在做什么"（纯运行时，不持久化）
    ///
    /// 介于 WorldSnapshot（外部）和 WorkingMemory（瞬时想法）之间的中间层，
    /// 记录活动类型 + 持续时间 + 最近相关事件，类似 Neuro-sama 的"持续状态"。
    pub current_activity: Arc<CurrentActivityTracker>,
    /// LLM 合成的"当前想法"缓存（混合策略：60s 节流 + 事件驱动刷新）
    ///
    /// None 表示尚未完成首次 LLM 合成（冷启动时由模板 fallback 填充）。
    pub current_thought: Arc<RwLock<Option<String>>>,
    /// current_thought 累积缓冲区（内心 OS 提示词注入用）
    ///
    /// 每次 `set_current_thought` 写入新值前，把即将被覆盖的旧值连同时间戳推入。
    /// 内心 OS 触发时由调用方同步 drain，注入提示词后清空。
    /// 纯运行时，不持久化；`reset_all` 时一并清空。
    pub accumulated_thoughts: Arc<RwLock<Vec<ThoughtSnapshot>>>,
    /// 是否需要重新合成当前想法（事件驱动触发时设 true，SelfUpdate 阶段检查并消费）
    pub thought_refresh_requested: Arc<std::sync::atomic::AtomicBool>,
    /// 持久化目录：`<user_data>/characters/<char_id>/mind/`
    persistence_dir: std::path::PathBuf,
}

impl Mind {
    /// 从已有 PsychologyManager 构建 Mind，并加载 Belief/Goal 持久化数据
    pub fn load_or_init(char_id: &str, psychology: Arc<PsychologyManager>) -> Self {
        let char_dir = get_character_data_dir(char_id);
        let mind_dir = char_dir.join("mind");

        let beliefs = {
            let path = mind_dir.join("beliefs.json");
            let store = BeliefStore::load(&path);
            Arc::new(RwLock::new(store))
        };

        let goals = {
            let path = mind_dir.join("goals.json");
            let store = GoalStore::load(&path);
            Arc::new(RwLock::new(store))
        };

        let user_goals = Arc::new(UserGoalLedger::new(mind_dir.join("user_goals.json")));

        // 恢复 Attention（上次关闭时的快照），并确保核心实体有基线权重
        let attention = {
            let path = mind_dir.join("attention.json");
            let mut att = if path.exists() {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                serde_json::from_str::<Attention>(&content).unwrap_or_default()
            } else {
                Attention::new()
            };
            att.seed_baseline(chrono::Local::now().timestamp());
            Arc::new(RwLock::new(att))
        };

        // 恢复 WorkingMemory（上次关闭时的快照，mind_tick 会自然衰减过期条目）
        let working_memory = {
            let path = mind_dir.join("working_memory.json");
            if path.exists() {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let wm = serde_json::from_str::<WorkingMemory>(&content).unwrap_or_default();
                Arc::new(RwLock::new(wm))
            } else {
                Arc::new(RwLock::new(WorkingMemory::new()))
            }
        };
        let current_activity = Arc::new(CurrentActivityTracker::new());

        Self {
            char_id: char_id.to_string(),
            psychology,
            beliefs,
            goals,
            user_goals,
            attention,
            working_memory,
            current_activity,
            current_thought: Arc::new(RwLock::new(None)),
            accumulated_thoughts: Arc::new(RwLock::new(Vec::new())),
            thought_refresh_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            persistence_dir: mind_dir,
        }
    }

    /// 持久化 Belief、Goal、Attention 和 WorkingMemory
    ///
    /// Attention 和 WorkingMemory 在重启后恢复，避免每次启动都"失忆"。
    /// Attention 衰减由 mind_tick 负责，持久化的快照可能略微过期但不影响正确性。
    pub fn persist(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.persistence_dir)?;
        {
            let store = self.beliefs.read();
            store.save(&self.persistence_dir.join("beliefs.json"))?;
        }
        {
            let store = self.goals.read();
            store.save(&self.persistence_dir.join("goals.json"))?;
        }
        // Attention 持久化
        {
            let att = self.attention.read();
            let json = serde_json::to_string(&*att).unwrap_or_default();
            let _ = std::fs::write(self.persistence_dir.join("attention.json"), json);
        }
        // WorkingMemory 持久化
        {
            let wm = self.working_memory.read();
            let json = serde_json::to_string(&*wm).unwrap_or_default();
            let _ = std::fs::write(self.persistence_dir.join("working_memory.json"), json);
        }
        Ok(())
    }

    /// Attention 快照（用于检索预过滤，clone 后释放锁）
    pub fn attention_snapshot(&self) -> Attention {
        self.attention.read().clone()
    }

    /// 取 Top-N 注意力焦点（prompt 注入用）
    pub fn attention_top_n(&self, n: usize) -> Vec<(String, f32)> {
        self.attention.read().top_n(n)
            .into_iter()
            .map(|(k, w)| (k.clone(), w))
            .collect()
    }

    /// 提升 Attention（事件驱动入口）
    pub fn boost_attention(&self, entity: &str, boost: f32, now: i64) {
        self.attention.write().boost(entity, boost, now);
    }

    /// Attention 衰减（tick 驱动）
    pub fn decay_attention(&self, factor: f32, floor: f32) {
        self.attention.write().decay(factor, floor);
    }

    /// 推入工作记忆条目（对话/独白/世界事件后调用）
    pub fn push_working_memory(
        &self,
        content: String,
        source: super::working_memory::WorkingMemorySource,
    ) {
        let now = chrono::Local::now().timestamp();
        self.working_memory.write().push(content, source, now);
    }

    /// 工作记忆序列化（prompt 注入用）
    pub fn working_memory_prompt_section(&self, lang: &str) -> Option<String> {
        self.working_memory.read().serialize_for_prompt(lang)
    }

    /// 清空工作记忆（会话关闭时调用）
    pub fn clear_working_memory(&self) {
        self.working_memory.write().clear();
    }

    /// 恢复出厂设置：清空所有认知层持久化状态
    ///
    /// 清空范围：
    /// - beliefs（信念存储）
    /// - goals（目标集合）
    /// - attention（注意力焦点）
    /// - working_memory（工作记忆缓冲区）
    /// - current_thought（LLM 合成的当前想法缓存）
    /// - accumulated_thoughts（内心 OS 用的累积缓冲区）
    /// - thought_refresh_requested（刷新请求标志位）
    ///
    /// 心理架构（emotion/needs/relationship）由 PsychologyManager.reset_to_initial 负责。
    /// 调用后会持久化清空后的状态到 mind/ 目录下的各 JSON 文件。
    pub fn reset_all(&self) -> std::io::Result<()> {
        {
            let mut store = self.beliefs.write();
            *store = BeliefStore::new();
        }
        {
            let mut store = self.goals.write();
            *store = GoalStore::new();
        }
        if let Err(e) = self.user_goals.clear_all() {
            tracing::warn!("[Mind:{}] 清空用户目标失败: {}", self.char_id, e);
        }
        {
            let mut att = self.attention.write();
            *att = Attention::new();
        }
        {
            let mut wm = self.working_memory.write();
            wm.clear();
        }
        {
            let mut thought = self.current_thought.write();
            *thought = None;
        }
        {
            let mut acc = self.accumulated_thoughts.write();
            acc.clear();
        }
        self.thought_refresh_requested
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.persist()
    }

    /// 读取 LLM 合成的"当前想法"缓存（clone 后释放锁）
    pub fn current_thought_snapshot(&self) -> Option<String> {
        self.current_thought.read().clone()
    }

    /// 写入 LLM 合成的"当前想法"
    ///
    /// 副作用：写入前把即将被覆盖的旧值（如非空）连同时间戳推入
    /// `accumulated_thoughts` 缓冲区，供下次内心 OS 消费。
    /// 超出 `MAX_ACCUMULATED_THOUGHTS` 时丢弃最旧条目。
    pub fn set_current_thought(&self, thought: String) {
        {
            let mut cur = self.current_thought.write();
            if let Some(prev) = cur.take() {
                if !prev.trim().is_empty() {
                    self.push_accumulated_thought(prev);
                }
            }
            *cur = Some(thought);
        }
    }

    /// 把一条旧 current_thought 推入累积缓冲区（带本地时间戳）
    fn push_accumulated_thought(&self, text: String) {
        let mut buf = self.accumulated_thoughts.write();
        buf.push(ThoughtSnapshot {
            timestamp: chrono::Local::now().timestamp(),
            text,
        });
        if buf.len() > MAX_ACCUMULATED_THOUGHTS {
            let overflow = buf.len() - MAX_ACCUMULATED_THOUGHTS;
            buf.drain(..overflow);
        }
    }

    /// 同步 drain 累积缓冲区，返回全部快照并清空（内心 OS 触发时调用）
    ///
    /// 过滤规则（在 drain 时应用，保证"触发时刻"的精确语义）：
    /// - 超过 `ACCUMULATED_THOUGHT_TTL_SECS`（默认 5 小时）的条目丢弃
    /// - 剩余条目按写入顺序（即时间顺序）返回
    pub fn drain_accumulated_thoughts(&self) -> Vec<ThoughtSnapshot> {
        let mut buf = self.accumulated_thoughts.write();
        let taken = std::mem::take(&mut *buf);
        let original_len = taken.len();
        let now = chrono::Local::now().timestamp();
        let cutoff = now - ACCUMULATED_THOUGHT_TTL_SECS;
        let fresh: Vec<ThoughtSnapshot> = taken
            .into_iter()
            .filter(|s| s.timestamp >= cutoff)
            .collect();
        let discarded = original_len.saturating_sub(fresh.len());
        if discarded > 0 {
            tracing::debug!(
                "[accumulated_thoughts] drain: kept {}, discarded {} (TTL={}s)",
                fresh.len(),
                discarded,
                ACCUMULATED_THOUGHT_TTL_SECS
            );
        }
        fresh
    }

    /// 清空累积缓冲区（不调用 drain 的清理场景，如 reset_all）
    pub fn clear_accumulated_thoughts(&self) {
        self.accumulated_thoughts.write().clear();
    }

    /// 请求重新合成当前想法（事件驱动触发，下次 SelfUpdate 阶段消费）
    pub fn request_thought_refresh(&self) {
        self.thought_refresh_requested
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// 消费刷新请求标志位（返回是否有待处理的刷新请求）
    pub fn consume_thought_refresh(&self) -> bool {
        self.thought_refresh_requested
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// 当前念头序列化（prompt 注入用）
    ///
    /// 只输出 LLM 合成的当前想法摘要，不包含工作记忆条目（避免对话记录混入念头 section）。
    pub fn working_memory_prompt_section_with_thought(&self, lang: &str) -> Option<String> {
        let thought = self.current_thought_snapshot();
        if let Some(t) = thought {
            if !t.is_empty() {
                let header = crate::pipeline::prompt_modules::section_heading("current_thoughts", lang);
                return Some(format!("{}\n> {}", header, t));
            }
        }
        None
    }

    /// Mind Tick —— 30 秒级认知节拍（无 LLM）
    ///
    /// 统一处理三个 Mind 一等公民的周期性更新：
    /// 1. **Attention Drift**：基于 `last_activated` 的时间感知衰减
    ///    （替代旧的固定 factor 衰减，避免频率敏感）
    /// 2. **Goal Update**：优先级缓慢衰减 + 低优先级淘汰 + 活跃数上限（≤5）
    /// 3. **Belief Consolidation**：按 `BeliefCategory` 差异化 confidence 衰减
    ///    （Trait 不衰减 / State 高衰减）+ 低置信淘汰
    /// 4. **Working Memory Decay**：30 秒级工作记忆衰减
    /// 5. **Current Activity Expiry**：当前活动超时检查
    ///
    /// **Need Change 不在此处理**：由 `PsychologyManager::homeostasis_tick`（10s）
    /// 和 `micro_tick`（3-5s）独立负责，Mind 不重复。
    ///
    /// 调用方应保证 ~30s 间隔（由 `Brain::proactive_tick` 节流）。
    /// 每次调用后持久化 Belief/Goal（Attention 不持久化）。
    pub fn mind_tick(&self, dt_secs: f64, now: i64) {
        // 1. Attention Drift：基于时间的指数衰减
        //    衰减率：每分钟衰减到 ~0.85（即 0.85^(dt/60)），低于 0.05 的条目移除
        {
            let mut att = self.attention.write();
            let decay_factor = 0.85f32.powf((dt_secs / 60.0).max(0.0) as f32);
            att.focus.retain(|_, f| {
                f.weight *= decay_factor;
                f.weight >= 0.05
            });
            att.seed_baseline(now);
            att.last_updated = now;
        }

        // 2. Goal Update：优先级衰减 + 超时淘汰 + 活跃数上限
        {
            let mut store = self.goals.write();
            let dt_factor = (dt_secs / 30.0).max(0.0);
            for g in &mut store.goals {
                if g.active {
                    // 超时目标直接淘汰
                    if g.is_expired(now) {
                        g.active = false;
                        continue;
                    }
                    // 优先级衰减：每 30s 衰减到 g.decay_rate（默认 0.97）
                    g.priority *= g.decay_rate.powf(dt_factor);
                    if g.priority < 0.1 {
                        g.active = false;
                    }
                }
            }
            // 活跃数上限：保留 Top-5，其余置 inactive
            let active_count = store.goals.iter().filter(|g| g.active).count();
            if active_count > 5 {
                let mut priorities: Vec<f64> = store
                    .goals
                    .iter()
                    .filter(|g| g.active)
                    .map(|g| g.priority)
                    .collect();
                priorities.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                // 第 5 个（index 4）的优先级作为阈值，低于此值的淘汰
                if let Some(&threshold) = priorities.get(4) {
                    for g in &mut store.goals {
                        if g.active && g.priority < threshold {
                            g.active = false;
                        }
                    }
                }
            }
        }

        // 3. Belief Consolidation：按 category 差异化 confidence 衰减
        {
            let mut store = self.beliefs.write();
            let dt_min: f64 = (dt_secs / 60.0).max(0.0);
            for b in &mut store.beliefs {
                let decay_per_min: f64 = match b.category {
                    super::belief::BeliefCategory::Trait => 1.0,         // 特质不衰减
                    super::belief::BeliefCategory::Habit => 0.995,      // 习惯极慢衰减
                    super::belief::BeliefCategory::Preference => 0.997, // 偏好几乎不衰减
                    super::belief::BeliefCategory::State => 0.95,       // 状态高衰减
                    super::belief::BeliefCategory::Relationship => 0.99, // 关系慢衰减
                };
                b.confidence *= decay_per_min.powf(dt_min);
            }
            // 淘汰低置信（< 0.1）
            store.beliefs.retain(|b| b.confidence >= 0.1);
        }

        // 4. Working Memory Decay：30 秒级工作记忆衰减
        {
            self.working_memory.write().decay(dt_secs);
        }

        // 5. Current Activity Expiry：当前活动超时检查
        //    超过最大持续时间（30 分钟）的活动自动回 Idle，
        //    避免"在打 Minecraft"这种活动状态在用户离开后无限残留。
        {
            self.current_activity.check_expiry(now);
        }

        // 持久化 Belief/Goal/Attention/WorkingMemory
        let _ = self.persist();
    }

    /// 持久化目录路径（外部检查用）
    pub fn persistence_dir(&self) -> &std::path::PathBuf {
        &self.persistence_dir
    }

    /// 序列化为 prompt 段落 —— 把 Belief / Goal / Attention 三个一等公民
    /// 统一渲染成结构化文本，供 PromptBuildingStep 注入。
    ///
    /// 注意：本方法只输出 Mind 新增的三部分。已有的 Emotion/Persona/Relationship
    /// 仍由 PsychologyManager 自己渲染，Mind 不重复输出。
    ///
    /// 返回 None 时表示三部分全空，不污染 prompt。
    pub fn serialize_for_prompt(&self, lang: &str) -> Option<String> {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let mut sections: Vec<String> = Vec::new();

        // Belief：取置信度 Top-5，按 subject 分组展示
        {
            let store = self.beliefs.read();
            let top = store.top_n_by_confidence(5);
            if !top.is_empty() {
                let header = crate::pipeline::prompt_modules::section_heading("beliefs", lang);
                let mut lines = vec![header.to_string()];
                for b in top {
                    lines.push(format!("- {}", b.statement));
                }
                sections.push(lines.join("\n"));
            }
        }

        // Goal：取活跃 Top-3
        {
            let store = self.goals.read();
            let top = store.active_top_n(3);
            if !top.is_empty() {
                let header = crate::pipeline::prompt_modules::section_heading("current_goals", lang);
                let mut lines = vec![header.to_string()];
                for g in top {
                    lines.push(format!("- {}", g.description));
                }
                sections.push(lines.join("\n"));
            }
        }

        // Attention：取 Top-3 焦点实体
        {
            let top = self.attention_top_n(3);
            if !top.is_empty() {
                let header = crate::pipeline::prompt_modules::section_heading("attention_focus", lang);
                let focus_label = match lang_norm {
                    "en" => "Focus:",
                    "ja" => "注目：",
                    _ => "关注：",
                };
                let sep = match lang_norm {
                    "en" => ", ",
                    _ => "、",
                };
                let mut lines = vec![header.to_string()];
                let entries: Vec<String> = top
                    .iter()
                    .map(|(entity, _w)| entity.clone())
                    .collect();
                lines.push(format!("{}{}", focus_label, entries.join(sep)));
                sections.push(lines.join("\n"));
            }
        }

        // Current Activity：当前活动状态（跨分钟级，类似 Neuro-sama 的 Continuous State）
        if let Some(activity_section) = self.current_activity.serialize_for_prompt(
            chrono::Local::now().timestamp(),
            lang,
        ) {
            sections.push(activity_section);
        }

        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n\n"))
        }
    }
}
