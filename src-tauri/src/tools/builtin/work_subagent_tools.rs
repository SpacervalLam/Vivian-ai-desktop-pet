//! 工作智能体委派工具（与陪伴/任务侧的 `spawn_subagent` 相互独立）。
//!
//! - `work_delegate`：把一个自包含的子任务交给**独立的子 agent**。默认前台
//!   （`await` 到结果）；`background: true` 时立刻返回任务号，主 agent 继续干活。
//! - `work_job`：后台任务的收集 / 取消 / 列表。
//!
//! 子 agent 有自己的消息历史和工具循环，跑完只把**最终文本**交回主 agent，
//! 中间的探索过程不会进入主上下文——这正是委派的价值所在。
//!
//! 后台任务的结果不会凭空消失：任务终结后结算会躺进收件箱，由主循环在下一轮
//! 主动送进上下文，模型不必记得回查。

use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tauri::{AppHandle, Emitter, Manager};

use crate::brain::coding_subagent::{
    run_subagent, SubagentRequest, SubagentStop, SUBAGENT_DEFAULT_ROUNDS, SUBAGENT_DEFAULT_TOOLS,
    SUBAGENT_MAX_DEPTH, SUBAGENT_MAX_ROUNDS,
};
use crate::brain::work_jobs::{global_work_job_registry, WorkJobStatus};
use crate::state::AppState;
use crate::tools::types::{
    AgentAccessLevel, PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write().unwrap() = Some(handle);
}

const DESC_DELEGATE_EN: &str = "Delegate one self-contained subtask to a fresh subagent. \
By default this waits for the result and returns the subagent's final text. \
With `background: true` it returns a job id immediately so you can keep working — the finished \
result is delivered back to you on its own; collect it early with `work_job` only when your next \
action really depends on it. \
The subagent starts with an EMPTY history: it sees only the `task` and `context` you write here, \
never your conversation. Give it everything it needs — goal, relevant paths, constraints. \
It runs its own tool loop and returns ONLY its final text, so its exploration stays out of your \
context. Use it for work you can hand off whole (survey a module, verify a hypothesis, implement \
one isolated change), not for things a single tool call settles. \
The subagent cannot ask the user anything, so resolve ambiguity yourself; if a genuine fork \
remains, say so in `context` and it will pick a direction and report which one it took.";

const DESC_DELEGATE_ZH: &str = "把一个自包含的子任务委派给一个全新的子 agent。\
默认等待结果并返回它的最终文本；`background: true` 时立即返回任务号，你可以继续干活——\
完成的结果会自己送回上下文，只有当你下一步确实依赖某个结果时，才用 `work_job` 提前收集。\
子 agent 的历史是**空的**——它只看到你在这里写的 `task` 与 `context`，看不到你的对话。\
所以要把目标、相关路径、约束一次性交代清楚。它跑自己的工具循环，只把**最终文本**返回给你，\
中间的探索过程不会进你的上下文。适用于能整块交出去的工作（摸清一个模块、验证一个猜测、\
完成一处独立改动），而不是一次工具调用就能解决的事。\
子 agent 无法向用户提问，所以歧义要你自己消化；如果确实存在分叉，在 `context` 里说明，\
它会自行选一条路并汇报选了哪条。";

const DESC_DELEGATE_JA: &str = "自己完結したサブタスクを新しいサブエージェントに委任します。\
既定では結果を待って最終テキストを返します。`background: true` なら即座にジョブ ID を返し、\
あなたは作業を続けられます。完了した結果は自動的に届きます。次の一手がその結果に依存する\
場合にだけ `work_job` で早期回収してください。\
サブエージェントの履歴は**空**です。ここに書いた `task` と `context` だけを見て、\
あなたの会話は見えません。目標・関連パス・制約をすべて書き込んでください。\
独自のツールループを回し、**最終テキストだけ**を返します。途中の探索はあなたのコンテキストに\
入りません。1モジュールの調査、仮説の検証、独立した1箇所の変更など、丸ごと任せられる作業に\
使い、1回のツール呼び出しで済むことには使わないでください。サブエージェントはユーザーに\
質問できません。曖昧さは自分で解消し、本当に分岐があるなら `context` に書いてください。";

const DESC_JOB_EN: &str = "Collect, cancel, or list background subagent jobs started with \
`work_delegate` (`background: true`). `output` returns a finished job's full text; if it is still \
running it says so instead of blocking. `cancel` abandons a job you no longer need. `list` shows \
every job in this session with its status. Finished results are delivered back to you automatically \
— reach for `output` only when your next action depends on that specific result.";

const DESC_JOB_ZH: &str = "收集 / 取消 / 列出后台子任务（由 `work_delegate` 的 `background: true` \
派发）。`output` 取回已完成任务的结果全文；任务仍在跑时会直接说明，不会阻塞等待。\
`cancel` 放弃一个不再需要的任务。`list` 列出本会话所有任务及其状态。\
已完成的结果会自动送回你的上下文——只有当你下一步确实依赖某个特定结果时，才需要用 `output`。";

const DESC_JOB_JA: &str = "バックグラウンドのサブエージェントジョブを回収・中止・一覧します\
（`work_delegate` の `background: true` で開始したもの）。`output` は完了済みジョブの全文を\
返します。実行中ならブロックせずその旨を伝えます。`cancel` は不要になったジョブを放棄します。\
`list` はセッション内の全ジョブと状態を表示します。完了した結果は自動的に届くため、\
次の一手が特定の結果に依存するときだけ `output` を使ってください。";

/// 工作智能体委派子任务。
pub struct WorkDelegateTool;

impl WorkDelegateTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkDelegateTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 收集/取消/列出后台子任务。
pub struct WorkJobTool;

impl WorkJobTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkJobTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WorkDelegateTool {
    fn name(&self) -> &str {
        "work_delegate"
    }

    fn description(&self) -> &str {
        DESC_DELEGATE_EN
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => DESC_DELEGATE_ZH,
            "ja" => DESC_DELEGATE_JA,
            _ => DESC_DELEGATE_EN,
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "派出去做\n交给子代理\n分头处理",
            "en" => "delegate this\nhand it to a subagent\nsplit it up",
            "ja" => "委任して\nサブエージェントに任せて\n分担して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        Self::schema_for("en")
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        Self::schema_for(lang)
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("task").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("task 是必填项且不能为空", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        // 递归预算：子 agent 再委派就 +1，到顶直接拒绝（工具仍然可见，
        // 但调用会拿到明确错误，模型据此改为自己做）。
        if context.subagent_depth >= SUBAGENT_MAX_DEPTH {
            return ToolResult::standard_error(
                &format!(
                    "委派深度已达上限（当前深度 {}，上限 {SUBAGENT_MAX_DEPTH}），不能再派下层子 agent。\
                     请自己用工具完成这部分工作，或把结果连同未完成的部分一并返回给上层。",
                    context.subagent_depth
                ),
                Some("SubagentDepthExceeded"),
                None,
            );
        }

        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if task.is_empty() {
            return ToolResult::standard_error("task 不能为空", Some("SubagentError"), None);
        }
        if task.chars().count() > 2000 {
            return ToolResult::standard_error(
                "task 过长（上限 2000 字）：子任务描述应当聚焦",
                Some("SubagentError"),
                None,
            );
        }
        let ctx_text = args
            .get("context")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty());
        let background = args
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 工具白名单：默认只读探索 + 命令；显式指定时追加，
        // 但始终剔除委派/询问/播报/进化类——那些归主 agent 所有。
        let mut tools: Vec<String> = SUBAGENT_DEFAULT_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect();
        if let Some(extra) = args.get("tools").and_then(|v| v.as_array()) {
            for item in extra {
                if let Some(name) = item.as_str() {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return ToolResult::standard_error(
                            "tools 里的工具名不能为空",
                            Some("SubagentError"),
                            None,
                        );
                    }
                    if !tools.contains(&name) {
                        tools.push(name);
                    }
                }
            }
        }
        tools.retain(|t| {
            !matches!(
                t.as_str(),
                "work_delegate" | "work_ask_user" | "work_job" | "notify_companion" | "send_image"
            )
        });

        let max_rounds = args
            .get("max_rounds")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, SUBAGENT_MAX_ROUNDS))
            .unwrap_or(SUBAGENT_DEFAULT_ROUNDS);

        let app = match APP_HANDLE.read().unwrap().clone() {
            Some(a) => a,
            None => {
                return ToolResult::standard_error(
                    "无法委派子任务（后端未初始化）",
                    Some("SubagentUnavailable"),
                    None,
                )
            }
        };
        let state = app.state::<Arc<AppState>>();
        let router = match state.model_router.read().clone() {
            Some(r) => Arc::new(r),
            None => {
                return ToolResult::standard_error(
                    "模型路由未就绪，无法委派子任务",
                    Some("SubagentUnavailable"),
                    None,
                )
            }
        };
        let tool_system = state.tool_system.clone();

        // 后台模式：登记任务 → 立刻返回任务号，执行交给独立任务
        if background {
            let jobs = global_work_job_registry();
            let job_id = match jobs.create(
                context.session_id.clone(),
                task.clone(),
                context.subagent_depth,
            ) {
                Ok(id) => id,
                Err(e) => return ToolResult::standard_error(&e, Some("SubagentError"), None),
            };
            let cancel = jobs.cancel_flag(&job_id);
            let request = SubagentRequest {
                parent_session_id: context.session_id.clone(),
                task: task.clone(),
                context: ctx_text,
                tools,
                max_rounds,
                depth: context.subagent_depth,
                working_directory: context.working_directory.clone(),
                access_level: context
                    .access_level
                    .clone()
                    .unwrap_or(AgentAccessLevel::FsWrite),
                char_id: context.char_id.clone(),
                cancel,
            };
            let app2 = app.clone();
            let jobs2 = Arc::clone(&jobs);
            let job_id2 = job_id.clone();
            let sid = context.session_id.clone();
            let _ = app.emit(
                "coding:job",
                json!({ "session_id": sid, "job_id": job_id, "status": "started" }),
            );
            tauri::async_runtime::spawn(async move {
                let outcome = run_subagent(&router, &tool_system, Some(&app2), request).await;
                let status = match outcome {
                    Ok(o) => {
                        let exhausted = o.stop == SubagentStop::MaxRounds;
                        let canceled = o.stop == SubagentStop::Canceled;
                        jobs2.complete(&job_id2, o, exhausted);
                        if canceled { "canceled" } else { "completed" }
                    }
                    Err(e) => {
                        jobs2.fail(&job_id2, e);
                        "failed"
                    }
                };
                let _ = app2.emit(
                    "coding:job",
                    json!({ "session_id": sid, "job_id": job_id2, "status": status }),
                );
            });
            return ToolResult::standard_success(
                &format!(
                    "已在后台启动子任务 {job_id}：{task}\n\
                     继续你手上的工作——它跑完会把结果主动送回给你。\
                     若某个后续步骤确实依赖这个结果，再用 work_job 取回。"
                ),
                Some(json!({ "job_id": job_id, "background": true })),
            );
        }

        let request = SubagentRequest {
            parent_session_id: context.session_id.clone(),
            task,
            context: ctx_text,
            tools,
            max_rounds,
            depth: context.subagent_depth,
            working_directory: context.working_directory.clone(),
            access_level: context
                .access_level
                .clone()
                .unwrap_or(AgentAccessLevel::FsWrite),
            char_id: context.char_id.clone(),
            cancel: None,
        };

        match run_subagent(&router, &tool_system, Some(&app), request).await {
            Ok(outcome) => {
                let mut text = outcome.output.clone();
                match outcome.stop {
                    SubagentStop::MaxRounds => text.push_str(&format!(
                        "\n\n（子 agent 用尽 {} 轮预算仍未收尾，以上为它在停止前的最后产出）",
                        outcome.rounds
                    )),
                    SubagentStop::Canceled => {
                        text.push_str("\n\n（该子任务已被取消）");
                    }
                    SubagentStop::Completed => {}
                }
                ToolResult::standard_success(
                    &text,
                    Some(json!({
                        "rounds": outcome.rounds,
                        "tool_calls": outcome.tool_calls,
                        "completed": outcome.stop == SubagentStop::Completed,
                    })),
                )
            }
            Err(e) => ToolResult::standard_error(&e, Some("SubagentError"), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Shell
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "work delegate subagent subtask background spawn child"
    }
}

#[async_trait]
impl Tool for WorkJobTool {
    fn name(&self) -> &str {
        "work_job"
    }

    fn description(&self) -> &str {
        DESC_JOB_EN
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => DESC_JOB_ZH,
            "ja" => DESC_JOB_JA,
            _ => DESC_JOB_EN,
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "后台跑着\n任务进度\n看看后台",
            "en" => "run it in the background\ncheck job progress\nbackground status",
            "ja" => "バックグラウンドで実行\nジョブの進捗\n裏の状況",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        Self::schema_for("en")
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        Self::schema_for(lang)
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(action, "output" | "cancel" | "list") {
            return ValidationResult::failure(
                "action 必须是 output / cancel / list 之一",
                2,
            );
        }
        if action != "list" && input.get("job_id").and_then(|v| v.as_str()).is_none() {
            return ValidationResult::failure("output / cancel 需要提供 job_id", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let jobs = global_work_job_registry();
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
        match action {
            "list" => {
                let list = jobs.list_for(&context.session_id);
                if list.is_empty() {
                    return ToolResult::standard_success(
                        "本会话没有后台子任务。",
                        Some(json!({ "jobs": [] })),
                    );
                }
                let lines: Vec<String> = list
                    .iter()
                    .map(|j| {
                        format!(
                            "{} [{}] {}（{} 轮 / {} 次工具调用）",
                            j.job_id,
                            j.status.as_str(),
                            truncate_for_display(&j.task, 60),
                            j.rounds,
                            j.tool_calls
                        )
                    })
                    .collect();
                ToolResult::standard_success(
                    &format!("本会话共 {} 个后台子任务：\n{}", list.len(), lines.join("\n")),
                    Some(json!({ "jobs": list })),
                )
            }
            "cancel" => {
                let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                if jobs.cancel(job_id) {
                    ToolResult::standard_success(
                        &format!("已取消后台子任务 {job_id}，其执行会在下一个轮次边界停下。"),
                        Some(json!({ "job_id": job_id, "canceled": true })),
                    )
                } else {
                    ToolResult::standard_error(
                        &format!("无法取消 {job_id}：任务不存在，或已经结束。"),
                        Some("JobNotRunning"),
                        None,
                    )
                }
            }
            _ => {
                // output
                let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                match jobs.get(job_id) {
                    None => ToolResult::standard_error(
                        &format!("没有找到任务 {job_id}（任务号来自 work_delegate 的返回值）。"),
                        Some("JobNotFound"),
                        None,
                    ),
                    // 仍在跑：直接说明，不阻塞——模型可以继续干活，结算会自己送上门
                    Some(job) if job.status == WorkJobStatus::Running => ToolResult::standard_success(
                        &format!(
                            "任务 {job_id} 仍在运行（{}）。它跑完会把结果主动送回给你；\
                             若你下一步必须依赖这个结果，稍后再调用一次本工具。",
                            truncate_for_display(&job.task, 60)
                        ),
                        Some(json!({ "job_id": job_id, "status": "running" })),
                    ),
                    Some(job) if job.status == WorkJobStatus::Completed => {
                        let mut text = job.output.clone().unwrap_or_default();
                        if job.budget_exhausted {
                            text.push_str(
                                "\n\n（该任务用尽轮次预算才停下，结果可能不完整）",
                            );
                        }
                        ToolResult::standard_success(
                            &text,
                            Some(json!({
                                "job_id": job_id,
                                "status": "completed",
                                "rounds": job.rounds,
                                "tool_calls": job.tool_calls,
                            })),
                        )
                    }
                    Some(job) => ToolResult::standard_error(
                        &format!(
                            "任务 {job_id} 未能产出结果（状态：{}）{}",
                            job.status.as_str(),
                            job.error
                                .as_deref()
                                .map(|e| format!("，原因：{e}"))
                                .unwrap_or_default()
                        ),
                        Some("JobNotCompleted"),
                        None,
                    ),
                }
            }
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "work job background subagent output cancel list collect"
    }
}

/// 列表展示用的任务描述截断。
fn truncate_for_display(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    format!("{}…", chars[..max].iter().collect::<String>())
}

impl WorkDelegateTool {
    fn schema_for(lang: &str) -> Value {
        let (task, ctx, tools, rounds, bg) = match lang {
            "zh" => (
                "子任务描述。要自包含：目标 + 相关路径 + 约束。",
                "子 agent 看不到主会话，把它需要知道的背景一次写清。",
                "额外开放的工具名（在默认只读探索工具之外）。",
                "轮次预算（默认 12，上限 24）。",
                "true = 派到后台并立即返回任务号，主流程继续；false（默认）= 等待结果。",
            ),
            "ja" => (
                "サブタスクの説明。目標・関連パス・制約を含めて自己完結させてください。",
                "サブエージェントはメイン会話を見られません。必要な背景をここに書いてください。",
                "追加で許可するツール名（既定の読み取り専用探索ツールに加えて）。",
                "ラウンド予算（既定 12、上限 24）。",
                "true = バックグラウンドに送り即座にジョブ ID を返します。false（既定）= 結果を待ちます。",
            ),
            _ => (
                "The subtask. Make it self-contained: goal, relevant paths, constraints.",
                "The subagent cannot see your conversation — write every piece of background it needs.",
                "Extra tool names to allow on top of the default read-only exploration set.",
                "Round budget for the child (default 12, hard cap 24).",
                "true = run in the background and return a job id immediately; false (default) = wait for the result.",
            ),
        };
        json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": task },
                "context": { "type": "string", "description": ctx },
                "tools": {
                    "type": "array",
                    "description": tools,
                    "items": { "type": "string" }
                },
                "max_rounds": { "type": "integer", "description": rounds, "minimum": 1 },
                "background": { "type": "boolean", "description": bg }
            },
            "required": ["task"]
        })
    }
}

impl WorkJobTool {
    fn schema_for(lang: &str) -> Value {
        let (action, job_id) = match lang {
            "zh" => (
                "output = 取回结果；cancel = 放弃任务；list = 列出本会话所有任务。",
                "任务号（work_delegate 后台模式返回）；list 时不需要。",
            ),
            "ja" => (
                "output = 結果を回収；cancel = 破棄；list = セッション内の全ジョブを一覧。",
                "ジョブ ID（work_delegate のバックグラウンド実行で返る）。list では不要。",
            ),
            _ => (
                "output = collect the result; cancel = abandon the job; list = show every job in this session.",
                "The job id returned by a background work_delegate; not needed for list.",
            ),
        };
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["output", "cancel", "list"],
                    "description": action
                },
                "job_id": { "type": "string", "description": job_id }
            },
            "required": ["action"]
        })
    }
}
