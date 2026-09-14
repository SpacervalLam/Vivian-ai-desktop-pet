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

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tauri::Emitter;

use crate::brain::coding_agent::{coding_sandbox_allow, summarize_result};
use crate::providers::base::{LLMRequest, StreamEvent, ToolDefinition};
use crate::providers::router::ModelRouter;
use crate::tools::executor::execute_tool_use;
use crate::tools::types::{AgentAccessLevel, ToolUseContext};
use crate::tools::ToolSystem;
use crate::types::response::{ChatMessage, MessageToolCall};

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
    pub access_level: AgentAccessLevel,
    pub char_id: String,
    /// 协作式取消标志（后台任务才有）。置位后在下一个轮次边界收手。
    pub cancel: Option<Arc<AtomicBool>>,
}

/// 把注册表里的工具 schema 转成 provider 侧定义（按白名单顺序输出，
/// 保证每轮 tools 参数字节一致，保住 API 前缀缓存）。
fn definitions_for(tool_system: &ToolSystem, allowed: &[String]) -> Vec<ToolDefinition> {
    let schemas: HashMap<String, ToolDefinition> = tool_system
        .get_tool_schemas()
        .into_iter()
        .map(|d| {
            (
                d.name.clone(),
                ToolDefinition {
                    name: d.name,
                    description: d.description,
                    parameters: d.input_schema,
                },
            )
        })
        .collect();
    allowed
        .iter()
        .filter_map(|name| schemas.get(name).cloned())
        .collect()
}

/// 子 agent 的 system prompt。
fn subagent_system_prompt(working_directory: &str, depth: usize, max_rounds: usize) -> String {
    let has_wd = !working_directory.trim().is_empty();
    let env = if has_wd {
        format!("- 工作目录：{working_directory}（文件操作仅限该目录内）")
    } else {
        "- 工作目录：未选择（文件操作使用绝对路径）".to_string()
    };
    // 与主智能体一致：有工作目录走相对路径，没有才退回绝对路径
    let link_protocol = if has_wd {
        "提到本地文件时使用可点击 Markdown 链接 `[文件名 (line N)](相对工作目录的路径:N)`：\
         路径相对于工作目录、使用正斜杠、行号从 1 开始，不要写盘符或绝对路径。"
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
         - 你**不能向用户提问**（没有人在等你回答）。遇到方向分叉就自行选最合理的路，\
         并在最终回复里写明「我选了 A 而不选 B，原因是…」，上层据此决定是否重新派发。\n\n\
         # 边界\n\
         - 只做被派发的事：不要顺手重构、不要扩大范围、不要改无关文件。\n\
         - 你有自己独立的工作待办清单，与上层的清单互不干扰。\n\n\
         # 环境\n\
         - 操作系统：Windows（命令用 PowerShell 语法）\n\
         {env}\n\
         - 轮次预算：{max_rounds} 轮工具调用\n\n\
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
    let definitions = definitions_for(tool_system, &req.tools);
    if definitions.is_empty() {
        return Err("没有可用的子 agent 工具（白名单为空或工具均未注册）".to_string());
    }
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
        ..Default::default()
    };

    let mut user_prompt = String::from("# 任务\n") + &req.task;
    if let Some(ctx) = req.context.as_deref().filter(|c| !c.trim().is_empty()) {
        user_prompt.push_str("\n\n# 背景\n");
        user_prompt.push_str(ctx);
    }

    let mut messages: Vec<ChatMessage> = vec![
        ChatMessage::system(subagent_system_prompt(
            &req.working_directory,
            child_depth,
            max_rounds,
        )),
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

    let outcome = loop {
        // 取消检查放在轮次边界：不打断正在执行的工具调用，避免留下半截写入
        if req.cancel.as_ref().map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            break SubagentOutcome {
                output: last_text.clone(),
                rounds,
                tool_calls: tool_calls_total,
                stop: SubagentStop::Canceled,
            };
        }
        if rounds >= max_rounds {
            break SubagentOutcome {
                output: last_text.clone(),
                rounds,
                tool_calls: tool_calls_total,
                stop: SubagentStop::MaxRounds,
            };
        }
        rounds += 1;

        let mut llm_req = LLMRequest::new(crate::providers::base::TASK_WORK_AGENT, messages.clone())
            .with_tools(definitions.clone());
        llm_req.reasoning = crate::brain::coding_agent::reasoning_level_to_pref("medium");
        let mut rx = router
            .generate_stream_with_tools(llm_req)
            .await
            .map_err(|e| format!("子 agent LLM 调用失败：{e}"))?;

        let mut text = String::new();
        let mut call_buf: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Text { content } => text.push_str(&content),
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
                StreamEvent::Done { .. } => break,
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

        // 没有工具调用 = 模型在给最终回复，子 agent 到此结束
        if calls.is_empty() {
            break SubagentOutcome {
                output: text.trim().to_string(),
                rounds,
                tool_calls: tool_calls_total,
                stop: SubagentStop::Completed,
            };
        }

        messages.push(ChatMessage::assistant_with_tool_calls(&text, calls.clone()));
        for call in &calls {
            let result = execute_tool_use(
                &call.name,
                call.arguments.clone(),
                tool_system,
                &tool_ctx,
                coding_sandbox_allow(),
            )
            .await;
            tool_calls_total += 1;
            // 子智能体改的文件也要记到父会话上：委托出去的改动正是模型最容易
            // 汇报不全的部分，而写工具执行成功的这一刻宿主是确切知道的。
            if result.success {
                crate::commands::coding_agent::CODING_AGENT.record_tool_file_change(
                    app,
                    &req.parent_session_id,
                    &call.name,
                    &call.arguments,
                    &result,
                );
            }
            let content = if result.success {
                let data = serde_json::to_string(
                    result.data.as_ref().unwrap_or(&Value::Null),
                )
                .unwrap_or_default();
                summarize_result(&data)
            } else {
                result
                    .error
                    .clone()
                    .unwrap_or_else(|| "执行失败".to_string())
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
