//! ReAct 工具循环共享骨架与对话阶段。
//!
//! 原生 function calling 的多轮工具循环只有一副骨架，两个入口
//! （`generation.rs` 的非流式 / 流式 `call_llm_native_fc*`）仅"首轮怎么拿响应"不同：
//! 首轮之后统一走本模块的 [`run_react_loop`]——每轮 = 窗口压缩 → 非流式响应 →
//! [`ReactLoop::react_round`] 处置（终止判定 / 工具执行回填 / 阶段迁移）。
//!
//! # 对话阶段（一等概念）
//!
//! [`DialoguePhase`] 取代散落的字符串补丁：
//! - 执行态：工具循环中间轮，system 换精简执行提示（persona token 经济），
//!   记忆要点随提示延续，防止中途丢上下文
//! - 表达态（完整人设）：首轮与最终回复
//!
//! 执行态 → 表达态的迁移方式由工具语义（[`ToolSemantics`]）决定：检索类注入
//! relay prompt 让角色转述关键内容，动作类直接收尾。工具语义由工具自身声明
//! （默认从 `is_read_only` 推导），新增工具无需改本模块。
//!
//! # 终止条件（多层防护，任一命中即迁移表达态收尾）
//!
//! 1. LLM 不再调用工具
//! 2. `goal_completed`（工具声明目标已达成）
//! 3. Doom loop（同签名重复调用达阈值，见 `doom_loop` 模块）
//! 4. 内部推演上限（`continue_thinking` 最多 4 次）
//! 5. 轮次上限（`max_rounds`，0 = 外部工具轮次不限，仍受 1-4 约束）
//!
//! # 轮 0 说明
//!
//! 轮 0 的消息是上游 prompt 步骤的产物，与文本路径同权，不做窗口压缩
//! （压缩职责在轮与轮之间，治理的是工具结果的累积）；轮 1 起每轮压缩。

use crate::brain::json_parser::JsonParser;
use crate::pipeline::doom_loop::{DoomLoopTracker, LoopStatus};
use crate::pipeline::steps::generation::{
    push_stream_chunk, AIResponseGenerationRunnable, SharedStreamEmitter,
};
use crate::providers::base::{StructuredToolCall, ToolDefinition};
use crate::providers::ModelRouter;
use crate::tools::tool_call_manager::{ToolCallManager, ToolCallResult};
use crate::tools::types::ToolSemantics;
use crate::types::response::{ChatMessage, MessageToolCall};

const CONTINUE_THINKING_TOOL: &str =
    crate::tools::builtin::thinking_tools::CONTINUE_THINKING_TOOL;
/// 内部推演是软循环而不是无限思维链；达到上限后必须给出当前最佳答案。
const MAX_COMPANION_DELIBERATION_ROUNDS: usize = 4;

// ============================================================================
// 对话阶段与阶段提示
// ============================================================================

/// 对话阶段：ReAct 工具循环的一等状态
///
/// - [`DialoguePhase::Persona`]：完整人设（首轮与最终回复）
/// - [`DialoguePhase::Execution`]：精简执行提示（工具循环中间轮，节省 persona token）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialoguePhase {
    /// 完整人设 system
    Persona,
    /// 精简执行提示 system
    Execution,
}

/// 工具执行阶段的精简系统提示（中间轮次不加载完整人设，节省 token）
const TOOL_EXECUTION_SYSTEM_PROMPT: &str =
    "你是一个工具执行助手。根据用户请求和工具返回的结果，决定下一步操作。\n\
     原则：\n\
     - 资料够用就停：搜索 2-3 次后若已有足够信息，立即执行能直接完成用户请求的工具（如 create_notebook / save_memory / share_link 等），不要为了追求更全而继续搜索。\n\
     - 轮次有限：工具调用轮次有上限，优先执行能直接达成目标的工具，而非反复检索。\n\
     - 对不确定的信息优先搜索而非猜测。\n\
     - 如果任务已完成，简要汇报结果。";

/// 精简工具执行提示 + 记忆延续块
///
/// native FC 多轮工具循环中，中间轮次切换到精简 system prompt 以节省 persona token；
/// 若本轮已检索到相关记忆，把记忆区块一并带到后续轮次，避免`[相关记忆]`仅存在于
/// 首轮导致中途丢失上下文、最终作答脱离记忆（如"你认识AlenTinn吗"被答成不认识）。
fn minimal_execution_prompt(memory_text: &str) -> String {
    let trimmed = memory_text.trim();
    if trimmed.is_empty() {
        return TOOL_EXECUTION_SYSTEM_PROMPT.to_string();
    }
    let lang = crate::i18n::get_language();
    let heading = if lang.starts_with("en") {
        "\n\nRelevant memories (already in your head — keep them in mind when replying):\n"
    } else if lang.starts_with("ja") {
        "\n\n関連記憶（頭の中にある記憶。返答時はこれも参照すること）：\n"
    } else {
        "\n\n相关记忆（你脑中已有的记忆，作答时一并参考）：\n"
    };
    format!("{}{}{}", TOOL_EXECUTION_SYSTEM_PROMPT, heading, trimmed)
}

/// 跨角色对话场景的工具执行提示
///
/// talk_to_character 返回的是对方角色的回复，不是"工具结果"。
/// 源角色应基于回复内容自然决定是否继续聊下去，而不是"汇报结果"就终止。
const CROSS_CHARACTER_EXECUTION_PROMPT: &str =
    "你正在与另一个角色对话。talk_to_character 返回的是她对你的回复。\
     根据她的回复内容决定下一步：如果话题还有得聊，自然地接话继续；\
     如果她问了问题就回答，如果她分享了什么就回应。\
     不要把对话当成任务来汇报结果，保持自然的聊天节奏。\
     仅当对话真的自然结束时才停止。";

/// 检测工具调用列表中是否包含 talk_to_character
fn has_cross_character_call(calls: &[StructuredToolCall]) -> bool {
    calls.iter().any(|c| c.name == "talk_to_character")
}

fn is_deliberation_only(calls: &[StructuredToolCall]) -> bool {
    !calls.is_empty() && calls.iter().all(|c| c.name == CONTINUE_THINKING_TOOL)
}

fn persona_reply_was_already_streamed(rounds: usize) -> bool {
    rounds == 1
}

/// 工具查到信息后，提醒角色用自己的语气转述关键内容（三语言 + 渠道感知）。
/// 与 goal_completed 分支逻辑一致：恢复完整人设后注入，避免角色只给一句评价就结束。
/// 气泡渠道（direct/proactive）精简转述，微信渠道（wechat）可详细展开。
fn tool_retrieval_relay_prompt(channel: &str) -> String {
    let lang = crate::i18n::get_language();
    let is_bubble = matches!(channel, "direct" | "proactive");
    if lang.starts_with("en") {
        if is_bubble {
            "[System] You just retrieved information via a tool. Summarize the key point to the user in ONE or TWO sentences in your own voice — be concise, don't dump everything. Don't give only a comment or reaction.".to_string()
        } else {
            "[System] You just retrieved information via a tool. Paraphrase the key findings to the user in your own voice — don't give only a comment or reaction.".to_string()
        }
    } else if lang.starts_with("ja") {
        if is_bubble {
            "[システム] ツールで情報を取得しました。キャラクターの口調で要点を1〜2文で伝えてください。長々とまとめず、簡潔に。感想や反応だけではいけません。".to_string()
        } else {
            "[システム] ツールで情報を取得しました。キャラクターの口調で検索内容の要点をユーザーに伝えてください。感想や反応だけではいけません。".to_string()
        }
    } else {
        if is_bubble {
            "[系统提示] 你刚才通过工具查到了信息。用你的语气把最关键的一点用一两句话告诉用户就行，不要长篇总结。不要只给出评价或反应。".to_string()
        } else {
            "[系统提示] 你刚才通过工具查到了信息。请用你的语气把查到的关键内容转述给用户，不要只给出评价或反应。".to_string()
        }
    }
}

/// 工具目标完成后，提醒角色告知用户结果（渠道感知）。
/// 气泡渠道精简，微信渠道可展开。
fn goal_completed_prompt(channel: &str) -> String {
    let lang = crate::i18n::get_language();
    let is_bubble = matches!(channel, "direct" | "proactive");
    if lang.starts_with("en") {
        if is_bubble {
            "[System] The user's goal was completed via tool calls. Tell the user the result in ONE or TWO sentences in character — be concise. Do not call any more tools.".to_string()
        } else {
            "[System] The user's goal was completed via tool calls. Briefly tell the user the result in character. Do not call any more tools.".to_string()
        }
    } else if lang.starts_with("ja") {
        if is_bubble {
            "[システム] ユーザーの目標はツール呼び出しで完了しました。1〜2文で簡潔に結果を伝えてください。ツールは呼ばないで。".to_string()
        } else {
            "[システム] ユーザーの目標はツール呼び出しで完了しました。キャラクターの口調で簡潔に結果を伝えてください。ツールは呼ばないで。".to_string()
        }
    } else {
        if is_bubble {
            "[系统提示] 用户的目标已通过工具调用完成。用一两句话简短告知用户结果就行，不要再调用任何工具。".to_string()
        } else {
            "[系统提示] 用户的目标已通过工具调用完成。请以角色人设口吻简短告知用户结果，不要再调用任何工具。".to_string()
        }
    }
}

/// 工具调用轮次达上限，强制回复（渠道感知）。
/// 气泡渠道精简，微信渠道可展开。
fn round_limit_prompt(channel: &str) -> String {
    let lang = crate::i18n::get_language();
    let is_bubble = matches!(channel, "direct" | "proactive");
    if lang.starts_with("en") {
        if is_bubble {
            "[System] Tool call rounds reached the limit. Reply to the user NOW in ONE or TWO sentences — briefly say what you did. Don't stay silent.".to_string()
        } else {
            "[System] Tool call rounds reached the limit. You must reply to the user now. Briefly explain what you did and what went wrong. Don't stay silent.".to_string()
        }
    } else if lang.starts_with("ja") {
        if is_bubble {
            "[システム] ツール呼び出し回数が上限に達しました。1〜2文で簡潔に何をしたか伝えてください。黙らないで。".to_string()
        } else {
            "[システム] ツール呼び出し回数が上限に達しました。今すぐ返信してください。何をしたか、何が問題だったか簡潔に。黙らないで。".to_string()
        }
    } else {
        if is_bubble {
            "[系统提示] 工具调用轮次已达上限，你必须立即回复用户。一两句话简要说明你做了什么就行，不要沉默。".to_string()
        } else {
            "[系统提示] 工具调用轮次已达上限，你必须立即回复用户。简要说明你做了什么、遇到了什么问题，不要沉默。".to_string()
        }
    }
}

// ============================================================================
// 工具结果回填与延迟加载
// ============================================================================

/// 工具结果 → `role=tool` 消息体
///
/// 成功：序列化 result payload；失败：透传完整结构化 payload
/// （含 candidates/next_action 等细节），让 LLM 能基于结构化数据消歧或重试。
fn tool_result_to_message_body(r: &ToolCallResult) -> String {
    if r.success {
        r.result
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string()))
            .unwrap_or_else(|| "null".to_string())
    } else if let Some(payload) = &r.result {
        serde_json::to_string(payload).unwrap_or_else(|_| {
            format!(
                "{{\"error\": {}}}",
                serde_json::Value::String(r.error.clone().unwrap_or_default())
            )
        })
    } else {
        format!(
            "{{\"error\": {}}}",
            serde_json::Value::String(r.error.clone().unwrap_or_default())
        )
    }
}

/// 延迟加载辅助函数：从工具调用结果中检测 `tool_search` 调用，
/// 把返回的匹配工具加入 `tools` 列表，供下一轮原生 FC 调用。
///
/// 工具名来源优先级：
/// 1. `select:A,B,C` 查询：直接从工具调用参数中提取工具名（最可靠，
///    不受结果截断影响——`functions_block` 过大时整个 result 会被
///    `enforce_result_budget` 替换为 `_truncated` 预览版，`matches` 字段丢失）
/// 2. 关键词查询：从 `result.data.matches` 或 `result.matches` 提取工具名
///    （`standard_success` 把实际数据包在 `data` 字段里，需兼容两种结构）
fn inject_deferred_tools_from_results(
    calls: &[StructuredToolCall],
    results: &[ToolCallResult],
    tool_call_manager: &ToolCallManager,
    tools: &mut Vec<ToolDefinition>,
) {
    for (i, r) in results.iter().enumerate() {
        let tc = match calls.get(i) {
            Some(c) => c,
            None => continue,
        };
        if tc.name != "tool_search" || !r.success {
            continue;
        }

        let tool_names: Vec<String> = extract_tool_search_matches(tc, r);

        for name in tool_names {
            if let Some(tool) = tool_call_manager.tool_system().find_tool(&name) {
                let def = ToolDefinition {
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    parameters: tool.parameters_schema(),
                };
                if !tools.iter().any(|t| t.name == def.name) {
                    tracing::info!("[react] tool_search 加载延迟工具: {}", def.name);
                    tools.push(def);
                }
            }
        }
    }
}

/// 从 `tool_search` 的调用参数和结果中提取匹配的工具名。
///
/// 优先解析 `select:` 查询参数（不受结果截断影响），
/// 关键词查询则从结果 JSON 的 `matches` 字段提取（兼容 `standard_success` 的 `data` 包装）。
fn extract_tool_search_matches(
    tc: &StructuredToolCall,
    r: &ToolCallResult,
) -> Vec<String> {
    if let Some(query) = tc.arguments.get("query").and_then(|v| v.as_str()) {
        if let Some(rest) = query
            .strip_prefix("select:")
            .or_else(|| query.strip_prefix("SELECT:"))
        {
            return rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    let data = match &r.result {
        Some(d) => d,
        None => return Vec::new(),
    };
    let matches_value = data
        .get("matches")
        .or_else(|| data.get("data").and_then(|d| d.get("matches")));
    match matches_value.and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        None => Vec::new(),
    }
}

// ============================================================================
// 循环骨架
// ============================================================================

/// 单轮 ReAct 骨架的处置结果
#[derive(Debug)]
enum RoundOutcome {
    /// LLM 不再调用工具：表达态迁移已完成，`final_content` 即最终回复
    Done,
    /// 本轮工具已执行并回填，继续下一轮
    Continue,
    /// 提前终止（goal_completed / doom loop），收尾回复已生成
    Terminated,
}

/// 共享循环的调用上下文（入口一次性构造，避免长参数列表穿透各层）
struct ReactCtx<'a> {
    router: &'a ModelRouter,
    tool_call_manager: &'a ToolCallManager,
    emitter: &'a SharedStreamEmitter,
    task_type: &'a str,
    channel: &'a str,
    memory_text: &'a str,
    compress_threshold_tokens: usize,
    compress_keep_recent: usize,
}

impl ReactCtx<'_> {
    /// 工具语义：查询注册表，由工具自身声明（默认按 `is_read_only` 推导）
    ///
    /// 查不到（未注册 / 已卸载）按动作类处理：宁可少一次 relay 提醒，
    /// 不给动作类结果强加"转述信息"的指令。
    fn tool_semantics(&self, name: &str) -> ToolSemantics {
        self.tool_call_manager
            .tool_system()
            .find_tool(name)
            .map(|t| t.semantics())
            .unwrap_or(ToolSemantics::Action)
    }
}

/// 共享 ReAct 循环入参（两入口仅首轮字段不同）
pub(crate) struct ReactParams {
    /// 首轮文本（入口已推送前端；同时作为 assistant 消息内容回填对话）
    pub(crate) first_content: String,
    /// 首轮工具调用（空 = 首轮即最终回复，循环不启动）
    pub(crate) first_calls: Vec<StructuredToolCall>,
    /// 对话消息（完整人设 system + 历史 + 本轮用户输入；不含首轮 assistant/tool 消息）
    pub(crate) messages: Vec<ChatMessage>,
    /// 初始工具集（循环中经延迟加载扩充）
    pub(crate) tools: Vec<ToolDefinition>,
    pub(crate) task_type: String,
    pub(crate) channel: String,
    pub(crate) memory_text: String,
    /// 轮次上限（0 = 无限，由终止条件自然结束）
    pub(crate) max_rounds: u32,
    pub(crate) compress_threshold_tokens: usize,
    pub(crate) compress_keep_recent: usize,
}

/// 共享 ReAct 循环状态
struct ReactLoop {
    /// 对话消息（system 槽位随阶段切换，其余只增）
    messages: Vec<ChatMessage>,
    /// 完整人设 system（表达态恢复用）
    original_system: ChatMessage,
    phase: DialoguePhase,
    /// 工具集（延迟加载扩充）
    tools: Vec<ToolDefinition>,
    all_results: Vec<ToolCallResult>,
    all_tool_names: Vec<String>,
    rounds: usize,
    final_content: String,
    first_tool_executed_at: Option<f64>,
    doom_tracker: DoomLoopTracker,
}

impl ReactLoop {
    fn new(messages: Vec<ChatMessage>, tools: Vec<ToolDefinition>) -> Self {
        // 约定：messages[0] 为完整人设 system（由 prompt 步骤保证，入口不传空表）
        let original_system = messages[0].clone();
        Self {
            messages,
            original_system,
            phase: DialoguePhase::Persona,
            tools,
            all_results: Vec::new(),
            all_tool_names: Vec::new(),
            rounds: 0,
            final_content: String::new(),
            first_tool_executed_at: None,
            doom_tracker: DoomLoopTracker::default(),
        }
    }

    fn finish(self) -> (String, Vec<ToolCallResult>, usize, Option<f64>) {
        (
            self.final_content,
            self.all_results,
            self.rounds,
            self.first_tool_executed_at,
        )
    }

    /// 回到表达态：恢复完整人设 system（幂等）
    fn restore_persona(&mut self) {
        if self.phase == DialoguePhase::Execution {
            self.messages[0] = self.original_system.clone();
            self.phase = DialoguePhase::Persona;
        }
    }

    /// 进入执行态：换精简执行提示（幂等；跨角色对话用专用变体，
    /// 同时把记忆要点带入后续轮次，防止中途丢失上下文）
    fn enter_execution(&mut self, calls: &[StructuredToolCall], memory_text: &str) {
        if self.phase != DialoguePhase::Persona {
            return;
        }
        // 纯内部推演不应卸载角色人格。下一轮仍在陪伴者的人格和完整上下文中
        // 继续分析；只有真正进入工具执行时才切换精简执行提示。
        if is_deliberation_only(calls) {
            return;
        }
        let prompt = if has_cross_character_call(calls) {
            CROSS_CHARACTER_EXECUTION_PROMPT.to_string()
        } else {
            minimal_execution_prompt(memory_text)
        };
        self.messages[0] = ChatMessage::system(prompt);
        self.phase = DialoguePhase::Execution;
    }

    /// 全程工具语义聚合：任一检索类工具命中 → 检索语义
    fn aggregate_semantics(&self, ctx: &ReactCtx<'_>) -> ToolSemantics {
        if self
            .all_tool_names
            .iter()
            .any(|n| ctx.tool_semantics(n) == ToolSemantics::Retrieval)
        {
            ToolSemantics::Retrieval
        } else {
            ToolSemantics::Action
        }
    }

    /// 窗口压缩：轮与轮之间治理对话总长度，避免工具结果累积撑爆上下文
    async fn compress_window(&mut self, ctx: &ReactCtx<'_>) {
        let mid_end = self.messages.len().saturating_sub(ctx.compress_keep_recent);
        let snapshot = if mid_end > 1 {
            Some(crate::pipeline::compaction_reminder::CompactionSnapshot::from_mid_section(
                &self.messages, 1, mid_end,
            ))
        } else {
            None
        };

        let query = crate::pipeline::context_compress::extract_query(&self.messages);
        let compress_result = crate::pipeline::context_compress::compress_conversation_context_aware(
            ctx.router,
            ctx.task_type,
            &mut self.messages,
            ctx.compress_threshold_tokens,
            ctx.compress_keep_recent,
            &query,
        )
        .await;
        if compress_result.saved_tokens > 0 {
            tracing::info!(
                "[react] 第 {} 轮前窗口压缩：节省约 {} tokens，压缩 {} 段历史",
                self.rounds + 1,
                compress_result.saved_tokens,
                compress_result.dropped_groups
            );

            // 注入压缩后的上下文提醒（帮助 LLM 记住之前在执行什么）
            if compress_result.dropped_groups > 0 {
                if let Some(ref snap) = snapshot {
                    if let Some(reminder) = snap.build_reminder(compress_result.dropped_groups) {
                        // 插入到 head 消息之后、中段之前
                        self.messages.insert(1, reminder);
                    }
                }
            }
        }
    }

    /// 执行一轮工具调用并回填对话（assistant 工具调用消息 + `role=tool` 结果消息）
    async fn execute_and_append(
        &mut self,
        ctx: &ReactCtx<'_>,
        assistant_content: &str,
        calls: &[StructuredToolCall],
    ) -> Vec<ToolCallResult> {
        let assistant_msg = ChatMessage::assistant_with_tool_calls(
            assistant_content.to_string(),
            calls
                .iter()
                .map(|tc| MessageToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                })
                .collect(),
        );
        self.messages.push(assistant_msg);

        let results = ctx.tool_call_manager.execute_structured_calls(calls).await;
        tracing::info!(
            "[react] 第 {} 轮执行 {} 个工具调用",
            self.rounds,
            results.len()
        );
        if self.first_tool_executed_at.is_none() && !results.is_empty() {
            self.first_tool_executed_at = Some(crate::memory::types::current_timestamp());
        }

        for (i, r) in results.iter().enumerate() {
            let tc_id = calls
                .get(i)
                .map(|c| c.id.clone())
                .unwrap_or_else(|| format!("call_{}", i));
            // 单条工具结果预截断：避免巨大的工具输出（如 read_file 读取大文件）
            // 直接撑爆上下文窗口。截断后再交给后续压缩流水线处理。
            let body = crate::pipeline::context_compress::truncate_tool_result(&tool_result_to_message_body(r));
            self.messages.push(ChatMessage::tool_result(body, tc_id));
        }
        self.all_results.extend(results.clone());
        self.all_tool_names.extend(calls.iter().map(|c| c.name.clone()));
        results
    }

    /// 表达态收尾：恢复人设 →（可选）注入收尾指令 → 生成最终回复并推送
    ///
    /// `with_tools=true` 保留工具通道（doom loop 打断后允许模型说明状态）；
    /// 生成失败时保留既有 `final_content`，不中断主流程。
    async fn wrap_up(&mut self, ctx: &ReactCtx<'_>, prompt: Option<String>, with_tools: bool) {
        self.restore_persona();
        if let Some(p) = prompt {
            self.messages.push(ChatMessage::user(p));
        }
        let reply = if with_tools {
            match ctx
                .router
                .generate_with_tools(
                    AIResponseGenerationRunnable::build_chat_request(
                        ctx.task_type,
                        self.messages.clone(),
                    )
                    .with_tools(self.tools.clone()),
                )
                .await
            {
                Ok(resp) => resp.content,
                Err(e) => {
                    tracing::warn!("[react] 收尾生成失败: {}", e);
                    String::new()
                }
            }
        } else {
            ctx.router
                .generate(AIResponseGenerationRunnable::build_chat_request(
                    ctx.task_type,
                    self.messages.clone(),
                ))
                .await
                .unwrap_or_default()
        };
        // 防御：模型偶尔仍包 JSON，提取 text 字段（纯文本时原样返回）
        let text = JsonParser::extract_text(&reply).unwrap_or(reply);
        if !text.is_empty() {
            push_stream_chunk(ctx.emitter, &text);
            self.final_content = text;
        }
    }

    /// 处置一轮 LLM 响应：终止判定 / 工具执行回填 / 阶段迁移（骨架核心）
    ///
    /// `content` 为本轮 LLM 文本（首轮由入口传入，后续轮为非流式响应），
    /// `calls` 为本轮工具调用。所有分支的消息回填在此完成。
    async fn react_round(
        &mut self,
        ctx: &ReactCtx<'_>,
        content: &str,
        calls: &[StructuredToolCall],
    ) -> RoundOutcome {
        self.rounds += 1;
        // 防御：模型偶尔仍包 JSON，提取 text 字段（纯文本时原样返回）
        let draft = JsonParser::extract_text(content).unwrap_or_else(|| content.to_string());
        self.final_content = draft.clone();

        // ── 无工具调用：表达态迁移，循环结束 ──
        if calls.is_empty() {
            if self.phase == DialoguePhase::Persona {
                // 首轮即最终回复时文本已由入口推送；纯内部推演会保持 Persona，
                // 其后续轮是非流式生成，必须在这里补推最终文本。
                if !persona_reply_was_already_streamed(self.rounds) && !draft.is_empty() {
                    push_stream_chunk(ctx.emitter, &draft);
                }
                return RoundOutcome::Done;
            }
            tracing::debug!("[react] 第 {} 轮无工具调用，结束循环", self.rounds);
            let semantics = self.aggregate_semantics(ctx);
            self.restore_persona();
            match semantics {
                ToolSemantics::Retrieval => {
                    // 检索类：注入 relay prompt，让角色用自己的语气转述关键内容
                    if !draft.is_empty() {
                        self.messages.push(ChatMessage::assistant(&draft));
                    }
                    self.messages
                        .push(ChatMessage::user(tool_retrieval_relay_prompt(ctx.channel)));
                    let persona_reply = ctx
                        .router
                        .generate(AIResponseGenerationRunnable::build_chat_request(
                            ctx.task_type,
                            self.messages.clone(),
                        ))
                        .await
                        .unwrap_or_default();
                    let persona_text =
                        JsonParser::extract_text(&persona_reply).unwrap_or(persona_reply);
                    if !persona_text.is_empty() {
                        push_stream_chunk(ctx.emitter, &persona_text);
                        self.final_content = persona_text;
                    }
                }
                ToolSemantics::Action => {
                    // 动作类：LLM 回复直接作为最终回复，不再额外调 LLM
                    if !draft.is_empty() {
                        push_stream_chunk(ctx.emitter, &draft);
                    }
                }
            }
            return RoundOutcome::Done;
        }

        // ── 工具执行与回填 ──
        let results = self.execute_and_append(ctx, content, calls).await;

        // 陪伴侧允许模型主动续思考，但不能形成无界的“反思自己是否还需反思”。
        // 第四次 checkpoint 已执行并回填；此时恢复完整人格，强制产出最佳答案。
        let deliberation_rounds = self
            .all_tool_names
            .iter()
            .filter(|name| name.as_str() == CONTINUE_THINKING_TOOL)
            .count();
        if deliberation_rounds >= MAX_COMPANION_DELIBERATION_ROUNDS {
            tracing::warn!(
                "[react] 陪伴侧内部推演达到 {} 轮上限，强制收尾",
                MAX_COMPANION_DELIBERATION_ROUNDS
            );
            self.wrap_up(
                ctx,
                Some(
                    "[系统提示] 内部推演预算已用完。请停止继续思考或调用工具，基于现有信息直接给出当前最佳答案；如确实缺少只有用户知道的信息，简短说明并提出一个必要问题。"
                        .to_string(),
                ),
                false,
            )
            .await;
            return RoundOutcome::Terminated;
        }

        // Goal Satisfaction：工具声明目标已完成 → 表达态收尾。
        // 避免 LLM 在任务已达成时继续推理出多余动作（如壁纸切换成功后又去 web_search 找图）。
        if results.iter().any(|r| r.goal_completed) {
            tracing::info!(
                "[react] 第 {} 轮检测到 goal_completed，提前终止工具循环",
                self.rounds
            );
            self.wrap_up(ctx, Some(goal_completed_prompt(ctx.channel)), false)
                .await;
            return RoundOutcome::Terminated;
        }

        // Doom Loop：同 (tool, args) 签名重复调用达阈值 → 注入打断消息，表达态收尾
        let round_calls: Vec<(String, serde_json::Value)> = calls
            .iter()
            .map(|tc| (tc.name.clone(), tc.arguments.clone()))
            .collect();
        let loop_status = self.doom_tracker.record_round(&round_calls);
        if let LoopStatus::Doomed { ref tool, count } = loop_status {
            tracing::warn!(
                "[react] Doom loop 检测：工具 `{}` 已被相同参数调用 {} 次，注入打断消息",
                tool, count
            );
            if let Some(msg) = DoomLoopTracker::build_intervention_message(&loop_status) {
                self.messages.push(ChatMessage::user(&msg));
            }
            self.wrap_up(ctx, None, true).await;
            return RoundOutcome::Terminated;
        }

        // 延迟加载：本轮若调用了 tool_search，把返回的工具加入下一轮工具集
        inject_deferred_tools_from_results(calls, &results, ctx.tool_call_manager, &mut self.tools);

        // 首轮工具执行完毕 → 进入执行态（后续轮精简 persona）
        self.enter_execution(calls, ctx.memory_text);

        RoundOutcome::Continue
    }
}

/// 运行共享 ReAct 工具循环。
///
/// 入口已获取首轮响应并推送首轮文本；本函数接管其余一切：首轮工具执行、
/// 执行态切换、后续轮次（窗口压缩 → 非流式响应 → 处置）、以及四类终止
/// 条件下的表达态收尾。
pub(crate) async fn run_react_loop(
    router: &ModelRouter,
    tool_call_manager: &ToolCallManager,
    emitter: &SharedStreamEmitter,
    params: ReactParams,
) -> crate::error::VivianResult<(String, Vec<ToolCallResult>, usize, Option<f64>)> {
    let ReactParams {
        first_content,
        first_calls,
        messages,
        tools,
        task_type,
        channel,
        memory_text,
        max_rounds,
        compress_threshold_tokens,
        compress_keep_recent,
    } = params;
    let ctx = ReactCtx {
        router,
        tool_call_manager,
        emitter,
        task_type: &task_type,
        channel: &channel,
        memory_text: &memory_text,
        compress_threshold_tokens,
        compress_keep_recent,
    };
    let mut lp = ReactLoop::new(messages, tools);

    // 首轮处置（无工具调用则直接以首轮文本收场，rounds=1）
    match lp.react_round(&ctx, &first_content, &first_calls).await {
        RoundOutcome::Continue => {}
        _ => return Ok(lp.finish()),
    }

    // 后续轮次；0 = 无限（由终止条件自然结束）
    let max_rounds = if max_rounds == 0 {
        usize::MAX
    } else {
        max_rounds.max(1) as usize
    };
    for _ in 1..max_rounds {
        lp.compress_window(&ctx).await;
        let resp = router
            .generate_with_tools(
                AIResponseGenerationRunnable::build_chat_request(ctx.task_type, lp.messages.clone())
                    .with_tools(lp.tools.clone()),
            )
            .await?;
        match lp.react_round(&ctx, &resp.content, &resp.tool_calls).await {
            RoundOutcome::Continue => {}
            _ => return Ok(lp.finish()),
        }
    }

    // 轮次上限：表达态强制收尾，让角色向用户交代进展
    tracing::warn!(
        "[react] 工具调用达到 {} 轮上限，强制生成最终回复",
        lp.rounds
    );
    lp.wrap_up(&ctx, Some(round_limit_prompt(ctx.channel)), false)
        .await;
    Ok(lp.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str) -> StructuredToolCall {
        StructuredToolCall {
            id: format!("call_{name}"),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }
    }

    #[test]
    fn pure_deliberation_keeps_persona_phase() {
        assert!(is_deliberation_only(&[call(CONTINUE_THINKING_TOOL)]));
        assert!(!is_deliberation_only(&[]));
        assert!(!is_deliberation_only(&[
            call(CONTINUE_THINKING_TOOL),
            call("web_search"),
        ]));
        assert!(persona_reply_was_already_streamed(1));
        assert!(!persona_reply_was_already_streamed(2));
    }
}
