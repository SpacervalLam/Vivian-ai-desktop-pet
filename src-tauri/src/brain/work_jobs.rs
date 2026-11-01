//! 后台子任务的注册表与结算收件箱。
//!
//! 主智能体可以把子任务派到后台：立刻拿回一个 `job_id` 继续干自己的活，
//! 稍后再收结果。任务跑完之后结果不会凭空消失——它会躺在收件箱里，
//! 由主循环在下一轮取出并注入上下文（结算通知）。
//!
//! 之所以要"收件箱"而不是让模型自己记得回查：模型可能压根不再调用工具
//! （本轮就此结束），也可能忙着别的事忘了收。结算主动送上门，才不会漏。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::brain::coding_subagent::SubagentOutcome;

/// 单调递增的任务号（展示给模型与用户的是 `job_<n>`）。
static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

/// 单会话同时存在的后台任务上限：防止模型无限派发把机器拖垮。
pub const MAX_JOBS_PER_SESSION: usize = 8;

/// 已确认结算的终态任务保留期（秒）；未读结果不按时间丢弃。
const WORK_JOB_RETENTION_SECS: i64 = 30 * 60;
pub const MAX_OUTPUT_PAGE_CHARS: usize = 8_000;
const SETTLEMENT_PREVIEW_CHARS: usize = 2_048;

/// 后台任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkJobStatus {
    /// 正在跑
    Running,
    /// 正常完成（有最终文本）
    Completed,
    /// 失败（启动或执行过程出错）
    Failed,
    /// 被取消
    Canceled,
}

impl WorkJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkJobStatus::Running => "running",
            WorkJobStatus::Completed => "completed",
            WorkJobStatus::Failed => "failed",
            WorkJobStatus::Canceled => "canceled",
        }
    }

    /// 是否已终结（不再变化）。
    pub fn is_terminal(&self) -> bool {
        !matches!(self, WorkJobStatus::Running)
    }
}

/// 一个后台子任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkJob {
    pub job_id: String,
    pub session_id: String,
    /// 派发时的任务描述（用于结算通知里让模型认出是哪个任务）。
    pub task: String,
    pub depth: usize,
    pub status: WorkJobStatus,
    /// 最终文本（仅 Completed 有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// 失败原因（仅 Failed 有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub rounds: usize,
    pub tool_calls: usize,
    /// 用尽轮次预算而停止（结果可能不完整）。
    pub budget_exhausted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_result: Option<super::work_contract::TaskResult>,
    /// 结算是否已经进入一次成功且未被用户引导作废的模型决策。
    #[serde(default)]
    pub notified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_at: Option<i64>,
    /// 进入终态的时间（Unix 秒）。终态任务保留期满后由滚动清理移除，
    /// 防止注册表（含完整输出文本）随派发次数无界累积。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// 协作式取消标志：置位后子 agent 会在下一轮边界自行退出。
    ///
    /// 为什么不是"立刻杀掉"：中断一次正在执行的工具调用可能留下半截的
    /// 文件写入。让它在轮次边界优雅收手更安全。
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}

impl WorkJob {
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "job_id": self.job_id, "status": self.status, "task": preview(&self.task, 160),
            "rounds": self.rounds, "tool_calls": self.tool_calls, "budget_exhausted": self.budget_exhausted,
            "task_status": self.task_result.as_ref().map(|result| result.status),
            "read_more": "work_job action=output",
        })
    }

    pub fn result_summary(&self) -> serde_json::Value {
        self.task_result.as_ref().map(|result| serde_json::json!({
            "status": result.status, "contract_validated": result.contract_validated,
            "remaining_work": result.remaining_work.iter().take(8).map(|s| preview(s, 256)).collect::<Vec<_>>(),
            "remaining_work_count": result.remaining_work.len(),
            "verification_count": result.verification.len(), "evidence_count": result.evidence.len(),
            "artifact_count": result.artifacts.len(),
            "read_more": "work_job action=output section=task_result",
        })).unwrap_or(serde_json::Value::Null)
    }
}

#[derive(Debug, Serialize)]
pub struct JobOutputPage {
    pub text: String,
    pub offset: usize,
    pub total_chars: usize,
    pub next_offset: Option<usize>,
}

/// Offsets count Unicode scalar values, never bytes, so CJK/emoji pages join
/// without missing characters or splitting a UTF-8 encoding.
pub fn output_page(text: &str, offset: usize, limit: usize) -> JobOutputPage {
    let total_chars = text.chars().count();
    let offset = offset.min(total_chars);
    let page: String = text
        .chars()
        .skip(offset)
        .take(limit.clamp(1, MAX_OUTPUT_PAGE_CHARS))
        .collect();
    let end = offset + page.chars().count();
    JobOutputPage {
        text: page,
        offset,
        total_chars,
        next_offset: (end < total_chars).then_some(end),
    }
}

fn preview(text: &str, limit: usize) -> String {
    let page = output_page(text, 0, limit);
    if page.next_offset.is_some() {
        format!("{}… [预览截断，完整材料可按 job_id 续读]", page.text)
    } else {
        page.text
    }
}

/// 后台任务注册表（全局单例，线程安全）。
pub struct WorkJobRegistry {
    jobs: RwLock<HashMap<String, WorkJob>>,
    path: Option<PathBuf>,
}

#[derive(Default, Serialize, Deserialize)]
struct JobSnapshot {
    next_job_id: u64,
    jobs: Vec<WorkJob>,
}

impl WorkJobRegistry {
    pub fn new() -> Self {
        Self {
            jobs: RwLock::new(HashMap::new()),
            path: None,
        }
    }

    /// Restore facts, never replay a worker whose external effects are unknown.
    pub fn persistent(path: PathBuf) -> Self {
        let saved: JobSnapshot = crate::utils::fs::load_json_or_backup(&path).unwrap_or_default();
        NEXT_JOB_ID.fetch_max(saved.next_job_id, Ordering::Relaxed);
        let mut jobs = HashMap::new();
        for mut job in saved.jobs {
            if let Some(id) = job
                .job_id
                .strip_prefix("job_")
                .and_then(|id| id.parse::<u64>().ok())
            {
                NEXT_JOB_ID.fetch_max(id.saturating_add(1), Ordering::Relaxed);
            }
            if job.status == WorkJobStatus::Running {
                job.status = WorkJobStatus::Failed;
                job.error = Some("宿主已重启，后台任务被中断；此前工具操作的结果可能未知。先检查工作区和已有产物，再决定是否继续，不要直接重复有副作用的操作。".into());
                job.finished_at = Some(unix_now());
                job.notified = false;
                job.notified_at = None;
            }
            jobs.insert(job.job_id.clone(), job);
        }
        let registry = Self {
            jobs: RwLock::new(jobs),
            path: Some(path),
        };
        registry.cleanup_terminal_jobs();
        registry.save_locked(&registry.jobs.read());
        registry
    }

    // The jobs lock covers snapshot creation AND replacement, so an older writer
    // cannot overwrite a newer completion or acknowledgement.
    fn persist_locked(&self, jobs: &HashMap<String, WorkJob>) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut snapshot: Vec<_> = jobs.values().collect();
        snapshot.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        #[derive(Serialize)]
        struct SnapshotRef<'a> {
            next_job_id: u64,
            jobs: Vec<&'a WorkJob>,
        }
        let bytes = serde_json::to_vec(&SnapshotRef {
            next_job_id: NEXT_JOB_ID.load(Ordering::Relaxed),
            jobs: snapshot,
        })
        .map_err(|error| error.to_string())?;
        crate::utils::fs::atomic_write(path, &bytes).map_err(|error| error.to_string())
    }

    fn save_locked(&self, jobs: &HashMap<String, WorkJob>) {
        if let Err(error) = self.persist_locked(jobs) {
            tracing::error!("[work_jobs] 无法保存后台任务状态: {error}");
        }
    }

    /// 登记一个刚派发的后台任务，返回 `job_id`。
    ///
    /// 会话内并发任务数超过上限时拒绝——模型应当先收掉几个再派新的。
    pub fn create(&self, session_id: String, task: String, depth: usize) -> Result<String, String> {
        self.cleanup_terminal_jobs();
        let mut jobs = self.jobs.write();
        let active = jobs
            .values()
            .filter(|j| j.session_id == session_id && j.status == WorkJobStatus::Running)
            .count();
        if active >= MAX_JOBS_PER_SESSION {
            return Err(format!(
                "后台任务并发已达上限（{MAX_JOBS_PER_SESSION} 个仍在运行）。请先收集已有任务的结果，或取消不再需要的任务。"
            ));
        }
        // 登记路径顺带做滚动清理：终态任务保留期满即移除（注册表是进程级单例，
        // 不清理的话每份任务的完整输出文本会随派发次数永久累积）。
        let job_id = format!("job_{}", NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed));
        jobs.insert(
            job_id.clone(),
            WorkJob {
                job_id: job_id.clone(),
                session_id,
                task,
                depth,
                status: WorkJobStatus::Running,
                output: None,
                error: None,
                rounds: 0,
                tool_calls: 0,
                budget_exhausted: false,
                task_result: None,
                notified: false,
                notified_at: None,
                finished_at: None,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        if let Err(error) = self.persist_locked(&jobs) {
            jobs.remove(&job_id);
            return Err(format!("无法持久化后台任务，尚未启动执行：{error}"));
        }
        Ok(job_id)
    }

    /// 取某任务的取消标志（交给执行中的子 agent 轮询）。
    pub fn cancel_flag(&self, job_id: &str) -> Option<Arc<AtomicBool>> {
        self.jobs.read().get(job_id).map(|j| Arc::clone(&j.cancel))
    }

    /// 任务正常结束。已被取消的任务不会被改回 Completed——取消是终局。
    pub fn complete(&self, job_id: &str, outcome: SubagentOutcome, budget_exhausted: bool) {
        let mut jobs = self.jobs.write();
        if let Some(job) = jobs.get_mut(job_id) {
            if job.status != WorkJobStatus::Running {
                return;
            }
            job.status = WorkJobStatus::Completed;
            job.output = Some(outcome.output);
            job.task_result = Some(outcome.result);
            job.rounds = outcome.rounds;
            job.tool_calls = outcome.tool_calls;
            job.budget_exhausted = budget_exhausted;
            job.finished_at = Some(unix_now());
            self.save_locked(&jobs);
        }
    }

    /// 任务失败（启动失败或执行中出错）。已被取消的任务同样不被覆盖。
    pub fn fail(&self, job_id: &str, error: String) {
        let mut jobs = self.jobs.write();
        if let Some(job) = jobs.get_mut(job_id) {
            if job.status != WorkJobStatus::Running {
                return;
            }
            job.status = WorkJobStatus::Failed;
            job.error = Some(error);
            job.finished_at = Some(unix_now());
            self.save_locked(&jobs);
        }
    }

    /// 取消任务。只有仍在跑的能被取消，已终结的返回 false。
    ///
    /// 置位取消标志后，子 agent 会在下一轮边界退出；状态立即标记为 Canceled，
    /// 之后即使它跑完也不会覆盖这里的结论。
    pub fn cancel(&self, job_id: &str) -> bool {
        let mut jobs = self.jobs.write();
        match jobs.get_mut(job_id) {
            Some(job) if job.status == WorkJobStatus::Running => {
                job.cancel.store(true, Ordering::SeqCst);
                job.status = WorkJobStatus::Canceled;
                job.finished_at = Some(unix_now());
                self.save_locked(&jobs);
                true
            }
            _ => false,
        }
    }

    /// 取单个任务（含完整输出）。
    pub fn get(&self, job_id: &str) -> Option<WorkJob> {
        self.jobs.read().get(job_id).cloned()
    }

    /// 列出某会话的全部任务（按任务号排序）。
    pub fn list_for(&self, session_id: &str) -> Vec<WorkJob> {
        let mut list: Vec<WorkJob> = self
            .jobs
            .read()
            .values()
            .filter(|j| j.session_id == session_id)
            .cloned()
            .collect();
        list.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        list
    }

    /// Running and uncollected terminal jobs must be considered before completion.
    pub fn pending_for(&self, session_id: &str) -> Vec<String> {
        let jobs = self.jobs.read();
        let mut pending: Vec<_> = jobs
            .values()
            .filter(|j| {
                j.session_id == session_id && (j.status == WorkJobStatus::Running || !j.notified)
            })
            .map(|j| format!("{} ({})", j.job_id, j.status.as_str()))
            .collect();
        pending.sort();
        pending
    }

    /// 取消某会话仍在跑的全部任务（会话取消 / 重置时用）。
    pub fn cancel_session(&self, session_id: &str) -> usize {
        let mut jobs = self.jobs.write();
        let mut n = 0;
        for job in jobs.values_mut() {
            if job.session_id == session_id && job.status == WorkJobStatus::Running {
                job.cancel.store(true, Ordering::SeqCst);
                job.status = WorkJobStatus::Canceled;
                job.finished_at = Some(unix_now());
                n += 1;
            }
        }
        if n > 0 {
            self.save_locked(&jobs);
        }
        n
    }

    /// Reading is not delivery. The exact batch is acknowledged after a usable
    /// model response; request errors, cancellation and steering leave it pending.
    pub fn pending_settlements(&self, session_id: &str) -> Vec<WorkJob> {
        let jobs = self.jobs.read();
        let mut out: Vec<_> = jobs
            .values()
            .filter(|job| job.session_id == session_id && job.status.is_terminal() && !job.notified)
            .collect();
        out.sort_by(|a, b| {
            a.finished_at
                .cmp(&b.finished_at)
                .then(a.job_id.cmp(&b.job_id))
        });
        out.into_iter()
            .take(MAX_JOBS_PER_SESSION)
            .cloned()
            .collect()
    }

    pub fn acknowledge_settlements(&self, session_id: &str, ids: &[String]) -> Result<(), String> {
        let mut jobs = self.jobs.write();
        let mut changed = Vec::new();
        for job in jobs.values_mut() {
            if job.session_id == session_id
                && job.status.is_terminal()
                && !job.notified
                && ids.contains(&job.job_id)
            {
                job.notified = true;
                job.notified_at = Some(unix_now());
                changed.push(job.job_id.clone());
            }
        }
        if changed.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.persist_locked(&jobs) {
            for id in changed {
                let job = jobs.get_mut(&id).unwrap();
                job.notified = false;
                job.notified_at = None;
            }
            return Err(error);
        }
        Ok(())
    }

    /// Keep a small recall index after delivery; full results stay outside the
    /// context and remain addressable through work_job output.
    pub fn recent_results(&self, session_id: &str) -> Vec<serde_json::Value> {
        let guard = self.jobs.read();
        let mut jobs: Vec<_> = guard
            .values()
            .filter(|job| job.session_id == session_id && job.status.is_terminal() && job.notified)
            .collect();
        jobs.sort_by(|a, b| {
            b.finished_at
                .cmp(&a.finished_at)
                .then(b.job_id.cmp(&a.job_id))
        });
        jobs.into_iter().take(MAX_JOBS_PER_SESSION).map(|job| serde_json::json!({
            "job_id":job.job_id,"status":job.status,"task":preview(&job.task, 160),
            "result_preview":preview(job.output.as_deref().or(job.error.as_deref()).unwrap_or(""), 512),
            "task_status":job.task_result.as_ref().map(|result| result.status),
            "read_more":"work_job action=output; offset/limit are character counts",
        })).collect()
    }

    /// 清理已终结且超过保留期的任务（旧数据无 finished_at 时也一并清理）。
    pub fn cleanup_terminal_jobs(&self) {
        let now = unix_now();
        let retention = WORK_JOB_RETENTION_SECS as f64;
        let mut jobs = self.jobs.write();
        let before = jobs.len();
        jobs.retain(|_, j| match j.status {
            WorkJobStatus::Running => true,
            _ if !j.notified => true,
            _ => j
                .notified_at
                .or(j.finished_at)
                .map(|ts| ((now - ts) as f64) < retention)
                .unwrap_or(false),
        });
        let removed = before - jobs.len();
        if removed > 0 {
            tracing::debug!(
                "[work_jobs] 清理 {} 个终态任务（保留期={}s）",
                removed,
                WORK_JOB_RETENTION_SECS
            );
            self.save_locked(&jobs);
        }
    }
}

/// 当前 Unix 时间（秒）。
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Default for WorkJobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

static WORK_JOB_REGISTRY: once_cell::sync::Lazy<Arc<WorkJobRegistry>> =
    once_cell::sync::Lazy::new(|| {
        // Unit tests use an ephemeral global; recovery tests supply their own
        // temporary paths rather than touching the desktop user's live jobs.
        #[cfg(test)]
        {
            Arc::new(WorkJobRegistry::new())
        }
        #[cfg(not(test))]
        {
            Arc::new(WorkJobRegistry::persistent(
                crate::utils::path::get_user_data_dir().join("work_jobs.json"),
            ))
        }
    });

/// 全局注册表访问器（工具与主循环共用同一实例）。
pub fn global_work_job_registry() -> Arc<WorkJobRegistry> {
    Arc::clone(&WORK_JOB_REGISTRY)
}

/// 把一个已终结的任务渲染成注入上下文的结算通知。
pub fn render_settlement(job: &WorkJob) -> String {
    let mut text = match job.status {
        WorkJobStatus::Completed => {
            let mut t = format!(
                "[后台子任务结算] {} 执行已结束（{} 轮 / {} 次工具调用）\n任务：{}\n结果：\n{}",
                job.job_id,
                job.rounds,
                job.tool_calls,
                preview(&job.task, 512),
                preview(
                    job.output.as_deref().unwrap_or("（无输出）"),
                    SETTLEMENT_PREVIEW_CHARS
                )
            );
            if job.budget_exhausted {
                t.push_str("\n注意：该任务用尽轮次预算才停下，结果可能不完整——如需继续，可以再派一个任务接手。");
            }
            if job.task_result.is_some() {
                t.push_str(&format!(
                    "\n结构化任务结算摘要（不等同于事实验证）：{}",
                    job.result_summary()
                ));
            }
            t
        }
        WorkJobStatus::Failed => format!(
            "[后台子任务结算] {} 执行失败\n任务：{}\n错误：{}",
            job.job_id,
            preview(&job.task, 512),
            preview(
                job.error.as_deref().unwrap_or("（未知错误）"),
                SETTLEMENT_PREVIEW_CHARS
            )
        ),
        WorkJobStatus::Canceled => format!(
            "[后台子任务结算] {} 已被取消\n任务：{}",
            job.job_id,
            preview(&job.task, 512)
        ),
        WorkJobStatus::Running => format!(
            "[后台子任务结算] {} 仍在运行（本不应出现在这里）\n任务：{}",
            job.job_id,
            preview(&job.task, 512)
        ),
    };
    text.push_str(&format!("\n完整材料：work_job action=output job_id={}；支持 offset/limit 按字符续读，section=task_result 可读取完整结构化结算。", job.job_id));
    // 强调本通知是系统注入的后台任务结果、不是用户的新指令：
    // 用户可能在同一轮刚发了新消息，模型须把结算当背景信息、优先响应用户最新指令。
    text.push_str(
        "\n（系统注：以上是系统自动注入的后台子任务**结果通知**，不是用户的新指令。\
         若它与你刚收到的用户新消息同时出现在本轮，请把它视为背景信息、不要当作待执行的新任务，\
         优先响应用户的最新指令。）",
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::coding_subagent::SubagentStop;
    use crate::brain::work_contract::{unvalidated_return, TaskStatus};

    #[test]
    fn completion_waits_for_running_and_uncollected_jobs_in_the_same_session() {
        let registry = WorkJobRegistry::new();
        let id = registry
            .create("parent".into(), "Inspect".into(), 1)
            .unwrap();
        registry
            .create("other".into(), "Unrelated task".into(), 1)
            .unwrap();
        assert_eq!(registry.pending_for("parent").len(), 1);
        registry.complete(
            &id,
            SubagentOutcome {
                output: "Partial findings".into(),
                rounds: 1,
                tool_calls: 0,
                stop: SubagentStop::MaxRounds,
                result: unvalidated_return(
                    "Partial findings".into(),
                    TaskStatus::Partial,
                    vec!["Verify".into()],
                    vec![],
                ),
            },
            true,
        );
        assert_eq!(registry.pending_for("parent").len(), 1);
        let settlements = registry.pending_settlements("parent");
        assert_eq!(
            settlements[0].task_result.as_ref().unwrap().status,
            TaskStatus::Partial
        );
        assert!(render_settlement(&settlements[0]).contains("partial"));
        assert_eq!(
            registry.pending_settlements("parent").len(),
            1,
            "reading is not delivery"
        );
        registry.acknowledge_settlements("parent", &[id]).unwrap();
        assert!(registry.pending_for("parent").is_empty());
        assert_eq!(registry.pending_for("other").len(), 1);
    }

    #[test]
    fn late_result_cannot_turn_a_cancelled_job_into_completion() {
        let registry = WorkJobRegistry::new();
        let id = registry
            .create("parent".into(), "Inspect".into(), 1)
            .unwrap();
        assert!(registry.cancel(&id));
        registry.complete(
            &id,
            SubagentOutcome {
                output: "Late answer".into(),
                rounds: 1,
                tool_calls: 0,
                stop: SubagentStop::Completed,
                result: unvalidated_return(
                    "Late answer".into(),
                    TaskStatus::Partial,
                    vec![],
                    vec![],
                ),
            },
            false,
        );
        assert_eq!(registry.get(&id).unwrap().status, WorkJobStatus::Canceled);
        assert!(registry.get(&id).unwrap().task_result.is_none());
    }

    fn outcome(output: String) -> SubagentOutcome {
        SubagentOutcome {
            result: unvalidated_return(
                output.clone(),
                TaskStatus::Partial,
                vec!["Verify".into()],
                vec![],
            ),
            output,
            rounds: 1,
            tool_calls: 0,
            stop: SubagentStop::Completed,
        }
    }

    #[test]
    fn restart_preserves_results_and_reports_interruption_without_replaying_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let registry = WorkJobRegistry::persistent(path.clone());
        let finished = registry
            .create("parent".into(), "Inspect".into(), 1)
            .unwrap();
        registry.complete(&finished, outcome("真实结果".into()), false);
        let interrupted = registry
            .create("parent".into(), "Write file".into(), 1)
            .unwrap();
        drop(registry);
        let restored = WorkJobRegistry::persistent(path.clone());
        assert_eq!(
            restored.get(&finished).unwrap().output.as_deref(),
            Some("真实结果")
        );
        let failed = restored.get(&interrupted).unwrap();
        assert_eq!(failed.status, WorkJobStatus::Failed);
        assert!(failed.error.unwrap().contains("不要直接重复"));
        assert_eq!(restored.pending_settlements("parent").len(), 2);
        restored
            .acknowledge_settlements("parent", &[finished.clone(), interrupted.clone()])
            .unwrap();
        assert!(WorkJobRegistry::persistent(path)
            .pending_settlements("parent")
            .is_empty());
        let new_id = restored.create("parent".into(), "Next".into(), 1).unwrap();
        assert_ne!(new_id, finished);
        assert_ne!(new_id, interrupted);
        assert!(restored
            .recent_results("parent")
            .iter()
            .any(|result| result["job_id"] == finished));
    }

    #[test]
    fn acknowledgement_is_scoped_to_the_exact_delivered_batch() {
        let registry = WorkJobRegistry::new();
        let first = registry.create("parent".into(), "First".into(), 1).unwrap();
        registry.complete(&first, outcome("first".into()), false);
        let batch = registry.pending_settlements("parent");
        let next = registry.create("parent".into(), "Next".into(), 1).unwrap();
        registry.complete(&next, outcome("next".into()), false);
        registry
            .acknowledge_settlements("other", &[first.clone()])
            .unwrap();
        assert_eq!(registry.pending_settlements("parent").len(), 2);
        registry
            .acknowledge_settlements(
                "parent",
                &batch
                    .iter()
                    .map(|job| job.job_id.clone())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        assert_eq!(registry.pending_settlements("parent")[0].job_id, next);
    }

    #[test]
    fn expired_unread_results_survive_cleanup_and_are_removed_after_delivery() {
        let registry = WorkJobRegistry::new();
        let id = registry
            .create("parent".into(), "Inspect".into(), 1)
            .unwrap();
        registry.fail(&id, "network error".into());
        registry.jobs.write().get_mut(&id).unwrap().finished_at = Some(0);
        registry.cleanup_terminal_jobs();
        assert!(registry.get(&id).is_some());
        registry
            .acknowledge_settlements("parent", &[id.clone()])
            .unwrap();
        registry.cleanup_terminal_jobs();
        assert!(
            registry.get(&id).is_some(),
            "a delayed delivery starts a fresh recall window"
        );
        registry.jobs.write().get_mut(&id).unwrap().notified_at = Some(0);
        registry.cleanup_terminal_jobs();
        assert!(registry.get(&id).is_none());
    }

    #[test]
    fn removing_the_last_result_preserves_the_id_watermark_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let registry = WorkJobRegistry::persistent(path.clone());
        let id = registry
            .create("parent".into(), "Inspect".into(), 1)
            .unwrap();
        registry.fail(&id, "failed".into());
        registry
            .acknowledge_settlements("parent", &[id.clone()])
            .unwrap();
        registry.jobs.write().get_mut(&id).unwrap().notified_at = Some(0);
        registry.cleanup_terminal_jobs();
        let saved: JobSnapshot = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.jobs.is_empty());
        assert!(saved.next_job_id > id.strip_prefix("job_").unwrap().parse::<u64>().unwrap());
        assert_ne!(
            WorkJobRegistry::persistent(path)
                .create("parent".into(), "Next".into(), 1)
                .unwrap(),
            id
        );
    }

    #[test]
    fn session_cancellation_sets_the_actual_worker_flags() {
        let registry = WorkJobRegistry::new();
        let id = registry
            .create("parent".into(), "Inspect".into(), 1)
            .unwrap();
        let other = registry
            .create("other".into(), "Continue".into(), 1)
            .unwrap();
        let flag = registry.cancel_flag(&id).unwrap();
        assert_eq!(registry.cancel_session("parent"), 1);
        assert!(flag.load(Ordering::SeqCst));
        assert!(!registry.cancel_flag(&other).unwrap().load(Ordering::SeqCst));
        registry.complete(&id, outcome("late result".into()), false);
        assert_eq!(registry.get(&id).unwrap().status, WorkJobStatus::Canceled);
    }

    #[test]
    fn concurrent_launches_obey_the_session_limit_and_preserve_the_latest_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let registry = Arc::new(WorkJobRegistry::persistent(path.clone()));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let registry = registry.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    registry.create("parent".into(), "Inspect".into(), 1)
                })
            })
            .collect();
        let successes = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap().ok())
            .count();
        assert_eq!(successes, MAX_JOBS_PER_SESSION);
        assert_eq!(registry.list_for("parent").len(), successes);
        assert_eq!(
            WorkJobRegistry::persistent(path).list_for("parent").len(),
            successes
        );
    }

    #[test]
    fn storage_failure_prevents_launch_and_keeps_unacknowledged_results_pending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let registry = WorkJobRegistry::persistent(path.clone());
        let id = registry
            .create("parent".into(), "Inspect".into(), 1)
            .unwrap();
        registry.complete(&id, outcome("result".into()), false);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(registry
            .create("parent".into(), "Must not start".into(), 1)
            .is_err());
        assert_eq!(registry.list_for("parent").len(), 1);
        assert!(registry.acknowledge_settlements("parent", &[id]).is_err());
        assert_eq!(registry.pending_settlements("parent").len(), 1);
    }

    #[test]
    fn unicode_pages_reconstruct_the_full_result_and_previews_stay_bounded() {
        let text = "中文🦀e\u{301}\n".repeat(3000);
        let mut reconstructed = String::new();
        let mut offset = 0;
        loop {
            let page = output_page(&text, offset, 137);
            reconstructed.push_str(&page.text);
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(reconstructed, text);
        assert!(output_page(&text, usize::MAX, 100).text.is_empty());
        assert_eq!(
            output_page(&text, 0, usize::MAX).text.chars().count(),
            MAX_OUTPUT_PAGE_CHARS
        );
        let registry = WorkJobRegistry::new();
        let id = registry.create("parent".into(), text.clone(), 1).unwrap();
        let mut result = outcome(text.clone());
        result.result.remaining_work = vec![text.clone(); 100];
        registry.complete(&id, result, false);
        let job = registry.get(&id).unwrap();
        assert!(render_settlement(&job).chars().count() < 6000);
        assert!(!job.summary().to_string().contains(&text));
        assert_eq!(job.output.as_deref(), Some(text.as_str()));
        assert_eq!(job.task_result.unwrap().remaining_work.len(), 100);
    }
}

// ============================================================================
// 父 → 子 agent 的单向消息收件箱
// ============================================================================

/// 收件箱：job_id → 待投递消息。
///
/// 为什么不"立刻打断"：子 agent 可能正处在一次工具调用中间，硬插消息会撕裂上下文
/// （半截写入、参数与结果错位）。消息在**轮次边界**被取走，作为一条普通 user 消息
/// 进入下一轮——这也是协作式取消用同一套思路的原因。
fn agent_inbox() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<String>>> {
    static INBOX: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
    > = std::sync::OnceLock::new();
    INBOX.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 投递一条消息给运行中的后台子 agent。任务不存在或已结束时返回错误。
pub fn push_agent_message(job_id: &str, text: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("消息不能为空".to_string());
    }
    let registry = global_work_job_registry();
    match registry.get(job_id) {
        None => return Err(format!("找不到任务 {job_id}")),
        Some(job) if job.status.is_terminal() => {
            return Err(format!(
                "任务 {job_id} 已结束（{}），消息无法送达；需要它继续工作就重新派发",
                job.status.as_str()
            ));
        }
        _ => {}
    }
    agent_inbox()
        .lock()
        .map_err(|_| "收件箱锁定失败".to_string())?
        .entry(job_id.to_string())
        .or_default()
        .push(trimmed.to_string());
    Ok(())
}

/// 取走该任务的待收消息（子 agent 在轮次边界调用）。
pub fn drain_agent_messages(job_id: &str) -> Vec<String> {
    agent_inbox()
        .lock()
        .map(|mut m| m.remove(job_id).unwrap_or_default())
        .unwrap_or_default()
}
