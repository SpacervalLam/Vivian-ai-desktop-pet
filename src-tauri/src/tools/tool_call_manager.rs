//! 工具调用编排器 - 多步工具调用循环 + 重复检测 + 参数注入
//!
//! - [`ToolCallManager`]：解析 AI 响应中的工具调用并循环执行
//! - [`ToolCallManager::parse_tool_calls`]：从文本中提取所有 JSON 工具调用
//! - [`ToolCallManager::extract_immediate_response`]：提取非 JSON 的即时响应文本
//! - [`NON_BLOCKING_TOOLS`]：异步执行不等待的工具集合
//! - [`ToolListTool`]：生成工具列表给 AI 的元工具

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::executor::execute_tool_use;
use super::registry::{normalize_tool_name, ToolSystem};
use super::permission::is_confirmation_required_tool;
use super::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolScene, ToolUseContext, ValidationResult,
};

/// 非阻塞工具集合 - 这些工具启动后立即返回，不等待完成
/// 注意：open_application / open_url 虽然是进程启动，
/// 但解析 + spawn 本身 < 100ms，应同步等待真实结果再反馈给 LLM，
/// 否则 LLM 会误以为成功而回复"已打开"（实际可能失败）。
pub const NON_BLOCKING_TOOLS: &[&str] = &[
    "set_timer",
    "take_screenshot",
];

/// 工具调用状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// 待执行
    Pending,
    /// 成功
    Success,
    /// 失败
    Error,
    /// 达到最大迭代次数
    MaxIterationsReached,
    /// 需要用户授权
    PermissionRequired,
    /// 非阻塞（已启动，未等待）
    NonBlocking,
}

impl Default for ToolCallStatus {
    fn default() -> Self {
        ToolCallStatus::Pending
    }
}

/// 解析出的工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedToolCall {
    /// 工具名
    pub tool: String,
    /// 参数
    pub arguments: Value,
}

/// 单次工具调用结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// 是否成功
    pub success: bool,
    /// 工具返回数据
    pub result: Option<Value>,
    /// 工具名
    pub tool_name: String,
    /// 调用时使用的参数
    #[serde(default)]
    pub arguments: Value,
    /// 工具调用 ID
    pub tool_call_id: String,
    /// 错误信息
    pub error: Option<String>,
    /// 状态
    pub status: ToolCallStatus,
    /// 是否需要用户确认
    pub requires_confirmation: bool,
    /// 是否标志用户目标已达成（Agent 循环检测到 true 应终止后续工具调用）
    #[serde(default)]
    pub goal_completed: bool,
}

/// 多步执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiStepResult {
    /// 所有工具调用结果
    pub results: Vec<ToolCallResult>,
    /// 即时响应文本（非 JSON 部分）
    pub immediate_response: Option<String>,
    /// 整体状态
    pub status: ToolCallStatus,
    /// 实际执行的迭代次数
    pub iterations_used: usize,
}

/// 工具调用管理器 - 解析 AI 响应并循环执行工具调用
pub struct ToolCallManager {
    /// 工具系统
    tool_system: Arc<ToolSystem>,
    /// 最大迭代次数
    max_iterations: usize,
    /// 反馈提示词中工具结果 JSON 的截断长度（来自 `config.tools.feedback_history_chars`）
    feedback_history_chars: usize,
    /// 工具调用上下文（运行时可刷新，让工具感知情绪/关系/记忆）
    context: Arc<RwLock<ToolUseContext>>,
    /// 角色 ID（注入 PERSONA_LOAD 标志用）
    char_id: String,
    /// 界面语言（约束工具反馈回复语言，来自 config.base.language）
    language: String,
}

impl ToolCallManager {
    /// 创建新的工具调用管理器，默认最大迭代次数 10，反馈截断 2000 字符
    pub fn new(tool_system: Arc<ToolSystem>, context: ToolUseContext) -> Self {
        Self {
            tool_system,
            max_iterations: 10,
            feedback_history_chars: 2000,
            context: Arc::new(RwLock::new(context)),
            char_id: String::new(),
            language: String::from("zh"),
        }
    }

    /// 设置最大迭代次数（0 = 无限：反馈循环不设上限，由 LLM 停止调用工具自然终止；最小为 1）
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = if max == 0 { usize::MAX } else { max.max(1) };
        self
    }

    /// 设置反馈提示词中工具结果 JSON 的截断长度（最小为 100）
    pub fn with_feedback_history_chars(mut self, chars: u32) -> Self {
        self.feedback_history_chars = (chars as usize).max(100);
        self
    }

    /// 设置角色 ID（用于注入 PERSONA_LOAD 标志）
    pub fn with_char_id(mut self, char_id: impl Into<String>) -> Self {
        self.char_id = char_id.into();
        self
    }

    /// 设置界面语言（约束工具反馈回复语言）
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = lang.into();
        self
    }

    /// 刷新工具调用上下文 —— 由 Brain 在每次对话开始时调用，
    /// 让工具能感知当前情绪 / 关系阶段 / 最近记忆摘要。
    pub fn update_context(&self, context: ToolUseContext) {
        *self.context.write() = context;
    }

    /// 工具系统引用（供外部直接查询工具使用）
    pub fn tool_system(&self) -> Arc<ToolSystem> {
        Arc::clone(&self.tool_system)
    }

    /// 当前上下文快照（供内部与外部读取当前工具上下文）
    pub fn context_snapshot(&self) -> ToolUseContext {
        self.context.read().clone()
    }

    /// 执行一组结构化工具调用（原生 function calling 路径使用）
    ///
    /// 与 `execute_multi_step` 的区别：
    /// - `execute_multi_step` 接收 LLM 文本响应，需要先解析 JSON
    /// - `execute_structured_calls` 直接接收 `StructuredToolCall`，跳过解析
    ///
    /// 并发策略：写工具串行执行（保持上下文修改顺序），只读工具批量并行。
    /// 权限确认：通过 `execute_tool_use` 内部的沙箱检查触发 toast。
    pub async fn execute_structured_calls(
        &self,
        calls: &[crate::providers::base::StructuredToolCall],
    ) -> Vec<ToolCallResult> {
        let tool_system = self.tool_system();
        let mut ctx_snapshot = self.context_snapshot();
        let mut results: Vec<ToolCallResult> = Vec::new();
        let mut parallel_batch: Vec<(String, Value, usize)> = Vec::new();
        let mut iterations_used = 0usize;

        // flush 并行批次（与 execute_multi_step 共用逻辑）
        async fn flush_batch(
            batch: &mut Vec<(String, Value, usize)>,
            ts: &Arc<ToolSystem>,
            ctx: &mut ToolUseContext,
            results: &mut Vec<ToolCallResult>,
        ) {
            if batch.is_empty() {
                return;
            }
            let mut tasks = Vec::new();
            for (name, args, idx) in batch.drain(..) {
                let ts = Arc::clone(ts);
                let ctx = ctx.clone();
                let args_for_result = args.clone();
                tasks.push(tokio::spawn(async move {
                    (
                        idx,
                        name.clone(),
                        args_for_result,
                        execute_tool_use(&name, args, &ts, &ctx, None).await,
                    )
                }));
            }
            for task in tasks {
                if let Ok((idx, name, args, mut r)) = task.await {
                    let requires_confirmation = matches!(
                        r.error.as_deref(),
                        Some("PermissionRequired")
                            | Some("SandboxConfirmationRequired")
                            | Some("UserDenied")
                    );
                    let status = if requires_confirmation {
                        ToolCallStatus::PermissionRequired
                    } else if r.success {
                        ToolCallStatus::Success
                    } else {
                        ToolCallStatus::Error
                    };
                    if let Some(modifier) = r.context_modifier.take() {
                        modifier(ctx);
                    }
                    results.push(ToolCallResult {
                        success: r.success,
                        result: r.data,
                        tool_name: name,
                        arguments: args,
                        tool_call_id: format!("call_{}", idx),
                        error: r.error,
                        status,
                        requires_confirmation,
                        goal_completed: r.goal_completed,
                    });
                }
            }
        }

        for tc in calls {
            iterations_used += 1;
            let tool_name = tc.name.clone();
            let arguments = tc.arguments.clone();

            // 非阻塞工具：spawn 后立即继续
            if NON_BLOCKING_TOOLS.contains(&tool_name.as_str()) {
                flush_batch(&mut parallel_batch, &tool_system, &mut ctx_snapshot, &mut results).await;
                tracing::info!("[ToolCallManager] 非阻塞工具(native fc): {}，启动后立即继续", tool_name);
                let ts = Arc::clone(&tool_system);
                let ctx = ctx_snapshot.clone();
                let spawn_name = tool_name.clone();
                let args = arguments.clone();
                tokio::spawn(async move {
                    let result = execute_tool_use(&spawn_name, args, &ts, &ctx, None).await;
                    if !result.success {
                        tracing::error!(
                            "[ToolCallManager] 非阻塞工具 {} 执行失败: {:?}",
                            spawn_name,
                            result.error
                        );
                    }
                });
                results.push(ToolCallResult {
                    success: true,
                    result: Some(json!({
                        "tool": tool_name,
                        "status": "started",
                        "message": "Tool started (non-blocking mode)"
                    })),
                    tool_name,
                    arguments,
                    tool_call_id: tc.id.clone(),
                    error: None,
                    status: ToolCallStatus::NonBlocking,
                    requires_confirmation: false,
                    goal_completed: false,
                });
                continue;
            }

            // 只读且无 placeholder → 入并行批次
            let can_parallel = tool_system
                .find_tool(&tool_name)
                .map(|t| t.is_read_only())
                .unwrap_or(false)
                && !has_placeholders(&arguments);

            if can_parallel {
                parallel_batch.push((tool_name, arguments, iterations_used));
                continue;
            }

            // 写工具：先 flush 并行批次再串行执行
            if !parallel_batch.is_empty() {
                flush_batch(&mut parallel_batch, &tool_system, &mut ctx_snapshot, &mut results).await;
            }

            let args_for_result = arguments.clone();
            let mut r = execute_tool_use(
                &tool_name,
                arguments,
                &tool_system,
                &ctx_snapshot,
                None,
            )
            .await;

            let requires_confirmation = matches!(
                r.error.as_deref(),
                Some("PermissionRequired")
                    | Some("SandboxConfirmationRequired")
                    | Some("UserDenied")
            );
            let status = if requires_confirmation {
                ToolCallStatus::PermissionRequired
            } else if r.success {
                ToolCallStatus::Success
            } else {
                ToolCallStatus::Error
            };
            if let Some(modifier) = r.context_modifier.take() {
                modifier(&mut ctx_snapshot);
                *self.context.write() = ctx_snapshot.clone();
            }
            results.push(ToolCallResult {
                success: r.success,
                result: r.data,
                tool_name,
                arguments: args_for_result,
                tool_call_id: tc.id.clone(),
                error: r.error,
                status,
                requires_confirmation,
                goal_completed: r.goal_completed,
            });
        }

        // flush 残留并行批次
        flush_batch(&mut parallel_batch, &tool_system, &mut ctx_snapshot, &mut results).await;

        results
    }

    /// 反馈循环：执行工具 → 把结果反馈给 LLM → LLM 再决策 → 直到 LLM 不再调用工具或达到上限
    ///
    /// `ai_generate` 回调接收 continue prompt，返回 LLM 文本响应（None 视为生成失败，终止循环）。
    /// `initial_response` 是首轮 LLM 响应（已经包含 tool_calls）。
    ///
    /// 返回 `(最终 LLM 响应文本, 实际执行的迭代轮数, 所有工具调用结果)`。
    /// 若 LLM 不再返回工具调用，返回最后一次响应；若达到上限或失败，返回最后一次响应。
    pub async fn run_feedback_loop<F, Fut>(
        &self,
        initial_response: &str,
        mut ai_generate: F,
    ) -> (Option<String>, usize, Vec<ToolCallResult>, Option<f64>)
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = Option<String>>,
    {
        let mut current_response = initial_response.to_string();
        let mut total_iterations = 0usize;
        let mut all_results: Vec<ToolCallResult> = Vec::new();
        let mut first_tool_executed_at: Option<f64> = None;
        // 反馈循环上限：留出 1 轮给初始响应，剩余次数用于反馈。
        // 0（无限）模式下不再钳到 4 轮，由 LLM 停止调用工具自然终止。
        let max_feedback_rounds = if self.max_iterations == usize::MAX {
            usize::MAX
        } else {
            self.max_iterations.saturating_sub(1).max(1).min(4)
        };

        for _ in 0..max_feedback_rounds {
            // 1. 解析并执行本轮工具调用
            let multi_result = self.execute_multi_step(&current_response).await;
            if multi_result.results.is_empty() {
                // LLM 没有再调用工具 → 任务完成
                return (Some(current_response), total_iterations, all_results, first_tool_executed_at);
            }
            total_iterations += 1;
            let results_ref: Vec<ToolCallResult> = multi_result.results.clone();
            if first_tool_executed_at.is_none() && !results_ref.is_empty() {
                first_tool_executed_at = Some(crate::memory::types::current_timestamp());
            }
            all_results.extend(results_ref.clone());

            // Goal Satisfaction：本轮有工具声明目标已完成 → 仍让 LLM 生成最终回复，
            // 但不再继续工具循环（即便 LLM 仍想调用工具也直接返回）。
            let goal_completed = results_ref.iter().any(|r| r.goal_completed);
            if goal_completed {
                tracing::info!(
                    "[ToolCallManager] 文本路径检测到 goal_completed，本轮为最后一轮反馈"
                );
            }

            // 2. 构建反馈提示词（汇总工具执行结果给 LLM）
            let continue_prompt = self.build_feedback_prompt(&results_ref, &current_response);

            // 3. 调用 LLM 让它基于工具结果继续生成
            match ai_generate(continue_prompt).await {
                Some(resp) if !resp.trim().is_empty() => {
                    current_response = resp;
                    // 任务已完成 → 直接返回最终回复，不再解析工具调用
                    if goal_completed {
                        return (Some(current_response), total_iterations, all_results, first_tool_executed_at);
                    }
                    // 4. 检查 LLM 是否还在调用工具
                    let next_calls = Self::parse_tool_calls(&current_response);
                    if next_calls.is_empty() {
                        // LLM 不再调用工具 → 任务完成
                        return (Some(current_response), total_iterations, all_results, first_tool_executed_at);
                    }
                    // 否则继续下一轮循环
                }
                _ => {
                    // LLM 生成失败 → 返回最后一次响应
                    return (Some(current_response), total_iterations, all_results, first_tool_executed_at);
                }
            }
        }

        // 达到反馈循环上限
        (Some(current_response), total_iterations, all_results, first_tool_executed_at)
    }

    /// 构建工具执行结果反馈提示词（让 LLM 基于结果决定下一步）
    ///
    /// 顶部注入 PERSONA_LOAD 标志 + 精简人设 + 语言约束，与主对话保持一致；
    /// 指令文本按界面语言三语化，人设语气提示按角色区分。
    fn build_feedback_prompt(&self, results: &[ToolCallResult], last_response: &str) -> String {
        let lang = crate::pipeline::prompt_modules::normalize_lang(&self.language);
        let is_nana = self.char_id.eq_ignore_ascii_case("nana");

        let mut lines: Vec<String> = Vec::new();
        lines.push(crate::pipeline::prompt_modules::build_tool_minimal_identity(
            &self.char_id,
            &self.language,
        ));
        lines.push(crate::pipeline::prompt_modules::tool_minimal_output_format(
            &self.language,
        ));

        let (header, last_resp_label) = match lang {
            "en" => ("## Tool Execution Results", "Last AI response: "),
            "ja" => ("## ツール実行結果", "前回の AI 応答："),
            _ => ("## 工具执行结果", "上一轮 AI 响应："),
        };

        lines.push(header.to_string());
        lines.push(String::new());

        let any_goal_completed = results.iter().any(|r| r.goal_completed);

        for r in results {
            let status = if r.goal_completed {
                "GOAL_COMPLETED"
            } else if r.success {
                "SUCCESS"
            } else {
                "FAILED"
            };
            lines.push(format!("### {} [{}]", r.tool_name, status));
            if r.success {
                if let Some(data) = &r.result {
                    let data_str = serde_json::to_string_pretty(data).unwrap_or_default();
                    // 截断过长的结果（避免提示词爆炸）—— 截断长度由 config.tools.feedback_history_chars 控制；
                    // 头尾保留 + 中段折叠（与编程智能体同一裁剪策略，尾部退出码/报错不丢）
                    let truncated = crate::tools::executor::prune_head_tail(&data_str, self.feedback_history_chars);
                    lines.push("```json".to_string());
                    lines.push(truncated);
                    lines.push("```".to_string());
                }
            } else if let Some(err) = &r.error {
                lines.push(format!("Error: {}", err));
            }
            lines.push(String::new());
        }

        lines.push(format!("{}{}", last_resp_label, last_response));
        lines.push(String::new());
        let (done_intro, done_forbid, ask_intro, ask_done, ask_more) = match lang {
            "en" => (
                "**The user's goal has been completed via tool calls.** Tell the user the result briefly in your persona's voice.",
                "**Do NOT call any more tools** (especially search/query tools), and don't try to \"help further\".",
                "Based on the tool results above, reply to the user in your persona's voice:",
                "- If the task is complete, tell the result briefly in persona voice (no more tool calls);",
                "- If more tools are needed, output the tool call JSON.",
            ),
            "ja" => (
                "**ユーザーの目標はツール呼び出しで完了しました。** キャラの口調で簡潔に結果を伝えて。",
                "**これ以上ツールを呼び出さないこと**（特に検索・照会系）、\"さらに手伝う\"ことはしない。",
                "上記のツール実行結果に基づき、キャラの口調でユーザーに返信してください：",
                "- タスクが完了していれば、口調に合わせて簡潔に結果を伝える（これ以上ツールを呼ばない）；",
                "- さらにツールが必要なら、ツール呼び出し JSON を出力する。",
            ),
            _ => (
                "**用户目标已通过工具调用完成。** 请直接以角色人设口吻简短告知用户结果，",
                "**禁止再调用任何工具**（尤其是搜索/查询类工具），不要尝试\"进一步帮助用户\"。",
                "请根据以上工具执行结果，以角色人设口吻回复用户：",
                "- 若任务已完成，用符合人设的语气简短告知结果（不要再调用工具）；",
                "- 若需要继续调用工具，请输出工具调用 JSON。",
            ),
        };
        // 人设语气红线：按角色区分（Nana 温柔从容 / Vivian 傲娇嘴硬）
        let forbid_service = match (lang, is_nana) {
            ("en", true) => "- NO customer-service tone (e.g. \"OK\" \"Done for you\" \"Right away\") — keep your gentle, composed, soft-spoken way of talking.",
            ("en", false) => "- NO customer-service tone (e.g. \"OK\" \"Done for you\" \"Right away\") — keep your tsundere, sharp-tongued-but-warm-hearted way of talking.",
            ("ja", true) => "- カスタマーサービス口調（「はい」「完了しました」「すぐ対応します」など）は禁止——温和で落ち着いた、そっと話す口調を保つこと。",
            ("ja", false) => "- カスタマーサービス口調（「はい」「完了しました」「すぐ対応します」など）は禁止——ツンデレで毒舌だけど根は優しい口調を保つこと。",
            (_, true) => "- 禁止使用客服/助手语气（如\"好的\"\"已为您完成\"\"这就帮您\"），必须保持温柔从容、轻声细语的说话方式。",
            (_, false) => "- 禁止使用客服/助手语气（如\"好的\"\"已为您完成\"\"这就帮您\"），必须保持傲娇、嘴硬心软的说话方式。",
        };
        if any_goal_completed {
            lines.push(done_intro.to_string());
            lines.push(done_forbid.to_string());
            lines.push(forbid_service.to_string());
        } else {
            lines.push(ask_intro.to_string());
            lines.push(ask_done.to_string());
            lines.push(ask_more.to_string());
            lines.push(forbid_service.to_string());
        }

        lines.join("\n")
    }


    /// 多步执行主循环：解析 AI 响应中的工具调用并执行
    ///
    /// 并发策略（细粒度并发控制）：
    /// - 只读工具且无 `${result}`/`${step.N.result}` 依赖 → 累积并行批次，`join_all` 并发执行
    /// - 写工具或有依赖的工具 → 先 flush 并行批次，再串行执行
    /// - 非阻塞工具 → 先 flush 并行批次，再 spawn 后立即继续
    pub async fn execute_multi_step(&self, ai_response: &str) -> MultiStepResult {
        let parsed = Self::parse_tool_calls(ai_response);
        let immediate_response = Self::extract_immediate_response(ai_response);

        if parsed.is_empty() {
            return MultiStepResult {
                results: Vec::new(),
                immediate_response,
                status: ToolCallStatus::Success,
                iterations_used: 0,
            };
        }

        // 拍快照：本次多步执行全程使用同一份上下文，避免半途被刷新
        // （工具的 context_modifier 会在快照上原地修改，并回写到共享 context）
        let mut ctx_snapshot = self.context.read().clone();

        let mut results: Vec<ToolCallResult> = Vec::with_capacity(parsed.len());
        let mut executed: HashSet<String> = HashSet::new();
        let mut last_result: Option<Value> = None;
        let mut iterations_used = 0usize;
        let mut final_status = ToolCallStatus::Success;
        // 并行批次：累积可并行的只读工具调用 (tool_name, arguments, iteration_index)
        let mut parallel_batch: Vec<(String, Value, usize)> = Vec::new();

        for tc in parsed {
            if iterations_used >= self.max_iterations {
                final_status = ToolCallStatus::MaxIterationsReached;
                break;
            }

            let tool_name = tc.tool;
            let mut arguments = tc.arguments;

            // 参数注入：把前一步结果注入 ${result} / ${step.N.result} 占位符
            inject_placeholders(&mut arguments, &results, &last_result);

            // 重复检测：相同工具 + 相同参数跳过
            let call_key = fingerprint(&tool_name, &arguments);
            if executed.contains(&call_key) {
                tracing::warn!(
                    "[ToolCallManager] 跳过重复调用: {} (相同参数已执行)",
                    tool_name
                );
                continue;
            }
            executed.insert(call_key);

            iterations_used += 1;

            // 非阻塞工具：先 flush 并行批次，再异步执行不等待
            if NON_BLOCKING_TOOLS.contains(&tool_name.as_str()) {
                flush_parallel_batch(
                    &mut parallel_batch,
                    &self.tool_system,
                    &mut ctx_snapshot,
                    &mut results,
                    &mut last_result,
                    &self.context,
                )
                .await;

                tracing::info!(
                    "[ToolCallManager] 非阻塞工具: {}，启动后立即继续",
                    tool_name
                );
                let ts = Arc::clone(&self.tool_system);
                let ctx = ctx_snapshot.clone();
                let spawn_name = tool_name.clone();
                let args = arguments.clone();
                tokio::spawn(async move {
                    let result = execute_tool_use(&spawn_name, args, &ts, &ctx, None).await;
                    if !result.success {
                        tracing::error!(
                            "[ToolCallManager] 非阻塞工具 {} 执行失败: {:?}",
                            spawn_name,
                            result.error
                        );
                    }
                });

                let started_payload = serde_json::json!({
                    "tool": tool_name,
                    "status": "started",
                    "message": "Tool started (non-blocking mode)"
                });

                results.push(ToolCallResult {
                    success: true,
                    result: Some(started_payload),
                    tool_name: tool_name.clone(),
                    arguments: arguments.clone(),
                    tool_call_id: format!("call_{}_nb", iterations_used),
                    error: None,
                    status: ToolCallStatus::NonBlocking,
                    requires_confirmation: false,
                    goal_completed: false,
                });
                continue;
            }

            // 判断是否可并行：只读 + 参数无 ${result}/${step.} 引用
            let can_parallel = self
                .tool_system
                .find_tool(&tool_name)
                .map(|t| t.is_read_only())
                .unwrap_or(false)
                && !has_placeholders(&arguments);

            if can_parallel {
                parallel_batch.push((tool_name, arguments, iterations_used));
                continue;
            }

            // 写工具或带依赖的工具：先 flush 并行批次，再串行执行
            if !parallel_batch.is_empty() {
                flush_parallel_batch(
                    &mut parallel_batch,
                    &self.tool_system,
                    &mut ctx_snapshot,
                    &mut results,
                    &mut last_result,
                    &self.context,
                )
                .await;
            }

            // 串行执行（复用 execute_tool_use，权限检查在内部完成）
            let mut tool_result = execute_tool_use(
                &tool_name,
                arguments.clone(),
                &self.tool_system,
                &ctx_snapshot,
                None,
            )
            .await;

            // 检测权限相关的错误码
            let requires_confirmation = matches!(
                tool_result.error.as_deref(),
                Some("PermissionRequired")
                    | Some("SandboxConfirmationRequired")
                    | Some("UserDenied")
            );

            let status = if requires_confirmation {
                ToolCallStatus::PermissionRequired
            } else if tool_result.success {
                ToolCallStatus::Success
            } else {
                ToolCallStatus::Error
            };

            let result_data = tool_result.data.clone();
            // 应用上下文修改器（让本步工具结果影响下一步工具可见的 context）
            if let Some(modifier) = tool_result.context_modifier.take() {
                modifier(&mut ctx_snapshot);
                // 把快照写回共享 context，让后续轮次与非阻塞 spawn 也能感知
                *self.context.write() = ctx_snapshot.clone();
            }
            let call_result = ToolCallResult {
                success: tool_result.success,
                result: tool_result.data,
                tool_name: tool_name.clone(),
                arguments: arguments.clone(),
                tool_call_id: format!("call_{}", iterations_used),
                error: tool_result.error,
                status,
                requires_confirmation,
                goal_completed: tool_result.goal_completed,
            };

            // 仅阻塞工具的结果作为下一步注入源
            last_result = result_data;
            results.push(call_result);
        }

        // flush 剩余并行批次
        if !parallel_batch.is_empty() {
            flush_parallel_batch(
                &mut parallel_batch,
                &self.tool_system,
                &mut ctx_snapshot,
                &mut results,
                &mut last_result,
                &self.context,
            )
            .await;
        }

        MultiStepResult {
            results,
            immediate_response,
            status: final_status,
            iterations_used,
        }
    }

    /// 从 AI 响应中解析工具调用
    ///
    /// 支持格式：
    /// - JSON 数组 `[{"tool":"...", "arguments":{...}}]`
    /// - 单个 JSON 对象 `{"tool":"...", "arguments":{...}}`
    /// - 文本中嵌入的多个 JSON 对象
    /// - 兼容 `tool`/`name` 两种键名，`arguments`/`args` 两种参数键名
    pub fn parse_tool_calls(ai_response: &str) -> Vec<ParsedToolCall> {
        let trimmed = ai_response.trim();
        let mut calls = Vec::new();

        // 先尝试整体解析为 JSON 数组
        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(trimmed) {
            for item in &arr {
                if let Some(tc) = value_to_tool_call(item) {
                    calls.push(tc);
                }
            }
            if !calls.is_empty() {
                return calls;
            }
        }

        // 整体解析为单个对象
        if let Ok(obj) = serde_json::from_str::<Value>(trimmed) {
            if let Some(tc) = value_to_tool_call(&obj) {
                return vec![tc];
            }
            // 对象内嵌 tool_calls 列表
            if let Some(list) = obj.get("tool_calls").and_then(Value::as_array) {
                for item in list {
                    if let Some(tc) = value_to_tool_call(item) {
                        calls.push(tc);
                    }
                }
                if !calls.is_empty() {
                    return calls;
                }
            }
        }

        // 从文本中提取所有 JSON 对象
        for obj in extract_all_json_objects(trimmed) {
            if let Some(tc) = value_to_tool_call(&obj) {
                calls.push(tc);
            }
        }

        calls
    }

    /// 提取即时响应文本（非 JSON 部分）
    ///
    /// 优先级：
    /// 1. JSON 数组中元素的 `text` 字段
    /// 2. 第一个 JSON 之前的文本
    /// 3. 第一个 JSON 对象的 `text` 字段
    /// 4. 最后一个 JSON 之后的文本
    pub fn extract_immediate_response(ai_response: &str) -> Option<String> {
        let trimmed = ai_response.trim();
        if trimmed.is_empty() {
            return None;
        }

        // JSON 数组：从元素的 text 字段提取
        if trimmed.starts_with('[') {
            if let Ok(arr) = serde_json::from_str::<Vec<Value>>(trimmed) {
                for item in &arr {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        let t = text.trim();
                        if !t.is_empty() {
                            return Some(t.to_string());
                        }
                    }
                }
            }
        }

        // 找出所有顶层 JSON 对象的字符范围
        let ranges = find_json_ranges(ai_response);
        if ranges.is_empty() {
            let t = trimmed.to_string();
            return if t.is_empty() { None } else { Some(t) };
        }

        // 第一个 JSON 之前的文本
        let first_start = ranges[0].0;
        let mut immediate = ai_response[..first_start].trim().to_string();

        // 如果前面没有文本，尝试从第一个 JSON 中提取 text 字段
        if immediate.is_empty() {
            let (s, e) = ranges[0];
            if let Ok(obj) = serde_json::from_str::<Value>(&ai_response[s..e]) {
                if let Some(text) = obj.get("text").and_then(Value::as_str) {
                    immediate = text.trim().to_string();
                }
            }
        }

        // 如果还是空，取最后一个 JSON 之后的文本
        if immediate.is_empty() {
            let last_end = ranges[ranges.len() - 1].1;
            immediate = ai_response[last_end..].trim().to_string();
        }

        if immediate.is_empty() {
            None
        } else {
            Some(immediate)
        }
    }
}

/// 把 Value 转成工具调用，兼容 `tool`/`name` 两种键名，`arguments`/`args` 两种参数键名
fn value_to_tool_call(v: &Value) -> Option<ParsedToolCall> {
    let obj = v.as_object()?;
    let tool = obj
        .get("tool")
        .and_then(Value::as_str)
        .or_else(|| obj.get("name").and_then(Value::as_str))?
        .to_string();
    let arguments = obj
        .get("arguments")
        .cloned()
        .or_else(|| obj.get("args").cloned())
        .unwrap_or_else(|| Value::Object(Map::new()));
    Some(ParsedToolCall { tool, arguments })
}

/// 从文本中提取所有顶层 JSON 对象
fn extract_all_json_objects(text: &str) -> Vec<Value> {
    find_json_ranges(text)
        .into_iter()
        .filter_map(|(s, e)| {
            serde_json::from_str::<Value>(&text[s..e])
                .ok()
                .filter(|v| v.is_object())
        })
        .collect()
}

/// 找出文本中所有顶层 JSON 对象的字符范围 `(start, end)`
///
/// 利用 `{`/`}` 均为 ASCII 单字节字符、不会出现在 UTF-8 多字节序列内部这一特性，
/// 按字节索引扫描是安全的，切片边界也必然落在字符边界上。
fn find_json_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let bytes = text.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => stack.push(i),
            b'}' if !stack.is_empty() => {
                let start = stack.pop().unwrap();
                if stack.is_empty() {
                    ranges.push((start, i + 1));
                }
            }
            _ => {}
        }
    }
    ranges
}

/// 计算工具调用指纹（工具名 + 规范化参数的哈希），用于重复检测
fn fingerprint(tool_name: &str, args: &Value) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let canonical = canonicalize_json(args);
    let mut hasher = DefaultHasher::new();
    tool_name.hash(&mut hasher);
    canonical.hash(&mut hasher);
    format!("{}:{:016x}", tool_name, hasher.finish())
}

/// 规范化 JSON：递归按键名排序后序列化，保证相同内容不同键序产生相同输出
fn canonicalize_json(v: &Value) -> String {
    let mut sorted = v.clone();
    sort_object_keys(&mut sorted);
    serde_json::to_string(&sorted).unwrap_or_default()
}

/// 递归排序 JSON 对象的键
fn sort_object_keys(v: &mut Value) {
    match v {
        Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            for (k, mut val) in std::mem::take(map) {
                sort_object_keys(&mut val);
                sorted.insert(k, val);
            }
            for (k, val) in sorted {
                map.insert(k, val);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                sort_object_keys(item);
            }
        }
        _ => {}
    }
}

/// 参数注入：替换 `${result}` / `${step.N.result}` 占位符
///
/// - `${result}`：上一步阻塞工具的结果
/// - `${step.N.result}`：第 N 步（按 0 起算）的结果
fn inject_placeholders(args: &mut Value, history: &[ToolCallResult], last: &Option<Value>) {
    inject_value(args, history, last);
}

fn inject_value(v: &mut Value, history: &[ToolCallResult], last: &Option<Value>) {
    match v {
        Value::String(s) => {
            if s.contains("${result}") {
                if let Some(last_val) = last {
                    *s = s.replace("${result}", &value_to_injectable_string(last_val));
                }
            }
            if s.contains("${step.") {
                for (i, r) in history.iter().enumerate() {
                    let placeholder = format!("${{step.{}.result}}", i);
                    if s.contains(&placeholder) {
                        let replacement = r
                            .result
                            .as_ref()
                            .map(value_to_injectable_string)
                            .unwrap_or_default();
                        *s = s.replace(&placeholder, &replacement);
                    }
                }
            }
        }
        Value::Object(map) => {
            for (_, val) in map.iter_mut() {
                inject_value(val, history, last);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                inject_value(item, history, last);
            }
        }
        _ => {}
    }
}

/// 把 Value 转成可注入字符串（字符串直接取值，其他序列化为 JSON）
fn value_to_injectable_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// 检查参数中是否包含 `${result}` 或 `${step.N.result}` 占位符（存在则不能并行）
fn has_placeholders(v: &Value) -> bool {
    match v {
        Value::String(s) => s.contains("${result}") || s.contains("${step."),
        Value::Object(map) => map.values().any(has_placeholders),
        Value::Array(arr) => arr.iter().any(has_placeholders),
        _ => false,
    }
}

/// 并行执行累积的只读工具批次
///
/// - 使用 `futures::future::join_all` 并发执行所有累积的只读工具
/// - 执行完成后按原始顺序排序结果，保持顺序稳定
/// - 应用 context_modifier（read-only 工具也可能产出 modifier，如情绪检测）
/// - 同步到共享 context，让后续轮次感知 modifier 修改
async fn flush_parallel_batch(
    batch: &mut Vec<(String, Value, usize)>,
    tool_system: &Arc<ToolSystem>,
    ctx: &mut ToolUseContext,
    results: &mut Vec<ToolCallResult>,
    last_result: &mut Option<Value>,
    shared_context: &Arc<RwLock<ToolUseContext>>,
) {
    if batch.is_empty() {
        return;
    }

    let batch_count = batch.len();
    tracing::info!(
        "[ToolCallManager] 并行执行 {} 个只读工具: {:?}",
        batch_count,
        batch.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>()
    );

    let futures: Vec<_> = batch
        .drain(..)
        .map(|(name, args, idx)| {
            let ts = Arc::clone(tool_system);
            let ctx_snapshot = ctx.clone();
            async move {
                let args_for_result = args.clone();
                let result = execute_tool_use(&name, args, &ts, &ctx_snapshot, None).await;
                (name, args_for_result, result, idx)
            }
        })
        .collect();

    let mut batch_results = futures::future::join_all(futures).await;
    // 按原始顺序排序，保持结果顺序稳定
    batch_results.sort_by_key(|(_, _, _, idx)| *idx);

    for (tool_name, args, mut tool_result, iter_idx) in batch_results {
        let result_data = tool_result.data.clone();
        let success = tool_result.success;
        let error = tool_result.error.clone();

        // 应用上下文修改器（read-only 工具也可能产出 modifier，如情绪检测）
        if let Some(modifier) = tool_result.context_modifier.take() {
            modifier(ctx);
        }

        let status = if success {
            ToolCallStatus::Success
        } else {
            ToolCallStatus::Error
        };

        results.push(ToolCallResult {
            success,
            result: tool_result.data,
            tool_name,
            arguments: args,
            tool_call_id: format!("call_{}", iter_idx),
            error,
            status,
            requires_confirmation: false,
            goal_completed: tool_result.goal_completed,
        });

        *last_result = result_data;
    }

    // 同步到共享 context（让后续轮次感知 modifier 修改）
    *shared_context.write() = ctx.clone();
}

/// 工具列表元工具 - 生成可用工具列表给 AI
pub struct ToolListTool {
    tool_system: Arc<ToolSystem>,
}

impl ToolListTool {
    pub fn new(tool_system: Arc<ToolSystem>) -> Self {
        Self { tool_system }
    }

    /// 生成给 AI 的工具列表（紧凑格式，节省 token）
    pub fn get_tools_for_ai(&self) -> String {
        self.get_tools_for_ai_with_scene(ToolScene::Default, &HashSet::new(), "zh")
    }

    /// 按场景生成给 AI 的工具列表（场景化软提示 + 完整参数约束 + 权限标注 + 延迟加载）
    ///
    /// 输出格式（每个工具）：
    /// ```text
    /// - tool_name [需要确认]: tool description
    ///     - param_name (type)(required): description [enum: a|b|c] [range: 1-100] [default: 1]
    /// ```
    /// - `[需要确认]` 标记表示该工具执行前会弹窗请求用户许可
    /// - enum/range/default 帮助 LLM 生成符合约束的参数
    /// - `should_defer=true` 的工具只显示在末尾的 `<available-deferred-tools>` 块中
    ///   （仅工具名，无 schema），需要 LLM 调用 `tool_search` 拿到完整 schema 后才能调用
    /// - 场景不再硬屏蔽工具，改为在头部注入 `ToolScene::soft_hint()` 软提示，
    ///   引导 LLM 自主判断；危险操作由 `check_permissions` 在执行时确认
    pub fn get_tools_for_ai_with_scene(&self, scene: ToolScene, hidden: &HashSet<String>, lang: &str) -> String {
        let tools = self.tool_system.list_tools_for_scene(scene);
        let tools: Vec<_> = tools.into_iter().filter(|t| !hidden.contains(t.name())).collect();
        if tools.is_empty() {
            return match crate::pipeline::prompt_modules::normalize_lang(lang) {
                "en" => "No tools available".to_string(),
                "ja" => "利用可能なツールなし".to_string(),
                _ => "无可用工具".to_string(),
            };
        }

        // 三层可见性分离：Always（完整 schema）/ Lazy（名称+描述）/ Deferred（仅名称）
        // 可见性按场景动态判定（Chat/Task/Idle 三类场景下不同工具的暴露粒度不同）
        let mut always: Vec<_> = Vec::new();
        let mut lazy: Vec<_> = Vec::new();
        let mut deferred: Vec<_> = Vec::new();
        for t in &tools {
            match resolve_visibility(t, scene) {
                crate::tools::types::ToolVisibility::Always => always.push(t),
                crate::tools::types::ToolVisibility::Lazy => lazy.push(t),
                crate::tools::types::ToolVisibility::Deferred => deferred.push(t),
            }
        }

        // share_link 是网络检索专用工具：在 Chat/Idle 闲聊/后台场景下完全不注入（从 deferred 中也移除）
        if matches!(scene, ToolScene::Chat | ToolScene::Idle) {
            deferred.retain(|t| t.name() != "share_link");
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let (perm_mark_str, perm_notice, total_label, full_label, compact_label, deferred_label,
             compact_count_label, deferred_count_label, extra_label, schema_hint) = match lang_norm {
            "en" => (
                " [Confirmation Required]",
                "Tools marked [Confirmation Required] will prompt user for permission before execution.\n",
                "Total", "full", "compact", "deferred",
                "compact tools (name+description)", "deferred tools (name only)",
                "Also", ", full schema requires tool_search first.\n",
            ),
            "ja" => (
                " [要確認]",
                "[要確認]のツールは実行前にユーザーの許可を求めます。\n",
                "合計", "完全", "簡易", "遅延",
                "個の簡易ツール（名前+説明）", "個の遅延ツール（名前のみ）",
                "さらに", "、完全スキーマは tool_search で取得してください。\n",
            ),
            _ => (
                " [需要确认]",
                "标记 [需要确认] 的工具执行前会请求用户许可。\n",
                "总计", "完整", "精简", "延迟",
                "个精简工具（名称+描述）", "个延迟工具（仅名称）",
                "另有", "，完整 schema 需先用 tool_search 加载。\n",
            ),
        };

        let mut lines = vec!["# Available Tools\n".to_string()];
        lines.push(format!("{}: {} tools", total_label, tools.len()));
        lines.push(format!(
            "  ({}: {}, {}: {}, {}: {})\n",
            full_label, always.len(),
            compact_label, lazy.len(),
            deferred_label, deferred.len()
        ));
        // 场景软提示（不屏蔽工具，仅引导 LLM）
        let hint = scene.soft_hint();
        if !hint.is_empty() {
            lines.push(format!(
                "[Scene: {}] {}\n",
                scene_label(scene),
                hint
            ));
        }
        lines.push(perm_notice.to_string());

        // 延迟/精简加载提示
        if !deferred.is_empty() || !lazy.is_empty() {
            let mut hints = Vec::new();
            if !lazy.is_empty() {
                hints.push(format!("{} {}", lazy.len(), compact_count_label));
            }
            if !deferred.is_empty() {
                hints.push(format!("{} {}", deferred.len(), deferred_count_label));
            }
            lines.push(format!(
                "{}{}{}",
                extra_label,
                hints.join("、"),
                schema_hint
            ));
        }

        for tool in &always {
            let schema = tool.parameters_schema();
            let params = schema.get("properties").and_then(Value::as_object);
            let required: Vec<String> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let name = tool.name();
            let desc = tool.description();
            // 权限标注：destructive 工具或在确认列表中的工具标记
            let perm_mark = if tool.is_destructive() || is_confirmation_required_tool(name) {
                perm_mark_str
            } else {
                ""
            };

            if let Some(params) = params {
                if !params.is_empty() {
                    let mut param_strs = Vec::new();
                    for (p_name, p_info) in params {
                        let p_desc = p_info
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let p_type = p_info
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("string");
                        let req_mark = if required.contains(p_name) {
                            " (required)"
                        } else {
                            ""
                        };

                        // 收集约束信息：enum / range / default
                        let mut constraints: Vec<String> = Vec::new();

                        // enum 约束
                        if let Some(enum_vals) = p_info.get("enum").and_then(Value::as_array) {
                            let vals: Vec<String> = enum_vals
                                .iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect();
                            if !vals.is_empty() {
                                constraints.push(format!("enum: {}", vals.join("|")));
                            }
                        }

                        // 数值范围
                        let min = p_info.get("minimum").and_then(Value::as_f64);
                        let max = p_info.get("maximum").and_then(Value::as_f64);
                        match (min, max) {
                            (Some(lo), Some(hi)) => {
                                constraints.push(format!("range: {}-{}", lo, hi));
                            }
                            (Some(lo), None) => {
                                constraints.push(format!("min: {}", lo));
                            }
                            (None, Some(hi)) => {
                                constraints.push(format!("max: {}", hi));
                            }
                            _ => {}
                        }

                        // 默认值
                        if let Some(def) = p_info.get("default") {
                            let def_str = match def {
                                Value::String(s) => s.clone(),
                                Value::Number(n) => n.to_string(),
                                Value::Bool(b) => b.to_string(),
                                _ => def.to_string(),
                            };
                            if !def_str.is_empty() {
                                constraints.push(format!("default: {}", def_str));
                            }
                        }

                        let constraint_str = if constraints.is_empty() {
                            String::new()
                        } else {
                            format!(" [{}]", constraints.join(", "))
                        };

                        param_strs.push(format!(
                            "    - {} ({}){}: {}{}",
                            p_name, p_type, req_mark, p_desc, constraint_str
                        ));
                    }
                    lines.push(format!("- {}{}: {}", name, perm_mark, desc));
                    lines.extend(param_strs);
                } else {
                    lines.push(format!("- {}{}: {}", name, perm_mark, desc));
                }
            } else {
                lines.push(format!("- {}{}: {}", name, perm_mark, desc));
            }
        }

        // Lazy 工具：仅名称+一行描述（节省 token，完整 schema 通过 tool_search 加载）
        if !lazy.is_empty() {
            lines.push(String::new());
            let compact_header = crate::pipeline::prompt_modules::section_heading("compact_tools", lang);
            lines.push(format!("{}\n", compact_header));
            for tool in &lazy {
                let name = tool.name();
                let desc = tool.description();
                let perm_mark = if tool.is_destructive() || is_confirmation_required_tool(name) {
                    perm_mark_str
                } else {
                    ""
                };
                // 截取描述的第一行（避免长描述占用 token）
                let first_line = desc.lines().next().unwrap_or(desc);
                lines.push(format!("- {}{}: {}", name, perm_mark, first_line));
            }
        }

        // 末尾追加延迟/精简工具列表（仅工具名，供 tool_search 搜索）
        if !deferred.is_empty() || !lazy.is_empty() {
            lines.push(String::new());
            lines.push("<available-deferred-tools>".to_string());
            for t in &deferred {
                lines.push(t.name().to_string());
            }
            for t in &lazy {
                lines.push(t.name().to_string());
            }
            lines.push("</available-deferred-tools>".to_string());
            lines.push(String::new());
            lines.push(
                "这些工具未在上方列出完整 schema。若需调用，先调用 tool_search 拿到 schema。"
                    .to_string(),
            );
        }

        lines.join("\n")
    }

    /// 获取单个工具的 Markdown 描述
    pub fn get_tool_md(&self, tool_name: &str) -> String {
        let tool = match self.tool_system.find_tool(tool_name) {
            Some(t) => t,
            None => return format!("Unknown tool: {}", tool_name),
        };

        let mut lines = vec![
            format!("## {}", tool.name()),
            format!("Description: {}", tool.description()),
        ];

        let schema = tool.parameters_schema();
        if let Some(params) = schema.get("properties").and_then(Value::as_object) {
            if !params.is_empty() {
                lines.push("Parameters:".to_string());
                for (p_name, p_info) in params {
                    let p_type = p_info
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("any");
                    let desc = p_info
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    lines.push(format!("  - {} ({}): {}", p_name, p_type, desc));
                }
            }
        }

        lines.join("\n")
    }

    /// 执行元工具（返回工具列表）
    pub fn run(&self) -> String {
        self.get_tools_for_ai()
    }

    /// 按场景生成结构化工具定义（原生 function calling 路径使用）
    ///
    /// 与 `get_tools_for_ai_with_scene` 共享同一份场景筛选结果（现已不做硬过滤），
    /// 仅输出格式不同：
    /// - `get_tools_for_ai_with_scene` → Markdown 文本（注入 system prompt，含场景软提示）
    /// - `get_tool_definitions_for_scene` → `Vec<ToolDefinition>`（注入 API tools 字段）
    ///
    /// 延迟加载工具（`should_defer=true`）不在此列表中——它们通过 prompt 中的
    /// `<available-deferred-tools>` 块 + `tool_search` 元工具暴露给 LLM。
    pub fn get_tool_definitions_for_scene(
        &self,
        scene: ToolScene,
        hidden: &HashSet<String>,
    ) -> Vec<crate::providers::base::ToolDefinition> {
        self.tool_system
            .list_tools_for_scene(scene)
            .iter()
            .filter(|t| !hidden.contains(t.name()))
            .filter(|t| {
                matches!(
                    resolve_visibility(t, scene),
                    crate::tools::types::ToolVisibility::Always
                )
            })
            .map(|t| crate::providers::base::ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            })
            .collect()
    }
}

/// 场景的人类可读标签（给 LLM 看的提示）
fn scene_label(scene: ToolScene) -> &'static str {
    match scene {
        ToolScene::Default => "default",
        ToolScene::LowTrust => "low_trust",
        ToolScene::Focus => "focus",
        ToolScene::Chat => "chat",
        ToolScene::Task => "task",
        ToolScene::Idle => "idle",
    }
}

/// 判断工具是否属于"核心三件套"（search_memory / talk_to_character）
///
/// 这三个工具在所有场景下都保持 Always 可见：
/// - `tool_search` 通过 `always_load()=true` 自动保留
/// - `search_memory` / `talk_to_character` 通过此函数显式识别
fn is_core_tool(name: &str) -> bool {
    matches!(name, "search_memory" | "talk_to_character")
}

/// 推断工具的有效可见性层级
///
/// 优先级：
/// 1. `wallpaper_*` 前缀 → Always（强制始终可见，不受 should_defer 影响）
/// 2. `should_defer()=true` → Deferred（显式标记为延迟加载，跨场景不变）
/// 3. `always_load()=true` → Always（显式标记为必须加载，如 tool_search 自身）
/// 4. 场景矩阵：
///    - `Default` / `LowTrust` / `Focus`：Media/Mcp → Lazy，其他 Always
///    - `Chat`：仅核心工具 Always，其他 Lazy（节省 token，靠 tool_search 按需加载）
///    - `Task`：核心 + File/System/Memory/Pet/Web Always，Media/Mcp Lazy
///    - `Idle`：仅核心工具 Always，其他 Lazy
fn resolve_visibility(
    tool: &Arc<dyn Tool>,
    scene: ToolScene,
) -> crate::tools::types::ToolVisibility {
    use crate::tools::types::{ToolCategory, ToolVisibility};
    // wallpaper 工具强制 Always，优先于 should_defer 判定
    if tool.name().starts_with("wallpaper_") {
        return ToolVisibility::Always;
    }
    // share_link 是网络检索专用工具：
    // - Chat/Idle 闲聊/后台场景：完全隐藏（不注入）
    // - 其他场景（Default/LowTrust/Focus/Task，可能涉及 web_search）：完整注入
    if tool.name() == "share_link" {
        return match scene {
            ToolScene::Chat | ToolScene::Idle => ToolVisibility::Deferred,
            _ => ToolVisibility::Always,
        };
    }
    if tool.should_defer() {
        return ToolVisibility::Deferred;
    }
    if tool.always_load() {
        return ToolVisibility::Always;
    }
    let is_core = is_core_tool(tool.name());
    match scene {
        ToolScene::Default
        | ToolScene::LowTrust
        | ToolScene::Focus => match tool.category() {
            ToolCategory::Media | ToolCategory::Mcp => ToolVisibility::Lazy,
            _ => ToolVisibility::Always,
        },
        ToolScene::Chat => {
            if is_core {
                ToolVisibility::Always
            } else {
                ToolVisibility::Lazy
            }
        }
        ToolScene::Task => {
            if is_core {
                return ToolVisibility::Always;
            }
            match tool.category() {
                ToolCategory::Media | ToolCategory::Mcp => ToolVisibility::Lazy,
                _ => ToolVisibility::Always,
            }
        }
        ToolScene::Idle => {
            if is_core {
                ToolVisibility::Always
            } else {
                ToolVisibility::Lazy
            }
        }
    }
}

// ============================================================================
// ToolSearchTool - 延迟工具搜索元工具
// ============================================================================

/// 工具搜索元工具 - 让 LLM 按关键词或精确名称加载延迟工具的完整 schema
///
/// 行为：
/// - `select:A,B,C` — 按工具名精确加载（逗号分隔）
/// - 关键词搜索 — 匹配工具名 / search_hint / description，返回 top N
///
/// 返回结构是 `<functions>` 块，每行一个工具的完整 JSON schema，
/// LLM 拿到后即可像初始 prompt 中的工具一样调用。
///
/// 实现说明：持有 `Weak<ToolSystem>` 优先从活注册表搜索（自建工具在运行时注册，
/// 启动快照看不到），注册表已释放时回退到构造时快照；Weak 引用避免 Arc 循环。
pub struct ToolSearchTool {
    tools_snapshot: Arc<Vec<Arc<dyn Tool>>>,
    /// 工具系统弱引用：优先从活注册表搜索（自建工具运行时注册，快照看不到），
    /// Weak 避免 ToolSystem → tools map → ToolSearchTool 的 Arc 循环
    system: std::sync::Weak<ToolSystem>,
}

impl ToolSearchTool {
    pub fn new(tools_snapshot: Arc<Vec<Arc<dyn Tool>>>, system: std::sync::Weak<ToolSystem>) -> Self {
        Self { tools_snapshot, system }
    }

    /// 搜索域：活注册表优先（含运行时注册的自建工具），回退启动快照
    fn live_tools(&self) -> Vec<Arc<dyn Tool>> {
        match self.system.upgrade() {
            Some(ts) => ts.list_tools(),
            None => self.tools_snapshot.as_ref().clone(),
        }
    }

    /// 判断是否需要启用 ToolSearch（即是否存在 Deferred 或 Lazy 的工具）
    ///
    /// 注：此方法用 `ToolScene::Default` 作为基线判定，仅检查"是否存在可延迟工具"。
    /// 实际场景下的可见性由 `get_tools_for_ai_with_scene` 内的 `resolve_visibility(t, scene)` 动态判定。
    pub fn has_deferred_tools(tools: &[Arc<dyn Tool>]) -> bool {
        tools.iter().any(|t| {
            matches!(
                resolve_visibility(t, ToolScene::Default),
                crate::tools::types::ToolVisibility::Deferred
                    | crate::tools::types::ToolVisibility::Lazy
            )
        })
    }

    /// 生成 `<available-deferred-tools>` 块（仅工具名列表，每行一个）
    ///
    /// 注：此方法用 `ToolScene::Default` 作为基线。实际 prompt 中的 deferred 块
    /// 由 `get_tools_for_ai_with_scene` 按当前场景动态生成。
    pub fn format_deferred_tools_block(tools: &[Arc<dyn Tool>]) -> String {
        let deferred: Vec<String> = tools
            .iter()
            .filter(|t| {
                matches!(
                    resolve_visibility(t, ToolScene::Default),
                    crate::tools::types::ToolVisibility::Deferred
                        | crate::tools::types::ToolVisibility::Lazy
                )
            })
            .map(|t| t.name().to_string())
            .collect();
        if deferred.is_empty() {
            return String::new();
        }
        format!(
            "<available-deferred-tools>\n{}\n</available-deferred-tools>",
            deferred.join("\n")
        )
    }

    /// 执行搜索：返回匹配工具的完整 schema 字符串（`<functions>` 块）
    ///
    /// 搜索范围：全量工具快照（不按场景过滤）。
    /// 设计原因：场景化可见性会让同一工具在 Chat 场景为 Lazy、在 Default 场景为 Always，
    /// 若按场景过滤搜索域，LLM 在 Chat 场景下想加载 File 工具时会搜不到 schema。
    /// 因此 `tool_search` 始终在全量工具中搜索，由 LLM 自行判断是否调用。
    pub fn search(&self, query: &str, max_results: usize) -> (String, Vec<String>) {
        let live = self.live_tools();
        let all_tools = &live;

        // 1. select: 前缀精确加载（在全量工具中查找）
        if let Some(rest) = query.strip_prefix("select:").or_else(|| query.strip_prefix("SELECT:")) {
            let requested: Vec<String> = rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let mut found: Vec<String> = Vec::new();
            for name in &requested {
                // 先精确匹配，再规范化匹配（容错 LLM 输出大小写/分隔符偏差）
                if let Some(tool) = all_tools
                    .iter()
                    .find(|t| t.name() == name)
                    .or_else(|| {
                        let norm = normalize_tool_name(name);
                        all_tools
                            .iter()
                            .find(|t| normalize_tool_name(t.name()) == norm)
                    })
                    .cloned()
                {
                    found.push(tool.name().to_string());
                }
            }
            return (self.render_functions_block(&found), found);
        }

        // 2. 关键词搜索（在全量工具中搜索）
        let query_lower = query.to_lowercase();
        let query_terms: Vec<String> = query_lower
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        let mut scored: Vec<(String, i32)> = Vec::new();
        for tool in all_tools.iter() {
            let name = tool.name().to_lowercase();
            let hint = tool.search_hint().to_lowercase();
            let desc = tool.description().to_lowercase();
            let mut score: i32 = 0;
            for term in &query_terms {
                // 工具名精确包含
                if name == *term {
                    score += 10;
                } else if name.contains(term) {
                    score += 5;
                }
                // searchHint 匹配（高信号）
                if !hint.is_empty() && hint.contains(term) {
                    score += 4;
                }
                // description 匹配（低权重）
                if desc.contains(term) {
                    score += 2;
                }
            }
            if score > 0 {
                scored.push((tool.name().to_string(), score));
            }
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        let matches: Vec<String> = scored
            .into_iter()
            .take(max_results)
            .map(|(n, _)| n)
            .collect();
        (self.render_functions_block(&matches), matches)
    }

    /// 把匹配的工具渲染为 `<functions>` 块（每行一个工具的完整 schema）
    fn render_functions_block(&self, names: &[String]) -> String {
        if names.is_empty() {
            return "No matching deferred tools found".to_string();
        }
        let live = self.live_tools();
        let all_tools = &live;
        let mut lines: Vec<String> = vec!["<functions>".to_string()];
        for name in names {
            if let Some(tool) = all_tools.iter().find(|t| t.name() == name) {
                let schema = serde_json::json!({
                    "description": tool.description(),
                    "name": tool.name(),
                    "parameters": tool.parameters_schema(),
                });
                lines.push(format!("<function>{}</function>", schema));
            }
        }
        lines.push("</functions>".to_string());
        lines.join("\n")
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Search and load the full schema of deferred tools.\
         When you need to call a tool that appears in the <available-deferred-tools> block but has no full definition at the top of the prompt,\
         call this tool first to get its complete parameter schema so you can invoke it correctly.\n\
         Query forms:\n\
         - \"select:wallpaper_list,wallpaper_set\" — Load tools by exact name (comma-separated, recommended when you know the name)\n\
         - \"wallpaper\" — Keyword search, returns the top N most relevant results\n\
         The return structure is a <functions> block where each <function> contains description / name / parameters fields.\
         Once retrieved, you can call it like any tool defined in the initial prompt."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "搜索并加载延迟工具的完整 schema。当需要调用 <available-deferred-tools> 中列出但没有完整定义的工具时，\
                     先调用此工具获取其完整参数 schema，然后才能正确调用。\n\
                     查询形式：\n\
                     - \"select:wallpaper_list,wallpaper_set\" — 按名称精确加载（逗号分隔，推荐在已知名称时使用）\n\
                     - \"wallpaper\" — 关键词搜索，返回最相关的前 N 个结果\n\
                     返回结构为 <functions> 块，每个 <function> 包含 description / name / parameters 字段。\
                     获取后即可像初始提示中定义的工具一样调用。",
            "ja" => "遅延ツールの完全スキーマを検索して読み込む。<available-deferred-tools> に表示されているが完全な定義がないツールを呼び出す必要がある場合、\
                     まずこのツールで完全なパラメータスキーマを取得してから正しく呼び出してください。\n\
                     クエリ形式：\n\
                     - \"select:wallpaper_list,wallpaper_set\" — 名前で正確に読み込む（カンマ区切り、名前が分かっている場合に推奨）\n\
                     - \"wallpaper\" — キーワード検索、最も関連性の高い上位 N 件の結果を返す\n\
                     戻り値は <functions> ブロックで、各 <function> に description / name / parameters フィールドが含まれる。\
                     取得後は初期プロンプトで定義されたツールと同様に呼び出せる。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Query string. \"select:A,B,C\" loads tools by exact name; keyword search matches tool name/search_hint/description."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return for keyword search, default 5",
                    "minimum": 1
                }
            },
            "required": ["query"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> serde_json::Value {
        match lang {
            "zh" => serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "查询字符串。\"select:A,B,C\" 按名称精确加载工具；关键词搜索匹配工具名/搜索提示/描述。"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "关键词搜索返回的最大结果数，默认 5",
                        "minimum": 1
                    }
                },
                "required": ["query"]
            }),
            "ja" => serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "クエリ文字列。\"select:A,B,C\" は名前で正確にツールを読み込む；キーワード検索はツール名/検索ヒント/説明にマッチする。"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "キーワード検索の最大結果数、デフォルト 5",
                        "minimum": 1
                    }
                },
                "required": ["query"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> ValidationResult {
        let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.trim().is_empty() {
            return ValidationResult::failure("query 不能为空", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _ctx: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;
        let (block, matches) = self.search(query, max_results);
        ToolResult::standard_success(
            &format!("ToolSearch 返回 {} 个工具", matches.len()),
            Some(serde_json::json!({
                "matches": matches,
                "query": query,
                "max_results": max_results,
                "functions_block": block,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    // ToolSearch 自己永不延迟（否则鸡生蛋）
    fn always_load(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_array_of_tool_calls() {
        let resp = r#"[{"tool":"open_application","arguments":{"app_path":"notepad.exe"}}]"#;
        let calls = ToolCallManager::parse_tool_calls(resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "open_application");
    }

    #[test]
    fn parse_single_object_with_name_key() {
        let resp = r#"{"name":"web_search","arguments":{"query":"rust"}}"#;
        let calls = ToolCallManager::parse_tool_calls(resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "web_search");
    }

    #[test]
    fn parse_embedded_objects() {
        let resp = "好的，我来帮你\n{\"tool\":\"set_timer\",\"arguments\":{\"seconds\":10}}\n执行完毕";
        let calls = ToolCallManager::parse_tool_calls(resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "set_timer");
    }

    #[test]
    fn parse_tool_calls_aliases() {
        let resp = r#"{"tool":"read_file","args":{"path":"a.txt"}}"#;
        let calls = ToolCallManager::parse_tool_calls(resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "read_file");
    }

    #[test]
    fn extract_immediate_text_before_json() {
        let resp = "马上处理\n{\"tool\":\"open_url\",\"arguments\":{\"url\":\"https://example.com\"}}";
        let immediate = ToolCallManager::extract_immediate_response(resp);
        assert_eq!(immediate.as_deref(), Some("马上处理"));
    }

    #[test]
    fn extract_immediate_text_from_text_field() {
        let resp = r#"[{"text":"这是回复","tool":"open_url","arguments":{"url":"x"}}]"#;
        let immediate = ToolCallManager::extract_immediate_response(resp);
        assert_eq!(immediate.as_deref(), Some("这是回复"));
    }

    #[test]
    fn extract_immediate_text_after_json() {
        let resp = "{\"tool\":\"open_url\",\"arguments\":{\"url\":\"x\"}}\n执行完成";
        let immediate = ToolCallManager::extract_immediate_response(resp);
        assert_eq!(immediate.as_deref(), Some("执行完成"));
    }

    #[test]
    fn fingerprint_dedup_same_args() {
        let args = serde_json::json!({"path": "a.txt"});
        let a = fingerprint("read_file", &args);
        let b = fingerprint("read_file", &args);
        assert_eq!(a, b);
    }

    #[test]
    fn fingerprint_dedup_different_key_order() {
        let a = fingerprint("read_file", &serde_json::json!({"a":1,"b":2}));
        let b = fingerprint("read_file", &serde_json::json!({"b":2,"a":1}));
        assert_eq!(a, b);
    }

    #[test]
    fn fingerprint_differs_for_different_tools() {
        let args = serde_json::json!({"path": "a.txt"});
        let a = fingerprint("read_file", &args);
        let b = fingerprint("write_file", &args);
        assert_ne!(a, b);
    }

    #[test]
    fn inject_placeholder_simple_result() {
        let mut args = serde_json::json!({"query": "${result}"});
        let last = Some(serde_json::json!("previous output"));
        inject_placeholders(&mut args, &[], &last);
        assert_eq!(args["query"], "previous output");
    }

    #[test]
    fn inject_placeholder_step_index() {
        let mut args = serde_json::json!({"q": "${step.0.result}"});
        let history = vec![ToolCallResult {
            success: true,
            result: Some(serde_json::json!(42)),
            tool_name: "calc".into(),
            arguments: serde_json::Value::Null,
            tool_call_id: "call_1".into(),
            error: None,
            status: ToolCallStatus::Success,
            requires_confirmation: false,
            goal_completed: false,
        }];
        inject_placeholders(&mut args, &history, &None);
        assert_eq!(args["q"], "42");
    }

    #[test]
    fn inject_placeholder_nested_object() {
        let mut args = serde_json::json!({"filter": {"pattern": "${result}"}, "list": ["${result}", "fixed"]});
        let last = Some(serde_json::json!("matched"));
        inject_placeholders(&mut args, &[], &last);
        assert_eq!(args["filter"]["pattern"], "matched");
        assert_eq!(args["list"][0], "matched");
        assert_eq!(args["list"][1], "fixed");
    }

    #[test]
    fn non_blocking_tools_constant() {
        assert!(NON_BLOCKING_TOOLS.contains(&"open_application"));
        assert!(NON_BLOCKING_TOOLS.contains(&"take_screenshot"));
        assert!(NON_BLOCKING_TOOLS.contains(&"set_timer"));
        assert!(!NON_BLOCKING_TOOLS.contains(&"read_file"));
    }

    #[test]
    fn find_json_ranges_handles_nesting() {
        let text = r#"{"a":{"b":1}}"#;
        let ranges = find_json_ranges(text);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], (0, 13));
    }

    #[test]
    fn find_json_ranges_multiple_objects() {
        let text = "{\"a\":1} middle {\"b\":2}";
        let ranges = find_json_ranges(text);
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn extract_immediate_response_empty() {
        assert_eq!(ToolCallManager::extract_immediate_response(""), None);
        assert_eq!(ToolCallManager::extract_immediate_response("   "), None);
    }

    #[test]
    fn parse_tool_calls_empty() {
        assert!(ToolCallManager::parse_tool_calls("没有工具调用").is_empty());
        assert!(ToolCallManager::parse_tool_calls("").is_empty());
    }
}
