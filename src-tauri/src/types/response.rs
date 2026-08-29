use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiTaskType {
    SimpleChat,
    ComplexQuestion,
    Command,
    SystemControl,
    EmotionAnalysis,
    EnvironmentPerception,
}

impl Default for AiTaskType {
    fn default() -> Self {
        Self::SimpleChat
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    pub text: String,
    #[serde(default = "default_motion")]
    pub motion: String,
    #[serde(default = "default_expression")]
    pub expression: String,
    /// 表情持续时间（毫秒）
    ///
    /// 由 ExpressionMotionRunnable 调 LLM 决定，0 表示自然切换（前端不主动 reset）。
    /// 替代旧 SetExpressionTool 工具的 duration 参数。
    #[serde(default)]
    pub expression_duration_ms: u64,
    #[serde(default)]
    pub emotion_score: f64,
    #[serde(default)]
    pub execution_result: Option<String>,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub importance_user: f64,
    #[serde(default)]
    pub importance_ai: f64,
    /// 聊天表情包名称（空字符串表示不使用）
    #[serde(default)]
    pub sticker: String,
    /// LLM 在 JSON 中判定的当前用户情绪（如 happy/sad/angry/neutral）。
    /// 由 ResponseParsingRunnable 从 LLM 返回的 JSON 中提取；
    /// 纯文本兜底路径下为空串。供前端作为 proactive tick 的真实 user_emotion 来源。
    #[serde(default)]
    pub user_emotion: String,
    /// 响应模式（仅跨角色对话生效，主对话默认 speak）
    ///
    /// 由 LLM 在 JSON 中返回，决定本轮是否生成回复文本：
    /// - speak：正常回复（默认）
    /// - non_verbal：只做动作/表情
    /// - internal：只更新内部想法
    /// - ignore：完全忽略
    ///
    /// 主对话路径下永远为 speak（即使用户输入很无聊，也要回一个低信息量回复）。
    #[serde(default)]
    pub response_mode: String,
    /// 微信渠道语音消息标志：为 true 时前端不显示文本，而是合成 TTS 音频后
    /// 以微信风格语音气泡发出，点击可播放。
    #[serde(default)]
    pub voice_message: bool,
}

fn default_motion() -> String {
    "idle".to_string()
}

fn default_expression() -> String {
    "star_eyes".to_string()
}

fn default_source() -> String {
    String::new()
}

impl AiResponse {
    pub fn new(text: String) -> Self {
        Self {
            text,
            motion: default_motion(),
            expression: default_expression(),
            expression_duration_ms: 0,
            emotion_score: 0.0,
            execution_result: None,
            source: String::new(),
            importance_user: 0.0,
            importance_ai: 0.0,
            sticker: String::new(),
            user_emotion: String::new(),
            response_mode: "speak".to_string(),
            voice_message: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            motion: "idle".to_string(),
            expression: "angry".to_string(),
            expression_duration_ms: 0,
            emotion_score: 0.0,
            execution_result: None,
            source: String::new(),
            importance_user: 0.1,
            importance_ai: 0.1,
            sticker: String::new(),
            user_emotion: String::new(),
            response_mode: "speak".to_string(),
            voice_message: false,
        }
    }
}

/// 消息中的工具调用（assistant 消息携带，用于原生 function calling 多轮上下文）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageToolCall {
    /// 工具调用 ID（由 LLM 生成，关联后续 tool 结果消息）
    pub id: String,
    /// 工具名
    pub name: String,
    /// 工具参数（JSON 对象）
    pub arguments: serde_json::Value,
}

/// 多模态图片内容
///
/// 支持 base64 内联与 URL 两种来源。provider 层按各家协议翻译：
/// - OpenAI 兼容：content 数组中 `{"type":"image_url","image_url":{"url":"..."}}`
/// - Anthropic：content 数组中 `{"type":"image","source":{"type":"base64","media_type":"...","data":"..."}}`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageImage {
    /// 图片 MIME 类型，如 `image/png` / `image/jpeg`
    pub media_type: String,
    /// base64 编码的图片数据（不含 `data:` 前缀）
    pub data: String,
    /// 图片 URL（与 `data` 二选一；若两者都有，优先使用 `data`）
    pub url: Option<String>,
    /// 图片采样精度：auto / low / high（仅 OpenAI 兼容 API 使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<chrono::DateTime<chrono::Local>>,
    /// assistant 消息携带的工具调用列表（原生 function calling 路径使用）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<MessageToolCall>>,
    /// tool 角色消息携带的关联 ID（与 assistant.tool_calls[].id 对应）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    /// 推理/思维链内容（assistant 消息）
    ///
    /// 兼容三种字段名：
    /// - `reasoning_content`（DeepSeek-R1 / Qwen-QwQ / 智谱 GLM-Zero / 火山方舟）
    /// - `thinking`（Anthropic Claude extended thinking / Moonshot Kimi reasoning）
    /// - `reasoning_details`（部分 OpenAI o 系列兼容端点）
    ///
    /// 多轮工具调用时原样回传，保证上下文连续性。不入可见输出流。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<String>,
    /// user 消息附带的多模态图片列表
    ///
    /// 为空时按纯文本消息处理；非空时 provider 层把 content 转为数组形式，
    /// 在文本块之后追加 image 块。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<MessageImage>>,
    /// 消息元数据：标记内容来源与记忆策略。
    ///
    /// - `None` 或 `source=User/Assistant`：正常进入记忆系统
    /// - `source=Tool/InnerMonologue/Mirror` 或 `is_memory_disabled=true`：
    ///   AutoExtractor / UserFactStore 跳过，不抽取为用户事实
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub meta: Option<crate::messages::MessageMeta>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            timestamp: Some(chrono::Local::now()),
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
            images: None,
            meta: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            timestamp: Some(chrono::Local::now()),
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
            images: None,
            meta: None,
        }
    }

    /// 构造带图片的 user 消息
    pub fn user_with_images(
        content: impl Into<String>,
        images: Vec<MessageImage>,
    ) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            timestamp: Some(chrono::Local::now()),
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
            images: if images.is_empty() { None } else { Some(images) },
            meta: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            timestamp: Some(chrono::Local::now()),
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
            images: None,
            meta: None,
        }
    }

    /// 构造带工具调用的 assistant 消息（原生 function calling 路径使用）
    ///
    /// content 在 OpenAI 协议下可为空字符串；部分服务商要求 null，由 provider 自行处理。
    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<MessageToolCall>,
    ) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            timestamp: Some(chrono::Local::now()),
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
            reasoning: None,
            images: None,
            meta: None,
        }
    }

    /// 构造工具结果消息（role=tool，携带 tool_call_id 关联到发起调用的 assistant 消息）
    pub fn tool_result(
        content: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            timestamp: Some(chrono::Local::now()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            reasoning: None,
            images: None,
            // 工具结果默认不进入记忆
            meta: Some(crate::messages::MessageMeta::tool()),
        }
    }

    /// 设置消息元数据
    pub fn with_meta(mut self, meta: crate::messages::MessageMeta) -> Self {
        self.meta = Some(meta);
        self
    }

    /// 设置消息来源
    pub fn with_source(self, source: crate::messages::MessageSource) -> Self {
        self.with_meta(crate::messages::MessageMeta::new(source))
    }

    /// 该消息是否应被记忆系统跳过
    ///
    /// - `meta` 为 `None`：正常消息，不跳过
    /// - `meta.is_memory_disabled == true`：跳过
    /// - `meta.source` 不是 `User`/`Assistant`：跳过
    pub fn is_memory_disabled(&self) -> bool {
        match &self.meta {
            None => false,
            Some(m) => m.is_memory_disabled || !m.source.is_memory_eligible(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}
