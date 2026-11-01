//! 「这件事在等谁」的派生状态。
//!
//! `CodingStatus` 回答的是"agent 在不在跑"，三态。用户打开工作页真正要问的却是
//! "现在需不需要我"——那要把待办、后台任务、未答提问、最后一次结算放在一起看。
//! 这套判断原先散在暂停输入队列、转告标题、会话列表角标几处，各自维护各自的版本；
//! 收敛成一个纯函数后，它们只可能一起对，或者被同一条测试一起抓住。
//!
//! 规则按表从上到下取**第一条命中**的，顺序本身就是语义：一个卡在提问上的会话
//! 即使正在跑，也仍然在等人回答。
//!
//! | 顺序 | 条件 | 结果 | 面板含义 |
//! |---|---|---|---|
//! | 1 | 有未答提问 | `Person(Answer)` | 等你回答 |
//! | 2 | 正在跑 | `Working { queued: false }` | 执行中 |
//! | 3 | 上次结算 `blocked` | `Person(Blocked)` | 卡住了 |
//! | 4 | 有待办或未收集后台任务 | `Working { queued: true }` | 还有活没干完 |
//! | 5 | 其余 | `Idle` | 等新指令 |
//!
//! 刻意不引入"等你验收"这一档：工作侧是对话式会话，结果直接落在对话里，
//! 没有需要单独签收的交付物。硬加一档只会让每个完成的会话都挂上"需要你"。

use super::work_contract::TaskStatus;

/// 判定所需的最小输入。
///
/// 用标量而不是 `&CodingSession`：这个判断要被逐行验证，不该把整个会话结构
/// 拖进测试桩。填充由调用方负责（见 `coding_agent::work_signals`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkSignals {
    /// 当前是否有一轮在跑。
    pub running: bool,
    /// 未完成的主待办条数。
    pub open_todos: usize,
    /// 尚未收集结果的后台任务数。
    pub pending_jobs: usize,
    /// 是否有一个抛出去还没被回答的提问。
    pub question_pending: bool,
    /// 最近一次结构化结算的状态；从未结算过为 `None`。
    pub last_settlement: Option<TaskStatus>,
}

impl WorkSignals {
    /// 还有没有活没干完（不含"正在跑"与"在等人"两种情形）。
    pub fn has_open_work(&self) -> bool {
        self.open_todos > 0 || self.pending_jobs > 0
    }
}

/// 需要人做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonNeed {
    /// 有一个待答的提问。
    Answer,
    /// 上次结算说卡住了，要人推动才能继续。
    Blocked,
}

/// 会话此刻在等谁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkStage {
    /// 没有人的输入就走不下去。
    Person(PersonNeed),
    /// 还有活；`queued` 表示没在跑（排队或停在两个回合之间）。
    Working { queued: bool },
    /// 手上没有活，等新指令。
    Idle,
}

impl WorkStage {
    /// 是否需要人来推动。会话列表的排序与角标只看这一个判断。
    pub fn needs_person(self) -> bool {
        matches!(self, Self::Person(_))
    }

    /// 是否还有活在做或待做。
    pub fn is_working(self) -> bool {
        matches!(self, Self::Working { .. })
    }

    /// 会话列表头用的稳定标识。界面文案由前端按语言渲染，这里只给判据。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person(PersonNeed::Answer) => "answer",
            Self::Person(PersonNeed::Blocked) => "blocked",
            Self::Working { queued: false } => "working",
            Self::Working { queued: true } => "queued",
            Self::Idle => "idle",
        }
    }
}

/// 按模块文档里的表派生会话状态。
pub fn work_stage(signals: WorkSignals) -> WorkStage {
    if signals.question_pending {
        return WorkStage::Person(PersonNeed::Answer);
    }
    if signals.running {
        return WorkStage::Working { queued: false };
    }
    if signals.last_settlement == Some(TaskStatus::Blocked) {
        return WorkStage::Person(PersonNeed::Blocked);
    }
    if signals.has_open_work() {
        return WorkStage::Working { queued: true };
    }
    WorkStage::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals() -> WorkSignals {
        WorkSignals { running: false, open_todos: 0, pending_jobs: 0, question_pending: false, last_settlement: None }
    }

    #[test]
    fn a_pending_question_outranks_a_running_turn() {
        // 提问可以在还在跑的时候抛出：那时最需要人的仍然是回答，不是等它跑完。
        let stage = work_stage(WorkSignals { running: true, question_pending: true, ..signals() });
        assert_eq!(stage, WorkStage::Person(PersonNeed::Answer));
        assert!(stage.needs_person());
        assert!(!stage.is_working());
    }

    #[test]
    fn a_running_turn_is_working_not_queued() {
        let stage = work_stage(WorkSignals { running: true, open_todos: 3, ..signals() });
        assert_eq!(stage, WorkStage::Working { queued: false });
    }

    #[test]
    fn a_blocked_settlement_asks_for_a_person_even_with_open_work() {
        // 有待办却报 blocked，说明那些待办本身要人拍板，不能当成"还有活"往下推。
        let stage = work_stage(WorkSignals { open_todos: 2, last_settlement: Some(TaskStatus::Blocked), ..signals() });
        assert_eq!(stage, WorkStage::Person(PersonNeed::Blocked));
    }

    #[test]
    fn open_work_without_a_running_turn_is_queued() {
        assert_eq!(work_stage(WorkSignals { open_todos: 1, ..signals() }), WorkStage::Working { queued: true });
        assert_eq!(work_stage(WorkSignals { pending_jobs: 1, ..signals() }), WorkStage::Working { queued: true });
    }

    #[test]
    fn settled_work_with_nothing_open_is_idle() {
        for status in [TaskStatus::Completed, TaskStatus::Dropped, TaskStatus::Partial, TaskStatus::Cancelled] {
            assert_eq!(work_stage(WorkSignals { last_settlement: Some(status), ..signals() }), WorkStage::Idle,
                "{status:?} with no open work belongs to nobody");
        }
        // 从未结算过的空会话同理。
        assert_eq!(work_stage(signals()), WorkStage::Idle);
    }

    #[test]
    fn a_new_session_is_idle_and_asks_for_nobody() {
        let stage = work_stage(signals());
        assert!(!stage.needs_person());
        assert!(!stage.is_working());
    }
}
