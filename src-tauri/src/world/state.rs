//! World State —— 世界状态核心。
//!
//! 提供用户实体状态追踪（在场/离开/预期回归）与用户行为日志的统一 API。
//!
//! 状态机与行为日志的协作：
//! - 实时状态（State）由 UserEntityState 维护（current_activity）
//! - 状态切换时自动封存（seal）带时长的行为事件到 UserBehaviorLog（Event 层）
//! - 行为日志持久化到磁盘，供认知引擎在 Rest 时整理为用户习惯 Belief

use std::sync::Arc;

use parking_lot::RwLock;

use crate::world::user_behavior::{
    BehaviorEndReason, SharedUserBehaviorLog, UserBehaviorEntry, UserBehaviorLog,
};

/// 行为封存回调：当一条行为事件被写入日志后触发
///
/// 用途：让上层（Brain）注入认知引擎的冲突检测逻辑，
/// 避免在 WorldState 中直接依赖 Mind 模块。
pub type BehaviorSealHook = Arc<dyn Fn(&UserBehaviorEntry) + Send + Sync>;

/// World State —— 世界状态核心
///
/// 线程安全的用户实体状态追踪 + 行为日志。
pub struct WorldState {
    /// 用户实体状态（在场/离开/预期回归/当前活动）
    user_entity: RwLock<crate::world::entity_state::UserEntityState>,
    /// 用户行为日志（已封存的持续状态事件，带时长）
    user_behaviors: SharedUserBehaviorLog,
    /// 行为封存回调（可选，由 Brain 注入认知引擎的冲突检测）
    seal_hook: RwLock<Option<BehaviorSealHook>>,
}

impl WorldState {
    pub fn new() -> Self {
        Self {
            user_entity: RwLock::new(crate::world::entity_state::UserEntityState::new()),
            user_behaviors: Arc::new(RwLock::new(UserBehaviorLog::new())),
            seal_hook: RwLock::new(None),
        }
    }

    /// 构造并加载持久化的行为日志
    pub fn with_behavior_log(path: std::path::PathBuf) -> Self {
        let log = UserBehaviorLog::load(&path);
        Self {
            user_entity: RwLock::new(crate::world::entity_state::UserEntityState::new()),
            user_behaviors: Arc::new(RwLock::new(log)),
            seal_hook: RwLock::new(None),
        }
    }

    /// 行为日志句柄（供认知引擎读取）
    pub fn behavior_log(&self) -> SharedUserBehaviorLog {
        self.user_behaviors.clone()
    }

    /// 注入行为封存回调（由 Brain 在初始化时调用）
    ///
    /// 回调在每次 `seal_activity` 成功写入日志后同步执行，
    /// 用于触发认知引擎的冲突检测（不修改 WorldState 自身状态）。
    pub fn set_seal_hook(&self, hook: BehaviorSealHook) {
        *self.seal_hook.write() = Some(hook);
    }

    // ── 用户实体状态委托方法 ──

    /// 摄入对话文本，抽取预期回归时间（由 chat 命令层调用）
    pub fn ingest_dialogue(&self, text: &str) {
        self.user_entity.read().ingest_dialogue(text);
    }

    /// 标记用户在场（由 proactive tick 调用，idle_seconds < 60 时触发）
    ///
    /// 返回回归事件：若用户之前离开且携带预期，返回 ReturnEvent。
    /// 回归前自动封存进行中的持续活动到行为日志（source 标记为 `"return_detected"`）。
    pub fn mark_user_present(&self) -> Option<crate::world::entity_state::ReturnEvent> {
        // 先封存进行中的活动（用户回来意味着上一个持续状态已结束）
        let old_activity = self.user_entity.read().take_current_activity();
        if let Some(activity) = old_activity {
            self.seal_activity(activity, BehaviorEndReason::UserReturn, "return_detected");
        }
        self.user_entity.read().mark_present()
    }

    /// 标记用户离开（由 proactive tick 调用，idle_seconds > away_threshold 时触发）
    pub fn mark_user_away(&self) {
        self.user_entity.read().mark_away(None);
    }

    /// 生成用户实体状态快照（供 prompt 注入与决策查询）
    pub fn user_entity_snapshot(&self) -> crate::world::entity_state::UserEntitySnapshot {
        self.user_entity.read().snapshot()
    }

    /// 更新用户当前持续活动（由反思阶段 LLM 输出 world_update 时调用）
    ///
    /// 若与当前活动同名，仅刷新置信度（视为延续，不封存）。
    /// 若不同名，封存旧活动到行为日志，再设置新活动。
    /// label 为空串时清除活动（封存旧活动）。
    /// 封存时 source 标记为 `"llm_observation"`。
    pub fn update_user_activity(&self, label: &str, confidence: f64) {
        self.update_user_activity_inner(label, confidence, "llm_observation");
    }

    /// 由本地窗口分类器直接驱动，无需等待 LLM 反思。
    ///
    /// 与 `update_user_activity` 行为一致，仅封存时 source 标记为 `"local_classifier"`。
    pub fn update_user_activity_from_classifier(&self, label: &str, confidence: f64) {
        self.update_user_activity_inner(label, confidence, "local_classifier");
    }

    /// 内部实现：更新用户活动，指定来源标签
    fn update_user_activity_inner(&self, label: &str, confidence: f64, source: &str) {
        let label = label.trim();
        if label.is_empty() {
            self.clear_user_activity();
            return;
        }
        // 同名活动：刷新置信度，不封存（视为延续）
        let snapshot = self.user_entity_snapshot();
        if let Some(ref activity) = snapshot.current_activity {
            if activity.label == label {
                self.user_entity.read().refresh_activity_confidence(confidence);
                return;
            }
        }
        // 不同名：封存旧活动，设置新活动
        let old = self
            .user_entity
            .read()
            .swap_user_activity(label, confidence);
        if let Some(activity) = old {
            self.seal_activity(activity, BehaviorEndReason::StateChange, source);
        }
    }

    /// 清除用户当前持续活动（封存到行为日志后清除）
    pub fn clear_user_activity(&self) {
        let old = self.user_entity.read().take_current_activity();
        if let Some(activity) = old {
            self.seal_activity(activity, BehaviorEndReason::SystemClear, "system_clear");
        }
    }

    /// 封存一条持续活动到行为日志
    ///
    /// `source` 标记来源：`"llm_observation"`（LLM 反思）/ `"local_classifier"`（本地窗口分类器）/
    /// `"return_detected"`（用户回归时推断）/ `"system_clear"`（系统清除）。
    fn seal_activity(&self, activity: crate::world::entity_state::UserActivity, reason: BehaviorEndReason, source: &str) {
        let now = chrono::Local::now().timestamp() as f64;
        let duration_secs = (now - activity.started_at).max(0.0);
        // 过滤掉过短的活动（< 60 秒，视为瞬时动作不值得记录）
        if duration_secs < 60.0 {
            tracing::debug!(
                "[WorldState] 活动「{}」仅持续 {:.0}s，不封存",
                activity.label,
                duration_secs
            );
            return;
        }
        let entry = UserBehaviorEntry {
            id: format!(
                "behavior_{}_{}",
                activity.started_at as i64,
                activity.label.chars().take(6).collect::<String>()
            ),
            activity_label: activity.label.clone(),
            started_at: activity.started_at,
            ended_at: now,
            duration_secs,
            source: source.to_string(),
            ended_by: reason,
            confidence: activity.confidence,
        };
        tracing::info!(
            "[WorldState] 封存用户行为：{} 持续 {:.1}h（结束原因：{:?}）",
            entry.activity_label,
            entry.duration_hours(),
            entry.ended_by
        );
        self.user_behaviors.write().seal(entry.clone());
        // 触发封存回调（认知引擎冲突检测）
        if let Some(hook) = self.seal_hook.read().as_ref() {
            hook(&entry);
        }
    }
}

impl Default for WorldState {
    fn default() -> Self {
        Self::new()
    }
}
