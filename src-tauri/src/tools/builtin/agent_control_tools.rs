//! 多智能体协作工具：队友管理、团队任务板、消息投递、目标迭代。
//!
//! 设计前提：Vivian 的「队友」就是后台子 agent（`work_jobs` 注册表里的一条任务），
//! 没有独立的进程模型。所以这一层只做三件事：
//! - **看见**：`list_agents` / `wait_agent` 读注册表
//! - **干预**：`interrupt_agent` 置协作式取消位；`send_message` 投递到轮次边界收件箱
//! - **协作**：`spawn_teammate` 复用 `work_delegate` 的派发路径；`team_task_*` 维护
//!   一张只在会话内可见的任务板，用来协调"谁做哪块"
//!
//! 刻意不做：跨进程广播、队友之间直接对话。子 agent 只有「最终文本回传 + 轮次边界收信」
//! 两条通道，多一条都不该假装有。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::brain::work_jobs::{global_work_job_registry, push_agent_message, WorkJobStatus};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

// ============================================================================
// 团队任务板（会话内）
// ============================================================================

#[derive(Debug, Clone)]
struct TeamTask {
    id: String,
    title: String,
    detail: String,
    owner: String,
    status: String,
    job_id: Option<String>,
}

/// 队友名字 → job_id（按会话分桶）。
///
/// 为什么单独存一份：`send_message` 要支持「按名字发给队友」——子 agent 之间互相
/// 传话时手里只有对方的名字，没有 job_id。任务板上的 `owner` 是同一份事实，
/// 但那是给人看的列表，这里要的是 O(1) 反查。
fn teammate_registry() -> &'static Mutex<HashMap<String, HashMap<String, String>>> {
    static REG: OnceLock<Mutex<HashMap<String, HashMap<String, String>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_teammate(session_id: &str, name: &str, job_id: &str) {
    let mut reg = teammate_registry().lock().unwrap_or_else(|e| e.into_inner());
    reg.entry(session_id.to_string())
        .or_default()
        .insert(name.to_string(), job_id.to_string());
}

fn resolve_teammate(session_id: &str, name: &str) -> Option<String> {
    teammate_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .and_then(|m| m.get(name).cloned())
}

fn teammate_names(session_id: &str) -> Vec<String> {
    teammate_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn teammate_of_job(session_id: &str, job_id: &str) -> Option<String> {
    teammate_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .and_then(|m| {
            m.iter()
                .find(|(_, v)| v.as_str() == job_id)
                .map(|(k, _)| k.clone())
        })
}

fn team_board() -> &'static Mutex<HashMap<String, Vec<TeamTask>>> {
    static BOARD: OnceLock<Mutex<HashMap<String, Vec<TeamTask>>>> = OnceLock::new();
    BOARD.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_task_id(board: &[TeamTask]) -> String {
    let n = board.len() + 1;
    format!("t{n}")
}

fn render_tasks(tasks: &[TeamTask]) -> String {
    if tasks.is_empty() {
        return "团队任务板为空。".to_string();
    }
    let mut out = String::new();
    for t in tasks {
        let owner = if t.owner.is_empty() { "未指派" } else { &t.owner };
        let job = t
            .job_id
            .as_deref()
            .map(|j| format!(" job={j}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- [{}] {} ({}) {}{}\n",
            t.id, t.title, t.status, owner, job
        ));
        if !t.detail.trim().is_empty() {
            out.push_str(&format!("    {}\n", t.detail.trim()));
        }
    }
    out
}

// ============================================================================
// list_agents
// ============================================================================

pub struct ListAgentsTool;

impl ListAgentsTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for ListAgentsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ListAgentsTool {
    fn name(&self) -> &str {
        "list_agents"
    }

    fn description(&self) -> &str {
        "List the sub-agents (background jobs) this session has spawned, with status, rounds and tool-call counts. \
         Use it before dispatching more work: it shows what is still running, what already finished, and the job ids \
         you need for wait_agent / interrupt_agent / send_message."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "列出本会话派出的子 agent（后台任务）：状态、轮次、工具调用次数。\
                     再派活之前先看一眼——谁还在跑、谁已经结束、以及 wait_agent / interrupt_agent / \
                     send_message 要用的 job_id 都在这里。",
            "ja" => "このセッションが派遣したサブエージェント（バックグラウンドタスク）の一覧。状態・ラウンド数・ツール呼び出し回数。\
                     追加で仕事を振る前に確認する。まだ動いているもの・終わったもの・job_id が分かる。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "include_finished": { "type": "boolean", "description": "是否包含已结束的任务（默认 true）" }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let include_finished = args
            .get("include_finished")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let jobs = global_work_job_registry().list_for(&ctx.session_id);
        let rows: Vec<_> = jobs
            .iter()
            .filter(|j| include_finished || !j.status.is_terminal())
            .collect();
        if rows.is_empty() {
            return ToolResult::standard_success(
                "本会话没有派发过子 agent。用 work_delegate（background=true）或 spawn_teammate 派活。",
                Some(json!({"agents": [], "count": 0})),
            );
        }
        let mut text = format!("本会话的子 agent：{} 个\n", rows.len());
        let mut payload = Vec::new();
        for j in &rows {
            let task: String = j.task.chars().take(60).collect();
            let alias = teammate_of_job(&ctx.session_id, &j.job_id)
                .map(|n| format!("（队友 {n}）"))
                .unwrap_or_default();
            text.push_str(&format!(
                "- {} [{}]{} 轮次={} 工具={} — {}\n",
                j.job_id,
                j.status.as_str(),
                alias,
                j.rounds,
                j.tool_calls,
                task
            ));
            payload.push(json!({
                "job_id": j.job_id,
                "status": j.status.as_str(),
                "task": j.task,
                "rounds": j.rounds,
                "tool_calls": j.tool_calls,
                "budget_exhausted": j.budget_exhausted,
                "output_preview": j.output.as_ref().map(|o| o.chars().take(200).collect::<String>()),
                "error": j.error,
            }));
        }
        ToolResult::standard_success(&text, Some(json!({"agents": payload, "count": rows.len()})))
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "list agents subagents teammates 队友 子任务 并行 谁在跑"
    }
}

// ============================================================================
// interrupt_agent
// ============================================================================

pub struct InterruptAgentTool;

impl InterruptAgentTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for InterruptAgentTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for InterruptAgentTool {
    fn name(&self) -> &str {
        "interrupt_agent"
    }

    fn description(&self) -> &str {
        "Ask a running sub-agent to stop. Cooperative: it checks the flag at its next round boundary, \
         so a tool call already in flight finishes instead of being cut mid-write. \
         Use it when the sub-agent is clearly off track — otherwise send_message is gentler."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "让一个正在跑的子 agent 停下。**协作式**：它在下一个轮次边界检查标志，\
                     已经发出的工具调用会跑完，不会被从中间切断（避免半截写入）。\
                     发现它明显跑偏时才用；只是要点补充就用 send_message，更温和。",
            "ja" => "実行中のサブエージェントに停止を要求する。**協調的**：次のラウンド境界でフラグを見るため、\
                     実行中のツール呼び出しは完了する（書き込みの途中切断を避ける）。\
                     明らかに逸脱している時に使い、補足なら send_message のほうが穏やか。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "要中断的任务号（list_agents 里取）" }
            },
            "required": ["job_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("job_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("job_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let registry = global_work_job_registry();
        match registry.get(job_id) {
            None => ToolResult::standard_error(
                &format!("找不到任务 {job_id}。用 list_agents 核对任务号。"),
                Some("SubagentNotFound"),
                None,
            ),
            Some(job) if job.session_id != ctx.session_id => ToolResult::standard_error(
                "该任务不属于本会话，不能中断。",
                Some("PermissionDenied"),
                None,
            ),
            Some(job) if job.status.is_terminal() => ToolResult::standard_success(
                &format!("任务 {job_id} 已经是终态（{}），无需中断。", job.status.as_str()),
                Some(json!({"job_id": job_id, "status": job.status.as_str()})),
            ),
            Some(_) => {
                let ok = registry.cancel(job_id);
                if ok {
                    ToolResult::standard_success(
                        &format!("已请求 {job_id} 在下一个轮次边界停止。它会把已经完成的部分作为结果回传。"),
                        Some(json!({"job_id": job_id, "interrupt_requested": true})),
                    )
                } else {
                    ToolResult::standard_error("中断请求未被接受（任务可能刚刚结束）。", None, None)
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

    fn search_hint(&self) -> &str {
        "interrupt cancel stop agent 中断 停止 取消子任务"
    }
}

// ============================================================================
// send_message
// ============================================================================

pub struct SendMessageTool;

impl SendMessageTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for SendMessageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn description(&self) -> &str {
        "Send a message to a running agent — a teammate you spawned, or a sibling you discovered via list_agents. \
         Address it by teammate name or job id. It is not interrupted: the message is queued and delivered at its \
         next round boundary, as an additional instruction with priority over the original task. \
         Use it to correct course, add a constraint, hand over something you just learned, or tell a peer \
         that a shared assumption changed."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "给正在跑的 agent 发消息：你派出的队友，或 list_agents 里发现的同伴。                     收件人写队友名字或 job_id 都行。**不打断它**：消息先进队列，它在下一个轮次边界取走，                     作为补充指示生效，优先级高于原始任务。用来纠偏、补约束、递新信息，                     或者通知同伴某个共同前提变了。",
            "ja" => "実行中のサブエージェントにメッセージを送る。**中断はしない**：キューに入り、次のラウンド境界で\
                     追加指示として届く（元のタスクより優先）。軌道修正や制約の追加、新しい情報の受け渡しに使う。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "收件人：队友名字，或 job_id（list_agents / team_task_list 里都有）" },
                "job_id": { "type": "string", "description": "（兼容写法）等同于 to" },
                "message": { "type": "string", "description": "要传达的内容：一句话说清要求或修正，不要寒暄" }
            },
            "required": ["message"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let has_target = ["to", "job_id"].iter().any(|k| {
            input
                .get(*k)
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
        });
        if !has_target {
            return ValidationResult::failure("to（或 job_id）不能为空", 2);
        }
        if input
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return ValidationResult::failure("message 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let raw_to = args
            .get("to")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("job_id").and_then(|v| v.as_str()))
            .unwrap_or("")
            .trim()
            .to_string();
        let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("").trim();
        let registry = global_work_job_registry();
        // 先按 job_id 精确匹配，再按队友名字反查——子 agent 之间传话时手里只有名字。
        let job_id = if registry.get(&raw_to).is_some() {
            raw_to.clone()
        } else {
            match resolve_teammate(&ctx.session_id, &raw_to) {
                Some(id) => id,
                None => {
                    let known = teammate_names(&ctx.session_id);
                    return ToolResult::standard_error(
                        &format!(
                            "找不到收件人「{raw_to}」。本会话的队友：{}。也可以直接传 job_id（list_agents 里有）。",
                            if known.is_empty() {
                                "（还没有派过队友）".to_string()
                            } else {
                                known.join(" / ")
                            }
                        ),
                        Some("SubagentNotFound"),
                        None,
                    );
                }
            }
        };
        let job_id = job_id.as_str();
        if let Some(job) = registry.get(job_id) {
            if job.session_id != ctx.session_id {
                return ToolResult::standard_error(
                    "该任务不属于本会话。",
                    Some("PermissionDenied"),
                    None,
                );
            }
        }
        match push_agent_message(job_id, message) {
            Ok(()) => ToolResult::standard_success(
                &format!("已投递到 {job_id} 的收件箱，它会在下一个轮次边界读到。"),
                Some(json!({"job_id": job_id, "delivered": "next_round_boundary"})),
            ),
            Err(e) => ToolResult::standard_error(&e, Some("SubagentUnavailable"), None),
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

    fn search_hint(&self) -> &str {
        "send message to agent 传话 通知子任务 补充说明"
    }
}

// ============================================================================
// wait_agent
// ============================================================================

pub struct WaitAgentTool;

impl WaitAgentTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for WaitAgentTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WaitAgentTool {
    fn name(&self) -> &str {
        "wait_agent"
    }

    fn description(&self) -> &str {
        "Block until a sub-agent finishes (or the timeout elapses) and return its final text. \
         Only use it when your next step truly depends on that result — otherwise keep working and let the \
         settlement arrive by itself."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "等一个子 agent 跑完（或等到超时），返回它的最终文本。\
                     只有下一步**确实依赖**这个结果时才用；否则继续干自己的活，结算会自动送回来。",
            "ja" => "サブエージェントの完了（またはタイムアウト）まで待ち、最終テキストを返す。\
                     次の手順がその結果に本当に依存する場合のみ使う。そうでなければ自分の作業を続ければ、結果は自動で届く。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "要等待的任务号" },
                "timeout_secs": { "type": "integer", "description": "最长等待秒数（默认 60，上限 600）", "minimum": 5 }
            },
            "required": ["job_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("job_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("job_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let timeout = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60)
            .clamp(5, 600);
        let registry = global_work_job_registry();
        let Some(job) = registry.get(&job_id) else {
            return ToolResult::standard_error(
                &format!("找不到任务 {job_id}。用 list_agents 核对任务号。"),
                Some("SubagentNotFound"),
                None,
            );
        };
        if job.session_id != ctx.session_id {
            return ToolResult::standard_error("该任务不属于本会话。", Some("PermissionDenied"), None);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
        loop {
            if let Some(j) = registry.get(&job_id) {
                if j.status.is_terminal() {
                    let body = j
                        .output
                        .clone()
                        .filter(|o| !o.trim().is_empty())
                        .or_else(|| j.error.clone())
                        .unwrap_or_else(|| "（任务结束但没有输出文本）".to_string());
                    return ToolResult::standard_success(
                        &format!("任务 {job_id} 已结束（{}）：\n{}", j.status.as_str(), body),
                        Some(json!({
                            "job_id": job_id,
                            "status": j.status.as_str(),
                            "output": j.output,
                            "error": j.error,
                            "rounds": j.rounds,
                            "tool_calls": j.tool_calls,
                            "budget_exhausted": j.budget_exhausted,
                        })),
                    );
                }
            } else {
                return ToolResult::standard_error("任务已从注册表移除（可能已过期清理）。", None, None);
            }
            if std::time::Instant::now() >= deadline {
                return ToolResult::standard_success(
                    &format!("等待 {timeout} 秒后任务 {job_id} 仍在运行。可以稍后再 wait_agent，或继续做别的。"),
                    Some(json!({"job_id": job_id, "status": "running", "timed_out": true})),
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "wait agent join 等待 子任务结果 阻塞"
    }
}

// ============================================================================
// list_subagent_models
// ============================================================================

pub struct ListSubagentModelsTool;

impl ListSubagentModelsTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for ListSubagentModelsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ListSubagentModelsTool {
    fn name(&self) -> &str {
        "list_subagent_models"
    }

    fn description(&self) -> &str {
        "Report which model sub-agents run on. Sub-agents inherit the work-agent model route; this tool tells you \
         the currently configured one so you can judge whether delegating is worth it (a small model may not handle \
         a subtle refactor)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "报告子 agent 用的是哪个模型。子 agent 继承工作智能体的模型路由——\
                     知道当前配置才能判断「这块活值不值得派下去」（小模型未必扛得住需要判断力的重构）。",
            "ja" => "サブエージェントが使うモデルを報告する。サブエージェントは作業エージェントのモデル経路を継承する。\
                     現在の設定を知ることで、委譲する価値があるか判断できる。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "required": []})
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, _ctx: &ToolUseContext) -> ToolResult {
        // 不假装能查到"具体是哪个模型"：路由由用户配置决定，工具层只看得到约定。
        // 能确定的是三件事，而且这三件正好是判断"值不值得派"的依据。
        let text = format!(
            "子 agent 的模型路由：继承工作智能体（task = `{}`）。\n\
             默认轮次预算：{}；最大 {}。\n\
             默认只读白名单：{}。\n\
             判断委派是否划算看两条：需要长链推理/跨文件重构的活，小模型容易半途给出\"看起来完成\"的结论；\
             纯检索、批量改写、按明确规范产出这类活，派下去最划算。",
            crate::providers::base::TASK_WORK_AGENT,
            crate::brain::coding_subagent::SUBAGENT_DEFAULT_ROUNDS,
            crate::brain::coding_subagent::SUBAGENT_MAX_ROUNDS,
            crate::brain::coding_subagent::SUBAGENT_DEFAULT_TOOLS.join(" / "),
        );
        ToolResult::standard_success(
            &text,
            Some(json!({
                "task": crate::providers::base::TASK_WORK_AGENT,
                "default_rounds": crate::brain::coding_subagent::SUBAGENT_DEFAULT_ROUNDS,
                "max_rounds": crate::brain::coding_subagent::SUBAGENT_MAX_ROUNDS,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "subagent model 子任务用什么模型 路由"
    }
}

// ============================================================================
// spawn_teammate
// ============================================================================

pub struct SpawnTeammateTool;

impl SpawnTeammateTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for SpawnTeammateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SpawnTeammateTool {
    fn name(&self) -> &str {
        "spawn_teammate"
    }

    fn description(&self) -> &str {
        "Spawn a named teammate: a background sub-agent with a role framing, tracked on the team task board. \
         Unlike work_delegate this records who owns which task, so several streams can be coordinated \
         (list via team_task_list, nudge via send_message, stop via interrupt_agent).\n\
         The teammate still has no memory of this conversation — the task text must be self-contained."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "派一个**有名字的队友**：带角色设定、跑在后台的子 agent，并登记到团队任务板上。\
                     与 work_delegate 的区别是它记「谁负责哪块」，多条线并行时才好协调\
                     （team_task_list 看板、send_message 传话、interrupt_agent 叫停）。\n\
                     队友依然看不到这段对话——task 必须自包含。",
            "ja" => "名前付きのチームメイトを派遣する：役割を与えたバックグラウンドのサブエージェントで、チーム掲示板に登録される。\
                     work_delegate との違いは「誰が何を担当するか」を記録する点（team_task_list / send_message / interrupt_agent で調整できる）。\
                     会話は見えないので task は自己完結で書くこと。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "队友名字（如 tester / doc-writer），用于看板与后续指令" },
                "role": { "type": "string", "description": "一句话角色定位：它负责什么、不负责什么" },
                "task": { "type": "string", "description": "自包含的任务描述（队友看不到当前对话）" },
                "context": { "type": "string", "description": "可选：必要的背景与约束" },
                "max_rounds": { "type": "integer", "description": "轮次预算（默认继承 work_delegate 默认值）" }
            },
            "required": ["name", "task"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        for key in ["name", "task"] {
            if input.get(key).and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
                return ValidationResult::failure(&format!("{key} 不能为空"), 2);
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let role = args.get("role").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let max_rounds = args.get("max_rounds").and_then(|v| v.as_u64());

        let framed_context = if role.is_empty() {
            context.clone()
        } else {
            format!("# 你的角色\n你是团队里的「{name}」：{role}\n\n{context}")
        };

        // 复用 work_delegate 的派发路径：不重复实现 router / 工作区 / 白名单解析。
        let delegate = crate::tools::builtin::work_subagent_tools::WorkDelegateTool::new();
        let mut payload = json!({
            "task": format!("[{name}] {task}"),
            "context": framed_context,
            "background": true,
        });
        if let Some(n) = max_rounds {
            payload["max_rounds"] = json!(n);
        }
        let result = delegate.call(payload, ctx).await;
        if !result.success {
            return result;
        }
        let job_id = result
            .data
            .as_ref()
            .and_then(|d| d.get("job_id").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        if let Some(j) = &job_id {
            register_teammate(&ctx.session_id, &name, j);
        }

        let task_id = {
            let mut board = team_board().lock().unwrap_or_else(|e| e.into_inner());
            let entry = board.entry(ctx.session_id.clone()).or_default();
            let id = next_task_id(entry);
            entry.push(TeamTask {
                id: id.clone(),
                title: task.chars().take(60).collect(),
                detail: role.clone(),
                owner: name.clone(),
                status: "running".to_string(),
                job_id: job_id.clone(),
            });
            id
        };

        let mut text = format!("已派出队友「{name}」（看板条目 {task_id}）");
        if let Some(j) = &job_id {
            text.push_str(&format!("，任务号 {j}"));
        }
        text.push_str("。它会跑完把结果送回；要看进度用 list_agents / team_task_list。");
        ToolResult::standard_success(
            &text,
            Some(json!({"teammate": name, "task_id": task_id, "job_id": job_id})),
        )
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

    fn search_hint(&self) -> &str {
        "spawn teammate 队友 并行 分工 团队"
    }
}

// ============================================================================
// team_task_* 四件套
// ============================================================================

pub struct TeamTaskCreateTool;

impl TeamTaskCreateTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TeamTaskCreateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TeamTaskCreateTool {
    fn name(&self) -> &str {
        "team_task_create"
    }

    fn description(&self) -> &str {
        "Create an entry on the team task board (no execution happens — this is the plan, spawn_teammate is the doing). \
         Useful when you need to state ownership before dispatching, or when the user wants to see the split."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在团队任务板上创建一条（**不执行**，只是计划；真正派活是 spawn_teammate）。\
                     需要在派活前先讲清分工、或者用户想看到拆分时用。",
            "ja" => "チーム掲示板に項目を作る（**実行はしない**、計画のみ。実際の派遣は spawn_teammate）。\
                     分担を先に明示したいとき、ユーザーに分割を見せたいときに使う。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "任务标题（一句话）" },
                "detail": { "type": "string", "description": "可选：补充说明与验收标准" },
                "owner": { "type": "string", "description": "可选：指派给哪个队友" }
            },
            "required": ["title"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("title 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let detail = args.get("detail").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let owner = args.get("owner").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let id = {
            let mut board = team_board().lock().unwrap_or_else(|e| e.into_inner());
            let entry = board.entry(ctx.session_id.clone()).or_default();
            let id = next_task_id(entry);
            entry.push(TeamTask {
                id: id.clone(),
                title,
                detail,
                owner,
                status: "pending".to_string(),
                job_id: None,
            });
            id
        };
        ToolResult::standard_success(&format!("已创建团队任务 {id}。"), Some(json!({"task_id": id})))
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

    fn search_hint(&self) -> &str {
        "team task create 分工 计划 看板"
    }
}

pub struct TeamTaskListTool;

impl TeamTaskListTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TeamTaskListTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TeamTaskListTool {
    fn name(&self) -> &str {
        "team_task_list"
    }

    fn description(&self) -> &str {
        "List the team task board for this session (id, title, status, owner, job id)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "列出本会话的团队任务板（编号、标题、状态、负责人、任务号）。",
            "ja" => "このセッションのチーム掲示板を一覧表示する（ID・タイトル・状態・担当・ジョブID）。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "required": []})
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, ctx: &ToolUseContext) -> ToolResult {
        let board = team_board().lock().unwrap_or_else(|e| e.into_inner());
        let tasks = board.get(&ctx.session_id).cloned().unwrap_or_default();
        drop(board);
        let text = render_tasks(&tasks);
        let payload: Vec<Value> = tasks
            .iter()
            .map(|t| {
                json!({
                    "id": t.id, "title": t.title, "status": t.status,
                    "owner": t.owner, "job_id": t.job_id, "detail": t.detail
                })
            })
            .collect();
        ToolResult::standard_success(&text, Some(json!({"tasks": payload})))
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "team task list 看板 分工进度"
    }
}

pub struct TeamTaskGetTool;

impl TeamTaskGetTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TeamTaskGetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TeamTaskGetTool {
    fn name(&self) -> &str {
        "team_task_get"
    }

    fn description(&self) -> &str {
        "Read one team task by id, including its linked job status and output preview."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按编号读取一条团队任务，附带它关联的任务状态与输出预览。",
            "ja" => "ID を指定してチームタスクを 1 件読む。関連ジョブの状態と出力プレビュー付き。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "task_id": { "type": "string", "description": "团队任务编号（如 t1）" } },
            "required": ["task_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("task_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("task_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let task_id = args.get("task_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let board = team_board().lock().unwrap_or_else(|e| e.into_inner());
        let found = board
            .get(&ctx.session_id)
            .and_then(|v| v.iter().find(|t| t.id == task_id))
            .cloned();
        drop(board);
        let Some(task) = found else {
            return ToolResult::standard_error(
                &format!("没有编号为 {task_id} 的团队任务。用 team_task_list 看现有编号。"),
                Some("NotFound"),
                None,
            );
        };
        let job = task
            .job_id
            .as_deref()
            .and_then(|j| global_work_job_registry().get(j));
        let mut text = format!(
            "{} [{}] {}\n负责人：{}\n",
            task.id,
            task.status,
            task.title,
            if task.owner.is_empty() { "未指派" } else { &task.owner }
        );
        if !task.detail.trim().is_empty() {
            text.push_str(&format!("说明：{}\n", task.detail));
        }
        if let Some(j) = &job {
            text.push_str(&format!(
                "关联任务 {}：{}，轮次 {}，工具 {} 次\n",
                j.job_id,
                j.status.as_str(),
                j.rounds,
                j.tool_calls
            ));
            if let Some(o) = j.output.as_ref().filter(|o| !o.trim().is_empty()) {
                text.push_str(&format!("输出预览：{}\n", o.chars().take(400).collect::<String>()));
            }
        }
        ToolResult::standard_success(
            &text,
            Some(json!({
                "id": task.id, "title": task.title, "status": task.status,
                "owner": task.owner, "detail": task.detail, "job_id": task.job_id,
                "job_status": job.as_ref().map(|j| j.status.as_str()),
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "team task get 查看某条分工"
    }
}

pub struct TeamTaskUpdateTool;

impl TeamTaskUpdateTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TeamTaskUpdateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TeamTaskUpdateTool {
    fn name(&self) -> &str {
        "team_task_update"
    }

    fn description(&self) -> &str {
        "Update a team task's status / owner / detail. Status values: pending / running / done / blocked. \
         Keep it honest — mark blocked when it is blocked, not when you are unsure."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "更新团队任务的状态 / 负责人 / 说明。状态取值：pending / running / done / blocked。\
                     如实标注——卡住就是 blocked，不要用「不确定」混过去。",
            "ja" => "チームタスクの状態・担当・説明を更新する。状態：pending / running / done / blocked。\
                     正直に付けること——詰まっているなら blocked。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "团队任务编号" },
                "status": { "type": "string", "enum": ["pending", "running", "done", "blocked"], "description": "新状态" },
                "owner": { "type": "string", "description": "新负责人" },
                "detail": { "type": "string", "description": "替换说明" }
            },
            "required": ["task_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("task_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("task_id 不能为空", 2);
        }
        if let Some(s) = input.get("status").and_then(|v| v.as_str()) {
            if !matches!(s, "pending" | "running" | "done" | "blocked") {
                return ValidationResult::failure("status 取值必须是 pending / running / done / blocked", 2);
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let task_id = args.get("task_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let mut board = team_board().lock().unwrap_or_else(|e| e.into_inner());
        let entry = board.entry(ctx.session_id.clone()).or_default();
        let Some(task) = entry.iter_mut().find(|t| t.id == task_id) else {
            return ToolResult::standard_error(
                &format!("没有编号为 {task_id} 的团队任务。"),
                Some("NotFound"),
                None,
            );
        };
        if let Some(s) = args.get("status").and_then(|v| v.as_str()) {
            task.status = s.to_string();
        }
        if let Some(o) = args.get("owner").and_then(|v| v.as_str()) {
            task.owner = o.to_string();
        }
        if let Some(d) = args.get("detail").and_then(|v| v.as_str()) {
            task.detail = d.to_string();
        }
        let snapshot = task.clone();
        drop(board);
        ToolResult::standard_success(
            &format!("已更新 {} → [{}] {}", snapshot.id, snapshot.status, snapshot.title),
            Some(json!({
                "id": snapshot.id, "status": snapshot.status,
                "owner": snapshot.owner, "detail": snapshot.detail
            })),
        )
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

    fn search_hint(&self) -> &str {
        "team task update 改状态 标记完成 阻塞"
    }
}

// ============================================================================
// ralph
// ============================================================================

pub struct RalphTool;

impl RalphTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for RalphTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RalphTool {
    fn name(&self) -> &str {
        "ralph"
    }

    fn description(&self) -> &str {
        "Iterate a fresh sub-agent around one immutable goal until it converges. Each iteration starts with an \
         empty context (it only sees the goal plus the previous iteration's result), which is exactly what you want \
         when the work is 'keep improving until it passes' and a long transcript would just anchor it on past attempts.\n\
         Stops early on an empty or 'no further changes' reply. Expensive: use it for polish loops, not exploration."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "围绕**一个不可变目标**反复迭代：每轮都起一个全新上下文的子 agent（它只看到目标 + 上一轮结果）。\
                     适用于「一直改到过为止」这类工作——长对话历史反而会把新尝试锚定在旧结论上。\
                     某轮返回空或「没有可改的了」就提前收敛。代价高：用于打磨，不用于探索。",
            "ja" => "**不変の目標**を中心に、毎回まっさらなサブエージェントで反復する（見えるのは目標と前回の結果だけ）。\
                     「通るまで直す」タイプの作業に適する——長い履歴は新しい試みを過去の結論に縛りつける。\
                     空応答や「これ以上変更なし」で早期終了。高コストなので探索ではなく仕上げに使う。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string", "description": "不可变目标：说清「什么算达成」，每一轮都原样传下去" },
                "iterations": { "type": "integer", "description": "最多迭代几次（默认 3，上限 5）", "minimum": 1 },
                "context": { "type": "string", "description": "可选：每轮都要知道的背景（项目约定、验证命令）" }
            },
            "required": ["goal"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("goal").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("goal 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let goal = args.get("goal").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let iterations = args
            .get("iterations")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(1, 5);
        let base_context = args.get("context").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let delegate = crate::tools::builtin::work_subagent_tools::WorkDelegateTool::new();
        let mut previous = String::new();
        let mut log = String::new();
        for i in 1..=iterations {
            let context = if i == 1 {
                base_context.clone()
            } else {
                format!(
                    "{base_context}\n\n# 上一轮的结果（第 {} 轮）\n{}\n\n# 你的任务\n站在上一轮的结果上继续推进；\
                     如果已经达成目标，直接回复「已完成，无需改动」并说明依据。",
                    i - 1,
                    previous.chars().take(4000).collect::<String>()
                )
            };
            let payload = json!({
                "task": goal.clone(),
                "context": context,
                "background": false,
            });
            let result = delegate.call(payload, ctx).await;
            let text = result
                .data
                .as_ref()
                .and_then(|d| d.get("output").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    result
                        .data
                        .as_ref()
                        .map(|d| d.to_string())
                        .unwrap_or_default()
                });
            log.push_str(&format!(
                "\n## 第 {i} 轮\n{}\n",
                text.chars().take(600).collect::<String>()
            ));
            if text.trim().is_empty() || text.contains("已完成，无需改动") {
                break;
            }
            previous = text;
        }
        ToolResult::standard_success(
            &format!("ralph 迭代完成（目标：{goal}）{log}"),
            Some(json!({"goal": goal, "iterations": iterations, "last": previous})),
        )
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

    fn search_hint(&self) -> &str {
        "ralph iterate loop until done 反复打磨 迭代到通过"
    }
}

/// 供前端/测试读取状态用（未结束时为空）。
pub fn running_agents(session_id: &str) -> Vec<String> {
    global_work_job_registry()
        .list_for(session_id)
        .into_iter()
        .filter(|j| j.status == WorkJobStatus::Running)
        .map(|j| j.job_id)
        .collect()
}
