//! AI 响应生成流水线步骤：响应生成与解析。
//!
//! - [`AIResponseGenerationRunnable`]：智能路由 + 故障降级 + graceful_exit 告别生成
//! - [`ResponseParsingRunnable`]：使用 `JsonProcessor::process_response` 解析响应，
//!   提取 text / motion / expression / importance / long_term_memory / intent /
//!   user_emotion / ai_emotion / tool_calls

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::{json, Value};

use crate::brain::json_parser::{
    JsonParser, JsonProcessor, ProcessedResponse, StreamingJsonParser,
    StreamEvent as JsonStreamEvent,
};
use crate::cross_character::parse_any_speaker_prefix;
use crate::error::{VivianError, VivianResult};
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::state::PipelineState;
use crate::providers::base::{
    LLMRequest, ProviderCallOptions, StreamEvent as ProviderStreamEvent, ToolDefinition,
};
use crate::providers::{ModelRouter, vivian_response_schema};
use crate::tools::tool_call_manager::{ToolCallManager, ToolCallResult};
use crate::types::response::{AiResponse, ChatMessage};

/// 流式 chunk 推送回调：每个 chunk 调用此回调推送前端
pub type StreamEmitter = Arc<dyn Fn(&str) + Send + Sync>;

/// 共享流式回调容器（BrainChatChain 与 AIResponseGenerationRunnable 共享同一实例）
///
/// 使用 `Arc<RwLock<Option<...>>>` 让外部（chat 命令层）能随时注入/清理回调，
/// 而 Runnable 内部在流式分支中读取。
pub type SharedStreamEmitter = Arc<RwLock<Option<StreamEmitter>>>;

/// 创建一个空的共享流式回调容器
pub fn new_shared_stream_emitter() -> SharedStreamEmitter {
    Arc::new(RwLock::new(None))
}

/// 推送一段文本到流式 emitter。
/// emitter 未设置时静默跳过；回调内部 panic 被 catch_unwind 吞掉，不中断生成主流程。
pub(crate) fn push_stream_chunk(emitter: &SharedStreamEmitter, text: &str) {
    let emitter_guard = emitter.read();
    if let Some(emitter_fn) = emitter_guard.as_ref() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emitter_fn(text);
        }));
    }
}

/// 长输入阈值（字符数），超过此值触发特殊处理路径
const LONG_INPUT_THRESHOLD: usize = 100;

// ============================================================================
// AIResponseGenerationRunnable：智能路由 + 故障降级 + graceful_exit
// ============================================================================

/// AI 响应生成 Runnable。
///
/// 流程：
/// 1. 命令或不应答时直接返回（不调用 LLM）
/// 2. `graceful_exit=true` 时生成轻量告别消息（调用 LLM，失败返回 None）
/// 3. 主路径：调用 `router.generate` / `generate_stream`，
///    解析 JSON 提取 `tool_calls`，设置 `response_text` / `response_json` /
///    `tool_calls` / `immediate_response_text`
/// 4. 主路径失败 → 故障降级：直接推理（不带工具调用）
/// 5. 降级也失败 → 返回 `Err`，由上层 chat 命令 emit `chat:error` 事件，
///    前端通过 toast 显示具体错误信息（不写入对话历史与记忆，避免污染）
///
/// 设置字段：`response_text` / `response_json` / `tool_calls` /
/// `immediate_response_text` / `generation_status` / `tool_call_executed`。
pub struct AIResponseGenerationRunnable {
    pub router: Option<Arc<ModelRouter>>,
    pub json_processor: Option<Arc<JsonProcessor>>,
    pub tool_call_manager: Option<Arc<ToolCallManager>>,
    /// 流式 chunk 推送回调（与 BrainChatChain 共享同一 Arc<RwLock<...>>）
    pub stream_emitter: SharedStreamEmitter,
    /// 是否启用原生 function calling 路径（来自 `config.tools.enable_native_function_calling`）
    ///
    /// true：当 provider 支持且 state.tool_definitions 非空时走 `generate_with_tools`
    /// false：始终走文本路径（system prompt 注入工具列表 + JSON 解析）
    pub enable_native_fc: bool,
    /// 原生 function calling 路径单次对话最大 LLM↔工具 往返轮次（来自 `config.tools.max_rounds`）
    pub max_rounds: u32,
    /// 窗口压缩阈值（来自 `config.tools.compress_threshold_tokens`）
    pub compress_threshold_tokens: usize,
    /// 窗口压缩保留的最近消息轮数（来自 `config.tools.compress_keep_recent`）
    pub compress_keep_recent: usize,
}

impl AIResponseGenerationRunnable {
    pub fn new(router: Arc<ModelRouter>) -> Self {
        Self {
            router: Some(router),
            json_processor: Some(Arc::new(JsonProcessor::new())),
            tool_call_manager: None,
            stream_emitter: new_shared_stream_emitter(),
            enable_native_fc: true,
            max_rounds: crate::config::manager::default_tool_max_rounds(), // 与 config.tools.max_rounds 默认值同源
            compress_threshold_tokens: 20000,
            compress_keep_recent: 6,
        }
    }

    pub fn with_tool_call_manager(
        router: Arc<ModelRouter>,
        tool_call_manager: Arc<ToolCallManager>,
        stream_emitter: SharedStreamEmitter,
        enable_native_fc: bool,
        max_rounds: u32,
        compress_threshold_tokens: usize,
        compress_keep_recent: usize,
    ) -> Self {
        Self {
            router: Some(router),
            json_processor: Some(Arc::new(JsonProcessor::new())),
            tool_call_manager: Some(tool_call_manager),
            stream_emitter,
            enable_native_fc,
            max_rounds,
            compress_threshold_tokens,
            compress_keep_recent,
        }
    }

    pub fn empty() -> Self {
        Self {
            router: None,
            json_processor: Some(Arc::new(JsonProcessor::new())),
            tool_call_manager: None,
            stream_emitter: new_shared_stream_emitter(),
            enable_native_fc: false,
            max_rounds: crate::config::manager::default_tool_max_rounds(), // 与 config.tools.max_rounds 默认值同源
            compress_threshold_tokens: 20000,
            compress_keep_recent: 6,
        }
    }

    /// 判断是否走流式分支（tags 含 "stream"）
    fn is_streaming(config: &Option<RunnableConfig>) -> bool {
        config
            .as_ref()
            .map(|c| c.tags.iter().any(|t| t == "stream"))
            .unwrap_or(false)
    }

    /// 从 config.metadata 中读取 task_type，默认 "chat"
    fn task_type(config: &Option<RunnableConfig>) -> String {
        config
            .as_ref()
            .and_then(|c| c.metadata.get("task_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("chat")
            .to_string()
    }

    /// 生成轻量告别消息（graceful_exit 路径）
    ///
    /// 调用 LLM 生成 1-2 句告别；失败时返回兜底文案。
    async fn generate_farewell(router: &ModelRouter, state: &PipelineState) -> String {
        let lang = crate::i18n::get_language();
        let context = if state.system_prompt.is_empty() {
            String::new()
        } else {
            state.system_prompt.chars().take(200).collect::<String>()
        };
        let exit_reason = if state.exit_reason.is_empty() {
            if lang.starts_with("en") {
                "user ended conversation".to_string()
            } else if lang.starts_with("ja") {
                "ユーザーが会話を終了した".to_string()
            } else {
                "用户结束了对话".to_string()
            }
        } else {
            state.exit_reason.clone()
        };

        let prompt = if lang.starts_with("en") {
            format!(
                "You are a warm AI companion finishing a conversation naturally.\n\n\
                 Context: {context}\n\
                 Exit reason: {exit_reason}\n\n\
                 Generate a brief, natural farewell (1-2 sentences). \
                 Warm but not overly emotional. Keep under 50 chars. \
                 No markdown. Just the farewell text."
            )
        } else if lang.starts_with("ja") {
            format!(
                "あなたは温かいAIコンパニオン。会話を自然に締めくくって。\n\n\
                 コンテキスト: {context}\n\
                 終了理由: {exit_reason}\n\n\
                 短く自然な別れの挨拶を生成して（1-2文）。\
                 温かすぎず、感情を抑えめに。50字以内。\
                 markdownなし。別れの挨拶のみ。"
            )
        } else {
            format!(
                "你是一个温暖的 AI 伙伴，正在自然地结束一段对话。\n\n\
                 上下文: {context}\n\
                 退出原因: {exit_reason}\n\n\
                 生成一句简短自然的告别（1-2 句）。\
                 温暖但不要过于感性。50 字以内。\
                 不要 markdown。直接输出告别文本。"
            )
        };
        let messages = vec![ChatMessage::user(&prompt)];

        let fallback = if lang.starts_with("en") {
            "Okay, I won't bother you for now~"
        } else if lang.starts_with("ja") {
            "じゃあ、今は邪魔しないね〜"
        } else {
            "好的，那我先不打扰你啦~"
        };

        match router
            .generate(LLMRequest::new("chat", messages))
            .await
        {
            Ok(text) => {
                let trimmed = text.trim().trim_matches('"').trim_matches('\'').to_string();
                let cleaned: String = trimmed.chars().take(200).collect();
                if cleaned.len() >= 5 {
                    cleaned
                } else {
                    fallback.to_string()
                }
            }
            Err(e) => {
                tracing::warn!("[AIResponse] 生成告别失败: {}", e);
                fallback.to_string()
            }
        }
    }

    /// 从响应文本提取 JSON（优先数组首元素，其次对象）
    fn extract_json(text: &str) -> Option<Value> {
        match JsonParser::parse(text) {
            Ok(values) if !values.is_empty() => {
                Some(values.into_iter().next().unwrap())
            }
            _ => None,
        }
    }

    /// 检测 JSON 解析是否失败（返回 None 或不包含必需的文本字段）
    fn is_json_parse_failed(text: &str) -> bool {
        let parsed = Self::extract_json(text);
        if parsed.is_none() {
            return true;
        }
        if let Some(map) = parsed.unwrap().as_object() {
            for key in ["text", "content", "output", "reply"] {
                if let Some(Value::String(s)) = map.get(key) {
                    if !s.trim().is_empty() {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// 从 parsed JSON 中提取 tool_calls 列表
    ///
    /// 兼容两种格式：
    /// - 顶层 `tool_calls` 数组
    /// - 顶层对象本身就是 `{"tool": ..., "arguments": ...}`
    fn extract_tool_calls(parsed: &Value) -> Vec<Value> {
        let mut calls: Vec<Value> = Vec::new();
        if let Some(arr) = parsed.as_array() {
            for item in arr {
                if let Some(map) = item.as_object() {
                    if map.contains_key("tool") && map.contains_key("arguments") {
                        calls.push(item.clone());
                    }
                }
            }
        } else if let Some(map) = parsed.as_object() {
            if let Some(tc_arr) = map.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in tc_arr {
                    if let Some(tc_map) = tc.as_object() {
                        if tc_map.contains_key("tool") || tc_map.contains_key("name") {
                            calls.push(tc.clone());
                        }
                    }
                }
            }
            // 单条工具调用直接挂在顶层
            if map.contains_key("tool") && map.contains_key("arguments") {
                calls.push(parsed.clone());
            }
        }
        calls
    }

    /// Add speaker prefix for user role messages so the LLM can clearly distinguish message sources.
    ///
    /// - Already has any speaker prefix (cross-character / bystander / first-person): keep as-is
    /// - No prefix (normal user message): prepend `[User says to me]`
    ///
    /// Only called when building the LLM messages array; does not modify conversation history/memory storage.
    fn ensure_speaker_prefix(content: &str) -> String {
        let (_, existing_speaker, _) = parse_any_speaker_prefix(content);
        if existing_speaker.is_some() {
            content.to_string()
        } else {
            format!("[User says to me] {}", content)
        }
    }

    /// 构造 LLMRequest，对主对话路径（chat/reasoning/vision_describe）注入 Vivian 通用响应 Schema
    ///
    /// 通过 Structured Outputs / JSON Mode 通道下发 schema 约束，让 LLM 按结构化 JSON 返回。
    /// 非主对话任务（reflection/consolidation/farewell 等）不注入 schema，保持纯文本。
    pub(crate) fn build_chat_request(task_type: &str, messages: Vec<ChatMessage>) -> LLMRequest {
        let mut req = LLMRequest::new(task_type, messages);
        if matches!(task_type, "chat" | "reasoning" | "vision_describe") {
            req = req.with_json_schema(vivian_response_schema());
        }
        req
    }

    /// 主路径：调用 LLM 生成响应（流式 / 非流式）
    ///
    /// 流式分支：通过 `StreamingJsonParser` 增量解析 LLM 输出，
    /// 每识别出 `text` 字段增量（`StreamEvent::TextChunk`）就调用 `stream_emitter` 推送前端；
    /// 这样前端 `chat:chunk` 收到的是纯文本片段，无需再做 JSON 解析。
    ///
    /// 兜底：若 LLM 输出非 JSON（parser 未提取出任何 text），最后把整个 raw buf 作为单个 chunk 推送。
    /// 三语 JSON 格式约束强调（最后一次重试时添加）
    fn json_format_emphasis() -> String {
        format!(
            "\n\n### 必须返回有效的 JSON 格式 ###\n\
             中文：你的响应必须是一个有效的 JSON 对象，包含 text 字段。\n\
             English: Your response must be a valid JSON object with a text field.\n\
             日本語：あなたの応答は、text フィールドを含む有効な JSON オブジェクトでなければなりません。\n\
             示例格式：{{\"text\": \"你的回答内容\"}}\n\
             ###############################"
        )
    }

    /// 执行单次 LLM 调用（非流式）
    async fn call_llm_once(
        router: &ModelRouter,
        messages: Vec<ChatMessage>,
        task_type: &str,
        add_emphasis: bool,
    ) -> VivianResult<String> {
        let mut req = Self::build_chat_request(task_type, messages);
        if add_emphasis {
            if let Some(last_msg) = req.messages.last_mut() {
                if last_msg.role == "user" {
                    last_msg.content.push_str(&Self::json_format_emphasis());
                }
            }
        }
        router.generate(req).await
    }

    async fn call_llm(
        router: &ModelRouter,
        messages: Vec<ChatMessage>,
        task_type: &str,
        stream: bool,
        emitter: &SharedStreamEmitter,
    ) -> VivianResult<String> {
        if stream {
            let mut rx = router
                .generate_stream(Self::build_chat_request(task_type, messages).with_stream(true))
                .await?;
            let mut buf = String::new();
            let mut parser = StreamingJsonParser::new();
            let mut any_text_emitted = false;
            while let Some(event) = rx.recv().await {
                match event {
                    ProviderStreamEvent::Text { content } => {
                        buf.push_str(&content);
                        let events = parser.feed(&content);
                        for ev in events {
                            if let JsonStreamEvent::TextChunk(text) = ev {
                                any_text_emitted = true;
                                push_stream_chunk(emitter, &text);
                            }
                        }
                    }
                    ProviderStreamEvent::Error { message } => {
                        return Err(VivianError::Provider(format!("流式响应中断: {}", message)));
                    }
                    _ => {}
                }
            }
            if !any_text_emitted && !buf.is_empty() {
                let text_to_push = JsonParser::extract_text(&buf).unwrap_or_else(|| buf.clone());
                push_stream_chunk(emitter, &text_to_push);
            }
            Ok(buf)
        } else {
            let max_retries = 3;
            for attempt in 0..max_retries {
                let result = Self::call_llm_once(
                    router,
                    messages.clone(),
                    task_type,
                    attempt == max_retries - 1,
                ).await;
                match result {
                    Ok(text) => {
                        if !Self::is_json_parse_failed(&text) {
                            return Ok(text);
                        }
                        tracing::warn!(
                            "[AIResponse] JSON 解析失败（第 {} 次尝试），内容前 200 字符: {}",
                            attempt + 1,
                            text.chars().take(200).collect::<String>()
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[AIResponse] LLM 调用失败（第 {} 次尝试）: {}",
                            attempt + 1,
                            e
                        );
                        if attempt == max_retries - 1 {
                            return Err(e);
                        }
                    }
                }
            }
            router.generate(Self::build_chat_request(task_type, messages)).await
        }
    }

    /// 原生 function calling 路径（非流式入口）
    ///
    /// 首轮响应通过 `generate_with_tools` 一次性获取（完整人设轮）并推送文本；
    /// 首轮之后的一切——工具执行回填、执行态切换、窗口压缩、终止判定
    /// （无工具调用 / goal_completed / doom loop / 轮次上限）与表达态收尾——
    /// 由 `crate::pipeline::react` 的共享骨架接管，与流式入口仅差首轮取响应的方式。
    ///
    /// 与文本路径的差异：
    /// - 工具 schema 走 API 专用通道，不占 prompt token
    /// - 模型返回结构化 `tool_calls`，无需解析 JSON 文本
    /// - 工具结果用 `role=tool` + `tool_call_id` 回喂（OpenAI 风格）
    async fn call_llm_native_fc(
        router: &ModelRouter,
        tool_call_manager: &ToolCallManager,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
        task_type: &str,
        emitter: &SharedStreamEmitter,
        max_rounds: u32,
        compress_threshold_tokens: usize,
        compress_keep_recent: usize,
        channel: &str,
        memory_text: &str,
    ) -> VivianResult<(String, Vec<ToolCallResult>, usize, Option<f64>)> {
        // 首轮（完整人设轮）响应
        let first = router
            .generate_with_tools(
                Self::build_chat_request(task_type, messages.clone()).with_tools(tools.clone()),
            )
            .await?;
        // 防御：模型偶尔仍包 JSON，提取 text 字段（纯文本时原样返回）
        let first_content = JsonParser::extract_text(&first.content).unwrap_or(first.content);
        // 首轮文本直接推送（对齐流式入口的首轮流式推送；中间轮文本不推送）
        if !first_content.is_empty() {
            push_stream_chunk(emitter, &first_content);
        }

        crate::pipeline::react::run_react_loop(
            router,
            tool_call_manager,
            emitter,
            crate::pipeline::react::ReactParams {
                first_content,
                first_calls: first.tool_calls,
                messages,
                tools,
                task_type: task_type.to_string(),
                channel: channel.to_string(),
                memory_text: memory_text.to_string(),
                max_rounds,
                compress_threshold_tokens,
                compress_keep_recent,
            },
        )
        .await
    }

    /// 流式 + 原生 function calling 路径
    ///
    /// 与 `call_llm_native_fc` 的区别：使用 `generate_stream_with_tools` 获取流式响应。
    /// 文本增量通过 `stream_emitter` 实时推送前端；工具调用增量按 `index` 累积，
    /// 流结束后一次性执行所有工具调用，再把结果以非流式 `invoke` 回喂给 LLM
    /// 生成最终自然语言总结。
    ///
    /// 设计权衡：
    /// - 文本部分实时推流（前端打字机效果）
    /// - 工具调用部分累积后批量执行（避免流式执行 + 流式生成交织的复杂性）
    /// - 工具结果回喂用非流式 `invoke`（简化实现，工具结果后的总结文本不长）
    ///
    /// 多轮工具调用：首轮流式获取 → 执行工具与后续轮次由 `crate::pipeline::react` 共享骨架接管
    async fn call_llm_native_fc_stream(
        router: &ModelRouter,
        tool_call_manager: &ToolCallManager,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
        task_type: &str,
        emitter: &SharedStreamEmitter,
        max_rounds: u32,
        compress_threshold_tokens: usize,
        compress_keep_recent: usize,
        channel: &str,
        memory_text: &str,
    ) -> VivianResult<(String, Vec<ToolCallResult>, usize, Option<f64>)> {
        // === 第一轮：流式获取 LLM 响应（带重试机制）===
        // DeepSeek V4 Flash 流式 native function calling 偶发失效：
        // finish_reason=tool_calls 但 SSE delta 中无 tool_calls 数据。
        // 识别此错误并自动重试，最多 3 次流式尝试，全部失败后回退非流式调用。
        const MAX_STREAM_ATTEMPTS: u32 = 3;
        let mut first_round_calls: Vec<crate::providers::base::StructuredToolCall> = Vec::new();
        let mut final_first_text = String::new();
        let mut finish_reason: Option<String> = None;

        'stream_attempt: for attempt in 1..=MAX_STREAM_ATTEMPTS {
            // 重试时（attempt >= 2）追加引导消息，提醒模型使用 function calling 接口
            let mut attempt_msgs = messages.clone();
            if attempt >= 2 {
                attempt_msgs.push(ChatMessage::system(
                    "【系统指令】你拥有一组可用工具（function calling）。当用户的请求需要你执行操作时（如换壁纸、搜索、打开应用等），你必须调用对应的工具函数，而不是在回复文本中描述你会去做。请先调用 tool，再根据结果回复用户。"
                ));
            }

            let mut rx = router
                .generate_stream_with_tools(
                    Self::build_chat_request(task_type, attempt_msgs).with_tools(tools.clone()),
                )
                .await?;

            // 累积工具调用：按 index 分组，拼接 arguments 字符串
            let mut tool_call_ids: Vec<Option<String>> = Vec::new();
            let mut tool_call_names: Vec<Option<String>> = Vec::new();
            let mut tool_call_args: Vec<String> = Vec::new();
            let mut text_content = String::new();
            let mut attempt_finish_reason: Option<String> = None;
            let mut stream_error: Option<String> = None;

            // 流式 JSON 解析器：原生 FC 路径下模型可能仍返回 JSON 格式
            let mut fc_parser = StreamingJsonParser::new();
            let mut fc_any_text_emitted = false;

            // 首次尝试实时推送文本到前端；重试时仅缓冲（避免重复推送）
            let emit_text = attempt == 1;

            while let Some(ev) = rx.recv().await {
                match ev {
                    ProviderStreamEvent::Text { content } => {
                        text_content.push_str(&content);
                        if emit_text {
                            let events = fc_parser.feed(&content);
                            for event in events {
                                if let JsonStreamEvent::TextChunk(text) = event {
                                    fc_any_text_emitted = true;
                                    push_stream_chunk(emitter, &text);
                                }
                            }
                        }
                    }
                    ProviderStreamEvent::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments_delta,
                    } => {
                        while tool_call_ids.len() <= index {
                            tool_call_ids.push(None);
                            tool_call_names.push(None);
                            tool_call_args.push(String::new());
                        }
                        if let Some(i) = id {
                            tool_call_ids[index] = Some(i);
                        }
                        if let Some(n) = name {
                            tool_call_names[index] = Some(n);
                        }
                        if let Some(a) = arguments_delta {
                            tool_call_args[index].push_str(&a);
                        }
                    }
                    ProviderStreamEvent::Thinking { .. } => {}
                    ProviderStreamEvent::Usage { .. } => {}
                    ProviderStreamEvent::Done { finish_reason: fr } => {
                        attempt_finish_reason = fr;
                        break;
                    }
                    ProviderStreamEvent::Error { message } => {
                        stream_error = Some(message);
                        break;
                    }
                }
            }

            if let Some(err) = stream_error {
                return Err(VivianError::Provider(format!(
                    "流式原生 function calling 失败: {}",
                    err
                )));
            }

            // 兜底：首次尝试时若流式解析未提取到 text，补发一次
            if emit_text && !fc_any_text_emitted && !text_content.is_empty() {
                let text_to_push = JsonParser::extract_text(&text_content).unwrap_or_else(|| text_content.clone());
                push_stream_chunk(emitter, &text_to_push);
            }

            // 收集工具调用
            let mut calls: Vec<crate::providers::base::StructuredToolCall> = Vec::new();
            for i in 0..tool_call_ids.len() {
                let id = tool_call_ids[i].clone().unwrap_or_else(|| format!("call_{}", i));
                let name = tool_call_names[i].clone().unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let args_str = if tool_call_args[i].is_empty() {
                    "{}".to_string()
                } else {
                    tool_call_args[i].clone()
                };
                let arguments: Value = serde_json::from_str(&args_str).unwrap_or(Value::Object(Default::default()));
                calls.push(crate::providers::base::StructuredToolCall {
                    id,
                    name,
                    arguments,
                });
            }

            // 错误识别：finish_reason=tool_calls 但 0 个有效工具调用 → 流式解析缺陷
            let is_parse_failure = calls.is_empty()
                && attempt_finish_reason.as_deref() == Some("tool_calls");

            if !is_parse_failure {
                // 成功：有工具调用，或 finish_reason 非 tool_calls（纯文本回复）
                finish_reason = attempt_finish_reason;
                final_first_text = JsonParser::extract_text(&text_content).unwrap_or(text_content);
                first_round_calls = calls;

                // 重试成功时补发缓冲文本到前端（首次尝试已实时推送，无需补发）
                if attempt > 1 && !final_first_text.is_empty() {
                    push_stream_chunk(emitter, &final_first_text);
                }

                if attempt > 1 {
                    tracing::info!(
                        "[AIResponse][native_fc_stream] 第 {} 次流式尝试成功：{} 个工具调用",
                        attempt, first_round_calls.len()
                    );
                }
                break 'stream_attempt;
            }

            // 解析失败：继续重试或回退
            if attempt < MAX_STREAM_ATTEMPTS {
                tracing::warn!(
                    "[AIResponse][native_fc_stream] 第 {} 次流式尝试失败 (finish_reason=tool_calls 但无有效工具调用)，重试中",
                    attempt
                );
            } else {
                tracing::warn!(
                    "[AIResponse][native_fc_stream] {} 次流式尝试均失败，回退到非流式调用",
                    MAX_STREAM_ATTEMPTS
                );
            }
        }

        // 流式重试全部失败 → 非流式回退
        // 非流式模式下 tool_calls 作为完整 JSON 返回，解析更可靠
        if first_round_calls.is_empty() && finish_reason.is_none() {
            let fallback_req =
                Self::build_chat_request(task_type, messages.clone()).with_tools(tools.clone());
            match router.generate_with_tools(fallback_req).await {
                Ok(resp) if !resp.tool_calls.is_empty() => {
                    tracing::info!(
                        "[AIResponse][native_fc_stream] 非流式回退成功：{} 个工具调用",
                        resp.tool_calls.len()
                    );
                    if !resp.content.is_empty() {
                        final_first_text =
                            JsonParser::extract_text(&resp.content).unwrap_or(resp.content);
                        push_stream_chunk(emitter, &final_first_text);
                    }
                    first_round_calls = resp
                        .tool_calls
                        .into_iter()
                        .map(|tc| crate::providers::base::StructuredToolCall {
                            id: tc.id,
                            name: tc.name,
                            arguments: tc.arguments,
                        })
                        .collect();
                }
                Ok(resp) => {
                    tracing::warn!(
                        "[AIResponse][native_fc_stream] 非流式回退也无工具调用，返回文本回复 (finish_reason={:?})",
                        resp.finish_reason
                    );
                    if !resp.content.is_empty() {
                        final_first_text =
                            JsonParser::extract_text(&resp.content).unwrap_or(resp.content);
                        push_stream_chunk(emitter, &final_first_text);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[AIResponse][native_fc_stream] 非流式回退失败: {}，返回文本回复",
                        e
                    );
                }
            }
        }

        // 首轮到此为止：无工具调用则由共享骨架直接以首轮文本收场；
        // 有工具调用则执行、切换执行态、继续后续轮次（与非流式入口共享骨架）
        crate::pipeline::react::run_react_loop(
            router,
            tool_call_manager,
            emitter,
            crate::pipeline::react::ReactParams {
                first_content: final_first_text,
                first_calls: first_round_calls,
                messages,
                tools,
                task_type: task_type.to_string(),
                channel: channel.to_string(),
                memory_text: memory_text.to_string(),
                max_rounds,
                compress_threshold_tokens,
                compress_keep_recent,
            },
        )
        .await
    }
}

#[async_trait]
impl Runnable for AIResponseGenerationRunnable {
    async fn ainvoke(&self, input: Value, config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 命令或不应答：跳过 LLM 调用
        if !state.should_respond || state.is_command {
            return Ok(state.to_json());
        }

        let router = match &self.router {
            Some(r) => r.clone(),
            None => {
                tracing::warn!("[AIResponse] router 未注入，返回 Err 由前端 toast 提示");
                return Err(VivianError::Engine("router 未注入".to_string()));
            }
        };

        // 用 task-local 递归进入一次有作用域的调用，避免修改共享 provider 状态。
        // 第二次进入时可见当前 extra，因而不会再次递归。
        if state.focus_active
            && state.focus_extra_tokens > 0
            && ProviderCallOptions::current().max_tokens_extra == 0
        {
            return router
                .scope_call_options(
                    ProviderCallOptions {
                        max_tokens_extra: state.focus_extra_tokens,
                        ..ProviderCallOptions::default()
                    },
                    self.ainvoke(state.to_json(), config),
                )
                .await;
        }

        let stream = Self::is_streaming(&config);
        let mut task_type = Self::task_type(&config);

        // 自动升级到 reasoning 模型的条件：
        // 1. 用户输入较长（>100字），需要更强的理解与组织能力
        // 2. 本次请求携带工具定义（需要 function calling / JSON 结构化输出 / 多轮推理）
        if task_type == "chat" {
            let long_input = state.user_input.chars().count() > LONG_INPUT_THRESHOLD;
            let has_tools = !state.tool_definitions.is_empty();
            if long_input || has_tools {
                task_type = "reasoning".to_string();
            }
        }

        // ── graceful_exit：生成告别 ──
        if state.graceful_exit {
            let farewell = Self::generate_farewell(&router, &state).await;
            state.response_text = farewell.clone();
            state.generation_status = "graceful_exit_farewell".to_string();
            // 同步 ai_response 字段以兼容下游
            state.ai_response = Some(AiResponse::new(farewell));
            state.metadata["graceful_exit"] = json!(true);
            // TODO: 流式模式下推送 stream_callback（当前 stream_callback 机制尚未实现，graceful_exit 走非流式分支）
            return Ok(state.to_json());
        }

        // 构建 messages 列表（KV-cache 友好顺序）：
        // [static system] → [历史消息] → [动态感知便签(user)] → [用户输入] → [status_bar]
        //
        // 静态段（人设/规则/格式）作为 system message 冻结在前缀；
        // 动态段（时间/环境/情绪/记忆）作为 user 便签插在历史**之后**、用户输入之前——
        // 每轮变化的只有尾部便签，前缀（静态 system + 全部历史）逐字节复用，
        // 实现缓存友好的"重放前缀 + 追加尾部"策略。
        let mut messages_vec: Vec<ChatMessage> = Vec::new();
        let mut dynamic_note: Option<String> = None;
        let sys = state.system_prompt.clone();
        // 尝试按 STATIC_OPEN/STATIC_CLOSE 与边界分割
        if !sys.is_empty() {
            let static_open = crate::pipeline::prompt_modules::STATIC_OPEN;
            let static_close = crate::pipeline::prompt_modules::STATIC_CLOSE;
            let boundary = crate::pipeline::prompt_modules::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;

            if let (Some(start), Some(end)) = (sys.find(static_open), sys.find(static_close)) {
                // 提取静态段内容
                let s = &sys[start + static_open.len()..end];
                messages_vec.push(ChatMessage::system(s.trim().to_string()));
                // 若存在 boundary，提取 boundary 之后到用户输入之前的动态段（暂存，历史后插入）
                if let Some(bpos) = sys.find(boundary) {
                    let after_b = &sys[bpos + boundary.len()..];
                    // 找到用户输入标题（语言相关的本地化标题，如 "# User Input" / "# 用户输入"），
                    // 动态便签截到用户输入之前——用户输入由下方单独的 user message 注入，避免重复
                    let ui_lang = crate::i18n::get_language();
                    let ui_marker = format!(
                        "{}\n",
                        crate::pipeline::prompt_modules::section_heading("user_input", &ui_lang)
                    );
                    let dynamic_section = if let Some(upos) = after_b.find(&ui_marker) {
                        after_b[..upos].to_string()
                    } else {
                        after_b.to_string()
                    };

                    if !dynamic_section.trim().is_empty() {
                        dynamic_note = Some(dynamic_section.trim().to_string());
                    }
                }
            } else {
                // 无法找到静态标签时，退回到把整个 system_prompt 当作 system message
                messages_vec.push(ChatMessage::system(sys.trim().to_string()));
            }
        }

        // 插入历史消息（如果有）
        // 不再添加 [直接对话]/[微信聊天] 程序化前缀——这类标签会让 LLM 进入"处理结构化数据"模式。
        // 当前消息的渠道风格由 build_channel_style_guide 在 system prompt 末尾统一告知。
        // user role 消息统一加发言者前缀（[用户 对你说] / [X 对你说]），让 LLM 区分消息来源。
        if !state.messages.is_empty() {
            for msg in &state.messages {
                if msg.role == "user" {
                    let prefixed = Self::ensure_speaker_prefix(&msg.content);
                    messages_vec.push(ChatMessage {
                        content: prefixed,
                        ..msg.clone()
                    });
                } else {
                    messages_vec.push(msg.clone());
                }
            }
        }

        // 动态感知便签：历史之后、用户输入之前（每轮重新生成，只影响尾部，前缀不受损）。
        // 内容 = 时间/环境/情绪/记忆/工具提示等"此刻状态"，让 Agent 在不动历史的前提下保持知情。
        if let Some(note) = dynamic_note {
            messages_vec.push(ChatMessage::user(note));
        }

        // 后台任务报告已随本次请求注入（便签或整体 system 两条路径均覆盖）
        // → 标记消费，后续轮次不再重复注入
        if let Some(ids) = state
            .metadata
            .get("bg_report_task_ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect::<Vec<String>>()
            })
        {
            if let Some(ts) = crate::brain::task_service::global() {
                ts.mark_reports_consumed(&ids);
            }
        }

        // 凝神模式：激活时追加认知模式指令，让 LLM 进入更深度的思考与陪伴状态
        if state.focus_active {
            messages_vec.push(ChatMessage::system(
                "【凝神模式】用户正处于需要专注或深度陪伴的状态。放慢节奏，回答更周全、更安静，\
                          避免轻率或跳跃式回应；优先给出有深度的内容而非寒暄。",
            ));
        }

        // 最终的用户输入总是作为最后一条 user message（统一加发言者前缀）
        let prefixed_input = Self::ensure_speaker_prefix(&state.user_input);
        messages_vec.push(ChatMessage::user(&prefixed_input));

        // Agent 状态栏：以 user-role 元消息追加在用户输入之后、紧邻生成位置。
        // 键值对 + 时间感操作策略，KV-cache 友好（只追加不修改前缀）。
        if let Some(status_bar) = crate::pipeline::prompt_modules::build_agent_status_bar(
            &state.messages,
            &state.user_input,
            state.focus_active,
        ) {
            messages_vec.push(ChatMessage::user(status_bar));
        }

        // ── 双路径切换：原生 function calling vs 文本路径 ──
        //
        // 满足以下全部条件时走原生路径：
        // 1. config 开关 `enable_native_function_calling=true`
        // 2. 当前路由目标 provider 支持原生 fc（`supports_native_function_calling`）
        // 3. 有可用的结构化工具定义（`state.tool_definitions` 非空）
        // 4. ToolCallManager 已注入（执行工具调用所需）
        //
        // 流式模式与非流式模式都走原生路径：
        // - 非流式：`call_llm_native_fc` → `generate_with_tools`
        // - 流式：`call_llm_native_fc_stream` → `generate_stream_with_tools`（文本实时推流，
        //   工具调用增量累积后批量执行，后续轮次非流式 invoke）
        //
        // 任一条件不满足则走文本路径（system prompt 注入工具列表 + JSON 解析）。
        //
        // 注：OUTPUT_FORMAT 段（"Entire response = one JSON object"）已在 PromptBuilder
        // 阶段根据 `enable_native_fc` 标志跳过注入——native FC 路径不需要 JSON 包装。
        let use_native_fc = self.enable_native_fc
            && router.supports_native_function_calling(&task_type)
            && !state.tool_definitions.is_empty()
            && self.tool_call_manager.is_some();

        if use_native_fc {
            let tcm = self.tool_call_manager.as_ref().unwrap();
            let native_result = if stream {
                Self::call_llm_native_fc_stream(
                    &router,
                    tcm,
                    messages_vec.clone(),
                    state.tool_definitions.clone(),
                    &task_type,
                    &self.stream_emitter,
                    self.max_rounds,
                    self.compress_threshold_tokens,
                    self.compress_keep_recent,
                    &state.current_channel,
                    &state.memory_text,
                )
                .await
            } else {
                Self::call_llm_native_fc(
                    &router,
                    tcm,
                    messages_vec.clone(),
                    state.tool_definitions.clone(),
                    &task_type,
                    &self.stream_emitter,
                    self.max_rounds,
                    self.compress_threshold_tokens,
                    self.compress_keep_recent,
                    &state.current_channel,
                    &state.memory_text,
                )
                .await
            };

            match native_result {
                Ok((final_text, all_results, iterations, first_tool_ts)) => {
                    state.response_text = final_text.clone();
                    // 原生 FC 路径下 LLM 直接返回自然语言文本，不再走 JSON 解析路径
                    // （避免对自然语言触发 RobustJSON 全部阶段失败的 warn 日志）
                    state.response_json = None;

                    if !all_results.is_empty() {
                        state.tool_call_executed = true;
                        state.metadata["tool_call_count"] = json!(all_results.len());
                        state.metadata["tool_call_iterations"] = json!(iterations);
                        state.metadata["native_function_calling"] = json!(true);
                        state.metadata["tool_executed_at"] = json!(first_tool_ts.unwrap_or_else(crate::memory::types::current_timestamp));

                        // 工具失败容错：收集失败的工具记录到 metadata
                        // 同名工具去重：若该工具后续调用成功，则不计入 failures，
                        // 避免"重试成功后仍报失败"导致 LLM 产生自相矛盾的回复
                        let mut failed_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
                        for r in &all_results {
                            if r.success {
                                failed_names.remove(r.tool_name.as_str());
                            } else {
                                failed_names.insert(r.tool_name.as_str());
                            }
                        }
                        let failures: Vec<Value> = all_results
                            .iter()
                            .filter(|r| !r.success && failed_names.contains(r.tool_name.as_str()))
                            .map(|f| {
                                json!({
                                    "tool": f.tool_name,
                                    "error": f.error,
                                })
                            })
                            .collect();
                        if !failures.is_empty() {
                            state.metadata["tool_failures"] = json!(failures);
                        }

                        // 把工具调用记录到 state.tool_calls（供下游 ToolCallExecutor 等使用）
                        let calls: Vec<Value> = all_results
                            .iter()
                            .map(|r| {
                                json!({
                                    "tool": r.tool_name,
                                    "arguments": r.arguments,
                                    "success": r.success,
                                    "result": r.result,
                                    "error": r.error,
                                })
                            })
                            .collect();
                        state.tool_calls = calls;
                    }

                    state.generation_status = "ai_generation_complete".to_string();
                    state.ai_response = Some(AiResponse::new(final_text));
                    state.metadata["streamed"] = json!(stream);
                    state.metadata["native_fc_stream"] = json!(stream);

                    tracing::info!(
                        "[AIResponse] 走原生 function calling 路径（{}）：{} 轮，{} 个工具调用",
                        if stream { "流式" } else { "非流式" },
                        iterations,
                        all_results.len()
                    );

                    return Ok(state.to_json());
                }
                Err(e) => {
                    tracing::warn!(
                        "[AIResponse] 原生 function calling 路径失败，回退到文本路径: {}",
                        e
                    );
                    state.metadata["native_fc_fallback"] = json!(true);

                    // 注入文本版工具块（native FC 启用时 prompt 中工具区段为空，回退须补回）
                    if let Some(ref tools_text) = state.tools_text_fallback {
                        messages_vec.push(ChatMessage::system(tools_text.clone()));
                        tracing::info!(
                            "[AIResponse] 回退路径注入工具文本 ({}chars)",
                            tools_text.chars().count()
                        );
                    }
                    // 注入输出格式指令（native FC / JSON Schema 启用时 prompt 中跳过了 output_format）
                    if let Some(ref fmt) = state.output_format_fallback {
                        messages_vec.push(ChatMessage::system(format!(
                            "[FORMAT SPEC - DO NOT EMBODY]\n{}\n[END FORMAT]",
                            fmt
                        )));
                        tracing::info!("[AIResponse] 回退路径注入输出格式指令");
                    }
                }
            }
        }

        // 主路径：LLM 生成 → ToolCallManager 执行工具调用（如有）
        match Self::call_llm(&router, messages_vec.clone(), &task_type, stream, &self.stream_emitter).await {
            Ok(text) => {
                state.response_text = text.clone();
                // 提取 JSON
                let parsed = Self::extract_json(&text);
                state.response_json = parsed.clone();

                // 从 parsed 提取 tool_calls 列表
                if let Some(ref p) = parsed {
                    let calls = Self::extract_tool_calls(p);
                    if !calls.is_empty() {
                        state.tool_calls = calls;
                    }
                }

                // 执行工具调用（ToolCallManager 注入时启用）
                if let Some(tcm) = &self.tool_call_manager {
                    // 反馈循环：执行首轮工具 → 把结果反馈给 LLM → LLM 再决策
                    // 这样 LLM 能基于工具结果生成自然语言总结，而非仅输出工具调用前的 immediate_response
                    let router_clone = Arc::clone(&router);
                    let task_type_clone = task_type.clone();
                    let emitter_clone = Arc::clone(&self.stream_emitter);
                    let messages_clone = messages_vec.clone();

                    let (final_response, iterations, all_results, first_tool_ts) = tcm
                        .run_feedback_loop(&text, |continue_prompt| {
                            let router = Arc::clone(&router_clone);
                            let emitter = Arc::clone(&emitter_clone);
                            let messages = messages_clone.clone();
                            let task_type = task_type_clone.clone();
                            async move {
                                // 把 continue_prompt 作为 user 消息追加到对话历史末尾
                                let mut msgs = messages;
                                msgs.push(ChatMessage::user(&continue_prompt));
                                // 反馈轮次不使用流式（避免重复推流）
                                Self::call_llm(&router, msgs, &task_type, false, &emitter)
                                    .await
                                    .ok()
                            }
                        })
                        .await;

                    if !all_results.is_empty() {
                        state.tool_call_executed = true;
                        state.metadata["tool_call_count"] = json!(all_results.len());
                        state.metadata["tool_call_iterations"] = json!(iterations);
                        state.metadata["tool_executed_at"] = json!(first_tool_ts.unwrap_or_else(crate::memory::types::current_timestamp));

                        // 工具失败容错：收集失败的工具记录到 metadata，供下游追加提示
                        // 同名工具去重：若该工具后续调用成功，则不计入 failures，
                        // 避免"重试成功后仍报失败"导致 LLM 产生自相矛盾的回复
                        let mut failed_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
                        for r in &all_results {
                            if r.success {
                                failed_names.remove(r.tool_name.as_str());
                            } else {
                                failed_names.insert(r.tool_name.as_str());
                            }
                        }
                        let failures: Vec<Value> = all_results
                            .iter()
                            .filter(|r| !r.success && failed_names.contains(r.tool_name.as_str()))
                            .map(|f| {
                                json!({
                                    "tool": f.tool_name,
                                    "error": f.error,
                                })
                            })
                            .collect();
                        if !failures.is_empty() {
                            state.metadata["tool_failures"] = json!(failures);
                        }
                    }

                    // 用反馈循环的最终响应覆盖 state（LLM 基于工具结果生成的自然语言回复）
                    if let Some(final_resp) = final_response {
                        if !final_resp.trim().is_empty() {
                            state.response_text = final_resp.clone();
                            // 重新提取 JSON（LLM 反馈轮次可能输出了新的 JSON）
                            if let Some(parsed) = Self::extract_json(&final_resp) {
                                state.response_json = Some(parsed.clone());
                                let calls = Self::extract_tool_calls(&parsed);
                                if !calls.is_empty() {
                                    state.tool_calls = calls;
                                }
                            }
                        }
                    }
                } else if !state.tool_calls.is_empty() {
                    // 无 ToolCallManager 时仅标记，不执行
                    state.tool_call_executed = true;
                }

                state.generation_status = "ai_generation_complete".to_string();

                // 同步 ai_response 字段以兼容下游（如 MoodStep）
                state.ai_response = Some(AiResponse::new(text));
                state.metadata["streamed"] = json!(stream);
            }
            Err(e) => {
                tracing::warn!("[AIResponse] 主路径失败，降级到直接推理: {}", e);

                // ── 故障降级：直接调用 chat 任务（不带工具/不带 stream）──
                // 构建降级调用的 messages（保留 system + history，如上）
                let mut fallback_messages: Vec<ChatMessage> = Vec::new();
                if !state.system_prompt.is_empty() {
                    fallback_messages.push(ChatMessage::system(state.system_prompt.clone()));
                }
                if !state.messages.is_empty() {
                    for msg in &state.messages {
                        if msg.role == "user" {
                            let prefixed = Self::ensure_speaker_prefix(&msg.content);
                            fallback_messages.push(ChatMessage {
                                content: prefixed,
                                ..msg.clone()
                            });
                        } else {
                            fallback_messages.push(msg.clone());
                        }
                    }
                } else if !state.prompt.is_empty() {
                    let prefixed = Self::ensure_speaker_prefix(&state.prompt);
                    fallback_messages.push(ChatMessage::user(&prefixed));
                } else {
                    let prefixed = Self::ensure_speaker_prefix(&state.user_input);
                    fallback_messages.push(ChatMessage::user(&prefixed));
                }

                match Self::call_llm(&router, fallback_messages, "chat", false, &self.stream_emitter).await {
                    Ok(text) => {
                        state.response_text = text.clone();
                        state.response_json = Self::extract_json(&text);
                        state.generation_status = "ai_generation_fallback".to_string();
                        state.ai_response = Some(AiResponse::new(text));
                        state.metadata["streamed"] = json!(false);
                        state.metadata["fallback_used"] = json!(true);
                    }
                    Err(fallback_err) => {
                        // 主路径与降级均失败：返回 Err，由 chat 命令 emit `chat:error`，
                        // 前端通过 toast 显示具体错误（不写入对话历史与记忆，避免兜底文案污染）
                        tracing::warn!("[AIResponse] 主路径与降级均失败: {}", fallback_err);
                        return Err(fallback_err);
                    }
                }
            }
        }

        Ok(state.to_json())
    }
}

// ============================================================================
// ResponseParsingRunnable：使用 JsonProcessor 解析响应
// ============================================================================

/// 响应解析 Runnable。
///
/// 使用 `JsonProcessor::process_response` 解析 `response_text`，
/// 提取标准字段：
/// - `text` / `motion` / `expression`
/// - `importance_user` / `importance_ai`
/// - `long_term_memory`
/// - `intent`（reply / short_reply / no_reply）
/// - `tool_calls`
///
/// 特殊处理：
/// - `intent=no_reply` 时主动把 `text` 置空（不展示回复）
/// - `text` 为空且 `intent != no_reply` 时尝试从原始 `response_text` 提取文本；
///   提取仍为空则保持为空（不再注入兜底文案，避免污染对话历史与记忆）
/// - 冷却期内移除用户名（避免称呼冷却失效）
/// - 解析失败时直接使用 `response_text.strip()` 兜底
pub struct ResponseParsingRunnable {
    pub json_processor: Option<Arc<JsonProcessor>>,
    /// 对话管理器（用于冷却期称呼移除，注入后启用）
    pub dialogue_manager: Option<Arc<crate::dialogue::DialogueManager>>,
    /// 人格引擎（注入后启用回复后处理：客服话术过滤 + 禁忌关键词检测）
    pub persona: Option<Arc<crate::persona::PersonaEngine>>,
}

impl ResponseParsingRunnable {
    pub fn new() -> Self {
        Self {
            json_processor: Some(Arc::new(JsonProcessor::new())),
            dialogue_manager: None,
            persona: None,
        }
    }

    pub fn with_processor(json_processor: Arc<JsonProcessor>) -> Self {
        Self {
            json_processor: Some(json_processor),
            dialogue_manager: None,
            persona: None,
        }
    }

    /// 注入 PersonaEngine，启用回复后处理（客服话术过滤 + 禁忌关键词检测）
    pub fn with_persona(mut self, persona: Arc<crate::persona::PersonaEngine>) -> Self {
        self.persona = Some(persona);
        self
    }

    /// 从 ProcessedResponse 提取字段到 PipelineState
    fn extract_from_processed(state: &mut PipelineState, processed: &ProcessedResponse) {
        state.text = processed.text.clone();

        // 意图标记：仅接受三种合法值
        let raw_intent = processed.intent.as_str();
        if matches!(raw_intent, "reply" | "short_reply" | "no_reply") {
            state.intent = raw_intent.to_string();
        }

        // 响应模式：speak/non_verbal/internal/ignore
        // 仅跨角色对话场景由 LLM 主动返回非 speak 值，主对话场景下永远为 speak
        let raw_mode = processed.response_mode.trim().to_lowercase();
        if matches!(raw_mode.as_str(), "speak" | "non_verbal" | "internal" | "ignore") {
            state.response_mode = raw_mode;
        } else {
            state.response_mode = "speak".to_string();
        }

        // 微信渠道语音消息标志
        state.voice_message = processed.voice_message;

        // no_reply → 不展示回复（API 调用不浪费，通过 text 置空实现）
        if state.intent == "no_reply" {
            state.text = String::new();
            tracing::debug!("[ResponseParsing] intent=no_reply, 跳过回复展示");
        }

        // 非语言响应模式：清空 text（动作/表情由下游 ExpressionMotionStep 处理）
        if state.response_mode != "speak" {
            state.text = String::new();
            tracing::debug!(
                "[ResponseParsing] response_mode={}, 清空 text（非语言响应）",
                state.response_mode
            );
        }

        // 工具调用列表同步（保留 AIResponseGenerationRunnable 已设置的 tool_calls）
        if !processed.tool_calls.is_empty() && state.tool_calls.is_empty() {
            state.tool_calls = processed.tool_calls.clone();
        }
    }

    /// 从原始 LLM 输出中尝试提取纯文本（兜底）
    ///
    /// 当 JSON 解析主路径未命中时调用。若 raw 形如 `{"text": "...", ...}`，
    /// 提取其中 text 字段；否则返回 `raw.trim()`。
    fn try_extract_text_from_raw(raw: &str) -> String {
        if raw.is_empty() {
            return String::new();
        }
        let s = raw.trim();
        // 只在看起来像 JSON 对象时才尝试解析
        if s.starts_with('{') && s.ends_with('}') {
            if let Ok(obj) = serde_json::from_str::<Value>(s) {
                if let Some(map) = obj.as_object() {
                    for key in ["text", "reply", "content", "output"] {
                        if let Some(Value::String(inner)) = map.get(key) {
                            let trimmed = inner.trim();
                            if !trimmed.is_empty() {
                                return trimmed.to_string();
                            }
                        }
                    }
                    // 没有可用字段时返回空，让调用方走默认兜底文案
                    return String::new();
                }
            }
        }
        s.to_string()
    }

    /// 冷却期移除用户名（避免称呼冷却失效）
    fn remove_name(text: &str, name: &str) -> String {
        if name.is_empty() || text.is_empty() {
            return text.to_string();
        }
        text.replace(name, "")
            .replace(",,", ",")
            .trim()
            .to_string()
    }
}

impl Default for ResponseParsingRunnable {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for ResponseParsingRunnable {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 命令或不应答：跳过解析
        if !state.should_respond || state.is_command {
            return Ok(state.to_json());
        }

        match &self.json_processor {
            Some(processor) => {
                let processed = processor.process_response(&state.response_text);
                Self::extract_from_processed(&mut state, &processed);
            }
            None => {
                tracing::debug!("[ResponseParsing] json_processor 未注入，使用兜底解析");
                // 无 processor 时直接使用 response_text，并兜底 motion
                state.text = state.response_text.trim().to_string();
                state.motion = "idle".to_string();
            }
        }

        // 兜底：没有 text 时尝试从原始 response_text 提取文本（尊重 no_reply 语义）
        // 不再注入兜底文案，避免污染对话历史与记忆：
        // - generation 阶段 API 错误已返回 Err，由前端 toast 提示
        // - LLM 真正返回空内容时保持 text 为空，由 chat:done 路径跳过空助手消息
        if state.text.is_empty() && state.intent != "no_reply" {
            // response_text 可能是未走 JSON 解析路径的原始 LLM 输出
            // 再做一次 JSON 提取尝试，避免原始 JSON 串进入下游记忆/展示
            let cleaned = Self::try_extract_text_from_raw(&state.response_text);
            if !cleaned.is_empty() {
                state.text = cleaned;
            }
        }

        // 冷却期称呼移除（避免称呼冷却失效）
        if state.in_cooldown {
            state.text = Self::remove_name(&state.text, &state.user_name);
        }

        // 工具失败容错：用角色化台词反馈（而非冷冰冰的错误信息）
        // 跨角色对话场景下不追加，避免向室友泄露工具调用细节
        let is_cross_character_input = state.user_input.starts_with('[')
            && state.user_input.contains(" says to me]");
        if !is_cross_character_input {
            if let Some(failures) = state.metadata.get("tool_failures").and_then(Value::as_array) {
                if !failures.is_empty() && !state.text.is_empty() {
                    let failed_tools: Vec<&str> = failures
                        .iter()
                        .filter_map(|f| f.get("tool").and_then(Value::as_str))
                        .collect();
                    if !failed_tools.is_empty() {
                        let tool_name = failed_tools.first().unwrap_or(&"");
                        let error_msg = failures
                            .first()
                            .and_then(|f| f.get("error").and_then(Value::as_str))
                            .unwrap_or("");
                        let note = crate::engine::feedback::tool_failure_to_character_text(tool_name, error_msg);
                        state.text.push_str(&note);
                    }
                }
            }
        }

        state.generation_status = "response_parsing_complete".to_string();

        // 同步 ai_response 字段以兼容下游（如 MoodStep / MemorySaving）
        if let Some(resp) = state.ai_response.as_mut() {
            resp.text = state.text.clone();
            resp.importance_user = state.importance_user;
            resp.importance_ai = state.importance_ai;
            resp.response_mode = state.response_mode.clone();
            resp.voice_message = state.voice_message;
        }

        Ok(state.to_json())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_object() {
        let text = r#"{"text":"你好","motion":"idle"}"#;
        let val = AIResponseGenerationRunnable::extract_json(text);
        assert!(val.is_some());
        let val = val.unwrap();
        assert_eq!(val["text"], "你好");
    }

    #[test]
    fn test_extract_json_array() {
        let text = r#"[{"text":"你好"},{"text":"世界"}]"#;
        let val = AIResponseGenerationRunnable::extract_json(text);
        assert!(val.is_some());
        // 返回第一个元素
        let val = val.unwrap();
        assert!(val.is_array() || val.is_object());
    }

    #[test]
    fn test_extract_json_none() {
        let val = AIResponseGenerationRunnable::extract_json("普通文本");
        assert!(val.is_none());
    }

    #[test]
    fn test_extract_tool_calls_from_array() {
        let parsed = json!([
            {"tool": "search", "arguments": {"q": "test"}},
            {"text": "结果"}
        ]);
        let calls = AIResponseGenerationRunnable::extract_tool_calls(&parsed);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["tool"], "search");
    }

    #[test]
    fn test_extract_tool_calls_from_tool_calls_field() {
        let parsed = json!({
            "text": "调用工具",
            "tool_calls": [
                {"tool": "calc", "arguments": {"x": 1}},
                {"tool": "search", "arguments": {"q": "test"}}
            ]
        });
        let calls = AIResponseGenerationRunnable::extract_tool_calls(&parsed);
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn test_extract_tool_calls_top_level() {
        let parsed = json!({"tool": "calc", "arguments": {"x": 1}});
        let calls = AIResponseGenerationRunnable::extract_tool_calls(&parsed);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn test_extract_tool_calls_empty() {
        let parsed = json!({"text": "纯文本回复"});
        let calls = AIResponseGenerationRunnable::extract_tool_calls(&parsed);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_try_extract_text_from_raw_json() {
        let raw = r#"{"text":"晚安","motion":"idle"}"#;
        assert_eq!(
            ResponseParsingRunnable::try_extract_text_from_raw(raw),
            "晚安"
        );
    }

    #[test]
    fn test_try_extract_text_from_raw_reply_field() {
        let raw = r#"{"reply":"你好"}"#;
        assert_eq!(
            ResponseParsingRunnable::try_extract_text_from_raw(raw),
            "你好"
        );
    }

    #[test]
    fn test_try_extract_text_from_raw_plain() {
        let raw = "纯文本回复";
        assert_eq!(
            ResponseParsingRunnable::try_extract_text_from_raw(raw),
            "纯文本回复"
        );
    }

    #[test]
    fn test_try_extract_text_from_raw_empty() {
        assert_eq!(ResponseParsingRunnable::try_extract_text_from_raw(""), "");
    }

    #[test]
    fn test_try_extract_text_from_raw_empty_text_field() {
        // text 字段为空时返回空（让调用方走兜底文案）
        let raw = r#"{"text":"","motion":"idle"}"#;
        assert_eq!(
            ResponseParsingRunnable::try_extract_text_from_raw(raw),
            ""
        );
    }

    #[test]
    fn test_remove_name_basic() {
        let text = "Master，你好呀";
        assert_eq!(
            ResponseParsingRunnable::remove_name(text, "Master"),
            "，你好呀"
        );
    }

    #[test]
    fn test_remove_name_empty_name() {
        let text = "你好呀";
        assert_eq!(ResponseParsingRunnable::remove_name(text, ""), "你好呀");
    }

    #[test]
    fn test_extract_from_processed_no_reply() {
        let mut state = PipelineState::default();
        state.text = "默认文本".to_string();
        let processed = ProcessedResponse {
            text: "本应被清空".to_string(),
            intent: "no_reply".to_string(),
            response_mode: "speak".to_string(),
            voice_message: false,
            tool_calls: Vec::new(),
        };
        ResponseParsingRunnable::extract_from_processed(&mut state, &processed);
        assert_eq!(state.intent, "no_reply");
        assert!(state.text.is_empty());
    }

    #[test]
    fn test_extract_from_processed_short_reply() {
        let mut state = PipelineState::default();
        let processed = ProcessedResponse {
            text: "嗯嗯".to_string(),
            intent: "short_reply".to_string(),
            response_mode: "speak".to_string(),
            voice_message: false,
            tool_calls: Vec::new(),
        };
        ResponseParsingRunnable::extract_from_processed(&mut state, &processed);
        assert_eq!(state.intent, "short_reply");
        assert_eq!(state.text, "嗯嗯");
    }

    #[test]
    fn test_extract_from_processed_invalid_intent_ignored() {
        let mut state = PipelineState::default();
        state.intent = "reply".to_string();
        let processed = ProcessedResponse {
            text: "你好".to_string(),
            intent: "unknown_intent".to_string(),
            response_mode: "speak".to_string(),
            voice_message: false,
            tool_calls: Vec::new(),
        };
        ResponseParsingRunnable::extract_from_processed(&mut state, &processed);
        // 非法 intent 不覆盖
        assert_eq!(state.intent, "reply");
        assert_eq!(state.text, "你好");
    }

    #[tokio::test]
    async fn test_response_parsing_skips_command() {
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.is_command = true;
        state.response_text = "命令响应".to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        // 命令跳过，text 应保持为空
        assert!(new_state.text.is_empty());
    }

    #[tokio::test]
    async fn test_response_parsing_skips_no_respond() {
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.should_respond = false;
        state.response_text = "不应答".to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        assert!(new_state.text.is_empty());
    }

    #[tokio::test]
    async fn test_response_parsing_plain_text() {
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.should_respond = true;
        state.response_text = "你好呀，今天天气不错".to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        assert_eq!(new_state.text, "你好呀，今天天气不错");
        assert_eq!(new_state.motion, "idle");
        assert_eq!(new_state.generation_status, "response_parsing_complete");
    }

    #[tokio::test]
    async fn test_response_parsing_json_response() {
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.should_respond = true;
        state.response_text = r#"{"text":"晚安","motion":"sleep","expression":"peaceful","importance_user":0.8,"importance_ai":0.6,"long_term_memory":"用户晚上10点睡觉","intent":"short_reply"}"#.to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        assert_eq!(new_state.text, "晚安");
        assert_eq!(new_state.motion, "sleep");
        assert_eq!(new_state.expression, "peaceful");
        assert!((new_state.importance_user - 0.8).abs() < 1e-6);
        assert!((new_state.importance_ai - 0.6).abs() < 1e-6);
        assert_eq!(new_state.long_term_memory, "用户晚上10点睡觉");
        assert_eq!(new_state.intent, "short_reply");
    }

    #[tokio::test]
    async fn test_response_parsing_no_reply_intent() {
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.should_respond = true;
        state.response_text =
            r#"{"text":"本应被清空","intent":"no_reply"}"#.to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        assert_eq!(new_state.intent, "no_reply");
        // no_reply 时 text 应被置空
        assert!(new_state.text.is_empty());
    }

    #[tokio::test]
    async fn test_response_parsing_cooldown_removes_name() {
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.should_respond = true;
        state.response_text = "Master，你好呀".to_string();
        state.in_cooldown = true;
        state.user_name = "Master".to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        assert!(!new_state.text.contains("Master"));
    }

    #[tokio::test]
    async fn test_response_parsing_empty_response_keeps_empty() {
        // 空响应不再注入兜底文案，避免污染对话历史与记忆
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.should_respond = true;
        state.response_text = "".to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        // 空响应保持为空（不注入兜底文案）
        assert!(new_state.text.is_empty());
        assert_eq!(new_state.motion, "idle");
    }

    #[tokio::test]
    async fn test_response_parsing_error_status_keeps_empty() {
        // generation 阶段 API 错误已返回 Err 不会进入解析；
        // 此处仅验证即便人为构造 error 状态，解析也不会注入兜底文案
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.should_respond = true;
        state.response_text = "".to_string();
        state.generation_status = "error: something went wrong".to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        assert!(new_state.text.is_empty());
    }

    #[tokio::test]
    async fn test_response_parsing_raw_json_string_falls_back_to_text() {
        let runnable = ResponseParsingRunnable::new();
        let mut state = PipelineState::default();
        state.should_respond = true;
        // 响应文本本身是 JSON 字符串但 JSONProcessor 未提取出 text（理论上不会发生，
        // 但兜底逻辑应能从原始 JSON 提取 text 字段）
        state.response_text = r#"{"text":"原始JSON中的文本"}"#.to_string();
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        // JsonProcessor 应该能提取出 text 字段
        assert_eq!(new_state.text, "原始JSON中的文本");
    }

    #[tokio::test]
    async fn test_ai_generation_skips_command() {
        // 没有 router 注入的情况下，命令应直接跳过
        let runnable = AIResponseGenerationRunnable::empty();
        let mut state = PipelineState::default();
        state.is_command = true;
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        // 命令跳过，response_text 应保持为空
        assert!(new_state.response_text.is_empty());
    }

    #[tokio::test]
    async fn test_ai_generation_skips_no_respond() {
        let runnable = AIResponseGenerationRunnable::empty();
        let mut state = PipelineState::default();
        state.should_respond = false;
        let result = runnable.ainvoke(state.to_json(), None).await.unwrap();
        let new_state = PipelineState::from_json(result);
        assert!(new_state.response_text.is_empty());
    }

    #[tokio::test]
    async fn test_ai_generation_no_router_returns_err() {
        // router 未注入：返回 Err，由 chat 命令 emit `chat:error`，前端 toast 提示
        let runnable = AIResponseGenerationRunnable::empty();
        let mut state = PipelineState::default();
        state.should_respond = true;
        state.user_input = "你好".to_string();
        let result = runnable.ainvoke(state.to_json(), None).await;
        assert!(result.is_err());
    }
}
