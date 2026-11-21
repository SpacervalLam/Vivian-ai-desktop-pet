//! 目标服务（ctx.goals）—— 包住角色的 `GoalStore` 并提供类型化事件出口
//!
//! 目标是"同一会话内持续演化的状态"，
//! 增改目标都通过服务完成，并广播 `goal/*` 类型事件，供关注者（Attention 分配、
//! 主动行为、跨角色分享、前端心智面板）订阅与响应。
//!
//! 底层的 `GoalStore`（`SharedGoalStore`）仍由 `Mind` 持有并持久化；
//! 本服务只做**薄包装 + 事件发射**，不复制存储，避免双写。

use std::sync::Arc;

use parking_lot::RwLock;

use crate::cordis::{RuntimeContext, global_ctx};

use super::goal::{Goal, GoalOrigin, GoalStore, SharedGoalStore};

/// 目标事件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalEventKind {
    /// 新增了一个活跃目标
    Added,
    /// 目标被标记为完成
    Completed,
    /// 目标被放弃/取消
    Deactivated,
    /// 优先级等高内字段被修改
    Updated,
}

/// 目标事件载荷（在 `ctx` 事件总线上广播，`GoalEventKind` 作为事件名携带）。
#[derive(Debug, Clone)]
pub struct GoalEvent {
    pub char_id: String,
    pub kind: GoalEventKind,
    pub goal: Goal,
}

/// 目标服务：包住某个角色的 `SharedGoalStore`，提供带事件广播的目标增改。
pub struct GoalService {
    char_id: String,
    store: SharedGoalStore,
    /// 运行时上下文（优先用注入的，回退到进程级全局 ctx）
    ctx: Arc<RuntimeContext>,
}

impl GoalService {
    pub fn new(char_id: impl Into<String>, store: SharedGoalStore) -> Arc<Self> {
        let ctx = global_ctx().map(Arc::new).unwrap_or_else(|| Arc::new(RuntimeContext::new()));
        Arc::new(Self {
            char_id: char_id.into(),
            store,
            ctx,
        })
    }

    /// 追加一个新目标，并广播 `Added` 事件。返回新目标。
    ///
    /// 自动生成去重 id，并做活跃数量上限保护（超限的最旧目标会被平替为 inactive，
    /// 与 `GoalStore` 容量约定一致，避免无限膨胀）。
    pub fn add(
        &self,
        description: impl Into<String>,
        origin: GoalOrigin,
        priority: f64,
    ) -> Goal {
        let now = chrono::Local::now().timestamp();
        let goal = Goal::new(
            format!("g-{}", uuid::Uuid::new_v4().simple()),
            description,
            origin,
            priority.clamp(0.0, 1.0),
            now,
        );
        {
            let mut store = self.store.write();
            store.add(goal.clone());
        }
        self.emit(GoalEventKind::Added, &goal);
        goal
    }

    /// 标记目标完成（active=false），广播 `Completed`。
    pub fn complete(&self, id: &str) -> bool {
        self.deactivate_internal(id, GoalEventKind::Completed)
    }

    /// 标记目标放弃（active=false），广播 `Deactivated`。
    pub fn deactivate(&self, id: &str) -> bool {
        self.deactivate_internal(id, GoalEventKind::Deactivated)
    }

    fn deactivate_internal(&self, id: &str, kind: GoalEventKind) -> bool {
        let goal = {
            let mut store = self.store.write();
            let goal = store.goals.iter().find(|g| g.id == id && g.active).cloned();
            if goal.is_some() {
                store.deactivate(id);
            }
            goal
        };
        if let Some(goal) = goal {
            self.emit(kind, &goal);
            true
        } else {
            false
        }
    }

    /// 以 CAS 围栏更新目标：`expected_revision` 与当前 revision 不一致则拒绝（返回 None）。
    ///
    /// `mutate` 接收目标可变引用，修改后 revision 自增。用于优先级/描述/时限等
    /// 高层内字段的并发安全修改，避免两个写入方互相覆盖。
    pub fn update_cas<F>(&self, id: &str, expected_revision: u64, mutate: F) -> Option<Goal>
    where
        F: FnOnce(&mut Goal),
    {
        let updated = {
            let mut store = self.store.write();
            let goal = store.goals.iter_mut().find(|g| g.id == id && g.active)?;
            if goal.revision != expected_revision {
                return None;
            }
            mutate(goal);
            goal.revision = goal.revision.wrapping_add(1);
            goal.clone()
        };
        self.emit(GoalEventKind::Updated, &updated);
        Some(updated)
    }

    /// 读取单个目标的当前 revision（用于发起 CAS 更新前的快照）。
    pub fn revision_of(&self, id: &str) -> Option<u64> {
        self.store
            .read()
            .goals
            .iter()
            .find(|g| g.id == id && g.active)
            .map(|g| g.revision)
    }

    /// 当前活跃目标（按优先级降序，Top-N）。
    pub fn active_top_n(&self, n: usize) -> Vec<Goal> {
        self.store
            .read()
            .active_top_n(n)
            .into_iter()
            .cloned()
            .collect()
    }

    /// 活跃目标描述列表（prompt 注入 / 展示用）。
    pub fn active_descriptions(&self) -> Vec<String> {
        self.store
            .read()
            .active_sorted()
            .into_iter()
            .map(|g| {
                if g.deadline.is_some() {
                    format!("{}（有时限）", g.description)
                } else {
                    g.description.clone()
                }
            })
            .collect()
    }

    fn emit(&self, kind: GoalEventKind, goal: &Goal) {
        let event = GoalEvent {
            char_id: self.char_id.clone(),
            kind,
            goal: goal.clone(),
        };
        let ctx = Arc::clone(&self.ctx);
        tauri::async_runtime::spawn(async move {
            let _ = ctx.emit_serial(event).await;
        });
    }

    pub fn char_id(&self) -> &str {
        &self.char_id
    }
}

impl GoalService {
    /// 供内部清理/测试：把底层 store 暴露为只读快照。
    pub fn snapshot(&self) -> Arc<RwLock<GoalStore>> {
        Arc::clone(&self.store)
    }
}