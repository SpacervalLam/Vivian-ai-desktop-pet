//! work 产品线的任务生命周期：唯一状态机、结果标签与迁移规则。
//!
//! 会话只是界面，任务才是持久执行状态。全 work 侧只有这里定义生命周期状态、
//! 只有 `transition` 允许改写状态；任何模块都不许自己拼状态字符串或直接改字段。
//!
//! 生命周期与结果质量是两个正交维度：`WorkTaskStatus` 说「这件活走到哪一步」，
//! `TaskOutcome` 说「交回来的东西质量如何」。把两者塞进一个枚举，会让
//! `partial`（结果不完整）被误读成「任务还在跑」。

use serde::{Deserialize, Serialize};

/// 唯一的生命周期状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkTaskStatus {
    /// 已派发 / Worker 正在跑；值守中的两次 run 之间也保持 Active。
    /// `running` 是旧 `work_jobs.json` 里的写法，存量数据靠这个别名读回来。
    #[serde(alias = "running")]
    Active,
    /// 等用户补充信息（前端显示「待你输入」）。
    NeedsInput,
    /// Worker 已完成、结果就绪，等人工按精确版本验收。
    ReadyForReview,
    /// 已交付并结束。若经过强制验收，`accepted_revision` 有值。
    Completed,
    /// 执行失败或宿主重启中断了 Active 任务。
    Failed,
    /// 用户取消；终局，迟到的结果不得改回。
    Canceled,
}

impl WorkTaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NeedsInput => "needs_input",
            Self::ReadyForReview => "ready_for_review",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    /// Worker 是否还会产出新结果。**不等于任务结束**：
    /// `ReadyForReview` 时 Worker 已经停了，但任务仍在等验收。
    pub fn is_worker_done(self) -> bool {
        !matches!(self, Self::Active | Self::NeedsInput)
    }

    /// 任务是否彻底结束。用于取消拒绝、清理资格、主循环完成判定。
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }

    /// 前端任务轨的状态桶。`Failed` / `Canceled` 不进四个主桶，
    /// 由详情面板按终态展示，避免「已完成」混入失败任务。
    pub fn bucket(self) -> &'static str {
        match self {
            Self::Active => "in_progress",
            Self::NeedsInput => "needs_input",
            Self::ReadyForReview => "needs_review",
            Self::Completed => "done",
            Self::Failed | Self::Canceled => "closed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "进行中",
            Self::NeedsInput => "待你输入",
            Self::ReadyForReview => "待验收",
            Self::Completed => "已完成",
            Self::Failed => "失败",
            Self::Canceled => "已取消",
        }
    }
}

/// 结果质量标签，不是生命周期状态。
///
/// 字符串值与 `return_task_result` 的模型工具契约保持一致，
/// 不要为了「名字更好听」改掉 `completed` / `partial` / `blocked` / `dropped`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Completed,
    Partial,
    Blocked,
    Dropped,
}

impl TaskOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Partial => "partial",
            Self::Blocked => "blocked",
            Self::Dropped => "dropped",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "completed" => Some(Self::Completed),
            "partial" => Some(Self::Partial),
            "blocked" => Some(Self::Blocked),
            "dropped" => Some(Self::Dropped),
            _ => None,
        }
    }

    /// 是否算「有了一个可交付的结论」。`Dropped` 也是结论：
    /// 查过了，确认没有需要做的事。
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Completed | Self::Dropped)
    }
}

/// 只有它能改变状态。调用方先落盘再用新状态，不要先发副作用后改状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkTaskEvent {
    Create,
    QuestionCreated,
    QuestionAnswered,
    Settle {
        outcome: TaskOutcome,
        requires_review: bool,
    },
    Accept {
        revision: u32,
        result_revision: u32,
    },
    Reject,
    /// 继续同一轮：清掉上一份结果，回到 Active（前台调用契约用）。
    Resume,
    Fail,
    Cancel,
    Restart,
}

/// 唯一迁移表。非法迁移返回原因，调用方必须如实报错而不是静默改写。
pub fn transition(
    current: Option<WorkTaskStatus>,
    event: WorkTaskEvent,
) -> Result<WorkTaskStatus, String> {
    use WorkTaskEvent::*;
    use WorkTaskStatus as S;
    let next = match (current, event) {
        (None, Create) => S::Active,
        (Some(S::Active), QuestionCreated) => S::NeedsInput,
        (Some(S::NeedsInput), QuestionAnswered) => S::Active,

        // 结果不完整 / 有具体阻塞时，即使没改文件也不能静默算完成：
        // 那会把一个没做完的活伪装成「已完成」。
        (Some(S::Active), Settle { outcome: TaskOutcome::Blocked, .. }) => S::NeedsInput,
        (
            Some(S::Active),
            Settle {
                outcome,
                requires_review,
            },
        ) if requires_review || !outcome.is_settled() => S::ReadyForReview,
        (Some(S::Active), Settle { .. }) => S::Completed,

        (Some(S::ReadyForReview), Accept { revision, result_revision }) => {
            if revision != result_revision {
                return Err(format!(
                    "结果已改为 rev {result_revision}，验收依据的是 rev {revision}；请重新查看后再验收"
                ));
            }
            S::Completed
        }
        (Some(S::ReadyForReview), Reject) => S::Active,
        // 续跑：从任意状态回到进行中。调用方负责同时清空上一份结果。
        (Some(_), Resume) => S::Active,

        (Some(S::Active | S::NeedsInput | S::ReadyForReview), Fail) => S::Failed,
        (Some(S::Active | S::NeedsInput | S::ReadyForReview), Cancel) => S::Canceled,
        // 宿主重启：跑着的活可能已经有外部副作用，不重放；
        // 等输入 / 等验收的成果都在，保留现场。
        (Some(S::Active), Restart) => S::Failed,
        (Some(S::NeedsInput | S::ReadyForReview), Restart) => current.unwrap(),

        (from, event) => {
            return Err(format!(
                "非法任务状态迁移：{:?} --{:?}-> 不允许",
                from.map(S::as_str).unwrap_or("(新建)"),
                event
            ));
        }
    };
    Ok(next)
}

/// 是否强制人工验收。判定只认**宿主观察到的回执**，不采信模型自述：
/// 改了工作区文件、或执行了对外发送动作，才需要人点一下。
/// 纯问答与只读检查自动完成，不制造注意力噪音。
pub fn requires_review(artifacts_changed: bool, outbound_send: bool) -> bool {
    artifacts_changed || outbound_send
}

#[cfg(test)]
mod tests {
    use super::*;
    use WorkTaskEvent as E;
    use WorkTaskStatus as S;

    fn move_to(event: E, from: Option<S>) -> Result<S, String> {
        transition(from, event)
    }

    #[test]
    fn full_happy_path_and_review_rejection_cycle() {
        let a = move_to(E::Create, None).unwrap();
        assert_eq!(a, S::Active);
        let r = transition(
            Some(a),
            E::Settle { outcome: TaskOutcome::Completed, requires_review: true },
        ).unwrap();
        assert_eq!(r, S::ReadyForReview);
        assert!(r.is_worker_done(), "Worker 已停");
        assert!(!r.is_closed(), "但任务还没结束");
        let back = transition(Some(r), E::Reject).unwrap();
        assert_eq!(back, S::Active);
        let r2 = transition(
            Some(back),
            E::Settle { outcome: TaskOutcome::Completed, requires_review: true },
        ).unwrap();
        assert_eq!(
            transition(Some(r2), E::Accept { revision: 1, result_revision: 2 }),
            Err("结果已改为 rev 2，验收依据的是 rev 1；请重新查看后再验收".into())
        );
        assert_eq!(
            transition(Some(r2), E::Accept { revision: 2, result_revision: 2 }).unwrap(),
            S::Completed
        );
    }

    #[test]
    fn pure_question_completes_without_review() {
        let a = S::Active;
        assert_eq!(
            transition(Some(a), E::Settle { outcome: TaskOutcome::Completed, requires_review: false }).unwrap(),
            S::Completed
        );
        assert_eq!(
            transition(Some(a), E::Settle { outcome: TaskOutcome::Dropped, requires_review: false }).unwrap(),
            S::Completed
        );
    }

    #[test]
    fn incomplete_or_blocked_results_never_look_completed() {
        assert_eq!(
            transition(Some(S::Active), E::Settle { outcome: TaskOutcome::Partial, requires_review: false }).unwrap(),
            S::ReadyForReview
        );
        assert_eq!(
            transition(Some(S::Active), E::Settle { outcome: TaskOutcome::Blocked, requires_review: false }).unwrap(),
            S::NeedsInput
        );
    }

    #[test]
    fn question_card_moves_between_active_and_needs_input() {
        let n = transition(Some(S::Active), E::QuestionCreated).unwrap();
        assert_eq!(n, S::NeedsInput);
        assert!(!n.is_worker_done());
        assert_eq!(transition(Some(n), E::QuestionAnswered).unwrap(), S::Active);
    }

    #[test]
    fn terminal_states_are_final_and_restart_keeps_waiting_work() {
        for closed in [S::Completed, S::Failed, S::Canceled] {
            assert!(closed.is_closed() && closed.is_worker_done());
            assert!(transition(Some(closed), E::Cancel).is_err());
            assert!(transition(Some(closed), E::Fail).is_err());
            assert!(transition(Some(closed), E::Settle { outcome: TaskOutcome::Completed, requires_review: false }).is_err());
        }
        assert_eq!(transition(Some(S::Active), E::Restart).unwrap(), S::Failed);
        assert_eq!(transition(Some(S::NeedsInput), E::Restart).unwrap(), S::NeedsInput);
        assert_eq!(transition(Some(S::ReadyForReview), E::Restart).unwrap(), S::ReadyForReview);
    }

    #[test]
    fn illegal_transitions_are_rejected_with_a_reason() {
        assert!(transition(None, E::Cancel).is_err());
        assert!(transition(None, E::Accept { revision: 1, result_revision: 1 }).is_err());
        assert!(transition(Some(S::Completed), E::QuestionCreated).is_err());
        assert!(transition(Some(S::Active), E::Accept { revision: 1, result_revision: 1 }).is_err());
        assert!(transition(Some(S::Active), E::QuestionAnswered).is_err());
        assert!(transition(Some(S::Active), E::Create).is_err());
    }

    #[test]
    fn buckets_and_labels_stay_disjoint() {
        let buckets: Vec<_> = [S::Active, S::NeedsInput, S::ReadyForReview, S::Completed, S::Failed, S::Canceled]
            .iter().map(|s| s.bucket()).collect();
        assert_eq!(buckets, ["in_progress", "needs_input", "needs_review", "done", "closed", "closed"]);
        assert_eq!(S::ReadyForReview.label(), "待验收");
        assert_eq!(S::NeedsInput.as_str(), "needs_input");
    }

    #[test]
    fn review_gate_only_triggers_on_host_observed_effects() {
        assert!(!requires_review(false, false));
        assert!(requires_review(true, false));
        assert!(requires_review(false, true));
    }

    #[test]
    fn outcome_strings_match_the_model_tool_contract() {
        assert_eq!(TaskOutcome::Completed.as_str(), "completed");
        assert_eq!(TaskOutcome::Partial.as_str(), "partial");
        assert_eq!(TaskOutcome::Blocked.as_str(), "blocked");
        assert_eq!(TaskOutcome::Dropped.as_str(), "dropped");
        assert!(TaskOutcome::Dropped.is_settled());
        assert!(!TaskOutcome::Partial.is_settled());
        assert_eq!(TaskOutcome::from_str("nonsense"), None);
    }
}
