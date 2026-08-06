//! Cognitive Tick —— 统一认知循环流水线。
//!
//! 把原来的双 tick（30s `mind_tick` + 10s `proactive.tick`）重构为显式 6 阶段流水线，
//! 让"说话"从系统驱动力变成认知循环的可能输出之一。
//!
//! ## 设计哲学
//!
//! 参考 Neuro-sama 的"持续自主活动（Continuous Autonomous Activity）"模式：
//! 不是"收到事件 → 思考 → 等待"，而是"固定节拍 → 更新世界 → 更新自我 →
//! 决策观察 → 决策思考 → 决策行动 → 决策说话"。
//!
//! ## 六阶段
//!
//! 1. **World ingest**：摄入世界状态（每次执行，无 LLM）
//!    - WorldState 超时检测
//!    - 注：窗口轮询/世界事件检测由 `proactive.tick` 在 Speak 阶段完成
//! 2. **Self update**：更新自我认知（规则层，无 LLM）
//!    - PsychologyManager homeostasis_tick（每次，10s 级 Needs/Emotion 回归）
//!    - Mind mind_tick（30s 节流：Attention Drift + Goal Update + Belief Consolidation + Working Memory Decay）
//! 3. **Observe decision**：决策是否主动观察（规则层）
//!    - 当前默认每次观察，为未来"选择性观察"留接口
//! 4. **Think decision**：决策是否进行内部 LLM 思考（规则层）
//!    - social_urge 高但被防打扰阻止 → 内心独白
//!    - 用户长时间不在 → 内心独白
//!    - 长时间未发言 + 非安静模式 → 内心独白
//!    - 注：实际 LLM 调用由 `proactive.tick` 内 `maybe_spawn_inner_monologue` 完成
//! 5. **Act decision**：决策是否执行工具/操作（规则层，当前占位）
//!    - 未来可接入：主动检索记忆 / 主动截图 / 主动调用工具
//! 6. **Speak decision**：决策是否产生用户可见消息（规则 + LLM）
//!    - 调用 `proactive.tick` 完成实际的触发器检查、内容生成、消息推送
//!
//! ## 节流策略
//!
//! - WorldIngest / Observe / Speak：每次 tick（10s）执行
//! - SelfUpdate：homeostasis 每次执行，mind_tick 30s 节流
//! - Think：5 分钟节流（避免频繁 LLM 独白）
//! - Act：当前占位，每次返回 skip
//!
//! ## LLM 调用控制
//!
//! 仅以下情况调用 LLM：
//! - Think 阶段决策通过 + `proactive.tick` 内 inner monologue 冷却到期 → 异步 LLM 独白
//! - Speak 阶段触发器命中 + `proactive.tick` 内 LLM 生成 → 同步 LLM 主动消息
//! 其余阶段纯规则，零 LLM 调用。

use std::sync::Arc;

use parking_lot::Mutex;

use crate::error::VivianResult;
use crate::proactive::TickContext;
use crate::tools::types::ToolUseContext;

use super::action_planner::{ActionExecutor, ActionPlanner};
use super::Brain;

/// Cognitive Tick 的六个阶段（按执行顺序）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitiveTickPhase {
    /// 1. 摄入世界状态：WorldState 超时检测
    WorldIngest,
    /// 2. 更新自我：homeostasis + mind_tick
    SelfUpdate,
    /// 3. 决策是否主动观察 —— 规则层
    Observe,
    /// 4. 决策是否进行内部思考 —— 规则层
    Think,
    /// 5. 决策是否执行行动 —— 规则层（当前占位）
    Act,
    /// 6. 决策是否说话 —— 规则 + LLM
    Speak,
}

impl CognitiveTickPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WorldIngest => "world_ingest",
            Self::SelfUpdate => "self_update",
            Self::Observe => "observe",
            Self::Think => "think",
            Self::Act => "act",
            Self::Speak => "speak",
        }
    }
}

/// 单阶段决策结果
#[derive(Debug, Clone, Default)]
pub struct PhaseDecision {
    /// 是否执行该阶段
    pub executed: bool,
    /// 跳过原因（executed=false 时填）
    pub skip_reason: Option<String>,
    /// 该阶段产出（如生成了消息/独白/工具调用）
    pub produced: bool,
}

impl PhaseDecision {
    fn executed() -> Self {
        Self {
            executed: true,
            skip_reason: None,
            produced: false,
        }
    }

    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            executed: false,
            skip_reason: Some(reason.into()),
            produced: false,
        }
    }

    fn executed_with_produced() -> Self {
        Self {
            executed: true,
            skip_reason: None,
            produced: true,
        }
    }
}

/// 一次 Cognitive Tick 的完整结果
#[derive(Debug, Clone, Default)]
pub struct CognitiveTickResult {
    /// World ingest 阶段
    pub world_ingest: PhaseDecision,
    /// Self update 阶段
    pub self_update: PhaseDecision,
    /// Observe 阶段
    pub observe: PhaseDecision,
    /// Think 阶段
    pub think: PhaseDecision,
    /// Act 阶段
    pub act: PhaseDecision,
    /// Speak 阶段
    pub speak: PhaseDecision,
    /// 本 tick 是否产生了用户可见消息
    pub produced_user_message: bool,
}

impl CognitiveTickResult {
    /// 本 tick 是否产生了任何输出（消息/独白/工具调用）
    pub fn any_produced(&self) -> bool {
        self.world_ingest.produced
            || self.self_update.produced
            || self.observe.produced
            || self.think.produced
            || self.act.produced
            || self.speak.produced
    }

    /// 调试用：把每阶段决策渲染为紧凑文本
    pub fn render_summary(&self) -> String {
        let fmt = |phase: &str, d: &PhaseDecision| {
            if d.executed {
                if d.produced {
                    format!("{}=exec+produced", phase)
                } else {
                    format!("{}=exec", phase)
                }
            } else {
                format!(
                    "{}=skip({})",
                    phase,
                    d.skip_reason.as_deref().unwrap_or("?")
                )
            }
        };
        vec![
            fmt("world", &self.world_ingest),
            fmt("self", &self.self_update),
            fmt("obs", &self.observe),
            fmt("think", &self.think),
            fmt("act", &self.act),
            fmt("speak", &self.speak),
        ]
        .join(" | ")
    }
}

/// Cognitive Tick 调度器
///
/// 把 Brain 的 30s mind_tick 与 10s proactive_tick 合并为统一的 6 阶段认知循环。
/// 每阶段独立决策是否执行，仅 Speak 阶段调用 LLM（Think 阶段的 LLM 调用
/// 由 proactive.tick 内的 inner monologue 机制完成）。
pub struct CognitiveTickRunner {
    /// 上次完整 mind_tick 时间戳（30s 节流）
    last_self_update_at: Arc<Mutex<f64>>,
    /// 上次 Think 决策通过时间戳（5 分钟节流）
    last_think_at: Arc<Mutex<f64>>,
    /// 上次 Observe 决策通过时间戳
    last_observe_at: Arc<Mutex<f64>>,
    /// 上次 LLM 合成 current_thought 时间戳（60s 节流）
    last_thought_at: Arc<Mutex<f64>>,
    /// 上次 Act 决策通过时间戳（60s 节流）
    last_act_at: Arc<Mutex<f64>>,
    /// 上次 pending conflict 仲裁时间戳（5 分钟节流，fire-and-forget）
    last_conflict_arbitration_at: Arc<Mutex<f64>>,
}

impl Default for CognitiveTickRunner {
    fn default() -> Self {
        Self {
            last_self_update_at: Arc::new(Mutex::new(0.0)),
            last_think_at: Arc::new(Mutex::new(0.0)),
            last_observe_at: Arc::new(Mutex::new(0.0)),
            last_thought_at: Arc::new(Mutex::new(0.0)),
            last_act_at: Arc::new(Mutex::new(0.0)),
            last_conflict_arbitration_at: Arc::new(Mutex::new(0.0)),
        }
    }
}

impl CognitiveTickRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// 执行一次完整的 Cognitive Tick
    ///
    /// 替换原 Brain::proactive_tick 内的双 tick 逻辑，统一为 6 阶段流水线。
    /// 返回各阶段决策结果，调用方据此更新 Focus cooldown 等下游逻辑。
    pub fn run(
        &self,
        brain: &Brain,
        context: &TickContext,
    ) -> VivianResult<CognitiveTickResult> {
        let now = context.now;
        let mut result = CognitiveTickResult::default();

        // ── 阶段 1: World ingest ──
        // 每次 tick 都执行：WorldState 超时检测
        // 注：窗口轮询/世界事件检测由阶段 6 的 proactive.tick 完成
        result.world_ingest = self.phase_world_ingest(brain);

        // ── 阶段 2: Self update ──
        // homeostasis 每次执行；mind_tick 30s 节流
        result.self_update = self.phase_self_update(brain, now);

        // ── 阶段 3: Observe decision ──
        // 规则决策：是否主动观察。当前默认每次观察。
        result.observe = self.phase_observe(brain, context, now);

        // ── 阶段 4: Think decision ──
        // 规则决策：是否需要内部 LLM 思考
        result.think = self.phase_think(brain, context, now);

        // ── 阶段 5: Act decision ──
        // 规则决策：是否执行工具/操作。当前为占位实现。
        result.act = self.phase_act(brain, context, now);

        // ── 阶段 6: Speak decision ──
        // 规则 + LLM：是否产生用户可见消息
        // proactive.tick 内部同时处理：
        // - homeostasis（重复执行，幂等，成本可忽略）
        // - poll_window / update_sustained_activity / behavior_mode
        // - detect_and_apply_world_events（世界事件检测）
        // - update_mind_state
        // - try_special_date_greeting / 触发器检查 / 内容生成（Speak）
        // - maybe_spawn_inner_monologue（Think 阶段实际 LLM 调用）
        result.speak = self.phase_speak(brain, context)?;
        result.produced_user_message = result.speak.produced;

        Ok(result)
    }

    // ── 阶段 1: World ingest ──
    //
    // 摄入世界状态：WorldState 超时检测。
    // 窗口轮询和世界事件检测留给阶段 6 的 proactive.tick 完成（它需要 TickContext
    // 的完整字段，且内部按顺序依赖这些数据）。
    fn phase_world_ingest(&self, _brain: &Brain) -> PhaseDecision {
        PhaseDecision::executed()
    }

    // ── 阶段 2: Self update ──
    //
    // 更新自我认知：
    // - homeostasis_tick：每次执行，让 Needs/Emotion 向 set point 回归（10s 级）
    // - mind_tick：30s 节流，Attention Drift + Goal Update + Belief Consolidation + Working Memory Decay + CurrentActivity 过期
    // - current_activity.update_from_snapshot：每次执行，根据世界/自我状态自动切换活动
    //
    // 注：proactive.tick 内部也会调一次 homeostasis_tick（幂等，成本可忽略）。
    fn phase_self_update(&self, brain: &Brain, now: f64) -> PhaseDecision {
        // Homeostasis tick（每次执行）
        brain.psychology.homeostasis_tick();

        // Current Activity 状态机更新（每次执行，规则层）
        // 根据当前 presence/behavior_mode/对话状态自动切换活动类型，
        // 让"持续状态"随世界变化而更新。
        {
            let snapshot = brain.self_state.snapshot();
            let now_i64 = now as i64;
            let presence_busy = matches!(
                snapshot.presence,
                crate::presence::PresenceState::Busy
            );
            let presence_rest = matches!(
                snapshot.presence,
                crate::presence::PresenceState::Rest
            );
            // behavior_mode 从 proactive status 读取（snapshot 不直接暴露）
            let behavior_mode = brain
                .proactive
                .get_status()
                .get("behavior_mode")
                .and_then(|v| v.as_str())
                .unwrap_or("none")
                .to_string();

            brain.mind.current_activity.update_from_snapshot(
                presence_busy,
                presence_rest,
                &behavior_mode,
                snapshot.last_spoken_secs_ago,
                0.0, // user_idle_secs 由 phase_observe 阶段更精确处理
                now_i64,
            );
        }

        // mind_tick（30s 节流）
        let mut last = self.last_self_update_at.lock();
        let dt = now - *last;
        let decision = if dt >= 30.0 {
            brain.mind.mind_tick(dt, now as i64);
            *last = now;
            PhaseDecision::executed()
        } else {
            PhaseDecision::skipped(format!(
                "mind_tick throttled (dt={:.0}s < 30s)",
                dt
            ))
        };
        drop(last);

        // 记忆冲突 LLM 仲裁（5 分钟节流，fire-and-forget）
        // 消费 QueueLlm 推入的 pending_conflicts 队列，调 LLM 仲裁矛盾记忆
        {
            let mut last_arb = self.last_conflict_arbitration_at.lock();
            let arb_dt = now - *last_arb;
            if arb_dt >= 300.0 && brain.memory.pending_conflict_count() > 0 {
                *last_arb = now;
                drop(last_arb);
                let memory = Arc::clone(&brain.memory);
                let router = Arc::clone(&brain.router);
                tauri::async_runtime::spawn(async move {
                    let arbiter: std::sync::Arc<dyn crate::memory::conflict::ConflictLlmArbiter> =
                        std::sync::Arc::new(crate::memory::conflict::DefaultConflictArbiter::new(router));
                    let processed = memory.process_pending_conflicts(&arbiter).await;
                    if processed > 0 {
                        tracing::info!(
                            "[cognitive_tick] 记忆冲突仲裁：本次处理 {} 条 pending conflicts",
                            processed
                        );
                    }
                });
            }
        }

        // ── current_thought 合成（60s 节流 + 事件驱动，混合策略）──
        // fire-and-forget：不阻塞认知循环，LLM 请求在后台完成
        {
            let refresh_requested = brain.mind.consume_thought_refresh();
            let mut last_thought = self.last_thought_at.lock();
            let thought_dt = now - *last_thought;
            if refresh_requested || thought_dt >= 60.0 {
                *last_thought = now;
                drop(last_thought);
                let mind = Arc::clone(&brain.mind);
                let router = Arc::clone(&brain.router);
                let world_provider = Arc::clone(&brain.world_provider);
                let lang = lang_code_from_config(&brain.config);
                tauri::async_runtime::spawn(async move {
                    crate::mind::thought_synthesis::refresh_current_thought(
                        &mind, &router, &world_provider, &lang,
                    )
                    .await;
                });
            }
        }

        decision
    }

    // ── 阶段 3: Observe decision ──
    //
    // 规则决策：是否主动观察世界。
    // 当前策略：每次 tick 都"观察"——world_ingest 阶段已做基础世界状态摄入，
    // proactive.tick 阶段会做窗口轮询和世界事件检测。
    //
    // 未来扩展点：
    // - 用户长时间不在 → 降低观察频率（节省 CPU）
    // - 桌面有变化 → 提高观察频率
    // - 跨角色事件到达 → 立即观察
    fn phase_observe(&self, _brain: &Brain, context: &TickContext, now: f64) -> PhaseDecision {
        *self.last_observe_at.lock() = now;

        // 未来扩展：长时间无活动 + 无跨角色事件 → skip 观察
        // 当前：总是观察
        if context.idle_seconds > 1800.0 {
            // 用户超过 30 分钟没活动：观察频率降级（标记但不实际 skip，因为 proactive.tick 仍需运行）
            tracing::trace!(
                "[cognitive_tick] observe: user idle {:.0}min, observation degraded",
                context.idle_seconds / 60.0
            );
        }

        PhaseDecision::executed()
    }

    // ── 阶段 4: Think decision ──
    //
    // 规则决策：是否需要进行内部 LLM 思考（inner monologue）。
    //
    // 触发条件（任一满足即决策通过）：
    // 1. social_urge 高但被 lay_low 阻止说话 → 内心独白替代
    // 2. 用户长时间不在 → 让内心生活继续
    // 3. 长时间未发言 + 非安静模式 → 自我对话
    //
    // **人格权重调整**：think_propensity 高的角色（如 Nana）更容易触发思考，
    // 低的角色（如 Vivian）需要更强的信号才会思考。
    // 具体：基础 social_urge 阈值 0.6，按 think_threshold() 调整。
    //
    // 节流：5 分钟一次（避免频繁 LLM 独白）
    //
    // 注：决策通过后，实际 LLM 调用由 proactive.tick 内的
    // maybe_spawn_inner_monologue 完成（它有自己的冷却检查）。
    fn phase_think(&self, brain: &Brain, context: &TickContext, now: f64) -> PhaseDecision {
        // 节流检查
        const THINK_MIN_INTERVAL: f64 = 300.0; // 5 分钟
        {
            let last = self.last_think_at.lock();
            let dt = now - *last;
            if dt < THINK_MIN_INTERVAL {
                return PhaseDecision::skipped(format!(
                    "think throttled (dt={:.0}s < {:.0}s)",
                    dt, THINK_MIN_INTERVAL
                ));
            }
        }

        // 读取人格决策权重：think_propensity 高 → 阈值降低，更易触发思考
        let persona_weights = brain.persona.decision_weights();
        let urge_threshold = persona_weights.think_threshold();

        // 规则决策
        let snapshot = brain.self_state.snapshot();
        let should_think = {
            // 条件 1：social_urge 高但被防打扰阻止（阈值受人格影响）
            let urge_blocked = snapshot.social_urge > urge_threshold && context.lay_low;
            // 条件 2：用户长时间不在（固定阈值，不受人格影响）
            let user_away = context.idle_seconds > 300.0;
            // 条件 3：长时间未发言 + 非安静模式（固定阈值）
            let long_silent = snapshot
                .last_spoken_secs_ago
                .map(|s| s > 600.0)
                .unwrap_or(false)
                && !snapshot.quiet_mode;

            urge_blocked || user_away || long_silent
        };

        if !should_think {
            return PhaseDecision::skipped(format!(
                "think condition not met (urge_thr={:.2}, urge={:.2})",
                urge_threshold,
                snapshot.social_urge
            ));
        }

        // 决策通过：记录时间戳，实际 LLM 调用由阶段 6 的 proactive.tick 完成
        *self.last_think_at.lock() = now;
        PhaseDecision::executed()
    }

    // ── 阶段 5: Act decision ──
    //
    // 规则决策：是否执行工具/操作。
    //
    // 由 ActionPlanner 从 Goal/BehaviorDrive/WorldState 推导候选动作序列，
    // ActionExecutor 按 ActionActivationType 分流执行：
    //   ALWAYS / RANDOM / KEYWORD → 立即执行
    //   LLM_JUDGE → 并行 LLM 判定（带 30s 缓存）后执行
    //   NEVER → 跳过
    //
    // 节流：60s 一次，避免频繁主动调用工具
    fn phase_act(
        &self,
        brain: &Brain,
        context: &TickContext,
        now: f64,
    ) -> PhaseDecision {
        const ACT_MIN_INTERVAL: f64 = 60.0;
        {
            let last = self.last_act_at.lock();
            let dt = now - *last;
            if dt < ACT_MIN_INTERVAL {
                return PhaseDecision::skipped(format!(
                    "act throttled (dt={:.0}s < {:.0}s)",
                    dt, ACT_MIN_INTERVAL
                ));
            }
        }

        // 防打扰窗口下不主动行动
        if context.lay_low {
            return PhaseDecision::skipped("lay_low active");
        }

        // 收集规划输入：active goals + current behavior drive
        let goals: Vec<crate::mind::goal::Goal> = {
            let store = brain.mind.goals.read();
            store.active_top_n(3).into_iter().cloned().collect()
        };
        let drive = brain.psychology.current_drive();

        let sequence = ActionPlanner::plan(&goals, drive.as_ref(), context);
        if sequence.is_empty() {
            return PhaseDecision::skipped("no actionable plan");
        }

        *self.last_act_at.lock() = now;

        // fire-and-forget：不阻塞认知循环
        let executor = ActionExecutor::new(
            Some(Arc::clone(&brain.router)),
            Arc::clone(&brain.tool_system),
            brain.char_id.clone(),
        );
        let tool_ctx = ToolUseContext::default();
        let ctx = context.clone();
        let seq = sequence.clone();
        let char_id = brain.char_id.clone();
        tauri::async_runtime::spawn(async move {
            match executor.execute(&seq, &ctx, &tool_ctx).await {
                Ok(results) => {
                    let executed = results.iter().filter(|r| r.executed).count();
                    if executed > 0 {
                        tracing::info!(
                            "[cognitive_tick:{}] act executed {}/{} actions (plan: {})",
                            char_id,
                            executed,
                            results.len(),
                            seq.rationale
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[cognitive_tick:{}] act execution failed: {}",
                        char_id,
                        e
                    );
                }
            }
        });

        PhaseDecision::executed()
    }

    // ── 阶段 6: Speak decision ──
    //
    // 规则 + LLM：是否产生用户可见消息。
    //
    // 调用 proactive.tick 完成实际决策，它内部按顺序处理：
    // 1. 安静模式检查（lay_low 时跳过）
    // 2. BehaviorDrive 计算
    // 3. CapabilityPlanner.plan（决定是否跳过行动）
    // 4. try_special_date_greeting（最高优先级）
    // 5. ordered_active_triggers + check_trigger + generate_content + push_message
    // 6. maybe_spawn_inner_monologue（Think 阶段决策通过时的实际 LLM 调用）
    //
    // 返回 produced=true 表示产生了用户可见消息。
    fn phase_speak(&self, brain: &Brain, context: &TickContext) -> VivianResult<PhaseDecision> {
        let produced = brain.proactive.tick(context)?;
        if produced {
            Ok(PhaseDecision::executed_with_produced())
        } else {
            Ok(PhaseDecision::executed())
        }
    }
}

/// 从 AppConfig 提取语言代码，返回 "zh" / "en" / "ja"
fn lang_code_from_config(config: &crate::config::manager::AppConfig) -> String {
    let lang = &config.base.language;
    if lang.starts_with("ja") {
        "ja"
    } else if lang.starts_with("en") {
        "en"
    } else {
        "zh"
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_decision_executed_correct() {
        let d = PhaseDecision::executed();
        assert!(d.executed);
        assert!(!d.produced);
        assert!(d.skip_reason.is_none());
    }

    #[test]
    fn phase_decision_skipped_correct() {
        let d = PhaseDecision::skipped("test reason");
        assert!(!d.executed);
        assert!(!d.produced);
        assert_eq!(d.skip_reason.as_deref(), Some("test reason"));
    }

    #[test]
    fn phase_decision_executed_with_produced_correct() {
        let d = PhaseDecision::executed_with_produced();
        assert!(d.executed);
        assert!(d.produced);
        assert!(d.skip_reason.is_none());
    }

    #[test]
    fn cognitive_tick_result_any_produced_logic() {
        let mut r = CognitiveTickResult::default();
        assert!(!r.any_produced());

        r.world_ingest.produced = true;
        assert!(r.any_produced());

        r.world_ingest.produced = false;
        r.speak.produced = true;
        assert!(r.any_produced());
    }

    #[test]
    fn cognitive_tick_result_render_summary_format() {
        let mut r = CognitiveTickResult::default();
        r.world_ingest = PhaseDecision::executed();
        r.self_update = PhaseDecision::skipped("throttled");
        r.observe = PhaseDecision::executed();
        r.think = PhaseDecision::skipped("condition not met");
        r.act = PhaseDecision::skipped("not implemented");
        r.speak = PhaseDecision::executed_with_produced();

        let summary = r.render_summary();
        assert!(summary.contains("world=exec"));
        assert!(summary.contains("self=skip(throttled)"));
        assert!(summary.contains("speak=exec+produced"));
    }

    #[test]
    fn runner_default_initializes_zero_timestamps() {
        let runner = CognitiveTickRunner::new();
        assert_eq!(*runner.last_self_update_at.lock(), 0.0);
        assert_eq!(*runner.last_think_at.lock(), 0.0);
        assert_eq!(*runner.last_observe_at.lock(), 0.0);
    }

    #[test]
    fn phase_enum_as_str_correct() {
        assert_eq!(CognitiveTickPhase::WorldIngest.as_str(), "world_ingest");
        assert_eq!(CognitiveTickPhase::SelfUpdate.as_str(), "self_update");
        assert_eq!(CognitiveTickPhase::Observe.as_str(), "observe");
        assert_eq!(CognitiveTickPhase::Think.as_str(), "think");
        assert_eq!(CognitiveTickPhase::Act.as_str(), "act");
        assert_eq!(CognitiveTickPhase::Speak.as_str(), "speak");
    }
}
