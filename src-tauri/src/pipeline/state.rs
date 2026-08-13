use serde::{Deserialize, Serialize};

use crate::providers::base::ToolDefinition;
use crate::types::response::{AiResponse, ChatMessage};

// ── serde 默认值辅助函数 ──

fn default_user_emotion() -> String {
    "neutral".to_string()
}

fn default_should_respond() -> bool {
    true
}

fn default_full_response() -> bool {
    true
}

fn default_intent() -> String {
    "reply".to_string()
}

fn default_response_mode() -> String {
    "speak".to_string()
}

fn default_topic_activeness() -> i32 {
    10
}

fn default_motion() -> String {
    "idle".to_string()
}

fn default_importance_user() -> f64 {
    0.3
}

fn default_importance_ai() -> f64 {
    0.3
}

fn default_user_name() -> String {
    "Master".to_string()
}

fn default_generation_status() -> String {
    "pending".to_string()
}

fn default_json_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// 对话处理状态（40+ 字段）
///
/// 在原有 8 字段基础上扩展至 50+ 字段，完整覆盖：
/// 输入层 / 用户情绪 / 决策层 / 命令层 / Prompt 组装层 /
/// 生成层 / 输出层 / 记忆层 / 元数据 / 运行时。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineState {
    // ── 输入层 ──
    /// 用户输入
    #[serde(default)]
    pub user_input: String,
    /// 经查询重写后的自包含输入（由 QueryRewriteStep 写入，用于记忆检索）
    #[serde(default)]
    pub resolved_user_input: String,
    /// 检测到的用户情绪（14类）
    #[serde(default = "default_user_emotion")]
    pub user_emotion: String,
    /// 用户情绪强度
    #[serde(default)]
    pub user_emotion_intensity: f64,

    // ── 决策层 ──
    /// 是否应该响应
    #[serde(default = "default_should_respond")]
    pub should_respond: bool,
    /// 是否允许完整响应
    #[serde(default = "default_full_response")]
    pub full_response: bool,
    /// LLM 返回的意图标记：reply/short_reply/no_reply
    #[serde(default = "default_intent")]
    pub intent: String,
    /// 响应模式：speak/non_verbal/internal/ignore（仅跨角色对话生效，主对话默认 speak）
    ///
    /// 由 LLM 在 JSON 中返回，ResponseParsingRunnable 解析。
    /// cross_character.rs 读取此字段决定是否进入 Cooling、是否投递下一轮。
    #[serde(default = "default_response_mode")]
    pub response_mode: String,
    /// 微信渠道语音消息标志：为 true 时前端不显示文本，合成 TTS 后以语音气泡发出
    #[serde(default)]
    pub voice_message: bool,
    /// 话题活跃度
    #[serde(default = "default_topic_activeness")]
    pub topic_activeness: i32,
    /// 凝神模式是否激活（本轮）
    #[serde(default)]
    pub focus_active: bool,
    /// 凝神模式激活时的 max_tokens 额外余量
    #[serde(default)]
    pub focus_extra_tokens: u32,

    // ── 命令层 ──
    /// 命令类型
    #[serde(default)]
    pub command: Option<String>,
    /// 命令参数
    #[serde(default = "default_json_object")]
    pub command_args: serde_json::Value,
    /// 是否是命令
    #[serde(default)]
    pub is_command: bool,
    /// 命令响应
    #[serde(default)]
    pub command_response: Option<serde_json::Value>,

    // ── Prompt 组装层 ──
    /// 系统提示词
    #[serde(default)]
    pub system_prompt: String,
    /// 系统提示词扩展（如心情注入）
    #[serde(default)]
    pub system_prompt_extension: String,
    /// 当前消息渠道（"wechat" 聊天面板 / "direct" 直接说话），影响 LLM 回复风格
    #[serde(default)]
    pub current_channel: String,
    /// 当前会话 ID（来自 ConversationManager）
    ///
    /// User↔Agent 与 Agent↔Agent 通用。LLM 不直接消费此字段，
    /// 但 dialogue 写入历史时会把它写入 HistoryEntry.session_id，
    /// 实现对话历史按会话切分。
    #[serde(default)]
    pub conversation_id: String,
    /// 当前会话状态（"created"/"active"/"cooling"/"closed"）
    ///
    /// 由 commands/chat.rs 或 cross_character.rs 在调用 brain.think 前注入。
    /// PromptBuildingStep 可据此调整提示词（如 Cooling 时提示 LLM 对话正在冷却）。
    #[serde(default)]
    pub conv_state: String,
    /// 上一轮会话关闭原因（"good_night"/"good_bye"/"no_response"/...）
    ///
    /// 仅在新会话首轮注入，让 LLM 知道上次是怎么结束的（如 GoodNight → 新会话开场可问候"早上好"）。
    #[serde(default)]
    pub last_close_reason: String,
    /// 当前在场状态（"online"/"busy"/"rest"/"offline"），影响 LLM 回复 + 告知可用 set_presence_state 工具
    #[serde(default)]
    pub presence_state: String,
    /// SelfState 快照文本（由 Brain 在 think 前注入，PromptBuildingStep 读取）
    ///
    /// 包含角色当前心理状态/在场状态/当前活动/今日主动次数/被忽略次数/疲劳/社交满足度等。
    /// 让 LLM 感知"我现在正在做什么、我今天的节奏如何"，避免行为失控和重复主动。
    #[serde(default)]
    pub self_state_text: String,
    /// 记忆文本
    #[serde(default)]
    pub memory_text: String,
    /// 上下文文本（环境信息）
    #[serde(default)]
    pub context_text: String,
    /// Web 检索上下文（WebContextRunnable 注入）
    #[serde(default)]
    pub web_context: String,
    /// 用户认知模型文本（UserModelManager 注入，PromptBuildingStep 读取）
    #[serde(default)]
    pub user_model_text: String,
    /// 工具描述文本
    #[serde(default)]
    pub tools_text: String,
    /// 结构化工具定义（原生 function calling 路径使用）
    ///
    /// 由 PromptBuildingStep 与 `tools_text` 同步填充（同一份工具子集，两种表示）。
    /// 当 provider 支持原生 fc 且 config 开关启用时，generation 层会读取此字段
    /// 调用 `router.generate_with_tools`，跳过 prompt 注入和 JSON 解析。
    /// 文本路径（fallback）只读 `tools_text`，忽略此字段。
    ///
    /// 注意：不能 `skip`，否则跨 step 序列化时会丢失，导致 generation 层拿不到工具定义
    /// 而误判 `use_native_fc=false`，回退到依赖 LLM 自觉输出 JSON 的文本路径。
    #[serde(default)]
    pub tool_definitions: Vec<ToolDefinition>,
    /// Native FC 回退到文本路径时使用的工具文本（含格式说明）
    ///
    /// 当 `enable_native_fc=true` 时，`build_tools_block()` 返回空字符串（工具通过 API tools 参数传递）。
    /// 如果原生 FC 路径失败回退到文本路径，prompt 中工具区段为空。此字段预存一份文本版工具块，
    /// 供 generation 层在回退时作为额外 system message 注入 messages_vec。
    #[serde(default)]
    pub tools_text_fallback: Option<String>,
    /// Native FC / JSON Schema 回退时使用的输出格式指令
    ///
    /// 与 `tools_text_fallback` 同理：当 `enable_native_fc` 或 `has_native_schema` 启用时，
    /// prompt 中跳过 output_format 文本。回退时需要补回格式指令。
    #[serde(default)]
    pub output_format_fallback: Option<String>,

    // ── 生成层 ──
    /// AI 原始响应文本
    #[serde(default)]
    pub response_text: String,
    /// 解析后的 JSON
    #[serde(default)]
    pub response_json: Option<serde_json::Value>,
    /// JSONProcessor 解析结果
    #[serde(default)]
    pub parsed_json: Option<serde_json::Value>,
    /// 工具调用列表
    #[serde(default)]
    pub tool_calls: Vec<serde_json::Value>,
    /// 工具调用是否已执行
    #[serde(default)]
    pub tool_call_executed: bool,

    // ── 输出层 ──
    /// 最终响应文本
    #[serde(default)]
    pub text: String,
    /// 即时回复文本
    #[serde(default)]
    pub immediate_response_text: String,
    /// 动作
    #[serde(default = "default_motion")]
    pub motion: String,
    /// 表情
    #[serde(default)]
    pub expression: String,
    /// 表情持续时间（毫秒）
    ///
    /// 由 ExpressionMotionRunnable 在 text 生成后调 LLM 决定：
    /// - 0 = 自然切换（前端兜底默认值，不主动 reset）
    /// - >0 = 在指定毫秒后自动结束表情，回到默认
    ///
    /// 替代旧 SetExpressionTool 工具的 duration 参数，统一由独立表情选择步骤产出。
    #[serde(default)]
    pub expression_duration_ms: u64,
    /// 用户侧重要性
    #[serde(default = "default_importance_user")]
    pub importance_user: f64,
    /// AI 侧重要性
    #[serde(default = "default_importance_ai")]
    pub importance_ai: f64,
    /// LLM 生成的长期记忆
    #[serde(default)]
    pub long_term_memory: String,
    /// 桌宠自控动作指令（chat 任务产出 Live2D 模型控制，由 ControlActionExecutor 执行）
    #[serde(default)]
    pub control_actions: Vec<serde_json::Value>,
    /// 聊天表情包名称（可选，空字符串表示不使用）
    #[serde(default)]
    pub sticker: String,

    // ── 记忆层 ──
    /// 记忆是否已保存
    #[serde(default)]
    pub memory_saved: bool,
    /// 时间戳记忆系统
    #[serde(default)]
    pub time_stamped_memory: Option<serde_json::Value>,
    /// 记忆变量
    #[serde(default = "default_json_object")]
    pub memory_vars: serde_json::Value,
    /// 记忆资料
    #[serde(default = "default_json_object")]
    pub memory_profile: serde_json::Value,
    /// fast 检索返回的原始 Memory 列表（供后台差异判定）
    #[serde(default)]
    pub raw_semantic_memory: Vec<serde_json::Value>,
    /// 主动召回的记忆提示
    #[serde(default)]
    pub proactive_recall_text: String,

    // ── 对话元数据 ──
    /// 是否在称呼冷却期
    #[serde(default)]
    pub in_cooldown: bool,
    /// 用户名
    #[serde(default = "default_user_name")]
    pub user_name: String,
    /// 是否自然结束对话
    #[serde(default)]
    pub graceful_exit: bool,
    /// 结束原因
    #[serde(default)]
    pub exit_reason: String,

    // ── 运行时 ──
    /// 生成状态
    #[serde(default = "default_generation_status")]
    pub generation_status: String,
    /// 情感
    #[serde(default)]
    pub emotion: Option<String>,
    /// 处理耗时（毫秒）
    #[serde(default)]
    pub duration_ms: f64,
    /// 错误信息
    #[serde(default)]
    pub error: Option<String>,

    // ── 心理架构层（PsychologyManager 消费）──
    /// LLM 产出的认知评估（6 项）
    #[serde(default)]
    pub appraisal: Option<crate::psychology::Appraisal>,
    /// LLM 产出的情绪增量（8 项）
    #[serde(default)]
    pub emotion_update: Option<crate::psychology::EmotionDeltas>,
    /// LLM 产出的行为驱动（8 项）
    #[serde(default)]
    pub behavior_drive: Option<crate::psychology::BehaviorDrive>,
    /// LLM 自判的事件摘要（非空时写入记忆系统 ImportantEvent）
    #[serde(default)]
    pub event_summary: String,

    /// 快速语义感知结果（在流水线执行前由 FastSemanticAnalyzer 填充）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_perception: Option<crate::emotion::FastPerceptionResult>,

    /// 认知知识需求评估（在 FastSemantic 阶段同步计算）
    ///
    /// 多维评估用户输入是否需要外部知识验证，驱动 WebContext 预搜索和 Prompt 认知信号注入。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epistemic_assessment: Option<crate::emotion::EpistemicAssessment>,

    // ── 原有 Rust 字段（保留以兼容现有步骤）──
    /// 对话消息列表
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    /// 检索到的记忆内容列表
    #[serde(default)]
    pub memories: Vec<String>,
    /// 组装后的完整 prompt
    #[serde(default)]
    pub prompt: String,
    /// AI 响应
    #[serde(default)]
    pub ai_response: Option<AiResponse>,
    /// 已使用的工具列表
    #[serde(default)]
    pub tools_used: Vec<String>,
    /// 运行时元数据（JSON 对象）
    #[serde(default = "default_json_object")]
    pub metadata: serde_json::Value,
}

impl Default for PipelineState {
    fn default() -> Self {
        Self {
            // 输入层
            user_input: String::new(),
            resolved_user_input: String::new(),
            user_emotion: default_user_emotion(),
            user_emotion_intensity: 0.0,
            // 决策层
            should_respond: default_should_respond(),
            full_response: default_full_response(),
            intent: default_intent(),
            response_mode: default_response_mode(),
            voice_message: false,
            topic_activeness: default_topic_activeness(),
            focus_active: false,
            focus_extra_tokens: 0,
            // 命令层
            command: None,
            command_args: default_json_object(),
            is_command: false,
            command_response: None,
            // Prompt 组装层
            system_prompt: String::new(),
            system_prompt_extension: String::new(),
            current_channel: String::new(),
            conversation_id: String::new(),
            conv_state: String::new(),
            last_close_reason: String::new(),
            presence_state: String::new(),
            self_state_text: String::new(),
            memory_text: String::new(),
            context_text: String::new(),
            web_context: String::new(),
            user_model_text: String::new(),
            tools_text: String::new(),
            tool_definitions: Vec::new(),
            tools_text_fallback: None,
            output_format_fallback: None,
            // 生成层
            response_text: String::new(),
            response_json: None,
            parsed_json: None,
            tool_calls: Vec::new(),
            tool_call_executed: false,
            // 输出层
            text: String::new(),
            immediate_response_text: String::new(),
            motion: default_motion(),
            expression: String::new(),
            expression_duration_ms: 0,
            importance_user: default_importance_user(),
            importance_ai: default_importance_ai(),
            long_term_memory: String::new(),
            control_actions: Vec::new(),
            sticker: String::new(),
            // 记忆层
            memory_saved: false,
            time_stamped_memory: None,
            memory_vars: default_json_object(),
            memory_profile: default_json_object(),
            raw_semantic_memory: Vec::new(),
            proactive_recall_text: String::new(),
            // 元数据
            in_cooldown: false,
            user_name: default_user_name(),
            graceful_exit: false,
            exit_reason: String::new(),
            // 运行时
            generation_status: default_generation_status(),
            emotion: None,
            duration_ms: 0.0,
            error: None,
            // 心理架构层
            appraisal: None,
            emotion_update: None,
            behavior_drive: None,
            event_summary: String::new(),
            fast_perception: None,
            epistemic_assessment: None,
            // 原有 Rust 字段
            messages: Vec::new(),
            memories: Vec::new(),
            prompt: String::new(),
            ai_response: None,
            tools_used: Vec::new(),
            metadata: default_json_object(),
        }
    }
}

impl PipelineState {
    /// 创建新的 PipelineState，设置用户输入
    pub fn new(user_input: String) -> Self {
        let mut state = Self::default();
        state.user_input = user_input;
        state
    }

    /// 序列化为 JSON
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// 从 JSON 反序列化
    pub fn from_json(value: serde_json::Value) -> Self {
        serde_json::from_value(value).unwrap_or_default()
    }

    /// 统一处理 Runnable 执行错误
    ///
    /// 记录错误信息并将 generation_status 标记为对应的失败状态。
    pub fn handle_runnable_error(
        &mut self,
        error: &crate::pipeline::errors::PipelineError,
    ) {
        tracing::error!(error = %error, "Runnable 执行失败");
        self.error = Some(error.to_string());
        self.generation_status = match error {
            crate::pipeline::errors::PipelineError::StageTimeout => {
                "stage_timeout_failed".to_string()
            }
            crate::pipeline::errors::PipelineError::StageExecution(_) => {
                "stage_execution_failed".to_string()
            }
            crate::pipeline::errors::PipelineError::Recoverable(_) => {
                "recoverable_failed".to_string()
            }
        };
    }

    /// 克隆当前状态用于并行关键路径
    pub fn clone_for_parallel(&self) -> Self {
        self.clone()
    }

    /// 将并行分支的结果合并回当前状态
    ///
    /// 合并策略：取 `other` 中非默认（已设置）的字段值；
    /// metadata 执行 JSON 对象合并（逐键覆盖）。
    pub fn merge_parallel_result(&mut self, other: Self) {
        // 输入层
        if !other.user_input.is_empty() {
            self.user_input = other.user_input;
        }
        if !other.resolved_user_input.is_empty() {
            self.resolved_user_input = other.resolved_user_input;
        }
        if other.user_emotion != default_user_emotion() {
            self.user_emotion = other.user_emotion;
        }
        if other.user_emotion_intensity != 0.0 {
            self.user_emotion_intensity = other.user_emotion_intensity;
        }

        // 决策层
        if !other.should_respond {
            self.should_respond = other.should_respond;
        }
        if !other.full_response {
            self.full_response = other.full_response;
        }
        if other.intent != default_intent() {
            self.intent = other.intent;
        }
        if other.topic_activeness != default_topic_activeness() {
            self.topic_activeness = other.topic_activeness;
        }

        // 命令层
        if other.command.is_some() {
            self.command = other.command;
        }
        if let serde_json::Value::Object(ref map) = other.command_args {
            if !map.is_empty() {
                self.command_args = other.command_args;
            }
        }
        if other.is_command {
            self.is_command = other.is_command;
        }
        if other.command_response.is_some() {
            self.command_response = other.command_response;
        }

        // Prompt 组装层
        if !other.system_prompt.is_empty() {
            self.system_prompt = other.system_prompt;
        }
        if !other.system_prompt_extension.is_empty() {
            self.system_prompt_extension = other.system_prompt_extension;
        }
        if !other.current_channel.is_empty() {
            self.current_channel = other.current_channel;
        }
        if !other.memory_text.is_empty() {
            self.memory_text = other.memory_text;
        }
        if !other.context_text.is_empty() {
            self.context_text = other.context_text;
        }
        if !other.web_context.is_empty() {
            self.web_context = other.web_context;
        }
        if !other.tools_text.is_empty() {
            self.tools_text = other.tools_text;
        }
        if !other.tool_definitions.is_empty() {
            self.tool_definitions = other.tool_definitions;
        }
        if other.tools_text_fallback.is_some() {
            self.tools_text_fallback = other.tools_text_fallback;
        }
        if other.output_format_fallback.is_some() {
            self.output_format_fallback = other.output_format_fallback;
        }

        // 生成层
        if !other.response_text.is_empty() {
            self.response_text = other.response_text;
        }
        if other.response_json.is_some() {
            self.response_json = other.response_json;
        }
        if other.parsed_json.is_some() {
            self.parsed_json = other.parsed_json;
        }
        if !other.tool_calls.is_empty() {
            self.tool_calls = other.tool_calls;
        }
        if other.tool_call_executed {
            self.tool_call_executed = other.tool_call_executed;
        }

        // 输出层
        if !other.text.is_empty() {
            self.text = other.text;
        }
        if !other.immediate_response_text.is_empty() {
            self.immediate_response_text = other.immediate_response_text;
        }
        if other.motion != default_motion() {
            self.motion = other.motion;
        }
        if !other.expression.is_empty() {
            self.expression = other.expression;
        }
        if other.importance_user != default_importance_user() {
            self.importance_user = other.importance_user;
        }
        if other.importance_ai != default_importance_ai() {
            self.importance_ai = other.importance_ai;
        }
        if !other.long_term_memory.is_empty() {
            self.long_term_memory = other.long_term_memory;
        }

        // 记忆层
        if other.memory_saved {
            self.memory_saved = other.memory_saved;
        }
        if other.time_stamped_memory.is_some() {
            self.time_stamped_memory = other.time_stamped_memory;
        }
        if let serde_json::Value::Object(ref map) = other.memory_vars {
            if !map.is_empty() {
                self.memory_vars = other.memory_vars;
            }
        }
        if let serde_json::Value::Object(ref map) = other.memory_profile {
            if !map.is_empty() {
                self.memory_profile = other.memory_profile;
            }
        }
        if !other.raw_semantic_memory.is_empty() {
            self.raw_semantic_memory = other.raw_semantic_memory;
        }
        if !other.proactive_recall_text.is_empty() {
            self.proactive_recall_text = other.proactive_recall_text;
        }

        // 元数据
        if other.in_cooldown {
            self.in_cooldown = other.in_cooldown;
        }
        if other.user_name != default_user_name() {
            self.user_name = other.user_name;
        }
        if other.graceful_exit {
            self.graceful_exit = other.graceful_exit;
        }
        if !other.exit_reason.is_empty() {
            self.exit_reason = other.exit_reason;
        }

        // 运行时
        if other.generation_status != default_generation_status() {
            self.generation_status = other.generation_status;
        }
        if other.emotion.is_some() {
            self.emotion = other.emotion;
        }
        if other.duration_ms != 0.0 {
            self.duration_ms = other.duration_ms;
        }
        if other.error.is_some() {
            self.error = other.error;
        }

        // 原有 Rust 字段
        if other.fast_perception.is_some() {
            self.fast_perception = other.fast_perception;
        }
        if !other.messages.is_empty() {
            self.messages = other.messages;
        }
        if !other.memories.is_empty() {
            self.memories = other.memories;
        }
        if !other.prompt.is_empty() {
            self.prompt = other.prompt;
        }
        if other.ai_response.is_some() {
            self.ai_response = other.ai_response;
        }
        if !other.tools_used.is_empty() {
            self.tools_used = other.tools_used;
        }

        // metadata 执行 JSON 对象合并（逐键覆盖）
        merge_json_objects(&mut self.metadata, &other.metadata);
    }
}

/// 合并两个 JSON 对象（将 src 的键合并到 dst，已存在则覆盖）
fn merge_json_objects(dst: &mut serde_json::Value, src: &serde_json::Value) {
    if let (serde_json::Value::Object(dst_map), serde_json::Value::Object(src_map)) = (dst, src)
    {
        for (key, value) in src_map {
            dst_map.insert(key.clone(), value.clone());
        }
    }
}
