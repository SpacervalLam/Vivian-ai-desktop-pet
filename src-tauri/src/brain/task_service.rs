//! 自治任务执行服务（ctx.tasks）
//!
//! Agent-loop 形态：给定一个目标（directive），由 LLM
//! 逐步决策"下一步调用哪个工具"，执行并把结果回填，直到 LLM 声明完成、
//! 某工具声明 `goal_completed`，或达到最大步数。
//!
//! 关键点：
//! - 复用主脑的 `execute_tool_use`，因此自动经过阶段 B 接入的 guard / post-execute
//!   策略缝（沙箱/审批守卫对子任务同样生效）；
//! - 每一步通过 `ctx` 事件总线广播 [`TaskEvent`]，供主动行为、跨角色分享、前端
//!   任务面板订阅；
//! - 任务以 `char_id` 归类，同一角色可并发多个任务，互不干扰。

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::cordis::{RuntimeContext, global_ctx};
use crate::providers::base::LLMRequest;
use crate::providers::router::ModelRouter;
use crate::tools::executor::execute_tool_use;
use crate::tools::types::ToolUseContext;
use crate::tools::ToolSystem;
use crate::types::response::ChatMessage;

/// 单任务最大步数（避免失控死循环）。
pub const MAX_TASK_STEPS: usize = 8;

/// 任务事件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskEventKind {
    Started,
    Step,
    Completed,
    Failed,
    Canceled,
}

/// 任务事件载荷（在 `ctx` 事件总线上广播）。
#[derive(Debug, Clone)]
pub struct TaskEvent {
    pub task_id: String,
    pub char_id: String,
    pub kind: TaskEventKind,
    pub message: String,
}

/// 任务运行状态。
#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Running,
    Succeeded,
    Failed(String),
    Canceled,
}

impl TaskStatus {
    /// 状态字符串（前端面板展示用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Succeeded => "succeeded",
            TaskStatus::Failed(_) => "failed",
            TaskStatus::Canceled => "canceled",
        }
    }

    /// 是否仍在执行。
    pub fn is_running(&self) -> bool {
        matches!(self, TaskStatus::Running)
    }
}

/// 任务对外展示摘要（前端任务/子代理面板 + 谱系树）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub task_id: String,
    pub char_id: String,
    pub directive: String,
    pub status: String,
    /// 失败原因（仅失败时非空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub steps: usize,
    /// 父任务 id（子代理谱系；None 表示顶级任务）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// 子任务 id 列表（谱系追踪）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    /// 子代理回传的报告文本
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
struct TaskState {
    task_id: String,
    char_id: String,
    directive: String,
    steps: usize,
    status: TaskStatus,
    /// 任务完成后的报告（子代理回传给父代理/调用方的文本）
    report: Option<String>,
    /// 报告是否已被陪伴对话注入消费（每份报告只注入一次）
    report_consumed: bool,
    /// 父任务 id（子代理谱系）
    parent: Option<String>,
    /// 子任务 id 列表（谱系追踪）
    children: Vec<String>,
    created_at: i64,
    updated_at: i64,
}

fn to_summary(t: &TaskState) -> TaskSummary {
    let error = match &t.status {
        TaskStatus::Failed(e) => Some(e.clone()),
        _ => None,
    };
    TaskSummary {
        task_id: t.task_id.clone(),
        char_id: t.char_id.clone(),
        directive: t.directive.clone(),
        status: t.status.as_str().to_string(),
        error,
        steps: t.steps,
        parent: t.parent.clone(),
        children: t.children.clone(),
        report: t.report.clone(),
        created_at: t.created_at,
        updated_at: t.updated_at,
    }
}

/// 自治任务服务。
#[derive(Clone)]
pub struct TaskService {
    ctx: Arc<RuntimeContext>,
    tasks: Arc<RwLock<BTreeMap<String, TaskState>>>,
}

impl TaskService {
    pub fn new() -> Arc<Self> {
        let ctx = global_ctx().map(|c| Arc::new(c)).unwrap_or_else(|| Arc::new(RuntimeContext::new()));
        Arc::new(Self {
            ctx,
            tasks: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    /// 获取某角色正在运行的任务数。
    pub fn running_count(&self, char_id: &str) -> usize {
        self.tasks
            .read()
            .values()
            .filter(|t| t.char_id == char_id && t.status == TaskStatus::Running)
            .count()
    }

    /// 获取某角色的任务简表。
    pub fn list_for(&self, char_id: &str) -> Vec<(String, String, TaskStatus, usize)> {
        self.tasks
            .read()
            .values()
            .filter(|t| t.char_id == char_id)
            .map(|t| (t.task_id.clone(), t.directive.clone(), t.status.clone(), t.steps))
            .collect()
    }

    /// 获取某角色的任务摘要（含谱系，供前端任务面板）。
    pub fn summaries_for(&self, char_id: &str) -> Vec<TaskSummary> {
        self.tasks
            .read()
            .values()
            .filter(|t| t.char_id == char_id)
            .map(to_summary)
            .collect()
    }

    /// 获取全部任务摘要（前端任务面板：跨角色总览）。
    pub fn all_summaries(&self) -> Vec<TaskSummary> {
        self.tasks.read().values().map(to_summary).collect()
    }

    /// 某角色顶级（非子代理）运行中任务的摘要（陪伴上下文注入用）。
    pub fn running_top_level_for(&self, char_id: &str) -> Vec<TaskSummary> {
        self.tasks
            .read()
            .values()
            .filter(|t| t.char_id == char_id && t.parent.is_none() && t.status.is_running())
            .map(to_summary)
            .collect()
    }

    /// 某角色已完成但报告尚未注入陪伴对话的顶级任务摘要。
    ///
    /// 每份报告只注入一次（注入方消费后调 [`mark_reports_consumed`]）；
    /// 无报告的失败/取消任务以 error/状态文本代替报告。
    pub fn unconsumed_reports_for(&self, char_id: &str) -> Vec<TaskSummary> {
        let now = chrono::Utc::now().timestamp_millis();
        self.tasks
            .read()
            .values()
            .filter(|t| {
                t.char_id == char_id
                    && t.parent.is_none()
                    && !t.status.is_running()
                    && !t.report_consumed
                    // 完成超过 2 小时的旧报告不再打扰
                    && now - t.updated_at < 2 * 3600 * 1000
            })
            .map(to_summary)
            .collect()
    }

    /// 标记任务报告已注入消费（后续轮次不再重复注入）。
    pub fn mark_reports_consumed(&self, task_ids: &[String]) {
        if task_ids.is_empty() {
            return;
        }
        let mut tasks = self.tasks.write();
        for id in task_ids {
            if let Some(t) = tasks.get_mut(id) {
                t.report_consumed = true;
            }
        }
    }

    /// 获取单个任务摘要（子代理控制：按 task_id 查询）。
    pub fn summary_of(&self, task_id: &str) -> Option<TaskSummary> {
        self.tasks.read().get(task_id).map(to_summary)
    }

    /// 获取某任务的全部后代（递归，按创建顺序），用于谱系追踪。
    pub fn descendants_of(&self, task_id: &str) -> Vec<TaskSummary> {
        let tasks = self.tasks.read();
        let mut out = Vec::new();
        let mut stack = vec![task_id.to_string()];
        while let Some(id) = stack.pop() {
            if let Some(t) = tasks.get(&id) {
                for child in &t.children {
                    if let Some(c) = tasks.get(child) {
                        out.push(to_summary(c));
                    }
                    stack.push(child.clone());
                }
            }
        }
        out
    }

    /// 发起一个自治任务（fire-and-forget 后台循环）。
    ///
    /// 返回任务 ID。`router` / `tool_system` 由调用方（主脑）传入。
    pub fn start(
        &self,
        char_id: impl Into<String>,
        router: Arc<ModelRouter>,
        tool_system: Arc<ToolSystem>,
        directive: impl Into<String>,
    ) -> String {
        self.start_with_parent(char_id, router, tool_system, directive, None)
    }

    /// 发起自治任务，并挂到指定父任务名下（子代理委派 + 谱系追踪）。
    ///
    /// `parent` 为 None 时为顶级任务；Some 时写入父任务的 children 列表。
    pub fn start_with_parent(
        &self,
        char_id: impl Into<String>,
        router: Arc<ModelRouter>,
        tool_system: Arc<ToolSystem>,
        directive: impl Into<String>,
        parent: Option<String>,
    ) -> String {
        let char_id = char_id.into();
        let directive = directive.into();
        let task_id = format!("task-{}", uuid::Uuid::new_v4().simple());
        let now = chrono::Utc::now().timestamp_millis();
        {
            let mut tasks = self.tasks.write();
            if let Some(p) = &parent {
                if let Some(ps) = tasks.get_mut(p) {
                    ps.children.push(task_id.clone());
                }
            }
            tasks.insert(
                task_id.clone(),
                TaskState {
                    task_id: task_id.clone(),
                    char_id: char_id.clone(),
                    directive: directive.clone(),
                    steps: 0,
                    status: TaskStatus::Running,
                    report: None,
                    report_consumed: false,
                    parent,
                    children: Vec::new(),
                    created_at: now,
                    updated_at: now,
                },
            );
        }
        self.emit(&task_id, &char_id, TaskEventKind::Started, &directive);

        let ctx = Arc::clone(&self.ctx);
        let tasks = Arc::clone(&self.tasks);
        let task_id_run = task_id.clone();
        tauri::async_runtime::spawn(async move {
            Self::run_loop(
                &ctx,
                &tasks,
                &task_id_run,
                &char_id,
                &directive,
                router,
                tool_system,
            )
            .await;
        });
        task_id
    }

    /// 主动取消一个任务。
    pub fn cancel(&self, task_id: &str) -> bool {
        let state = {
            let mut tasks = self.tasks.write();
            match tasks.get_mut(task_id) {
                Some(s) if s.status == TaskStatus::Running => {
                    s.status = TaskStatus::Canceled;
                    s.updated_at = chrono::Utc::now().timestamp_millis();
                    Some(s.clone())
                }
                _ => None,
            }
        };
        if let Some(s) = state {
            self.emit(&s.task_id, &s.char_id, TaskEventKind::Canceled, "任务已取消");
            return true;
        }
        false
    }

    /// 对已结束的任务追加新指令继续执行（可延续子代理）。
    ///
    /// 把追加指令拼进原 directive 重新启动循环；已有报告保留（新报告覆盖）。
    pub fn followup(
        &self,
        task_id: &str,
        additional_directive: &str,
        router: Arc<ModelRouter>,
        tool_system: Arc<ToolSystem>,
    ) -> Result<(), String> {
        let (char_id, directive, task_id) = {
            let mut tasks = self.tasks.write();
            let s = tasks
                .get_mut(task_id)
                .ok_or_else(|| format!("任务不存在: {task_id}"))?;
            if s.status == TaskStatus::Running {
                return Err("任务正在运行，不能追加指令".into());
            }
            s.status = TaskStatus::Running;
            s.steps = 0;
            s.updated_at = chrono::Utc::now().timestamp_millis();
            let combined = format!("{}\n\n[追加要求] {}", s.directive, additional_directive);
            s.directive = combined.clone();
            (s.char_id.clone(), combined, s.task_id.clone())
        };
        self.emit(&task_id, &char_id, TaskEventKind::Started, "任务延续执行");
        let ctx = Arc::clone(&self.ctx);
        let tasks = Arc::clone(&self.tasks);
        let tid = task_id.clone();
        tauri::async_runtime::spawn(async move {
            Self::run_loop(&ctx, &tasks, &tid, &char_id, &directive, router, tool_system).await;
        });
        Ok(())
    }

    async fn run_loop(
        ctx: &Arc<RuntimeContext>,
        tasks: &Arc<RwLock<BTreeMap<String, TaskState>>>,
        task_id: &str,
        char_id: &str,
        directive: &str,
        router: Arc<ModelRouter>,
        tool_system: Arc<ToolSystem>,
    ) {
        // 工具上下文携带任务 ID（session_id 字段）：子任务内调用的工具
        // （如 subagent_report）据此知道自己归属哪个任务
        let tool_ctx = ToolUseContext::default().with_char_id(char_id.to_string()).with_session_id(task_id.to_string());
        // 上下文压实：早期步骤折叠进 `compacted` 摘要，近期步骤留 `history` 明细。
        let mut compacted = String::new();
        let mut history: Vec<String> = Vec::new();
        // 只读规划阶段：执行前先由 LLM 给出简短计划引导（不调用任何工具）。
        let plan = Self::plan(&router, char_id, directive).await;
        // 收益递减检测：连续多步结果摘要极短且无目标完成 → 提前终止，不磨满步数上限
        let mut output_tracker = crate::brain::budget::OutputBudgetTracker::new();

        for step in 1..=MAX_TASK_STEPS {
            // 每步前检查是否已被取消
            {
                let guard = tasks.read();
                if let Some(s) = guard.get(task_id) {
                    if s.status != TaskStatus::Running {
                        return;
                    }
                }
            }

            let progress = Self::progress_text(&compacted, &history);
            let decision = Self::decide_next(&router, char_id, directive, Some(plan.as_str()), &progress).await;
            let (done, tool_name, arguments) = match decision {
                Some(d) => d,
                None => {
                    Self::finish(tasks, ctx, task_id, char_id, TaskStatus::Failed("无法解析下一步决策".into()), "任务失败：无法解析智能体决策");
                    return;
                }
            };
            if done {
                Self::set_fallback_report(tasks, task_id, directive, &history);
                Self::finish(tasks, ctx, task_id, char_id, TaskStatus::Succeeded, "已完成");
                return;
            }

            if !tool_system.has_tool(&tool_name) {
                history.push(format!("[step {step}] 工具 {tool_name} 不存在，跳过"));
                continue;
            }

            let result = execute_tool_use(&tool_name, arguments, &tool_system, &tool_ctx, None).await;
            let summary = if result.success {
                serde_json::to_string(result.data.as_ref().unwrap_or(&serde_json::Value::Null))
                    .unwrap_or_else(|_| "[unserializable]".to_string())
            } else {
                result.error.clone().unwrap_or_else(|| "执行失败".to_string())
            };
            history.push(format!("[step {step}] {tool_name}: {summary}"));
            // 上下文压实：超出近期上限的早期步骤折叠进摘要，而非直接丢弃
            Self::compact_history(&mut history, &mut compacted);
            let was_goaled = result.goal_completed;
            // 收益递减检测：结果摘要极短且未完成目标时计数，连续达标提前终止
            if let crate::brain::budget::BudgetVerdict::StopDiminishing { low_rounds } =
                output_tracker.record_chars(summary.chars().count(), was_goaled)
            {
                Self::set_fallback_report(tasks, task_id, directive, &history);
                Self::finish(
                    tasks,
                    ctx,
                    task_id,
                    char_id,
                    TaskStatus::Failed(format!("收益递减：连续 {low_rounds} 步无实质产出")),
                    "任务连续多步无实质产出，已提前终止以节省配额",
                );
                return;
            }
            {
                let mut guard = tasks.write();
                if let Some(s) = guard.get_mut(task_id) {
                    s.steps = step;
                    s.updated_at = chrono::Utc::now().timestamp_millis();
                }
            }
            let step_msg = if result.success {
                format!("步 {step}：调用 {tool_name} 完成")
            } else {
                format!("步 {step}：调用 {tool_name} 失败")
            };
            Self::emit_task(ctx, task_id, char_id, TaskEventKind::Step, &step_msg);
            if was_goaled {
                Self::set_fallback_report(tasks, task_id, directive, &history);
                Self::finish(tasks, ctx, task_id, char_id, TaskStatus::Succeeded, "工具声明目标已完成");
                return;
            }
        }

        Self::finish(tasks, ctx, task_id, char_id, TaskStatus::Failed("达到最大步骤数".into()), "任务达到最大步骤数，已终止");
    }

    /// 成功结束但模型未调 subagent_report 时，用末尾步骤摘要生成兜底报告，
    /// 保证陪伴对话的「后台任务」回流段始终有可汇报的内容。
    fn set_fallback_report(
        tasks: &Arc<RwLock<BTreeMap<String, TaskState>>>,
        task_id: &str,
        directive: &str,
        history: &[String],
    ) {
        {
            let guard = tasks.read();
            if let Some(s) = guard.get(task_id) {
                if s.report.is_some() {
                    return;
                }
            }
        }
        let tail: Vec<String> = history
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|h| {
                let t: String = h.chars().take(200).collect();
                t
            })
            .collect();
        let report = format!("目标：{directive}\n结果：已完成\n末尾步骤：\n{}", tail.join("\n"));
        let mut guard = tasks.write();
        if let Some(s) = guard.get_mut(task_id) {
            if s.report.is_none() {
                s.report = Some(report);
            }
        }
    }

    /// LLM 决策下一步：返回 Some((done, tool_name, arguments))。
    async fn decide_next(
        router: &ModelRouter,
        char_id: &str,
        directive: &str,
        plan: Option<&str>,
        progress: &str,
    ) -> Option<(bool, String, serde_json::Value)> {
        let mut task_text = format!("角色：{char_id}\n目标：{directive}");
        if let Some(p) = plan {
            if !p.is_empty() {
                task_text.push_str("\n执行计划（只读规划，仅供引导，仍以每步实际决策为准）：\n");
                task_text.push_str(p);
            }
        }
        task_text.push_str("\n已执行的步骤：\n");
        task_text.push_str(progress);
        task_text.push_str("\n\n请决策下一步。");
        let system = ChatMessage::system(
            "你是一个桌宠角色的自治任务执行器。你会逐步调用工具来完成给定目标。\
             每次只决策并返回一步，输出**只有一行 JSON**，格式：\
             {\"done\":false,\"tool_name\":\"工具名\",\"arguments\":{...}}\
             当目标已达成、无需再调用工具时，返回 {\"done\":true}。\
             不要输出 JSON 之外的任何文字。",
        );
        let user = ChatMessage::user(task_text);
        let req = LLMRequest::new("reasoning", vec![system, user]);
        let resp = match router.generate(req).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("[TaskService] 决策调用失败: {e}");
                return None;
            }
        };
        let parsed = crate::brain::json_parser::JsonParser::parse_single(&resp).ok()?;
        let done = parsed
            .get("done")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let tool_name = parsed
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let arguments = parsed
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Some((done, tool_name, arguments))
    }

    /// 只读规划：执行前由 LLM 给出简短执行计划引导（只调用生成，不执行任何工具）。
    async fn plan(router: &ModelRouter, char_id: &str, directive: &str) -> String {
        let system = ChatMessage::system(
            "你是桌宠角色的任务规划器。任务执行前，请用两三句话说清打算怎么逐步完成目标。\
             这是只读规划：不许调用任何工具，也不执行任何动作，只输出计划正文。\
             若目标极简单无需规划，回答“无需额外计划，直接执行”。不要输出 JSON。",
        );
        let user = ChatMessage::user(format!("角色：{char_id}\n目标：{directive}\n\n请给出只读执行计划："));
        let req = LLMRequest::new("reasoning", vec![system, user]);
        match router.generate(req).await {
            Ok(t) => t.trim().to_string(),
            Err(e) => {
                tracing::warn!("[TaskService] 规划调用失败: {e}");
                String::new()
            }
        }
    }

    /// 组合压实摘要与近期步骤明细为“已执行记录”文本。
    fn progress_text(compacted: &str, history: &[String]) -> String {
        let mut parts = Vec::new();
        if !compacted.is_empty() {
            parts.push("[早期步骤已压实]".to_string());
            parts.push(compacted.to_string());
        }
        if !history.is_empty() {
            parts.push(history.join("\n"));
        }
        if parts.is_empty() {
            "（尚无执行记录）".to_string()
        } else {
            parts.join("\n")
        }
    }

    /// 上下文压实：超出近期上限的早期步骤折叠进摘要，摘要自身也设上限以控制体积。
    fn compact_history(history: &mut Vec<String>, compacted: &mut String) {
        const MAX_RECENT: usize = 8;
        if history.len() <= MAX_RECENT {
            return;
        }
        let overflow = history.drain(..(history.len() - MAX_RECENT)).collect::<Vec<_>>();
        if overflow.is_empty() {
            return;
        }
        if compacted.len() > 1_200 {
            // 摘要过长时保留尾部最近的部分，前面加省略标记
            let tail = tail_on_boundary(compacted, 800);
            *compacted = format!("…(省略更早步骤)…{}", tail);
        }
        if !compacted.is_empty() {
            compacted.push_str("；");
        }
        compacted.push_str(&overflow.join("；"));
    }

    fn finish(
        tasks: &Arc<RwLock<BTreeMap<String, TaskState>>>,
        ctx: &Arc<RuntimeContext>,
        task_id: &str,
        char_id: &str,
        status: TaskStatus,
        message: &str,
    ) {
        {
            let mut guard = tasks.write();
            if let Some(s) = guard.get_mut(task_id) {
                if !s.status.is_running() {
                    return;
                }
                s.status = status.clone();
                s.updated_at = chrono::Utc::now().timestamp_millis();
            }
        }
        let kind = match &status {
            TaskStatus::Succeeded => TaskEventKind::Completed,
            _ => TaskEventKind::Failed,
        };
        Self::emit_task(ctx, task_id, char_id, kind, message);
    }

    fn emit(&self, task_id: &str, char_id: &str, kind: TaskEventKind, message: &str) {
        Self::emit_task(&self.ctx, task_id, char_id, kind, message);
    }

    /// 查询某任务的最终报告（子代理回传给调用方的文本）。
    ///
    /// 任务尚未结束或没有报告时返回 None。
    pub fn report_of(&self, task_id: &str) -> Option<String> {
        self.tasks.read().get(task_id).and_then(|t| t.report.clone())
    }

    /// 写入任务报告（通常由任务完成后的收尾步骤调用）。
    ///
    /// 报告是子代理工作结果的浓缩摘要，父代理/主动行为可据此决定后续动作。
    pub fn set_report(&self, task_id: &str, report: impl Into<String>) -> bool {
        let mut tasks = self.tasks.write();
        match tasks.get_mut(task_id) {
            Some(s) => {
                s.report = Some(report.into());
                s.updated_at = chrono::Utc::now().timestamp_millis();
                true
            }
            None => false,
        }
    }

    fn emit_task(
        ctx: &Arc<RuntimeContext>,
        task_id: &str,
        char_id: &str,
        kind: TaskEventKind,
        message: &str,
    ) {
        let event = TaskEvent {
            task_id: task_id.to_string(),
            char_id: char_id.to_string(),
            kind,
            message: message.to_string(),
        };
        let c = Arc::clone(ctx);
        tauri::async_runtime::spawn(async move {
            let _ = c.emit_serial(event).await;
        });
    }
}

/// 全局任务服务句柄（AppState 创建时注册，供管线步骤等无 AppState 上下文的代码访问）。
static GLOBAL_SERVICE: std::sync::OnceLock<Arc<TaskService>> = std::sync::OnceLock::new();

/// 注册全局任务服务（幂等，仅首次生效）。
pub fn set_global(svc: Arc<TaskService>) {
    let _ = GLOBAL_SERVICE.set(svc);
}

/// 取全局任务服务（AppState 尚未创建时返回 None）。
pub fn global() -> Option<Arc<TaskService>> {
    GLOBAL_SERVICE.get().cloned()
}

/// 安全截取字符串尾部 `max` 字节（对齐到字符边界）。
fn tail_on_boundary(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start > 0 && !s.is_char_boundary(start) {
        start -= 1;
    }
    s[start..].to_string()
}