//! 工作智能体的子 agent 执行器。
//!
//! 主智能体可以把一个自包含的子任务派给子 agent：子 agent 有**自己独立的
//! 消息历史**，跑一个精简的工具循环，最后只把**最终文本**交回主 agent——
//! 中间的探索步骤不进主上下文——委派的价值恰恰在于"过程不外溢"。
//!
//! 两条硬约束（都靠 `ToolUseContext::subagent_depth` 传导）：
//! - **递归深度**：主 agent 为 0，最多再套 `SUBAGENT_MAX_DEPTH` 层
//! - **子 agent 不能向用户提问**：它面前没有人类应答者，`work_ask_user` 挂起
//!   等待会永久阻塞。被拒时模型应把未决问题写进最终结果带回父级

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tauri::Emitter;

use crate::brain::coding_agent::{coding_sandbox_confirm, summarize_result, execute_audited_tool_use};
use crate::providers::base::{LLMRequest, StreamEvent, ToolDefinition};
use crate::providers::router::ModelRouter;
use crate::tools::types::{AgentAccessLevel, ToolUseContext, WorkingDirectoryPermission};
use crate::tools::ToolSystem;
use crate::types::response::{ChatMessage, MessageToolCall};
use super::work_contract::{self, Evidence, TaskResult, TaskStatus};

/// 委派深度上限：主 agent 为 0，最多再套这么多层子 agent。
pub const SUBAGENT_MAX_DEPTH: usize = 2;
/// 子 agent 默认轮次预算（模型未指定时）。
pub const SUBAGENT_DEFAULT_ROUNDS: usize = 12;
/// 子 agent 轮次预算硬上限——子任务应该小而明确，不该是个无底洞。
pub const SUBAGENT_MAX_ROUNDS: usize = 24;
/// 子 agent 默认可用工具：只读探索 + 命令执行。
///
/// 刻意不含 `work_delegate`（默认不开委派，需要时显式要求）、`work_ask_user`
/// （子 agent 不能提问）、`work_todo_write`（默认不需要计划清单，需要时显式
/// 要求）、`notify_companion` / `send_image`（面向用户的播报归主 agent 所有）
/// 与能力进化工具（沉淀归主 agent 所有）。
pub const SUBAGENT_DEFAULT_TOOLS: &[&str] = &[
    "read_file",
    "grep_search",
    "list_dir",
    "run_command",
    "lsp_query",
    // 队友之间互相传话：只给「看同伴」和「递一句话」两条。
    // 委派 / 提问 / 结算仍归主 agent 所有——子 agent 面前没有人类应答者，
    // 但同伴是有的，所以这两条不违背上面那条边界。
    "list_agents",
    "send_message",
];

/// 子 agent 的终止原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentStop {
    /// 模型给出最终文本，正常完成。
    Completed,
    /// 用尽轮次预算（结果可能不完整，交给上层判断）。
    MaxRounds,
    /// 被叫停（后台任务被上层取消）——在轮次边界收手。
    Canceled,
}

/// 子 agent 的执行结果。
#[derive(Debug, Clone)]
pub struct SubagentOutcome {
    /// 最终文本——作为工具返回值交给主 agent。
    pub output: String,
    /// 实际消耗的轮次。
    pub rounds: usize,
    /// 累计工具调用次数。
    pub tool_calls: usize,
    pub stop: SubagentStop,
    pub result: TaskResult,
}

/// 启动一个子 agent 所需的全部信息。
pub struct SubagentRequest {
    /// 父会话 ID（用于事件广播与工具侧路由，不共享其消息历史）。
    pub parent_session_id: String,
    /// 子任务描述。
    pub task: String,
    /// 必要背景（子 agent 看不到父会话，缺的信息必须在这里交代）。
    pub context: Option<String>,
    /// 工具白名单（默认 `SUBAGENT_DEFAULT_TOOLS`）。
    pub tools: Vec<String>,
    /// 轮次预算。
    pub max_rounds: usize,
    /// 发起方的深度；子 agent 内执行的工具将看到 `depth + 1`。
    pub depth: usize,
    pub working_directory: String,
    /// 附加工作区（主工作区之外的授权目录，逐个带只读标记）。
    ///
    /// 由上层 `work_delegate` 决定：默认继承父会话的全部工作区，也可以只给一个子集，
    /// 甚至一个都不给（此时 `working_directory` 为空串，子 agent 进入无工作区模式）。
    pub extra_workspaces: Vec<WorkingDirectoryPermission>,
    /// 上层是否**显式限定**过工作区范围（而非默认继承）。
    ///
    /// 用于提示词：受限的子 agent 不该反复尝试越界路径，而应把「需要别处的信息」
    /// 写进最终回复交回上层处理。
    pub workspaces_restricted: bool,
    pub access_level: AgentAccessLevel,
    pub char_id: String,
    /// 协作式取消标志（后台任务才有）。置位后在下一个轮次边界收手。
    pub cancel: Option<Arc<AtomicBool>>,
    /// 后台任务号（与 `work_jobs` 注册表对应）。有值时父子之间多一条单向消息通道：
    /// 主 agent 用 `send_message` 投递，子 agent 在**轮次边界**取走——不打断正在跑的
    /// 工具调用，避免留下半截写入。
    pub job_id: Option<String>,
}

/// 把注册表里的工具 schema 转成 provider 侧定义（按白名单顺序输出，
/// 保证每轮 tools 参数字节一致，保住 API 前缀缓存）。
fn definitions_for(tool_system: &ToolSystem, allowed: &[String]) -> Vec<ToolDefinition> {
    tool_system.work_tool_schemas(&allowed.iter().map(String::as_str).collect::<Vec<_>>())
        .into_iter().map(|d| ToolDefinition { name: d.name, description: d.description, parameters: d.input_schema }).collect()
}

/// 子 agent 的 system prompt。
fn subagent_system_prompt(
    working_directory: &str,
    extra_workspaces: &[WorkingDirectoryPermission],
    workspaces_restricted: bool,
    depth: usize,
    max_rounds: usize,
) -> String {
    let has_wd = !working_directory.trim().is_empty();
    // 被上层限定过范围时，明确告诉它越界不会有结果——否则它会把轮次烧在反复被沙箱拒绝上，
    // 而这本来是可以直接汇报给上层的信息差。
    let scope = if !workspaces_restricted {
        ""
    } else if has_wd {
        "\n- 你的工作区范围由上层指定：只有上面列出的目录在授权范围内，文件工具的路径校验会拒绝\
         其它位置，不要尝试越界。如果任务需要别处的信息，在最终回复里写清「需要哪里的什么信息、\
         为什么需要」，由上层决定是否重新派发——不要用一堆失败的尝试凑答案。"
    } else {
        "\n- 上层没有给你任何工作区：本任务不绑定目录。你可以用绝对路径**读取**文件，\
         但**不能修改任何文件、也不能执行命令**——这类调用会被直接拒绝。\
         如果任务本来就需要写入或跑命令，不要反复尝试：在最终回复里说明需要做什么、为什么，\
         由上层决定是否换一种派发方式（给它一个工作区）。"
    };
    let env = if has_wd {
        let extras = if extra_workspaces.is_empty() {
            String::new()
        } else {
            let mut s = String::from("\n- 附加工作区（同样在授权范围内，需用绝对路径）：");
            for w in extra_workspaces {
                s.push_str(&format!(
                    "\n  - {}{}",
                    w.path,
                    if w.is_read_only { "（只读）" } else { "" }
                ));
            }
            s
        };
        format!(
            "- 主工作目录：{working_directory}（文件操作仅限该目录与下列附加工作区内）{extras}"
        )
    } else {
        "- 工作目录：未选择（文件操作使用绝对路径）".to_string()
    };
    let env = format!("{env}{scope}");
    // 联网能力随主进程状态变化（浏览器桥连没连），子 agent 同样要知道：
    // 它是被派去「摸清一个模块/查一批资料」的那一方，最容易在同一类取法上反复换变体。
    let env = format!("{env}{}", super::coding_agent::web_capability_section());
    let execution_contract = include_str!("../../prompts/work/execution.md");
    // 与主智能体一致：有工作目录走相对路径，没有才退回绝对路径
    let link_protocol = if has_wd {
        "提到主工作目录内的本地文件时使用可点击 Markdown 链接 \
         `[文件名 (line N)](相对主工作目录的路径:N)`：路径使用正斜杠、行号从 1 开始，\
         不要写盘符或绝对路径；提到附加工作区内的文件时给绝对路径（相对路径只能还原到主工作目录）。"
    } else {
        "提到本地文件时使用可点击 Markdown 链接 `[文件名 (line N)](绝对路径:N)`：\
         路径使用正斜杠、行号从 1 开始（本任务无工作目录，只能给绝对路径）。"
    };
    format!(
        "你是一个**子智能体**（subagent，深度 {depth}），由上层编程智能体派发一个自包含的子任务。\n\n\
         # 你的处境\n\
         - 你看不到上层会话的任何历史，只有下面「任务」与「背景」里的信息。缺什么就自己用工具查，不要凭猜测。\n\
         - 你的**最终回复文本**会作为返回值交给上层，中间过程不会进入上层的上下文。\
         所以要写得自包含：结论、关键发现、改了哪些文件、还有什么没做。\n\
         - 你**不能向用户提问**（没有人在等你回答）。常规实现选择可自行决定；缺少授权或关键需求时停止依赖该决定的工作并向上层报告，不替用户批准。做出常规选择后，\
         并在最终回复里写明「我选了 A 而不选 B，原因是…」，上层据此决定是否重新派发。\n\n\
         # 边界\n\
         - 只做被派发的事：不要顺手重构、不要扩大范围、不要改无关文件。\n\
         - 你有自己独立的工作待办清单，与上层的清单互不干扰。\n\n\
         # 环境\n\
         - 操作系统：Windows（命令用 PowerShell 语法）\n\
         {env}\n\
         - 轮次预算：{max_rounds} 轮工具调用\n\n\
         {execution_contract}\n\n\
         # 收尾\n\
         不需要再调用工具时，直接用自然语言给出最终结果。\
         {link_protocol}\
         若任务无法完成，说清卡在哪里、你已经确认了什么（这些对上层同样有价值）。"
    )
}

/// 跑一个子 agent 到结束，返回其最终文本。
///
/// 上下文与主 agent 完全隔离：这里只有 `[system, user(task+context), ...]`
/// 这一条独立消息链，执行过程中的工具调用与结果也只留在这条链里。
pub async fn run_subagent(
    router: &ModelRouter,
    tool_system: &ToolSystem,
    app: Option<&tauri::AppHandle>,
    req: SubagentRequest,
) -> Result<SubagentOutcome, String> {
    let mut context = super::coding_event_log::context(&req.parent_session_id);
    context.agent_id = Some(uuid::Uuid::new_v4().to_string());
    let mut span = super::coding_event_log::Span::start(Some(context.clone()), None, "subagent",
        serde_json::json!({"depth":req.depth + 1,"max_rounds":req.max_rounds}));
    let result = super::coding_event_log::scope(context, run_subagent_inner(router, tool_system, app, req)).await;
    span.finish(match &result {
        Ok(outcome) => serde_json::json!({"status":format!("{:?}",outcome.stop),"rounds":outcome.rounds,"tool_calls":outcome.tool_calls}),
        Err(error) => serde_json::json!({"status":"error","error":error}),
    });
    result
}
async fn wait_for_subagent_cancel(cancel: Option<&Arc<AtomicBool>>) {
    let Some(cancel) = cancel else { std::future::pending::<()>().await; return; };
    while !cancel.load(Ordering::SeqCst) { tokio::time::sleep(std::time::Duration::from_millis(200)).await; }
}
fn cancelled_outcome(text: &str, rounds: usize, tool_calls: usize, receipts: &[Evidence]) -> SubagentOutcome {
    SubagentOutcome { output: text.into(), rounds, tool_calls, stop: SubagentStop::Canceled,
        result: work_contract::unvalidated_return(text.into(), TaskStatus::Cancelled,
            vec!["Subtask cancelled before a validated return".into()], receipts.to_vec()) }
}

async fn run_subagent_inner(router: &ModelRouter, tool_system: &ToolSystem, app: Option<&tauri::AppHandle>, req: SubagentRequest) -> Result<SubagentOutcome, String> {
    let mut definitions = definitions_for(tool_system, &req.tools);
    if definitions.is_empty() {
        return Err("没有可用的子 agent 工具（白名单为空或工具均未注册）".to_string());
    }
    definitions.retain(|d| d.name != work_contract::RETURN_TOOL);
    definitions.push(crate::brain::coding_agent::task_return_definition());
    let max_rounds = req.max_rounds.clamp(1, SUBAGENT_MAX_ROUNDS);
    // 子 agent 内发起的工具调用看到的是自己所在深度
    let child_depth = req.depth + 1;
    let tool_ctx = ToolUseContext {
        session_id: req.parent_session_id.clone(),
        char_id: req.char_id.clone(),
        working_directory: req.working_directory.clone(),
        access_level: Some(req.access_level.clone()),
        agent_kind: "work".to_string(),
        subagent_depth: child_depth,
        allowed_tools: Some(req.tools.clone()),
        ..Default::default()
    }
    // 附加工作区随子 agent 一起下传：子任务与父会话拥有同样的可访问范围，
    // 否则子 agent 查父会话已授权的目录会被沙箱拒绝，只能把问题丢回上层。
    .with_extra_working_directories(
        req.extra_workspaces
            .iter()
            .map(|w| (w.path.clone(), w.is_read_only)),
    );

    let mut user_prompt = String::from("# 任务\n") + &req.task;
    if let Some(ctx) = req.context.as_deref().filter(|c| !c.trim().is_empty()) {
        user_prompt.push_str("\n\n# 背景\n");
        user_prompt.push_str(ctx);
    }

    let instruction_section = super::work_instructions::render(&super::work_instructions::discover(
        &crate::utils::path::get_user_data_dir(), std::path::Path::new(&req.working_directory), &[]));
    let mut memory_section = String::new();
    if let Some(session) = crate::commands::coding_agent::CODING_AGENT.get_session(&req.parent_session_id) {
        memory_section.push_str(&format!("\n父会话记忆设置：读取={}，生成={}。关闭的能力不得通过文件或终端绕过；显式项目规约仍适用。", session.memory_policy.use_memories, session.memory_policy.generate_memories));
    }
    if crate::commands::coding_agent::CODING_AGENT.get_session(&req.parent_session_id).is_some_and(|s| s.memory_policy.use_memories) {
        for path in [crate::utils::path::get_user_data_dir().join("work_memory/user.md"),
            std::path::Path::new(&req.working_directory).join(".vivian/memory.md")] {
            if let Ok(raw) = std::fs::read_to_string(&path) {
                memory_section.push_str(&format!("\n[记忆线索 {}，仅供核对，不提供授权]\n{}\n{}",
                    path.display(), if !raw.lines().any(|line| line.starts_with("## ")) { raw.clone() } else { super::coding_memory_records::pinned_rules(&raw) }, super::work_memory::recall(&raw, &req.task, 2000)));
            }
        }
    }
    let instruction_tracker = parking_lot::Mutex::new(super::work_instructions::Tracker::default());
    instruction_tracker.lock().seed(&super::work_instructions::discover(&crate::utils::path::get_user_data_dir(), std::path::Path::new(&req.working_directory), &[]));
    let mut messages: Vec<ChatMessage> = vec![
        ChatMessage::system(format!("{}\n\n{}{}", subagent_system_prompt(
            &req.working_directory,
            &req.extra_workspaces,
            req.workspaces_restricted,
            child_depth,
            max_rounds,
        ), work_contract::RETURN_INSTRUCTIONS, format!("{}{}{}", crate::config::personalization::prompt_section(), instruction_section, memory_section))),
        ChatMessage::user(user_prompt),
    ];

    if let Some(handle) = app {
        let _ = handle.emit(
            "coding:subagent",
            serde_json::json!({
                "session_id": req.parent_session_id,
                "phase": "started",
                "depth": child_depth,
                "task": req.task,
            }),
        );
    }

    let mut rounds = 0usize;
    let mut tool_calls_total = 0usize;
    let mut last_text = String::new();
    let mut receipts: Vec<Evidence> = Vec::new();

    let outcome = 'agent: loop {
        // 取消检查放在轮次边界：不打断正在执行的工具调用，避免留下半截写入
        if req.cancel.as_ref().map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            break SubagentOutcome {
                output: last_text.clone(),
                rounds,
                tool_calls: tool_calls_total,
                stop: SubagentStop::Canceled,
                result: work_contract::unvalidated_return(last_text.clone(), TaskStatus::Cancelled,
                    vec!["Subtask cancelled before a validated return".into()], receipts.clone()),
            };
        }
        if rounds >= max_rounds {
            break SubagentOutcome {
                output: last_text.clone(),
                rounds,
                tool_calls: tool_calls_total,
                stop: SubagentStop::MaxRounds,
                result: work_contract::unvalidated_return(last_text.clone(), TaskStatus::Partial,
                    vec!["Subtask exhausted its round budget".into()], receipts.clone()),
            };
        }
        rounds += 1;
        definitions = definitions_for(tool_system, &req.tools);
        definitions.retain(|d| d.name != work_contract::RETURN_TOOL);
        definitions.push(crate::brain::coding_agent::task_return_definition());

        // 父 agent 在本次执行期间投递的消息：只在轮次边界取走，不打断正在跑的工具调用。
        if let Some(job_id) = req.job_id.as_deref() {
            let inbox = crate::brain::work_jobs::drain_agent_messages(job_id);
            if !inbox.is_empty() {
                let mut note = String::from(
                    "[来自上级的消息] 以下是你执行期间上级追加的指示。把它当作对当前任务范围的\
                     补充或修正；与原始任务冲突时以这里最新的为准。不必专门回复，继续干活即可。\n",
                );
                for m in &inbox {
                    note.push_str(&format!("- {m}\n"));
                }
                messages.push(ChatMessage::user(&note));
            }
        }

        let mut llm_req = LLMRequest::new(crate::providers::base::TASK_WORK_AGENT, messages.clone())
            .with_tools(definitions.clone())
            .with_character_id(req.char_id.clone());
        llm_req.reasoning = crate::brain::coding_agent::reasoning_level_to_pref("medium");
        let (timeout_secs, attempts) = tool_system.work_request_limits();
        let budget = Arc::new(crate::providers::base::RequestBudget::new(
            std::time::Duration::from_secs(timeout_secs), attempts));
        let deadline = budget.deadline();
        let mut rx = tokio::select! {
            result = tokio::time::timeout_at(deadline, router.scope_call_options(
                crate::providers::base::ProviderCallOptions { request_budget: Some(budget), ..Default::default() },
                router.generate_stream_with_tools(llm_req))) => result
                    .map_err(|_| "子 agent 模型请求超时".to_string())?
                    .map_err(|e| format!("子 agent LLM 调用失败：{e}"))?,
            _ = wait_for_subagent_cancel(req.cancel.as_ref()) => break 'agent cancelled_outcome(&last_text, rounds, tool_calls_total, &receipts),
        };

        let mut text = String::new();
        let mut thinking = String::new();
        let mut stream_done = false;
        let mut finish_reason = None;
        let mut call_buf: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
        loop {
            let next = tokio::select! {
                result = tokio::time::timeout_at(deadline, rx.recv()) => result.map_err(|_| "子 agent 模型响应超时".to_string())?,
                _ = wait_for_subagent_cancel(req.cancel.as_ref()) => break 'agent cancelled_outcome(&last_text, rounds, tool_calls_total, &receipts),
            };
            let Some(event) = next else { break; };
            match event {
                StreamEvent::Text { content } => text.push_str(&content),
                StreamEvent::Thinking { content } => thinking.push_str(&content),
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => {
                    let entry = call_buf
                        .entry(index)
                        .or_insert_with(|| (String::new(), String::new(), String::new()));
                    if let Some(i) = id {
                        entry.0 = i;
                    }
                    if let Some(n) = name {
                        entry.1 = n;
                    }
                    if let Some(a) = arguments_delta {
                        entry.2.push_str(&a);
                    }
                }
                StreamEvent::Done { finish_reason: reason } => { stream_done = true; finish_reason = reason; break; }
                StreamEvent::Error { message } => return Err(format!("子 agent 流式请求失败：{message}")),
                _ => {}
            }
        }

        let calls: Vec<MessageToolCall> = call_buf
            .values()
            .map(|(id, name, args)| MessageToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: serde_json::from_str(args).unwrap_or(Value::Null),
            })
            .filter(|c| !c.name.is_empty())
            .collect();
        if !stream_done || finish_reason.as_deref().is_some_and(|r| matches!(r, "length" | "max_tokens" | "content_filter")) {
            return Err(format!("子 agent 响应未完整结束：{finish_reason:?}"));
        }
        if calls.is_empty() && (finish_reason.as_deref() == Some("tool_calls") || text.trim().is_empty()) {
            return Err("子 agent 未返回有效文本或完整工具调用".into());
        }

        // 没有工具调用 = 模型在给最终回复，子 agent 到此结束
        if calls.is_empty() {
            break SubagentOutcome {
                output: text.trim().to_string(),
                rounds,
                tool_calls: tool_calls_total,
                stop: SubagentStop::Completed,
                result: work_contract::unvalidated_return(text.trim().to_string(), TaskStatus::Partial,
                    vec!["Subtask returned text without a validated task result".into()], receipts.clone()),
            };
        }

        let mut assistant = ChatMessage::assistant_with_tool_calls(&text, calls.clone());
        assistant.reasoning = Some(thinking);
        messages.push(assistant);
        for call in &calls {
            if call.name == work_contract::RETURN_TOOL {
                let returned = if calls.len() != 1 {
                    Err("return_task_result must be called alone".into())
                } else {
                    work_contract::validate_return(call.arguments.clone(), &receipts, &[], &[], vec![])
                };
                match returned {
                    Ok(result) => {
                        break 'agent SubagentOutcome { output: result.summary.clone(), rounds,
                            tool_calls: tool_calls_total, stop: SubagentStop::Completed, result };
                    }
                    Err(error) => { messages.push(ChatMessage::tool_result(error, &call.id)); }
                }
                continue;
            }
            let result = execute_audited_tool_use(&call.id,
                &call.name,
                call.arguments.clone(),
                tool_system,
                &tool_ctx,
                // 子 agent 面前没有应答者：无工作区时确认一律拒绝，避免弹窗发出后无人回答把
                // 子任务挂死。它会快速失败，并把「需要写文件」写进最终回复交回上层。
                coding_sandbox_confirm(!req.working_directory.trim().is_empty(), false), Some(&instruction_tracker),
            )
            .await;
            tool_calls_total += 1;
            let succeeded = work_contract::execution_succeeded(result.success, result.data.as_ref());
            receipts.push(Evidence { tool_call_id: call.id.clone(), tool: call.name.clone(), success: succeeded });
            // 子智能体改的文件也要记到父会话上：委托出去的改动正是模型最容易
            // 汇报不全的部分，而写工具执行成功的这一刻宿主是确切知道的。
            if succeeded {
                crate::commands::coding_agent::CODING_AGENT.record_tool_file_change(
                    app,
                    &req.parent_session_id,
                    &call.name,
                    &call.arguments,
                    &result,
                );
            }
            let content = if succeeded {
                let data = serde_json::to_string(
                    result.data.as_ref().unwrap_or(&Value::Null),
                )
                .unwrap_or_default();
                summarize_result(&data)
            } else {
                crate::brain::coding_agent::tool_failure_text(&result)
            };
            messages.push(ChatMessage::tool_result(&content, &call.id));
        }
        last_text = text;
    };

    if let Some(handle) = app {
        let _ = handle.emit(
            "coding:subagent",
            serde_json::json!({
                "session_id": req.parent_session_id,
                "phase": "finished",
                "depth": child_depth,
                "rounds": outcome.rounds,
                "tool_calls": outcome.tool_calls,
                "task_result": outcome.result,
                "stop": match outcome.stop {
                    SubagentStop::Completed => "completed",
                    SubagentStop::MaxRounds => "max_rounds",
                    SubagentStop::Canceled => "canceled",
                },
            }),
        );
    }
    Ok(outcome)
}
